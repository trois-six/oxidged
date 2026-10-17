//! A dataset: the header and the records of a file, typed, in the tables
//! of [`super::table`], and the store their texts point into.

use std::fmt;

use crate::profile::{Detection, Interpretation};
use crate::tree::{Flat, Node as _, PayloadRef, Structure, Tag};
use crate::version::GedcomVersion;

use super::driver::{StdTag, ToNodes, WriteCx};
use super::header::Header;
use super::node::{Node, Value};
use super::record::{
    Family, Individual, Multimedia, Repository, SharedNote, Source, Submission, Submitter,
};
use super::table::{HasTable, Item, Items, TableBox, View};
use super::text::{Store, XrefId};

/// A GEDCOM dataset: its header and records, typed, and the store their
/// texts and identifiers resolve against.
///
/// Records are kept by type, each type in file order
/// ([`Dataset::individuals`], [`Dataset::families`], …); records of no type
/// the model has (extension and unknown records, a second header, a
/// trailer with content) are kept in [`Dataset::extra`], in order. The
/// trailer itself carries nothing and is written by the writer.
///
/// Every structure of a record is a row of the table of its type (see
/// [`crate::model`], Tables and views): the dataset allocates nothing per
/// record, and reads it where it is, through views ([`View`], [`Items`]). The owned types of
/// the model are what a program builds and edits: [`Dataset::add`] stores
/// one, [`View::into_owned`] and [`Dataset::get_mut`] give one back.
///
/// A dataset keeps its references whole as it is edited: the records are
/// read through it and changed through it ([`Dataset::add`],
/// [`Dataset::remove`], [`Dataset::rename`], [`Dataset::link`],
/// [`Dataset::get_mut`], …), which knows the record of each identifier and
/// counts the pointers to it ([`Dataset::references`]). Lookups by
/// identifier take constant time. See [`crate::model`] (Editing).
///
/// ```rust
/// use oxidged::model::{Dataset, RecordRef};
///
/// let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n0 @F1@ FAM\n1 CHIL @I1@\n0 _LOC Place\n0 TRLR\n");
/// assert_eq!(data.individuals().len(), 1);
/// assert_eq!(data.families().first().unwrap().children().len(), 1);
/// assert_eq!(data.extra().len(), 1); // the extension record
/// assert!(matches!(data.find("@F1@"), Some(RecordRef::Family(_))));
/// assert_eq!(data.records().count(), 4); // the header, two records, the extension
/// ```
#[derive(Clone, Default)]
pub struct Dataset {
    /// The header (`HEAD`), when the file has one.
    pub(crate) header: Option<Box<Header>>,
    /// Every structure but the header, in its table: the records in the
    /// tables of their types, in order.
    pub(crate) tables: TableBox,
    pub(crate) store: Store,
    pub(crate) version: GedcomVersion,
    pub(crate) declared_version: Option<Box<str>>,
    /// Handles, identifiers and pointer counts (see [`super::edit`]).
    pub(crate) index: super::edit::Index,
    /// The producer reading found and how far it interpreted its
    /// conventions ([`crate::profile`]); what is left to interpret only in
    /// 7.x is interpreted when the dataset is written in 7.x.
    pub(crate) producer: Option<Box<(Detection, Interpretation)>>,
    /// The `HEAD.SOUR` system identifier the dataset was read with: an
    /// export tells it from one the program set
    /// ([`crate::profile::HeadSource`]).
    pub(crate) read_source: Option<Box<str>>,
}

impl AsRef<Store> for Dataset {
    fn as_ref(&self) -> &Store {
        &self.store
    }
}

/// Two datasets are equal when they declare the same version and hold the
/// same structures (whatever their stores: texts compare by their
/// characters, identifiers by their text).
impl PartialEq for Dataset {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            && self.declared_version == other.declared_version
            && self.record_count() == other.record_count()
            && self.to_structures() == other.to_structures()
    }
}

impl fmt::Debug for Dataset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dataset")
            .field("version", &self.version)
            .field("header", &self.header.is_some())
            .field("individuals", &self.tables.individual.len())
            .field("families", &self.tables.family.len())
            .field("sources", &self.tables.source.len())
            .field("repositories", &self.tables.repository.len())
            .field("multimedia", &self.tables.multimedia.len())
            .field("submitters", &self.tables.submitter.len())
            .field("submissions", &self.submissions().len())
            .field("notes", &self.tables.shared_note.len())
            .field("extra", &self.tables.others.len())
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

