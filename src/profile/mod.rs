//! Producer profiles: reading a file as its producer meant it, and writing
//! one shaped for the program that will import it.
//!
//! Genealogy programs extend GEDCOM in their own ways: extension tags
//! (`_MARNM`, `_FREL`, `_SDATE`), standard tags given another meaning
//! (Family Tree Maker for Windows' `INDI.ADDR`, `GeneWeb`'s `GIVN`), date
//! dialects, character sets they label `ANSI`. A [`Profile`] describes one
//! producer: how to recognise its files ([`Detection`]), how it reads
//! `HEAD.CHAR`, and the rules that rewrite its conventions as the standard
//! says them. An [`ExportProfile`] describes an importer: what it reads and
//! what it loses, and how to write a conformant file it reads best.
//!
//! The profiles are data, written in a small `macro_rules!` language and
//! compiled into the library; each rule cites the rows of the
//! compatibility research it implements.
//!
//! # Reading
//!
//! [`GedcomBuilder`](crate::GedcomBuilder) detects the producer from the
//! header (before decoding, so that its reading of `CHAR` applies: `ANSI`
//! is Windows-1252 unless the producer says otherwise, Windows-1250 when the
//! bytes say so), then refines the detection with the extension tags the
//! file uses, and interprets the file at [`Interpretation::Lossless`] by
//! default:
//!
//! ```rust
//! use oxidged::profile::{Action, Interpretation};
//! use oxidged::GedcomBuilder;
//!
//! # fn main() -> Result<(), oxidged::GedcomError> {
//! let file = "0 HEAD\n1 SOUR PAF\n2 NAME Personal Ancestral File\n1 GEDC\n2 VERS 5.5.1\n\
//!             0 @I1@ INDI\n1 NAME Anne /Example/\n2 _AKA Annie /Example/\n0 TRLR\n";
//! let (data, report) = GedcomBuilder::new().build_from_str_with_report(file)?;
//! assert_eq!(report.detection.profile().id(), "paf");
//! let anne = data.individuals().first().unwrap();
//! assert_eq!(anne.names().len(), 2); // `_AKA` is a name of type AKA too
//! // PAF's `_AKA` may list several names: the name is added, `_AKA` kept.
//! assert_eq!(report.interpretation.with_action(Action::Added).count(), 1);
//!
//! // Interpretation can be turned off: then nothing costs anything.
//! let raw = GedcomBuilder::new()
//!     .interpretation(Interpretation::None)
//!     .build_from_str(file)?;
//! assert_eq!(raw.individuals().first().unwrap().names().len(), 1);
//! # Ok(())
//! # }
//! ```
//!
//! Interpretation never loses what was read (see [`interpret`]), and it
//! keeps the dataset in its version family: what only 7.x can say is
//! interpreted when the writer converts the dataset to 7.x. Extension tags
//! a profile documents and keeps are declared in a 7.x `HEAD.SCHMA`, with
//! the registered URI when there is one, else with the documentation URI
//! oxidged publishes for them ([`EXTENSION_DOCS`], [`docs_uri`]).
//!
//! # Writing
//!
//! [`Exporter`] writes a dataset for an importer: always conformant
//! GEDCOM, its version, character set and line layout chosen for the
//! importer, its data folded into the standard structures the importer
//! reads, and an [`ExportReport`] listing every fold that lost something:
//!
//! ```rust
//! use oxidged::profile::Exporter;
//! use oxidged::Dataset;
//!
//! let data = Dataset::parse(
//!     "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
//!      1 OCCU Weaver\n1 OCCU Miller\n0 TRLR\n",
//! );
//! let (text, report) = Exporter::named("geneanet").unwrap().write_to_string(&data).unwrap();
//! assert!(text.contains("1 OCCU Weaver, Miller\n"));
//! assert!(text.contains("2 VERS 5.5.1\n"));
//! assert_eq!(report.lossy().count(), 1); // two occupations made one
//! ```
//!
//! The tier-1 export profiles (Ancestry, Family Tree Maker, `MyHeritage`,
//! Geni, `FamilySearch`, PAF, Ancestral Quest, Geneanet, `GeneWeb`, Heredis)
//! follow published and observed importer behaviour, much of it dated:
//! they are to be re-verified against recent imports ([`Verification`]).

