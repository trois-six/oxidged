//! The dataset in JSON (feature `serde`): its shape, locked by a snapshot,
//! and every fixture read back from its JSON with the same structures.
#![cfg(feature = "serde")]

use std::path::{Path, PathBuf};

use oxidged::GedcomStreamParser;
use oxidged::model::{Dataset, RecordRef};

/// Every `.ged` file under `dir`.
fn ged_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            ged_files(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ged")) {
            out.push(p);
        }
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The JSON of `tests/fixtures/json/dataset.ged` is the snapshot
/// `dataset.json` (`JSON_SNAPSHOT_WRITE=1` rewrites it): a change of the
/// model's JSON shape shows here.
#[test]
fn json_shape_is_the_snapshot() {
    let dir = fixtures().join("json");
    let data = Dataset::from_bytes(std::fs::read(dir.join("dataset.ged")).unwrap());
    let json = serde_json::to_string_pretty(&data).unwrap() + "\n";
    let snapshot = dir.join("dataset.json");
    if std::env::var_os("JSON_SNAPSHOT_WRITE").is_some() {
        std::fs::write(&snapshot, &json).unwrap();
    }
    let expected = std::fs::read_to_string(&snapshot).unwrap();
    assert!(json == expected, "the JSON shape changed:\n{json}");
}

/// The snapshot reads back as the dataset it was written from.
#[test]
fn the_snapshot_reads_back() {
    let dir = fixtures().join("json");
    let data = Dataset::from_bytes(std::fs::read(dir.join("dataset.ged")).unwrap());
    let back: Dataset =
        serde_json::from_str(&std::fs::read_to_string(dir.join("dataset.json")).unwrap()).unwrap();
    assert_eq!(back.to_structures(), data.to_structures());
    assert_eq!(back.version(), data.version());
    assert_eq!(back.declared_version(), Some("7.0"));
    let ann = back.find_individual("@I1@").unwrap().into_owned();
    assert_eq!(ann.full_name(&back).as_deref(), Some("Ann Marie Example"));
    assert_eq!(back.families_as_spouse(ann.xref).count(), 1);
    assert!(back.dangling_references().is_empty());
}

/// Every fixture's dataset reads back from its JSON with the same
/// structures, and writes the same file.
#[test]
fn every_fixture_round_trips_through_json() {
    let mut files = Vec::new();
    ged_files(&fixtures(), &mut files);
    assert!(files.len() > 50);
    for path in files {
        let data = Dataset::from_bytes(std::fs::read(&path).unwrap());
        let json = serde_json::to_string(&data).unwrap();
        let back: Dataset =
            serde_json::from_str(&json).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(
            back.to_structures() == data.to_structures(),
            "{}",
            path.display()
        );
        let writer = oxidged::GedcomWriter::new();
        assert_eq!(
            writer.write_to_string(&back).unwrap(),
            writer.write_to_string(&data).unwrap(),
            "{}",
            path.display()
        );
    }
}

/// A streamed record serializes as its record, its version and its line.
#[test]
fn streamed_records_serialize() {
    let text = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 SEX f\n0 TRLR\n";
    let records: Vec<_> = GedcomStreamParser::new(text.as_bytes())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(matches!(records[1].record(), RecordRef::Individual(_)));
    assert_eq!(
        serde_json::to_string(&records[1]).unwrap(),
        r#"{"version":"5.5.1","line":4,"record":{"Individual":{"xref":"@I1@","names":[{"value":"Ann /Example/"}],"sex":"Female"}}}"#
    );
}

/// The largest number of arrays and maps open at once in `json`.
fn nesting(json: &str) -> usize {
    let (mut depth, mut max, mut string, mut escaped) = (0_usize, 0, false, false);
    for c in json.chars() {
        match c {
            _ if escaped => escaped = false,
            '\\' if string => escaped = true,
            '"' => string = !string,
            '{' | '[' if !string => {
                depth += 1;
                max = max.max(depth);
            }
            '}' | ']' if !string => depth -= 1,
            _ => {}
        }
    }
    max
}

/// Structures nested as deep as a file can nest them (255 levels): notes
/// and citations holding each other and extension structures. Their JSON
/// nests a few levels only, so that it reads back within the nesting limit
/// of `serde_json` (128), with the same structures.
#[test]
fn deep_structures_read_back_within_the_nesting_limit() {
    let mut text = String::from("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @S1@ SOUR\n0 @I1@ INDI\n");
    for level in 1..255 {
        let line = if level % 2 == 1 {
            "NOTE Copied"
        } else {
            "SOUR @S1@"
        };
        text.push_str(&format!("{level} {line}\n"));
    }
    text.push_str("1 BIRT\n2 SOUR @S1@\n3 DATA\n4 TEXT Entry\n5 _X\n");
    for level in 6..255 {
        text.push_str(&format!("{level} _X {level}\n"));
    }
    text.push_str("0 _REC\n");
    for level in 1..255 {
        text.push_str(&format!("{level} _Y\n"));
    }
    text.push_str("0 TRLR\n");
    let data = Dataset::parse(text.as_str());
    let json = serde_json::to_string(&data).unwrap();
    assert!(nesting(&json) < 16, "{}", nesting(&json));
    let back: Dataset = serde_json::from_str(&json).unwrap();
    assert!(back.to_structures() == data.to_structures());
    let note_record = back.find_individual("@I1@").unwrap().into_owned();
    let note = &note_record.notes[0];
    assert_eq!(note.detail().citations.len(), 1);
    // A list jumping a depth or a level is refused.
    for json in [
        r#"{"individuals":[{"notes":[{"content":{"Text":"a"}},[2,{}]]}]}"#,
        r#"{"individuals":[{"notes":[[1,{}]]}]}"#,
        r#"{"extra":[{"tag":"_X","descendants":[{"level":2,"tag":"_Y"}]}]}"#,
    ] {
        assert!(serde_json::from_str::<Dataset>(json).is_err(), "{json}");
    }
}

/// What JSON cannot hold for the dataset is refused, not guessed.
#[test]
fn malformed_json_is_an_error() {
    for json in [
        r#"{"version":"6.0"}"#,
        r#"{"individuals":[{"sex":"Robot"}]}"#,
        r#"{"individuals":[{"sex":{"Other":"x"}}]}"#,
        r#"{"individuals":{"xref":"@I1@"}}"#,
        r#"{"individuals":[{"names":[{"pieces":[{"kind":"Nickname","value":1}]}]}]}"#,
    ] {
        assert!(serde_json::from_str::<Dataset>(json).is_err(), "{json}");
    }
    // Fields it does not know are skipped.
    let data: Dataset = serde_json::from_str(r#"{"version":"7.0","comment":"x"}"#).unwrap();
    assert_eq!(data.version(), oxidged::GedcomVersion::V7_0);
}
