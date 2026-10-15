//! Untyped structures: what typed structures keep in `extra`.

use std::ops::{Deref, DerefMut};

use std::borrow::Cow;

use crate::tree::{Flat, FlatPayload, Payload, Structure, Tag, Xref};

use super::driver::WriteCx;
use super::list::ThinVec;
use super::text::{Store, TagId, Text, XrefId};

/// The payload of a [`Node`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Value {
    /// No payload.
    #[default]
    None,
    /// A pointer.
    Pointer(XrefId),
    /// Text, unescaped, continuations joined.
    Text(Text),
}

/// A structure kept as it was read: a tag, an identifier, a payload and
/// substructures. Typed structures keep in their `extra` every substructure
/// they have no field for, and a dataset keeps so the records it has no
/// type for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Node {
    /// The tag.
    pub tag: TagId,
    /// An identifier, which a substructure seldom has.
    pub xref: Option<XrefId>,
    /// The payload.
    pub payload: Value,
    /// The substructures, in order.
    pub children: Vec<Node>,
}

impl Node {
    /// A node with a tag and nothing else.
    #[must_use]
    pub fn new(tag: TagId) -> Self {
        Self {
            tag,
            xref: None,
            payload: Value::None,
            children: Vec::new(),
        }
    }

    /// A node with the contents of an owned [`Structure`]: its tag and
    /// identifiers interned in `store`, its text owned.
    #[must_use]
    pub fn from_structure(structure: &Structure, store: &mut Store) -> Self {
        let pointer = |p: &str, store: &mut Store| match store.intern_xref(p) {
            Some(id) => Value::Pointer(id),
            None => Value::Text(Text::new(p)),
        };
        Self {
            tag: store.intern_tag(structure.tag.as_str()),
            xref: structure.xref.as_deref().and_then(|x| store.intern_xref(x)),
            payload: match &structure.payload {
                Payload::None => Value::None,
                Payload::Pointer(p) => pointer(p, store),
                Payload::Text(t) => Value::Text(Text::new(&**t)),
            },
            children: structure
                .substructures
                .iter()
                .map(|s| Self::from_structure(s, store))
                .collect(),
        }
    }

    /// An owned [`Structure`] with the same contents, read from `store`.
    #[must_use]
    pub fn to_structure<S: AsRef<Store> + ?Sized>(&self, store: &S) -> Structure {
        let store = store.as_ref();
        Structure {
            tag: Tag::new(store.tag(self.tag)),
            xref: self.xref.map(|x| Xref::new(store.xref(x))),
            payload: match &self.payload {
                Value::None => Payload::None,
                Value::Pointer(p) => Payload::Pointer(Xref::new(store.xref(*p))),
                Value::Text(t) => WriteCx::str(&t.to_str(store)),
            },
            substructures: self
                .children
                .iter()
                .map(|c| c.to_structure(store))
                .collect(),
            line: 0,
        }
    }

    /// The payload, borrowed.
    pub(crate) fn payload_flat<'s>(&'s self, store: &'s Store) -> FlatPayload<'s> {
        match &self.payload {
            Value::None => FlatPayload::None,
            Value::Pointer(p) => FlatPayload::Pointer(Cow::Borrowed(store.xref(*p)), Some(*p)),
            Value::Text(t) if t.is_empty() => FlatPayload::None,
            Value::Text(t) => FlatPayload::Text(t.to_str(store), t.special()),
        }
    }

    /// [`Node::to_structure`], into a flat arena, borrowing the texts.
    pub(crate) fn to_flat<'s>(&'s self, store: &'s Store, out: &mut Flat<'s>) {
        let payload = self.payload_flat(store);
        let standard = u16::try_from(self.tag.get())
            .ok()
            .filter(|&t| usize::from(t) < crate::tree::STANDARD_TAGS.len());
        let at = out.open_standard(
            Cow::Borrowed(store.tag(self.tag)),
            standard,
            self.xref.map(|x| Cow::Borrowed(store.xref(x))),
            payload,
        );
        for c in &self.children {
            c.to_flat(store, out);
        }
        out.close(at);
    }
}

/// The substructures of a typed structure that no field holds, in order.
///
/// Empty, it takes one word and no allocation; it dereferences to a
/// [`ThinVec`] of [`Node`]s.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extra(ThinVec<Node>);

impl Extra {
    /// No node.
    #[must_use]
    pub const fn new() -> Self {
        Self(ThinVec::new())
    }
}

impl Deref for Extra {
    type Target = ThinVec<Node>;

    fn deref(&self) -> &ThinVec<Node> {
        &self.0
    }
}

impl DerefMut for Extra {
    fn deref_mut(&mut self) -> &mut ThinVec<Node> {
        &mut self.0
    }
}

impl From<Vec<Node>> for Extra {
    fn from(nodes: Vec<Node>) -> Self {
        Self(nodes.into())
    }
}

