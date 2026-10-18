//! The generic driver behind every typed structure.
//!
//! [`gedcom_struct!`](super::gedcom_struct) declares a structure: its
//! identifier, tag and payload when it has them, its substructures by tag
//! and its field types. The field types give the cardinality through
//! [`Slot`] and [`RowSlot`](super::table::RowSlot): an `Option` holds one occurrence (a second one
//! goes to `extra`), a `Vec` or a [`ThinVec`](super::ThinVec) any number.
//! The macro generates the owned struct, its row, its field table and a
//! few short methods ([`RowFields`], [`RowWrite`], [`Fields`]); everything
//! else is done here, once for all structures — the loops below have one
//! copy, which every type calls through a trait object:
//!
//! - [`read_row_into`] reads a structure from a node of the record arena
//!   into its row of a dataset's tables: the identifier, the tag and the
//!   payload, then each substructure through the field its tag selects (a
//!   table of the standard tags, computed at compile time), and whatever no
//!   field takes — an unknown or extension tag, a repeated singleton, a
//!   substructure whose shape its field cannot hold — into `extra`, in
//!   order. Reading never fails: a node that does not fit a type is refused
//!   (its row reads `None`) and its superstructure keeps it whole in its own
//!   `extra`.
//! - [`write_row_flat`] writes a row into the writer's flat arena, and
//!   [`write`] and [`write_flat`] an owned structure: identifier, payload,
//!   fields in declaration order, then `extra`.
//!
//! A structure may keep the fields that few of its occurrences use in a
//! *detail* (`@detail`): a row of its own, which the structure's row holds
//! by index when one of them is set (a box on the owned type). Its fields
//! read, write and are checked by the coverage ledger like the others.

use std::borrow::Cow;

use crate::spec::schema::Kind;
use crate::tree::{
    Escaping, Flat, FlatPayload, Payload, RawKind, RawNode, RawSpan, STANDARD_TAGS, Structure, Tag,
    TextPiece, Xref, unescape_spans,
};
use crate::version::GedcomVersion;

use super::list::ThinVec;
use super::node::{Extra, Node};
use super::table::{
    Code, CopyCx, EMPTY_TABLES, List, NodeRow, StoreCx, Tables, push_span, reserve,
};
use super::text::{EXACT_DATE, SIMPLE_DATE, Span, Store, StoreParts, TagId, Text, TextRef, XrefId};

/// The arena of the record being read: the builder's nodes, identifiers
/// and joined pieces, with offsets relative to `text`, which starts at
/// `base` in the store's input.
#[derive(Clone, Copy)]
pub(crate) struct Arena<'a> {
    /// The text the builder read (a segment of the store's input).
    pub text: &'a str,
    /// The offset of `text` in the store's input.
    pub base: usize,
    pub joined: &'a [TextPiece],
    pub nodes: &'a [RawNode],
    pub xrefs: &'a [(u32, RawSpan)],
    /// The store's input, which spans flagged [`Arena::INPUT`] are of.
    pub input: &'a str,
    /// Texts written beside the input for the record (by rules that
    /// rewrite it as it is read), which the store appends at `side_base`
    /// in its text: what spans flagged [`Arena::INPUT`] past the input's
    /// end are of.
    pub side: &'a str,
    pub side_base: usize,
    /// The tags the record was read with that are not standard (the
    /// tree's other tags), by [`NodeRef::name`].
    pub others: &'a [Box<str>],
}

impl<'a> Arena<'a> {
    /// The flag of a span of the store's input, at an offset from its
    /// start, in an arena whose text is another (a record rewritten as it
    /// is read: [`crate::model::read`]), instead of a span of `text`.
    pub(crate) const INPUT: u32 = 1 << 31;

    /// The characters at `start` (an offset in the store's text, of its
    /// input or of the side texts past it), `len` long.
    #[inline]
    fn flagged(&self, start: usize, len: usize) -> &'a str {
        let (text, start) = if start < self.input.len() {
            (self.input, start)
        } else {
            (self.side, start.wrapping_sub(self.side_base))
        };
        text.get(start..start.saturating_add(len))
            .unwrap_or_default()
    }

    /// The characters of `span`: of `text`, or of `input` (the store's
    /// input, or its side texts past it) when flagged [`Arena::INPUT`].
    #[inline]
    pub(crate) fn get(&self, span: RawSpan) -> &'a str {
        if span.start & Self::INPUT == 0 {
            span.get(self.text)
        } else {
            self.flagged((span.start & !Self::INPUT) as usize, span.len as usize)
        }
    }

    /// The characters of a piece of a joined text: of `text`, or of
    /// `input` when flagged [`Arena::INPUT`].
    #[inline]
    pub(crate) fn piece(&self, piece: TextPiece) -> &'a str {
        if piece.start() & Self::INPUT == 0 {
            let start = piece.start() as usize;
            self.text
                .get(start..start + piece.len() as usize)
                .unwrap_or_default()
        } else {
            self.flagged(
                (piece.start() & !Self::INPUT) as usize,
                piece.len() as usize,
            )
        }
    }

    /// Where `span` starts in the store's text.
    #[inline]
    pub(crate) fn start(&self, span: RawSpan) -> usize {
        if span.start & Self::INPUT == 0 {
            self.base + span.start as usize
        } else {
            (span.start & !Self::INPUT) as usize
        }
    }
}

/// A node of the record arena, with its entry.
#[derive(Clone, Copy)]
pub(crate) struct NodeRef<'a> {
    arena: &'a Arena<'a>,
    raw: RawNode,
    index: u32,
}

impl<'a> NodeRef<'a> {
    /// The record at the start of the arena.
    pub(crate) fn root(arena: &'a Arena<'a>) -> Option<Self> {
        Self::at(arena, 0)
    }

    pub(crate) fn at(arena: &'a Arena<'a>, index: u32) -> Option<Self> {
        let raw = *arena.nodes.get(index as usize)?;
        Some(Self { arena, raw, index })
    }

    fn raw(self) -> RawNode {
        self.raw
    }

    /// The tag's identifier: a standard tag's index, or past the table.
    pub(crate) fn tag_id(self) -> u32 {
        self.raw().tag
    }

    /// The tag, when it is standard.
    pub(crate) fn standard_tag(self) -> Option<&'static str> {
        STANDARD_TAGS.get(self.tag_id() as usize).copied()
    }

    /// Whether the node has substructures.
    pub(crate) fn has_children(self) -> bool {
        self.raw().end > self.index + 1
    }

    /// Whether the node holds an identifier.
    pub(crate) fn has_xref(self) -> bool {
        self.raw().has_xref
    }

    /// Whether the payload is a pointer.
    pub(crate) fn is_pointer(self) -> bool {
        self.raw().kind == RawKind::Pointer
    }

    /// Whether there is no payload.
    pub(crate) fn has_no_payload(self) -> bool {
        self.raw().kind == RawKind::None
    }

    /// The payload's characters: the pointer or the text, unescaped and
    /// joined (borrowed unless joined from several lines).
    pub(crate) fn payload_str(self, escaping: Escaping) -> Cow<'a, str> {
        let raw = self.raw();
        let text = self.arena.text;
        match raw.kind {
            RawKind::None | RawKind::Side => Cow::Borrowed(""),
            RawKind::Pointer | RawKind::Text => Cow::Borrowed(self.arena.get(raw.payload)),
            RawKind::Escaped => {
                let raw = raw.payload.get(text);
                let mut out = String::with_capacity(raw.len());
                crate::tree::unescape_into(raw, escaping, &mut out);
                Cow::Owned(out)
            }
            RawKind::Joined => {
                let mut out = String::new();
                for piece in self.joined(raw) {
                    if piece.newline() {
                        out.push('\n');
                    }
                    out.push_str(self.arena.piece(*piece));
                }
                Cow::Owned(out)
            }
        }
    }

    fn joined(self, raw: RawNode) -> &'a [TextPiece] {
        let start = raw.payload.start as usize;
        self.arena
            .joined
            .get(start..start + raw.payload.len as usize)
            .unwrap_or_default()
    }

    /// The substructures, in order.
    pub(crate) fn children(self) -> Children<'a> {
        let raw = self.raw();
        Children {
            arena: self.arena,
            next: self.index + 1,
            end: raw.end,
        }
    }

    /// The node and every structure below it, in the order read, as the
    /// lexer left them: `subtree()[k]` is [`NodeRef::below`]`(k)`.
    pub(crate) fn subtree(self) -> &'a [RawNode] {
        self.arena
            .nodes
            .get(self.index as usize..self.raw.end as usize)
            .unwrap_or_default()
    }

    /// The structure `k` places after this one in [`NodeRef::subtree`].
    pub(crate) fn below(self, k: usize) -> Option<NodeRef<'a>> {
        let index = self.index.checked_add(u32::try_from(k).ok()?)?;
        (index < self.raw.end)
            .then(|| NodeRef::at(self.arena, index))
            .flatten()
    }

    /// The tag's text: a standard tag's, else one of `others` (the tags of
    /// the [`crate::tree::TagInterner`] the record was read with).
    #[inline]
    pub(crate) fn tag_text<'t>(self, others: &'t [Box<str>]) -> &'t str
    where
        'a: 't,
    {
        let id = self.tag_id() as usize;
        match STANDARD_TAGS.get(id) {
            Some(tag) => tag,
            None => others.get(id - STANDARD_TAGS.len()).map_or("", |t| t),
        }
    }

    /// The tag's text: a standard tag's, else the one the record was read
    /// with ([`Arena::others`]).
    pub(crate) fn name(self) -> &'a str {
        self.tag_text(self.arena.others)
    }

    /// The same node read as the standard tag `tag` (an index of
    /// [`STANDARD_TAGS`]): what the reader makes of an extension structure
    /// the conformance repair wrote for it ([`read_tag_of`]).
    pub(crate) fn read_as(self, tag: u16) -> Self {
        Self {
            raw: RawNode {
                tag: u32::from(tag),
                ..self.raw
            },
            ..self
        }
    }

    /// The pieces of the input a joined payload is made of, and where the
    /// text they are pieces of starts in the store's text.
    pub(crate) fn joined_pieces(self) -> Option<(&'a [TextPiece], usize)> {
        let raw = self.raw();
        (raw.kind == RawKind::Joined).then(|| (self.joined(raw), self.arena.base))
    }

    /// The identifier, delimiters included.
    #[inline]
    pub(crate) fn xref_text(self) -> Option<&'a str> {
        self.xref_span().map(|span| self.arena.get(span))
    }

    /// The 1-based source line (from the first line of the text read).
    #[inline]
    pub(crate) fn line(self) -> u32 {
        self.raw().line
    }

    fn xref_span(self) -> Option<RawSpan> {
        if !self.raw().has_xref {
            return None;
        }
        let at = self
            .arena
            .xrefs
            .binary_search_by_key(&self.index, |&(n, _)| n)
            .ok()?;
        self.arena.xrefs.get(at).map(|&(_, span)| span)
    }
}

/// The substructures of a [`NodeRef`].
pub(crate) struct Children<'a> {
    arena: &'a Arena<'a>,
    next: u32,
    end: u32,
}

impl<'a> Iterator for Children<'a> {
    type Item = NodeRef<'a>;

    fn next(&mut self) -> Option<NodeRef<'a>> {
        if self.next >= self.end {
            return None;
        }
        let child = NodeRef::at(self.arena, self.next)?;
        self.next = child.raw.end.max(self.next + 1);
        Some(child)
    }
}

/// What reading into the tables keeps aside while a structure is read: its
/// substructures that go untyped, and the items its lists reserve. Stacks,
/// so that nested structures share them.
#[derive(Default)]
pub(crate) struct Scratch {
    pending: Vec<Pending>,
    /// By field of each structure being read, the substructures of its
    /// list fields.
    counts: Vec<u32>,
    /// The list fields of each structure being read that have some.
    lists: Vec<u16>,
}

/// A substructure of the structure being read that goes untyped, by its
/// index in the record's arena.
#[derive(Clone, Copy)]
enum Pending {
    /// A substructure no field takes.
    Node(u32),
    /// What the structure itself has no place for (its payload, its
    /// identifier), kept aside ([`TagId::ASIDE`]).
    Aside {
        node: u32,
        xref: bool,
        payload: bool,
    },
}

/// What reading needs besides the node: the store's tables and the
/// dataset's, being filled, how the file escapes `@` and the version it
/// declares.
pub(crate) struct ReadCx<'s> {
    parts: StoreParts<'s>,
    /// The tables the structures read go into.
    pub(crate) tables: &'s mut Tables,
    scratch: &'s mut Scratch,
    escaping: Escaping,
    /// The version of the file.
    pub(crate) version: GedcomVersion,
    /// Whether the text is read as written: a repaired structure stays the
    /// extension the writer made of it ([`read_tag_of`]).
    pub(crate) as_written: bool,
}

impl<'s> ReadCx<'s> {
    pub(crate) fn new(
        parts: StoreParts<'s>,
        tables: &'s mut Tables,
        scratch: &'s mut Scratch,
        version: GedcomVersion,
    ) -> Self {
        Self {
            parts,
            tables,
            scratch,
            escaping: if version.is_v7() {
                Escaping::V70
            } else {
                Escaping::V551
            },
            version,
            as_written: false,
        }
    }

    /// The context of a text read instead of the store's, which starts at
    /// `base` in the store's text: records rewritten as they are read,
    /// whose text the store appends once read ([`StoreParts::with_text`]).
    pub(crate) fn with_text<'x>(&'x mut self, text: &'x str, base: usize) -> ReadCx<'x> {
        ReadCx {
            parts: self.parts.with_text(text, base),
            tables: self.tables,
            scratch: self.scratch,
            escaping: self.escaping,
            version: self.version,
            as_written: self.as_written,
        }
    }

    /// The id of identifier `text`, when one was read.
    pub(crate) fn find_xref(&self, text: &str) -> Option<XrefId> {
        self.parts.find_xref(text)
    }

