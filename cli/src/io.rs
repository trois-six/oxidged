//! Reading the input (GEDCOM, GEDZIP or JSON, from a file or standard
//! input) and writing the output, with every reading and writing option.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use oxidged::builder::ReadReport;
use oxidged::convert::ConversionOptions;
use oxidged::encoding::GedcomEncoding;
use oxidged::gedzip::{GedzipReader, GedzipWriter};
use oxidged::model::Dataset;
use oxidged::profile::{self, Interpretation};
use oxidged::writer::{Bom, Credentials, LineEnding, RepairPolicy, WriteReport};
use oxidged::{GedcomBuilder, GedcomVersion, GedcomWriter, OutputEncoding};
use serde_json::json;

use crate::{CliError, usage};

/// A file format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// GEDCOM text (`.ged`)
    Ged,
    /// A GEDZIP archive (`.gdz`): GEDCOM 7.x with its media files
    Gdz,
    /// The dataset as JSON (`.json`)
    Json,
}

impl Format {
    /// The format a path's extension names.
    fn of_path(path: &str) -> Option<Self> {
        let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "ged" => Some(Format::Ged),
            "gdz" | "zip" => Some(Format::Gdz),
            "json" => Some(Format::Json),
            _ => None,
        }
    }

    /// The format bytes look like: a ZIP archive, a JSON object, else GEDCOM.
    fn sniff(bytes: &[u8]) -> Self {
        if bytes.starts_with(b"PK\x03\x04") {
            return Format::Gdz;
        }
        let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        match text.iter().find(|b| !b.is_ascii_whitespace()) {
            Some(b'{') => Format::Json,
            _ => Format::Ged,
        }
    }
}

/// How a producer's conventions are read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum InterpretationArg {
    /// As written
    None,
    /// The rules whose standard form keeps all of the data
    #[default]
    Lossless,
    /// Also the rules that infer a meaning
    BestEffort,
}

impl From<InterpretationArg> for Interpretation {
    fn from(i: InterpretationArg) -> Self {
        match i {
            InterpretationArg::None => Interpretation::None,
            InterpretationArg::Lossless => Interpretation::Lossless,
            InterpretationArg::BestEffort => Interpretation::BestEffort,
        }
    }
}

/// The input and how to read it.
#[derive(Args, Debug, Clone)]
pub struct ReadArgs {
    /// The file to read (GEDCOM, GEDZIP or JSON); `-` for standard input
    #[arg(value_name = "FILE")]
    pub input: String,
    /// The input's format [default: from its extension, else its content]
    #[arg(long, value_enum, value_name = "FORMAT", help_heading = "Reading")]
    pub from: Option<Format>,
    /// Decode the bytes in this encoding, whatever they declare (UTF-8,
    /// UNICODE, ANSEL, ANSI, ISO-8859-1, IBMPC, MACINTOSH, …)
    #[arg(long, value_name = "ENCODING", help_heading = "Reading")]
    pub encoding: Option<String>,
    /// Refuse an input that does not follow its specification (exit code 2)
    #[arg(long, help_heading = "Reading")]
    pub strict: bool,
    /// Refuse an input larger than this many bytes
    #[arg(long, value_name = "BYTES", help_heading = "Reading")]
    pub max_size: Option<usize>,
    /// Read the input as this producer wrote it, instead of detecting it
    /// (see `oxidged profiles`)
    #[arg(long, value_name = "PROFILE", help_heading = "Reading")]
    pub producer: Option<String>,
    /// How a producer's conventions are read
    #[arg(
        long,
        value_enum,
        default_value_t,
        value_name = "LEVEL",
        help_heading = "Reading"
    )]
    pub interpretation: InterpretationArg,
}

/// A dataset read, and what reading it found.
pub struct Loaded {
    pub data: Dataset,
    /// What reading found, for GEDCOM and GEDZIP input decoded by evidence.
    pub report: Option<ReadReport>,
    /// The format read.
    pub format: Format,
    /// The media files of a GEDZIP input, by name.
    pub media: HashMap<String, Vec<u8>>,
}

/// The bytes of a file, or of standard input for `-`.
pub fn read_bytes(path: &str) -> Result<Vec<u8>, CliError> {
    if path == "-" {
        let mut bytes = Vec::new();
        io::stdin().lock().read_to_end(&mut bytes)?;
        Ok(bytes)
    } else {
        fs::read(path).map_err(|e| CliError::Io(io::Error::new(e.kind(), format!("{path}: {e}"))))
    }
}

