//! The line lexer and the record builder.
//!
//! The lexer is total: every input yields a tree, and no line is dropped
//! unless it carries no data (a blank line, or a level with nothing after
//! it). The recovery rules are fixed and silent; they are listed on
//! [`Tree`](super::Tree).

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use super::tag::{STANDARD_TAGS, standard_index};

/// The deepest level a structure can have. Deeper lines are attached at this
/// depth, as siblings, so that nothing recursive can overflow the stack.
pub(crate) const MAX_DEPTH: u8 = u8::MAX;

/// How `@` is escaped in text payloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Escaping {
    /// GEDCOM 5.5.1: every `@` of text is doubled; `@#…@` escapes stay.
    V551,
    /// GEDCOM 7.0: only a leading `@` is doubled.
    V70,
}

impl Escaping {
    /// The escaping of a `HEAD.GEDC.VERS` value: 7.x is 7.0, anything else
    /// (5.5, 5.5.1, 5.5.5, missing) is 5.5.1.
    pub(crate) fn of(vers: Option<&str>) -> Self {
        if vers.is_some_and(|v| crate::GedcomVersion::from_version_str(v).is_v7()) {
            Escaping::V70
        } else {
            Escaping::V551
        }
    }
}

/// A byte range of a segment's text or side buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: u32,
    pub len: u32,
}

impl Span {
    fn new(start: usize, len: usize) -> Self {
        // Segments are at most `SEGMENT_LIMIT` bytes and their side buffer at
        // most twice that, so offsets always fit.
        Self {
            start: u32::try_from(start).unwrap_or(u32::MAX),
            len: u32::try_from(len).unwrap_or(0),
        }
    }

    #[inline]
    pub(crate) fn get(self, buffer: &str) -> &str {
        let start = self.start as usize;
        buffer
            .get(start..start + self.len as usize)
            .unwrap_or_default()
    }
}

/// Where a node's payload lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Kind {
    None,
    /// A pointer, in the source text.
    Pointer,
    /// Text that needed no rewriting, in the source text.
    Text,
    /// Text rewritten (unescaped or joined), in the side buffer.
    Side,
    /// Text to unescape, in the source text (span mode only).
    Escaped,
    /// Text made of pieces of the source text (span mode only): the payload
    /// spans the builder's `joined` pieces.
    Joined,
}

/// A piece of a text joined from several spans of the source text: a span,
/// and whether a newline comes before it (a `CONT`). The piece's length
/// takes 31 bits (segments are at most 2 GiB), the flag the last one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TextPiece {
    start: u32,
    len_newline: u32,
}

impl TextPiece {
    const NEWLINE: u32 = 1 << 31;

    pub(crate) fn new(start: u32, len: u32, newline: bool) -> Self {
        Self {
            start,
            len_newline: (len & !Self::NEWLINE) | if newline { Self::NEWLINE } else { 0 },
        }
    }

    pub(crate) fn start(self) -> u32 {
        self.start
    }

    pub(crate) fn len(self) -> u32 {
        self.len_newline & !Self::NEWLINE
    }

    /// Whether a newline comes before the piece.
    pub(crate) fn newline(self) -> bool {
        self.len_newline & Self::NEWLINE != 0
    }

    /// The same piece, its start without the bits of `flag`.
    pub(crate) fn unflagged(self, flag: u32) -> Self {
        Self {
            start: self.start & !flag,
            ..self
        }
    }

    /// The same piece, `by` bytes further.
    pub(crate) fn shifted(self, by: u32) -> Self {
        Self {
            start: self.start.saturating_add(by),
            ..self
        }
    }
}

/// One structure in a segment's flat, pre-order arena.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RawNode {
    pub payload: Span,
    /// Index into the standard tag table, or past it into the tree's other tags.
    pub tag: u32,
    /// 1-based source line.
    pub line: u32,
    /// Index just past this node's subtree.
    pub end: u32,
    pub depth: u8,
    pub kind: Kind,
    /// Whether the segment's identifier table has an entry for this node.
    pub has_xref: bool,
}

/// The words of a set of standard tags, by index.
pub(crate) const SEEN_WORDS: usize = STANDARD_TAGS.len().div_ceil(64);

/// The tags that are not standard, interned per tree.
#[derive(Debug, Default)]
pub(crate) struct TagInterner {
    pub others: Vec<Box<str>>,
    /// The standard tags interned, by index ([`TagInterner::seen`]).
    seen: [u64; SEEN_WORDS],
    map: HashMap<Box<str>, u32, BuildHasherDefault<TagHasher>>,
}

/// A fast hash for short keys (the multiply-rotate scheme of `FxHash`):
/// tags are a few bytes long and come from the file itself, but a tree is
/// built once and the map dies with it, so flooding only slows that one
/// read down.
#[derive(Default)]
pub(crate) struct TagHasher(u64);

impl Hasher for TagHasher {
    fn write(&mut self, bytes: &[u8]) {
        const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;
        for chunk in bytes.chunks(8) {
            let mut word = [0_u8; 8];
            word.get_mut(..chunk.len())
                .into_iter()
                .for_each(|w| w.copy_from_slice(chunk));
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(word)).wrapping_mul(SEED);
        }
    }

    fn finish(&self) -> u64 {
        // A product's high bits mix every input bit, its low bits only the
        // low input bits: rotate them down, where hash tables take their
        // bucket index (keys such as `@I123@` differ in their high bytes).
        self.0.rotate_left(26)
    }
}

impl TagInterner {
    /// Rebuilds the lookup of [`TagInterner::others`], set from elsewhere
    /// (the tags of a dataset's store, to read more records into it).
    pub(crate) fn reindex(&mut self) {
        self.map.clear();
        for (i, tag) in self.others.iter().enumerate() {
            let id = u32::try_from(STANDARD_TAGS.len() + i).unwrap_or(u32::MAX);
            self.map.insert(tag.clone(), id);
        }
    }

