//! The structures interpretation and export rewrite: a [`Structure`] whose
//! texts stay borrowed from what they were read from (the text of a file,
//! a parsed tree, a dataset's store) until a rule changes them.
//!
//! A record is copied into drafts only when a rule may change it, and only
//! its tree is allocated: its tags, identifiers and payloads are the
//! source's own characters. A draft written back ([`Draft::write_gedcom`])
//! or handed to the writer (as a [`Node`]) is never made an owned
//! [`Structure`] on the way.

use std::borrow::Cow;
use std::fmt;

use crate::GedcomVersion;
use crate::model::driver::NodeRef;
use crate::tree::{
    Escaping, Node, Payload, PayloadRef, Structure, StructureRef, Tag, Xref, standard_index,
};
use crate::tree::{Flat, FlatPayload, TextPiece};

/// The tag of a [`Draft`].
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct DraftTag<'s>(Cow<'s, str>);

impl<'s> DraftTag<'s> {
    /// A tag of borrowed text.
    #[inline]
    pub(crate) fn new(tag: &'s str) -> Self {
        Self(Cow::Borrowed(tag))
    }

    /// A tag of owned text.
    pub(crate) fn owned(tag: String) -> Self {
        Self(Cow::Owned(tag))
    }

    /// The tag's text.
    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is an extension tag: one that starts with `_`.
    #[inline]
    pub(crate) fn is_extension(&self) -> bool {
        self.0.starts_with('_')
    }

    /// The tag with its text owned.
    pub(crate) fn into_owned(self) -> DraftTag<'static> {
        DraftTag(Cow::Owned(self.0.into_owned()))
    }
}

impl AsRef<str> for DraftTag<'_> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DraftTag<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for DraftTag<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<str> for DraftTag<'_> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for DraftTag<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// The payload of a [`Draft`]: none, a pointer (delimiters included) or a
/// text (unescaped, continuations joined).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Value<'s> {
    #[default]
    None,
    Pointer(Cow<'s, str>),
    Text(Cow<'s, str>),
}

impl<'s> Value<'s> {
    /// The pointer or the text; `None` when there is none.
    #[inline]
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Value::None => None,
            Value::Pointer(s) | Value::Text(s) => Some(s),
        }
    }

    /// The payload, borrowed.
    #[inline]
    pub(crate) fn borrowed(&self) -> PayloadRef<'_> {
        match self {
            Value::None => PayloadRef::None,
            Value::Pointer(p) => PayloadRef::Pointer(p),
            Value::Text(t) => PayloadRef::Text(t),
        }
    }

    /// A payload borrowed from `p`.
    #[inline]
    pub(crate) fn of(p: PayloadRef<'s>) -> Self {
        match p {
            PayloadRef::None => Value::None,
            PayloadRef::Pointer(x) => Value::Pointer(Cow::Borrowed(x)),
            PayloadRef::Text(t) => Value::Text(Cow::Borrowed(t)),
        }
    }

    fn into_owned(self) -> Value<'static> {
        match self {
            Value::None => Value::None,
            Value::Pointer(p) => Value::Pointer(Cow::Owned(p.into_owned())),
            Value::Text(t) => Value::Text(Cow::Owned(t.into_owned())),
        }
    }

    fn to_payload(&self) -> Payload {
        match self {
            Value::None => Payload::None,
            Value::Pointer(p) => Payload::Pointer(Xref::new(&**p)),
            Value::Text(t) => Payload::Text((&**t).into()),
        }
    }
}

/// A structure being rewritten. See the [module documentation](self).
///
/// Equality ignores [`line`](Self::line), as [`Structure`]'s does.
#[derive(Clone, Default)]
pub(crate) struct Draft<'s> {
    pub(crate) tag: DraftTag<'s>,
    pub(crate) xref: Option<Cow<'s, str>>,
    pub(crate) payload: Value<'s>,
    pub(crate) substructures: Vec<Draft<'s>>,
    /// The 1-based source line; 0 for a structure that was not read.
    pub(crate) line: u32,
    /// For a text payload read joined from several lines: the pieces of
    /// the input it was joined from, and where the text they are pieces of
    /// starts in the input; what a reader types it from again while the
    /// text is the same, without a copy.
    pub(crate) joined: Option<(&'s [TextPiece], usize)>,
}

impl PartialEq for Draft<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag
            && self.xref == other.xref
            && self.payload == other.payload
            && self.substructures == other.substructures
    }
}

impl Eq for Draft<'_> {}

impl fmt::Debug for Draft<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Draft")
            .field("tag", &self.tag)
            .field("xref", &self.xref)
            .field("payload", &self.payload)
            .field("substructures", &self.substructures)
            .finish_non_exhaustive()
    }
}

impl<'s> Draft<'s> {
    /// A structure with a tag and nothing else.
    #[inline]
    pub(crate) fn new(tag: &'s str) -> Self {
        Self {
            tag: DraftTag::new(tag),
            ..Self::default()
        }
    }

    /// The text payload, if the payload is text.
    #[inline]
    pub(crate) fn text(&self) -> Option<&str> {
        match &self.payload {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    /// The pointer, if the payload is a pointer.
    #[inline]
    pub(crate) fn pointer(&self) -> Option<&str> {
        match &self.payload {
            Value::Pointer(p) => Some(p),
            _ => None,
        }
    }

    /// The first substructure with this tag.
    pub(crate) fn first(&self, tag: &str) -> Option<&Draft<'s>> {
        self.substructures.iter().find(|s| s.tag.as_str() == tag)
    }

    /// The structure with every text owned, to keep beyond what it was read
    /// from.
    pub(crate) fn into_owned(self) -> Draft<'static> {
        Draft {
            tag: self.tag.into_owned(),
            xref: self.xref.map(|x| Cow::Owned(x.into_owned())),
            payload: self.payload.into_owned(),
            substructures: self
                .substructures
                .into_iter()
                .map(Draft::into_owned)
                .collect(),
            line: self.line,
            joined: None,
        }
    }