/// An encoding named by a `CHAR` label.
pub fn encoding_named(label: &str) -> Result<GedcomEncoding, CliError> {
    GedcomEncoding::from_label(label).ok_or_else(|| usage(format!("unknown encoding: {label}")))
}

impl ReadArgs {
    /// The builder these options configure.
    pub fn builder(&self) -> Result<GedcomBuilder, CliError> {
        let mut builder = GedcomBuilder::new()
            .strict(self.strict)
            .interpretation(self.interpretation.into());
        if let Some(max) = self.max_size {
            builder = builder.max_file_size(max);
        }
        if let Some(id) = &self.producer {
            let p = profile::profile(id)
                .ok_or_else(|| usage(format!("unknown producer: {id} (see `oxidged profiles`)")))?;
            builder = builder.profile(Some(p));
        }
        Ok(builder)
    }

    /// The format of the input.
    fn format(&self, bytes: &[u8]) -> Format {
        self.from
            .or_else(|| Format::of_path(&self.input))
            .unwrap_or_else(|| Format::sniff(bytes))
    }

    /// The input's bytes and format, a GEDZIP archive opened.
    pub fn bytes(&self) -> Result<(Vec<u8>, Format), CliError> {
        let bytes = read_bytes(&self.input)?;
        let format = self.format(&bytes);
        Ok((bytes, format))
    }

    /// Reads the input; with `report`, what reading found too (GEDCOM
    /// decoded by evidence).
    pub fn load(&self, report: bool) -> Result<Loaded, CliError> {
        let (bytes, format) = self.bytes()?;
        let builder = self.builder()?;
        let mut media = HashMap::new();
        let gedcom = match format {
            Format::Json => {
                if self.encoding.is_some() || self.strict || self.producer.is_some() {
                    return Err(usage(
                        "--encoding, --strict and --producer read GEDCOM, not JSON",
                    ));
                }
                let data: Dataset = serde_json::from_slice(&bytes)
                    .map_err(|e| CliError::Input(format!("JSON: {e}")))?;
                return Ok(Loaded {
                    data,
                    report: None,
                    format,
                    media,
                });
            }
            Format::Ged => bytes,
            Format::Gdz => {
                let mut archive = GedzipReader::new(Cursor::new(bytes))
                    .map_err(|e| CliError::Input(format!("GEDZIP: {e}")))?;
                let gedcom = archive
                    .read_gedcom_bytes()
                    .map_err(|e| CliError::Input(format!("GEDZIP: {e}")))?;
                let names: Vec<String> = archive
                    .media_files()
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                for name in names {
                    let file = archive
                        .read_media_file(&name)
                        .map_err(|e| CliError::Input(format!("GEDZIP: {e}")))?;
                    media.insert(name, file);
                }
                gedcom
            }
        };
        let (data, report) = match &self.encoding {
            Some(label) => (
                builder.build_from_bytes_with_encoding(&gedcom, encoding_named(label)?)?,
                None,
            ),
            None if report => {
                let (data, report) = builder.build_from_bytes_with_report(gedcom)?;
                (data, Some(report))
            }
            None => (builder.build_from_bytes(gedcom)?, None),
        };
        Ok(Loaded {
            data,
            report,
            format,
            media,
        })
    }
}

/// A GEDCOM version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum VersionArg {
    #[value(name = "5.5.1")]
    V551,
    #[value(name = "7.0")]
    V70,
    #[value(name = "7.1")]
    V71,
}

impl From<VersionArg> for GedcomVersion {
    fn from(v: VersionArg) -> Self {
        match v {
            VersionArg::V551 => GedcomVersion::V5_5_1,
            VersionArg::V70 => GedcomVersion::V7_0,
            VersionArg::V71 => GedcomVersion::V7_1,
        }
    }
}

/// The encoding of 5.5.1 output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum EncodingArg {
    #[value(name = "UTF-8", alias = "utf-8", alias = "utf8")]
    Utf8,
    /// UTF-16 little endian
    #[value(name = "UTF-16", alias = "utf-16", alias = "utf16", alias = "unicode")]
    Utf16,
    #[value(name = "ANSEL", alias = "ansel")]
    Ansel,
    #[value(name = "ASCII", alias = "ascii")]
    Ascii,
}

impl From<EncodingArg> for OutputEncoding {
    fn from(e: EncodingArg) -> Self {
        match e {
            EncodingArg::Utf8 => OutputEncoding::Utf8,
            EncodingArg::Utf16 => OutputEncoding::Utf16Le,
            EncodingArg::Ansel => OutputEncoding::Ansel,
            EncodingArg::Ascii => OutputEncoding::Ascii,
        }
    }
}

