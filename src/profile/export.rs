//! Export shaped for an importer.
//!
//! An export profile only ever writes conformant GEDCOM. It may do four
//! things (REPORT §7.1):
//!
//! 1. choose among standard alternatives: version, character set, byte
//!    order mark, line terminator and length, record order, place format;
//! 2. fold data into standard structures the importer reads (several
//!    occupations into one, a name's other surnames into its `SURN` list,
//!    an event's age into its note, …);
//! 3. add `_` extensions the importer understands, next to the standard
//!    form they repeat (`_FREL`/`_MREL`, `_PRIM`, `_PHOTO`, `_SHAR`);
//! 4. leave data out, only when asked ([`ExportOptions`]).
//!
//! Every fold that loses something (a type, a structure, a pointer made
//! text) is listed in the [`ExportReport`] with the record it was in and
//! the row of the research it follows; so are the importer's limits the
//! output exceeds. Nothing is printed.
//!
//! The folds rewrite drafts of the records, which borrow the dataset's
//! texts. They run in the profile's order, as if each ran on every record
//! before the next; but folds that read nothing of other records that the
//! folds beside them change run record after record, each record going
//! through all of them at once. Conformance looks again only at what the
//! folds changed since it last looked.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::ops::Range;
use std::rc::Rc;

use crate::convert::{ConversionOptions, convert_structures};
use crate::model::driver::WriteCx;
use crate::model::{Dataset, RecordRef};
use std::borrow::Cow;

use crate::GedcomVersion;
use crate::spec::conform::{Checked, Rec, conform_records};
use crate::tree::{Flat, STANDARD_TAGS, Structure, Xref, standard_index, standard_index_const};
use crate::writer::{
    Bom, Credentials, GedcomWriter, LineEnding, LineSink, OutputEncoding, Repair, WriteError,
    XrefIndex,
};

use super::ExportProfile;
use super::draft::{Draft, Value};
use super::util::{
    child, child_mut, child_text, given_part, has, is, name_value, payload_str, pointer_node,
    retag, set_text, suffix_part, surname_part, text_node,
};

/// How written files name their media files.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MediaPaths {
    /// As the data holds them.
    #[default]
    Keep,
    /// Relative to [`ExportOptions::media_root`] when it is given, with
    /// `/` as the separator (`C:\photos\a.jpg` → `photos/a.jpg`, without
    /// the drive).
    Relative,
    /// The file name alone.
    FileName,
}

/// What an export profile writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub(crate) version: GedcomVersion,
    pub(crate) encoding: OutputEncoding,
    pub(crate) bom: bool,
    pub(crate) line_ending: LineEnding,
    pub(crate) max_line_length: usize,
    pub(crate) media: MediaPaths,
    /// Importer limits, reported when exceeded (never enforced).
    pub(crate) max_bytes: Option<u64>,
    pub(crate) max_individuals: Option<usize>,
    pub(crate) max_depth: Option<usize>,
    pub(crate) max_name: Option<usize>,
    pub(crate) max_children: Option<usize>,
    /// The rows of `export.tsv` the target follows.
    pub(crate) rows: &'static [&'static str],
}

impl Target {
    /// GEDCOM 5.5.1 in UTF-8 without a byte order mark, LF, 255-byte lines:
    /// what most importers read best.
    pub(crate) const V551: Target = Target {
        version: GedcomVersion::V5_5_1,
        encoding: OutputEncoding::Utf8,
        bom: false,
        line_ending: LineEnding::Lf,
        max_line_length: 255,
        media: MediaPaths::Keep,
        max_bytes: None,
        max_individuals: None,
        max_depth: None,
        max_name: None,
        max_children: None,
        rows: &[],
    };

    /// GEDCOM 7.0, UTF-8 with a byte order mark, LF.
    pub(crate) const V70: Target = Target {
        version: GedcomVersion::V7_0,
        bom: true,
        ..Target::V551
    };

    /// The version written.
    #[must_use]
    pub fn version(&self) -> GedcomVersion {
        self.version
    }

    /// The encoding written (5.5.1; 7.x is UTF-8).
    #[must_use]
    pub fn encoding(&self) -> OutputEncoding {
        self.encoding
    }

    /// Whether a byte order mark is written.
    #[must_use]
    pub fn bom(&self) -> bool {
        self.bom
    }

    /// The line terminator.
    #[must_use]
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// The longest line (5.5.1).
    #[must_use]
    pub fn max_line_length(&self) -> usize {
        self.max_line_length
    }

    /// How media file paths are written.
    #[must_use]
    pub fn media(&self) -> MediaPaths {
        self.media
    }

    /// The largest file the importer takes, when it says.
    #[must_use]
    pub fn max_bytes(&self) -> Option<u64> {
        self.max_bytes
    }

    /// The most individuals the importer takes, when it says.
    #[must_use]
    pub fn max_individuals(&self) -> Option<usize> {
        self.max_individuals
    }
}

/// What the user asks of an export beyond its profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Write this version instead of the profile's (for an importer that
    /// reads both, such as Heredis's GEDZIP import of 7.0).
    pub version: Option<GedcomVersion>,
    /// Leave out multimedia links and records (for sites that drop media).
    pub omit_media: bool,
    /// Leave out the records whose `RESN` says `confidential` or `privacy`,
    /// and the pointers to them (for importers that ignore `RESN`).
    pub omit_private: bool,
    /// Leave out `CHAN` (`RootsMagic`).
    pub omit_change_dates: bool,
    /// The individual to write first (the home person of Ancestry and
    /// Legacy); default: the header's `_ROOT`, else as the data says.
    pub home: Option<String>,
    /// The folder media paths are made relative to.
    pub media_root: Option<String>,
    /// Whether the credentials the data holds are written ([`Credentials`];
    /// by default they are left out, and the report says so).
    pub credentials: Credentials,
    /// Which system `HEAD.SOUR` names ([`HeadSource`]).
    pub source: HeadSource,
}

/// The system an export names in `HEAD.SOUR`: GEDCOM's "system that
/// produced the file".
///
/// An application that writes its data through this crate produces the
/// file: the `HEAD.SOUR` it gives the dataset (its identifier, `VERS`,
/// `NAME`, `CORP`) is written as it is, and the file says it passed through
/// this crate by a `HEAD.SOUR._OXIDGED` naming the crate's version (declared
/// in a 7.x `HEAD.SCHMA`), by which reading it back finds the `oxidged`
/// profile ([`crate::profile`]: what was exported is not interpreted
/// again). A dataset read from a file and written again names the crate:
/// the producer of the file read did not produce this one, and an importer
/// that switches dialects on `HEAD.SOUR` must not take it for that
/// producer's export (EXF-082, EXC-018, EXC-057).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HeadSource {
    /// The dataset's `HEAD.SOUR` when the program set it (it differs from
    /// the one read, or the dataset was not read from a file); else the
    /// crate's.
    #[default]
    Auto,
    /// The crate's, always.
    Library,
    /// The dataset's, whatever it is (the crate's when it has none).
    Dataset,
}

/// What kind of change a fold made.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FoldKind {
    /// Several structures made one.
    Merged,
    /// A structure moved elsewhere.
    Moved,
    /// A structure given another tag or type.
    Relabelled,
    /// A structure added (a redundant extension, an explicit value).
    Added,
    /// Data written as the text of a note or payload.
    AsText,
    /// Data left out.
    Omitted,
    /// Records or structures put in another order.
    Reordered,
    /// A value rewritten (escaped, a path, an identifier).
    Recoded,
}

impl fmt::Display for FoldKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FoldKind::Merged => "merged",
            FoldKind::Moved => "moved",
            FoldKind::Relabelled => "relabelled",
            FoldKind::Added => "added",
            FoldKind::AsText => "as text",
            FoldKind::Omitted => "omitted",
            FoldKind::Reordered => "reordered",
            FoldKind::Recoded => "recoded",
        })
    }
}

/// One change an export made for its importer.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fold {
    /// The rows of `export.tsv` it follows (none for what every export
    /// does: leaving credentials out).
    pub rows: &'static [&'static str],
    /// What kind of change.
    pub kind: FoldKind,
    /// Whether something was lost (a type, a structure, a link).
    pub lossy: bool,
    /// The record it was in.
    pub record: Option<Xref>,
    /// What was changed, in words.
    pub detail: String,
}

impl fmt::Display for Fold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(x) = &self.record {
            write!(f, "{x}: ")?;
        }
        write!(f, "{} ({}", self.detail, self.kind)?;
        if self.lossy {
            f.write_str(", lossy")?;
        }
        if !self.rows.is_empty() {
            write!(f, "; {}", self.rows.join(", "))?;
        }
        f.write_str(")")
    }
}

/// An importer limit the output exceeds.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limit {
    /// The rows of `export.tsv` it comes from.
    pub rows: &'static [&'static str],
    /// The record concerned, if one.
    pub record: Option<Xref>,
    /// The limit and what exceeds it, in words.
    pub detail: String,
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(x) = &self.record {
            write!(f, "{x}: ")?;
        }
        write!(f, "{} ({})", self.detail, self.rows.join(", "))
    }
}

/// What an export did: its folds, the importer limits exceeded and the
/// writer's repairs. A query, not a warnings channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportReport {
    /// Every fold, in the order made.
    pub folds: Vec<Fold>,
    /// The importer limits the output exceeds.
    pub limits: Vec<Limit>,
    /// The writer's conformance repairs (see [`WriteReport`](crate::WriteReport)).
    pub repairs: Vec<Repair>,
}

impl ExportReport {
    /// The folds that lost something.
    pub fn lossy(&self) -> impl Iterator<Item = &Fold> {
        self.folds.iter().filter(|f| f.lossy)
    }

    /// Whether no fold lost anything and no limit is exceeded.
    #[must_use]
    pub fn is_lossless(&self) -> bool {
        self.lossy().next().is_none() && self.limits.is_empty()
    }

    /// The folds that follow row `row`.
    pub fn for_row<'a>(&'a self, row: &'a str) -> impl Iterator<Item = &'a Fold> {
        self.folds.iter().filter(move |f| f.rows.contains(&row))
    }

    /// The folds made in record `xref`.
    pub fn for_record<'a>(&'a self, xref: &'a str) -> impl Iterator<Item = &'a Fold> {
        self.folds
            .iter()
            .filter(move |f| f.record.as_ref().is_some_and(|x| x.as_str() == xref))
    }
}

/// How the witnesses and other associations of events are written in
/// 5.5.1, which has no place for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AssocStyle {
    /// `_SHAR @I@` + `ROLE` (`RootsMagic`, Legacy, Family Historian); a
    /// witness named in words becomes a note.
    Shar,
    /// `_WITN @I@` (Gramps).
    Witn,
    /// `_ASSO @I@` + `RELA` (GEDCOM-L, webtrees).
    GedcomL,
    /// On the associated person: `ASSO @A@` + `RELA role@INDI:EVENT`
    /// (Ancestris).
    Reversed,
    /// As the event's note (Geneanet, Ancestry, …).
    Note,
}

/// One step of an export profile.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Step {
    pub(crate) fold: Op,
    pub(crate) rows: &'static [&'static str],
}

