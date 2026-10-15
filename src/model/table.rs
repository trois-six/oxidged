//! The tables of a dataset: every typed structure in a contiguous table of
//! its type, addressed by 32-bit indices and ranges.
//!
//! A dataset keeps no structure in an allocation of its own. Each type of
//! the model has a table of *rows* (its fields in a fixed-size, `Copy`
//! form) in the dataset's [`Tables`]:
//!
//! - a text is a [`Span`] of the store (8 bytes), an identifier or a
//!   pointer an [`XrefId`] (4 bytes), an enumeration value a [`Code`] (4
//!   bytes: its variant, or the index of the text of a value it does not
//!   name);
//! - an optional substructure is the [`Idx`] of its row in its table (4
//!   bytes), a list of them a [`List`]: a run of rows of their table
//!   (8 bytes). A structure's rows of one list are contiguous, whatever
//!   they hold: a reader reserves them before it reads them;
//! - lists of texts, pointers and enumeration values are runs of shared
//!   tables of spans, ids and codes;
//! - the substructures a structure keeps untyped (`extra`) are rows of the
//!   table of nodes, which a side table of the structure's table finds by
//!   the structure's row: most structures have none, and pay nothing.
//!
//! A table grows as a file is read and is shrunk to its size when the read
//! completes. Views ([`View`], [`Items`]) read the rows where they are;
//! the owned types of the model (the structs of [`crate::model`]) are what
//! a program builds and edits, and a dataset stores them as rows
//! ([`Dataset::add`](super::Dataset::add)) and gives them back
//! ([`Dataset::get_mut`](super::Dataset::get_mut)).

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroU32;

use super::dataset::Dataset;
use super::node::Node;
use super::text::{Span, Store, TagId, XrefId};

/// The index of a row of a table of `T`, plus one (so that an
/// `Option<Idx<T>>` is 4 bytes).
#[doc(hidden)]
pub struct Idx<T>(NonZeroU32, PhantomData<fn() -> T>);

impl<T> Idx<T> {
    /// The index of row `at`; `None` past four billion rows.
    #[inline]
    pub(crate) fn new(at: u32) -> Option<Self> {
        at.checked_add(1)
            .and_then(NonZeroU32::new)
            .map(|i| Self(i, PhantomData))
    }

    /// The row.
    #[inline]
    pub(crate) fn get(self) -> u32 {
        self.0.get() - 1
    }
}

impl<T> Clone for Idx<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Idx<T> {}

impl<T> PartialEq for Idx<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T> Eq for Idx<T> {}

impl<T> fmt::Debug for Idx<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.get())
    }
}

/// A run of `len` items of a table from `start`.
#[doc(hidden)]
pub struct List<T> {
    pub(crate) start: u32,
    pub(crate) len: u32,
    item: PhantomData<fn() -> T>,
}

impl<T> List<T> {
    /// No item.
    pub(crate) const EMPTY: Self = Self {
        start: 0,
        len: 0,
        item: PhantomData,
    };

    #[inline]
    pub(crate) fn new(start: u32, len: u32) -> Self {
        Self {
            start,
            len,
            item: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn is_empty(self) -> bool {
        self.len == 0
    }

    /// The range of the table's items.
    #[inline]
    pub(crate) fn range(self) -> std::ops::Range<usize> {
        let start = self.start as usize;
        start..start + self.len as usize
    }

    /// The items of `table`: none when the run is not within it.
    #[inline]
    pub(crate) fn of<E>(self, table: &[E]) -> &[E] {
        table.get(self.range()).unwrap_or_default()
    }
}

impl<T> Default for List<T> {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl<T> Clone for List<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for List<T> {}

impl<T> PartialEq for List<T> {
    fn eq(&self, other: &Self) -> bool {
        (self.start, self.len) == (other.start, other.len)
    }
}

impl<T> fmt::Debug for List<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}+{}]", self.start, self.len)
    }
}

/// An enumeration value in a table: a variant the enumeration names (its
/// index plus one), or, high bit set, the index in [`Tables::spans`] of
/// the text of a value it does not name.
#[doc(hidden)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Code(NonZeroU32);

impl Code {
    const UNKNOWN: u32 = 1 << 31;

    /// The variant at `index`.
    #[inline]
    pub(crate) fn known(index: usize) -> Self {
        let i = u32::try_from(index).unwrap_or(0) & !Self::UNKNOWN;
        Self(NonZeroU32::new(i + 1).unwrap_or(NonZeroU32::MIN))
    }

    /// A value no variant names, its text at `index` of the spans.
    #[inline]
    pub(crate) fn unknown(index: u32) -> Self {
        Self(NonZeroU32::new(Self::UNKNOWN | index).unwrap_or(NonZeroU32::MIN))
    }

    /// The variant's index, for a value the enumeration names.
    #[inline]
    pub(crate) fn variant(self) -> Option<usize> {
        let v = self.0.get();
        (v & Self::UNKNOWN == 0).then(|| (v - 1) as usize)
    }

    /// The index of the text of a value the enumeration does not name.
    #[inline]
    pub(crate) fn text_index(self) -> Option<usize> {
        let v = self.0.get();
        (v & Self::UNKNOWN != 0).then_some((v & !Self::UNKNOWN) as usize)
    }
}

impl Default for Code {
    fn default() -> Self {
        Self::known(0)
    }
}

/// A table of rows, and the untyped substructures of its rows that have
/// some.
#[derive(Clone)]
pub(crate) struct Table<R> {
    pub(crate) rows: Vec<R>,
    /// The rows that have untyped substructures.
    extra: RowExtras,
}

/// The rows of a table that have untyped substructures, sorted by row:
/// each with its run of [`Tables::nodes`]. Boxed: most tables have none.
/// Whatever the type of the rows, one copy of its code.
#[derive(Default)]
#[expect(
    clippy::box_collection,
    reason = "one word in each of the tables, most of which have no extras"
)]
struct RowExtras(Option<Box<Vec<RowExtra>>>);

/// A row with untyped substructures, and their run of [`Tables::nodes`].
type RowExtra = (u32, List<Node>);

impl Clone for RowExtras {
    #[inline(never)]
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl RowExtras {
    const fn new() -> Self {
        Self(None)
    }

    #[inline]
    fn get(&self, at: u32) -> List<Node> {
        let Some(extra) = self.0.as_deref() else {
            return List::EMPTY;
        };
        match extra.binary_search_by_key(&at, |&(r, _)| r) {
            Ok(i) => extra.get(i).map_or(List::EMPTY, |&(_, l)| l),
            Err(_) => List::EMPTY,
        }
    }

    #[inline(never)]
    fn set(&mut self, at: u32, list: List<Node>) {
        if self.0.is_none() && list.is_empty() {
            return;
        }
        let extra = self.0.get_or_insert_with(Box::default);
        match extra.binary_search_by_key(&at, |&(r, _)| r) {
            Ok(i) if list.is_empty() => {
                extra.remove(i);
            }
            Ok(i) => {
                if let Some(e) = extra.get_mut(i) {
                    e.1 = list;
                }
            }
            Err(_) if list.is_empty() => {}
            Err(i) => extra.insert(i, (at, list)),
        }
    }

    /// Row `at` removed: the rows after it move back by one.
    #[inline(never)]
    fn remove(&mut self, at: u32) {
        if let Some(extra) = self.0.as_deref_mut() {
            extra.retain(|&(r, _)| r != at);
            for e in extra.iter_mut() {
                if e.0 > at {
                    e.0 -= 1;
                }
            }
        }
    }

    /// Empties the list, keeping its capacity: the bytes it holds.
    #[inline(never)]
    fn clear(&mut self) -> usize {
        self.0.as_deref_mut().map_or(0, |e| {
            e.clear();
            24 + e.capacity() * 12
        })
    }

    #[inline(never)]
    fn shrink(&mut self) {
        if let Some(extra) = self.0.as_deref_mut() {
            extra.shrink_to_fit();
        }
    }

    fn heap_size(&self) -> usize {
        self.0.as_deref().map_or(0, |e| 24 + e.capacity() * 12)
    }
}

impl<R> Table<R> {
    pub(crate) const fn new() -> Self {
        Self {
            rows: Vec::new(),
            extra: RowExtras::new(),
        }
    }