/// Whether to write a byte order mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum BomArg {
    /// For 7.x and UTF-16, not for 5.5.1 UTF-8
    Auto,
    Always,
    Never,
}

/// A line terminator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum LineEndingArg {
    Lf,
    Crlf,
    Cr,
}

/// The output and how to write it.
#[derive(Args, Debug, Clone)]
pub struct WriteArgs {
    /// The file to write; `-` or nothing for standard output
    #[arg(short, long, value_name = "FILE", help_heading = "Writing")]
    pub output: Option<String>,
    /// The output's format [default: from its extension, else GEDCOM]
    #[arg(long, value_enum, value_name = "FORMAT", help_heading = "Writing")]
    pub to: Option<Format>,
    /// The GEDCOM version to write [default: the dataset's]; 5.5.1 and 7.x
    /// are converted into each other
    #[arg(
        short = 'g',
        long = "gedcom-version",
        value_enum,
        value_name = "VERSION",
        help_heading = "Writing"
    )]
    pub gedcom_version: Option<VersionArg>,
    /// The encoding of 5.5.1 output (7.x is always UTF-8)
    #[arg(long, value_enum, value_name = "ENCODING", help_heading = "Writing")]
    pub output_encoding: Option<EncodingArg>,
    /// Whether to write a byte order mark
    #[arg(long, value_enum, value_name = "WHEN", help_heading = "Writing")]
    pub bom: Option<BomArg>,
    /// The line terminator
    #[arg(long, value_enum, value_name = "ENDING", help_heading = "Writing")]
    pub line_ending: Option<LineEndingArg>,
    /// The longest 5.5.1 line, longer payloads continued (at most 255)
    #[arg(long, value_name = "N", help_heading = "Writing")]
    pub max_line_length: Option<usize>,
    /// Writing 7.x from 5.5.1: keep the 5.5.1 structures 7.x drops (SUBN,
    /// HEAD.FILE, escapes) as declared extensions
    #[arg(long, help_heading = "Writing")]
    pub keep_obsolete: bool,
    /// Write the credentials a file may hold (_PASS, an encrypted or server
    /// login _USER), left out by default
    #[arg(long, help_heading = "Writing")]
    pub credentials: bool,
    /// Fail (exit code 2) instead of repairing what the version does not
    /// permit
    #[arg(long, help_heading = "Writing")]
    pub no_repair: bool,
    /// GEDZIP output: also add the media files the dataset references that
    /// are under this folder (a GEDZIP input's media files are always kept)
    #[arg(long, value_name = "DIR", help_heading = "Writing")]
    pub media_dir: Option<PathBuf>,
}

impl WriteArgs {
    /// The format to write.
    pub fn format(&self) -> Format {
        self.to
            .or_else(|| self.output.as_deref().and_then(Format::of_path))
            .unwrap_or(Format::Ged)
    }

    /// The GEDCOM writer these options configure, for `data`.
    pub fn writer(&self, data: &Dataset) -> Result<GedcomWriter, CliError> {
        let version = self
            .gedcom_version
            .map_or_else(|| data.version(), Into::into);
        let encoding: OutputEncoding = self
            .output_encoding
            .map_or(OutputEncoding::Utf8, Into::into);
        if version.is_v7() && encoding != OutputEncoding::Utf8 {
            return Err(usage(format!(
                "GEDCOM {version} is always UTF-8: --output-encoding {encoding} needs 5.5.1"
            )));
        }
        let mut writer = GedcomWriter::new()
            .gedcom_version(version)
            .output_encoding(encoding)
            .conversion(ConversionOptions::default().keep_obsolete(self.keep_obsolete))
            .credentials(if self.credentials {
                Credentials::Written
            } else {
                Credentials::LeftOut
            })
            .on_nonconformant(if self.no_repair {
                RepairPolicy::Error
            } else {
                RepairPolicy::Repair
            });
        if let Some(bom) = self.bom {
            writer = writer.bom(match bom {
                BomArg::Auto => Bom::Auto,
                BomArg::Always => Bom::Always,
                BomArg::Never => Bom::Never,
            });
        }
        if let Some(ending) = self.line_ending {
            writer = writer.line_ending(match ending {
                LineEndingArg::Lf => LineEnding::Lf,
                LineEndingArg::Crlf => LineEnding::CrLf,
                LineEndingArg::Cr => LineEnding::Cr,
            });
        }
        if let Some(n) = self.max_line_length {
            if !(1..=255).contains(&n) {
                return Err(usage("--max-line-length is between 1 and 255"));
            }
            writer = writer.max_line_length(n);
        }
        Ok(writer)
    }