/// A record of any type, owned: what [`Dataset::push`] takes and what a
/// record of a dataset copies out as ([`RecordRef::into_owned`]). Its
/// variants are boxed, so that it is two words.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    /// The header.
    Header(Box<Header>),
    /// An individual.
    Individual(Box<Individual>),
    /// A family.
    Family(Box<Family>),
    /// A source.
    Source(Box<Source>),
    /// A repository.
    Repository(Box<Repository>),
    /// A multimedia object.
    Multimedia(Box<Multimedia>),
    /// A submitter.
    Submitter(Box<Submitter>),
    /// A submission.
    Submission(Box<Submission>),
    /// A shared note.
    Note(Box<SharedNote>),
    /// A record of no type the model has, or one that did not fit its type.
    Other(Box<Node>),
}

impl Record {
    /// The record as an owned structure, written by its type's
    /// [`ToNodes`].
    pub(crate) fn to_node(&self, cx: &WriteCx<'_>) -> Structure {
        match self {
            Record::Header(r) => r.to_node("HEAD", cx),
            Record::Individual(r) => r.to_node("INDI", cx),
            Record::Family(r) => r.to_node("FAM", cx),
            Record::Source(r) => r.to_node("SOUR", cx),
            Record::Repository(r) => r.to_node("REPO", cx),
            Record::Multimedia(r) => r.to_node("OBJE", cx),
            Record::Submitter(r) => r.to_node("SUBM", cx),
            Record::Submission(r) => r.to_node("SUBN", cx),
            Record::Note(r) => r.to_node("SNOTE", cx),
            Record::Other(n) => n.to_structure(cx.store),
        }
    }
}

/// The header of a dataset, with the dataset: it dereferences to the
/// [`Header`].
#[derive(Clone, Copy)]
pub struct HeaderRef<'a> {
    data: &'a Dataset,
    header: &'a Header,
}

impl<'a> HeaderRef<'a> {
    /// The header.
    #[must_use]
    pub fn header(self) -> &'a Header {
        self.header
    }

    /// The dataset the header is of.
    #[must_use]
    pub fn dataset(self) -> &'a Dataset {
        self.data
    }

    /// The store its texts and identifiers resolve against.
    #[must_use]
    pub fn store(self) -> &'a Store {
        &self.data.store
    }
}

impl std::ops::Deref for HeaderRef<'_> {
    type Target = Header;

    fn deref(&self) -> &Header {
        self.header
    }
}

impl AsRef<Store> for HeaderRef<'_> {
    fn as_ref(&self) -> &Store {
        &self.data.store
    }
}

impl fmt::Debug for HeaderRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.header.fmt(f)
    }
}

impl PartialEq for HeaderRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.header, other.header)
    }
}

