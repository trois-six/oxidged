//! The lossless tree: every line of a GEDCOM file as a structure, with
//! nothing interpreted and nothing lost.
//!
//! GEDCOM 7.0 (§1) describes a dataset as a sequence of *records*, each a
//! *structure* made of a *tag*, an optional *cross-reference identifier*, an
//! optional *payload* (a *pointer* or text) and its *substructures*.
//! [`Tree`] holds exactly that, read with fixed and silent recovery rules,
//! so that any input — conformant or not, in any line-terminator
//! convention — gives a tree that keeps all its data.
//!
//! # Reading rules
//!
//! | Input | Result |
//! |---|---|
//! | CR, LF, CR LF or LF CR terminators | All accepted, mixed or not. |
//! | Blank or whitespace-only lines; a level with nothing after it | Skipped. |
//! | Spaces, tabs or a byte order mark before the level; leading zeros or more than two digits in it; runs of spaces or tabs between level, identifier and tag | Accepted. |
//! | The delimiter after the tag | Exactly one space (or tab); the payload keeps its own leading and trailing spaces. |
//! | `CONT` and `CONC` | Joined into the payload they continue (an identifier on the line is dropped), under any tag, at any level, also after substructures: the parent's text, else the previous sibling's (a continuation written one level too high), else the parent's empty payload. Under a pointer with nothing else to continue, kept as a structure. |
//! | A line without a level number | Continues the previous line's text after a newline; after a pointer (or before any record) it becomes a structure with an empty tag holding the line. |
//! | A level that jumps by more than one | The line nests in the deepest open structure of its record, never in another record. |
//! | Deeper than 255 levels | Attached at depth 255, as a sibling. |
//! | An identifier on a substructure; content after `TRLR`; substructures of `TRLR`; no `HEAD` or no `TRLR` | Kept where they are. |
//! | Unknown, extension, lower-case or misplaced tags | Kept as they are, under their real parent. |
//! | Control characters in payloads | Kept. |
//!
//! A payload is a **pointer** when it has the pointer shape: `@`, a first
//! character other than `@`, `#` or a space, any characters but `@`, then
//! `@` (5.5.1, p. 13, allows spaces inside; trailing spaces after the closing
//! `@` are ignored). This covers 5.5.1's substructure (`@I1!2@`),
//! intra-record (`@!2@`) and network (`:`) forms. Anything else is **text**,
//! unescaped for the file's version, which `HEAD.GEDC.VERS` gives (7.x is
//! 7.0, anything else 5.5.1): 7.0 drops the first `@` of a leading `@@`;
//! 5.5.1 turns every `@@` into `@` and keeps escape sequences such as
//! `@#DJULIAN@` as they are.
//!
//! # Memory
//!
//! A tree keeps the decoded text it was read from and points into it: each
//! structure is a 24-byte entry of a flat, pre-order array, with its tag as
//! a 32-bit index (standard tags are interned in a static table) and its
//! payload as a 32-bit offset and length into the text; identifiers, which
//! records carry and few substructures do, sit in a side table. Only
//! payloads that reading rewrote (joined continuations, unescaped `@@`) are
//! copied, into a side buffer. No structure allocates. Read from a
//! `Vec<u8>` of UTF-8 ([`Tree::from_bytes`]), the buffer itself becomes the
//! text: a 64 MB file peaks at about 2.5 times its size.
//!
//! # Example
//!
//! ```rust
//! use oxidged::tree::{parse_tree, PayloadRef};
//!
//! let tree = parse_tree("0 HEAD\r1 GEDC\r2 VERS 7.0\r0 @I1@ INDI\r1 NOTE First\r2 CONT second\r1 FAMC @F1@\r0 TRLR\r");
//! let indi = tree.records().nth(1).unwrap();
//! assert_eq!(indi.xref(), Some("@I1@"));
//! let mut subs = indi.substructures();
//! assert_eq!(subs.next().unwrap().payload(), PayloadRef::Text("First\nsecond"));
//! assert_eq!(subs.next().unwrap().pointer(), Some("@F1@"));
//! ```

mod arena;
mod flat;
mod lexer;
pub(crate) mod node;
mod reader;
mod tag;
mod write;
mod xref;

