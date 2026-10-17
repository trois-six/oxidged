//! The features the documentation describes, exercised on the suite's fixtures
//! (`feature/…`): dual 5.5.1/7.0 support, reading and writing, the streaming
//! parser, indexed lookups, GEDZIP, the listed encodings, JSON export and
//! performance (opt-in). Leniency ("Compatible") is the subject of every L
//! case; the encodings of the `encoding` and `matrix` families.

use crate::support::adapter::{self, Target as WriteTarget};
use crate::support::checker::{self, Target};
use crate::support::ratchet::{self, Failure};
use crate::support::{read_fixture, tree};

/// Fixtures every feature runs on: (name, bytes).
fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let mut v = vec![
        ("maximal551", read_fixture("maximal551.ged")),
        ("extensions70", read_fixture("extensions70.ged")),
    ];
    for name in ["7/date-all", "7/obje-1", "5/notes-1", "5/char_utf16le-2"] {
        v.push((
            Box::leak(format!("test-files-{}", name.replace('/', "-")).into_boxed_str()),
            read_fixture(&format!("test-files/{name}.ged")),
        ));
    }
    v
}

fn class_of(e: &str) -> &'static str {
    if e.starts_with("PANIC") {
        "PANIC"
    } else {
        "FATAL"
    }
}

#[test]
fn readme_features() {
    let mut failures = Vec::new();
    let mut n = 0;
    for (name, bytes) in fixtures() {
        let id = |f: &str| format!("feature/{f}/{name}");
        let model = match adapter::read(&bytes) {
            Ok(m) => m,
            Err(e) => {
                failures.push(Failure::new(id("read"), "parse", class_of(&e), e));
                continue;
            }
        };
        // Dual format: the same data written as 5.5.1 and as 7.0, both conformant.
        for (t, wt, label) in [
            (Target::V551, WriteTarget::V551, "write-551"),
            (Target::V70, WriteTarget::V70, "write-70"),
        ] {
            n += 1;
            match adapter::write(&model, wt) {
                Ok(out) => {
                    for i in checker::check(&out, t, None) {
                        failures.push(Failure::new(
                            id(label),
                            format!("output:{}", i.rule),
                            "NONCONFORMANT",
                            i.to_string(),
                        ));
                    }
                    if let Err(e) = adapter::read(out.as_bytes()) {
                        failures.push(Failure::new(id(label), "reparse", class_of(&e), e));
                    }
                }
                Err(e) => failures.push(Failure::new(id(label), "write", class_of(&e), e)),
            }
        }
        // Streaming parser: the same records as the in-memory parse.
        n += 1;
        match adapter::read_streaming(&bytes) {
            Ok(records) => {
                // The header is a record of the stream, not of the data set.
                let want = adapter::record_count(&model) + 1;
                if records.len() != want {
                    failures.push(Failure::new(
                        id("streaming"),
                        "records",
                        "LOST",
                        format!("{} records streamed, {want} in memory", records.len()),
                    ));
                }
            }
            Err(e) => failures.push(Failure::new(id("streaming"), "parse", class_of(&e), e)),
        }
        // Indexed lookups: every record xref of the source resolves.
        n += 1;
        let text = tree::decode_bytes(&bytes);
        let (_, recs) = tree::parse(&text);
        let indexable = ["INDI", "FAM", "SOUR", "REPO", "OBJE", "SUBM"];
        let xrefs: Vec<&str> = recs
            .iter()
            .filter(|r| indexable.contains(&r.tag.as_str()))
            .filter_map(|r| r.xref.as_deref())
            .collect();
        let model2 = adapter::read(&bytes).expect("read twice");
        let found = adapter::indexed_find(model2, &xrefs);
        let missing: Vec<&str> = xrefs
            .iter()
            .zip(&found)
            .filter(|(_, f)| f.is_none())
            .map(|(x, _)| *x)
            .collect();
        if !missing.is_empty() {
            failures.push(Failure::new(
                id("indexed"),
                "lookup",
                "LOST",
                missing.join(" "),
            ));
        }
        json_feature(&id("json"), &model, &mut failures, &mut n);
        gedzip_feature(&id("gedzip"), &model, &mut failures, &mut n);
    }
    let mut skipped = Vec::new();
    if cfg!(not(feature = "serde")) {
        skipped.push("feature/json/");
    }
    if cfg!(not(feature = "gedzip")) {
        skipped.push("feature/gedzip/");
    }
    ratchet::verify_skipping("feature", n, failures, &skipped);
}

#[cfg(feature = "serde")]
fn json_feature(id: &str, model: &adapter::Model, failures: &mut Vec<Failure>, n: &mut usize) {
    *n += 1;
    match adapter::json_round_trip(model) {
        Ok((_, true)) => {}
        Ok((_, false)) => failures.push(Failure::new(
            id,
            "round-trip",
            "CHANGED",
            "JSON read back differs",
        )),
        Err(e) => failures.push(Failure::new(id, "serialise", class_of(&e), e)),
    }
}

#[cfg(not(feature = "serde"))]
fn json_feature(_: &str, _: &adapter::Model, _: &mut Vec<Failure>, _: &mut usize) {}

