//! The decoder and the lexer are total and consistent: for any bytes,
//! decoding never panics and gives the same text in memory and streaming
//! (whatever the buffer size; but for the one documented case, an ASCII
//! first 64 KiB), the lossless tree reads its own dump back unchanged and
//! dumps it again as it was, and streaming yields the tree's records, line
//! numbers included.
#![no_main]

use std::io::{BufReader, Read};

use libfuzzer_sys::fuzz_target;
use oxidged::encoding::{DecodeReader, decode};
use oxidged::tree::{Structure, Tree, TreeReader, parse_tree};

/// The bytes a stream reads to choose the decoding.
const WINDOW: usize = 64 * 1024;

fn lines(s: &Structure, out: &mut Vec<u32>) {
    out.push(s.line);
    for c in &s.substructures {
        lines(c, out);
    }
}

fuzz_target!(|data: &[u8]| {
    // The first byte picks a buffer size for the streaming paths.
    let (capacity, bytes) = match data.split_first() {
        Some((&c, rest)) => (usize::from(c % 32) + 1, rest),
        None => (1, data),
    };

    let decoded = decode(bytes);
    let mut streamed = String::new();
    DecodeReader::new(BufReader::with_capacity(capacity, bytes))
        .expect("reading a slice cannot fail")
        .read_to_string(&mut streamed)
        .expect("decoded text is UTF-8");
    // Both decide from the first 64 KiB; when those are ASCII, which tells
    // nothing, the rest of the input decides in memory, out of the stream's
    // sight (`oxidged::encoding`).
    let ascii_window = bytes.get(..WINDOW).is_some_and(|w| w.is_ascii()) && bytes.len() > WINDOW;
    assert!(
        streamed == decoded.text || ascii_window,
        "decoded differently in memory and streaming"
    );
    let same = streamed == decoded.text;

    let tree = Tree::parse(decoded.text);
    let records = tree.to_structures();
    let dump = tree.to_gedcom();
    let back = parse_tree(&dump);
    assert_eq!(back.to_structures(), records);
    // Dumping is idempotent: dump(parse(dump(x))) == dump(x).
    assert_eq!(back.to_gedcom(), dump);

    let reader: Vec<Structure> = TreeReader::new(BufReader::with_capacity(capacity, bytes))
        .expect("reading a slice cannot fail")
        .collect::<Result<_, _>>()
        .expect("reading a slice cannot fail");
    let records = if same {
        records
    } else {
        Tree::parse(streamed).to_structures()
    };
    assert_eq!(reader, records);
    let (mut a, mut b) = (Vec::new(), Vec::new());
    reader.iter().for_each(|s| lines(s, &mut a));
    records.iter().for_each(|s| lines(s, &mut b));
    assert_eq!(a, b);
});
