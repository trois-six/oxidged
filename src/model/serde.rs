//! `Serialize` and `Deserialize` for the typed model (feature `serde`).
//!
//! A text or an identifier of the model means something only with the store
//! it points into, so the model is serialized with its store: a
//! [`Dataset`] (and a [`StreamedRecord`](crate::StreamedRecord)) serializes
//! its records with their texts and identifiers as strings, and a dataset
//! deserializes into a fresh store, its texts owned and its identifiers
//! interned. The traits below carry the store through every type; the
//! macros that declare the model implement them for its structures.
//!
//! Structures nest to any depth only where they hold each other: notes and
//! citations ([`Chain`]), and untyped nodes. Both are serialized flat, so
//! that the JSON of any dataset nests a bounded number of levels and reads
//! back within the nesting limit of deserializers (128 for `serde_json`):
//! a list of notes or citations lists each note and citation after the one
//! that holds it, with its depth ([`serialize_chain`]), and a node lists its
//! descendants with their levels ([`Node`]'s implementation).

use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::version::GedcomVersion;

use super::dataset::{Dataset, Record};
use super::list::ThinVec;
use super::node::{Extra, Node, Value};
use super::text::{Store, TagId, Text, XrefId};

/// A value of the model, serialized with the store its texts point into.
pub(crate) trait SerializeIn {
    /// Serializes the value, its texts and identifiers read from `store`.
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error>;

    /// Whether the value is empty: a field that is, is left out.
    fn is_empty_in(&self) -> bool {
        false
    }

    /// Serializes a structure as [`SerializeIn::serialize_in`] does, but
    /// for the field named `skip` ([`serialize_chain`]).
    fn serialize_but<S: Serializer>(
        &self,
        store: &Store,
        s: S,
        skip: &str,
    ) -> Result<S::Ok, S::Error> {
        let _ = skip;
        self.serialize_in(store, s)
    }

    /// Serializes a list of values: each in turn, unless the type says
    /// otherwise.
    fn serialize_list<S: Serializer>(items: &[Self], store: &Store, s: S) -> Result<S::Ok, S::Error>
    where
        Self: Sized,
    {
        serialize_items(items, store, s)
    }
}

/// A value of the model, deserialized into a store.
pub(crate) trait DeserializeIn: Sized {
    /// Deserializes the value, its identifiers and tags interned in
    /// `store`, its texts owned.
    fn deserialize_in<'de, D: Deserializer<'de>>(store: &mut Store, d: D)
    -> Result<Self, D::Error>;

    /// Deserializes a list of values, each given to `push`, as
    /// [`SerializeIn::serialize_list`] writes it.
    fn deserialize_list<'de, D: Deserializer<'de>>(
        store: &mut Store,
        d: D,
        push: &mut dyn FnMut(Self),
    ) -> Result<(), D::Error> {
        deserialize_items(store, d, push)
    }
}

/// A value and its store, to serialize.
pub(crate) struct In<'a, T: ?Sized>(pub(crate) &'a Store, pub(crate) &'a T);

impl<T: SerializeIn + ?Sized> Serialize for In<'_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.1.serialize_in(self.0, s)
    }
}

/// A store to deserialize a value of type `T` into.
pub(crate) struct Seed<'s, T>(&'s mut Store, PhantomData<T>);

impl<'s, T> Seed<'s, T> {
    pub(crate) fn new(store: &'s mut Store) -> Self {
        Self(store, PhantomData)
    }
}

impl<'de, T: DeserializeIn> DeserializeSeed<'de> for Seed<'_, T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<T, D::Error> {
        T::deserialize_in(self.0, d)
    }
}

/// A string, map key or value: borrowed when the format can.
pub(crate) struct Str<'de>(pub(crate) Cow<'de, str>);

impl<'de> Deserialize<'de> for Str<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Str<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Str<'de>, E> {
                Ok(Str(Cow::Borrowed(v)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Str<'de>, E> {
                Ok(Str(Cow::Owned(v.to_string())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Str<'de>, E> {
                Ok(Str(Cow::Owned(v)))
            }
        }
        d.deserialize_str(V)
    }
}

/// Skips the value of a map entry no field takes.
pub(crate) fn skip<'de, A: MapAccess<'de>>(map: &mut A) -> Result<(), A::Error> {
    map.next_value::<IgnoredAny>().map(|_| ())
}

