//! GEDZIP file format support for GEDCOM 7.0.
//!
//! This module provides functionality to read and write GEDZIP files, which are
//! ZIP archives containing a GEDCOM dataset along with associated media files.
//!
//! # GEDZIP Format
//!
//! A GEDZIP file is a ZIP archive (ISO/IEC 21320-1:2015) containing:
//! - `gedcom.ged` - The main GEDCOM 7.x data stream
//! - Media files referenced by FILE structures in the GEDCOM
//!
//! GEDZIP files should use the `.gdz` file extension.
//!
//! Entry names are UTF-8 and not percent-escaped, while a `FILE` payload
//! is a URL string that percent-escapes what it cannot hold (§4, 7.0.17):
//! a reference finds its entry once decoded
//! ([`GedzipReader::find_entry`]), and a media file added by its `FILE`
//! payload is stored under the decoded name
//! ([`GedzipWriter::add_media_file`]). Zip can encrypt entry contents (not
//! their names and sizes); this crate reads and writes no encrypted entry:
//! reading one fails with [`GedzipError::Zip`].
//!
//! An archive's dataset is GEDCOM 7.x: [`GedzipWriter::write_dataset`]
//! writes a 5.5.1 dataset as 7.0, converting it ([`crate::convert`]), in
//! UTF-8 with a byte order mark. Media names follow the GEDZIP rules: the
//! names a reader uses for itself (`gedcom.ged`, `META-INF/…`,
//! `MANIFEST.MF`) are refused, and the names of directories are not media.
//!
//! # Example
//!
//! ```rust,no_run
//! # #[cfg(feature = "gedzip")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use oxidged::gedzip::{GedzipReader, GedzipWriter};
//! use oxidged::model::Dataset;
//! use oxidged::GedcomBuilder;
//! use std::fs::File;
//!
//! // Read a GEDZIP file
//! let file = File::open("family.gdz")?;
//! let mut reader = GedzipReader::new(file)?;
//! let data = reader.read_dataset(&GedcomBuilder::new())?;
//! println!("Found {} individuals", data.individuals().len());
//!
//! // List media files in the archive
//! for name in reader.media_files() {
//!     println!("Media file: {}", name);
//! }
//!
//! // Write a GEDZIP file
//! let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n");
//! let output = File::create("output.gdz")?;
//! let mut writer = GedzipWriter::new(output)?;
//! writer.write_dataset(&data)?;
//! writer.finish()?;
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "gedzip"))]
//! # fn main() {}
//! ```

use std::collections::HashMap;
use std::io::{Read, Seek, Write};

use zip::CompressionMethod;
use zip::read::ZipArchive;
use zip::write::ZipWriter;

use crate::model::Dataset;
use crate::writer::{GedcomWriter, WriteError};
use crate::{GedcomBuilder, GedcomError, GedcomVersion};

/// The required filename for the GEDCOM data stream within a GEDZIP archive.
pub const GEDCOM_FILENAME: &str = "gedcom.ged";

/// Why a GEDZIP archive cannot be read or written.
#[non_exhaustive]
#[derive(Debug)]
pub enum GedzipError {
    /// The ZIP archive cannot be read or written.
    Zip(zip::result::ZipError),
    /// The archive has no `gedcom.ged`.
    MissingGedcom,
    /// The archive has no entry of this name.
    MissingMedia(String),
    /// An entry is larger than the limit set with
    /// [`GedzipReader::max_entry_size`].
    EntryTooLarge {
        /// The entry's name in the archive.
        name: String,
        /// The limit, in bytes.
        limit: u64,
    },
    /// Reading or writing failed.
    Io(std::io::Error),
    /// Writing the dataset failed.
    Write(WriteError),
    /// A media file cannot take this name: `gedcom.ged`, a name under
    /// `META-INF/` or `MANIFEST.MF`, which GEDZIP reserves (§2.12, §4), or
    /// the name of a directory.
    ReservedName(String),
}

