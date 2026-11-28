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
//! An entry can be read and written whole (as bytes) or streamed: read as it
//! is decompressed ([`GedzipReader::open_media_file`],
//! [`GedzipReader::open_gedcom`]) and written from any reader
//! ([`GedzipWriter::add_media_file_from`],
//! [`GedzipWriter::write_gedcom_from`]), so that no entry is held whole in
//! memory.
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
use std::fmt;
use std::io::{self, Read, Seek, Write};

use zip::CompressionMethod;
use zip::read::{ZipArchive, ZipFile};
use zip::write::ZipWriter;

use crate::model::Dataset;
use crate::writer::{GedcomWriter, RepairPolicy, WriteError};
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

/// An I/O error, or [`GedzipError::EntryTooLarge`] for the error a
/// [`GedzipEntry`] read past its limit fails with.
impl From<std::io::Error> for GedzipError {
    fn from(err: std::io::Error) -> Self {
        entry_too_large(&err).unwrap_or(Self::Io(err))
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

        // The names of the entries: UTF-8, else CP437 (ZIP's legacy
        // encoding of names).
        let file_names = archive
            .file_names()
            .map(|name| name.map(String::from))
            .collect::<Result<Vec<_>, _>>()?;

        // Verify gedcom.ged exists
        if !file_names.iter().any(|name| name == GEDCOM_FILENAME) {
            return Err(GedzipError::MissingGedcom);
        }

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

    /// Opens the entry `name` (an exact entry name) to read it within
    /// `limit`: refused when the size it declares exceeds the limit, and
    /// failing as it is read past it.
    fn open_within(
        &mut self,
        name: &str,
        limit: Option<u64>,
    ) -> Result<GedzipEntry<'_, R>, GedzipError> {
        let file = self.archive.by_name(name)?;
        if let Some(limit) = limit
            && file.size() > limit
        {
            return Err(GedzipError::EntryTooLarge {
                name: name.to_string(),
                limit,
            });
        }
        Ok(GedzipEntry {
            inner: Within::new(file, name, limit),
        })
    }

    /// Reads a whole entry, within the configured limit.
    fn read_entry(&mut self, name: &str) -> Result<Vec<u8>, GedzipError> {
        let mut bytes = Vec::new();
        self.open_within(name, self.max_entry_size)?
            .read_to_end(&mut bytes)?;
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

    /// Reads the dataset of the archive (its `gedcom.ged`) with `builder`,
    /// as [`GedcomBuilder::build_from_reader`] reads the entry as it is
    /// decompressed: the bytes read become the dataset's store. The
    /// builder's size limit applies to `gedcom.ged` as it is decompressed,
    /// with the reader's own ([`GedzipReader::max_entry_size`]), the
    /// smaller one first: over it, the entry is refused
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
        // The entry fails past `limit`, which is at most the builder's: the
        // builder never reaches its own.
        builder.build_from_reader(self.open_within(GEDCOM_FILENAME, limit)?)
    }

    /// Reads the raw bytes of the `gedcom.ged` file, within the limit set
    /// with [`GedzipReader::max_entry_size`].
    ///
    /// This is useful if you need to process the GEDCOM data with custom settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read.
    pub fn read_gedcom_bytes(&mut self) -> Result<Vec<u8>, GedzipError> {
        self.read_entry(GEDCOM_FILENAME)
    }

