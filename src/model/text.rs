//! The shared text store of a dataset and the compact handles the model
//! keeps into it: [`Text`] and [`TextRef`] for payloads, [`XrefId`] for
//! identifiers and pointers, [`TagId`] for the tags of [`Node`](super::Node)s.

use std::borrow::Cow;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU32;

use crate::tree::{STANDARD_TAGS, TagHasher, TextPiece};
use crate::writer::{LINE_FEED, special_bytes};

/// Text of the model: a payload, a value, a phrase.
///
/// Read from a file, a text points into the text that the dataset's
/// [`Store`] keeps, and costs 16 bytes and no allocation: a span (32-bit
/// offset and length) of the store's text, or, when reading rewrote it
/// (continuation lines joined, `@@` unescaped), a run of the store's
/// pieces — spans of its text, each after a newline or not — so that
/// nothing is copied. Made by a program ([`Text::new`], `From<&str>`,
/// `From<String>`), it owns its characters until it is stored in a
/// dataset, which copies them into its store.
///
/// A text read means nothing without its store: [`Text::to_str`] takes the
/// [`Store`] (or anything that holds one, such as a
/// [`Dataset`](super::Dataset)), and borrows its characters unless they
/// are joined from several pieces; [`Text::chunks`] gives them without a
/// copy. The texts of a dataset's views are [`TextRef`]s, which carry
/// their store. Equality compares the representations: two texts of
/// different stores with the same characters differ. An empty text stands
/// for an absent payload.
///
/// ```rust
/// use oxidged::model::{Dataset, Text};
///
/// let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @N1@ SNOTE Shared\n1 CONT text\n0 TRLR\n");
/// let text = data.notes().first().unwrap().text().to_text();
/// assert_eq!(text.to_str(&data), "Shared\ntext");
/// assert_eq!(text.chunks(&data).collect::<Vec<_>>(), ["Shared", "\n", "text"]);
/// assert_eq!(Text::from("Shared").to_str(&data), "Shared");
/// assert!(Text::default().is_empty());
/// ```
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct Text(Repr);

#[derive(Clone, PartialEq, Eq, Hash)]
enum Repr {
    /// A span of the store's text, with the [`special_bytes`] of its
    /// characters.
    Input { start: u32, len: u32, special: u8 },
    /// `count` pieces of the store, from `start`, with the
    /// [`special_bytes`] of the characters they join.
    Pieces { start: u32, count: u32, special: u8 },
    /// A text the store keeps whole, past what 32-bit offsets reach.
    Big { index: u32, special: u8 },
    /// Characters of its own. Boxed twice so that a text stays two words
    /// with a spare niche: `Option<Text>` is 16 bytes too.
    Owned(Box<Owned>),
}

/// [`Text::valid_as`]: a date of the commonest shapes (`YYYY`, `MON YYYY`,
/// `D MON YYYY` and keywords, in upper case), valid in every version
/// ([`is_simple_date`](crate::spec::is_simple_date)), kept beside the
/// special bytes.
pub(crate) const SIMPLE_DATE: u8 = 64;
/// [`Text::valid_as`]: an exact date of the commonest shape (`D MON YYYY`).
pub(crate) const EXACT_DATE: u8 = 128;

/// The characters of an owned [`Text`], with their [`special_bytes`].
#[derive(Clone, PartialEq, Eq, Hash)]
struct Owned(Box<str>, u8);

impl Default for Repr {
    fn default() -> Self {
        Repr::Input {
            start: 0,
            len: 0,
            special: 0,
        }
    }
}

impl Text {
    /// A text that owns its characters.
    #[must_use]
    pub fn new(text: impl Into<Box<str>>) -> Self {
        let text = text.into();
        if text.is_empty() {
            Self::default()
        } else {
            let special = special_bytes(&text);
            Self(Repr::Owned(Box::new(Owned(text, special))))
        }
    }

    /// The characters, read from `store` when this text points into it:
    /// borrowed, unless they are joined from several pieces.
    #[must_use]
    pub fn to_str<'a, S: AsRef<Store> + ?Sized>(&'a self, store: &'a S) -> Cow<'a, str> {
        let store = store.as_ref();
        match (&self.0, self.raw()) {
            (Repr::Owned(owned), _) => Cow::Borrowed(&owned.0),
            (_, Some((kind, at, len))) => store.raw_str(kind, at, len),
            (_, None) => Cow::Borrowed(""),
        }
    }

    /// The characters in order, without a copy: the spans of the store's
    /// text and the newlines between them (a text of one span is one
    /// chunk).
    pub fn chunks<'a, S: AsRef<Store> + ?Sized>(
        &'a self,
        store: &'a S,
    ) -> impl Iterator<Item = &'a str> + 'a {
        let store = store.as_ref();
        let owned = match &self.0 {
            Repr::Owned(owned) => Some(&*owned.0),
            _ => None,
        };
        let raw = self.raw();
        owned.filter(|s| !s.is_empty()).into_iter().chain(
            raw.into_iter()
                .flat_map(move |(kind, at, len)| store.raw_chunks(kind, at, len)),
        )
    }

    /// Whether the characters are `other`, without a copy.
    #[must_use]
    pub fn eq_str<S: AsRef<Store> + ?Sized>(&self, store: &S, other: &str) -> bool {
        eq_chunks(self.chunks(store), other)
    }

    /// The characters when this text owns them; `None` for a text read.
    #[must_use]
    pub fn as_owned(&self) -> Option<&str> {
        match &self.0 {
            Repr::Owned(owned) => Some(&owned.0),
            Repr::Input { len: 0, .. } => Some(""),
            _ => None,
        }
    }

    /// The [`special_bytes`] of the characters: found once, when the text
    /// is made.
    #[inline]
    pub(crate) fn special(&self) -> u8 {
        let bits = match &self.0 {
            Repr::Input { special, .. }
            | Repr::Pieces { special, .. }
            | Repr::Big { special, .. } => *special,
            Repr::Owned(owned) => owned.1,
        };
        bits & !(SIMPLE_DATE | EXACT_DATE)
    }

    /// What the text is known to be valid as, found as it was read
    /// ([`SIMPLE_DATE`], [`EXACT_DATE`]).
    #[inline]
    pub(crate) fn valid_as(&self) -> u8 {
        match &self.0 {
            Repr::Input { special, .. } => *special & (SIMPLE_DATE | EXACT_DATE),
            _ => 0,
        }
    }

    /// Whether the text is empty (an absent payload).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match &self.0 {
            Repr::Input { len, .. } => *len == 0,
            // Pieces and big texts are only made of a non-empty text.
            Repr::Pieces { .. } | Repr::Big { .. } => false,
            Repr::Owned(owned) => owned.0.is_empty(),
        }
    }

    /// The length in bytes.
    #[must_use]
    pub fn len<S: AsRef<Store> + ?Sized>(&self, store: &S) -> usize {
        self.chunks(store).map(str::len).sum()
    }

    /// Turns a text read into an owned copy of its characters (copy on
    /// write), so that it no longer depends on `store`.
    pub fn make_owned<S: AsRef<Store> + ?Sized>(&mut self, store: &S) {
        if !matches!(self.0, Repr::Owned(_)) && !self.is_empty() {
            *self = Self::new(self.to_str(store));
        }
    }

    /// The span of a text that points into a store; `None` for an owned
    /// text.
    pub(crate) fn as_span(&self) -> Option<Span> {
        match &self.0 {
            Repr::Input {
                start,
                len,
                special,
            } if *len <= Span::MAX_LEN => Some(Span::new(Span::INPUT, *start, *len, *special)),
            Repr::Pieces {
                start,
                count,
                special,
            } if *count <= Span::MAX_LEN => Some(Span::new(Span::PIECES, *start, *count, *special)),
            Repr::Big { index, special } => Some(Span::new(Span::BIG, *index, 0, *special)),
            _ => None,
        }
    }

    /// The text of a span of a store: no copy.
    #[inline]
    pub(crate) fn from_span(span: Span) -> Self {
        let special = span.special_all();
        Self(match span.kind() {
            Span::PIECES => Repr::Pieces {
                start: span.at,
                count: span.len_field(),
                special,
            },
            Span::BIG => Repr::Big {
                index: span.at,
                special,
            },
            // A pointer is no text: an empty one stands for it.
            Span::POINTER => Repr::default(),
            _ => Repr::Input {
                start: span.at,
                len: span.len_field(),
                special,
            },
        })
    }

    /// What a text that points into a store points to: the kind of span,
    /// where and how long; `None` for an owned text.
    fn raw(&self) -> Option<(u32, u32, u32)> {
        match &self.0 {
            Repr::Input { start, len, .. } => Some((Span::INPUT, *start, *len)),
            Repr::Pieces { start, count, .. } => Some((Span::PIECES, *start, *count)),
            Repr::Big { index, .. } => Some((Span::BIG, *index, 0)),
            Repr::Owned(_) => None,
        }
    }

    /// The text as a span of `store`: its own when it points into the
    /// store; an owned text's characters are copied into the store.
    pub(crate) fn to_span(&self, store: &mut Store) -> Span {
        match &self.0 {
            Repr::Owned(owned) => store.push_text(&owned.0, owned.1),
            Repr::Input {
                start,
                len,
                special,
            } if *len > Span::MAX_LEN => store.long_span(*start, *len, *special),
            Repr::Input {
                start,
                len,
                special,
            } => Span::new(Span::INPUT, *start, *len, *special),
            Repr::Pieces { count, .. } if *count > Span::MAX_LEN => {
                let text = self.to_str(store).into_owned();
                let special = self.special();
                store.push_big(text.into(), special)
            }
            Repr::Pieces {
                start,
                count,
                special,
            } => Span::new(Span::PIECES, *start, *count, *special),
            Repr::Big { index, special } => Span::new(Span::BIG, *index, 0, *special),
        }
    }
}

