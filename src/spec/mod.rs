//! The GEDCOM specifications as data: a validator and a conformance repair
//! pass over [`tree`](crate::tree) structures, for GEDCOM 5.5.1, 7.0 and
//! 7.1.
//!
//! # Tables
//!
//! Every structure type of each version, its tag, its payload type, its
//! permitted substructures with their cardinalities and every enumeration
//! set are generated at development time by `tools/spec-tables/gen.py`
//! into `src/spec/tables.rs`, which holds facts only:
//!
//! - GEDCOM 7.0 from FamilySearch/GEDCOM `extracted-files/` of the 7.0.18
//!   release, and 7.1, a draft, from the same files on the `v7.1` branch of
//!   FamilySearch/GEDCOM (Apache-2.0, attributed in `NOTICE`);
//! - GEDCOM 5.5.1 from the gedcom-go transcription of the specification
//!   (MIT, attributed in `NOTICE`), corrected and completed against the
//!   PDF by `tools/spec-tables/551-errata.tsv`, which cites a page for each
//!   row.
//!
//! The tables take a few kilobytes: types are numbered, tags are indices
//! into one shared list, and no build script or dependency is involved.
//!
//! # Validation
//!
//! [`validate`] checks a tree (parsed, or owned [`Structure`]s) against the
//! tables of a version; [`validate_text`] and [`validate_bytes`] also check
//! what only the text shows. Every deviation is reported, with its line,
//! not just the first:
//!
//! | Rule | [`DeviationKind`] |
//! |---|---|
//! | 7.x is UTF-8; a 5.5.1 file is in the character set `HEAD.CHAR` names | `Encoding` |
//! | Line grammar: a level, single spaces, a tag, no empty payload after a space; 7.x: no blank line, nothing before the level, no LF CR | `LineSyntax` |
//! | Levels start at 0, rise by one at most, have no leading zero (5.5.1: at most two digits) | `Level` |
//! | Identifiers: 7.x `@[A-Z0-9_]+@` but not `@VOID@`; 5.5.1 a letter, digit or `_` first, no control character, at most 22 characters; on records only (not `HEAD` or `TRLR`); unique | `Xref` |
//! | `@` in text: 7.x doubles a leading one, 5.5.1 every one outside an `@#…@` escape | `Escape` |
//! | 5.5.1 lines of at most 255 characters, terminator included | `LineLength` |
//! | `CONT` (5.5.1 also `CONC`) right under the line it continues, before the other substructures, never under a pointer, without substructures; 5.5.1 `CONC` never splits at a space; 7.x has no `CONC` | `Continuation` |
//! | 5.5.1 records under 32K | `RecordSize` |
//! | Tags: 7.x `[A-Z][A-Z0-9_]*` or `_[A-Z0-9_]+`, 5.5.1 alphanumerics, upper case unless the tag starts with `_`; a standard tag is one the version defines | `UnknownTag` |
//! | Each standard tag under a superstructure that permits it | `Misplaced` |
//! | At most the permitted number of each substructure | `Cardinality` |
//! | Every required substructure; 7.x: a note translation has a `MIME` or a `LANG` | `MissingRequired` |
//! | Payloads of their type: none, `Y`, integers, names, dates, ages, times, languages, media types (7.x `MIME` of a text: `text/…`, any `text/` type, as 7.0.17 clarifies), file paths (7.x: WHATWG valid URL strings), URIs (7.x: RFC 3986 URI references), coordinates (7.x: the ABNF of §2.15–2.16), tag definitions (an extension tag and a URI reference), pointers where pointers belong and only there; 7.x: a structure has a payload or a substructure (records and pseudo-structures aside) | `Payload` |
//! | Enumeration values: 7.x exact or an extension value, 5.5.1 in any case (p. 21); open sets admit any value; 5.5.1 file formats are open, but a media type is no format ([accepted practice](#accepted-practice)) | `EnumValue` |
//! | Pointers name a record of the type the structure points to | `PointerTarget` |
//! | Pointers name a record of the file (7.x `@VOID@` and 5.5.1 substructure and network pointers aside) | `DanglingPointer` |
//! | 7.x (7.0.17): an `ALIA` does not point to its own record; pointers between source and multimedia records, and between source and shared note records, form no cycle (the pointer that closes one, records taken in order) | `Cycle` |
//! | `HEAD` first and once, `TRLR` last, once and empty, `GEDC.VERS` naming the version | `Header` |
//! | No banned character in a payload: 7.x C0 controls but tab and line breaks, DEL, C1 controls, U+FFFE and U+FFFF; 5.5.1 no control character at all | `Character` |
//!
//! ## Accepted practice
//!
//! One deliberate exception to strict conformance, an owner decision: the
//! 5.5.1 file formats (`MULTIMEDIA_FORMAT`, p. 54) are an open list. The
//! specification lists the seven formats of 1999 (`bmp`, `gif`, `jpg`,
//! `ole`, `pcx`, `tif`, `wav`), while files name `png`, `pdf`, `webp` and
//! the like, and no other 5.5.1 structure says what a file is: holding the
//! list closed would write every such `FORM` as an extension, then its
//! `FILE` (which requires a `FORM`), then the `OBJE` (which requires a
//! `FILE`), then every pointer to it. So the validator reports no
//! deviation for any word as a 5.5.1 `FORM` of a file (a value of the list
//! in any case, as every 5.5.1 value); a media type (`image/png`) is no
//! format and is reported. Strict reading therefore accepts any 5.5.1
//! format but a media type. 7.x, whose `FORM` is a media type, is unchanged.
//!
//! Extension structures (`_TAG`) are checked for what holds everywhere —
//! tags, identifiers, pointers, characters — and their substructures mean
//! what their definer says. Payload sizes of 5.5.1 (`{Size=1:90}`) are not
//! checked: they describe storage limits of 1999-era systems, and no reader
//! relies on them.
//!
//! # Conformance repair
//!
//! [`conform`] rewrites owned structures so that [`validate`] finds nothing,
//! deterministically, keeping all data: what has no standard place becomes
//! an extension structure (`_TAG`) where it was, with its payload and
//! substructures. Each change is reported as a [`Repair`]. This is the
//! structural half of a conformant writer, which
//! [`GedcomWriter::write_tree`](crate::GedcomWriter::write_tree) and
//! [`write_structures`](crate::GedcomWriter::write_structures) run before
//! the line emitter applies the line-level rules (escapes, line length,
//! `CONC`, terminators). The header is completed as the writer completes
//! every header — `GEDC.VERS`; in 5.5.1 `GEDC.FORM`, `CHAR`, `SOUR` and
//! `SUBM` (with a submitter record named `Unknown` when there is none); in
//! 7.x no `CHAR` and no `GEDC.FORM` — which is not reported as a repair.
//!
//! | # | Situation | 7.x repair | 5.5.1 repair |
//! |---|---|---|---|
//! | a | Closed-enumeration value not in the set | `OTHER` and a `PHRASE` with the value, where the set has `OTHER` and the structure takes a `PHRASE` it has not; otherwise `_TAG` (a list with such a value too) | `_TAG` |
//! | b | Value in the wrong case | Upper case; a list is written `A, B` | The specification's spelling |
//! | b | A media type as a 5.5.1 file format ([accepted practice](#accepted-practice)) | — | Its file format, lower case: the extension of a known type (`image/jpeg` is `jpg`, `application/pdf` is `pdf`), else the subtype without `x-`, `vnd.` and `+…` (`image/png` is `png`); any other format as written |
//! | c | Text or nothing where a pointer belongs, a dangling pointer | `@VOID@`, and a `PHRASE` with the text or the pointer, where the structure takes one it has not; nothing becomes `@VOID@`; otherwise `_TAG` with the text | `_TAG` with the text (an empty nullable pointer stays) |
//! | c | A pointer where text belongs | The pointer as text | Same |
//! | c | A pointer to a record of another type | `_TAG` with the pointer | Same |
//! | c | A pointer of an extension to no record | The pointer as text | Same |
//! | c | An `ALIA` to its own record; a pointer that closes a cycle between source, multimedia and shared note records (7.0.17; records in order, the pointer that closes the cycle) | `_TAG` with the pointer | — |
//! | d | A date or an age outside the grammar | Empty payload and a `PHRASE` with the text, where the structure takes one it has not; otherwise `_TAG` | A date value becomes the date phrase `(text)`; otherwise `_TAG` |
//! | d | An event (`Y` payload) with other text; text where no payload belongs | `Y` (nothing) and a `NOTE` with the text, where a `NOTE` is permitted; otherwise `_TAG` | Same |
//! | d | A file path that is no valid URL string, a coordinate outside its grammar | 7.x: the file path percent-encoded as conversion writes a 5.5.1 file reference (a local path as a `file` URL), the coordinate in its canonical form (upper-case direction, no extra leading zero), where that is valid; otherwise `_TAG` | `_TAG` |
//! | d | Any other payload outside its grammar (names, integers, languages, media types, URIs, tag definitions; 7.x `MIME` of a text not `text/…`) | `_TAG` | Same |
//! | e | A standard tag not permitted here, an unknown tag, a tag outside the grammar, a `CONT`/`CONC` structure | `_TAG` (letters upper-cased, other characters `_`); a standard structure only one type has the tag of, valid as that type, keeps its meaning: `HEAD.SCHMA` declares `TAG _TAG` with the type's URI, unless the file uses `_TAG` already | `_TAG` |
//! | f | More occurrences than permitted | The first ones kept; the others become `_TAG`, declared with their type's URI as in (e) | The first ones kept; the others become `_TAG` |
//! | g | A required substructure missing | Synthesised from the list below; otherwise the superstructure becomes `_TAG`, and so on up | Same |
//! | g | 7.x structure with neither payload nor substructure (§1.2) | `Y` where `Y` is a payload of the type; otherwise left out, as it holds nothing | — |
//! | h | Identifiers outside the grammar, duplicated, `@VOID@`, on a substructure, on `HEAD` or `TRLR`; banned characters | Renamed (`@i 1@` becomes `@I_1@`, made unique with `_2`, `_3`…, never an identifier a pointer holds; pointers follow a renamed invalid identifier, and a duplicate's pointers stay with the first record); left out | Same; 5.5.1 tabs become spaces |
//! | i | `HEAD` missing, misplaced or repeated; `TRLR` missing, misplaced, repeated or not empty | `HEAD` first (another becomes a `_HEAD` record); one empty `TRLR` last (one with content becomes a `_TRLR` record) | Same |
//! | j | A 5.5.1 record over 32K | — | Inline notes longer than the pointer line that replaces them (32 bytes at most) moved to `NOTE` records, longest first, until the record fits or none is left; a note over 32K itself split at line breaks into consecutive `NOTE` records (its citations with the first). A standard record still over the limit holds no such note: its other structures, its own text (a shared note's) and a header's note cannot move out losslessly, and are left so, as is an extension record (the limit is a recommendation, p. 10) |
//!
//! An extension tag the header documents with a standard URI (7.x §1.5.1)
//! is that standard structure, wherever it is, and is repaired as one; an
//! extension that cannot stay standard is never given such a tag.
//!
//! Synthesised substructures (g), besides the header's: a `FORM` of a `FILE` from the file's extension (7.x a media type,
//! `application/octet-stream` when unknown; 5.5.1 the extension, lower case,
//! in the list's spelling when it has one, `jpeg` as `jpg`, and `unknown`
//! for a path without one); 7.x `LANG` of a translation (`und`),
//! `ASSO.ROLE` (`OTHER`) and `SLGC.FAMC` (`@VOID@`); any other required
//! text without required parts of its own, such as `TYPE`, `RELA` or a
//! record's `NAME`: `Unknown`. Dates, ages, coordinates and files are never
//! invented.
//!
//! ```rust
//! use oxidged::spec::{conform, validate, validate_text, DeviationKind, RepairKind};
//! use oxidged::tree::parse_tree;
//! use oxidged::GedcomVersion;
//!
//! let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n0 TRLR\n";
//! let issues = validate(&parse_tree(text), GedcomVersion::V7_0);
//! assert_eq!(issues.len(), 1);
//! assert_eq!(issues[0].kind, DeviationKind::EnumValue);
//! assert_eq!(issues[0].to_string(), "line 5: SEX \"male\": not a value of enumset-SEX");
//!
//! // The text also shows the line grammar: two spaces before a tag.
//! let issues = validate_text("0 HEAD\n1 GEDC\n2  VERS 7.0\n0 TRLR\n");
//! assert_eq!(issues[0].kind, DeviationKind::LineSyntax);
//!
//! // The repair keeps the value, as an extension structure.
//! let mut records = parse_tree(text).to_structures();
//! let repairs = conform(&mut records, GedcomVersion::V7_0);
//! assert_eq!(repairs.len(), 1);
//! assert_eq!(repairs[0].kind, RepairKind::EnumValue);
//! assert!(validate(&records, GedcomVersion::V7_0).is_empty());
//! assert_eq!(records[1].substructures[0].tag, "_SEX");
//! ```

