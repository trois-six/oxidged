//! Structures written into a flat arena, their texts borrowed where they
//! can be: what a typed model writes, one record at a time, for the writer
//! to check and emit without an owned [`Structure`] per line.
//!
//! A record of the typed model writing itself for the conformance check
//! types its structures as it goes ([`Flat::open_typed`], [`Typing`]):
//! when every structure is typed and placed, the check looks only at the
//! payloads that need it ([`Flat::checks`]) instead of walking the record.
//!
//! A [`Flat`] in stream mode ([`Flat::stream`]) keeps no structure: each
//! line is written as the record writes the structure, into a buffer of
//! the record's lines, while the structure is typed as above; what the
//! check must look at is kept aside ([`Flat::checks`]). Anything that would
//! need the structures — a structure not typed, or not placed, or missing
//! what its type requires, a line the emitter would repair — spoils the
//! stream ([`Flat::spoiled`]): the record is then written again as
//! structures, for the check and the repair to walk.

use std::borrow::Cow;

use super::{Node, Payload, PayloadRef, Structure, Tag, Xref};
use crate::convert::Watch;
use crate::model::XrefId;
use std::rc::Rc;

use crate::spec::conform::{IdTypes, Typing};
use crate::spec::payload::Family;
use crate::spec::schema::{DATASET, Kind, StructId};
use crate::version::VersionRules;
use crate::writer::{LineSink, emit_typed, special_bytes};

/// The type of a structure not typed ([`Flat::open`]).
const UNTYPED: StructId = StructId::MAX;

/// [`FlatPayload::Text`] of a text whose [`special_bytes`] are not known
/// yet.
pub(crate) const UNKNOWN: u8 = 0x80;

/// The [`special_bytes`] of `text`, known as `special` unless
/// [`UNKNOWN`].
#[inline]
fn special_of(text: &str, special: u8) -> u8 {
    if special == UNKNOWN {
        special_bytes(text)
    } else {
        debug_assert_eq!(special, special_bytes(text), "{text:?}");
        special
    }
}

/// A payload in a [`Flat`] arena.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum FlatPayload<'s> {
    /// No payload.
    #[default]
    None,
    /// A pointer, delimiters included, with its id in the store of the
    /// dataset it was written from, when it has one.
    Pointer(Cow<'s, str>, Option<XrefId>),
    /// Text, unescaped, continuations joined, with its [`special_bytes`]
    /// when known ([`UNKNOWN`] otherwise).
    Text(Cow<'s, str>, u8),
}

impl From<Payload> for FlatPayload<'_> {
    fn from(p: Payload) -> Self {
        match p {
            Payload::None => FlatPayload::None,
            Payload::Pointer(x) => FlatPayload::Pointer(Cow::Owned(x.as_str().to_string()), None),
            Payload::Text(t) => FlatPayload::Text(Cow::Owned(t.into_string()), UNKNOWN),
        }
    }
}

#[derive(Clone, Debug)]
struct FlatNode<'s> {
    tag: Cow<'s, str>,
    xref: Option<Cow<'s, str>>,
    payload: FlatPayload<'s>,
    /// Index just past this node's subtree.
    end: u32,
    /// The number of structures above it.
    depth: u32,
    /// The index of the tag among the standard tags, when it is one.
    standard: Option<u16>,
    /// Its structure type, when it was typed ([`Flat::open_typed`]).
    ty: StructId,
    /// The [`special_bytes`] of a text payload, found as it is written:
    /// the check and the emitter read them instead of the text.
    special: u8,
}

/// Structures in write order (depth first), each with the end of its
/// subtree. Cleared and refilled record after record, it keeps its room;
/// or records are written one after the other ([`Flat::begin`]), and kept.
#[derive(Default)]
pub(crate) struct Flat<'s> {
    nodes: Vec<FlatNode<'s>>,
    /// Where the record being written starts.
    start: usize,
    /// The structures open.
    depth: u32,
    /// The tables that type the structures as they are written, while a
    /// record writes itself for the conformance check.
    typing: Option<Typing>,
    /// Whether a structure of the record being written was not typed, or
    /// not placed where its type permits it.
    untyped: bool,
    /// The typed structures open, while typing.
    open: Vec<u32>,
    /// The next typed structure is one more of a field that holds several.
    repeat: bool,
    /// The typed structures whose payloads need a look.
    checks: Vec<u32>,
    /// An upper bound of the size of the typed structures as written in
    /// 5.5.1.
    size: usize,
    /// The depth of the extension structure being written under a typed
    /// one, while one is.
    extension: Option<u32>,
    /// The extension structures under typed ones, which the check walks.
    extensions: Vec<u32>,
    /// The state of stream mode, when set.
    stream: Option<Box<Stream<'s>>>,
}

/// A structure open in stream mode.
#[derive(Clone, Copy, Debug)]
struct Open {
    /// Its type.
    ty: StructId,
    /// The index of its tag among the standard tags.
    standard: u16,
    /// Where the occurrences of the tags of its typed substructures start
    /// in [`Stream::counts`].
    counts: u32,
    /// Whether it has a substructure.
    children: bool,
    /// Whether its payload is empty.
    empty: bool,
    /// Whether a substructure is tagged `MIME` or `LANG`.
    lang: bool,
}