    /// Opens `gedcom.ged` to read it as it is decompressed, within the limit
    /// set with [`GedzipReader::max_entry_size`] (see [`GedzipEntry`]).
    ///
    /// Wrapped in a [`std::io::BufReader`], it feeds a
    /// [`GedcomStreamParser`](crate::GedcomStreamParser), which reads the
    /// dataset one record at a time without holding the file:
    ///
    /// ```rust
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxidged::gedzip::{write_gedzip, GedzipReader};
    /// use oxidged::model::RecordRef;
    /// use oxidged::{Dataset, GedcomStreamParser};
    /// use std::io::{BufReader, Cursor};
    ///
    /// let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n");
    /// let mut archive = GedzipReader::new(Cursor::new(write_gedzip(&data)?))?;
    /// let mut names = Vec::new();
    /// for record in GedcomStreamParser::new(BufReader::new(archive.open_gedcom()?))? {
    ///     if let RecordRef::Individual(indi) = record?.record() {
    ///         names.extend(indi.full_name());
    ///     }
    /// }
    /// assert_eq!(names, ["Ann Example"]);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`GedzipError::EntryTooLarge`] when the size the entry declares
    /// exceeds the limit; otherwise when the entry cannot be opened.
    pub fn open_gedcom(&mut self) -> Result<GedzipEntry<'_, R>, GedzipError> {
        self.open_within(GEDCOM_FILENAME, self.max_entry_size)
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
        let mut bytes = Vec::new();
        self.open_media_file(name)?.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    /// Opens a media file of the archive to read it as it is decompressed,
    /// within the limit set with [`GedzipReader::max_entry_size`] (see
    /// [`GedzipEntry`]): a file of any size is copied elsewhere without
    /// being held in memory.
    ///
    /// # Arguments
    ///
    /// * `name` - The file name (path) within the archive, or a `FILE`
    ///   reference to it (see [`GedzipReader::find_entry`])
    ///
    /// # Errors
    ///
    /// [`GedzipError::MissingMedia`] when the archive has no such file,
    /// [`GedzipError::EntryTooLarge`] when the size it declares exceeds the
    /// limit; otherwise when the entry cannot be opened.
    pub fn open_media_file(&mut self, name: &str) -> Result<GedzipEntry<'_, R>, GedzipError> {
        let entry = self
            .find_entry(name)
            .ok_or_else(|| GedzipError::MissingMedia(name.to_string()))?
            .to_string();
        self.open_within(&entry, self.max_entry_size)
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

/// An entry of a GEDZIP archive, read as it is decompressed: what
/// [`GedzipReader::open_media_file`] and [`GedzipReader::open_gedcom`]
/// give. It borrows the reader until it is dropped.
///
/// It reads no more than the limit set with
/// [`GedzipReader::max_entry_size`]. An entry that declares more is refused
/// when it is opened; one that holds more than it declares fails with an
/// [`io::Error`] of kind [`io::ErrorKind::InvalidData`] once its declared
/// size is passed; and reading past the limit, whatever the entry declares,
/// fails with an [`io::Error`] of kind [`io::ErrorKind::FileTooLarge`],
/// which converts into [`GedzipError::EntryTooLarge`] (`?` in a function
/// returning a [`GedzipError`]) and into
/// [`GedcomError::Gedzip`]`(`[`GedzipError::EntryTooLarge`]`)`. The bytes it
/// gives are checked against the entry's CRC-32 at its end.
///
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxidged::gedzip::{GedzipReader, GedzipWriter};
/// use oxidged::Dataset;
/// use std::io::{self, Cursor, Read};
///
/// // An archive with a media file of 1 MiB, written from a reader.
/// let mut writer = GedzipWriter::new(Cursor::new(Vec::new()))?;
/// writer.write_dataset(&Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n"))?;
/// let written = writer.add_media_file_from("media/scan.tif", &mut io::repeat(0x2A).take(1 << 20))?;
/// assert_eq!(written, 1 << 20);
/// let archive = writer.finish()?.into_inner();
///
/// // Copy the media file out without holding it.
/// let mut reader = GedzipReader::new(Cursor::new(archive))?;
/// let mut entry = reader.open_media_file("media/scan.tif")?;
/// assert_eq!(entry.size(), 1 << 20);
/// assert_eq!(io::copy(&mut entry, &mut io::sink())?, 1 << 20);
/// # Ok(())
/// # }
/// ```
pub struct GedzipEntry<'a, R: Read> {
    inner: Within<ZipFile<'a, R>>,
}

impl<R: Read> GedzipEntry<'_, R> {
    /// The entry's name in the archive.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// The uncompressed size the archive declares for the entry, in bytes:
    /// what it holds, unless the archive lies (the bytes read are checked).
    #[must_use]
    pub fn size(&self) -> u64 {
        self.inner.inner.size()
    }
}

impl<R: Read> Read for GedzipEntry<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read> fmt::Debug for GedzipEntry<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GedzipEntry")
            .field("name", &self.inner.name)
            .field("size", &self.size())
            .field("limit", &self.inner.limit)
            .field("read", &self.inner.read)
            .finish()
    }
}

/// A reader of an entry that fails once more than `limit` bytes are read.
struct Within<T> {
    inner: T,
    name: String,
    limit: Option<u64>,
    /// The bytes read so far.
    read: u64,
}