pub(crate) mod conform;
pub(crate) mod graph;
pub(crate) mod index;
mod lines;
pub(crate) mod payload;
pub(crate) mod schema;
#[rustfmt::skip]
pub(crate) mod tables;
mod validate;

use std::fmt;

use crate::GedcomVersion;
use crate::encoding::decode;
use crate::tree::{Structure, Tree};

use payload::Family;
pub(crate) use payload::is_simple_date;
pub(crate) use schema::Schema;
use sealed::Sealed;
pub(crate) use validate::is_external_pointer;

pub use crate::writer::{Repair, RepairKind};
pub use conform::conform;

/// A way in which a dataset does not follow its specification.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Deviation {
    /// The 1-based line where it shows; 0 when no line holds it (a missing
    /// record, the encoding of the whole file).
    pub line: u32,
    /// The rule broken.
    pub kind: DeviationKind,
    /// What is wrong, in words.
    pub detail: Box<str>,
}

impl fmt::Display for Deviation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            f.write_str(&self.detail)
        } else {
            write!(f, "line {}: {}", self.line, self.detail)
        }
    }
}

/// The rule a [`Deviation`] breaks. The [module documentation](self) lists
/// what each covers.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviationKind {
    /// The bytes are not in the encoding the version or the header says.
    Encoding,
    /// A line does not follow the line grammar.
    LineSyntax,
    /// A level number is malformed or jumps.
    Level,
    /// An identifier is malformed, duplicated or misplaced.
    Xref,
    /// An `@` in text is not escaped as the version wants.
    Escape,
    /// A 5.5.1 line is longer than 255 characters.
    LineLength,
    /// A continuation line is misplaced, or 7.x uses `CONC`.
    Continuation,
    /// A 5.5.1 record is larger than 32K.
    RecordSize,
    /// A tag is malformed, or not one the version defines.
    UnknownTag,
    /// A standard structure where its superstructure does not permit it.
    Misplaced,
    /// More occurrences of a substructure than permitted.
    Cardinality,
    /// A required substructure is missing.
    MissingRequired,
    /// A payload is not of the structure's payload type.
    Payload,
    /// An enumeration value is not in its set.
    EnumValue,
    /// A pointer names a record of the wrong type.
    PointerTarget,
    /// A pointer names no record.
    DanglingPointer,
    /// The header or the trailer is missing, misplaced, repeated or wrong.
    Header,
    /// A character the version bans.
    Character,
    /// A pointer 7.x prohibits between records: an `ALIA` to its own
    /// record, or one that closes a cycle of pointers between source,
    /// multimedia and shared note records (7.0.17).
    Cycle,
}

