//! Reading decoded text into the typed model, one record at a time.
//!
//! Every reader — [`Dataset::parse`], the
//! [`GedcomBuilder`](crate::GedcomBuilder), the
//! [`GedcomStreamParser`](crate::GedcomStreamParser) and GEDZIP archives —
//! lexes records into the arena of [`crate::tree`] and types each one
//! through [`dispatch_record`], the one place that knows which type a
//! level-0 tag stands for, into the tables of a dataset.

use crate::profile::draft::Draft;
use crate::profile::draft::Value;
use crate::profile::interpret::{
    Edit, InArena, InterpretReport, Interpreter, PLACEHOLDER, Rewrite, RuleSet,
};
use crate::tree::{
    Builder, Escaping, PayloadRef, RawKind, RawSpan, TagInterner, TextPiece, head_version,
    segment_bounds,
};
use crate::version::GedcomVersion;

use std::rc::Rc;

use super::dataset::Dataset;
use super::driver::{Arena, NodeRef, ReadCx, Scratch};
use super::edit::RecordKind;
use super::header::Header;
use super::record::{
    Family, Individual, Multimedia, Repository, SharedNote, Source, Submission, Submitter,
};
use super::table::HasTable;
use super::text::{Store, XrefId};

/// The largest text one pass of the lexer indexes: offsets are 32-bit, and
/// a piece's length takes 31 bits.
const SEGMENT_LIMIT: usize = (u32::MAX / 2) as usize;

#[cfg(test)]
thread_local! {
    /// The records of this thread that rules changed where they were read.
    pub(crate) static IN_PLACE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What [`dispatch_record`] tells of the records it types into the
/// tables.
pub(crate) trait RecordSink {
    /// Whether a header is already kept: a second one stays untyped.
    fn has_header(&self) -> bool;
    /// Takes the header.
    fn header(&mut self, header: Header);
    /// A record was placed: the `pos`-th of its kind, with identifier
    /// `xref`.
    fn placed(&mut self, kind: RecordKind, pos: usize, xref: Option<XrefId>);
}

/// Types the record at `node` by its tag into the tables of `cx`, and
/// tells `sink`.
///
/// A record whose tag the model has no type for (an extension or unknown
/// record, a header after the first, a trailer with content), or that does
/// not fit its type (`0 INDI payload`), is kept untyped, whole. An empty
/// trailer carries nothing and is dropped: writers end every file with
/// their own.
pub(crate) fn dispatch_record(node: NodeRef<'_>, cx: &mut ReadCx<'_>, sink: &mut dyn RecordSink) {
    let typed = match node.standard_tag() {
        Some("HEAD") if !sink.has_header() => match read_header(node, cx) {
            Some(header) => {
                sink.header(header);
                true
            }
            None => false,
        },
        Some("INDI") => typed::<Individual>(node, cx, sink),
        Some("FAM") => typed::<Family>(node, cx, sink),
        Some("SOUR") => typed::<Source>(node, cx, sink),
        Some("REPO") => typed::<Repository>(node, cx, sink),
        Some("OBJE") => typed::<Multimedia>(node, cx, sink),
        Some("SUBM") => typed::<Submitter>(node, cx, sink),
        Some("SUBN") => typed::<Submission>(node, cx, sink),
        Some("NOTE" | "SNOTE") => typed::<SharedNote>(node, cx, sink),
        Some("TRLR") if !node.has_children() && node.has_no_payload() && !node.has_xref() => {
            return;
        }
        _ => false,
    };
    if !typed {
        let row = cx.node_row(node);
        let pos = cx.tables.others.rows.len();
        cx.tables.others.rows.push(row);
        sink.placed(RecordKind::Other, pos, row.xref);
    }
}

/// Reads the header, which a dataset keeps as its owned type: read into
/// the tables as any structure, then copied out. Its rows, a few, stay in
/// the tables, held by nothing (a compaction drops them).
fn read_header(node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> Option<Header> {
    super::table::read_one_row::<Header>(node, cx)
        .map(|at| <Header as super::table::Item>::load_one(at, cx.tables))
}

/// Reads a record of type `T` into its table: whether it fits.
fn typed<T>(node: NodeRef<'_>, cx: &mut ReadCx<'_>, sink: &mut dyn RecordSink) -> bool
where
    T: super::edit::RecordRows
        + HasTable
        + super::table::sealed::Listed<Elem = <T as super::table::sealed::Stored>::Row>,
{
    let at = super::table::reserve(&mut T::table_mut(cx.tables).rows, 1, T::fill());
    let Some(row) = T::read_elem(node, at, cx) else {
        let rows = &mut T::table_mut(cx.tables).rows;
        if rows.len() == at as usize + 1 {
            rows.pop();
        } else {
            cx.tables.garbage += 1;
        }
        return false;
    };
    let xref = T::row_xref(&row);
    if let Some(slot) = T::table_mut(cx.tables).rows.get_mut(at as usize) {
        *slot = row;
    }
    sink.placed(T::ROWS_KIND, at as usize, xref);
    true
}

/// A dataset's header and index, which reading fills as it places the
/// records.
struct DatasetSink<'d> {
    header: &'d mut Option<Box<Header>>,
    index: &'d mut super::edit::Index,
}

impl RecordSink for DatasetSink<'_> {
    fn has_header(&self) -> bool {
        self.header.is_some()
    }

    fn header(&mut self, header: Header) {
        *self.header = Some(Box::new(header));
    }

    /// Each record placed is noted in the dataset's index while it is at
    /// hand ([`super::edit::Index::define_read`]).
    fn placed(&mut self, kind: RecordKind, pos: usize, xref: Option<XrefId>) {
        self.index.define_read(kind, pos, xref);
    }
}

/// The version a file declares, and how its text escapes `@`.
fn version_of(declared: Option<&str>) -> (GedcomVersion, Escaping) {
    let version = declared.map_or(GedcomVersion::V5_5_1, GedcomVersion::from_version_str);
    (version, Escaping::of(declared))
}

/// Reads a dataset from decoded text, keeping the text as its store.
pub(crate) fn read_string(text: String) -> Dataset {
    read_segmented(text, SEGMENT_LIMIT, false)
}

/// [`read_string`] of the exporter's own text, read as written
/// ([`ReadCx::as_written`]).
pub(crate) fn read_as_written(text: String) -> Dataset {
    read_segmented(text, SEGMENT_LIMIT, true)
}