impl std::fmt::Display for GedzipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zip(e) => write!(f, "ZIP error: {e}"),
            Self::MissingGedcom => write!(f, "the archive has no {GEDCOM_FILENAME}"),
            Self::MissingMedia(name) => write!(f, "the archive has no entry {name}"),
            Self::EntryTooLarge { name, limit } => {
                write!(f, "the archive entry {name} exceeds the {limit}-byte limit")
            }
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Write(e) => write!(f, "{e}"),
            Self::ReservedName(name) => write!(f, "a media file cannot be named {name}"),
        }
    }
}

impl std::error::Error for GedzipError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Zip(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Write(e) => Some(e),
            Self::MissingGedcom
            | Self::MissingMedia(_)
            | Self::EntryTooLarge { .. }
            | Self::ReservedName(_) => None,
        }
    }
}

impl From<zip::result::ZipError> for GedzipError {
    fn from(err: zip::result::ZipError) -> Self {
        Self::Zip(err)
    }
}

impl From<std::io::Error> for GedzipError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<WriteError> for GedzipError {
    fn from(err: WriteError) -> Self {
        Self::Write(err)
    }
}

/// A reader for GEDZIP files.
///
/// `GedzipReader` wraps a ZIP archive and provides methods to:
/// - Parse the GEDCOM data from `gedcom.ged`
/// - List and read media files included in the archive
///
/// # Example
///
/// ```rust,no_run
/// # #[cfg(feature = "gedzip")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxidged::gedzip::GedzipReader;
/// use std::fs::File;
///
/// let file = File::open("family.gdz")?;
/// let mut reader = GedzipReader::new(file)?;
///
/// // Read the dataset
/// let data = reader.read_dataset(&oxidged::GedcomBuilder::new())?;
/// println!("Individuals: {}", data.individuals().len());
///
/// // Read a specific media file
/// if let Ok(bytes) = reader.read_media_file("photos/grandpa.jpg") {
///     println!("Read {} bytes", bytes.len());
/// }
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "gedzip"))]
/// # fn main() {}
/// ```
pub struct GedzipReader<R: Read + Seek> {
    archive: ZipArchive<R>,
    file_names: Vec<String>,
    max_entry_size: Option<u64>,
}

impl<R: Read + Seek> GedzipReader<R> {
    /// Creates a new `GedzipReader` from a readable and seekable source.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The source is not a valid ZIP archive
    /// - The archive does not contain a `gedcom.ged` file
    pub fn new(reader: R) -> Result<Self, GedzipError> {
        let archive = ZipArchive::new(reader)?;

        // Verify gedcom.ged exists
        let has_gedcom = archive.file_names().any(|name| name == GEDCOM_FILENAME);
        if !has_gedcom {
            return Err(GedzipError::MissingGedcom);
        }

        // Collect file names
        let file_names: Vec<String> = archive.file_names().map(String::from).collect();

        Ok(Self {
            archive,
            file_names,
            max_entry_size: None,
        })
    }

    /// Refuses to read any entry whose uncompressed size exceeds `limit`
    /// bytes, with [`GedzipError::EntryTooLarge`].
    ///
    /// An archive comes from outside: without a limit, a small archive can
    /// expand to any size in memory (a "zip bomb"). The limit is checked
    /// against the size the archive declares and enforced while reading, so
    /// an entry that declares less than it holds is refused too.
    #[must_use]
    pub fn max_entry_size(mut self, limit: u64) -> Self {
        self.max_entry_size = Some(limit);
        self
    }

    /// Reads a whole entry, within the configured limit.
    fn read_entry(&mut self, name: &str) -> Result<Vec<u8>, GedzipError> {
        self.read_entry_within(name, self.max_entry_size)
    }

    /// Reads a whole entry, within `limit`.
    fn read_entry_within(
        &mut self,
        name: &str,
        limit: Option<u64>,
    ) -> Result<Vec<u8>, GedzipError> {
        let file = self.archive.by_name(name)?;
        let too_large = || GedzipError::EntryTooLarge {
            name: name.to_string(),
            limit: limit.unwrap_or_default(),
        };
        let mut bytes = Vec::new();
        if let Some(limit) = limit {
            if file.size() > limit {
                return Err(too_large());
            }
            file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > limit {
                return Err(too_large());
            }
        } else {
            let mut file = file;
            file.read_to_end(&mut bytes)?;
        }
        Ok(bytes)
    }