/// What a step does.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Op {
    /// The undated occupations (no substructure) of a person as one,
    /// comma-joined.
    OccupationsMerged,
    /// `FACT` + `TYPE Profession` (or occupation) as `OCCU`.
    ProfessionFacts,
    /// The surnames of the other names that differ from the primary name by
    /// their surname only, in the primary name's `SURN` list.
    SurnamesPacked,
    /// The surname prefix (`SPFX`) inside the slashes of the name value.
    ParticleInSurname,
    /// Every nickname of a person as one `NICK` under the primary name.
    NicknamesJoined,
    /// The usual given name (`_RUFNAME`, `_USED`) in double quotes in the
    /// name value.
    UsualNameQuoted,
    /// `:` left out of names.
    NoColonInNames,
    /// Events that say nothing more said `Y`.
    EventsSaidY,
    /// A death `Y` for a person buried or cremated without one.
    DeathSaidY,
    /// Report a person with both a baptism and a christening (one is read).
    OneBaptism,
    /// These events and attributes as `EVEN` + `TYPE <label>`.
    AsEven(&'static [(&'static str, &'static str)]),
    /// These attributes as `FACT` + `TYPE <label>` under a person, `EVEN`
    /// under a family.
    AsFact(&'static [(&'static str, &'static str)]),
    /// `FACT` as `EVEN` + `TYPE`.
    FactsAsEven,
    /// The union type said: `NO MARR` as `EVEN TYPE unmarried`, no union
    /// event as `EVEN TYPE nomen`.
    UnionType,
    /// Report an empty PACS, which the importer drops.
    EmptyPacs,
    /// Godparents of a baptism as person-level `ASSO` + `RELA Godfather`
    /// then `Godmother`.
    Godparents,
    /// How the remaining event associations are written.
    EventAssociations(AssocStyle),
    /// An adoption link (`FAMC` + `PEDI adopted`) also as `ADOP` + `FAMC`
    /// + `ADOP BOTH`, and the birth family first.
    Adoptions,
    /// Every citation of a source record written inline, as one text.
    InlineSources,
    /// Every text citation made a pointer to a new source record.
    SourceRecords,
    /// Notes as plain text with `<` and `&` escaped (importers that read
    /// notes as HTML).
    EscapedNotes,
    /// Notes with their HTML markup removed.
    PlainNotes,
    /// One header note.
    OneHeadNote,
    /// Citations of a child link as a note of the person.
    ChildLinkSources,
    /// Places in this format (by the jurisdictions the header names), with
    /// `HEAD.PLAC.FORM` set to it; a subdivision as `[sub] - ` before.
    Places(PlaceStyle),
    /// Event addresses as notes.
    AddressesAsNotes,
    /// Event addresses appended to the place, after ` at `.
    AddressesInPlace,
    /// Report impossible dates (31 FEB).
    BadDates,
    /// Extension structures left after conformance as notes of their
    /// parent (or left out where no note fits).
    ExtensionsAsNotes,
    /// Extension structures directly under a person or family as `EVEN` +
    /// `TYPE <tag>`.
    ExtensionsAsEvents,
    /// Links to multimedia records written inline (5.5.1 `OBJE` + `FILE`).
    InlineMedia,
    /// Each person's and family's associations as `EVEN` + `TYPE
    /// Associate` + a note; with `true`, the associate's name as the
    /// payload.
    AssociationsAsEvents(bool),
    /// Event substructures the importer ignores as labelled notes.
    FieldsAsNotes(&'static [(&'static str, &'static str)]),
    /// A cause of an event other than death as a note.
    CausesAsNotes,
    /// Record-level notes of persons and families as `EVEN` + `TYPE` +
    /// `NOTE`.
    RecordNotesAsEvent(&'static str),
    /// Record-level citations of persons and families on a synthetic `EVEN`.
    RecordCitationsOnEvent(&'static str),
    /// A source's `ABBR` as its title when it has none; `PUBL`, `TEXT`,
    /// `ABBR` in its note.
    SourceFieldsAsNote,
    /// A repository's contact details in its note.
    RepositoryContactsAsNote,
    /// `_FREL` and `_MREL` under `FAM.CHIL`, from the child's `PEDI`.
    ParentRelations,
    /// `_PHOTO @O@` for the preferred image.
    Photo,
    /// These flags on the preferred multimedia link (the first), and with
    /// `true` a `_TYPE PHOTO|OTHER` on each link.
    Preferred(&'static [&'static str], bool),
    /// `NPFX` as a title of the person; a suffix in the name value.
    PrefixAsTitle,
    /// Name values made complete from their pieces; pieces that disagree
    /// with the value in a note.
    NameValueComplete,
    /// The usual given name (`_RUFNAME`, `_USED`) as a nickname.
    UsualNameAsNick,
    /// The usual given name as a note.
    UsualNameAsNote,
    /// These identifiers as a labelled note.
    IdentifiersAsNote(&'static [&'static str]),
    /// An interpreted date's phrase in the event's note.
    InterpretedDatesAsNote,
    /// No line break in the header.
    FlatHead,
    /// `_UID` in the PAF form: 32 hexadecimal digits and a checksum.
    PafUids,
    /// These tags written as these others (`SDATE` as `_SDATE`).
    Retag(&'static [(&'static str, &'static str)]),
    /// `NO MARR` as `_STAT Never Married` (Legacy).
    MarriageStatus,
    /// Citations under notes moved to the note's structure.
    NoteSourcesLifted,
    /// One `EMAIL` (and `WWW` with it) and one `PHON` (and `FAX` with it).
    ContactsJoined,
    /// `FAX` as `PHON`.
    FaxAsPhone,
    /// The home person first.
    HomeFirst,
    /// A note on facts that have no date, place or note (Ancestry drops
    /// them).
    BareFactsNoted,
    /// The other names in one `_AKA` under the primary name (PAF).
    NamesInAka,
    /// Every name with surname slashes.
    NamesWithSlashes,
    /// Record identifiers of at most this many characters.
    ShortXrefs(usize),
    /// Report records without an identifier (dropped by the importer).
    XreflessRecords,
    /// One place per event: the others as notes.
    OnePlace,
    /// A citation's `REFN` moved to its source record.
    CitationRefnToSource,
    /// PACS as `MARR` + `TYPE Civil Union`, `NO MARR` as `MARR` + `TYPE
    /// Unmarried` (Gramps).
    UnionsAsMarriage,
    /// A free `_GODP` beside each baptism's godparents (Ahnenblatt).
    GodparentText,
    /// `SEX U` for a person whose sex is not said in the target (none, or
    /// one 5.5.1 lacks such as `X`): `GeneWeb` reads no `SEX` as unknown.
    SexSaid,
}

/// A place format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlaceStyle {
    /// Geneanet: `[sub] - Town, Area code, County, Region, Country`.
    Geneanet,
    /// Heredis: `Town, Area code, County, Region, Country, Subdivision`.
    Heredis,
}

impl PlaceStyle {
    /// The format, as `HEAD.PLAC.FORM` says it.
    pub(super) fn form(self) -> &'static str {
        match self {
            PlaceStyle::Geneanet => "Town, Area code, County, Region, Country",
            PlaceStyle::Heredis => "Town, Area code, County, Region, Country, Subdivision",
        }
    }

    /// Whether a place follows the format: its jurisdictions, with
    /// Geneanet's `[subdivision] - ` before them.
    pub(super) fn fits(self, place: &str) -> bool {
        let place = match self {
            PlaceStyle::Geneanet if place.trim_start().starts_with('[') => {
                place.split_once("] - ").map_or(place, |(_, rest)| rest)
            }
            _ => place,
        };
        jurisdictions(place) == jurisdictions(self.form())
    }
}

/// The jurisdictions of a place, comma-separated: one more than its commas.
pub(super) fn jurisdictions(place: &str) -> usize {
    1 + place.bytes().filter(|&b| b == b',').count()
}

/// Writes datasets for an importer: see the [`profile` documentation](crate::profile).
///
/// ```rust
/// use oxidged::profile::{Exporter, ExportOptions};
/// use oxidged::Dataset;
///
/// let data = Dataset::parse(
///     "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
///      1 OBJE @M1@\n0 @M1@ OBJE\n1 FILE photo.jpg\n2 FORM image/jpeg\n0 TRLR\n",
/// );
/// let exporter = Exporter::named("ancestry").unwrap().options(ExportOptions {
///     omit_media: true,
///     ..ExportOptions::default()
/// });
/// let (text, report) = exporter.write_to_string(&data).unwrap();
/// assert!(!text.contains("OBJE"));
/// assert!(report.lossy().any(|f| f.rows.contains(&"EXC-002")));
/// ```
#[derive(Clone, Debug)]
pub struct Exporter {
    profile: &'static ExportProfile,
    options: ExportOptions,
}

impl Exporter {
    /// An exporter for `profile`.
    #[must_use]
    pub fn new(profile: &'static ExportProfile) -> Self {
        Self {
            profile,
            options: ExportOptions::default(),
        }
    }

    /// An exporter for the profile named `id` ([`super::export_profile`]).
    #[must_use]
    pub fn named(id: &str) -> Option<Self> {
        super::export_profile(id).map(Self::new)
    }

    /// Sets the options.
    #[must_use]
    pub fn options(mut self, options: ExportOptions) -> Self {
        self.options = options;
        self
    }

    /// The profile.
    #[must_use]
    pub fn profile(&self) -> &'static ExportProfile {
        self.profile
    }

    fn version(&self) -> GedcomVersion {
        self.options.version.unwrap_or(self.profile.target.version)
    }

    fn writer(&self) -> GedcomWriter {
        let t = &self.profile.target;
        GedcomWriter::new()
            .gedcom_version(self.version())
            .line_ending(t.line_ending)
            .max_line_length(t.max_line_length)
            .bom(if t.bom { Bom::Always } else { Bom::Never })
            .output_encoding(t.encoding)
            .credentials(self.options.credentials)
    }

    /// The records the profile writes for `data`, before the writer's
    /// conformance repair, and what was folded.
    #[must_use]
    pub fn structures(&self, data: &Dataset) -> (Vec<Structure>, ExportReport) {
        let (text, report) = self.folded(data, false);
        (Dataset::parse(text).to_structures(), report)
    }

    /// The records the profile writes for `data`, in the order the folds
    /// leave them, as text to type again (so that the writer writes them in
    /// the model's order, what reading the output back gives: exporting is
    /// idempotent), and what was folded; with the importer's limits on the
    /// records when `limits`.
    fn folded(&self, data: &Dataset, limits: bool) -> (String, ExportReport) {
        let to = self.version();
        let structures;
        let mut records: Vec<Draft<'_>> = if data.version() == to {
            structures = Vec::new();
            drafts(data)
        } else {
            let mut owned = data.to_structures();
            let options = ConversionOptions::default();
            convert_structures(&mut owned, data.version(), to, &options);
            structures = owned;
            structures.iter().map(Draft::of_structure).collect()
        };
        let mut report = self.fold_with(data, &mut records, false).report;
        let room = data.store.input().len();
        let text = self.text(records, limits.then_some(&mut report), room);
        drop(structures);
        (text, report)
    }

    /// The folds of `records` (those of `data`, in the target version), in
    /// place, and what they folded. `direct`: for a direct export
    /// ([`Exporter::direct`]), each step runs on every record before the
    /// next, what each fold and limit was reported by is kept, and the
    /// places folded are kept to report with the write's.
    fn fold_with(&self, data: &Dataset, records: &mut Vec<Draft<'_>>, direct: bool) -> Folded {
        let to = self.version();
        if to.is_v7()
            && !data.version().is_v7()
            && let Some(producer) = data.producer()
        {
            let rules = super::RuleSet::of(producer, data.interpretation());
            super::interpret::interpret_with(records, to, &rules);
        }
        let mut cx = Cx {
            v7: to.is_v7(),
            report: ExportReport::default(),
            options: &self.options,
            target: &self.profile.target,
            next_xref: 0,
            place_style: None,
            seq: 0,
            next_seq: 1,
            tags: direct.then(Default::default),
            alone: direct,
            places: direct.then(Default::default),
        };
        let read = data.read_source.as_deref();
        let program = own_source(records, &mut cx, read);
        if self.options.credentials == Credentials::LeftOut {
            omit_credentials(records, &mut cx);
        }
        if self.options.omit_private {
            omit_private(records, &mut cx);
        }
        if self.options.omit_media {
            omit_media(records, &mut cx);
        }
        if self.options.omit_change_dates {
            omit_change_dates(records, &mut cx);
        }
        // Steps in three rounds: the folds; then, once conformance has made
        // extensions of what the target has no place for, the extension
        // folds; last the notes, which the folds before write to.
        let round = step_round;
        let first: Vec<&Step> = self
            .profile
            .steps
            .iter()
            .filter(|s| round(s) == 0)
            .collect();
        run_steps(&first, records, &mut cx, None);
        let later: Vec<&Step> = self
            .profile
            .steps
            .iter()
            .filter(|s| round(s) == 1)
            .collect();
        // What conformance found clean of each record.
        let mut checked = Vec::new();
        if !later.is_empty() {
            let (repairs, found) = conform_drafts(records, to, None);
            cx.report.repairs.extend(repairs);
            checked = found;
        }
        // The extension folds, then the notes: no conformance between them.
        let last: Vec<&Step> = later
            .into_iter()
            .chain(self.profile.steps.iter().filter(|s| round(s) == 2))
            .collect();
        // The records they change.
        let mut touched = vec![false; records.len()];
        #[cfg(debug_assertions)]
        let before = records.clone();
        run_steps(&last, records, &mut cx, Some(&mut touched));
        cx.seq = cx.next_seq;
        media_paths(records, &mut cx, &mut touched);
        #[cfg(debug_assertions)]
        for ((b, r), t) in before.iter().zip(records.iter()).zip(&touched) {
            debug_assert!(*t || b == r, "a record changed untold: {r:?}");
        }
        if let Some(style) = cx.place_style {
            // As the writer repairs them: the places of what conformance
            // makes extensions are not the format's. What it found clean
            // and the steps since left as it was is clean still.
            let known = (!checked.is_empty()).then_some((checked.as_slice(), touched.as_slice()));
            let (repairs, _) = conform_drafts(records, to, known);
            cx.report.repairs.extend(repairs);
            place_form(records, style);
        }
        if program {
            name_the_crate(records, cx.v7);
        }
        let (fold_steps, limit_steps) = cx.tags.unwrap_or_default();
        Folded {
            report: cx.report,
            fold_steps,
            limit_steps,
            places: cx.places,
        }
    }

    /// The order in which the steps of the profile run ([`Cx::seq`]), by
    /// their index: the folds, then those that look at what conformance
    /// made, then the notes.
    fn step_seqs(&self) -> Vec<u16> {
        let steps = &self.profile.steps;
        let mut seqs = vec![0_u16; steps.len()];
        let mut next = 1_u16;
        for round in 0..3 {
            for (k, s) in steps.iter().enumerate() {
                if step_round(s) == round {
                    if let Some(slot) = seqs.get_mut(k) {
                        *slot = next;
                    }
                    next += 1;
                }
            }
        }
        seqs
    }

    /// The export written record by record as the dataset is
    /// ([`super::export_direct`]), when nothing changes a record but the
    /// header, the submitters and the submission (folded as drafts) except
    /// the folds made as it is written: the sink, in the encoding of
    /// `writer`, and the report, the size aside. `None`: the dataset is to
    /// be folded whole.
    fn direct(
        &self,
        data: &Dataset,
        writer: &GedcomWriter,
    ) -> Result<Option<(LineSink<'static>, ExportReport)>, WriteError> {
        use super::export_direct::{Census, Rewrite};
        if !self.direct_fits(data) {
            return Ok(None);
        }
        let Some(front) = self.fold_front(data) else {
            return Ok(None);
        };
        let steps = &self.profile.steps;
        let rewrite = Rewrite::new(data, steps, front.mapping);
        let media = self.profile.target.media != MediaPaths::Keep;
        let census = Census::new(steps, false, media, front.limits, &rewrite);
        let census = Rc::new(RefCell::new(census));
        let looker: Rc<RefCell<dyn crate::writer::Looker>> = census.clone();
        let Some((mut sink, written)) = writer.write_looked(data, &rewrite, looker, front.head)?
        else {
            return Ok(None);
        };
        sink.looker = None;
        drop(rewrite);
        let Ok(census) = Rc::try_unwrap(census).map(RefCell::into_inner) else {
            return Ok(None);
        };
        let (found, limits) = census.into_found();
        if found.whole {
            return Ok(None);
        }
        let mut report = self.direct_report(front.folded, found);
        limits.done(&mut report);
        report.repairs.extend(written.repairs);
        #[cfg(test)]
        DIRECT.with(|n| n.set(n.get() + 1));
        Ok(Some((sink, report)))
    }

    /// Whether `data` may be exported directly ([`Exporter::direct`]): in
    /// the target's version (5.5.1), without the options that leave
    /// records out, with a header, the identifiers valid and unique, and
    /// only the folds a direct export makes or looks out for.
    fn direct_fits(&self, data: &Dataset) -> bool {
        let to = self.version();
        let options = &self.options;
        data.version() == to
            && !to.is_v7()
            && !options.omit_private
            && !options.omit_media
            && !options.omit_change_dates
            && data.header_ref().is_some()
            && self
                .profile
                .steps
                .iter()
                .all(|s| super::export_direct::made_direct(&s.fold))
            && data.xrefs_conform(to.rules())
    }

    /// The header, the submitters and the submissions of `data` folded as
    /// drafts ([`Exporter::direct`]), when they point to no other record
    /// and the folds change none but the header (and make a submitter the
    /// writer makes the same).
    fn fold_front(&self, data: &Dataset) -> Option<Front> {
        let mut front = front_drafts(data);
        let named: Vec<Option<String>> = front
            .iter()
            .map(|r| r.xref.as_deref().map(str::to_string))
            .collect();
        if !front.iter().all(|r| points_within(r, &named)) {
            return None;
        }
        let mapping = place_mapping(&front);
        let given = front.clone();
        let folded = self.fold_with(data, &mut front, true);
        let mut limits = Limits::new(&self.profile.target);
        for r in &front {
            limits.record(r);
        }
        let mut made = None;
        for r in front.iter().skip(1).filter(|r| !is(r, "TRLR")) {
            match given.iter().find(|g| g.xref == r.xref && g.tag == r.tag) {
                Some(g) if g == r => {}
                None if made.is_none() && is(r, "SUBM") && data.submitters().is_empty() => {
                    made = r.xref.as_deref().map(str::to_string);
                }
                _ => return None,
            }
        }
        if made
            .as_deref()
            .is_some_and(|x| data.store().find_xref(x).is_some())
        {
            return None;
        }
        let head = front.first().filter(|h| is(h, "HEAD"))?;
        // Typed again, written as the writer writes a typed header.
        let mut text = String::new();
        let version = child(head, "GEDC")
            .and_then(|g| child_text(g, "VERS"))
            .map_or(GedcomVersion::V5_5_1, GedcomVersion::from_version_str);
        head.write_gedcom(&mut text, 0, version);
        drop(front);
        let typed = Dataset::parse(text);
        let Some(RecordRef::Header(h)) = typed.records().next() else {
            return None;
        };
        let mut head =
            crate::model::write::Built::new(RecordRef::Header(h), self.version()).owned_record()?;
        if let Some(x) = &made {
            // Its pointer: the writer completes the header with its own.
            head.substructures
                .retain(|c| !(c.tag == "SUBM" && c.pointer().is_some_and(|p| p.as_str() == x)));
        }
        Some(Front {
            folded,
            head,
            limits,
            mapping,
        })
    }

    /// The report of a direct export ([`Exporter::direct`]): the front's
    /// (`folded`), and what the write `found`, in the place of the steps
    /// that made them.
    fn direct_report(&self, folded: Folded, found: super::export_direct::Found) -> ExportReport {
        let Folded {
            mut report,
            fold_steps,
            limit_steps,
            places,
        } = folded;
        let steps = &self.profile.steps;
        let seqs = self.step_seqs();
        let seq_of = |op: fn(&Op) -> bool| {
            steps
                .iter()
                .position(|s| op(&s.fold))
                .and_then(|k| Some((seqs.get(k).copied()?, steps.get(k)?.rows)))
        };
        let mut folds: Vec<(u16, Fold)> = report
            .folds
            .drain(..)
            .zip(fold_steps)
            .map(|(f, k)| (k, f))
            .collect();
        let mut kept: Vec<(u16, Limit)> = report
            .limits
            .drain(..)
            .zip(limit_steps)
            .map(|(l, k)| (k, l))
            .collect();
        if let Some((seq, rows)) = seq_of(|op| matches!(op, Op::InlineSources)) {
            folds.reserve(found.inline.len());
            for (record, n) in found.inline {
                let mut detail = String::with_capacity(48);
                super::export_direct::push_count(&mut detail, n);
                detail.push_str(" citations of source records written inline");
                let kind = FoldKind::AsText;
                let fold = Fold {
                    rows,
                    kind,
                    lossy: false,
                    record,
                    detail,
                };
                folds.push((seq, fold));
            }
        }
        let style = steps.iter().find_map(|s| match s.fold {
            Op::Places(style) => Some(style),
            _ => None,
        });
        if let (Some((seq, mut seen)), Some(style)) = (places, style) {
            seen.changed += found.changed;
            seen.guessed += found.guessed;
            seen.skipped += found.skipped;
            let rows = seq_of(|op| matches!(op, Op::Places(_))).map_or(&[][..], |(_, r)| r);
            let (limit, fold) = places_report(rows, style, &seen);
            kept.extend(limit.map(|l| (seq, l)));
            folds.extend(fold.map(|f| (seq, f)));
        }
        folds.sort_by_key(|(k, _)| *k);
        kept.sort_by_key(|(k, _)| *k);
        report.folds = folds.into_iter().map(|(_, f)| f).collect();
        report.limits = kept.into_iter().map(|(_, l)| l).collect();
        report
    }

    /// The records as text, each let go once written; with `report`, the
    /// importer's limits on them reported. `room`: about as long.
    fn text(
        &self,
        records: Vec<Draft<'_>>,
        report: Option<&mut ExportReport>,
        room: usize,
    ) -> String {
        // `@` escaped as the header says: the version the text is read in
        // (the writer converts to the target's).
        let version = records
            .iter()
            .find(|r| is(r, "HEAD"))
            .and_then(|h| child(h, "GEDC"))
            .and_then(|g| child_text(g, "VERS"))
            .map_or(GedcomVersion::V5_5_1, GedcomVersion::from_version_str);
        let mut seen = report.is_some().then(|| self.limits());
        let mut text = String::with_capacity(room.saturating_add(room / 4));
        for r in records {
            if let Some(seen) = &mut seen {
                seen.record(&r);
            }
            r.write_gedcom(&mut text, 0, version);
        }
        if let (Some(seen), Some(report)) = (seen, report) {
            seen.done(report);
        }
        text
    }

    /// The dataset to write for `data`, and the report so far (what was
    /// folded, the limits but the size).
    fn prepared(&self, data: &Dataset) -> (Dataset, ExportReport) {
        let (text, report) = self.folded(data, true);
        (Dataset::parse_as_written(text), report)
    }

    /// Writes `data` for the importer to `writer`.
    ///
    /// # Errors
    ///
    /// As [`GedcomWriter::write`].
    pub fn write<W: io::Write>(
        &self,
        writer: W,
        data: &Dataset,
    ) -> Result<ExportReport, WriteError> {
        let mut writer = writer;
        if let Some((sink, mut report)) = self.direct(data, &self.writer())? {
            let bytes = sink.into_bytes();
            writer.write_all(&bytes)?;
            writer.flush()?;
            self.size_limit(bytes.len() as u64, &mut report);
            return Ok(report);
        }
        let (typed, mut report) = self.prepared(data);
        let mut counter = Counter {
            inner: writer,
            bytes: 0,
        };
        let written = self.writer().write(&mut counter, &typed)?;
        report.repairs.extend(written.repairs);
        self.size_limit(counter.bytes, &mut report);
        Ok(report)
    }

    /// Writes `data` for the importer as text (UTF-8, whatever the
    /// profile's encoding; a byte order mark only when the profile writes
    /// one).
    ///
    /// # Errors
    ///
    /// As [`GedcomWriter::write_to_string`].
    pub fn write_to_string(&self, data: &Dataset) -> Result<(String, ExportReport), WriteError> {
        let utf8 = self.writer().output_encoding(OutputEncoding::Utf8);
        if let Some((sink, mut report)) = self.direct(data, &utf8)? {
            let text = sink.into_string();
            self.size_limit(text.len() as u64, &mut report);
            return Ok((text, report));
        }
        self.whole_to_string(data)
    }

    /// [`Exporter::write_to_string`] of the dataset folded whole, as drafts.
    fn whole_to_string(&self, data: &Dataset) -> Result<(String, ExportReport), WriteError> {
        let (typed, mut report) = self.prepared(data);
        let mut bytes = Vec::new();
        let written = self
            .writer()
            .output_encoding(OutputEncoding::Utf8)
            .write(&mut bytes, &typed)?;
        drop(typed);
        report.repairs.extend(written.repairs);
        self.size_limit(bytes.len() as u64, &mut report);
        let text = String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        Ok((text, report))
    }

    /// The importer's limit on the file's size, first among the limits.
    fn size_limit(&self, bytes: u64, report: &mut ExportReport) {
        let t = &self.profile.target;
        if let Some(max) = t.max_bytes
            && bytes > max
        {
            report.limits.insert(
                0,
                Limit {
                    rows: t.rows,
                    record: None,
                    detail: format!("{bytes} bytes, over the importer's {max}"),
                },
            );
        }
    }

    /// The importer's limits on the records, found record after record.
    fn limits(&self) -> Limits<'static> {
        Limits::new(&self.profile.target)
    }
}

/// The header, the submitters and the submission of a direct export,
/// folded ([`Exporter::fold_front`]).
struct Front {
    /// What their folds folded.
    folded: Folded,
    /// The header, as the writer writes it.
    head: Structure,
    /// The importer's limits on them.
    limits: Limits<'static>,
    /// The jurisdictions the header names, as the place fold reads them.
    mapping: (Vec<Option<usize>>, bool),
}

/// What [`Exporter::fold_with`] folded.
struct Folded {
    report: ExportReport,
    /// With each fold and each limit reported, the step that reported it
    /// ([`Cx::seq`]), for a direct export.
    fold_steps: Vec<u16>,
    limit_steps: Vec<u16>,
    /// The places the place fold folded, and its step, for a direct
    /// export.
    places: Option<(u16, PlacesSeen)>,
}

/// The round a step runs in: the folds; then, once conformance has made
/// extensions of what the target has no place for, the extension folds;
/// last the notes, which the folds before write to.
fn step_round(s: &Step) -> u8 {
    match s.fold {
        Op::ExtensionsAsNotes | Op::ExtensionsAsEvents | Op::SexSaid => 1,
        Op::EscapedNotes | Op::PlainNotes => 2,
        _ => 0,
    }
}

/// The header, the submitters and the submissions of `data`, then the
/// trailer, as [`drafts`] gives them.
fn front_drafts(data: &Dataset) -> Vec<Draft<'_>> {
    let n = 1 + data.submitters().len() + data.submissions().len();
    let mut out = drafts_of(data, data.records().take(n));
    out.push(Draft::new("TRLR"));
    out
}

/// Whether every pointer of `s` names one of `named` (or none).
fn points_within(s: &Draft<'_>, named: &[Option<String>]) -> bool {
    let within = match &s.payload {
        Value::Pointer(p) => named.iter().flatten().any(|n| n == &**p),
        _ => true,
    };
    within && s.substructures.iter().all(|c| points_within(c, named))
}

/// The importer's limits on the records ([`Exporter::limits`]), as the
/// records go by.
pub(super) struct Limits<'a> {
    target: &'a Target,
    individuals: usize,
    deep: Vec<Limit>,
    names: Vec<Limit>,
    children: Vec<Limit>,
}

impl<'a> Limits<'a> {
    /// The longest name the importer takes, when it says.
    pub(super) fn max_name(&self) -> Option<usize> {
        self.target.max_name
    }

    pub(super) fn new(target: &'a Target) -> Self {
        Self {
            target,
            individuals: 0,
            deep: Vec::new(),
            names: Vec::new(),
            children: Vec::new(),
        }
    }

    fn record(&mut self, r: &Draft<'_>) {
        let (indi, fam) = (is(r, "INDI"), is(r, "FAM"));
        let names = r
            .substructures
            .iter()
            .filter(|n| indi && is(n, "NAME"))
            .map(|n| payload_str(n).chars().count());
        let children = r.substructures.iter().filter(|c| is(c, "CHIL")).count();
        self.add(
            r.xref.as_deref(),
            indi,
            depth(r),
            names,
            fam.then_some(children),
        );
    }

    /// The limits on record `xref` (a person when `indi`), whose
    /// structures go `depth` levels deep, whose names (a person's) are
    /// `names` characters long, and which has `children` (a family).
    pub(super) fn add(
        &mut self,
        xref: Option<&str>,
        indi: bool,
        depth: usize,
        names: impl IntoIterator<Item = usize>,
        children: Option<usize>,
    ) {
        let t = self.target;
        let record = || xref.map(Xref::new);
        if indi {
            self.individuals += 1;
        }
        if let Some(max) = t.max_depth
            && depth > max
        {
            self.deep.push(Limit {
                rows: t.rows,
                record: record(),
                detail: format!("structures {depth} levels deep, over the importer's {max}"),
            });
        }
        if let (Some(max), true) = (t.max_name, indi) {
            for len in names {
                if len > max {
                    self.names.push(Limit {
                        rows: t.rows,
                        record: record(),
                        detail: format!("a name of {len} characters, over the importer's {max}"),
                    });
                }
            }
        }
        if let (Some(max), Some(n)) = (t.max_children, children)
            && n > max
        {
            self.children.push(Limit {
                rows: t.rows,
                record: record(),
                detail: format!("{n} children, over the importer's {max}"),
            });
        }
    }

    /// The limits, in the order of the importer's: the individuals, then
    /// the depth, the names and the children of each record.
    pub(super) fn done(self, report: &mut ExportReport) {
        let t = self.target;
        if let Some(max) = t.max_individuals {
            let n = self.individuals;
            if n > max {
                report.limits.push(Limit {
                    rows: t.rows,
                    record: None,
                    detail: format!("{n} individuals, over the importer's {max}"),
                });
            }
        }
        report.limits.extend(self.deep);
        report.limits.extend(self.names);
        report.limits.extend(self.children);
    }
}

/// The records of `data` and the trailer, as [`Dataset::to_structures`]
/// gives them, their texts borrowed from the dataset.
fn drafts(data: &Dataset) -> Vec<Draft<'_>> {
    let mut out = drafts_of(data, data.records());
    out.push(Draft::new("TRLR"));
    out
}

/// The `records` of `data`, as [`drafts`] gives them.
fn drafts_of<'d>(
    data: &'d Dataset,
    records: impl Iterator<Item = RecordRef<'d>>,
) -> Vec<Draft<'d>> {
    let cx = WriteCx::of(data, data.version(), false);
    let mut flat = Flat::default();
    let mut out = Vec::with_capacity(data.record_count() + 1);
    for r in records {
        let draft = if matches!(r, RecordRef::Header(_)) {
            // The header writes itself owned.
            Some(Draft::from_owned(r.to_structure(&cx)))
        } else {
            r.to_flat(&cx, &mut flat);
            Draft::of_flat(&mut flat)
        };
        out.extend(draft);
    }
    out
}

#[cfg(test)]
thread_local! {
    /// The exports of this thread written directly ([`Exporter::direct`]).
    static DIRECT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts the bytes written.
struct Counter<W> {
    inner: W,
    bytes: u64,
}

impl<W: io::Write> io::Write for Counter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn depth(s: &Draft<'_>) -> usize {
    s.substructures
        .iter()
        .map(|c| 1 + depth(c))
        .max()
        .unwrap_or(0)
}

/// The state of an export.
struct Cx<'a> {
    v7: bool,
    report: ExportReport,
    options: &'a ExportOptions,
    target: &'a Target,
    next_xref: u32,
    /// The places' format to name in the header ([`place_form`]).
    place_style: Option<PlaceStyle>,
    /// The step running, by its place in the order the steps run (0
    /// before them), and the next step's.
    seq: u16,
    next_seq: u16,
    /// With each fold and limit reported, the step that reported it
    /// ([`Cx::seq`]): kept for a direct export ([`Exporter::direct`]).
    tags: Option<(Vec<u16>, Vec<u16>)>,
    /// Whether each step runs on every record before the next
    /// ([`run_steps`]).
    alone: bool,
    /// The places the place fold folded, and its step, kept to report with
    /// a direct export's, which then reports them.
    places: Option<(u16, PlacesSeen)>,
}