/// An enumeration value no variant names: `{"Unknown": "text"}`.
pub(crate) fn serialize_unknown<S: Serializer>(
    text: &Text,
    store: &Store,
    s: S,
) -> Result<S::Ok, S::Error> {
    let mut map = s.serialize_map(Some(1))?;
    map.serialize_entry("Unknown", &In(store, text))?;
    map.end()
}

/// An enumeration value: a variant's name, or `{"Unknown": "text"}`.
pub(crate) fn deserialize_enum<'de, D: Deserializer<'de>, E>(
    d: D,
    names: &'static [&'static str],
    known: fn(&str) -> Option<E>,
    unknown: fn(Text) -> E,
) -> Result<E, D::Error> {
    struct V<E> {
        names: &'static [&'static str],
        known: fn(&str) -> Option<E>,
        unknown: fn(Text) -> E,
    }
    impl<'de, E> Visitor<'de> for V<E> {
        type Value = E;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a variant's name or {\"Unknown\": text}")
        }
        fn visit_str<Er: de::Error>(self, v: &str) -> Result<E, Er> {
            (self.known)(v).ok_or_else(|| de::Error::unknown_variant(v, self.names))
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<E, A::Error> {
            let mut out = None;
            while let Some(key) = map.next_key::<Str<'de>>()? {
                if &*key.0 == "Unknown" {
                    out = Some((self.unknown)(Text::new(map.next_value::<Str<'de>>()?.0)));
                } else {
                    return Err(de::Error::unknown_variant(&key.0, &["Unknown"]));
                }
            }
            out.ok_or_else(|| de::Error::missing_field("Unknown"))
        }
    }
    d.deserialize_any(V {
        names,
        known,
        unknown,
    })
}

/// A value of a variant that holds one value: `{"Variant": value}`.
pub(crate) fn serialize_tagged<S: Serializer, T: SerializeIn>(
    variant: &'static str,
    value: &T,
    store: &Store,
    s: S,
) -> Result<S::Ok, S::Error> {
    let mut map = s.serialize_map(Some(1))?;
    map.serialize_entry(variant, &In(store, value))?;
    map.end()
}

/// A value of one of two variants that hold one value each:
/// `{"A": a}` or `{"B": b}`.
pub(crate) fn deserialize_either<'de, D, A, B, T>(
    store: &mut Store,
    d: D,
    names: &'static [&'static str; 2],
    a: fn(A) -> T,
    b: fn(B) -> T,
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    A: DeserializeIn,
    B: DeserializeIn,
{
    struct V<'s, A, B, T> {
        store: &'s mut Store,
        names: &'static [&'static str; 2],
        a: fn(A) -> T,
        b: fn(B) -> T,
    }
    impl<'de, A: DeserializeIn, B: DeserializeIn, T> Visitor<'de> for V<'_, A, B, T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "{{\"{}\": …}} or {{\"{}\": …}}",
                self.names[0], self.names[1]
            )
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<T, M::Error> {
            let mut out = None;
            while let Some(key) = map.next_key::<Str<'de>>()? {
                if *key.0 == *self.names[0] {
                    out = Some((self.a)(map.next_value_seed(Seed::new(&mut *self.store))?));
                } else if *key.0 == *self.names[1] {
                    out = Some((self.b)(map.next_value_seed(Seed::new(&mut *self.store))?));
                } else {
                    return Err(de::Error::unknown_variant(&key.0, self.names));
                }
            }
            out.ok_or_else(|| de::Error::invalid_length(0, &"one variant"))
        }
    }
    d.deserialize_map(V { store, names, a, b })
}

impl SerializeIn for Text {
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_str(store))
    }

    fn is_empty_in(&self) -> bool {
        self.is_empty()
    }
}

impl DeserializeIn for Text {
    fn deserialize_in<'de, D: Deserializer<'de>>(_: &mut Store, d: D) -> Result<Self, D::Error> {
        Ok(Text::new(Str::deserialize(d)?.0))
    }
}

impl SerializeIn for XrefId {
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(store.xref(*self))
    }
}