    /// The payload's characters (see [`NodeRef::payload_str`]).
    pub(crate) fn payload_str<'a>(&self, node: NodeRef<'a>) -> Cow<'a, str> {
        node.payload_str(self.escaping)
    }

    /// The text payload of a node, as a span of the store (empty when it
    /// has none); `None` for a pointer. A payload reading rewrote is pieces
    /// of the input, not a copy.
    pub(crate) fn span(&mut self, node: NodeRef<'_>) -> Option<Span> {
        let raw = node.raw();
        let base = node.arena.base;
        let start = node.arena.start(raw.payload);
        match raw.kind {
            RawKind::None => Some(Span::EMPTY),
            RawKind::Pointer => None,
            // A date's validity is found once, as it is read.
            RawKind::Text if raw.tag == const { DATE_TAG } => {
                let span = self.parts.span(start, raw.payload.len as usize);
                let date = node.arena.get(raw.payload);
                let bits = if crate::spec::is_simple_date(date, true, false) {
                    SIMPLE_DATE | EXACT_DATE
                } else if crate::spec::is_simple_date(date, false, false) {
                    SIMPLE_DATE
                } else {
                    0
                };
                Some(if bits != 0 && span.kind() == Span::INPUT {
                    span.with_valid(bits)
                } else {
                    span
                })
            }
            RawKind::Text => Some(self.parts.span(start, raw.payload.len as usize)),
            RawKind::Escaped => {
                let at = self.parts.pieces.len();
                let mut fits = true;
                let pieces = &mut *self.parts.pieces;
                unescape_spans(raw.payload.get(node.arena.text), self.escaping, |s, len| {
                    if len > 0 {
                        match (u32::try_from(start + s), u32::try_from(len)) {
                            (Ok(s), Ok(len)) if s.checked_add(len).is_some() => {
                                pieces.push(TextPiece::new(s, len, false));
                            }
                            _ => fits = false,
                        }
                    }
                });
                Some(self.finish_pieces(at, fits, node))
            }
            RawKind::Joined => {
                let at = self.parts.pieces.len();
                let shift = u32::try_from(base).ok();
                let mut fits = shift.is_some();
                for &piece in node.joined(raw) {
                    // A piece of the store's input, from a record rewritten
                    // as it is read, is where it says.
                    let (piece, by) = if piece.start() & Arena::INPUT == 0 {
                        (piece, shift)
                    } else {
                        (piece.unflagged(Arena::INPUT), Some(0))
                    };
                    let end = by.and_then(|b| b.checked_add(piece.start() + piece.len()));
                    fits &= end.is_some();
                    self.parts.pieces.push(piece.shifted(by.unwrap_or(0)));
                }
                Some(self.finish_pieces(at, fits, node))
            }
            // Only a builder outside span mode makes these.
            RawKind::Side => {
                let text = self.payload_str(node);
                let special = crate::writer::special_bytes(&text);
                Some(self.big(text.into_owned().into(), special))
            }
        }
    }

    /// The span of the pieces pushed from `at`; a copy when their offsets
    /// do not fit in 32 bits.
    fn finish_pieces(&mut self, at: usize, fits: bool, node: NodeRef<'_>) -> Span {
        if fits {
            self.parts.pieces_from(at)
        } else {
            self.parts.pieces.truncate(at);
            let text = self.payload_str(node);
            let special = crate::writer::special_bytes(&text);
            self.big(text.into_owned().into(), special)
        }
    }

    /// A text kept whole by the store.
    fn big(&mut self, text: Box<str>, special: u8) -> Span {
        let at = u32::try_from(self.parts.big.len()).unwrap_or(u32::MAX);
        self.parts.big.push(text);
        Span::big(at, special)
    }

    /// The pointer payload of a node, interned; `None` when the payload is
    /// not a pointer.
    pub(crate) fn pointer(&mut self, node: NodeRef<'_>) -> Option<XrefId> {
        let raw = node.raw();
        (raw.kind == RawKind::Pointer)
            .then(|| self.intern(node.arena, raw.payload))
            .flatten()
    }

    /// The identifier of a node, interned.
    pub(crate) fn xref(&mut self, node: NodeRef<'_>) -> Option<XrefId> {
        let span = node.xref_span()?;
        self.intern(node.arena, span)
    }

    fn intern(&mut self, arena: &Arena<'_>, span: RawSpan) -> Option<XrefId> {
        self.parts.xrefs.intern(arena.get(span))
    }

    /// The payload of a node, as an untyped node keeps it: a pointer, a
    /// text when it is not empty, or none.
    fn node_payload(&mut self, node: NodeRef<'_>) -> Option<Span> {
        match self.pointer(node) {
            Some(id) => Some(Span::pointer(id)),
            None if node.is_pointer() => Some(self.span_of_pointer(node)),
            None => self.span(node).filter(|s| !s.is_empty()),
        }
    }

    /// A node and its substructures, untyped, as rows of the table of
    /// nodes.
    pub(crate) fn node_row(&mut self, node: NodeRef<'_>) -> NodeRow {
        let payload = self.node_payload(node);
        let xref = self.xref(node);
        let count = node.children().count();
        let children = if count == 0 {
            List::EMPTY
        } else {
            let start = reserve(&mut self.tables.nodes, count, NodeRow::default());
            for (i, child) in node.children().enumerate() {
                let row = self.node_row(child);
                if let Some(slot) = self.tables.nodes.get_mut(start as usize + i) {
                    *slot = row;
                }
            }
            List::new(start, u32::try_from(count).unwrap_or(u32::MAX))
        };
        NodeRow {
            tag: TagId::raw(node.tag_id()),
            xref,
            payload,
            children,
        }
    }

    fn span_of_pointer(&mut self, node: NodeRef<'_>) -> Span {
        let raw = node.raw();
        self.parts
            .span(node.arena.start(raw.payload), raw.payload.len as usize)
    }

    /// The untyped substructures kept aside from `mark` on, as rows of the
    /// table of nodes, in order.
    fn flush_pending(&mut self, arena: &Arena<'_>, mark: usize) -> List<Node> {
        let n = self.scratch.pending.len().saturating_sub(mark);
        if n == 0 {
            return List::EMPTY;
        }
        let start = reserve(&mut self.tables.nodes, n, NodeRow::default());
        for i in 0..n {
            let Some(&p) = self.scratch.pending.get(mark + i) else {
                continue;
            };
            let row = match p {
                Pending::Node(at) => match NodeRef::at(arena, at) {
                    Some(node) => self.node_row(node),
                    None => NodeRow::default(),
                },
                Pending::Aside {
                    node,
                    xref,
                    payload,
                } => match NodeRef::at(arena, node) {
                    Some(node) => NodeRow {
                        tag: TagId::ASIDE,
                        xref: if xref { self.xref(node) } else { None },
                        payload: if payload {
                            self.node_payload(node)
                        } else {
                            None
                        },
                        children: List::EMPTY,
                    },
                    None => NodeRow::default(),
                },
            };
            if let Some(slot) = self.tables.nodes.get_mut(start as usize + i) {
                *slot = row;
            }
        }
        self.scratch.pending.truncate(mark);
        List::new(start, u32::try_from(n).unwrap_or(u32::MAX))
    }

    /// Reads an enumeration value: a variant `known` finds (its index), or
    /// the text as written. No payload, or a pointer, does not fit.
    pub(crate) fn enum_code(
        &mut self,
        node: NodeRef<'_>,
        known: fn(&str, Option<GedcomVersion>) -> Option<usize>,
    ) -> Option<Code> {
        if node.is_pointer() || node.has_no_payload() {
            return None;
        }
        if let Some(i) = known(&self.payload_str(node), Some(self.version)) {
            return Some(Code::known(i));
        }
        let span = self.span(node)?;
        Some(push_span(&mut self.tables.spans, span))
    }

    /// An unknown enumeration value's text, kept: its code.
    pub(crate) fn unknown_code(&mut self, span: Span) -> Code {
        push_span(&mut self.tables.spans, span)
    }

    /// A part of the payload of `node` as a span: of the input when the
    /// payload is the input's characters as they are, a copy otherwise.
    pub(crate) fn sub_span(&mut self, node: NodeRef<'_>, part: &str) -> Span {
        let within = |text: &str| {
            let (start, end) = (text.as_ptr() as usize, text.as_ptr() as usize + text.len());
            let at = part.as_ptr() as usize;
            (at >= start && at + part.len() <= end).then(|| at - start)
        };
        if node.raw().kind != RawKind::Text {
            return self.owned_span(part);
        }
        if let Some(at) = within(node.arena.text) {
            self.parts.span(node.arena.base + at, part.len())
        } else if let Some(at) = within(node.arena.input) {
            self.parts.span(at, part.len())
        } else {
            self.owned_span(part)
        }
    }

    /// A string copied into the store, while it is read: a big text.
    pub(crate) fn owned_span(&mut self, text: &str) -> Span {
        if text.is_empty() {
            return Span::EMPTY;
        }
        self.big(text.into(), crate::writer::special_bytes(text))
    }
}

/// What reading a structure into a row needs of its type: its fields, and
/// how each takes a substructure. It is dyn-compatible, so that
/// [`read_row_into`] has one copy for every type.
pub(crate) trait RowFields {
    /// The type's fields.
    fn fields(&self) -> &'static [FieldDesc];
    /// The field of each standard tag.
    fn lookup(&self) -> &'static Lookup;
    /// The names of the structure types the type stands for in `version`
    /// ([`SpecNames`]); none for a type that stands for none.
    fn spec_names(&self, _version: GedcomVersion) -> &'static [&'static str] {
        &[]
    }
    /// Reads the tag of a type that keeps it; `false` when the type does
    /// not name it.
    fn read_tag(&mut self, node: NodeRef<'_>) -> bool;
    /// Reads the identifier; `false` when the node has one and the type
    /// has none.
    fn read_xref(&mut self, node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> bool;
    /// Reads the payload; `false` when the node's payload does not fit.
    fn read_payload(&mut self, node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> bool;
    /// Reserves the items of list field `field`.
    fn reserve(&mut self, field: usize, count: u32, cx: &mut ReadCx<'_>);
    /// Gives a substructure to field `field`; `false` when it does not
    /// take it.
    fn accept(
        &mut self,
        field: usize,
        node: NodeRef<'_>,
        reserved: u32,
        cx: &mut ReadCx<'_>,
    ) -> bool;
    /// Gives back the items of list field `field` reserved and not taken.
    fn finish(&mut self, field: usize, reserved: u32, cx: &mut ReadCx<'_>);
}

/// The node a substructure is read as. An extension the conformance repair
/// of the writer made of a standard substructure, that the structure's
/// field takes, is read as that standard structure, so that it fills the
/// field the standard tag would fill: one without a payload (`_INIL` for an
/// `INIL` the version does not permit under an `INDI`, `_SLGC` for an `SLGC`
/// without its `FAMC`), or one whose pointer payload the standard tag takes
/// (`_FAMC @F1@` under an `EVEN`). Any other node, a text payload included,
/// is read as it is, and so is one whose field holds one structure when the
/// parent holds a standard substructure of its tag, which takes the field.
/// `parent`: the structure the substructure is of; `fields` and `lookup`,
/// its row's fields and the field of each standard tag; `spec`:
/// the structure types of the reading structure
/// ([`RowFields::spec_names`]), of which the parent's own tag is the type the
/// substructure is read in.
fn read_tag_of<'a>(
    child: NodeRef<'a>,
    parent: NodeRef<'_>,
    (fields, lookup): (&[FieldDesc], &Lookup),
    spec: &[&str],
    cx: &ReadCx<'_>,
) -> NodeRef<'a> {
    if cx.as_written || (child.tag_id() as usize) < STANDARD_TAGS.len() || spec.is_empty() {
        return child;
    }
    let Some(std) = child
        .name()
        .strip_prefix('_')
        .and_then(crate::tree::standard_index)
    else {
        return child;
    };
    let field = usize::from(lookup.get(usize::from(std)).copied().unwrap_or(0));
    if field == 0 {
        return child;
    }
    // A field holding one structure goes to the standard substructure of
    // its tag, which a read-back extension would otherwise take first.
    if fields.get(field - 1).is_some_and(|f| !f.many)
        && parent
            .children()
            .any(|c| c.tag_id() as usize == usize::from(std))
    {
        return child;
    }
    let nested = child
        .children()
        .any(|c| c.tag_id() as usize >= STANDARD_TAGS.len());
    let version = cx.version;
    let types = crate::spec::conform::type_ids(version, spec, parent.name(), parent.is_pointer());
    let body = if child.is_pointer() {
        crate::spec::conform::ExtBody::Pointer
    } else if child.has_no_payload() {
        crate::spec::conform::ExtBody::None
    } else {
        crate::spec::conform::ExtBody::Text
    };
    let read = crate::spec::conform::read_back(version, &types, child.name(), body, nested, |t| {
        child.children().any(|c| c.standard_tag() == Some(t))
    });
    if read.is_some() {
        child.read_as(std)
    } else {
        child
    }
}

/// Reads a structure into its row ([`RowFields`]): its identifier, tag and
/// payload, then each substructure into the field its tag selects, or
/// aside: the substructures kept untyped, as rows of the table of nodes;
/// `None` when the node does not fit.
///
/// Substructures of one tag keep their order (the first is the preferred
/// one): once one does not fit its field (its shape, or a field already
/// full), the later ones of its tag go to `extra` after it, so that the
/// field's, written first, all came before it.
///
/// A payload or an identifier the type has no place for (text where its
/// pointer belongs, a payload on a record, an identifier on a
/// substructure) does not make the structure untyped: it is kept aside, as
/// the first node of `extra`, tagged [`TagId::ASIDE`], and written back in
/// its place.
///
/// The items of each list are reserved before any is read, so that they
/// are contiguous whatever the substructures they hold put in the same
/// table: the substructures of each tag are counted first.
pub(crate) fn read_row_into(
    s: &mut dyn RowFields,
    node: NodeRef<'_>,
    cx: &mut ReadCx<'_>,
) -> Option<List<Node>> {
    if !s.read_tag(node) {
        return None;
    }
    let xref_fits = s.read_xref(node, cx);
    // No payload where one is expected leaves the type's empty value.
    let payload_fits = s.read_payload(node, cx) || node.has_no_payload();
    let mark = cx.scratch.pending.len();
    if !xref_fits || !payload_fits {
        cx.scratch.pending.push(Pending::Aside {
            node: node.index,
            xref: !xref_fits,
            payload: !payload_fits,
        });
    }
    let fields = s.fields();
    let lookup = s.lookup();
    let counts = cx.scratch.counts.len();
    let lists = cx.scratch.lists.len();
    let spec = s.spec_names(cx.version);
    if node.has_children() {
        cx.scratch.counts.resize(counts + fields.len(), 0);
        for child in node.children() {
            let child = read_tag_of(child, node, (fields, lookup), spec, cx);
            let field = usize::from(lookup.get(child.tag_id() as usize).copied().unwrap_or(0));
            if field > 0
                && fields.get(field - 1).is_some_and(|f| f.many)
                && let Some(c) = cx.scratch.counts.get_mut(counts + field - 1)
            {
                if *c == 0 {
                    cx.scratch
                        .lists
                        .push(u16::try_from(field - 1).unwrap_or(u16::MAX));
                }
                *c += 1;
            }
        }
        for i in lists..cx.scratch.lists.len() {
            let field = usize::from(cx.scratch.lists.get(i).copied().unwrap_or(u16::MAX));
            let count = cx.scratch.counts.get(counts + field).copied().unwrap_or(0);
            s.reserve(field, count, cx);
        }
    }
    let many = cx.scratch.lists.len() > lists;
    // The standard tags already sent aside, as a bit set.
    let mut sent = [0_u64; STANDARD_TAGS.len().div_ceil(64)];
    for child in node.children() {
        let read = read_tag_of(child, node, (fields, lookup), spec, cx);
        let tag = read.tag_id() as usize;
        let field = usize::from(lookup.get(tag).copied().unwrap_or(0));
        let (word, bit) = (tag / 64, 1_u64 << (tag % 64));
        let blocked = sent.get(word).is_some_and(|w| w & bit != 0);
        let reserved = if many && field > 0 {
            cx.scratch
                .counts
                .get(counts + field - 1)
                .copied()
                .unwrap_or(0)
        } else {
            0
        };
        if field == 0 || blocked || !s.accept(field - 1, read, reserved, cx) {
            if let Some(w) = sent.get_mut(word).filter(|_| field != 0) {
                *w |= bit;
            }
            cx.scratch.pending.push(Pending::Node(child.index));
        }
    }
    // Last reserved first: what was not taken at the end of a table is
    // given back.
    for i in (lists..cx.scratch.lists.len()).rev() {
        let field = usize::from(cx.scratch.lists.get(i).copied().unwrap_or(u16::MAX));
        let count = cx.scratch.counts.get(counts + field).copied().unwrap_or(0);
        s.finish(field, count, cx);
    }
    cx.scratch.lists.truncate(lists);
    cx.scratch.counts.truncate(counts);
    Some(cx.flush_pending(node.arena, mark))
}

/// A structure being read into a row: its row, and its detail's row when
/// a field of the detail is read. What the generated [`RowFields`] work
/// on.
pub(crate) struct Building<R, D> {
    pub(crate) row: R,
    pub(crate) detail: Option<D>,
}

/// What writing needs: the store and the tables the model points into
/// and the target version.
#[derive(Clone, Copy)]
pub(crate) struct WriteCx<'s> {
    pub(crate) store: &'s Store,
    /// The tables of the dataset the structures written are of (empty for
    /// owned structures).
    pub(crate) tables: &'s Tables,
    pub(crate) version: GedcomVersion,
    /// Whether values are converted to `version`'s grammars (dates, ages,
    /// times), as for writing a file, or given as the model holds them.
    pub(crate) convert: bool,
    /// The extension structures written as the standard structures a
    /// conversion makes of them, where their structure's type has a field
    /// for them ([`crate::convert::Adoptions`]).
    pub(crate) adopt: Option<&'s crate::convert::Adoptions>,
    /// What an export writes instead of some structures ([`RowRewrite`]),
    /// and the tags of those structures ([`RowRewrite::tags`]).
    pub(crate) rewrite: Option<&'s dyn RowRewrite>,
    pub(crate) rewrite_tags: [Option<u16>; 2],
}

impl<'a> WriteCx<'a> {
    /// A text payload; none when empty.
    pub(crate) fn text(&self, text: &Text) -> Payload {
        if text.is_empty() {
            Payload::None
        } else {
            Payload::Text(text.to_str(self.store).into())
        }
    }

    /// A pointer payload.
    pub(crate) fn pointer(&self, id: XrefId) -> Payload {
        Payload::Pointer(Xref::new(self.store.xref(id)))
    }

    /// [`WriteCx::text`], borrowed.
    pub(crate) fn text_flat<'s>(&self, text: &'s Text) -> FlatPayload<'s>
    where
        'a: 's,
    {
        if text.is_empty() {
            FlatPayload::None
        } else {
            FlatPayload::Text(text.to_str(self.store), text.special())
        }
    }

    /// [`WriteCx::pointer`], borrowed.
    pub(crate) fn pointer_flat(&self, id: XrefId) -> FlatPayload<'a> {
        FlatPayload::Pointer(Cow::Borrowed(self.store.xref(id)), Some(id))
    }

    /// [`WriteCx::str`] of static characters, borrowed.
    pub(crate) fn static_flat(s: &'static str) -> FlatPayload<'static> {
        if s.is_empty() {
            FlatPayload::None
        } else {
            FlatPayload::Text(Cow::Borrowed(s), crate::tree::UNKNOWN)
        }
    }

    /// A span's payload, borrowed: none when empty, a pointer when it
    /// holds one.
    #[inline]
    pub(crate) fn span_flat(&self, s: Span) -> FlatPayload<'a> {
        match s.as_pointer() {
            Some(id) => self.pointer_flat(id),
            None if s.is_empty() => FlatPayload::None,
            None => FlatPayload::Text(self.store.span_str(s), s.special()),
        }
    }

    /// A context for the structures of `data`.
    pub(crate) fn of(data: &'a super::Dataset, version: GedcomVersion, convert: bool) -> Self {
        Self {
            store: &data.store,
            tables: &data.tables,
            version,
            convert,
            adopt: None,
            rewrite: None,
            rewrite_tags: [None; 2],
        }
    }

    /// A context for owned structures whose texts point into `store`.
    pub(crate) fn owned(store: &'a Store, version: GedcomVersion, convert: bool) -> Self {
        Self {
            store,
            tables: &EMPTY_TABLES,
            version,
            convert,
            adopt: None,
            rewrite: None,
            rewrite_tags: [None; 2],
        }
    }

    /// A text payload from static or computed characters.
    pub(crate) fn str(s: &str) -> Payload {
        if s.is_empty() {
            Payload::None
        } else {
            Payload::Text(s.into())
        }
    }
}