    /// The untyped substructures of row `at`.
    #[inline]
    pub(crate) fn extra(&self, at: u32) -> List<Node> {
        self.extra.get(at)
    }

    /// Gives row `at` untyped substructures (none: takes them away).
    pub(crate) fn set_extra(&mut self, at: u32, list: List<Node>) {
        self.extra.set(at, list);
    }

    /// Removes row `at`, the rows after it moving back by one with their
    /// untyped substructures.
    pub(crate) fn remove(&mut self, at: u32) {
        if (at as usize) < self.rows.len() {
            self.rows.remove(at as usize);
        }
        self.extra.remove(at);
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// Empties the table, keeping its capacity: the bytes it holds.
    #[inline]
    fn clear(&mut self) -> usize {
        let extra = if self.extra.0.is_some() {
            self.extra.clear()
        } else {
            0
        };
        clear_vec(&mut self.rows) + extra
    }

    #[inline]
    fn shrink(&mut self) {
        // A few spare rows are not worth a reallocation.
        let spare = (self.rows.capacity() - self.rows.len()) * std::mem::size_of::<R>();
        if spare > SHRINK_SLACK {
            self.rows.shrink_to_fit();
        }
        if self.extra.0.is_some() {
            self.extra.shrink();
        }
    }

    fn heap_size(&self) -> usize {
        self.rows.capacity() * std::mem::size_of::<R>() + self.extra.heap_size()
    }
}

/// The most spare bytes a table keeps when a dataset releases its spare
/// capacity ([`Tables::shrink`]).
const SHRINK_SLACK: usize = 128;

impl<R> Default for Table<R> {
    fn default() -> Self {
        Self::new()
    }
}

/// An untyped structure in a table: its tag, identifier, payload (a span,
/// a pointer in a span, or none) and substructures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeRow {
    pub(crate) tag: TagId,
    pub(crate) xref: Option<XrefId>,
    pub(crate) payload: Option<Span>,
    pub(crate) children: List<Node>,
}

impl Default for NodeRow {
    fn default() -> Self {
        Self {
            tag: TagId::ASIDE,
            xref: None,
            payload: None,
            children: List::EMPTY,
        }
    }
}

/// A name piece in a table: its kind and its text.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NamePieceRow {
    pub(crate) kind: super::name::NamePieceKind,
    pub(crate) value: Span,
}

/// An enumeration value with its phrase in a table.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhrasedRow {
    pub(crate) value: Option<Code>,
    pub(crate) phrase: Option<Span>,
}

/// Declares the tables: one per typed structure (and detail), and the
/// shared ones. The tables of the structures most files have are kept
/// inline (`hot`), the others in a box allocated with the first of them
/// (`cold`): a dataset of one record, which a stream yields, stays small.
macro_rules! tables {
    (
        hot { $($field:ident: $ty:ty,)* }
        cold { $($cfield:ident: $cty:ty,)* }
    ) => {
        /// Every table of a dataset.
        #[derive(Clone)]
        pub(crate) struct Tables {
            $(pub(crate) $field: Table<<$ty as sealed::Stored>::Row>,)*
            /// The tables of the structures few files have.
            pub(crate) cold: Option<Box<ColdTables>>,
            /// The texts of lists of texts, and of enumeration values no
            /// variant names.
            pub(crate) spans: Vec<Span>,
            /// The pointers of lists of pointers.
            pub(crate) xrefs: Vec<Option<XrefId>>,
            /// The values of lists of enumeration values.
            pub(crate) codes: Vec<Option<Code>>,
            /// Lists of enumeration values, in a list.
            pub(crate) code_lists: Vec<List<Code>>,
            /// Integers in a list.
            pub(crate) numbers: Vec<u32>,
            /// Name pieces.
            pub(crate) name_pieces: Vec<NamePieceRow>,
            /// Enumeration values with their phrases.
            pub(crate) phrased: Table<PhrasedRow>,
            /// Untyped structures: the substructures typed structures keep
            /// untyped, and theirs.
            pub(crate) nodes: Vec<NodeRow>,
            /// The records of no type the model has.
            pub(crate) others: Table<NodeRow>,
            /// Rows no structure holds any more (edits, structures that did
            /// not fit): what a compaction would give back.
            pub(crate) garbage: usize,
            /// The record lent by [`Dataset::get_mut`](super::Dataset::get_mut),
            /// copied out, until the guard stores it back.
            pub(crate) lent: Lent,
        }

        /// The tables of the structures few files have.
        #[derive(Clone, Default)]
        pub(crate) struct ColdTables {
            $(pub(crate) $cfield: Table<<$cty as sealed::Stored>::Row>,)*
        }

        $(
            impl HasTable for $ty {
                #[inline]
                fn table(t: &Tables) -> &Table<Self::Row> {
                    &t.$field
                }
                #[inline]
                fn table_mut(t: &mut Tables) -> &mut Table<Self::Row> {
                    &mut t.$field
                }
            }
        )*

        $(
            impl HasTable for $cty {
                #[inline]
                fn table(t: &Tables) -> &Table<Self::Row> {
                    static EMPTY: Table<<$cty as sealed::Stored>::Row> = Table::new();
                    t.cold.as_deref().map_or(&EMPTY, |c| &c.$cfield)
                }
                #[inline]
                fn table_mut(t: &mut Tables) -> &mut Table<Self::Row> {
                    &mut t.cold.get_or_insert_with(Box::default).$cfield
                }
            }
        )*

        impl Tables {
            /// No rows.
            pub(crate) const fn new() -> Self {
                Self {
                    $($field: Table::new(),)*
                    cold: None,
                    spans: Vec::new(),
                    xrefs: Vec::new(),
                    codes: Vec::new(),
                    code_lists: Vec::new(),
                    numbers: Vec::new(),
                    name_pieces: Vec::new(),
                    phrased: Table::new(),
                    nodes: Vec::new(),
                    others: Table::new(),
                    garbage: 0,
                    lent: Lent(None),
                }
            }

            /// Empties every table, keeping its capacity: the bytes it
            /// holds.
            fn clear(&mut self) -> usize {
                use std::mem::size_of;
                let cold = self.cold.as_deref_mut().map_or(0, |c| {
                    size_of::<ColdTables>() $(+ c.$cfield.clear())*
                });
                self.garbage = 0;
                self.lent = Lent(None);
                cold $(+ self.$field.clear())*
                    + clear_vec(&mut self.spans)
                    + clear_vec(&mut self.xrefs)
                    + clear_vec(&mut self.codes)
                    + clear_vec(&mut self.code_lists)
                    + clear_vec(&mut self.numbers)
                    + clear_vec(&mut self.name_pieces)
                    + self.phrased.clear()
                    + clear_vec(&mut self.nodes)
                    + self.others.clear()
            }

            /// Releases spare capacity.
            pub(crate) fn shrink(&mut self) {
                $(self.$field.shrink();)*
                if let Some(c) = self.cold.as_deref_mut() {
                    $(c.$cfield.shrink();)*
                }
                self.spans.shrink_to_fit();
                self.xrefs.shrink_to_fit();
                self.codes.shrink_to_fit();
                self.code_lists.shrink_to_fit();
                self.numbers.shrink_to_fit();
                self.name_pieces.shrink_to_fit();
                self.phrased.shrink();
                self.nodes.shrink_to_fit();
                self.others.shrink();
            }

            /// Bytes held.
            pub(crate) fn heap_size(&self) -> usize {
                use std::mem::size_of;
                let cold = self.cold.as_deref().map_or(0, |c| {
                    size_of::<ColdTables>() $(+ c.$cfield.heap_size())*
                });
                cold $(+ self.$field.heap_size())*
                    + self.spans.capacity() * size_of::<Span>()
                    + self.xrefs.capacity() * 4
                    + self.codes.capacity() * 4
                    + self.code_lists.capacity() * size_of::<List<Code>>()
                    + self.numbers.capacity() * 4
                    + self.name_pieces.capacity() * size_of::<NamePieceRow>()
                    + self.phrased.heap_size()
                    + self.nodes.capacity() * size_of::<NodeRow>()
                    + self.others.heap_size()
            }

            /// The number of rows of every table, for the record.
            pub(crate) fn row_counts(&self) -> Vec<(&'static str, usize, usize)> {
                use std::mem::size_of;
                let mut out = vec![
                    $((stringify!($field), self.$field.len(), size_of::<<$ty as sealed::Stored>::Row>()),)*
                ];
                if let Some(c) = self.cold.as_deref() {
                    out.extend([
                        $((stringify!($cfield), c.$cfield.len(), size_of::<<$cty as sealed::Stored>::Row>()),)*
                    ]);
                }
                out.extend([
                    ("spans", self.spans.len(), size_of::<Span>()),
                    ("xrefs", self.xrefs.len(), 4),
                    ("codes", self.codes.len(), 4),
                    ("name_pieces", self.name_pieces.len(), size_of::<NamePieceRow>()),
                    ("phrased", self.phrased.len(), size_of::<PhrasedRow>()),
                    ("nodes", self.nodes.len(), size_of::<NodeRow>()),
                    ("others", self.others.len(), size_of::<NodeRow>()),
                ]);
                out
            }

            /// The rows of every table.
            pub(crate) fn rows(&self) -> usize {
                let cold = self.cold.as_deref().map_or(0, |c| 0 $(+ c.$cfield.len())*);
                cold $(+ self.$field.len())*
                    + self.spans.len()
                    + self.xrefs.len()
                    + self.codes.len()
                    + self.code_lists.len()
                    + self.numbers.len()
                    + self.name_pieces.len()
                    + self.phrased.len()
                    + self.nodes.len()
                    + self.others.len()
            }
        }
    };
}