impl DeserializeIn for XrefId {
    fn deserialize_in<'de, D: Deserializer<'de>>(
        store: &mut Store,
        d: D,
    ) -> Result<Self, D::Error> {
        let xref = Str::deserialize(d)?;
        store
            .intern_xref(&xref.0)
            .ok_or_else(|| de::Error::custom("too many identifiers"))
    }
}

impl SerializeIn for u32 {
    fn serialize_in<S: Serializer>(&self, _: &Store, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(*self)
    }
}

impl DeserializeIn for u32 {
    fn deserialize_in<'de, D: Deserializer<'de>>(_: &mut Store, d: D) -> Result<Self, D::Error> {
        u32::deserialize(d)
    }
}

impl<T: SerializeIn> SerializeIn for Box<T> {
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        (**self).serialize_in(store, s)
    }

    fn is_empty_in(&self) -> bool {
        (**self).is_empty_in()
    }
}

impl<T: DeserializeIn> DeserializeIn for Box<T> {
    fn deserialize_in<'de, D: Deserializer<'de>>(
        store: &mut Store,
        d: D,
    ) -> Result<Self, D::Error> {
        T::deserialize_in(store, d).map(Box::new)
    }
}

impl<T: SerializeIn> SerializeIn for Option<T> {
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Some(v) => s.serialize_some(&In(store, v)),
            None => s.serialize_none(),
        }
    }

    fn is_empty_in(&self) -> bool {
        self.is_none()
    }
}

impl<T: DeserializeIn> DeserializeIn for Option<T> {
    fn deserialize_in<'de, D: Deserializer<'de>>(
        store: &mut Store,
        d: D,
    ) -> Result<Self, D::Error> {
        struct V<'s, T>(&'s mut Store, PhantomData<T>);
        impl<'de, T: DeserializeIn> Visitor<'de> for V<'_, T> {
            type Value = Option<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an optional value")
            }
            fn visit_none<E: de::Error>(self) -> Result<Option<T>, E> {
                Ok(None)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Option<T>, E> {
                Ok(None)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Option<T>, D::Error> {
                T::deserialize_in(self.0, d).map(Some)
            }
        }
        d.deserialize_option(V(store, PhantomData))
    }
}

/// Serializes the items of a list.
fn serialize_items<'a, T: SerializeIn + 'a, S: Serializer>(
    items: &'a [T],
    store: &Store,
    s: S,
) -> Result<S::Ok, S::Error> {
    let mut seq = s.serialize_seq(Some(items.len()))?;
    for item in items {
        seq.serialize_element(&In(store, item))?;
    }
    seq.end()
}

/// Deserializes the items of a list, each given to `push`.
fn deserialize_items<'de, T: DeserializeIn, D: Deserializer<'de>>(
    store: &mut Store,
    d: D,
    push: &mut dyn FnMut(T),
) -> Result<(), D::Error> {
    struct V<'s, 'p, T>(&'s mut Store, &'p mut dyn FnMut(T));
    impl<'de, T: DeserializeIn> Visitor<'de> for V<'_, '_, T> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a list")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while let Some(item) = seq.next_element_seed(Seed::<T>::new(&mut *self.0))? {
                (self.1)(item);
            }
            Ok(())
        }
    }
    d.deserialize_seq(V(store, push))
}

impl<T: SerializeIn> SerializeIn for [T] {
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        T::serialize_list(self, store, s)
    }

    fn is_empty_in(&self) -> bool {
        self.is_empty()
    }
}

impl<T: SerializeIn> SerializeIn for Vec<T> {
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        T::serialize_list(self, store, s)
    }

    fn is_empty_in(&self) -> bool {
        self.is_empty()
    }
}

impl<T: DeserializeIn> DeserializeIn for Vec<T> {
    fn deserialize_in<'de, D: Deserializer<'de>>(
        store: &mut Store,
        d: D,
    ) -> Result<Self, D::Error> {
        let mut out = Vec::new();
        T::deserialize_list(store, d, &mut |item| out.push(item))?;
        out.shrink_to_fit();
        Ok(out)
    }
}

impl<T: SerializeIn> SerializeIn for ThinVec<T> {
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        T::serialize_list(self, store, s)
    }

    fn is_empty_in(&self) -> bool {
        self.is_empty()
    }
}