/// A record of a [`Dataset`], whatever its type, read where it is.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecordRef<'a> {
    /// The header.
    Header(HeaderRef<'a>),
    /// An individual.
    Individual(View<'a, Individual>),
    /// A family.
    Family(View<'a, Family>),
    /// A source.
    Source(View<'a, Source>),
    /// A repository.
    Repository(View<'a, Repository>),
    /// A multimedia object.
    Multimedia(View<'a, Multimedia>),
    /// A submitter.
    Submitter(View<'a, Submitter>),
    /// A submission.
    Submission(View<'a, Submission>),
    /// A shared note.
    Note(View<'a, SharedNote>),
    /// A record of no type the model has.
    Other(View<'a, Node>),
}

/// Runs `$body` with `$r` the view of a typed record, `$h` the header and
/// `$n` an untyped record.
macro_rules! each_kind {
    ($record:expr, |$r:ident| $typed:expr, |$h:ident| $header:expr, |$n:ident| $other:expr) => {
        match $record {
            RecordRef::Header($h) => $header,
            RecordRef::Individual($r) => $typed,
            RecordRef::Family($r) => $typed,
            RecordRef::Source($r) => $typed,
            RecordRef::Repository($r) => $typed,
            RecordRef::Multimedia($r) => $typed,
            RecordRef::Submitter($r) => $typed,
            RecordRef::Submission($r) => $typed,
            RecordRef::Note($r) => $typed,
            RecordRef::Other($n) => $other,
        }
    };
}

impl<'a> RecordRef<'a> {
    /// The dataset the record is of.
    #[must_use]
    pub fn dataset(self) -> &'a Dataset {
        each_kind!(self, |r| r.dataset(), |h| h.dataset(), |n| n.dataset())
    }

    /// The store the record's texts and identifiers resolve against.
    #[must_use]
    pub fn store(self) -> &'a Store {
        &self.dataset().store
    }

    /// The record's identifier.
    #[must_use]
    pub fn xref(self) -> Option<XrefId> {
        match self {
            RecordRef::Header(_) => None,
            RecordRef::Individual(r) => r.xref(),
            RecordRef::Family(r) => r.xref(),
            RecordRef::Source(r) => r.xref(),
            RecordRef::Repository(r) => r.xref(),
            RecordRef::Multimedia(r) => r.xref(),
            RecordRef::Submitter(r) => r.xref(),
            RecordRef::Submission(r) => r.xref(),
            RecordRef::Note(r) => r.xref(),
            RecordRef::Other(n) => n.xref(),
        }
    }

    /// The record's tag: the version's tag of its type (5.5.1 `NOTE`, 7.x
    /// `SNOTE` for a shared note), or an untyped record's own.
    #[must_use]
    pub fn tag(self, version: GedcomVersion) -> &'a str {
        match self {
            RecordRef::Other(n) => n.tag(),
            RecordRef::Note(_) if version.is_v7() => "SNOTE",
            RecordRef::Note(_) => "NOTE",
            r => r.std_tag(),
        }
    }

    /// The index among the standard tags of the record's tag
    /// ([`RecordRef::tag`]).
    pub(crate) fn standard_tag(self, version: GedcomVersion) -> Option<u16> {
        use crate::tree::standard_index_const as index;
        match self {
            RecordRef::Other(n) => u16::try_from(n.tag_id().get())
                .ok()
                .filter(|&t| usize::from(t) < crate::tree::STANDARD_TAGS.len()),
            RecordRef::Note(_) if version.is_v7() => const { index("SNOTE") },
            RecordRef::Note(_) => const { index("NOTE") },
            RecordRef::Header(_) => const { index("HEAD") },
            RecordRef::Individual(_) => const { index("INDI") },
            RecordRef::Family(_) => const { index("FAM") },
            RecordRef::Source(_) => const { index("SOUR") },
            RecordRef::Repository(_) => const { index("REPO") },
            RecordRef::Multimedia(_) => const { index("OBJE") },
            RecordRef::Submitter(_) => const { index("SUBM") },
            RecordRef::Submission(_) => const { index("SUBN") },
        }
    }

    /// The tag of a typed record (`SNOTE` for a shared note).
    fn std_tag(self) -> &'static str {
        match self {
            RecordRef::Header(_) => "HEAD",
            RecordRef::Individual(_) => "INDI",
            RecordRef::Family(_) => "FAM",
            RecordRef::Source(_) => "SOUR",
            RecordRef::Repository(_) => "REPO",
            RecordRef::Multimedia(_) => "OBJE",
            RecordRef::Submitter(_) => "SUBM",
            RecordRef::Submission(_) => "SUBN",
            RecordRef::Note(_) | RecordRef::Other(_) => "SNOTE",
        }
    }

    /// Whether the record has neither payload nor substructure. Asked of
    /// trailers only, which are never typed: a typed record answers no.
    pub(crate) fn is_empty(self) -> bool {
        match self {
            RecordRef::Other(n) => {
                let payload = match n.payload() {
                    Value::None => true,
                    Value::Pointer(_) => false,
                    Value::Text(t) => t.is_empty(),
                };
                payload && n.children().is_empty()
            }
            _ => false,
        }
    }

    /// The record, into a flat arena, its texts borrowed, for `cx.version`.
    pub(crate) fn to_flat(self, cx: &WriteCx<'a>, out: &mut Flat<'a>) {
        macro_rules! row {
            ($t:ty, $r:expr, $tag:literal) => {
                <$t as Item>::write_elem($r.row, $r.at, const { StdTag::new($tag) }, cx, out)
            };
        }
        match self {
            RecordRef::Header(h) => h.header.to_flat(const { StdTag::new("HEAD") }, cx, out),
            RecordRef::Individual(r) => row!(Individual, r, "INDI"),
            RecordRef::Family(r) => row!(Family, r, "FAM"),
            RecordRef::Source(r) => row!(Source, r, "SOUR"),
            RecordRef::Repository(r) => row!(Repository, r, "REPO"),
            RecordRef::Multimedia(r) => row!(Multimedia, r, "OBJE"),
            RecordRef::Submitter(r) => row!(Submitter, r, "SUBM"),
            RecordRef::Submission(r) => row!(Submission, r, "SUBN"),
            RecordRef::Note(r) => row!(SharedNote, r, "SNOTE"),
            RecordRef::Other(n) => super::node::node_row_flat(n.row, cx, out),
        }
    }

    /// The record as an owned structure, its values written for
    /// `cx.version` as `cx` says: copied out as its owned type, which
    /// writes itself so.
    pub(crate) fn to_structure(self, cx: &WriteCx<'_>) -> Structure {
        let cx = WriteCx::owned(self.store(), cx.version, cx.convert);
        match self {
            // The header is owned already.
            RecordRef::Header(h) => h.header.to_node("HEAD", &cx),
            r => r.into_owned().to_node(&cx),
        }
    }

    /// The record as an owned structure, as the dataset holds it: values
    /// as written, enumeration values in `version`'s spelling.
    #[must_use]
    pub fn to_structure_in(self, version: GedcomVersion) -> Structure {
        let data = self.dataset();
        self.to_structure(&WriteCx::of(data, version, false))
    }

    /// The record, copied out as its owned type.
    #[must_use]
    pub fn into_owned(self) -> Record {
        match self {
            RecordRef::Header(h) => Record::Header(Box::new(h.header.clone())),
            RecordRef::Individual(r) => Record::Individual(Box::new(r.into_owned())),
            RecordRef::Family(r) => Record::Family(Box::new(r.into_owned())),
            RecordRef::Source(r) => Record::Source(Box::new(r.into_owned())),
            RecordRef::Repository(r) => Record::Repository(Box::new(r.into_owned())),
            RecordRef::Multimedia(r) => Record::Multimedia(Box::new(r.into_owned())),
            RecordRef::Submitter(r) => Record::Submitter(Box::new(r.into_owned())),
            RecordRef::Submission(r) => Record::Submission(Box::new(r.into_owned())),
            RecordRef::Note(r) => Record::Note(Box::new(r.into_owned())),
            RecordRef::Other(n) => Record::Other(Box::new(n.into_owned())),
        }
    }

    /// Calls `f` with every pointer of the record.
    pub(crate) fn each_pointer(self, f: &mut dyn FnMut(XrefId)) {
        macro_rules! row {
            ($t:ty, $r:expr) => {
                <$t as Item>::each_elem($r.row, $r.at, $r.tables(), f)
            };
        }
        match self {
            RecordRef::Header(h) => super::pointers::Pointers::each(h.header, f),
            RecordRef::Individual(r) => row!(Individual, r),
            RecordRef::Family(r) => row!(Family, r),
            RecordRef::Source(r) => row!(Source, r),
            RecordRef::Repository(r) => row!(Repository, r),
            RecordRef::Multimedia(r) => row!(Multimedia, r),
            RecordRef::Submitter(r) => row!(Submitter, r),
            RecordRef::Submission(r) => row!(Submission, r),
            RecordRef::Note(r) => row!(SharedNote, r),
            RecordRef::Other(n) => super::node::each_node(n.row, n.tables(), f),
        }
    }
}

/// An identifier to look a record up by: its text (`"@I1@"`) or its
/// [`XrefId`].
pub trait XrefKey: sealed::Sealed {
    /// The id of the identifier in `store`, if it has one.
    fn id_in(self, store: &Store) -> Option<XrefId>;
}

pub(crate) mod sealed {
    use super::{Store, XrefId};

    /// The identifier's text, for errors.
    pub trait Sealed {
        fn text_in(&self, store: &Store) -> Box<str>;
    }
    impl Sealed for &str {
        fn text_in(&self, _store: &Store) -> Box<str> {
            (*self).into()
        }
    }
    impl Sealed for String {
        fn text_in(&self, _store: &Store) -> Box<str> {
            self.as_str().into()
        }
    }
    impl Sealed for &String {
        fn text_in(&self, _store: &Store) -> Box<str> {
            self.as_str().into()
        }
    }
    impl Sealed for XrefId {
        fn text_in(&self, store: &Store) -> Box<str> {
            store.xref(*self).into()
        }
    }
    impl Sealed for Option<XrefId> {
        fn text_in(&self, store: &Store) -> Box<str> {
            self.map_or_else(Box::default, |id| store.xref(id).into())
        }
    }
}

impl XrefKey for &str {
    fn id_in(self, store: &Store) -> Option<XrefId> {
        store.find_xref(self)
    }
}

impl XrefKey for String {
    fn id_in(self, store: &Store) -> Option<XrefId> {
        store.find_xref(&self)
    }
}

impl XrefKey for &String {
    fn id_in(self, store: &Store) -> Option<XrefId> {
        store.find_xref(self)
    }
}

impl XrefKey for XrefId {
    fn id_in(self, _store: &Store) -> Option<XrefId> {
        Some(self)
    }
}

impl XrefKey for Option<XrefId> {
    fn id_in(self, _store: &Store) -> Option<XrefId> {
        self
    }
}

/// A pointer of a dataset to no record of it: see
/// [`Dataset::dangling_references`].
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DanglingReference {
    /// The identifier of the record that holds the pointer; `None` for the
    /// header and records without one.
    pub record: Option<XrefId>,
    /// The tag of the structure whose payload the pointer is.
    pub tag: Tag,
    /// The pointer.
    pub pointer: XrefId,
}

impl Dataset {
    /// An empty dataset of `version`, to fill.
    #[must_use]
    pub fn new(version: GedcomVersion) -> Self {
        Self {
            version,
            ..Self::default()
        }
    }

    /// The producer of the file the dataset was read from, when
    /// [`GedcomBuilder`](crate::GedcomBuilder) detected (or was given) one
    /// and interpreted its conventions ([`crate::profile`]).
    #[must_use]
    pub fn producer(&self) -> Option<&Detection> {
        self.producer.as_deref().map(|(d, _)| d)
    }

    /// How far reading interpreted the producer's conventions
    /// ([`Interpretation::None`] for a dataset read without a profile).
    #[must_use]
    pub fn interpretation(&self) -> Interpretation {
        self.producer
            .as_deref()
            .map_or(Interpretation::None, |&(_, level)| level)
    }

    /// Notes the `HEAD.SOUR` system identifier the dataset was just read
    /// with ([`Dataset::read_source`]).
    pub(crate) fn note_read_source(&mut self) {
        let source = self.header.as_deref().and_then(|h| h.source.as_ref());
        // A program's export through this crate names the program: its
        // `HEAD.SOUR` is the program's (`profile::HeadSource`).
        let exported = source.is_some_and(|s| {
            s.extra
                .iter()
                .any(|n| self.store.tag(n.tag) == crate::profile::export::MARKER)
        });
        self.read_source = source
            .filter(|_| !exported)
            .map(|s| s.product.to_str(&self.store).into());
    }

    /// Records the producer and the interpretation level, so that what
    /// only 7.x can say is interpreted when the dataset is converted.
    pub(crate) fn set_producer(&mut self, detection: Detection, level: Interpretation) {
        self.producer = (level != Interpretation::None).then(|| Box::new((detection, level)));
    }

    /// Reads a dataset from decoded text, leniently: never fails, keeps
    /// everything. The dataset keeps the text as its store (a `String` is
    /// not copied). [`GedcomBuilder`](crate::GedcomBuilder) reads with
    /// limits or in strict mode.
    #[must_use]
    pub fn parse(text: impl Into<String>) -> Self {
        super::read::read_string(text.into())
    }

    /// [`Dataset::parse`] of text the exporter itself wrote and conformed:
    /// a structure the writer repaired (`_CROP` for `CROP`, `_INIL` for
    /// `INIL`) is read as the extension it was written as, not as the
    /// standard structure it stands for, so that the writer, given the
    /// dataset, does not repair it a second time and report it again.
    pub(crate) fn parse_as_written(text: impl Into<String>) -> Self {
        super::read::read_as_written(text.into())
    }

    /// Reads a dataset from bytes of any encoding (see
    /// [`crate::encoding::decode`]), leniently. Given a `Vec<u8>` of UTF-8,
    /// the dataset keeps that buffer as its text, without a copy.
    #[must_use]
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self::parse(crate::encoding::decode_owned(bytes.into()).text)
    }

    /// The version the file declares (`HEAD.GEDC.VERS`): 7.0 or 7.1 for
    /// 7.x, 5.5.1 otherwise; for a dataset made with [`Dataset::new`], its
    /// version. The writer writes it unless told otherwise.
    #[must_use]
    pub fn version(&self) -> GedcomVersion {
        self.version
    }

    /// The `HEAD.GEDC.VERS` payload as read.
    #[must_use]
    pub fn declared_version(&self) -> Option<&str> {
        self.declared_version.as_deref()
    }

    /// The store the dataset's texts and identifiers resolve against.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The store, to intern the identifiers and tags of new structures.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// Bytes the dataset holds: its store, its tables and its index.
    #[must_use]
    pub fn heap_size(&self) -> usize {
        self.store.heap_size() + self.tables.heap_size() + self.index.heap_size()
    }

    /// The tables' rows by table: name, rows, bytes a row (for the
    /// memory budget's record).
    #[doc(hidden)]
    #[must_use]
    pub fn table_rows(&self) -> Vec<(&'static str, usize, usize)> {
        self.tables.row_counts()
    }

    /// Releases the spare capacity of the tables.
    pub(crate) fn shrink(&mut self) {
        self.tables.shrink();
    }

    // ---- Reading the records ------------------------------------------

    /// The header (`HEAD`), when the file has one.
    #[must_use]
    pub fn header(&self) -> Option<&Header> {
        self.header.as_deref()
    }

    /// The header, with the dataset ([`HeaderRef`]).
    #[must_use]
    pub fn header_ref(&self) -> Option<HeaderRef<'_>> {
        self.header
            .as_deref()
            .map(|header| HeaderRef { data: self, header })
    }

    /// The records of a typed record table, as views.
    fn rows<
        T: HasTable
            + Item
            + super::table::sealed::Listed<Elem = <T as super::table::sealed::Stored>::Row>,
    >(
        &self,
    ) -> Items<'_, T> {
        Items::new(self, &T::table(&self.tables).rows, 0)
    }

    /// The individuals (`INDI`), in order.
    #[must_use]
    pub fn individuals(&self) -> Items<'_, Individual> {
        self.rows()
    }

    /// The families (`FAM`), in order.
    #[must_use]
    pub fn families(&self) -> Items<'_, Family> {
        self.rows()
    }

    /// The sources (`SOUR`), in order.
    #[must_use]
    pub fn sources(&self) -> Items<'_, Source> {
        self.rows()
    }

    /// The repositories (`REPO`), in order.
    #[must_use]
    pub fn repositories(&self) -> Items<'_, Repository> {
        self.rows()
    }

    /// The multimedia objects (`OBJE`), in order.
    #[must_use]
    pub fn multimedia(&self) -> Items<'_, Multimedia> {
        self.rows()
    }

    /// The submitters (`SUBM`), in order.
    #[must_use]
    pub fn submitters(&self) -> Items<'_, Submitter> {
        self.rows()
    }

    /// The submissions (5.5.1 `SUBN`), in order.
    #[must_use]
    pub fn submissions(&self) -> Items<'_, Submission> {
        self.rows()
    }

    /// The shared notes (5.5.1 `NOTE`, 7.x `SNOTE`), in order.
    #[must_use]
    pub fn notes(&self) -> Items<'_, SharedNote> {
        self.rows()
    }

    /// The records of no type the model has, in order.
    #[must_use]
    pub fn extra(&self) -> Items<'_, Node> {
        Items::new(self, &self.tables.others.rows, 0)
    }

    /// Every record, the header first, then by type (submitters,
    /// submissions, individuals, families, notes, sources, repositories,
    /// multimedia objects), then the others, in order.
    pub fn records(&self) -> impl Iterator<Item = RecordRef<'_>> {
        self.header_ref()
            .into_iter()
            .map(RecordRef::Header)
            .chain(self.submitters().into_iter().map(RecordRef::Submitter))
            .chain(self.submissions().into_iter().map(RecordRef::Submission))
            .chain(self.individuals().into_iter().map(RecordRef::Individual))
            .chain(self.families().into_iter().map(RecordRef::Family))
            .chain(self.notes().into_iter().map(RecordRef::Note))
            .chain(self.sources().into_iter().map(RecordRef::Source))
            .chain(self.repositories().into_iter().map(RecordRef::Repository))
            .chain(self.multimedia().into_iter().map(RecordRef::Multimedia))
            .chain(self.extra().into_iter().map(RecordRef::Other))
    }

    /// The number of records, the header included.
    #[must_use]
    pub fn record_count(&self) -> usize {
        let t = &self.tables;
        usize::from(self.header.is_some())
            + t.individual.len()
            + t.family.len()
            + t.source.len()
            + t.repository.len()
            + t.multimedia.len()
            + t.submitter.len()
            + Submission::table(t).len()
            + t.shared_note.len()
            + t.others.len()
    }

    /// The record with this identifier: the first one, when several have
    /// it. Constant time.
    #[must_use]
    pub fn find(&self, xref: impl XrefKey) -> Option<RecordRef<'_>> {
        self.find_record(xref.id_in(&self.store)?)
    }

    /// The record of `T` with identifier `xref` (the first).
    fn find_of<T>(&self, xref: impl XrefKey) -> Option<View<'_, T>>
    where
        T: super::RecordType
            + HasTable
            + Item
            + super::table::sealed::Listed<Elem = <T as super::table::sealed::Stored>::Row>,
    {
        let pos = self.first_of(T::KIND, xref.id_in(&self.store)?)?;
        let row = T::table(&self.tables).rows.get(pos)?;
        Some(View::new(self, row, u32::try_from(pos).ok()?))
    }

    /// The individual with this identifier (the first, when several have
    /// it). Constant time, as every `find_*`.
    #[must_use]
    pub fn find_individual(&self, xref: impl XrefKey) -> Option<View<'_, Individual>> {
        self.find_of(xref)
    }

    /// The family with this identifier.
    #[must_use]
    pub fn find_family(&self, xref: impl XrefKey) -> Option<View<'_, Family>> {
        self.find_of(xref)
    }

    /// The source with this identifier.
    #[must_use]
    pub fn find_source(&self, xref: impl XrefKey) -> Option<View<'_, Source>> {
        self.find_of(xref)
    }

    /// The repository with this identifier.
    #[must_use]
    pub fn find_repository(&self, xref: impl XrefKey) -> Option<View<'_, Repository>> {
        self.find_of(xref)
    }

    /// The multimedia object with this identifier.
    #[must_use]
    pub fn find_multimedia(&self, xref: impl XrefKey) -> Option<View<'_, Multimedia>> {
        self.find_of(xref)
    }

    /// The submitter with this identifier.
    #[must_use]
    pub fn find_submitter(&self, xref: impl XrefKey) -> Option<View<'_, Submitter>> {
        self.find_of(xref)
    }

    /// The shared note with this identifier.
    #[must_use]
    pub fn find_note(&self, xref: impl XrefKey) -> Option<View<'_, SharedNote>> {
        self.find_of(xref)
    }

    /// The families in which this individual is a partner (`HUSB` or
    /// `WIFE`).
    pub fn families_as_spouse(
        &self,
        individual: impl XrefKey,
    ) -> impl Iterator<Item = View<'_, Family>> {
        let id = individual.id_in(&self.store);
        self.families()
            .into_iter()
            .filter(move |f| id.is_some() && (f.husband_id() == id || f.wife_id() == id))
    }

    /// The families in which this individual is a child (`CHIL`).
    pub fn families_as_child(
        &self,
        individual: impl XrefKey,
    ) -> impl Iterator<Item = View<'_, Family>> {
        let id = individual.id_in(&self.store);
        self.families()
            .into_iter()
            .filter(move |f| id.is_some() && f.children().into_iter().any(|c| c.individual() == id))
    }

    /// The partners of a family (`HUSB`, then `WIFE`) that have a record.
    pub fn parents<'s>(
        &'s self,
        family: View<'s, Family>,
    ) -> impl Iterator<Item = View<'s, Individual>> + 's {
        [family.husband_id(), family.wife_id()]
            .into_iter()
            .flatten()
            .filter_map(move |id| self.find_individual(id))
    }

    /// The children of a family (`CHIL`) that have a record, in order.
    pub fn children<'s>(
        &'s self,
        family: View<'s, Family>,
    ) -> impl Iterator<Item = View<'s, Individual>> + 's {
        family
            .children()
            .into_iter()
            .filter_map(View::<crate::model::IndividualRef>::individual)
            .filter_map(move |id| self.find_individual(id))
    }

    /// The other partner of `individual` in `family`, when it has a record.
    #[must_use]
    pub fn spouse(
        &self,
        individual: impl XrefKey,
        family: View<'_, Family>,
    ) -> Option<View<'_, Individual>> {
        let id = individual.id_in(&self.store)?;
        let other = if family.husband_id() == Some(id) {
            family.wife_id()
        } else if family.wife_id() == Some(id) {
            family.husband_id()
        } else {
            None
        };
        other.and_then(|o| self.find_individual(o))
    }

    /// The individuals one of whose names contains `query`, ignoring case
    /// (the surname slashes of a name are ignored: `ann example` finds
    /// `Ann /Example/`).
    pub fn search_individuals<'s>(
        &'s self,
        query: &'s str,
    ) -> impl Iterator<Item = View<'s, Individual>> + 's {
        let query = query.to_lowercase();
        self.individuals().into_iter().filter(move |i| {
            i.names()
                .into_iter()
                .any(|n| n.full().to_lowercase().contains(&query))
        })
    }

    /// Every pointer of the dataset to no record of it, in record order:
    /// pointers of every kind (families, individuals, sources,
    /// repositories, multimedia objects, notes, submitters, extensions'),
    /// but 7.x `@VOID@`, which points to nothing by definition, and the
    /// 5.5.1 pointers into a record (`@I1!2@`, `@!2@`) or to another
    /// dataset (`:`). A query, not a check: reading never fails on a
    /// dangling pointer.
    ///
    /// ```rust
    /// use oxidged::model::Dataset;
    ///
    /// let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 FAMC @F9@\n1 SOUR @VOID@\n0 TRLR\n");
    /// let dangling = data.dangling_references();
    /// assert_eq!(dangling.len(), 1);
    /// assert_eq!(data.store().xref(dangling[0].pointer), "@F9@");
    /// assert_eq!(dangling[0].tag, "FAMC");
    /// ```
    #[must_use]
    pub fn dangling_references(&self) -> Vec<DanglingReference> {
        let mut defined = vec![false; self.store.xref_count()];
        for id in self.records().filter_map(RecordRef::xref) {
            if let Some(d) = defined.get_mut(id.index()) {
                *d = true;
            }
        }
        let family = crate::spec::payload::Family::of(self.version.rules());
        let cx = WriteCx::of(self, self.version, false);
        let mut out = Vec::new();
        let mut flat = Flat::default();
        for record in self.records() {
            flat.clear();
            record.to_flat(&cx, &mut flat);
            let Some(root) = flat.root() else { continue };
            for (_, n) in root.preorder() {
                let PayloadRef::Pointer(p) = n.payload() else {
                    continue;
                };
                if (self.version.is_v7() && p == crate::tree::Xref::VOID)
                    || crate::spec::is_external_pointer(p, family)
                {
                    continue;
                }
                let Some(id) = self.store.find_xref(p) else {
                    continue;
                };
                if !defined.get(id.index()).copied().unwrap_or(false) {
                    out.push(DanglingReference {
                        record: record.xref(),
                        tag: Tag::new(n.tag()),
                        pointer: id,
                    });
                }
            }
        }
        out
    }

    /// The records as owned structures, then the trailer, as the model
    /// holds them (known enumeration values in the dataset's version's
    /// spelling).
    #[must_use]
    pub fn to_structures(&self) -> Vec<Structure> {
        self.structures(self.version, false)
    }

    /// The records as owned structures, then the trailer, for writing
    /// `version`: dates, ages and times in its grammars, enumeration values
    /// in its spelling.
    #[must_use]
    pub fn to_structures_for(&self, version: GedcomVersion) -> Vec<Structure> {
        self.structures(version, true)
    }

    fn structures(&self, version: GedcomVersion, convert: bool) -> Vec<Structure> {
        let cx = WriteCx::of(self, version, convert);
        // The trailer, which the dataset does not keep, ends them.
        let mut out = Vec::with_capacity(self.record_count() + 1);
        out.extend(self.records().map(|r| r.to_structure(&cx)));
        out.push(Structure::new("TRLR"));
        out
    }
}