use super::address::Address;
use super::citation::{
    CallNumber, Citation, CitationData, CitationDetail, CitedEvent, RepositoryCitation,
};
use super::date::{Age, ChangeDate, CreationDate, Date, DateDetail, ExactDate, Period};
use super::enums::LdsStatus;
use super::event::{Event, EventDetail, EventFamily, EventSpouse, NonEvent};
use super::header::{
    Charset, Corporation, GedcomInfo, Header, HeaderPlace, HeaderSource, HeaderSourceData,
    HeaderText, Schema,
};
use super::identifier::{Exid, Refn};
use super::lds::Ordinance;
use super::link::{
    Association, ChildLink, ChildLinkDetail, IndividualRef, IndividualRefDetail, SpouseLink,
};
use super::multimedia::{Crop, File, FileForm, FileTranslation, MultimediaLink};
use super::name::{Name, NameDetail, NameTranslation, PhoneticName, RomanizedName};
use super::note::{Note, NoteDetail, NoteTranslation, SourceText, TextTranslation};
use super::place::{
    Map, PhoneticVariation, Place, PlaceDetail, PlaceTranslation, RomanizedVariation,
};
use super::record::{
    Family, FamilyDetail, Individual, IndividualDetail, Multimedia, RecordedEvents, Repository,
    SharedNote, Source, SourceData, Submission, Submitter,
};

tables! {
    hot {
        individual: Individual,
        individual_detail: IndividualDetail,
        family: Family,
        family_detail: FamilyDetail,
        source: Source,
        repository: Repository,
        repository_citation: RepositoryCitation,
        multimedia: Multimedia,
        submitter: Submitter,
        shared_note: SharedNote,
        event: Event,
        event_detail: EventDetail,
        name: Name,
        name_detail: NameDetail,
        date: Date,
        place: Place,
        place_detail: PlaceDetail,
        citation: Citation,
        citation_detail: CitationDetail,
        citation_data: CitationData,
        note: Note,
        spouse_link: SpouseLink,
        child_link: ChildLink,
        individual_ref: IndividualRef,
        change_date: ChangeDate,
        exact_date: ExactDate,
        multimedia_link: MultimediaLink,
        file: File,
        file_form: FileForm,
        address: Address,
        refn: Refn,
        age: Age,
        source_text: SourceText,
    }
    cold {
        source_data: SourceData,
        recorded_events: RecordedEvents,
        submission: Submission,
        header: Header,
        gedcom_info: GedcomInfo,
        charset: Charset,
        header_source: HeaderSource,
        corporation: Corporation,
        header_source_data: HeaderSourceData,
        header_place: HeaderPlace,
        schema: Schema,
        header_text: HeaderText,
        cited_event: CitedEvent,
        call_number: CallNumber,
        date_detail: DateDetail,
        period: Period,
        creation_date: CreationDate,
        lds_status: LdsStatus,
        event_spouse: EventSpouse,
        event_family: EventFamily,
        non_event: NonEvent,
        exid: Exid,
        ordinance: Ordinance,
        individual_ref_detail: IndividualRefDetail,
        child_link_detail: ChildLinkDetail,
        association: Association,
        crop: Crop,
        file_translation: FileTranslation,
        name_translation: NameTranslation,
        phonetic_name: PhoneticName,
        romanized_name: RomanizedName,
        note_detail: NoteDetail,
        note_translation: NoteTranslation,
        text_translation: TextTranslation,
        place_translation: PlaceTranslation,
        map: Map,
        phonetic_variation: PhoneticVariation,
        romanized_variation: RomanizedVariation,
    }
}

/// The record a dataset lends ([`Dataset::get_mut`](super::Dataset::get_mut)),
/// copied out as its owned type, while it is lent. A copy of the tables
/// has none: the loan is the original's.
#[derive(Default)]
pub(crate) struct Lent(pub(crate) Option<Box<dyn std::any::Any + Send + Sync>>);

impl Clone for Lent {
    fn clone(&self) -> Self {
        Self(None)
    }
}

/// Empties a list, keeping its capacity: the bytes it holds.
fn clear_vec<T>(v: &mut Vec<T>) -> usize {
    v.clear();
    v.capacity() * std::mem::size_of::<T>()
}

impl Default for Tables {
    fn default() -> Self {
        Self::new()
    }
}

/// The most bytes the tables of a dropped dataset may hold to be kept for
/// the next dataset of the thread: those of a record, not of a file.
const SPARE_LIMIT: usize = 16 * 1024;

thread_local! {
    /// The tables of the last small dataset the thread dropped, emptied.
    static SPARE: Cell<Option<Box<Tables>>> = const { Cell::new(None) };
}