#[cfg(feature = "gedzip")]
fn gedzip_feature(id: &str, model: &adapter::Model, failures: &mut Vec<Failure>, n: &mut usize) {
    *n += 1;
    let media: [(&str, &[u8]); 2] = [
        ("media/photo.jpg", b"\xFF\xD8\xFF\xE0fictitious"),
        ("media/scan.png", b"\x89PNG fictitious"),
    ];
    match adapter::gedzip_round_trip(model, &media) {
        Ok((back, names)) => {
            for (m, _) in media {
                if !names.iter().any(|x| x == m) {
                    failures.push(Failure::new(
                        id,
                        "media",
                        "LOST",
                        format!("{m} not in the archive"),
                    ));
                }
            }
            // An archive holds 7.x: a 5.5.1 dataset converted, records
            // added or left out as converting says.
            if adapter::record_count(&back) != adapter::record_count(&adapter::as_archived(model)) {
                failures.push(Failure::new(
                    id,
                    "records",
                    "LOST",
                    "record count differs after the archive",
                ));
            }
        }
        Err(e) => failures.push(Failure::new(id, "archive", class_of(&e), e)),
    }
}

#[cfg(not(feature = "gedzip"))]
fn gedzip_feature(_: &str, _: &adapter::Model, _: &mut Vec<Failure>, _: &mut usize) {}

/// Every encoding the documentation lists has cases in the `encoding` family.
#[test]
fn readme_encodings_are_covered() {
    let ids: Vec<String> = crate::encodings::encoding_cases()
        .into_iter()
        .map(|c| c.id)
        .collect();
    for (enc, prefix) in [
        ("UTF-8", "utf8-"),
        ("UTF-16", "utf16"),
        ("ISO-8859-1", "latin1"),
        ("ISO-8859-15", "latin9"),
        ("ANSEL", "ansel"),
    ] {
        assert!(
            ids.iter().any(|i| i.starts_with(prefix)),
            "documented encoding {enc} has no case"
        );
    }
}

/// A fictitious data set of about `people` individuals and half as many
/// families, in the given version.
pub fn synthetic(people: usize, v7: bool) -> String {
    let mut s = if v7 {
        String::from("0 HEAD\n1 GEDC\n2 VERS 7.0\n")
    } else {
        String::from(
            "0 HEAD\n1 SOUR EXAMPLE_APP\n1 SUBM @U1@\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n0 @U1@ SUBM\n1 NAME Example Submitter\n",
        )
    };
    for i in 0..people {
        let fam = i / 2;
        s += &format!(
            "0 @I{i}@ INDI\n1 NAME Person{i} /Sampleton/\n2 GIVN Person{i}\n2 SURN Sampleton\n1 SEX {}\n1 BIRT\n2 DATE {} JAN {}\n2 PLAC Sampleton, Example County\n1 NOTE Fictitious research note number {i}, long enough to look like a real note.\n1 FAMS @F{fam}@\n",
            if i % 2 == 0 { "M" } else { "F" },
            i % 28 + 1,
            1800 + i % 200
        );
        if i % 2 == 1 {
            s += &format!(
                "0 @F{fam}@ FAM\n1 HUSB @I{}@\n1 WIFE @I{i}@\n1 MARR\n2 DATE {}\n",
                i - 1,
                1820 + i % 200
            );
        }
    }
    s + "0 TRLR\n"
}

/// Opt-in performance smoke test (the documentation's performance figures): a 5 MB fictitious file is
/// parsed, written and streamed within generous bounds. Run it in release:
/// `cargo test --release --all-features --test conformance -- --ignored performance`.
#[test]
#[ignore = "opt-in: performance smoke test, run with --release"]
fn performance_smoke() {
    for v7 in [false, true] {
        let text = synthetic(20_000, v7);
        let mb = text.len() as f64 / 1_000_000.0;
        let t = std::time::Instant::now();
        let m = adapter::read(text.as_bytes()).expect("parse");
        let parse = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let out = adapter::write(&m, WriteTarget::Same).expect("write");
        let write = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let n = adapter::read_streaming(text.as_bytes())
            .expect("stream")
            .len();
        let stream = t.elapsed().as_secs_f64();
        eprintln!(
            "{} {mb:.1} MB: parse {:.1} MB/s, write {:.1} MB/s, stream {:.1} MB/s ({n} records)",
            if v7 { "7.0" } else { "5.5.1" },
            mb / parse,
            out.len() as f64 / 1e6 / write,
            mb / stream
        );
        assert!(mb / parse > 2.0 && mb / stream > 2.0, "slower than 2 MB/s");
    }
}

/// Reference validation (the documentation's integrity checks) accepts `@VOID@`
/// and reports a dangling pointer.
#[test]
fn reference_validation() {
    let mut failures = Vec::new();
    let all = crate::cases::all_cases();
    let mut n = 0;
    for id in [
        "G7-VOIDPTR",
        "G7-PHRASE-PTRS",
        "G7-FORWARD-PTR",
        "G5-ORDER-INDEPENDENT",
    ] {
        let c = all.iter().find(|c| c.id == id).expect("case");
        n += 1;
        if let Err(e) = adapter::read_checking_references(&c.input) {
            failures.push(Failure::new(
                format!("references/{id}"),
                "validate",
                class_of(&e),
                e,
            ));
        }
    }
    let dangling = all.iter().find(|c| c.id == "G7-DANGLING").expect("case");
    n += 1;
    if adapter::read_checking_references(&dangling.input).is_ok() {
        failures.push(Failure::new(
            "references/G7-DANGLING",
            "validate",
            "ACCEPTED",
            "a dangling pointer is not reported",
        ));
    }
    ratchet::verify("references", n, failures);
}