impl FromIterator<Node> for Extra {
    fn from_iter<I: IntoIterator<Item = Node>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<'a> IntoIterator for &'a Extra {
    type Item = &'a Node;
    type IntoIter = std::slice::Iter<'a, Node>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

// ---- Untyped structures in a dataset's tables ---------------------------

use super::table::{CopyCx, Item, Items, List, NodeRow, StoreCx, Tables, View, copy_list, reserve};
use super::text::{Span, TextRef};

impl super::table::sealed::Stored for Node {
    type Row = NodeRow;

    fn empty() -> &'static NodeRow {
        static EMPTY: std::sync::LazyLock<NodeRow> = std::sync::LazyLock::new(NodeRow::default);
        &EMPTY
    }
}

/// The payload of a node row, as a [`Value`].
fn load_value(payload: Option<Span>) -> Value {
    match payload {
        None => Value::None,
        Some(s) => match s.as_pointer() {
            Some(id) => Value::Pointer(id),
            None => Value::Text(Text::from_span(s)),
        },
    }
}

/// A node of the tables, owned.
pub(crate) fn load_node(row: &NodeRow, t: &Tables) -> Node {
    Node {
        tag: row.tag,
        xref: row.xref,
        payload: load_value(row.payload),
        children: row
            .children
            .of(&t.nodes)
            .iter()
            .map(|c| load_node(c, t))
            .collect(),
    }
}

/// The untyped substructures of a structure of the tables, owned.
pub(crate) fn load_extra(list: List<Node>, t: &Tables) -> Extra {
    if list.is_empty() {
        return Extra::new();
    }
    list.of(&t.nodes).iter().map(|n| load_node(n, t)).collect()
}

/// An owned node, stored.
pub(crate) fn store_node(node: &Node, cx: &mut StoreCx<'_>) -> NodeRow {
    let payload = match &node.payload {
        Value::None => None,
        Value::Pointer(id) => Some(Span::pointer(*id)),
        Value::Text(t) => Some(t.to_span(cx.store)),
    };
    NodeRow {
        tag: node.tag,
        xref: node.xref,
        payload,
        children: store_nodes(&node.children, cx),
    }
}

/// Owned nodes, stored as a contiguous run of the table of nodes.
pub(crate) fn store_nodes(nodes: &[Node], cx: &mut StoreCx<'_>) -> List<Node> {
    if nodes.is_empty() {
        return List::EMPTY;
    }
    let start = reserve(&mut cx.tables.nodes, nodes.len(), NodeRow::default());
    for (i, n) in nodes.iter().enumerate() {
        let row = store_node(n, cx);
        if let Some(slot) = cx.tables.nodes.get_mut(start as usize + i) {
            *slot = row;
        }
    }
    List::new(start, u32::try_from(nodes.len()).unwrap_or(u32::MAX))
}

/// The untyped substructures of an owned structure, stored.
pub(crate) fn store_extra(extra: &Extra, cx: &mut StoreCx<'_>) -> List<Node> {
    store_nodes(extra, cx)
}

/// Calls `f` with every pointer of a node of the tables and its
/// substructures.
pub(crate) fn each_node(row: &NodeRow, t: &Tables, f: &mut dyn FnMut(XrefId)) {
    if let Some(id) = row.payload.and_then(Span::as_pointer) {
        f(id);
    }
    for c in row.children.of(&t.nodes) {
        each_node(c, t, f);
    }
}

/// [`each_node`] of a structure's untyped substructures.
pub(crate) fn each_extra(list: List<Node>, t: &Tables, f: &mut dyn FnMut(XrefId)) {
    for n in list.of(&t.nodes) {
        each_node(n, t, f);
    }
}

/// A node of `cx.from`, copied into `cx.to`.
pub(crate) fn copy_node(row: &NodeRow, cx: &mut CopyCx<'_>) -> NodeRow {
    let payload = row.payload.map(|s| match s.as_pointer() {
        Some(id) => Span::pointer(cx.xref(id)),
        None => cx.span(s),
    });
    NodeRow {
        tag: if row.tag == TagId::ASIDE {
            row.tag
        } else {
            cx.tag(row.tag)
        },
        xref: row.xref.map(|x| cx.xref(x)),
        payload,
        children: copy_list::<Node>(row.children, cx),
    }
}

/// A structure's untyped substructures, copied.
pub(crate) fn copy_extra(list: List<Node>, cx: &mut CopyCx<'_>) -> List<Node> {
    copy_list::<Node>(list, cx)
}

/// A node of the tables, into a flat arena, its texts borrowed.
pub(crate) fn node_row_flat<'s>(row: &NodeRow, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
    let payload = row.payload.map_or(FlatPayload::None, |s| cx.span_flat(s));
    let standard = u16::try_from(row.tag.get())
        .ok()
        .filter(|&t| usize::from(t) < crate::tree::STANDARD_TAGS.len());
    let at = out.open_standard(
        Cow::Borrowed(cx.store.tag(row.tag)),
        standard,
        row.xref.map(|x| Cow::Borrowed(cx.store.xref(x))),
        payload,
    );
    // Most untyped structures (extension tags) are leaves.
    if !row.children.is_empty() {
        for c in row.children.of(&cx.tables.nodes) {
            node_row_flat(c, cx, out);
        }
    }
    out.close(at);
}