/// The tables of a dataset, boxed, so that a dataset of one record (what
/// a stream yields) moves cheaply.
///
/// A stream makes a dataset of every record, and its reader drops it: the
/// tables of a dropped dataset, when they hold at most [`SPARE_LIMIT`]
/// bytes, are emptied and kept for the next dataset of the thread, which
/// fills them instead of allocating each of its tables again.
#[derive(Clone)]
pub(crate) struct TableBox(Option<Box<Tables>>);

impl Default for TableBox {
    fn default() -> Self {
        let spare = SPARE.try_with(Cell::take).ok().flatten();
        Self(Some(spare.unwrap_or_default()))
    }
}

impl Drop for TableBox {
    fn drop(&mut self) {
        if let Some(mut tables) = self.0.take()
            && tables.clear() <= SPARE_LIMIT
        {
            // Fails only while the thread ends: the tables are dropped.
            let _ = SPARE.try_with(|spare| spare.set(Some(tables)));
        }
    }
}

impl std::ops::Deref for TableBox {
    type Target = Tables;
    #[inline]
    fn deref(&self) -> &Tables {
        self.0.as_deref().unwrap_or(&EMPTY_TABLES)
    }
}

impl std::ops::DerefMut for TableBox {
    #[inline]
    fn deref_mut(&mut self) -> &mut Tables {
        self.0.get_or_insert_with(Box::default)
    }
}

/// The tables of a dataset that has none, for views of an owned structure
/// with no dataset at hand.
pub(crate) static EMPTY_TABLES: Tables = Tables::new();

pub(crate) mod sealed {
    use super::super::Dataset;

    /// The row of a type of the model: its fields in a dataset's tables.
    pub trait Stored {
        /// The row.
        type Row: Copy + Default + Send + Sync + 'static;
        /// A row with every field empty: what a view of no row reads.
        fn empty() -> &'static Self::Row;
    }

    /// A type of a field of the model, and what a view gives of it.
    pub trait Field {
        /// What a view gives of a field of this type.
        type View<'a>;
    }

    /// What a list of a type of the model holds, and its view: the side
    /// of [`Item`](super::Item) that [`Items`](super::Items) needs.
    pub trait Listed: Sized {
        /// An item of a list.
        type Elem: Copy + PartialEq + std::fmt::Debug + 'static;
        /// What a view gives of an item.
        type View<'a>;
        /// The view of item `at`.
        fn view_in<'a>(e: &'a Self::Elem, at: u32, data: &'a Dataset) -> Self::View<'a>;
        /// Item `at`, as its owned type.
        fn load_in(e: &Self::Elem, at: u32, data: &Dataset) -> Self;
    }
}

/// Implements [`sealed::Listed`] for a type that implements [`Item`], with
/// its item and its view.
macro_rules! listed {
    ([$($gen:tt)*] $ty:ty, $elem:ty, $view:ty) => {
        impl<$($gen)*> $crate::model::table::sealed::Listed for $ty {
            type Elem = $elem;
            type View<'a> = $view;

            #[inline]
            fn view_in<'a>(
                e: &'a Self::Elem,
                at: u32,
                data: &'a $crate::model::Dataset,
            ) -> Self::View<'a> {
                <Self as $crate::model::table::Item>::view_elem(e, at, data)
            }

            fn load_in(e: &Self::Elem, at: u32, data: &$crate::model::Dataset) -> Self {
                <Self as $crate::model::table::Item>::load_elem(e, at, &data.tables)
            }
        }
    };
}
pub(crate) use listed;

/// A type of the model that lists of a dataset hold: what [`Items`] lists.
pub trait Listed: sealed::Listed {}

impl<T: sealed::Listed> Listed for T {}

/// A type of a field of the model: what [`FieldView`] maps.
pub trait Field: sealed::Field {}

impl<T: sealed::Field> Field for T {}

/// What the view of a structure gives of a field of type `F`: the view of
/// what the field holds.
///
/// | Field | View |
/// |---|---|
/// | a structure `T` | [`View<'a, T>`](View) |
/// | `Option<T>` | `Option` of the view of `T` |
/// | `Vec<T>`, [`ThinVec<T>`](super::ThinVec) | [`Items<'a, T>`](Items), which gives the view of each |
/// | [`Text`](super::Text) | [`TextRef<'a>`](super::TextRef) |
/// | [`XrefId`], an enumeration value, a number | the value itself |
pub type FieldView<'a, F> = <F as sealed::Field>::View<'a>;

impl<T: Listed> sealed::Field for Option<T> {
    type View<'a> = Option<<T as sealed::Listed>::View<'a>>;
}

impl<T: Listed> sealed::Field for Vec<T> {
    type View<'a> = Items<'a, T>;
}

impl<T: Listed> sealed::Field for super::list::ThinVec<T> {
    type View<'a> = Items<'a, T>;
}

impl sealed::Field for super::text::Text {
    type View<'a> = super::text::TextRef<'a>;
}

/// A type of the model that a dataset keeps in a table of rows: a
/// structure ([`View`] reads one where it is).
pub trait Stored: sealed::Stored {}

impl<T: sealed::Stored> Stored for T {}

/// Where the table of a type is.
pub(crate) trait HasTable: sealed::Stored {
    fn table(t: &Tables) -> &Table<Self::Row>;
    fn table_mut(t: &mut Tables) -> &mut Table<Self::Row>;
}

/// A structure of a dataset, read where its row is: what the dataset's
/// accessors give ([`Dataset::individuals`](super::Dataset::individuals),
/// [`Dataset::find_family`](super::Dataset::find_family), …).
///
/// A view is `Copy`: the dataset, the row and its index. It has a method
/// per field of its type, which gives the field's value as a view in turn:
/// a substructure as a `View` (`Option<View<'a, Date>>`), a list as
/// [`Items`], a text as a [`TextRef`](super::TextRef), a pointer as an
/// [`XrefId`], an enumeration value as its type. The fields a type keeps
/// in its *detail* (`event.detail().age` on the owned type) are methods of
/// the view like the others (`event.age()`). [`View::into_owned`] copies the
/// structure out as its owned type, to edit or keep.
///
/// ```rust
/// use oxidged::model::{Dataset, EventKind};
///
/// let data = Dataset::parse(
///     "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 BIRT\n2 DATE 1 JAN 1900\n\
///      2 AGE 0y\n1 OCCU Miller\n0 TRLR\n",
/// );
/// let ann = data.individuals().first().unwrap();
/// let birth = ann.events().first().unwrap();
/// assert_eq!(birth.kind(), EventKind::Birth);
/// assert_eq!(birth.date().unwrap().value(), "1 JAN 1900");
/// assert_eq!(birth.age().unwrap().value(), "0y");
/// assert_eq!(ann.events().get(1).unwrap().value(), "Miller");
/// assert_eq!(ann.into_owned().events.len(), 2);
/// ```
pub struct View<'a, T: Stored> {
    pub(crate) data: &'a Dataset,
    pub(crate) row: &'a T::Row,
    pub(crate) at: u32,
}

impl<T: Stored> Clone for View<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Stored> Copy for View<'_, T> {}

impl<'a, T: Stored> View<'a, T> {
    pub(crate) fn new(data: &'a Dataset, row: &'a T::Row, at: u32) -> Self {
        Self { data, row, at }
    }

    /// The dataset the structure is of.
    #[must_use]
    pub fn dataset(self) -> &'a Dataset {
        self.data
    }

    /// The store its texts and identifiers resolve against.
    #[must_use]
    pub fn store(self) -> &'a Store {
        &self.data.store
    }

    pub(crate) fn tables(self) -> &'a Tables {
        &self.data.tables
    }
}

impl<T: Stored> AsRef<Store> for View<'_, T> {
    fn as_ref(&self) -> &Store {
        &self.data.store
    }
}