pub(crate) mod detect;
pub(crate) mod draft;
mod dsl;
pub(crate) mod export;
mod export_data;
mod export_direct;
mod import_data;
pub(crate) mod interpret;
mod util;

use std::fmt;

pub use detect::{Detection, FINGERPRINT_CAP, HeadFields, ProducerVersion, THRESHOLD};
pub use export::{
    ExportOptions, ExportReport, Exporter, Fold, FoldKind, HeadSource, Limit, MediaPaths, Target,
};
pub use interpret::{Action, InterpretReport, Interpretation, Mapping, docs_uri, interpret};

pub(crate) use import_data::GENERIC;
pub(crate) use interpret::RuleSet;
pub(crate) use util::long_form;

use crate::GedcomVersion;
use crate::encoding::GedcomEncoding;

/// Where oxidged documents the producers' extension tags it writes in
/// GEDCOM 7.x files: each tag has the anchor `#<profile>-<TAG>` there
/// ([`docs_uri`]).
pub const EXTENSION_DOCS: &str =
    "https://github.com/trois-six/oxidged/blob/main/docs/extensions.md";

/// The registered `EXID.TYPE` of `FamilySearch` person identifiers.
pub(crate) const FAMILYSEARCH_PERSON: &str = "https://gedcom.io/exid-type/FamilySearch-PersonId";

/// How far a profile's facts have been checked.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verification {
    /// Built from documentation, source code and real exports; to be
    /// re-verified against recent versions of the program before it is
    /// relied on (tier 1 and the profiles derived from them).
    ToReverify,
    /// Built from the research only, less widely used programs (tiers 2
    /// and 3).
    Research,
}

/// An import profile: how a producer writes GEDCOM, and how to read it.
/// See the [module documentation](self).
#[derive(Debug)]
pub struct Profile {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) tier: u8,
    pub(crate) parent: Option<&'static str>,
    pub(crate) detect: &'static [detect::Sig],
    /// How the producer reads `HEAD.CHAR` values (normalised: upper case,
    /// no space, hyphen or underscore).
    pub(crate) charset: &'static [(&'static str, GedcomEncoding)],
    /// Whether byte statistics may find Windows-1250 under an `ANSI` (or
    /// unknown) label.
    pub(crate) stats: bool,
    pub(crate) rules: &'static [interpret::Rule],
}

impl PartialEq for Profile {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Profile {}

impl Profile {
    /// The profile's name (`ftm`, `geneanet`, …).
    #[must_use]
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// The producer, in words.
    #[must_use]
    pub fn label(&self) -> &'static str {
        self.label
    }

    /// The research's priority tier: 1 for the most used programs, 3 for
    /// the least; 0 for the generic profiles.
    #[must_use]
    pub fn tier(&self) -> u8 {
        self.tier
    }

    /// How far its facts have been checked.
    #[must_use]
    pub fn verification(&self) -> Verification {
        if self.tier <= 1 {
            Verification::ToReverify
        } else {
            Verification::Research
        }
    }

    /// The profile it extends (`generic` for most).
    #[must_use]
    pub fn parent(&self) -> Option<&'static Profile> {
        self.parent.and_then(profile)
    }

    /// How many rules it has (its parents' not counted).
    #[must_use]
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// The catalogue rows its rules implement, once each, sorted.
    #[must_use]
    pub fn rows(&self) -> Vec<&'static str> {
        let mut rows: Vec<&'static str> = self
            .rules
            .iter()
            .flat_map(|r| r.rows.iter().copied())
            .collect();
        rows.sort_unstable();
        rows.dedup();
        rows
    }

    /// The encoding the producer means by a `HEAD.CHAR` value, when it
    /// differs from the reader's default; `None` to use the default.
    #[must_use]
    pub fn charset(&self, label: &str) -> Option<GedcomEncoding> {
        let key: String = label
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | ' '))
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let mut chain = Some(self);
        while let Some(p) = chain {
            if let Some(&(_, e)) = p.charset.iter().find(|(l, _)| *l == key) {
                return Some(e);
            }
            chain = p.parent();
        }
        None
    }

    /// Whether byte statistics may choose Windows-1250 for single-byte
    /// text it labels `ANSI` (or not at all).
    #[must_use]
    pub fn uses_statistics(&self) -> bool {
        let mut chain = Some(self);
        while let Some(p) = chain {
            if p.stats {
                return true;
            }
            chain = p.parent();
        }
        false
    }

    /// The extension tags its rules keep as they are: what a 7.x header
    /// declares for them, with the registered URI when there is one, else
    /// the one oxidged documents ([`docs_uri`]).
    pub fn documented_tags(&self) -> impl Iterator<Item = DocumentedTag> + '_ {
        self.rules
            .iter()
            .filter(|r| matches!(r.op, interpret::Op::Known) && r.tag.starts_with('_'))
            .map(|r| DocumentedTag {
                tag: r.tag,
                context: r.ctx,
                rows: r.rows,
                uri: interpret::registered_uri(r.tag)
                    .map_or_else(|| docs_uri(self.id, r.tag), str::to_string),
                registered: interpret::registered_uri(r.tag).is_some(),
            })
    }
}