/// [`read_string`], lexing at most `limit` bytes at a time; `as_written`
/// as [`read_as_written`].
pub(crate) fn read_segmented(text: String, limit: usize, as_written: bool) -> Dataset {
    let declared = head_version(&text);
    let (version, escaping) = version_of(declared.as_deref());
    let mut data = Dataset::new(version);
    data.declared_version = declared.map(Into::into);
    let mut store = Store::new(text);
    let mut tags = TagInterner::default();
    let mut scratch = Scratch::default();
    {
        let Dataset {
            header,
            tables,
            index,
            ..
        } = &mut data;
        let mut sink = DatasetSink { header, index };
        let parts = store.parts_mut();
        let input = parts.text;
        let mut cx = ReadCx::new(parts, tables, &mut scratch, version);
        cx.as_written = as_written;
        let mut builder = Builder::new(escaping, 1).with_spans();
        for (start, end, first_line) in segment_bounds(input, limit) {
            let part = input.get(start..end).unwrap_or_default();
            builder.reset(first_line);
            builder.read_records(part, &mut tags, |b, tags| {
                let arena = Arena {
                    text: part,
                    base: start,
                    joined: &b.joined,
                    nodes: &b.nodes,
                    xrefs: &b.xrefs,
                    input,
                    side: "",
                    side_base: 0,
                    others: &tags.others,
                };
                if let Some(root) = NodeRef::root(&arena) {
                    dispatch_record(root, &mut cx, &mut sink);
                }
            });
        }
    }
    store.add_seen(tags.seen());
    store.set_tags(tags.others);
    store.shrink();
    // The spare rows of a small file, a few kilobytes at most, are not
    // worth reallocating every table.
    let small = store.input_len() < SMALL_INPUT;
    data.store = store;
    if !small {
        data.shrink();
    }
    data.index_read();
    data.note_read_source();
    data
}

/// The size of input under which a dataset keeps its tables' spare rows.
const SMALL_INPUT: usize = 64 * 1024;

// ---- Reading a producer's conventions --------------------------------------

/// A record read by [`read_interpreting`]: where it was placed, and its
/// rank among the file's records.
#[derive(Clone, Copy, Debug)]
struct Located {
    placed: Placed,
    seq: u32,
}

/// What [`read_interpreting`] keeps of the records it read, for the
/// changes that span records, once every record is read.
#[derive(Default)]
struct Kept {
    /// By identifier (the index of its id): the first record with it, when
    /// some rule changes other records than the one it reads.
    first: Option<Vec<Option<Located>>>,
    /// The records that link to records interpretation adds.
    holders: Vec<Located>,
    /// The records read so far.
    seq: u32,
}

impl Kept {
    /// Notes the records just placed (`placed`, with their identifiers):
    /// the records of one record read.
    fn note(&mut self, placed: &[(Placed, Option<XrefId>)], holder: bool) {
        for &(at, xref) in placed {
            let located = Located {
                placed: at,
                seq: self.seq,
            };
            if let (Some(first), Some(id)) = (&mut self.first, xref) {
                if first.len() <= id.index() {
                    first.resize(id.index() + 1, None);
                }
                if let Some(slot @ None) = first.get_mut(id.index()) {
                    *slot = Some(located);
                }
            }
            if holder {
                self.holders.push(located);
            }
        }
        self.seq += 1;
    }
}

/// A [`DatasetSink`] that notes where each record goes.
struct Noting<'s, 'd> {
    sink: &'s mut DatasetSink<'d>,
    placed: Vec<(Placed, Option<XrefId>)>,
}

impl RecordSink for Noting<'_, '_> {
    fn has_header(&self) -> bool {
        self.sink.has_header()
    }

    fn header(&mut self, header: Header) {
        self.sink.header(header);
        self.placed.push((Placed::Header, None));
    }

    fn placed(&mut self, kind: RecordKind, pos: usize, xref: Option<XrefId>) {
        self.sink.placed(kind, pos, xref);
        self.placed.push((Placed::Record(kind, pos), xref));
    }
}

/// The text of tag `id`: a standard tag's, else one of `others`.
fn tag_of(id: u32, others: &[Box<str>]) -> &str {
    let id = id as usize;
    match crate::tree::STANDARD_TAGS.get(id) {
        Some(tag) => tag,
        None => others
            .get(id - crate::tree::STANDARD_TAGS.len())
            .map_or("", |t| t),
    }
}

/// The buffers of the arenas [`arena_of`] builds.
#[derive(Default)]
struct Arenas {
    nodes: Vec<crate::tree::RawNode>,
    xrefs: Vec<(u32, RawSpan)>,
    joined: Vec<TextPiece>,
    /// Where the text the drafts' texts are appended to starts in the
    /// store's text, when their spans are to say so ([`Arena::INPUT`] past
    /// the input: [`Arena::side`]) rather than be spans of that text.
    side: Option<u32>,
}

/// Whether `text` is the text of the `pieces` of the text that starts at
/// `base` in `input`, and the pieces can say where they are in it.
fn same(text: &str, pieces: &[TextPiece], base: usize, input: &str) -> bool {
    let mut rest = text;
    for p in pieces {
        let start = base + p.start() as usize;
        if (start + p.len() as usize) & !(Arena::INPUT as usize - 1) != 0 {
            return false;
        }
        if p.newline() {
            let Some(r) = rest.strip_prefix('\n') else {
                return false;
            };
            rest = r;
        }
        let Some(piece) = input.get(start..start + p.len() as usize) else {
            return false;
        };
        let Some(r) = rest.strip_prefix(piece) else {
            return false;
        };
        rest = r;
    }
    rest.is_empty()
}

/// Whether lexing `d`'s GEDCOM lines (as [`Draft::write`] writes them, at
/// `depth`) gives the arena of its structures as they are ([`arena_of`]):
/// not when a structure has no tag, is a continuation the lexer would
/// join, holds a pointer the lexer would read as text or an identifier
/// holding a blank, or lies too deep.
fn simple(d: &Draft<'_>, depth: u8) -> bool {
    let tag = d.tag.as_str();
    !tag.is_empty()
        && !tag.bytes().any(|b| b <= b' ' || b == b'@')
        && !matches!(tag, "CONT" | "CONC")
        && depth < crate::tree::MAX_DEPTH - 1
        && d.pointer()
            .is_none_or(|p| crate::tree::pointer(p) == Some(p))
        && d.xref
            .as_deref()
            .is_none_or(|x| !x.is_empty() && !x.bytes().any(|b| b == b' ' || b == b'\t'))
        && d.substructures.iter().all(|c| simple(c, depth + 1))
}

/// [`simple`] of the record at `root` of the lexer's arena, as read: what
/// its draft would be, found node after node.
fn simple_read(root: NodeRef<'_>, others: &[Box<str>]) -> bool {
    const CONT: u32 = const { std_id("CONT") };
    const CONC: u32 = const { std_id("CONC") };
    let standard = crate::tree::STANDARD_TAGS.len();
    let top = root.subtree().first().map_or(0, |n| n.depth);
    // (The lexer reads as pointers what `simple` takes for pointers, and
    // identifiers without blanks.)
    root.subtree().iter().all(|raw| {
        // (Standard tags are words of capitals and digits.)
        let tag_ok = match (raw.tag as usize).checked_sub(standard) {
            None => raw.tag != CONT && raw.tag != CONC,
            Some(other) => others
                .get(other)
                .is_some_and(|t| !t.is_empty() && !t.bytes().any(|b| b <= b' ' || b == b'@')),
        };
        tag_ok && raw.depth.wrapping_sub(top) < crate::tree::MAX_DEPTH - 1
    })
}

