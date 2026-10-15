//! The header of a written file, completed for the target version.
//!
//! Whatever header the data carries (or none), the written `HEAD` has:
//!
//! - `GEDC` with `VERS` naming the target version and, in 5.5.1 only,
//!   `FORM LINEAGE-LINKED`;
//! - in 5.5.1, `CHAR` naming the encoding of the output bytes; the input's
//!   `CHAR` (and its `VERS`, which describes the input bytes) is not
//!   carried, and 7.x has no `CHAR`;
//! - in 5.5.1, the required `SOUR` (`oxidged` and its version when the data
//!   has none) and `SUBM` pointer;
//! - no cross-reference identifier.
//!
//! Every other substructure is kept, in its order. The structures added go
//! where the typed model writes them: `GEDC`, `CHAR` and `SOUR` first, `SUBM`
//! after `DEST` and `DATE`.

use crate::tree::{Payload, Structure, Xref};
use crate::version::VersionRules;

/// The product written as `HEAD.SOUR` when the data names none.
pub(crate) const PRODUCT: &str = "oxidged";

/// The products pre-release builds of this crate wrote as `HEAD.SOUR`
/// (before it was named oxidged): what they wrote is the crate's too.
pub(crate) const FORMER_PRODUCTS: &[&str] = &["ged_io"];

/// The text of a required structure nothing else can fill (the name of a
/// stub submitter, a missing `TYPE`, …).
pub(crate) const PLACEHOLDER: &str = "Unknown";

/// Whether the completed header needs a `SUBM` pointer that `head` lacks.
pub(crate) fn needs_submitter(head: &Structure, rules: &VersionRules) -> bool {
    rules.head_sour_subm && head.first("SUBM").is_none()
}

/// Completes `head` for `rules`. `charset` is the `CHAR` payload of the
/// output; `submitter` the pointer to add when [`needs_submitter`] holds.
pub(crate) fn complete(
    head: &mut Structure,
    rules: &VersionRules,
    charset: &str,
    submitter: Option<String>,
) {
    if submitter.is_none() && is_complete(head, rules, charset) {
        return;
    }
    head.xref = None;
    head.tag = "HEAD".into();
    head.payload = Payload::None;

    // GEDC first, with its VERS (and 5.5.1 FORM) first.
    let mut gedc = match head.substructures.iter().position(|s| s.tag == "GEDC") {
        Some(at) => head.substructures.remove(at),
        None => Structure::new("GEDC"),
    };
    gedc.xref = None;
    gedc.payload = Payload::None;
    gedc.substructures
        .retain(|s| s.tag != "VERS" && s.tag != "FORM");
    let mut front = vec![text("VERS", rules.vers_payload)];
    if let Some(form) = rules.gedc_form {
        front.push(text("FORM", form));
    }
    gedc.substructures.splice(0..0, front);

    head.substructures.retain(|s| s.tag != "CHAR");
    let mut front = vec![gedc];
    if rules.head_char {
        front.push(text("CHAR", charset));
    }
    if rules.head_sour_subm && head.first("SOUR").is_none() {
        let mut sour = text("SOUR", PRODUCT);
        sour.substructures
            .push(text("VERS", env!("CARGO_PKG_VERSION")));
        sour.substructures.push(text("NAME", PRODUCT));
        front.push(sour);
    }
    head.substructures.splice(0..0, front);
    if let Some(submitter) = submitter {
        // After what the typed model writes before it (`model::Header`), so
        // that a header read back from the output is written the same.
        let at = head
            .substructures
            .iter()
            .rposition(|s| matches!(s.tag.as_str(), "GEDC" | "CHAR" | "SOUR" | "DEST" | "DATE"))
            .map_or(0, |i| i + 1);
        head.substructures.insert(
            at,
            Structure {
                payload: Payload::Pointer(Xref::new(submitter)),
                ..Structure::new("SUBM")
            },
        );
    }
}