/// An extension tag a profile keeps as written and documents.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentedTag {
    /// The tag.
    pub tag: &'static str,
    /// Where it stands (the rule's context: tags from the record down,
    /// `.` separated, `*` one tag, `**` any, `|` and `,` alternatives).
    pub context: &'static str,
    /// The compatibility catalogue rows that describe it.
    pub rows: &'static [&'static str],
    /// The URI a 7.x header declares it with.
    pub uri: String,
    /// Whether the URI is registered elsewhere (GEDCOM-L), not oxidged's.
    pub registered: bool,
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.id, self.label)
    }
}

/// An export profile: what an importer reads, and how to write for it.
/// See [`Exporter`].
#[derive(Debug)]
pub struct ExportProfile {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) tier: u8,
    pub(crate) target: Target,
    pub(crate) steps: &'static [export::Step],
}

impl PartialEq for ExportProfile {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ExportProfile {}

impl ExportProfile {
    /// The profile's name.
    #[must_use]
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// The importer, in words.
    #[must_use]
    pub fn label(&self) -> &'static str {
        self.label
    }

    /// The research's priority tier (0 for the generic profiles).
    #[must_use]
    pub fn tier(&self) -> u8 {
        self.tier
    }

    /// How far its facts have been checked.
    #[must_use]
    pub fn verification(&self) -> Verification {
        if self.tier <= 1 {
            Verification::ToReverify
        } else {
            Verification::Research
        }
    }

    /// What it writes: version, encoding, line layout, limits.
    #[must_use]
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The rows of `export.tsv` its target and steps follow, once each,
    /// sorted.
    #[must_use]
    pub fn rows(&self) -> Vec<&'static str> {
        let mut rows: Vec<&'static str> = self
            .steps
            .iter()
            .flat_map(|s| s.rows.iter().copied())
            .chain(self.target.rows.iter().copied())
            .collect();
        rows.sort_unstable();
        rows.dedup();
        rows
    }
}

impl fmt::Display for ExportProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.id, self.label)
    }
}

/// Every import profile.
#[must_use]
pub fn import_profiles() -> &'static [&'static Profile] {
    import_data::PROFILES
}

/// The import profile named `id`.
#[must_use]
pub fn profile(id: &str) -> Option<&'static Profile> {
    import_data::PROFILES
        .iter()
        .copied()
        .find(|p| p.id.eq_ignore_ascii_case(id))
}

/// Every export profile.
#[must_use]
pub fn export_profiles() -> &'static [&'static ExportProfile] {
    export_data::PROFILES
}

/// The export profile named `id`.
#[must_use]
pub fn export_profile(id: &str) -> Option<&'static ExportProfile> {
    export_data::PROFILES
        .iter()
        .copied()
        .find(|p| p.id.eq_ignore_ascii_case(id))
}

/// Detects the producer of GEDCOM bytes: the header, then the tags of the
/// whole file (which this lexes). [`Detection::from_head`] reads the
/// header alone.
#[must_use]
pub fn detect(bytes: &[u8]) -> Detection {
    let mut detection = Detection::from_head(bytes);
    let tree = crate::tree::Tree::from_bytes(bytes.to_vec());
    let mut tags: Vec<String> = Vec::new();
    collect_tags(tree.records(), &mut tags);
    detection.refine(tags.iter().map(String::as_str));
    detection
}

fn collect_tags<'t>(
    nodes: impl Iterator<Item = crate::tree::StructureRef<'t>>,
    out: &mut Vec<String>,
) {
    for n in nodes {
        let tag = n.tag();
        if !crate::tree::Tag::new(tag).is_standard() && !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
        collect_tags(n.substructures(), out);
    }
}