/// Whether `chunks` joined are `other`, without a copy.
fn eq_chunks<'a>(chunks: impl Iterator<Item = &'a str>, other: &str) -> bool {
    let mut rest = other;
    for chunk in chunks {
        match rest.strip_prefix(chunk) {
            Some(r) => rest = r,
            None => return false,
        }
    }
    rest.is_empty()
}

fn span(buffer: &str, start: u32, len: u32) -> &str {
    let start = start as usize;
    buffer
        .get(start..start.saturating_add(len as usize))
        .unwrap_or_default()
}

/// The characters of `pieces` of `input`, joined.
fn join(input: &str, pieces: &[TextPiece]) -> String {
    let mut out = String::with_capacity(pieces.iter().map(|p| p.len() as usize + 1).sum());
    for p in pieces {
        if p.newline() {
            out.push('\n');
        }
        out.push_str(span(input, p.start(), p.len()));
    }
    out
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::Input { start, len, .. } => write!(f, "Text(input {start}+{len})"),
            Repr::Pieces { start, count, .. } => write!(f, "Text(pieces {start}+{count})"),
            Repr::Big { index, .. } => write!(f, "Text(big {index})"),
            Repr::Owned(owned) => write!(f, "Text({:?})", owned.0),
        }
    }
}

impl From<&str> for Text {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for Text {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<Box<str>> for Text {
    fn from(text: Box<str>) -> Self {
        Self::new(text)
    }
}

impl From<Cow<'_, str>> for Text {
    fn from(text: Cow<'_, str>) -> Self {
        Self::new(text)
    }
}

impl From<TextRef<'_>> for Text {
    fn from(text: TextRef<'_>) -> Self {
        text.to_text()
    }
}

/// A text of a dataset's tables: 8 bytes, which a [`Store`] resolves.
///
/// The first word is an offset into the store's text, the index of the
/// first of its pieces, the index of a big text, or a pointer's id (for
/// a payload that is a pointer or a text). The second holds the kind (3
/// bits, never zero, so that an `Option<Span>` is 8 bytes too), the
/// [`special_bytes`] and date bits (8 bits) and the length or the number
/// of pieces (21 bits): a longer span is one piece.
#[doc(hidden)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    at: u32,
    meta: NonZeroU32,
}

impl Default for Span {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Span {
    pub(crate) const INPUT: u32 = 1;
    pub(crate) const PIECES: u32 = 2;
    pub(crate) const BIG: u32 = 3;
    pub(crate) const POINTER: u32 = 4;
    const KIND_SHIFT: u32 = 29;
    const SPECIAL_SHIFT: u32 = 21;
    /// The longest span, and the most pieces, a span holds.
    pub(crate) const MAX_LEN: u32 = (1 << 21) - 1;

    /// An empty text.
    pub(crate) const EMPTY: Span = Span {
        at: 0,
        meta: match NonZeroU32::new(Self::INPUT << Self::KIND_SHIFT) {
            Some(m) => m,
            None => NonZeroU32::MIN,
        },
    };

    #[inline]
    fn new(kind: u32, at: u32, len: u32, special: u8) -> Self {
        let meta = (kind << Self::KIND_SHIFT)
            | (u32::from(special) << Self::SPECIAL_SHIFT)
            | (len & Self::MAX_LEN);
        Self {
            at,
            meta: NonZeroU32::new(meta).unwrap_or(NonZeroU32::MIN),
        }
    }

    /// A span of `input`, which the caller knows to be at most
    /// [`Span::MAX_LEN`] long and within 32 bits, with its special bytes.
    #[cfg(test)]
    pub(crate) fn of_input(input: &str, start: u32, len: u32) -> Self {
        if len == 0 {
            return Self::EMPTY;
        }
        Self::new(
            Self::INPUT,
            start,
            len,
            special_bytes(span(input, start, len)),
        )
    }

    /// A span of `count` pieces from `start`, which the caller knows to be
    /// at most [`Span::MAX_LEN`].
    #[inline]
    pub(crate) fn of_pieces(start: u32, count: u32, special: u8) -> Self {
        Self::new(Self::PIECES, start, count, special)
    }

    /// A big text, kept whole by the store at `index`.
    #[inline]
    pub(crate) fn big(index: u32, special: u8) -> Self {
        Self::new(Self::BIG, index, 0, special)
    }

    /// A pointer held where a text may be (a citation's source, a note's
    /// content).
    #[inline]
    pub(crate) fn pointer(id: XrefId) -> Self {
        Self::new(Self::POINTER, id.0.get(), 0, 0)
    }

    /// The pointer, when the span holds one.
    #[inline]
    pub(crate) fn as_pointer(self) -> Option<XrefId> {
        (self.kind() == Self::POINTER)
            .then(|| NonZeroU32::new(self.at).map(XrefId))
            .flatten()
    }

    #[inline]
    pub(crate) fn kind(self) -> u32 {
        self.meta.get() >> Self::KIND_SHIFT
    }

    #[inline]
    fn len_field(self) -> u32 {
        self.meta.get() & Self::MAX_LEN
    }