/// What a `[convert = …]` function of [`gedcom_struct!`] makes of a value
/// for the target version.
pub(crate) enum Converted<T> {
    /// Written as it is: valid in the target's grammar as this kind of
    /// payload when known ([`PayloadField::claim`]).
    AsIs(Option<Kind>),
    /// Written as this value instead.
    Into(T),
}

impl<T> Converted<T> {
    /// The value to write instead, if any.
    pub(crate) fn into_value(self) -> Option<T> {
        match self {
            Converted::Into(v) => Some(v),
            Converted::AsIs(_) => None,
        }
    }
}

/// A type written as one structure.
pub(crate) trait ToNodes {
    /// Whether the type is a leaf: a payload without substructures (and
    /// without `extra`, so that a node with substructures does not fit).
    const LEAF: bool = false;

    /// The structure, tagged `tag` unless the type chooses its own tag.
    fn to_node(&self, tag: &'static str, cx: &WriteCx<'_>) -> Structure;

    /// The same structure, written into `out` with its texts borrowed from
    /// the model and its source where they can be: what the writer checks
    /// and emits. By default, [`ToNodes::to_node`]'s structure.
    fn to_flat<'s>(&'s self, tag: StdTag, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        out.push_structure(self.to_node(tag.name, cx));
    }

    /// Calls `f` with this value and its tag when it is a typed structure
    /// (leaves are not).
    fn visit(&self, tag: &'static str, f: &mut dyn FnMut(&'static str, &dyn Fields)) {
        let _ = (tag, f);
    }

    /// Whether [`ToNodes::write_extra`] may convert an untyped node.
    const CONVERTS: bool = false;

    /// An untyped node of this type's tag, kept in `extra` (a repeated
    /// singleton, or one after it), written as this type converts its
    /// values for the target version; `None`: as it is.
    fn write_extra(node: &Node, tag: &'static str, cx: &WriteCx<'_>) -> Option<Structure>
    where
        Self: Sized,
    {
        let _ = (node, tag, cx);
        None
    }
}

/// The payload of a structure.
pub(crate) trait PayloadField: Sized {
    /// The payload to write.
    fn write(&self, cx: &WriteCx<'_>) -> Payload;

    /// The same payload, borrowed where it can be. By default,
    /// [`PayloadField::write`]'s.
    fn payload<'s>(&'s self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        self.write(cx).into()
    }

    /// The kind of payload [`PayloadField::payload`] is valid for by
    /// construction (a known enumeration value, a number), which the
    /// conformance check then trusts ([`Flat::open_typed`]); `None` for a
    /// value that may be invalid (a text). By default, none.
    fn claim(&self, cx: &WriteCx<'_>) -> Option<Kind> {
        let _ = cx;
        None
    }

    /// How a row keeps the payload.
    type Raw: Copy + Default + PartialEq + 'static;
    /// What a view gives of it.
    type View<'a>;
    /// Reads the payload into a row; `None` when it does not fit.
    fn read_raw(node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> Option<Self::Raw>;
    /// The payload of a row, to write, borrowed where it can be.
    fn payload_raw<'s>(raw: &Self::Raw, cx: &WriteCx<'s>) -> FlatPayload<'s>;
    /// [`PayloadField::claim`] of a row's payload.
    fn claim_raw(raw: &Self::Raw, cx: &WriteCx<'_>) -> Option<Kind> {
        let _ = (raw, cx);
        None
    }
    /// A row's payload, owned.
    fn load_raw(raw: &Self::Raw, t: &Tables) -> Self;
    /// The owned payload, for a row.
    fn store_raw(&self, cx: &mut StoreCx<'_>) -> Self::Raw;
    /// Calls `f` with the payload's pointer.
    fn each_raw(raw: &Self::Raw, f: &mut dyn FnMut(XrefId)) {
        let _ = (raw, f);
    }
    /// The payload, copied between tables.
    fn copy_raw(raw: &Self::Raw, cx: &mut CopyCx<'_>) -> Self::Raw;
    /// The view of the payload.
    fn view_raw(raw: Self::Raw, data: &super::Dataset) -> Self::View<'_>;
    /// Whether the payload is a text ([`PayloadField::raw_text`]).
    const TEXT: bool = false;
    /// A row's payload, when it is a text.
    fn raw_text(raw: &Self::Raw) -> Option<Span> {
        let _ = raw;
        None
    }
}

/// A table of leaf values of one kind: where lists of them are kept.
pub(crate) trait RawTable: Copy + Default + 'static {
    fn table(t: &Tables) -> &Vec<Self>;
    fn table_mut(t: &mut Tables) -> &mut Vec<Self>;
}

impl RawTable for Span {
    fn table(t: &Tables) -> &Vec<Self> {
        &t.spans
    }
    fn table_mut(t: &mut Tables) -> &mut Vec<Self> {
        &mut t.spans
    }
}

impl RawTable for Option<XrefId> {
    fn table(t: &Tables) -> &Vec<Self> {
        &t.xrefs
    }
    fn table_mut(t: &mut Tables) -> &mut Vec<Self> {
        &mut t.xrefs
    }
}

impl RawTable for Option<Code> {
    fn table(t: &Tables) -> &Vec<Self> {
        &t.codes
    }
    fn table_mut(t: &mut Tables) -> &mut Vec<Self> {
        &mut t.codes
    }
}

impl RawTable for u32 {
    fn table(t: &Tables) -> &Vec<Self> {
        &t.numbers
    }
    fn table_mut(t: &mut Tables) -> &mut Vec<Self> {
        &mut t.numbers
    }
}

impl RawTable for List<Code> {
    fn table(t: &Tables) -> &Vec<Self> {
        &t.code_lists
    }
    fn table_mut(t: &mut Tables) -> &mut Vec<Self> {
        &mut t.code_lists
    }
}

impl PayloadField for Text {
    fn write(&self, cx: &WriteCx<'_>) -> Payload {
        cx.text(self)
    }

    fn payload<'s>(&'s self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        cx.text_flat(self)
    }

    type Raw = Span;
    type View<'a> = TextRef<'a>;
    const TEXT: bool = true;

    fn raw_text(raw: &Span) -> Option<Span> {
        Some(*raw)
    }

    #[inline]
    fn read_raw(node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> Option<Span> {
        cx.span(node)
    }

    #[inline]
    fn payload_raw<'s>(raw: &Span, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        cx.span_flat(*raw)
    }

    fn load_raw(raw: &Span, _t: &Tables) -> Self {
        Text::from_span(*raw)
    }

    fn store_raw(&self, cx: &mut StoreCx<'_>) -> Span {
        self.to_span(cx.store)
    }

    fn copy_raw(raw: &Span, cx: &mut CopyCx<'_>) -> Span {
        cx.span(*raw)
    }

    #[inline]
    fn view_raw(raw: Span, data: &super::Dataset) -> TextRef<'_> {
        TextRef::new(&data.store, raw)
    }
}

impl PayloadField for Option<XrefId> {
    fn write(&self, cx: &WriteCx<'_>) -> Payload {
        self.map_or(Payload::None, |id| cx.pointer(id))
    }

    fn payload<'s>(&'s self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        self.map_or(FlatPayload::None, |id| cx.pointer_flat(id))
    }

    type Raw = Option<XrefId>;
    type View<'a> = Option<XrefId>;

    #[inline]
    fn read_raw(node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> Option<Self::Raw> {
        if node.has_no_payload() {
            Some(None)
        } else {
            cx.pointer(node).map(Some)
        }
    }

    #[inline]
    fn payload_raw<'s>(raw: &Self::Raw, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        raw.map_or(FlatPayload::None, |id| cx.pointer_flat(id))
    }

    fn load_raw(raw: &Self::Raw, _t: &Tables) -> Self {
        *raw
    }

    fn store_raw(&self, _cx: &mut StoreCx<'_>) -> Self::Raw {
        *self
    }

    fn each_raw(raw: &Self::Raw, f: &mut dyn FnMut(XrefId)) {
        if let Some(id) = raw {
            f(*id);
        }
    }

    fn copy_raw(raw: &Self::Raw, cx: &mut CopyCx<'_>) -> Self::Raw {
        raw.map(|id| cx.xref(id))
    }

    fn view_raw(raw: Self::Raw, _data: &super::Dataset) -> Self::View<'_> {
        raw
    }
}

/// The tag id of `DATE` ([`NodeRef::tag_id`]).
const DATE_TAG: u32 = match crate::tree::standard_index_const("DATE") {
    Some(i) => i as u32,
    None => u32::MAX,
};

/// A tag that a structure keeps as a field (`@tag`): the structure stands
/// for several tags (`BIRT`, `DEAT`, … of an event), read from its node
/// and written back.
pub(crate) trait TagField: Sized {
    /// The value of a standard tag; `None` for a tag the type does not
    /// name.
    fn from_tag(tag: &'static str) -> Option<Self>;
    /// The tag to write.
    fn tag(&self) -> &'static str;
    /// The same, with its index among the standard tags.
    fn std_tag(&self) -> StdTag {
        StdTag::of(self.tag())
    }

    /// How a row keeps the tag.
    type Raw: Copy + Default + PartialEq + 'static;
    /// The row's value of a standard tag; `None` for a tag the type does
    /// not name.
    fn raw_of_tag(tag: &'static str) -> Option<Self::Raw>;
    /// The tag a row writes.
    fn std_tag_raw(raw: &Self::Raw, t: &Tables) -> StdTag;
    /// A row's tag, owned.
    fn load_raw(raw: &Self::Raw, t: &Tables) -> Self;
    /// The owned tag, for a row.
    fn store_raw(&self, cx: &mut StoreCx<'_>) -> Self::Raw;
    /// The tag, copied between tables.
    fn copy_raw(raw: &Self::Raw, cx: &mut CopyCx<'_>) -> Self::Raw;
}

/// A standard tag a typed structure is written with, and its index among
/// the standard tags ([`crate::tree::standard_index`]), found once: in a
/// constant for the tags of fields.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StdTag {
    /// The tag.
    pub(crate) name: &'static str,
    /// Its index among the standard tags.
    pub(crate) index: Option<u16>,
}

impl StdTag {
    /// A tag, its index found by a search (for a constant).
    pub(crate) const fn new(name: &'static str) -> Self {
        Self {
            name,
            index: crate::tree::standard_index_const(name),
        }
    }

    /// A tag known at run time.
    pub(crate) fn of(name: &'static str) -> Self {
        Self {
            name,
            index: crate::tree::standard_index(name),
        }
    }
}

/// Declares a fieldless enumeration of tags, for a [`TagField`]:
/// `Variant = "TAG"`.
macro_rules! tag_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(#[$fmeta:meta])* $first:ident = $ftag:literal,
            $($(#[$vmeta:meta])* $variant:ident = $tag:literal,)*
        }
    ) => {
        $(#[$meta])*
        #[non_exhaustive]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        pub enum $name {
            $(#[$fmeta])*
            #[default]
            $first,
            $($(#[$vmeta])* $variant,)*
        }

        impl $name {
            /// The tag.
            #[must_use]
            pub const fn tag(self) -> &'static str {
                match self {
                    Self::$first => $ftag,
                    $(Self::$variant => $tag,)*
                }
            }
        }

        $crate::model::relocate::no_pointers!($name);

        #[cfg(feature = "serde")]
        impl $crate::model::serde::SerializeIn for $name {
            fn serialize_in<S: ::serde::Serializer>(
                &self,
                _store: &$crate::model::Store,
                s: S,
            ) -> Result<S::Ok, S::Error> {
                match self {
                    Self::$first => s.serialize_str(stringify!($first)),
                    $(Self::$variant => s.serialize_str(stringify!($variant)),)*
                }
            }
        }

        #[cfg(feature = "serde")]
        impl $crate::model::serde::DeserializeIn for $name {
            fn deserialize_in<'de, D: ::serde::Deserializer<'de>>(
                _store: &mut $crate::model::Store,
                d: D,
            ) -> Result<Self, D::Error> {
                const NAMES: &[&str] = &[stringify!($first), $(stringify!($variant)),*];
                let name = <$crate::model::serde::Str<'de> as ::serde::Deserialize>::deserialize(d)?;
                match &*name.0 {
                    stringify!($first) => Ok(Self::$first),
                    $(stringify!($variant) => Ok(Self::$variant),)*
                    other => Err(::serde::de::Error::unknown_variant(other, NAMES)),
                }
            }
        }

        impl $crate::model::driver::TagField for $name {
            fn from_tag(tag: &'static str) -> Option<Self> {
                match tag {
                    $ftag => Some(Self::$first),
                    $($tag => Some(Self::$variant),)*
                    _ => None,
                }
            }

            fn tag(&self) -> &'static str {
                (*self).tag()
            }

            fn std_tag(&self) -> $crate::model::driver::StdTag {
                match self {
                    Self::$first => const { $crate::model::driver::StdTag::new($ftag) },
                    $(Self::$variant => const { $crate::model::driver::StdTag::new($tag) },)*
                }
            }

            type Raw = Self;

            fn raw_of_tag(tag: &'static str) -> Option<Self> {
                <Self as $crate::model::driver::TagField>::from_tag(tag)
            }

            fn std_tag_raw(raw: &Self, _t: &$crate::model::table::Tables) -> $crate::model::driver::StdTag {
                $crate::model::driver::TagField::std_tag(raw)
            }

            fn load_raw(raw: &Self, _t: &$crate::model::table::Tables) -> Self {
                *raw
            }

            fn store_raw(&self, _cx: &mut $crate::model::table::StoreCx<'_>) -> Self {
                *self
            }

            fn copy_raw(raw: &Self, _cx: &mut $crate::model::table::CopyCx<'_>) -> Self {
                *raw
            }
        }
    };
}
pub(crate) use tag_enum;

/// A field: how many occurrences of its substructure it holds.
pub(crate) trait Slot {
    /// Whether the field holds any number of occurrences.
    const MANY: bool;
    /// Whether its substructure is a leaf ([`ToNodes::LEAF`]).
    const LEAF: bool;
    /// Writes the occurrences.
    fn write(&self, tag: &'static str, cx: &WriteCx<'_>, out: &mut Vec<Structure>);
    /// Writes the occurrences into a flat arena ([`ToNodes::to_flat`]).
    fn to_flat<'s>(&'s self, tag: StdTag, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        let mut owned = Vec::new();
        self.write(tag.name, cx, &mut owned);
        for s in owned {
            out.push_structure(s);
        }
    }
    /// Calls `f` with each occurrence that is a typed structure.
    fn visit(&self, tag: &'static str, f: &mut dyn FnMut(&'static str, &dyn Fields));
    /// An untyped node of the field's tag, as its type writes it
    /// ([`ToNodes::write_extra`]).
    fn write_extra(node: &Node, tag: &'static str, cx: &WriteCx<'_>) -> Option<Structure>;
    /// Whether [`Slot::write_extra`] may convert an untyped node.
    const CONVERTS: bool;
    /// [`Slot::write_extra`] of an untyped node of a dataset's tables:
    /// copied out only for a type that converts.
    fn write_extra_row(node: &NodeRow, tag: &'static str, cx: &WriteCx<'_>) -> Option<Structure> {
        if Self::CONVERTS {
            Self::write_extra(&super::node::load_node(node, cx.tables), tag, cx)
        } else {
            None
        }
    }
}

impl<T: ToNodes> Slot for Option<T> {
    const MANY: bool = false;
    const LEAF: bool = T::LEAF;
    const CONVERTS: bool = T::CONVERTS;

    fn write(&self, tag: &'static str, cx: &WriteCx<'_>, out: &mut Vec<Structure>) {
        if let Some(v) = self {
            out.push(v.to_node(tag, cx));
        }
    }

    fn to_flat<'s>(&'s self, tag: StdTag, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        if let Some(v) = self {
            v.to_flat(tag, cx, out);
        }
    }

    fn write_extra(node: &Node, tag: &'static str, cx: &WriteCx<'_>) -> Option<Structure> {
        T::write_extra(node, tag, cx)
    }

    fn visit(&self, tag: &'static str, f: &mut dyn FnMut(&'static str, &dyn Fields)) {
        if let Some(v) = self {
            v.visit(tag, f);
        }
    }
}

