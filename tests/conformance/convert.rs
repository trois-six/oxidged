//! Conversion between 5.5.1 and 7.x beyond the expected files of
//! `vendored.rs`: round trips that keep the data, conversion of the
//! lossless tree, and the 7.x to 5.5.1 direction.
//!
//! * Round trip (`roundtrip/…`): every fixture through 5.5.1 → 7.x →
//!   5.5.1 and 7.x → 5.5.1 → 7.x, keeping the 5.5.1 structures 7.x drops:
//!   no text is lost, and every output validates.
//! * Tree (`tree-convert/…`): each 5.5.1 test file converted as a lossless
//!   tree is equivalent to its expected 7.0 file as the typed dataset is.
//! * Downgrade (`downgrade/…`): 7.x structures written as 5.5.1.

use crate::support::adapter::{self, Target};
use crate::support::checker::{self, Target as CheckTarget};
use crate::support::ratchet::{self, Failure};
use crate::support::{semantic, tree};
use std::collections::HashMap;

/// Every fixture file: the vendored test files, the maximal and full files,
/// the crate's own fixtures.
fn fixtures() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for dir in ["test-files/5", "test-files/7", ""] {
        let path = crate::support::fixture(dir);
        let mut names: Vec<String> = std::fs::read_dir(&path)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".ged"))
            .collect();
        names.sort();
        for n in names {
            let rel = if dir.is_empty() {
                n.clone()
            } else {
                format!("{dir}/{n}")
            };
            out.push((rel.clone(), crate::support::read_fixture(&rel)));
        }
    }
    for n in ["sample.ged", "simple.ged", "washington.ged"] {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(n);
        out.push((n.to_string(), std::fs::read(p).unwrap()));
    }
    out
}

/// The texts of `before` that `after` lacks: each text payload under its
/// tag (an extension tag `_X` counting as `X`), or under any tag, outside
/// the header, as a multiset. A conversion may move data (a multimedia link's files into a
/// record) and rename identifiers; it must not lose a value.
fn lost(before: &str, after: &str, any_tag: bool) -> Vec<String> {
    let texts = |text: &str| {
        let mut out: HashMap<(String, String), usize> = HashMap::new();
        let (version, records) = tree::parse(text);
        for r in records.iter().filter(|r| r.tag != "HEAD") {
            r.walk(&mut Vec::new(), &mut |path, n| {
                let tree::Payload::Text(t) = &n.payload else {
                    return;
                };
                let parent = path.iter().rev().nth(1).copied().unwrap_or("");
                let tag = n.tag.trim_start_matches('_');
                let tag = match (tag, parent.trim_start_matches('_')) {
                    // The medium of a file: `MEDI` in a link, `TYPE` in a
                    // record (a 5.5.1 link's files move to a record).
                    ("TYPE", "FORM") => "MEDI",
                    // A source in words becomes its record's note.
                    ("SOUR", _) => "NOTE",
                    (tag, _) => tag,
                };
                // Values compare by meaning (dates, ages, times, paths), a
                // date phrase as its words, a path with either slash,
                // enumeration and language names in any case.
                let t = if tag == "DATE" {
                    t.trim_start_matches('(').trim_end_matches(')')
                } else {
                    t
                };
                let mut value = semantic::normalise_value(tag, t, version);
                if tag == "FILE" {
                    value = value.replace('\\', "/");
                }
                if any_tag {
                    // 5.5.1 spells some values otherwise (`DNS/CAN`), and
                    // says `Y` and `OTHER` by the structure and the phrase.
                    value = value.to_uppercase().replace(['/', '-'], "_");
                    if value == "Y" || value == "OTHER" {
                        return;
                    }
                } else if matches!(tag, "LANG" | "MEDI" | "PEDI" | "RESN") {
                    value = value.to_uppercase();
                }
                let tag = if any_tag {
                    String::new()
                } else {
                    tag.to_string()
                };
                *out.entry((tag, value)).or_insert(0) += 1;
            });
        }
        out
    };
    let (b, a) = (texts(before), texts(after));
    let mut gone: Vec<String> = b
        .into_iter()
        .filter(|(k, n)| a.get(k).copied().unwrap_or(0) < *n)
        .map(|((tag, value), _)| format!("{tag} {value:?}"))
        .collect();
    gone.sort();
    gone
}

/// Writes `bytes`, read, in `targets` one after the other, each output
/// read back for the next; every output.
fn chain(bytes: &[u8], targets: &[Target]) -> Result<Vec<String>, String> {
    let mut outs: Vec<String> = Vec::new();
    for &t in targets {
        let input = outs.last().map_or(bytes, |o| o.as_bytes());
        let m = adapter::read(input)?;
        outs.push(adapter::write_keeping(&m, t)?);
    }
    Ok(outs)
}