    /// The special bytes and the date bits.
    #[inline]
    fn special_all(self) -> u8 {
        #[allow(clippy::cast_possible_truncation)] // Eight bits, by construction.
        let bits = (self.meta.get() >> Self::SPECIAL_SHIFT) as u8;
        bits
    }

    /// The [`special_bytes`] of the characters.
    #[inline]
    pub(crate) fn special(self) -> u8 {
        self.special_all() & !(SIMPLE_DATE | EXACT_DATE)
    }

    /// The span with `bits` added to its date bits.
    pub(crate) fn with_valid(self, bits: u8) -> Self {
        Self {
            meta: NonZeroU32::new(self.meta.get() | (u32::from(bits) << Self::SPECIAL_SHIFT))
                .unwrap_or(self.meta),
            ..self
        }
    }

    /// Whether the text is empty.
    #[inline]
    pub(crate) fn is_empty(self) -> bool {
        self.kind() == Self::INPUT && self.len_field() == 0
    }

    /// The span moved by `text` bytes of text and `pieces` pieces, and its
    /// big text found at `big` on.
    pub(crate) fn shifted(self, text: u32, pieces: u32, big: u32) -> Self {
        match self.kind() {
            Self::INPUT if !self.is_empty() => Self {
                at: self.at.saturating_add(text),
                ..self
            },
            Self::PIECES => Self {
                at: self.at.saturating_add(pieces),
                ..self
            },
            Self::BIG => Self {
                at: self.at.saturating_add(big),
                ..self
            },
            _ => self,
        }
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind() {
            Self::PIECES => write!(f, "Span(pieces {}+{})", self.at, self.len_field()),
            Self::BIG => write!(f, "Span(big {})", self.at),
            Self::POINTER => write!(f, "Span(pointer {})", self.at),
            _ => write!(f, "Span({}+{})", self.at, self.len_field()),
        }
    }
}

/// A text of a dataset, with the store it reads from: what the views of
/// a [`Dataset`](super::Dataset) give for a text.
///
/// It reads its characters without being given the store
/// ([`TextRef::to_str`], [`TextRef::chunks`]), compares with a `&str`
/// without a copy, and prints them (`Display`).
///
/// ```rust
/// use oxidged::Dataset;
///
/// let data = Dataset::parse("0 HEAD\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 NOTE a\n2 CONT b\n0 TRLR\n");
/// let ann = data.individuals().first().unwrap();
/// let name = ann.names().first().unwrap().value();
/// assert_eq!(name, "Ann /Example/");
/// assert_eq!(name.to_string(), "Ann /Example/");
/// let note = ann.notes().first().unwrap().text().unwrap();
/// assert_eq!(note.to_str(), "a\nb");
/// ```
#[derive(Clone, Copy)]
pub struct TextRef<'a> {
    store: &'a Store,
    span: Span,
}

impl<'a> TextRef<'a> {
    pub(crate) fn new(store: &'a Store, span: Span) -> Self {
        Self { store, span }
    }

    /// The characters: borrowed, unless they are joined from several
    /// pieces.
    #[must_use]
    pub fn to_str(self) -> Cow<'a, str> {
        self.store.span_str(self.span)
    }

    /// The characters in order, without a copy (see [`Text::chunks`]).
    pub fn chunks(self) -> impl Iterator<Item = &'a str> + 'a {
        self.store.span_chunks(self.span)
    }

    /// Whether the text is empty (an absent payload).
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.span.is_empty()
    }

    /// The length in bytes.
    #[must_use]
    pub fn len(self) -> usize {
        self.chunks().map(str::len).sum()
    }

    /// The text, as a [`Text`] of the same store (no copy).
    #[must_use]
    pub fn to_text(self) -> Text {
        Text::from_span(self.span)
    }

    /// The store the text reads from.
    #[must_use]
    pub fn store(self) -> &'a Store {
        self.store
    }
}

impl fmt::Display for TextRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

impl fmt::Debug for TextRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.to_str(), f)
    }
}

impl PartialEq<str> for TextRef<'_> {
    fn eq(&self, other: &str) -> bool {
        eq_chunks(self.chunks(), other)
    }
}

impl PartialEq<&str> for TextRef<'_> {
    fn eq(&self, other: &&str) -> bool {
        eq_chunks(self.chunks(), other)
    }
}

impl PartialEq<String> for TextRef<'_> {
    fn eq(&self, other: &String) -> bool {
        eq_chunks(self.chunks(), other)
    }
}

impl PartialEq<TextRef<'_>> for str {
    fn eq(&self, other: &TextRef<'_>) -> bool {
        other == self
    }
}

impl PartialEq<TextRef<'_>> for &str {
    fn eq(&self, other: &TextRef<'_>) -> bool {
        other == *self
    }
}

impl PartialEq for TextRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.to_str() == other.to_str()
    }
}

impl Eq for TextRef<'_> {}

impl AsRef<Store> for TextRef<'_> {
    fn as_ref(&self) -> &Store {
        self.store
    }
}

/// A cross-reference identifier of a dataset (`@I1@`), interned: the
/// identifier of a record and every pointer to it share one 32-bit id.
///
/// [`Store::xref`] gives its text, delimiters included;
/// [`Store::find_xref`] finds the id of a text and [`Store::intern_xref`]
/// makes one. Pointers to identifiers no record holds have ids too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct XrefId(NonZeroU32);

impl XrefId {
    /// An id no identifier has: what a pointer of a table reads as when
    /// it holds none (never, but for a row no reader made).
    pub(crate) const DANGLING: Self = Self(NonZeroU32::MAX);

    /// The position in the table of identifiers.
    pub(crate) fn index(self) -> usize {
        self.0.get() as usize - 1
    }

    fn from_index(index: usize) -> Option<Self> {
        u32::try_from(index + 1)
            .ok()
            .and_then(NonZeroU32::new)
            .map(Self)
    }
}

/// The tag of a [`Node`](super::Node): a standard tag's index in the
/// crate's table of every 5.5.1, 7.0 and 7.1 tag, or an index past it into
/// the tags of the dataset ([`Store::tag`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TagId(u32);

impl TagId {
    /// The id of a standard tag; `None` for any other tag, which only a
    /// [`Store`] can intern ([`Store::intern_tag`]).
    #[must_use]
    pub fn standard(tag: &str) -> Option<Self> {
        crate::tree::standard_index(tag).map(|i| Self(u32::from(i)))
    }

    /// The tag of the node a typed structure keeps in its `extra` for the
    /// payload or identifier its type has no place for (text where its
    /// pointer belongs, a payload on a record): it is written back as the
    /// structure's own. It names no tag ([`Store::tag`] gives `""`).
    pub const ASIDE: Self = Self(u32::MAX);

    /// The id of a tag of the lexer's arena.
    pub(crate) const fn raw(id: u32) -> Self {
        Self(id)
    }

    pub(crate) const fn get(self) -> u32 {
        self.0
    }

    /// The index among the tags of a store, for a tag that is neither
    /// standard nor [`TagId::ASIDE`].
    pub(crate) fn local_index(self) -> Option<usize> {
        (self != Self::ASIDE)
            .then(|| (self.0 as usize).checked_sub(STANDARD_TAGS.len()))
            .flatten()
    }
}