/// A dataset [`validate`] reads: a parsed [`Tree`] or owned
/// [`Structure`]s (records, header and trailer included).
pub trait Dataset: sealed::Sealed {}

impl Dataset for Tree {}
impl Dataset for [Structure] {}
impl Dataset for Vec<Structure> {}

mod sealed {
    use super::{Deviation, GedcomVersion, Structure, Tree, validate};

    /// Runs the validator.
    pub trait Sealed {
        fn deviations(&self, version: GedcomVersion, record_size: bool) -> Vec<Deviation>;
    }

    impl Sealed for Tree {
        fn deviations(&self, version: GedcomVersion, record_size: bool) -> Vec<Deviation> {
            validate::run(
                self.records(),
                version.rules(),
                validate::Options { record_size },
            )
        }
    }

    impl Sealed for [Structure] {
        fn deviations(&self, version: GedcomVersion, record_size: bool) -> Vec<Deviation> {
            validate::run(
                self.iter(),
                version.rules(),
                validate::Options { record_size },
            )
        }
    }

    impl Sealed for Vec<Structure> {
        fn deviations(&self, version: GedcomVersion, record_size: bool) -> Vec<Deviation> {
            self.as_slice().deviations(version, record_size)
        }
    }
}

/// The grammar family of a version.
fn family(version: GedcomVersion) -> Family {
    Family::of(version.rules())
}