/// Whether [`complete`] leaves `head` as it is (with no submitter to add):
/// a header the writer completed already, written again.
fn is_complete(head: &Structure, rules: &VersionRules, charset: &str) -> bool {
    let is_text = |s: &Structure, tag: &str, value: &str| {
        s.tag == tag
            && s.xref.is_none()
            && matches!(&s.payload, Payload::Text(t) if **t == *value)
            && s.substructures.is_empty()
    };
    let mut subs = head.substructures.iter();
    let Some(gedc) = subs.next() else {
        return false;
    };
    let mut gedc_subs = gedc.substructures.iter();
    let gedc_ok = gedc.tag == "GEDC"
        && gedc.xref.is_none()
        && gedc.payload == Payload::None
        && gedc_subs
            .next()
            .is_some_and(|v| is_text(v, "VERS", rules.vers_payload))
        && rules
            .gedc_form
            .is_none_or(|form| gedc_subs.next().is_some_and(|f| is_text(f, "FORM", form)))
        && gedc_subs.all(|s| s.tag != "VERS" && s.tag != "FORM");
    let char_ok = !rules.head_char || subs.next().is_some_and(|c| is_text(c, "CHAR", charset));
    head.xref.is_none()
        && head.tag == "HEAD"
        && head.payload == Payload::None
        && gedc_ok
        && char_ok
        && subs.all(|s| s.tag != "CHAR" && s.tag != "GEDC")
        && (!rules.head_sour_subm || head.first("SOUR").is_some())
}

/// The stub submitter record a 5.5.1 file without one points to.
pub(crate) fn stub_submitter(xref: String) -> Structure {
    let mut subm = Structure {
        xref: Some(Xref::new(xref)),
        ..Structure::new("SUBM")
    };
    subm.substructures.push(text("NAME", PLACEHOLDER));
    subm
}

fn text(tag: &str, value: &str) -> Structure {
    Structure {
        payload: Payload::Text(value.into()),
        ..Structure::new(tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::parse_tree;
    use crate::version::{V70, V551};

    fn head(text: &str) -> Structure {
        parse_tree(text).to_structures().remove(0)
    }

    #[test]
    fn seven_has_no_char_and_no_form() {
        let mut h = head(
            "0 HEAD\n1 CHAR ANSEL\n2 VERS 1\n1 GEDC\n2 VERS 5.5\n2 FORM LINEAGE-LINKED\n1 _X y\n",
        );
        complete(&mut h, &V70, "UTF-8", None);
        assert_eq!(
            h.to_gedcom(0, crate::GedcomVersion::V7_0),
            "0 HEAD\n1 GEDC\n2 VERS 7.0\n1 _X y\n"
        );
    }

    /// A header completed already is written as it is, without a copy.
    #[test]
    fn a_complete_header_is_left_as_it_is() {
        for rules in [&V551, &V70] {
            let mut h = head("0 HEAD\n1 SOUR Sample\n1 GEDC\n2 VERS 5.5\n1 CHAR ANSEL\n1 _X y\n");
            assert!(!is_complete(&h, rules, "UTF-8"));
            complete(&mut h, rules, "UTF-8", None);
            assert!(is_complete(&h, rules, "UTF-8"));
            let mut again = h.clone();
            complete(&mut again, rules, "UTF-8", None);
            assert_eq!(again, h);
            // Another output encoding is another header.
            assert_eq!(is_complete(&h, rules, "UTF-16"), !rules.head_char);
        }
    }

    #[test]
    fn five_gets_its_required_lines() {
        let mut h =
            head("0 @H@ HEAD\n1 CHAR UNICODE\n1 CHAR ANSEL\n1 GEDC\n2 FORM Lineage-Linked\n");
        assert!(needs_submitter(&h, &V551));
        complete(&mut h, &V551, "UTF-8", Some("@U1@".into()));
        let out = h.to_gedcom(0, crate::GedcomVersion::V5_5_1);
        assert!(out.starts_with(
            "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n1 SOUR oxidged\n2 VERS "
        ), "{out}");
        assert!(out.ends_with("2 NAME oxidged\n1 SUBM @U1@\n"), "{out}");
    }
}