impl Cx<'_> {
    fn fold(
        &mut self,
        rows: &'static [&'static str],
        kind: FoldKind,
        lossy: bool,
        record: Option<&str>,
        detail: impl Into<String>,
    ) {
        self.report.folds.push(Fold {
            rows,
            kind,
            lossy,
            record: record.map(Xref::new),
            detail: detail.into(),
        });
        if let Some((folds, _)) = &mut self.tags {
            folds.push(self.seq);
        }
    }

    fn limit(
        &mut self,
        rows: &'static [&'static str],
        record: Option<&str>,
        detail: impl Into<String>,
    ) {
        self.report.limits.push(Limit {
            rows,
            record: record.map(Xref::new),
            detail: detail.into(),
        });
        if let Some((_, limits)) = &mut self.tags {
            limits.push(self.seq);
        }
    }
}

/// The 5.5.1 and 7.x events that say `Y` when nothing more is known.
pub(super) const Y_EVENTS: &[&str] = &[
    "ADOP", "ANUL", "BAPM", "BARM", "BASM", "BIRT", "BLES", "BURI", "CENS", "CHR", "CHRA", "CONF",
    "CREM", "DEAT", "DIV", "DIVF", "EMIG", "ENGA", "FCOM", "GRAD", "IMMI", "MARB", "MARC", "MARL",
    "MARR", "MARS", "NATU", "ORDN", "PROB", "RETI", "WILL",
];

/// The events and attributes of a person or family that are not
/// [`Y_EVENTS`].
const OTHER_EVENTS: &[&str] = &[
    "EVEN", "FACT", "OCCU", "RESI", "EDUC", "RELI", "NATI", "TITL", "DSCR", "PROP", "CAST", "IDNO",
    "NCHI", "NMR", "SSN",
];

/// Which standard tags (by their index) are among `tags`, all standard.
const fn standard_set(tags: &[&[&str]]) -> [bool; STANDARD_TAGS.len()] {
    let mut set = [false; STANDARD_TAGS.len()];
    let mut i = 0;
    while i < tags.len() {
        let mut j = 0;
        while j < tags[i].len() {
            // (Every tag is standard: a test sees to it.)
            if let Some(k) = standard_index_const(tags[i][j]) {
                set[k as usize] = true;
            }
            j += 1;
        }
        i += 1;
    }
    set
}

/// Whether `tag` is one of `set` ([`standard_set`]).
#[inline]
fn in_set(set: &[bool; STANDARD_TAGS.len()], tag: &str) -> bool {
    standard_index(tag).is_some_and(|i| set.get(usize::from(i)) == Some(&true))
}

/// Whether `tag` is one of [`Y_EVENTS`].
pub(super) fn is_y_event(tag: &str) -> bool {
    const SET: [bool; STANDARD_TAGS.len()] = standard_set(&[Y_EVENTS]);
    in_set(&SET, tag)
}

/// Whether `tag` is an event or attribute of a person or family.
fn is_event(tag: &str) -> bool {
    const SET: [bool; STANDARD_TAGS.len()] = standard_set(&[Y_EVENTS, OTHER_EVENTS]);
    in_set(&SET, tag)
}

/// The tags of family events that say a union.
pub(super) const UNION_EVENTS: &[&str] = &[
    "MARR", "ENGA", "MARB", "MARC", "MARL", "MARS", "DIV", "DIVF", "ANUL",
];

/// Where each record is, by identifier (the last of records that share
/// one); the identifiers borrowed as the records hold them.
type Index<'s> = XrefIndex<'s, usize>;

fn index<'s>(records: &[Draft<'s>]) -> Index<'s> {
    let mut idx = Index::default();
    idx.reserve(records.len());
    for (i, r) in records.iter().enumerate() {
        if let Some(x) = &r.xref {
            idx.insert(x.clone(), i);
        }
    }
    idx
}

/// The substructures of a person or a family that the folds read of
/// other records: names (their values), sex, family links.
const SLIM: &[&str] = &["NAME", "SEX", "FAMC", "FAMS", "HUSB", "WIFE", "CHIL"];

/// The other records, as a fold that rewrites one reads them: by
/// identifier, a person or a family showing only [`SLIM`].
///
/// A fold reads them before it rewrites a record, then rewrites it, record
/// after record: what it reads is never what it rewrites, so that each
/// record reads the others as they were before the fold.
#[derive(Clone, Copy)]
struct Lookup<'a, 's> {
    records: &'a [Draft<'s>],
    idx: &'a Index<'s>,
}

/// A record as [`Lookup`] shows it.
#[derive(Clone, Copy)]
struct Seen<'a, 's> {
    record: &'a Draft<'s>,
    slim: bool,
}

impl<'a, 's> Lookup<'a, 's> {
    fn get(self, x: &str) -> Option<Seen<'a, 's>> {
        let record = self.records.get(*self.idx.get(x)?)?;
        Some(Seen {
            record,
            slim: is(record, "INDI") || is(record, "FAM"),
        })
    }

    /// A person's name for text: the first name, slashes out.
    fn person_name(self, x: &str) -> Option<String> {
        let name = self.get(x)?.child_text("NAME")?;
        Some(
            name.replace('/', " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    fn person_sex(self, x: &str) -> Option<String> {
        self.get(x)?
            .child_text("SEX")
            .map(|s| s.trim().to_ascii_uppercase())
    }
}

impl<'a, 's> Seen<'a, 's> {
    fn children(self) -> impl Iterator<Item = &'a Draft<'s>> {
        let slim = self.slim;
        self.record
            .substructures
            .iter()
            .filter(move |c| !slim || SLIM.contains(&c.tag.as_str()))
    }

    fn child(self, tag: &str) -> Option<&'a Draft<'s>> {
        self.children().find(|c| is(c, tag))
    }

    fn child_text(self, tag: &str) -> Option<&'a str> {
        self.child(tag).and_then(Draft::text)
    }
}

/// Whether `s` has nothing but a payload.
fn bare(s: &Draft<'_>) -> bool {
    s.substructures.is_empty()
}

/// What the steps run on the same records share: where each record is,
/// and what a step reports once it ran on every record.
#[derive(Default)]
struct Shared<'s> {
    idx: Option<Index<'s>>,
    /// Where the (first) header is, once looked for.
    head: std::cell::OnceCell<Option<usize>>,
    places: PlacesSeen,
}

impl<'s> Shared<'s> {
    /// Where the header is, when it is in `range`.
    fn head(&mut self, records: &[Draft<'s>], range: Range<usize>) -> Option<usize> {
        self.head
            .get_or_init(|| records.iter().position(|r| is(r, "HEAD")))
            .filter(|i| range.contains(i))
    }

    /// Where each record is, found once: the steps that share it neither
    /// add, remove nor move records.
    fn index(&mut self, records: &[Draft<'s>]) -> &Index<'s> {
        self.idx.get_or_insert_with(|| index(records))
    }
}

/// How a step goes through the records.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// It rewrites a record reading that record only.
    Own,
    /// It rewrites a record reading others ([`Op::reads`]), or reports
    /// what it did once it ran on every record ([`finish`]): it runs on a
    /// range of the records, the others in sight.
    Range,
    /// It rewrites other records than the one it reads, adds, removes or
    /// moves records, or numbers new ones: it runs on every record, alone.
    Dataset,
}

/// What a step reads of the records other than the one it rewrites.
enum Reads {
    Nothing,
    /// The structures with these tags (and the payloads and order of
    /// their substructures with these tags…), from a record down.
    Tags(&'static [&'static str]),
    Anything,
}

impl Op {
    fn reach(&self) -> Reach {
        match self {
            Op::AsEven(_)
            | Op::Godparents
            | Op::ChildLinkSources
            | Op::InlineSources
            | Op::InlineMedia
            | Op::AssociationsAsEvents(_)
            | Op::GodparentText
            | Op::ParentRelations
            | Op::Preferred(..)
            | Op::Places(_)
            | Op::OneHeadNote
            | Op::FlatHead => Reach::Range,
            Op::EventAssociations(style) => {
                if *style == AssocStyle::Reversed {
                    Reach::Dataset
                } else {
                    Reach::Range
                }
            }
            Op::SourceRecords | Op::HomeFirst | Op::ShortXrefs(_) | Op::CitationRefnToSource => {
                Reach::Dataset
            }
            _ => Reach::Own,
        }
    }

    fn reads(&self) -> Reads {
        match self {
            Op::AsEven(_)
            | Op::EventAssociations(_)
            | Op::AssociationsAsEvents(_)
            | Op::GodparentText => Reads::Tags(&["NAME"]),
            Op::Godparents => Reads::Tags(&["SEX"]),
            // A citation's text: its source's title, author, repositories
            // and their call numbers and names.
            Op::ChildLinkSources | Op::InlineSources => {
                Reads::Tags(&["TITL", "ABBR", "AUTH", "REPO", "CALN", "NAME"])
            }
            Op::ParentRelations => Reads::Tags(&["FAMC", "PEDI"]),
            Op::Preferred(..) => Reads::Tags(&["FILE", "FORM"]),
            // The header's format.
            Op::Places(_) => Reads::Tags(&["PLAC", "FORM"]),
            Op::InlineMedia => Reads::Anything,
            _ => Reads::Nothing,
        }
    }

    /// Whether the step may change structures tagged `tag`: add, remove,
    /// move, retag (to or from `tag`) them, or change their payloads or
    /// anything under them. What it changes is always under (or at) a
    /// structure with one of these tags, below the record.
    #[allow(clippy::match_same_arms)]
    fn changes(&self, tag: &str) -> bool {
        let any = |tags: &[&str]| tags.contains(&tag);
        match self {
            Op::OneBaptism | Op::EmptyPacs | Op::BadDates | Op::XreflessRecords => false,
            Op::OccupationsMerged => any(&["OCCU"]),
            Op::ProfessionFacts => any(&["FACT", "OCCU"]),
            Op::SurnamesPacked
            | Op::ParticleInSurname
            | Op::UsualNameQuoted
            | Op::NoColonInNames
            | Op::NamesInAka
            | Op::NamesWithSlashes => any(&["NAME"]),
            Op::NicknamesJoined => any(&["NICK"]),
            Op::EventsSaidY => is_y_event(tag),
            Op::DeathSaidY => any(&["DEAT"]),
            Op::AsEven(map) => tag == "EVEN" || map.iter().any(|(t, _)| *t == tag),
            Op::AsFact(map) => any(&["FACT", "EVEN"]) || map.iter().any(|(t, _)| *t == tag),
            Op::FactsAsEven => any(&["FACT", "EVEN"]),
            Op::UnionType => any(&["NO", "_NO", "_NMR", "EVEN"]),
            Op::Godparents => any(&["ASSO", "_ASSO"]),
            Op::EventAssociations(_) => any(&["ASSO", "_ASSO", "_SHAR", "_WITN", "NOTE"]),
            Op::Adoptions => any(&["ADOP", "FAMC"]),
            Op::InlineSources | Op::NoteSourcesLifted => any(&["SOUR"]),
            Op::EscapedNotes | Op::PlainNotes => any(&["NOTE", "MIME"]),
            Op::OneHeadNote | Op::BareFactsNoted => any(&["NOTE"]),
            Op::ChildLinkSources => any(&["SOUR", "NOTE"]),
            Op::Places(_) => any(&["PLAC"]),
            Op::AddressesAsNotes => any(&["ADDR", "NOTE"]),
            Op::AddressesInPlace => any(&["ADDR", "PLAC"]),
            Op::ExtensionsAsNotes => tag.starts_with('_') || tag == "NOTE",
            Op::ExtensionsAsEvents => tag.starts_with('_') || tag == "EVEN",
            Op::InlineMedia => any(&["OBJE"]),
            Op::AssociationsAsEvents(_) => any(&["ASSO", "EVEN"]),
            Op::FieldsAsNotes(fields) => {
                any(&["HUSB", "WIFE", "NOTE"]) || fields.iter().any(|(t, _)| *t == tag)
            }
            Op::CausesAsNotes => any(&["CAUS", "NOTE"]),
            Op::RecordNotesAsEvent(_) => any(&["NOTE", "EVEN"]),
            Op::RecordCitationsOnEvent(_) => any(&["SOUR", "EVEN"]),
            Op::SourceFieldsAsNote => any(&["TITL", "ABBR", "PUBL", "TEXT", "NOTE"]),
            Op::RepositoryContactsAsNote => {
                any(&["ADDR", "PHON", "EMAIL", "FAX", "WWW", "REFN", "NOTE"])
            }
            Op::ParentRelations => any(&["_FREL", "_MREL"]),
            Op::Photo => any(&["_PHOTO"]),
            Op::Preferred(flags, _) => tag == "_TYPE" || flags.contains(&tag),
            Op::PrefixAsTitle => any(&["NAME", "TITL"]),
            Op::NameValueComplete => any(&["NAME", "NOTE"]),
            Op::UsualNameAsNick | Op::UsualNameAsNote => {
                any(&["_RUFNAME", "_USED", "NICK", "NOTE"])
            }
            Op::IdentifiersAsNote(tags) => tag == "NOTE" || tags.contains(&tag),
            Op::InterpretedDatesAsNote => any(&["DATE", "NOTE"]),
            Op::PafUids => any(&["UID", "_UID"]),
            Op::Retag(map) => map.iter().any(|(a, b)| *a == tag || *b == tag),
            Op::MarriageStatus => any(&["NO", "_NO", "_STAT"]),
            Op::ContactsJoined | Op::FaxAsPhone => any(&["FAX", "PHON", "EMAIL", "WWW"]),
            Op::OnePlace => any(&["PLAC", "NOTE"]),
            Op::UnionsAsMarriage => any(&["NO", "_NO", "EVEN", "MARR"]),
            Op::SexSaid => any(&["SEX"]),
            Op::GodparentText => any(&["_GODP"]),
            Op::FlatHead
            | Op::SourceRecords
            | Op::HomeFirst
            | Op::ShortXrefs(_)
            | Op::CitationRefnToSource => true,
        }
    }

    /// Whether the step reports a fold of every record it changes, as it
    /// changes it.
    fn folds_each_change(&self) -> bool {
        matches!(
            self,
            Op::SexSaid
                | Op::ExtensionsAsNotes
                | Op::ExtensionsAsEvents
                | Op::EscapedNotes
                | Op::PlainNotes
        )
    }

    /// Whether the step only reports.
    fn reports_only(&self) -> bool {
        matches!(
            self,
            Op::OneBaptism | Op::EmptyPacs | Op::BadDates | Op::XreflessRecords
        )
    }

    /// Whether what `self` reads of other records is what `other` may
    /// change: the two then run one after the other on every record.
    fn reads_what_changes(&self, other: &Op) -> bool {
        match self.reads() {
            Reads::Nothing => false,
            Reads::Tags(tags) => tags.iter().any(|t| other.changes(t)),
            Reads::Anything => !other.reports_only(),
        }
    }
}

/// Runs `steps` in order on `records`, as [`run`] runs each on every
/// record, one after the other; but where nothing a step reads of the
/// other records is what the steps run with it change, those steps run
/// record after record, each record going through all of them while it is
/// at hand (which gives the same records, and the same report).
///
/// With `touched`, marks the records the steps change.
fn run_steps(
    steps: &[&Step],
    records: &mut Vec<Draft<'_>>,
    cx: &mut Cx<'_>,
    mut touched: Option<&mut [bool]>,
) {
    // The records a step changes are those it reports a fold of.
    let told = steps.iter().all(|s| s.fold.folds_each_change());
    if !told && let Some(t) = touched.as_deref_mut() {
        t.fill(true);
    }
    let base = cx.next_seq;
    cx.next_seq += u16::try_from(steps.len()).unwrap_or(u16::MAX);
    let mut at = 0;
    while let Some(rest) = steps.get(at..).filter(|r| !r.is_empty()) {
        let n = if cx.alone { 1 } else { together(rest) };
        cx.seq = base + u16::try_from(at).unwrap_or(u16::MAX);
        match rest.get(..n) {
            Some([one]) if !(told && touched.is_some()) => {
                let mut shared = Shared::default();
                run(one, records, 0..records.len(), cx, &mut shared);
                finish(one, cx, &mut shared);
            }
            Some(some) => run_together(some, records, cx, touched.as_deref_mut().filter(|_| told)),
            None => break,
        }
        at += n.max(1);
    }
}

/// How many of the first `steps` run record after record together.
fn together(steps: &[&Step]) -> usize {
    for (k, step) in steps.iter().enumerate() {
        let before = steps.get(..k).unwrap_or_default();
        let clash = step.fold.reach() == Reach::Dataset
            || before.iter().any(|b| {
                b.fold.reach() == Reach::Dataset
                    || b.fold.reads_what_changes(&step.fold)
                    || step.fold.reads_what_changes(&b.fold)
                    || matches!((b.fold, step.fold), (Op::Places(_), Op::Places(_)))
            });
        if clash {
            return k.max(1);
        }
    }
    steps.len()
}

/// Runs `steps` record after record (see [`run_steps`]); their reports in
/// the order running them one after the other gives.
fn run_together(
    steps: &[&Step],
    records: &mut Vec<Draft<'_>>,
    cx: &mut Cx<'_>,
    mut touched: Option<&mut [bool]>,
) {
    let (folds, limits) = (cx.report.folds.len(), cx.report.limits.len());
    // The step each fold and limit reported is of.
    let mut fold_of: Vec<usize> = Vec::new();
    let mut limit_of: Vec<usize> = Vec::new();
    let mark = |cx: &Cx<'_>, k: usize, fold_of: &mut Vec<usize>, limit_of: &mut Vec<usize>| {
        fold_of.resize(cx.report.folds.len() - folds, k);
        limit_of.resize(cx.report.limits.len() - limits, k);
    };
    let mut shared = Shared::default();
    for i in 0..records.len() {
        for (k, step) in steps.iter().enumerate() {
            if step.fold.reach() == Reach::Own {
                // On the record alone.
                let Some(r) = records.get_mut(i) else { break };
                run_on(step, std::slice::from_mut(r), 0..1, cx, &mut shared);
            } else {
                run(step, records, i..i + 1, cx, &mut shared);
            }
            let reported = cx.report.folds.len() - folds > fold_of.len()
                || cx.report.limits.len() - limits > limit_of.len();
            if let (true, Some(t)) = (reported, touched.as_deref_mut().and_then(|t| t.get_mut(i))) {
                *t = true;
            }
            mark(cx, k, &mut fold_of, &mut limit_of);
        }
    }
    for (k, step) in steps.iter().enumerate() {
        finish(step, cx, &mut shared);
        mark(cx, k, &mut fold_of, &mut limit_of);
    }
    order_by_step(&mut cx.report.folds, folds, &fold_of);
    order_by_step(&mut cx.report.limits, limits, &limit_of);
}

/// Puts the entries of `list` from `from` on in the order of the steps
/// that made them (`step_of`), in the order made for each step.
fn order_by_step<T>(list: &mut Vec<T>, from: usize, step_of: &[usize]) {
    if step_of.windows(2).all(|w| w[0] <= w[1]) {
        return;
    }
    let mut tail: Vec<(usize, T)> = step_of.iter().copied().zip(list.drain(from..)).collect();
    tail.sort_by_key(|(k, _)| *k);
    list.extend(tail.into_iter().map(|(_, t)| t));
}

/// Reports what `step` did once it ran on every record.
fn finish(step: &Step, cx: &mut Cx<'_>, shared: &mut Shared<'_>) {
    if let Op::Places(style) = step.fold {
        let seen = std::mem::take(&mut shared.places);
        if let Some(kept) = &mut cx.places {
            // Reported with the direct export's.
            *kept = (cx.seq, seen);
            return;
        }
        places_done(step.rows, style, &seen, cx);
    }
}

/// Runs `step` on the records in `range` (all of them for a step of
/// [`Reach::Dataset`]).
fn run<'s>(
    step: &Step,
    records: &mut Vec<Draft<'s>>,
    range: Range<usize>,
    cx: &mut Cx<'_>,
    shared: &mut Shared<'s>,
) {
    match step.fold {
        // The one step that adds records.
        Op::SourceRecords => source_records(records, step.rows, cx),
        _ => run_on(step, records, range, cx, shared),
    }
}

