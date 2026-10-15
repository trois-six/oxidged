//! The streaming writer writes what the in-memory writer writes, record by
//! record, on every fixture where knowing the records to come changes
//! nothing; elsewhere, only in the ways its documentation lists. Its output
//! conforms, but for the pointers it cannot check.

use oxidged::model::{Header, Individual};
use oxidged::spec::{DeviationKind, validate_text};
use oxidged::{
    Bom, Dataset, GedcomStreamWriter, GedcomVersion, GedcomWriter, RepairKind, RepairPolicy,
    WriteReport, WriterConfig,
};
use std::path::{Path, PathBuf};

fn ged_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            ged_files(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ged")) {
            out.push(p);
        }
    }
}

/// `data` written by the streaming writer, its records in the order the
/// in-memory writer writes them.
fn streamed(data: &Dataset, config: WriterConfig) -> (String, WriteReport) {
    let mut w = GedcomStreamWriter::for_dataset(Vec::new(), config, data).unwrap();
    for r in data.records().skip(usize::from(data.header().is_some())) {
        w.write_record(r).unwrap();
    }
    let (bytes, report) = w.finish().unwrap();
    (String::from_utf8(bytes).unwrap(), report)
}

/// Why the outputs may differ, from the repairs: one of the cases the
/// streaming writer documents.
fn documented(in_memory: &WriteReport, stream: &WriteReport, stream_text: &str) -> bool {
    let renamed = |r: &WriteReport| r.repairs.iter().any(|r| r.kind == RepairKind::Xref);
    let pointer = in_memory
        .repairs
        .iter()
        .any(|r| r.kind == RepairKind::Pointer);
    let relocated = in_memory
        .repairs
        .iter()
        .any(|r| r.detail.contains("(declared as "));
    let large = in_memory
        .repairs
        .iter()
        .any(|r| r.kind == RepairKind::RecordSize);
    let stub = stream_text.contains("0 @U1@ SUBM\n1 NAME Unknown\n");
    renamed(in_memory) || renamed(stream) || pointer || relocated || large || stub
}

/// The records of a written file: the lines of each, from its level-0 line.
fn records(text: &str) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        match out.last_mut() {
            Some(r) if !line.starts_with("0 ") => r.push(line),
            _ => out.push(vec![line]),
        }
    }
    out
}

/// [`records`], sorted.
fn sorted_records(text: &str) -> Vec<Vec<&str>> {
    let mut r = records(text);
    r.sort();
    r
}

/// Whether `stream`, written converted to the other version family, is
/// `in_memory` but for what the streaming writer documents of a
/// conversion: the records a conversion adds come right after their
/// source record, and the header declares every extension tag of known
/// meaning (`HEAD.SCHMA`).
fn converted_alike(in_memory: &str, stream: &str) -> bool {
    let (mut a, mut b) = (records(in_memory), records(stream));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let (head_a, head_b) = (a.remove(0), b.remove(0));
    // The header's lines, in order, with declarations added.
    let mut rest = head_b.iter();
    let kept = head_a.iter().all(|l| {
        rest.by_ref()
            .find(|m| *m == l || !(m.starts_with("1 SCHMA") || m.starts_with("2 TAG ")))
            .is_some_and(|m| m == l)
    });
    let added = rest.all(|m| m.starts_with("1 SCHMA") || m.starts_with("2 TAG "));
    a.sort();
    b.sort();
    kept && added && a == b
}