/// The room of a [`Stream`] that outlives the records it wrote: its line
/// buffer and its stacks, to reuse for records that borrow other stores.
pub(crate) struct StreamBuffers {
    lines: LineSink<'static>,
    open: Vec<Open>,
    counts: Vec<(u8, u32)>,
}

impl StreamBuffers {
    /// Buffers for lines as `lines` (a record's sink) writes them.
    pub(crate) fn new(lines: LineSink<'static>) -> Self {
        Self {
            lines,
            open: Vec::new(),
            counts: Vec::new(),
        }
    }

    /// The sink of the lines of the record being written.
    pub(crate) fn lines_mut(&mut self) -> &mut LineSink<'static> {
        &mut self.lines
    }
}

/// What stream mode keeps of a record: its lines, and what the check must
/// look at.
pub(crate) struct Stream<'s> {
    rules: &'static VersionRules,
    family: Family,
    /// The records pointers name, when they are looked up as they are
    /// written.
    targets: Option<Rc<IdTypes<'s>>>,
    /// The record's lines, emitted under [`RepairPolicy::Error`]: a line
    /// the emitter would repair spoils the stream.
    ///
    /// [`RepairPolicy::Error`]: crate::writer::RepairPolicy::Error
    lines: LineSink<'static>,
    /// The identifier written on the record's line.
    xref: Option<Cow<'s, str>>,
    /// The structures open.
    open: Vec<Open>,
    /// The occurrences of each tag (an index into the tables' tags) of the
    /// typed substructures of each structure open, from its
    /// [`Open::counts`] on.
    counts: Vec<(u8, u32)>,
    /// The depth of the extension structure being written, while one is.
    extension: Option<u32>,
    /// Whether the stream is spoiled.
    spoiled: bool,
    /// The typed pointers the check looks up, with their types (and the
    /// [`special_bytes`] of any payload).
    checks: Vec<(FlatPayload<'s>, StructId, u8)>,
    /// The pointers of extension structures, each to name a record, with
    /// their ids.
    pointers: Vec<(Cow<'s, str>, Option<XrefId>)>,
    /// The typed pointers the 7.x pointer rules between records concern
    /// (`crate::spec::graph`), with their types.
    graph: Vec<(Cow<'s, str>, StructId)>,
    /// The type of the record, once typed.
    root: StructId,
    /// The structures written.
    written: usize,
    /// What a conversion looks for, when the dataset is written in another
    /// version family without one ([`crate::convert::Watch`]).
    watch: Option<Rc<Watch>>,
    /// Whether a structure written needs converting: the stream is
    /// spoiled.
    converts: bool,
}

impl<'s> Stream<'s> {
    /// The room to reuse for another record ([`Stream::with_buffers`]).
    pub(crate) fn into_buffers(self) -> StreamBuffers {
        StreamBuffers {
            lines: self.lines,
            open: self.open,
            counts: self.counts,
        }
    }

    /// The occurrences of `tag` among the counts from `from` on.
    #[inline]
    fn count(&self, from: u32, tag: u8) -> u32 {
        self.counts
            .get(from as usize..)
            .and_then(|c| c.iter().find(|&&(t, _)| t == tag))
            .map_or(0, |&(_, n)| n)
    }

    /// Counts one more `tag` among the counts from `from` on.
    ///
    /// Always inlined: once the stream checks conversions too, the
    /// compiler leaves it outlined, which costs one more call for each
    /// structure written (measured: 3% of the instructions of a write).
    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn bump(&mut self, from: u32, tag: u8) {
        match self
            .counts
            .get_mut(from as usize..)
            .and_then(|c| c.iter_mut().find(|(t, _)| *t == tag))
        {
            Some((_, n)) => *n += 1,
            None => self.counts.push((tag, 1)),
        }
    }

    /// Stream mode for `rules`, with lines as `sink` writes them.
    /// `targets`: the records the pointers name, to look them up as they
    /// are written; without, they are kept for the check.
    pub(crate) fn new(
        rules: &'static VersionRules,
        lines: LineSink<'static>,
        targets: Option<Rc<IdTypes<'s>>>,
    ) -> Self {
        Self::with_buffers(
            rules,
            StreamBuffers {
                lines,
                open: Vec::new(),
                counts: Vec::new(),
            },
            targets,
        )
    }

    /// [`Stream::new`] reusing the room of `buffers`, which another
    /// stream kept ([`Stream::into_buffers`]).
    pub(crate) fn with_buffers(
        rules: &'static VersionRules,
        buffers: StreamBuffers,
        targets: Option<Rc<IdTypes<'s>>>,
    ) -> Self {
        let StreamBuffers {
            lines,
            open,
            counts,
        } = buffers;
        Self {
            rules,
            family: Family::of(rules),
            targets,
            lines,
            xref: None,
            open,
            counts,
            extension: None,
            spoiled: false,
            checks: Vec::new(),
            pointers: Vec::new(),
            graph: Vec::new(),
            root: UNTYPED,
            written: 0,
            watch: None,
            converts: false,
        }
    }

    /// Writes one line: the structure at `level`, its payload's
    /// [`special_bytes`] given for a text. A line the emitter would repair
    /// spoils the stream.
    #[inline]
    fn line(
        &mut self,
        level: u32,
        tag: &str,
        standard: Option<u16>,
        payload: &FlatPayload<'_>,
        special: u8,
    ) {
        // Every tag streamed is in the grammar, and only the record has an
        // identifier (valid: the writer's).
        let xref = if level == 0 {
            self.xref.as_deref()
        } else {
            None
        };
        let payload = match payload {
            FlatPayload::None => PayloadRef::None,
            FlatPayload::Text(t, _) => PayloadRef::Text(t),
            FlatPayload::Pointer(p, _) => PayloadRef::Pointer(p),
        };
        if emit_typed(
            self.rules,
            &mut self.lines,
            level as usize,
            xref,
            tag,
            standard,
            payload,
            special,
        )
        .is_err()
        {
            self.spoiled = true;
        }
    }
}

impl std::fmt::Debug for Flat<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flat")
            .field("nodes", &self.nodes)
            .field("typed", &self.typed())
            .finish_non_exhaustive()
    }
}