    /// The name of the archive entry a `FILE` reference points at, if the
    /// archive holds it.
    ///
    /// GEDZIP names are case-sensitive and not percent-escaped (§4), so the
    /// exact name comes first, then the name the reference gives once
    /// decoded (GEDCOM 7.x `FILE` values are URL strings, so a space is
    /// `%20`, 7.0.17). Producers that do
    /// not follow the rules are read too: with backslashes or forward
    /// slashes, a leading `./` or `/`, and in any letter case. The names of
    /// directories are no entries.
    #[must_use]
    pub fn find_entry(&self, file_ref: &str) -> Option<&str> {
        let files = || self.file_names.iter().filter(|n| !is_directory(n));
        if let Some(name) = files().find(|n| *n == file_ref) {
            return Some(name);
        }
        let wanted = entry_key(file_ref);
        if let Some(name) = files().find(|n| entry_key(n) == wanted) {
            return Some(name);
        }
        let wanted = wanted.to_lowercase();
        files()
            .find(|n| entry_key(n).to_lowercase() == wanted)
            .map(String::as_str)
    }

    /// Reads the dataset of the archive (its `gedcom.ged`) with `builder`.
    /// The builder's size limit applies to `gedcom.ged` as it is
    /// decompressed: over it, the entry is refused
    /// ([`GedzipError::EntryTooLarge`]) before it is read whole.
    ///
    /// # Errors
    ///
    /// [`GedcomError::Gedzip`] when the entry cannot be read; otherwise as
    /// [`GedcomBuilder::build_from_bytes`].
    pub fn read_dataset(&mut self, builder: &GedcomBuilder) -> Result<Dataset, GedcomError> {
        let limit = match (self.max_entry_size, builder.file_size_limit()) {
            (own, None) => own,
            (own, Some(max)) => {
                let max = u64::try_from(max).unwrap_or(u64::MAX);
                Some(own.map_or(max, |own| own.min(max)))
            }
        };
        let bytes = self.read_entry_within(GEDCOM_FILENAME, limit)?;
        builder.build_from_bytes(bytes)
    }

    /// Reads the raw bytes of the `gedcom.ged` file.
    ///
    /// This is useful if you need to process the GEDCOM data with custom settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read.
    pub fn read_gedcom_bytes(&mut self) -> Result<Vec<u8>, GedzipError> {
        self.read_entry(GEDCOM_FILENAME)
    }

    /// Returns a list of all file names in the archive.
    ///
    /// This includes `gedcom.ged` and all media files.
    #[must_use]
    pub fn file_names(&self) -> &[String] {
        &self.file_names
    }

    /// Returns a list of media file names: every entry but `gedcom.ged`
    /// and the directories.
    #[must_use]
    pub fn media_files(&self) -> Vec<&str> {
        self.file_names
            .iter()
            .filter(|name| *name != GEDCOM_FILENAME && !is_directory(name))
            .map(String::as_str)
            .collect()
    }

    /// Reads a media file from the archive by name.
    ///
    /// # Arguments
    ///
    /// * `name` - The file name (path) within the archive, or a `FILE`
    ///   reference to it (see [`GedzipReader::find_entry`])
    ///
    /// # Errors
    ///
    /// Returns an error if the file does not exist, cannot be read, or
    /// exceeds the limit set with [`GedzipReader::max_entry_size`].
    pub fn read_media_file(&mut self, name: &str) -> Result<Vec<u8>, GedzipError> {
        let entry = self
            .find_entry(name)
            .ok_or_else(|| GedzipError::MissingMedia(name.to_string()))?
            .to_string();
        self.read_entry(&entry)
    }

    /// Checks if a file exists in the archive, by name or `FILE` reference
    /// (see [`GedzipReader::find_entry`]).
    #[must_use]
    pub fn contains_file(&self, name: &str) -> bool {
        self.find_entry(name).is_some()
    }

    /// Returns the number of files in the archive.
    #[must_use]
    pub fn len(&self) -> usize {
        self.archive.len()
    }

    /// Returns `true` if the archive contains only `gedcom.ged`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.archive.len() <= 1
    }
}