#[test]
fn streaming_writes_as_the_in_memory_writer() {
    let mut files = Vec::new();
    ged_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        &mut files,
    );
    files.sort();
    let (mut same, mut converted, mut different) = (0, 0, 0);
    for path in files {
        let name = path.display();
        let data = Dataset::from_bytes(std::fs::read(&path).unwrap());
        for version in [
            GedcomVersion::V5_5_1,
            GedcomVersion::V7_0,
            GedcomVersion::V7_1,
        ] {
            let config = WriterConfig {
                version: Some(version),
                bom: Bom::Never,
                ..WriterConfig::default()
            };
            let mut bytes = Vec::new();
            let report = GedcomWriter::with_config(config.clone())
                .write(&mut bytes, &data)
                .unwrap();
            let in_memory = String::from_utf8(bytes).unwrap();
            let (stream, stream_report) = streamed(&data, config);
            if stream == in_memory {
                // The same repairs; renames come first in memory, where
                // the record is in a stream.
                let sorted = |r: &WriteReport| {
                    let mut d: Vec<String> = r.repairs.iter().map(ToString::to_string).collect();
                    d.sort();
                    d
                };
                assert_eq!(
                    sorted(&stream_report),
                    sorted(&report),
                    "{name} as {version}"
                );
                same += 1;
            } else if data.version().is_v7() != version.is_v7()
                && converted_alike(&in_memory, &stream)
            {
                converted += 1;
            } else {
                assert!(
                    documented(&report, &stream_report, &stream),
                    "{name} as {version}:\n{in_memory}\n---\n{stream}"
                );
                different += 1;
            }
            // Every line and structure conforms; a pointer may name no
            // record, or one of another type, and a 5.5.1 record without
            // notes to move may stay over 32K, as in memory.
            let left: Vec<_> = validate_text(&stream)
                .into_iter()
                .filter(|d| {
                    !matches!(
                        d.kind,
                        DeviationKind::PointerTarget
                            | DeviationKind::DanglingPointer
                            | DeviationKind::Xref
                            | DeviationKind::RecordSize
                    )
                })
                .collect();
            assert!(left.is_empty(), "{name} as {version}: {left:?}\n{stream}");
        }
    }
    eprintln!("{same} the same, {converted} converted alike, {different} documented");
    assert!(
        same + converted > 150 && different < 25,
        "{same} {converted} {different}"
    );
}

/// Under the strict policy, the first repair is an error, as soon as the
/// record that needs it is written.
#[test]
fn strict_streaming_fails_on_the_first_repair() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX M\n0 @I2@ INDI\n1 SEX X\n1 SEX F\n0 TRLR\n",
    );
    let config = WriterConfig {
        on_nonconformant: RepairPolicy::Error,
        ..WriterConfig::default()
    };
    let header = data.header().unwrap();
    let mut w = GedcomStreamWriter::new(Vec::new(), config, header, &data).unwrap();
    let mut records = data.records().skip(1);
    w.write_record(records.next().unwrap()).unwrap();
    assert!(w.write_record(records.next().unwrap()).is_err());
}

/// Identifiers stay unique and valid, record by record: a repeated one is
/// renamed, an invalid one written in its valid form, and pointers to it
/// follow; a record without one gets the first free one.
#[test]
fn identifiers_are_kept_unique_record_by_record() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @i1@ INDI\n1 FAMS @F1@\n0 @F1@ FAM\n1 HUSB @i1@\n0 @F1@ FAM\n0 INDI\n0 TRLR\n",
    );
    let (text, report) = streamed(&data, WriterConfig::default());
    assert!(text.contains("0 @I1@ INDI\n"), "{text}");
    assert!(text.contains("1 HUSB @I1@\n"), "{text}");
    assert!(text.contains("0 @F1_2@ FAM\n"), "{text}");
    assert!(text.contains("0 @I2@ INDI\n"), "{text}");
    assert_eq!(
        report
            .repairs
            .iter()
            .filter(|r| r.kind == RepairKind::Xref)
            .count(),
        2,
        "{report:?}"
    );
}