impl<'s> Flat<'s> {
    /// Sets stream mode ([`Stream`]), or node mode (`None`), for the
    /// records written from now on.
    pub(crate) fn set_stream(&mut self, stream: Option<Stream<'s>>) {
        self.stream = stream.map(Box::new);
    }

    /// Sets the records the pointers of the records streamed name, looked
    /// up as they are written ([`Stream::new`]).
    pub(crate) fn set_stream_targets(&mut self, targets: Option<Rc<IdTypes<'s>>>) {
        if let Some(st) = self.stream.as_deref_mut() {
            st.targets = targets;
        }
    }

    /// Whether the arena is in stream mode.
    pub(crate) fn is_streaming(&self) -> bool {
        self.stream.is_some()
    }

    /// The state of stream mode, taken out: the arena is in node mode.
    pub(crate) fn take_stream(&mut self) -> Option<Stream<'s>> {
        self.stream.take().map(|b| *b)
    }

    /// Starts a record in stream mode, written with the identifier `xref`
    /// on its line.
    pub(crate) fn begin_stream(&mut self, xref: Option<Cow<'s, str>>) {
        self.clear();
        if let Some(st) = self.stream.as_deref_mut() {
            st.lines.clear();
            st.xref = xref;
            st.open.clear();
            st.counts.clear();
            st.extension = None;
            st.spoiled = false;
            st.checks.clear();
            st.pointers.clear();
            st.graph.clear();
            st.root = UNTYPED;
            st.written = 0;
            st.converts = false;
        }
    }

    /// Looks out, as records are streamed, for structures that `watch`
    /// says need converting ([`Flat::stream_converts`]).
    pub(crate) fn set_stream_watch(&mut self, watch: Option<Rc<Watch>>) {
        if let Some(st) = self.stream.as_deref_mut() {
            st.watch = watch;
        }
    }

    /// Whether a structure of the record streamed needs converting (the
    /// stream is spoiled).
    pub(crate) fn stream_converts(&self) -> bool {
        self.stream.as_ref().is_some_and(|st| st.converts)
    }

    /// Whether the record streamed needs to be written as structures: a
    /// structure of it is not typed, not placed or incomplete, or one of
    /// its lines would be repaired.
    pub(crate) fn spoiled(&self) -> bool {
        self.stream
            .as_ref()
            .is_none_or(|st| st.spoiled || st.written == 0 || !st.open.is_empty())
    }

    /// The lines of the record streamed.
    pub(crate) fn stream_lines(&mut self) -> Option<&mut LineSink<'static>> {
        self.stream.as_deref_mut().map(|st| &mut st.lines)
    }

    /// The typed pointers of the record streamed, which the check looks
    /// up: their payloads, ids and types.
    pub(crate) fn stream_pointers(
        &self,
    ) -> impl Iterator<Item = (PayloadRef<'_>, Option<XrefId>, StructId)> {
        let checks: &[_] = self.stream.as_deref().map_or(&[], |st| &st.checks);
        checks.iter().map(|(p, ty, _)| {
            let id = match p {
                FlatPayload::Pointer(_, id) => *id,
                FlatPayload::None | FlatPayload::Text(..) => None,
            };
            (payload_ref(p), id, *ty)
        })
    }

    /// The typed pointers of the record streamed that the 7.x pointer
    /// rules between records concern, with their types.
    pub(crate) fn stream_graph(&self) -> impl Iterator<Item = (&str, StructId)> {
        let graph: &[_] = self.stream.as_deref().map_or(&[], |st| &st.graph);
        graph.iter().map(|(p, ty)| (&**p, *ty))
    }

    /// The pointers of the extension structures of the record streamed,
    /// each to name a record, with their ids.
    pub(crate) fn stream_extension_pointers(&self) -> impl Iterator<Item = (&str, Option<XrefId>)> {
        let pointers: &[_] = self.stream.as_deref().map_or(&[], |st| &st.pointers);
        pointers.iter().map(|(p, id)| (&**p, *id))
    }

    /// The number of structures.
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Room for `n` more structures.
    pub(crate) fn reserve(&mut self, n: usize) {
        self.nodes.reserve(n);
    }

    /// Empties the arena, keeping its room (and its typing).
    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.begin();
    }

    /// Empties the arena, handing its structures to `f` in write order,
    /// each with the number of structures above it, their texts moved out.
    /// Each also with the number of its substructures.
    pub(crate) fn drain_into(
        &mut self,
        mut f: impl FnMut(u32, Cow<'s, str>, Option<Cow<'s, str>>, FlatPayload<'s>, usize),
    ) {
        let counts: Vec<usize> = (0..self.nodes.len())
            .map(|i| {
                let end = self.nodes.get(i).map_or(0, |n| n.end as usize);
                let mut n = 0;
                let mut j = i + 1;
                while let Some(c) = self.nodes.get(j).filter(|_| j < end) {
                    n += 1;
                    j = (c.end as usize).max(j + 1);
                }
                n
            })
            .collect();
        for (n, count) in self.nodes.drain(..).zip(counts) {
            f(n.depth, n.tag, n.xref, n.payload, count);
        }
        self.begin();
    }

    /// Starts a record after those written: where it starts.
    pub(crate) fn begin(&mut self) -> usize {
        self.start = self.nodes.len();
        self.depth = 0;
        self.untyped = false;
        self.open.clear();
        self.repeat = false;
        self.checks.clear();
        self.size = 0;
        self.extension = None;
        self.extensions.clear();
        self.start
    }

    /// Drops the records from `start` on.
    pub(crate) fn truncate(&mut self, start: usize) {
        self.nodes.truncate(start);
        self.begin();
    }

    /// Types the structures written from now on with `typing`, or not.
    pub(crate) fn set_typing(&mut self, typing: Option<Typing>) {
        self.typing = typing;
    }

    /// Whether every structure of the record being written was typed and
    /// placed where its type permits it, and has what its type requires:
    /// the conformance check then needs to look at [`Flat::checks`] and
    /// [`Flat::extensions`] only.
    pub(crate) fn typed(&self) -> bool {
        if self.stream.is_some() {
            return !self.spoiled();
        }
        self.typing.is_some() && !self.untyped && self.nodes.len() > self.start
    }

    /// The type of the record being written, once typed.
    pub(crate) fn root_type(&self) -> Option<StructId> {
        if let Some(st) = self.stream.as_deref() {
            return Some(st.root).filter(|&t| t != UNTYPED);
        }
        self.nodes
            .get(self.start)
            .map(|n| n.ty)
            .filter(|&t| t != UNTYPED)
    }

    /// The payloads of the typed structures that need a look, with their
    /// types and [`special_bytes`].
    pub(crate) fn checks(&self) -> impl Iterator<Item = (PayloadRef<'_>, StructId, u8)> {
        self.checks.iter().filter_map(|&i| {
            let n = self.nodes.get(i as usize)?;
            Some((payload_ref(&n.payload), n.ty, n.special))
        })
    }

    /// The tags of [`Flat::checks`].
    #[cfg(test)]
    pub(crate) fn check_tags(&self) -> impl Iterator<Item = &str> {
        self.checks
            .iter()
            .filter_map(|&i| self.nodes.get(i as usize).map(|n| &*n.tag))
    }

    /// The extension structures written under typed ones (or as the
    /// record), which the check walks as it walks any extension.
    pub(crate) fn extensions(&self) -> impl Iterator<Item = FlatRef<'_>> {
        self.extensions.iter().map(|&index| FlatRef {
            nodes: &self.nodes,
            index,
        })
    }

    /// An upper bound of the size of the typed structures as written in
    /// 5.5.1.
    pub(crate) fn size(&self) -> usize {
        self.size
    }

    /// Something not typed is written: the check walks the record.
    pub(crate) fn untrusted(&mut self) {
        self.untyped = true;
        if let Some(st) = self.stream.as_deref_mut() {
            st.spoiled = true;
        }
    }

    /// Whether the next typed structure is one more of a field that holds
    /// several.
    pub(crate) fn repeat(&mut self, repeat: bool) {
        self.repeat = repeat;
    }

    /// Starts a typed structure: placed by its tag under the structure
    /// open ([`Typing::place`]), its payload trusted when `claim` is the
    /// kind of payload of its type (a typed value made valid for it) or
    /// left to the check.
    #[inline]
    pub(crate) fn open_typed(
        &mut self,
        tag: &'static str,
        standard: Option<u16>,
        xref: Option<Cow<'s, str>>,
        payload: FlatPayload<'s>,
        claim: Option<Kind>,
    ) -> usize {
        if self.stream.is_some() {
            self.stream_typed(tag, standard, xref.as_deref(), payload, claim);
            return 0;
        }
        let repeat = std::mem::take(&mut self.repeat);
        let index = self.nodes.len();
        let mut ty = UNTYPED;
        if let (false, Some(typing)) = (self.untyped, self.typing.as_ref()) {
            let parent = self.open.last().map(|&p| p as usize);
            let sup = match parent {
                Some(p) => self.nodes.get(p).map_or(UNTYPED, |n| n.ty),
                None => DATASET,
            };
            // Only a record has an identifier.
            let fits = sup != UNTYPED && (xref.is_none() || parent.is_none());
            let is_pointer = matches!(payload, FlatPayload::Pointer(..));
            // One more of a field that holds several: the occurrences of
            // its tag so far.
            let count = match (repeat, parent) {
                (true, Some(p)) => self.count_children(p, standard) + 1,
                _ => 1,
            };
            match fits
                .then(|| typing.place(sup, standard, is_pointer, count))
                .flatten()
            {
                Some(found) => {
                    ty = found;
                    if !typing.trusts(found, &payload, claim) {
                        self.checks.push(u32::try_from(index).unwrap_or(u32::MAX));
                    }
                    self.size += size_bound(tag, standard, xref.as_deref(), &payload);
                    self.open.push(u32::try_from(index).unwrap_or(u32::MAX));
                }
                None => self.untyped = true,
            }
        }
        self.push(Cow::Borrowed(tag), standard, xref, payload, ty)
    }

    /// A typed structure without substructures ([`Flat::open_typed`]).
    #[inline]
    pub(crate) fn leaf_typed(
        &mut self,
        tag: &'static str,
        standard: Option<u16>,
        payload: FlatPayload<'s>,
        claim: Option<Kind>,
    ) {
        let at = self.open_typed(tag, standard, None, payload, claim);
        self.close_inline(at);
    }

    /// Starts a structure; its substructures follow until [`Flat::close`].
    pub(crate) fn open(
        &mut self,
        tag: Cow<'s, str>,
        xref: Option<Cow<'s, str>>,
        payload: FlatPayload<'s>,
    ) -> usize {
        let standard = super::standard_index(&tag);
        self.open_standard(tag, standard, xref, payload)
    }

    /// [`Flat::open`], the tag's standard index known (`standard`).
    pub(crate) fn open_standard(
        &mut self,
        tag: Cow<'s, str>,
        standard: Option<u16>,
        xref: Option<Cow<'s, str>>,
        payload: FlatPayload<'s>,
    ) -> usize {
        self.repeat = false;
        if self.stream.is_some() {
            self.stream_standard(&tag, standard, xref.as_deref(), payload);
            return 0;
        }
        if !self.untyped && self.typing.is_some() {
            // An extension structure under a typed one (or as the record)
            // is left to the check; anything else untyped is not.
            if self.extension.is_none() {
                if tag.starts_with('_') {
                    self.extensions
                        .push(u32::try_from(self.nodes.len()).unwrap_or(u32::MAX));
                    self.extension = Some(self.depth);
                } else {
                    self.untyped = true;
                }
            }
        }
        self.push(tag, standard, xref, payload, UNTYPED)
    }

    /// The substructures with the standard tag `standard` of the structure
    /// at `parent`, written so far.
    fn count_children(&self, parent: usize, standard: Option<u16>) -> u32 {
        let mut n = 0;
        let mut i = parent + 1;
        while let Some(c) = self.nodes.get(i) {
            if c.standard == standard {
                n += 1;
            }
            i = (c.end as usize).max(i + 1);
        }
        n
    }

    #[inline]
    fn push(
        &mut self,
        tag: Cow<'s, str>,
        standard: Option<u16>,
        xref: Option<Cow<'s, str>>,
        payload: FlatPayload<'s>,
        ty: StructId,
    ) -> usize {
        let special = match &payload {
            FlatPayload::Text(t, special) => special_of(t, *special),
            FlatPayload::None | FlatPayload::Pointer(..) => 0,
        };
        self.nodes.push(FlatNode {
            standard,
            tag,
            xref,
            payload,
            end: 0,
            depth: self.depth,
            ty,
            special,
        });
        self.depth += 1;
        self.nodes.len() - 1
    }

    /// Ends the structure [`Flat::open`] started at `at`.
    #[inline]
    pub(crate) fn close(&mut self, at: usize) {
        self.close_inline(at);
    }

    /// [`Flat::close`], inlined where it is called.
    #[expect(
        clippy::inline_always,
        reason = "a leaf that closes in its own code is written about 1% faster"
    )]
    #[inline(always)]
    fn close_inline(&mut self, at: usize) {
        if self.stream.is_some() {
            self.stream_close();
            return;
        }
        let end = u32::try_from(self.nodes.len()).unwrap_or(u32::MAX);
        if let Some(node) = self.nodes.get_mut(at) {
            node.end = end;
        }
        self.depth = self.depth.saturating_sub(1);
        if self.untyped {
            return;
        }
        if let Some(typing) = self.typing.as_ref() {
            if let Some(depth) = self.extension {
                if depth == self.depth {
                    self.extension = None;
                }
                return;
            }
            self.open.pop();
            if !self.has_required(typing, at) {
                self.untyped = true;
            }
        }
    }

    /// [`Flat::open_typed`] in stream mode.
    #[inline]
    fn stream_typed(
        &mut self,
        tag: &'static str,
        standard: Option<u16>,
        xref: Option<&str>,
        payload: FlatPayload<'s>,
        claim: Option<Kind>,
    ) {
        let repeat = std::mem::take(&mut self.repeat);
        let depth = self.depth;
        self.depth += 1;
        let (Some(st), Some(typing)) = (self.stream.as_deref_mut(), self.typing.as_ref()) else {
            return;
        };
        if st.spoiled {
            return;
        }
        // Typed under an extension structure, or an identifier below the
        // record, is for the check's walk.
        let parent = st.open.last();
        let sup = match parent {
            Some(p) => p.ty,
            None => DATASET,
        };
        if st.extension.is_some()
            || (parent.is_none() && depth > 0)
            || (xref.is_some() && parent.is_some())
        {
            st.spoiled = true;
            return;
        }
        let spec = standard.and_then(|s| typing.spec_tag(s));
        let count = match (repeat, parent, spec) {
            (true, Some(p), Some(t)) => st.count(p.counts, t) + 1,
            _ => 1,
        };
        let is_pointer = matches!(payload, FlatPayload::Pointer(..));
        let (Some(ty), Some(spec)) = (typing.place(sup, standard, is_pointer, count), spec) else {
            st.spoiled = true;
            return;
        };
        let special = match &payload {
            FlatPayload::Text(t, special) => special_of(t, *special),
            FlatPayload::None | FlatPayload::Pointer(..) => 0,
        };
        if let Some(w) = st.watch.as_deref() {
            let (kind, len) = match &payload {
                FlatPayload::None => (crate::convert::NONE, 0),
                FlatPayload::Text(t, _) => (crate::convert::TEXT, t.len()),
                FlatPayload::Pointer(..) => (crate::convert::POINTER, 0),
            };
            if w.may_apply(standard, kind, len, special)
                && watched(w, &st.open, st.xref.is_some(), tag, &payload)
            {
                st.converts = true;
                st.spoiled = true;
                return;
            }
        }
        if !typing.v7() {
            self.size += size_bound(tag, standard, xref, &payload);
        }
        st.line(depth, tag, standard, &payload, special);
        let empty = match &payload {
            FlatPayload::None => true,
            FlatPayload::Pointer(..) => false,
            FlatPayload::Text(t, _) => t.is_empty(),
        };
        match st.open.last_mut() {
            Some(p) => {
                p.children = true;
                p.lang |= matches!(tag, "MIME" | "LANG");
                let from = p.counts;
                st.bump(from, spec);
            }
            None => st.root = ty,
        }
        st.open.push(Open {
            ty,
            standard: standard.unwrap_or(u16::MAX),
            counts: u32::try_from(st.counts.len()).unwrap_or(u32::MAX),
            children: false,
            empty,
            lang: false,
        });
        if let FlatPayload::Pointer(p, _) = &payload
            && typing.is_graph_pointer(st.root, ty)
        {
            st.graph.push((p.clone(), ty));
        }
        match (
            typing.check(ty, &payload, claim, special),
            &st.targets,
            &payload,
        ) {
            (Some(true), _, _) => {}
            (Some(false), _, _) => st.spoiled = true,
            // A pointer names a record of its type.
            (None, Some(targets), FlatPayload::Pointer(p, id)) => {
                if !targets.resolves(p, *id, Some(typing.pointer_target(ty)), st.family) {
                    st.spoiled = true;
                }
            }
            // Looked up with the dataset's records later.
            (None, _, _) => st.checks.push((payload, ty, special)),
        }
        st.written += 1;
    }

    /// [`Flat::open_standard`] in stream mode: an extension structure, as
    /// the check walks it (`Conformer::clean_extension`).
    #[inline]
    fn stream_standard(
        &mut self,
        tag: &str,
        standard: Option<u16>,
        xref: Option<&str>,
        payload: FlatPayload<'s>,
    ) {
        let depth = self.depth;
        self.depth += 1;
        let Some(st) = self.stream.as_deref_mut() else {
            return;
        };
        if st.spoiled {
            return;
        }
        let rules = st.rules;
        let top = st.extension.is_none();
        if top {
            // An extension structure under a typed one, or as the record,
            // with a tag in the grammar; anything else is untyped.
            if !tag.starts_with('_') || !rules.is_valid_tag(tag) {
                st.spoiled = true;
                return;
            }
            st.extension = Some(depth);
            if let Some(p) = st.open.last_mut() {
                p.children = true;
            }
        } else if !(standard.is_some() || rules.is_valid_tag(tag)) {
            st.spoiled = true;
            return;
        }
        // Only a record has an identifier.
        if xref.is_some() && depth > 0 {
            st.spoiled = true;
            return;
        }
        let special = match &payload {
            FlatPayload::Text(t, special) => {
                let special = special_of(t, *special);
                if crate::spec::payload::has_banned_known(t, special, rules) {
                    st.spoiled = true;
                    return;
                }
                special
            }
            FlatPayload::None | FlatPayload::Pointer(..) => 0,
        };
        // Whether it needs converting, or interpreting, is for the
        // record's look.
        if st.watch.as_deref().is_some_and(|w| {
            w.may(tag, payload_ref(&payload), special)
                || w.interprets_node(
                    tag,
                    standard,
                    payload_ref(&payload),
                    xref.is_some(),
                    depth == 0,
                )
        }) {
            st.spoiled = true;
            return;
        }
        self.size += size_bound(tag, standard, xref, &payload);
        st.line(depth, tag, standard, &payload, special);
        if let FlatPayload::Pointer(p, id) = payload {
            match &st.targets {
                Some(targets) => {
                    if !targets.resolves(&p, id, None, st.family) {
                        st.spoiled = true;
                    }
                }
                None => st.pointers.push((p, id)),
            }
        }
        st.written += 1;
    }

    /// [`Flat::close`] in stream mode: a typed structure has what its type
    /// requires ([`Flat::has_required`]).
    #[inline]
    fn stream_close(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        let (Some(st), Some(typing)) = (self.stream.as_deref_mut(), self.typing.as_ref()) else {
            return;
        };
        if st.spoiled {
            return;
        }
        if let Some(depth) = st.extension {
            if depth == self.depth {
                st.extension = None;
            }
            return;
        }
        let Some(o) = st.open.pop() else {
            st.spoiled = true;
            return;
        };
        let complete = typing
            .required(o.ty)
            .all(|(tag, min)| st.count(o.counts, tag) >= u32::from(min));
        st.counts.truncate(o.counts as usize);
        let v7_ok = !typing.v7()
            || ((st.open.is_empty() || !o.empty || o.children)
                && (!typing.is_note_tran(o.ty) || o.lang));
        if !complete || !v7_ok {
            st.spoiled = true;
        }
    }

    /// Whether the typed structure at `at`, complete, has the
    /// substructures its type requires and, in 7.x, a payload or a
    /// substructure ([`Conformer::clean`](crate::spec::conform)).
    fn has_required(&self, typing: &Typing, at: usize) -> bool {
        let Some(node) = self.nodes.get(at) else {
            return false;
        };
        let children = || {
            FlatRef {
                nodes: &self.nodes,
                index: u32::try_from(at).unwrap_or(u32::MAX),
            }
            .children()
        };
        for (tag, min) in typing.required(node.ty) {
            let n = children()
                .filter(|c| {
                    c.standard_tag()
                        .and_then(|s| typing.spec_tag(s))
                        .is_some_and(|t| t == tag)
                })
                .count();
            if n < usize::from(min) {
                return false;
            }
        }
        if typing.v7() {
            let empty = match &node.payload {
                FlatPayload::None => true,
                FlatPayload::Pointer(..) => false,
                FlatPayload::Text(t, _) => t.is_empty(),
            };
            let record = self.open.is_empty();
            if !record && empty && node.end as usize == at + 1 {
                return false;
            }
            if typing.is_note_tran(node.ty)
                && !children().any(|c| matches!(c.tag(), "MIME" | "LANG"))
            {
                return false;
            }
        }
        true
    }

    /// An owned structure and its substructures.
    pub(crate) fn push_structure(&mut self, s: Structure) {
        let tag = match s.tag.standard_index() {
            Some(_) => Cow::Borrowed(tag_text(&s.tag)),
            None => Cow::Owned(s.tag.as_str().to_string()),
        };
        let at = self.open(
            tag,
            s.xref.map(|x| Cow::Owned(x.as_str().to_string())),
            s.payload.into(),
        );
        for c in s.substructures {
            self.push_structure(c);
        }
        self.close(at);
    }

    /// Writes `s` and its substructures, borrowed, as the typed model
    /// writes its structures ([`Flat::open_typed`]): typed by their
    /// standard tags where their types place them, their payloads checked
    /// and the occurrences of each tag counted. In stream mode, a structure
    /// converted outside the typed model is so written as lines.
    pub(crate) fn push_typed(&mut self, s: &'s Structure) {
        // Apart: the structures of the typed model are written inline.
        #[inline(never)]
        fn open<'s>(flat: &mut Flat<'s>, s: &'s Structure) -> usize {
            let standard = s.tag.standard_index();
            let xref = s.xref.as_ref().map(|x| Cow::Borrowed(x.as_str()));
            let payload = match &s.payload {
                Payload::None => FlatPayload::None,
                Payload::Text(t) => FlatPayload::Text(Cow::Borrowed(&**t), UNKNOWN),
                Payload::Pointer(p) => FlatPayload::Pointer(Cow::Borrowed(p.as_str()), None),
            };
            match standard.and_then(|i| super::STANDARD_TAGS.get(usize::from(i))) {
                Some(&tag) => {
                    // The occurrences of each tag counted, as for a field
                    // that holds several.
                    flat.repeat = true;
                    flat.open_typed(tag, standard, xref, payload, None)
                }
                None => flat.open_standard(Cow::Borrowed(s.tag.as_str()), None, xref, payload),
            }
        }
        // Depth first, without recursion.
        let mut stack = vec![(open(self, s), s.substructures.iter())];
        while let Some((at, children)) = stack.last_mut() {
            if let Some(c) = children.next() {
                let at = open(self, c);
                stack.push((at, c.substructures.iter()));
            } else {
                let at = *at;
                stack.pop();
                self.close(at);
            }
        }
    }

    /// The record being written (or the last one), with its
    /// substructures.
    pub(crate) fn root(&self) -> Option<FlatRef<'_>> {
        self.root_at(self.start)
    }

    /// The record written from `start`.
    pub(crate) fn root_at(&self, start: impl TryInto<u32>) -> Option<FlatRef<'_>> {
        let index = start.try_into().ok()?;
        self.nodes.get(index as usize).map(|_| FlatRef {
            nodes: &self.nodes,
            index,
        })
    }
}