/// A writer for GEDZIP files.
///
/// `GedzipWriter` creates a ZIP archive containing a GEDCOM dataset and
/// optionally associated media files.
///
/// # Example
///
/// ```rust,no_run
/// # #[cfg(feature = "gedzip")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxidged::gedzip::GedzipWriter;
/// use oxidged::model::Dataset;
/// use std::fs::File;
///
/// let source = r#"
/// 0 HEAD
/// 1 GEDC
/// 2 VERS 7.0
/// 0 @I1@ INDI
/// 1 NAME John /Doe/
/// 0 @M1@ OBJE
/// 1 FILE photos/john.jpg
/// 2 FORM image/jpeg
/// 0 TRLR
/// "#;
/// let data = Dataset::parse(source);
///
/// let output = File::create("family.gdz")?;
/// let mut writer = GedzipWriter::new(output)?;
///
/// // Write the dataset
/// writer.write_dataset(&data)?;
///
/// // Add media files
/// let photo_bytes = std::fs::read("photos/john.jpg")?;
/// writer.add_media_file("photos/john.jpg", &photo_bytes)?;
///
/// // Finalize the archive
/// writer.finish()?;
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "gedzip"))]
/// # fn main() {}
/// ```
pub struct GedzipWriter<W: Write + Seek> {
    zip: ZipWriter<W>,
    has_gedcom: bool,
}

impl<W: Write + Seek> GedzipWriter<W> {
    /// Creates a new `GedzipWriter` that will write to the given destination.
    ///
    /// # Errors
    ///
    /// Returns an error if the ZIP writer cannot be initialized.
    pub fn new(writer: W) -> Result<Self, GedzipError> {
        let zip = ZipWriter::new(writer);
        Ok(Self {
            zip,
            has_gedcom: false,
        })
    }

    /// Writes a dataset to the archive as `gedcom.ged`, with the default
    /// [`GedcomWriter`]: as GEDCOM 7.x, a 5.5.1 dataset converted to 7.0.
    ///
    /// This should be called before adding any media files.
    ///
    /// # Errors
    ///
    /// Returns an error if the archive cannot be written.
    pub fn write_dataset(&mut self, data: &Dataset) -> Result<(), GedzipError> {
        self.write_dataset_with(data, &GedcomWriter::new())
    }

    /// Writes a dataset to the archive as `gedcom.ged`, with `writer`, in
    /// the 7.x version it is configured with, else in the dataset's own
    /// when it is 7.x, else in 7.0: a GEDZIP archive holds GEDCOM 7.x
    /// (§4). The bytes are UTF-8, with a byte order mark unless `writer`
    /// says otherwise.
    ///
    /// # Errors
    ///
    /// [`GedzipError::Write`] when `writer` fails (under
    /// [`RepairPolicy::Error`](crate::RepairPolicy::Error)); otherwise when
    /// the archive cannot be written.
    pub fn write_dataset_with(
        &mut self,
        data: &Dataset,
        writer: &GedcomWriter,
    ) -> Result<(), GedzipError> {
        let version = writer
            .config()
            .version
            .filter(|v| v.is_v7())
            .or_else(|| data.version().is_v7().then(|| data.version()))
            .unwrap_or(GedcomVersion::V7_0);
        let mut bytes = Vec::new();
        writer
            .clone()
            .gedcom_version(version)
            .write(&mut bytes, data)?;
        self.write_gedcom_bytes(&bytes)
    }

    /// Writes raw GEDCOM bytes to the archive as `gedcom.ged`.
    ///
    /// This is useful if you have pre-formatted GEDCOM content.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn write_gedcom_bytes(&mut self, bytes: &[u8]) -> Result<(), GedzipError> {
        let options = zip::write::FileOptions::<()>::default()
            .compression_method(CompressionMethod::Deflated);

