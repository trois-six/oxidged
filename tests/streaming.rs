//! Streaming reads exactly what in-memory reading reads.
//!
//! [`GedcomStreamParser`] reads any encoding and line terminator one record
//! at a time; the dataset its records collect into equals the one
//! [`Dataset::parse`] reads, and its lossless structures
//! ([`GedcomStreamParser::nodes`], [`TreeReader`]) equal those of
//! [`Tree::from_bytes`], line numbers included. Every fixture under
//! `tests/fixtures` is compared with the normal suite; external corpora on
//! request: `GED_IO_CORPORA=dir1:dir2 cargo test --test streaming --
//! --ignored`. Every name and value is fictitious.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use oxidged::encoding::decode;
use oxidged::model::{Dataset, RecordRef};
use oxidged::tree::{Structure, Tree, TreeReader, parse_tree};
use oxidged::{GedcomEncoding, GedcomError, GedcomStreamParser, GedcomVersion, StreamedRecord};

const FILE: &str = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n\
    0 @I1@ INDI\n1 NAME Zoë /Example/\n1 NOTE a@@b\n2 CONC c\n1 _X y\n1 FAMS @F1@\n\
    0 @F1@ FAM\n1 HUSB @I1@\n\
    0 @N1@ NOTE Shared\n1 CONT text\n\
    0 _LOC Sampleton\n\
    0 TRLR\n";

fn stream(reader: impl BufRead) -> Vec<StreamedRecord> {
    GedcomStreamParser::new(reader)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let units = text.encode_utf16();
    let mut out = if little_endian {
        vec![0xFF, 0xFE]
    } else {
        vec![0xFE, 0xFF]
    };
    for u in units {
        out.extend(if little_endian {
            u.to_le_bytes()
        } else {
            u.to_be_bytes()
        });
    }
    out
}

fn lines(s: &Structure, out: &mut Vec<u32>) {
    out.push(s.line);
    for c in &s.substructures {
        lines(c, out);
    }
}

/// Structures and their line numbers equal.
fn same_structures(a: &[Structure], b: &[Structure]) -> Result<(), String> {
    if a.len() != b.len() {
        return Err(format!("{} records against {}", a.len(), b.len()));
    }
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if x != y {
            return Err(format!("record {i} differs:\n{x:?}\n{y:?}"));
        }
        let (mut lx, mut ly) = (Vec::new(), Vec::new());
        lines(x, &mut lx);
        lines(y, &mut ly);
        if lx != ly {
            return Err(format!("record {i}: line numbers differ"));
        }
    }
    Ok(())
}

/// Compares streaming and in-memory reading of one input; returns a
/// description of the first difference.
fn compare(bytes: &[u8]) -> Result<(), String> {
    let in_memory = Dataset::parse(decode(bytes).text);
    let expected = in_memory.to_structures();
    let tree = Tree::from_bytes(bytes);
    let tree_records = tree.to_structures();
    for capacity in [64 * 1024, 97, 61] {
        let reader = || BufReader::with_capacity(capacity, bytes);

        // The typed model.
        let parser = GedcomStreamParser::new(reader()).map_err(|e| e.to_string())?;
        let streamed: Vec<StreamedRecord> = parser
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let collected: Dataset = streamed.into_iter().collect();
        if collected.to_structures() != expected {
            return Err(format!(
                "model ({capacity}): streamed and in-memory datasets differ"
            ));
        }
        if collected.version() != in_memory.version()
            || collected.declared_version() != in_memory.declared_version()
        {
            return Err(format!("model ({capacity}): versions differ"));
        }

        // The lossless structures, through the parser and the tree reader.
        let nodes: Vec<Structure> = GedcomStreamParser::new(reader())
            .map_err(|e| e.to_string())?
            .nodes()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        same_structures(&nodes, &tree_records).map_err(|e| format!("nodes ({capacity}): {e}"))?;
        let read: Vec<Structure> = TreeReader::new(reader())
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        same_structures(&read, &tree_records).map_err(|e| format!("tree ({capacity}): {e}"))?;
    }
    if parse_tree(&tree.to_gedcom()).to_structures() != tree_records {
        return Err("tree: the dump does not read back to the same tree".into());
    }
    Ok(())
}