impl<T> View<'_, T>
where
    T: Stored + sealed::Listed<Elem = <T as sealed::Stored>::Row>,
{
    /// The structure, copied out as its owned type (to edit, or to keep
    /// without the dataset): its texts still point into the dataset's
    /// store.
    #[must_use]
    pub fn into_owned(self) -> T {
        T::load_in(self.row, self.at, self.data)
    }
}

/// Two views are equal when they are of the same structure of the same
/// dataset.
impl<T: Stored> PartialEq for View<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.data, other.data) && std::ptr::eq(self.row, other.row)
    }
}

/// A list of a dataset: structures, texts, pointers or values, read where
/// they are, each as the type's view ([`View`] for a structure). Iterating
/// gives them in order; it also has `len`, `get`, `first` and `last`.
pub struct Items<'a, T: Listed> {
    data: &'a Dataset,
    elems: &'a [T::Elem],
    start: u32,
}

impl<T: Listed> Clone for Items<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Listed> Copy for Items<'_, T> {}

impl<'a, T: Listed> Items<'a, T> {
    pub(crate) fn new(data: &'a Dataset, items: &'a [T::Elem], start: u32) -> Self {
        Self {
            data,
            elems: items,
            start,
        }
    }

    /// The number of items.
    #[must_use]
    pub fn len(self) -> usize {
        self.elems.len()
    }

    /// Whether there is none.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.elems.is_empty()
    }

    /// The item at `index`.
    #[must_use]
    pub fn get(self, index: usize) -> Option<T::View<'a>> {
        let e = self.elems.get(index)?;
        let at = self
            .start
            .saturating_add(u32::try_from(index).unwrap_or(u32::MAX));
        Some(T::view_in(e, at, self.data))
    }

    /// The first item.
    #[must_use]
    pub fn first(self) -> Option<T::View<'a>> {
        self.get(0)
    }

    /// The last item.
    #[must_use]
    pub fn last(self) -> Option<T::View<'a>> {
        self.len().checked_sub(1).and_then(|i| self.get(i))
    }

    /// The items, in order.
    #[must_use]
    pub fn iter(self) -> ItemsIter<'a, T> {
        ItemsIter {
            data: self.data,
            items: self.elems.iter(),
            front: self.start,
        }
    }

    /// The dataset the items are of.
    #[must_use]
    pub fn dataset(self) -> &'a Dataset {
        self.data
    }

    /// The items, each copied out as its owned type in turn.
    pub fn into_owned_iter(self) -> impl Iterator<Item = T> + 'a {
        self.elems.iter().enumerate().map(move |(i, e)| {
            let at = self
                .start
                .saturating_add(u32::try_from(i).unwrap_or(u32::MAX));
            T::load_in(e, at, self.data)
        })
    }

    /// The items, copied out as their owned type.
    #[must_use]
    pub fn to_vec(self) -> Vec<T> {
        self.elems
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let at = self
                    .start
                    .saturating_add(u32::try_from(i).unwrap_or(u32::MAX));
                T::load_in(e, at, self.data)
            })
            .collect()
    }
}

impl<'a, T: Listed> IntoIterator for Items<'a, T> {
    type Item = T::View<'a>;
    type IntoIter = ItemsIter<'a, T>;

    fn into_iter(self) -> ItemsIter<'a, T> {
        self.iter()
    }
}

impl<T: Listed> fmt::Debug for Items<'_, T>
where
    for<'b> T::View<'b>: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// The iterator of [`Items`].
pub struct ItemsIter<'a, T: Listed> {
    data: &'a Dataset,
    items: std::slice::Iter<'a, T::Elem>,
    /// The index of the next item in its table.
    front: u32,
}

impl<T: Listed> Clone for ItemsIter<'_, T> {
    fn clone(&self) -> Self {
        Self {
            data: self.data,
            items: self.items.clone(),
            front: self.front,
        }
    }
}

impl<'a, T: Listed> Iterator for ItemsIter<'a, T> {
    type Item = T::View<'a>;

    #[inline]
    fn next(&mut self) -> Option<T::View<'a>> {
        let e = self.items.next()?;
        let at = self.front;
        self.front = self.front.wrapping_add(1);
        Some(T::view_in(e, at, self.data))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.items.size_hint()
    }

    fn nth(&mut self, n: usize) -> Option<T::View<'a>> {
        let skip = n.min(self.items.len());
        if skip > 0 {
            self.items.nth(skip - 1);
            self.front = self
                .front
                .wrapping_add(u32::try_from(skip).unwrap_or(u32::MAX));
        }
        self.next()
    }
}

impl<T: Listed> DoubleEndedIterator for ItemsIter<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let e = self.items.next_back()?;
        let at = self
            .front
            .wrapping_add(u32::try_from(self.items.len()).unwrap_or(u32::MAX));
        Some(T::view_in(e, at, self.data))
    }
}

impl<T: Listed> ExactSizeIterator for ItemsIter<'_, T> {}

impl<T: Listed> std::iter::FusedIterator for ItemsIter<'_, T> {}

/// What storing an owned structure in a dataset needs: its tables and its
/// store (for owned texts).
pub(crate) struct StoreCx<'a> {
    pub(crate) tables: &'a mut Tables,
    pub(crate) store: &'a mut Store,
}

/// What copying rows needs: the tables and store they are in, the tables
/// and store they go to, and how texts, identifiers and tags move.
pub(crate) struct CopyCx<'a> {
    pub(crate) from: &'a Tables,
    pub(crate) to: &'a mut Tables,
    pub(crate) moved: Moved<'a>,
}

/// How texts, identifiers and tags move from one store to another (or
/// stay, within one).
pub(crate) enum Moved<'a> {
    /// Within one store: nothing changes.
    Same,
    /// Into another store.
    Into(super::relocate::Relocation<'a>),
}

impl CopyCx<'_> {
    #[inline]
    pub(crate) fn span(&mut self, s: Span) -> Span {
        match &mut self.moved {
            Moved::Same => s,
            Moved::Into(r) => r.span(s),
        }
    }

    #[inline]
    pub(crate) fn opt_span(&mut self, s: Option<Span>) -> Option<Span> {
        s.map(|s| self.span(s))
    }

    #[inline]
    pub(crate) fn xref(&mut self, id: XrefId) -> XrefId {
        match &mut self.moved {
            Moved::Same => id,
            Moved::Into(r) => r.xref(id),
        }
    }

    #[inline]
    pub(crate) fn tag(&mut self, id: TagId) -> TagId {
        match &mut self.moved {
            Moved::Same => id,
            Moved::Into(r) => r.tag(id),
        }
    }

    /// An enumeration value: an unknown one's text is copied.
    pub(crate) fn code(&mut self, c: Code) -> Code {
        match c.text_index() {
            Some(i) => {
                let span = self.from.spans.get(i).copied().unwrap_or_default();
                let span = self.span(span);
                push_span(&mut self.to.spans, span)
            }
            None => c,
        }
    }
}

/// An unknown enumeration value's text, pushed: its code.
pub(crate) fn push_span(spans: &mut Vec<Span>, span: Span) -> Code {
    let at = u32::try_from(spans.len()).unwrap_or(u32::MAX >> 1);
    spans.push(span);
    Code::unknown(at)
}

/// The rows of `n` items reserved at the end of `table`: where they start.
#[inline]
pub(crate) fn reserve<E: Copy>(table: &mut Vec<E>, n: usize, fill: E) -> u32 {
    let start = table.len();
    table.resize(start + n, fill);
    u32::try_from(start).unwrap_or(u32::MAX)
}