    /// The id of `tag` when it is interned (or standard).
    pub(crate) fn find(&self, tag: &str) -> Option<u32> {
        match standard_index(tag) {
            Some(i) => Some(u32::from(i)),
            None => self.map.get(tag).copied(),
        }
    }

    /// The standard tags interned, as bits by index.
    pub(crate) fn seen(&self) -> &[u64; SEEN_WORDS] {
        &self.seen
    }

    /// The id the next tag interned takes.
    pub(crate) fn next_id(&self) -> u32 {
        u32::try_from(STANDARD_TAGS.len() + self.others.len()).unwrap_or(u32::MAX)
    }

    pub(crate) fn intern(&mut self, tag: &str) -> u32 {
        if let Some(i) = standard_index(tag) {
            if let Some(w) = self.seen.get_mut(usize::from(i) / 64) {
                *w |= 1 << (i % 64);
            }
            return u32::from(i);
        }
        if let Some(&id) = self.map.get(tag) {
            return id;
        }
        let id = u32::try_from(STANDARD_TAGS.len() + self.others.len()).unwrap_or(u32::MAX);
        self.others.push(tag.into());
        self.map.insert(tag.into(), id);
        id
    }
}

/// Finds the first CR or LF, eight bytes at a time.
#[inline]
pub(crate) fn find_eol(bytes: &[u8]) -> Option<usize> {
    find_eol_and_at(bytes).0
}

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

/// The bytes of `word` equal to `byte`, flagged in their high bit. The
/// lowest flag is exact; a flag above a true one may be spurious, which
/// "is there one" and "where is the first" questions do not mind.
#[inline]
const fn matches(word: u64, byte: u8) -> u64 {
    let x = word ^ (ONES * byte as u64);
    x.wrapping_sub(ONES) & !x & HIGH
}

/// Finds the first CR or LF, and tells whether an `@` comes before it, in
/// one pass eight bytes at a time.
#[inline]
fn find_eol_and_at(bytes: &[u8]) -> (Option<usize>, bool) {
    let (words, remainder) = bytes.as_chunks::<8>();
    let mut offset = 0;
    let mut at = false;
    for &chunk in words {
        let word = u64::from_le_bytes(chunk);
        let hits = matches(word, b'\n') | matches(word, b'\r');
        let ats = matches(word, b'@');
        if hits != 0 {
            let first = hits.trailing_zeros();
            // The `@` flags below the first terminator.
            at |= ats & ((1_u64 << first) - 1) != 0;
            return (Some(offset + (first / 8) as usize), at);
        }
        at |= ats != 0;
        offset += 8;
    }
    for (i, &b) in remainder.iter().enumerate() {
        match b {
            b'\n' | b'\r' => return (Some(offset + i), at),
            b'@' => at = true,
            _ => {}
        }
    }
    (None, at)
}

/// The length of the line terminator at the start of `rest`, which starts
/// with CR or LF: CR LF and LF CR count as one terminator (LF CR only when
/// not followed by LF, where it reads as LF then CR LF).
#[inline]
pub(crate) fn terminator_len(rest: &[u8]) -> usize {
    match rest {
        [b'\n', b'\r', b'\n', ..] => 1,
        [b'\r', b'\n', ..] | [b'\n', b'\r', ..] => 2,
        _ => 1,
    }
}

/// Bytes of lookahead [`terminator_len`] needs.
pub(crate) const TERMINATOR_LOOKAHEAD: usize = 3;

/// Iterates over the lines of a text, without their terminators, as
/// `(start, end)` byte ranges.
pub(crate) struct Lines<'s> {
    bytes: &'s [u8],
    pos: usize,
    at: bool,
}

impl<'s> Lines<'s> {
    pub(crate) fn new(text: &'s str) -> Self {
        Self {
            bytes: text.as_bytes(),
            pos: 0,
            at: false,
        }
    }

    /// Whether the line last returned holds an `@`.
    pub(crate) fn has_at(&self) -> bool {
        self.at
    }
}

impl Iterator for Lines<'_> {
    type Item = (usize, usize);

    #[inline]
    fn next(&mut self) -> Option<(usize, usize)> {
        let rest = self.bytes.get(self.pos..)?;
        if rest.is_empty() {
            return None;
        }
        let start = self.pos;
        let (eol, at) = find_eol_and_at(rest);
        self.at = at;
        if let Some(k) = eol {
            let term = rest.get(k..).map_or(1, terminator_len);
            self.pos = start + k + term;
            Some((start, start + k))
        } else {
            self.pos = self.bytes.len();
            Some((start, self.bytes.len()))
        }
    }
}

/// A line split on the line grammar. Ranges are byte offsets in the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    /// Empty or whitespace only.
    Blank,
    /// A level with nothing after it.
    LevelOnly,
    /// No level number: text that lost its `CONT`.
    NoLevel,
    /// `level [xref] tag [payload]`.
    Structure {
        level: u32,
        xref: Option<(usize, usize)>,
        tag: (usize, usize),
        payload: Option<(usize, usize)>,
    },
}

#[inline]
const fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Whether a line is a structure at level 0, as [`lex_line`] would read it:
/// leading blanks and byte order marks, a level whose digits are all zeros,
/// then a blank and something that is not blank.
#[inline]
fn starts_at_level_zero(b: &[u8]) -> bool {
    let mut i = 0;
    loop {
        match b.get(i..) {
            Some([c, ..]) if is_blank(*c) => i += 1,
            Some([0xEF, 0xBB, 0xBF, ..]) => i += 3,
            _ => break,
        }
    }
    let digits = i;
    while b.get(i) == Some(&b'0') {
        i += 1;
    }
    if i == digits || b.get(i).is_some_and(u8::is_ascii_digit) {
        return false;
    }
    if !b.get(i).is_some_and(|&c| is_blank(c)) {
        return false;
    }
    b.get(i..)
        .is_some_and(|rest| rest.iter().any(|&c| !is_blank(c)))
}

