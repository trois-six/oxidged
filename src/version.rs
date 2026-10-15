//! GEDCOM version detection and handling.
//!
//! This module provides the ability to detect and work with different GEDCOM versions,
//! primarily GEDCOM 5.5.1 and GEDCOM 7.0. The two versions have significant differences
//! in encoding, structure, and feature support.
//!
//! # Version Differences
//!
//! ## GEDCOM 5.5.1 (1999/2019)
//! - Multiple character encodings (ANSEL, ASCII, UTF-8, UNICODE)
//! - `CONC` and `CONT` for line continuation
//! - All `@` characters doubled in payloads
//! - `CHAR` tag in header specifies encoding
//! - `SUBN` (submission) record supported
//!
//! ## GEDCOM 7.0 (2021+)
//! - UTF-8 encoding only (with optional BOM)
//! - Only `CONT` for line continuation (`CONC` removed)
//! - Only leading `@` doubled in payloads
//! - `SCHMA` tag for extension schema
//! - `SNOTE` for shared notes
//! - New structures: `EXID`, `MIME`, `CREA`, `SDATE`, `CROP`, `NO`, `INIL`, `TRAN`
//! - URIs for all structure types

use crate::spec::index::{self, SchemaIndex};
use crate::spec::{Schema, tables};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::fmt;

/// A GEDCOM version this crate reads and writes.
///
/// Reading maps whatever `HEAD.GEDC.VERS` declares to the version whose rules
/// apply ([`GedcomVersion::from_version_str`]): 7.0 for `7` and `7.0.x`, 7.1
/// for any later 7.x, and 5.5.1 for anything else (5.5, 5.5.1, 5.5.5, no
/// declaration). The declared string itself stays on the header.
///
/// Each version has a static table of the rules the writer follows,
/// [`GedcomVersion::rules`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum GedcomVersion {
    /// GEDCOM 5.5.1 (1999, re-released as a standard in 2019).
    #[default]
    V5_5_1,

    /// GEDCOM 7.0 (2021): UTF-8 only, no `CONC`, extension schemas.
    V7_0,

    /// GEDCOM 7.1: the line grammar and header of 7.0, with more structures.
    V7_1,
}

impl GedcomVersion {
    /// The version whose rules apply to a declared `GEDC.VERS` value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxidged::version::GedcomVersion;
    ///
    /// assert_eq!(GedcomVersion::from_version_str("5.5.1"), GedcomVersion::V5_5_1);
    /// assert_eq!(GedcomVersion::from_version_str("5.5"), GedcomVersion::V5_5_1);
    /// assert_eq!(GedcomVersion::from_version_str("7.0"), GedcomVersion::V7_0);
    /// assert_eq!(GedcomVersion::from_version_str("7.0.14"), GedcomVersion::V7_0);
    /// assert_eq!(GedcomVersion::from_version_str("7.1"), GedcomVersion::V7_1);
    /// assert_eq!(GedcomVersion::from_version_str("4.0"), GedcomVersion::V5_5_1);
    /// ```
    #[must_use]
    pub fn from_version_str(version: &str) -> Self {
        let version = version.trim();
        let Some(rest) = version.strip_prefix('7') else {
            return GedcomVersion::V5_5_1;
        };
        if rest.is_empty() {
            return GedcomVersion::V7_0;
        }
        let Some(rest) = rest.strip_prefix('.') else {
            // `70`, `7a`: not a 7.x version.
            return GedcomVersion::V5_5_1;
        };
        let minor = rest.split('.').next().unwrap_or_default();
        if minor.bytes().all(|b| b == b'0') {
            GedcomVersion::V7_0
        } else {
            GedcomVersion::V7_1
        }
    }

    /// Whether this is GEDCOM 7.0 or a later 7.x.
    #[must_use]
    pub fn is_v7(self) -> bool {
        matches!(self, GedcomVersion::V7_0 | GedcomVersion::V7_1)
    }

    /// Whether this is GEDCOM 5.5.1.
    #[must_use]
    pub fn is_v5(self) -> bool {
        matches!(self, GedcomVersion::V5_5_1)
    }