/// The identifiers of a dataset: their texts, by id, and an
/// open-addressing index from text to id.
///
/// The texts are copied together into one buffer, ended by a 32-bit offset
/// each: a writer resolving every pointer reads them there, not across the
/// whole input. The index has 8 bytes a slot (the id and 32 bits of its
/// text's hash) and is kept at most three quarters full; a table of a few
/// identifiers has none.
#[derive(Clone, Default)]
pub(crate) struct XrefTable {
    /// The texts, one after the other.
    names: String,
    /// For each id, the end of its text in `names`.
    ends: Vec<u32>,
    /// For each id, the grammars its text is a valid identifier of
    /// ([`VALID_V551`], [`VALID_V7`]), found once, for the writers that
    /// look again at each identifier they write.
    valid: Vec<u8>,
    /// `0`: empty; otherwise the hash (high half) and the id (low half).
    slots: Vec<u64>,
    /// The identifiers renamed ([`XrefTable::rename`]), when any is.
    renamed: Option<Box<Renamed>>,
    /// Up to how many identifiers the table has no index, and is searched
    /// in order: a few for a record of a stream ([`Store::of_record`]),
    /// none for a file.
    unindexed: u8,
}

/// The texts [`XrefTable::rename`] gave identifiers.
#[derive(Clone, Default)]
struct Renamed {
    /// The texts, one after the other.
    names: String,
    /// The text of each renamed id, by index, as its span of `names`: the
    /// others are where [`XrefTable::ends`] says.
    spans: std::collections::HashMap<u32, (u32, u32)>,
}

/// [`XrefTable::valid`]: a valid 5.5.1 identifier.
const VALID_V551: u8 = 1;
/// [`XrefTable::valid`]: a valid 7.x identifier.
const VALID_V7: u8 = 2;

/// The grammars `text` is a valid identifier of, as bits.
fn validity(text: &str) -> u8 {
    let mut bits = 0;
    if crate::version::V551.is_valid_xref(text) {
        bits |= VALID_V551;
    }
    if crate::version::V70.is_valid_xref(text) {
        bits |= VALID_V7;
    }
    bits
}

fn hash(text: &str) -> u32 {
    let mut h = TagHasher::default();
    text.hash(&mut h);
    // The finished hash's low half, which the table's index and tag use.
    #[allow(clippy::cast_possible_truncation)] // Truncation is the intent.
    let h = h.finish() as u32;
    h
}

/// Up to this many identifiers, the table of a record of a stream has no
/// index ([`XrefTable::unindexed`]): a record has a few.
const UNINDEXED: u8 = 4;

impl XrefTable {
    /// The text of the identifier at `index`.
    #[inline]
    fn name(&self, index: usize) -> Option<&str> {
        if let Some(renamed) = &self.renamed {
            return self.renamed_name(renamed, index);
        }
        self.read_name(index)
    }

    /// The text `index` was read with (or interned with).
    #[inline]
    fn read_name(&self, index: usize) -> Option<&str> {
        let end = *self.ends.get(index)? as usize;
        let start = index
            .checked_sub(1)
            .and_then(|i| self.ends.get(i))
            .map_or(0, |&e| e as usize);
        self.names.get(start..end)
    }

    /// [`XrefTable::name`] once identifiers were renamed.
    #[cold]
    #[inline(never)]
    fn renamed_name<'a>(&'a self, renamed: &'a Renamed, index: usize) -> Option<&'a str> {
        match u32::try_from(index)
            .ok()
            .and_then(|i| renamed.spans.get(&i))
        {
            Some(&(start, end)) => renamed.names.get(start as usize..end as usize),
            None => self.read_name(index),
        }
    }

    /// The slot of `text`: where it is, or the empty slot it would take.
    fn slot(&self, text: &str, hash: u32) -> (usize, Option<XrefId>) {
        let mask = self.slots.len().wrapping_sub(1);
        let mut i = hash as usize & mask;
        loop {
            let Some(&s) = self.slots.get(i) else {
                return (i, None);
            };
            if s == 0 {
                return (i, None);
            }
            #[allow(clippy::cast_possible_truncation)] // The id is the low half.
            let id = s as u32;
            if (s >> 32) as u32 == hash && self.name(id as usize - 1) == Some(text) {
                return (i, XrefId::from_index(id as usize - 1));
            }
            i = (i + 1) & mask;
        }
    }

    fn find(&self, text: &str) -> Option<XrefId> {
        if self.slots.is_empty() {
            return self.find_in_order(text);
        }
        self.slot(text, hash(text)).1
    }

    /// The id of `text`, searched in order: a table without an index.
    fn find_in_order(&self, text: &str) -> Option<XrefId> {
        (0..self.ends.len())
            .find(|&i| self.read_name(i) == Some(text))
            .and_then(XrefId::from_index)
    }

    /// The id of `text`; a new one when it has none. `None` when four
    /// billion identifiers, or 4 GiB of them, are taken.
    pub(crate) fn intern(&mut self, text: &str) -> Option<XrefId> {
        if self.slots.is_empty() {
            if self.ends.len() < usize::from(self.unindexed) {
                return self.find_in_order(text).or_else(|| self.push(text));
            }
            self.index();
        } else if (self.ends.len() + 1) * 4 > self.slots.len() * 3 {
            self.grow();
        }
        let hash = hash(text);
        let (at, found) = self.slot(text, hash);
        if found.is_some() {
            return found;
        }
        let id = self.push(text)?;
        if let Some(slot) = self.slots.get_mut(at) {
            *slot = (u64::from(hash) << 32) | u64::from(id.0.get());
        }
        Some(id)
    }

    /// Gives `text` a new id, not indexed.
    fn push(&mut self, text: &str) -> Option<XrefId> {
        let id = XrefId::from_index(self.ends.len())?;
        let end = u32::try_from(self.names.len() + text.len()).ok()?;
        self.names.push_str(text);
        self.ends.push(end);
        self.valid.push(validity(text));
        Some(id)
    }

    /// Indexes the ids of a table that has no index yet.
    fn index(&mut self) {
        let mut len = 64;
        while (self.ends.len() + 1) * 4 > len * 3 {
            len *= 2;
        }
        self.slots = vec![0; len];
        let mask = len - 1;
        for index in 0..self.ends.len() {
            let (Some(name), Some(id)) = (self.read_name(index), XrefId::from_index(index)) else {
                continue;
            };
            let hash = hash(name);
            let mut i = hash as usize & mask;
            while self.slots.get(i).is_some_and(|&t| t != 0) {
                i = (i + 1) & mask;
            }
            if let Some(slot) = self.slots.get_mut(i) {
                *slot = (u64::from(hash) << 32) | u64::from(id.0.get());
            }
        }
    }

    /// Gives id `id` the text `text`, which must differ from its own: the
    /// index then finds `id` by `text`, no longer by its old text. When
    /// another id has `text` (the caller makes sure nothing uses it), that
    /// id is found no more: it keeps its text, but `text` is `id`'s.
    /// `false` when the table cannot take more text (4 GiB).
    pub(crate) fn rename(&mut self, id: XrefId, text: &str) -> bool {
        let Some(old) = self.name(id.index()).map(str::to_owned) else {
            return false;
        };
        if self.slots.is_empty() {
            self.index();
        }
        let renamed = self.renamed.get_or_insert_with(Box::default);
        let (Ok(start), Ok(end)) = (
            u32::try_from(renamed.names.len()),
            u32::try_from(renamed.names.len() + text.len()),
        ) else {
            return false;
        };
        let old_hash = hash(&old);
        if let (at, Some(found)) = self.slot(&old, old_hash)
            && found == id
        {
            self.remove_slot(at);
        }
        if let Some(renamed) = self.renamed.as_deref_mut() {
            renamed.names.push_str(text);
            if let Ok(index) = u32::try_from(id.index()) {
                renamed.spans.insert(index, (start, end));
            }
        }
        if let Some(v) = self.valid.get_mut(id.index()) {
            *v = validity(text);
        }
        if (self.ends.len() + 1) * 4 > self.slots.len() * 3 {
            self.grow();
        }
        let new_hash = hash(text);
        let (at, _) = self.slot(text, new_hash);
        if let Some(slot) = self.slots.get_mut(at) {
            *slot = (u64::from(new_hash) << 32) | u64::from(id.0.get());
        }
        true
    }