impl<T: DeserializeIn> DeserializeIn for ThinVec<T> {
    fn deserialize_in<'de, D: Deserializer<'de>>(
        store: &mut Store,
        d: D,
    ) -> Result<Self, D::Error> {
        let mut out = ThinVec::new();
        T::deserialize_list(store, d, &mut |item| out.push(item))?;
        Ok(out)
    }
}

impl SerializeIn for Extra {
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        serialize_items(self, store, s)
    }

    fn is_empty_in(&self) -> bool {
        self.is_empty()
    }
}

impl DeserializeIn for Extra {
    fn deserialize_in<'de, D: Deserializer<'de>>(
        store: &mut Store,
        d: D,
    ) -> Result<Self, D::Error> {
        let mut out = Extra::new();
        deserialize_items(store, d, &mut |item| out.push(item))?;
        Ok(out)
    }
}

/// The tag, identifier and payload of a node, the payload a typed
/// structure keeps aside having no tag: the keys a node's map and each of
/// its descendants' have.
fn node_entries<M: SerializeMap>(map: &mut M, node: &Node, store: &Store) -> Result<(), M::Error> {
    if node.tag != TagId::ASIDE {
        map.serialize_entry("tag", store.tag(node.tag))?;
    }
    if let Some(xref) = node.xref {
        map.serialize_entry("xref", store.xref(xref))?;
    }
    match &node.payload {
        Value::None => {}
        Value::Pointer(p) => map.serialize_entry("pointer", store.xref(*p))?,
        Value::Text(t) if t.is_empty() => {}
        Value::Text(t) => map.serialize_entry("text", &*t.to_str(store))?,
    }
    Ok(())
}

/// The number of [`node_entries`] of `node`.
fn node_entry_count(node: &Node) -> usize {
    usize::from(node.tag != TagId::ASIDE)
        + usize::from(node.xref.is_some())
        + usize::from(match &node.payload {
            Value::None => false,
            Value::Pointer(_) => true,
            Value::Text(t) => !t.is_empty(),
        })
}

/// A descendant of a node, its level below it (1 for a child), without
/// its own substructures: they follow it.
struct Descendant<'a>(&'a Store, &'a Node, u32);

impl Serialize for Descendant<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Descendant(store, node, level) = *self;
        let mut map = s.serialize_map(Some(1 + node_entry_count(node)))?;
        map.serialize_entry("level", &level)?;
        node_entries(&mut map, node, store)?;
        map.end()
    }
}

/// The descendants of a node, depth first, each with its level.
struct Descendants<'a>(&'a Store, &'a Node);

impl Serialize for Descendants<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Descendants(store, node) = *self;
        let mut seq = s.serialize_seq(None)?;
        // The children not visited yet of each open level.
        let mut open = vec![node.children.iter()];
        while let Some(children) = open.last_mut() {
            let Some(child) = children.next() else {
                open.pop();
                continue;
            };
            let level = u32::try_from(open.len()).unwrap_or(u32::MAX);
            seq.serialize_element(&Descendant(store, child, level))?;
            open.push(child.children.iter());
        }
        seq.end()
    }
}

/// A node: `{"tag", "xref", "pointer" or "text", "descendants"}`, its
/// descendants listed depth first, each with its `"level"` below the node
/// (1 for a child) and its tag, identifier and payload, so that the nodes
/// nest no deeper in JSON however deep they are; the payload or identifier
/// a typed structure keeps aside has no tag.
impl SerializeIn for Node {
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        let count = node_entry_count(self) + usize::from(!self.children.is_empty());
        let mut map = s.serialize_map(Some(count))?;
        node_entries(&mut map, self, store)?;
        if !self.children.is_empty() {
            map.serialize_entry("descendants", &Descendants(store, self))?;
        }
        map.end()
    }
}

/// Reads the tag, identifier or payload of a node under `key`; whether
/// `key` names one.
fn node_entry<'de, A: MapAccess<'de>>(
    node: &mut Node,
    key: &str,
    map: &mut A,
    store: &mut Store,
) -> Result<bool, A::Error> {
    match key {
        "tag" => node.tag = store.intern_tag(&map.next_value::<Str<'de>>()?.0),
        "xref" => node.xref = map.next_value_seed(Seed::new(&mut *store))?,
        "pointer" => node.payload = Value::Pointer(map.next_value_seed(Seed::new(&mut *store))?),
        "text" => node.payload = Value::Text(map.next_value_seed(Seed::new(&mut *store))?),
        _ => return Ok(false),
    }
    Ok(true)
}

/// The descendants of a node, as [`Descendants`] lists them, into the
/// children of the node they belong to.
struct DescendantsSeed<'s>(&'s mut Store);