impl<T: ToNodes> Slot for Vec<T> {
    const MANY: bool = true;
    const LEAF: bool = T::LEAF;
    const CONVERTS: bool = T::CONVERTS;

    fn write(&self, tag: &'static str, cx: &WriteCx<'_>, out: &mut Vec<Structure>) {
        for v in self {
            out.push(v.to_node(tag, cx));
        }
    }

    fn to_flat<'s>(&'s self, tag: StdTag, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        for (i, v) in self.iter().enumerate() {
            out.repeat(i > 0);
            v.to_flat(tag, cx, out);
        }
    }

    fn write_extra(node: &Node, tag: &'static str, cx: &WriteCx<'_>) -> Option<Structure> {
        T::write_extra(node, tag, cx)
    }

    fn visit(&self, tag: &'static str, f: &mut dyn FnMut(&'static str, &dyn Fields)) {
        for v in self {
            v.visit(tag, f);
        }
    }
}

impl<T: ToNodes> Slot for ThinVec<T> {
    const MANY: bool = true;
    const LEAF: bool = T::LEAF;
    const CONVERTS: bool = T::CONVERTS;

    fn write(&self, tag: &'static str, cx: &WriteCx<'_>, out: &mut Vec<Structure>) {
        for v in self {
            out.push(v.to_node(tag, cx));
        }
    }

    fn to_flat<'s>(&'s self, tag: StdTag, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        for (i, v) in self.iter().enumerate() {
            out.repeat(i > 0);
            v.to_flat(tag, cx, out);
        }
    }

    fn write_extra(node: &Node, tag: &'static str, cx: &WriteCx<'_>) -> Option<Structure> {
        T::write_extra(node, tag, cx)
    }

    fn visit(&self, tag: &'static str, f: &mut dyn FnMut(&'static str, &dyn Fields)) {
        for v in self {
            v.visit(tag, f);
        }
    }
}

impl<T: ToNodes> ToNodes for Box<T> {
    const LEAF: bool = T::LEAF;
    const CONVERTS: bool = T::CONVERTS;

    fn to_node(&self, tag: &'static str, cx: &WriteCx<'_>) -> Structure {
        (**self).to_node(tag, cx)
    }

    fn to_flat<'s>(&'s self, tag: StdTag, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        (**self).to_flat(tag, cx, out);
    }

    fn write_extra(node: &Node, tag: &'static str, cx: &WriteCx<'_>) -> Option<Structure> {
        T::write_extra(node, tag, cx)
    }

    fn visit(&self, tag: &'static str, f: &mut dyn FnMut(&'static str, &dyn Fields)) {
        (**self).visit(tag, f);
    }
}

/// The empty value of a field, as a constant: the fields of a detail that
/// has not been allocated read as these.
pub(crate) trait ConstDefault {
    /// The empty value.
    const EMPTY: Self;
}

impl<T> ConstDefault for Option<T> {
    const EMPTY: Self = None;
}

impl<T> ConstDefault for Vec<T> {
    const EMPTY: Self = Vec::new();
}

impl<T> ConstDefault for ThinVec<T> {
    const EMPTY: Self = ThinVec::new();
}

/// A leaf's structure.
pub(crate) fn write_leaf<T: PayloadField>(
    value: &T,
    tag: &'static str,
    cx: &WriteCx<'_>,
) -> Structure {
    leaf_structure(tag, value.write(cx))
}

/// A leaf's structure, into a flat arena.
pub(crate) fn write_leaf_flat<'s, T: PayloadField>(
    value: &'s T,
    tag: StdTag,
    cx: &WriteCx<'s>,
    out: &mut Flat<'s>,
) {
    leaf_flat(out, tag, value.payload(cx), value.claim(cx));
}

/// A leaf, into a flat arena: one copy of [`Flat::leaf_typed`] for the
/// leaves of every type, so that the writer's hot code stays small.
#[inline(never)]
pub(crate) fn leaf_flat<'s>(
    out: &mut Flat<'s>,
    tag: StdTag,
    payload: FlatPayload<'s>,
    claim: Option<Kind>,
) {
    out.leaf_typed(tag.name, tag.index, payload, claim);
}

/// A structure with a tag and a payload.
pub(crate) fn leaf_structure(tag: &'static str, payload: Payload) -> Structure {
    Structure {
        tag: Tag::new(tag),
        payload,
        ..Structure::default()
    }
}

/// Implements [`ToNodes`] for leaves.
macro_rules! leaf {
    ($($ty:ty),* $(,)?) => {$(
        impl $crate::model::driver::ToNodes for $ty {
            const LEAF: bool = true;

            fn to_node(
                &self,
                tag: &'static str,
                cx: &$crate::model::driver::WriteCx<'_>,
            ) -> $crate::tree::Structure {
                $crate::model::driver::write_leaf(self, tag, cx)
            }

            fn to_flat<'s>(
                &'s self,
                tag: $crate::model::driver::StdTag,
                cx: &$crate::model::driver::WriteCx<'s>,
                out: &mut $crate::tree::Flat<'s>,
            ) {
                $crate::model::driver::write_leaf_flat(self, tag, cx, out);
            }
        }

    )*};
}
pub(crate) use leaf;

/// Implements [`Item`] for a leaf, from its [`PayloadField`]: a list of it
/// is a run of the table of its rows' values ([`RawTable`]).
macro_rules! leaf_item {
    ([$($gen:tt)*] $ty:ty, $elem:ty, $view:ty) => {
        $crate::model::table::listed!([$($gen)*] $ty, $elem, $view);

        impl<$($gen)*> $crate::model::table::Item for $ty {
            type One = <$ty as $crate::model::driver::LeafOne>::One;
            const TEXT: bool = <$ty as $crate::model::driver::PayloadField>::TEXT;

            fn text_of(e: &Self::Elem) -> Option<$crate::model::text::Span> {
                <$ty as $crate::model::driver::PayloadField>::raw_text(e)
            }

            fn one_text(o: Self::One) -> Option<$crate::model::text::Span> {
                <$ty as $crate::model::driver::PayloadField>::raw_text(
                    &<$ty as $crate::model::driver::LeafOne>::raw(o),
                )
            }

            fn elems(t: &$crate::model::table::Tables) -> &[Self::Elem] {
                <Self::Elem as $crate::model::driver::RawTable>::table(t)
            }

            fn elems_mut(t: &mut $crate::model::table::Tables) -> &mut Vec<Self::Elem> {
                <Self::Elem as $crate::model::driver::RawTable>::table_mut(t)
            }

            fn fill() -> Self::Elem {
                Self::Elem::default()
            }

            #[inline]
            fn read_elem(
                node: $crate::model::driver::NodeRef<'_>,
                _at: u32,
                cx: &mut $crate::model::driver::ReadCx<'_>,
            ) -> Option<Self::Elem> {
                if node.has_children() || node.has_xref() {
                    return None;
                }
                <$ty as $crate::model::driver::PayloadField>::read_raw(node, cx)
            }

            #[inline]
            fn write_elem<'s>(
                e: &Self::Elem,
                _at: u32,
                tag: $crate::model::driver::StdTag,
                cx: &$crate::model::driver::WriteCx<'s>,
                out: &mut $crate::tree::Flat<'s>,
            ) {
                $crate::model::driver::leaf_flat(
                    out,
                    tag,
                    <$ty as $crate::model::driver::PayloadField>::payload_raw(e, cx),
                    <$ty as $crate::model::driver::PayloadField>::claim_raw(e, cx),
                );
            }

            fn load_elem(e: &Self::Elem, _at: u32, t: &$crate::model::table::Tables) -> Self {
                <$ty as $crate::model::driver::PayloadField>::load_raw(e, t)
            }

            fn store_elem(
                &self,
                _at: u32,
                cx: &mut $crate::model::table::StoreCx<'_>,
            ) -> Self::Elem {
                <$ty as $crate::model::driver::PayloadField>::store_raw(self, cx)
            }

            fn each_elem(
                e: &Self::Elem,
                _at: u32,
                _t: &$crate::model::table::Tables,
                f: &mut dyn FnMut($crate::model::XrefId),
            ) {
                <$ty as $crate::model::driver::PayloadField>::each_raw(e, f);
            }

            fn copy_elem(
                e: &Self::Elem,
                _at: u32,
                cx: &mut $crate::model::table::CopyCx<'_>,
                _to: u32,
            ) -> Self::Elem {
                <$ty as $crate::model::driver::PayloadField>::copy_raw(e, cx)
            }

            #[inline]
            fn view_elem<'a>(
                e: &'a Self::Elem,
                _at: u32,
                data: &'a $crate::model::Dataset,
            ) -> Self::View<'a> {
                <$ty as $crate::model::driver::PayloadField>::view_raw(*e, data)
            }

            fn read_one(
                node: $crate::model::driver::NodeRef<'_>,
                cx: &mut $crate::model::driver::ReadCx<'_>,
            ) -> Option<Self::One> {
                Self::read_elem(node, 0, cx).and_then(<$ty as $crate::model::driver::LeafOne>::one)
            }

            fn write_one<'s>(
                o: &Self::One,
                tag: $crate::model::driver::StdTag,
                cx: &$crate::model::driver::WriteCx<'s>,
                out: &mut $crate::tree::Flat<'s>,
            ) {
                let raw = <$ty as $crate::model::driver::LeafOne>::raw(*o);
                Self::write_elem(&raw, 0, tag, cx, out);
            }

            fn load_one(o: Self::One, t: &$crate::model::table::Tables) -> Self {
                Self::load_elem(&<$ty as $crate::model::driver::LeafOne>::raw(o), 0, t)
            }

            fn store_one(&self, cx: &mut $crate::model::table::StoreCx<'_>) -> Self::One {
                <$ty as $crate::model::driver::LeafOne>::store_one(self, cx)
            }

            fn each_one(
                o: Self::One,
                t: &$crate::model::table::Tables,
                f: &mut dyn FnMut($crate::model::XrefId),
            ) {
                Self::each_elem(&<$ty as $crate::model::driver::LeafOne>::raw(o), 0, t, f);
            }

            fn copy_one(o: Self::One, cx: &mut $crate::model::table::CopyCx<'_>) -> Self::One {
                let raw = <$ty as $crate::model::driver::LeafOne>::raw(o);
                <$ty as $crate::model::driver::LeafOne>::one(Self::copy_elem(&raw, 0, cx, 0))
                    .unwrap_or(o)
            }

            #[inline]
            fn view_one(o: Self::One, data: &$crate::model::Dataset) -> Self::View<'_> {
                let raw = <$ty as $crate::model::driver::LeafOne>::raw(o);
                <$ty as $crate::model::driver::PayloadField>::view_raw(raw, data)
            }
        }
    };
}
pub(crate) use leaf_item;

/// How a field holds one occurrence of a leaf ([`Item::One`](super::table::Item::One)):
/// as the leaf's row value, or, when that value has an empty state that an
/// `Option` around it cannot share, as what remains of it (an enumeration
/// value's code, a pointer's id), so that an optional leaf is no larger
/// than the leaf.
pub(crate) trait LeafOne: PayloadField {
    /// What a field holding one occurrence keeps.
    type One: Copy + PartialEq + std::fmt::Debug + 'static;
    /// The occurrence of a row value; `None` for the empty one.
    fn one(raw: Self::Raw) -> Option<Self::One>;
    /// The row value of an occurrence.
    fn raw(one: Self::One) -> Self::Raw;
    /// An owned value, stored as an occurrence.
    fn store_one(&self, cx: &mut StoreCx<'_>) -> Self::One;
}

/// [`LeafOne`] of a leaf kept as its row value.
macro_rules! leaf_one_same {
    ([$($gen:tt)*] $ty:ty) => {
        impl<$($gen)*> $crate::model::driver::LeafOne for $ty {
            type One = <$ty as $crate::model::driver::PayloadField>::Raw;

            #[inline]
            fn one(raw: Self::Raw) -> Option<Self::One> {
                Some(raw)
            }

            #[inline]
            fn raw(one: Self::One) -> Self::Raw {
                one
            }

            fn store_one(&self, cx: &mut $crate::model::table::StoreCx<'_>) -> Self::One {
                <$ty as $crate::model::driver::PayloadField>::store_raw(self, cx)
            }
        }
    };
}
pub(crate) use leaf_one_same;

leaf_one_same!([] Text);
leaf_one_same!([] u32);

impl LeafOne for XrefId {
    type One = XrefId;

    #[inline]
    fn one(raw: Option<XrefId>) -> Option<XrefId> {
        raw
    }

    #[inline]
    fn raw(one: XrefId) -> Option<XrefId> {
        Some(one)
    }

    fn store_one(&self, _cx: &mut StoreCx<'_>) -> XrefId {
        *self
    }
}

/// A pointer leaf (`ANCI @U1@`, `SUBM @U1@`): text in its place does not
/// fit.
impl PayloadField for XrefId {
    fn write(&self, cx: &WriteCx<'_>) -> Payload {
        cx.pointer(*self)
    }

    fn payload<'s>(&'s self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        cx.pointer_flat(*self)
    }

    type Raw = Option<XrefId>;
    type View<'a> = XrefId;

    #[inline]
    fn read_raw(node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> Option<Self::Raw> {
        cx.pointer(node).map(Some)
    }

    #[inline]
    fn payload_raw<'s>(raw: &Self::Raw, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        raw.map_or(FlatPayload::None, |id| cx.pointer_flat(id))
    }

    fn load_raw(raw: &Self::Raw, _t: &Tables) -> Self {
        // Every row's pointer was read or stored as one.
        raw.unwrap_or(XrefId::DANGLING)
    }

    fn store_raw(&self, _cx: &mut StoreCx<'_>) -> Self::Raw {
        Some(*self)
    }

    fn each_raw(raw: &Self::Raw, f: &mut dyn FnMut(XrefId)) {
        if let Some(id) = raw {
            f(*id);
        }
    }

    fn copy_raw(raw: &Self::Raw, cx: &mut CopyCx<'_>) -> Self::Raw {
        raw.map(|id| cx.xref(id))
    }

    fn view_raw(raw: Self::Raw, _data: &super::Dataset) -> Self::View<'_> {
        raw.unwrap_or(XrefId::DANGLING)
    }
}

/// An integer (`HEIGHT 100`): digits only, at most `u32::MAX`; anything
/// else does not fit and is kept as it is, in `extra`.
impl PayloadField for u32 {
    fn write(&self, _cx: &WriteCx<'_>) -> Payload {
        Payload::Text(self.to_string().into())
    }

    fn claim(&self, _cx: &WriteCx<'_>) -> Option<Kind> {
        Some(Kind::Int)
    }

    type Raw = u32;
    type View<'a> = u32;

    fn read_raw(node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> Option<u32> {
        let s = cx.payload_str(node);
        if node.is_pointer() || s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    }

    fn payload_raw<'s>(raw: &u32, _cx: &WriteCx<'s>) -> FlatPayload<'s> {
        FlatPayload::Text(Cow::Owned(raw.to_string()), 0)
    }

    fn claim_raw(_raw: &u32, _cx: &WriteCx<'_>) -> Option<Kind> {
        Some(Kind::Int)
    }

    fn load_raw(raw: &u32, _t: &Tables) -> Self {
        *raw
    }

    fn store_raw(&self, _cx: &mut StoreCx<'_>) -> u32 {
        *self
    }

    fn copy_raw(raw: &u32, _cx: &mut CopyCx<'_>) -> u32 {
        *raw
    }

    fn view_raw(raw: u32, _data: &super::Dataset) -> u32 {
        raw
    }
}

leaf!(Text, XrefId, u32);
leaf_item!([] Text, Span, TextRef<'a>);
leaf_item!([] XrefId, Option<XrefId>, XrefId);
leaf_item!([] u32, u32, u32);

/// The names of the structure types of the specifications a type stands
/// for, per version (7.x: the URI without its prefix; 5.5.1: the context
/// path of the tables).
#[derive(Clone, Copy, Debug)]
pub struct SpecNames {
    /// GEDCOM 5.5.1.
    pub v551: &'static [&'static str],
    /// GEDCOM 7.0.
    pub v70: &'static [&'static str],
    /// GEDCOM 7.1.
    pub v71: &'static [&'static str],
}

impl SpecNames {
    /// The names of the structure types of `version`.
    pub(crate) fn of(&self, version: GedcomVersion) -> &'static [&'static str] {
        match version {
            GedcomVersion::V5_5_1 => self.v551,
            GedcomVersion::V7_0 => self.v70,
            GedcomVersion::V7_1 => self.v71,
        }
    }
}

/// A field of a typed structure.
#[derive(Clone, Copy, Debug)]
pub struct FieldDesc {
    /// The tags it takes; the first is the one written.
    pub tags: &'static [&'static str],
    /// The field's name.
    pub name: &'static str,
    /// Whether it holds any number of occurrences (`Vec`) rather than one
    /// (`Option`).
    pub many: bool,
    /// Whether its substructure is a leaf (a value without substructures).
    pub leaf: bool,
}

