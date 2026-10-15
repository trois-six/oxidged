//! Writing GEDCOM files: conformant GEDCOM 5.5.1, 7.0 or 7.1 lines.
//!
//! [`GedcomWriter`] writes a [`Dataset`], or a lossless
//! [`Tree`] of structures, in a target version. Every
//! line goes through one emitter that applies the line rules of the
//! version ([`VersionRules`]):
//!
//! | Rule | GEDCOM 5.5.1 | GEDCOM 7.x |
//! |---|---|---|
//! | Line length | 255 characters, the whole line and its terminator; longer payloads continue with `CONC` | No limit, no `CONC` |
//! | Line breaks in text | `CONT` (from LF, CR LF or CR) | `CONT` |
//! | `@` in text | Every `@` doubled, `@#…@` escapes kept | A leading `@` doubled |
//! | Identifiers | A letter or digit then visible ASCII, at most 22 characters | `@[A-Z0-9_]+@`, never `@VOID@` |
//! | Header | `GEDC.VERS 5.5.1`, `GEDC.FORM LINEAGE-LINKED`, `CHAR` of the output encoding, `SOUR`, `SUBM` | `GEDC.VERS 7.0` (or `7.1`); no `FORM`, no `CHAR` |
//!
//! In both versions every line, the last one included, ends with the
//! terminator, record identifiers are unique, identifiers appear on records
//! only, tags follow the version's grammar and banned control characters
//! are not written. Pointers are written only from pointer fields, never
//! guessed from text.
//!
//! Data that does not fit these rules is rewritten into them (an invalid
//! identifier renamed, a tag made an extension tag, a control character
//! removed, …) and each rewrite is reported as a [`Repair`] in the
//! [`WriteReport`]; [`RepairPolicy::Error`] makes the first one an error
//! instead.
//!
//! # Example
//!
//! ```rust
//! use oxidged::model::Dataset;
//! use oxidged::{GedcomVersion, GedcomWriter};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n");
//!
//! let output = GedcomWriter::new()
//!     .gedcom_version(GedcomVersion::V7_0)
//!     .write_to_string(&data)?;
//! assert!(output.starts_with("0 HEAD\n1 GEDC\n2 VERS 7.0\n"));
//! assert!(output.ends_with("0 TRLR\n"));
//!
//! // Or straight into any `io::Write`, with a byte order mark in 7.x.
//! let mut bytes = Vec::new();
//! let report = GedcomWriter::new().write(&mut bytes, &data)?;
//! assert!(report.repairs.is_empty());
//! # Ok(())
//! # }
//! ```

mod emit;
pub(crate) mod head;
mod stream;
mod xref;

use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::io;
use std::rc::Rc;

use crate::convert::{ConversionOptions, Plan};
use crate::model::Dataset;
use crate::model::Store;
use crate::spec::conform::{
    Build, Direct, FieldOrder, Place, Rec, RecRef, conform_records, take_head, with_node,
};
use crate::tree::{Flat, FlatRef, Node, PayloadRef, Structure, Tree};
use crate::version::{GedcomVersion, VersionRules};
use emit::{emit, emit_known, emit_tagged};
use xref::XrefMap;

pub(crate) use emit::{
    AT_SIGN, CONTROL, LINE_FEED, LineSink, Looker, MAYBE_BANNED, TAB, emit_typed, extension_tag,
    is_valid_pointer, new_xref, special_bytes,
};
pub(crate) use head::{PLACEHOLDER, complete as complete_head, needs_submitter, stub_submitter};
pub use stream::GedcomStreamWriter;
pub(crate) use xref::{XrefIndex, numbered_xref};

/// The line terminator of written files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineEnding {
    /// LF (`\n`), the default.
    #[default]
    Lf,
    /// CR LF (`\r\n`).
    CrLf,
    /// CR (`\r`).
    Cr,
}

impl LineEnding {
    /// The terminator's characters.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

/// Whether the credentials a file may hold are written: `_PASS`, an
/// `_USER` that is encrypted (`_ENCR`, as Ancestry writes the uploader of
/// a medium) or a server login (under `PhpGedView`'s `_DBID`). A dataset
/// keeps them; only the output leaves them out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Credentials {
    /// Left out (the default).
    #[default]
    LeftOut,
    /// Written as the data holds them.
    Written,
}

/// Whether written bytes start with a byte order mark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Bom {
    /// The default: a mark for GEDCOM 7.x (which recommends one, §1.1) and
    /// for UTF-16 output, none otherwise. [`GedcomWriter::write_to_string`]
    /// never writes one in this mode.
    #[default]
    Auto,
    /// Always a mark (none for the single-byte encodings, which have none).
    Always,
    /// Never a mark.
    Never,
}

/// The character encoding of written bytes, named by the 5.5.1 `HEAD.CHAR`.
///
/// GEDCOM 7.x is always UTF-8: the other encodings apply to 5.5.1 output
/// only, and a 7.x file is written in UTF-8 whatever this says.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OutputEncoding {
    /// UTF-8 (`CHAR UTF-8`), the default.
    #[default]
    Utf8,
    /// UTF-16 little endian (`CHAR UNICODE`).
    Utf16Le,
    /// ANSEL (`CHAR ANSEL`).
    Ansel,
    /// ASCII (`CHAR ASCII`).
    Ascii,
}

impl OutputEncoding {
    /// The `HEAD.CHAR` payload naming this encoding in GEDCOM 5.5.1.
    #[must_use]
    pub fn char_label(self) -> &'static str {
        match self {
            OutputEncoding::Utf8 => "UTF-8",
            OutputEncoding::Utf16Le => "UNICODE",
            OutputEncoding::Ansel => "ANSEL",
            OutputEncoding::Ascii => "ASCII",
        }
    }
}

impl fmt::Display for OutputEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.char_label())
    }
}

/// What the writer does with data that does not fit the target version.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RepairPolicy {
    /// Rewrite it into a conformant form and report a [`Repair`] (the
    /// default).
    #[default]
    Repair,
    /// Fail with [`WriteError::NonConformant`] on the first repair.
    Error,
}

/// A change made so that written data conforms to its version.
///
/// The writer's line emitter reports its rewrites with this type (row (h)
/// of the conformance repair table: identifiers, banned characters, and
/// tags and levels outside the line grammar); the structure-level rows
/// are listed in [`RepairKind`].
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Repair {
    /// The 1-based source line of the structure changed; 0 for a structure
    /// that was not read from a file (typed data), or was synthesised.
    pub line: u32,
    /// The kind of repair: its row of the repair table.
    pub kind: RepairKind,
    /// What was changed, in words (with the output line for line repairs).
    pub detail: Box<str>,
}

impl Repair {
    pub(crate) fn new(line: u32, kind: RepairKind, detail: impl Into<Box<str>>) -> Self {
        Self {
            line,
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Repair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            f.write_str(&self.detail)
        } else {
            write!(f, "line {}: {}", self.line, self.detail)
        }
    }
}

/// The row of the conformance repair table a [`Repair`] applies.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RepairKind {
    /// (a) An enumeration value not in its set.
    EnumValue,
    /// (b) An enumeration value in the wrong case.
    EnumCase,
    /// (c) A pointer missing, misplaced, dangling or to the wrong record type.
    Pointer,
    /// (c) A pointer 7.x prohibits between records (7.0.17): an `ALIA` to
    /// its own record, or one that closes a cycle of pointers between
    /// source, multimedia and shared note records.
    Cycle,
    /// (d) A payload outside its type's grammar.
    Payload,
    /// (e) A structure not permitted where it is, or a malformed tag (also a
    /// structure tagged `CONT`, `CONC`, or `HEAD`/`TRLR` as a record).
    Misplaced,
    /// (f) More occurrences than permitted.
    Repeated,
    /// (g) A required substructure synthesised, or its superstructure made an
    /// extension when none can be.
    Required,
    /// (g) A 7.x structure with neither payload nor substructure.
    Empty,
    /// (h) An identifier renamed or left out.
    Xref,
    /// (h) Banned characters left out (5.5.1 tabs written as spaces, and a
    /// space written as a line break where a 5.5.1 line has no place to
    /// continue with `CONC`).
    Characters,
    /// (h) A structure nested deeper than the version allows, written at
    /// the deepest level.
    Level,
    /// (i) The header or the trailer placed, completed or rebuilt.
    Header,
    /// (j) Text moved out of a 5.5.1 record over 32K.
    RecordSize,
}