impl<'de> DeserializeSeed<'de> for DescendantsSeed<'_> {
    type Value = Vec<Node>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Vec<Node>, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for DescendantsSeed<'_> {
    type Value = Vec<Node>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a node's descendants")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<Node>, A::Error> {
        // The open nodes, the node itself first, as a stand-in.
        let mut open = vec![Node::new(TagId::ASIDE)];
        while let Some((level, node)) = seq.next_element_seed(DescendantSeed(&mut *self.0))? {
            if level == 0 || level > open.len() {
                return Err(de::Error::custom(format!(
                    "a descendant at level {level} under one at level {}",
                    open.len() - 1
                )));
            }
            close_nodes(&mut open, level);
            open.push(node);
        }
        close_nodes(&mut open, 1);
        Ok(open.pop().map(|root| root.children).unwrap_or_default())
    }
}

/// Closes the open nodes down to `len` of them, each a child of the one
/// before it.
fn close_nodes(open: &mut Vec<Node>, len: usize) {
    while open.len() > len {
        let Some(node) = open.pop() else { break };
        if let Some(parent) = open.last_mut() {
            parent.children.push(node);
        }
    }
}

/// A descendant of a node, as [`Descendant`] writes it: its level and the
/// node.
struct DescendantSeed<'s>(&'s mut Store);

impl<'de> DeserializeSeed<'de> for DescendantSeed<'_> {
    type Value = (usize, Node);

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(usize, Node), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for DescendantSeed<'_> {
    type Value = (usize, Node);

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a node's descendant")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(usize, Node), A::Error> {
        let store = self.0;
        let (mut node, mut level) = (Node::new(TagId::ASIDE), None);
        while let Some(key) = map.next_key::<Str<'de>>()? {
            if &*key.0 == "level" {
                level = Some(map.next_value::<u32>()?);
            } else if !node_entry(&mut node, &key.0, &mut map, store)? {
                skip(&mut map)?;
            }
        }
        let level = level.ok_or_else(|| de::Error::missing_field("level"))?;
        Ok((usize::try_from(level).unwrap_or(usize::MAX), node))
    }
}

impl DeserializeIn for Node {
    fn deserialize_in<'de, D: Deserializer<'de>>(
        store: &mut Store,
        d: D,
    ) -> Result<Self, D::Error> {
        struct V<'s>(&'s mut Store);
        impl<'de> Visitor<'de> for V<'_> {
            type Value = Node;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a node")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
                let store = self.0;
                let mut node = Node::new(TagId::ASIDE);
                while let Some(key) = map.next_key::<Str<'de>>()? {
                    if &*key.0 == "descendants" {
                        node.children = map.next_value_seed(DescendantsSeed(&mut *store))?;
                    } else if !node_entry(&mut node, &key.0, &mut map, store)? {
                        skip(&mut map)?;
                    }
                }
                Ok(node)
            }
        }
        d.deserialize_map(V(store))
    }
}

/// A type whose structures hold, in one of their fields ([`Chain::LINK`]),
/// structures of another type that hold structures of this one in turn:
/// notes hold citations (their sources), citations hold notes, to any
/// depth.
pub(crate) trait Chain: SerializeIn + DeserializeIn + Sized {
    /// The type of the structures held.
    type Other: Chain<Other = Self>;
    /// The name of the field that holds them.
    const LINK: &'static str;
    /// The structures held.
    fn links(&self) -> &[Self::Other];
    /// The structures held, to add to.
    fn links_mut(&mut self) -> &mut Vec<Self::Other>;
}

/// A structure of a chain as an entry of its flattened list.
struct Entry<'a, T>(&'a Store, &'a T, u32);

impl<T: Chain> Serialize for Entry<'_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Entry(store, value, depth) = *self;
        if depth == 0 {
            value.serialize_but(store, s, T::LINK)
        } else {
            let mut seq = s.serialize_seq(Some(2))?;
            seq.serialize_element(&depth)?;
            seq.serialize_element(&EntryFields(store, value))?;
            seq.end()
        }
    }
}