/// For each standard tag, the field that takes it (its index plus one), or
/// 0.
pub(crate) type Lookup = [u8; STANDARD_TAGS.len()];

/// The lookup table of a list of fields, at compile time. A field tag that
/// is not standard is a compile error.
// Constant functions can only use `as`; indices are below 256 (checked).
#[allow(clippy::cast_possible_truncation)]
pub(crate) const fn lookup(fields: &[FieldDesc]) -> Lookup {
    assert!(fields.len() < 255, "too many fields");
    let mut table = [0_u8; STANDARD_TAGS.len()];
    let mut f = 0;
    while f < fields.len() {
        let mut t = 0;
        while t < fields[f].tags.len() {
            let index = crate::tree::standard_index_const(fields[f].tags[t]);
            assert!(index.is_some(), "a field tag is not a standard tag");
            if let Some(i) = index {
                table[i as usize] = f as u8 + 1;
            }
            t += 1;
        }
        f += 1;
    }
    table
}

/// What the driver reads and writes of a typed structure. It is
/// dyn-compatible, so that the driver's loops have one copy for every type.
pub(crate) trait Fields {
    /// The type's fields.
    fn fields(&self) -> &'static [FieldDesc];
    /// The field of each standard tag.
    fn lookup(&self) -> &'static Lookup;
    /// The substructures no field holds.
    fn extra(&self) -> &Extra;
    /// The identifier to write.
    fn write_xref(&self, cx: &WriteCx<'_>) -> Option<Xref>;
    /// The payload to write.
    fn write_payload(&self, cx: &WriteCx<'_>) -> Payload;
    /// The fields' structures, in order.
    fn write_fields(&self, cx: &WriteCx<'_>, out: &mut Vec<Structure>);
    /// [`Fields::write_payload`], borrowed where it can be.
    fn write_payload_flat<'s>(&'s self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        self.write_payload(cx).into()
    }
    /// [`Fields::write_fields`], into a flat arena.
    fn write_fields_flat<'s>(&'s self, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        let mut owned = Vec::new();
        self.write_fields(cx, &mut owned);
        for s in owned {
            out.push_structure(s);
        }
    }
    /// [`Fields::write_xref`], borrowed where it can be.
    fn write_xref_flat<'s>(&'s self, cx: &WriteCx<'s>) -> Option<Cow<'s, str>> {
        self.write_xref(cx)
            .map(|x| Cow::Owned(x.as_str().to_string()))
    }
    /// [`PayloadField::claim`] of the payload.
    fn payload_claim(&self, cx: &WriteCx<'_>) -> Option<Kind> {
        let _ = cx;
        None
    }
    /// Calls `f` with every typed substructure held by a field, and its
    /// tag.
    fn walk(&self, f: &mut dyn FnMut(&'static str, &dyn Fields));
    /// An untyped node of field `field`'s tag, as the field's type writes
    /// it ([`ToNodes::write_extra`]).
    fn write_extra(
        &self,
        field: usize,
        node: &Node,
        tag: &'static str,
        cx: &WriteCx<'_>,
    ) -> Option<Structure>;
}

/// A typed structure, as `gedcom_struct!` declares it: its fields and the
/// structure types it stands for.
pub(crate) trait Struct: Fields + Default {
    /// The fields, in declaration (and writing) order.
    const FIELDS: &'static [FieldDesc];
    /// The field of each standard tag.
    const LOOKUP: Lookup;
    /// The structure types of the specifications this type stands for.
    const SPEC: SpecNames;
}

/// [`write`] into a flat arena, the texts borrowed where they can be:
/// typed ([`Flat::open_typed`]), the payload trusted as `claim` (or the
/// type's [`Fields::payload_claim`]) says.
pub(crate) fn write_flat<'s, F: Fields + ?Sized>(
    s: &'s F,
    tag: StdTag,
    cx: &WriteCx<'s>,
    out: &mut Flat<'s>,
    claim: Option<Kind>,
) {
    let mut xref = s.write_xref_flat(cx);
    let mut payload = s.write_payload_flat(cx);
    let mut claim = claim.or_else(|| s.payload_claim(cx));
    let extra = s.extra();
    for node in extra.iter().filter(|n| n.tag == TagId::ASIDE) {
        // What the type has no place for is no typed value.
        out.untrusted();
        claim = None;
        if xref.is_none() {
            xref = node.xref.map(|x| Cow::Borrowed(cx.store.xref(x)));
        }
        match node.payload_flat(cx.store) {
            FlatPayload::None => {}
            p => payload = p,
        }
    }
    let at = out.open_typed(tag.name, tag.index, xref, payload, claim);
    s.write_fields_flat(cx, out);
    let lookup = s.lookup();
    for node in extra.iter().filter(|n| n.tag != TagId::ASIDE) {
        // As in [`write`]: a node a field takes is written as its type
        // writes its values.
        let index = node.tag.get() as usize;
        let field = lookup.get(index).copied().unwrap_or(0);
        let written = match (cx.convert, STANDARD_TAGS.get(index)) {
            (true, Some(std)) if field > 0 => s.write_extra(usize::from(field - 1), node, std, cx),
            _ => None,
        };
        match written {
            Some(w) => out.push_structure(w),
            None => node.to_flat(cx.store, out),
        }
    }
    out.close(at);
}

/// What writing a row needs of its type. It is dyn-compatible, so that
/// [`write_row_flat`] has one copy for every type.
pub(crate) trait RowWrite {
    /// The row, as a trait object.
    fn as_dyn(&self) -> &dyn RowWrite;
    /// The identifier to write.
    fn write_xref<'s>(&self, cx: &WriteCx<'s>) -> Option<Cow<'s, str>>;
    /// The payload to write.
    fn write_payload<'s>(&self, cx: &WriteCx<'s>) -> FlatPayload<'s>;
    /// [`PayloadField::claim`] of the payload.
    fn payload_claim(&self, cx: &WriteCx<'_>) -> Option<Kind>;
    /// The fields' structures, in order.
    fn write_fields<'s>(&self, cx: &WriteCx<'s>, out: &mut Flat<'s>);
    /// The field of each standard tag.
    fn lookup(&self) -> &'static Lookup;
    /// An untyped node of field `field`'s tag, as the field's type writes
    /// it ([`Slot::write_extra_row`]).
    fn write_extra(
        &self,
        field: usize,
        node: &NodeRow,
        tag: &'static str,
        cx: &WriteCx<'_>,
    ) -> Option<Structure>;
    /// The fields of the type ([`Struct::FIELDS`]).
    fn field_descs(&self) -> &'static [FieldDesc];
    /// [`RowWrite::write_fields`], calling `after` with the index of each
    /// field (in [`RowWrite::field_descs`]) once its structures are
    /// written, when the field has none too.
    fn write_fields_with<'s>(
        &self,
        cx: &WriteCx<'s>,
        out: &mut Flat<'s>,
        after: &mut dyn FnMut(usize, &mut Flat<'s>),
    );
    /// The texts of field `field`, when it holds texts
    /// ([`RowSlot::texts`](super::table::RowSlot::texts)): how many it
    /// holds, `f` called with each.
    fn field_texts(&self, field: usize, t: &Tables, f: &mut dyn FnMut(Span)) -> Option<usize>;
    /// Calls `f` with the index of each field (in
    /// [`RowWrite::field_descs`]) that holds something.
    fn holds(&self, t: &Tables, f: &mut dyn FnMut(usize));
    /// The type of the structure.
    fn struct_type(&self) -> std::any::TypeId;
}

/// What an export writes of a structure of a dataset instead of the
/// structure ([`crate::profile::Exporter`]): the folds that rewrite a
/// structure where it stands, made as it is written.
pub(crate) trait RowRewrite {
    /// The standard tags of the structures it may rewrite (by index): the
    /// others are written without a look.
    fn tags(&self) -> [Option<u16>; 2];
    /// A record tagged `tag` is written (again: a record may be written
    /// more than once, its structures rewritten each time).
    fn record(&self, tag: &str);
    /// The text structure `s` (with untyped substructures when `extra`)
    /// is written with instead of its own payload, without its
    /// substructures, and its [`special_bytes`](crate::writer::special_bytes)
    /// ([`crate::tree::UNKNOWN`] when not known); `None`: as it is.
    fn rewrite(&self, s: &dyn RowWrite, extra: bool, cx: &WriteCx<'_>) -> Option<(String, u8)>;
}

/// Writes a row of a dataset's tables into a flat arena, its texts
/// borrowed from the store, as [`write_flat`] writes an owned structure:
/// its identifier and payload (or what is kept aside in their place), its
/// fields in order, then `extra`, its untyped substructures.
pub(crate) fn write_row_flat<'s, R: RowWrite + ?Sized>(
    s: &R,
    extra: List<Node>,
    tag: StdTag,
    cx: &WriteCx<'s>,
    out: &mut Flat<'s>,
    claim: Option<Kind>,
) {
    if cx.rewrite.is_some() && rewritten(s.as_dyn(), extra, tag, cx, out, claim) {
        return;
    }
    // Most structures have no untyped substructure.
    if !extra.is_empty() {
        write_row_extra(s.as_dyn(), extra, tag, cx, out, claim);
        return;
    }
    let xref = s.write_xref(cx);
    let payload = s.write_payload(cx);
    let claim = claim.or_else(|| s.payload_claim(cx));
    let at = out.open_typed(tag.name, tag.index, xref, payload, claim);
    s.write_fields(cx, out);
    out.close(at);
}

/// Writes the structure `s` (with the untyped substructures `extra`), tagged
/// `tag`, as [`WriteCx::rewrite`] rewrites it, when it does: whether it
/// did.
#[inline(never)]
fn rewritten<'s>(
    s: &dyn RowWrite,
    extra: List<Node>,
    tag: StdTag,
    cx: &WriteCx<'s>,
    out: &mut Flat<'s>,
    claim: Option<Kind>,
) -> bool {
    let (Some(r), Some(i)) = (cx.rewrite, tag.index) else {
        return false;
    };
    if !cx.rewrite_tags.contains(&Some(i)) {
        return false;
    }
    let Some((text, special)) = r.rewrite(s, !extra.is_empty(), cx) else {
        return false;
    };
    let payload = if text.is_empty() {
        FlatPayload::None
    } else {
        FlatPayload::Text(Cow::Owned(text), special)
    };
    let at = out.open_typed(tag.name, tag.index, None, payload, claim);
    out.close(at);
    true
}

/// An untyped substructure written as the standard structure it stands for
/// ([`crate::convert::Adoptions`]).
struct Adopted {
    /// Its index among the untyped substructures.
    node: usize,
    /// The field that takes it, and how many structures it holds before.
    field: usize,
    after: usize,
    tag: StdTag,
    text: Span,
}

/// The untyped substructures `nodes` of the structure `s` that `adopt`
/// writes as the standard structures they stand for (`_UID` as `UID`):
/// texts without identifier, substructure or `@`, which a field of `s`
/// holding texts takes.
fn adoptions(
    s: &dyn RowWrite,
    nodes: &[NodeRow],
    adopt: &crate::convert::Adoptions,
    cx: &WriteCx<'_>,
) -> Vec<Adopted> {
    let mut out: Vec<Adopted> = Vec::new();
    for (k, n) in nodes.iter().enumerate() {
        if n.tag == TagId::ASIDE || n.xref.is_some() || !n.children.is_empty() {
            continue;
        }
        let Some(target) = adopt.target(cx.store.tag(n.tag)) else {
            continue;
        };
        let Some(text) = n
            .payload
            .filter(|p| p.as_pointer().is_none() && !p.is_empty())
        else {
            continue;
        };
        let tag = StdTag::of(target);
        let Some(field) = tag
            .index
            .and_then(|i| s.lookup().get(usize::from(i)).copied())
            .filter(|&f| f > 0)
            .map(|f| usize::from(f - 1))
        else {
            continue;
        };
        if !s.field_descs().get(field).is_some_and(|d| d.many && d.leaf) {
            continue;
        }
        if cx.store.span_str(text).contains('@') {
            continue;
        }
        let Some(held) = s.field_texts(field, cx.tables, &mut |_| {}) else {
            continue;
        };
        let before = out.iter().filter(|a| a.field == field).count();
        out.push(Adopted {
            node: k,
            field,
            after: held + before,
            tag,
            text,
        });
    }
    out
}

/// [`write_row_flat`] for a structure with untyped substructures: one
/// copy for every type.
#[inline(never)]
fn write_row_extra<'s>(
    s: &dyn RowWrite,
    extra: List<Node>,
    tag: StdTag,
    cx: &WriteCx<'s>,
    out: &mut Flat<'s>,
    claim: Option<Kind>,
) {
    let mut xref = s.write_xref(cx);
    let mut payload = s.write_payload(cx);
    let mut claim = claim.or_else(|| s.payload_claim(cx));
    let nodes = extra.of(&cx.tables.nodes);
    for node in nodes.iter().filter(|n| n.tag == TagId::ASIDE) {
        // What the type has no place for is no typed value.
        out.untrusted();
        claim = None;
        if xref.is_none() {
            xref = node.xref.map(|x| Cow::Borrowed(cx.store.xref(x)));
        }
        if let Some(p) = node.payload {
            payload = cx.span_flat(p);
        }
    }
    let at = out.open_typed(tag.name, tag.index, xref, payload, claim);
    let adopted = cx
        .adopt
        .map(|a| adoptions(s, nodes, a, cx))
        .unwrap_or_default();
    if adopted.is_empty() {
        s.write_fields(cx, out);
    } else {
        s.write_fields_with(cx, out, &mut |field, out| {
            for a in adopted.iter().filter(|a| a.field == field) {
                out.repeat(a.after > 0);
                <Text as super::table::Item>::write_elem(&a.text, 0, a.tag, cx, out);
            }
        });
    }
    let lookup = s.lookup();
    for (k, node) in nodes.iter().enumerate() {
        if node.tag == TagId::ASIDE || adopted.iter().any(|a| a.node == k) {
            continue;
        }
        // A node a field takes is written as its type writes its values.
        let index = node.tag.get() as usize;
        let field = lookup.get(index).copied().unwrap_or(0);
        let written = match (cx.convert, STANDARD_TAGS.get(index)) {
            (true, Some(std)) if field > 0 => s.write_extra(usize::from(field - 1), node, std, cx),
            _ => None,
        };
        match written {
            Some(w) => out.push_structure(w),
            None => super::node::node_row_flat(node, cx, out),
        }
    }
    out.close(at);
}

/// Writes a typed structure as `tag`: its identifier and payload, its
/// fields in order, then `extra`.
pub(crate) fn write(s: &dyn Fields, tag: &'static str, cx: &WriteCx<'_>) -> Structure {
    let mut substructures = Vec::new();
    s.write_fields(cx, &mut substructures);
    let lookup = s.lookup();
    let mut xref = s.write_xref(cx);
    let mut payload = s.write_payload(cx);
    for node in s.extra() {
        if node.tag == TagId::ASIDE {
            let aside = node.to_structure(cx.store);
            xref = xref.or(aside.xref);
            if aside.payload != Payload::None {
                payload = aside.payload;
            }
            continue;
        }
        // A node a field takes is written as the field's type writes its
        // values (a second date, in the target's grammar), so that what
        // reads back typed writes alike.
        let index = node.tag.get() as usize;
        let field = lookup.get(index).copied().unwrap_or(0);
        let written = match (cx.convert, STANDARD_TAGS.get(index)) {
            (true, Some(std)) if field > 0 => s.write_extra(usize::from(field - 1), node, std, cx),
            _ => None,
        };
        substructures.push(written.unwrap_or_else(|| node.to_structure(cx.store)));
    }
    Structure {
        tag: Tag::new(tag),
        xref,
        payload,
        substructures,
        line: 0,
    }
}

