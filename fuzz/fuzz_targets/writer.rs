//! Writer output re-parses and follows the line grammar: for any input,
//! the typed model and the lossless tree read from it are written in each
//! version without panicking, every written line passes the line rules of
//! the version, the output reads back with the same records, and writing
//! it again gives the same bytes: write(read(write(x))) == write(x). The
//! same holds of the model's records written one at a time
//! (`GedcomStreamWriter`).
#![no_main]

use libfuzzer_sys::fuzz_target;
use oxidged::model::Header;
use oxidged::tree::{Tree, parse_tree};
use oxidged::{
    Dataset, GedcomStreamWriter, GedcomVersion, GedcomWriter, OutputEncoding, WriterConfig,
};
use std::collections::HashSet;

const VERSIONS: [GedcomVersion; 3] = [
    GedcomVersion::V5_5_1,
    GedcomVersion::V7_0,
    GedcomVersion::V7_1,
];

/// Asserts the line rules the emitter guarantees.
fn check_lines(out: &str, version: GedcomVersion) {
    let rules = version.rules();
    assert!(out.ends_with('\n'), "no final terminator");
    let mut previous: Option<usize> = None;
    let mut xrefs = HashSet::new();
    for line in out.split_terminator('\n') {
        assert!(!line.contains('\r'), "CR inside a line: {line:?}");
        assert!(
            !line.chars().any(|c| rules.is_banned(c)),
            "banned: {line:?}"
        );
        if let Some(max) = rules.max_line_length() {
            assert!(line.len() < max, "too long: {} {line:?}", line.len());
        }
        let mut parts = line.splitn(2, ' ');
        let level: usize = parts.next().unwrap().parse().expect("a level");
        let rest = parts.next().expect("a tag");
        match previous {
            None => assert_eq!(level, 0),
            Some(p) => assert!(level <= p + 1, "level jump: {line:?}"),
        }
        previous = Some(level);
        let (xref, rest) = match rest.strip_prefix('@') {
            Some(r) => {
                let end = r.find("@ ").expect("an xref") + 1;
                (Some(&rest[..=end]), &r[end + 1..])
            }
            None => (None, rest),
        };
        if let Some(x) = xref {
            assert_eq!(level, 0, "xref on a substructure: {line:?}");
            assert!(rules.is_valid_xref(x), "xref: {line:?}");
            assert!(xrefs.insert(x.to_string()), "duplicate xref: {line:?}");
        }
        let (tag, payload) = match rest.split_once(' ') {
            Some((t, p)) => (t, Some(p)),
            None => (rest, None),
        };
        assert!(rules.is_valid_tag(tag), "tag: {line:?}");
        assert!(!(tag == "CONC" && !rules.uses_conc()), "CONC: {line:?}");
        if let Some(p) = payload {
            assert!(!p.is_empty(), "trailing delimiter: {line:?}");
            if !rules.doubles_every_at_sign() && p.starts_with('@') && !p.starts_with("@@") {
                assert!(
                    p.ends_with('@') && !p[1..p.len() - 1].contains('@'),
                    "escape: {line:?}"
                );
            }
        }
    }
    assert!(out.ends_with("0 TRLR\n"), "no trailer");
}

/// `data`'s records written one at a time, in the order the in-memory
/// writer writes them, and whether notes were moved out of a 5.5.1 record
/// over 32K.
fn streamed(data: &Dataset, version: GedcomVersion) -> (String, bool) {
    let config = WriterConfig {
        version: Some(version),
        bom: oxidged::Bom::Never,
        ..WriterConfig::default()
    };
    let default = Header::default();
    let header = data.header().unwrap_or(&default);
    let mut w = GedcomStreamWriter::new(Vec::new(), config, header, data).expect("repairs");
    for r in data.records().skip(usize::from(data.header().is_some())) {
        w.write_record(r).expect("repairs, never fails");
    }
    let (bytes, report) = w.finish().expect("writes");
    let moved = report
        .repairs
        .iter()
        .any(|r| r.kind == oxidged::RepairKind::RecordSize);
    (String::from_utf8(bytes).expect("UTF-8"), moved)
}

/// The records of `out` (the lines of each), sorted.
fn sorted_records(out: &str) -> Vec<Vec<&str>> {
    let mut records: Vec<Vec<&str>> = Vec::new();
    for line in out.lines() {
        match records.last_mut() {
            Some(r) if !line.starts_with("0 ") => r.push(line),
            _ => records.push(vec![line]),
        }
    }
    records.sort();
    records
}