/// Streaming is idempotent: what it wrote, read back and streamed again,
/// is the same bytes.
#[test]
fn streaming_is_idempotent() {
    let mut files = Vec::new();
    ged_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        &mut files,
    );
    files.sort();
    for path in files {
        let name = path.display();
        let data = Dataset::from_bytes(std::fs::read(&path).unwrap());
        for version in [
            GedcomVersion::V5_5_1,
            GedcomVersion::V7_0,
            GedcomVersion::V7_1,
        ] {
            let config = WriterConfig {
                version: Some(version),
                bom: Bom::Never,
                ..WriterConfig::default()
            };
            let (once, report) = streamed(&data, config.clone());
            let (twice, _) = streamed(&Dataset::parse(once.as_str()), config);
            // Converted, a record a conversion adds comes right after its
            // source, and so do the notes moved out of a 5.5.1 record over
            // 32K, which a dataset read back puts with their type.
            let converted = data.version().is_v7() != version.is_v7();
            let moved = report
                .repairs
                .iter()
                .any(|r| r.kind == RepairKind::RecordSize);
            assert!(
                once == twice
                    || ((converted || moved) && sorted_records(&once) == sorted_records(&twice)),
                "{name} as {version}:\n{once}\n---\n{twice}"
            );
        }
    }
}

/// A 5.5.1 header whose `SUBM` the repair makes an extension still points
/// to a submitter, the stub (fuzz case).
#[test]
fn a_repaired_header_still_has_a_submitter() {
    let data = Dataset::from_bytes(&b"0 HEAD\n1 SUBM @\x001@\n0 TRLR\n"[..]);
    let config = WriterConfig {
        version: Some(GedcomVersion::V5_5_1),
        ..WriterConfig::default()
    };
    let (once, _) = streamed(&data, config.clone());
    assert!(once.contains("\n1 SUBM @U1@\n"), "{once}");
    assert!(once.contains("\n0 @U1@ SUBM\n1 NAME Unknown\n"), "{once}");
    let (twice, _) = streamed(&Dataset::parse(once.as_str()), config);
    assert_eq!(once, twice);
}

/// Written in the other version family, records are converted as the
/// in-memory writer converts them: a record a conversion adds comes right
/// after its source, and the header declares every extension tag of known
/// meaning.
#[test]
fn streaming_converts_to_the_other_family() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n1 SUBM @U1@\n\
         0 @U1@ SUBM\n1 NAME Ann\n\
         0 @I1@ INDI\n1 NAME Ann /Example/\n1 _UID 0123456789ABCDEF\n1 SOUR A parish register\n\
         1 OBJE\n2 FILE photo.jpg\n3 FORM jpg\n\
         0 @I2@ INDI\n1 NAME Bob /Example/\n1 ASSO @I1@\n2 RELA Godfather\n0 TRLR\n",
    );
    let config = WriterConfig {
        version: Some(GedcomVersion::V7_0),
        bom: Bom::Never,
        ..WriterConfig::default()
    };
    let (stream, _) = streamed(&data, config.clone());
    let in_memory = GedcomWriter::with_config(config)
        .write_to_string(&data)
        .unwrap();
    assert!(
        converted_alike(&in_memory, &stream),
        "{in_memory}\n---\n{stream}"
    );
    assert!(stream.contains("1 UID 0123456789ABCDEF\n"), "{stream}");
    assert!(
        stream.contains("2 ROLE GODP\n3 PHRASE Godfather\n"),
        "{stream}"
    );
    assert!(
        stream.contains("2 TAG _LOC http://genealogy.net/GEDCOM#_LOC\n"),
        "{stream}"
    );
    // The records taken out of @I1@ come right after it.
    let at = |s: &str| stream.find(s).unwrap();
    assert!(
        at("0 @I1@ INDI") < at(" SOUR\n1 NOTE A parish register"),
        "{stream}"
    );
    assert!(at(" OBJE\n1 FILE") < at("0 @I2@ INDI"), "{stream}");
    assert!(validate_text(&stream).iter().all(|d| matches!(
        d.kind,
        DeviationKind::PointerTarget | DeviationKind::DanglingPointer
    )));
}