    /// The `GEDC.VERS` payload of this version: `5.5.1`, `7.0` or `7.1`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.rules().vers_payload
    }

    /// The writing rules of this version.
    #[must_use]
    pub const fn rules(self) -> &'static VersionRules {
        match self {
            GedcomVersion::V5_5_1 => &V551,
            GedcomVersion::V7_0 => &V70,
            GedcomVersion::V7_1 => &V71,
        }
    }
}

impl fmt::Display for GedcomVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How text escapes the `@` sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AtEscape {
    /// GEDCOM 5.5.1 (p. 12): every `@` of text is doubled, except in an
    /// escape sequence such as `@#DJULIAN@`.
    AllAtSigns,
    /// GEDCOM 7.x (§1.3): only a leading `@` of a line is doubled.
    LeadingOnly,
}

/// The grammar of cross-reference identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum XrefGrammar {
    /// GEDCOM 5.5.1 (`pointer`, `pointer_char`, `xref_ID`, pp. 13 and 16):
    /// a letter, digit or `_`, then any characters but `@` and the control
    /// characters (spaces, `!` and `:` included), at most `max_len`
    /// characters with the delimiters.
    V551 {
        /// The longest identifier, delimiters included.
        max_len: usize,
    },
    /// GEDCOM 7.x (§1.3): `@[A-Z0-9_]+@`, never `@VOID@`.
    V7,
}

/// The rules of the line grammar and the header that one GEDCOM version sets
/// for written files.
///
/// The writer consults these tables instead of testing version strings: one
/// static per version, reached with [`GedcomVersion::rules`].
///
/// ```
/// use oxidged::GedcomVersion;
///
/// let rules = GedcomVersion::V5_5_1.rules();
/// assert_eq!(rules.max_line_length(), Some(255));
/// assert!(rules.uses_conc());
/// assert!(GedcomVersion::V7_0.rules().max_line_length().is_none());
/// assert!(!GedcomVersion::V7_0.rules().is_valid_xref("@i1@"));
/// ```
#[derive(Debug)]
pub struct VersionRules {
    pub(crate) version: GedcomVersion,
    /// The `HEAD.GEDC.VERS` payload.
    pub(crate) vers_payload: &'static str,
    /// The longest line, terminator included (5.5.1 p. 11).
    pub(crate) max_line_len: Option<usize>,
    /// Whether `CONC` exists.
    pub(crate) conc: bool,
    pub(crate) at_escape: AtEscape,
    /// The `HEAD.GEDC.FORM` payload, when the version has one.
    pub(crate) gedc_form: Option<&'static str>,
    /// Whether `HEAD.CHAR` exists (it must then name the output encoding).
    pub(crate) head_char: bool,
    /// Whether `HEAD.SOUR` and `HEAD.SUBM` are required.
    pub(crate) head_sour_subm: bool,
    pub(crate) xref: XrefGrammar,
    /// The deepest level a structure is written at: one less than the
    /// deepest level of the version (5.5.1: two digits; 7.x: the 255 levels
    /// readers nest), so that its `CONT` lines fit.
    pub(crate) max_level: usize,
    /// The specification tables of the version ([`crate::spec`]).
    pub(crate) spec: &'static Schema,
    /// Their index, computed at compile time.
    pub(crate) index: &'static SchemaIndex,
}

/// GEDCOM 5.5.1.
pub static V551: VersionRules = VersionRules {
    version: GedcomVersion::V5_5_1,
    vers_payload: "5.5.1",
    max_line_len: Some(255),
    conc: true,
    at_escape: AtEscape::AllAtSigns,
    gedc_form: Some("LINEAGE-LINKED"),
    head_char: true,
    head_sour_subm: true,
    xref: XrefGrammar::V551 { max_len: 22 },
    max_level: 98,
    spec: &tables::V551,
    index: &index::V551,
};

/// GEDCOM 7.0.
pub static V70: VersionRules = V70_LINES;

/// GEDCOM 7.1, whose line grammar and header rules are those of 7.0.
pub static V71: VersionRules = VersionRules {
    version: GedcomVersion::V7_1,
    vers_payload: "7.1",
    spec: &tables::V71,
    index: &index::V71,
    ..V70_LINES
};

