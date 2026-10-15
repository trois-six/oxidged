//! Personal names, their pieces, translations and variations.

use crate::tree::{Flat, Structure};

use super::citation::Citation;
use super::driver::{
    NodeRef, ReadCx, StdTag, TagField, ToNodes, WriteCx, gedcom_struct, leaf_structure, tag_enum,
};
use super::enums::{NameType, PhoneticType, Phrased, RomanizedType};
use super::list::ThinVec;
use super::note::Note;
use super::table::{CopyCx, NamePieceRow, StoreCx, Tables};
use super::text::{Store, Text};

tag_enum! {
    /// Which part of a name a [`NamePiece`] is.
    pub enum NamePieceKind {
        /// A prefix (`NPFX`: `Dr.`).
        Prefix = "NPFX",
        /// A given name (`GIVN`).
        Given = "GIVN",
        /// A nickname (`NICK`).
        Nickname = "NICK",
        /// A surname prefix (`SPFX`: `van`).
        SurnamePrefix = "SPFX",
        /// A surname (`SURN`).
        Surname = "SURN",
        /// A suffix (`NSFX`: `Jr.`).
        Suffix = "NSFX",
    }
}

/// A piece of a name (`GIVN`, `SURN`, …) as written: a name keeps the
/// pieces it was given, in order, and gains none (a surname between
/// slashes in the name itself is not a `SURN`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamePiece {
    /// Which part of the name.
    pub kind: NamePieceKind,
    /// The piece. 5.5.1 writes several of a kind in one, comma-separated.
    pub value: Text,
}

impl ToNodes for NamePiece {
    const LEAF: bool = true;

    fn to_node(&self, _tag: &'static str, cx: &WriteCx<'_>) -> Structure {
        leaf_structure(self.kind.tag(), cx.text(&self.value))
    }

    fn to_flat<'s>(&'s self, _tag: StdTag, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        let tag = self.kind.std_tag();
        out.leaf_typed(tag.name, tag.index, cx.text_flat(&self.value), None);
    }
}

impl super::pointers::Pointers for NamePiece {}

super::table::listed!([] NamePiece, NamePieceRow, NamePiece);

impl super::table::Item for NamePiece {
    type One = NamePieceRow;

    fn elems(t: &Tables) -> &[NamePieceRow] {
        &t.name_pieces
    }

    fn elems_mut(t: &mut Tables) -> &mut Vec<NamePieceRow> {
        &mut t.name_pieces
    }

    fn fill() -> NamePieceRow {
        NamePieceRow::default()
    }

    fn read_elem(node: NodeRef<'_>, _at: u32, cx: &mut ReadCx<'_>) -> Option<NamePieceRow> {
        if node.has_children() || node.has_xref() {
            return None;
        }
        Some(NamePieceRow {
            kind: NamePieceKind::from_tag(node.standard_tag()?)?,
            value: cx.span(node)?,
        })
    }

    fn read_one(node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> Option<NamePieceRow> {
        Self::read_elem(node, 0, cx)
    }

    fn write_elem<'s>(
        e: &NamePieceRow,
        _at: u32,
        _tag: StdTag,
        cx: &WriteCx<'s>,
        out: &mut Flat<'s>,
    ) {
        let tag = e.kind.std_tag();
        super::driver::leaf_flat(out, tag, cx.span_flat(e.value), None);
    }

    fn write_one<'s>(o: &NamePieceRow, tag: StdTag, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        Self::write_elem(o, 0, tag, cx, out);
    }

    fn load_elem(e: &NamePieceRow, _at: u32, _t: &Tables) -> Self {
        Self {
            kind: e.kind,
            value: Text::from_span(e.value),
        }
    }

    fn load_one(o: NamePieceRow, t: &Tables) -> Self {
        Self::load_elem(&o, 0, t)
    }

    fn store_elem(&self, _at: u32, cx: &mut StoreCx<'_>) -> NamePieceRow {
        NamePieceRow {
            kind: self.kind,
            value: self.value.to_span(cx.store),
        }
    }

    fn store_one(&self, cx: &mut StoreCx<'_>) -> NamePieceRow {
        self.store_elem(0, cx)
    }

    fn each_elem(_e: &NamePieceRow, _at: u32, _t: &Tables, _f: &mut dyn FnMut(super::XrefId)) {}

    fn each_one(_o: NamePieceRow, _t: &Tables, _f: &mut dyn FnMut(super::XrefId)) {}

    fn copy_elem(e: &NamePieceRow, _at: u32, cx: &mut CopyCx<'_>, _to: u32) -> NamePieceRow {
        NamePieceRow {
            kind: e.kind,
            value: cx.span(e.value),
        }
    }

    fn copy_one(o: NamePieceRow, cx: &mut CopyCx<'_>) -> NamePieceRow {
        Self::copy_elem(&o, 0, cx, 0)
    }

    fn view_elem<'a>(e: &'a NamePieceRow, at: u32, data: &'a super::Dataset) -> NamePiece {
        Self::load_elem(e, at, &data.tables)
    }

    fn view_one(o: NamePieceRow, data: &super::Dataset) -> NamePiece {
        Self::load_elem(&o, 0, &data.tables)
    }
}