/// What a write did besides writing: the repairs it made.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteReport {
    /// Every repair, in output order (identifier renames first).
    pub repairs: Vec<Repair>,
}

/// Why a write failed. Its variants are boxed, so that every write call
/// returns a small `Result`.
#[non_exhaustive]
#[derive(Debug)]
pub enum WriteError {
    /// The underlying writer failed.
    Io(io::Error),
    /// The data needed a repair and the policy is [`RepairPolicy::Error`].
    NonConformant(Box<Repair>),
    /// The output encoding cannot represent a character of the data.
    Unencodable(Box<Unencodable>),
}

/// A character the output encoding cannot represent.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unencodable {
    /// The output encoding.
    pub encoding: OutputEncoding,
    /// The first character it cannot represent.
    pub character: char,
    /// The output line.
    pub line: u64,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Io(e) => write!(f, "write failed: {e}"),
            WriteError::NonConformant(repair) => write!(f, "non-conformant data: {repair}"),
            WriteError::Unencodable(u) => write!(
                f,
                "line {}: {} cannot represent {:?} (U+{:04X})",
                u.line,
                u.encoding,
                u.character,
                u32::from(u.character)
            ),
        }
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WriteError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for WriteError {
    fn from(e: io::Error) -> Self {
        WriteError::Io(e)
    }
}

/// Configuration of a [`GedcomWriter`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriterConfig {
    /// The version to write; `None` (the default) writes the data's own
    /// version: a dataset's ([`Dataset::version`]), or the version records,
    /// a tree or a streamed header declare in `HEAD.GEDC.VERS` (5.5.1 when
    /// they declare none).
    pub version: Option<GedcomVersion>,
    /// The line terminator (default LF).
    pub line_ending: LineEnding,
    /// The longest 5.5.1 line, counting the level, identifier, tag,
    /// payload, delimiters and terminator, in bytes (an upper bound of its
    /// characters). Default and maximum 255; 7.x has no limit.
    pub max_line_length: usize,
    /// Whether to write a byte order mark.
    pub bom: Bom,
    /// The encoding of the bytes [`GedcomWriter::write`] produces, which
    /// `HEAD.CHAR` names (5.5.1 only).
    pub output_encoding: OutputEncoding,
    /// What to do with data that does not fit the target version.
    pub on_nonconformant: RepairPolicy,
    /// How data is converted when the target version is of the other
    /// family than the data's (5.5.1 and 7.x, [`crate::convert`]).
    pub conversion: ConversionOptions,
    /// Whether the credentials a file may hold are written (by default they
    /// are left out).
    pub credentials: Credentials,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            version: None,
            line_ending: LineEnding::Lf,
            max_line_length: 255,
            bom: Bom::Auto,
            output_encoding: OutputEncoding::Utf8,
            on_nonconformant: RepairPolicy::Repair,
            conversion: ConversionOptions::default(),
            credentials: Credentials::LeftOut,
        }
    }
}

/// Writes GEDCOM files. See the [module documentation](self).
#[derive(Clone, Debug, Default)]
pub struct GedcomWriter {
    config: WriterConfig,
}

/// The emitter of a write: the version's rules and the line sink.
struct Emitter<'s, 'w> {
    rules: &'static VersionRules,
    sink: &'s mut LineSink<'w>,
}

impl Emitter<'_, '_> {
    /// Writes one structure (its substructures follow at deeper levels).
    fn put(
        &mut self,
        level: usize,
        xref: Option<&str>,
        tag: &str,
        payload: PayloadRef<'_>,
    ) -> Result<(), WriteError> {
        emit(self.rules, self.sink, level, xref, tag, payload)
    }
}

impl GedcomWriter {
    /// A writer with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A writer with `config`.
    #[must_use]
    pub fn with_config(config: WriterConfig) -> Self {
        Self { config }
    }

    /// Sets the version to write, whatever the data declares.
    #[must_use]
    pub fn gedcom_version(mut self, version: GedcomVersion) -> Self {
        self.config.version = Some(version);
        self
    }

    /// Sets the line terminator.
    #[must_use]
    pub fn line_ending(mut self, ending: LineEnding) -> Self {
        self.config.line_ending = ending;
        self
    }

    /// Sets the longest 5.5.1 line, the whole line and its terminator
    /// counted; values above 255 are taken as 255.
    #[must_use]
    pub fn max_line_length(mut self, length: usize) -> Self {
        self.config.max_line_length = length;
        self
    }

    /// Sets whether to write a byte order mark.
    #[must_use]
    pub fn bom(mut self, bom: Bom) -> Self {
        self.config.bom = bom;
        self
    }

    /// Sets the encoding of the bytes [`write`](Self::write) produces.
    #[must_use]
    pub fn output_encoding(mut self, encoding: OutputEncoding) -> Self {
        self.config.output_encoding = encoding;
        self
    }

    /// Sets what to do with data that does not fit the target version.
    #[must_use]
    pub fn on_nonconformant(mut self, policy: RepairPolicy) -> Self {
        self.config.on_nonconformant = policy;
        self
    }

    /// Sets how data is converted to a version of the other family
    /// ([`crate::convert`]).
    #[must_use]
    pub fn conversion(mut self, options: ConversionOptions) -> Self {
        self.config.conversion = options;
        self
    }

    /// Sets whether the credentials a file may hold are written
    /// ([`Credentials`]; by default they are left out).
    #[must_use]
    pub fn credentials(mut self, credentials: Credentials) -> Self {
        self.config.credentials = credentials;
        self
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &WriterConfig {
        &self.config
    }

    /// This writer, set to write `data` in its own version
    /// ([`Dataset::version`]): what a writer with no version configured
    /// writes a dataset in, whatever its header declares or whether it has
    /// one (a dataset made with [`Dataset::new`] may have none).
    fn in_version_of(&self, data: &Dataset) -> Self {
        self.clone().gedcom_version(data.version())
    }

    /// The rules of the version `data` is written in.
    fn rules_for(&self, declared: Option<&str>) -> &'static VersionRules {
        self.config
            .version
            .or_else(|| declared.map(GedcomVersion::from_version_str))
            .unwrap_or_default()
            .rules()
    }