/// [`run`], for a step that adds no record.
#[allow(clippy::too_many_lines)]
fn run_on<'s>(
    step: &Step,
    records: &mut [Draft<'s>],
    range: Range<usize>,
    cx: &mut Cx<'_>,
    shared: &mut Shared<'s>,
) {
    let rows = step.rows;
    match step.fold {
        Op::OccupationsMerged => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let plain: Vec<usize> = r
                    .substructures
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| is(c, "OCCU") && bare(c) && !payload_str(c).trim().is_empty())
                    .map(|(i, _)| i)
                    .collect();
                if plain.len() < 2 {
                    continue;
                }
                let joined: Vec<String> = plain
                    .iter()
                    .filter_map(|&i| {
                        r.substructures
                            .get(i)
                            .map(|c| payload_str(c).trim().to_string())
                    })
                    .collect();
                let first = plain.first().copied().unwrap_or(0);
                if let Some(c) = r.substructures.get_mut(first) {
                    set_text(c, &joined.join(", "));
                }
                for &i in plain.iter().skip(1).rev() {
                    r.substructures.remove(i);
                }
                let x = r.xref.clone();
                cx.fold(
                    rows,
                    FoldKind::Merged,
                    true,
                    x.as_deref(),
                    format!("{} occupations made one", plain.len()),
                );
            }
        }
        Op::ProfessionFacts => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let x = r.xref.clone();
                for c in r.substructures.iter_mut().filter(|c| is(c, "FACT")) {
                    let ty = child_text(c, "TYPE").map(|t| t.trim().to_ascii_lowercase());
                    if ty.as_deref().is_some_and(|t| {
                        matches!(t, "profession" | "occupation" | "métier" | "metier" | "job")
                    }) {
                        retag(c, "OCCU");
                        c.substructures.retain(|s| !is(s, "TYPE"));
                        cx.fold(
                            rows,
                            FoldKind::Relabelled,
                            false,
                            x.as_deref(),
                            "a profession fact as an occupation",
                        );
                    }
                }
            }
        }
        Op::SurnamesPacked => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let names: Vec<usize> = r
                    .substructures
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| is(c, "NAME"))
                    .map(|(i, _)| i)
                    .collect();
                let Some((&first, others)) = names.split_first() else {
                    continue;
                };
                let Some(primary) = r.substructures.get(first).cloned() else {
                    continue;
                };
                let value = payload_str(&primary).to_string();
                let Some(main) = surname_part(&value).map(str::to_string) else {
                    continue;
                };
                let given = given_part(&value).to_string();
                let mut list: Vec<String> = child_text(&primary, "SURN").map_or_else(
                    || vec![main.clone()],
                    |s| s.split(',').map(|p| p.trim().to_string()).collect(),
                );
                let mut packed = Vec::new();
                for &i in others {
                    let Some(n) = r.substructures.get(i) else {
                        continue;
                    };
                    let v = payload_str(n);
                    let (Some(s), g) = (surname_part(v), given_part(v)) else {
                        continue;
                    };
                    if (g.is_empty() || g == given)
                        && !s.is_empty()
                        && !s.eq_ignore_ascii_case(&main)
                    {
                        if !list.iter().any(|l| l.eq_ignore_ascii_case(s)) {
                            list.push(s.to_string());
                        }
                        packed.push(i);
                    }
                }
                if packed.is_empty() {
                    continue;
                }
                let x = r.xref.clone();
                for &i in packed.iter().rev() {
                    r.substructures.remove(i);
                }
                if let Some(p) = r.substructures.get_mut(first) {
                    let surn = list.join(",");
                    match child_mut(p, "SURN") {
                        Some(s) => set_text(s, &surn),
                        None => p.substructures.push(text_node("SURN", &surn)),
                    }
                }
                cx.fold(
                    rows,
                    FoldKind::Merged,
                    true,
                    x.as_deref(),
                    format!(
                        "{} surname-only names in the primary SURN list",
                        packed.len()
                    ),
                );
            }
        }
        Op::ParticleInSurname => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let x = r.xref.clone();
                for n in r.substructures.iter_mut().filter(|c| is(c, "NAME")) {
                    let (Some(spfx), Some(surn)) = (
                        child_text(n, "SPFX").map(str::to_string),
                        child_text(n, "SURN").map(str::to_string),
                    ) else {
                        continue;
                    };
                    let value = payload_str(n).to_string();
                    let Some(current) = surname_part(&value) else {
                        continue;
                    };
                    let wanted = format!("{} {}", spfx.trim(), surn.trim());
                    if current.trim() == surn.trim() && current.trim() != wanted {
                        let new = name_value(given_part(&value), &wanted, suffix_part(&value));
                        set_text(n, &new);
                        cx.fold(
                            rows,
                            FoldKind::Recoded,
                            false,
                            x.as_deref(),
                            "the surname prefix inside the slashes",
                        );
                    }
                }
            }
        }
        Op::NicknamesJoined => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let mut nicks = Vec::new();
                for n in r.substructures.iter_mut().filter(|c| is(c, "NAME")) {
                    for k in n.substructures.iter().filter(|s| is(s, "NICK")) {
                        let v = payload_str(k).trim().to_string();
                        if !v.is_empty() && !nicks.contains(&v) {
                            nicks.push(v);
                        }
                    }
                }
                let count = r
                    .substructures
                    .iter()
                    .filter(|c| is(c, "NAME"))
                    .map(|n| n.substructures.iter().filter(|s| is(s, "NICK")).count())
                    .sum::<usize>();
                let first_has_one = r
                    .substructures
                    .iter()
                    .find(|c| is(c, "NAME"))
                    .is_some_and(|n| n.substructures.iter().filter(|s| is(s, "NICK")).count() == 1);
                if count <= 1 && (count == 0 || first_has_one) {
                    continue;
                }
                let mut first = true;
                for n in r.substructures.iter_mut().filter(|c| is(c, "NAME")) {
                    n.substructures.retain(|s| !is(s, "NICK"));
                    if first {
                        n.substructures.push(text_node("NICK", &nicks.join(", ")));
                        first = false;
                    }
                }
                let x = r.xref.clone();
                cx.fold(
                    rows,
                    FoldKind::Merged,
                    count > 1,
                    x.as_deref(),
                    format!("{count} nicknames in one NICK of the primary name"),
                );
            }
        }
        Op::UsualNameQuoted => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let x = r.xref.clone();
                for n in r.substructures.iter_mut().filter(|c| is(c, "NAME")) {
                    let Some(at) = n
                        .substructures
                        .iter()
                        .position(|s| is(s, "_RUFNAME") || is(s, "_USED"))
                    else {
                        continue;
                    };
                    let used = n
                        .substructures
                        .get(at)
                        .map(|s| payload_str(s).trim().to_string())
                        .unwrap_or_default();
                    let value = payload_str(n).to_string();
                    let given = given_part(&value);
                    if used.is_empty()
                        || given.contains('"')
                        || !given.split_whitespace().any(|w| w == used)
                    {
                        continue;
                    }
                    let quoted: Vec<String> = given
                        .split_whitespace()
                        .map(|w| {
                            if w == used {
                                format!("\"{w}\"")
                            } else {
                                w.to_string()
                            }
                        })
                        .collect();
                    let new = match surname_part(&value) {
                        Some(s) => name_value(&quoted.join(" "), s, suffix_part(&value)),
                        None => quoted.join(" "),
                    };
                    set_text(n, &new);
                    n.substructures.remove(at);
                    cx.fold(
                        rows,
                        FoldKind::Recoded,
                        false,
                        x.as_deref(),
                        "the usual given name in double quotes",
                    );
                }
            }
        }
        Op::NoColonInNames => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let x = r.xref.clone();
                for n in r.substructures.iter_mut().filter(|c| is(c, "NAME")) {
                    let mut changed = false;
                    fix_colons(n, &mut changed);
                    if changed {
                        cx.fold(
                            rows,
                            FoldKind::Recoded,
                            true,
                            x.as_deref(),
                            "':' left out of a name",
                        );
                    }
                }
            }
        }
        Op::EventsSaidY => {
            // 5.5.1 says `Y` for these four only (7.x for every event).
            let v7 = cx.v7;
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                for e in &mut r.substructures {
                    let said = if v7 {
                        is_y_event(e.tag.as_str())
                    } else {
                        matches!(e.tag.as_str(), "BIRT" | "CHR" | "DEAT" | "MARR")
                    };
                    if said && bare(e) && matches!(e.payload, Value::None) {
                        set_text(e, "Y");
                        cx.fold(
                            rows,
                            FoldKind::Recoded,
                            false,
                            x.as_deref(),
                            format!("{} said Y", e.tag),
                        );
                    }
                }
            }
        }
        Op::DeathSaidY => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                if has(r, "DEAT") || !(has(r, "BURI") || has(r, "CREM")) {
                    continue;
                }
                let at = r
                    .substructures
                    .iter()
                    .position(|c| is(c, "BURI") || is(c, "CREM"))
                    .unwrap_or(r.substructures.len());
                r.substructures.insert(at, text_node("DEAT", "Y"));
                let x = r.xref.clone();
                cx.fold(
                    rows,
                    FoldKind::Added,
                    false,
                    x.as_deref(),
                    "a death said Y for a person buried or cremated",
                );
            }
        }
        Op::OneBaptism => {
            for r in records
                .iter()
                .filter(|r| is(r, "INDI") && has(r, "BAPM") && has(r, "CHR"))
            {
                let x = r.xref.clone();
                cx.limit(
                    rows,
                    x.as_deref(),
                    "a baptism and a christening: the importer reads one",
                );
            }
        }
        Op::AsEven(map) => {
            let mapped = |c: &Draft<'_>| map.iter().find(|(t, _)| *t == c.tag.as_str());
            // The names of the persons pointers name, when one does.
            let pointed = records
                .get(range.clone())
                .unwrap_or_default()
                .iter()
                .filter(|r| is(r, "INDI") || is(r, "FAM"))
                .flat_map(|r| &r.substructures)
                .any(|c| matches!(c.payload, Value::Pointer(_)) && mapped(c).is_some());
            let idx = if pointed {
                shared.index(records)
            } else {
                &Index::default()
            };
            for i in range {
                let names: Vec<Option<String>> = match records.get(i) {
                    Some(r) if pointed && (is(r, "INDI") || is(r, "FAM")) => {
                        let look = Lookup { records, idx };
                        r.substructures
                            .iter()
                            .filter(|c| mapped(c).is_some())
                            .filter_map(Draft::pointer)
                            .map(|p| look.person_name(p))
                            .collect()
                    }
                    _ => Vec::new(),
                };
                let mut names = names.into_iter();
                let Some(r) = records.get_mut(i) else { break };
                if !(is(r, "INDI") || is(r, "FAM")) {
                    continue;
                }
                let x = r.xref.clone();
                for c in &mut r.substructures {
                    let Some(&(_, label)) = mapped(c) else {
                        continue;
                    };
                    let tag = c.tag.as_str().to_string();
                    let value = match &c.payload {
                        Value::Pointer(p) => {
                            names.next().flatten().unwrap_or_else(|| p.to_string())
                        }
                        Value::Text(t)
                            if is_y_event(tag.as_str()) && t.trim().eq_ignore_ascii_case("y") =>
                        {
                            String::new()
                        }
                        p => p.as_str().unwrap_or_default().to_string(),
                    };
                    retag(c, "EVEN");
                    set_text(c, value.trim());
                    // The label, and the kind the attribute had, if any.
                    match child_mut(c, "TYPE") {
                        Some(t) => {
                            let kind = format!("{label} ({})", payload_str(t).trim());
                            set_text(t, &kind);
                        }
                        None => c.substructures.insert(0, text_node("TYPE", label)),
                    }
                    cx.fold(
                        rows,
                        FoldKind::Relabelled,
                        true,
                        x.as_deref(),
                        format!("{tag} as EVEN + TYPE {label}"),
                    );
                }
            }
        }
        Op::AsFact(map) => {
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                let family = is(r, "FAM");
                for c in &mut r.substructures {
                    let Some(&(_, label)) = map.iter().find(|(t, _)| *t == c.tag.as_str()) else {
                        continue;
                    };
                    let tag = c.tag.as_str().to_string();
                    let to = if family || c.text().is_none() {
                        "EVEN"
                    } else {
                        "FACT"
                    };
                    retag(c, to);
                    if !has(c, "TYPE") {
                        c.substructures.insert(0, text_node("TYPE", label));
                    }
                    cx.fold(
                        rows,
                        FoldKind::Relabelled,
                        true,
                        x.as_deref(),
                        format!("{tag} as {to} + TYPE {label}"),
                    );
                }
            }
        }
        Op::FactsAsEven => {
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                for c in r.substructures.iter_mut().filter(|c| is(c, "FACT")) {
                    retag(c, "EVEN");
                    cx.fold(
                        rows,
                        FoldKind::Relabelled,
                        true,
                        x.as_deref(),
                        "FACT as EVEN",
                    );
                }
            }
        }
        Op::UnionType => {
            for r in records.iter_mut().filter(|r| is(r, "FAM")) {
                let x = r.xref.clone();
                let unmarried = |c: &Draft<'s>| {
                    (is(c, "NO") && payload_str(c).trim() == "MARR")
                        || (is(c, "_NO") && payload_str(c).trim() == "MARR")
                        || is(c, "_NMR")
                };
                if let Some(at) = r.substructures.iter().position(unmarried) {
                    let mut even = Draft::new("EVEN");
                    even.substructures.push(text_node("TYPE", "unmarried"));
                    if let Some(old) = r.substructures.get(at) {
                        even.substructures.extend(
                            old.substructures
                                .iter()
                                .filter(|s| !is(s, "PHRASE"))
                                .cloned(),
                        );
                    }
                    r.substructures.remove(at);
                    r.substructures.insert(at, even);
                    cx.fold(
                        rows,
                        FoldKind::Relabelled,
                        false,
                        x.as_deref(),
                        "never married said as EVEN TYPE unmarried",
                    );
                    continue;
                }
                let said = r.substructures.iter().any(|c| {
                    UNION_EVENTS.contains(&c.tag.as_str())
                        || (is(c, "EVEN")
                            && child_text(c, "TYPE").is_some_and(|t| {
                                matches!(
                                    t.trim().to_ascii_lowercase().as_str(),
                                    "unmarried" | "nomen" | "pacs" | "relation"
                                )
                            }))
                });
                let couple = has(r, "HUSB") || has(r, "WIFE");
                if !said && couple {
                    let mut even = Draft::new("EVEN");
                    even.substructures.push(text_node("TYPE", "nomen"));
                    let at = r
                        .substructures
                        .iter()
                        .position(|c| !matches!(c.tag.as_str(), "HUSB" | "WIFE" | "RESN"))
                        .unwrap_or(r.substructures.len());
                    r.substructures.insert(at, even);
                    cx.fold(
                        rows,
                        FoldKind::Added,
                        false,
                        x.as_deref(),
                        "an unknown union said as EVEN TYPE nomen",
                    );
                }
            }
        }
        Op::EmptyPacs => {
            for r in records.iter().filter(|r| is(r, "FAM")) {
                for c in &r.substructures {
                    if is(c, "EVEN")
                        && child_text(c, "TYPE")
                            .is_some_and(|t| t.trim().eq_ignore_ascii_case("pacs"))
                        && c.substructures.iter().all(|s| is(s, "TYPE"))
                    {
                        let x = r.xref.clone();
                        cx.limit(
                            rows,
                            x.as_deref(),
                            "a PACS with no date, place or note, which the importer drops",
                        );
                    }
                }
            }
        }
        Op::Godparents => godparents(records, range, rows, cx, shared),
        Op::EventAssociations(style) => {
            event_associations(records, range, rows, style, cx, shared);
        }
        Op::Adoptions => {
            let adopted = if cx.v7 { "ADOPTED" } else { "adopted" };
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let x = r.xref.clone();
                let links: Vec<Xref> = r
                    .substructures
                    .iter()
                    .filter(|c| {
                        is(c, "FAMC")
                            && child_text(c, "PEDI")
                                .is_some_and(|p| p.trim().eq_ignore_ascii_case(adopted))
                    })
                    .filter_map(|c| c.pointer().map(Xref::new))
                    .collect();
                for family in links {
                    let already = r.substructures.iter().any(|c| {
                        is(c, "ADOP") && child(c, "FAMC").and_then(Draft::pointer) == Some(&family)
                    });
                    if already {
                        continue;
                    }
                    let mut adop = Draft::new("ADOP");
                    let mut famc = pointer_node("FAMC", &family);
                    famc.substructures.push(text_node("ADOP", "BOTH"));
                    adop.substructures.push(famc);
                    r.substructures.push(adop);
                    cx.fold(
                        rows,
                        FoldKind::Added,
                        false,
                        x.as_deref(),
                        "an adoption as ADOP + FAMC + ADOP BOTH",
                    );
                }
                // The birth family first.
                let famcs: Vec<usize> = r
                    .substructures
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| is(c, "FAMC"))
                    .map(|(i, _)| i)
                    .collect();
                let birth = famcs.iter().copied().find(|&i| {
                    r.substructures.get(i).is_some_and(|c| {
                        child_text(c, "PEDI").is_none_or(|p| p.trim().eq_ignore_ascii_case("birth"))
                    })
                });
                if let (Some(&first), Some(birth)) = (famcs.first(), birth)
                    && birth != first
                {
                    let moved = r.substructures.remove(birth);
                    r.substructures.insert(first, moved);
                    cx.fold(
                        rows,
                        FoldKind::Reordered,
                        false,
                        x.as_deref(),
                        "the birth family first",
                    );
                }
            }
        }
        Op::InlineSources => inline_sources(records, range, rows, cx, shared),
        Op::SourceRecords => debug_assert!(false, "SourceRecords adds records: run it with run"),
        Op::EscapedNotes => {
            for r in records.iter_mut() {
                let x = r.xref.clone();
                let mut n = 0;
                each_note(r, &mut |note| {
                    // HTML already, which the importer reads as such.
                    let before = note.substructures.len();
                    note.substructures.retain(|m| {
                        !(is(m, "MIME") && payload_str(m).trim().eq_ignore_ascii_case("text/html"))
                    });
                    if note.substructures.len() != before {
                        n += 1;
                    }
                    if has(note, "MIME") {
                        return;
                    }
                    if let Some(t) = note.text().filter(|t| t.contains(['<', '&'])) {
                        let escaped = escape_markup(t);
                        if escaped != t {
                            set_text(note, &escaped);
                            n += 1;
                        }
                    }
                });
                if n > 0 {
                    cx.fold(
                        rows,
                        FoldKind::Recoded,
                        false,
                        x.as_deref(),
                        format!("{n} notes with < and & escaped"),
                    );
                }
            }
        }
        Op::PlainNotes => {
            for r in records.iter_mut() {
                let x = r.xref.clone();
                let mut n = 0;
                each_note(r, &mut |note| {
                    let html = has(note, "MIME") || note.text().is_some_and(looks_html);
                    if !html {
                        return;
                    }
                    if let Some(t) = note.text() {
                        let plain = strip_html(t);
                        set_text(note, &plain);
                    }
                    note.substructures.retain(|s| !is(s, "MIME"));
                    n += 1;
                });
                if n > 0 {
                    cx.fold(
                        rows,
                        FoldKind::Recoded,
                        true,
                        x.as_deref(),
                        format!("{n} notes stripped of their markup"),
                    );
                }
            }
        }
        Op::OneHeadNote => {
            if let Some(head) = shared.head(records, range).and_then(|i| records.get_mut(i)) {
                let notes: Vec<usize> = head
                    .substructures
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| is(c, "NOTE"))
                    .map(|(i, _)| i)
                    .collect();
                if notes.len() > 1 {
                    let text: Vec<String> = notes
                        .iter()
                        .filter_map(|&i| {
                            head.substructures
                                .get(i)
                                .map(|n| payload_str(n).to_string())
                        })
                        .collect();
                    if let Some(first) = notes.first().and_then(|&i| head.substructures.get_mut(i))
                    {
                        set_text(first, &text.join("\n"));
                        first.substructures.clear();
                    }
                    for &i in notes.iter().skip(1).rev() {
                        head.substructures.remove(i);
                    }
                    cx.fold(
                        rows,
                        FoldKind::Merged,
                        true,
                        None,
                        format!("{} header notes made one", notes.len()),
                    );
                }
            }
        }
        Op::ChildLinkSources => {
            let idx = shared.index(records);
            for i in range {
                let texts: Vec<String> = match records.get(i) {
                    Some(r) if is(r, "INDI") => {
                        let look = Lookup { records, idx };
                        r.substructures
                            .iter()
                            .filter(|c| is(c, "FAMC"))
                            .flat_map(|famc| &famc.substructures)
                            .filter(|s| is(s, "SOUR"))
                            .map(|s| citation_text(s, look))
                            .collect()
                    }
                    _ => continue,
                };
                let Some(r) = records.get_mut(i) else { break };
                let x = r.xref.clone();
                for famc in r.substructures.iter_mut().filter(|c| is(c, "FAMC")) {
                    famc.substructures.retain(|s| !is(s, "SOUR"));
                }
                if !texts.is_empty() {
                    let n = texts.len();
                    r.substructures.push(text_node(
                        "NOTE",
                        &format!("Filiation source: {}", texts.join("; ")),
                    ));
                    cx.fold(
                        rows,
                        FoldKind::AsText,
                        true,
                        x.as_deref(),
                        format!("{n} citations of a child link as a note"),
                    );
                }
            }
        }
        Op::Places(style) => places(records, range, style, cx, shared),
        Op::AddressesAsNotes | Op::AddressesInPlace => {
            let in_place = matches!(step.fold, Op::AddressesInPlace);
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                for e in &mut r.substructures {
                    let Some(at) = e.substructures.iter().position(|s| is(s, "ADDR")) else {
                        continue;
                    };
                    if !(is_y_event(e.tag.as_str())
                        || matches!(
                            e.tag.as_str(),
                            "EVEN" | "RESI" | "OCCU" | "FACT" | "EDUC" | "PROP"
                        ))
                    {
                        continue;
                    }
                    let addr = e.substructures.remove(at);
                    let text = address_text(&addr);
                    if text.is_empty() {
                        continue;
                    }
                    if in_place {
                        match child_mut(e, "PLAC") {
                            Some(p) => {
                                let v = format!("{} at {text}", payload_str(p));
                                set_text(p, v.trim());
                            }
                            None => e.substructures.push(text_node("PLAC", &text)),
                        }
                    } else {
                        e.substructures
                            .push(text_node("NOTE", &format!("Address: {text}")));
                    }
                    cx.fold(
                        rows,
                        FoldKind::AsText,
                        true,
                        x.as_deref(),
                        format!("an address of {} as text", e.tag),
                    );
                }
            }
        }
        Op::BadDates => {
            for r in records.iter() {
                let x = r.xref.clone();
                let mut bad = Vec::new();
                each_date(r, &mut |d| {
                    if impossible_date(d) {
                        bad.push(d.to_string());
                    }
                });
                for d in bad {
                    cx.limit(
                        rows,
                        x.as_deref(),
                        format!("impossible date {d}: the importer keeps part of it"),
                    );
                }
            }
        }
        Op::ExtensionsAsNotes => {
            // (A record without extension structures has nothing to fold.)
            for r in records.iter_mut().filter(|r| has_extension_below(r)) {
                let x = r.xref.clone();
                let mut lost = Vec::new();
                let _ = extensions_as_notes(r, &mut Vec::new(), &mut lost);
                for (tag, kept) in lost {
                    cx.fold(
                        rows,
                        if kept {
                            FoldKind::AsText
                        } else {
                            FoldKind::Omitted
                        },
                        true,
                        x.as_deref(),
                        format!(
                            "extension {tag} {}",
                            if kept { "as a note" } else { "left out" }
                        ),
                    );
                }
            }
        }
        Op::ExtensionsAsEvents => {
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                for c in &mut r.substructures {
                    if !c.tag.is_extension() || c.pointer().is_some() {
                        continue;
                    }
                    let tag = c.tag.as_str().to_string();
                    retag(c, "EVEN");
                    if !has(c, "TYPE") {
                        c.substructures
                            .insert(0, text_node("TYPE", tag.trim_start_matches('_')));
                    }
                    cx.fold(
                        rows,
                        FoldKind::Relabelled,
                        true,
                        x.as_deref(),
                        format!("extension {tag} as EVEN"),
                    );
                }
            }
        }
        Op::InlineMedia => inline_media(records, range, rows, cx, shared),
        Op::AssociationsAsEvents(name_in_payload) => {
            associations_as_events(records, range, rows, name_in_payload, cx, shared);
        }
        Op::FieldsAsNotes(fields) => {
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                for e in r
                    .substructures
                    .iter_mut()
                    .filter(|e| is_event(e.tag.as_str()))
                {
                    let mut notes = Vec::new();
                    let mut k = 0;
                    while k < e.substructures.len() {
                        let label = e.substructures.get(k).and_then(|s| {
                            fields
                                .iter()
                                .find(|(t, _)| *t == s.tag.as_str())
                                .map(|&(_, l)| l)
                        });
                        if let Some(label) = label {
                            let s = e.substructures.remove(k);
                            notes.push(format!("{label}: {}", flatten(&s)));
                            continue;
                        }
                        // HUSB/WIFE ages.
                        if let Some(s) = e.substructures.get(k)
                            && matches!(s.tag.as_str(), "HUSB" | "WIFE")
                            && fields.iter().any(|(t, _)| *t == "AGE")
                            && let Some(age) = child_text(s, "AGE")
                        {
                            notes.push(format!(
                                "{} age: {age}",
                                if is(s, "HUSB") { "Husband" } else { "Wife" }
                            ));
                            e.substructures.remove(k);
                            continue;
                        }
                        k += 1;
                    }
                    if !notes.is_empty() {
                        let n = notes.len();
                        e.substructures.push(text_node("NOTE", &notes.join("\n")));
                        cx.fold(
                            rows,
                            FoldKind::AsText,
                            true,
                            x.as_deref(),
                            format!("{n} fields of {} in its note", e.tag),
                        );
                    }
                }
            }
        }
        Op::CausesAsNotes => {
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                for e in r
                    .substructures
                    .iter_mut()
                    .filter(|e| !is(e, "DEAT") && is_event(e.tag.as_str()))
                {
                    if let Some(at) = e.substructures.iter().position(|s| is(s, "CAUS")) {
                        let c = e.substructures.remove(at);
                        e.substructures
                            .push(text_node("NOTE", &format!("Cause: {}", payload_str(&c))));
                        cx.fold(
                            rows,
                            FoldKind::AsText,
                            true,
                            x.as_deref(),
                            format!("the cause of {} in its note", e.tag),
                        );
                    }
                }
            }
        }
        Op::RecordNotesAsEvent(label) => {
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                let notes: Vec<Draft<'s>> = r
                    .substructures
                    .iter()
                    .filter(|c| is(c, "NOTE"))
                    .cloned()
                    .collect();
                if notes.is_empty() {
                    continue;
                }
                r.substructures.retain(|c| !is(c, "NOTE"));
                let mut even = Draft::new("EVEN");
                even.substructures.push(text_node("TYPE", label));
                even.substructures.extend(notes);
                r.substructures.push(even);
                cx.fold(
                    rows,
                    FoldKind::Moved,
                    true,
                    x.as_deref(),
                    format!("record notes on an EVEN TYPE {label}"),
                );
            }
        }
        Op::RecordCitationsOnEvent(label) => {
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                let sources: Vec<Draft<'s>> = r
                    .substructures
                    .iter()
                    .filter(|c| is(c, "SOUR"))
                    .cloned()
                    .collect();
                if sources.is_empty() {
                    continue;
                }
                r.substructures.retain(|c| !is(c, "SOUR"));
                let mut even = Draft::new("EVEN");
                even.substructures.push(text_node("TYPE", label));
                even.substructures.extend(sources);
                r.substructures.push(even);
                cx.fold(
                    rows,
                    FoldKind::Moved,
                    true,
                    x.as_deref(),
                    format!("record citations on an EVEN TYPE {label}"),
                );
            }
        }
        Op::SourceFieldsAsNote => {
            for r in records.iter_mut().filter(|r| is(r, "SOUR")) {
                let x = r.xref.clone();
                if !has(r, "TITL")
                    && let Some(abbr) = child_text(r, "ABBR").map(str::to_string)
                {
                    r.substructures.insert(0, text_node("TITL", &abbr));
                }
                let mut lines = Vec::new();
                for (tag, label) in [
                    ("ABBR", "Abbreviation"),
                    ("PUBL", "Publication"),
                    ("TEXT", "Text"),
                ] {
                    while let Some(at) = r.substructures.iter().position(|c| is(c, tag)) {
                        let c = r.substructures.remove(at);
                        lines.push(format!("{label}: {}", payload_str(&c)));
                    }
                }
                if !lines.is_empty() {
                    r.substructures.push(text_node("NOTE", &lines.join("\n")));
                    cx.fold(
                        rows,
                        FoldKind::AsText,
                        true,
                        x.as_deref(),
                        "source fields in its note",
                    );
                }
            }
        }
        Op::RepositoryContactsAsNote => {
            for r in records.iter_mut().filter(|r| is(r, "REPO")) {
                let x = r.xref.clone();
                let mut lines = Vec::new();
                for (tag, label) in [
                    ("ADDR", "Address"),
                    ("PHON", "Phone"),
                    ("EMAIL", "Email"),
                    ("FAX", "Fax"),
                    ("WWW", "Web"),
                    ("REFN", "Reference"),
                ] {
                    while let Some(at) = r.substructures.iter().position(|c| is(c, tag)) {
                        let c = r.substructures.remove(at);
                        let v = if tag == "ADDR" {
                            address_text(&c)
                        } else {
                            payload_str(&c).to_string()
                        };
                        lines.push(format!("{label}: {v}"));
                    }
                }
                if !lines.is_empty() {
                    r.substructures.push(text_node("NOTE", &lines.join("\n")));
                    cx.fold(
                        rows,
                        FoldKind::AsText,
                        true,
                        x.as_deref(),
                        "repository contacts in its note",
                    );
                }
            }
        }
        Op::ParentRelations => {
            let idx = shared.index(records);
            for i in range {
                // The pedigree each child's link says, before the fold.
                let pedis: Vec<Option<String>> = match records.get(i) {
                    Some(f) if is(f, "FAM") => {
                        let Some(fx) = f.xref.as_deref() else {
                            continue;
                        };
                        let look = Lookup { records, idx };
                        f.substructures
                            .iter()
                            .filter(|c| is(c, "CHIL"))
                            .map(|c| {
                                look.get(c.pointer()?)?
                                    .children()
                                    .find(|s| is(s, "FAMC") && s.pointer() == Some(fx))
                                    .and_then(|famc| child_text(famc, "PEDI"))
                                    .map(|p| p.trim().to_ascii_lowercase())
                            })
                            .collect()
                    }
                    _ => continue,
                };
                let Some(f) = records.get_mut(i) else { break };
                let Some(fx) = f.xref.clone() else { continue };
                let mut pedis = pedis.into_iter();
                for c in f.substructures.iter_mut().filter(|c| is(c, "CHIL")) {
                    let pedi = pedis.next().flatten();
                    if has(c, "_FREL") || has(c, "_MREL") {
                        continue;
                    }
                    if c.pointer().is_none() {
                        continue;
                    }
                    let Some(word) = pedi.as_deref().map(|p| match p {
                        "birth" => "Natural",
                        "adopted" => "Adopted",
                        "foster" => "Foster",
                        "sealing" => "Sealing",
                        _ => "Unknown",
                    }) else {
                        continue;
                    };
                    c.substructures.push(text_node("_FREL", word));
                    c.substructures.push(text_node("_MREL", word));
                    cx.fold(
                        rows,
                        FoldKind::Added,
                        false,
                        Some(&fx),
                        "_FREL and _MREL beside the child's PEDI",
                    );
                }
            }
        }
        Op::Photo => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                if has(r, "_PHOTO") {
                    continue;
                }
                let Some(first) = r
                    .substructures
                    .iter()
                    .find(|c| is(c, "OBJE"))
                    .and_then(|c| c.pointer().map(str::to_string))
                else {
                    continue;
                };
                r.substructures.push(pointer_node("_PHOTO", &first));
                let x = r.xref.clone();
                cx.fold(
                    rows,
                    FoldKind::Added,
                    false,
                    x.as_deref(),
                    "_PHOTO for the preferred image",
                );
            }
        }
        Op::Preferred(flags, media_type) => {
            let idx = if media_type {
                shared.index(records)
            } else {
                &Index::default()
            };
            for i in range {
                // The media type of each link, before the fold.
                let forms: Vec<Option<String>> = match records.get(i) {
                    Some(r) if is(r, "INDI") => {
                        if media_type {
                            let look = Lookup { records, idx };
                            r.substructures
                                .iter()
                                .filter(|c| is(c, "OBJE"))
                                .map(|o| media_form(o, look))
                                .collect()
                        } else {
                            Vec::new()
                        }
                    }
                    _ => continue,
                };
                let mut forms = forms.into_iter();
                let Some(r) = records.get_mut(i) else { break };
                let x = r.xref.clone();
                let mut first = true;
                for o in r.substructures.iter_mut().filter(|c| is(c, "OBJE")) {
                    if first {
                        for flag in flags {
                            if !has(o, flag) {
                                o.substructures.push(text_node(flag, "Y"));
                            }
                        }
                        first = false;
                        if !flags.is_empty() {
                            cx.fold(
                                rows,
                                FoldKind::Added,
                                false,
                                x.as_deref(),
                                format!("{} on the preferred image", flags.join(", ")),
                            );
                        }
                    }
                    let form = forms.next().flatten();
                    if media_type && !has(o, "_TYPE") {
                        let photo = form.is_some_and(|f| {
                            let f = f.to_ascii_lowercase();
                            f.starts_with("image/")
                                || matches!(
                                    f.as_str(),
                                    "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tif" | "tiff"
                                )
                        });
                        o.substructures
                            .push(text_node("_TYPE", if photo { "PHOTO" } else { "OTHER" }));
                    }
                }
            }
        }
        Op::PrefixAsTitle => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let x = r.xref.clone();
                let mut titles = Vec::new();
                for n in r.substructures.iter_mut().filter(|c| is(c, "NAME")).take(1) {
                    if let Some(at) = n.substructures.iter().position(|s| is(s, "NPFX")) {
                        let p = n.substructures.remove(at);
                        titles.push(payload_str(&p).trim().to_string());
                    }
                    if let Some(sfx) = child_text(n, "NSFX").map(str::to_string) {
                        let value = payload_str(n).to_string();
                        if suffix_part(&value).is_empty()
                            && let Some(s) = surname_part(&value)
                        {
                            let v = name_value(given_part(&value), s, sfx.trim());
                            set_text(n, &v);
                        }
                    }
                }
                for t in titles {
                    if !r
                        .substructures
                        .iter()
                        .any(|c| is(c, "TITL") && payload_str(c).trim() == t)
                    {
                        r.substructures.push(text_node("TITL", &t));
                    }
                    cx.fold(
                        rows,
                        FoldKind::Moved,
                        true,
                        x.as_deref(),
                        "the name prefix as a title",
                    );
                }
            }
        }
        Op::NameValueComplete => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let x = r.xref.clone();
                let mut notes = Vec::new();
                for n in r.substructures.iter_mut().filter(|c| is(c, "NAME")) {
                    let value = payload_str(n).to_string();
                    let givn = child_text(n, "GIVN").map(str::to_string);
                    let surn = child_text(n, "SURN").map(str::to_string);
                    if value.trim().is_empty() {
                        let v = name_value(
                            givn.as_deref().unwrap_or(""),
                            surn.as_deref().unwrap_or(""),
                            child_text(n, "NSFX").unwrap_or(""),
                        );
                        set_text(n, &v);
                        cx.fold(
                            rows,
                            FoldKind::Recoded,
                            false,
                            x.as_deref(),
                            "a name value made from its pieces",
                        );
                        continue;
                    }
                    let disagree = givn
                        .as_deref()
                        .is_some_and(|g| !given_part(&value).contains(g.trim()))
                        || surn.as_deref().is_some_and(|s| {
                            surname_part(&value).is_none_or(|v| {
                                !v.contains(s.split(',').next().unwrap_or(s).trim())
                            })
                        });
                    if disagree {
                        let pieces: Vec<String> = n
                            .substructures
                            .iter()
                            .filter(|s| matches!(s.tag.as_str(), "GIVN" | "SURN"))
                            .map(|s| format!("{} {}", s.tag, payload_str(s)))
                            .collect();
                        n.substructures
                            .retain(|s| !matches!(s.tag.as_str(), "GIVN" | "SURN"));
                        notes.push(format!(
                            "Name pieces of {}: {}",
                            value.trim(),
                            pieces.join(", ")
                        ));
                    }
                }
                for note in notes {
                    r.substructures.push(text_node("NOTE", &note));
                    cx.fold(
                        rows,
                        FoldKind::AsText,
                        true,
                        x.as_deref(),
                        "name pieces that disagree with the name in a note",
                    );
                }
            }
        }
        Op::UsualNameAsNick | Op::UsualNameAsNote => {
            let as_nick = matches!(step.fold, Op::UsualNameAsNick);
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let x = r.xref.clone();
                let mut notes = Vec::new();
                for n in r.substructures.iter_mut().filter(|c| is(c, "NAME")) {
                    while let Some(at) = n
                        .substructures
                        .iter()
                        .position(|s| is(s, "_RUFNAME") || is(s, "_USED"))
                    {
                        let u = n.substructures.remove(at);
                        let v = payload_str(&u).trim().to_string();
                        if as_nick {
                            match child_mut(n, "NICK") {
                                Some(k) => {
                                    let joined = format!("{}, {v}", payload_str(k));
                                    set_text(k, &joined);
                                }
                                None => n.substructures.push(text_node("NICK", &v)),
                            }
                        } else {
                            notes.push(format!("Given name used: {v}"));
                        }
                        cx.fold(
                            rows,
                            if as_nick {
                                FoldKind::Merged
                            } else {
                                FoldKind::AsText
                            },
                            true,
                            x.as_deref(),
                            "the usual given name",
                        );
                    }
                }
                for note in notes {
                    r.substructures.push(text_node("NOTE", &note));
                }
            }
        }
        Op::IdentifiersAsNote(tags) => {
            for r in records.iter_mut() {
                let x = r.xref.clone();
                let mut lines = Vec::new();
                for tag in tags {
                    while let Some(at) = r.substructures.iter().position(|c| is(c, tag)) {
                        let c = r.substructures.remove(at);
                        let ty = child_text(&c, "TYPE")
                            .map(|t| format!(" ({t})"))
                            .unwrap_or_default();
                        lines.push(format!("Custom Id: {}{ty}", payload_str(&c)));
                    }
                }
                if !lines.is_empty() && !is(r, "HEAD") {
                    r.substructures.push(text_node("NOTE", &lines.join("\n")));
                    cx.fold(
                        rows,
                        FoldKind::AsText,
                        true,
                        x.as_deref(),
                        "identifiers in a note",
                    );
                }
            }
        }
        Op::InterpretedDatesAsNote => {
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                for e in &mut r.substructures {
                    let Some(d) = child_mut(e, "DATE") else {
                        continue;
                    };
                    let v = payload_str(d).trim().to_string();
                    let Some(rest) = v.strip_prefix("INT ") else {
                        continue;
                    };
                    let Some((date, phrase)) = rest.split_once('(') else {
                        continue;
                    };
                    let phrase = phrase.trim_end_matches(')').trim().to_string();
                    set_text(d, date.trim());
                    e.substructures.push(text_node("NOTE", &phrase));
                    cx.fold(
                        rows,
                        FoldKind::AsText,
                        true,
                        x.as_deref(),
                        "an interpreted date's phrase in the note",
                    );
                }
            }
        }
        Op::FlatHead => {
            if let Some(head) = shared.head(records, range).and_then(|i| records.get_mut(i)) {
                let mut changed = false;
                flatten_lines(head, &mut changed);
                if changed {
                    cx.fold(
                        rows,
                        FoldKind::Recoded,
                        true,
                        None,
                        "line breaks in the header as spaces",
                    );
                }
            }
        }
        Op::PafUids => {
            for r in records.iter_mut() {
                let x = r.xref.clone();
                let mut n = 0;
                let v7 = cx.v7;
                for tag in ["UID", "_UID"] {
                    each_tag(r, tag, &mut |u| {
                        // 5.5.1 says it `_UID`.
                        if !v7 {
                            retag(u, "_UID");
                        }
                        if let Some(paf) = paf_uid(payload_str(u)) {
                            set_text(u, &paf);
                            n += 1;
                        }
                    });
                }
                if n > 0 {
                    cx.fold(
                        rows,
                        FoldKind::Recoded,
                        false,
                        x.as_deref(),
                        "_UID in the PAF form",
                    );
                }
            }
        }
        Op::Retag(map) => {
            for r in records.iter_mut() {
                let x = r.xref.clone();
                for &(from, to) in map {
                    let mut n = 0;
                    each_tag(r, from, &mut |s| {
                        retag(s, to);
                        n += 1;
                    });
                    if n > 0 {
                        cx.fold(
                            rows,
                            FoldKind::Relabelled,
                            false,
                            x.as_deref(),
                            format!("{from} as {to}"),
                        );
                    }
                }
            }
        }
        Op::MarriageStatus => {
            for r in records.iter_mut().filter(|r| is(r, "FAM")) {
                let x = r.xref.clone();
                let never = |c: &Draft<'s>| {
                    (is(c, "NO") || is(c, "_NO")) && payload_str(c).trim() == "MARR"
                };
                let mut k = 0;
                let mut n = 0;
                while k < r.substructures.len() {
                    if !r.substructures.get(k).is_some_and(never) {
                        k += 1;
                        continue;
                    }
                    let stated = has(r, "_STAT");
                    let empty = r.substructures.get(k).is_some_and(bare);
                    if stated && empty {
                        r.substructures.remove(k);
                    } else if let Some(c) = r.substructures.get_mut(k) {
                        // Its date, notes and citations stay with it.
                        retag(c, "_STAT");
                        set_text(c, "Never Married");
                        k += 1;
                    }
                    n += 1;
                }
                if n > 0 {
                    cx.fold(
                        rows,
                        FoldKind::Relabelled,
                        false,
                        x.as_deref(),
                        "never married as _STAT Never Married",
                    );
                }
            }
        }
        Op::NoteSourcesLifted => {
            for r in records.iter_mut() {
                let x = r.xref.clone();
                let mut n = 0;
                lift_note_sources(r, &mut n);
                if n > 0 {
                    cx.fold(
                        rows,
                        FoldKind::Moved,
                        true,
                        x.as_deref(),
                        format!("{n} citations of notes lifted to their structure"),
                    );
                }
            }
        }
        Op::ContactsJoined | Op::FaxAsPhone => {
            let join = matches!(step.fold, Op::ContactsJoined);
            for r in records.iter_mut() {
                let x = r.xref.clone();
                let mut n = 0;
                join_contacts(r, join, &mut n);
                if n > 0 {
                    cx.fold(
                        rows,
                        FoldKind::Merged,
                        true,
                        x.as_deref(),
                        "contact details joined",
                    );
                }
            }
        }
        Op::HomeFirst => {
            let home = cx.options.home.clone().or_else(|| {
                records
                    .iter()
                    .find(|r| is(r, "HEAD"))
                    .and_then(|h| child(h, "_ROOT"))
                    .and_then(|r| r.pointer().map(str::to_string))
            });
            if let Some(home) = home {
                let first_indi = records.iter().position(|r| is(r, "INDI"));
                let at = records
                    .iter()
                    .position(|r| is(r, "INDI") && r.xref.as_deref().is_some_and(|x| x == home));
                if let (Some(first), Some(at)) = (first_indi, at)
                    && at != first
                {
                    let x = records.get(at).and_then(|r| r.xref.clone());
                    if let Some(moved) = records.get_mut(first..=at) {
                        moved.rotate_right(1);
                    }
                    cx.fold(
                        rows,
                        FoldKind::Reordered,
                        false,
                        x.as_deref(),
                        "the home person first",
                    );
                }
            }
        }
        Op::BareFactsNoted => {
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                for e in &mut r.substructures {
                    let fact = is_y_event(e.tag.as_str())
                        || matches!(
                            e.tag.as_str(),
                            "EVEN"
                                | "FACT"
                                | "OCCU"
                                | "RESI"
                                | "EDUC"
                                | "RELI"
                                | "NATI"
                                | "TITL"
                                | "DSCR"
                                | "PROP"
                                | "CAST"
                        );
                    if fact && !has(e, "DATE") && !has(e, "PLAC") && !has(e, "NOTE") {
                        e.substructures
                            .push(text_node("NOTE", "No recorded date or place"));
                        cx.fold(
                            rows,
                            FoldKind::Added,
                            false,
                            x.as_deref(),
                            format!("a note on {} so that it is kept", e.tag),
                        );
                    }
                }
            }
        }
        Op::NamesInAka => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let x = r.xref.clone();
                let names: Vec<usize> = r
                    .substructures
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| is(c, "NAME"))
                    .map(|(i, _)| i)
                    .collect();
                if names.len() < 2 {
                    continue;
                }
                let others: Vec<String> = names
                    .iter()
                    .skip(1)
                    .filter_map(|&i| r.substructures.get(i).map(|n| payload_str(n).to_string()))
                    .collect();
                for &i in names.iter().skip(1).rev() {
                    r.substructures.remove(i);
                }
                if let Some(first) = names.first().and_then(|&i| r.substructures.get_mut(i)) {
                    // One `_AKA`: the names it held already, then the others,
                    // each once.
                    let mut all: Vec<String> = Vec::new();
                    for aka in first.substructures.iter().filter(|s| is(s, "_AKA")) {
                        let v = payload_str(aka).trim();
                        let parts: Vec<&str> = v.split([',', ';']).map(str::trim).collect();
                        if parts.len() > 1 && parts.iter().all(|p| p.contains('/')) {
                            all.extend(parts.into_iter().map(str::to_string));
                        } else if !v.is_empty() {
                            all.push(v.to_string());
                        }
                    }
                    for o in &others {
                        if !all.contains(o) {
                            all.push(o.clone());
                        }
                    }
                    let mut seen = Vec::new();
                    all.retain(|v| {
                        let fresh = !seen.contains(v);
                        seen.push(v.clone());
                        fresh
                    });
                    first.substructures.retain(|s| !is(s, "_AKA"));
                    first.substructures.push(text_node("_AKA", &all.join(", ")));
                }
                cx.fold(
                    rows,
                    FoldKind::Merged,
                    true,
                    x.as_deref(),
                    format!("{} other names in one _AKA", others.len()),
                );
            }
        }
        Op::NamesWithSlashes => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                let x = r.xref.clone();
                for n in r.substructures.iter_mut().filter(|c| is(c, "NAME")) {
                    let v = payload_str(n).trim().to_string();
                    if !v.is_empty() && !v.contains('/') {
                        set_text(n, &format!("{v} //"));
                        cx.fold(
                            rows,
                            FoldKind::Recoded,
                            false,
                            x.as_deref(),
                            "a name given empty surname slashes",
                        );
                    }
                }
            }
        }
        Op::ShortXrefs(max) => short_xrefs(records, rows, max, cx),
        Op::XreflessRecords => {
            for r in records.iter() {
                if r.xref.is_none() && !matches!(r.tag.as_str(), "HEAD" | "TRLR") {
                    let detail = format!(
                        "a {} record without identifier, which the importer drops",
                        r.tag
                    );
                    cx.limit(rows, None, detail);
                }
            }
        }
        Op::OnePlace => {
            for r in records.iter_mut().filter(|r| is(r, "INDI") || is(r, "FAM")) {
                let x = r.xref.clone();
                for e in &mut r.substructures {
                    let places: Vec<usize> = e
                        .substructures
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| is(s, "PLAC"))
                        .map(|(i, _)| i)
                        .collect();
                    for &i in places.iter().skip(1).rev() {
                        let p = e.substructures.remove(i);
                        e.substructures
                            .push(text_node("NOTE", &format!("Place: {}", payload_str(&p))));
                        cx.fold(
                            rows,
                            FoldKind::AsText,
                            true,
                            x.as_deref(),
                            "a second place as a note",
                        );
                    }
                }
            }
        }
        Op::CitationRefnToSource => {
            let mut moves: Vec<(String, Draft<'s>)> = Vec::new();
            for r in records.iter_mut() {
                collect_citation_refns(r, &mut moves);
            }
            let idx = index(records);
            for (source, refn) in moves {
                if let Some(s) = idx.get(source.as_str()).and_then(|&i| records.get_mut(i)) {
                    let x = s.xref.clone();
                    if !s.substructures.contains(&refn) {
                        s.substructures.push(refn);
                    }
                    cx.fold(
                        rows,
                        FoldKind::Moved,
                        true,
                        x.as_deref(),
                        "a citation's REFN on its source",
                    );
                }
            }
        }
        Op::UnionsAsMarriage => {
            for r in records.iter_mut().filter(|r| is(r, "FAM")) {
                let x = r.xref.clone();
                for c in &mut r.substructures {
                    let ty = child_text(c, "TYPE").map(|t| t.trim().to_ascii_lowercase());
                    let label = if (is(c, "NO") || is(c, "_NO")) && payload_str(c).trim() == "MARR"
                    {
                        Some("Unmarried")
                    } else if is(c, "EVEN")
                        && ty.as_deref().is_some_and(|t| {
                            matches!(t, "pacs" | "civil union" | "registered partnership")
                        })
                    {
                        Some("Civil Union")
                    } else if is(c, "EVEN") && ty.as_deref() == Some("unmarried") {
                        Some("Unmarried")
                    } else {
                        None
                    };
                    if let Some(label) = label {
                        retag(c, "MARR");
                        c.payload = Value::None;
                        c.substructures
                            .retain(|s| !matches!(s.tag.as_str(), "TYPE" | "PHRASE"));
                        c.substructures.insert(0, text_node("TYPE", label));
                        cx.fold(
                            rows,
                            FoldKind::Relabelled,
                            false,
                            x.as_deref(),
                            format!("a union as MARR TYPE {label}"),
                        );
                    }
                }
            }
        }
        Op::SexSaid => {
            for r in records.iter_mut().filter(|r| is(r, "INDI")) {
                if has(r, "SEX") {
                    continue;
                }
                let at = r
                    .substructures
                    .iter()
                    .rposition(|c| is(c, "NAME"))
                    .map_or(0, |k| k + 1);
                r.substructures.insert(at, text_node("SEX", "U"));
                let x = r.xref.clone();
                cx.fold(
                    rows,
                    FoldKind::Added,
                    false,
                    x.as_deref(),
                    "SEX U for an unknown sex",
                );
            }
        }
        Op::GodparentText => {
            let idx = shared.index(records);
            for i in range {
                let godparents: Vec<String> = match records.get(i) {
                    Some(r) if is(r, "INDI") => {
                        let look = Lookup { records, idx };
                        r.substructures
                            .iter()
                            .filter(|c| {
                                is(c, "ASSO")
                                    && child_text(c, "RELA")
                                        .is_some_and(|t| t.to_ascii_lowercase().starts_with("god"))
                            })
                            .filter_map(|c| c.pointer().and_then(|p| look.person_name(p)))
                            .collect()
                    }
                    _ => continue,
                };
                let Some(r) = records.get_mut(i) else { break };
                let x = r.xref.clone();
                if godparents.is_empty() {
                    continue;
                }
                if let Some(bapm) = r
                    .substructures
                    .iter_mut()
                    .find(|c| is(c, "BAPM") || is(c, "CHR"))
                    && !has(bapm, "_GODP")
                {
                    bapm.substructures
                        .push(text_node("_GODP", &godparents.join(", ")));
                    cx.fold(
                        rows,
                        FoldKind::Added,
                        false,
                        x.as_deref(),
                        "_GODP beside the godparents",
                    );
                }
            }
        }
    }
}