/// Splits one line (without its terminator).
///
/// Leading spaces, tabs and byte order marks are skipped; a leading zero or
/// any number of digits is accepted in the level; runs of spaces or tabs
/// separate the level, the identifier and the tag. Exactly one space (or
/// tab) separates the tag from the payload, which keeps its own leading
/// spaces.
#[inline]
pub(crate) fn lex_line(line: &str) -> Line {
    let b = line.as_bytes();
    let mut i = 0;
    loop {
        match b.get(i..) {
            Some([c, ..]) if is_blank(*c) => i += 1,
            Some([0xEF, 0xBB, 0xBF, ..]) => i += 3,
            _ => break,
        }
    }
    if i == b.len() {
        return Line::Blank;
    }
    let digits_start = i;
    let mut level: u32 = 0;
    while let Some(&d) = b.get(i).filter(|d| d.is_ascii_digit()) {
        level = level.saturating_mul(10).saturating_add(u32::from(d - b'0'));
        i += 1;
    }
    if i == digits_start {
        return Line::NoLevel;
    }
    match b.get(i) {
        None => return Line::LevelOnly,
        Some(&c) if !is_blank(c) => return Line::NoLevel,
        Some(_) => {}
    }
    while b.get(i).is_some_and(|&c| is_blank(c)) {
        i += 1;
    }
    if i == b.len() {
        return Line::LevelOnly;
    }
    let mut xref = None;
    if b.get(i) == Some(&b'@') {
        // An identifier runs to the next `@` followed by a delimiter (5.5.1
        // identifiers may hold spaces), else to the next delimiter.
        let close = b
            .get(i + 1..)
            .and_then(|r| r.iter().position(|&c| c == b'@'))
            .map(|p| i + 1 + p)
            .filter(|&p| b.get(p + 1).is_none_or(|&c| is_blank(c)));
        let end = match close {
            Some(p) => p + 1,
            None => b
                .get(i..)
                .and_then(|r| r.iter().position(|&c| is_blank(c)))
                .map_or(b.len(), |p| i + p),
        };
        xref = Some((i, end));
        i = end;
        while b.get(i).is_some_and(|&c| is_blank(c)) {
            i += 1;
        }
    }
    let tag_start = i;
    while b.get(i).is_some_and(|&c| !is_blank(c)) {
        i += 1;
    }
    let payload = if i + 1 < b.len() {
        Some((i + 1, b.len()))
    } else {
        None
    };
    Line::Structure {
        level,
        xref,
        tag: (tag_start, i),
        payload,
    }
}

/// Whether a raw payload is a pointer: `@`, a first character that is not
/// `@`, `#` or a space, then any characters but `@` and control codes, then
/// `@`. Spaces inside are allowed, as 5.5.1's `pointer_char` allows them;
/// trailing spaces after the closing `@` are ignored. Returns the pointer.
#[inline]
pub(crate) fn pointer(raw: &str) -> Option<&str> {
    let p = raw.trim_end_matches([' ', '\t']);
    let b = p.as_bytes();
    let inner = b.get(1..b.len().checked_sub(1)?)?;
    let ok = b.len() >= 3
        && b.first() == Some(&b'@')
        && b.last() == Some(&b'@')
        && !matches!(inner.first(), Some(b'@' | b'#' | b' ' | b'\t'))
        && !inner.iter().any(|&c| c == b'@' || c < 0x20 || c == 0x7F);
    ok.then_some(p)
}

/// Whether unescaping changes a raw text payload.
#[inline]
pub(crate) fn needs_unescape(raw: &str, escaping: Escaping) -> bool {
    match escaping {
        Escaping::V70 => raw.starts_with("@@"),
        Escaping::V551 => raw.contains("@@"),
    }
}

/// The text a raw payload stands for, as the spans of `raw` (offset,
/// length) it is made of, in order.
///
/// GEDCOM 7.0 doubles only a leading `@`. GEDCOM 5.5.1 doubles every `@` of
/// text, except in escape sequences (`@#DJULIAN@`), which are kept as they
/// are; a lone `@` is kept too. Unescaping only drops characters, so the
/// text is `raw` with gaps.
pub(crate) fn unescape_spans(raw: &str, escaping: Escaping, mut span: impl FnMut(usize, usize)) {
    match escaping {
        Escaping::V70 => {
            let skip = usize::from(raw.starts_with("@@"));
            span(skip, raw.len() - skip);
        }
        Escaping::V551 => {
            let bytes = raw.as_bytes();
            let mut start = 0;
            let mut from = 0;
            while let Some(at) = bytes
                .get(from..)
                .and_then(|rest| rest.iter().position(|&b| b == b'@'))
                .map(|p| from + p)
            {
                from = match bytes.get(at + 1) {
                    // `@@`: the first `@` stays, the second goes.
                    Some(b'@') => {
                        span(start, at + 1 - start);
                        start = at + 2;
                        at + 2
                    }
                    // An escape sequence, kept to its closing `@`.
                    Some(b'#') => bytes
                        .get(at + 2..)
                        .and_then(|rest| rest.iter().position(|&b| b == b'@'))
                        .map_or(at + 1, |p| at + p + 3),
                    _ => at + 1,
                };
            }
            span(start, raw.len().saturating_sub(start));
        }
    }
}

/// Appends the text a raw payload stands for (see [`unescape_spans`]).
pub(crate) fn unescape_into(raw: &str, escaping: Escaping, out: &mut String) {
    unescape_spans(raw, escaping, |start, len| {
        out.push_str(raw.get(start..start + len).unwrap_or_default());
    });
}