        self.zip.start_file(GEDCOM_FILENAME, options)?;
        self.zip.write_all(bytes)?;
        self.has_gedcom = true;
        Ok(())
    }

    /// Adds a media file to the archive.
    ///
    /// `name` is the `FILE` payload that names the file in the dataset, or
    /// its entry name. GEDZIP entry names are UTF-8 and not percent-escaped,
    /// while a `FILE` payload percent-escapes what a URL string cannot hold
    /// (§4, 7.0.17): the entry is named by `name` with its
    /// percent-encodings decoded (`photos/a%20b.jpg` is the entry
    /// `photos/a b.jpg`), backslashes as slashes and no leading `./` or
    /// `/`, as [`GedzipReader::find_entry`] matches them.
    ///
    /// # Arguments
    ///
    /// * `name` - The `FILE` payload naming the file, or its entry name
    /// * `bytes` - The file contents
    ///
    /// # Errors
    ///
    /// [`GedzipError::ReservedName`] for a name GEDZIP reserves
    /// (`gedcom.ged`, `META-INF/…`, `MANIFEST.MF`) or a directory's (ending
    /// with `/`); otherwise when the file cannot be written.
    pub fn add_media_file(&mut self, name: &str, bytes: &[u8]) -> Result<(), GedzipError> {
        if is_reserved(name) {
            return Err(GedzipError::ReservedName(name.to_string()));
        }
        let entry = entry_key(name);
        let options =
            zip::write::FileOptions::<()>::default().compression_method(media_compression(&entry));

        self.zip.start_file(entry, options)?;
        self.zip.write_all(bytes)?;
        Ok(())
    }

    /// Adds multiple media files to the archive.
    ///
    /// # Arguments
    ///
    /// * `files` - A map of file names to file contents
    ///
    /// # Errors
    ///
    /// Returns an error if any file cannot be written.
    pub fn add_media_files<S: std::hash::BuildHasher>(
        &mut self,
        files: &HashMap<String, Vec<u8>, S>,
    ) -> Result<(), GedzipError> {
        for (name, bytes) in files {
            self.add_media_file(name, bytes)?;
        }
        Ok(())
    }

    /// Finalizes the archive and returns the underlying writer.
    ///
    /// This must be called to ensure the ZIP archive is properly closed.
    ///
    /// # Errors
    ///
    /// Returns an error if the archive cannot be finalized.
    pub fn finish(self) -> Result<W, GedzipError> {
        Ok(self.zip.finish()?)
    }

    /// Returns `true` if the GEDCOM data has been written.
    #[must_use]
    pub fn has_gedcom(&self) -> bool {
        self.has_gedcom
    }
}

/// How to store a media file: formats that are already compressed (most
/// images, audio, video, PDF and archives) gain nothing from Deflate and cost
/// time on both ends, so they are stored as they are, as GEDZIP suggests
/// (§4, 7.0.17); `gedcom.ged` and other text is deflated.
fn media_compression(name: &str) -> CompressionMethod {
    const COMPRESSED: [&str; 18] = [
        "jpg", "jpeg", "png", "gif", "webp", "avif", "heic", "jp2", "pdf", "mp3", "m4a", "ogg",
        "opus", "mp4", "m4v", "webm", "zip", "gz",
    ];
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    match extension {
        Some(ext) if COMPRESSED.contains(&ext.as_str()) => CompressionMethod::Stored,
        _ => CompressionMethod::Deflated,
    }
}

/// The form of an entry name or `FILE` reference used to match one against
/// the other: forward slashes, no leading `./` or `/`, percent-decoded, lower
/// case.
/// Whether an archive entry is a directory.
fn is_directory(name: &str) -> bool {
    name.ends_with('/')
}

/// Whether a media file cannot take `name`: the dataset's own, the names
/// GEDZIP recommends against because readers of ZIP-based formats use them
/// (§2.12: `META-INF/…`, `MANIFEST.MF`), and a directory's.
fn is_reserved(name: &str) -> bool {
    let key = entry_key(name);
    key == GEDCOM_FILENAME
        || key == "MANIFEST.MF"
        || key.starts_with("META-INF/")
        || is_directory(name)
        || key.is_empty()
}