fn fix_colons(s: &mut Draft<'_>, changed: &mut bool) {
    if let Some(t) = s.text()
        && t.contains(':')
    {
        let fixed = t.replace(':', " ");
        set_text(
            s,
            fixed
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .as_str(),
        );
        *changed = true;
    }
    for c in s.substructures.iter_mut().filter(|c| {
        matches!(
            c.tag.as_str(),
            "GIVN" | "SURN" | "NICK" | "SPFX" | "NPFX" | "NSFX"
        )
    }) {
        fix_colons(c, changed);
    }
}

/// Calls `f` on every note text (`NOTE` with text) under `s`.
fn each_note<'s>(s: &mut Draft<'s>, f: &mut dyn FnMut(&mut Draft<'s>)) {
    for c in &mut s.substructures {
        if is(c, "NOTE") && c.text().is_some() {
            f(c);
        }
        each_note(c, f);
    }
    if is(s, "NOTE") && s.xref.is_some() {
        f(s);
    }
}

fn each_tag<'s>(s: &mut Draft<'s>, tag: &str, f: &mut dyn FnMut(&mut Draft<'s>)) {
    for c in &mut s.substructures {
        if is(c, tag) {
            f(c);
        }
        each_tag(c, tag, f);
    }
}

fn each_date<'s>(s: &'s Draft<'s>, f: &mut dyn FnMut(&'s str)) {
    for c in &s.substructures {
        if is(c, "DATE")
            && let Some(t) = c.text()
        {
            f(t);
        }
        each_date(c, f);
    }
}