#[cfg(feature = "serde")]
impl super::serde::SerializeIn for NamePiece {
    fn serialize_in<S: ::serde::Serializer>(&self, store: &Store, s: S) -> Result<S::Ok, S::Error> {
        use super::serde::In;
        use ::serde::ser::SerializeMap as _;
        let mut map = s.serialize_map(Some(2))?;
        map.serialize_entry("kind", &In(store, &self.kind))?;
        map.serialize_entry("value", &In(store, &self.value))?;
        map.end()
    }
}

#[cfg(feature = "serde")]
impl super::serde::DeserializeIn for NamePiece {
    fn deserialize_in<'de, D: ::serde::Deserializer<'de>>(
        store: &mut Store,
        d: D,
    ) -> Result<Self, D::Error> {
        /// The visitor of the fields.
        struct V<'s>(&'s mut Store);
        impl<'de> ::serde::de::Visitor<'de> for V<'_> {
            type Value = NamePiece;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a name piece")
            }
            fn visit_map<A: ::serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<NamePiece, A::Error> {
                use super::serde::Seed;
                let store = self.0;
                let mut out = NamePiece::default();
                while let Some(key) = map.next_key::<super::serde::Str<'de>>()? {
                    match &*key.0 {
                        "kind" => out.kind = map.next_value_seed(Seed::new(&mut *store))?,
                        "value" => out.value = map.next_value_seed(Seed::new(&mut *store))?,
                        _ => super::serde::skip(&mut map)?,
                    }
                }
                Ok(out)
            }
        }
        d.deserialize_map(V(store))
    }
}

/// The pieces of `kind` among `pieces`.
fn pieces_of(pieces: &[NamePiece], kind: NamePieceKind) -> impl Iterator<Item = &Text> {
    pieces
        .iter()
        .filter(move |p| p.kind == kind)
        .map(|p| &p.value)
}

gedcom_struct! {
    /// A personal name (`NAME`): the name as written, the surname between
    /// slashes (`John /Smith/`), its pieces, and what kind of name it is.
    ///
    /// ```rust
    /// use oxidged::model::{Dataset, NameType};
    ///
    /// let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n2 TYPE BIRTH\n2 GIVN Ann\n2 GIVN Marie\n0 TRLR\n");
    /// let name = data.individuals().first().unwrap().name().unwrap();
    /// assert_eq!(name.surname_in_value().as_deref(), Some("Example"));
    /// assert_eq!(name.givens().count(), 2);
    /// assert_eq!(name.surnames().count(), 0); // none is invented
    /// assert_eq!(name.kind().unwrap().value(), NameType::Birth);
    /// ```
    pub struct Name => NameRow {
        @payload
        /// The name as written, the surname between slashes.
        value: Text;
        /// The pieces (`NPFX`, `GIVN`, `NICK`, `SPFX`, `SURN`, `NSFX`), in
        /// order.
        "NPFX" | "GIVN" | "NICK" | "SPFX" | "SURN" | "NSFX" => pieces: ThinVec<NamePiece>,
        @detail
        /// What fewer names have: a kind, translations, variations, notes
        /// and sources.
        NameDetail => NameDetailRow {
            /// What kind of name it is (`TYPE`), with a phrase in 7.x.
            "TYPE" => kind: Option<Phrased<NameType>>,
            /// The name in other languages or scripts (7.x `TRAN`).
            "TRAN" => translations: Vec<NameTranslation>,
            /// Phonetic variations (5.5.1 `FONE`).
            "FONE" => phonetic: Vec<PhoneticName>,
            /// Romanized variations (5.5.1 `ROMN`).
            "ROMN" => romanized: Vec<RomanizedName>,
            /// Notes (`NOTE`, 7.x `SNOTE`).
            "NOTE" | "SNOTE" => notes: Vec<Note>,
            /// Sources (`SOUR`).
            "SOUR" => citations: Vec<Citation>,
        }
    }
    spec {
        v551: ["PERSONAL_NAME_STRUCTURE.NAME"],
        v70: ["INDI-NAME"],
        v71: ["INDI-NAME"],
    }
}

impl Name {
    /// A name with this value (`John /Smith/`).
    #[must_use]
    pub fn new(value: impl Into<Text>) -> Self {
        Self {
            value: value.into(),
            ..Self::default()
        }
    }

    /// The pieces of a kind, in order.
    pub fn pieces_of(&self, kind: NamePieceKind) -> impl Iterator<Item = &Text> {
        pieces_of(&self.pieces, kind)
    }

    /// The given names (`GIVN`).
    pub fn givens(&self) -> impl Iterator<Item = &Text> {
        self.pieces_of(NamePieceKind::Given)
    }

    /// The surnames (`SURN`).
    pub fn surnames(&self) -> impl Iterator<Item = &Text> {
        self.pieces_of(NamePieceKind::Surname)
    }

    /// The first given name piece (`GIVN`).
    #[must_use]
    pub fn given(&self) -> Option<&Text> {
        self.givens().next()
    }