super::table::listed!([] Node, NodeRow, View<'a, Node>);

impl Item for Node {
    type One = super::table::Idx<Node>;

    fn elems(t: &Tables) -> &[NodeRow] {
        &t.nodes
    }

    fn elems_mut(t: &mut Tables) -> &mut Vec<NodeRow> {
        &mut t.nodes
    }

    fn fill() -> NodeRow {
        NodeRow::default()
    }

    fn read_elem(
        node: super::driver::NodeRef<'_>,
        _at: u32,
        cx: &mut super::driver::ReadCx<'_>,
    ) -> Option<NodeRow> {
        Some(cx.node_row(node))
    }

    fn read_one(
        node: super::driver::NodeRef<'_>,
        cx: &mut super::driver::ReadCx<'_>,
    ) -> Option<Self::One> {
        super::table::read_one_row::<Node>(node, cx)
    }

    fn write_elem<'s>(
        e: &NodeRow,
        _at: u32,
        _tag: super::driver::StdTag,
        cx: &WriteCx<'s>,
        out: &mut Flat<'s>,
    ) {
        node_row_flat(e, cx, out);
    }

    fn write_one<'s>(
        o: &Self::One,
        tag: super::driver::StdTag,
        cx: &WriteCx<'s>,
        out: &mut Flat<'s>,
    ) {
        if let Some(e) = cx.tables.nodes.get(o.get() as usize) {
            Self::write_elem(e, o.get(), tag, cx, out);
        }
    }

    fn load_elem(e: &NodeRow, _at: u32, t: &Tables) -> Self {
        load_node(e, t)
    }

    fn load_one(o: Self::One, t: &Tables) -> Self {
        t.nodes
            .get(o.get() as usize)
            .map_or_else(|| Node::new(TagId::ASIDE), |e| load_node(e, t))
    }

    fn store_elem(&self, _at: u32, cx: &mut StoreCx<'_>) -> NodeRow {
        store_node(self, cx)
    }

    fn store_one(&self, cx: &mut StoreCx<'_>) -> Self::One {
        super::table::store_one_row(self, cx)
    }

    fn each_elem(e: &NodeRow, _at: u32, t: &Tables, f: &mut dyn FnMut(XrefId)) {
        each_node(e, t, f);
    }

    fn each_one(o: Self::One, t: &Tables, f: &mut dyn FnMut(XrefId)) {
        if let Some(e) = t.nodes.get(o.get() as usize) {
            each_node(e, t, f);
        }
    }

    fn copy_elem(e: &NodeRow, _at: u32, cx: &mut CopyCx<'_>, _to: u32) -> NodeRow {
        copy_node(e, cx)
    }

    fn copy_one(o: Self::One, cx: &mut CopyCx<'_>) -> Self::One {
        super::table::copy_one_row::<Node>(o, cx)
    }

    fn view_elem<'a>(e: &'a NodeRow, at: u32, data: &'a super::Dataset) -> View<'a, Node> {
        View::new(data, e, at)
    }

    fn view_one(o: Self::One, data: &super::Dataset) -> View<'_, Node> {
        super::table::view_one_row::<Node>(o, data)
    }
}

impl<'a> View<'a, Node> {
    /// The tag.
    #[must_use]
    pub fn tag(self) -> &'a str {
        self.store().tag(self.row.tag)
    }

    /// The tag's id.
    #[must_use]
    pub fn tag_id(self) -> TagId {
        self.row.tag
    }

    /// The identifier, which a substructure seldom has.
    #[must_use]
    pub fn xref(self) -> Option<XrefId> {
        self.row.xref
    }

    /// The payload.
    #[must_use]
    pub fn payload(self) -> Value {
        load_value(self.row.payload)
    }

    /// The text payload, when it is one.
    #[must_use]
    pub fn text(self) -> Option<TextRef<'a>> {
        self.row
            .payload
            .filter(|s| s.as_pointer().is_none())
            .map(|s| TextRef::new(self.store(), s))
    }

    /// The pointer payload, when it is one.
    #[must_use]
    pub fn pointer(self) -> Option<XrefId> {
        self.row.payload.and_then(Span::as_pointer)
    }

    /// The substructures, in order.
    #[must_use]
    pub fn children(self) -> Items<'a, Node> {
        Items::new(
            self.data,
            self.row.children.of(&self.tables().nodes),
            self.row.children.start,
        )
    }

    /// The node as an owned [`Structure`], read from the store.
    #[must_use]
    pub fn to_structure(self) -> Structure {
        self.into_owned().to_structure(self.store())
    }
}

impl std::fmt::Debug for View<'_, Node> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("tag", &self.tag())
            .field("xref", &self.xref())
            .field("payload", &self.payload())
            .field("children", &self.children().len())
            .finish()
    }
}