/// Whether a date names a day its month does not have.
pub(super) fn impossible_date(d: &str) -> bool {
    const MONTHS: [(&str, u32); 12] = [
        ("JAN", 31),
        ("FEB", 29),
        ("MAR", 31),
        ("APR", 30),
        ("MAY", 31),
        ("JUN", 30),
        ("JUL", 31),
        ("AUG", 31),
        ("SEP", 30),
        ("OCT", 31),
        ("NOV", 30),
        ("DEC", 31),
    ];
    let impossible = |day: &str, month: &str, year: &str| {
        let (Ok(day), Ok(year)) = (day.parse::<u32>(), year.parse::<i64>()) else {
            return false;
        };
        MONTHS
            .iter()
            .find(|(m, _)| m.eq_ignore_ascii_case(month))
            .is_some_and(|&(m, max)| {
                let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
                day == 0 || day > if m == "FEB" && !leap { 28 } else { max }
            })
    };
    // Each three words in a row.
    let mut words = d.split_whitespace();
    let (Some(mut day), Some(mut month)) = (words.next(), words.next()) else {
        return false;
    };
    for year in words {
        if impossible(day, month, year) {
            return true;
        }
        (day, month) = (month, year);
    }
    false
}

/// `<` and a bare `&` escaped as HTML entities.
fn escape_markup(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let bytes = t.as_bytes();
    for (i, c) in t.char_indices() {
        match c {
            // Markup (a tag, a closing tag, a comment) stays: the importer
            // reads notes as HTML.
            '<' if t
                .get(i + 1..)
                .and_then(|r| r.chars().next())
                .is_some_and(|n| n.is_ascii_alphabetic() || n == '/' || n == '!') =>
            {
                out.push('<');
            }
            '<' => out.push_str("&lt;"),
            '&' => {
                // An entity already: kept.
                let rest = bytes.get(i + 1..).unwrap_or_default();
                let end = rest.iter().position(|&b| b == b';');
                let entity = end.is_some_and(|e| {
                    e > 0
                        && e < 10
                        && rest.get(..e).is_some_and(|n| {
                            n.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'#')
                        })
                });
                if entity {
                    out.push('&');
                } else {
                    out.push_str("&amp;");
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn looks_html(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    [
        "<p>", "<br", "<b>", "<i>", "<u>", "<a ", "<div", "</", "&lt;", "&amp;", "&nbsp;",
    ]
    .iter()
    .any(|m| l.contains(m))
}

/// Text without HTML markup: line breaks for `<br>` and paragraphs,
/// entities decoded.
fn strip_html(t: &str) -> String {
    // Escaped markup (`&lt;i&gt;`) is markup too.
    let decoded = decode_entities(t);
    let t = decoded.as_str();
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(start) = rest.find('<') {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start..).unwrap_or_default();
        let Some(end) = after.find('>') else {
            out.push_str(after);
            rest = "";
            break;
        };
        let tag = after
            .get(1..end)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if tag.starts_with("br") || tag == "/p" || tag == "/div" {
            out.push('\n');
        }
        rest = after.get(end + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out.trim_end_matches('\n').to_string()
}

/// HTML entities decoded once.
fn decode_entities(t: &str) -> String {
    t.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// A structure and its substructures as one line of text.
fn flatten(s: &Draft<'_>) -> String {
    let mut parts = Vec::new();
    if let Some(v) = s.payload.as_str() {
        parts.push(v.to_string());
    }
    for c in &s.substructures {
        parts.push(format!("{} {}", c.tag, flatten(c)));
    }
    parts.join("; ")
}

/// An address as one line.
fn address_text(addr: &Draft<'_>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = addr.text() {
        parts.extend(
            t.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string),
        );
    }
    if parts.is_empty() {
        for c in &addr.substructures {
            if let Some(t) = c.text() {
                parts.push(t.trim().to_string());
            }
        }
    }
    parts.join(", ")
}

/// Whether the 5.5.1 structure at `path` (tags from the record down)
/// takes a note: a record but a note record, an event, attribute or link
/// of a person or family, the place of such an event, a citation.
fn takes_note(path: &[&str]) -> bool {
    let event_like = |t: &str| {
        is_y_event(t)
            || matches!(
                t,
                "EVEN"
                    | "FACT"
                    | "OCCU"
                    | "RESI"
                    | "EDUC"
                    | "RELI"
                    | "NATI"
                    | "TITL"
                    | "DSCR"
                    | "PROP"
                    | "CAST"
                    | "IDNO"
                    | "NCHI"
                    | "NMR"
                    | "SSN"
                    | "BAPL"
                    | "CONL"
                    | "ENDL"
                    | "SLGC"
                    | "SLGS"
            )
    };
    match path {
        [record] => matches!(*record, "INDI" | "FAM" | "SOUR" | "REPO" | "OBJE" | "SUBM"),
        ["INDI" | "FAM", t] => event_like(t) || matches!(*t, "NAME" | "FAMC" | "FAMS" | "ASSO"),
        ["INDI" | "FAM", e, "PLAC"] => event_like(e),
        [first, .., "SOUR"] => *first != "SOUR",
        _ => false,
    }
}

/// Every extension structure under `s` (at `path`) as a note of the
/// nearest structure that takes one: the texts that found none, for the
/// caller; `lost` the tags folded (and whether as a note).
/// Whether a structure below `s` has an extension tag.
fn has_extension_below(s: &Draft<'_>) -> bool {
    s.substructures
        .iter()
        .any(|c| c.tag.is_extension() || has_extension_below(c))
}

fn extensions_as_notes(
    s: &mut Draft<'_>,
    path: &mut Vec<String>,
    lost: &mut Vec<(String, bool)>,
) -> Vec<String> {
    path.push(s.tag.as_str().to_string());
    let mut k = 0;
    let mut texts = Vec::new();
    while k < s.substructures.len() {
        let ext = s.substructures.get(k).is_some_and(|c| c.tag.is_extension());
        if ext {
            let c = s.substructures.remove(k);
            texts.push(format!("{}: {}", c.tag, flatten(&c)));
            continue;
        }
        if let Some(c) = s.substructures.get_mut(k) {
            texts.extend(extensions_as_notes(c, path, lost));
        }
        k += 1;
    }
    let tags: Vec<&str> = path.iter().map(String::as_str).collect();
    let tag_of = |t: &str| t.split(':').next().unwrap_or_default().to_string();
    let rest = if takes_note(&tags) {
        for t in texts {
            lost.push((tag_of(&t), true));
            s.substructures.push(text_node("NOTE", &t));
        }
        Vec::new()
    } else if tags.len() == 1 {
        lost.extend(texts.iter().map(|t| (tag_of(t), false)));
        Vec::new()
    } else {
        texts
    };
    path.pop();
    rest
}

fn flatten_lines(s: &mut Draft<'_>, changed: &mut bool) {
    if let Some(t) = s.text()
        && t.contains('\n')
    {
        let flat = t.split('\n').map(str::trim).collect::<Vec<_>>().join(" ");
        set_text(s, &flat);
        *changed = true;
    }
    for c in &mut s.substructures {
        flatten_lines(c, changed);
    }
}

/// A `_UID` in the PAF form (32 hexadecimal digits and the 4-digit
/// checksum), when `value` is a UUID without the checksum.
fn paf_uid(value: &str) -> Option<String> {
    let hex: String = value
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .chars()
        .filter(|&c| c != '-')
        .collect();
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (mut a, mut b) = (0_u32, 0_u32);
    for i in (0..32).step_by(2) {
        let byte = u32::from_str_radix(hex.get(i..i + 2)?, 16).ok()?;
        a += byte;
        b += a & 0xFF;
    }
    Some(format!(
        "{}{:02X}{:02X}",
        hex.to_ascii_uppercase(),
        a & 0xFF,
        b & 0xFF
    ))
}

fn lift_note_sources<'s>(s: &mut Draft<'s>, n: &mut usize) {
    let mut lifted = Vec::new();
    for c in &mut s.substructures {
        if is(c, "NOTE") {
            let sources: Vec<Draft<'s>> = c
                .substructures
                .iter()
                .filter(|x| is(x, "SOUR"))
                .cloned()
                .collect();
            if !sources.is_empty() {
                c.substructures.retain(|x| !is(x, "SOUR"));
                *n += sources.len();
                lifted.extend(sources);
            }
        } else {
            lift_note_sources(c, n);
        }
    }
    s.substructures.extend(lifted);
}

fn join_contacts(s: &mut Draft<'_>, join: bool, n: &mut usize) {
    for c in &mut s.substructures {
        join_contacts(c, join, n);
    }
    // FAX as PHON.
    let faxes: Vec<String> = s
        .substructures
        .iter()
        .filter(|c| is(c, "FAX"))
        .map(|c| payload_str(c).to_string())
        .collect();
    if !faxes.is_empty() {
        s.substructures.retain(|c| !is(c, "FAX"));
        for f in faxes {
            s.substructures
                .push(text_node("PHON", &format!("{f} (fax)")));
            *n += 1;
        }
    }
    if !join {
        return;
    }
    for (tag, also) in [("EMAIL", Some("WWW")), ("PHON", None)] {
        let values: Vec<String> = s
            .substructures
            .iter()
            .filter(|c| is(c, tag) || also.is_some_and(|a| is(c, a)))
            .map(|c| payload_str(c).to_string())
            .collect();
        if values.len() < 2 {
            continue;
        }
        let at = s
            .substructures
            .iter()
            .position(|c| is(c, tag) || also.is_some_and(|a| is(c, a)))
            .unwrap_or(0);
        s.substructures
            .retain(|c| !(is(c, tag) || also.is_some_and(|a| is(c, a))));
        s.substructures.insert(
            at.min(s.substructures.len()),
            text_node(tag, &values.join("; ")),
        );
        *n += 1;
    }
}

fn collect_citation_refns<'s>(s: &mut Draft<'s>, out: &mut Vec<(String, Draft<'s>)>) {
    for c in &mut s.substructures {
        if is(c, "SOUR")
            && let Some(p) = c.pointer().map(str::to_string)
        {
            let refns: Vec<Draft<'s>> = c
                .substructures
                .iter()
                .filter(|x| is(x, "REFN"))
                .cloned()
                .collect();
            if !refns.is_empty() {
                c.substructures.retain(|x| !is(x, "REFN"));
                out.extend(refns.into_iter().map(|r| (p.clone(), r)));
            }
        }
        collect_citation_refns(c, out);
    }
}

/// The media type or format of a multimedia link (its own `FILE.FORM`, or
/// its record's).
fn media_form(link: &Draft<'_>, look: Lookup<'_, '_>) -> Option<String> {
    match link.pointer() {
        Some(p) => {
            let record = look.get(p)?;
            record
                .child("FILE")
                .and_then(|f| child_text(f, "FORM"))
                .or_else(|| record.child_text("FORM"))
                .map(str::to_string)
        }
        None => child(link, "FILE")
            .and_then(|f| child_text(f, "FORM"))
            .or_else(|| child_text(link, "FORM"))
            .map(str::to_string),
    }
}

/// A citation as one text: the source's title, the page, the repository.
fn citation_text(c: &Draft<'_>, look: Lookup<'_, '_>) -> String {
    // The parts that say something, `, ` between; line breaks as spaces
    // in those that are `flat`.
    let mut out = String::new();
    let mut part = |p: &str, flat: bool| {
        if p.trim().is_empty() {
            return;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        if flat && p.contains('\n') {
            out.push_str(&p.replace('\n', " "));
        } else {
            out.push_str(p);
        }
    };
    match c.pointer() {
        Some(p) => {
            if let Some(s) = look.get(p) {
                if let Some(title) = s.child_text("TITL").or_else(|| s.child_text("ABBR")) {
                    part(title, true);
                }
                if let Some(auth) = s.child_text("AUTH") {
                    part(auth, true);
                }
                for repo in s.children().filter(|r| is(r, "REPO")) {
                    if let Some(name) = repo
                        .pointer()
                        .and_then(|r| look.get(r))
                        .and_then(|r| r.child_text("NAME"))
                    {
                        part(name, false);
                    }
                    if let Some(caln) = child_text(repo, "CALN") {
                        part(caln, false);
                    }
                }
            }
        }
        None => {
            if let Some(t) = c.text() {
                part(t, true);
            }
        }
    }
    if let Some(page) = child_text(c, "PAGE") {
        part(page, true);
    }
    out
}

/// Every citation of a source record written inline.
fn inline_sources<'s>(
    records: &mut [Draft<'s>],
    range: Range<usize>,
    rows: &'static [&'static str],
    cx: &mut Cx<'_>,
    shared: &mut Shared<'s>,
) {
    let idx = shared.index(records);
    for i in range {
        let mut texts = Vec::new();
        match records.get(i) {
            Some(r) if !matches!(r.tag.as_str(), "HEAD" | "SOUR" | "REPO" | "TRLR") => {
                let look = Lookup { records, idx };
                citation_texts(&r.substructures, look, &mut texts);
            }
            _ => continue,
        }
        let Some(r) = records.get_mut(i) else { break };
        let x = r.xref.clone();
        let mut n = 0;
        let mut lossy = false;
        inline_citations(r, &mut texts.into_iter(), &mut n, &mut lossy);
        if n > 0 {
            cx.fold(
                rows,
                FoldKind::AsText,
                lossy,
                x.as_deref(),
                format!("{n} citations of source records written inline"),
            );
        }
    }
}

/// The texts of the citations of records [`inline_citations`] writes
/// inline among `structures`, in the order it writes them.
fn citation_texts<'s>(structures: &[Draft<'s>], look: Lookup<'_, 's>, out: &mut Vec<String>) {
    for c in structures {
        citation_texts_in(c, look, out);
    }
}

fn citation_texts_in<'s>(c: &Draft<'s>, look: Lookup<'_, 's>, out: &mut Vec<String>) {
    if !(is(c, "SOUR") && c.pointer().is_some()) {
        citation_texts(&c.substructures, look, out);
        return;
    }
    out.push(citation_text(c, look));
    // The substructures it keeps, in their order.
    for sub in &c.substructures {
        match sub.tag.as_str() {
            "DATA" => {
                for d in sub.substructures.iter().filter(|d| is(d, "TEXT")) {
                    citation_texts_in(d, look, out);
                }
            }
            "NOTE" | "QUAY" | "OBJE" => citation_texts_in(sub, look, out),
            _ => {}
        }
    }
}