pub use reader::TreeReader;
pub use tag::Tag;
pub use xref::{Xref, XrefForm};

pub(crate) use flat::{Flat, FlatPayload, FlatRef, Stream, StreamBuffers, UNKNOWN};
pub(crate) use lexer::{
    Builder, Escaping, Kind as RawKind, Line, Lines, MAX_DEPTH, RawNode, Span as RawSpan,
    TagHasher, TagInterner, TextPiece, find_eol, head_version, lex_line, pointer, terminator_len,
    unescape_into, unescape_spans,
};
pub(crate) use node::Node;
pub(crate) use reader::RecordSource;
pub(crate) use tag::{STANDARD_TAGS, standard_index, standard_index_const};
pub(crate) use write::line as write_line;

use std::fmt;

use crate::version::GedcomVersion;
use lexer::Span;

/// The largest segment of text one arena indexes: offsets are 32-bit, and the
/// side buffer of a segment can grow to twice its text.
const SEGMENT_LIMIT: usize = (u32::MAX / 2) as usize;

/// A whole GEDCOM file as a lossless tree of structures.
///
/// See the [module documentation](self) for the reading rules.
pub struct Tree {
    segments: Vec<Segment>,
    /// The tags that are not standard, by identifier minus the table length.
    tags: Box<[Box<str>]>,
    vers: Option<Box<str>>,
    escaping: Escaping,
}

/// A run of whole records sharing one text buffer and one arena. A file has
/// a single segment unless its text exceeds 2 GiB.
struct Segment {
    text: Box<str>,
    side: Box<str>,
    nodes: Box<[RawNode]>,
    xrefs: Box<[(u32, Span)]>,
}

impl Segment {
    fn build(text: Box<str>, escaping: Escaping, first_line: u32, tags: &mut TagInterner) -> Self {
        let mut builder = Builder::new(escaping, first_line);
        builder.read(&text, tags);
        Self::from_builder(text, builder)
    }

    fn from_builder(text: Box<str>, builder: Builder) -> Self {
        Self {
            text,
            side: builder.side.into_boxed_str(),
            nodes: builder.nodes.into_boxed_slice(),
            xrefs: builder.xrefs.into_boxed_slice(),
        }
    }
}

/// Parses text into a tree. See [`Tree::parse`].
#[must_use]
pub fn parse_tree(text: &str) -> Tree {
    Tree::parse(text)
}

impl Tree {
    /// Parses text, decoded already, into a tree. Never fails.
    ///
    /// Taking a `String` avoids a copy: the tree keeps the text.
    #[must_use]
    pub fn parse(text: impl Into<String>) -> Self {
        Self::parse_segmented(text.into(), SEGMENT_LIMIT)
    }

