//! Small helpers over owned structures, shared by interpretation and
//! export.

use std::borrow::Cow;

use super::draft::{Draft, DraftTag, Value};
use crate::tree::Xref;

/// A structure with a text payload (none when empty), the text copied.
pub(crate) fn text_node<'s>(tag: &'static str, text: &str) -> Draft<'s> {
    Draft {
        tag: DraftTag::new(tag),
        payload: if text.is_empty() {
            Value::None
        } else {
            Value::Text(Cow::Owned(text.to_string()))
        },
        ..Draft::default()
    }
}

/// A structure with a pointer payload.
pub(crate) fn pointer_node<'s>(tag: &'static str, xref: &str) -> Draft<'s> {
    Draft {
        tag: DraftTag::new(tag),
        payload: Value::Pointer(Cow::Owned(xref.to_string())),
        ..Draft::default()
    }
}

/// Whether `s` has tag `tag`.
#[inline]
pub(crate) fn is(s: &Draft<'_>, tag: &str) -> bool {
    s.tag.as_str() == tag
}

/// The first substructure tagged `tag`.
pub(crate) fn child<'a, 's>(s: &'a Draft<'s>, tag: &str) -> Option<&'a Draft<'s>> {
    s.substructures.iter().find(|c| is(c, tag))
}

/// The first substructure tagged `tag`, to change.
pub(crate) fn child_mut<'a, 's>(s: &'a mut Draft<'s>, tag: &str) -> Option<&'a mut Draft<'s>> {
    s.substructures.iter_mut().find(|c| is(c, tag))
}

/// The text of the first substructure tagged `tag`.
pub(crate) fn child_text<'a>(s: &'a Draft<'_>, tag: &str) -> Option<&'a str> {
    child(s, tag).and_then(Draft::text)
}

/// Whether `s` has a substructure tagged `tag`.
pub(crate) fn has(s: &Draft<'_>, tag: &str) -> bool {
    child(s, tag).is_some()
}

/// Gives `s` another tag.
pub(crate) fn retag(s: &mut Draft<'_>, tag: &'static str) {
    s.tag = DraftTag::new(tag);
}

/// Sets the text payload of `s` (none when empty), the text copied.
pub(crate) fn set_text(s: &mut Draft<'_>, text: &str) {
    s.payload = if text.is_empty() {
        Value::None
    } else {
        Value::Text(Cow::Owned(text.to_string()))
    };
}

/// The payload of `s` as text: its text, or its pointer.
#[inline]
pub(crate) fn payload_str<'a>(s: &'a Draft<'_>) -> &'a str {
    s.payload.as_str().unwrap_or_default()
}

/// Whether `s` says yes: `Y`, `YES`, `1`, `TRUE` (any case).
pub(crate) fn is_yes(s: &Draft<'_>) -> bool {
    matches!(
        payload_str(s).trim().to_ascii_uppercase().as_str(),
        "Y" | "YES" | "1" | "TRUE" | "J" | "JA" | "O" | "OUI"
    )
}

/// The given part of a `NAME` value: before the first slash, trimmed.
pub(crate) fn given_part(name: &str) -> &str {
    name.split('/').next().unwrap_or_default().trim()
}

/// The surname of a `NAME` value: between the first two slashes.
pub(crate) fn surname_part(name: &str) -> Option<&str> {
    let mut parts = name.split('/');
    parts.next()?;
    let surname = parts.next()?;
    parts.next().map(|_| surname.trim())
}

/// The part of a `NAME` value after its surname.
pub(crate) fn suffix_part(name: &str) -> &str {
    name.splitn(3, '/').nth(2).unwrap_or_default().trim()
}

/// A `NAME` value made of parts.
pub(crate) fn name_value(given: &str, surname: &str, suffix: &str) -> String {
    let mut out = String::new();
    if !given.is_empty() {
        out.push_str(given);
        out.push(' ');
    }
    out.push('/');
    out.push_str(surname);
    out.push('/');
    if !suffix.is_empty() {
        out.push(' ');
        out.push_str(suffix);
    }
    out
}