/// The text of a standard tag, for as long as the program runs.
fn tag_text(tag: &Tag) -> &'static str {
    tag.standard_index()
        .and_then(|i| super::STANDARD_TAGS.get(usize::from(i)))
        .copied()
        .unwrap_or("")
}

/// An upper bound of what a structure adds to the size of its record as
/// written in 5.5.1, as the conformance check bounds it: a standard tag has
/// at most six letters.
fn size_bound(
    tag: &str,
    standard: Option<u16>,
    xref: Option<&str>,
    payload: &FlatPayload<'_>,
) -> usize {
    let tag = standard.map_or(tag.len(), |_| 6);
    let payload = match payload {
        FlatPayload::None => 0,
        FlatPayload::Pointer(p, _) | FlatPayload::Text(p, _) => p.len(),
    };
    8 + tag + xref.map_or(0, str::len) + 13 * payload
}

/// Whether `watch` says that a typed structure tagged `tag` with
/// `payload`, under the structures open in `st`, needs converting
/// ([`Watch::typed`]) or interpreting ([`Watch::interprets_node`]), once
/// [`Watch::may_apply`] says it may. Apart, so that stream mode stays small
/// (and the same size: more arguments make the compiler outline what each
/// structure written calls).
#[inline(never)]
fn watched(
    watch: &Watch,
    open: &[Open],
    named: bool,
    tag: &str,
    payload: &FlatPayload<'_>,
) -> bool {
    // Only a record has an identifier, there (`named`).
    let record = open.is_empty();
    if watch.has_interpretation()
        && watch.interprets_node(
            tag,
            super::standard_index(tag),
            payload_ref(payload),
            record && named,
            record,
        )
    {
        return true;
    }
    let n = open.len();
    let tag_at = |i: Option<usize>| {
        i.and_then(|i| open.get(i))
            .and_then(|o| super::STANDARD_TAGS.get(usize::from(o.standard)))
            .map_or("", |t| t)
    };
    let (parent, grand) = (tag_at(n.checked_sub(1)), tag_at(n.checked_sub(2)));
    watch.typed(tag, payload_ref(payload), parent, grand, n > 0)
}