    /// Decodes bytes of any encoding (see [`crate::encoding::decode`]) and
    /// parses them. Given a `Vec<u8>` of UTF-8, the tree keeps that buffer
    /// as its text, without a copy.
    #[must_use]
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self::parse(crate::encoding::decode_owned(bytes.into()).text)
    }

    fn parse_segmented(text: String, limit: usize) -> Self {
        let vers: Option<Box<str>> = head_version(&text).map(Box::from);
        let escaping = Escaping::of(vers.as_deref());
        let mut tags = TagInterner::default();
        let bounds = segment_bounds(&text, limit);
        let segments = if let [(_, _, first_line)] = bounds.as_slice() {
            vec![Segment::build(
                text.into_boxed_str(),
                escaping,
                *first_line,
                &mut tags,
            )]
        } else {
            bounds
                .iter()
                .map(|&(start, end, first_line)| {
                    let part: Box<str> = text.get(start..end).unwrap_or_default().into();
                    Segment::build(part, escaping, first_line, &mut tags)
                })
                .collect()
        };
        Self {
            segments,
            tags: tags.others.into_boxed_slice(),
            vers,
            escaping,
        }
    }

    /// The records, in file order.
    #[must_use]
    pub fn records(&self) -> Records<'_> {
        Records {
            tree: self,
            segment: 0,
            next: 0,
        }
    }

    /// The version the file declares, as [`GedcomVersion::from_version_str`]
    /// reads `HEAD.GEDC.VERS`: 7.0 or 7.1 for 7.x, 5.5.1 otherwise (5.5,
    /// 5.5.1, 5.5.5, or no declaration).
    #[must_use]
    pub fn version(&self) -> GedcomVersion {
        self.vers
            .as_deref()
            .map_or(GedcomVersion::V5_5_1, GedcomVersion::from_version_str)
    }

    /// The `HEAD.GEDC.VERS` payload as written, trimmed.
    #[must_use]
    pub fn declared_version(&self) -> Option<&str> {
        self.vers.as_deref()
    }

    /// The tags read that are not standard, once each.
    pub(crate) fn other_tags(&self) -> impl Iterator<Item = &str> {
        self.tags.iter().map(|t| &**t)
    }

    /// The number of structures, records included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.segments.iter().map(|s| s.nodes.len()).sum()
    }

    /// Whether the tree has no record.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.iter().all(|s| s.nodes.is_empty())
    }

    /// Copies the records out as owned structures.
    #[must_use]
    pub fn to_structures(&self) -> Vec<Structure> {
        self.records().map(StructureRef::to_structure).collect()
    }

    /// Writes the tree back as GEDCOM text with LF terminators: each
    /// structure on one line, text with newlines continued with `CONT`, `@`
    /// escaped for the tree's version. Reading the output gives an equal
    /// tree.
    ///
    /// This is a faithful dump, not a conformant writer: it keeps whatever
    /// the tree holds and wraps no long line.
    #[must_use]
    pub fn to_gedcom(&self) -> String {
        let mut out = String::with_capacity(self.segments.iter().map(|s| s.text.len()).sum());
        for segment in &self.segments {
            for (index, node) in segment.nodes.iter().enumerate() {
                let node = StructureRef {
                    tree: self,
                    segment,
                    node,
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                };
                write::line(
                    &mut out,
                    usize::from(node.level()),
                    node.xref(),
                    node.tag(),
                    node.payload(),
                    self.escaping,
                );
            }
        }
        out
    }
}

impl fmt::Debug for Tree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tree")
            .field("version", &self.version())
            .field("records", &self.records().count())
            .field("structures", &self.len())
            .finish_non_exhaustive()
    }
}

impl fmt::Display for Tree {
    /// The tree as GEDCOM text, as [`Tree::to_gedcom`] writes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_gedcom())
    }
}

/// The byte ranges and first line numbers of the segments of a text: whole
/// records, cut before a record that would take a segment past `limit`
/// (or, for a single record larger than that, before the line that would).
pub(crate) fn segment_bounds(text: &str, limit: usize) -> Vec<(usize, usize, u32)> {
    if text.len() <= limit {
        return vec![(0, text.len(), 1)];
    }
    let mut bounds = Vec::new();
    let (mut seg_start, mut seg_line) = (0, 1_u32);
    let mut last_record: Option<(usize, u32)> = None;
    let mut line_no = 0_u32;
    for (start, end) in Lines::new(text) {
        line_no = line_no.saturating_add(1);
        let line = text.get(start..end).unwrap_or_default();
        if start > seg_start && matches!(lex_line(line), Line::Structure { level: 0, .. }) {
            last_record = Some((start, line_no));
        }
        if end - seg_start > limit {
            let cut = last_record.or((start > seg_start).then_some((start, line_no)));
            if let Some((cut, cut_line)) = cut {
                bounds.push((seg_start, cut, seg_line));
                (seg_start, seg_line) = (cut, cut_line);
                last_record = None;
            }
        }
    }
    bounds.push((seg_start, text.len(), seg_line));
    bounds
}

/// The records of a [`Tree`].
#[derive(Clone)]
pub struct Records<'t> {
    tree: &'t Tree,
    segment: usize,
    next: u32,
}

impl<'t> Iterator for Records<'t> {
    type Item = StructureRef<'t>;

    #[inline]
    fn next(&mut self) -> Option<StructureRef<'t>> {
        loop {
            let segment = self.tree.segments.get(self.segment)?;
            if let Some(node) = segment.nodes.get(self.next as usize) {
                let index = self.next;
                self.next = node.end;
                return Some(StructureRef {
                    tree: self.tree,
                    segment,
                    node,
                    index,
                });
            }
            self.segment += 1;
            self.next = 0;
        }
    }
}