    /// Empties slot `at`, moving back the entries after it that it kept
    /// from their place (linear probing's deletion).
    fn remove_slot(&mut self, mut at: usize) {
        let mask = self.slots.len().wrapping_sub(1);
        if let Some(s) = self.slots.get_mut(at) {
            *s = 0;
        }
        let mut next = at;
        loop {
            next = (next + 1) & mask;
            let Some(&s) = self.slots.get(next) else {
                return;
            };
            if s == 0 {
                return;
            }
            let home = (s >> 32) as usize & mask;
            // The entry stays when its home is cyclically in (at, next].
            let stays = if at <= next {
                at < home && home <= next
            } else {
                at < home || home <= next
            };
            if !stays {
                if let Some(free) = self.slots.get_mut(at) {
                    *free = s;
                }
                if let Some(moved) = self.slots.get_mut(next) {
                    *moved = 0;
                }
                at = next;
            }
        }
    }

    /// Doubles the index, at least 64 slots.
    fn grow(&mut self) {
        let len = (self.slots.len() * 2).max(64);
        let old = std::mem::replace(&mut self.slots, vec![0; len]);
        let mask = len - 1;
        for s in old.into_iter().filter(|&s| s != 0) {
            let mut i = (s >> 32) as usize & mask;
            while self.slots.get(i).is_some_and(|&t| t != 0) {
                i = (i + 1) & mask;
            }
            if let Some(slot) = self.slots.get_mut(i) {
                *slot = s;
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.ends.len()
    }

    pub(crate) fn shrink(&mut self) {
        self.names.shrink_to_fit();
        self.ends.shrink_to_fit();
        self.valid.shrink_to_fit();
    }

    fn heap_size(&self) -> usize {
        self.names.capacity()
            + self
                .renamed
                .as_ref()
                .map_or(0, |r| r.names.capacity() + r.spans.capacity() * 12)
            + self.ends.capacity() * 4
            + self.valid.capacity()
            + self.slots.capacity() * 8
    }
}

/// The text a dataset was read from, and the tables its model points into.
///
/// It holds the decoded input, followed by the texts stored since (owned
/// texts a program set, the texts of records moved in from other stores),
/// the pieces of the texts reading rewrote (8 bytes each, instead of a
/// copy of their characters), the identifiers ([`XrefId`]) and the tags
/// that are not standard ([`TagId`]). Every text and id of a dataset
/// resolves against its store.
#[derive(Clone, Default)]
pub struct Store {
    /// The decoded input, then the texts appended.
    text: String,
    /// How much of `text` the input is.
    read: usize,
    pieces: Vec<TextPiece>,
    xrefs: XrefTable,
    tags: Vec<Box<str>>,
    /// Texts past what 32-bit offsets reach, whole.
    big: Vec<Box<str>>,
    /// The standard tags of the lines read, as bits by index, when known
    /// ([`Store::read_tag`]).
    seen: Option<Box<[u64]>>,
}

/// The parts of a [`Store`] that reading fills: the text read (borrowed),
/// the pieces, the identifiers and the big texts.
pub(crate) struct StoreParts<'s> {
    /// The text read, which starts at `base` in the store's text.
    pub(crate) text: &'s str,
    base: usize,
    /// The store's input, which the store's text starts with.
    input: &'s str,
    pub(crate) pieces: &'s mut Vec<TextPiece>,
    pub(crate) xrefs: &'s mut XrefTable,
    pub(crate) big: &'s mut Vec<Box<str>>,
}

impl<'s> StoreParts<'s> {
    /// The parts with `text` read instead, which starts at `base` in the
    /// store's text: the text of records the store appends once they are
    /// read ([`Store::append_text`]), as reading rewrites records it reads.
    pub(crate) fn with_text<'x>(&'x mut self, text: &'x str, base: usize) -> StoreParts<'x> {
        StoreParts {
            text,
            base,
            input: self.input,
            pieces: self.pieces,
            xrefs: self.xrefs,
            big: self.big,
        }
    }

    /// The characters at `start` in the store's text, `len` long, of the
    /// text read.
    #[inline]
    fn local(&self, start: usize, len: usize) -> &'s str {
        let (text, start) = match start.checked_sub(self.base) {
            Some(start) => (self.text, start),
            None => (self.input, start),
        };
        text.get(start..start.saturating_add(len))
            .unwrap_or_default()
    }

    /// The identifier `text`'s id, if one was read.
    pub(crate) fn find_xref(&self, text: &str) -> Option<XrefId> {
        self.xrefs.find(text)
    }

    /// A span of the text from `start` (an offset in the store's text that
    /// may not fit in 32 bits), `len` long: a span, one piece when it is
    /// long, or a big text past 32 bits.
    pub(crate) fn span(&mut self, start: usize, len: usize) -> Span {
        if len == 0 {
            return Span::EMPTY;
        }
        match (u32::try_from(start), u32::try_from(len)) {
            (Ok(s), Ok(l)) if s.checked_add(l).is_some() => {
                let special = special_bytes(self.local(start, len));
                if l <= Span::MAX_LEN {
                    Span::new(Span::INPUT, s, l, special)
                } else {
                    long_span(self.pieces, self.big, self.local(start, len), s, l, special)
                }
            }
            _ => {
                let text = self.local(start, len);
                push_big(self.big, text.into(), special_bytes(text))
            }
        }
    }

    /// The text made of `pieces[at..]`, which the caller just pushed: a
    /// plain span when there is one piece without a newline (it is then
    /// taken back), nothing when there is none; a big text when they are
    /// too many.
    pub(crate) fn pieces_from(&mut self, at: usize) -> Span {
        match self.pieces.get(at..) {
            None | Some([]) => Span::EMPTY,
            Some(&[piece]) if !piece.newline() => {
                self.pieces.truncate(at);
                self.span(piece.start() as usize, piece.len() as usize)
            }
            Some(run) => {
                let special = run.iter().fold(0, |f, p| {
                    let newline = if p.newline() { LINE_FEED } else { 0 };
                    f | newline | special_bytes(self.local(p.start() as usize, p.len() as usize))
                });
                match (u32::try_from(at), u32::try_from(run.len())) {
                    (Ok(start), Ok(count)) if count <= Span::MAX_LEN => {
                        Span::of_pieces(start, count, special)
                    }
                    _ => {
                        let mut text = String::new();
                        for p in run {
                            if p.newline() {
                                text.push('\n');
                            }
                            text.push_str(self.local(p.start() as usize, p.len() as usize));
                        }
                        self.pieces.truncate(at);
                        push_big(self.big, text.into(), special)
                    }
                }
            }
        }
    }
}