/// The `HEAD.GEDC.VERS` payload, trimmed, of the first record that starts
/// with a structure line, if that record is `HEAD`. A record made of stray
/// lines without a level, before it, does not count.
///
/// Only the start of `text` is read: the record found, and a stray record
/// before it.
pub(crate) fn head_version(text: &str) -> Option<String> {
    let prefix = text.get(..first_records_end(text)).unwrap_or_default();
    let mut builder = Builder::new(Escaping::V551, 1);
    let mut tags = TagInterner::default();
    builder.read(prefix, &mut tags);
    let empty = tags.intern("");
    let nodes = &builder.nodes;
    // The first substructure of `parent` with tag `tag`.
    let child = |parent: usize, tag: &str| -> Option<usize> {
        let tag = u32::from(standard_index(tag)?);
        let end = nodes.get(parent)?.end as usize;
        let mut i = parent + 1;
        while i < end {
            let node = nodes.get(i)?;
            if node.tag == tag {
                return Some(i);
            }
            i = (node.end as usize).max(i + 1);
        }
        None
    };
    // The first root that came from a structure line.
    let mut root = 0;
    while let Some(node) = nodes.get(root) {
        if node.tag != empty || node.has_xref {
            break;
        }
        root = (node.end as usize).max(root + 1);
    }
    let head = u32::from(standard_index("HEAD")?);
    if nodes.get(root)?.tag != head {
        return None;
    }
    let vers = nodes.get(child(child(root, "GEDC")?, "VERS")?)?;
    let payload = match vers.kind {
        Kind::Side => vers.payload.get(&builder.side),
        Kind::Joined => "",
        Kind::None | Kind::Pointer | Kind::Text | Kind::Escaped => vers.payload.get(prefix),
    };
    Some(payload.trim().to_owned())
}

/// Whether a record starts with a line without a level: stray lines at the
/// start of the input, which come before the record that tells the version.
pub(crate) fn starts_stray(record: &str) -> bool {
    Lines::new(record)
        .map(|(s, e)| lex_line(record.get(s..e).unwrap_or_default()))
        .find(|l| !matches!(l, Line::Blank | Line::LevelOnly))
        == Some(Line::NoLevel)
}

/// The end of the first records that matter for the version: the first
/// record, and the next one too when the first is made of lines without a
/// level.
pub(crate) fn first_records_end(text: &str) -> usize {
    let mut records = 0;
    let mut stray = false;
    for (start, end) in Lines::new(text) {
        match lex_line(text.get(start..end).unwrap_or_default()) {
            Line::NoLevel if records == 0 => {
                records = 1;
                stray = true;
            }
            Line::Structure { level, .. } if records == 0 || level == 0 => {
                if records == 1 && !stray || records == 2 {
                    return start;
                }
                records += 1;
            }
            _ => {}
        }
    }
    text.len()
}

/// A structure line, with offsets into the text being read.
#[derive(Clone, Copy)]
struct Parts<'l> {
    level: u32,
    xref: Option<(usize, usize)>,
    tag: &'l str,
    payload: Option<(usize, usize)>,
    /// Whether the line holds an `@` (else the payload is plain text).
    has_at: bool,
}

/// An open structure while its record is being read.
#[derive(Clone, Copy, Debug)]
struct Open {
    node: u32,
    /// The level as written, for nesting the next lines.
    raw_level: u32,
    /// The last direct child, or `u32::MAX`.
    last_child: u32,
}

/// A piece of text to append to a node's payload when the record closes.
#[derive(Clone, Copy, Debug)]
struct Piece {
    target: u32,
    /// `CONT` (or a line without a level): a newline comes first.
    newline: bool,
    /// The text is literal (a line without a level), not a payload to unescape.
    literal: bool,
    raw: Span,
}

/// Builds the arena of one segment, line by line.
#[derive(Debug)]
pub(crate) struct Builder {
    pub nodes: Vec<RawNode>,
    /// `(node, identifier)`, by node.
    pub xrefs: Vec<(u32, Span)>,
    pub side: String,
    /// In span mode, the pieces of the joined payloads (offsets relative to
    /// the text read), which `Kind::Joined` payloads span.
    pub joined: Vec<TextPiece>,
    /// Span mode: payloads that reading rewrites are left as spans of the
    /// text (`Kind::Escaped`, `Kind::Joined`) instead of being copied into
    /// the side buffer.
    spans: bool,
    escaping: Escaping,
    stack: Vec<Open>,
    pieces: Vec<Piece>,
    /// The node the previous line went to, for a line without a level.
    previous: Option<u32>,
    line: u32,
}

impl Builder {
    pub(crate) fn new(escaping: Escaping, first_line: u32) -> Self {
        Self {
            nodes: Vec::new(),
            xrefs: Vec::new(),
            side: String::new(),
            joined: Vec::new(),
            spans: false,
            escaping,
            stack: Vec::new(),
            pieces: Vec::new(),
            previous: None,
            line: first_line.saturating_sub(1),
        }
    }

    /// Sets how text payloads are unescaped from now on.
    pub(crate) fn set_escaping(&mut self, escaping: Escaping) {
        self.escaping = escaping;
    }

    /// A builder in span mode: rewritten payloads stay spans of the text
    /// read (see [`Kind::Escaped`] and [`Kind::Joined`]), nothing is copied.
    pub(crate) fn with_spans(mut self) -> Self {
        self.spans = true;
        self
    }

    /// Clears the arena for the next record, keeping the allocations.
    pub(crate) fn reset(&mut self, first_line: u32) {
        self.nodes.clear();
        self.xrefs.clear();
        self.side.clear();
        self.joined.clear();
        self.stack.clear();
        self.pieces.clear();
        self.previous = None;
        self.line = first_line.saturating_sub(1);
    }