/// The rules of 7.0, whose line rules 7.1 shares.
const V70_LINES: VersionRules = VersionRules {
    version: GedcomVersion::V7_0,
    vers_payload: "7.0",
    max_line_len: None,
    conc: false,
    at_escape: AtEscape::LeadingOnly,
    gedc_form: None,
    head_char: false,
    head_sour_subm: false,
    xref: XrefGrammar::V7,
    max_level: 254,
    spec: &tables::V70,
    index: &index::V70,
};

impl VersionRules {
    /// The version these rules belong to.
    #[must_use]
    pub fn version(&self) -> GedcomVersion {
        self.version
    }

    /// The `HEAD.GEDC.VERS` payload written.
    #[must_use]
    pub fn vers(&self) -> &'static str {
        self.vers_payload
    }

    /// The longest line, level, identifier, tag, payload, delimiters and
    /// terminator included: `Some(255)` in 5.5.1, `None` (no limit) in 7.x.
    #[must_use]
    pub fn max_line_length(&self) -> Option<usize> {
        self.max_line_len
    }

    /// Whether long payloads are continued with `CONC` (5.5.1 only).
    #[must_use]
    pub fn uses_conc(&self) -> bool {
        self.conc
    }

    /// Whether every `@` of text is doubled (5.5.1) rather than only a
    /// leading one (7.x).
    #[must_use]
    pub fn doubles_every_at_sign(&self) -> bool {
        self.at_escape == AtEscape::AllAtSigns
    }

    /// The `HEAD.GEDC.FORM` payload, `LINEAGE-LINKED` in 5.5.1; 7.x has no
    /// `FORM`.
    #[must_use]
    pub fn gedc_form(&self) -> Option<&'static str> {
        self.gedc_form
    }

    /// Whether the header names its character set in `HEAD.CHAR` (5.5.1);
    /// 7.x is always UTF-8 and has no `CHAR`.
    #[must_use]
    pub fn has_head_char(&self) -> bool {
        self.head_char
    }

    /// Whether a cross-reference identifier, delimiters included, follows
    /// the grammar of the version.
    #[must_use]
    #[inline]
    pub fn is_valid_xref(&self, xref: &str) -> bool {
        let Some(id) = xref
            .strip_prefix('@')
            .and_then(|x| x.strip_suffix('@'))
            .filter(|id| !id.is_empty())
        else {
            return false;
        };
        match self.xref {
            XrefGrammar::V7 => {
                id != "VOID"
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            }
            XrefGrammar::V551 { max_len } => {
                let bytes = id.as_bytes();
                let first = bytes
                    .first()
                    .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_');
                // One pass over the bytes of an ASCII identifier, as nearly
                // all are; bytes bound characters, so it needs no count.
                let mut ascii = true;
                for &b in bytes {
                    if b == b'@' || b.is_ascii_control() {
                        return false;
                    }
                    ascii &= b.is_ascii();
                }
                first
                    && if ascii {
                        xref.len() <= max_len
                    } else {
                        xref.chars().count() <= max_len && !id.chars().any(char::is_control)
                    }
            }
        }
    }

    /// Whether a tag follows the grammar of the version: 7.x `[A-Z][A-Z0-9_]*`
    /// or `_[A-Z0-9_]+`; 5.5.1 letters, digits and underscores, starting with
    /// a letter or `_`, upper case unless it starts with `_`, at most 31
    /// characters (p. 41).
    #[must_use]
    #[inline]
    pub fn is_valid_tag(&self, tag: &str) -> bool {
        let b = tag.as_bytes();
        let Some(&first) = b.first() else {
            return false;
        };
        match self.xref {
            XrefGrammar::V7 => {
                let tail_ok = |s: &[u8]| {
                    s.iter()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
                };
                if first == b'_' {
                    b.len() > 1 && tail_ok(&b[1..])
                } else {
                    first.is_ascii_uppercase() && tail_ok(b)
                }
            }
            XrefGrammar::V551 { .. } => {
                // One pass: letters, digits and `_`, and whether any letter
                // is lower case.
                let (mut ok, mut lower) = (true, false);
                for c in b {
                    ok &= c.is_ascii_alphanumeric() || *c == b'_';
                    lower |= c.is_ascii_lowercase();
                }
                b.len() <= MAX_TAG_551
                    && (first == b'_' || first.is_ascii_uppercase())
                    && ok
                    && (first == b'_' || !lower)
                    && (first != b'_' || b.len() > 1)
            }
        }
    }

    /// Whether a character may not appear in a payload. GEDCOM 7.x (§1.1)
    /// bans the C0 controls but tab, DEL, the C1 controls, U+FFFE and
    /// U+FFFF; GEDCOM 5.5.1 (`any_char`, pp. 11 and 14) admits no control
    /// character at all, tab included. Line breaks inside text are written
    /// as `CONT` lines, never as characters.
    #[must_use]
    #[inline]
    pub fn is_banned(&self, c: char) -> bool {
        is_banned(c) || (c == '\t' && self.xref != XrefGrammar::V7)
    }
}