/// The encoding a detection reads single-byte bytes in: the producer's
/// reading of the label, else Windows-1250 when byte statistics find it
/// under an `ANSI` (or no) label; `None` for the reader's default.
pub(crate) fn charset_for(
    detection: &Detection,
    declared: Option<&str>,
    body: &[u8],
) -> Option<GedcomEncoding> {
    let head = detection.head();
    // A version under CHAR names the code page (GEDitCOM, MacFamilyTree,
    // Ahnenblatt).
    if let Some(v) = head.charset_version.as_deref() {
        let by_version = match v.to_ascii_uppercase().replace([' ', '-', '_'], "").as_str() {
            "MACOSROMAN" | "MACINTOSH" | "MACROMAN" => Some(GedcomEncoding::MacRoman),
            "1252" | "CP1252" | "WINDOWS1252" => Some(GedcomEncoding::Windows1252),
            "1250" | "CP1250" | "WINDOWS1250" => Some(GedcomEncoding::Windows1250),
            "437" | "CP437" => Some(GedcomEncoding::Cp437),
            _ => None,
        };
        if by_version.is_some() {
            return by_version;
        }
    }
    let chosen = declared.and_then(|d| detection.profile().charset(d));
    let ambiguous = declared.is_none_or(|d| {
        matches!(
            d.trim().to_ascii_uppercase().as_str(),
            "ANSI" | "ASCII" | "LATIN1" | "ISO-8859-1" | "ISO8859-1"
        )
    });
    let ansi_like = matches!(
        chosen,
        None | Some(
            GedcomEncoding::Windows1252 | GedcomEncoding::Iso8859_1 | GedcomEncoding::Ascii
        )
    );
    if ambiguous
        && ansi_like
        && detection.profile().uses_statistics()
        && crate::encoding::reads_as_cp1250(body)
    {
        return Some(GedcomEncoding::Windows1250);
    }
    chosen
}

/// The records of `text` interpreted with `rules`, and
/// what was done; with the records written back as text in `version` when
/// anything changed.
pub(crate) fn interpret_text(
    text: &str,
    version: GedcomVersion,
    rules: &RuleSet,
) -> (Option<String>, InterpretReport) {
    let tree = crate::tree::Tree::parse(text);
    let mut records: Vec<draft::Draft<'_>> = tree.records().map(draft::Draft::of_ref).collect();
    let report = interpret::interpret_with(&mut records, version, rules);
    if !report.changed() {
        return (None, report);
    }
    let mut out = String::with_capacity(text.len() + text.len() / 16);
    for r in records {
        r.write_gedcom(&mut out, 0, version);
    }
    (Some(out), report)
}

/// Reads decoded text as `detection`'s producer meant it, at `level`: the
/// dataset (which remembers its producer) and what was interpreted.
/// `report`: whether the documented extensions kept are reported too.
///
/// The file is read once: each record a rule may change is interpreted as
/// it is read, the others are typed as they are
/// ([`crate::model::read::read_interpreting`]). Detection reads the
/// fingerprint tags of the file first, by a quick look at its lines, which
/// the records read then confirm. When they do not, or when a record's
/// interpretation would reorder the file's records, the file is read and
/// interpreted whole ([`read_whole`]).
pub(crate) fn read_interpreted(
    text: String,
    detection: &mut Detection,
    level: Interpretation,
    report: bool,
) -> (crate::Dataset, InterpretReport) {
    if level == Interpretation::None {
        return (crate::Dataset::parse(text), InterpretReport::default());
    }
    let before = detection.clone();
    let mut prints = detect::fingerprints_in(&text);
    prints.sort_unstable();
    detection.refine(prints.iter().copied());
    match read_once(text, detection, level, report, &prints) {
        Ok(read) => read,
        Err(text) => {
            *detection = before;
            read_whole(text, detection, level, report)
        }
    }
}