/// The check target of a write target.
fn check_target(t: Target) -> CheckTarget {
    match t {
        Target::V551 => CheckTarget::V551,
        Target::V71 => CheckTarget::V71,
        Target::V70 | Target::Same => CheckTarget::V70,
    }
}

/// 5.5.1 → 7.x → 5.5.1 keeps the data: every text of each fixture written
/// as 5.5.1 is in what that output, written as 7.x, read back and written
/// as 5.5.1, holds. 7.x → 5.5.1 → 7.x keeps every text too, under any tag
/// (5.5.1 keeps what it has no place for as extensions). Every output
/// validates (but for 5.5.1 records over 32K that no note can bring under
/// the limit).
#[test]
fn round_trips_keep_the_data() {
    let mut failures = Vec::new();
    let mut n = 0;
    for (name, bytes) in fixtures() {
        for v7 in [Target::V70, Target::V71] {
            for (family, targets, any_tag) in [
                ("551", [Target::V551, v7, Target::V551], false),
                ("7", [v7, Target::V551, v7], true),
            ] {
                n += 1;
                let id = format!("roundtrip/{family}/{name}/{v7:?}");
                let outs = match chain(&bytes, &targets) {
                    Ok(o) => o,
                    Err(e) => {
                        failures.push(Failure::new(&id, "write", "FATAL", e));
                        continue;
                    }
                };
                for (out, &t) in outs.iter().zip(&targets) {
                    for i in checker::check(out, check_target(t), None) {
                        if i.rule != "record-size" {
                            failures.push(Failure::new(
                                &id,
                                format!("output:{}", i.rule),
                                "NONCONFORMANT",
                                i.to_string(),
                            ));
                        }
                    }
                }
                let gone = lost(&outs[0], &outs[2], any_tag);
                if !gone.is_empty() {
                    failures.push(Failure::new(
                        &id,
                        "roundtrip",
                        "LOST",
                        format!("{} lost: {}", gone.len(), gone.join("; ")),
                    ));
                }
            }
        }
    }
    ratchet::verify("roundtrip", n, failures);
}

/// The lossless tree of a 5.5.1 test file converts as the typed dataset
/// does: written as 7.0, each is equivalent to the expected file but where
/// the dataset is.
#[test]
fn trees_convert_as_datasets_do() {
    let mut failures = Vec::new();
    let mut n = 0;
    for (name, bytes) in fixtures() {
        let Some(base) = name.strip_prefix("test-files/5/") else {
            continue;
        };
        let expected = crate::support::fixture(&format!("test-files/7/{base}"));
        if !expected.exists() {
            continue;
        }
        n += 1;
        let id = format!("tree-convert/{base}");
        let expected = tree::decode_bytes(&std::fs::read(expected).unwrap());
        let text = tree::decode_bytes(&bytes);
        let from_tree = adapter::write_tree(&text, Target::V70).unwrap();
        let from_model = adapter::read(&bytes)
            .and_then(|m| adapter::write(&m, Target::V70))
            .unwrap();
        let opts = semantic::Options::conversion();
        let mut tree_diff = semantic::compare(&expected, &from_tree, &opts);
        let model_diff = semantic::compare(&expected, &from_model, &opts);
        tree_diff.retain(|d| !model_diff.contains(d));
        if !tree_diff.is_empty() {
            failures.push(Failure::new(
                &id,
                "convert",
                "DIFF",
                format!("{} differences: {}", tree_diff.len(), tree_diff.join("; ")),
            ));
        }
        for i in checker::check(&from_tree, CheckTarget::V70, None) {
            failures.push(Failure::new(
                &id,
                format!("output:{}", i.rule),
                "NONCONFORMANT",
                i.to_string(),
            ));
        }
    }
    ratchet::verify("tree-convert", n, failures);
}