impl<T> Within<T> {
    fn new(inner: T, name: &str, limit: Option<u64>) -> Self {
        Self {
            inner,
            name: name.to_string(),
            limit,
            read: 0,
        }
    }
}

impl<T: Read> Read for Within<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(limit) = self.limit else {
            return self.inner.read(buf);
        };
        // One byte past the limit at most: enough to tell it is passed.
        let room = limit.saturating_sub(self.read).saturating_add(1);
        let len = usize::try_from(room).map_or(buf.len(), |room| room.min(buf.len()));
        let n = self.inner.read(&mut buf[..len])?;
        self.read = self
            .read
            .saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        if self.read > limit {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                TooLarge {
                    name: self.name.clone(),
                    limit,
                },
            ));
        }
        Ok(n)
    }
}

/// The error an entry read past its limit fails with, carried by an
/// [`io::Error`] ([`entry_too_large`]).
#[derive(Debug)]
struct TooLarge {
    name: String,
    limit: u64,
}

impl fmt::Display for TooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (name, limit) = (&self.name, self.limit);
        write!(f, "the archive entry {name} exceeds the {limit}-byte limit")
    }
}

impl std::error::Error for TooLarge {}

/// [`GedzipError::EntryTooLarge`], when `err` is the error a
/// [`GedzipEntry`] read past its limit fails with.
pub(crate) fn entry_too_large(err: &io::Error) -> Option<GedzipError> {
    let too_large = err.get_ref()?.downcast_ref::<TooLarge>()?;
    Some(GedzipError::EntryTooLarge {
        name: too_large.name.clone(),
        limit: too_large.limit,
    })
}

/// A writer that passes no flush on: [`GedcomWriter::write`] flushes at its
/// end, and flushing a Deflate stream ends a block, which would change the
/// bytes of the entry. The entry is complete when the next one starts or
/// the archive is finished.
struct Unflushed<'a, W: Write>(&'a mut W);