/// The structures rules made of a record's substructures, each ready to go
/// where it goes ([`splice`]): their nodes, identifiers and joined pieces
/// as [`push_arena`] fills them, one after the other; and the tags they
/// use that the arena's interner does not have yet, in the order to intern
/// them. Kept from record to record.
#[derive(Default)]
struct Splices {
    made: Arenas,
    items: Vec<Splice>,
    tags: Vec<String>,
}

/// A structure made of a record's substructure `at` (as read), replacing
/// it or after it: its nodes and identifiers in [`Splices::made`].
struct Splice {
    at: usize,
    replace: bool,
    nodes: std::ops::Range<usize>,
    xrefs: std::ops::Range<usize>,
}

impl Splices {
    fn clear(&mut self) {
        self.made.nodes.clear();
        self.made.xrefs.clear();
        self.made.joined.clear();
        self.items.clear();
        self.tags.clear();
    }
}

/// Fills `s` with the structures `edits` makes, each where it goes among
/// the record's substructures, their texts appended to `written`, which
/// starts at `side` in the store's text: `false` when one is not
/// [`simple`].
fn make_splices(
    edits: &[Edit<'_>],
    input: &str,
    written: &mut String,
    tags: &TagInterner,
    side: usize,
    s: &mut Splices,
) -> bool {
    s.clear();
    s.made.side = u32::try_from(side).ok();
    for e in edits {
        let (at, replace, new) = match e {
            Edit::Replace(at, new) => (*at, true, new.as_deref()),
            Edit::After(at, new) => (*at, false, Some(&**new)),
        };
        let (nodes, xrefs) = (s.made.nodes.len(), s.made.xrefs.len());
        if let Some(d) = new {
            if !simple(d, 1) {
                return false;
            }
            push_arena(d, 1, input, written, &mut s.made, tags, &mut s.tags);
        }
        s.items.push(Splice {
            at,
            replace,
            nodes: nodes..s.made.nodes.len(),
            xrefs: xrefs..s.made.xrefs.len(),
        });
    }
    true
}

/// Where the record's substructure `at` lies in the arena `nodes`.
fn child_range(nodes: &[crate::tree::RawNode], at: usize) -> Option<(usize, usize)> {
    let root_end = nodes.first()?.end as usize;
    let mut k = 1;
    let mut ordinal = 0;
    while k < root_end {
        let end = nodes.get(k).map_or(root_end, |n| n.end as usize);
        if ordinal == at {
            return Some((k, end));
        }
        ordinal += 1;
        k = end.max(k + 1);
    }
    None
}

/// Puts the structures of `s` where they go in the record the lexer left
/// in `b`: the arena then holds the record as the rules made it, and is
/// typed as any record read.
fn splice(b: &mut Builder, s: &Splices) {
    let joined = u32::try_from(b.joined.len()).unwrap_or(u32::MAX);
    b.joined.extend_from_slice(&s.made.joined);
    // The last first: the places of those before stay.
    for sp in s.items.iter().rev() {
        let Some((start, end)) = child_range(&b.nodes, sp.at) else {
            continue;
        };
        let (from, to) = if sp.replace { (start, end) } else { (end, end) };
        let made = s.made.nodes.get(sp.nodes.clone()).unwrap_or_default();
        let len = made.len();
        let first = u32::try_from(sp.nodes.start).unwrap_or(0);
        let at = u32::try_from(from).unwrap_or(u32::MAX);
        let shift = |i: u32| -> u32 {
            let i = i as usize;
            let moved = if i >= to { i - (to - from) + len } else { i };
            u32::try_from(moved).unwrap_or(u32::MAX)
        };
        b.nodes.splice(
            from..to,
            made.iter().map(|&n| {
                let mut n = n;
                n.end = n.end - first + at;
                if n.kind == RawKind::Joined {
                    n.payload.start += joined;
                }
                n
            }),
        );
        for n in b.nodes.iter_mut().skip(from + len) {
            n.end = shift(n.end);
        }
        if let Some(r) = b.nodes.first_mut() {
            r.end = shift(r.end);
        }
        // The identifiers, by node: those of what goes, out; those after,
        // moved; those made, in.
        let (lo, hi) = (
            u32::try_from(from).unwrap_or(u32::MAX),
            u32::try_from(to).unwrap_or(u32::MAX),
        );
        if b.xrefs.iter().any(|&(i, _)| i >= lo && i < hi) {
            b.xrefs.retain(|&(i, _)| i < lo || i >= hi);
        }
        for x in &mut b.xrefs {
            if x.0 >= hi {
                x.0 = shift(x.0);
            }
        }
        let new = s.made.xrefs.get(sp.xrefs.clone()).unwrap_or_default();
        if !new.is_empty() {
            let place = b.xrefs.partition_point(|&(i, _)| i < lo);
            b.xrefs
                .splice(place..place, new.iter().map(|&(i, x)| (i - first + at, x)));
        }
    }
}

/// The id a standard tag has in the lexer's arena.
const fn std_id(tag: &str) -> u32 {
    match crate::tree::standard_index_const(tag) {
        Some(i) => i as u32,
        None => u32::MAX,
    }
}

/// Fills `arenas` with the arena lexing the GEDCOM lines of `draft` (one
/// that is [`simple`]) would give, its texts appended to `text` (spans
/// relative to it): no line is written and read again. The tags `tags`
/// does not have yet take the ids interning them in order would give them,
/// and are put in `new`, to intern once the drafts are done with.
///
/// A text of the draft that is the store's `input` itself (borrowed from
/// it, as a record read is) is not copied: its span is of the input
/// ([`Arena::INPUT`]).
fn arena_of(
    draft: &Draft<'_>,
    input: &str,
    text: &mut String,
    arenas: &mut Arenas,
    tags: &TagInterner,
    new: &mut Vec<String>,
) {
    arenas.nodes.clear();
    arenas.xrefs.clear();
    arenas.joined.clear();
    arenas.side = None;
    push_arena(draft, 0, input, text, arenas, tags, new);
}

fn push_arena(
    d: &Draft<'_>,
    depth: u8,
    input: &str,
    text: &mut String,
    arenas: &mut Arenas,
    tags: &TagInterner,
    new: &mut Vec<String>,
) {
    let tag = d.tag.as_str();
    let side = arenas.side;
    let span = |s: &str, text: &mut String| {
        let len = u32::try_from(s.len()).unwrap_or(0);
        let from = input.as_ptr() as usize;
        let at = s.as_ptr() as usize;
        if at >= from
            && at + s.len() <= from + input.len()
            && let Ok(start) = u32::try_from(at - from)
            && start & Arena::INPUT == 0
        {
            return RawSpan {
                start: start | Arena::INPUT,
                len,
            };
        }
        let at = u32::try_from(text.len()).unwrap_or(u32::MAX);
        text.push_str(s);
        RawSpan {
            start: match side {
                Some(base) => base.saturating_add(at) | Arena::INPUT,
                None => at,
            },
            len,
        }
    };
    let (kind, payload) =
        match &d.payload {
            Value::None => (RawKind::None, RawSpan::default()),
            Value::Pointer(p) => (RawKind::Pointer, span(p, text)),
            Value::Text(t) if t.is_empty() => (RawKind::None, RawSpan::default()),
            // Joined from lines of the input, and the same still: the pieces.
            Value::Text(t)
                if d.joined
                    .is_some_and(|(pieces, base)| same(t, pieces, base, input)) =>
            {
                let (pieces, base) = d.joined.unwrap_or_default();
                // Within 31 bits, as `same` found.
                let base = u32::try_from(base).unwrap_or_default();
                let at = arenas.joined.len();
                arenas.joined.extend(pieces.iter().map(|p| {
                    TextPiece::new((p.start() + base) | Arena::INPUT, p.len(), p.newline())
                }));
                (
                    RawKind::Joined,
                    RawSpan {
                        start: u32::try_from(at).unwrap_or(u32::MAX),
                        len: u32::try_from(pieces.len()).unwrap_or(0),
                    },
                )
            }
            // As written: `\r` ends lines.
            Value::Text(t) if t.contains('\r') => {
                let t = t.replace("\r\n", "\n").replace('\r', "\n");
                (RawKind::Text, span(&t, text))
            }
            Value::Text(t) => (RawKind::Text, span(t, text)),
        };
    let index = u32::try_from(arenas.nodes.len()).unwrap_or(u32::MAX);
    if let Some(x) = &d.xref {
        let x = span(x, text);
        arenas.xrefs.push((index, x));
    }
    arenas.nodes.push(crate::tree::RawNode {
        payload,
        tag: tags.find(tag).unwrap_or_else(|| {
            let k = new.iter().position(|t| t == tag).unwrap_or_else(|| {
                new.push(tag.to_string());
                new.len() - 1
            });
            tags.next_id() + u32::try_from(k).unwrap_or(u32::MAX)
        }),
        line: d.line,
        end: index + 1,
        depth,
        kind,
        has_xref: d.xref.is_some(),
    });
    for c in &d.substructures {
        push_arena(c, depth + 1, input, text, arenas, tags, new);
    }
    let end = u32::try_from(arenas.nodes.len()).unwrap_or(u32::MAX);
    if let Some(n) = arenas.nodes.get_mut(index as usize) {
        n.end = end;
    }
}

/// Reads decoded text as [`read_string`] does, interpreting each record
/// that a rule of `interp` may change as it is read ([`Interpreter`]),
/// then the changes that span records: the dataset and what was
/// interpreted. `known`: whether the documented extensions kept count
/// (reported, or declared in 7.x).
///
/// Only the records a rule may change are copied, as drafts borrowing the
/// text read; each one that changed is typed in its place from an arena
/// its drafts fill, its texts kept after the text read. A record whose
/// interpretation would reorder the file's records hands the text back
/// (`Err`), for the whole file to be interpreted at once
/// ([`crate::profile`]).
///
/// `prints` gets the fingerprint tags the records read have
/// ([`crate::profile::detect::fingerprint`]), each once: what detection
/// reads of the file.
pub(crate) fn read_interpreting(
    text: String,
    interp: Interpreter<'_>,
    known: bool,
    prints: &mut Vec<&'static str>,
) -> Result<(Dataset, InterpretReport), String> {
    let declared = head_version(&text);
    let (version, escaping) = version_of(declared.as_deref());
    let mut data = Dataset::new(version);
    data.declared_version = declared.map(Into::into);
    let mut store = Store::new(text);
    let input_len = store.input_len();
    let mut tags = TagInterner::default();
    let mut scratch = Scratch::default();
    let rules = interp.rules();
    let mut pass = Pass {
        kept: Kept {
            first: rules.spans_records().then(Vec::new),
            ..Kept::default()
        },
        interp,
        rules,
        look: Look {
            schema: rules.reads_schema(),
            v7: version.is_v7(),
            known,
        },
        escaping,
        input_len,
        side: String::new(),
        gave_up: false,
        names: Rc::from([]),
        prints,
        print_of: Vec::new(),
        unseen_prints: false,
        read: Vec::new(),
        clash: None,
        again: Builder::new(escaping, 1).with_spans(),
        arenas: Arenas::default(),
        splices: Splices::default(),
        written: String::new(),
        placed: Vec::new(),
    };
    {
        let Dataset {
            header,
            tables,
            index,
            ..
        } = &mut data;
        let mut sink = DatasetSink { header, index };
        let parts = store.parts_mut();
        let input = parts.text;
        let mut cx = ReadCx::new(parts, tables, &mut scratch, version);
        let mut builder = Builder::new(escaping, 1).with_spans();
        for (start, end, first_line) in segment_bounds(input, SEGMENT_LIMIT) {
            let part = input.get(start..end).unwrap_or_default();
            builder.reset(first_line);
            builder.read_records(part, &mut tags, |b, tags| {
                pass.record(b, part, start, input, tags, &mut cx, &mut sink);
            });
        }
    }
    let Pass {
        interp,
        kept,
        side,
        gave_up,
        ..
    } = pass;
    if gave_up {
        return Err(store.into_input());
    }
    store.append_text(&side);
    drop(side);
    store.add_seen(tags.seen());
    store.set_tags(std::mem::take(&mut tags.others));
    data.store = store;
    let report = if interp.has_pending() {
        let Some(report) = finish(&mut data, &mut tags, &mut scratch, &kept, interp, escaping)
        else {
            let mut text = std::mem::take(&mut data).store.into_input();
            text.truncate(input_len);
            return Err(text);
        };
        report
    } else {
        interp.into_report()
    };
    data.store.shrink();
    if data.store.input_len() >= SMALL_INPUT {
        data.shrink();
    }
    data.index_read();
    data.note_read_source();
    Ok((data, report))
}

/// What [`read_interpreting`] looks for in each record.
#[derive(Clone, Copy)]
struct Look {
    /// Whether a rule reads the header's `_SCHEMA` labels.
    schema: bool,
    /// Whether the file is 7.x.
    v7: bool,
    /// Whether the documented extensions kept count.
    known: bool,
}

/// The state of [`read_interpreting`] from record to record.
struct Pass<'p, 'r> {
    interp: Interpreter<'r>,
    rules: &'r RuleSet,
    kept: Kept,
    look: Look,
    escaping: Escaping,
    input_len: usize,
    /// The texts of the records rewritten, which the store appends.
    side: String,
    /// Whether the file is to be interpreted whole.
    gave_up: bool,
    /// A copy of the tags the record reads are named by (the tags only
    /// grow, so a copy of the same length is the same): held apart from
    /// the tags, which the record's own reading may intern into.
    names: Rc<[Box<str>]>,
    prints: &'p mut Vec<&'static str>,
    /// By tag past the standard ones: the fingerprint it is, if any.
    print_of: Vec<Option<&'static str>>,
    /// Whether a fingerprint tag interned is not in `prints` yet.
    unseen_prints: bool,
    /// By tag (standard ones first): whether a rule may read it, in a
    /// record (bit 0) or under one (bit 1).
    read: Vec<u8>,
    /// Whether the text has a pointer like those of links to records to
    /// add, once asked.
    clash: Option<bool>,
    /// What reads the lines of what only lines say.
    again: Builder,
    arenas: Arenas,
    /// What the rules made of the record interpreted where it was read.
    splices: Splices,
    /// The texts of the record rewritten last.
    written: String,
    placed: Vec<(Placed, Option<XrefId>)>,
}

impl Pass<'_, '_> {
    /// Reads the record the lexer left in `b` (of `part`, which starts at
    /// `base` in the store's input `input`): interpreted when a rule may
    /// change it, typed into the tables either way.
    #[allow(clippy::too_many_arguments)] // The lexer's arena and where it is.
    fn record(
        &mut self,
        b: &mut Builder,
        part: &str,
        base: usize,
        input: &str,
        tags: &mut TagInterner,
        cx: &mut ReadCx<'_>,
        sink: &mut DatasetSink<'_>,
    ) {
        if self.gave_up {
            return;
        }
        // Most rewrites change a substructure of the record or two: the
        // record is interpreted where it was read, and what changed made
        // where it stands (offsets past 31 bits aside), its structures
        // looked at once.
        let fast = self.input_len + self.side.len() < Arena::INPUT as usize / 2;
        if self.names.len() != tags.others.len() {
            self.names = Rc::from(tags.others.as_slice());
        }
        let names = Rc::clone(&self.names);
        let splices = {
            let arena = Arena {
                text: part,
                base,
                joined: &b.joined,
                nodes: &b.nodes,
                xrefs: &b.xrefs,
                input,
                side: "",
                side_base: 0,
                others: &names,
            };
            self.fingerprints(&arena, tags);
            let Some(root) = NodeRef::root(&arena) else {
                return;
            };
            let head = self.look.schema && root.standard_tag() == Some("HEAD");
            if !fast || head {
                self.record_drafts(&arena, root, head, None, input, tags, cx, sink);
                return;
            }
            if !self.reads_any(&arena, tags) {
                self.place(root, cx, sink, false);
                return;
            }
            let in_place = self.interp.record_in_arena(
                root,
                &InArena {
                    others: &tags.others,
                    escaping: self.escaping,
                    known: self.look.known,
                    tried: &self.read,
                },
            );
            if !in_place.wanted {
                self.place(root, cx, sink, false);
                return;
            }
            let Some(done) = in_place.done else {
                self.record_drafts(&arena, root, false, Some(true), input, tags, cx, sink);
                return;
            };
            // What the rules made, ready to take the place of what it
            // replaces, its texts in `written` (after the side's).
            let mut written = std::mem::take(&mut self.written);
            written.clear();
            let side = self.input_len + self.side.len();
            let mut splices = std::mem::take(&mut self.splices);
            let fits = (done.edits.is_empty()
                || simple_read(root, &tags.others)
                    && make_splices(&done.edits, input, &mut written, tags, side, &mut splices))
                && side + written.len() < Arena::INPUT as usize;
            self.written = written;
            if !fits {
                self.splices = splices;
                self.record_drafts(&arena, root, false, Some(true), input, tags, cx, sink);
                return;
            }
            let index = &*sink.index;
            let defined = |x: &str| cx.find_xref(x).is_some_and(|id| index.is_defined(id));
            self.interp.commit(done.notes, root.xref_text(), &defined);
            if done.edits.is_empty() {
                self.splices = splices;
                self.place(root, cx, sink, false);
                return;
            }
            splices
        };
        #[cfg(test)]
        IN_PLACE.with(|n| n.set(n.get() + 1));
        splice(b, &splices);
        for tag in &splices.tags {
            tags.intern(tag);
        }
        self.splices = splices;
        let written = std::mem::take(&mut self.written);
        let side = self.input_len + self.side.len();
        let arena = Arena {
            text: part,
            base,
            joined: &b.joined,
            nodes: &b.nodes,
            xrefs: &b.xrefs,
            input,
            side: &written,
            side_base: side,
            others: &tags.others,
        };
        if let Some(root) = NodeRef::root(&arena) {
            self.place(root, &mut cx.with_text(&written, side), sink, false);
        }
        self.side.push_str(&written);
        self.written = written;
    }

    /// Interprets the record at `root` of `arena` as drafts, then types it
    /// ([`Interpreter::record`]). `head`: it is the header, whose schema
    /// labels interpret the records after it. `wanted`: whether a rule may
    /// change it, when known.
    #[allow(clippy::too_many_arguments)] // The record, where it is, and how.
    fn record_drafts(
        &mut self,
        arena: &Arena<'_>,
        root: NodeRef<'_>,
        head: bool,
        wanted: Option<bool>,
        input: &str,
        tags: &mut TagInterner,
        cx: &mut ReadCx<'_>,
        sink: &mut DatasetSink<'_>,
    ) {
        let escaping = self.escaping;
        let wanted = if let Some(w) = wanted {
            w
        } else {
            let wanted = self.wants(arena, tags);
            if !(wanted || head) {
                self.place(root, cx, sink, false);
                return;
            }
            wanted
        };
        let mut drafts = vec![Draft::of_arena(root, &tags.others, escaping)];
        if head && let Some(h) = drafts.first() {
            self.interp.read_head(h);
        }
        let rewrite = if wanted {
            let index = &*sink.index;
            let defined = |x: &str| cx.find_xref(x).is_some_and(|id| index.is_defined(id));
            self.interp.record(&mut drafts, &defined)
        } else {
            Rewrite::default()
        };
        if rewrite.promoted {
            self.gave_up = true;
            return;
        }
        if rewrite.placeholder {
            // Typed as a pointer, which no pointer of the text can be.
            if *self
                .clash
                .get_or_insert_with(|| input.contains(SIDE_PLACEHOLDER))
            {
                self.gave_up = true;
                return;
            }
            for d in &mut drafts {
                swap_pointers(d, PLACEHOLDER, SIDE_PLACEHOLDER);
            }
        }
        if rewrite.changed {
            let new = self.rewritten(&drafts, rewrite.placeholder, input, tags, cx, sink);
            drop(drafts);
            match new {
                Some(new) => {
                    for tag in new {
                        tags.intern(&tag);
                    }
                }
                None => self.lex_written(rewrite.placeholder, input, tags, cx, sink),
            }
            let written = std::mem::take(&mut self.written);
            self.side.push_str(&written);
            self.written = written;
        } else {
            drop(drafts);
            self.place(root, cx, sink, false);
        }
    }

    /// Whether a rule may read a structure of the record of `arena`, by
    /// its tag alone: most records have none, and are not looked at
    /// further.
    fn reads_any(&mut self, arena: &Arena<'_>, tags: &TagInterner) -> bool {
        let standard = crate::tree::STANDARD_TAGS.len();
        if self.read.len() < standard + tags.others.len() {
            let (v7, known) = (self.look.v7, self.look.known);
            for id in self.read.len()..standard + tags.others.len() {
                let (tag, index) = match crate::tree::STANDARD_TAGS.get(id) {
                    Some(tag) => (*tag, u16::try_from(id).ok()),
                    None => (tags.others.get(id - standard).map_or("", |t| &**t), None),
                };
                let record = self.rules.reads(tag, index, true, v7, known);
                let below = self.rules.reads(tag, index, false, v7, known);
                // What a rule may do to it, documented extensions counted:
                // where interpretation tries the rules.
                let tried = self.rules.reads(tag, index, true, v7, true);
                let tried_below = self.rules.reads(tag, index, false, v7, true);
                self.read.push(
                    u8::from(record)
                        | u8::from(below) << 1
                        | u8::from(tried) << 2
                        | u8::from(tried_below) << 3,
                );
            }
        }
        arena.nodes.iter().any(|n| self.read_at(n))
    }

    /// Whether a rule may read the structure `n` by its tag, where it is.
    #[inline]
    fn read_at(&self, n: &crate::tree::RawNode) -> bool {
        let bit = if n.depth == 0 { 1 } else { 2 };
        self.read.get(n.tag as usize).is_none_or(|r| r & bit != 0)
    }

    /// Whether a rule may change a structure of the record of `arena`:
    /// what interpreting the record would find, found on the lexer's arena
    /// (each structure a rule may read by its tag looked at, under the tags
    /// above it).
    fn wants(&mut self, arena: &Arena<'_>, tags: &TagInterner) -> bool {
        if !self.reads_any(arena, tags) {
            return false;
        }
        let standard = crate::tree::STANDARD_TAGS.len();
        let mut path: Vec<&str> = Vec::new();
        for (i, n) in arena.nodes.iter().enumerate() {
            if !self.read_at(n) {
                continue;
            }
            let Some(node) = u32::try_from(i).ok().and_then(|i| NodeRef::at(arena, i)) else {
                break;
            };
            let tag = node.tag_text(&tags.others);
            // The tags above it: the last structure before it of each
            // depth above its own.
            path.clear();
            let mut want = n.depth;
            for above in arena.nodes.get(..i).unwrap_or_default().iter().rev() {
                if want == 0 {
                    break;
                }
                if above.depth < want {
                    want = above.depth;
                    path.push(tag_of(above.tag, &tags.others));
                }
            }
            path.reverse();
            {
                let index = u16::try_from(n.tag)
                    .ok()
                    .filter(|&i| usize::from(i) < standard);
                let text = std::cell::OnceCell::new();
                let escaping = self.escaping;
                let payload = || {
                    if node.is_pointer() {
                        PayloadRef::Pointer(text.get_or_init(|| node.payload_str(escaping)))
                    } else if node.has_no_payload() {
                        PayloadRef::None
                    } else {
                        PayloadRef::Text(text.get_or_init(|| node.payload_str(escaping)))
                    }
                };
                if self.rules.may_rewrite(
                    tag,
                    index,
                    &path,
                    payload,
                    node.has_xref(),
                    self.look.v7,
                    self.look.known,
                ) {
                    return true;
                }
            }
        }
        false
    }

    /// Notes the fingerprint tags of the record of `arena`.
    fn fingerprints(&mut self, arena: &Arena<'_>, tags: &TagInterner) {
        let new = tags.others.get(self.print_of.len()..).unwrap_or_default();
        if !new.is_empty() {
            for other in new {
                self.print_of
                    .push(crate::profile::detect::fingerprint(other));
            }
            self.watch_prints();
        }
        // (Most files have no fingerprint tag, or show them all early:
        // nothing to look at.)
        if !self.unseen_prints {
            return;
        }
        let standard = crate::tree::STANDARD_TAGS.len();
        for n in arena.nodes {
            let print = (n.tag as usize)
                .checked_sub(standard)
                .and_then(|k| self.print_of.get(k).copied().flatten());
            if let Some(p) = print.filter(|p| !self.prints.contains(p)) {
                self.prints.push(p);
                self.watch_prints();
            }
        }
    }

    /// Notes whether a fingerprint tag interned is not seen yet.
    fn watch_prints(&mut self) {
        self.unseen_prints = self
            .print_of
            .iter()
            .flatten()
            .any(|p| !self.prints.contains(p));
    }

    /// Types the record at `root` into the tables (`holder`: it links to
    /// a record to add).
    fn place(
        &mut self,
        root: NodeRef<'_>,
        cx: &mut ReadCx<'_>,
        sink: &mut DatasetSink<'_>,
        holder: bool,
    ) {
        let mut noting = Noting {
            sink,
            placed: std::mem::take(&mut self.placed),
        };
        dispatch_record(root, cx, &mut noting);
        self.kept.note(&noting.placed, holder);
        self.placed = noting.placed;
        self.placed.clear();
    }

    /// Types `drafts`, a record rewritten (and what it lifted beside it),
    /// where the record was, from arenas the drafts fill, their texts in
    /// [`Pass::written`]: the tags to intern then, once the drafts are
    /// done with. `None` when only their lines say what they are: written
    /// in [`Pass::written`], for [`Pass::lex_written`].
    fn rewritten(
        &mut self,
        drafts: &[Draft<'_>],
        holder: bool,
        input: &str,
        tags: &TagInterner,
        cx: &mut ReadCx<'_>,
        sink: &mut DatasetSink<'_>,
    ) -> Option<Vec<String>> {
        let mut written = std::mem::take(&mut self.written);
        written.clear();
        let base = self.input_len + self.side.len();
        let mut new = Vec::new();
        if drafts.iter().all(|d| simple(d, 0)) {
            let mut arenas = std::mem::take(&mut self.arenas);
            for d in drafts {
                arena_of(d, input, &mut written, &mut arenas, tags, &mut new);
                let arena = Arena {
                    text: &written,
                    base,
                    joined: &arenas.joined,
                    nodes: &arenas.nodes,
                    xrefs: &arenas.xrefs,
                    input,
                    side: "",
                    side_base: 0,
                    others: &tags.others,
                };
                if let Some(root) = NodeRef::root(&arena) {
                    self.place(root, &mut cx.with_text(&written, base), sink, holder);
                }
            }
            self.arenas = arenas;
            self.written = written;
            return Some(new);
        }
        for d in drafts {
            d.write(&mut written, 0, self.escaping);
        }
        self.written = written;
        None
    }

    /// Types the records of the lines [`Pass::rewritten`] wrote, where the
    /// record they rewrite was.
    fn lex_written(
        &mut self,
        holder: bool,
        input: &str,
        tags: &mut TagInterner,
        cx: &mut ReadCx<'_>,
        sink: &mut DatasetSink<'_>,
    ) {
        let written = std::mem::take(&mut self.written);
        let base = self.input_len + self.side.len();
        let mut again = std::mem::replace(&mut self.again, Builder::new(self.escaping, 1));
        again.reset(1);
        again.read_records(&written, tags, |b, tags| {
            let arena = Arena {
                text: &written,
                base,
                joined: &b.joined,
                nodes: &b.nodes,
                xrefs: &b.xrefs,
                input,
                side: "",
                side_base: 0,
                others: &tags.others,
            };
            if let Some(root) = NodeRef::root(&arena) {
                self.place(root, &mut cx.with_text(&written, base), sink, holder);
            }
        });
        self.again = again;
        self.written = written;
    }
}

/// The changes that span records, once every record is read
/// ([`Interpreter::finish`]): the records they concern copied out of the
/// dataset, changed, written after the store's text and typed in their
/// places; the records added typed after the others. `None` when a record
/// changed no longer fits its type.
fn finish(
    data: &mut Dataset,
    tags: &mut TagInterner,
    scratch: &mut Scratch,
    kept: &Kept,
    interp: Interpreter<'_>,
    escaping: Escaping,
) -> Option<InterpretReport> {
    // The records concerned, in the file's order.
    let mut concerned: Vec<Located> = interp
        .targets()
        .filter_map(|x| data.store.find_xref(x))
        .filter_map(|id| kept.first.as_ref()?.get(id.index()).copied().flatten())
        .chain(kept.holders.iter().copied())
        .filter(|l| l.placed != Placed::Header)
        .collect();
    concerned.sort_by_key(|l| l.seq);
    concerned.dedup_by_key(|l| l.seq);
    let version = data.version;
    let structure = |placed: Placed| match placed {
        Placed::Record(kind, pos) => data
            .record_at(kind, pos)
            .map(|r| r.to_structure_in(version)),
        Placed::Header => data
            .header_ref()
            .map(|h| super::dataset::RecordRef::Header(h).to_structure_in(version)),
    };
    let mut records: Vec<Draft<'static>> = concerned
        .iter()
        .map(|l| {
            structure(l.placed)
                .map(Draft::from_owned)
                .unwrap_or_default()
        })
        .collect();
    for r in &mut records {
        swap_pointers(r, SIDE_PLACEHOLDER, PLACEHOLDER);
    }
    let mut head = if interp.declares() {
        structure(Placed::Header).map(Draft::from_owned)
    } else {
        None
    };
    // A record whose lines say more than its structures (a line without a
    // tag, which reading joins to the line before) is changed where the
    // whole file is.
    if !records.iter().chain(&head).all(|r| simple(r, 0)) {
        return None;
    }
    let before = records.clone();
    let head_before = head.clone();
    let taken = |x: &str| {
        data.store
            .find_xref(x)
            .is_some_and(|id| data.index.is_defined(id))
    };
    let report = interp.finish(&mut records, head.as_mut(), &taken);
    // What changed, written after the store's text.
    let mut written = String::new();
    let mut retyped: Vec<(Option<Placed>, usize, usize)> = Vec::new();
    let base = data.store.input_len();
    let mut put = |d: &Draft<'_>, placed: Option<Placed>| {
        let start = written.len();
        d.write(&mut written, 0, escaping);
        retyped.push((placed, base + start, written.len() - start));
    };
    for (k, d) in records.iter().enumerate() {
        match (concerned.get(k), before.get(k)) {
            (Some(l), Some(b)) if b != d => put(d, Some(l.placed)),
            (Some(_), _) => {}
            (None, _) => put(d, None),
        }
    }
    if let Some(h) = head.as_ref().filter(|h| head_before.as_ref() != Some(*h)) {
        put(h, Some(Placed::Header));
    }
    drop(records);
    drop(before);
    // The tags read go back to the interner, which types the records again.
    tags.others = data.store.take_tags();
    tags.reindex();
    data.store.append_text(&written);
    drop(written);
    retype_all(data, tags, scratch, &retyped, escaping).then_some(report)
}

/// Types the records written at `retyped` (where in the store's text) in
/// their places (the header; a record of a kind at a position; none: after
/// the others): whether each still fits its type.
fn retype_all(
    data: &mut Dataset,
    tags: &mut TagInterner,
    scratch: &mut Scratch,
    retyped: &[(Option<Placed>, usize, usize)],
    escaping: Escaping,
) -> bool {
    let version = data.version;
    let Dataset {
        header,
        tables,
        index,
        store,
        ..
    } = data;
    let mut sink = DatasetSink { header, index };
    let parts = store.parts_mut();
    let input = parts.text;
    let mut cx = ReadCx::new(parts, tables, scratch, version);
    let mut builder = Builder::new(escaping, 1).with_spans();
    let mut fits = true;
    for &(placed, start, len) in retyped {
        let text = input.get(start..start + len).unwrap_or_default();

        builder.reset(1);
        builder.read_records(text, tags, |b, tags| {
            let arena = Arena {
                text,
                base: start,
                joined: &b.joined,
                nodes: &b.nodes,
                xrefs: &b.xrefs,
                input,
                side: "",
                side_base: 0,
                others: &tags.others,
            };
            let Some(root) = NodeRef::root(&arena) else {
                return;
            };
            match placed {
                None => dispatch_record(root, &mut cx, &mut sink),
                Some(Placed::Header) => match read_header(root, &mut cx) {
                    Some(h) => *sink.header = Some(Box::new(h)),
                    None => fits = false,
                },
                Some(Placed::Record(kind, pos)) => fits &= retype(root, kind, pos, &mut cx),
            }
        });
    }
    store.add_seen(tags.seen());
    store.set_tags(std::mem::take(&mut tags.others));
    fits
}

/// The pointer a record links to a record interpretation adds with, as
/// written for the lexer ([`PLACEHOLDER`] is no pointer to it), until the
/// record is added.
const SIDE_PLACEHOLDER: &str = "@\u{fffe}@";

/// Every pointer `from` under `d` made `to`.
fn swap_pointers(d: &mut Draft<'_>, from: &'static str, to: &'static str) {
    if d.pointer() == Some(from) {
        d.payload = Value::Pointer(std::borrow::Cow::Borrowed(to));
    }
    for c in &mut d.substructures {
        swap_pointers(c, from, to);
    }
}

/// Types the record at `node` again into the row at `pos` of the records
/// of `kind`, whose rows are left where they are: whether it still fits
/// the type.
fn retype(node: NodeRef<'_>, kind: RecordKind, pos: usize, cx: &mut ReadCx<'_>) -> bool {
    fn at<T>(node: NodeRef<'_>, pos: usize, cx: &mut ReadCx<'_>) -> bool
    where
        T: super::edit::RecordRows
            + HasTable
            + super::table::sealed::Listed<Elem = <T as super::table::sealed::Stored>::Row>,
    {
        let Ok(at) = u32::try_from(pos) else {
            return false;
        };
        T::table_mut(cx.tables).set_extra(at, super::table::List::EMPTY);
        let Some(row) = T::read_elem(node, at, cx) else {
            return false;
        };
        match T::table_mut(cx.tables).rows.get_mut(pos) {
            Some(slot) => {
                *slot = row;
                cx.tables.garbage += 1;
                true
            }
            None => false,
        }
    }
    match kind {
        RecordKind::Individual => at::<Individual>(node, pos, cx),
        RecordKind::Family => at::<Family>(node, pos, cx),
        RecordKind::Source => at::<Source>(node, pos, cx),
        RecordKind::Repository => at::<Repository>(node, pos, cx),
        RecordKind::Multimedia => at::<Multimedia>(node, pos, cx),
        RecordKind::Submitter => at::<Submitter>(node, pos, cx),
        RecordKind::Submission => at::<Submission>(node, pos, cx),
        RecordKind::Note => at::<SharedNote>(node, pos, cx),
        RecordKind::Other => {
            let row = cx.node_row(node);
            match cx.tables.others.rows.get_mut(pos) {
                Some(slot) => {
                    *slot = row;
                    true
                }
                None => false,
            }
        }
    }
}

/// A record a [`RecordReader`] read: the header, or the position of a
/// record of a kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placed {
    Header,
    Record(RecordKind, usize),
}