/// A value of a field of a structure: a structure kept as a row of its
/// table, or a leaf (a text, a pointer, an enumeration value) kept in its
/// parent's row. Everything the dataset does with a field goes through
/// it: reading, writing, the views, copying out and storing owned values,
/// pointers and copies between tables.
///
/// What a list of it holds ([`sealed::Listed::Elem`]: rows of its table,
/// or leaf values in a shared table) and its view are its
/// [`sealed::Listed`] side.
pub(crate) trait Item: sealed::Listed {
    /// What a field holding one occurrence keeps: the index of its row, or
    /// the leaf value.
    type One: Copy + PartialEq + std::fmt::Debug + 'static;

    /// The table its lists are runs of.
    fn elems(t: &Tables) -> &[Self::Elem];
    /// The same, to fill.
    fn elems_mut(t: &mut Tables) -> &mut Vec<Self::Elem>;
    /// A placeholder item, reserved before it is read.
    fn fill() -> Self::Elem;

    /// Reads one occurrence, which will be item `at` of its table; `None`
    /// when its shape does not fit.
    fn read_elem(
        node: super::driver::NodeRef<'_>,
        at: u32,
        cx: &mut super::driver::ReadCx<'_>,
    ) -> Option<Self::Elem>;
    /// Reads the occurrence of a field that holds one.
    fn read_one(
        node: super::driver::NodeRef<'_>,
        cx: &mut super::driver::ReadCx<'_>,
    ) -> Option<Self::One>;