/// Inserts `new` into `parent` at `at`, unless a substructure equal to it
/// is already there: whether it was inserted.
pub(crate) fn insert_new<'s>(parent: &mut Draft<'s>, at: usize, new: Draft<'s>) -> bool {
    if parent.substructures.contains(&new) {
        return false;
    }
    let at = at.min(parent.substructures.len());
    parent.substructures.insert(at, new);
    true
}

/// Pushes `new` into `parent` unless an equal substructure is there.
pub(crate) fn push_new<'s>(parent: &mut Draft<'s>, new: Draft<'s>) -> bool {
    let at = parent.substructures.len();
    insert_new(parent, at, new)
}

/// An identifier `@<prefix><n>@` that `taken` says no record uses yet.
pub(crate) fn fresh_xref(taken: impl Fn(&str) -> bool, prefix: &str, next: &mut u32) -> Xref {
    loop {
        *next += 1;
        let candidate = format!("@{prefix}{next}@");
        if !taken(&candidate) {
            return Xref::new(candidate);
        }
    }
}

/// Every standard tag's long form (GEDCOM 5.5.1, Appendix A), which some
/// old producers write instead of the tag (X-167, TJ-035).
pub(crate) const LONG_FORMS: &[(&str, &str)] = &[
    ("ABBREVIATION", "ABBR"),
    ("ADDRESS", "ADDR"),
    ("ADDRESS1", "ADR1"),
    ("ADDRESS2", "ADR2"),
    ("ADOPTION", "ADOP"),
    ("ADULT_CHRISTENING", "CHRA"),
    ("AGENCY", "AGNC"),
    ("ALIAS", "ALIA"),
    ("ANCESTORS", "ANCE"),
    ("ANCES_INTEREST", "ANCI"),
    ("ANNULMENT", "ANUL"),
    ("ASSOCIATES", "ASSO"),
    ("AUTHOR", "AUTH"),
    ("BAPTISM", "BAPM"),
    ("BAPTISM-LDS", "BAPL"),
    ("BAR_MITZVAH", "BARM"),
    ("BAS_MITZVAH", "BASM"),
    ("BIRTH", "BIRT"),
    ("BLESSING", "BLES"),
    ("BURIAL", "BURI"),
    ("CALL_NUMBER", "CALN"),
    ("CASTE", "CAST"),
    ("CAUSE", "CAUS"),
    ("CENSUS", "CENS"),
    ("CHANGE", "CHAN"),
    ("CHARACTER", "CHAR"),
    ("CHILD", "CHIL"),
    ("CHILDREN_COUNT", "NCHI"),
    ("CHRISTENING", "CHR"),
    ("CONCATENATION", "CONC"),
    ("CONFIRMATION", "CONF"),
    ("CONFIRMATION_L", "CONL"),
    ("CONTINUED", "CONT"),
    ("COPYRIGHT", "COPR"),
    ("CORPORATE", "CORP"),
    ("CREMATION", "CREM"),
    ("COUNTRY", "CTRY"),
    ("DEATH", "DEAT"),
    ("DESCENDANTS", "DESC"),
    ("DESCENDANT_INT", "DESI"),
    ("DESTINATION", "DEST"),
    ("DIVORCE", "DIV"),
    ("DIVORCE_FILED", "DIVF"),
    ("PHY_DESCRIPTION", "DSCR"),
    ("EDUCATION", "EDUC"),
    ("EMIGRATION", "EMIG"),
    ("ENDOWMENT", "ENDL"),
    ("ENGAGEMENT", "ENGA"),
    ("EVENT", "EVEN"),
    ("FAMILY", "FAM"),
    ("FAMILY_CHILD", "FAMC"),
    ("FAMILY_FILE", "FAMF"),
    ("FAMILY_SPOUSE", "FAMS"),
    ("FIRST_COMMUNION", "FCOM"),
    ("FORMAT", "FORM"),
    ("GEDCOM", "GEDC"),
    ("GIVEN_NAME", "GIVN"),
    ("GRADUATION", "GRAD"),
    ("HEADER", "HEAD"),
    ("HUSBAND", "HUSB"),
    ("IDENT_NUMBER", "IDNO"),
    ("IMMIGRATION", "IMMI"),
    ("INDIVIDUAL", "INDI"),
    ("LANGUAGE", "LANG"),
    ("LATITUDE", "LATI"),
    ("LONGITUDE", "LONG"),
    ("MARRIAGE_BANN", "MARB"),
    ("MARR_CONTRACT", "MARC"),
    ("MARR_LICENSE", "MARL"),
    ("MARRIAGE", "MARR"),
    ("MARR_SETTLEMENT", "MARS"),
    ("MEDIA", "MEDI"),
    ("NAME_PREFIX", "NPFX"),
    ("NAME_SUFFIX", "NSFX"),
    ("NATIONALITY", "NATI"),
    ("NATURALIZATION", "NATU"),
    ("NICKNAME", "NICK"),
    ("NOBILITY_TITLE", "TITL"),
    ("OBJECT", "OBJE"),
    ("OCCUPATION", "OCCU"),
    ("ORDINANCE", "ORDI"),
    ("ORDINATION", "ORDN"),
    ("PEDIGREE", "PEDI"),
    ("PHONE", "PHON"),
    ("PHONETIC", "FONE"),
    ("PLACE", "PLAC"),
    ("POSTAL_CODE", "POST"),
    ("PROBATE", "PROB"),
    ("PROPERTY", "PROP"),
    ("PUBLICATION", "PUBL"),
    ("QUALITY_OF_DATA", "QUAY"),
    ("REFERENCE", "REFN"),
    ("RELATIONSHIP", "RELA"),
    ("RELIGION", "RELI"),
    ("REPOSITORY", "REPO"),
    ("RESIDENCE", "RESI"),
    ("RESTRICTION", "RESN"),
    ("RETIREMENT", "RETI"),
    ("REC_FILE_NUMBER", "RFN"),
    ("REC_ID_NUMBER", "RIN"),
    ("ROMANIZED", "ROMN"),
    ("SEALING_CHILD", "SLGC"),
    ("SEALING_SPOUSE", "SLGS"),
    ("SOURCE", "SOUR"),
    ("SURN_PREFIX", "SPFX"),
    ("SOC_SEC_NUMBER", "SSN"),
    ("STATE", "STAE"),
    ("STATUS", "STAT"),
    ("SUBMITTER", "SUBM"),
    ("SUBMISSION", "SUBN"),
    ("SURNAME", "SURN"),
    ("TEMPLE", "TEMP"),
    ("TITLE", "TITL"),
    ("TRAILER", "TRLR"),
    ("VERSION", "VERS"),
    ("WIFE", "WIFE"),
    ("WILL", "WILL"),
];

/// The standard tag a long form names.
pub(crate) fn long_form(tag: &str) -> Option<&'static str> {
    // (Every long form is a word of letters: an extension tag is none.)
    if tag.len() <= 4 || !tag.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    LONG_FORMS
        .iter()
        .find(|(long, _)| long.eq_ignore_ascii_case(tag))
        .map(|&(_, short)| short)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`long_form`] passes over what does not start with a letter.
    #[test]
    fn long_forms_start_with_a_letter() {
        assert!(
            LONG_FORMS
                .iter()
                .all(|(long, _)| long.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()))
        );
        assert_eq!(long_form("birth"), Some("BIRT"));
        assert_eq!(long_form("_BIRTH"), None);
    }

    #[test]
    fn name_parts() {
        assert_eq!(given_part("Ann Marie /Example/ Jr"), "Ann Marie");
        assert_eq!(surname_part("Ann /Example/ Jr"), Some("Example"));
        assert_eq!(surname_part("Ann Example"), None);
        assert_eq!(suffix_part("Ann /Example/ Jr"), "Jr");
        assert_eq!(name_value("Ann", "Example", ""), "Ann /Example/");
        assert_eq!(long_form("individual"), Some("INDI"));
        assert_eq!(long_form("NAME"), None);
    }
}