    /// The first surname piece (`SURN`).
    #[must_use]
    pub fn surname(&self) -> Option<&Text> {
        self.surnames().next()
    }

    /// The name as written, without the slashes around its surname and
    /// with its spaces collapsed: `Ann Example` for `Ann /Example/`.
    #[must_use]
    pub fn full<S: AsRef<Store> + ?Sized>(&self, store: &S) -> String {
        full(&self.value.to_str(store))
    }

    /// The surname written between slashes in the name itself, if any:
    /// `Smith` for `John /Smith/`.
    #[must_use]
    pub fn surname_in_value<S: AsRef<Store> + ?Sized>(&self, store: &S) -> Option<String> {
        surname_in_value(&self.value.to_str(store))
    }
}

gedcom_struct! {
    /// A name in another language or script (7.x `TRAN` of `NAME`): its
    /// language and its pieces.
    pub struct NameTranslation => NameTranslationRow {
        @payload
        /// The name as written.
        value: Text;
        /// The language (`LANG`).
        "LANG" => language: Option<Text>,
        /// The pieces, in order.
        "NPFX" | "GIVN" | "NICK" | "SPFX" | "SURN" | "NSFX" => pieces: ThinVec<NamePiece>,
    }
    spec {
        v551: [],
        v70: ["NAME-TRAN"],
        v71: ["NAME-TRAN"],
    }
}

gedcom_struct! {
    /// A phonetic variation of a name (5.5.1 `FONE` of `NAME`): its method
    /// (`TYPE`), its pieces, notes and sources.
    pub struct PhoneticName => PhoneticNameRow {
        @payload
        /// The variation.
        value: Text;
        /// The method (`TYPE`).
        "TYPE" => kind: Option<PhoneticType>,
        /// The pieces, in order.
        "NPFX" | "GIVN" | "NICK" | "SPFX" | "SURN" | "NSFX" => pieces: ThinVec<NamePiece>,
        /// Notes (`NOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
        /// Sources (`SOUR`).
        "SOUR" => citations: ThinVec<Citation>,
    }
    spec {
        v551: ["PERSONAL_NAME_STRUCTURE.NAME.FONE"],
        v70: [],
        v71: [],
    }
}

gedcom_struct! {
    /// A romanized variation of a name (5.5.1 `ROMN` of `NAME`): its method
    /// (`TYPE`), its pieces, notes and sources.
    pub struct RomanizedName => RomanizedNameRow {
        @payload
        /// The variation.
        value: Text;
        /// The method (`TYPE`).
        "TYPE" => kind: Option<RomanizedType>,
        /// The pieces, in order.
        "NPFX" | "GIVN" | "NICK" | "SPFX" | "SURN" | "NSFX" => pieces: ThinVec<NamePiece>,
        /// Notes (`NOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
        /// Sources (`SOUR`).
        "SOUR" => citations: ThinVec<Citation>,
    }
    spec {
        v551: ["PERSONAL_NAME_STRUCTURE.NAME.ROMN"],
        v70: [],
        v71: [],
    }
}

impl<'a> super::table::View<'a, Name> {
    /// The pieces of a kind, in order.
    pub fn pieces_of(self, kind: NamePieceKind) -> impl Iterator<Item = super::TextRef<'a>> + 'a {
        let store = self.store();
        self.pieces()
            .into_iter()
            .filter(move |p| p.kind == kind)
            .filter_map(move |p| p.value.as_span().map(|s| super::TextRef::new(store, s)))
    }

    /// The given names (`GIVN`).
    pub fn givens(self) -> impl Iterator<Item = super::TextRef<'a>> + 'a {
        self.pieces_of(NamePieceKind::Given)
    }

    /// The surnames (`SURN`).
    pub fn surnames(self) -> impl Iterator<Item = super::TextRef<'a>> + 'a {
        self.pieces_of(NamePieceKind::Surname)
    }

    /// The first given name piece (`GIVN`).
    #[must_use]
    pub fn given(self) -> Option<super::TextRef<'a>> {
        self.givens().next()
    }

    /// The first surname piece (`SURN`).
    #[must_use]
    pub fn surname(self) -> Option<super::TextRef<'a>> {
        self.surnames().next()
    }

    /// The name as written, without the slashes around its surname and
    /// with its spaces collapsed: `Ann Example` for `Ann /Example/`.
    #[must_use]
    pub fn full(self) -> String {
        full(&self.value().to_str())
    }

    /// The surname written between slashes in the name itself, if any:
    /// `Smith` for `John /Smith/`.
    #[must_use]
    pub fn surname_in_value(self) -> Option<String> {
        surname_in_value(&self.value().to_str())
    }
}

/// [`Name::full`] of a name's value.
fn full(value: &str) -> String {
    value
        .split(|c: char| c == '/' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// [`Name::surname_in_value`] of a name's value.
fn surname_in_value(value: &str) -> Option<String> {
    let (_, rest) = value.split_once('/')?;
    let surname = rest.split_once('/').map_or(rest, |(s, _)| s);
    Some(surname.trim().to_string())
}