    /// The structure of an owned [`Structure`], its texts moved.
    pub(crate) fn from_owned(s: Structure) -> Draft<'static> {
        Draft {
            tag: DraftTag::owned(s.tag.as_str().to_string()),
            xref: s.xref.map(|x| Cow::Owned(x.as_str().to_string())),
            payload: match s.payload {
                Payload::None => Value::None,
                Payload::Pointer(p) => Value::Pointer(Cow::Owned(p.as_str().to_string())),
                Payload::Text(t) => Value::Text(Cow::Owned(t.into_string())),
            },
            substructures: s.substructures.into_iter().map(Draft::from_owned).collect(),
            line: s.line,
            joined: None,
        }
    }

    /// The structure borrowing `s`'s texts.
    pub(crate) fn of_structure(s: &'s Structure) -> Self {
        Self {
            tag: DraftTag::new(s.tag.as_str()),
            xref: s.xref.as_deref().map(Cow::Borrowed),
            payload: Value::of(s.payload.borrowed()),
            substructures: s.substructures.iter().map(Draft::of_structure).collect(),
            line: s.line,
            joined: None,
        }
    }

    /// The structure at `node` of a record's arena as reading lexes it,
    /// borrowing its texts (`others`: the tags that are not standard, as
    /// the arena's builder interned them; `escaping`: the file's).
    pub(crate) fn of_arena(node: NodeRef<'s>, others: &'s [Box<str>], escaping: Escaping) -> Self {
        Self {
            tag: DraftTag::new(node.tag_text(others)),
            xref: node.xref_text().map(Cow::Borrowed),
            payload: if node.is_pointer() {
                Value::Pointer(node.payload_str(escaping))
            } else if node.has_no_payload() {
                Value::None
            } else {
                Value::Text(node.payload_str(escaping))
            },
            substructures: if node.has_children() {
                let mut children = Vec::with_capacity(node.children().count());
                children.extend(
                    node.children()
                        .map(|c| Draft::of_arena(c, others, escaping)),
                );
                children
            } else {
                Vec::new()
            },
            line: node.line(),
            joined: node.joined_pieces(),
        }
    }

    /// The record written into `flat` (its only one), its texts moved out
    /// of the arena, which is left empty.
    pub(crate) fn of_flat(flat: &mut Flat<'s>) -> Option<Self> {
        let mut open: Vec<Draft<'s>> = Vec::new();
        let close = |open: &mut Vec<Draft<'s>>| {
            if let Some(done) = open.pop() {
                if let Some(parent) = open.last_mut() {
                    parent.substructures.push(done);
                } else {
                    open.push(done);
                }
            }
        };
        flat.drain_into(|depth, tag, xref, payload, children| {
            while open.len() > (depth as usize).max(1) {
                close(&mut open);
            }
            open.push(Draft {
                tag: DraftTag(tag),
                xref,
                payload: match payload {
                    FlatPayload::None => Value::None,
                    FlatPayload::Pointer(p, _) => Value::Pointer(p),
                    FlatPayload::Text(t, _) => Value::Text(t),
                },
                substructures: Vec::with_capacity(children),
                line: 0,
                joined: None,
            });
        });
        while open.len() > 1 {
            close(&mut open);
        }
        open.pop()
    }

    /// The structure of a parsed tree, borrowing its texts.
    pub(crate) fn of_ref(s: StructureRef<'s>) -> Self {
        Self {
            tag: DraftTag::new(s.tag()),
            xref: s.xref().map(Cow::Borrowed),
            payload: Value::of(s.payload()),
            substructures: s.substructures().map(Draft::of_ref).collect(),
            line: s.line(),
            joined: None,
        }
    }

    /// An owned copy, as a [`Structure`].
    pub(crate) fn to_structure(&self) -> Structure {
        Structure {
            tag: Tag::new(self.tag.as_str()),
            xref: self.xref.as_deref().map(Xref::new),
            payload: self.payload.to_payload(),
            substructures: self.substructures.iter().map(Draft::to_structure).collect(),
            line: self.line,
        }
    }

    /// Appends the structure and its substructures as GEDCOM lines at
    /// `level`, `@` escaped for `version`: what [`Structure::write_gedcom`]
    /// writes of the same structure.
    pub(crate) fn write_gedcom(&self, out: &mut String, level: u8, version: GedcomVersion) {
        let escaping = if version.is_v7() {
            Escaping::V70
        } else {
            Escaping::V551
        };
        self.write(out, usize::from(level), escaping);
    }

    /// Appends the structure and its substructures as GEDCOM lines at
    /// `level`, `@` escaped as `escaping` says.
    pub(crate) fn write(&self, out: &mut String, level: usize, escaping: Escaping) {
        crate::tree::write_line(
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

impl<'n, 's: 'n> Node<'n> for &'n Draft<'s> {
    type Children = std::slice::Iter<'n, Draft<'s>>;
    #[inline]
    fn tag(self) -> &'n str {
        self.tag.as_str()
    }
    #[inline]
    fn standard_tag(self) -> Option<u16> {
        standard_index(self.tag.as_str())
    }
    #[inline]
    fn xref(self) -> Option<&'n str> {
        self.xref.as_deref()
    }
    #[inline]
    fn payload(self) -> PayloadRef<'n> {
        self.payload.borrowed()
    }
    #[inline]
    fn line(self) -> u32 {
        self.line
    }
    #[inline]
    fn children(self) -> Self::Children {
        self.substructures.iter()
    }
    fn to_owned_structure(self) -> Structure {
        self.to_structure()
    }
}