/// The number of level-0 lines.
fn records(out: &str) -> usize {
    out.split_terminator('\n')
        .filter(|l| l.starts_with("0 "))
        .count()
}

fuzz_target!(|data: &[u8]| {
    // The typed model.
    {
        let model = Dataset::from_bytes(data);
        for version in VERSIONS {
            let writer = GedcomWriter::new().gedcom_version(version);
            let out = writer
                .write_to_string(&model)
                .expect("repairs, never fails");
            check_lines(&out, version);
            let back = Dataset::parse(out.as_str());
            // Every record is written (a header is synthesised when there is
            // none), but a trailer that holds nothing but an identifier,
            // which is left out (it ends the file); 5.5.1 adds a submitter
            // record when there is none, and the conformance repair moves
            // notes out of records over 32K.
            let bare_trailers = model
                .extra()
                .iter()
                .filter(|n| {
                    n.tag() == "TRLR"
                        && n.children().is_empty()
                        && matches!(n.payload(), oxidged::model::Value::None)
                })
                .count();
            let (written, read) = (back.record_count(), model.record_count() - bare_trailers);
            let header = usize::from(model.header().is_none());
            // Converted to the other version family, records are added
            // (multimedia and source records taken out of others) and 5.5.1
            // submissions left out.
            let converted = model.version().is_v7() != version.is_v7();
            let submissions = if converted {
                model.submissions().len()
            } else {
                0
            };
            assert!(
                written == read + header
                    || (version.rules().max_line_length().is_some() && written > read)
                    || (converted && written + submissions >= read),
                "{version}: {written} records read back for {read}\n{out}"
            );
            // Writing is idempotent: write(read(write(x))) == write(x).
            let again = writer.write_to_string(&back).expect("repairs, never fails");
            assert!(again == out, "{version}: not stable\n{out}\n{again}");
            // One record at a time: conformant lines, and idempotent.
            let (once, moved) = streamed(&model, version);
            check_lines(&once, version);
            let (twice, _) = streamed(&Dataset::parse(once.as_str()), version);
            // Converted, a record a conversion adds comes right after its
            // source, and so do the notes moved out of a 5.5.1 record over
            // 32K, which a dataset read back puts with their type.
            assert!(
                twice == once
                    || ((converted || moved) && sorted_records(&twice) == sorted_records(&once)),
                "{version}: streaming not stable\n{once}\n{twice}"
            );
            for encoding in [
                OutputEncoding::Ansel,
                OutputEncoding::Ascii,
                OutputEncoding::Utf16Le,
            ] {
                let _ = writer
                    .clone()
                    .output_encoding(encoding)
                    .write(std::io::sink(), &model);
            }
        }
    }

    // The lossless tree.
    let tree = Tree::from_bytes(data);
    let mut kept = 0;
    let mut head = false;
    for record in tree.records() {
        let empty_trailer = record.tag() == "TRLR"
            && record.payload() == oxidged::tree::PayloadRef::None
            && record.substructures().next().is_none();
        if record.tag() == "HEAD" && !head {
            head = true;
        } else if !empty_trailer {
            kept += 1;
        }
    }
    for version in VERSIONS {
        let mut bytes = Vec::new();
        GedcomWriter::new()
            .gedcom_version(version)
            .bom(oxidged::Bom::Never)
            .write_tree(&mut bytes, &tree)
            .expect("repairs, never fails");
        let out = String::from_utf8(bytes).expect("UTF-8");
        check_lines(&out, version);
        // But for HEAD and TRLR. 5.5.1 adds a submitter record when there is
        // none, and the conformance repair moves notes out of records over
        // 32K; converted to the other version family, records are added and
        // 5.5.1 submissions left out.
        let written = records(&out) - 2;
        let converted = tree.version().is_v7() != version.is_v7();
        let submissions = tree.records().filter(|r| r.tag() == "SUBN").count();
        assert!(
            written == kept
                || (version.rules().max_line_length().is_some() && written > kept)
                || (converted && written + submissions >= kept),
            "{written} records written for {kept}"
        );
        assert_eq!(parse_tree(&out).records().count(), records(&out));
        // Writing is idempotent: write(read(write(x))) == write(x).
        let mut again = Vec::new();
        GedcomWriter::new()
            .gedcom_version(version)
            .bom(oxidged::Bom::Never)
            .write_tree(&mut again, &Tree::from_bytes(out.as_bytes()))
            .expect("repairs, never fails");
        assert!(again == out.as_bytes(), "{version}: not stable\n{out}");
    }
});