/// Declares a typed structure: the owned struct, its row and its detail's,
/// its field table, how a dataset reads, writes, views, copies and stores
/// its rows ([`RowFields`], [`RowWrite`], [`Item`](super::table::Item)), and its [`ToNodes`]
/// implementation, which calls [`write`].
///
/// ```text
/// gedcom_struct! {
///     /// Docs.
///     pub struct Name {
///         @xref /// Docs. xref;                   // records only
///         @tag /// Docs. kind: TagType;           // a TagField
///         @payload /// Docs. value: Text;         // a PayloadField
///         /// Docs.
///         "TAG" | "ALT" => field: Option<Type>,   // or Vec<Type>, ThinVec<Type>
///         @detail /// Docs. NameDetail {          // boxed, for rare fields
///             /// Docs.
///             "TAG" => field: Option<Type>,
///         }
///     }
///     spec { v551: ["…"], v70: ["…"], v71: ["…"] }
/// }
/// ```
///
/// Every part but the fields is optional. A `[tag = path]` before the
/// fields names a function choosing the written tag
/// (`fn(&Self, &'static str, &WriteCx) -> &'static str`); a
/// `[convert = path, path]` after it, a function giving the structure to
/// write in place of this one for the target version
/// (`fn(&Self, &WriteCx) -> Converted<Self>`: [`Converted::AsIs`] with the
/// kind of payload it is valid as, when it is known valid), and one
/// reading the type from an untyped node (`fn(&Node, &Store) ->
/// Option<Self>`), with which the nodes of its tag kept in `extra` are
/// converted alike.
///
/// A detail is a struct of its own, with every field of the group; the
/// structure holds it as `detail: Option<Box<…>>`, allocated when one of
/// its fields is read or set, and has `detail()` (the detail, or an empty
/// one) and `detail_mut()` (allocating it).
///
/// A `[chain = field: Type]` last says that the detail's `field`, a `Vec`
/// of `Type`, holds structures that hold this type in turn (notes and
/// citations): a list of either is serialized flat
/// ([`serialize_chain`](crate::model::serde::serialize_chain)).
macro_rules! gedcom_struct {
    (
        $(#[$meta:meta])*
        pub struct $name:ident => $row:ident $([tag = $tagfn:path, $rowtagfn:path])? $([convert = $convfn:path, $untypedfn:path, $convrow:path])? $([chain = $link:ident : $other:ty])? {
            $(@xref $(#[$xmeta:meta])* $xfield:ident;)?
            $(@tag $(#[$tmeta:meta])* $tfield:ident : $tty:ty;)?
            $(@payload $(#[$pmeta:meta])* $pfield:ident : $pty:ty;)?
            $(
                $(#[$fmeta:meta])*
                $tag:literal $(| $alt:literal)* => $field:ident : $fty:ty,
            )*
            $(
                @detail $(#[$dmeta:meta])* $dname:ident => $drow:ident {
                    $(
                        $(#[$gmeta:meta])*
                        $gtag:literal $(| $galt:literal)* => $gfield:ident : $gty:ty,
                    )*
                }
            )?
        }
        spec { v551: [$($v551:literal),* $(,)?], v70: [$($v70:literal),* $(,)?], v71: [$($v71:literal),* $(,)?] $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct $name {
            $($(#[$xmeta])* pub $xfield: Option<$crate::model::XrefId>,)?
            $($(#[$tmeta])* pub $tfield: $tty,)?
            $($(#[$pmeta])* pub $pfield: $pty,)?
            $($(#[$fmeta])* pub $field: $fty,)*
            $(
                #[doc = concat!("The fields few structures use (a [`", stringify!($dname), "`]), when one is: see [`", stringify!($name), "::detail`].")]
                pub detail: Option<Box<$dname>>,
            )?
            /// Substructures no field holds: extensions, unknown tags,
            /// repeated singletons and structures of another shape, in
            /// order.
            pub extra: $crate::model::Extra,
        }

        $(
            $(#[$dmeta])*
            #[derive(Clone, Debug, Default, PartialEq)]
            pub struct $dname {
                $($(#[$gmeta])* pub $gfield: $gty,)*
            }

            impl $dname {
                /// A detail with every field empty.
                const EMPTY: Self = Self {
                    $($gfield: <$gty as $crate::model::driver::ConstDefault>::EMPTY,)*
                };
            }

            impl $name {
                /// The fields few structures use: the detail, or an empty
                /// one when none of them is set.
                #[must_use]
                pub fn detail(&self) -> &$dname {
                    static EMPTY: $dname = $dname::EMPTY;
                    self.detail.as_deref().unwrap_or(&EMPTY)
                }

                /// The fields few structures use, to edit: the detail,
                /// allocated when none of them is set yet.
                pub fn detail_mut(&mut self) -> &mut $dname {
                    self.detail.get_or_insert_with(Box::default)
                }
            }
        )?

        impl $crate::model::driver::Struct for $name {
            const FIELDS: &'static [$crate::model::driver::FieldDesc] = &[
                $(
                    $crate::model::driver::FieldDesc {
                        tags: &[$tag $(, $alt)*],
                        name: stringify!($field),
                        many: <$fty as $crate::model::driver::Slot>::MANY,
                        leaf: <$fty as $crate::model::driver::Slot>::LEAF,
                    },
                )*
                $($(
                    $crate::model::driver::FieldDesc {
                        tags: &[$gtag $(, $galt)*],
                        name: stringify!($gfield),
                        many: <$gty as $crate::model::driver::Slot>::MANY,
                        leaf: <$gty as $crate::model::driver::Slot>::LEAF,
                    },
                )*)?
            ];
            const LOOKUP: $crate::model::driver::Lookup =
                $crate::model::driver::lookup(Self::FIELDS);
            const SPEC: $crate::model::driver::SpecNames = $crate::model::driver::SpecNames {
                v551: &[$($v551),*],
                v70: &[$($v70),*],
                v71: &[$($v71),*],
            };
        }

        impl $crate::model::driver::Fields for $name {
            fn fields(&self) -> &'static [$crate::model::driver::FieldDesc] {
                <Self as $crate::model::driver::Struct>::FIELDS
            }

            fn lookup(&self) -> &'static $crate::model::driver::Lookup {
                &<Self as $crate::model::driver::Struct>::LOOKUP
            }

            fn extra(&self) -> &$crate::model::Extra {
                &self.extra
            }

            fn write_xref(
                &self,
                cx: &$crate::model::driver::WriteCx<'_>,
            ) -> Option<$crate::tree::Xref> {
                gedcom_struct!(@write_xref self cx $($xfield)?)
            }

            fn write_payload(&self, cx: &$crate::model::driver::WriteCx<'_>) -> $crate::tree::Payload {
                gedcom_struct!(@write_payload self cx $($pfield $pty)?)
            }

            #[allow(unused_variables)]
            fn write_fields(
                &self,
                cx: &$crate::model::driver::WriteCx<'_>,
                out: &mut Vec<$crate::tree::Structure>,
            ) {
                $($crate::model::driver::Slot::write(&self.$field, $tag, cx, out);)*
                $(
                    if let Some(d) = &self.detail {
                        let d: &$dname = d;
                        $($crate::model::driver::Slot::write(&d.$gfield, $gtag, cx, out);)*
                    }
                )?
            }

            #[allow(unused_assignments, unused_mut, unused_variables)]
            fn write_extra(
                &self,
                field: usize,
                node: &$crate::model::Node,
                tag: &'static str,
                cx: &$crate::model::driver::WriteCx<'_>,
            ) -> Option<$crate::tree::Structure> {
                let mut i = 0_usize;
                $(
                    if field == i {
                        return <$fty as $crate::model::driver::Slot>::write_extra(node, tag, cx);
                    }
                    i += 1;
                )*
                $($(
                    if field == i {
                        return <$gty as $crate::model::driver::Slot>::write_extra(node, tag, cx);
                    }
                    i += 1;
                )*)?
                None
            }

            #[allow(unused_variables)]
            fn walk(&self, f: &mut dyn FnMut(&'static str, &dyn $crate::model::driver::Fields)) {
                $($crate::model::driver::Slot::visit(&self.$field, $tag, f);)*
                $(
                    if let Some(d) = &self.detail {
                        let d: &$dname = d;
                        $($crate::model::driver::Slot::visit(&d.$gfield, $gtag, f);)*
                    }
                )?
            }

            fn write_payload_flat<'s>(
                &'s self,
                cx: &$crate::model::driver::WriteCx<'s>,
            ) -> $crate::tree::FlatPayload<'s> {
                gedcom_struct!(@payload_flat self cx $($pfield $pty)?)
            }

            #[allow(unused_variables)]
            fn write_fields_flat<'s>(
                &'s self,
                cx: &$crate::model::driver::WriteCx<'s>,
                out: &mut $crate::tree::Flat<'s>,
            ) {
                $($crate::model::driver::Slot::to_flat(
                    &self.$field,
                    const { $crate::model::driver::StdTag::new($tag) },
                    cx,
                    out,
                );)*
                $(
                    if let Some(d) = &self.detail {
                        let d: &$dname = d;
                        $($crate::model::driver::Slot::to_flat(
                            &d.$gfield,
                            const { $crate::model::driver::StdTag::new($gtag) },
                            cx,
                            out,
                        );)*
                    }
                )?
            }

            fn write_xref_flat<'s>(
                &'s self,
                cx: &$crate::model::driver::WriteCx<'s>,
            ) -> Option<std::borrow::Cow<'s, str>> {
                gedcom_struct!(@xref_flat self cx $($xfield)?)
            }

            fn payload_claim(
                &self,
                cx: &$crate::model::driver::WriteCx<'_>,
            ) -> Option<$crate::spec::schema::Kind> {
                gedcom_struct!(@payload_claim self cx $($pfield $pty)?)
            }
        }

        impl $crate::model::driver::ToNodes for $name {
            fn to_node(
                &self,
                tag: &'static str,
                cx: &$crate::model::driver::WriteCx<'_>,
            ) -> $crate::tree::Structure {
                // A structure that keeps its tag writes it.
                $(let tag = { let _ = tag; $crate::model::driver::TagField::tag(&self.$tfield) };)?
                $(let tag = $tagfn(self, $crate::model::driver::StdTag::of(tag), cx).name;)?
                $(
                    if cx.convert {
                        if let $crate::model::driver::Converted::Into(converted) = $convfn(self, cx) {
                            return $crate::model::driver::write(&converted, tag, cx);
                        }
                    }
                )?
                $crate::model::driver::write(self, tag, cx)
            }

            fn to_flat<'s>(
                &'s self,
                tag: $crate::model::driver::StdTag,
                cx: &$crate::model::driver::WriteCx<'s>,
                out: &mut $crate::tree::Flat<'s>,
            ) {
                $(let tag = { let _ = tag; $crate::model::driver::TagField::std_tag(&self.$tfield) };)?
                $(let tag = $tagfn(self, tag, cx);)?
                let claim = None;
                $(
                    let claim = if cx.convert {
                        match $convfn(self, cx) {
                            // A converted structure is a temporary: owned.
                            $crate::model::driver::Converted::Into(converted) => {
                                out.push_structure($crate::model::driver::write(&converted, tag.name, cx));
                                return;
                            }
                            $crate::model::driver::Converted::AsIs(kind) => kind,
                        }
                    } else {
                        claim
                    };
                )?
                $crate::model::driver::write_flat(self, tag, cx, out, claim);
            }

            fn visit(
                &self,
                tag: &'static str,
                f: &mut dyn FnMut(&'static str, &dyn $crate::model::driver::Fields),
            ) {
                f(tag, self);
            }

            $(
                const CONVERTS: bool = {
                    let _ = $convfn;
                    true
                };

                fn write_extra(
                    node: &$crate::model::Node,
                    tag: &'static str,
                    cx: &$crate::model::driver::WriteCx<'_>,
                ) -> Option<$crate::tree::Structure> {
                    let value: Self = $untypedfn(node, cx.store)?;
                    let converted = $convfn(&value, cx).into_value()?;
                    Some($crate::model::driver::write(&converted, tag, cx))
                }
            )?
        }

        // The identifier (`$xfield`) names the structure: it is no pointer.
        impl $crate::model::pointers::Pointers for $name {
            fn own(&self) -> Option<$crate::model::XrefId> {
                None $(.or($crate::model::pointers::Pointers::own(&self.$pfield)))?
            }

            #[allow(unused_variables)]
            fn each(&self, f: &mut dyn FnMut($crate::model::XrefId)) {
                $($crate::model::pointers::Pointers::each(&self.$pfield, f);)?
                $($crate::model::pointers::Pointers::each(&self.$field, f);)*
                $(
                    if let Some(d) = &self.detail {
                        let d: &$dname = d;
                        $($crate::model::pointers::Pointers::each(&d.$gfield, f);)*
                    }
                )?
                $crate::model::pointers::Pointers::each(&self.extra, f);
            }

            #[allow(unused_variables)]
            fn map(&mut self, f: &mut dyn FnMut($crate::model::XrefId) -> $crate::model::XrefId) {
                $($crate::model::pointers::Pointers::map(&mut self.$pfield, f);)?
                $($crate::model::pointers::Pointers::map(&mut self.$field, f);)*
                $(
                    if let Some(d) = &mut self.detail {
                        let d: &mut $dname = d;
                        $($crate::model::pointers::Pointers::map(&mut d.$gfield, f);)*
                    }
                )?
                $crate::model::pointers::Pointers::map(&mut self.extra, f);
            }

            // The payload is the structure's own pointer: what holds the
            // structure cuts it.
            #[allow(unused_variables)]
            fn retain(&mut self, cut: &mut $crate::model::pointers::Cut<'_>) {
                $($crate::model::pointers::Pointers::retain(&mut self.$field, cut);)*
                $(
                    if let Some(d) = &mut self.detail {
                        let d: &mut $dname = d;
                        $($crate::model::pointers::Pointers::retain(&mut d.$gfield, cut);)*
                    }
                )?
                $crate::model::pointers::Pointers::retain(&mut self.extra, cut);
            }
        }

        #[cfg(feature = "serde")]
        impl $crate::model::serde::SerializeIn for $name {
            fn serialize_in<S: ::serde::Serializer>(
                &self,
                store: &$crate::model::Store,
                s: S,
            ) -> Result<S::Ok, S::Error> {
                self.serialize_but(store, s, "")
            }

            fn serialize_but<S: ::serde::Serializer>(
                &self,
                store: &$crate::model::Store,
                s: S,
                skip: &str,
            ) -> Result<S::Ok, S::Error> {
                use ::serde::ser::SerializeMap as _;
                use $crate::model::serde::In;
                let mut count = usize::from(!self.extra.is_empty_in());
                $(count += usize::from(!self.$xfield.is_empty_in());)?
                $(count += usize::from(!self.$tfield.is_empty_in());)?
                $(count += usize::from(!self.$pfield.is_empty_in());)?
                $(count += usize::from(stringify!($field) != skip && !self.$field.is_empty_in());)*
                $(
                    {
                        let d: &$dname = self.detail();
                        $(count += usize::from(stringify!($gfield) != skip && !d.$gfield.is_empty_in());)*
                    }
                )?
                let mut map = s.serialize_map(Some(count))?;
                $(
                    if !self.$xfield.is_empty_in() {
                        map.serialize_entry(stringify!($xfield), &In(store, &self.$xfield))?;
                    }
                )?
                $(
                    if !self.$tfield.is_empty_in() {
                        map.serialize_entry(stringify!($tfield), &In(store, &self.$tfield))?;
                    }
                )?
                $(
                    if !self.$pfield.is_empty_in() {
                        map.serialize_entry(stringify!($pfield), &In(store, &self.$pfield))?;
                    }
                )?
                $(
                    if stringify!($field) != skip && !self.$field.is_empty_in() {
                        map.serialize_entry(stringify!($field), &In(store, &self.$field))?;
                    }
                )*
                $(
                    {
                        let d: &$dname = self.detail();
                        $(
                            if stringify!($gfield) != skip && !d.$gfield.is_empty_in() {
                                map.serialize_entry(stringify!($gfield), &In(store, &d.$gfield))?;
                            }
                        )*
                    }
                )?
                if !self.extra.is_empty_in() {
                    map.serialize_entry("extra", &In(store, &self.extra))?;
                }
                map.end()
            }

            $(
                fn serialize_list<S: ::serde::Serializer>(
                    items: &[Self],
                    store: &$crate::model::Store,
                    s: S,
                ) -> Result<S::Ok, S::Error> {
                    let _: ::std::marker::PhantomData<$other> = ::std::marker::PhantomData;
                    $crate::model::serde::serialize_chain(items, store, s)
                }
            )?
        }

        $(
            #[cfg(feature = "serde")]
            impl $crate::model::serde::Chain for $name {
                type Other = $other;
                const LINK: &'static str = stringify!($link);

                fn links(&self) -> &[$other] {
                    &self.detail().$link
                }

                fn links_mut(&mut self) -> &mut Vec<$other> {
                    &mut self.detail_mut().$link
                }
            }
        )?

        #[cfg(feature = "serde")]
        impl $crate::model::serde::DeserializeIn for $name {
            fn deserialize_in<'de, D: ::serde::Deserializer<'de>>(
                store: &mut $crate::model::Store,
                d: D,
            ) -> Result<Self, D::Error> {
                /// The visitor of the fields.
                struct V<'s>(&'s mut $crate::model::Store);
                impl<'de> ::serde::de::Visitor<'de> for V<'_> {
                    type Value = $name;
                    fn expecting(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                        f.write_str(concat!("the fields of ", stringify!($name)))
                    }
                    fn visit_map<A: ::serde::de::MapAccess<'de>>(
                        self,
                        mut map: A,
                    ) -> Result<$name, A::Error> {
                        use $crate::model::serde::Seed;
                        let store = self.0;
                        let mut out = <$name as ::std::default::Default>::default();
                        while let Some(key) = map.next_key::<$crate::model::serde::Str<'de>>()? {
                            match &*key.0 {
                                $(stringify!($xfield) => {
                                    out.$xfield = map.next_value_seed(Seed::new(&mut *store))?;
                                })?
                                $(stringify!($tfield) => {
                                    out.$tfield = map.next_value_seed(Seed::new(&mut *store))?;
                                })?
                                $(stringify!($pfield) => {
                                    out.$pfield = map.next_value_seed(Seed::new(&mut *store))?;
                                })?
                                $(stringify!($field) => {
                                    out.$field = map.next_value_seed(Seed::new(&mut *store))?;
                                })*
                                $($(stringify!($gfield) => {
                                    out.detail_mut().$gfield =
                                        map.next_value_seed(Seed::new(&mut *store))?;
                                })*)?
                                "extra" => {
                                    out.extra = map.next_value_seed(Seed::new(&mut *store))?;
                                }
                                _ => $crate::model::serde::skip(&mut map)?,
                            }
                        }
                        Ok(out)
                    }
                }
                d.deserialize_map(V(store))
            }

            $(
                fn deserialize_list<'de, D: ::serde::Deserializer<'de>>(
                    store: &mut $crate::model::Store,
                    d: D,
                    push: &mut dyn FnMut(Self),
                ) -> Result<(), D::Error> {
                    let _: ::std::marker::PhantomData<$other> = ::std::marker::PhantomData;
                    $crate::model::serde::deserialize_chain(store, d, push)
                }
            )?
        }

        // ---- The row of the type in a dataset's tables -----------------

        #[doc(hidden)]
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        pub struct $row {
            $(pub(crate) $xfield: Option<$crate::model::XrefId>,)?
            $(pub(crate) $tfield: <$tty as $crate::model::driver::TagField>::Raw,)?
            $(pub(crate) $pfield: <$pty as $crate::model::driver::PayloadField>::Raw,)?
            $(pub(crate) $field: <$fty as $crate::model::table::RowSlot>::Stored,)*
            $(pub(crate) detail: Option<$crate::model::table::Idx<$dname>>,)?
            // A record keeps its untyped substructures in its row: records
            // often have some (extension tags); other structures, in a side
            // table of their table.
            $(pub(crate) extra: gedcom_struct!(@extra_ty $xfield),)?
        }

        impl $crate::model::table::sealed::Stored for $name {
            type Row = $row;

            fn empty() -> &'static $row {
                static EMPTY: std::sync::LazyLock<$row> = std::sync::LazyLock::new(Default::default);
                &EMPTY
            }
        }

        impl $row {
            /// The untyped substructures of the structure of this row, the
            /// row `at` of its table.
            #[inline]
            #[allow(unused_variables)]
            pub(crate) fn extra_in(
                &self,
                at: u32,
                t: &$crate::model::table::Tables,
            ) -> $crate::model::table::List<$crate::model::Node> {
                gedcom_struct!(@extra_get self at t $name $($xfield)?)
            }

            /// Gives the structure of this row, the row `at` of its table,
            /// untyped substructures.
            #[inline]
            #[allow(unused_variables)]
            pub(crate) fn set_extra_in(
                &mut self,
                at: u32,
                t: &mut $crate::model::table::Tables,
                extra: $crate::model::table::List<$crate::model::Node>,
            ) {
                gedcom_struct!(@extra_set self at t extra $name $($xfield)?)
            }
        }

        $(
            #[doc(hidden)]
            #[derive(Clone, Copy, Debug, Default, PartialEq)]
            pub struct $drow {
                $(pub(crate) $gfield: <$gty as $crate::model::table::RowSlot>::Stored,)*
            }

            impl $crate::model::table::sealed::Stored for $dname {
                type Row = $drow;

                fn empty() -> &'static $drow {
                    static EMPTY: std::sync::LazyLock<$drow> =
                        std::sync::LazyLock::new(Default::default);
                    &EMPTY
                }
            }

            $crate::model::table::listed!([] $dname, $drow, ());

            impl $crate::model::table::Item for $dname {
                type One = $crate::model::table::Idx<$dname>;

                fn elems(t: &$crate::model::table::Tables) -> &[$drow] {
                    &<$dname as $crate::model::table::HasTable>::table(t).rows
                }
                fn elems_mut(t: &mut $crate::model::table::Tables) -> &mut Vec<$drow> {
                    &mut <$dname as $crate::model::table::HasTable>::table_mut(t).rows
                }
                fn fill() -> $drow {
                    $drow::default()
                }
                fn read_elem(
                    _node: $crate::model::driver::NodeRef<'_>,
                    _at: u32,
                    _cx: &mut $crate::model::driver::ReadCx<'_>,
                ) -> Option<$drow> {
                    None
                }
                fn read_one(
                    _node: $crate::model::driver::NodeRef<'_>,
                    _cx: &mut $crate::model::driver::ReadCx<'_>,
                ) -> Option<Self::One> {
                    None
                }
                fn write_elem<'s>(
                    e: &$drow,
                    _at: u32,
                    _tag: $crate::model::driver::StdTag,
                    cx: &$crate::model::driver::WriteCx<'s>,
                    out: &mut $crate::tree::Flat<'s>,
                ) {
                    $(<$gty as $crate::model::table::RowSlot>::write(
                        &e.$gfield,
                        const { $crate::model::driver::StdTag::new($gtag) },
                        cx,
                        out,
                    );)*
                }
                fn write_one<'s>(
                    o: &Self::One,
                    tag: $crate::model::driver::StdTag,
                    cx: &$crate::model::driver::WriteCx<'s>,
                    out: &mut $crate::tree::Flat<'s>,
                ) {
                    if let Some(e) = Self::elems(cx.tables).get(o.get() as usize) {
                        Self::write_elem(e, o.get(), tag, cx, out);
                    }
                }
                fn load_elem(e: &$drow, _at: u32, t: &$crate::model::table::Tables) -> Self {
                    Self {
                        $($gfield: <$gty as $crate::model::table::RowSlot>::load(&e.$gfield, t),)*
                    }
                }
                fn load_one(o: Self::One, t: &$crate::model::table::Tables) -> Self {
                    Self::elems(t)
                        .get(o.get() as usize)
                        .map(|e| Self::load_elem(e, o.get(), t))
                        .unwrap_or_default()
                }
                fn store_elem(&self, _at: u32, cx: &mut $crate::model::table::StoreCx<'_>) -> $drow {
                    $drow {
                        $($gfield: <$gty as $crate::model::table::RowSlot>::store(&self.$gfield, cx),)*
                    }
                }
                fn store_one(&self, cx: &mut $crate::model::table::StoreCx<'_>) -> Self::One {
                    let e = self.store_elem(0, cx);
                    $crate::model::table::push_row::<$dname>(cx.tables, e)
                }
                fn each_elem(
                    e: &$drow,
                    _at: u32,
                    t: &$crate::model::table::Tables,
                    f: &mut dyn FnMut($crate::model::XrefId),
                ) {
                    $(<$gty as $crate::model::table::RowSlot>::each(&e.$gfield, t, f);)*
                }
                fn each_one(
                    o: Self::One,
                    t: &$crate::model::table::Tables,
                    f: &mut dyn FnMut($crate::model::XrefId),
                ) {
                    if let Some(e) = Self::elems(t).get(o.get() as usize) {
                        Self::each_elem(e, o.get(), t, f);
                    }
                }
                fn copy_elem(
                    e: &$drow,
                    _at: u32,
                    cx: &mut $crate::model::table::CopyCx<'_>,
                    _to: u32,
                ) -> $drow {
                    $drow {
                        $($gfield: <$gty as $crate::model::table::RowSlot>::copy(&e.$gfield, cx),)*
                    }
                }
                fn copy_one(o: Self::One, cx: &mut $crate::model::table::CopyCx<'_>) -> Self::One {
                    let e = Self::elems(cx.from).get(o.get() as usize).copied().unwrap_or_default();
                    let c = Self::copy_elem(&e, o.get(), cx, 0);
                    $crate::model::table::push_row::<$dname>(cx.to, c)
                }
                fn view_elem<'a>(_e: &'a $drow, _at: u32, _data: &'a $crate::model::Dataset) {}
                fn view_one(_o: Self::One, _data: &$crate::model::Dataset) {}
            }
        )?

        impl $crate::model::driver::RowFields
            for $crate::model::driver::Building<$row, gedcom_struct!(@drow $($drow)?)>
        {
            fn fields(&self) -> &'static [$crate::model::driver::FieldDesc] {
                <$name as $crate::model::driver::Struct>::FIELDS
            }

            fn lookup(&self) -> &'static $crate::model::driver::Lookup {
                &<$name as $crate::model::driver::Struct>::LOOKUP
            }

            fn spec_names(&self, version: $crate::GedcomVersion) -> &'static [&'static str] {
                <$name as $crate::model::driver::Struct>::SPEC.of(version)
            }

            fn read_tag(&mut self, node: $crate::model::driver::NodeRef<'_>) -> bool {
                gedcom_struct!(@row_read_tag self node $($tfield $tty)?)
            }

            fn read_xref(
                &mut self,
                node: $crate::model::driver::NodeRef<'_>,
                cx: &mut $crate::model::driver::ReadCx<'_>,
            ) -> bool {
                gedcom_struct!(@row_read_xref self node cx $($xfield)?)
            }

            fn read_payload(
                &mut self,
                node: $crate::model::driver::NodeRef<'_>,
                cx: &mut $crate::model::driver::ReadCx<'_>,
            ) -> bool {
                gedcom_struct!(@row_read_payload self node cx $($pfield $pty)?)
            }

            #[allow(unused_assignments, unused_mut, unused_variables)]
            fn reserve(
                &mut self,
                field: usize,
                count: u32,
                cx: &mut $crate::model::driver::ReadCx<'_>,
            ) {
                let mut i = 0_usize;
                $(
                    if field == i {
                        return <$fty as $crate::model::table::RowSlot>::reserve(&mut self.row.$field, count, cx);
                    }
                    i += 1;
                )*
                $($(
                    if field == i {
                        let d = self.detail.get_or_insert_with(Default::default);
                        return <$gty as $crate::model::table::RowSlot>::reserve(&mut d.$gfield, count, cx);
                    }
                    i += 1;
                )*)?
            }

            #[allow(unused_assignments, unused_mut, unused_variables)]
            #[inline]
            fn accept(
                &mut self,
                field: usize,
                node: $crate::model::driver::NodeRef<'_>,
                reserved: u32,
                cx: &mut $crate::model::driver::ReadCx<'_>,
            ) -> bool {
                let mut i = 0_usize;
                $(
                    if field == i {
                        return <$fty as $crate::model::table::RowSlot>::accept(&mut self.row.$field, node, reserved, cx);
                    }
                    i += 1;
                )*
                $($(
                    if field == i {
                        let fresh = self.detail.is_none();
                        let d = self.detail.get_or_insert_with(Default::default);
                        let taken = <$gty as $crate::model::table::RowSlot>::accept(&mut d.$gfield, node, reserved, cx);
                        if !taken && fresh {
                            self.detail = None;
                        }
                        return taken;
                    }
                    i += 1;
                )*)?
                false
            }

            #[allow(unused_assignments, unused_mut, unused_variables)]
            fn finish(
                &mut self,
                field: usize,
                reserved: u32,
                cx: &mut $crate::model::driver::ReadCx<'_>,
            ) {
                let mut i = 0_usize;
                $(
                    if field == i {
                        return <$fty as $crate::model::table::RowSlot>::finish(&mut self.row.$field, reserved, cx);
                    }
                    i += 1;
                )*
                $($(
                    if field == i {
                        if let Some(d) = &mut self.detail {
                            <$gty as $crate::model::table::RowSlot>::finish(&mut d.$gfield, reserved, cx);
                        }
                        return;
                    }
                    i += 1;
                )*)?
            }
        }

        impl $crate::model::driver::RowWrite for $row {
            fn as_dyn(&self) -> &dyn $crate::model::driver::RowWrite {
                self
            }

            #[inline]
            fn write_xref<'s>(
                &self,
                cx: &$crate::model::driver::WriteCx<'s>,
            ) -> Option<std::borrow::Cow<'s, str>> {
                gedcom_struct!(@row_xref self cx $($xfield)?)
            }

            #[inline]
            fn write_payload<'s>(
                &self,
                cx: &$crate::model::driver::WriteCx<'s>,
            ) -> $crate::tree::FlatPayload<'s> {
                gedcom_struct!(@row_payload self cx $($pfield $pty)?)
            }

            #[inline]
            fn payload_claim(
                &self,
                cx: &$crate::model::driver::WriteCx<'_>,
            ) -> Option<$crate::spec::schema::Kind> {
                gedcom_struct!(@row_claim self cx $($pfield $pty)?)
            }

            #[allow(unused_variables)]
            fn write_fields<'s>(
                &self,
                cx: &$crate::model::driver::WriteCx<'s>,
                out: &mut $crate::tree::Flat<'s>,
            ) {
                $(<$fty as $crate::model::table::RowSlot>::write(
                    &self.$field,
                    const { $crate::model::driver::StdTag::new($tag) },
                    cx,
                    out,
                );)*
                $(
                    if let Some(d) = &self.detail {
                        <$dname as $crate::model::table::Item>::write_one(
                            d,
                            const { $crate::model::driver::StdTag::new("") },
                            cx,
                            out,
                        );
                    }
                )?
            }

            fn lookup(&self) -> &'static $crate::model::driver::Lookup {
                &<$name as $crate::model::driver::Struct>::LOOKUP
            }

            #[allow(unused_assignments, unused_mut, unused_variables)]
            fn write_extra(
                &self,
                field: usize,
                node: &$crate::model::table::NodeRow,
                tag: &'static str,
                cx: &$crate::model::driver::WriteCx<'_>,
            ) -> Option<$crate::tree::Structure> {
                let mut i = 0_usize;
                $(
                    if field == i {
                        return <$fty as $crate::model::driver::Slot>::write_extra_row(node, tag, cx);
                    }
                    i += 1;
                )*
                $($(
                    if field == i {
                        return <$gty as $crate::model::driver::Slot>::write_extra_row(node, tag, cx);
                    }
                    i += 1;
                )*)?
                None
            }

            fn field_descs(&self) -> &'static [$crate::model::driver::FieldDesc] {
                <$name as $crate::model::driver::Struct>::FIELDS
            }

            #[allow(unused_variables, unused_assignments, unused_mut)]
            fn write_fields_with<'s>(
                &self,
                cx: &$crate::model::driver::WriteCx<'s>,
                out: &mut $crate::tree::Flat<'s>,
                after: &mut dyn FnMut(usize, &mut $crate::tree::Flat<'s>),
            ) {
                let mut i = 0_usize;
                $(
                    <$fty as $crate::model::table::RowSlot>::write(
                        &self.$field,
                        const { $crate::model::driver::StdTag::new($tag) },
                        cx,
                        out,
                    );
                    after(i, out);
                    i += 1;
                )*
                $(
                    // An empty detail when there is none: its fields hold
                    // nothing.
                    let d: $drow = self
                        .detail
                        .and_then(|d| {
                            <$dname as $crate::model::table::Item>::elems(cx.tables)
                                .get(d.get() as usize)
                                .copied()
                        })
                        .unwrap_or_default();
                    $(
                        <$gty as $crate::model::table::RowSlot>::write(
                            &d.$gfield,
                            const { $crate::model::driver::StdTag::new($gtag) },
                            cx,
                            out,
                        );
                        after(i, out);
                        i += 1;
                    )*
                )?
            }

            #[allow(unused_variables, unused_assignments, unused_mut)]
            fn field_texts(
                &self,
                field: usize,
                t: &$crate::model::table::Tables,
                f: &mut dyn FnMut($crate::model::text::Span),
            ) -> Option<usize> {
                let mut i = 0_usize;
                $(
                    if field == i {
                        return <$fty as $crate::model::table::RowSlot>::texts(&self.$field, t, f);
                    }
                    i += 1;
                )*
                $(
                    let d: $drow = self
                        .detail
                        .and_then(|d| {
                            <$dname as $crate::model::table::Item>::elems(t)
                                .get(d.get() as usize)
                                .copied()
                        })
                        .unwrap_or_default();
                    $(
                        if field == i {
                            return <$gty as $crate::model::table::RowSlot>::texts(&d.$gfield, t, f);
                        }
                        i += 1;
                    )*
                )?
                None
            }

            fn struct_type(&self) -> std::any::TypeId {
                std::any::TypeId::of::<$name>()
            }

            #[allow(unused_variables, unused_assignments, unused_mut)]
            fn holds(
                &self,
                t: &$crate::model::table::Tables,
                f: &mut dyn FnMut(usize),
            ) {
                let mut i = 0_usize;
                $(
                    if self.$field != Default::default() {
                        f(i);
                    }
                    i += 1;
                )*
                $(
                    if let Some(d) = self.detail.and_then(|d| {
                        <$dname as $crate::model::table::Item>::elems(t)
                            .get(d.get() as usize)
                            .copied()
                    }) {
                        let d: $drow = d;
                        $(
                            if d.$gfield != Default::default() {
                                f(i);
                            }
                            i += 1;
                        )*
                    }
                )?
            }
        }

        $crate::model::table::listed!([] $name, $row, $crate::model::table::View<'a, $name>);

        impl $crate::model::table::Item for $name {
            type One = $crate::model::table::Idx<$name>;

            #[inline]
            fn elems(t: &$crate::model::table::Tables) -> &[$row] {
                &<$name as $crate::model::table::HasTable>::table(t).rows
            }

            #[inline]
            fn elems_mut(t: &mut $crate::model::table::Tables) -> &mut Vec<$row> {
                &mut <$name as $crate::model::table::HasTable>::table_mut(t).rows
            }

            fn fill() -> $row {
                $row::default()
            }

            fn read_elem(
                node: $crate::model::driver::NodeRef<'_>,
                at: u32,
                cx: &mut $crate::model::driver::ReadCx<'_>,
            ) -> Option<$row> {
                let mut b = $crate::model::driver::Building::<$row, gedcom_struct!(@drow $($drow)?)> {
                    row: $row::default(),
                    detail: None,
                };
                let extra = $crate::model::driver::read_row_into(&mut b, node, cx)?;
                if !extra.is_empty() {
                    b.row.set_extra_in(at, cx.tables, extra);
                }
                $(
                    if let Some(d) = b.detail {
                        if d != $drow::default() {
                            b.row.detail = Some($crate::model::table::push_row::<$dname>(cx.tables, d));
                        }
                    }
                )?
                Some(b.row)
            }

            #[inline]
            fn read_one(
                node: $crate::model::driver::NodeRef<'_>,
                cx: &mut $crate::model::driver::ReadCx<'_>,
            ) -> Option<Self::One> {
                $crate::model::table::read_one_row::<$name>(node, cx)
            }

            fn write_elem<'s>(
                e: &$row,
                at: u32,
                tag: $crate::model::driver::StdTag,
                cx: &$crate::model::driver::WriteCx<'s>,
                out: &mut $crate::tree::Flat<'s>,
            ) {
                // A structure that keeps its tag writes it.
                $(let tag = {
                    let _ = tag;
                    <$tty as $crate::model::driver::TagField>::std_tag_raw(&e.$tfield, cx.tables)
                };)?
                $(let tag = $rowtagfn(e, tag, cx);)?
                let claim = None;
                $(
                    let claim = if cx.convert {
                        match $convrow(e, at, cx) {
                            // A converted structure is a temporary: owned.
                            $crate::model::driver::Converted::Into(converted) => {
                                out.push_structure($crate::model::driver::write(&converted, tag.name, cx));
                                return;
                            }
                            $crate::model::driver::Converted::AsIs(kind) => kind,
                        }
                    } else {
                        claim
                    };
                )?
                let extra = e.extra_in(at, cx.tables);
                $crate::model::driver::write_row_flat(e, extra, tag, cx, out, claim);
            }

            #[inline]
            fn write_one<'s>(
                o: &Self::One,
                tag: $crate::model::driver::StdTag,
                cx: &$crate::model::driver::WriteCx<'s>,
                out: &mut $crate::tree::Flat<'s>,
            ) {
                if let Some(e) = Self::elems(cx.tables).get(o.get() as usize) {
                    Self::write_elem(e, o.get(), tag, cx, out);
                }
            }

            fn load_elem(e: &$row, at: u32, t: &$crate::model::table::Tables) -> Self {
                Self {
                    $($xfield: e.$xfield,)?
                    $($tfield: <$tty as $crate::model::driver::TagField>::load_raw(&e.$tfield, t),)?
                    $($pfield: <$pty as $crate::model::driver::PayloadField>::load_raw(&e.$pfield, t),)?
                    $($field: <$fty as $crate::model::table::RowSlot>::load(&e.$field, t),)*
                    $(detail: e.detail.map(|d| Box::new(<$dname as $crate::model::table::Item>::load_one(d, t))),)?
                    extra: $crate::model::node::load_extra(e.extra_in(at, t), t),
                }
            }

            fn load_one(o: Self::One, t: &$crate::model::table::Tables) -> Self {
                Self::elems(t)
                    .get(o.get() as usize)
                    .map(|e| Self::load_elem(e, o.get(), t))
                    .unwrap_or_default()
            }

            fn store_elem(&self, at: u32, cx: &mut $crate::model::table::StoreCx<'_>) -> $row {
                #[allow(unused_mut)]
                let mut row = $row {
                    $($xfield: self.$xfield,)?
                    $($tfield: <$tty as $crate::model::driver::TagField>::store_raw(&self.$tfield, cx),)?
                    $($pfield: <$pty as $crate::model::driver::PayloadField>::store_raw(&self.$pfield, cx),)?
                    $($field: <$fty as $crate::model::table::RowSlot>::store(&self.$field, cx),)*
                    $(detail: None::<$crate::model::table::Idx<$dname>>,)?
                    $(extra: gedcom_struct!(@extra_empty $xfield),)?
                };
                $(
                    if let Some(d) = &self.detail {
                        let d: &$dname = d;
                        if *d != $dname::default() {
                            row.detail = Some(<$dname as $crate::model::table::Item>::store_one(d, cx));
                        }
                    }
                )?
                let extra = $crate::model::node::store_extra(&self.extra, cx);
                if !extra.is_empty() {
                    row.set_extra_in(at, cx.tables, extra);
                }
                row
            }

            fn store_one(&self, cx: &mut $crate::model::table::StoreCx<'_>) -> Self::One {
                $crate::model::table::store_one_row(self, cx)
            }

            #[allow(unused_variables)]
            fn each_elem(
                e: &$row,
                at: u32,
                t: &$crate::model::table::Tables,
                f: &mut dyn FnMut($crate::model::XrefId),
            ) {
                $(<$pty as $crate::model::driver::PayloadField>::each_raw(&e.$pfield, f);)?
                $(<$fty as $crate::model::table::RowSlot>::each(&e.$field, t, f);)*
                $(
                    if let Some(d) = e.detail {
                        <$dname as $crate::model::table::Item>::each_one(d, t, f);
                    }
                )?
                $crate::model::node::each_extra(e.extra_in(at, t), t, f);
            }

            fn each_one(
                o: Self::One,
                t: &$crate::model::table::Tables,
                f: &mut dyn FnMut($crate::model::XrefId),
            ) {
                if let Some(e) = Self::elems(t).get(o.get() as usize) {
                    Self::each_elem(e, o.get(), t, f);
                }
            }

            fn copy_elem(
                e: &$row,
                at: u32,
                cx: &mut $crate::model::table::CopyCx<'_>,
                to: u32,
            ) -> $row {
                #[allow(unused_mut)]
                let mut row = $row {
                    $($xfield: e.$xfield.map(|x| cx.xref(x)),)?
                    $($tfield: <$tty as $crate::model::driver::TagField>::copy_raw(&e.$tfield, cx),)?
                    $($pfield: <$pty as $crate::model::driver::PayloadField>::copy_raw(&e.$pfield, cx),)?
                    $($field: <$fty as $crate::model::table::RowSlot>::copy(&e.$field, cx),)*
                    $(detail: e.detail.map(|d: $crate::model::table::Idx<$dname>| {
                        <$dname as $crate::model::table::Item>::copy_one(d, cx)
                    }),)?
                    $(extra: gedcom_struct!(@extra_empty $xfield),)?
                };
                let extra = e.extra_in(at, cx.from);
                if !extra.is_empty() {
                    let copied = $crate::model::node::copy_extra(extra, cx);
                    row.set_extra_in(to, cx.to, copied);
                }
                row
            }

            fn copy_one(o: Self::One, cx: &mut $crate::model::table::CopyCx<'_>) -> Self::One {
                $crate::model::table::copy_one_row::<$name>(o, cx)
            }

            #[inline]
            fn view_elem<'a>(
                e: &'a $row,
                at: u32,
                data: &'a $crate::model::Dataset,
            ) -> Self::View<'a> {
                $crate::model::table::View::new(data, e, at)
            }

            #[inline]
            fn view_one(o: Self::One, data: &$crate::model::Dataset) -> Self::View<'_> {
                $crate::model::table::view_one_row::<$name>(o, data)
            }
        }

        impl<'a> $crate::model::table::View<'a, $name> {
            $(
                $(#[$xmeta])*
                #[must_use]
                pub fn $xfield(self) -> Option<$crate::model::XrefId> {
                    self.row.$xfield
                }
            )?
            $(
                $(#[$tmeta])*
                #[must_use]
                pub fn $tfield(self) -> $tty {
                    <$tty as $crate::model::driver::TagField>::load_raw(&self.row.$tfield, self.tables())
                }
            )?
            $(
                $(#[$pmeta])*
                #[must_use]
                pub fn $pfield(self) -> $crate::model::FieldView<'a, $pty> {
                    <$pty as $crate::model::driver::PayloadField>::view_raw(self.row.$pfield, self.data)
                }
            )?
            $(
                $(#[$fmeta])*
                #[must_use]
                pub fn $field(self) -> $crate::model::FieldView<'a, $fty> {
                    <$fty as $crate::model::table::RowSlot>::view(self.row.$field, self.data)
                }
            )*
            $(
                /// The row of the detail, when the structure has one.
                fn detail_row(self) -> Option<&'a $drow> {
                    let d = self.row.detail?;
                    <$dname as $crate::model::table::HasTable>::table(self.tables())
                        .rows
                        .get(d.get() as usize)
                }

                $(
                    $(#[$gmeta])*
                    #[must_use]
                    pub fn $gfield(self) -> $crate::model::FieldView<'a, $gty> {
                        <$gty as $crate::model::table::RowSlot>::view(
                            self.detail_row().map(|d| d.$gfield).unwrap_or_default(),
                            self.data,
                        )
                    }
                )*
            )?

            /// The substructures it keeps untyped: extensions, unknown tags,
            /// repeated singletons and structures of another shape, in
            /// order.
            #[must_use]
            pub fn extra(self) -> $crate::model::Items<'a, $crate::model::Node> {
                let list = self.row.extra_in(self.at, self.tables());
                $crate::model::Items::new(self.data, list.of(&self.tables().nodes), list.start)
            }

        }

        /// The structure, copied out.
        impl std::fmt::Debug for $crate::model::table::View<'_, $name> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_tuple("View").field(&self.into_owned()).finish()
            }
        }
    };
    (@extra_ty $xfield:ident) => { $crate::model::table::List<$crate::model::Node> };
    (@extra_empty $xfield:ident) => { $crate::model::table::List::EMPTY };
    (@extra_get $self:ident $at:ident $t:ident $name:ident) => {
        <$name as $crate::model::table::HasTable>::table($t).extra($at)
    };
    (@extra_get $self:ident $at:ident $t:ident $name:ident $xfield:ident) => {
        $self.extra
    };
    (@extra_set $self:ident $at:ident $t:ident $extra:ident $name:ident) => {
        <$name as $crate::model::table::HasTable>::table_mut($t).set_extra($at, $extra)
    };
    (@extra_set $self:ident $at:ident $t:ident $extra:ident $name:ident $xfield:ident) => {
        $self.extra = $extra
    };
    (@drow) => { () };
    (@drow $drow:ident) => { $drow };
    (@row_read_tag $self:ident $node:ident) => {{
        let _ = $node;
        true
    }};
    (@row_read_tag $self:ident $node:ident $tfield:ident $tty:ty) => {
        match $node.standard_tag().and_then(<$tty as $crate::model::driver::TagField>::raw_of_tag) {
            Some(v) => {
                $self.row.$tfield = v;
                true
            }
            None => false,
        }
    };
    (@row_read_xref $self:ident $node:ident $cx:ident) => {{
        let _ = &$cx;
        !$node.has_xref()
    }};
    (@row_read_xref $self:ident $node:ident $cx:ident $xfield:ident) => {{
        $self.row.$xfield = $cx.xref($node);
        true
    }};
    (@row_read_payload $self:ident $node:ident $cx:ident) => {{
        let _ = $cx;
        $node.has_no_payload()
    }};
    (@row_read_payload $self:ident $node:ident $cx:ident $pfield:ident $pty:ty) => {
        match <$pty as $crate::model::driver::PayloadField>::read_raw($node, $cx) {
            Some(v) => {
                $self.row.$pfield = v;
                true
            }
            None => false,
        }
    };
    (@row_xref $self:ident $cx:ident) => {{
        let _ = $cx;
        None
    }};
    (@row_xref $self:ident $cx:ident $xfield:ident) => {
        $self
            .$xfield
            .map(|x| std::borrow::Cow::Borrowed($cx.store.xref(x)))
    };
    (@row_payload $self:ident $cx:ident) => {{
        let _ = $cx;
        $crate::tree::FlatPayload::None
    }};
    (@row_payload $self:ident $cx:ident $pfield:ident $pty:ty) => {
        <$pty as $crate::model::driver::PayloadField>::payload_raw(&$self.$pfield, $cx)
    };
    (@row_claim $self:ident $cx:ident) => {{
        let _ = $cx;
        None
    }};
    (@row_claim $self:ident $cx:ident $pfield:ident $pty:ty) => {
        <$pty as $crate::model::driver::PayloadField>::claim_raw(&$self.$pfield, $cx)
    };
    (@write_xref $self:ident $cx:ident) => {{
        let _ = $cx;
        None
    }};
    (@write_xref $self:ident $cx:ident $xfield:ident) => {
        $self.$xfield.map(|x| $crate::tree::Xref::new($cx.store.xref(x)))
    };
    (@write_payload $self:ident $cx:ident) => {{
        let _ = $cx;
        $crate::tree::Payload::None
    }};
    (@write_payload $self:ident $cx:ident $pfield:ident $pty:ty) => {
        $crate::model::driver::PayloadField::write(&$self.$pfield, $cx)
    };
    (@payload_claim $self:ident $cx:ident) => {{
        let _ = $cx;
        None
    }};
    (@payload_claim $self:ident $cx:ident $pfield:ident $pty:ty) => {
        $crate::model::driver::PayloadField::claim(&$self.$pfield, $cx)
    };
    (@xref_flat $self:ident $cx:ident) => {{
        let _ = $cx;
        None
    }};
    (@xref_flat $self:ident $cx:ident $xfield:ident) => {
        $self
            .$xfield
            .map(|x| std::borrow::Cow::Borrowed($cx.store.xref(x)))
    };
    (@payload_flat $self:ident $cx:ident) => {{
        let _ = $cx;
        $crate::tree::FlatPayload::None
    }};
    (@payload_flat $self:ident $cx:ident $pfield:ident $pty:ty) => {
        $crate::model::driver::PayloadField::payload(&$self.$pfield, $cx)
    };
}
pub(crate) use gedcom_struct;