/// [`read_interpreted`] in one pass, with the fingerprint tags `prints`
/// (sorted) found: the text back when the file reads otherwise.
fn read_once(
    text: String,
    detection: &Detection,
    level: Interpretation,
    report: bool,
    prints: &[&'static str],
) -> Result<(crate::Dataset, InterpretReport), String> {
    let version = crate::tree::head_version(&text)
        .as_deref()
        .map_or(GedcomVersion::V5_5_1, GedcomVersion::from_version_str);
    let rules = RuleSet::of(detection, level);
    let len = text.len();
    let mut read = Vec::new();
    let (mut data, interpreted) = if rules.is_empty() {
        let data = crate::Dataset::parse(text);
        read.extend(data.store().other_tags().filter_map(detect::fingerprint));
        (data, InterpretReport::default())
    } else {
        let interp = interpret::Interpreter::new(&rules, version);
        let known = report || version.is_v7();
        crate::model::read::read_interpreting(text, interp, known, &mut read)?
    };
    read.sort_unstable();
    read.dedup();
    if read != prints {
        let mut text = std::mem::take(&mut data).store.into_input();
        text.truncate(len);
        return Err(text);
    }
    data.set_producer(detection.clone(), level);
    Ok((data, interpreted))
}

/// [`read_interpreted`] of the whole file at once: read, its producer
/// detected from every tag it has, then, when a rule may apply, every
/// record interpreted ([`interpret_text`]) and the result read again.
fn read_whole(
    text: String,
    detection: &mut Detection,
    level: Interpretation,
    report: bool,
) -> (crate::Dataset, InterpretReport) {
    let mut data = crate::Dataset::parse(text);
    detection.refine(data.store().other_tags());
    let rules = RuleSet::of(detection, level);
    let untyped = data
        .extra()
        .iter()
        .any(|n| crate::tree::Tag::new(n.tag()).is_standard());
    let known = report || data.version().is_v7();
    let mut interpreted = InterpretReport::default();
    if !rules.is_empty()
        && rules.may_apply(
            data.store(),
            data.store().other_tags(),
            untyped,
            known,
            data.version().is_v7(),
        )
    {
        // Only the text read is kept while it is interpreted: a dataset
        // interpreted is read again (and one that was not too: the rare
        // file whose rules found nothing to change after all).
        let version = data.version();
        let text = std::mem::take(&mut data).store.into_input();
        let (changed, done) = interpret_text(&text, version, &rules);
        interpreted = done;
        data = match changed {
            Some(changed) => {
                drop(text);
                crate::Dataset::parse(changed)
            }
            None => crate::Dataset::parse(text),
        };
    }
    data.set_producer(detection.clone(), level);
    (data, interpreted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a record whose rules rename, retype or make events of its
    /// substructures gives, interpreted where it is read: what the drafts of
    /// the whole file give, report included.
    #[test]
    fn records_interpreted_where_read_are_as_drafts() {
        let cases = [
            "0 HEAD\n1 SOUR FTM\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 _MILT\n2 DATE 1850\n2 PLAC Sampletown\n1 NAME Ann /Example/\n1 _MILT\n2 DATE 1850\n2 PLAC Sampletown\n1 EVEN\n2 TYPE Military service\n2 DATE 1851\n1 _MILT\n2 DATE 1851\n1 _TODO Check\n0 @I1@ INDI\n1 _MILT Y\n0 TRLR\n",
            "0 HEAD\n1 SOUR BROSKEEP\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 _INTE Field 4\n2 DATE 1900\n1 _NLIV\n1 _NMAR x\n2 NOTE kept\n1 DEAT Y\n0 @F1@ FAM\n1 _NMR\n0 TRLR\n",
            "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @R1@ REPO\n1 NAME Sample archive\n1 _EMAIL desk(at)example.invalid\n1 PHON 555\n0 @I1@ INDI\n1 NAME Ann /Example/\n2 _AKA Annie /Example/\n1 _FUN\n2 DATE 1900\n0 TRLR\n",
        ];
        let in_place = || crate::model::read::IN_PLACE.with(std::cell::Cell::get);
        for text in cases {
            let before = in_place();
            for report in [false, true] {
                let mut detection = Detection::from_head(text.as_bytes());
                let level = Interpretation::Lossless;
                let (fast, done) =
                    read_interpreted(text.to_string(), &mut detection, level, report);
                let mut detection = Detection::from_head(text.as_bytes());
                let (whole, all) = read_whole(text.to_string(), &mut detection, level, report);
                let write =
                    |d: &crate::Dataset| crate::GedcomWriter::new().write_to_string(d).unwrap();
                assert_eq!(write(&fast), write(&whole), "{text}");
                assert_eq!(done.mappings(), all.mappings(), "{text}");
            }
            assert!(in_place() > before, "not interpreted where read: {text}");
        }
    }
}