impl<W: Write> Write for Unflushed<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.write_all(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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
/// // Add media files, whole or from a reader
/// let photo_bytes = std::fs::read("photos/john.jpg")?;
/// writer.add_media_file("photos/john.jpg", &photo_bytes)?;
/// let mut scan = File::open("scans/census.tif")?;
/// writer.add_media_file_from("scans/census.tif", &mut scan)?;
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
    /// The text goes into the archive entry as `writer` produces it, not
    /// through a copy of it. Under
    /// [`RepairPolicy::Error`], it is written
    /// whole first, so that a refusal leaves the archive without
    /// `gedcom.ged`.
    ///
    /// # Errors
    ///
    /// [`GedzipError::Write`] when `writer` fails (under
    /// [`RepairPolicy::Error`]); otherwise when
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
        let writer = writer.clone().gedcom_version(version);
        if writer.config().on_nonconformant == RepairPolicy::Error {
            let mut bytes = Vec::new();
            writer.write(&mut bytes, data)?;
            return self.write_gedcom_bytes(&bytes);
        }
        self.start_gedcom()?;
        // The archive's own failures are I/O errors, as for the bytes.
        writer
            .write(Unflushed(&mut self.zip), data)
            .map_err(|e| match e {
                WriteError::Io(e) => GedzipError::from(e),
                e => GedzipError::Write(e),
            })?;
        self.has_gedcom = true;
        Ok(())
    }

    /// Writes raw GEDCOM bytes to the archive as `gedcom.ged`.
    ///
    /// This is useful if you have pre-formatted GEDCOM content.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn write_gedcom_bytes(&mut self, bytes: &[u8]) -> Result<(), GedzipError> {
        self.write_gedcom_from(&mut &*bytes).map(drop)
    }

    /// Writes the GEDCOM text `reader` gives to the archive as
    /// `gedcom.ged`, as it is read, and returns its size in bytes: what
    /// [`GedzipWriter::write_gedcom_bytes`] does without holding the text.
    ///
    /// # Errors
    ///
    /// When `reader` fails or the archive cannot be written; the entry is
    /// then left incomplete, and the archive is to be discarded.
    pub fn write_gedcom_from(
        &mut self,
        reader: &mut (impl Read + ?Sized),
    ) -> Result<u64, GedzipError> {
        self.start_gedcom()?;
        let size = io::copy(reader, &mut self.zip)?;
        self.has_gedcom = true;
        Ok(size)
    }

    /// Starts the `gedcom.ged` entry, deflated.
    fn start_gedcom(&mut self) -> Result<(), GedzipError> {
        let options = zip::write::FileOptions::<()>::default()
            .compression_method(CompressionMethod::Deflated);
        self.zip.start_file(GEDCOM_FILENAME, options)?;
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
        self.add_media_file_from(name, &mut &*bytes).map(drop)
    }

    /// Adds a media file to the archive from what `reader` gives, as it is
    /// read, and returns its size in bytes: what
    /// [`GedzipWriter::add_media_file`] does without holding the file. The
    /// file is named and stored as `add_media_file` names and stores it.
    ///
    /// ```rust
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxidged::gedzip::GedzipWriter;
    /// use oxidged::Dataset;
    /// use std::io::{Cursor, Read};
    ///
    /// let mut writer = GedzipWriter::new(Cursor::new(Vec::new()))?;
    /// writer.write_dataset(&Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n"))?;
    /// // Any reader: an open file, a network stream, another archive's entry.
    /// let mut source = Cursor::new(b"letter text".to_vec());
    /// assert_eq!(writer.add_media_file_from("letters/a%20letter.txt", &mut source)?, 11);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`GedzipError::ReservedName`] for a name GEDZIP reserves, as
    /// [`GedzipWriter::add_media_file`]; otherwise when `reader` fails or
    /// the file cannot be written, the entry then left incomplete and the
    /// archive to be discarded.
    pub fn add_media_file_from(
        &mut self,
        name: &str,
        reader: &mut (impl Read + ?Sized),
    ) -> Result<u64, GedzipError> {
        if is_reserved(name) {
            return Err(GedzipError::ReservedName(name.to_string()));
        }
        let entry = entry_key(name);
        let options =
            zip::write::FileOptions::<()>::default().compression_method(media_compression(&entry));

        self.zip.start_file(entry, options)?;
        Ok(io::copy(reader, &mut self.zip)?)
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
    const COMPRESSED: [&str; 19] = [
        "jpg", "jpeg", "png", "gif", "webp", "avif", "heic", "jp2", "jxl", "pdf", "mp3", "m4a",
        "ogg", "opus", "mp4", "m4v", "webm", "zip", "gz",
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

    /// An entry is refused once more than the limit is read, whatever size
    /// it declares: the bytes up to the limit are given, the next read
    /// fails, and the error converts into `EntryTooLarge`.
    #[test]
    fn an_entry_fails_past_its_limit_as_it_is_read() {
        let source = [b'x'; 100];
        let mut exact = Within::new(&source[..], "media/a.txt", Some(100));
        let mut bytes = Vec::new();
        assert_eq!(exact.read_to_end(&mut bytes).unwrap(), 100);

        let mut within = Within::new(&source[..], "media/a.txt", Some(60));
        let mut buf = [0; 50];
        assert_eq!(within.read(&mut buf).unwrap(), 50);
        // Ten bytes are left within the limit; reading eleven passes it.
        let err = within.read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::FileTooLarge);
        assert_eq!(
            err.to_string(),
            "the archive entry media/a.txt exceeds the 60-byte limit"
        );
        assert!(matches!(
            GedzipError::from(err),
            GedzipError::EntryTooLarge { name, limit: 60 } if name == "media/a.txt"
        ));

        // Through the stream parser, a `GedcomError::Gedzip`.
        let text = b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n0 TRLR\n";
        let within = Within::new(&text[..], GEDCOM_FILENAME, Some(20));
        let read: Result<Vec<_>, _> =
            crate::GedcomStreamParser::new(std::io::BufReader::new(within))
                .and_then(Iterator::collect);
        assert!(matches!(
            read,
            Err(GedcomError::Gedzip(GedzipError::EntryTooLarge {
                limit: 20,
                ..
            }))
        ));
        // Other I/O errors stay what they are.
        assert!(matches!(
            GedzipError::from(std::io::Error::other("failed")),
            GedzipError::Io(_)
        ));
        assert!(matches!(
            GedcomError::from(std::io::Error::other("failed")),
            GedcomError::Io(_)
        ));
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