    /// Reads every line of `text` (offsets are relative to it).
    pub(crate) fn read(&mut self, text: &str, tags: &mut TagInterner) {
        // Lines average 20 to 30 bytes: this rarely needs to grow, and the
        // part never written is never touched (the arena is shrunk to fit
        // once built).
        self.nodes.reserve(text.len() / 16);
        let mut lines = Lines::new(text);
        while let Some((start, end)) = lines.next() {
            self.push_line(text, start, end, lines.has_at(), tags);
        }
        self.close_record(text);
    }

    /// Reads every line of `text` one record at a time: once a record is
    /// complete, `record` sees the arena holding it alone, which is then
    /// cleared for the next one. Memory is bounded by the largest record,
    /// and offsets stay relative to `text`.
    pub(crate) fn read_records(
        &mut self,
        text: &str,
        tags: &mut TagInterner,
        mut record: impl FnMut(&mut Self, &mut TagInterner),
    ) {
        let mut lines = Lines::new(text);
        while let Some((start, end)) = lines.next() {
            let bytes = text.as_bytes().get(start..end).unwrap_or_default();
            if !self.nodes.is_empty() && starts_at_level_zero(bytes) {
                self.close_record(text);
                record(self, tags);
                self.clear_record();
            }
            self.push_line(text, start, end, lines.has_at(), tags);
        }
        self.close_record(text);
        if !self.nodes.is_empty() {
            record(self, tags);
            self.clear_record();
        }
    }

    /// Clears the arena of a record read by [`read_records`](Self::read_records),
    /// keeping the allocations and the line count.
    fn clear_record(&mut self) {
        self.nodes.clear();
        self.xrefs.clear();
        self.side.clear();
        self.joined.clear();
        self.stack.clear();
        self.pieces.clear();
        self.previous = None;
    }

    fn push_line(
        &mut self,
        text: &str,
        start: usize,
        end: usize,
        has_at: bool,
        tags: &mut TagInterner,
    ) {
        self.line = self.line.saturating_add(1);
        let line = text.get(start..end).unwrap_or_default();
        match lex_line(line) {
            Line::Blank | Line::LevelOnly => {}
            Line::NoLevel => self.no_level(start, end, tags),
            Line::Structure {
                level,
                xref,
                tag,
                payload,
            } => {
                let at = |(s, e): (usize, usize)| (start + s, start + e);
                let parts = Parts {
                    level,
                    xref: xref.map(at),
                    tag: line.get(tag.0..tag.1).unwrap_or_default(),
                    payload: payload.map(at),
                    has_at,
                };
                self.structure(text, parts, tags);
            }
        }
    }

    fn kind(&self, node: u32) -> Kind {
        self.nodes.get(node as usize).map_or(Kind::None, |n| n.kind)
    }

    /// Whether text can be appended to a node's payload: not a pointer, and
    /// not an identifier without a tag (no line could hold both).
    fn accepts_text(&self, node: u32, empty_tag: u32) -> bool {
        self.nodes
            .get(node as usize)
            .is_some_and(|n| n.kind != Kind::Pointer && !(n.tag == empty_tag && n.has_xref))
    }

    fn len(&self) -> u32 {
        u32::try_from(self.nodes.len()).unwrap_or(u32::MAX)
    }

    /// Closes the open structures down to `keep` entries.
    fn pop_to(&mut self, keep: usize) {
        let end = self.len();
        while self.stack.len() > keep {
            if let Some(open) = self.stack.pop()
                && let Some(n) = self.nodes.get_mut(open.node as usize)
            {
                n.end = end;
            }
        }
    }

    /// Closes the open structures that a line at `level` does not nest in;
    /// the record itself stays open.
    fn pop_for(&mut self, level: u32) {
        let keep = self
            .stack
            .iter()
            .rposition(|o| o.raw_level < level)
            .map_or(1, |p| p + 1)
            .max(1);
        self.pop_to(keep);
    }

    fn push_node(&mut self, node: RawNode) -> u32 {
        let index = self.len();
        self.nodes.push(node);
        index
    }

    fn structure(&mut self, text: &str, parts: Parts<'_>, tags: &mut TagInterner) {
        let Parts {
            level,
            xref,
            tag,
            payload,
            has_at,
        } = parts;
        let starts_record = level == 0 || self.stack.is_empty();
        if starts_record {
            self.close_record(text);
        } else {
            self.pop_for(level);
            // An identifier on a continuation names nothing a reader could
            // point to: the line continues its text all the same.
            if (tag == "CONT" || tag == "CONC")
                && let Some(target) = self.continuation_target(tags.intern(""))
            {
                let raw = payload.map_or(Span::default(), |(s, e)| Span::new(s, e - s));
                self.add_piece(target, tag == "CONT", false, raw);
                return;
            }
        }

        let (kind, span) = match payload {
            None => (Kind::None, Span::default()),
            Some((s, e)) => {
                let raw = text.get(s..e).unwrap_or_default();
                if !has_at || !raw.as_bytes().contains(&b'@') {
                    // Most payloads: neither a pointer nor an escape.
                    (Kind::Text, Span::new(s, e - s))
                } else if let Some(p) = pointer(raw) {
                    (Kind::Pointer, Span::new(s, p.len()))
                } else if needs_unescape(raw, self.escaping) && self.spans {
                    (Kind::Escaped, Span::new(s, e - s))
                } else if needs_unescape(raw, self.escaping) {
                    let at = self.side.len();
                    unescape_into(raw, self.escaping, &mut self.side);
                    (Kind::Side, Span::new(at, self.side.len() - at))
                } else {
                    (Kind::Text, Span::new(s, e - s))
                }
            }
        };
        let index = self.len();
        if let Some((s, e)) = xref {
            self.xrefs.push((index, Span::new(s, e - s)));
        }
        let mut node = RawNode {
            payload: span,
            tag: tags.intern(tag),
            line: self.line,
            end: 0,
            depth: 0,
            kind,
            has_xref: xref.is_some(),
        };
        if starts_record {
            node.end = index + 1;
            self.push_node(node);
            self.stack.push(Open {
                node: index,
                raw_level: 0,
                last_child: u32::MAX,
            });
        } else {
            // Below the depth limit the parent is the deepest open structure;
            // at the limit, its parent.
            let parent = (self.stack.len() - 1).min(usize::from(MAX_DEPTH) - 1);
            self.pop_to(parent + 1);
            node.depth = u8::try_from(parent + 1).unwrap_or(MAX_DEPTH);
            self.push_node(node);
            if let Some(p) = self.stack.get_mut(parent) {
                p.last_child = index;
            }
            self.stack.push(Open {
                node: index,
                raw_level: level,
                last_child: u32::MAX,
            });
        }
        self.previous = Some(index);
    }

