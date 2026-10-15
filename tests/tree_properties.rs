//! Property checks of the decoder and the lexer on generated inputs.
//!
//! A small deterministic generator (no dependency) builds byte strings from
//! GEDCOM fragments, line terminators, `@` forms, ANSEL marks, UTF-8 and
//! UTF-16 pieces and random bytes. For each one:
//!
//! * decoding and reading never panic;
//! * the tree's dump reads back to the same tree;
//! * streaming (any buffer size) reads the same records, line numbers
//!   included, and decodes the same text;
//! * the typed readers (in memory and streaming) do not panic either.
//!
//! The `fuzz/` crate runs the same checks under libFuzzer.

use std::io::{BufReader, Read};

use oxidged::encoding::{DecodeReader, decode};
use oxidged::tree::{Structure, Tree, TreeReader, parse_tree};
use oxidged::version::detect_version;
use oxidged::{Dataset, GedcomBuilder, GedcomStreamParser};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const FRAGMENTS: &[&[u8]] = &[
    b"0 HEAD",
    b"1 GEDC",
    b"2 VERS 7.0",
    b"2 VERS 5.5.1",
    b"1 CHAR ANSEL",
    b"1 CHAR ANSI",
    b"1 CHAR UNICODE",
    b"0 @I1@ INDI",
    b"0 @N1@ NOTE ",
    b"0 TRLR",
    b"1 NOTE ",
    b"2 CONT ",
    b"2 CONC ",
    b"1 CONT",
    b"3 CONC",
    b"1 FAMC @F1@",
    b"@@",
    b"@#DJULIAN@ ",
    b"@I1@",
    b"@",
    b"@NoTe ref@",
    b"1 NAME A /B/",
    b"2 _X",
    b"01",
    b"  ",
    b"\t",
    b"\r",
    b"\n",
    b"\r\n",
    b"\n\r",
    b"\xE2",
    b"\xE3\xE8",
    b"\xA1",
    b"\xEF\xBB\xBF",
    b"\xC3\xA9",
    b"\xE9",
    b"\x80",
    b"\x00",
    b"0\x00 \x00",
    b"\xFF\xFE",
    b"\xFE\xFF",
    b"99 ",
    b"256 _D",
    b"garbage",
    b"\xF0\x9F\x98",
];

fn generate(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..rng.below(60) {
        match rng.below(10) {
            0 => out.push(u8::try_from(rng.below(256)).unwrap_or(0)),
            1..=3 => out.extend_from_slice(b"\n"),
            _ => out.extend_from_slice(FRAGMENTS[rng.below(FRAGMENTS.len())]),
        }
    }
    out
}

fn lines(s: &Structure, out: &mut Vec<u32>) {
    out.push(s.line);
    for c in &s.substructures {
        lines(c, out);
    }
}

fn check(bytes: &[u8], capacity: usize) {
    // Decoding: in memory and streaming agree.
    let decoded = decode(bytes);
    let mut streamed = String::new();
    DecodeReader::new(BufReader::with_capacity(capacity, bytes))
        .unwrap()
        .read_to_string(&mut streamed)
        .unwrap();
    assert_eq!(streamed, decoded.text, "decode {bytes:?}");

    // The tree: dump and re-read, stream and compare.
    let tree = Tree::parse(decoded.text.clone());
    let records = tree.to_structures();
    assert_eq!(
        parse_tree(&tree.to_gedcom()).to_structures(),
        records,
        "dump {bytes:?}"
    );
    let reader: Vec<Structure> = TreeReader::new(BufReader::with_capacity(capacity, bytes))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(reader, records, "stream {bytes:?}");
    let (mut a, mut b) = (Vec::new(), Vec::new());
    reader.iter().for_each(|s| lines(s, &mut a));
    records.iter().for_each(|s| lines(s, &mut b));
    assert_eq!(a, b, "lines {bytes:?}");

    // The typed readers must not panic.
    let _ = GedcomBuilder::new().build_from_bytes(bytes);
    let _ = GedcomBuilder::new().build_from_str(decoded.text.as_str());
    let _ = GedcomBuilder::new().strict(true).build_from_bytes(bytes);
    let _ = Dataset::from_bytes(bytes);
    if let Ok(parser) = GedcomStreamParser::new(BufReader::with_capacity(capacity, bytes)) {
        for record in parser {
            let _ = record;
        }
    }
    let _ = detect_version(&decoded.text);
}

#[test]
fn generated_inputs_never_panic_and_agree() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..4000 {
        let bytes = generate(&mut rng);
        let capacity = 1 + rng.below(16);
        check(&bytes, capacity);
    }
}

#[test]
fn fixtures_agree_with_small_buffers() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for name in ["simple.ged", "sample.ged"] {
        let bytes = std::fs::read(dir.join(name)).unwrap();
        for capacity in [1, 3, 64] {
            check(&bytes, capacity);
        }
    }
}