/// A payload, borrowed.
#[inline]
fn payload_ref<'a>(payload: &'a FlatPayload<'_>) -> PayloadRef<'a> {
    match payload {
        FlatPayload::Pointer(p, _) => PayloadRef::Pointer(p),
        FlatPayload::Text(t, _) => PayloadRef::Text(t),
        FlatPayload::None => PayloadRef::None,
    }
}

/// A structure of a [`Flat`] arena.
#[derive(Clone, Copy)]
pub(crate) struct FlatRef<'a> {
    nodes: &'a [FlatNode<'a>],
    index: u32,
}

impl<'a> FlatRef<'a> {
    #[inline]
    fn node(self) -> Option<&'a FlatNode<'a>> {
        self.nodes.get(self.index as usize)
    }

    /// The [`special_bytes`] of the text payload.
    pub(crate) fn special(self) -> Option<u8> {
        self.node().map(|n| n.special)
    }

    /// The structure and its substructures, in write order, with their
    /// depth below it.
    pub(crate) fn preorder(self) -> impl Iterator<Item = (usize, FlatRef<'a>)> {
        let nodes = self.nodes;
        let (end, base) = self
            .node()
            .map_or((0, 0), |n| (n.end.max(self.index + 1), n.depth));
        (self.index..end).filter_map(move |index| {
            let depth = nodes.get(index as usize)?.depth.checked_sub(base)?;
            Some((depth as usize, FlatRef { nodes, index }))
        })
    }
}