    /// The payload a `CONT` or `CONC` continues: its parent's text; else
    /// the previous sibling's (a continuation written at the level of the
    /// line it continues); else the parent's empty payload. `None` when the
    /// parent holds a pointer and has no sibling to continue: the line is
    /// then kept as a structure of its own.
    fn continuation_target(&self, empty_tag: u32) -> Option<u32> {
        let top = self.stack.last()?;
        let parent = top.node;
        if matches!(self.kind(parent), Kind::Text | Kind::Side | Kind::Escaped) {
            return Some(parent);
        }
        if top.last_child != u32::MAX && self.accepts_text(top.last_child, empty_tag) {
            return Some(top.last_child);
        }
        (self.kind(parent) == Kind::None && self.accepts_text(parent, empty_tag)).then_some(parent)
    }

    fn add_piece(&mut self, target: u32, newline: bool, literal: bool, raw: Span) {
        if let Some(n) = self.nodes.get_mut(target as usize)
            && n.kind == Kind::None
        {
            // The payload is built when the record closes.
            n.kind = Kind::Text;
            n.payload = Span::default();
        }
        self.pieces.push(Piece {
            target,
            newline,
            literal,
            raw,
        });
        self.previous = Some(target);
    }

    /// A line without a level continues the previous line's text; after a
    /// pointer (or before any record), it is kept as a structure with an
    /// empty tag and no identifier.
    fn no_level(&mut self, start: usize, end: usize, tags: &mut TagInterner) {
        let raw = Span::new(start, end - start);
        let empty_tag = tags.intern("");
        if let Some(prev) = self.previous
            && self.accepts_text(prev, empty_tag)
        {
            self.add_piece(prev, true, true, raw);
            return;
        }
        let index = self.len();
        let mut node = RawNode {
            payload: raw,
            tag: empty_tag,
            has_xref: false,
            line: self.line,
            end: index + 1,
            depth: 0,
            kind: Kind::Text,
        };
        if self.stack.is_empty() {
            self.push_node(node);
            self.stack.push(Open {
                node: index,
                raw_level: 0,
                last_child: u32::MAX,
            });
        } else {
            let parent = (self.stack.len() - 1).min(usize::from(MAX_DEPTH) - 1);
            self.pop_to(parent + 1);
            node.depth = u8::try_from(parent + 1).unwrap_or(MAX_DEPTH);
            self.push_node(node);
            if let Some(p) = self.stack.get_mut(parent) {
                p.last_child = index;
            }
        }
        self.previous = Some(index);
    }

    /// Span mode: lists, for each payload continued in the record, the
    /// spans of the text it is made of (its own payload, then each
    /// continuation, unescaped), and makes it a `Kind::Joined` payload.
    fn join_spans(&mut self, text: &str, pieces: &[Piece]) {
        for group in pieces.chunk_by(|a, b| a.target == b.target) {
            let Some(first) = group.first() else { continue };
            let Some(node) = self.nodes.get(first.target as usize).copied() else {
                continue;
            };
            let at = self.joined.len();
            let escaping = self.escaping;
            let joined = &mut self.joined;
            // The spans of a raw payload starting at `start`, the first one
            // after a newline when `newline` holds.
            let mut push = |start: u32, raw: &str, literal: bool, newline: bool| {
                let mut newline = newline;
                let mut add = |s: usize, len: usize| {
                    if len > 0 || newline {
                        let s = start.saturating_add(u32::try_from(s).unwrap_or(u32::MAX));
                        let len = u32::try_from(len).unwrap_or(0);
                        joined.push(TextPiece::new(s, len, newline));
                        newline = false;
                    }
                };
                // `add` is called at least once, so a newline is never lost.
                if literal {
                    add(0, raw.len());
                } else {
                    unescape_spans(raw, escaping, &mut add);
                }
            };
            match node.kind {
                Kind::Text => push(node.payload.start, node.payload.get(text), true, false),
                Kind::Escaped => push(node.payload.start, node.payload.get(text), false, false),
                Kind::None | Kind::Pointer | Kind::Side | Kind::Joined => {}
            }
            for piece in group {
                push(
                    piece.raw.start,
                    piece.raw.get(text),
                    piece.literal,
                    piece.newline,
                );
            }
            let count = self.joined.len() - at;
            if let Some(n) = self.nodes.get_mut(first.target as usize) {
                n.payload = Span::new(at, count);
                n.kind = if count == 0 { Kind::None } else { Kind::Joined };
            }
        }
    }