/// The row side of [`PayloadField`] for a payload that is a pointer or a
/// text (a citation's source, a note's content): one span, which holds the
/// pointer when it is one.
macro_rules! pointer_or_text_raw {
    ($ty:ident, $pointer:ident, $text:ident) => {
        type Raw = $crate::model::text::Span;
        type View<'a> = $ty;

        fn read_raw(
            node: $crate::model::driver::NodeRef<'_>,
            cx: &mut $crate::model::driver::ReadCx<'_>,
        ) -> Option<$crate::model::text::Span> {
            match cx.pointer(node) {
                Some(id) => Some($crate::model::text::Span::pointer(id)),
                None => cx.span(node),
            }
        }

        fn payload_raw<'s>(
            raw: &$crate::model::text::Span,
            cx: &$crate::model::driver::WriteCx<'s>,
        ) -> $crate::tree::FlatPayload<'s> {
            cx.span_flat(*raw)
        }

        fn load_raw(raw: &$crate::model::text::Span, _t: &$crate::model::table::Tables) -> Self {
            match raw.as_pointer() {
                Some(id) => $ty::$pointer(id),
                None => $ty::$text($crate::model::Text::from_span(*raw)),
            }
        }

        fn store_raw(
            &self,
            cx: &mut $crate::model::table::StoreCx<'_>,
        ) -> $crate::model::text::Span {
            match self {
                $ty::$pointer(id) => $crate::model::text::Span::pointer(*id),
                $ty::$text(t) => t.to_span(cx.store),
            }
        }

        fn each_raw(raw: &$crate::model::text::Span, f: &mut dyn FnMut($crate::model::XrefId)) {
            if let Some(id) = raw.as_pointer() {
                f(id);
            }
        }

        fn copy_raw(
            raw: &$crate::model::text::Span,
            cx: &mut $crate::model::table::CopyCx<'_>,
        ) -> $crate::model::text::Span {
            cx.span(*raw)
        }

        fn view_raw(raw: $crate::model::text::Span, data: &$crate::model::Dataset) -> $ty {
            Self::load_raw(&raw, &data.tables)
        }
    };
}
pub(crate) use pointer_or_text_raw;