/// A structure of a [`Tree`]: a cheap, copyable view.
#[derive(Clone, Copy)]
pub struct StructureRef<'t> {
    tree: &'t Tree,
    segment: &'t Segment,
    /// The node at `index`, found once.
    node: &'t RawNode,
    index: u32,
}

/// A payload, borrowed from a [`Tree`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum PayloadRef<'t> {
    /// No payload.
    #[default]
    None,
    /// A pointer, delimiters included: `@I1@`.
    Pointer(&'t str),
    /// Text, unescaped, with continuations joined (`CONT` as `\n`).
    Text(&'t str),
}

impl<'t> PayloadRef<'t> {
    /// The pointer or the text; `None` when there is no payload.
    #[must_use]
    #[inline]
    pub fn as_str(self) -> Option<&'t str> {
        match self {
            PayloadRef::None => None,
            PayloadRef::Pointer(s) | PayloadRef::Text(s) => Some(s),
        }
    }

    /// An owned copy.
    #[must_use]
    pub fn to_payload(self) -> Payload {
        match self {
            PayloadRef::None => Payload::None,
            PayloadRef::Pointer(p) => Payload::Pointer(Xref::new(p)),
            PayloadRef::Text(t) => Payload::Text(t.into()),
        }
    }
}

impl<'t> StructureRef<'t> {
    #[inline]
    fn view(self) -> arena::View<'t> {
        arena::View {
            text: &self.segment.text,
            side: &self.segment.side,
            nodes: &self.segment.nodes,
            xrefs: &self.segment.xrefs,
            tags: &self.tree.tags,
        }
    }

    /// The tag's index: in the standard tags, or past them in the tree's
    /// other tags.
    #[inline]
    pub(crate) fn raw_tag(self) -> u32 {
        self.node().tag
    }

    #[inline]
    fn node(self) -> &'t RawNode {
        self.node
    }

    /// The tag; empty for a line that had no level number.
    #[must_use]
    #[inline]
    pub fn tag(self) -> &'t str {
        self.view().tag(self.node())
    }

    /// The tag as an owned [`Tag`].
    #[must_use]
    pub fn tag_owned(self) -> Tag {
        self.view().tag_owned(self.node())
    }

    /// The cross-reference identifier, delimiters included.
    #[must_use]
    #[inline]
    pub fn xref(self) -> Option<&'t str> {
        if !self.node.has_xref {
            return None;
        }
        self.view().xref(self.index as usize)
    }

    /// The payload.
    #[must_use]
    #[inline]
    pub fn payload(self) -> PayloadRef<'t> {
        self.view().payload(self.node())
    }

    /// The text payload, if the payload is text.
    #[must_use]
    pub fn text(self) -> Option<&'t str> {
        match self.payload() {
            PayloadRef::Text(t) => Some(t),
            _ => None,
        }
    }

    /// The pointer, if the payload is a pointer.
    #[must_use]
    pub fn pointer(self) -> Option<&'t str> {
        match self.payload() {
            PayloadRef::Pointer(p) => Some(p),
            _ => None,
        }
    }

    /// The nesting depth: 0 for a record.
    #[must_use]
    pub fn level(self) -> u8 {
        self.node().depth
    }

    /// The 1-based line the structure starts on in the source text.
    #[must_use]
    #[inline]
    pub fn line(self) -> u32 {
        self.node().line
    }

    /// The substructures, in order.
    #[must_use]
    #[inline]
    pub fn substructures(self) -> Substructures<'t> {
        Substructures {
            parent: self,
            next: self.index + 1,
            end: self.node().end,
        }
    }

    /// The first substructure with this tag.
    #[must_use]
    pub fn first(self, tag: &str) -> Option<Self> {
        self.substructures().find(|s| s.tag() == tag)
    }

    /// An owned copy of the structure and its substructures.
    #[must_use]
    pub fn to_structure(self) -> Structure {
        self.view().to_structure(self.index as usize)
    }
}

impl fmt::Debug for StructureRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StructureRef")
            .field("line", &self.line())
            .field("level", &self.level())
            .field("xref", &self.xref())
            .field("tag", &self.tag())
            .field("payload", &self.payload())
            .finish_non_exhaustive()
    }
}