/// The fields of a structure of a chain, but the one that holds the next
/// structures.
struct EntryFields<'a, T>(&'a Store, &'a T);

impl<T: Chain> Serialize for EntryFields<'_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.1.serialize_but(self.0, s, T::LINK)
    }
}

/// Serializes `items`, structures of a [`Chain`], with every structure they
/// hold, depth first, in one flat list: an item is the map of its fields
/// but the one that holds the next structures, and each structure held
/// follows the one that holds it as `[depth, {fields}]`, `depth` counting
/// from the items' (1 for what an item holds); its type follows from its
/// depth, the items' type at even depths, the other at odd ones. The JSON
/// of a note held in a citation held in a note… thus nests no deeper than
/// one note.
pub(crate) fn serialize_chain<T: Chain, S: Serializer>(
    items: &[T],
    store: &Store,
    s: S,
) -> Result<S::Ok, S::Error> {
    let mut seq = s.serialize_seq(None)?;
    chain_entries(&mut seq, items, store, 0)?;
    seq.end()
}

/// Serializes `items`, at `depth`, and what they hold, into `seq`.
fn chain_entries<T: Chain, M: SerializeSeq>(
    seq: &mut M,
    items: &[T],
    store: &Store,
    depth: u32,
) -> Result<(), M::Error> {
    for item in items {
        seq.serialize_element(&Entry(store, item, depth))?;
        chain_entries(seq, item.links(), store, depth.saturating_add(1))?;
    }
    Ok(())
}

/// A structure of a chain being read: of the list's type or the other.
enum Open<T: Chain> {
    This(T),
    Other(T::Other),
}

/// Deserializes a list [`serialize_chain`] wrote: each item of the list's
/// type to `push`, with what it holds.
pub(crate) fn deserialize_chain<'de, T: Chain, D: Deserializer<'de>>(
    store: &mut Store,
    d: D,
    push: &mut dyn FnMut(T),
) -> Result<(), D::Error> {
    struct V<'s, 'p, T>(&'s mut Store, &'p mut dyn FnMut(T));
    impl<'de, T: Chain> Visitor<'de> for V<'_, '_, T> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a list of notes or citations")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            let V(store, push) = self;
            // The structures open, one per depth.
            let mut open: Vec<Open<T>> = Vec::new();
            loop {
                let seed = EntrySeed::<T>(&mut *store, open.len(), PhantomData);
                let Some((depth, entry)) = seq.next_element_seed(seed)? else {
                    break;
                };
                close_chain(&mut open, depth, push);
                open.push(entry);
            }
            close_chain(&mut open, 0, push);
            Ok(())
        }
    }
    d.deserialize_seq(V(store, push))
}

/// Closes the open structures of a chain down to `len` of them, each held
/// by the one before it, the first given to `push`.
fn close_chain<T: Chain>(open: &mut Vec<Open<T>>, len: usize, push: &mut dyn FnMut(T)) {
    while open.len() > len {
        let Some(last) = open.pop() else { break };
        match (last, open.last_mut()) {
            (Open::This(t), None) => push(t),
            (Open::This(t), Some(Open::Other(o))) => o.links_mut().push(t),
            (Open::Other(o), Some(Open::This(t))) => t.links_mut().push(o),
            // The types alternate with the depth ([`EntrySeed`]): nothing
            // else is ever open.
            (Open::Other(_), None | Some(Open::Other(_)))
            | (Open::This(_), Some(Open::This(_))) => {}
        }
    }
}

/// An entry of a chain's list, `open` structures being open: its depth and
/// the structure, of the list's type at even depths.
struct EntrySeed<'s, T>(&'s mut Store, usize, PhantomData<T>);