/// 7.x structures written as 5.5.1: what 5.5.1 can say is said its way,
/// and the output conforms.
#[test]
fn downgrades_to_5_5_1() {
    const HEAD: &str = "0 HEAD\n1 GEDC\n2 VERS 7.0\n";
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "role",
            "0 @I1@ INDI\n1 ASSO @I2@\n2 ROLE FRIEND\n1 ASSO @I2@\n2 ROLE OTHER\n3 PHRASE Landlord\n0 @I2@ INDI\n",
            &[
                "1 ASSO @I2@\n2 RELA friend\n",
                "1 ASSO @I2@\n2 RELA Landlord\n",
            ],
        ),
        (
            "exid",
            "0 @I1@ INDI\n1 EXID 42\n2 TYPE https://gedcom.io/terms/v7/AFN\n1 EXID 7\n2 TYPE https://gedcom.io/terms/v7/RFN#Archive\n1 EXID 9\n2 TYPE https://gedcom.io/terms/v7/RIN#Sampler\n",
            &["1 AFN 42\n", "1 RFN Archive:7\n", "1 RIN 9\n"],
        ),
        (
            "translation",
            "0 @I1@ INDI\n1 NAME Ann /Example/\n2 TRAN アン\n3 LANG ja-Hrkt\n2 TRAN Ann\n3 LANG ja-Latn\n",
            &["2 FONE アン\n3 TYPE kana\n", "2 ROMN Ann\n3 TYPE romaji\n"],
        ),
        (
            "language",
            "0 @U1@ SUBM\n1 NAME Ann\n1 LANG de\n",
            &["1 LANG German\n"],
        ),
        (
            "media",
            "0 @M1@ OBJE\n1 FILE file:///C:/photos/a%20b.png\n2 FORM image/png\n3 MEDI PHOTO\n",
            &["1 FILE C:\\photos\\a b.png\n2 FORM png\n3 TYPE photo\n"],
        ),
        (
            "name-type",
            "0 @I1@ INDI\n1 NAME Ann /Example/\n2 TYPE OTHER\n3 PHRASE Stage name\n",
            &["2 TYPE Stage name\n"],
        ),
        (
            "event-role",
            "0 @S1@ SOUR\n1 TITL Register\n0 @I1@ INDI\n1 BIRT\n2 SOUR @S1@\n3 EVEN BIRT\n4 ROLE OTHER\n5 PHRASE Informant\n",
            &["4 ROLE (Informant)\n"],
        ),
        (
            "shared-note",
            "0 @I1@ INDI\n1 SNOTE @N1@\n0 @N1@ SNOTE Shared text\n",
            &["1 NOTE @N1@\n", "0 @N1@ NOTE Shared text\n"],
        ),
        (
            "dual-year",
            "0 @I1@ INDI\n1 BIRT\n2 DATE 30 JAN 1649\n3 PHRASE 30 JAN 1648/49\n",
            &["2 DATE 30 JAN 1648/49\n"],
        ),
        (
            "png-object",
            "0 @M1@ OBJE\n1 FILE photo.png\n2 FORM image/png\n0 @I1@ INDI\n1 OBJE @M1@\n",
            &[
                "0 @M1@ OBJE\n1 FILE photo.png\n2 FORM png\n",
                "1 OBJE @M1@\n",
            ],
        ),
        (
            "event-association",
            "0 @I1@ INDI\n1 BAPM\n2 ASSO @I2@\n3 ROLE GODP\n0 @I2@ INDI\n",
            &["2 _ASSO @I2@\n3 RELA godparent\n"],
        ),
        (
            "kept-submission",
            "1 SCHMA\n2 TAG _SUBN https://gedcom.io/terms/v5.5.1/SUBN\n2 TAG _SUBMISSION https://gedcom.io/terms/v5.5.1/record-SUBN\n1 _SUBN @B1@\n0 @B1@ _SUBMISSION\n1 FAMF Example\n",
            &["1 SUBN @B1@\n", "0 @B1@ SUBN\n1 FAMF Example\n"],
        ),
    ];
    let mut failures = Vec::new();
    for (name, body, wanted) in cases {
        let id = format!("downgrade/{name}");
        // Lines at level 1 first continue the header.
        let input = format!("{HEAD}{body}0 TRLR\n");
        for (label, out) in [
            (
                "model",
                adapter::read(input.as_bytes()).and_then(|m| adapter::write(&m, Target::V551)),
            ),
            ("tree", adapter::write_tree(&input, Target::V551)),
        ] {
            let out = match out {
                Ok(o) => o,
                Err(e) => {
                    failures.push(Failure::new(&id, label, "FATAL", e));
                    continue;
                }
            };
            for w in *wanted {
                if !out.contains(w) {
                    failures.push(Failure::new(
                        &id,
                        label,
                        "MISSING",
                        format!("{w:?} not in\n{out}"),
                    ));
                }
            }
            for i in checker::check(&out, CheckTarget::V551, None) {
                failures.push(Failure::new(
                    &id,
                    format!("{label}:output:{}", i.rule),
                    "NONCONFORMANT",
                    i.to_string(),
                ));
            }
        }
    }
    ratchet::verify("downgrade", cases.len(), failures);
}