    /// A sink for `rules`: the configured terminator and line length,
    /// `encoding` and `policy`.
    fn sink<'w>(
        &self,
        writer: Option<&'w mut dyn io::Write>,
        rules: &VersionRules,
        encoding: OutputEncoding,
        policy: RepairPolicy,
    ) -> LineSink<'w> {
        let max_line = rules
            .max_line_len
            .map(|max| max.min(self.config.max_line_length));
        LineSink::new(
            writer,
            self.config.line_ending.as_str(),
            max_line,
            encoding,
            policy,
        )
    }

    /// The encoding of output bytes for `rules`.
    fn encoding(&self, rules: &VersionRules) -> OutputEncoding {
        if rules.head_char {
            self.config.output_encoding
        } else {
            OutputEncoding::Utf8
        }
    }

    fn wants_bom(&self, rules: &VersionRules, encoding: OutputEncoding, to_string: bool) -> bool {
        match self.config.bom {
            Bom::Always => true,
            Bom::Never => false,
            Bom::Auto => !to_string && (!rules.head_char || encoding == OutputEncoding::Utf16Le),
        }
    }

    /// Writes `data` to `writer` in the configured encoding and returns the
    /// repairs made.
    ///
    /// The dataset is written in its version unless one is configured:
    /// values converted to the target's grammars, structures repaired to
    /// conform to its specification ([`crate::spec::conform`]), then every
    /// line emitted by its line rules. The report lists the structural
    /// repairs, then the line repairs.
    ///
    /// # Errors
    ///
    /// [`WriteError::Io`] when `writer` fails, [`WriteError::Unencodable`]
    /// when the output encoding cannot represent the data, and
    /// [`WriteError::NonConformant`] on the first repair under
    /// [`RepairPolicy::Error`].
    pub fn write<W: io::Write>(
        &self,
        mut writer: W,
        data: &Dataset,
    ) -> Result<WriteReport, WriteError> {
        if self.config.version.is_none() {
            return self.in_version_of(data).write(writer, data);
        }
        let rules = self.interpretation(data);
        let interprets = rules.is_some();
        if let Some(rules) = rules.filter(|_| !self.drops_credentials(data))
            && let Some((sink, report)) = self.interpreting(data, rules, None)?
        {
            writer.write_all(&sink.into_bytes())?;
            writer.flush()?;
            return Ok(report);
        }
        if let Some((records, mut report, typed)) = self.rewritten(data, interprets) {
            let more = self.write_rewritten(writer, &records, data, typed)?;
            report.repairs.extend(more.repairs);
            return Ok(report);
        }
        let built = data.built_adopting(self, self.adoptions(data));
        let store = Some(data.store());
        let order = crate::model::field_order;
        if let Some(plan) = self.plan_for(data, &built, false) {
            if let Some((sink, report)) = self.unconverted(data, &plan, &built, None)? {
                writer.write_all(&sink.into_bytes())?;
                writer.flush()?;
                return Ok(report);
            }
            if let Some(plan) = self.plan_for(data, &built, true) {
                return self.write_built(writer, &plan.entries(&built), order, store, None);
            }
        }
        self.write_built(writer, &built, order, store, Some(data))
    }

    /// The records written instead of `data`'s own, and the report so far:
    /// those [`reinterpreted`](Self::reinterpreted) for 7.x, or `data`'s
    /// own structures (`true`: typed, in the model's order); either without
    /// the credentials [`Credentials::LeftOut`] leaves out. `None` when
    /// the dataset is written as it is. `interprets`: whether
    /// [`interpretation`](Self::interpretation) gives rules for `data`.
    fn rewritten(
        &self,
        data: &Dataset,
        interprets: bool,
    ) -> Option<(Vec<Structure>, WriteReport, bool)> {
        let credentials = self.drops_credentials(data);
        if let Some((mut records, report)) = interprets.then(|| self.reinterpreted(data)).flatten()
        {
            if credentials {
                for r in &mut records {
                    drop_credentials(r);
                }
            }
            return Some((records, report, false));
        }
        if !credentials {
            return None;
        }
        let mut records = data.to_structures();
        let dropped: usize = records.iter_mut().map(drop_credentials).sum();
        (dropped > 0).then(|| (records, WriteReport::default(), true))
    }

    /// Whether credentials may be left out of `data` ([`Credentials`]).
    fn drops_credentials(&self, data: &Dataset) -> bool {
        self.config.credentials == Credentials::LeftOut
            && holds_credentials(data.store().other_tags())
    }

    /// Writes the records [`rewritten`](Self::rewritten) gave for `data`.
    fn write_rewritten<W: io::Write>(
        &self,
        writer: W,
        records: &[Structure],
        data: &Dataset,
        typed: bool,
    ) -> Result<WriteReport, WriteError> {
        if typed {
            self.write_model_structures(writer, records, data.version(), crate::model::field_order)
        } else {
            self.write_given(writer, records)
        }
    }

    /// The records of a 5.5.1 dataset converted to the 7.x target and
    /// interpreted there for the producer reading found (what only 7.x can
    /// say: `UID`, `SDATE`, `CROP`, associations under events, …), when
    /// that interprets anything ([`crate::profile`]).
    fn reinterpreted(&self, data: &Dataset) -> Option<(Vec<Structure>, WriteReport)> {
        let to = self.config.version?;
        let rules = self.interpretation(data)?;
        // The conversion as the writer makes it, read back to interpret.
        let mut plain = self.clone().output_encoding(OutputEncoding::Utf8);
        plain.config.bom = Bom::Never;
        let (text, report) = plain.write_to_string_plain(data).ok()?;
        let tree = Tree::parse(text);
        let mut records: Vec<crate::profile::draft::Draft<'_>> = tree
            .records()
            .map(crate::profile::draft::Draft::of_ref)
            .collect();
        let interpreted = crate::profile::interpret::interpret_with(&mut records, to, &rules);
        if !interpreted.changed() {
            return None;
        }
        // Typed again, so that the structures come in the model's order
        // (what reading this output back gives).
        let mut text = String::new();
        for r in &records {
            r.write_gedcom(&mut text, 0, to);
        }
        Some((Dataset::parse(text).to_structures(), report))
    }

    /// `data` written in its own version family, record by record, as
    /// [`write`](Self::write) writes it, with `rewrite` rewriting
    /// structures and `looker` looking at each structure and record as
    /// they are written ([`crate::profile::Exporter`]): the sink, in the
    /// configured encoding, and the report. `None` when the write is not
    /// one record at a time (or the looker stopped it).
    pub(crate) fn write_looked<'d>(
        &self,
        data: &'d Dataset,
        rewrite: &'d dyn crate::model::driver::RowRewrite,
        looker: Rc<RefCell<dyn Looker>>,
        head: Structure,
    ) -> Result<Option<(LineSink<'static>, WriteReport)>, WriteError> {
        if self.config.version.is_none() {
            return self
                .in_version_of(data)
                .write_looked(data, rewrite, looker, head);
        }
        if self.interpretation(data).is_some() || self.drops_credentials(data) {
            return Ok(None);
        }
        let built = data.built_with(self, self.adoptions(data), Some(rewrite));
        if self.plan_for(data, &built, false).is_some() {
            return Ok(None);
        }
        let order = crate::model::field_order;
        let watched = Looked {
            looker,
            head: Some(head),
        };
        self.direct_built(
            &built,
            order,
            data.store(),
            None,
            None,
            Some(data),
            Some(watched),
        )
    }

    /// The rules of the producer of a 5.5.1 dataset written in 7.x, when
    /// some rule only says something in 7.x and may apply
    /// ([`crate::profile`]): what is interpreted as it is written.
    fn interpretation(&self, data: &Dataset) -> Option<crate::profile::RuleSet> {
        let to = self.config.version?;
        if !to.is_v7() || data.version().is_v7() {
            return None;
        }
        let detection = data.producer()?;
        let rules = crate::profile::RuleSet::of(detection, data.interpretation());
        (rules.has_v7_only()
            && rules.may_apply(data.store(), data.store().other_tags(), false, true, true))
        .then_some(rules)
    }

    /// `data`, a 5.5.1 dataset written in 7.x, its producer's conventions
    /// that only 7.x says interpreted with `rules` as each record is
    /// written ([`crate::convert::Watch::interpreting`]): a held sink and
    /// the report, or `None` when a record's interpretation reaches
    /// outside it (a change in another record, an extension to declare in
    /// the header), or the header's would: the whole dataset is then
    /// interpreted at once ([`Self::reinterpreted`]). `text`: as
    /// [`Self::unconverted`] says.
    fn interpreting(
        &self,
        data: &Dataset,
        rules: crate::profile::RuleSet,
        text: Option<usize>,
    ) -> Result<Option<(LineSink<'static>, WriteReport)>, WriteError> {
        let built = data.built_adopting(self, self.adoptions(data));
        let Some(plan) = self.plan_for(data, &built, false) else {
            return Ok(None);
        };
        let mut watch = plan.watch();
        watch.interpreting(rules);
        let head = match plan.head() {
            Some((_, h)) => watch.interprets(h),
            None => built
                .first()
                .is_some_and(|h| watch.interprets(&crate::convert::built_structure(h))),
        };
        if head {
            return Ok(None);
        }
        self.unconverted_with(data, &plan, &built, text, watch)
    }

    /// `data` written in a version of the other family with its header
    /// converted and its records as they are, as long as none needs
    /// converting ([`Plan::of_dataset`] without a scan, `plan`): a held
    /// sink and the report; `None` when a record needs converting, or the
    /// direct write gives up. `text`: written as text, of about that size.
    ///
    /// A dataset that needs nothing more converted (nearly every dataset
    /// written in its own version family but for the header, and many
    /// across) is so written without a pass of its own: each record is
    /// looked at as it is written ([`crate::convert::Watch`]).
    fn unconverted<'d>(
        &self,
        data: &'d Dataset,
        plan: &Plan,
        built: &'d [crate::model::write::Built<'d>],
        text: Option<usize>,
    ) -> Result<Option<(LineSink<'static>, WriteReport)>, WriteError> {
        self.unconverted_with(data, plan, built, text, plan.watch())
    }

    /// [`Self::unconverted`], its records looked at as `watch` says.
    fn unconverted_with<'d>(
        &self,
        data: &'d Dataset,
        plan: &Plan,
        built: &'d [crate::model::write::Built<'d>],
        text: Option<usize>,
        watch: crate::convert::Watch,
    ) -> Result<Option<(LineSink<'static>, WriteReport)>, WriteError> {
        let watch = std::rc::Rc::new(watch);
        let order = crate::model::field_order;
        let head = plan.head().map(|(i, h)| (i, h.clone()));
        let unconverted = Unconverted { watch, head };
        let out = self.direct_built(
            built,
            order,
            data.store(),
            text,
            Some(unconverted),
            Some(data),
            None,
        )?;
        Ok(out)
    }

    /// What the records of `data` write as the standard structures
    /// converting them makes of their extensions, as they write
    /// themselves ([`crate::convert::Adoptions`]): when a 5.5.1 dataset is
    /// written in 7.x.
    fn adoptions(&self, data: &Dataset) -> Option<&'static crate::convert::Adoptions> {
        let to = self.config.version?;
        (to.is_v7() && !data.version().is_v7()).then_some(&crate::convert::ADOPTED_UP)
    }

    /// The conversion of `data`, whose records are `built`, to the target
    /// version, when it is of the other version family.
    /// Without `scan`, the plan converts the header only
    /// ([`Plan::of_dataset`]).
    fn plan_for<'d, B: Build<'d>>(
        &self,
        data: &'d Dataset,
        built: &[B],
        scan: bool,
    ) -> Option<Plan> {
        let to = self.config.version?;
        Plan::of_dataset(
            data,
            built,
            to,
            self.config.conversion,
            Some(crate::model::field_order(to)),
            scan,
        )
    }

    /// Writes `data` as text: UTF-8 (whatever the output encoding), labelled
    /// `CHAR UTF-8` in 5.5.1, with a byte order mark only under
    /// [`Bom::Always`].
    ///
    /// # Errors
    ///
    /// [`WriteError::NonConformant`] on the first repair under
    /// [`RepairPolicy::Error`].
    pub fn write_to_string(&self, data: &Dataset) -> Result<String, WriteError> {
        self.write_to_string_with_report(data).map(|(text, _)| text)
    }

    /// [`write_to_string`](Self::write_to_string), with the report of the
    /// repairs made.
    ///
    /// # Errors
    ///
    /// As [`write_to_string`](Self::write_to_string).
    pub fn write_to_string_with_report(
        &self,
        data: &Dataset,
    ) -> Result<(String, WriteReport), WriteError> {
        if self.config.version.is_none() {
            return self.in_version_of(data).write_to_string_with_report(data);
        }
        let rules = self.interpretation(data);
        let interprets = rules.is_some();
        if let Some(rules) = rules.filter(|_| !self.drops_credentials(data)) {
            let size = data.store().input_len();
            if let Some((sink, report)) = self.interpreting(data, rules, Some(size))? {
                let text = sink.into_string();
                return Ok((text, report));
            }
        }
        if let Some((records, mut report, typed)) = self.rewritten(data, interprets) {
            let mut text = self.clone().output_encoding(OutputEncoding::Utf8);
            if text.config.bom == Bom::Auto {
                text.config.bom = Bom::Never;
            }
            let mut bytes = Vec::new();
            let more = text.write_rewritten(&mut bytes, &records, data, typed)?;
            report.repairs.extend(more.repairs);
            let text = String::from_utf8(bytes)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
            return Ok((text, report));
        }
        self.write_to_string_plain(data)
    }

    /// [`write_to_string_with_report`](Self::write_to_string_with_report)
    /// without the interpretation a dataset's producer may still need in
    /// 7.x.
    fn write_to_string_plain(&self, data: &Dataset) -> Result<(String, WriteReport), WriteError> {
        let built = data.built_adopting(self, self.adoptions(data));
        let size = data.store().input_len();
        let store = Some(data.store());
        let order = crate::model::field_order;
        if let Some(plan) = self.plan_for(data, &built, false) {
            if let Some((sink, report)) = self.unconverted(data, &plan, &built, Some(size))? {
                let text = sink.into_string();
                // The records converted one at a time are those the whole
                // plan converts, and as it does.
                #[cfg(debug_assertions)]
                if let Some(plan) = self.plan_for(data, &built, true) {
                    let planned =
                        self.write_built_to_string(&plan.entries(&built), order, size, store, None);
                    if let Ok((planned, planned_report)) = planned {
                        debug_assert_eq!(text, planned, "converted alone");
                        debug_assert_eq!(report, planned_report, "converted alone");
                    }
                }
                return Ok((text, report));
            }
            if let Some(plan) = self.plan_for(data, &built, true) {
                return self.write_built_to_string(&plan.entries(&built), order, size, store, None);
            }
        }
        self.write_built_to_string(&built, order, size, store, Some(data))
    }

    /// Writes a lossless [`Tree`] to `writer` in the target version (the
    /// tree's own version unless one is configured): structures that the
    /// version does not permit as they are are first repaired by
    /// [`spec::conform`](crate::spec::conform) (as extension structures,
    /// keeping their data), then every line follows the line rules: header
    /// completed, identifiers valid and unique, text escaped and continued,
    /// a final `TRLR`. The report lists the structural repairs, then the
    /// line repairs.
    ///
    /// # Errors
    ///
    /// As [`write`](Self::write).
    pub fn write_tree<W: io::Write>(
        &self,
        mut writer: W,
        tree: &Tree,
    ) -> Result<WriteReport, WriteError> {
        if self.config.credentials == Credentials::LeftOut && holds_credentials(tree.other_tags()) {
            let mut records = tree.to_structures();
            for r in &mut records {
                drop_credentials(r);
            }
            return self.write_given(writer, &records);
        }
        let rules = self.rules_for(tree.declared_version());
        let given: Vec<_> = tree.records().collect();
        let records = match self.plan_nodes(&given, tree.version(), rules, None) {
            Some(plan) => plan.recs(&given),
            None => given.into_iter().map(Rec::given).collect(),
        };
        self.write_conformed(&mut writer, rules, records, None)
    }

    /// Writes owned records — such as [`Tree::to_structures`] gives — like
    /// [`write_tree`](Self::write_tree). With no version configured, they
    /// are written in the version their `HEAD.GEDC.VERS` declares.
    ///
    /// # Errors
    ///
    /// As [`write`](Self::write).
    pub fn write_structures<W: io::Write>(
        &self,
        writer: W,
        records: &[Structure],
    ) -> Result<WriteReport, WriteError> {
        if self.config.credentials == Credentials::LeftOut && records.iter().any(has_credentials) {
            let mut records = records.to_vec();
            for r in &mut records {
                drop_credentials(r);
            }
            return self.write_given(writer, &records);
        }
        self.write_given(writer, records)
    }

    /// [`write_structures`](Self::write_structures) of `records` as they
    /// are.
    fn write_given<W: io::Write>(
        &self,
        mut writer: W,
        records: &[Structure],
    ) -> Result<WriteReport, WriteError> {
        let rules = self.rules_for_records(records);
        let given: Vec<&Structure> = records.iter().collect();
        let recs = match self.plan_nodes(&given, declared_version(records), rules, None) {
            Some(plan) => plan.recs(&given),
            None => given.into_iter().map(Rec::given).collect(),
        };
        self.write_conformed(&mut writer, rules, recs, None)
    }

    /// The conversion of records given as nodes, in version `from`, to the
    /// version of `rules`, when it is of the other version family.
    fn plan_nodes<'n, N: Node<'n>>(
        &self,
        records: &[N],
        from: GedcomVersion,
        rules: &VersionRules,
        order: Option<&'static FieldOrder>,
    ) -> Option<Plan> {
        // Owned structures of a typed model hold its values converted.
        let values = order.is_none();
        Plan::of_nodes(
            records,
            from,
            rules.version,
            self.config.conversion,
            values,
            order,
        )
    }

    /// [`write_structures`](Self::write_structures) of the owned structures
    /// of a typed model, repaired in its order ([`FieldOrder`]), as the
    /// model itself is written ([`write_built`](Self::write_built)).
    pub(crate) fn write_model_structures<W: io::Write>(
        &self,
        mut writer: W,
        records: &[Structure],
        from: GedcomVersion,
        order: fn(GedcomVersion) -> &'static FieldOrder,
    ) -> Result<WriteReport, WriteError> {
        let rules = self.rules_for_records(records);
        let order = order(rules.version);
        let given: Vec<&Structure> = records.iter().collect();
        let records = match self.plan_nodes(&given, from, rules, Some(order)) {
            Some(plan) => plan.recs(&given),
            None => given.into_iter().map(Rec::given).collect(),
        };
        self.write_conformed(&mut writer, rules, records, Some(order))
    }

    /// The version owned records are written in: the configured one, else
    /// the one their `HEAD.GEDC.VERS` declares.
    fn rules_for_records(&self, records: &[Structure]) -> &'static VersionRules {
        let declared = records
            .iter()
            .find(|r| r.tag == "HEAD")
            .and_then(|h| h.first("GEDC"))
            .and_then(|g| g.first("VERS"))
            .and_then(Structure::text);
        self.rules_for(declared)
    }

    /// Writes records that write their own structures when needed (a
    /// typed dataset) like
    /// [`write_structures`](Self::write_structures).
    /// `order` gives the order of the typed model they come from, in the
    /// target version ([`FieldOrder`]). With the `store` of the dataset
    /// they come from, they are first written one at a time
    /// ([`Self::write_direct`]), the output held until the last.
    fn write_built<'n, W: io::Write>(
        &self,
        mut writer: W,
        built: &'n [impl Build<'n>],
        order: fn(GedcomVersion) -> &'static FieldOrder,
        store: Option<&'n Store>,
        whole: Option<&'n Dataset>,
    ) -> Result<WriteReport, WriteError> {
        if let Some(store) = store
            && let Some((sink, report)) =
                self.direct_built(built, order, store, None, None, whole, None)?
        {
            writer.write_all(&sink.into_bytes())?;
            writer.flush()?;
            return Ok(report);
        }
        let records: Vec<Rec<'n, &'n Structure>> = built.iter().map(|r| Rec::built(r)).collect();
        let rules = self.rules_for_built(&records);
        self.write_conformed(&mut writer, rules, records, Some(order(rules.version)))
    }

    /// The records that write themselves (`built`, of the dataset whose
    /// store is `store`) written one at a time ([`write_direct`]) into a
    /// sink held whole: the sink and the report; `None` when the dataset
    /// needs the whole pass, or (with `watch`) converting. `text`: as text
    /// (UTF-8) of about that size, else in the configured encoding. With
    /// `watch`, the records are those of a dataset written in a version of
    /// the other family: the header converted replaces theirs, if given,
    /// and no other record may need converting
    /// ([`GedcomWriter::unconverted`]). `whole`: the dataset `built` are
    /// the records of, one for one, when they are.
    #[allow(clippy::too_many_arguments)] // The records, how to write them, and what watches them.
    fn direct_built<'n>(
        &self,
        built: &'n [impl Build<'n>],
        order: fn(GedcomVersion) -> &'static FieldOrder,
        store: &'n Store,
        text: Option<usize>,
        watch: Option<Unconverted>,
        whole: Option<&'n Dataset>,
        watched: Option<Looked>,
    ) -> Result<Option<(LineSink<'static>, WriteReport)>, WriteError> {
        let mut records: Vec<Rec<'n, &'n Structure>> =
            built.iter().map(|r| Rec::built(r)).collect();
        // The header given instead of the dataset's.
        let looker = watched.map(|Looked { looker, head }| {
            if let (Some(head), Some(r)) = (head, records.first_mut()) {
                *r = Rec::owned(head);
            }
            looker
        });
        // The header converted.
        let watch = watch.map(|Unconverted { watch, head }| {
            if let Some((at, head)) = head
                && let Some(r) = records.get_mut(at)
            {
                *r = Rec::owned(head);
            }
            watch
        });
        let rules = self.rules_for_built(&records);
        let (encoding, room) = match text {
            // A little more than the input: the header is completed, `@`
            // doubled.
            Some(size) => (
                OutputEncoding::Utf8,
                size.saturating_add(size / 16).saturating_add(256),
            ),
            None => (
                self.encoding(rules),
                store.input_len().saturating_add(store.input_len() / 16),
            ),
        };
        let mut sink = self.sink(None, rules, encoding, RepairPolicy::Repair);
        sink.reserve(room);
        if self.wants_bom(rules, encoding, text.is_some()) {
            sink.bom();
        }
        sink.looker.clone_from(&looker);
        let order = order(rules.version);
        let Some(conformed) = write_direct(
            rules,
            &mut sink,
            &mut records,
            order,
            store,
            watch,
            whole,
            looker.as_ref(),
        )?
        else {
            return Ok(None);
        };
        let lines = std::mem::take(&mut sink.repairs);
        let report = self.direct_report(conformed, lines)?;
        Ok(Some((sink, report)))
    }

    /// The report of a direct write ([`Self::write_direct`]): the repairs
    /// of the conformance pass, then those of the lines, as the whole pass
    /// reports them; the first of them as an error under
    /// [`RepairPolicy::Error`].
    fn direct_report(
        &self,
        mut conformed: Vec<Repair>,
        mut lines: Vec<Repair>,
    ) -> Result<WriteReport, WriteError> {
        if self.config.on_nonconformant == RepairPolicy::Error {
            if let Some(first) = conformed.into_iter().chain(lines).next() {
                return Err(WriteError::NonConformant(Box::new(first)));
            }
            return Ok(WriteReport::default());
        }
        conformed.append(&mut lines);
        Ok(WriteReport { repairs: conformed })
    }

    /// [`write_built`](Self::write_built) as text, as
    /// [`write_to_string`](Self::write_to_string) writes a dataset.
    /// `size` is about the size of the output (the size of the input the
    /// records were read from), the room the text starts with.
    fn write_built_to_string<'n>(
        &self,
        built: &'n [impl Build<'n>],
        order: fn(GedcomVersion) -> &'static FieldOrder,
        size: usize,
        store: Option<&'n Store>,
        whole: Option<&'n Dataset>,
    ) -> Result<(String, WriteReport), WriteError> {
        if let Some(store) = store {
            if let Some((sink, report)) =
                self.direct_built(built, order, store, Some(size), None, whole, None)?
            {
                return Ok((sink.into_string(), report));
            }
            #[cfg(test)]
            RESTARTS.with(|r| r.set(r.get() + 1));
        }
        let mut records: Vec<Rec<'n, &'n Structure>> =
            built.iter().map(|r| Rec::built(r)).collect();
        let rules = self.rules_for_built(&records);
        // A little more than the input: the header is completed, `@` doubled.
        let room = size.saturating_add(size / 16).saturating_add(256);
        let mut conformed =
            conform_records(&mut records, rules.version, Some(order(rules.version)));
        let mut repairs = std::mem::take(&mut conformed.repairs);
        if self.config.on_nonconformant == RepairPolicy::Error {
            if let Some(first) = repairs.into_iter().next() {
                return Err(WriteError::NonConformant(Box::new(first)));
            }
            repairs = Vec::new();
        }
        let mut sink = self.sink(
            None,
            rules,
            OutputEncoding::Utf8,
            self.config.on_nonconformant,
        );
        sink.reserve(room);
        if self.wants_bom(rules, OutputEncoding::Utf8, true) {
            sink.bom();
        }
        write_records(
            rules,
            &mut sink,
            &mut records,
            &conformed.arena,
            Some(&|x| conformed.uses(x)),
        )?;
        repairs.append(&mut sink.repairs);
        Ok((sink.into_string(), WriteReport { repairs }))
    }

    /// [`write_to_string_with_report`](Self::write_to_string_with_report)
    /// through the whole conformance pass, never one record at a time:
    /// what the direct write must equal.
    #[cfg(test)]
    pub(crate) fn write_to_string_whole(
        &self,
        data: &Dataset,
    ) -> Result<(String, WriteReport), WriteError> {
        if self.config.version.is_none() {
            return self.in_version_of(data).write_to_string_whole(data);
        }
        let built = data.built(self);
        let order = crate::model::field_order;
        match self.plan_for(data, &built, true) {
            Some(plan) => self.write_built_to_string(&plan.entries(&built), order, 0, None, None),
            None => self.write_built_to_string(&built, order, 0, None, None),
        }
    }

    /// [`rules_for_records`](Self::rules_for_records) of records that write
    /// themselves.
    fn rules_for_built<'n, N: Node<'n>>(&self, records: &[Rec<'n, N>]) -> &'static VersionRules {
        if let Some(version) = self.config.version {
            return version.rules();
        }
        let head = records.iter().find(|r| r.tag() == "HEAD");
        // A typed header says it without writing itself.
        if let Some(declared) = head.and_then(Rec::declared_version) {
            return self.rules_for(Some(&declared));
        }
        let mut scratch = Flat::default();
        let declared = head.and_then(|h| {
            with_node!(h.get(&Flat::default()), scratch, |h| h
                .children()
                .find(|c| c.tag() == "GEDC")
                .and_then(|g| g.children().find(|c| c.tag() == "VERS"))
                .and_then(|v| match v.payload() {
                    PayloadRef::Text(t) => Some(t.to_string()),
                    _ => None,
                }))
        });
        self.rules_for(declared.as_deref())
    }

    /// Repairs `records` for `rules` ([`crate::spec::conform`]), then writes
    /// them.
    fn write_conformed<'n, N: Node<'n>>(
        &self,
        writer: &mut dyn io::Write,
        rules: &'static VersionRules,
        mut records: Vec<Rec<'n, N>>,
        order: Option<&'static FieldOrder>,
    ) -> Result<WriteReport, WriteError> {
        let mut conformed = conform_records(&mut records, rules.version, order);
        let repairs = std::mem::take(&mut conformed.repairs);
        let known = |x: &str| conformed.uses(x);
        let arena = &conformed.arena;
        if self.config.on_nonconformant == RepairPolicy::Error {
            if let Some(first) = repairs.into_iter().next() {
                return Err(WriteError::NonConformant(Box::new(first)));
            }
            return self.write_nodes(writer, rules, &mut records, arena, &known);
        }
        let mut report = self.write_nodes(writer, rules, &mut records, arena, &known)?;
        report.repairs.splice(0..0, repairs);
        Ok(report)
    }

    fn write_nodes<'n, N: Node<'n>>(
        &self,
        writer: &mut dyn io::Write,
        rules: &'static VersionRules,
        records: &mut [Rec<'n, N>],
        arena: &Flat<'n>,
        known: &dyn Fn(&str) -> bool,
    ) -> Result<WriteReport, WriteError> {
        let encoding = self.encoding(rules);
        let mut sink = self.sink(Some(writer), rules, encoding, self.config.on_nonconformant);
        if self.wants_bom(rules, encoding, false) {
            sink.bom();
        }
        write_records(rules, &mut sink, records, arena, Some(known))?;
        sink.finish()?;
        Ok(WriteReport {
            repairs: sink.repairs,
        })
    }
}

/// The extension tags that tell a file holds credentials
/// ([`Credentials`]).
const CREDENTIAL_TAGS: [&str; 3] = ["_PASS", "_ENCR", "_DBID"];

/// Whether a file whose extension tags are `tags` may hold credentials.
pub(crate) fn holds_credentials<'t>(mut tags: impl Iterator<Item = &'t str>) -> bool {
    tags.any(|t| CREDENTIAL_TAGS.contains(&t))
}

/// Whether `s` holds credentials ([`Credentials`]).
fn has_credentials(s: &Structure) -> bool {
    CREDENTIAL_TAGS.iter().any(|t| s.tag == *t) || s.substructures.iter().any(has_credentials)
}

/// Leaves out the credentials `s` holds ([`Credentials`]):
/// `_PASS`, and an `_USER` encrypted (`_ENCR`) or under `_DBID`. The
/// number left out.
pub(crate) fn drop_credentials(s: &mut Structure) -> usize {
    let login = s.tag == "_DBID";
    let before = s.substructures.len();
    s.substructures.retain(|c| {
        !(c.tag == "_PASS" || (c.tag == "_USER" && (login || c.first("_ENCR").is_some())))
    });
    let mut dropped = before - s.substructures.len();
    for c in &mut s.substructures {
        dropped += drop_credentials(c);
    }
    dropped
}

/// [`drop_credentials`] of a draft ([`crate::profile`]).
pub(crate) fn drop_draft_credentials(s: &mut crate::profile::draft::Draft<'_>) -> usize {
    let login = s.tag == "_DBID";
    let before = s.substructures.len();
    s.substructures.retain(|c| {
        !(c.tag == "_PASS" || (c.tag == "_USER" && (login || c.first("_ENCR").is_some())))
    });
    let mut dropped = before - s.substructures.len();
    for c in &mut s.substructures {
        dropped += drop_draft_credentials(c);
    }
    dropped
}

/// The version owned records declare (`HEAD.GEDC.VERS`), 5.5.1 when they
/// declare none.
fn declared_version(records: &[Structure]) -> GedcomVersion {
    records
        .iter()
        .find(|r| r.tag == "HEAD")
        .and_then(|h| h.first("GEDC"))
        .and_then(|g| g.first("VERS"))
        .and_then(Structure::text)
        .map(GedcomVersion::from_version_str)
        .unwrap_or_default()
}

/// What a pointer is written as: [`XrefMap::pointer`] for a whole
/// dataset, [`stream::StreamXrefs::pointer`] record by record.
pub(crate) trait Pointers {
    /// The identifier `pointer` is written with.
    fn pointer<'p>(&mut self, pointer: &'p str) -> Cow<'p, str>;
}