/// A span longer than [`Span::MAX_LEN`]: one piece.
fn long_piece(
    pieces: &mut Vec<TextPiece>,
    big: &mut Vec<Box<str>>,
    text: &str,
    start: u32,
    len: u32,
    special: u8,
) -> Span {
    match u32::try_from(pieces.len()) {
        Ok(at) if len < (1 << 31) => {
            pieces.push(TextPiece::new(start, len, false));
            Span::of_pieces(at, 1, special)
        }
        _ => push_big(big, span(text, start, len).into(), special),
    }
}

/// [`long_piece`] of the span at `start` whose characters are `chars`.
fn long_span(
    pieces: &mut Vec<TextPiece>,
    big: &mut Vec<Box<str>>,
    chars: &str,
    start: u32,
    len: u32,
    special: u8,
) -> Span {
    match u32::try_from(pieces.len()) {
        Ok(at) if len < (1 << 31) => {
            pieces.push(TextPiece::new(start, len, false));
            Span::of_pieces(at, 1, special)
        }
        _ => push_big(big, chars.into(), special),
    }
}

/// A big text, kept whole.
fn push_big(big: &mut Vec<Box<str>>, text: Box<str>, special: u8) -> Span {
    let at = u32::try_from(big.len()).unwrap_or(u32::MAX);
    big.push(text);
    Span::new(Span::BIG, at, 0, special)
}

impl Store {
    pub(crate) fn new(text: String) -> Self {
        Self {
            read: text.len(),
            text,
            ..Self::default()
        }
    }

    /// [`Store::new`] for the text of one record: its few identifiers are
    /// searched in order, without an index.
    pub(crate) fn of_record(text: String) -> Self {
        let mut store = Self::new(text);
        store.xrefs.unindexed = UNINDEXED;
        store
    }

    /// The text read (without what was appended since).
    pub(crate) fn input(&self) -> &str {
        self.text.get(..self.read).unwrap_or(&self.text)
    }

    /// The text, the rest of the store dropped.
    pub(crate) fn into_input(self) -> String {
        self.text
    }

    /// The size of the text, in bytes.
    pub(crate) fn input_len(&self) -> usize {
        self.text.len()
    }

    /// The tables being filled while reading.
    pub(crate) fn parts_mut(&mut self) -> StoreParts<'_> {
        StoreParts {
            text: &self.text,
            base: 0,
            input: &self.text,
            pieces: &mut self.pieces,
            xrefs: &mut self.xrefs,
            big: &mut self.big,
        }
    }

    /// Appends `text` to the store's text, where the spans of the records
    /// read from it point ([`StoreParts::with_text`]).
    pub(crate) fn append_text(&mut self, text: &str) {
        self.text.push_str(text);
    }

    pub(crate) fn set_tags(&mut self, tags: Vec<Box<str>>) {
        self.tags = tags;
    }

    /// Notes the standard tags of lines read (`seen`, as bits by index).
    pub(crate) fn add_seen(&mut self, seen: &[u64]) {
        match &mut self.seen {
            Some(s) => s.iter_mut().zip(seen).for_each(|(a, b)| *a |= b),
            None => self.seen = Some(seen.into()),
        }
    }

    /// Whether a line read has the standard tag `tag`; `None` when the
    /// store does not know (its text was not read here).
    pub(crate) fn read_tag(&self, tag: &str) -> Option<bool> {
        let i = usize::from(crate::tree::standard_index(tag)?);
        self.seen
            .as_ref()
            .map(|s| s.get(i / 64).is_some_and(|w| w & (1 << (i % 64)) != 0))
    }

    /// The tags that are not standard, taken out (to read more records
    /// into the store, [`Store::set_tags`] gives them back).
    pub(crate) fn take_tags(&mut self) -> Vec<Box<str>> {
        std::mem::take(&mut self.tags)
    }

    /// The tags read that are not standard, once each.
    pub(crate) fn other_tags(&self) -> impl Iterator<Item = &str> {
        self.tags.iter().map(|t| &**t)
    }

    pub(crate) fn shrink(&mut self) {
        self.pieces.shrink_to_fit();
        self.xrefs.shrink();
    }

    /// Copies `text`, whose special bytes are `special`, into the store.
    pub(crate) fn push_text(&mut self, text: &str, special: u8) -> Span {
        if text.is_empty() {
            return Span::EMPTY;
        }
        let start = self.text.len();
        match (u32::try_from(start), u32::try_from(text.len())) {
            (Ok(s), Ok(l)) if s.checked_add(l).is_some() => {
                self.text.push_str(text);
                if l <= Span::MAX_LEN {
                    Span::new(Span::INPUT, s, l, special)
                } else {
                    long_piece(&mut self.pieces, &mut self.big, &self.text, s, l, special)
                }
            }
            _ => push_big(&mut self.big, text.into(), special),
        }
    }

    /// A span of the store's text longer than [`Span::MAX_LEN`].
    fn long_span(&mut self, start: u32, len: u32, special: u8) -> Span {
        long_piece(
            &mut self.pieces,
            &mut self.big,
            &self.text,
            start,
            len,
            special,
        )
    }

    /// A big text, kept whole.
    fn push_big(&mut self, text: Box<str>, special: u8) -> Span {
        push_big(&mut self.big, text, special)
    }

    /// The characters of a span: borrowed, unless they are joined from
    /// several pieces.
    #[inline]
    pub(crate) fn span_str(&self, s: Span) -> Cow<'_, str> {
        // Most spans are of the input, which is borrowed.
        if s.kind() == Span::INPUT {
            return Cow::Borrowed(span(&self.text, s.at, s.len_field()));
        }
        self.raw_str(s.kind(), s.at, s.len_field())
    }

    /// The characters of a span of `kind` at `at`, `len` long (the number
    /// of pieces for pieces).
    fn raw_str(&self, kind: u32, at: u32, len: u32) -> Cow<'_, str> {
        match kind {
            Span::PIECES => {
                let pieces = self.pieces_at(at, len);
                match pieces {
                    [p] if !p.newline() => Cow::Borrowed(span(&self.text, p.start(), p.len())),
                    _ => Cow::Owned(join(&self.text, pieces)),
                }
            }
            Span::BIG => Cow::Borrowed(self.big.get(at as usize).map_or("", |b| b)),
            Span::POINTER => Cow::Borrowed(""),
            _ => Cow::Borrowed(span(&self.text, at, len)),
        }
    }

    fn pieces_at(&self, at: u32, count: u32) -> &[TextPiece] {
        let start = at as usize;
        self.pieces
            .get(start..start.saturating_add(count as usize))
            .unwrap_or_default()
    }

    /// The characters of a span in order, without a copy.
    pub(crate) fn span_chunks(&self, s: Span) -> impl Iterator<Item = &str> + '_ {
        self.raw_chunks(s.kind(), s.at, s.len_field())
    }

    /// [`Store::span_chunks`] of a span's parts.
    fn raw_chunks(&self, kind: u32, at: u32, len: u32) -> impl Iterator<Item = &str> + '_ {
        let (single, pieces): (Option<&str>, &[TextPiece]) = match kind {
            Span::PIECES => (None, self.pieces_at(at, len)),
            Span::BIG => (self.big.get(at as usize).map(|b| &**b), &[]),
            Span::POINTER => (None, &[]),
            _ => (Some(span(&self.text, at, len)), &[]),
        };
        single
            .filter(|s| !s.is_empty())
            .into_iter()
            .chain(pieces.iter().flat_map(move |p| {
                let text = span(&self.text, p.start(), p.len());
                p.newline()
                    .then_some("\n")
                    .into_iter()
                    .chain(Some(text).filter(|t| !t.is_empty()))
            }))
    }

    /// Appends the text, pieces and big texts of `from`, for its spans to
    /// move here: how far each starts in this store's (text, pieces, big
    /// texts), or `None` when the offsets would pass the 32 bits spans
    /// hold (nothing is appended then).
    pub(crate) fn append(&mut self, from: &Store) -> Option<(u32, u32, u32)> {
        let text = u32::try_from(self.text.len()).ok()?;
        let pieces = u32::try_from(self.pieces.len()).ok()?;
        let big = u32::try_from(self.big.len()).ok()?;
        let end = self.text.len().checked_add(from.text.len())?;
        if u32::try_from(end).is_err()
            || u32::try_from(pieces as usize + from.pieces.len()).is_err()
            || u32::try_from(big as usize + from.big.len()).is_err()
        {
            return None;
        }
        self.text.push_str(&from.text);
        self.pieces
            .extend(from.pieces.iter().map(|p| p.shifted(text)));
        self.big.extend(from.big.iter().cloned());
        Some((text, pieces, big))
    }

    /// A span of `from` copied into this store (when [`Store::append`]
    /// could not take `from` whole).
    pub(crate) fn copy_span(&mut self, from: &Store, s: Span) -> Span {
        if s.kind() == Span::POINTER || s.is_empty() {
            return s;
        }
        let text = from.span_str(s);
        self.push_text(&text, s.special_all())
    }

    /// The text of an identifier, delimiters included: `@I1@`.
    #[must_use]
    pub fn xref(&self, id: XrefId) -> &str {
        self.xrefs.name(id.index()).unwrap_or("")
    }

    /// Whether identifier `id` is valid in the grammar of `rules`
    /// ([`VersionRules::is_valid_xref`](crate::version::VersionRules::is_valid_xref)).
    pub(crate) fn is_valid_xref(&self, id: XrefId, rules: &crate::version::VersionRules) -> bool {
        let bit = match rules.xref {
            crate::version::XrefGrammar::V7 => VALID_V7,
            crate::version::XrefGrammar::V551 { .. } => VALID_V551,
        };
        self.xrefs
            .valid
            .get(id.index())
            .is_some_and(|v| v & bit != 0)
    }

    /// The id of an identifier, if the dataset has it.
    #[must_use]
    pub fn find_xref(&self, xref: &str) -> Option<XrefId> {
        self.xrefs.find(xref)
    }

    /// The id of an identifier, made when the dataset does not have it
    /// yet. `None` only when four billion identifiers are taken.
    pub fn intern_xref(&mut self, xref: &str) -> Option<XrefId> {
        self.xrefs.intern(xref)
    }

    /// Gives identifier `id` the text `xref` ([`XrefTable::rename`]).
    pub(crate) fn rename_xref(&mut self, id: XrefId, xref: &str) -> bool {
        self.xrefs.rename(id, xref)
    }

    /// Whether `id` is an identifier of this store.
    pub(crate) fn has_xref(&self, id: XrefId) -> bool {
        id.index() < self.xrefs.len()
    }

    /// Whether identifier `id` is valid in 5.5.1 and in 7.x: what
    /// [`Store::is_valid_xref`] answers for each grammar.
    pub(crate) fn xref_validity(&self, id: XrefId) -> (bool, bool) {
        self.xref_validity_at(id.index())
    }

    /// [`Store::xref_validity`] of the identifier at `index`.
    pub(crate) fn xref_validity_at(&self, index: usize) -> (bool, bool) {
        let v = self.xrefs.valid.get(index).copied().unwrap_or(0);
        (v & VALID_V551 != 0, v & VALID_V7 != 0)
    }

    /// The number of identifiers, pointers to no record included.
    #[must_use]
    pub fn xref_count(&self) -> usize {
        self.xrefs.len()
    }

    /// The text of a tag.
    #[must_use]
    pub fn tag(&self, id: TagId) -> &str {
        let i = id.0 as usize;
        match STANDARD_TAGS.get(i) {
            Some(tag) => tag,
            None => self
                .tags
                .get(i.saturating_sub(STANDARD_TAGS.len()))
                .map_or("", |t| t),
        }
    }

    /// The id of a tag, made when it is neither standard nor known to the
    /// dataset yet.
    pub fn intern_tag(&mut self, tag: &str) -> TagId {
        if let Some(id) = TagId::standard(tag) {
            return id;
        }
        let at = self
            .tags
            .iter()
            .position(|t| &**t == tag)
            .unwrap_or_else(|| {
                self.tags.push(tag.into());
                self.tags.len() - 1
            });
        TagId(u32::try_from(STANDARD_TAGS.len() + at).unwrap_or(u32::MAX))
    }

    /// Bytes held: the text, the pieces and the tables.
    #[must_use]
    pub fn heap_size(&self) -> usize {
        self.text.capacity()
            + self.pieces.capacity() * std::mem::size_of::<TextPiece>()
            + self.xrefs.heap_size()
            + self.tags.iter().map(|t| t.len() + 16).sum::<usize>()
            + self.big.iter().map(|t| t.len() + 16).sum::<usize>()
    }
}