/// A 5.5.1 submission kept as an extension record (`keep_obsolete`) is
/// written last, as in memory.
#[test]
fn a_record_a_conversion_makes_an_extension_record_is_written_last() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 SUBM @U1@\n1 SUBN @SN1@\n0 @U1@ SUBM\n1 NAME Ann\n\
         0 @SN1@ SUBN\n1 FAMF Family file\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n",
    );
    let config = WriterConfig {
        version: Some(GedcomVersion::V7_0),
        bom: Bom::Never,
        conversion: oxidged::convert::ConversionOptions::default().keep_obsolete(true),
        ..WriterConfig::default()
    };
    let (stream, _) = streamed(&data, config.clone());
    let in_memory = GedcomWriter::with_config(config)
        .write_to_string(&data)
        .unwrap();
    assert!(
        stream.ends_with("0 @SN1@ _SUBMISSION\n1 FAMF Family file\n0 TRLR\n"),
        "{stream}"
    );
    assert!(
        converted_alike(&in_memory, &stream),
        "{in_memory}\n---\n{stream}"
    );
}

/// The 7.x pointer rules between records (7.0.17) hold record by record:
/// the streaming writer cuts the pointer that closes a cycle, and an `ALIA`
/// to its own record, as the in-memory writer does.
#[test]
fn streaming_keeps_the_pointer_rules_between_records() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 ALIA @I1@\n\
         0 @S1@ SOUR\n1 TITL Register\n1 OBJE @O1@\n\
         0 @O1@ OBJE\n1 FILE scan.jpg\n2 FORM image/jpeg\n1 SOUR @S1@\n2 PAGE 4\n0 TRLR\n",
    );
    let (text, report) = streamed(&data, WriterConfig::default());
    let (in_memory, memory_report) = GedcomWriter::new()
        .write_to_string_with_report(&data)
        .unwrap();
    assert_eq!(text.trim_start_matches('\u{feff}'), in_memory);
    assert_eq!(report, memory_report);
    assert!(text.contains("0 @I1@ INDI\n1 _ALIA @I1@\n"), "{text}");
    assert!(text.contains("1 OBJE @O1@\n"), "{text}");
    assert!(text.contains("1 _SOUR @S1@\n2 PAGE 4\n"), "{text}");
    let cycles = report
        .repairs
        .iter()
        .filter(|r| r.kind == RepairKind::Cycle)
        .count();
    assert_eq!(cycles, 2, "{:?}", report.repairs);
    assert!(
        validate_text(&text).is_empty(),
        "{:?}",
        validate_text(&text)
    );
}

/// The streaming writer leaves credentials out too, unless asked for.
#[test]
fn credentials_are_left_out_by_default() {
    let file = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n\
        0 @S1@ SOUR\n1 TITL Example register\n1 _DBID example-db\n2 _USER reader\n\
        2 _PASS secret\n0 @M1@ OBJE\n1 FILE photo.jpg\n2 FORM jpg\n1 _USER ZXhhbXBsZQ==\n\
        2 _ENCR 1\n0 TRLR\n";
    let data = Dataset::parse(file);
    for version in [GedcomVersion::V5_5_1, GedcomVersion::V7_0] {
        let config = WriterConfig {
            version: Some(version),
            ..WriterConfig::default()
        };
        let (out, _) = streamed(&data, config.clone());
        assert!(
            !out.contains("secret") && !out.contains("ZXhh") && !out.contains("_ENCR"),
            "{out}"
        );
        assert!(
            out.contains("_DBID example-db\n") && out.contains("1 FILE photo.jpg\n"),
            "{out}"
        );
        let (all, _) = streamed(
            &data,
            WriterConfig {
                credentials: oxidged::Credentials::Written,
                ..config
            },
        );
        assert!(
            all.contains(" _PASS secret\n") && all.contains("_ENCR 1\n"),
            "{all}"
        );
    }
}