/// The substructures of a [`FlatRef`].
#[derive(Clone)]
pub(crate) struct FlatChildren<'a> {
    nodes: &'a [FlatNode<'a>],
    next: u32,
    end: u32,
}

impl<'a> Iterator for FlatChildren<'a> {
    type Item = FlatRef<'a>;

    #[inline]
    fn next(&mut self) -> Option<FlatRef<'a>> {
        if self.next >= self.end {
            return None;
        }
        let index = self.next;
        let end = self.nodes.get(index as usize).map_or(index + 1, |n| n.end);
        self.next = end.max(index + 1);
        Some(FlatRef {
            nodes: self.nodes,
            index,
        })
    }
}

impl<'a> Node<'a> for FlatRef<'a> {
    type Children = FlatChildren<'a>;

    #[inline]
    fn tag(self) -> &'a str {
        self.node().map_or("", |n| &n.tag)
    }

    #[inline]
    fn standard_tag(self) -> Option<u16> {
        self.node()?.standard
    }

    #[inline]
    fn xref(self) -> Option<&'a str> {
        self.node()?.xref.as_deref()
    }

    #[inline]
    fn payload(self) -> PayloadRef<'a> {
        self.node()
            .map_or(PayloadRef::None, |n| payload_ref(&n.payload))
    }

    fn line(self) -> u32 {
        0
    }

    fn children(self) -> FlatChildren<'a> {
        FlatChildren {
            nodes: self.nodes,
            next: self.index + 1,
            end: self.node().map_or(0, |n| n.end),
        }
    }

    fn to_owned_structure(self) -> Structure {
        let tag = match self.standard_tag() {
            Some(i) => Tag::standard(i),
            None => Tag::new(self.tag()),
        };
        let mut substructures = Vec::with_capacity(self.children().count());
        substructures.extend(self.children().map(Node::to_owned_structure));
        Structure {
            tag,
            xref: self.xref().map(Xref::new),
            payload: self.payload().to_payload(),
            substructures,
            line: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_arena_reads_as_its_structures() {
        let tree = crate::tree::parse_tree(
            "0 @I1@ INDI\n1 NAME A /B/\n2 GIVN A\n1 FAMS @F1@\n1 _X\n2 _Y y\n0 TRLR\n",
        );
        let indi = tree.to_structures().remove(0);
        let mut flat = Flat::default();
        flat.push_structure(indi.clone());
        let root = flat.root().unwrap();
        assert_eq!(root.to_owned_structure(), indi);
        assert_eq!(root.children().count(), 3);
        assert_eq!(root.xref(), Some("@I1@"));
        let fams = root.children().nth(1).unwrap();
        assert_eq!(fams.payload(), PayloadRef::Pointer("@F1@"));
        let levels: Vec<(usize, &str)> = root.preorder().map(|(l, n)| (l, n.tag())).collect();
        assert_eq!(
            levels,
            [
                (0, "INDI"),
                (1, "NAME"),
                (2, "GIVN"),
                (1, "FAMS"),
                (1, "_X"),
                (2, "_Y")
            ]
        );
        flat.clear();
        assert!(flat.root().is_none());
    }
}