    /// Writes item `at` as `tag`.
    fn write_elem<'s>(
        e: &Self::Elem,
        at: u32,
        tag: super::driver::StdTag,
        cx: &super::driver::WriteCx<'s>,
        out: &mut crate::tree::Flat<'s>,
    );
    /// Writes the occurrence of a field that holds one.
    fn write_one<'s>(
        o: &Self::One,
        tag: super::driver::StdTag,
        cx: &super::driver::WriteCx<'s>,
        out: &mut crate::tree::Flat<'s>,
    );

    /// Item `at`, as its owned type.
    fn load_elem(e: &Self::Elem, at: u32, t: &Tables) -> Self;
    /// The occurrence of a field that holds one, as its owned type.
    fn load_one(o: Self::One, t: &Tables) -> Self;
    /// The owned value stored as item `at` of its table (reserved).
    fn store_elem(&self, at: u32, cx: &mut StoreCx<'_>) -> Self::Elem;
    /// The owned value stored for a field that holds one.
    fn store_one(&self, cx: &mut StoreCx<'_>) -> Self::One;

    /// Calls `f` with every pointer of item `at`.
    fn each_elem(e: &Self::Elem, at: u32, t: &Tables, f: &mut dyn FnMut(XrefId));
    /// Calls `f` with every pointer of the occurrence.
    fn each_one(o: Self::One, t: &Tables, f: &mut dyn FnMut(XrefId));

    /// Item `at` of `cx.from`, copied as item `to` of `cx.to` (reserved).
    fn copy_elem(e: &Self::Elem, at: u32, cx: &mut CopyCx<'_>, to: u32) -> Self::Elem;
    /// The occurrence, copied.
    fn copy_one(o: Self::One, cx: &mut CopyCx<'_>) -> Self::One;

    /// The view of item `at`.
    fn view_elem<'a>(e: &'a Self::Elem, at: u32, data: &'a Dataset) -> Self::View<'a>;
    /// The view of the occurrence.
    fn view_one(o: Self::One, data: &Dataset) -> Self::View<'_>;

    /// Whether its items are texts ([`Item::text_of`]).
    const TEXT: bool = false;
    /// The text of item `e`, when the items are texts.
    fn text_of(e: &Self::Elem) -> Option<Span> {
        let _ = e;
        None
    }
    /// The text of occurrence `o`, when the items are texts.
    fn one_text(o: Self::One) -> Option<Span> {
        let _ = o;
        None
    }
}

/// A field of a structure, in a row: one occurrence or a list of them.
pub(crate) trait RowSlot: Sized {
    /// What the row keeps.
    type Stored: Copy + Default + 'static;
    /// What a view gives of it.
    type View<'a>;
    /// Reserves `count` items of a list before they are read.
    fn reserve(s: &mut Self::Stored, count: u32, cx: &mut super::driver::ReadCx<'_>);
    /// Takes an occurrence; `false` when the field is full or the node
    /// does not fit. `reserved`: the items reserved for a list.
    fn accept(
        s: &mut Self::Stored,
        node: super::driver::NodeRef<'_>,
        reserved: u32,
        cx: &mut super::driver::ReadCx<'_>,
    ) -> bool;
    /// Gives back the items reserved and not taken.
    fn finish(s: &mut Self::Stored, reserved: u32, cx: &mut super::driver::ReadCx<'_>);

    /// Writes the occurrences as `tag`.
    fn write<'s>(
        s: &Self::Stored,
        tag: super::driver::StdTag,
        cx: &super::driver::WriteCx<'s>,
        out: &mut crate::tree::Flat<'s>,
    );
    /// The field as its owned type.
    fn load(s: &Self::Stored, t: &Tables) -> Self;
    /// The owned field, stored.
    fn store(&self, cx: &mut StoreCx<'_>) -> Self::Stored;
    /// Calls `f` with every pointer.
    fn each(s: &Self::Stored, t: &Tables, f: &mut dyn FnMut(XrefId));
    /// The field, copied.
    fn copy(s: &Self::Stored, cx: &mut CopyCx<'_>) -> Self::Stored;
    /// The view of the field.
    fn view(s: Self::Stored, data: &Dataset) -> Self::View<'_>;
    /// When the field holds a list of texts: how many, `f` called with each.
    fn texts(s: &Self::Stored, t: &Tables, f: &mut dyn FnMut(Span)) -> Option<usize> {
        let _ = (s, t, f);
        None
    }
}

impl<T: Item + 'static> RowSlot for Option<T> {
    type Stored = Option<T::One>;
    type View<'a> = Option<T::View<'a>>;

    fn reserve(_: &mut Self::Stored, _: u32, _: &mut super::driver::ReadCx<'_>) {}

    #[inline]
    fn accept(
        s: &mut Self::Stored,
        node: super::driver::NodeRef<'_>,
        _: u32,
        cx: &mut super::driver::ReadCx<'_>,
    ) -> bool {
        if s.is_some() {
            return false;
        }
        *s = T::read_one(node, cx);
        s.is_some()
    }

    fn finish(_: &mut Self::Stored, _: u32, _: &mut super::driver::ReadCx<'_>) {}

    #[inline]
    fn write<'s>(
        s: &Self::Stored,
        tag: super::driver::StdTag,
        cx: &super::driver::WriteCx<'s>,
        out: &mut crate::tree::Flat<'s>,
    ) {
        if let Some(o) = s {
            T::write_one(o, tag, cx, out);
        }
    }

    fn load(s: &Self::Stored, t: &Tables) -> Self {
        s.map(|o| T::load_one(o, t))
    }

    fn store(&self, cx: &mut StoreCx<'_>) -> Self::Stored {
        self.as_ref().map(|v| v.store_one(cx))
    }

    fn each(s: &Self::Stored, t: &Tables, f: &mut dyn FnMut(XrefId)) {
        if let Some(o) = s {
            T::each_one(*o, t, f);
        }
    }

    fn copy(s: &Self::Stored, cx: &mut CopyCx<'_>) -> Self::Stored {
        s.map(|o| T::copy_one(o, cx))
    }

    #[inline]
    fn view(s: Self::Stored, data: &Dataset) -> Self::View<'_> {
        s.map(|o| T::view_one(o, data))
    }

    fn texts(s: &Self::Stored, _: &Tables, f: &mut dyn FnMut(Span)) -> Option<usize> {
        if !T::TEXT {
            return None;
        }
        let text = s.and_then(T::one_text);
        if let Some(text) = text {
            f(text);
        }
        Some(usize::from(text.is_some()))
    }
}

/// Reserves the items of a list before they are read ([`RowSlot::reserve`]
/// of a list): one copy for each type of item, whatever holds the list.
#[inline(never)]
fn reserve_list<T: Item>(count: u32, cx: &mut super::driver::ReadCx<'_>) -> List<T> {
    let start = reserve(T::elems_mut(cx.tables), count as usize, T::fill());
    List::new(start, 0)
}

/// Reads the next item of a list into its reserved row
/// ([`RowSlot::accept`] of a list); `false` when the list is full or the
/// node does not fit.
#[inline(never)]
fn accept_list<T: Item>(
    s: &mut List<T>,
    node: super::driver::NodeRef<'_>,
    reserved: u32,
    cx: &mut super::driver::ReadCx<'_>,
) -> bool {
    if s.len >= reserved {
        return false;
    }
    let at = s.start + s.len;
    let Some(e) = T::read_elem(node, at, cx) else {
        return false;
    };
    if let Some(slot) = T::elems_mut(cx.tables).get_mut(at as usize) {
        *slot = e;
    }
    s.len += 1;
    true
}

/// Gives back the rows of a list reserved and not taken
/// ([`RowSlot::finish`] of a list).
#[inline(never)]
fn finish_list<T: Item>(s: &mut List<T>, reserved: u32, cx: &mut super::driver::ReadCx<'_>) {
    if s.len < reserved {
        let table = T::elems_mut(cx.tables);
        let end = s.start as usize + reserved as usize;
        if table.len() == end {
            table.truncate(s.start as usize + s.len as usize);
        } else {
            cx.tables.garbage += (reserved - s.len) as usize;
        }
    }
    if s.len == 0 {
        *s = List::EMPTY;
    }
}

/// The list fields: `Vec` and `ThinVec`.
macro_rules! list_slot {
    ($($list:ident),*) => {$(
        impl<T: Item + 'static> RowSlot for $list<T> {
            type Stored = List<T>;
            type View<'a> = Items<'a, T>;

            #[inline]
            fn reserve(s: &mut Self::Stored, count: u32, cx: &mut super::driver::ReadCx<'_>) {
                *s = reserve_list::<T>(count, cx);
            }

            #[inline]
            fn accept(
                s: &mut Self::Stored,
                node: super::driver::NodeRef<'_>,
                reserved: u32,
                cx: &mut super::driver::ReadCx<'_>,
            ) -> bool {
                accept_list::<T>(s, node, reserved, cx)
            }

            #[inline]
            fn finish(s: &mut Self::Stored, reserved: u32, cx: &mut super::driver::ReadCx<'_>) {
                finish_list::<T>(s, reserved, cx);
            }

            #[inline]
            fn write<'s>(
                s: &Self::Stored,
                tag: super::driver::StdTag,
                cx: &super::driver::WriteCx<'s>,
                out: &mut crate::tree::Flat<'s>,
            ) {
                // Most lists are empty: their table is not looked up.
                if s.len == 0 {
                    return;
                }
                let items = s.of(T::elems(cx.tables));
                for (i, e) in items.iter().enumerate() {
                    out.repeat(i > 0);
                    let at = s.start + u32::try_from(i).unwrap_or(0);
                    T::write_elem(e, at, tag, cx, out);
                }
            }

            fn load(s: &Self::Stored, t: &Tables) -> Self {
                s.of(T::elems(t))
                    .iter()
                    .enumerate()
                    .map(|(i, e)| T::load_elem(e, s.start + u32::try_from(i).unwrap_or(0), t))
                    .collect()
            }

            fn store(&self, cx: &mut StoreCx<'_>) -> Self::Stored {
                if self.is_empty() {
                    return List::EMPTY;
                }
                let start = reserve(T::elems_mut(cx.tables), self.len(), T::fill());
                for (i, v) in self.iter().enumerate() {
                    let at = start + u32::try_from(i).unwrap_or(0);
                    let e = v.store_elem(at, cx);
                    if let Some(slot) = T::elems_mut(cx.tables).get_mut(at as usize) {
                        *slot = e;
                    }
                }
                List::new(start, u32::try_from(self.len()).unwrap_or(u32::MAX))
            }

            fn each(s: &Self::Stored, t: &Tables, f: &mut dyn FnMut(XrefId)) {
                for (i, e) in s.of(T::elems(t)).iter().enumerate() {
                    T::each_elem(e, s.start + u32::try_from(i).unwrap_or(0), t, f);
                }
            }

            fn copy(s: &Self::Stored, cx: &mut CopyCx<'_>) -> Self::Stored {
                copy_list::<T>(*s, cx)
            }

            #[inline]
            fn view(s: Self::Stored, data: &Dataset) -> Self::View<'_> {
                Items::new(data, s.of(T::elems(&data.tables)), s.start)
            }

            fn texts(s: &Self::Stored, t: &Tables, f: &mut dyn FnMut(Span)) -> Option<usize> {
                if !T::TEXT {
                    return None;
                }
                let items = s.of(T::elems(t));
                for e in items {
                    if let Some(text) = T::text_of(e) {
                        f(text);
                    }
                }
                Some(items.len())
            }

        }
    )*};
}

use super::list::ThinVec;
list_slot!(Vec, ThinVec);

/// A list of `cx.from`, copied into `cx.to`, contiguous.
pub(crate) fn copy_list<T: Item>(s: List<T>, cx: &mut CopyCx<'_>) -> List<T> {
    if s.is_empty() {
        return List::EMPTY;
    }
    let start = reserve(T::elems_mut(cx.to), s.len as usize, T::fill());
    for i in 0..s.len {
        let Some(&e) = T::elems(cx.from).get((s.start + i) as usize) else {
            continue;
        };
        let copied = T::copy_elem(&e, s.start + i, cx, start + i);
        if let Some(slot) = T::elems_mut(cx.to).get_mut((start + i) as usize) {
            *slot = copied;
        }
    }
    List::new(start, s.len)
}

listed!([T: Item] Box<T>, <T as sealed::Listed>::Elem, <T as sealed::Listed>::View<'a>);

/// A boxed value is stored as its value.
impl<T: Item> Item for Box<T> {
    type One = T::One;

    fn elems(t: &Tables) -> &[Self::Elem] {
        T::elems(t)
    }
    fn elems_mut(t: &mut Tables) -> &mut Vec<Self::Elem> {
        T::elems_mut(t)
    }
    fn fill() -> Self::Elem {
        T::fill()
    }
    fn read_elem(
        node: super::driver::NodeRef<'_>,
        at: u32,
        cx: &mut super::driver::ReadCx<'_>,
    ) -> Option<Self::Elem> {
        T::read_elem(node, at, cx)
    }
    fn read_one(
        node: super::driver::NodeRef<'_>,
        cx: &mut super::driver::ReadCx<'_>,
    ) -> Option<Self::One> {
        T::read_one(node, cx)
    }
    fn write_elem<'s>(
        e: &Self::Elem,
        at: u32,
        tag: super::driver::StdTag,
        cx: &super::driver::WriteCx<'s>,
        out: &mut crate::tree::Flat<'s>,
    ) {
        T::write_elem(e, at, tag, cx, out);
    }
    fn write_one<'s>(
        o: &Self::One,
        tag: super::driver::StdTag,
        cx: &super::driver::WriteCx<'s>,
        out: &mut crate::tree::Flat<'s>,
    ) {
        T::write_one(o, tag, cx, out);
    }
    fn load_elem(e: &Self::Elem, at: u32, t: &Tables) -> Self {
        Box::new(T::load_elem(e, at, t))
    }
    fn load_one(o: Self::One, t: &Tables) -> Self {
        Box::new(T::load_one(o, t))
    }
    fn store_elem(&self, at: u32, cx: &mut StoreCx<'_>) -> Self::Elem {
        (**self).store_elem(at, cx)
    }
    fn store_one(&self, cx: &mut StoreCx<'_>) -> Self::One {
        (**self).store_one(cx)
    }
    fn each_elem(e: &Self::Elem, at: u32, t: &Tables, f: &mut dyn FnMut(XrefId)) {
        T::each_elem(e, at, t, f);
    }
    fn each_one(o: Self::One, t: &Tables, f: &mut dyn FnMut(XrefId)) {
        T::each_one(o, t, f);
    }
    fn copy_elem(e: &Self::Elem, at: u32, cx: &mut CopyCx<'_>, to: u32) -> Self::Elem {
        T::copy_elem(e, at, cx, to)
    }
    fn copy_one(o: Self::One, cx: &mut CopyCx<'_>) -> Self::One {
        T::copy_one(o, cx)
    }
    fn view_elem<'a>(e: &'a Self::Elem, at: u32, data: &'a Dataset) -> Self::View<'a> {
        T::view_elem(e, at, data)
    }
    fn view_one(o: Self::One, data: &Dataset) -> Self::View<'_> {
        T::view_one(o, data)
    }
}