impl Pointers for XrefMap<'_> {
    fn pointer<'p>(&mut self, pointer: &'p str) -> Cow<'p, str> {
        XrefMap::pointer(self, pointer)
    }
}

/// What a direct write is looked at with ([`GedcomWriter::write_looked`]),
/// and the header written instead of the dataset's.
struct Looked {
    looker: Rc<RefCell<dyn Looker>>,
    head: Option<Structure>,
}

/// A dataset written in a version of the other family as it is
/// ([`GedcomWriter::unconverted`]): what its records are looked at for,
/// and its header converted with its index, when the conversion changes it.
struct Unconverted {
    watch: std::rc::Rc<crate::convert::Watch>,
    head: Option<(usize, Structure)>,
}

/// Whether a record is the trailer the writer replaces by its own: an
/// empty `TRLR`.
fn is_trailer<'n, N: Node<'n>>(r: &Rec<'n, N>) -> bool {
    r.tag() == "TRLR" && r.is_empty()
}

/// The prefix of the identifiers generated for a standard record without
/// one; `None` for other records, which need none.
pub(crate) fn record_prefix(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "INDI" => "I",
        "FAM" => "F",
        "SOUR" => "S",
        "REPO" => "R",
        "OBJE" => "M",
        "SUBM" => "U",
        "SUBN" => "SUBN",
        "NOTE" | "SNOTE" => "N",
        _ => return None,
    })
}