/// Reads a file one record at a time, each into a dataset of its own: what
/// a stream reads.
pub(crate) struct RecordReader {
    builder: Builder,
    version: GedcomVersion,
    header_seen: bool,
    scratch: Scratch,
}

impl RecordReader {
    /// A reader of a 5.5.1 file, until [`set_version`](Self::set_version)
    /// says otherwise.
    pub(crate) fn new() -> Self {
        Self {
            builder: Builder::new(Escaping::V551, 1).with_spans(),
            version: GedcomVersion::V5_5_1,
            header_seen: false,
            scratch: Scratch::default(),
        }
    }

    /// The version the file declares, and how it escapes `@`.
    pub(crate) fn set_version(&mut self, version: GedcomVersion, escaping: Escaping) {
        self.version = version;
        self.builder.set_escaping(escaping);
    }

    /// Reads the records of `text` (normally one; lines without a level
    /// before the first record form one of their own) into a dataset of
    /// their own, which keeps `text` as its store: the dataset and where
    /// each record is in it.
    pub(crate) fn read(&mut self, text: String, first_line: u32, out: &mut Vec<Placed>) -> Dataset {
        struct Sink<'o> {
            header_seen: &'o mut bool,
            header: &'o mut Option<Box<Header>>,
            out: &'o mut Vec<Placed>,
        }
        impl RecordSink for Sink<'_> {
            fn has_header(&self) -> bool {
                *self.header_seen
            }
            fn header(&mut self, header: Header) {
                *self.header_seen = true;
                *self.header = Some(Box::new(header));
                self.out.push(Placed::Header);
            }
            fn placed(&mut self, kind: RecordKind, pos: usize, _xref: Option<XrefId>) {
                self.out.push(Placed::Record(kind, pos));
            }
        }

        let mut data = Dataset::new(self.version);
        let mut store = Store::of_record(text);
        let mut tags = TagInterner::default();
        {
            let Dataset { header, tables, .. } = &mut data;
            let mut sink = Sink {
                header_seen: &mut self.header_seen,
                header,
                out,
            };
            let parts = store.parts_mut();
            let input = parts.text;
            let mut cx = ReadCx::new(parts, tables, &mut self.scratch, self.version);
            self.builder.reset(first_line);
            self.builder.read_records(input, &mut tags, |b, tags| {
                let arena = Arena {
                    text: input,
                    base: 0,
                    joined: &b.joined,
                    nodes: &b.nodes,
                    xrefs: &b.xrefs,
                    input,
                    side: "",
                    side_base: 0,
                    others: &tags.others,
                };
                if let Some(root) = NodeRef::root(&arena) {
                    dispatch_record(root, &mut cx, &mut sink);
                }
            });
        }
        store.add_seen(tags.seen());
        store.set_tags(tags.others);
        store.shrink();
        data.store = store;
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NoteContent;

    #[test]
    fn segments_read_as_one_text() {
        let text = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NOTE a@@b\n2 CONT c\n1 BIRT\n2 DATE 1 JAN 1900\n2 PLAC Sampleton\n0 @I2@ INDI\n1 ASSO @I1@\n2 RELA Friend\n0 TRLR\n";
        let whole = Dataset::parse(text);
        // Segments of whole records (the largest is 75 bytes).
        for limit in [80, 120, 200] {
            let parts = read_segmented(text.to_owned(), limit, false);
            assert_eq!(parts.to_structures(), whole.to_structures(), "{limit}");
        }
        assert_eq!(
            whole
                .find_individual("@I2@")
                .map(|r| r.associations().len()),
            Some(1)
        );
        let note = whole
            .individuals()
            .first()
            .unwrap()
            .notes()
            .first()
            .unwrap();
        assert!(matches!(note.content(), NoteContent::Text(t) if t.to_str(&whole) == "a@b\nc"));
    }

    #[test]
    fn records_of_one_text() {
        let mut reader = RecordReader::new();
        reader.set_version(GedcomVersion::V7_0, Escaping::V70);
        let mut out = Vec::new();
        reader.read("0 HEAD\n1 GEDC\n2 VERS 7.0\n".into(), 1, &mut out);
        assert_eq!(out, [Placed::Header]);
        out.clear();
        let data = reader.read(
            "0 @I1@ INDI\n1 NAME Ann /Example/\n1 _X y\n".into(),
            4,
            &mut out,
        );
        assert_eq!(out, [Placed::Record(RecordKind::Individual, 0)]);
        let indi = data.individuals().first().unwrap();
        assert_eq!(data.store().xref(indi.xref().unwrap()), "@I1@");
        assert_eq!(indi.extra().first().unwrap().tag(), "_X");
        out.clear();
        // A second header is kept untyped; an empty trailer, not at all.
        reader.read("0 HEAD\n".into(), 8, &mut out);
        assert_eq!(out, [Placed::Record(RecordKind::Other, 0)]);
        out.clear();
        reader.read("0 TRLR\n".into(), 9, &mut out);
        assert!(out.is_empty());
    }
}