/// Equal stores hold the same text, pieces, identifiers (in the same
/// order) and tags: what their texts and ids resolve to.
impl PartialEq for Store {
    fn eq(&self, other: &Self) -> bool {
        let same_xrefs = if self.xrefs.renamed.is_none() && other.xrefs.renamed.is_none() {
            self.xrefs.names == other.xrefs.names && self.xrefs.ends == other.xrefs.ends
        } else {
            self.xrefs.len() == other.xrefs.len()
                && (0..self.xrefs.len()).all(|i| self.xrefs.name(i) == other.xrefs.name(i))
        };
        self.text == other.text
            && self.pieces == other.pieces
            && same_xrefs
            && self.tags == other.tags
            && self.big == other.big
    }
}

impl AsRef<Store> for Store {
    fn as_ref(&self) -> &Store {
        self
    }
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("text", &self.text.len())
            .field("read", &self.read)
            .field("pieces", &self.pieces.len())
            .field("xrefs", &self.xrefs.len())
            .field("tags", &self.tags)
            .field("big", &self.big.len())
            .field("seen", &self.seen.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_knows_the_standard_tags_read() {
        let data = crate::Dataset::parse(
            "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 BIRT\n2 AGE 0y\n\
             1 NOTE AGES and EMAIL in a text\n0 TRLR\n",
        );
        let store = data.store();
        assert_eq!(store.read_tag("AGE"), Some(true));
        assert_eq!(store.read_tag("BIRT"), Some(true));
        assert_eq!(store.read_tag("EMAIL"), Some(false));
        assert_eq!(store.read_tag("_UID"), None);
        assert_eq!(Store::default().read_tag("AGE"), None);
    }

    #[test]
    fn spans_pieces_owned_and_empty() {
        let mut store = Store::new("0 @I1@ INDI\nab@@cd".into());
        let t = Text::from_span(Span::of_input(store.input(), 2, 4));
        assert_eq!(t.to_str(&store), "@I1@");
        let parts = store.parts_mut();
        parts.pieces.extend([
            TextPiece::new(12, 3, false),
            TextPiece::new(16, 2, false),
            TextPiece::new(7, 4, true),
        ]);
        let joined = Text::from_span(Span::of_pieces(0, 3, LINE_FEED));
        assert_eq!(joined.to_str(&store), "ab@cd\nINDI");
        assert_eq!(joined.len(&store), 10);
        assert!(joined.eq_str(&store, "ab@cd\nINDI"));
        assert!(!joined.eq_str(&store, "ab@cd\nIND"));
        assert!(!joined.eq_str(&store, "ab@cd\nINDIX"));
        let r = TextRef::new(&store, Span::of_pieces(0, 3, LINE_FEED));
        assert_eq!(r, "ab@cd\nINDI");
        assert_eq!(r.to_string(), "ab@cd\nINDI");
        let mut copy = joined.clone();
        copy.make_owned(&store);
        assert_eq!(copy.as_owned(), Some("ab@cd\nINDI"));
        assert_eq!(Text::from(""), Text::default());
        assert!(Text::from_span(Span::EMPTY).is_empty());
        assert_eq!(Text::default().chunks(&store).count(), 0);
        // An owned text stored is copied into the store's text.
        let owned = Text::from("new");
        let span = owned.to_span(&mut store);
        assert_eq!(store.span_str(span), "new");
        assert_eq!(Text::from_span(span).to_str(&store), "new");
    }

    /// A few identifiers are searched in order; more are indexed; both
    /// find the same ids, before and after a rename.
    #[test]
    fn identifiers_found_in_order_then_indexed() {
        let mut store = Store::of_record(String::new());
        let names: Vec<String> = (0..40).map(|i| format!("@I{i}@")).collect();
        let ids: Vec<XrefId> = names
            .iter()
            .map(|n| store.intern_xref(n).unwrap())
            .collect();
        for (n, id) in names.iter().zip(&ids) {
            assert_eq!(store.intern_xref(n), Some(*id));
            assert_eq!(store.find_xref(n), Some(*id));
            assert_eq!(store.xref(*id), n);
        }
        assert_eq!(store.find_xref("@X@"), None);
        // A rename while the table has no index yet.
        let mut small = Store::of_record(String::new());
        let a = small.intern_xref("@A@").unwrap();
        let b = small.intern_xref("@B@").unwrap();
        assert!(small.rename_xref(a, "@C@"));
        assert_eq!(small.find_xref("@C@"), Some(a));
        assert_eq!(small.find_xref("@A@"), None);
        assert_eq!(small.find_xref("@B@"), Some(b));
        assert_eq!(small.intern_xref("@D@").map(XrefId::index), Some(2));
    }

    #[test]
    fn a_single_piece_is_a_span() {
        let mut store = Store::new("abcdef".into());
        let mut parts = store.parts_mut();
        parts.pieces.push(TextPiece::new(1, 3, false));
        let s = parts.pieces_from(0);
        assert!(parts.pieces.is_empty());
        assert_eq!(s, Span::of_input("abcdef", 1, 3));
        assert_eq!(parts.pieces_from(0), Span::EMPTY);
    }

    #[test]
    fn long_and_big_texts() {
        let long = "x".repeat(Span::MAX_LEN as usize + 10);
        let mut store = Store::new(long.clone());
        let s = store.parts_mut().span(0, long.len());
        assert_eq!(s.kind(), Span::PIECES);
        assert_eq!(store.span_str(s), long);
        let copied = Text::from(long.as_str()).to_span(&mut store);
        assert_eq!(store.span_str(copied), long);
        let big = push_big(&mut store.big, "big".into(), 0);
        assert_eq!(Text::from_span(big).to_str(&store), "big");
        assert_eq!(store.span_chunks(big).collect::<Vec<_>>(), ["big"]);
    }

    #[test]
    fn pointers_in_spans() {
        let mut store = Store::default();
        let id = store.intern_xref("@S1@").unwrap();
        let s = Span::pointer(id);
        assert_eq!(s.as_pointer(), Some(id));
        assert_eq!(Span::EMPTY.as_pointer(), None);
        assert_eq!(std::mem::size_of::<Option<Span>>(), 8);
    }

    #[test]
    fn identifiers_are_interned_once() {
        let mut store = Store::new("@I1@ @I1@ @F1@".into());
        let a = store.intern_xref("@I1@").unwrap();
        let b = store.intern_xref("@I1@").unwrap();
        let c = store.intern_xref("@F1@").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(store.xref(c), "@F1@");
        assert_eq!(store.find_xref("@F1@"), Some(c));
        assert_eq!(store.find_xref("@X@"), None);
        assert_eq!(store.xref_count(), 2);
    }

    #[test]
    fn many_identifiers() {
        let text: String = (0..5000).map(|i| format!("@I{i}@")).collect();
        let mut store = Store::new(text.clone());
        let parts = store.parts_mut();
        let mut ids = Vec::new();
        for i in 0..5000 {
            let x = format!("@I{i}@");
            ids.push(parts.xrefs.intern(&x).unwrap());
        }
        assert_eq!(store.xref_count(), 5000);
        for (i, id) in ids.iter().enumerate() {
            let x = format!("@I{i}@");
            assert_eq!(store.xref(*id), x);
            assert_eq!(store.find_xref(&x), Some(*id));
        }
        let made = store.intern_xref("@NEW@").unwrap();
        assert_eq!(store.xref(made), "@NEW@");
        assert_eq!(store.intern_xref("@I42@"), Some(ids[42]));
    }

    #[test]
    fn identifiers_are_renamed_in_place() {
        let text: String = (0..300).map(|i| format!("@I{i}@")).collect();
        let mut store = Store::new(text);
        let ids: Vec<XrefId> = (0..300)
            .map(|i| store.intern_xref(&format!("@I{i}@")).unwrap())
            .collect();
        assert!(store.rename_xref(ids[7], "@P7@"));
        assert_eq!(store.xref(ids[7]), "@P7@");
        assert_eq!(store.find_xref("@P7@"), Some(ids[7]));
        assert_eq!(store.find_xref("@I7@"), None);
        // Every other identifier is still found, whatever the probing.
        for (i, id) in ids.iter().enumerate().filter(|(i, _)| *i != 7) {
            assert_eq!(store.find_xref(&format!("@I{i}@")), Some(*id), "{i}");
        }
        assert!(store.is_valid_xref(ids[7], &crate::version::V70));
        assert!(store.rename_xref(ids[7], "@p7@"));
        assert!(!store.is_valid_xref(ids[7], &crate::version::V70));
        // Taking the text of an unused id: that id is no longer found.
        let spare = store.intern_xref("@SPARE@").unwrap();
        assert!(store.rename_xref(ids[8], "@SPARE@"));
        assert_eq!(store.find_xref("@SPARE@"), Some(ids[8]));
        assert_ne!(spare, ids[8]);
        let copy = store.clone();
        assert_eq!(copy, store);
        // A new identifier after renames is interned once.
        let made = store.intern_xref("@I7@").unwrap();
        assert_eq!(store.intern_xref("@I7@"), Some(made));
        assert_ne!(copy, store);
    }

    #[test]
    fn tags() {
        let mut store = Store::default();
        let name = TagId::standard("NAME").unwrap();
        assert_eq!(store.tag(name), "NAME");
        let ext = store.intern_tag("_X");
        assert_eq!(store.intern_tag("_X"), ext);
        assert_eq!(store.tag(ext), "_X");
    }

    #[test]
    fn sizes() {
        assert_eq!(std::mem::size_of::<Text>(), 16);
        assert_eq!(std::mem::size_of::<Option<Text>>(), 16);
        assert_eq!(std::mem::size_of::<Option<XrefId>>(), 4);
        assert_eq!(std::mem::size_of::<TextPiece>(), 8);
    }
}