    /// Writes `data` in the chosen format, and reports the repairs on
    /// standard error. `media` are the media files of the input.
    pub fn write(
        &self,
        data: &Dataset,
        media: &HashMap<String, Vec<u8>>,
        json: bool,
    ) -> Result<(), CliError> {
        let format = self.format();
        if format != Format::Gdz && self.media_dir.is_some() {
            return Err(usage("--media-dir writes GEDZIP (--to gdz)"));
        }
        let bytes = match format {
            Format::Json => {
                if self.gedcom_version.is_some()
                    || self.output_encoding.is_some()
                    || self.bom.is_some()
                    || self.line_ending.is_some()
                    || self.max_line_length.is_some()
                {
                    return Err(usage(
                        "the writing options write GEDCOM or GEDZIP, not JSON",
                    ));
                }
                let mut bytes = serde_json::to_vec_pretty(data)
                    .map_err(|e| CliError::Output(format!("JSON: {e}")))?;
                bytes.push(b'\n');
                bytes
            }
            Format::Ged => {
                let mut bytes = Vec::new();
                let report = self.writer(data)?.write(&mut bytes, data)?;
                report_repairs(&report, json);
                bytes
            }
            Format::Gdz => self.gedzip(data, media, json)?,
        };
        write_output(self.output.as_deref(), &bytes)
    }

    /// A GEDZIP archive of `data` (GEDCOM 7.x) and its media files.
    fn gedzip(
        &self,
        data: &Dataset,
        media: &HashMap<String, Vec<u8>>,
        json: bool,
    ) -> Result<Vec<u8>, CliError> {
        let version = match self.gedcom_version.map(GedcomVersion::from) {
            Some(v) if !v.is_v7() => {
                return Err(usage(
                    "a GEDZIP archive holds GEDCOM 7.x: --gedcom-version 7.0 or 7.1",
                ));
            }
            Some(v) => v,
            None if data.version().is_v7() => data.version(),
            None => GedcomVersion::V7_0,
        };
        let writer = self.writer(data)?.gedcom_version(version);
        let mut gedcom = Vec::new();
        let report = writer.write(&mut gedcom, data)?;
        report_repairs(&report, json);
        let zip = |e: oxidged::gedzip::GedzipError| CliError::Output(format!("GEDZIP: {e}"));
        let mut archive = GedzipWriter::new(Cursor::new(Vec::new())).map_err(zip)?;
        archive.write_gedcom_bytes(&gedcom).map_err(zip)?;
        let mut files = media.clone();
        if let Some(dir) = &self.media_dir {
            for name in media_references(data) {
                if files.contains_key(&name) {
                    continue;
                }
                let path = dir.join(&name);
                if path.is_file() {
                    files.insert(name, fs::read(&path)?);
                } else {
                    eprintln!("warning: no media file {}", path.display());
                }
            }
        }
        let mut names: Vec<&String> = files.keys().collect();
        names.sort();
        for name in names {
            archive.add_media_file(name, &files[name]).map_err(zip)?;
        }
        Ok(archive.finish().map_err(zip)?.into_inner())
    }
}

/// The relative file paths the dataset's multimedia `FILE`s name (not a
/// URL, an absolute path or a `file:` URI).
fn media_references(data: &Dataset) -> Vec<String> {
    fn walk(s: &oxidged::tree::Structure, out: &mut Vec<String>) {
        if s.tag == "FILE"
            && let Some(path) = s.text()
            && !path.contains(':')
            && !path.starts_with(['/', '\\'])
        {
            out.push(path.to_string());
        }
        for c in &s.substructures {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    for s in data.to_structures() {
        walk(&s, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Writes bytes to a file, or to standard output for `-` or none.
pub fn write_output(path: Option<&str>, bytes: &[u8]) -> Result<(), CliError> {
    match path {
        None | Some("-") => {
            let mut out = io::stdout().lock();
            out.write_all(bytes)?;
            out.flush()?;
        }
        Some(path) => fs::write(path, bytes)
            .map_err(|e| CliError::Io(io::Error::new(e.kind(), format!("{path}: {e}"))))?,
    }
    Ok(())
}

/// The repairs a write made, on standard error.
pub fn report_repairs(report: &WriteReport, json: bool) {
    if json {
        let repairs: Vec<_> = report
            .repairs
            .iter()
            .map(|r| json!({ "kind": format!("{:?}", r.kind), "detail": r.detail.to_string() }))
            .collect();
        eprintln!("{}", json!({ "repairs": repairs }));
    } else {
        for repair in &report.repairs {
            eprintln!("repair: {repair}");
        }
    }
}