/// A `FILE` reference or an entry name as a name of the archive: forward
/// slashes, no leading `./` or `/`, percent-encodings decoded.
fn entry_key(name: &str) -> String {
    let slashed = name.replace('\\', "/");
    let trimmed = slashed.trim_start_matches("./").trim_start_matches('/');
    let bytes = trimmed.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            // Both digits are below 16, so the byte fits.
            decoded.push(u8::try_from(hi * 16 + lo).unwrap_or(b'%'));
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Reads a GEDZIP file from bytes and returns the parsed GEDCOM data.
///
/// This is a convenience function for simple use cases.
///
/// # Errors
///
/// Returns an error if:
/// - The bytes are not a valid ZIP archive
/// - The archive does not contain a `gedcom.ged` file
/// - The GEDCOM data cannot be parsed
///
/// # Example
///
/// ```rust,no_run
/// # #[cfg(feature = "gedzip")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxidged::gedzip::read_gedzip;
///
/// let bytes = std::fs::read("family.gdz")?;
/// let data = read_gedzip(&bytes)?;
/// println!("Found {} individuals", data.individuals().len());
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "gedzip"))]
/// # fn main() {}
/// ```
pub fn read_gedzip(bytes: &[u8]) -> Result<Dataset, GedcomError> {
    GedcomBuilder::new().build_from_gedzip(std::io::Cursor::new(bytes))
}

/// Writes GEDCOM data to a GEDZIP file and returns the bytes.
///
/// This is a convenience function for simple use cases without media files.
///
/// # Example
///
/// ```rust,no_run
/// # #[cfg(feature = "gedzip")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxidged::gedzip::write_gedzip;
/// use oxidged::model::Dataset;
///
/// let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n");
///
/// let bytes = write_gedzip(&data)?;
/// std::fs::write("output.gdz", bytes)?;
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "gedzip"))]
/// # fn main() {}
/// ```
///
/// # Errors
///
/// Returns a `GedzipError` if:
/// - The GEDCOM data cannot be serialized
/// - The ZIP archive cannot be created
pub fn write_gedzip(data: &Dataset) -> Result<Vec<u8>, GedzipError> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = GedzipWriter::new(cursor)?;
    writer.write_dataset(data)?;
    let cursor = writer.finish()?;
    Ok(cursor.into_inner())
}