/// Writes the records of a dataset: the completed header (the first
/// `HEAD`), the other records in order (identifiers mapped), then `TRLR`
/// (in place of any empty one). `known` tells the identifiers of records
/// the conformance repair made valid and unique.
fn write_records<'n, N: Node<'n>>(
    rules: &'static VersionRules,
    sink: &mut LineSink<'_>,
    records: &mut [Rec<'n, N>],
    arena: &Flat<'n>,
    known: Option<&dyn Fn(&str) -> bool>,
) -> Result<(), WriteError> {
    let head_at = records.iter().position(|r| r.tag() == "HEAD");
    let mut head = match head_at.and_then(|i| records.get_mut(i)) {
        Some(h) => h.take_structure(arena),
        None => Structure::new("HEAD"),
    };
    let records = &*records;
    let others = records
        .iter()
        .enumerate()
        .filter(move |&(i, r)| Some(i) != head_at && !is_trailer(r))
        .map(|(_, r)| r);
    let ids = others.clone().map(|r| (record_prefix(r.tag()), r.xref()));
    let mut xrefs = match known {
        Some(known) => XrefMap::conformed(rules, ids, known),
        None => XrefMap::new(rules, ids),
    };
    for repair in std::mem::take(&mut xrefs.repairs) {
        sink.repair(repair)?;
    }

    map_pointers(&mut head, &mut xrefs);
    let mut stub = None;
    let submitter = if head::needs_submitter(&head, rules) {
        let first = others
            .clone()
            .enumerate()
            .find(|(_, r)| r.tag() == "SUBM")
            .and_then(|(i, r)| xrefs.record(i, r.xref()).map(Cow::into_owned));
        Some(first.unwrap_or_else(|| {
            let xref = xrefs.fresh("U");
            stub = Some(head::stub_submitter(xref.clone()));
            xref
        }))
    } else {
        None
    };
    head::complete(&mut head, rules, sink.charset(), submitter);
    let mut out = Emitter { rules, sink };
    put_structure(&mut out, &head)?;
    if let Some(stub) = stub {
        put_structure(&mut out, &stub)?;
    }

    let (mut given, mut owned) = (Vec::new(), Vec::new());
    let mut scratch = Flat::default();
    for (index, record) in others.enumerate() {
        let xref = xrefs.record(index, record.xref());
        let xref = xref.as_deref();
        match record.get(arena) {
            RecRef::Given(n) => put_record(&mut out, &mut xrefs, xref, n, &mut given)?,
            RecRef::Owned(s) => put_record(&mut out, &mut xrefs, xref, s, &mut owned)?,
            RecRef::Built(b) => {
                scratch.clear();
                b.build(&mut scratch);
                if let Some(root) = scratch.root() {
                    put_flat(&mut out, &mut xrefs, xref, root)?;
                }
            }
            RecRef::Kept(root) => put_flat(&mut out, &mut xrefs, xref, root)?,
        }
    }
    out.sink.source_line = 0;
    out.put(0, None, "TRLR", PayloadRef::None)
}

#[cfg(test)]
thread_local! {
    /// The direct writes of this thread that started again with the whole
    /// conformance pass.
    pub(crate) static RESTARTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The records of this thread's direct writes converted on their own
    /// as owned structures.
    pub(crate) static ALONE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Writes a typed dataset's records (the header first, the trailer last)
/// one at a time, each checked and, when its repair stays inside it,
/// repaired just before it is written ([`Direct`]): what
/// [`write_records`] writes once the whole conformance pass is done, with
/// the pass's repairs. `None` when the dataset needs the whole pass: what
/// was written is to be thrown away.
#[allow(clippy::too_many_arguments)] // The records, how to write them, and who looks.
fn write_direct<'n>(
    rules: &'static VersionRules,
    sink: &mut LineSink<'_>,
    records: &mut Vec<Rec<'n, &'n Structure>>,
    order: &'static FieldOrder,
    store: &'n Store,
    watch: Option<std::rc::Rc<crate::convert::Watch>>,
    whole: Option<&'n Dataset>,
    looker: Option<&Rc<RefCell<dyn Looker>>>,
) -> Result<Option<Vec<Repair>>, WriteError> {
    let utf8 = sink.encoding() == OutputEncoding::Utf8;
    let stream = utf8.then_some(&*sink);
    let Some(mut direct) =
        Direct::prepare(records, rules.version, Some(order), store, stream, whole)
    else {
        return Ok(None);
    };
    let alone = watch.clone();
    if let Some(watch) = watch {
        direct.watch(watch);
    }
    let last = records.len().saturating_sub(1);
    // Identifiers for the standard records without one, avoiding those of
    // the others.
    let missing: Vec<(usize, &'static str)> = records
        .get(1..last)
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .filter(|(_, r)| r.xref().is_none())
        .filter_map(|(i, r)| record_prefix(r.tag()).map(|p| (i, p)))
        .collect();
    let ids = direct.ids();
    let known = move |x: &str| ids.uses(x);
    let mut xrefs = XrefMap::missing(rules, missing.into_iter(), &known);
    let mut out = Emitter { rules, sink };
    if let Some(lines) = direct.head_lines() {
        // Completed and written as lines already.
        out.sink.append(lines)?;
        if let Some(stub) = direct.take_first() {
            let mut stub: Rec<'n, &'n Structure> = Rec::owned(stub);
            if direct.record(0, &mut stub, None) != Some(Place::Record) {
                return Ok(None);
            }
            if let Some(s) = stub.as_owned() {
                put_record(&mut out, &mut xrefs, s.xref.as_deref(), s, &mut Vec::new())?;
            }
        }
    } else {
        let Some(mut head) = take_head(records) else {
            return Ok(None);
        };
        // The header's pointers name records, whose identifiers the pass
        // made valid: the map leaves them as they are.
        if head::needs_submitter(&head, rules) {
            return Ok(None);
        }
        head::complete(&mut head, rules, out.sink.charset(), None);
        put_clean(&mut out, &head)?;
    }
    for i in 1..last {
        let Some(record) = records.get_mut(i) else {
            break;
        };
        if let Some(looker) = &looker {
            looker.borrow_mut().begin(i);
        }
        let place = place_record(
            &mut direct,
            &mut out,
            &mut xrefs,
            record,
            i,
            alone.as_deref(),
        );
        let Some(place) = place else {
            return Ok(None);
        };
        if place != Place::Lines {
            let xref = xrefs.record(i - 1, record.xref());
            let xref = xref.as_deref();
            match (place, record.as_structure()) {
                (Place::Arena, _) => {
                    if let Some(root) = direct.root() {
                        put_flat(&mut out, &mut xrefs, xref, root)?;
                    }
                }
                (Place::Record, Some(s)) => {
                    put_record(&mut out, &mut xrefs, xref, s, &mut Vec::new())?;
                    // A record converted is not kept once written.
                    *record = Rec::owned(Structure::default());
                }
                (Place::Record, None) | (Place::Lines, _) => return Ok(None),
            }
        }
        if let Some(looker) = &looker
            && !looker.borrow_mut().end(i, place == Place::Record)
        {
            return Ok(None);
        }
    }
    out.sink.source_line = 0;
    out.put(0, None, "TRLR", PayloadRef::None)?;
    Ok(Some(direct.into_repairs()))
}

/// Checks record `i` (`record`) of a direct write ([`write_direct`]):
/// where it is then. A record that needs converting is converted alone
/// when that changes nothing outside it (most conversions: `_UID` as
/// `UID`, a role as a 7.x `ROLE`), and checked again: the dataset is
/// planned whole only for the others.
fn place_record<'n>(
    direct: &mut Direct<'n>,
    out: &mut Emitter<'_, '_>,
    xrefs: &mut XrefMap<'_>,
    record: &mut Rec<'n, &'n Structure>,
    i: usize,
    watch: Option<&crate::convert::Watch>,
) -> Option<Place> {
    let mut converted = false;
    loop {
        // The identifier of a record that writes itself, for its lines.
        let built = if record.is_built() {
            xrefs.record(i - 1, record.built_xref())
        } else {
            None
        };
        // A record that writes itself streams its lines right after the
        // output, kept when they are its own (`Place::Lines`).
        let lent = match (record.is_built(), direct.lines()) {
            (true, Some(lines)) => {
                out.sink.lend(lines);
                true
            }
            _ => false,
        };
        let place = if converted {
            direct.record_converted(i, record, built)
        } else {
            direct.record(i, record, built)
        };
        let alone = match (&place, watch) {
            (None, Some(w)) if !converted && direct.wants_converting() => {
                #[cfg(test)]
                ALONE.with(|n| n.set(n.get() + 1));
                let mut s = record.to_structure(direct.c_arena());
                w.rewrite_alone(&mut s).map(|changed| changed.then_some(s))
            }
            _ => None,
        };
        if lent && let Some(lines) = direct.lines() {
            let keep = place == Some(Place::Lines);
            #[cfg(debug_assertions)]
            if keep {
                assert_lines(out.rules, out, xrefs, record, i - 1, lines);
            }
            out.sink.take_back(lines, keep);
        }
        match alone {
            Some(s) => {
                if let Some(s) = s {
                    *record = Rec::owned(s);
                }
                converted = true;
            }
            None => return place,
        }
    }
}

/// Checks, in debug builds, that the lines a record wrote as it wrote
/// itself ([`Place::Lines`]) are those the writer writes of its
/// structures.
#[cfg(debug_assertions)]
fn assert_lines<'n>(
    rules: &'static VersionRules,
    out: &Emitter<'_, '_>,
    xrefs: &mut XrefMap<'_>,
    record: &Rec<'n, &'n Structure>,
    index: usize,
    lines: &LineSink<'_>,
) {
    let mut expected = out.sink.record_lines();
    // (What the record wrote was looked at already.)
    expected.looker = None;
    let mut flat = Flat::default();
    let mut scratch = Flat::default();
    let xref = xrefs.record(index, record.xref()).map(Cow::into_owned);
    let mut check = Emitter {
        rules,
        sink: &mut expected,
    };
    let written = with_node!(record.get(&scratch), flat, |n| put_record(
        &mut check,
        xrefs,
        xref.as_deref(),
        n,
        &mut Vec::new()
    ));
    scratch.clear();
    assert!(written.is_ok(), "{written:?}");
    assert_eq!(expected.text(), lines.text());
}