    /// Joins the record's continuation pieces into their payloads and closes it.
    fn close_record(&mut self, text: &str) {
        self.pop_to(0);
        self.previous = None;
        if self.pieces.is_empty() {
            return;
        }
        let mut pieces = std::mem::take(&mut self.pieces);
        pieces.sort_by_key(|p| p.target);
        if self.spans {
            self.join_spans(text, &pieces);
            pieces.clear();
            self.pieces = pieces;
            return;
        }
        for group in pieces.chunk_by(|a, b| a.target == b.target) {
            let Some(first) = group.first() else { continue };
            let Some(node) = self.nodes.get(first.target as usize).copied() else {
                continue;
            };
            let at = self.side.len();
            match node.kind {
                Kind::Text => self.side.push_str(node.payload.get(text)),
                Kind::Side => {
                    let s = node.payload.start as usize;
                    self.side
                        .extend_from_within(s..s + node.payload.len as usize);
                }
                Kind::None | Kind::Pointer | Kind::Escaped | Kind::Joined => {}
            }
            for piece in group {
                if piece.newline {
                    self.side.push('\n');
                }
                let raw = piece.raw.get(text);
                if piece.literal {
                    self.side.push_str(raw);
                } else {
                    unescape_into(raw, self.escaping, &mut self.side);
                }
            }
            let len = self.side.len() - at;
            if let Some(n) = self.nodes.get_mut(first.target as usize) {
                n.payload = Span::new(at, len);
                n.kind = if len == 0 { Kind::None } else { Kind::Side };
            }
        }
        pieces.clear();
        self.pieces = pieces;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<&str> {
        Lines::new(text).map(|(s, e)| &text[s..e]).collect()
    }

    #[test]
    fn every_terminator_ends_a_line() {
        assert_eq!(lines("a\rb\r\nc\nd\n\re"), ["a", "b", "c", "d", "e"]);
        assert_eq!(lines("a\n\r\nb"), ["a", "", "b"]);
        assert_eq!(lines("a\r\n\r\nb\n"), ["a", "", "b"]);
        assert_eq!(
            lines("0123456789abcdef\r0123456789"),
            ["0123456789abcdef", "0123456789"]
        );
    }

    #[test]
    fn at_signs_before_the_terminator_are_seen() {
        for text in [
            "@\n",
            "abcdefgh@\nx",
            "abcdefghijklmno@",
            "a\n@",
            "abcdefgh\r@@@@@@@",
        ] {
            let plain = text.split(['\n', '\r']).next().unwrap().contains('@');
            assert_eq!(find_eol_and_at(text.as_bytes()).1, plain, "{text:?}");
        }
    }

    #[test]
    fn level_zero_lines_as_the_lexer_reads_them() {
        for line in [
            "0 HEAD",
            "00 HEAD",
            " \t0\t@I1@ INDI",
            "\u{feff}0 TRLR",
            "0",
            "0 ",
            "01 X",
            "10 X",
            "1 X",
            "x",
            "",
            "0x",
            "0 \t ",
            "000 @X@",
        ] {
            let lexed = matches!(lex_line(line), Line::Structure { level: 0, .. });
            assert_eq!(starts_at_level_zero(line.as_bytes()), lexed, "{line:?}");
        }
    }

    #[test]
    fn records_one_at_a_time_match_the_whole_read() {
        let text = "stray\n1 X\n0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @N1@ NOTE a@@b\n1 CONC c\n\n0 @I1@ INDI\n3 NOTE deep\n0 TRLR";
        let mut tags = TagInterner::default();
        let mut whole = Builder::new(Escaping::V551, 1);
        whole.read(text, &mut tags);
        let mut seen = Vec::new();
        let mut split = Builder::new(Escaping::V551, 1);
        split.read_records(text, &mut tags, |b, _| {
            let first = b.nodes[0];
            let payload = if first.kind == Kind::Side {
                first.payload.get(&b.side).to_string()
            } else {
                String::new()
            };
            seen.push((b.nodes.len(), b.nodes[0].line, payload));
        });
        let roots: Vec<usize> = (0..whole.nodes.len())
            .filter(|&i| whole.nodes[i].depth == 0)
            .collect();
        assert_eq!(seen.len(), roots.len());
        for (k, &root) in roots.iter().enumerate() {
            let end = whole.nodes[root].end as usize;
            assert_eq!(seen[k].0, end - root, "record {k}");
            assert_eq!(seen[k].1, whole.nodes[root].line);
        }
        // The note record's text, unescaped and joined, in its side buffer.
        assert_eq!(seen[2].2, "a@bc");
    }

    /// The payload of every node of a builder, as text.
    fn payloads(b: &Builder, text: &str, escaping: Escaping) -> Vec<String> {
        b.nodes
            .iter()
            .map(|n| match n.kind {
                Kind::None => String::new(),
                Kind::Pointer | Kind::Text => n.payload.get(text).to_string(),
                Kind::Side => n.payload.get(&b.side).to_string(),
                Kind::Escaped => {
                    let mut out = String::new();
                    unescape_into(n.payload.get(text), escaping, &mut out);
                    out
                }
                Kind::Joined => {
                    let start = n.payload.start as usize;
                    let mut out = String::new();
                    for p in &b.joined[start..start + n.payload.len as usize] {
                        if p.newline() {
                            out.push('\n');
                        }
                        let s = p.start() as usize;
                        out.push_str(&text[s..s + p.len() as usize]);
                    }
                    out
                }
            })
            .collect()
    }

    #[test]
    fn span_mode_reads_what_the_side_buffer_holds() {
        let fragments = [
            "0 @N1@ NOTE a@@b",
            "1 CONT @@c",
            "1 CONC d@@@@e",
            "1 CONT",
            "1 CONC ",
            "2 CONT x",
            "stray line @@",
            "1 SOUR @S1@",
            "1 NOTE @@N1@ text",
            "1 NOTE @#DJULIAN@@ 1700",
            "1 _X @",
            "0 @I1@ INDI",
            "1 NAME A@@B /C/",
            "2 CONC @@",
            "1 @C@ CONT with an identifier",
        ];
        let mut seed = 7_u64;
        let mut tags = TagInterner::default();
        for case in 0..400 {
            let mut text = String::new();
            for _ in 0..(case % 12) + 1 {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                text.push_str(fragments[(seed >> 33) as usize % fragments.len()]);
                text.push('\n');
            }
            for escaping in [Escaping::V551, Escaping::V70] {
                let mut copy = Builder::new(escaping, 1);
                copy.read(&text, &mut tags);
                let mut spans = Builder::new(escaping, 1).with_spans();
                spans.read(&text, &mut tags);
                assert_eq!(
                    payloads(&copy, &text, escaping),
                    payloads(&spans, &text, escaping),
                    "{escaping:?}: {text:?}"
                );
                assert!(spans.side.is_empty());
            }
        }
    }

    #[test]
    fn unescaped_spans_are_the_unescaped_text() {
        for raw in [
            "a@@b",
            "@@",
            "@@@",
            "@#DJULIAN@@x",
            "@#D",
            "x@",
            "@@a@@",
            "plain",
        ] {
            for escaping in [Escaping::V551, Escaping::V70] {
                let mut joined = String::new();
                unescape_spans(raw, escaping, |s, l| joined.push_str(&raw[s..s + l]));
                let mut out = String::new();
                unescape_into(raw, escaping, &mut out);
                assert_eq!(joined, out, "{raw:?} {escaping:?}");
            }
        }
        let mut out = String::new();
        unescape_into("a@@b@#DJULIAN@ c@@", Escaping::V551, &mut out);
        assert_eq!(out, "a@b@#DJULIAN@ c@");
    }

    #[test]
    fn find_eol_matches_a_plain_search() {
        let text: Vec<u8> = (0..300_u32)
            .map(|i| [b'a', b'\n', b'\r', 0x8A, 0x0D ^ 0x80][(i * 7 % 13 % 5) as usize])
            .collect();
        for start in 0..text.len() {
            let plain = text[start..].iter().position(|&b| b == b'\n' || b == b'\r');
            assert_eq!(find_eol(&text[start..]), plain, "{start}");
        }
    }

    #[test]
    fn line_grammar() {
        assert_eq!(lex_line("  \t"), Line::Blank);
        assert_eq!(lex_line("12"), Line::LevelOnly);
        assert_eq!(lex_line("1990s were"), Line::NoLevel);
        assert_eq!(lex_line("text"), Line::NoLevel);
        assert_eq!(
            lex_line("1 NOTE  two"),
            Line::Structure {
                level: 1,
                xref: None,
                tag: (2, 6),
                payload: Some((7, 11))
            }
        );
        assert_eq!(
            lex_line(" 01\t @I1@  INDI"),
            Line::Structure {
                level: 1,
                xref: Some((5, 9)),
                tag: (11, 15),
                payload: None
            }
        );
        assert_eq!(
            lex_line("0 @NoTe ref@ NOTE x"),
            Line::Structure {
                level: 0,
                xref: Some((2, 12)),
                tag: (13, 17),
                payload: Some((18, 19))
            }
        );
        assert_eq!(
            lex_line("1 NOTE "),
            Line::Structure {
                level: 1,
                xref: None,
                tag: (2, 6),
                payload: None
            }
        );
    }

    #[test]
    fn pointers() {
        assert_eq!(pointer("@I1@"), Some("@I1@"));
        assert_eq!(pointer("@I1@ "), Some("@I1@"));
        assert_eq!(pointer("@NoTe ref@"), Some("@NoTe ref@"));
        assert_eq!(pointer("@I132!1@"), Some("@I132!1@"));
        assert_eq!(pointer("@!1@"), Some("@!1@"));
        for text in [
            "@@I1@",
            "@#DJULIAN@",
            "@I1@ x",
            "@ I1@",
            "@@",
            "@",
            "a@I1@",
            "@I@1@",
        ] {
            assert_eq!(pointer(text), None, "{text}");
        }
    }

    #[test]
    fn unescaping() {
        let un = |raw: &str, e| {
            let mut s = String::new();
            unescape_into(raw, e, &mut s);
            s
        };
        assert_eq!(un("@@@@@ has four", Escaping::V70), "@@@@ has four");
        assert_eq!(un("a@@b", Escaping::V70), "a@@b");
        assert_eq!(un("a@@b @@c", Escaping::V551), "a@b @c");
        assert_eq!(
            un("@#DJULIAN@ 1 JAN 1700", Escaping::V551),
            "@#DJULIAN@ 1 JAN 1700"
        );
        assert_eq!(un("@#DJULIAN@@@x", Escaping::V551), "@#DJULIAN@@x");
        assert_eq!(un("lone @ and @#open", Escaping::V551), "lone @ and @#open");
        assert!(!needs_unescape("@#DJULIAN@ 1700", Escaping::V551));
    }

    #[test]
    fn version_from_the_head_record_only() {
        let v = |t: &str| head_version(t);
        assert_eq!(
            v("0 HEAD\r1 SOUR x\r2 VERS 9\r1 GEDC\r2 VERS 7.0\r0 TRLR").as_deref(),
            Some("7.0")
        );
        assert_eq!(v("0 HEAD\n1 GEDC\n0 @I1@ INDI\n1 GEDC\n2 VERS 7.0"), None);
        assert_eq!(v("0 @I1@ INDI\n0 HEAD\n1 GEDC\n2 VERS 7.0"), None);
        assert_eq!(
            v("stray\n1 CONT x\n0 HEAD\n1 GEDC\n2 VERS 7.0").as_deref(),
            Some("7.0")
        );
        assert_eq!(v("1 STRAY\n0 HEAD\n1 GEDC\n2 VERS 7.0"), None);
        assert_eq!(v("1 HEAD\n3 GEDC\n4 VERS 7.0").as_deref(), Some("7.0"));
        assert_eq!(
            v("\n\n0 HEAD\n1 GEDC\n2 VERS  5.5.1 \n").as_deref(),
            Some("5.5.1")
        );
        assert_eq!(first_records_end("x\n1 A\n0 HEAD\n0 B\n"), 13);
        assert_eq!(first_records_end("0 HEAD\n1 A\n0 B\n"), 11);
    }
}