/// Writes GEDCOM data with media files to a GEDZIP file and returns the bytes.
///
/// # Arguments
///
/// * `data` - The GEDCOM data to write
/// * `media_files` - A map of file names to file contents
///
/// # Example
///
/// ```rust,no_run
/// # #[cfg(feature = "gedzip")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxidged::gedzip::write_gedzip_with_media;
/// use oxidged::model::Dataset;
/// use std::collections::HashMap;
///
/// let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n");
///
/// let mut media = HashMap::new();
/// media.insert("photos/test.jpg".to_string(), vec![0xFF, 0xD8, 0xFF]);
///
/// let bytes = write_gedzip_with_media(&data, &media)?;
/// std::fs::write("output.gdz", bytes)?;
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "gedzip"))]
/// # fn main() {}
/// ```
///
/// # Errors
///
/// Returns a `GedzipError` if:
/// - The GEDCOM data cannot be serialized
/// - A media file cannot be written to the archive
/// - The ZIP archive cannot be created
pub fn write_gedzip_with_media<S: std::hash::BuildHasher>(
    data: &Dataset,
    media_files: &HashMap<String, Vec<u8>, S>,
) -> Result<Vec<u8>, GedzipError> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = GedzipWriter::new(cursor)?;
    writer.write_dataset(data)?;
    writer.add_media_files(media_files)?;
    let cursor = writer.finish()?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_minimal_gedcom() -> Dataset {
        Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n")
    }

    #[test]
    fn test_write_and_read_minimal_gedzip() {
        let data = create_minimal_gedcom();

        // Write to GEDZIP
        let bytes = write_gedzip(&data).expect("Failed to write GEDZIP");

        // Read back
        let parsed = read_gedzip(&bytes).expect("Failed to read GEDZIP");

        // Verify
        assert!(parsed.version().is_v7());
    }

    #[test]
    fn test_gedzip_with_media_files() {
        let data = create_minimal_gedcom();
        let mut media = HashMap::new();
        media.insert("test.txt".to_string(), b"Hello, World!".to_vec());
        media.insert("photos/image.jpg".to_string(), vec![0xFF, 0xD8, 0xFF, 0xE0]);

        // Write
        let bytes = write_gedzip_with_media(&data, &media).expect("Failed to write GEDZIP");

        // Read back
        let cursor = std::io::Cursor::new(bytes);
        let mut reader = GedzipReader::new(cursor).expect("Failed to create reader");

        // Verify files are present
        assert!(reader.contains_file(GEDCOM_FILENAME));
        assert!(reader.contains_file("test.txt"));
        assert!(reader.contains_file("photos/image.jpg"));

        // Verify media files list
        let media_files = reader.media_files();
        assert_eq!(media_files.len(), 2);

        // Read media file content
        let txt_content = reader
            .read_media_file("test.txt")
            .expect("Failed to read test.txt");
        assert_eq!(txt_content, b"Hello, World!");
    }

    #[test]
    fn test_missing_gedcom_file() {
        // Create a ZIP without gedcom.ged
        let cursor = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(cursor);
        let options = zip::write::FileOptions::<()>::default();
        zip.start_file("other.txt", options).unwrap();
        zip.write_all(b"test").unwrap();
        let cursor = zip.finish().unwrap();

        // Try to read as GEDZIP
        let result = GedzipReader::new(std::io::Cursor::new(cursor.into_inner()));
        assert!(matches!(result, Err(GedzipError::MissingGedcom)));
    }

    #[test]
    fn test_reader_len_and_is_empty() {
        let data = create_minimal_gedcom();

        // Create GEDZIP with only gedcom.ged
        let bytes = write_gedzip(&data).expect("Failed to write");
        let cursor = std::io::Cursor::new(bytes);
        let reader = GedzipReader::new(cursor).expect("Failed to create reader");

        assert_eq!(reader.len(), 1);
        assert!(reader.is_empty()); // Only has gedcom.ged

        // Create GEDZIP with media
        let mut media = HashMap::new();
        media.insert("test.txt".to_string(), b"test".to_vec());
        let bytes = write_gedzip_with_media(&data, &media).expect("Failed to write");
        let cursor = std::io::Cursor::new(bytes);
        let reader = GedzipReader::new(cursor).expect("Failed to create reader");

        assert_eq!(reader.len(), 2);
        assert!(!reader.is_empty());
    }

    #[test]
    fn test_gedzip_roundtrip_with_individuals() {
        let source = r"0 HEAD
1 GEDC
2 VERS 7.0
0 @I1@ INDI
1 NAME John /Doe/
1 SEX M
0 @I2@ INDI
1 NAME Jane /Doe/
1 SEX F
0 @F1@ FAM
1 HUSB @I1@
1 WIFE @I2@
0 TRLR";

        let data = Dataset::parse(source);

        // Roundtrip
        let bytes = write_gedzip(&data).expect("Failed to write");
        let parsed = read_gedzip(&bytes).expect("Failed to read");

        assert_eq!(parsed.individuals().len(), 2);
        assert_eq!(parsed.families().len(), 1);
    }

    #[test]
    fn test_read_missing_media_file() {
        let data = create_minimal_gedcom();
        let bytes = write_gedzip(&data).expect("Failed to write");
        let cursor = std::io::Cursor::new(bytes);
        let mut reader = GedzipReader::new(cursor).expect("Failed to create reader");

        let result = reader.read_media_file("nonexistent.jpg");
        assert!(matches!(result, Err(GedzipError::MissingMedia(_))));
    }

    #[test]
    fn test_read_through_the_builder() {
        let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n0 TRLR\n");
        let bytes = write_gedzip(&data).unwrap();
        let archive = || std::io::Cursor::new(bytes.as_slice());
        assert!(GedcomBuilder::new().build_from_gedzip(archive()).is_ok());
        // The builder's limit applies to gedcom.ged, as it is decompressed.
        assert!(matches!(
            GedcomBuilder::new()
                .max_file_size(10)
                .build_from_gedzip(archive()),
            Err(GedcomError::Gedzip(GedzipError::EntryTooLarge {
                limit: 10,
                ..
            }))
        ));
        // Strict mode applies too: the writer repaired `SEX male`, so the
        // archive holds a conformant file.
        assert!(
            GedcomBuilder::new()
                .strict(true)
                .build_from_gedzip(archive())
                .is_ok()
        );
        assert!(matches!(
            GedcomBuilder::new().build_from_gedzip(std::io::Cursor::new(b"not a zip")),
            Err(GedcomError::Gedzip(GedzipError::Zip(_)))
        ));
    }
}