/// Writes a record, with the identifier `xref`, and its substructures,
/// pointers mapped. `stack` is room for the walk, kept from record to
/// record.
fn put_record<'a, M: Node<'a>>(
    out: &mut Emitter<'_, '_>,
    xrefs: &mut dyn Pointers,
    xref: Option<&str>,
    record: M,
    stack: &mut Vec<(usize, M::Children)>,
) -> Result<(), WriteError> {
    put_record_line(out, xrefs, xref, record)?;
    // The substructures, depth first, without recursion.
    stack.clear();
    stack.push((1, record.children()));
    while let Some((level, children)) = stack.last_mut() {
        let level = *level;
        let Some(child) = children.next() else {
            stack.pop();
            continue;
        };
        out.sink.source_line = child.line();
        put_node(
            out,
            xrefs,
            level,
            child.xref(),
            child.tag(),
            child.standard_tag().is_some(),
            child.payload(),
        )?;
        stack.push((level + 1, child.children()));
    }
    Ok(())
}

/// Writes a record written into a flat arena, with the identifier `xref`.
fn put_flat(
    out: &mut Emitter<'_, '_>,
    xrefs: &mut dyn Pointers,
    xref: Option<&str>,
    root: FlatRef<'_>,
) -> Result<(), WriteError> {
    let mut nodes = root.preorder();
    if let Some((_, record)) = nodes.next() {
        put_record_line(out, xrefs, xref, record)?;
        for (level, n) in nodes {
            let standard = n.standard_tag().is_some();
            match n.payload() {
                // A text, its special bytes found as it was written.
                text @ PayloadRef::Text(_) => {
                    let (rules, sink) = (out.rules, &mut *out.sink);
                    emit_known(
                        rules,
                        sink,
                        level,
                        n.xref(),
                        n.tag(),
                        standard,
                        text,
                        n.special(),
                    )?;
                }
                payload => put_node(out, xrefs, level, n.xref(), n.tag(), standard, payload)?,
            }
        }
    }
    Ok(())
}