fn ged_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            ged_files(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ged"))
        {
            out.push(path);
        }
    }
}

fn run(dirs: &[PathBuf]) -> usize {
    let mut files = Vec::new();
    for dir in dirs {
        ged_files(dir, &mut files);
    }
    files.sort();
    let mut failures = Vec::new();
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        if let Err(e) = compare(&bytes) {
            failures.push(format!("{}: {e}", file.display()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    files.len()
}

#[test]
fn every_fixture_streams_to_what_is_read_in_memory() {
    let n = run(&[Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")]);
    assert!(n >= 50, "{n} fixtures");
}

#[test]
#[ignore = "reads the external corpora listed in GED_IO_CORPORA"]
fn corpora_stream_to_what_is_read_in_memory() {
    let dirs: Vec<PathBuf> = std::env::var("GED_IO_CORPORA")
        .expect("GED_IO_CORPORA: colon-separated corpus directories")
        .split(':')
        .map(PathBuf::from)
        .collect();
    let n = run(&dirs);
    eprintln!("{n} files compared");
}

#[test]
fn records_are_typed_one_at_a_time() {
    let records = stream(FILE.as_bytes());
    assert_eq!(records.len(), 5); // the trailer is not a record
    assert!(matches!(records[0].record(), RecordRef::Header(_)));
    assert_eq!(records[0].line(), 1);
    assert!(records.iter().all(|r| r.version() == GedcomVersion::V5_5_1));

    let indi = &records[1];
    assert_eq!(indi.line(), 6);
    let RecordRef::Individual(person) = indi.record() else {
        panic!("not an individual")
    };
    assert_eq!(person.full_name().as_deref(), Some("Zoë Example"));
    assert_eq!(person.extra().first().unwrap().tag(), "_X");
    assert_eq!(indi.store().xref(person.xref().unwrap()), "@I1@");

    let RecordRef::Note(note) = records[3].record() else {
        panic!("not a note")
    };
    assert_eq!(note.text().to_str(), "Shared\ntext");
    assert!(matches!(records[4].record(), RecordRef::Other(_)));

    assert_eq!(records[2].record().tag(GedcomVersion::V5_5_1), "FAM");
    let (record, store) = records[2].clone().into_parts();
    assert!(matches!(record, oxidged::model::Record::Family(_)));
    assert!(store.find_xref("@F1@").is_some());
}

#[test]
fn any_encoding_and_line_terminator() {
    let expected = Dataset::parse(FILE).to_structures();
    let inputs: [(Vec<u8>, GedcomEncoding); 6] = [
        (FILE.as_bytes().to_vec(), GedcomEncoding::Utf8),
        (format!("\u{feff}{FILE}").into_bytes(), GedcomEncoding::Utf8),
        (
            FILE.replace('\n', "\r\n").into_bytes(),
            GedcomEncoding::Utf8,
        ),
        (FILE.replace('\n', "\r").into_bytes(), GedcomEncoding::Utf8),
        (
            utf16(&FILE.replace('\n', "\r"), true),
            GedcomEncoding::Utf16Le,
        ),
        (
            utf16(&FILE.replace('\n', "\r\n"), false),
            GedcomEncoding::Utf16Be,
        ),
    ];
    for (bytes, encoding) in inputs {
        let parser =
            GedcomStreamParser::new(BufReader::with_capacity(16, bytes.as_slice())).unwrap();
        assert_eq!(parser.encoding(), encoding);
        let collected: Dataset = parser.map(Result::unwrap).collect();
        assert_eq!(collected.to_structures(), expected, "{encoding:?}");
        let zoe = collected.find_individual("@I1@").unwrap().into_owned();
        assert_eq!(zoe.full_name(&collected).as_deref(), Some("Zoë Example"));
    }
}

#[test]
fn the_version_is_known_once_the_header_is_read() {
    let text = "0 HEAD\n1 GEDC\n2 VERS 7.0.3\n0 @I1@ INDI\n1 NOTE @@a@@b\n0 TRLR\n";
    let mut parser = GedcomStreamParser::new(text.as_bytes()).unwrap();
    let header = parser.next().unwrap().unwrap();
    assert!(matches!(header.record(), RecordRef::Header(_)));
    assert_eq!(parser.version(), GedcomVersion::V7_0);
    assert_eq!(parser.declared_version(), Some("7.0.3"));
    // The 7.0 escaping rules apply to the records that follow.
    let indi = parser.next().unwrap().unwrap();
    let RecordRef::Individual(person) = indi.record() else {
        panic!("not an individual")
    };
    let oxidged::model::NoteContent::Text(note) = &person.notes().first().unwrap().content() else {
        panic!("not a note text")
    };
    assert_eq!(note.to_str(&indi), "@a@@b");
    assert!(parser.next().is_none());
}

#[test]
fn a_filtered_stream_collects_into_a_smaller_dataset() {
    let data: Dataset = GedcomStreamParser::new(FILE.as_bytes())
        .unwrap()
        .map(Result::unwrap)
        .filter(|r| matches!(r.record(), RecordRef::Header(_) | RecordRef::Individual(_)))
        .collect();
    assert!(data.header().is_some());
    assert_eq!(data.individuals().len(), 1);
    assert!(data.families().is_empty() && data.notes().is_empty() && data.extra().is_empty());
    assert_eq!(data.declared_version(), Some("5.5.1"));
    // The individual's pointer to the family left out now dangles.
    let dangling = data.dangling_references();
    assert_eq!(dangling.len(), 1);
    assert_eq!(data.store().xref(dangling[0].pointer), "@F1@");
}

#[test]
fn extending_a_dataset_keeps_a_second_header_aside() {
    let mut data: Dataset = stream(FILE.as_bytes()).into_iter().collect();
    let other =
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 LANG English\n0 @I2@ INDI\n1 NAME Ann /Sample/\n0 TRLR\n";
    data.extend(stream(other.as_bytes()));
    assert_eq!(data.individuals().len(), 2);
    let ann = data.find_individual("@I2@").unwrap().into_owned();
    assert_eq!(ann.full_name(&data).as_deref(), Some("Ann Sample"));
    // The first header stays the header; the second is an untyped record.
    assert!(data.header().unwrap().language.is_none());
    let tags: Vec<_> = data.extra().iter().map(|n| n.tag()).collect();
    assert_eq!(tags, ["_LOC", "HEAD"]);
}

#[test]
fn nodes_continue_after_typed_records() {
    let mut parser = GedcomStreamParser::new(FILE.as_bytes()).unwrap();
    assert!(parser.next().is_some());
    let rest: Vec<_> = parser.nodes().collect::<Result<_, _>>().unwrap();
    let tags: Vec<_> = rest.iter().map(|s| s.tag.as_str().to_string()).collect();
    assert_eq!(tags, ["INDI", "FAM", "NOTE", "_LOC", "TRLR"]);
    assert_eq!(rest[0].line, 6);
    assert_eq!(rest[0].first("NOTE").unwrap().text(), Some("a@bc"));
    assert_eq!(rest[2].text(), Some("Shared\ntext"));
}

#[test]
fn empty_input_and_a_lone_trailer_have_no_records() {
    assert!(stream(&b""[..]).is_empty());
    assert!(stream(&b"0 TRLR\n"[..]).is_empty());
    let empty: Dataset = stream(&b""[..]).into_iter().collect();
    assert_eq!(empty.record_count(), 0);
}

/// Yields `bytes`, then fails.
struct FailingAfter<'a> {
    bytes: &'a [u8],
}

impl Read for FailingAfter<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.bytes.is_empty() {
            return Err(std::io::Error::other("broken"));
        }
        self.bytes.read(buf)
    }
}

#[test]
fn an_io_error_at_the_start_fails_the_parser() {
    let parser = GedcomStreamParser::new(BufReader::new(FailingAfter { bytes: b"" }));
    assert!(matches!(parser, Err(GedcomError::Io(_))));
    assert!(TreeReader::new(BufReader::new(FailingAfter { bytes: b"" })).is_err());
}

#[test]
fn an_io_error_mid_stream_ends_it() {
    // More than the 64 KiB read at the start: the error comes while
    // records are being read.
    let mut text = String::from("0 HEAD\n1 GEDC\n2 VERS 7.0\n");
    for i in 0..4000 {
        text.push_str(&format!("0 @I{i}@ INDI\n1 NAME Ann /Example/\n"));
    }
    let mut parser = GedcomStreamParser::new(BufReader::new(FailingAfter {
        bytes: text.as_bytes(),
    }))
    .unwrap();
    let mut records = 0;
    let error = loop {
        match parser.next() {
            Some(Ok(_)) => records += 1,
            Some(Err(e)) => break e,
            None => panic!("the stream ended without the error"),
        }
    };
    assert!(matches!(error, GedcomError::Io(_)));
    assert!(records > 1);
    assert!(parser.next().is_none());
}

/// A visitor sees every record, by its type, in file order; it can stop,
/// and the parser goes on from there.
#[test]
fn visitors_see_records_by_type_and_can_stop() {
    use oxidged::model::{Family, HeaderRef, Individual, Node, View};
    use oxidged::stream::{StreamedRecord, Visitor};
    use std::ops::ControlFlow;

    #[derive(Default)]
    struct Counter {
        seen: Vec<String>,
        stop_at: Option<&'static str>,
    }
    impl Visitor for Counter {
        type Break = usize;
        fn header(&mut self, _: HeaderRef<'_>, _: &StreamedRecord) -> ControlFlow<usize> {
            self.seen.push("HEAD".into());
            ControlFlow::Continue(())
        }
        fn individual(
            &mut self,
            i: View<'_, Individual>,
            r: &StreamedRecord,
        ) -> ControlFlow<usize> {
            let x = r.store().xref(i.xref().unwrap()).to_string();
            self.seen.push(x.clone());
            if self.stop_at == Some(x.as_str()) {
                return ControlFlow::Break(self.seen.len());
            }
            ControlFlow::Continue(())
        }
        fn family(&mut self, _: View<'_, Family>, _: &StreamedRecord) -> ControlFlow<usize> {
            self.seen.push("FAM".into());
            ControlFlow::Continue(())
        }
        fn other(&mut self, n: View<'_, Node>, _: &StreamedRecord) -> ControlFlow<usize> {
            self.seen.push(n.tag().to_string());
            ControlFlow::Continue(())
        }
    }

    let file: &[u8] = b"0 HEAD\r1 GEDC\r2 VERS 7.0\r0 @I1@ INDI\r0 @F1@ FAM\r0 _LOC Here\r0 @I2@ INDI\r0 @I3@ INDI\r0 TRLR\r";
    let mut all = Counter::default();
    let flow = GedcomStreamParser::new(file)
        .unwrap()
        .visit(&mut all)
        .unwrap();
    assert_eq!(flow, ControlFlow::Continue(()));
    assert_eq!(all.seen, ["HEAD", "@I1@", "FAM", "_LOC", "@I2@", "@I3@"]);

    let mut parser = GedcomStreamParser::new(file).unwrap();
    let mut stop = Counter {
        stop_at: Some("@I2@"),
        ..Counter::default()
    };
    assert_eq!(parser.visit(&mut stop).unwrap(), ControlFlow::Break(5));
    let rest: Vec<_> = parser.map(Result::unwrap).collect();
    assert_eq!(rest.len(), 1);
}
