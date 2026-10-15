//! One view over the two forms of a tree — the arena of a parsed
//! [`Tree`](super::Tree) and owned [`Structure`]s — so that the writer and
//! the validator walk both with one code path.

use super::{PayloadRef, Structure, StructureRef, Substructures};

/// The standard tags and the index of one: what lets the validator and the
/// repair find a tag's type without its text.
pub(crate) use super::tag::{STANDARD_TAGS, standard_index};

/// Gives `f` the identifier and the pointers of `n` and of every structure
/// under it: every identifier it uses.
pub(crate) fn identifiers<'n, N: Node<'n>>(n: N, f: &mut dyn FnMut(&'n str)) {
    if let Some(x) = n.xref() {
        f(x);
    }
    if let PayloadRef::Pointer(p) = n.payload() {
        f(p);
    }
    for c in n.children() {
        identifiers(c, f);
    }
}

/// A structure, borrowed for `'n`.
pub(crate) trait Node<'n>: Copy {
    /// The substructures.
    type Children: Iterator<Item = Self>;
    /// The tag.
    fn tag(self) -> &'n str;
    /// The index of the tag in the table of standard tags, when it is one:
    /// what lets the validator and the repair skip a lookup by text.
    fn standard_tag(self) -> Option<u16> {
        standard_index(self.tag())
    }
    /// The identifier.
    fn xref(self) -> Option<&'n str>;
    /// The payload.
    fn payload(self) -> PayloadRef<'n>;
    /// The 1-based source line; 0 when not read from a file.
    fn line(self) -> u32;
    /// The substructures.
    fn children(self) -> Self::Children;
    /// An owned copy.
    fn to_owned_structure(self) -> Structure;
}

impl<'t> Node<'t> for StructureRef<'t> {
    type Children = Substructures<'t>;
    #[inline]
    fn tag(self) -> &'t str {
        StructureRef::tag(self)
    }
    #[inline]
    fn standard_tag(self) -> Option<u16> {
        u16::try_from(self.raw_tag())
            .ok()
            .filter(|&t| usize::from(t) < STANDARD_TAGS.len())
    }
    #[inline]
    fn xref(self) -> Option<&'t str> {
        StructureRef::xref(self)
    }
    #[inline]
    fn payload(self) -> PayloadRef<'t> {
        StructureRef::payload(self)
    }
    #[inline]
    fn line(self) -> u32 {
        StructureRef::line(self)
    }
    #[inline]
    fn children(self) -> Self::Children {
        self.substructures()
    }
    fn to_owned_structure(self) -> Structure {
        self.to_structure()
    }
}

impl<'n> Node<'n> for &'n Structure {
    type Children = std::slice::Iter<'n, Structure>;
    #[inline]
    fn tag(self) -> &'n str {
        self.tag.as_str()
    }
    #[inline]
    fn standard_tag(self) -> Option<u16> {
        self.tag.standard_index()
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
        self.clone()
    }
}