/// Pushes a row onto the table of `T`: its index.
pub(crate) fn push_row<T: Item<One = Idx<T>>>(t: &mut Tables, row: T::Elem) -> Idx<T> {
    let table = T::elems_mut(t);
    let at = u32::try_from(table.len()).unwrap_or(u32::MAX - 1);
    table.push(row);
    Idx::new(at).unwrap_or(Idx(NonZeroU32::MAX, PhantomData))
}

/// Reads the occurrence of a structure a field holds once: its row,
/// reserved before it is read so that what it holds comes after it. One
/// copy for each type, whatever holds it.
#[inline(never)]
pub(crate) fn read_one_row<T: Item<One = Idx<T>>>(
    node: super::driver::NodeRef<'_>,
    cx: &mut super::driver::ReadCx<'_>,
) -> Option<Idx<T>> {
    let at = reserve(T::elems_mut(cx.tables), 1, T::fill());
    let Some(row) = T::read_elem(node, at, cx) else {
        let table = T::elems_mut(cx.tables);
        if table.len() == at as usize + 1 {
            table.pop();
        } else {
            cx.tables.garbage += 1;
        }
        return None;
    };
    if let Some(slot) = T::elems_mut(cx.tables).get_mut(at as usize) {
        *slot = row;
    }
    Idx::new(at)
}

/// Stores an owned structure a field holds once: its row's index.
pub(crate) fn store_one_row<T: Item<One = Idx<T>>>(value: &T, cx: &mut StoreCx<'_>) -> Idx<T> {
    let at = reserve(T::elems_mut(cx.tables), 1, T::fill());
    let row = value.store_elem(at, cx);
    if let Some(slot) = T::elems_mut(cx.tables).get_mut(at as usize) {
        *slot = row;
    }
    Idx::new(at).unwrap_or(Idx(NonZeroU32::MAX, PhantomData))
}

/// Copies the row of a structure a field holds once.
pub(crate) fn copy_one_row<T: Item<One = Idx<T>>>(o: Idx<T>, cx: &mut CopyCx<'_>) -> Idx<T> {
    let Some(&row) = T::elems(cx.from).get(o.get() as usize) else {
        return o;
    };
    let to = reserve(T::elems_mut(cx.to), 1, T::fill());
    let copied = T::copy_elem(&row, o.get(), cx, to);
    if let Some(slot) = T::elems_mut(cx.to).get_mut(to as usize) {
        *slot = copied;
    }
    Idx::new(to).unwrap_or(o)
}

/// The view of the structure a field holds once.
#[inline]
pub(crate) fn view_one_row<T>(o: Idx<T>, data: &Dataset) -> View<'_, T>
where
    T: Item<One = Idx<T>> + Stored,
    T: sealed::Stored<Row = <T as sealed::Listed>::Elem>,
{
    match T::elems(&data.tables).get(o.get() as usize) {
        Some(row) => View::new(data, row, o.get()),
        // Every index of a dataset is within its table.
        None => View::new(data, T::empty(), u32::MAX),
    }
}

// The layout of what a dataset holds, checked at compile time: a row per
// structure, the frequent ones small (`tests/sizes.rs` checks the owned
// types). A text or a pointer is 8 bytes, an index or an enumeration
// value 4, a list 8.
const _: () = {
    use super::citation::CitationRow;
    use super::date::{ChangeDateRow, DateRow, ExactDateRow};
    use super::event::EventRow;
    use super::link::{ChildLinkRow, IndividualRefRow, SpouseLinkRow};
    use super::name::NameRow;
    use super::note::NoteRow;
    use super::place::PlaceRow;
    use super::record::{FamilyRow, IndividualRow, SourceRow};
    use std::mem::size_of;
    assert!(size_of::<Span>() == 8);
    assert!(size_of::<Option<Span>>() == 8);
    assert!(size_of::<Idx<Event>>() == 4);
    assert!(size_of::<Option<Idx<Event>>>() == 4);
    assert!(size_of::<Option<Code>>() == 4);
    assert!(size_of::<List<Event>>() == 8);
    assert!(size_of::<NodeRow>() == 24);
    assert!(size_of::<View<'_, Event>>() == 24);
    assert!(size_of::<IndividualRow>() <= 72);
    assert!(size_of::<FamilyRow>() <= 60);
    assert!(size_of::<SourceRow>() <= 128);
    assert!(size_of::<EventRow>() <= 32);
    assert!(size_of::<NameRow>() <= 20);
    assert!(size_of::<DateRow>() <= 12);
    assert!(size_of::<PlaceRow>() <= 12);
    assert!(size_of::<CitationRow>() <= 20);
    assert!(size_of::<NoteRow>() <= 12);
    assert!(size_of::<SpouseLinkRow>() <= 12);
    assert!(size_of::<ChildLinkRow>() <= 8);
    assert!(size_of::<IndividualRefRow>() <= 8);
    assert!(size_of::<ChangeDateRow>() <= 12);
    assert!(size_of::<ExactDateRow>() <= 16);
};

#[cfg(test)]
mod tests {
    use super::super::Dataset;

    /// A dataset starts from the tables a dropped one leaves, emptied.
    #[test]
    fn a_dataset_reuses_the_tables_of_a_dropped_one_emptied() {
        let first = Dataset::parse(
            "0 HEAD\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 BIRT\n2 DATE 1900\n1 _X y\n\
             0 @S1@ SOUR\n1 DATA\n2 EVEN BIRT\n0 TRLR\n",
        );
        assert!(first.tables.rows() > 5);
        drop(first);
        let second = Dataset::parse("0 @F1@ FAM\n0 TRLR\n");
        assert_eq!(second.individuals().len(), 0);
        assert_eq!(second.sources().len(), 0);
        assert_eq!(second.families().len(), 1);
        assert_eq!((second.tables.rows(), second.tables.garbage), (1, 0));
        drop(second);
        assert_eq!(Dataset::default().tables.rows(), 0);
    }
}