/// The longest GEDCOM 5.5.1 tag (p. 41, `TAG`: 1 to 31 characters).
pub(crate) const MAX_TAG_551: usize = 31;

/// The characters every version bans (see [`VersionRules::is_banned`],
/// which adds tab in 5.5.1).
pub(crate) fn is_banned(c: char) -> bool {
    (c < ' ' && c != '\t')
        || ('\u{7f}'..='\u{9f}').contains(&c)
        || c == '\u{fffe}'
        || c == '\u{ffff}'
}

/// Detects the GEDCOM version from file content.
///
/// This function reads the `HEAD.GEDC.VERS` line of the `HEAD` record, with
/// any line terminator; it does not look at other records.
///
/// # Arguments
///
/// * `content` - The GEDCOM file content as a string slice
///
/// # Returns
///
/// The detected `GedcomVersion`, or `GedcomVersion::V5_5_1` as default if detection fails.
///
/// # Examples
///
/// ```
/// use oxidged::version::{detect_version, GedcomVersion};
///
/// let content = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR";
/// assert_eq!(detect_version(content), GedcomVersion::V7_0);
///
/// let content = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 TRLR";
/// assert_eq!(detect_version(content), GedcomVersion::V5_5_1);
/// ```
#[must_use]
pub fn detect_version(content: &str) -> GedcomVersion {
    crate::tree::head_version(content).map_or(GedcomVersion::V5_5_1, |v| {
        GedcomVersion::from_version_str(&v)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_from_str() {
        for (vers, version) in [
            ("5.5.1", GedcomVersion::V5_5_1),
            ("5.5", GedcomVersion::V5_5_1),
            ("5.5.5", GedcomVersion::V5_5_1),
            ("", GedcomVersion::V5_5_1),
            ("4.0", GedcomVersion::V5_5_1),
            ("70", GedcomVersion::V5_5_1),
            ("7", GedcomVersion::V7_0),
            ("7.0", GedcomVersion::V7_0),
            (" 7.0.14 ", GedcomVersion::V7_0),
            ("7.00", GedcomVersion::V7_0),
            ("7.1", GedcomVersion::V7_1),
            ("7.1.2", GedcomVersion::V7_1),
            ("7.2", GedcomVersion::V7_1),
        ] {
            assert_eq!(GedcomVersion::from_version_str(vers), version, "{vers:?}");
        }
    }

    #[test]
    fn test_version_display() {
        assert_eq!(GedcomVersion::V5_5_1.to_string(), "5.5.1");
        assert_eq!(GedcomVersion::V7_0.to_string(), "7.0");
        assert_eq!(GedcomVersion::V7_1.to_string(), "7.1");
    }

    #[test]
    fn test_rules() {
        let v5 = GedcomVersion::V5_5_1.rules();
        assert_eq!(v5.version(), GedcomVersion::V5_5_1);
        assert!(v5.uses_conc() && v5.doubles_every_at_sign() && v5.has_head_char());
        assert_eq!(v5.gedc_form(), Some("LINEAGE-LINKED"));
        assert_eq!(v5.max_line_length(), Some(255));
        for v7 in [GedcomVersion::V7_0, GedcomVersion::V7_1] {
            let r = v7.rules();
            assert_eq!(r.version(), v7);
            assert_eq!(r.vers(), v7.as_str());
            assert!(!r.uses_conc() && !r.doubles_every_at_sign() && !r.has_head_char());
            assert_eq!(r.gedc_form(), None);
            assert_eq!(r.max_line_length(), None);
        }
    }

    #[test]
    fn test_xref_grammar() {
        let (v5, v7) = (&V551, &V70);
        for x in ["@I1@", "@F_2@", "@VOID@X@"] {
            assert_eq!(v7.is_valid_xref(x), x != "@VOID@X@", "{x}");
        }
        assert!(!v7.is_valid_xref("@VOID@"));
        assert!(!v7.is_valid_xref("@i1@"));
        assert!(!v7.is_valid_xref("@@"));
        assert!(v5.is_valid_xref("@i1@"));
        assert!(v5.is_valid_xref("@VOID@"));
        assert!(v5.is_valid_xref("@ABCDEFGHIJKLMNOPQRST@"));
        assert!(!v5.is_valid_xref("@ABCDEFGHIJKLMNOPQRSTU@"));
        assert!(!v5.is_valid_xref("@#I1@"));
        assert!(!v5.is_valid_xref("@I\t1@"));
        // p. 13: spaces, `!`, `:` and `_` first are 5.5.1 identifier characters.
        for x in ["@I 1@", "@I1!2@", "@!2@X@", "@NET:I1@", "@_I1@", "@Iö1@"] {
            assert_eq!(v5.is_valid_xref(x), x != "@!2@X@", "{x}");
        }
    }

    #[test]
    fn test_tag_grammar() {
        let (v5, v7) = (&V551, &V70);
        for t in ["NAME", "_X", "_EXT_1", "A1"] {
            assert!(v5.is_valid_tag(t) && v7.is_valid_tag(t), "{t}");
        }
        assert!(v5.is_valid_tag("_low"));
        assert!(!v7.is_valid_tag("_low"));
        assert!(v5.is_valid_tag(&"_X".repeat(15)));
        assert!(!v5.is_valid_tag(&"X".repeat(32)));
        assert!(v7.is_valid_tag(&"X".repeat(32)));
        for t in ["", "_", "name", "1A", "NA-ME", "NA ME"] {
            assert!(!v5.is_valid_tag(t) && !v7.is_valid_tag(t), "{t}");
        }
    }

    #[test]
    fn test_tab_is_banned_in_551_only() {
        assert!(V551.is_banned('\t'));
        assert!(!V70.is_banned('\t'));
        assert!(V70.is_banned('\u{7}') && V551.is_banned('\u{7}'));
    }

    #[test]
    fn test_banned() {
        for c in [
            '\0', '\u{7}', '\n', '\r', '\u{7f}', '\u{85}', '\u{fffe}', '\u{ffff}',
        ] {
            assert!(is_banned(c), "{c:?}");
        }
        for c in ['\t', ' ', 'a', '\u{a0}', '\u{feff}'] {
            assert!(!is_banned(c), "{c:?}");
        }
    }

    #[test]
    fn test_detect_version_v5() {
        let content = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n0 TRLR";
        assert_eq!(detect_version(content), GedcomVersion::V5_5_1);
    }

    #[test]
    fn test_detect_version_v7() {
        let content = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR";
        assert_eq!(detect_version(content), GedcomVersion::V7_0);

        let content = "0 HEAD\n1 GEDC\n2 VERS 7.0.14\n0 TRLR";
        assert_eq!(detect_version(content), GedcomVersion::V7_0);
    }

    #[test]
    fn test_detect_version_default() {
        // No version found, defaults to 5.5.1
        let content = "0 HEAD\n0 TRLR";
        assert_eq!(detect_version(content), GedcomVersion::V5_5_1);
    }

    #[test]
    fn test_is_predicates() {
        assert!(GedcomVersion::V5_5_1.is_v5());
        assert!(!GedcomVersion::V5_5_1.is_v7());
        assert!(!GedcomVersion::V7_0.is_v5());
        assert!(GedcomVersion::V7_0.is_v7());
        assert!(GedcomVersion::V7_1.is_v7());
    }
}