/// Validates a dataset against the specification of `version`: structure,
/// payloads, identifiers, pointers, header and trailer, characters, and the
/// estimated size of 5.5.1 records. See the [module documentation](self) for
/// the rules; the line grammar needs the text ([`validate_text`]).
///
/// The deviations come in line order.
pub fn validate<D: Dataset + ?Sized>(data: &D, version: GedcomVersion) -> Vec<Deviation> {
    data.deviations(version, true)
}

/// Validates GEDCOM text, decoded already, against the specification of the
/// version its header declares (`HEAD.GEDC.VERS`, as
/// [`GedcomVersion::from_version_str`] reads it):
/// the line grammar and everything [`validate`] checks.
///
/// The deviations come in line order.
#[must_use]
pub fn validate_text(text: &str) -> Vec<Deviation> {
    let tree = Tree::parse(text);
    let version = tree.version();
    let mut out = lines::check(text, family(version));
    out.extend(tree.deviations(version, false));
    out.sort_by_key(|d| d.line);
    out
}

/// Validates GEDCOM bytes: their encoding (7.x is UTF-8; 5.5.1 bytes match
/// `HEAD.CHAR`), then the text as [`validate_text`] does.
#[must_use]
pub fn validate_bytes(bytes: &[u8]) -> Vec<Deviation> {
    let decoded = decode(bytes);
    let tree = Tree::parse(decoded.text.as_str());
    let version = tree.version();
    let fam = family(version);
    let mut out: Vec<Deviation> = lines::encoding(&decoded, fam).into_iter().collect();
    out.extend(lines::check(&decoded.text, fam));
    out.extend(tree.deviations(version, false));
    out.sort_by_key(|d| d.line);
    out
}

#[cfg(test)]
mod tests;