/// Writes the line of a record, with the identifier `xref`.
fn put_record_line<'a, M: Node<'a>>(
    out: &mut Emitter<'_, '_>,
    xrefs: &mut dyn Pointers,
    xref: Option<&str>,
    record: M,
) -> Result<(), WriteError> {
    out.sink.source_line = record.line();
    let tag = match record.tag() {
        "HEAD" | "TRLR" => {
            let to = format!("_{}", record.tag());
            let detail = format!(
                "output line {}: record {} written as {to}",
                out.sink.line_no(),
                record.tag()
            );
            out.sink
                .repair(Repair::new(record.line(), RepairKind::Misplaced, detail))?;
            Cow::Owned(to)
        }
        tag => Cow::Borrowed(tag),
    };
    out.sink.source_line = record.line();
    let standard = matches!(tag, Cow::Borrowed(_)) && record.standard_tag().is_some();
    put_node(out, xrefs, 0, xref, &tag, standard, record.payload())
}

/// Writes one structure with its pointer mapped.
fn put_node(
    out: &mut Emitter<'_, '_>,
    xrefs: &mut dyn Pointers,
    level: usize,
    xref: Option<&str>,
    tag: &str,
    standard: bool,
    payload: PayloadRef<'_>,
) -> Result<(), WriteError> {
    let (rules, sink) = (out.rules, &mut *out.sink);
    match payload {
        PayloadRef::Pointer(p) => {
            let p = xrefs.pointer(p);
            // Mapped, it is valid.
            emit_known(
                rules,
                sink,
                level,
                xref,
                tag,
                standard,
                PayloadRef::Pointer(&p),
                Some(0),
            )
        }
        payload => emit_tagged(rules, sink, level, xref, tag, standard, payload),
    }
}