fn inline_citations(
    s: &mut Draft<'_>,
    texts: &mut std::vec::IntoIter<String>,
    n: &mut usize,
    lossy: &mut bool,
) {
    for c in &mut s.substructures {
        if is(c, "SOUR") && c.pointer().is_some() {
            let text = texts.next().unwrap_or_default();
            let mut inline = text_node("SOUR", if text.is_empty() { "Source" } else { &text });
            for sub in &c.substructures {
                match sub.tag.as_str() {
                    "PAGE" => {}
                    "DATA" => {
                        for d in &sub.substructures {
                            match d.tag.as_str() {
                                "TEXT" => inline.substructures.push(d.clone()),
                                _ => *lossy = true,
                            }
                        }
                    }
                    "NOTE" | "QUAY" | "OBJE" => inline.substructures.push(sub.clone()),
                    _ => *lossy = true,
                }
            }
            // Citations in its notes too, now that it is written inline.
            inline_citations(&mut inline, texts, n, lossy);
            *c = inline;
            *n += 1;
        } else {
            inline_citations(c, texts, n, lossy);
        }
    }
}

/// Every text citation made a pointer to a new source record.
fn source_records<'s>(
    records: &mut Vec<Draft<'s>>,
    rows: &'static [&'static str],
    cx: &mut Cx<'_>,
) {
    let mut new: Vec<Draft<'s>> = Vec::new();
    let mut taken: Vec<String> = records
        .iter()
        .filter_map(|r| r.xref.as_ref().map(std::string::ToString::to_string))
        .collect();
    let mut made: HashMap<String, Xref> = HashMap::new();
    for r in records
        .iter_mut()
        .filter(|r| !matches!(r.tag.as_str(), "HEAD" | "TRLR"))
    {
        let x = r.xref.clone();
        let mut n = 0;
        source_pointers(
            r,
            &mut |title| {
                if let Some(x) = made.get(title) {
                    return x.clone();
                }
                let xref = loop {
                    cx.next_xref += 1;
                    let candidate = format!("@S{}@", cx.next_xref);
                    if !taken.contains(&candidate) {
                        break Xref::new(candidate);
                    }
                };
                taken.push(xref.as_str().to_string());
                let mut s = Draft::new("SOUR");
                s.xref = Some(Cow::Owned(xref.as_str().to_string()));
                s.substructures.push(text_node("TITL", title));
                new.push(s);
                made.insert(title.to_string(), xref.clone());
                xref
            },
            &mut n,
        );
        if n > 0 {
            cx.fold(
                rows,
                FoldKind::Moved,
                false,
                x.as_deref(),
                format!("{n} text citations made source records"),
            );
        }
    }
    let at = records
        .iter()
        .rposition(|r| !is(r, "TRLR"))
        .map_or(records.len(), |k| k + 1);
    for (k, s) in new.into_iter().enumerate() {
        records.insert(at + k, s);
    }
}

fn source_pointers<'s>(s: &mut Draft<'s>, make: &mut dyn FnMut(&str) -> Xref, n: &mut usize) {
    for c in &mut s.substructures {
        if is(c, "SOUR")
            && let Value::Text(t) = &c.payload
        {
            let title = t.to_string();
            let x = make(&title);
            c.payload = Value::Pointer(Cow::Owned(x.as_str().to_string()));
            // The text's own lines are the title now; TEXT goes under DATA.
            let texts: Vec<Draft<'s>> = c
                .substructures
                .iter()
                .filter(|s| is(s, "TEXT"))
                .cloned()
                .collect();
            if !texts.is_empty() {
                c.substructures.retain(|s| !is(s, "TEXT"));
                let mut data = Draft::new("DATA");
                data.substructures.extend(texts);
                c.substructures.push(data);
            }
            *n += 1;
            continue;
        }
        source_pointers(c, make, n);
    }
}

/// Godparents of a baptism as person-level associations.
fn godparents<'s>(
    records: &mut [Draft<'s>],
    range: Range<usize>,
    rows: &'static [&'static str],
    cx: &mut Cx<'_>,
    shared: &mut Shared<'s>,
) {
    let idx = shared.index(records);
    for i in range {
        // The sex of each godparent, before the fold.
        let sexes: Vec<Option<String>> = match records.get(i) {
            Some(r) if is(r, "INDI") => {
                let look = Lookup { records, idx };
                r.substructures
                    .iter()
                    .filter(|e| matches!(e.tag.as_str(), "BAPM" | "CHR" | "CHRA"))
                    .flat_map(|e| &e.substructures)
                    .filter(|a| is_godparent_link(a))
                    .map(|a| a.pointer().and_then(|p| look.person_sex(p)))
                    .collect()
            }
            _ => continue,
        };
        let mut sexes = sexes.into_iter();
        let Some(r) = records.get_mut(i) else { break };
        let holder = r.xref.clone();
        let mut found: Vec<(Xref, String)> = Vec::new();
        for e in r
            .substructures
            .iter_mut()
            .filter(|e| matches!(e.tag.as_str(), "BAPM" | "CHR" | "CHRA"))
        {
            let mut k = 0;
            while k < e.substructures.len() {
                if e.substructures.get(k).is_some_and(is_godparent_link) {
                    let a = e.substructures.remove(k);
                    let sex = sexes.next().flatten();
                    if let Some(p) = a.pointer() {
                        let word = match sex.as_deref() {
                            Some("M") => "Godfather",
                            Some("F") => "Godmother",
                            _ => "Godparent",
                        };
                        found.push((Xref::new(p), word.to_string()));
                    }
                    continue;
                }
                k += 1;
            }
        }
        if found.is_empty() {
            continue;
        }
        // Godfathers first, each followed by a godmother.
        let (fathers, others): (Vec<_>, Vec<_>) =
            found.into_iter().partition(|(_, w)| w == "Godfather");
        let (mothers, rest): (Vec<_>, Vec<_>) =
            others.into_iter().partition(|(_, w)| w == "Godmother");
        let mut ordered = Vec::new();
        let mut m = mothers.into_iter();
        for f in fathers {
            ordered.push(f);
            if let Some(mo) = m.next() {
                ordered.push(mo);
            }
        }
        ordered.extend(m);
        ordered.extend(rest);
        let n = ordered.len();
        for (p, word) in ordered {
            let already = r.substructures.iter().any(|a| {
                is(a, "ASSO")
                    && a.pointer() == Some(&p)
                    && child_text(a, "RELA").is_some_and(|t| t.eq_ignore_ascii_case(&word))
            });
            if !already {
                let mut a = pointer_node("ASSO", &p);
                if cx.v7 {
                    let mut role = text_node("ROLE", "GODP");
                    role.substructures.push(text_node("PHRASE", &word));
                    a.substructures.push(role);
                } else {
                    a.substructures.push(text_node("RELA", &word));
                }
                r.substructures.push(a);
            }
        }
        cx.fold(
            rows,
            FoldKind::Moved,
            false,
            holder.as_deref(),
            format!("{n} godparents of a baptism as associations of the person"),
        );
    }
}

/// Whether `a` links a godparent: an association to a person.
fn is_godparent_link(a: &Draft<'_>) -> bool {
    (is(a, "ASSO") || is(a, "_ASSO"))
        && a.pointer().is_some_and(|p| p != Xref::VOID)
        && is_godparent(a)
}

fn is_godparent(a: &Draft<'_>) -> bool {
    let word = child_text(a, "RELA")
        .or_else(|| child_text(a, "ROLE"))
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let phrase = child(a, "ROLE")
        .and_then(|r| child_text(r, "PHRASE"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    word.starts_with("god") || word == "godp" || phrase.starts_with("god")
}

/// The event associations 5.5.1 has no place for, written in `style`.
fn event_associations<'s>(
    records: &mut [Draft<'s>],
    range: Range<usize>,
    rows: &'static [&'static str],
    style: AssocStyle,
    cx: &mut Cx<'_>,
    shared: &mut Shared<'s>,
) {
    if cx.v7 {
        return;
    }
    let idx = shared.index(records);
    let mut reversed: Vec<(String, Draft<'s>)> = Vec::new();
    for i in range {
        // The name of each associated person, before the fold.
        let names: Vec<Option<String>> = match records.get(i) {
            Some(r) if is(r, "INDI") || is(r, "FAM") => {
                let look = Lookup { records, idx };
                r.substructures
                    .iter()
                    .flat_map(|o| &o.substructures)
                    .filter(|c| is(c, "ASSO") || is(c, "_ASSO"))
                    .map(|c| association_target(c).and_then(|p| look.person_name(p.as_str())))
                    .collect()
            }
            _ => continue,
        };
        let mut names = names.into_iter();
        let Some(r) = records.get_mut(i) else { break };
        let holder = r.xref.clone();
        let owner = holder.clone();
        let record_tag = r.tag.clone();
        for occasion in &mut r.substructures {
            let event = occasion.tag.clone();
            let mut at = 0;
            let mut notes = Vec::new();
            while at < occasion.substructures.len() {
                let assoc = occasion
                    .substructures
                    .get(at)
                    .is_some_and(|c| is(c, "ASSO") || is(c, "_ASSO"));
                if !assoc {
                    at += 1;
                    continue;
                }
                let found = occasion.substructures.remove(at);
                let (role, target, name, rest) = association_parts(&found, names.next().flatten());
                let kept_as = match style {
                    AssocStyle::Shar => Some(("_SHAR", "ROLE")),
                    AssocStyle::Witn => Some(("_WITN", "TYPE")),
                    AssocStyle::GedcomL => Some(("_ASSO", "RELA")),
                    AssocStyle::Reversed | AssocStyle::Note => None,
                };
                match (style, target) {
                    (_, Some(person)) if kept_as.is_some() => {
                        let (tag, word) = kept_as.unwrap_or(("_ASSO", "RELA"));
                        let mut kept = pointer_node(tag, &person);
                        kept.substructures.push(text_node(word, &role));
                        kept.substructures.extend(rest);
                        occasion.substructures.insert(at, kept);
                        at += 1;
                        cx.fold(
                            rows,
                            FoldKind::Relabelled,
                            false,
                            holder.as_deref(),
                            format!("a witness of {event} as {tag}"),
                        );
                    }
                    (AssocStyle::Reversed, Some(p)) => {
                        if let Some(owner) = &owner {
                            let mut s = pointer_node("ASSO", owner);
                            s.substructures
                                .push(text_node("RELA", &format!("{role}@{record_tag}:{event}")));
                            s.substructures.extend(rest);
                            reversed.push((p.as_str().to_string(), s));
                            cx.fold(rows, FoldKind::Moved, false, holder.as_deref(), format!("a witness of {event} on the witness, RELA {role}@{record_tag}:{event}"));
                        }
                    }
                    (_, _) => {
                        let text = if name.is_empty() {
                            role.clone()
                        } else {
                            format!("{name} ({role})")
                        };
                        notes.push(format!("Witness: {text}"));
                        cx.fold(
                            rows,
                            FoldKind::AsText,
                            true,
                            holder.as_deref(),
                            format!("a witness of {event} as a note"),
                        );
                    }
                }
            }
            for n in notes {
                occasion.substructures.push(text_node("NOTE", &n));
            }
        }
    }
    if reversed.is_empty() {
        return;
    }
    for (person, assoc) in reversed {
        if let Some(r) = idx.get(person.as_str()).and_then(|&i| records.get_mut(i)) {
            r.substructures.push(assoc);
        }
    }
}

/// The person an association points to.
fn association_target(found: &Draft<'_>) -> Option<Xref> {
    found.pointer().filter(|p| *p != Xref::VOID).map(Xref::new)
}

/// What an association says: its role, the person it points to, that
/// person's name (or the phrase naming them) and its other substructures.
fn association_parts<'s>(
    found: &Draft<'s>,
    target_name: Option<String>,
) -> (String, Option<Xref>, String, Vec<Draft<'s>>) {
    let role = child_text(found, "RELA")
        .or_else(|| child(found, "ROLE").and_then(|r| child_text(r, "PHRASE")))
        .or_else(|| child_text(found, "ROLE"))
        .unwrap_or("Witness")
        .trim()
        .to_string();
    let target = association_target(found);
    let name = target_name
        .or_else(|| child_text(found, "PHRASE").map(str::to_string))
        .unwrap_or_default();
    let rest = found
        .substructures
        .iter()
        .filter(|s| !matches!(s.tag.as_str(), "RELA" | "ROLE" | "PHRASE" | "TYPE"))
        .cloned()
        .collect();
    (role, target, name, rest)
}

/// Each association as an `EVEN` + `TYPE Associate`.
fn associations_as_events<'s>(
    records: &mut [Draft<'s>],
    range: Range<usize>,
    rows: &'static [&'static str],
    name_in_payload: bool,
    cx: &mut Cx<'_>,
    shared: &mut Shared<'s>,
) {
    let idx = shared.index(records);
    for i in range {
        // The name of each associated person, before the fold, by
        // identifier.
        let names: HashMap<String, Option<String>> = match records.get(i) {
            Some(r) if is(r, "INDI") || is(r, "FAM") => {
                let look = Lookup { records, idx };
                r.substructures
                    .iter()
                    .chain(r.substructures.iter().flat_map(|e| &e.substructures))
                    .filter(|a| is(a, "ASSO"))
                    .filter_map(|a| a.pointer().filter(|p| *p != Xref::VOID))
                    .map(|p| (p.to_string(), look.person_name(p)))
                    .collect()
            }
            _ => continue,
        };
        let Some(r) = records.get_mut(i) else { break };
        let x = r.xref.clone();
        let mut assocs = Vec::new();
        let mut k = 0;
        while k < r.substructures.len() {
            if r.substructures.get(k).is_some_and(|a| is(a, "ASSO")) {
                assocs.push(r.substructures.remove(k));
            } else {
                k += 1;
            }
        }
        for e in &mut r.substructures {
            let mut k = 0;
            while k < e.substructures.len() {
                if e.substructures.get(k).is_some_and(|a| is(a, "ASSO")) {
                    assocs.push(e.substructures.remove(k));
                } else {
                    k += 1;
                }
            }
        }
        for a in assocs {
            let name = a
                .pointer()
                .filter(|p| *p != Xref::VOID)
                .and_then(|p| names.get(p).cloned().flatten())
                .or_else(|| child_text(&a, "PHRASE").map(str::to_string))
                .unwrap_or_default();
            let rela = child_text(&a, "RELA")
                .or_else(|| child(&a, "ROLE").and_then(|r| child_text(r, "PHRASE")))
                .or_else(|| child_text(&a, "ROLE"))
                .unwrap_or_default()
                .to_string();
            let mut even = text_node("EVEN", if name_in_payload { &name } else { "" });
            even.substructures.push(text_node("TYPE", "Associate"));
            even.substructures.push(text_node(
                "NOTE",
                &format!("Associate: {name}; Relationship: {rela}"),
            ));
            r.substructures.push(even);
            cx.fold(
                rows,
                FoldKind::AsText,
                true,
                x.as_deref(),
                "an association as EVEN TYPE Associate",
            );
        }
    }
}

/// What a link to a multimedia record written inline takes of the record.
struct MediaRecord<'s> {
    /// Its first file.
    file: Draft<'s>,
    /// Its title.
    title: Option<Draft<'s>>,
    /// Whether it holds more than files, a title and a change date.
    more: bool,
}

/// Links to multimedia records written inline.
fn inline_media<'s>(
    records: &mut [Draft<'s>],
    range: Range<usize>,
    rows: &'static [&'static str],
    cx: &mut Cx<'_>,
    shared: &mut Shared<'s>,
) {
    if cx.v7 {
        return;
    }
    let idx = shared.index(records);
    for i in range {
        // What each link's record gives: its first file, its title, and
        // whether it holds more.
        let found: Vec<Option<MediaRecord<'_>>> = match records.get(i) {
            Some(r) if is(r, "INDI") || is(r, "FAM") => {
                let look = Lookup { records, idx };
                r.substructures
                    .iter()
                    .filter(|c| is(c, "OBJE"))
                    .map(|link| {
                        let record = look.get(link.pointer()?)?;
                        let file = record.child("FILE")?.clone();
                        Some(MediaRecord {
                            file,
                            title: record.child("TITL").cloned(),
                            more: record
                                .children()
                                .any(|s| !matches!(s.tag.as_str(), "FILE" | "TITL" | "CHAN")),
                        })
                    })
                    .collect()
            }
            _ => continue,
        };
        let mut found = found.into_iter();
        let Some(r) = records.get_mut(i) else { break };
        let x = r.xref.clone();
        let mut n = 0;
        let mut lossy = false;
        for link in r.substructures.iter_mut().filter(|c| is(c, "OBJE")) {
            let Some(record) = found.next().flatten() else {
                continue;
            };
            // 5.5.1: `OBJE` + `FILE` (+ `FORM` + `TYPE`) + `TITL`.
            let mut inline = Draft::new("OBJE");
            let mut first = record.file;
            first
                .substructures
                .retain(|s| is(s, "FORM") || is(s, "TITL"));
            let title = child(&first, "TITL").cloned().or(record.title);
            first.substructures.retain(|s| !is(s, "TITL"));
            inline.substructures.push(first);
            inline.substructures.extend(title);
            lossy |= record.more;
            inline.substructures.extend(
                link.substructures
                    .iter()
                    .filter(|s| is(s, "_PRIM"))
                    .cloned(),
            );
            *link = inline;
            n += 1;
        }
        if n > 0 {
            cx.fold(
                rows,
                FoldKind::Moved,
                lossy,
                x.as_deref(),
                format!("{n} links to multimedia records written inline"),
            );
        }
    }
}

/// The names of the jurisdictions a place format may name, by field of
/// the formats of [`PlaceStyle`].
const TOWN: &[&str] = &[
    "town", "city", "commune", "ville", "village", "parish", "paroisse",
];
const CODE: &[&str] = &[
    "area code",
    "postal code",
    "code postal",
    "zip",
    "zip code",
    "insee",
    "code insee",
    "postcode",
];
const COUNTY: &[&str] = &[
    "county",
    "département",
    "departement",
    "department",
    "district",
];
const REGION: &[&str] = &["region", "région", "state", "province"];
const COUNTRY: &[&str] = &["country", "pays", "nation"];
const SUB: &[&str] = &[
    "subdivision",
    "hamlet",
    "lieu-dit",
    "lieudit",
    "hameau",
    "place",
    "locality",
];

/// What became of a place.
pub(super) enum Placed {
    /// Its jurisdictions as the header's format names them.
    Mapped(String),
    /// Its jurisdictions by position (the format named none).
    Guessed(String),
    /// More jurisdictions than the target's format has: left as it is.
    TooMany,
}

/// A place (`text`) in a target's `style`, its jurisdictions mapped by the
/// header's format (`mapping`, when `known`) or by position.
pub(super) fn place_in(
    text: &str,
    mapping: &[Option<usize>],
    known: bool,
    style: PlaceStyle,
) -> Placed {
    let parts = || text.split(',').map(str::trim);
    let mut fields: [&str; 6] = [""; 6];
    let mapped = known && jurisdictions(text) == mapping.len();
    if mapped {
        for (part, slot) in parts().zip(mapping) {
            if let Some(field) = slot.and_then(|at| fields.get_mut(at)) {
                *field = part;
            }
        }
    } else {
        // Smallest first: the first is the town, the last the country,
        // the ones between the county and the region.
        let mut each = parts();
        let (Some(first), last) = (each.next(), each.next_back()) else {
            return Placed::TooMany;
        };
        fields[0] = first;
        if let Some(last) = last {
            fields[4] = last;
            match (each.next(), each.next(), each.next()) {
                (None, ..) => {}
                (Some(county), None, _) => fields[2] = county,
                (Some(county), Some(region), None) => {
                    fields[2] = county;
                    fields[3] = region;
                }
                _ => return Placed::TooMany,
            }
        }
    }
    let [town, code, county, region, land, subdivision] = fields;
    let mut new = String::with_capacity(text.len() + 16);
    let main = [town, code, county, region, land];
    match style {
        PlaceStyle::Geneanet => {
            if !subdivision.is_empty() {
                new.push('[');
                new.push_str(subdivision);
                new.push_str("] - ");
            }
            for (k, f) in main.iter().enumerate() {
                if k > 0 {
                    new.push_str(", ");
                }
                new.push_str(f);
            }
        }
        PlaceStyle::Heredis => {
            for f in main {
                new.push_str(f);
                new.push_str(", ");
            }
            new.push_str(subdivision);
        }
    }
    if mapped {
        Placed::Mapped(new)
    } else {
        Placed::Guessed(new)
    }
}

