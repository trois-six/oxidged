//! Hand-written fictitious cases (`tests/fixtures/conformance/cases/*.txt`),
//! labelled conformance (C) or leniency (L). Each case is parsed, written,
//! re-read and checked; see `support/cases.rs`.

use crate::support::cases::{self, Case};
use crate::support::ratchet;
use std::collections::HashSet;

const FILES: [&str; 4] = ["lines.txt", "gedcom551.txt", "gedcom7.txt", "edges.txt"];

pub fn all_cases() -> Vec<Case> {
    let mut all = Vec::new();
    for f in FILES {
        let text = String::from_utf8(crate::support::read_fixture(&format!("cases/{f}"))).unwrap();
        all.extend(cases::parse_file(&text));
    }
    all.extend(generated_cases());
    all
}

/// Cases too large to read as text.
fn generated_cases() -> Vec<Case> {
    // A 5.5.1 record over 32,767 bytes (p.10): the writer must keep every
    // record under the limit, moving the text to NOTE records.
    let mut big = String::from(
        "0 HEAD\n1 SOUR EXAMPLE_APP\n1 SUBM @U0@\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n\
         0 @I1@ INDI\n1 NAME Ann /Example/\n1 NOTE Long research log.\n",
    );
    for i in 0..400 {
        big += &format!(
            "2 CONT Entry {i:03}: the register of Sampleton was searched again without a new result, page by page.\n"
        );
    }
    big += "0 @U0@ SUBM\n1 NAME Example Submitter\n0 TRLR\n";
    let mut c = Case::new("G5-RECORD-32K", cases::Kind::L, big);
    c.purpose =
        "5.5.1 record over 32K: written as records under the limit, text kept (p.10)".into();
    c.model.push("Entry 399: the register of Sampleton".into());
    vec![c]
}

#[test]
fn case_ids_are_unique_and_labelled() {
    let all = all_cases();
    let mut seen = HashSet::new();
    for c in &all {
        assert!(seen.insert(c.id.clone()), "duplicate case id {}", c.id);
        assert!(!c.purpose.is_empty(), "case {} has no purpose", c.id);
    }
    assert!(all.len() >= 120, "{} cases", all.len());
}

#[test]
fn hand_written_cases() {
    let all = all_cases();
    let mut failures = Vec::new();
    for c in &all {
        failures.extend(cases::run("case", c).failures);
    }
    ratchet::verify("case", all.len(), failures);
}

/// A conformance case must have conformant input: the checker that judges
/// the writer must accept it.
#[test]
fn conformance_case_inputs_are_conformant() {
    use crate::support::checker::{self, Target};
    use crate::support::tree;
    let mut bad = Vec::new();
    for c in all_cases().iter().filter(|c| c.kind == cases::Kind::C) {
        let text = tree::decode_bytes(&c.input);
        let target = Target::from_vers(&tree::declared_vers(&text).unwrap_or_default());
        for i in checker::check(&text, target, None) {
            bad.push(format!("{}: {i}", c.id));
        }
    }
    assert!(bad.is_empty(), "{} issues:\n{}", bad.len(), bad.join("\n"));
}