/// The substructures of a [`StructureRef`].
#[derive(Clone)]
pub struct Substructures<'t> {
    parent: StructureRef<'t>,
    next: u32,
    end: u32,
}

impl<'t> Iterator for Substructures<'t> {
    type Item = StructureRef<'t>;

    #[inline]
    fn next(&mut self) -> Option<StructureRef<'t>> {
        if self.next >= self.end {
            return None;
        }
        let node = self.parent.segment.nodes.get(self.next as usize)?;
        let child = StructureRef {
            node,
            index: self.next,
            ..self.parent
        };
        self.next = node.end.max(self.next + 1);
        Some(child)
    }
}

/// A payload: none, a pointer or text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Payload {
    /// No payload.
    #[default]
    None,
    /// A pointer to a record (or, in 5.5.1, to a substructure).
    Pointer(Xref),
    /// Text: unescaped, with continuations joined (`CONT` as `\n`).
    Text(Box<str>),
}

impl Payload {
    /// The pointer or the text; `None` when there is no payload.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Payload::None => None,
            Payload::Pointer(p) => Some(p.as_str()),
            Payload::Text(t) => Some(t),
        }
    }

    /// A borrowed view.
    #[must_use]
    pub fn borrowed(&self) -> PayloadRef<'_> {
        match self {
            Payload::None => PayloadRef::None,
            Payload::Pointer(p) => PayloadRef::Pointer(p.as_str()),
            Payload::Text(t) => PayloadRef::Text(t),
        }
    }
}

/// An owned structure: a tag, an optional identifier, an optional payload
/// and substructures.
///
/// Equality ignores [`line`](Self::line), which only records where the
/// structure was read.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Structure {
    /// The tag.
    pub tag: Tag,
    /// The cross-reference identifier, on a record.
    pub xref: Option<Xref>,
    /// The payload.
    pub payload: Payload,
    /// The substructures, in order.
    pub substructures: Vec<Structure>,
    /// The 1-based source line; 0 for a structure that was not read.
    pub line: u32,
}

impl Structure {
    /// A structure with a tag and nothing else.
    #[must_use]
    pub fn new(tag: impl Into<Tag>) -> Self {
        Self {
            tag: tag.into(),
            ..Self::default()
        }
    }

    /// The text payload, if the payload is text.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match &self.payload {
            Payload::Text(t) => Some(t),
            _ => None,
        }
    }

    /// The pointer, if the payload is a pointer.
    #[must_use]
    pub fn pointer(&self) -> Option<&Xref> {
        match &self.payload {
            Payload::Pointer(p) => Some(p),
            _ => None,
        }
    }

    /// The first substructure with this tag.
    #[must_use]
    pub fn first(&self, tag: &str) -> Option<&Structure> {
        self.substructures.iter().find(|s| s.tag == tag)
    }

    /// Writes the structure and its substructures as GEDCOM text, at
    /// `level`, with `@` escaped for `version`. See [`Tree::to_gedcom`].
    #[must_use]
    pub fn to_gedcom(&self, level: u8, version: GedcomVersion) -> String {
        let mut out = String::new();
        self.write_gedcom(&mut out, level, version);
        out
    }

    /// Appends what [`Structure::to_gedcom`] gives to `out`.
    pub(crate) fn write_gedcom(&self, out: &mut String, level: u8, version: GedcomVersion) {
        let escaping = if version.is_v7() {
            Escaping::V70
        } else {
            Escaping::V551
        };
        self.write(out, usize::from(level), escaping);
    }

    fn write(&self, out: &mut String, level: usize, escaping: Escaping) {
        write::line(
            out,
            level,
            self.xref.as_deref(),
            self.tag.as_str(),
            self.payload.borrowed(),
            escaping,
        );
        for s in &self.substructures {
            s.write(out, level + 1, escaping);
        }
    }
}

impl PartialEq for Structure {
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag
            && self.xref == other.xref
            && self.payload == other.payload
            && self.substructures == other.substructures
    }
}

impl Eq for Structure {}

impl From<StructureRef<'_>> for Structure {
    fn from(s: StructureRef<'_>) -> Self {
        s.to_structure()
    }
}

#[cfg(test)]
mod tests;