/// Names the places' format in the header ([`places`]) when every place
/// left, but those of extensions, follows it.
fn place_form(records: &mut [Draft<'_>], style: PlaceStyle) {
    fn all_fit(s: &Draft<'_>, style: PlaceStyle) -> bool {
        s.substructures.iter().all(|c| {
            if c.tag.is_extension() {
                return true;
            }
            let fits = !is(c, "PLAC") || c.text().is_none_or(|t| style.fits(t));
            fits && all_fit(c, style)
        })
    }
    if !records
        .iter()
        .filter(|r| !is(r, "HEAD") && !r.tag.is_extension())
        .all(|r| all_fit(r, style))
    {
        return;
    }
    let Some(head) = records.iter_mut().find(|r| is(r, "HEAD")) else {
        return;
    };
    let form = style.form();
    if let Some(p) = child_mut(head, "PLAC") {
        match child_mut(p, "FORM") {
            Some(f) => set_text(f, form),
            None => p.substructures.push(text_node("FORM", form)),
        }
    } else {
        let mut p = Draft::new("PLAC");
        p.substructures.push(text_node("FORM", form));
        head.substructures.push(p);
    }
}

/// Places in a target's format.
/// What [`places`] found so far, to report once it is done.
#[derive(Default)]
pub(super) struct PlacesSeen {
    /// The jurisdictions the header names, as slots of the target's, and
    /// whether each is known; read once.
    form: Option<(Vec<Option<usize>>, bool)>,
    pub(super) changed: usize,
    pub(super) guessed: usize,
    pub(super) skipped: usize,
}

fn places<'s>(
    records: &mut [Draft<'s>],
    range: Range<usize>,
    style: PlaceStyle,
    cx: &mut Cx<'_>,
    shared: &mut Shared<'s>,
) {
    let seen = &mut shared.places;
    let (mapping, known) = seen.form.get_or_insert_with(|| place_mapping(records));
    let known = *known;
    for r in records
        .get_mut(range)
        .unwrap_or_default()
        .iter_mut()
        .filter(|r| !is(r, "HEAD"))
    {
        each_tag(r, "PLAC", &mut |p| {
            let Some(text) = p.text() else { return };
            if text.trim_start().starts_with('[') {
                return;
            }
            // Without a format the header names, a place with as many
            // jurisdictions as the target's is taken to be in its format.
            let identity: Vec<Option<usize>>;
            let (mapping, known) = if !known && style.fits(text) {
                identity = (0..style.form().split(',').count()).map(Some).collect();
                (identity.as_slice(), true)
            } else {
                (mapping.as_slice(), known)
            };
            let new = match place_in(text, mapping, known, style) {
                Placed::Mapped(new) => new,
                Placed::Guessed(new) => {
                    seen.guessed += 1;
                    new
                }
                Placed::TooMany => {
                    seen.skipped += 1;
                    return;
                }
            };
            if new != text {
                set_text(p, &new);
                seen.changed += 1;
            }
        });
    }
    // The header names the format once the folds are done, when every
    // place written follows it ([`place_form`]).
    cx.place_style = Some(style);
}

/// The jurisdictions the header of `records` names, as slots of a
/// target's format, and whether each is known.
pub(super) fn place_mapping(records: &[Draft<'_>]) -> (Vec<Option<usize>>, bool) {
    // The jurisdictions the header names, in order.
    let form: Vec<String> = records
        .iter()
        .find(|r| is(r, "HEAD"))
        .and_then(|h| child(h, "PLAC"))
        .and_then(|p| child_text(p, "FORM"))
        .map(|f| {
            f.split(',')
                .map(|j| j.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let slot = |name: &str| -> Option<usize> {
        [TOWN, CODE, COUNTY, REGION, COUNTRY, SUB]
            .iter()
            .position(|names| names.contains(&name))
    };
    let mapping: Vec<Option<usize>> = form.iter().map(|j| slot(j)).collect();
    let known = !form.is_empty() && mapping.iter().all(Option::is_some);
    (mapping, known)
}

/// Reports what [`places`] did, once it ran on every record.
fn places_done(
    rows: &'static [&'static str],
    style: PlaceStyle,
    seen: &PlacesSeen,
    cx: &mut Cx<'_>,
) {
    let (limit, fold) = places_report(rows, style, seen);
    if let Some(l) = limit {
        cx.limit(l.rows, None, l.detail);
    }
    if let Some(f) = fold {
        cx.fold(f.rows, f.kind, f.lossy, None, f.detail);
    }
}

/// What [`places_done`] reports of `seen`: the places left, the places
/// changed.
fn places_report(
    rows: &'static [&'static str],
    style: PlaceStyle,
    seen: &PlacesSeen,
) -> (Option<Limit>, Option<Fold>) {
    let form_text = style.form();
    let PlacesSeen {
        changed: n,
        guessed,
        skipped,
        ..
    } = *seen;
    let limit = (skipped > 0).then(|| Limit {
        rows,
        record: None,
        detail: format!(
            "{skipped} places with more jurisdictions than {form_text}, left as they are"
        ),
    });
    let fold = (n > 0).then(|| Fold {
        rows,
        kind: FoldKind::Recoded,
        lossy: guessed > 0,
        record: None,
        detail: format!(
            "{n} places in the format {form_text}{}",
            if guessed > 0 {
                " (jurisdictions guessed by position)"
            } else {
                ""
            }
        ),
    });
    (limit, fold)
}

/// The system `HEAD.SOUR` names ([`HeadSource`]): the crate's, unless
/// the program set the dataset's (`read`: the one the dataset was read
/// with): whether it is the program's, for [`name_the_crate`] once the
/// steps are done. An export never passes for the producer of a file it
/// read (importers switch dialects on it, EXF-082, EXC-018, EXC-057).
fn own_source(records: &mut [Draft<'_>], cx: &mut Cx<'_>, read: Option<&str>) -> bool {
    const ROWS: &[&str] = &["EXF-082", "EXC-018", "EXC-057"];
    let Some(head) = records.iter_mut().find(|r| is(r, "HEAD")) else {
        return false;
    };
    let product = crate::writer::head::PRODUCT;
    let program = match (cx.options.source, child_text(head, "SOUR")) {
        (_, Some(s)) if s == product => return false,
        (_, None) | (HeadSource::Library, _) => false,
        (HeadSource::Dataset, _) => true,
        (HeadSource::Auto, Some(s)) => read != Some(s),
    };
    if program {
        // The crate is named again once the steps are done, which would
        // fold the name an export read back holds.
        if let Some(sour) = child_mut(head, "SOUR") {
            sour.substructures.retain(|s| !is(s, MARKER));
        }
        return true;
    }
    let mut sour = text_node("SOUR", product);
    sour.substructures
        .push(text_node("VERS", env!("CARGO_PKG_VERSION")));
    let previous = if let Some(at) = head.substructures.iter().position(|c| is(c, "SOUR")) {
        let old = std::mem::replace(
            head.substructures
                .get_mut(at)
                .unwrap_or(&mut Draft::default()),
            sour,
        );
        payload_str(&old).to_string()
    } else {
        head.substructures.push(sour);
        String::new()
    };
    // A name the crate wrote before it was named oxidged is the crate's:
    // naming it again relabels nothing.
    if !previous.is_empty() && !crate::writer::head::FORMER_PRODUCTS.contains(&previous.as_str()) {
        cx.fold(
            ROWS,
            FoldKind::Relabelled,
            false,
            None,
            format!("HEAD.SOUR {previous} written as {product}"),
        );
    }
    false
}

/// Names the crate under the program's `HEAD.SOUR` ([`MARKER`]), declared
/// in a 7.x `HEAD.SCHMA`.
fn name_the_crate(records: &mut [Draft<'_>], v7: bool) {
    let Some(head) = records.iter_mut().find(|r| is(r, "HEAD")) else {
        return;
    };
    if let Some(sour) = child_mut(head, "SOUR") {
        sour.substructures
            .push(text_node(MARKER, env!("CARGO_PKG_VERSION")));
    }
    if !v7 {
        return;
    }
    let declared = head
        .substructures
        .iter()
        .filter(|s| is(s, "SCHMA"))
        .flat_map(|s| &s.substructures)
        .any(|t| payload_str(t).split_whitespace().next() == Some(MARKER));
    if !declared {
        let uri = super::docs_uri("oxidged", MARKER);
        let tag = text_node("TAG", &format!("{MARKER} {uri}"));
        if let Some(schma) = child_mut(head, "SCHMA") {
            schma.substructures.push(tag);
        } else {
            let mut schma = Draft::new("SCHMA");
            schma.substructures.push(tag);
            head.substructures.push(schma);
        }
    }
}

/// The extension under `HEAD.SOUR` that says an export of a program
/// passed through this crate ([`HeadSource`]).
pub(crate) const MARKER: &str = "_OXIDGED";

/// Leaves out the records `RESN` restricts, and the pointers to them.
/// Leaves out the credentials the records hold, as the writer does
/// ([`Credentials`]), saying so.
fn omit_credentials(records: &mut [Draft<'_>], cx: &mut Cx<'_>) {
    for r in records {
        let dropped = crate::writer::drop_draft_credentials(r);
        if dropped > 0 {
            let x = r.xref.clone();
            cx.fold(
                &[],
                FoldKind::Omitted,
                true,
                x.as_deref(),
                format!("{dropped} credentials (_PASS, _USER) left out"),
            );
        }
    }
}

fn omit_private(records: &mut Vec<Draft<'_>>, cx: &mut Cx<'_>) {
    const ROWS: &[&str] = &["EXF-022", "EXC-056", "EXC-074"];
    let private: Vec<Xref> = records
        .iter()
        .filter(|r| {
            child_text(r, "RESN").is_some_and(|t| {
                let t = t.to_ascii_lowercase();
                t.contains("privacy") || t.contains("confidential")
            })
        })
        .filter_map(|r| r.xref.as_deref().map(Xref::new))
        .collect();
    if private.is_empty() {
        return;
    }
    records.retain(|r| {
        r.xref
            .as_deref()
            .is_none_or(|x| !private.iter().any(|p| p.as_str() == x))
    });
    for r in records.iter_mut() {
        drop_pointers(r, &|p| private.iter().any(|x| x.as_str() == p));
    }
    cx.fold(
        ROWS,
        FoldKind::Omitted,
        true,
        None,
        format!("{} restricted records left out", private.len()),
    );
}

fn drop_pointers(s: &mut Draft<'_>, gone: &dyn Fn(&str) -> bool) {
    s.substructures
        .retain(|c| c.pointer().is_none_or(|p| !gone(p)));
    for c in &mut s.substructures {
        drop_pointers(c, gone);
    }
}

/// Leaves out the multimedia links and records.
fn omit_media(records: &mut Vec<Draft<'_>>, cx: &mut Cx<'_>) {
    const ROWS: &[&str] = &["EXC-002", "EXC-047", "EXC-055", "EXC-107"];
    let before = records.len();
    records.retain(|r| !is(r, "OBJE"));
    let mut links = 0;
    for r in records.iter_mut() {
        drop_tag(r, "OBJE", &mut links);
    }
    let removed = before - records.len();
    if removed + links > 0 {
        cx.fold(
            ROWS,
            FoldKind::Omitted,
            true,
            None,
            format!("{removed} multimedia records and {links} links left out"),
        );
    }
}

fn drop_tag(s: &mut Draft<'_>, tag: &str, n: &mut usize) {
    let before = s.substructures.len();
    s.substructures.retain(|c| !is(c, tag));
    *n += before - s.substructures.len();
    for c in &mut s.substructures {
        drop_tag(c, tag, n);
    }
}

fn omit_change_dates(records: &mut [Draft<'_>], cx: &mut Cx<'_>) {
    const ROWS: &[&str] = &["EXC-068"];
    let mut n = 0;
    for r in records.iter_mut() {
        drop_tag(r, "CHAN", &mut n);
    }
    if n > 0 {
        cx.fold(
            ROWS,
            FoldKind::Omitted,
            true,
            None,
            format!("{n} change dates left out"),
        );
    }
}

/// Media paths in the target's form.
fn media_paths(records: &mut [Draft<'_>], cx: &mut Cx<'_>, touched: &mut [bool]) {
    let style = cx.target.media;
    if style == MediaPaths::Keep {
        return;
    }
    let root = cx.options.media_root.clone().map(|r| r.replace('\\', "/"));
    let mut n = 0;
    for (r, t) in records.iter_mut().zip(touched.iter_mut()) {
        let before = n;
        each_media_file(r, &mut |f| {
            let Some(path) = f.text().map(str::to_string) else {
                return;
            };
            if path.contains("://") && !path.starts_with("file://") {
                return;
            }
            let mut p = path
                .trim_start_matches("file:///")
                .trim_start_matches("file://")
                .replace('\\', "/");
            let new = match style {
                MediaPaths::FileName => p.rsplit('/').next().unwrap_or(&p).to_string(),
                MediaPaths::Relative => {
                    if let Some(root) = &root
                        && let Some(rest) = p.strip_prefix(root.trim_end_matches('/'))
                    {
                        p = rest.trim_start_matches('/').to_string();
                    }
                    // A drive letter and a leading slash out.
                    let bytes = p.as_bytes();
                    if bytes.get(1) == Some(&b':') {
                        p = p.get(2..).unwrap_or_default().to_string();
                    }
                    p.trim_start_matches('/').to_string()
                }
                MediaPaths::Keep => p,
            };
            if new != path {
                set_text(f, &new);
                n += 1;
            }
        });
        *t |= n != before;
    }
    if n > 0 {
        let rows = cx.target.rows;
        cx.fold(
            rows,
            FoldKind::Recoded,
            false,
            None,
            format!("{n} media paths rewritten"),
        );
    }
}

fn each_media_file<'s>(s: &mut Draft<'s>, f: &mut dyn FnMut(&mut Draft<'s>)) {
    let obje = is(s, "OBJE");
    for c in &mut s.substructures {
        if obje && is(c, "FILE") {
            f(c);
        }
        each_media_file(c, f);
    }
}

/// Record identifiers of at most `max` characters, delimiters excluded.
fn short_xrefs(
    records: &mut [Draft<'_>],
    rows: &'static [&'static str],
    max: usize,
    cx: &mut Cx<'_>,
) {
    let taken: Vec<String> = records
        .iter()
        .filter_map(|r| r.xref.as_ref().map(std::string::ToString::to_string))
        .collect();
    let mut map: HashMap<String, Xref> = HashMap::new();
    let mut next = 0_u32;
    for r in records.iter() {
        let Some(x) = &r.xref else { continue };
        if Xref::new(&**x).id().chars().count() <= max {
            continue;
        }
        let prefix = match r.tag.as_str() {
            "INDI" => "I",
            "FAM" => "F",
            "SOUR" => "S",
            "REPO" => "R",
            "OBJE" => "M",
            "NOTE" | "SNOTE" => "N",
            "SUBM" => "U",
            _ => "X",
        };
        let new = loop {
            next += 1;
            let candidate = format!("@{prefix}{next}@");
            if !taken.contains(&candidate) && !map.values().any(|v| v.as_str() == candidate) {
                break Xref::new(candidate);
            }
        };
        map.insert(x.to_string(), new);
    }
    if map.is_empty() {
        return;
    }
    for r in records.iter_mut() {
        remap(r, &map);
    }
    let n = map.len();
    cx.fold(
        rows,
        FoldKind::Recoded,
        false,
        None,
        format!("{n} identifiers over {max} characters renamed"),
    );
}

fn remap(s: &mut Draft<'_>, map: &HashMap<String, Xref>) {
    if let Some(x) = &s.xref
        && let Some(new) = map.get(&**x)
    {
        s.xref = Some(Cow::Owned(new.as_str().to_string()));
    }
    if let Value::Pointer(p) = &s.payload
        && let Some(new) = map.get(&**p)
    {
        s.payload = Value::Pointer(Cow::Owned(new.as_str().to_string()));
    }
    for c in &mut s.substructures {
        remap(c, map);
    }
}

/// The conformance repair of drafts, as the writer makes it.
/// Repairs the records as [`crate::spec::conform`] does, and what it
/// found clean of each record it left as it was ([`Rec::checked`]); with
/// `known`, what a conformance of the same records found clean of each, and
/// whether each changed since (not the header, which is looked at again).
fn conform_drafts(
    records: &mut Vec<Draft<'_>>,
    to: GedcomVersion,
    known: Option<(&[Option<Checked>], &[bool])>,
) -> (Vec<Repair>, Vec<Option<Checked>>) {
    // The records the pass leaves as they are stay drafts; only those it
    // rewrites are copied, and come back as owned drafts.
    let mut recs: Vec<Rec<'_, &Draft<'_>>> = records
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let rec = Rec::given(d);
            let still = known.and_then(|(checked, touched)| {
                let unchanged = touched.get(i) == Some(&false) && !is(d, "HEAD");
                checked.get(i).copied().flatten().filter(|_| unchanged)
            });
            match still {
                Some(c) => rec.known_clean(c),
                None => rec,
            }
        })
        .collect();
    // (The pass may complete the header without a repair to report.)
    let conformed = conform_records(&mut recs, to, None);
    let (repairs, state) = (conformed.repairs, conformed.state);
    let checked: Vec<Option<Checked>> = recs.iter().map(|r| r.checked(state)).collect();
    let base = records.as_ptr();
    let size = std::mem::size_of::<Draft<'_>>().max(1);
    let placed: Vec<Result<usize, Structure>> = recs
        .into_iter()
        .map(|r| {
            r.into_given()
                .map(|d| (std::ptr::from_ref(d) as usize - base as usize) / size)
        })
        .collect();
    let mut old: Vec<Option<Draft<'_>>> = std::mem::take(records).into_iter().map(Some).collect();
    records.reserve(placed.len());
    for p in placed {
        records.push(match p {
            Ok(i) => {
                let d = old.get_mut(i).and_then(Option::take);
                debug_assert!(d.is_some(), "a record given twice");
                d.unwrap_or_default()
            }
            Err(s) => Draft::from_owned(s),
        });
    }
    (repairs, checked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A fictitious tree whose records need, of the Geneanet folds, only
    /// those made as it is written: places, citations of source records.
    const PLAIN: &str = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n\
        0 @I1@ INDI\n1 NAME Ann /Example/\n1 SEX F\n1 BIRT\n2 DATE 1 JAN 1800\n\
        2 PLAC Sampletown, Region\n2 SOUR @S1@\n3 PAGE p. 1\n1 FAMS @F1@\n\
        1 NOTE word word\n2 CONT a second line with an @@ sign\n1 CHAN\n2 DATE 1 JAN 2000\n\
        0 @I2@ INDI\n1 NAME Bob /Example/\n1 SEX M\n1 FAMS @F1@\n1 SOUR @S1@\n\
        0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I1@\n1 MARR\n2 DATE 1820\n\
        2 PLAC Othertown, County, Region, Land\n2 SOUR @S1@\n3 PAGE p. 2\n\
        0 @S1@ SOUR\n1 TITL Parish register\n1 AUTH Sample archive\n1 REPO @R1@\n2 CALN 7\n\
        0 @R1@ REPO\n1 NAME Sample archive\n0 TRLR\n";

    /// Every profile's export of `text`, the direct one when it is made,
    /// is the export folded whole; how many were direct.
    fn direct_as_whole(text: &str) -> usize {
        let data = Dataset::parse(text);
        let before = DIRECT.with(Cell::get);
        for p in super::super::export_profiles() {
            let e = Exporter::new(p);
            let direct = e.write_to_string(&data).unwrap();
            let whole = e.whole_to_string(&data).unwrap();
            assert_eq!(direct, whole, "{}", p.id());
        }
        DIRECT.with(Cell::get) - before
    }

    #[test]
    fn direct_exports_are_the_whole_ones() {
        // Geneanet writes it directly.
        assert!(direct_as_whole(PLAIN) > 0);
        // A record another fold changes: folded whole.
        let markup = PLAIN.replace("1 NOTE word word", "1 NOTE a <b>bold</b> word");
        let before = DIRECT.with(Cell::get);
        let data = Dataset::parse(markup.as_str());
        let e = Exporter::named("geneanet").unwrap();
        assert_eq!(
            e.write_to_string(&data).unwrap(),
            e.whole_to_string(&data).unwrap()
        );
        assert_eq!(DIRECT.with(Cell::get), before);
        direct_as_whole(&markup);
        // A citation with more than a page, a place with a form, a header
        // without a submitter, bare events, an extension.
        for change in [
            ("3 PAGE p. 1", "3 PAGE p. 1\n3 QUAY 2"),
            (
                "2 PLAC Sampletown, Region",
                "2 PLAC Sampletown, Region\n3 FORM Town, Region",
            ),
            ("1 SEX M", "1 SEX M\n1 DEAT"),
            ("1 SEX M", "1 SEX M\n1 _MILT"),
            ("1 SEX F", "1 SEX F\n1 BURI"),
            ("2 DATE 1 JAN 1800", "2 DATE 31 FEB 1800"),
        ] {
            direct_as_whole(&PLAIN.replace(change.0, change.1));
        }
    }

    #[test]
    fn the_event_tags_are_standard() {
        for tag in Y_EVENTS.iter().chain(OTHER_EVENTS) {
            assert!(standard_index(tag).is_some(), "{tag}");
            assert!(is_event(tag));
        }
        assert!(Y_EVENTS.iter().all(|t| is_y_event(t)));
        assert!(!is_event("NAME") && !is_y_event("EVEN") && !is_event("_MILT"));
    }

    #[test]
    fn paf_checksums() {
        let uid = paf_uid("0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(uid.len(), 36);
        assert!(uid.starts_with("0123456789ABCDEF0123456789ABCDEF"));
        assert_eq!(
            paf_uid("{01234567-89AB-CDEF-0123-456789ABCDEF}").as_deref(),
            Some(uid.as_str())
        );
        assert_eq!(paf_uid(&uid), None);
    }

    #[test]
    fn markup() {
        assert_eq!(
            escape_markup("a < b & c &amp; d <i>e</i>"),
            "a &lt; b &amp; c &amp; d <i>e</i>"
        );
        assert_eq!(
            strip_html("<p>One<br>two &amp; <b>three</b></p>"),
            "One\ntwo & three"
        );
        assert_eq!(
            strip_html("&lt;i&gt;Example&lt;/i&gt; text"),
            "Example text"
        );
        assert!(impossible_date("31 FEB 1850"));
        assert!(impossible_date("ABT 29 FEB 1900"));
        assert!(!impossible_date("29 FEB 2000"));
        assert!(!impossible_date("BET 1850 AND 1851"));
    }
}