impl<'de, T: Chain> DeserializeSeed<'de> for EntrySeed<'_, T> {
    type Value = (usize, Open<T>);

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de, T: Chain> Visitor<'de> for EntrySeed<'_, T> {
    type Value = (usize, Open<T>);

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a note or citation, or [depth, note or citation]")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        let item = T::deserialize_in(self.0, de::value::MapAccessDeserializer::new(map))?;
        Ok((0, Open::This(item)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let EntrySeed(store, open, _) = self;
        let depth: u32 = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &"[depth, fields]"))?;
        let depth = usize::try_from(depth).unwrap_or(usize::MAX);
        if depth == 0 || depth > open {
            return Err(de::Error::custom(format!(
                "an entry at depth {depth} after one at depth {}",
                open.saturating_sub(1)
            )));
        }
        let missing = || de::Error::invalid_length(1, &"[depth, fields]");
        let entry = if depth % 2 == 0 {
            Open::This(
                seq.next_element_seed(Seed::<T>::new(store))?
                    .ok_or_else(missing)?,
            )
        } else {
            Open::Other(
                seq.next_element_seed(Seed::<T::Other>::new(store))?
                    .ok_or_else(missing)?,
            )
        };
        if seq.next_element::<IgnoredAny>()?.is_some() {
            return Err(de::Error::invalid_length(3, &"[depth, fields]"));
        }
        Ok((depth, entry))
    }
}

/// A record, externally tagged by its type: `{"Individual": {…}}`.
impl SerializeIn for Record {
    fn serialize_in<S: Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(1))?;
        match self {
            Record::Header(r) => map.serialize_entry("Header", &In(store, r))?,
            Record::Individual(r) => map.serialize_entry("Individual", &In(store, r))?,
            Record::Family(r) => map.serialize_entry("Family", &In(store, r))?,
            Record::Source(r) => map.serialize_entry("Source", &In(store, r))?,
            Record::Repository(r) => map.serialize_entry("Repository", &In(store, r))?,
            Record::Multimedia(r) => map.serialize_entry("Multimedia", &In(store, r))?,
            Record::Submitter(r) => map.serialize_entry("Submitter", &In(store, r))?,
            Record::Submission(r) => map.serialize_entry("Submission", &In(store, r))?,
            Record::Note(r) => map.serialize_entry("Note", &In(store, r))?,
            Record::Other(r) => map.serialize_entry("Other", &In(store, r))?,
        }
        map.end()
    }
}

/// The records of a dataset's table, serialized one at a time, each
/// copied out as its owned type.
struct Records<'a, T: super::Listed>(super::Items<'a, T>);

impl<T: super::Listed + SerializeIn> Serialize for Records<'_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let store = self.0.dataset().store();
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for record in self.0.into_owned_iter() {
            seq.serialize_element(&In(store, &record))?;
        }
        seq.end()
    }
}

/// Serializes a list of records under `name`, unless it is empty.
fn records<M: SerializeMap, T: super::Listed + SerializeIn>(
    map: &mut M,
    name: &'static str,
    list: super::Items<'_, T>,
) -> Result<(), M::Error> {
    if list.is_empty() {
        return Ok(());
    }
    map.serialize_entry(name, &Records(list))
}