/// With no version configured, the streaming writer writes a dataset in its
/// own version, as the in-memory writer does: a dataset made in a version,
/// without a header or with one that declares none, and every fixture.
#[test]
fn streams_a_dataset_in_its_own_version_by_default() {
    let config = WriterConfig {
        bom: Bom::Never,
        ..WriterConfig::default()
    };
    let in_memory = |data: &Dataset, config: &WriterConfig| {
        let mut bytes = Vec::new();
        GedcomWriter::with_config(config.clone())
            .write(&mut bytes, data)
            .unwrap();
        String::from_utf8(bytes).unwrap()
    };
    for version in [
        GedcomVersion::V5_5_1,
        GedcomVersion::V7_0,
        GedcomVersion::V7_1,
    ] {
        let mut data = Dataset::new(version);
        data.add(Individual::default()).unwrap();
        let mut headed = data.clone();
        headed.set_header(Some(Header::default()));
        for data in [data, headed] {
            let (stream, _) = streamed(&data, config.clone());
            assert!(
                stream.starts_with(&format!("0 HEAD\n1 GEDC\n2 VERS {version}\n")),
                "{version}: {stream}"
            );
            assert_eq!(stream, in_memory(&data, &config), "{version}");
        }
    }

    let mut files = Vec::new();
    ged_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        &mut files,
    );
    for path in files {
        let data = Dataset::from_bytes(std::fs::read(&path).unwrap());
        let vers = format!("2 VERS {}", data.version());
        let (stream, _) = streamed(&data, config.clone());
        let written = in_memory(&data, &config);
        for text in [&stream, &written] {
            assert_eq!(
                text.lines().nth(2),
                Some(vers.as_str()),
                "{}",
                path.display()
            );
        }
    }
}

/// A configured version is written whatever the dataset's own, streamed
/// as in memory (but for the header a conversion to 7.x declares).
#[test]
fn streams_the_configured_version_over_the_datasets() {
    for (own, target) in [
        (GedcomVersion::V7_0, GedcomVersion::V5_5_1),
        (GedcomVersion::V5_5_1, GedcomVersion::V7_0),
        (GedcomVersion::V7_1, GedcomVersion::V7_0),
    ] {
        let mut data = Dataset::new(own);
        data.add(Individual::default()).unwrap();
        let config = WriterConfig {
            version: Some(target),
            bom: Bom::Never,
            ..WriterConfig::default()
        };
        let (stream, _) = streamed(&data, config.clone());
        assert!(
            stream.starts_with(&format!("0 HEAD\n1 GEDC\n2 VERS {target}\n")),
            "{own} as {target}: {stream}"
        );
        let mut bytes = Vec::new();
        GedcomWriter::with_config(config)
            .write(&mut bytes, &data)
            .unwrap();
        let written = String::from_utf8(bytes).unwrap();
        // Converted to 7.x, the streamed header declares every extension
        // tag of known meaning; the records are the same.
        let records = |t: &str| t[t.find("0 @I1@").unwrap()..].to_owned();
        assert_eq!(records(&stream), records(&written), "{own} as {target}");
    }
}

/// A stream read by the parser and started with its header's dataset is
/// written as [`GedcomStreamWriter::new`] writes it with that header.
#[test]
fn streams_a_parsed_stream_from_its_headers_dataset() {
    let file: &[u8] = b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n";
    let write = |by_dataset: bool| {
        let mut records = oxidged::GedcomStreamParser::new(file).unwrap();
        let head = records.next().unwrap().unwrap();
        let mut w = if by_dataset {
            GedcomStreamWriter::for_dataset(Vec::new(), WriterConfig::default(), head.dataset())
        } else {
            let header = head.dataset().header().unwrap();
            GedcomStreamWriter::new(Vec::new(), WriterConfig::default(), header, &head)
        }
        .unwrap();
        for record in records {
            w.write_record(record.unwrap().record()).unwrap();
        }
        String::from_utf8(w.finish().unwrap().0).unwrap()
    };
    let text = write(true);
    assert!(text.contains("2 VERS 7.0\n"), "{text}");
    assert!(
        text.contains("0 @I1@ INDI\n1 NAME Ann /Example/\n"),
        "{text}"
    );
    assert_eq!(text, write(false));
}