/// Writes an owned structure and its substructures, as they are.
fn put_structure(out: &mut Emitter<'_, '_>, root: &Structure) -> Result<(), WriteError> {
    out.put(
        0,
        root.xref.as_deref(),
        root.tag.as_str(),
        root.payload.borrowed(),
    )?;
    let mut stack = vec![(1_usize, root.substructures.iter())];
    while let Some((level, children)) = stack.last_mut() {
        let level = *level;
        let Some(child) = children.next() else {
            stack.pop();
            continue;
        };
        out.put(
            level,
            child.xref.as_deref(),
            child.tag.as_str(),
            child.payload.borrowed(),
        )?;
        stack.push((level + 1, child.substructures.iter()));
    }
    Ok(())
}

/// [`put_structure`] of a structure the conformance pass found clean: its
/// tags are in the grammar, only the record has an identifier and its
/// pointers name records.
fn put_clean(out: &mut Emitter<'_, '_>, root: &Structure) -> Result<(), WriteError> {
    let mut stack = vec![(0_usize, std::slice::from_ref(root).iter())];
    while let Some((level, children)) = stack.last_mut() {
        let level = *level;
        let Some(s) = children.next() else {
            stack.pop();
            continue;
        };
        let payload = s.payload.borrowed();
        let special = payload.as_str().map_or(0, special_bytes);
        debug_assert!(
            !matches!(payload, PayloadRef::Pointer(p) if !emit::is_valid_pointer(out.rules, p))
        );
        emit::emit_typed(
            out.rules,
            out.sink,
            level,
            s.xref.as_deref(),
            s.tag.as_str(),
            s.tag.standard_index(),
            payload,
            special,
        )?;
        stack.push((level + 1, s.substructures.iter()));
    }
    Ok(())
}

/// Maps every pointer of an owned structure.
fn map_pointers(root: &mut Structure, xrefs: &mut dyn Pointers) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if let crate::tree::Payload::Pointer(p) = &node.payload {
            let mapped = xrefs.pointer(p.as_str()).into_owned();
            node.payload = crate::tree::Payload::Pointer(mapped.into());
        }
        stack.extend(node.substructures.iter_mut());
    }
}