impl Serialize for Dataset {
    /// The dataset as a map: `version` (`5.5.1`, `7.0` or `7.1`),
    /// `declared_version` (`HEAD.GEDC.VERS` as read, when the file has
    /// one), `header`, then the lists of records that are not empty, in the
    /// order they are written.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let store = &self.store;
        let lists = [
            self.submitters().is_empty(),
            self.submissions().is_empty(),
            self.individuals().is_empty(),
            self.families().is_empty(),
            self.notes().is_empty(),
            self.sources().is_empty(),
            self.repositories().is_empty(),
            self.multimedia().is_empty(),
            self.extra().is_empty(),
        ];
        let count = 1
            + usize::from(self.declared_version.is_some())
            + usize::from(self.header.is_some())
            + lists.iter().filter(|empty| !**empty).count();
        let mut map = s.serialize_map(Some(count))?;
        map.serialize_entry("version", self.version.as_str())?;
        if let Some(declared) = &self.declared_version {
            map.serialize_entry("declared_version", &**declared)?;
        }
        if let Some(header) = &self.header {
            map.serialize_entry("header", &In(store, header))?;
        }
        records(&mut map, "submitters", self.submitters())?;
        records(&mut map, "submissions", self.submissions())?;
        records(&mut map, "individuals", self.individuals())?;
        records(&mut map, "families", self.families())?;
        records(&mut map, "notes", self.notes())?;
        records(&mut map, "sources", self.sources())?;
        records(&mut map, "repositories", self.repositories())?;
        records(&mut map, "multimedia", self.multimedia())?;
        records(&mut map, "extra", self.extra())?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Dataset {
    /// A dataset as [`Serialize`] writes it, into a fresh store: its texts
    /// owned, its identifiers and tags interned.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Dataset;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a dataset")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Dataset, A::Error> {
                let mut data = Dataset::default();
                let mut store = Store::default();
                let mut lists = Lists::default();
                let s = &mut store;
                while let Some(key) = map.next_key::<Str<'de>>()? {
                    match &*key.0 {
                        "version" => {
                            let v = map.next_value::<Str<'de>>()?;
                            data.version = match &*v.0 {
                                "5.5.1" => GedcomVersion::V5_5_1,
                                "7.0" => GedcomVersion::V7_0,
                                "7.1" => GedcomVersion::V7_1,
                                other => {
                                    return Err(de::Error::invalid_value(
                                        de::Unexpected::Str(other),
                                        &"5.5.1, 7.0 or 7.1",
                                    ));
                                }
                            };
                        }
                        "declared_version" => {
                            data.declared_version = Some(map.next_value::<Str<'de>>()?.0.into());
                        }
                        "header" => data.header = map.next_value_seed(Seed::new(&mut *s))?,
                        "submitters" => {
                            lists.submitters = map.next_value_seed(Seed::new(&mut *s))?;
                        }
                        "submissions" => {
                            lists.submissions = map.next_value_seed(Seed::new(&mut *s))?;
                        }
                        "individuals" => {
                            lists.individuals = map.next_value_seed(Seed::new(&mut *s))?;
                        }
                        "families" => lists.families = map.next_value_seed(Seed::new(&mut *s))?,
                        "notes" => lists.notes = map.next_value_seed(Seed::new(&mut *s))?,
                        "sources" => lists.sources = map.next_value_seed(Seed::new(&mut *s))?,
                        "repositories" => {
                            lists.repositories = map.next_value_seed(Seed::new(&mut *s))?;
                        }
                        "multimedia" => {
                            lists.multimedia = map.next_value_seed(Seed::new(&mut *s))?;
                        }
                        "extra" => lists.extra = map.next_value_seed(Seed::new(&mut *s))?,
                        _ => skip(&mut map)?,
                    }
                }
                data.store = store;
                lists.store_in(&mut data);
                data.store.shrink();
                data.shrink();
                data.reindex();
                Ok(data)
            }
        }
        d.deserialize_map(V)
    }
}

/// The records of a dataset being deserialized, owned, before they are
/// stored in its tables.
#[derive(Default)]
struct Lists {
    submitters: Vec<super::Submitter>,
    submissions: Vec<super::Submission>,
    individuals: Vec<super::Individual>,
    families: Vec<super::Family>,
    notes: Vec<super::SharedNote>,
    sources: Vec<super::Source>,
    repositories: Vec<super::Repository>,
    multimedia: Vec<super::Multimedia>,
    extra: Vec<Node>,
}

impl Lists {
    /// Stores every record in `data`'s tables, in order.
    fn store_in(self, data: &mut Dataset) {
        fn store<T: super::edit::RecordRows>(data: &mut Dataset, list: Vec<T>) {
            for r in list {
                data.store_record(&r, None);
            }
        }
        store(data, self.submitters);
        store(data, self.submissions);
        store(data, self.individuals);
        store(data, self.families);
        store(data, self.notes);
        store(data, self.sources);
        store(data, self.repositories);
        store(data, self.multimedia);
        store(data, self.extra);
    }
}

impl Serialize for crate::StreamedRecord {
    /// The record as a map: `version`, `line` and `record`, externally
    /// tagged by its type (`{"Individual": {…}}`).
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (record, store) = (self.record().into_owned(), self.store());
        let mut map = s.serialize_map(Some(3))?;
        map.serialize_entry("version", self.version().as_str())?;
        map.serialize_entry("line", &self.line())?;
        map.serialize_entry("record", &In(store, &record))?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dataset_round_trips() {
        let data = Dataset::parse("0 HEAD\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n");
        let json = serde_json::to_string(&data).unwrap();
        assert_eq!(
            json,
            r#"{"version":"5.5.1","header":{},"individuals":[{"xref":"@I1@","names":[{"value":"Ann /Example/"}]}]}"#
        );
        let back: Dataset = serde_json::from_str(&json).unwrap();
        assert_eq!(back.to_structures(), data.to_structures());
    }
}
