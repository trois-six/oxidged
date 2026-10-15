//! Editing a dataset with its references kept whole: handles to records,
//! the index of identifiers, the count of pointers to each, and the
//! operations that keep them true.
//!
//! # Design
//!
//! Identifiers and pointers are interned [`XrefId`]s, so the integrity of
//! a dataset is a matter of tables keyed by id, not of the pointers
//! themselves:
//!
//! - the **definitions**: for each id, the record that has it (the first
//!   in [`Dataset::records`] order, with the others kept aside when several
//!   have it), and how many record identifiers each version's grammar
//!   refuses. Built as a dataset is read (one look at each record's
//!   identifier), they make every lookup by identifier and the writer's
//!   identifier check constant-time;
//! - the **counts**: for each id, how many pointers of the dataset hold it.
//!   They are built the first time they are asked for (one walk of every
//!   pointer: reading and writing never pay for them), then kept by every
//!   edit;
//! - the **handles**: a slot per record, which follows the record when the
//!   records before it are removed, and refuses to give a removed record's
//!   successor (a generation).
//!
//! Records are reached for writing only through the dataset, which lends a
//! copy of a record as its owned type ([`Dataset::get_mut`]) and stores it
//! back when the loan ends ([`RecordMut`]'s drop), counting the pointers of
//! the record replaced out and the new ones in: any field may be edited,
//! pointer fields and extension structures included, and the counts stay
//! exact, at a cost of the record's size per loan. A loan that is never
//! returned (a guard forgotten with `mem::forget`) changes nothing.
//!
//! A record stored back takes new rows in the dataset's tables; its old
//! rows hold nothing any more. When as many rows hold nothing as hold the
//! records, the dataset copies its records into new tables: the cost of
//! edits stays proportional to what they change.
//!
//! A pointer per `Arc` (each pointer a counted reference to its target's
//! name) would count by construction too, but at 16 bytes and an atomic
//! operation per pointer, a heap allocation per identifier, pointers that
//! another dataset made counted in the wrong one, and fields still public
//! for a program to fill with pointers no dataset counted. Here a pointer
//! stays a 4-byte id, a dataset holds a few bytes per record and per
//! identifier, and renaming an identifier changes one entry of the store:
//! every pointer follows.

use std::fmt;
use std::marker::PhantomData;
use std::sync::OnceLock;

use super::table::{
    CopyCx, HasTable, Item, Items, List, Listed, Moved, StoreCx, Stored, Table, Tables, View,
};

use super::citation::{Citation, CitationSource, RepositoryCitation};
use super::dataset::{Dataset, Record, RecordRef, XrefKey};
use super::header::Header;
use super::link::{Association, ChildLink, IndividualRef, SpouseLink};
use super::list::ThinVec;
use super::multimedia::MultimediaLink;
use super::node::{Node, Value};
use super::note::Note;
use super::pointers::{Cut, Pointers};
use super::record::{
    Family, Individual, Multimedia, Repository, SharedNote, Source, Submission, Submitter,
};
use super::text::{Store, TagId, XrefId};

/// The type of a record, in the order [`Dataset::records`] gives them.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RecordKind {
    /// A submitter (`SUBM`).
    Submitter,
    /// A submission (5.5.1 `SUBN`).
    Submission,
    /// An individual (`INDI`).
    Individual,
    /// A family (`FAM`).
    Family,
    /// A shared note (5.5.1 `NOTE`, 7.x `SNOTE`).
    Note,
    /// A source (`SOUR`).
    Source,
    /// A repository (`REPO`).
    Repository,
    /// A multimedia object (`OBJE`).
    Multimedia,
    /// A record of no type the model has (an extension record).
    Other,
}

const KINDS: [RecordKind; 9] = [
    RecordKind::Submitter,
    RecordKind::Submission,
    RecordKind::Individual,
    RecordKind::Family,
    RecordKind::Note,
    RecordKind::Source,
    RecordKind::Repository,
    RecordKind::Multimedia,
    RecordKind::Other,
];

/// The kind bits of a free slot.
const FREE: u32 = 0xF;

impl RecordKind {
    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    fn of(bits: u32) -> Option<Self> {
        KINDS.get(bits as usize).copied()
    }

    /// The prefix of the identifiers [`Dataset::add`] gives records of
    /// this kind (`@I1@`, `@F1@`, …), as the writer does.
    fn prefix(self) -> Option<&'static str> {
        Some(match self {
            RecordKind::Individual => "I",
            RecordKind::Family => "F",
            RecordKind::Source => "S",
            RecordKind::Repository => "R",
            RecordKind::Multimedia => "M",
            RecordKind::Submitter => "U",
            RecordKind::Submission => "SUBN",
            RecordKind::Note => "N",
            RecordKind::Other => return None,
        })
    }

    /// The record's type in words, for errors.
    fn name(self) -> &'static str {
        match self {
            RecordKind::Submitter => "a submitter",
            RecordKind::Submission => "a submission",
            RecordKind::Individual => "an individual",
            RecordKind::Family => "a family",
            RecordKind::Note => "a shared note",
            RecordKind::Source => "a source",
            RecordKind::Repository => "a repository",
            RecordKind::Multimedia => "a multimedia object",
            RecordKind::Other => "an extension record",
        }
    }
}

/// A record of a dataset, whatever its type: see [`Handle`].
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordHandle {
    slot: u32,
    /// The slot's generation (high bits) and the record's kind (4 bits).
    tag: u32,
}

impl RecordHandle {
    /// The type of the record.
    #[must_use]
    pub fn kind(self) -> RecordKind {
        RecordKind::of(self.tag & FREE).unwrap_or(RecordKind::Other)
    }

    /// The handle with its type, when the record is a `T`.
    #[must_use]
    pub fn typed<T: RecordType>(self) -> Option<Handle<T>> {
        (self.kind() == T::KIND).then_some(Handle {
            raw: self,
            record: PhantomData,
        })
    }
}

impl fmt::Debug for RecordHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}#{}v{}", self.kind(), self.slot, self.tag >> 4)
    }
}

/// A record of a dataset, of type `T`: what [`Dataset::add`] returns and
/// [`Dataset::get`], [`Dataset::get_mut`] and [`Dataset::remove`] take.
///
/// A handle stays valid across every edit of the dataset (records added
/// or removed before it, identifiers renamed, links made) until its record
/// is removed; after that, no operation finds a record by it, even when a
/// new record takes its place. A handle is of the dataset that made it and
/// of its clones.
pub struct Handle<T> {
    raw: RecordHandle,
    record: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    /// The handle without its type.
    #[must_use]
    pub fn erase(self) -> RecordHandle {
        self.raw
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Handle<T> {}

impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T> Eq for Handle<T> {}

impl<T> std::hash::Hash for Handle<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.raw.fmt(f)
    }
}

impl<T> From<Handle<T>> for RecordHandle {
    fn from(h: Handle<T>) -> Self {
        h.raw
    }
}

/// What holds a pointer: the header or a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Holder {
    /// The header.
    Header,
    /// A record.
    Record(RecordHandle),
}

impl From<RecordHandle> for Holder {
    fn from(h: RecordHandle) -> Self {
        Holder::Record(h)
    }
}

impl<T> From<Handle<T>> for Holder {
    fn from(h: Handle<T>) -> Self {
        Holder::Record(h.raw)
    }
}

/// What [`Dataset::remove`] does when other structures point to the
/// record.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Removal {
    /// Refuse to remove it ([`EditError::StillReferenced`]).
    #[default]
    Refuse,
    /// Remove every structure that points to it, with its substructures
    /// (a family's `CHIL`, an individual's `FAMC`, a citation, an
    /// extension structure; an untyped record whose payload points to it
    /// loses its payload), as [`Dataset::detach`] does.
    Detach,
    /// Remove it and leave the pointers to it dangling, as a file may
    /// have them ([`Dataset::dangling_references`] lists them).
    LeaveDangling,
}

/// A kind of pointer from a record (or the header) to another record:
/// what [`Dataset::link`] adds and [`Dataset::unlink`] removes.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Link {
    /// An individual's family as a child (`FAMC`), to a family.
    ChildOf,
    /// An individual's family as a spouse (`FAMS`), to a family.
    SpouseOf,
    /// A family's husband or first partner (`HUSB`), to an individual.
    Husband,
    /// A family's wife or second partner (`WIFE`), to an individual.
    Wife,
    /// A family's child (`CHIL`), to an individual.
    Child,
    /// An individual's alias (`ALIA`), to an individual.
    Alias,
    /// An association (`ASSO`) of an individual or a family, to an
    /// individual.
    Association,
    /// A citation (`SOUR`) of an individual, a family, a multimedia
    /// object or a shared note, to a source.
    Citation,
    /// A note (`NOTE @N1@`, 7.x `SNOTE`) of the header or a typed record,
    /// to a shared note.
    Note,
    /// A multimedia link (`OBJE`) of an individual, a family, a source or
    /// a submitter, to a multimedia object.
    Multimedia,
    /// A source's repository (`REPO`), to a repository.
    Repository,
    /// The submitter (`SUBM`) of the header, an individual, a family or a
    /// submission, to a submitter.
    Submitter,
    /// An individual's submitter interested in their ancestors (`ANCI`).
    AncestorInterest,
    /// An individual's submitter interested in their descendants
    /// (`DESI`).
    DescendantInterest,
    /// The header's submission (5.5.1 `HEAD.SUBN`), to a submission.
    Submission,
    /// An extension structure (`_TAG @X1@`) of any record or the header,
    /// to a record of any type: the node is added to its `extra`.
    Extension(TagId),
}

impl Link {
    /// The type of record the link points to; `None`: any.
    fn target(self) -> Option<RecordKind> {
        Some(match self {
            Link::ChildOf | Link::SpouseOf => RecordKind::Family,
            Link::Husband | Link::Wife | Link::Child | Link::Alias | Link::Association => {
                RecordKind::Individual
            }
            Link::Citation => RecordKind::Source,
            Link::Note => RecordKind::Note,
            Link::Multimedia => RecordKind::Multimedia,
            Link::Repository => RecordKind::Repository,
            Link::Submitter | Link::AncestorInterest | Link::DescendantInterest => {
                RecordKind::Submitter
            }
            Link::Submission => RecordKind::Submission,
            Link::Extension(_) => return None,
        })
    }
}

/// Why an edit was refused. The dataset is as it was.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    /// The handle's record was removed.
    StaleHandle,
    /// Another record has this identifier.
    DuplicateXref(Box<str>),
    /// No record has this identifier.
    UnknownXref(Box<str>),
    /// An identifier the dataset's store does not have: an id of another
    /// dataset, or of none.
    ForeignXref,
    /// Not an identifier: `@`, a character, `@`, without `@`, spaces or
    /// control characters inside.
    InvalidXref(Box<str>),
    /// Pointers of the dataset point to this identifier, which names no
    /// record: renaming a record to it would take them.
    XrefInUse(Box<str>),
    /// Other structures point to the record ([`Removal::Refuse`]).
    StillReferenced {
        /// The record's identifier.
        xref: Box<str>,
        /// The pointers to it from other records and the header.
        references: usize,
    },
    /// The record is not of the type the link points to.
    WrongType {
        /// The identifier.
        xref: Box<str>,
        /// The type the link points to.
        expected: RecordKind,
        /// The type of the record that has the identifier.
        found: RecordKind,
    },
    /// The holder has no structure of this kind (a family has no `FAMC`).
    NotApplicable(Link),
    /// The holder already points there with a structure of this kind.
    AlreadyLinked(Link),
    /// The holder has no structure of this kind pointing there.
    NotLinked(Link),
    /// The holder's only structure of this kind points elsewhere (a
    /// family's `HUSB`; both partners for [`Dataset::link_spouse`]).
    Occupied(Link),
    /// The dataset has no header.
    NoHeader,
    /// The record has no identifier to point to.
    NoXref,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::StaleHandle => f.write_str("the record was removed"),
            EditError::DuplicateXref(x) => write!(f, "{x}: another record has this identifier"),
            EditError::UnknownXref(x) => write!(f, "{x}: no record has this identifier"),
            EditError::ForeignXref => f.write_str("an identifier of another dataset"),
            EditError::InvalidXref(x) => write!(f, "{x:?}: not an identifier"),
            EditError::XrefInUse(x) => write!(f, "{x}: pointers name this identifier"),
            EditError::StillReferenced { xref, references } => {
                write!(f, "{xref}: {references} pointers to the record")
            }
            EditError::WrongType {
                xref,
                expected,
                found,
            } => write!(f, "{xref}: {}, not {}", found.name(), expected.name()),
            EditError::NotApplicable(l) => write!(f, "no {l:?} link from this record"),
            EditError::AlreadyLinked(l) => write!(f, "already a {l:?} link to this record"),
            EditError::NotLinked(l) => write!(f, "no {l:?} link to this record"),
            EditError::Occupied(l) => write!(f, "the {l:?} link points to another record"),
            EditError::NoHeader => f.write_str("the dataset has no header"),
            EditError::NoXref => f.write_str("the record has no identifier"),
        }
    }
}

impl std::error::Error for EditError {}

pub(crate) mod sealed {
    use super::super::dataset::{Dataset, RecordRef};
    use super::super::text::XrefId;

    /// What the dataset needs of a record type: where its records are,
    /// and their pointers. Not nameable outside the crate, so that only the
    /// dataset reaches its tables.
    pub trait Record: Sized + super::super::table::sealed::Listed {
        /// The number of records of the type.
        fn count(data: &Dataset) -> usize;
        /// The rows of the records of the type.
        fn records_in(data: &Dataset) -> &[Self::Elem];
        /// The record at `pos`.
        fn record_ref(data: &Dataset, pos: usize) -> Option<RecordRef<'_>>;
        /// The identifier of the record at `pos`.
        fn xref_at(data: &Dataset, pos: usize) -> Option<XrefId>;
        /// The record at `pos`, copied out.
        fn load_at(data: &Dataset, pos: usize) -> Option<Self>;
        /// Stores the record at `pos` (replacing the one there), or after
        /// the last one: its position.
        fn store_at(&self, data: &mut Dataset, pos: Option<usize>) -> usize;
        /// Removes the record at `pos`: those after it move back by one.
        fn remove_at(data: &mut Dataset, pos: usize);
        fn xref_of(&self) -> Option<XrefId>;
        fn set_xref(&mut self, xref: Option<XrefId>);
        fn pointers(&self, f: &mut dyn FnMut(XrefId));
    }

    /// What can be lent by [`RecordMut`](super::RecordMut): a record or
    /// the header, copied out and stored back.
    pub trait Edit: Sized + Send + Sync + 'static {
        /// The record at `pos` (the header: any), copied out.
        fn take(data: &Dataset, pos: usize) -> Option<Self>;
        /// Stores the record back at `pos`, its pointers counted again.
        fn put(self, data: &mut Dataset, pos: usize);
    }
}

/// A type of record: [`Individual`], [`Family`], [`Source`],
/// [`Repository`], [`Multimedia`], [`Submitter`], [`Submission`],
/// [`SharedNote`], or [`Node`] for the records of no type the model has.
pub trait RecordType:
    sealed::Record
    + Editable
    + Stored
    + for<'a> Listed<View<'a> = View<'a, Self>, Elem = <Self as super::table::sealed::Stored>::Row>
    + 'static
{
    /// The kind of record.
    const KIND: RecordKind;
}

/// What a [`RecordMut`] lends: a record ([`RecordType`]) or the
/// [`Header`].
pub trait Editable: sealed::Edit {}

/// The rows of a type of record: its table of records and the identifier
/// of a row. The crate's side of [`RecordType`].
pub(crate) trait RecordRows: Item + Stored {
    /// The kind of record.
    const ROWS_KIND: RecordKind;
    fn records(t: &Tables) -> &Table<Self::Elem>;
    fn records_mut(t: &mut Tables) -> &mut Table<Self::Elem>;
    fn row_xref(row: &Self::Elem) -> Option<XrefId>;
}

macro_rules! record_type {
    ($($ty:ty => $kind:ident, |$t:ident| $table:expr, |$m:ident| $table_mut:expr;)*) => {$(
        impl RecordRows for $ty {
            const ROWS_KIND: RecordKind = RecordKind::$kind;
            #[inline]
            fn records($t: &Tables) -> &Table<Self::Elem> {
                $table
            }
            #[inline]
            fn records_mut($m: &mut Tables) -> &mut Table<Self::Elem> {
                $table_mut
            }
            #[inline]
            fn row_xref(row: &Self::Elem) -> Option<XrefId> {
                row.xref
            }
        }

        impl sealed::Record for $ty {
            fn count(data: &Dataset) -> usize {
                <$ty as RecordRows>::records(&data.tables).len()
            }
            fn records_in(data: &Dataset) -> &[Self::Elem] {
                &<$ty as RecordRows>::records(&data.tables).rows
            }
            fn record_ref(data: &Dataset, pos: usize) -> Option<RecordRef<'_>> {
                data.record_at(RecordKind::$kind, pos)
            }
            fn xref_at(data: &Dataset, pos: usize) -> Option<XrefId> {
                <$ty as RecordRows>::records(&data.tables)
                    .rows
                    .get(pos)
                    .and_then(|r| r.xref)
            }
            fn load_at(data: &Dataset, pos: usize) -> Option<Self> {
                let row = <$ty as RecordRows>::records(&data.tables).rows.get(pos)?;
                Some(<$ty as Item>::load_elem(row, u32::try_from(pos).ok()?, &data.tables))
            }
            fn store_at(&self, data: &mut Dataset, pos: Option<usize>) -> usize {
                data.store_record::<$ty>(self, pos)
            }
            fn remove_at(data: &mut Dataset, pos: usize) {
                if let Ok(at) = u32::try_from(pos) {
                    <$ty as RecordRows>::records_mut(&mut data.tables).remove(at);
                }
            }
            fn xref_of(&self) -> Option<XrefId> {
                self.xref
            }
            fn set_xref(&mut self, xref: Option<XrefId>) {
                self.xref = xref;
            }
            fn pointers(&self, f: &mut dyn FnMut(XrefId)) {
                Pointers::each(self, f);
            }
        }

        impl sealed::Edit for $ty {
            fn take(data: &Dataset, pos: usize) -> Option<Self> {
                <$ty as sealed::Record>::load_at(data, pos)
            }
            fn put(self, data: &mut Dataset, pos: usize) {
                data.replace_record::<$ty>(&self, pos);
            }
        }

        impl Editable for $ty {}

        impl RecordType for $ty {
            const KIND: RecordKind = RecordKind::$kind;
        }
    )*};
}

record_type! {
    Submitter => Submitter, |t| Submitter::table(t), |t| Submitter::table_mut(t);
    Submission => Submission, |t| Submission::table(t), |t| Submission::table_mut(t);
    Individual => Individual, |t| Individual::table(t), |t| Individual::table_mut(t);
    Family => Family, |t| Family::table(t), |t| Family::table_mut(t);
    SharedNote => Note, |t| SharedNote::table(t), |t| SharedNote::table_mut(t);
    Source => Source, |t| Source::table(t), |t| Source::table_mut(t);
    Repository => Repository, |t| Repository::table(t), |t| Repository::table_mut(t);
    Multimedia => Multimedia, |t| Multimedia::table(t), |t| Multimedia::table_mut(t);
    Node => Other, |t| &t.others, |t| &mut t.others;
}

impl sealed::Edit for Header {
    fn take(data: &Dataset, _pos: usize) -> Option<Self> {
        data.header.as_deref().cloned()
    }
    fn put(self, data: &mut Dataset, _pos: usize) {
        data.set_header(Some(self));
    }
}

impl Editable for Header {}

/// Runs `$body` with `$t` the type of record of `$kind`.
macro_rules! with_kind {
    ($kind:expr, |$t:ident| $body:expr) => {
        match $kind {
            RecordKind::Submitter => {
                type $t = Submitter;
                $body
            }
            RecordKind::Submission => {
                type $t = Submission;
                $body
            }
            RecordKind::Individual => {
                type $t = Individual;
                $body
            }
            RecordKind::Family => {
                type $t = Family;
                $body
            }
            RecordKind::Note => {
                type $t = SharedNote;
                $body
            }
            RecordKind::Source => {
                type $t = Source;
                $body
            }
            RecordKind::Repository => {
                type $t = Repository;
                $body
            }
            RecordKind::Multimedia => {
                type $t = Multimedia;
                $body
            }
            RecordKind::Other => {
                type $t = Node;
                $body
            }
        }
    };
}

/// A handle's slot: the record's position in its list, and the slot's
/// generation and the record's kind (`FREE` when the slot is free).
#[derive(Clone, Copy, Debug)]
struct Slot {
    pos: u32,
    tag: u32,
}

/// A record whose pointers and identifier are counted: its kind and
/// position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lent {
    Record(RecordKind, usize),
}

/// The bits of a [`Place`] that hold the position.
const POS_BITS: u32 = 28;
/// Where a [`Place`]'s kind bits start.
pub(crate) const PLACE_KIND_SHIFT: u32 = POS_BITS;
/// A [`Place`]'s position part for a record too far in its list to say
/// where: it is searched for.
const FAR: u32 = (1 << POS_BITS) - 1;

/// Where a record is: its kind (the high 4 bits) and its position plus one
/// (the low 28 bits, [`FAR`] past them); 0 for none.
type Place = u32;

fn place(kind: RecordKind, pos: usize) -> Place {
    let pos = u32::try_from(pos)
        .ok()
        .and_then(|p| p.checked_add(1))
        .filter(|&p| p < FAR)
        .unwrap_or(FAR);
    (u32::try_from(kind.index()).unwrap_or(0) << POS_BITS) | pos
}

fn place_kind(place: Place) -> Option<RecordKind> {
    (place != 0)
        .then(|| RecordKind::of(place >> POS_BITS))
        .flatten()
}

/// The position of a place; `None` for none, or a record too far.
fn place_pos(place: Place) -> Option<usize> {
    let pos = place & FAR;
    (place != 0 && pos != FAR).then(|| pos as usize - 1)
}

/// The handles of a dataset's records: built the first time a handle is
/// asked for, then kept by every edit.
#[derive(Clone, Debug, Default)]
struct Handles {
    /// By handle slot.
    slots: Vec<Slot>,
    /// Free slots, to reuse.
    free: Vec<u32>,
    /// By kind, the slot of each record, in order.
    order: [Vec<u32>; 9],
    /// By kind, the number the next identifier [`Dataset::add`] makes
    /// starts from.
    fresh: [u32; 9],
}

impl Handles {
    /// The handles of records whose lists have `lens` records, by kind.
    fn new(lens: [usize; 9]) -> Self {
        let mut handles = Self {
            slots: Vec::with_capacity(lens.iter().sum()),
            ..Self::default()
        };
        for kind in KINDS {
            let n = lens.get(kind.index()).copied().unwrap_or(0);
            if let Some(order) = handles.order.get_mut(kind.index()) {
                order.reserve_exact(n);
            }
            for pos in 0..n {
                handles.alloc(kind, pos);
            }
        }
        handles
    }

    fn slot(&self, slot: u32) -> Option<Slot> {
        self.slots.get(slot as usize).copied()
    }

    /// The kind and position of a live handle's record.
    fn locate(&self, h: RecordHandle) -> Option<(RecordKind, usize)> {
        let s = self.slot(h.slot).filter(|s| s.tag == h.tag)?;
        Some((RecordKind::of(s.tag & FREE)?, s.pos as usize))
    }

    fn handle(&self, slot: u32) -> Option<RecordHandle> {
        let s = self.slot(slot)?;
        (s.tag & FREE != FREE).then_some(RecordHandle { slot, tag: s.tag })
    }

    /// The handle of the record of `kind` at `pos`.
    fn at(&self, kind: RecordKind, pos: usize) -> Option<RecordHandle> {
        self.handle(*self.order.get(kind.index())?.get(pos)?)
    }

    /// A slot for a record of `kind` at `pos`, the last of its list.
    fn alloc(&mut self, kind: RecordKind, pos: usize) -> RecordHandle {
        let pos = u32::try_from(pos).unwrap_or(u32::MAX);
        let kind_bits = u32::try_from(kind.index()).unwrap_or(FREE);
        let slot = if let Some(slot) = self.free.pop() {
            if let Some(s) = self.slots.get_mut(slot as usize) {
                *s = Slot {
                    pos,
                    tag: (s.tag & !FREE) | kind_bits,
                };
            }
            slot
        } else {
            let slot = u32::try_from(self.slots.len()).unwrap_or(u32::MAX);
            self.slots.push(Slot {
                pos,
                tag: kind_bits,
            });
            slot
        };
        if let Some(order) = self.order.get_mut(kind.index()) {
            order.push(slot);
        }
        let tag = self.slot(slot).map_or(0, |s| s.tag);
        RecordHandle { slot, tag }
    }

    /// Frees the slot of the record of `kind` at `pos`, moving the slots of
    /// the records after it back by one.
    fn release(&mut self, kind: RecordKind, pos: usize) {
        let Some(order) = self.order.get_mut(kind.index()) else {
            return;
        };
        if pos >= order.len() {
            return;
        }
        let slot = order.remove(pos);
        for &later in order.iter().skip(pos) {
            if let Some(s) = self.slots.get_mut(later as usize) {
                s.pos -= 1;
            }
        }
        if let Some(s) = self.slots.get_mut(slot as usize) {
            // The next generation, free.
            s.tag = (s.tag & !FREE).wrapping_add(1 << 4) | FREE;
        }
        self.free.push(slot);
    }
}

/// The tables of a dataset's integrity: see the [module
/// documentation](self). Derived from the records: two datasets with the
/// same records are equal whatever their tables.
#[derive(Clone, Default)]
pub(crate) struct Index {
    /// By identifier id: the place of the first record (in
    /// [`Dataset::records`] order) that has it; 0 when none has.
    defs: Vec<Place>,
    /// The other records that have an identifier a record has before
    /// them: (identifier, place).
    more: Vec<(XrefId, Place)>,
    /// The records whose identifier is invalid in 5.5.1, in 7.x: counted
    /// the first time the writer asks, from the definitions.
    invalid: OnceLock<[u32; 2]>,
    /// By identifier id, the pointers that hold it; built when first asked
    /// for.
    refs: OnceLock<Vec<u32>>,
    /// The handles; built when first asked for.
    handles: OnceLock<Box<Handles>>,
}

impl PartialEq for Index {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl fmt::Debug for Index {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Index")
            .field("duplicates", &self.more.len())
            .field("handles", &self.handles.get().is_some())
            .field("counted", &self.refs.get().is_some())
            .finish_non_exhaustive()
    }
}

impl Index {
    /// Notes, as a reader places it, that the record at `pos` of `kind` has
    /// identifier `id` (the store not yet at hand: [`Dataset::index_read`]
    /// completes the tables).
    #[inline]
    pub(crate) fn define_read(&mut self, kind: RecordKind, pos: usize, id: Option<XrefId>) {
        if let Some(id) = id {
            self.place_def(id, place(kind, pos));
        }
    }

    /// Whether a record placed so far has identifier `id`.
    #[inline]
    pub(crate) fn is_defined(&self, id: XrefId) -> bool {
        self.def(id) != 0
    }

    /// Bytes held.
    pub(crate) fn heap_size(&self) -> usize {
        self.defs.capacity() * 4
            + self.more.capacity() * 8
            + self.refs.get().map_or(0, |r| r.capacity() * 4)
            + self.handles.get().map_or(0, |h| {
                h.slots.capacity() * 8
                    + h.free.capacity() * 4
                    + h.order.iter().map(|o| o.capacity() * 4).sum::<usize>()
            })
    }

    fn def(&self, id: XrefId) -> Place {
        self.defs.get(id.index()).copied().unwrap_or(0)
    }

    /// Records that the record at `at` has identifier `id`. An id the store
    /// does not have is no identifier: left out.
    fn define(&mut self, id: XrefId, at: Place, store: &Store) {
        if !store.has_xref(id) {
            return;
        }
        if let Some(invalid) = self.invalid.get_mut() {
            let (v551, v7) = store.xref_validity(id);
            invalid[0] += u32::from(!v551);
            invalid[1] += u32::from(!v7);
        }
        self.place_def(id, at);
    }

    /// Records where the record that has `id` is, as [`Index::define`]
    /// does, its validity left to be counted.
    #[inline]
    fn place_def(&mut self, id: XrefId, at: Place) {
        let i = id.index();
        if self.defs.len() <= i {
            // Ids come mostly in order: room for the next ones at once.
            let len = (i + 1).max(self.defs.len() * 2).max(64);
            self.defs.resize(len, 0);
        }
        let Some(first) = self.defs.get_mut(i) else {
            return;
        };
        if *first == 0 {
            *first = at;
        } else if at < *first {
            // Places compare in record order (kind, then position).
            self.more.push((id, *first));
            *first = at;
        } else {
            self.more.push((id, at));
        }
    }

    /// Forgets that the record at `at` has identifier `id`.
    fn undefine(&mut self, id: XrefId, at: Place, store: &Store) {
        if !store.has_xref(id) {
            return;
        }
        if let Some(invalid) = self.invalid.get_mut() {
            let (v551, v7) = store.xref_validity(id);
            invalid[0] -= u32::from(!v551);
            invalid[1] -= u32::from(!v7);
        }
        if self.def(id) == at {
            // The next record with it, if any, comes first now.
            let next = self
                .more
                .iter()
                .enumerate()
                .filter(|(_, (x, _))| *x == id)
                .min_by_key(|(_, (_, p))| *p)
                .map(|(i, &(_, p))| (i, p));
            let new = match next {
                Some((i, p)) => {
                    self.more.swap_remove(i);
                    p
                }
                None => 0,
            };
            if let Some(d) = self.defs.get_mut(id.index()) {
                *d = new;
            }
        } else if let Some(i) = self.more.iter().position(|&m| m == (id, at)) {
            self.more.swap_remove(i);
        }
    }

    /// Adds `delta` to the count of each pointer `f` gives, when the counts
    /// are built. Ids from `limit` on are not the store's: they are not
    /// counted.
    fn count(&mut self, delta: i64, limit: usize, each: impl FnOnce(&mut dyn FnMut(XrefId))) {
        let Some(refs) = self.refs.get_mut() else {
            return;
        };
        each(&mut |p: XrefId| {
            let i = p.index();
            if i >= limit {
                return;
            }
            if refs.len() <= i {
                refs.resize(i + 1, 0);
            }
            if let Some(c) = refs.get_mut(i) {
                *c = u32::try_from(i64::from(*c) + delta).unwrap_or(0);
            }
        });
    }
}

/// A record or the header, lent by [`Dataset::get_mut`] or
/// [`Dataset::header_mut`]: a copy of it, as its owned type, which
/// dereferences to the structure, every field of which may be edited. When
/// it is dropped, the dataset stores it back in place of the record and
/// counts its pointers and identifier again. The copy is kept by the
/// dataset while it is lent: a guard forgotten (`mem::forget`) leaves the
/// dataset as it was, and the copy is dropped with the next one lent.
pub struct RecordMut<'a, T: Editable> {
    data: &'a mut Dataset,
    pos: usize,
    lent: std::marker::PhantomData<T>,
}

impl<'a, T: Editable> RecordMut<'a, T> {
    fn new(data: &'a mut Dataset, pos: usize, value: T) -> Self {
        data.tables.lent.0 = Some(Box::new(value));
        Self {
            data,
            pos,
            lent: std::marker::PhantomData,
        }
    }
}

impl<T: Editable> std::ops::Deref for RecordMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        match self
            .data
            .tables
            .lent
            .0
            .as_deref()
            .and_then(|v| v.downcast_ref::<T>())
        {
            Some(v) => v,
            // Taken only by the drop.
            None => unreachable!("a lent record is held until it is dropped"),
        }
    }
}

impl<T: Editable> std::ops::DerefMut for RecordMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        match self
            .data
            .tables
            .lent
            .0
            .as_deref_mut()
            .and_then(|v| v.downcast_mut::<T>())
        {
            Some(v) => v,
            None => unreachable!("a lent record is held until it is dropped"),
        }
    }
}

impl<T: Editable> Drop for RecordMut<'_, T> {
    fn drop(&mut self) {
        if let Some(value) = self
            .data
            .tables
            .lent
            .0
            .take()
            .and_then(|v| v.downcast::<T>().ok())
        {
            (*value).put(self.data, self.pos);
        }
    }
}

impl<T: Editable + fmt::Debug> fmt::Debug for RecordMut<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

/// Whether `xref` has the shape of an identifier: `@`, at least one
/// character, `@`, with no `@`, space or control character inside.
fn is_xref_shaped(xref: &str) -> bool {
    xref.strip_prefix('@')
        .and_then(|x| x.strip_suffix('@'))
        .is_some_and(|inner| {
            !inner.is_empty()
                && !inner
                    .chars()
                    .any(|c| c == '@' || c.is_whitespace() || c.is_control())
        })
}

impl Dataset {
    /// The records of type `T`, in order.
    #[must_use]
    pub fn all<T: RecordType>(&self) -> Items<'_, T> {
        Items::new(self, T::records_in(self), 0)
    }

    // ---- Handles --------------------------------------------------------

    /// The handles, built the first time they are asked for: reading and
    /// writing never need them.
    fn handle_table(&self) -> &Handles {
        self.index
            .handles
            .get_or_init(|| Box::new(Handles::new(self.lens())))
    }

    /// The handles, to change with the records.
    fn handle_table_mut(&mut self) -> &mut Handles {
        if self.index.handles.get().is_none() {
            let handles = Box::new(Handles::new(self.lens()));
            let _ = self.index.handles.set(handles);
        }
        match self.index.handles.get_mut() {
            Some(h) => h,
            None => unreachable!("the handles were just built"),
        }
    }

    /// The number of records of each kind.
    fn lens(&self) -> [usize; 9] {
        KINDS.map(|kind| self.len_of(kind))
    }

    fn len_of(&self, kind: RecordKind) -> usize {
        with_kind!(kind, |R| <R as RecordRows>::records(&self.tables).len())
    }

    /// The identifier of the record of `kind` at `pos`.
    fn xref_of_record(&self, kind: RecordKind, pos: usize) -> Option<XrefId> {
        with_kind!(kind, |R| <R as sealed::Record>::xref_at(self, pos))
    }

    /// The handles of the records of type `T`, in order.
    pub fn handles<T: RecordType>(&self) -> impl Iterator<Item = Handle<T>> + '_ {
        let handles = self.handle_table();
        handles
            .order
            .get(T::KIND.index())
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|&slot| handles.handle(slot))
            .filter_map(RecordHandle::typed)
    }

    /// The handle of the record of type `T` at `position` in its list.
    #[must_use]
    pub fn handle_at<T: RecordType>(&self, position: usize) -> Option<Handle<T>> {
        self.handle_table().at(T::KIND, position)?.typed()
    }

    /// The position of the first record of `kind` with identifier `id`, in
    /// constant time (but past 2^28 records of its kind: then a search).
    pub(crate) fn first_of(&self, kind: RecordKind, id: XrefId) -> Option<usize> {
        let search =
            || (0..self.len_of(kind)).find(|&pos| self.xref_of_record(kind, pos) == Some(id));
        let first = self.index.def(id);
        // The first of its kind: where the index says, else among those
        // after a record of another kind.
        let at = if place_kind(first) == Some(kind) {
            first
        } else {
            self.index
                .more
                .iter()
                .filter(|(x, p)| *x == id && place_kind(*p) == Some(kind))
                .map(|&(_, p)| p)
                .min()?
        };
        match place_pos(at) {
            Some(pos) => Some(pos),
            None => search(),
        }
    }

    /// The handle of the record of type `T` with this identifier (the
    /// first, when several have it). Constant time.
    #[must_use]
    pub fn handle<T: RecordType>(&self, xref: impl XrefKey) -> Option<Handle<T>> {
        let id = xref.id_in(&self.store)?;
        self.handle_at(self.first_of(T::KIND, id)?)
    }

    /// The first record with identifier `id`, whatever its type: its kind
    /// and position.
    fn first_record(&self, id: XrefId) -> Option<(RecordKind, usize)> {
        let kind = place_kind(self.index.def(id))?;
        Some((kind, self.first_of(kind, id)?))
    }

    /// The handle of the record with this identifier, whatever its type
    /// (the first, when several have it). Constant time.
    #[must_use]
    pub fn record_handle(&self, xref: impl XrefKey) -> Option<RecordHandle> {
        let (kind, pos) = self.first_record(xref.id_in(&self.store)?)?;
        self.handle_table().at(kind, pos)
    }

    /// The record with identifier `id`, whatever its type: the first, when
    /// several have it. Constant time, and no handle is made.
    pub(crate) fn find_record(&self, id: XrefId) -> Option<RecordRef<'_>> {
        let (kind, pos) = self.first_record(id)?;
        self.record_at(kind, pos)
    }

    /// Every record but the header with its handle, in
    /// [`Dataset::records`] order.
    pub fn records_with_handles(&self) -> impl Iterator<Item = (RecordHandle, RecordRef<'_>)> {
        let handles = self.handle_table();
        KINDS.into_iter().flat_map(move |kind| {
            handles
                .order
                .get(kind.index())
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .enumerate()
                .filter_map(move |(pos, &slot)| {
                    Some((handles.handle(slot)?, self.record_at(kind, pos)?))
                })
        })
    }

    /// The record of a handle.
    #[must_use]
    pub fn get<T: RecordType>(&self, handle: Handle<T>) -> Option<View<'_, T>> {
        let (_, pos) = self.handle_table().locate(handle.raw)?;
        self.all::<T>().get(pos)
    }

    /// The record of a handle, whatever its type.
    #[must_use]
    pub fn get_any(&self, handle: RecordHandle) -> Option<RecordRef<'_>> {
        let (kind, pos) = self.handle_table().locate(handle)?;
        self.record_at(kind, pos)
    }

    /// The position of a handle's record in its list.
    #[must_use]
    pub fn position<T: RecordType>(&self, handle: Handle<T>) -> Option<usize> {
        self.handle_table().locate(handle.raw).map(|(_, pos)| pos)
    }

    /// The record of a handle, to edit: any field, pointers and
    /// identifier included. The guard holds a copy of the record, which
    /// the dataset stores in its place, counting its pointers and its
    /// identifier again, when the guard is dropped (see [`RecordMut`]); an
    /// identifier another record has is kept as reading keeps one, the
    /// first record having it ([`Dataset::rename`] refuses it instead).
    pub fn get_mut<T: RecordType>(&mut self, handle: Handle<T>) -> Option<RecordMut<'_, T>> {
        let (_, pos) = self.handle_table().locate(handle.raw)?;
        self.get_mut_at(pos)
    }

    /// The record of type `T` at `position` in its list, to edit (see
    /// [`Dataset::get_mut`]).
    pub fn get_mut_at<T: RecordType>(&mut self, position: usize) -> Option<RecordMut<'_, T>> {
        let value = <T as sealed::Edit>::take(self, position)?;
        Some(RecordMut::new(self, position, value))
    }

    /// Edits every record of type `T` in turn, in order: each is lent to
    /// `edit` as [`Dataset::get_mut`] lends it.
    pub fn edit_all<T: RecordType>(&mut self, mut edit: impl FnMut(&mut T)) {
        for position in 0..<T as sealed::Record>::count(self) {
            if let Some(mut record) = self.get_mut_at::<T>(position) {
                edit(&mut record);
            }
        }
    }

    /// The header, to edit (see [`Dataset::get_mut`]).
    pub fn header_mut(&mut self) -> Option<RecordMut<'_, Header>> {
        let value = self.header.as_deref().cloned()?;
        Some(RecordMut::new(self, 0, value))
    }

    /// Replaces the header (with none: removes it); the one it had.
    pub fn set_header(&mut self, header: Option<Header>) -> Option<Header> {
        if let Some(old) = &self.header {
            self.index
                .count(-1, self.store.xref_count(), |f| old.each(f));
        }
        let old = std::mem::replace(&mut self.header, header.map(Box::new)).map(|h| *h);
        if let Some(new) = &self.header {
            self.index
                .count(1, self.store.xref_count(), |f| new.each(f));
        }
        old
    }

    // ---- The tables -----------------------------------------------------

    /// The record of `kind` at `pos`.
    pub(crate) fn record_at(&self, kind: RecordKind, pos: usize) -> Option<RecordRef<'_>> {
        macro_rules! view {
            ($t:ty, $variant:ident) => {{
                let row = <$t as RecordRows>::records(&self.tables).rows.get(pos)?;
                Some(RecordRef::$variant(View::new(
                    self,
                    row,
                    u32::try_from(pos).ok()?,
                )))
            }};
        }
        match kind {
            RecordKind::Submitter => view!(Submitter, Submitter),
            RecordKind::Submission => view!(Submission, Submission),
            RecordKind::Individual => view!(Individual, Individual),
            RecordKind::Family => view!(Family, Family),
            RecordKind::Note => view!(SharedNote, Note),
            RecordKind::Source => view!(Source, Source),
            RecordKind::Repository => view!(Repository, Repository),
            RecordKind::Multimedia => view!(Multimedia, Multimedia),
            RecordKind::Other => view!(Node, Other),
        }
    }

    /// The tables built anew from the records, in one pass over their
    /// identifiers: handles made before are no longer valid. For datasets
    /// made whole (read, deserialized).
    #[cfg(feature = "serde")]
    pub(crate) fn reindex(&mut self) {
        self.index = Index::default();
        for kind in KINDS {
            for pos in 0..self.len_of(kind) {
                if let Some(id) = self.xref_of_record(kind, pos) {
                    self.index.place_def(id, place(kind, pos));
                }
            }
        }
        self.index_read();
    }

    /// Completes the tables once every record is read and the store is
    /// the dataset's: the identifiers of another store left out.
    pub(crate) fn index_read(&mut self) {
        let store = &self.store;
        let index = &mut self.index;
        if index.defs.len() > store.xref_count() {
            // Ids of no identifier of the store: none is defined.
            index.defs.truncate(store.xref_count());
        }
        // Grown by doubling: what is spare is given back when it is worth
        // a copy.
        if index.defs.capacity() - index.defs.len() > 1 << 14 {
            index.defs.shrink_to_fit();
        }
        if !index.more.is_empty() {
            index.more.retain(|(id, _)| store.has_xref(*id));
        }
    }

    /// The records whose identifier is invalid in 5.5.1 and in 7.x,
    /// counted from the definitions, not the records: once.
    #[cold]
    #[inline(never)]
    fn count_invalid(&self) -> [u32; 2] {
        let mut invalid = [0_u32; 2];
        let ids = self
            .index
            .defs
            .iter()
            .enumerate()
            .filter(|&(_, &d)| d != 0)
            .map(|(i, _)| i)
            .chain(self.index.more.iter().map(|(id, _)| id.index()));
        for i in ids {
            let (v551, v7) = self.store.xref_validity_at(i);
            invalid[0] += u32::from(!v551);
            invalid[1] += u32::from(!v7);
        }
        invalid
    }

    /// Every pointer of the dataset, counted by id.
    fn count_all(&self) -> Vec<u32> {
        let mut refs = vec![0_u32; self.store.xref_count()];
        let mut add = |p: XrefId| {
            if let Some(c) = refs.get_mut(p.index()) {
                *c += 1;
            }
        };
        for record in self.records() {
            record.each_pointer(&mut add);
        }
        refs
    }

    /// The counts of pointers by id, built when first asked for.
    fn refs(&self) -> &[u32] {
        self.index.refs.get_or_init(|| self.count_all())
    }

    /// Counts the counts in, when they are not yet.
    fn counted(&mut self) {
        if self.index.refs.get().is_none() {
            let refs = self.count_all();
            let _ = self.index.refs.set(refs);
        }
    }

    /// Takes the identifier and pointers of the structure at `at` out of
    /// the tables (`delta` -1), or puts them in (+1).
    fn tally(&mut self, at: Lent, delta: i64) {
        let limit = self.store.xref_count();
        match at {
            Lent::Record(kind, pos) => {
                let Some(record) = self.record_at(kind, pos) else {
                    return;
                };
                let xref = record.xref();
                let mut pointers = Vec::new();
                if self.index.refs.get().is_some() {
                    record.each_pointer(&mut |p| pointers.push(p));
                }
                self.index
                    .count(delta, limit, |f| pointers.into_iter().for_each(f));
                if let Some(id) = xref {
                    if delta < 0 {
                        self.index.undefine(id, place(kind, pos), &self.store);
                    } else {
                        self.index.define(id, place(kind, pos), &self.store);
                    }
                }
            }
        }
    }

    /// Stores a record of type `T` at `pos`, in place of the one there, or
    /// after the last one: its position. Nothing is counted.
    pub(crate) fn store_record<T: RecordRows>(&mut self, record: &T, pos: Option<usize>) -> usize {
        let table = T::records_mut(&mut self.tables);
        let pos = match pos {
            Some(pos) if pos < table.len() => {
                // The untyped substructures of the record replaced go
                // with it.
                table.set_extra(u32::try_from(pos).unwrap_or(u32::MAX), List::EMPTY);
                pos
            }
            _ => {
                table.rows.push(T::fill());
                table.len() - 1
            }
        };
        let at = u32::try_from(pos).unwrap_or(u32::MAX);
        let mut cx = StoreCx {
            tables: &mut self.tables,
            store: &mut self.store,
        };
        let row = record.store_elem(at, &mut cx);
        if let Some(slot) = T::records_mut(&mut self.tables).rows.get_mut(pos) {
            *slot = row;
        }
        pos
    }

    /// Stores a record back at `pos` in place of the one there, its
    /// pointers and identifier counted out, then in again. The rows of the
    /// record replaced are left where they are, until the tables are
    /// compacted.
    pub(crate) fn replace_record<T: RecordRows>(&mut self, record: &T, pos: usize) {
        if pos >= T::records(&self.tables).len() {
            return;
        }
        self.counted();
        let at = Lent::Record(T::ROWS_KIND, pos);
        self.tally(at, -1);
        let before = self.tables.rows();
        self.store_record(record, Some(pos));
        self.tables.garbage += self.tables.rows().saturating_sub(before);
        self.tally(at, 1);
        self.compact_if_wasteful();
    }

    /// Copies every record into new tables when as many rows hold nothing
    /// any more as hold the records: what edits left behind.
    fn compact_if_wasteful(&mut self) {
        let garbage = self.tables.garbage;
        if garbage > 4096 && garbage * 2 > self.tables.rows() {
            self.compact();
        }
    }

    /// Copies every record into new tables, each structure's rows
    /// contiguous again: the records keep their positions, and handles
    /// stay valid.
    pub(crate) fn compact(&mut self) {
        let old = std::mem::take(&mut *self.tables);
        let mut cx = CopyCx {
            from: &old,
            to: &mut self.tables,
            moved: Moved::Same,
        };
        for kind in KINDS {
            with_kind!(kind, |R| copy_records::<R>(&mut cx));
        }
        self.tables.shrink();
    }

    /// Whether a pointer to `id` points to nothing by definition: 7.x
    /// `@VOID@`, a 5.5.1 pointer into a record or another dataset.
    pub(crate) fn points_nowhere(&self, id: XrefId) -> bool {
        let text = self.store.xref(id);
        (self.version.is_v7() && text == crate::tree::Xref::VOID)
            || crate::spec::is_external_pointer(
                text,
                crate::spec::payload::Family::of(self.version.rules()),
            )
    }

    /// Whether a record has `id`.
    fn is_defined(&self, id: XrefId) -> bool {
        self.index.def(id) != 0
    }

    /// Whether the identifiers of the records are unique and valid in the
    /// grammar of `rules`: the writer then renames none, and the index
    /// tells the record of each identifier. Constant time.
    pub(crate) fn xrefs_conform(&self, rules: &crate::version::VersionRules) -> bool {
        let i = usize::from(matches!(rules.xref, crate::version::XrefGrammar::V7));
        self.index.more.is_empty()
            && self
                .index
                .invalid
                .get_or_init(|| self.count_invalid())
                .get(i)
                == Some(&0)
    }

    /// By identifier id, where the record that has it is: its kind
    /// ([`RecordKind`] order) from bit [`PLACE_KIND_SHIFT`] on; 0 when no
    /// record has it.
    pub(crate) fn places(&self) -> &[u32] {
        &self.index.defs
    }

    // ---- References -----------------------------------------------------

    /// The number of pointers to this identifier, from the records and the
    /// header. Constant time once the counts are built, which the first
    /// query (or edit that needs them) does, walking every pointer once.
    #[must_use]
    pub fn references(&self, xref: impl XrefKey) -> usize {
        let Some(id) = xref.id_in(&self.store) else {
            return 0;
        };
        self.refs().get(id.index()).copied().unwrap_or(0) as usize
    }

    /// The header and records holding a pointer to this identifier, in
    /// [`Dataset::records`] order. Walks the records, unless the counts
    /// say none.
    #[must_use]
    pub fn referrers(&self, xref: impl XrefKey) -> Vec<Holder> {
        let Some(id) = xref.id_in(&self.store) else {
            return Vec::new();
        };
        if self.references(id) == 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Some(h) = &self.header {
            let mut found = false;
            h.each(&mut |p| found |= p == id);
            if found {
                out.push(Holder::Header);
            }
        }
        for (handle, record) in self.records_with_handles() {
            let mut found = false;
            record.each_pointer(&mut |p| found |= p == id);
            if found {
                out.push(Holder::Record(handle));
            }
        }
        out
    }

    /// The id of the identifier of a record of the dataset, to point to it.
    ///
    /// # Errors
    ///
    /// [`EditError::UnknownXref`] when no record has it.
    pub fn pointer(&self, xref: impl XrefKey) -> Result<XrefId, EditError> {
        let text = xref.text_in(&self.store);
        match xref.id_in(&self.store) {
            Some(id) if self.is_defined(id) => Ok(id),
            _ => Err(EditError::UnknownXref(text)),
        }
    }

    /// The id of `xref` when a record of the expected kind has it.
    fn target(
        &self,
        xref: impl XrefKey,
        expected: Option<RecordKind>,
    ) -> Result<XrefId, EditError> {
        let unknown = EditError::UnknownXref(xref.text_in(&self.store));
        let id = xref.id_in(&self.store).ok_or(unknown)?;
        if !self.store.has_xref(id) {
            return Err(EditError::ForeignXref);
        }
        let text = || -> Box<str> { self.store.xref(id).into() };
        let (found, _) = self
            .first_record(id)
            .ok_or_else(|| EditError::UnknownXref(text()))?;
        match expected {
            // A record of the expected type may have it after another.
            Some(kind) if found != kind && self.first_of(kind, id).is_none() => {
                Err(EditError::WrongType {
                    xref: text(),
                    expected: kind,
                    found,
                })
            }
            _ => Ok(id),
        }
    }

    // ---- Adding and removing records -------------------------------------

    /// Adds a record, checked: its identifier is the dataset's and no other
    /// record has it, and each of its pointers points to a record of the
    /// dataset (or to itself, or nowhere by definition: 7.x `@VOID@`).
    /// A record without an identifier is given a free one of its type
    /// (`@I1@`, `@F1@`, …, numbered on from the last one given; an
    /// extension record none). Its handle. The dataset stores it in its
    /// tables, its texts copied into its store.
    ///
    /// Its identifier and pointers are ids of this dataset's store
    /// ([`Dataset::pointer`], [`Dataset::store_mut`]).
    ///
    /// # Errors
    ///
    /// [`EditError::DuplicateXref`] when another record has its identifier,
    /// [`EditError::UnknownXref`] when a pointer points to no record,
    /// [`EditError::ForeignXref`] when an id is not of this dataset.
    ///
    /// ```rust
    /// use oxidged::model::{ChildLink, Dataset, Family, Individual, IndividualRef};
    /// use oxidged::GedcomVersion;
    ///
    /// let mut data = Dataset::new(GedcomVersion::V7_0);
    /// let ann = data.add(Individual::default()).unwrap();
    /// let ann_id = data.get(ann).unwrap().xref().unwrap();
    /// assert_eq!(data.store().xref(ann_id), "@I1@");
    /// let family = data
    ///     .add(Family { children: vec![IndividualRef::new(ann_id)], ..Family::default() })
    ///     .unwrap();
    /// assert_eq!(data.references(ann_id), 1);
    /// ```
    pub fn add<T: RecordType>(&mut self, mut record: T) -> Result<Handle<T>, EditError> {
        let own = if let Some(id) = record.xref_of() {
            if !self.store.has_xref(id) {
                return Err(EditError::ForeignXref);
            }
            if self.is_defined(id) {
                return Err(EditError::DuplicateXref(self.store.xref(id).into()));
            }
            Some(id)
        } else {
            let fresh = self.fresh_xref(T::KIND);
            record.set_xref(fresh);
            fresh
        };
        let mut problem = None;
        record.pointers(&mut |p| {
            if problem.is_some() || Some(p) == own {
                return;
            }
            if !self.store.has_xref(p) {
                problem = Some(EditError::ForeignXref);
            } else if !self.is_defined(p) && !self.points_nowhere(p) {
                problem = Some(EditError::UnknownXref(self.store.xref(p).into()));
            }
        });
        if let Some(problem) = problem {
            return Err(problem);
        }
        Ok(self.place(&record))
    }

    /// Puts a record after the last one of its type, in the tables:
    /// unchecked.
    fn place<T: RecordType>(&mut self, record: &T) -> Handle<T> {
        // Built first, the handles take the record as it is placed.
        self.handle_table_mut();
        let pos = self.place_unhandled(record);
        let raw = self.handle_table().at(T::KIND, pos);
        Handle {
            raw: raw.unwrap_or(RecordHandle {
                slot: u32::MAX,
                tag: FREE,
            }),
            record: PhantomData,
        }
    }

    /// [`Dataset::place`], making no handle unless the handles are built:
    /// the record's position.
    fn place_unhandled<T: RecordType>(&mut self, record: &T) -> usize {
        let pos = record.store_at(self, None);
        self.tally(Lent::Record(T::KIND, pos), 1);
        if let Some(handles) = self.index.handles.get_mut() {
            handles.alloc(T::KIND, pos);
        }
        pos
    }

    /// Adds a record unchecked, as reading adds one: an identifier another
    /// record has, pointers to no record are kept. A header replaces the
    /// header (`None`); [`Record::Other`] goes to the untyped records.
    /// Its texts and identifiers are of this dataset (owned texts, ids of
    /// [`Dataset::store_mut`]).
    pub fn push(&mut self, record: Record) -> Option<RecordHandle> {
        Some(match record {
            Record::Header(r) => {
                self.set_header(Some(*r));
                return None;
            }
            Record::Individual(r) => self.place(&*r).erase(),
            Record::Family(r) => self.place(&*r).erase(),
            Record::Source(r) => self.place(&*r).erase(),
            Record::Repository(r) => self.place(&*r).erase(),
            Record::Multimedia(r) => self.place(&*r).erase(),
            Record::Submitter(r) => self.place(&*r).erase(),
            Record::Submission(r) => self.place(&*r).erase(),
            Record::Note(r) => self.place(&*r).erase(),
            Record::Other(n) => self.place(&*n).erase(),
        })
    }

    /// An identifier `@<prefix><n>@` no record has and no pointer holds,
    /// `n` counting on from the last one given to a record of `kind`.
    fn fresh_xref(&mut self, kind: RecordKind) -> Option<XrefId> {
        let prefix = kind.prefix()?;
        let mut n = self
            .handle_table_mut()
            .fresh
            .get(kind.index())
            .copied()?
            .max(1);
        loop {
            let candidate = format!("@{prefix}{n}@");
            let free = match self.store.find_xref(&candidate) {
                None => self.store.intern_xref(&candidate),
                Some(id) if !self.is_defined(id) && self.references(id) == 0 => Some(id),
                Some(_) => None,
            };
            n = n.checked_add(1)?;
            if let Some(id) = free {
                if let Some(next) = self.handle_table_mut().fresh.get_mut(kind.index()) {
                    *next = n;
                }
                return Some(id);
            }
        }
    }

    /// Removes a record, and its pointers from the counts: the record
    /// removed (copied out), its pointers as they were.
    ///
    /// When other structures point to it (unless another record has its
    /// identifier too, which they then point to), `how` says what to do:
    /// refuse, remove them ([`Removal::Detach`]) or leave them dangling.
    ///
    /// # Errors
    ///
    /// [`EditError::StaleHandle`] when the record was removed already,
    /// [`EditError::StillReferenced`] under [`Removal::Refuse`].
    ///
    /// ```rust
    /// use oxidged::model::{Dataset, Individual, Removal};
    ///
    /// let mut data = Dataset::parse(
    ///     "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n0 @F1@ FAM\n1 CHIL @I1@\n0 TRLR\n",
    /// );
    /// let ann = data.handle::<Individual>("@I1@").unwrap();
    /// assert!(data.remove(ann, Removal::Refuse).is_err()); // the family's CHIL
    /// data.remove(ann, Removal::Detach).unwrap();
    /// assert!(data.families().first().unwrap().children().is_empty());
    /// assert!(data.dangling_references().is_empty());
    /// ```
    pub fn remove<T: RecordType>(
        &mut self,
        handle: Handle<T>,
        how: Removal,
    ) -> Result<T, EditError> {
        self.counted();
        let (kind, pos) = self
            .handle_table()
            .locate(handle.raw)
            .ok_or(EditError::StaleHandle)?;
        let record = <T as sealed::Record>::load_at(self, pos).ok_or(EditError::StaleHandle)?;
        let xref = record.xref_of();
        // The pointers to it that its removal leaves without a record.
        let orphaned = match xref {
            Some(id)
                if self.index.def(id) == place(kind, pos)
                    && !self.index.more.iter().any(|m| m.0 == id) =>
            {
                let mut own = 0_usize;
                record.pointers(&mut |p| own += usize::from(p == id));
                self.references(id).saturating_sub(own)
            }
            _ => 0,
        };
        if orphaned > 0 && how == Removal::Refuse {
            let xref = xref.map_or_else(Box::default, |id| self.store.xref(id).into());
            return Err(EditError::StillReferenced {
                xref,
                references: orphaned,
            });
        }
        self.tally(Lent::Record(kind, pos), -1);
        self.handle_table_mut().release(kind, pos);
        <T as sealed::Record>::remove_at(self, pos);
        self.shift_places(kind, pos);
        if let (Some(id), true, Removal::Detach) = (xref, orphaned > 0, how) {
            self.detach(id);
        }
        Ok(record)
    }

    /// The places of the records of `kind` from `pos` on, one back: the
    /// record that was at `pos` is gone.
    fn shift_places(&mut self, kind: RecordKind, pos: usize) {
        for at in pos..self.len_of(kind) {
            let Some(id) = self.xref_of_record(kind, at) else {
                continue;
            };
            let (old, new) = (place(kind, at + 1), place(kind, at));
            if let Some(d) = self.index.defs.get_mut(id.index())
                && *d == old
            {
                *d = new;
            }
        }
        for (_, p) in &mut self.index.more {
            if place_kind(*p) == Some(kind) && place_pos(*p).is_some_and(|at| at > pos) {
                *p = place(kind, place_pos(*p).unwrap_or(pos) - 1);
            }
        }
    }

    // ---- Identifiers ------------------------------------------------------

    /// Renames an identifier: the records that have it and every pointer
    /// to it follow (pointers are ids, the store gives an id its text), in
    /// constant time. The new identifier's id.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidXref`] when `new` is not shaped as an
    /// identifier (`@…@`), [`EditError::DuplicateXref`] when a record has
    /// `new`, [`EditError::XrefInUse`] when pointers hold `new` (redirect
    /// them first: [`Dataset::redirect`]), [`EditError::UnknownXref`] when
    /// the dataset has no `xref`.
    ///
    /// ```rust
    /// use oxidged::model::Dataset;
    ///
    /// let mut data = Dataset::parse(
    ///     "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n0 @F1@ FAM\n1 CHIL @I1@\n0 TRLR\n",
    /// );
    /// data.rename("@I1@", "@ANN@").unwrap();
    /// let out = oxidged::GedcomWriter::new().write_to_string(&data).unwrap();
    /// assert!(out.contains("0 @ANN@ INDI\n") && out.contains("1 CHIL @ANN@\n"));
    /// ```
    pub fn rename(&mut self, xref: impl XrefKey, new: &str) -> Result<XrefId, EditError> {
        let unknown = EditError::UnknownXref(xref.text_in(&self.store));
        let id = xref.id_in(&self.store).ok_or(unknown)?;
        if !self.store.has_xref(id) {
            return Err(EditError::ForeignXref);
        }
        if !is_xref_shaped(new) {
            return Err(EditError::InvalidXref(new.into()));
        }
        if self.store.xref(id) == new {
            return Ok(id);
        }
        if let Some(other) = self.store.find_xref(new) {
            if self.is_defined(other) {
                return Err(EditError::DuplicateXref(new.into()));
            }
            if self.references(other) > 0 {
                return Err(EditError::XrefInUse(new.into()));
            }
        }
        // The records that have it change validity with it.
        let holders = usize::from(self.index.def(id) != 0)
            + self.index.more.iter().filter(|m| m.0 == id).count();
        let (v551, v7) = self.store.xref_validity(id);
        if !self.store.rename_xref(id, new) {
            return Err(EditError::InvalidXref(new.into()));
        }
        let (n551, n7) = self.store.xref_validity(id);
        let n = u32::try_from(holders).unwrap_or(u32::MAX);
        if let Some(invalid) = self.index.invalid.get_mut() {
            invalid[0] = invalid[0] - n * u32::from(!v551) + n * u32::from(!n551);
            invalid[1] = invalid[1] - n * u32::from(!v7) + n * u32::from(!n7);
        }
        Ok(id)
    }

    /// Runs `edit` on the record of `kind` at `pos`, copied out, and
    /// stores it back when it says it changed it.
    fn edit_record(
        &mut self,
        kind: RecordKind,
        pos: usize,
        edit: &mut dyn FnMut(HolderMut<'_>) -> bool,
    ) {
        macro_rules! edit {
            ($t:ty, $variant:ident) => {{
                if let Some(mut r) = <$t as sealed::Record>::load_at(self, pos) {
                    if edit(HolderMut::$variant(&mut r)) {
                        self.replace_record(&r, pos);
                    }
                }
            }};
        }
        match kind {
            RecordKind::Submitter => edit!(Submitter, Submitter),
            RecordKind::Submission => edit!(Submission, Submission),
            RecordKind::Individual => edit!(Individual, Individual),
            RecordKind::Family => edit!(Family, Family),
            RecordKind::Note => edit!(SharedNote, Note),
            RecordKind::Source => edit!(Source, Source),
            RecordKind::Repository => edit!(Repository, Repository),
            RecordKind::Multimedia => edit!(Multimedia, Multimedia),
            RecordKind::Other => edit!(Node, Other),
        }
    }

    /// The records holding a pointer to `id`: their kinds and positions.
    fn holders_of(&self, id: XrefId) -> Vec<(RecordKind, usize)> {
        let mut out = Vec::new();
        for kind in KINDS {
            for pos in 0..self.len_of(kind) {
                let Some(r) = self.record_at(kind, pos) else {
                    continue;
                };
                let mut has = false;
                r.each_pointer(&mut |p| has |= p == id);
                if has {
                    out.push((kind, pos));
                }
            }
        }
        out
    }

    /// Points every pointer to `from` to `to` instead (to merge two
    /// records: redirect, then remove the one left without pointers). The
    /// number of pointers redirected.
    ///
    /// # Errors
    ///
    /// [`EditError::UnknownXref`] when no record has `to`.
    pub fn redirect(&mut self, from: impl XrefKey, to: impl XrefKey) -> Result<usize, EditError> {
        let to = self.pointer(to)?;
        let Some(from) = from.id_in(&self.store) else {
            return Ok(0);
        };
        if from == to || self.references(from) == 0 {
            return Ok(0);
        }
        let mut n = 0_usize;
        let mut f = |p: XrefId| {
            if p == from {
                n += 1;
                to
            } else {
                p
            }
        };
        if let Some(h) = self.header.as_deref_mut() {
            h.map(&mut f);
        }
        let delta_header = n;
        let mut redirected = 0_usize;
        for (kind, pos) in self.holders_of(from) {
            // Stored back with its pointers counted again.
            self.edit_record(kind, pos, &mut |mut h| {
                h.map(&mut |p| {
                    if p == from {
                        redirected += 1;
                        to
                    } else {
                        p
                    }
                });
                true
            });
        }
        let delta = i64::try_from(delta_header).unwrap_or(i64::MAX);
        self.index
            .count(-delta, self.store.xref_count(), |c| c(from));
        self.index.count(delta, self.store.xref_count(), |c| c(to));
        Ok(delta_header + redirected)
    }

    /// Removes every structure that points to this identifier, with its
    /// substructures, from the header and every record: the number
    /// removed. An untyped record whose own payload points there keeps its
    /// substructures and loses its payload. Walks the records holding such
    /// pointers, unless the counts say none.
    pub fn detach(&mut self, xref: impl XrefKey) -> usize {
        let Some(id) = xref.id_in(&self.store) else {
            return 0;
        };
        if self.references(id) == 0 {
            return 0;
        }
        let mut n = 0;
        if let Some(h) = self.header.as_deref_mut() {
            let mut removed = Vec::new();
            let mut keep = |p: XrefId| p != id;
            let mut gone = |p: XrefId| removed.push(p);
            let mut cut = Cut::new(&mut keep, &mut gone);
            h.retain(&mut cut);
            n += cut.count;
            self.index.count(-1, self.store.xref_count(), |c| {
                removed.into_iter().for_each(c);
            });
        }
        for (kind, pos) in self.holders_of(id) {
            self.edit_record(kind, pos, &mut |mut h| {
                let mut keep = |p: XrefId| p != id;
                let mut gone = |_: XrefId| {};
                let mut cut = Cut::new(&mut keep, &mut gone);
                h.retain(&mut cut);
                h.cut_own(&mut cut);
                n += cut.count;
                true
            });
        }
        n
    }

    // ---- Links ------------------------------------------------------------

    /// Adds a pointer of kind `link` from `holder` to the record with
    /// identifier `target`: a structure appended to its list (`FAMC`,
    /// `CHIL`, a citation, an extension structure, …), or set in its place
    /// (`HUSB`). Only this side: [`Dataset::link_child`] and
    /// [`Dataset::link_spouse`] link both a family and an individual.
    ///
    /// # Errors
    ///
    /// [`EditError::UnknownXref`] or [`EditError::WrongType`] when no
    /// record of the type the link points to has `target`,
    /// [`EditError::NotApplicable`] when `holder` has no such structure,
    /// [`EditError::AlreadyLinked`] when it already points there so,
    /// [`EditError::Occupied`] when its one structure of the kind points
    /// elsewhere, [`EditError::StaleHandle`], [`EditError::NoHeader`].
    pub fn link(
        &mut self,
        holder: impl Into<Holder>,
        link: Link,
        target: impl XrefKey,
    ) -> Result<(), EditError> {
        let target = self.target(target, link.target())?;
        self.with_holder(holder.into(), |h| h.link(link, target))
    }

    /// Removes the pointers of kind `link` from `holder` to `target`, with
    /// their structures (and their substructures): the number removed.
    ///
    /// # Errors
    ///
    /// [`EditError::NotLinked`] when there is none,
    /// [`EditError::NotApplicable`], [`EditError::StaleHandle`],
    /// [`EditError::NoHeader`].
    pub fn unlink(
        &mut self,
        holder: impl Into<Holder>,
        link: Link,
        target: impl XrefKey,
    ) -> Result<usize, EditError> {
        let Some(target) = target.id_in(&self.store) else {
            return Err(EditError::NotLinked(link));
        };
        match self.with_holder(holder.into(), |h| h.unlink(link, target))? {
            0 => Err(EditError::NotLinked(link)),
            n => Ok(n),
        }
    }

    /// Links a child and a family both ways: the family's `CHIL` and the
    /// individual's `FAMC`. Either side already there is kept.
    ///
    /// # Errors
    ///
    /// [`EditError::AlreadyLinked`] when both sides are,
    /// [`EditError::StaleHandle`], [`EditError::NoXref`] when either
    /// record has no identifier.
    ///
    /// ```rust
    /// use oxidged::model::{Dataset, Family, Individual};
    /// use oxidged::GedcomVersion;
    ///
    /// let mut data = Dataset::new(GedcomVersion::V7_0);
    /// let family = data.add(Family::default()).unwrap();
    /// let ann = data.add(Individual::default()).unwrap();
    /// data.link_child(family, ann).unwrap();
    /// let out = oxidged::GedcomWriter::new().write_to_string(&data).unwrap();
    /// assert!(out.contains("0 @F1@ FAM\n1 CHIL @I1@\n"));
    /// assert!(out.contains("0 @I1@ INDI\n1 FAMC @F1@\n"));
    /// ```
    pub fn link_child(
        &mut self,
        family: Handle<Family>,
        child: Handle<Individual>,
    ) -> Result<(), EditError> {
        self.link_both(family, child, &[Link::Child], Link::ChildOf)
    }

    /// Links a spouse and a family both ways: the family's `HUSB` (or
    /// `WIFE`, when `HUSB` is taken) and the individual's `FAMS`.
    ///
    /// # Errors
    ///
    /// [`EditError::Occupied`] when both partners are other individuals,
    /// [`EditError::AlreadyLinked`] when both sides are linked, as
    /// [`Dataset::link_child`].
    pub fn link_spouse(
        &mut self,
        family: Handle<Family>,
        spouse: Handle<Individual>,
    ) -> Result<(), EditError> {
        self.link_both(family, spouse, &[Link::Husband, Link::Wife], Link::SpouseOf)
    }

    /// Removes the links of [`Dataset::link_child`], both ways: the number
    /// of structures removed.
    ///
    /// # Errors
    ///
    /// [`EditError::NotLinked`] when neither side is linked.
    pub fn unlink_child(
        &mut self,
        family: Handle<Family>,
        child: Handle<Individual>,
    ) -> Result<usize, EditError> {
        self.unlink_both(family, child, &[Link::Child], Link::ChildOf)
    }

    /// Removes the links of [`Dataset::link_spouse`], both ways: the
    /// number of structures removed.
    ///
    /// # Errors
    ///
    /// [`EditError::NotLinked`] when neither side is linked.
    pub fn unlink_spouse(
        &mut self,
        family: Handle<Family>,
        spouse: Handle<Individual>,
    ) -> Result<usize, EditError> {
        self.unlink_both(family, spouse, &[Link::Husband, Link::Wife], Link::SpouseOf)
    }

    fn ids_of(
        &self,
        family: Handle<Family>,
        individual: Handle<Individual>,
    ) -> Result<(XrefId, XrefId), EditError> {
        let f = self.get(family).ok_or(EditError::StaleHandle)?.xref();
        let i = self.get(individual).ok_or(EditError::StaleHandle)?.xref();
        f.zip(i).ok_or(EditError::NoXref)
    }

    fn link_both(
        &mut self,
        family: Handle<Family>,
        individual: Handle<Individual>,
        family_side: &[Link],
        individual_side: Link,
    ) -> Result<(), EditError> {
        let (f, i) = self.ids_of(family, individual)?;
        let fam = self.get(family).ok_or(EditError::StaleHandle)?;
        let linked = |l: Link| match l {
            Link::Husband => fam.husband_id() == Some(i),
            Link::Wife => fam.wife_id() == Some(i),
            _ => fam
                .children()
                .into_iter()
                .any(|c| c.individual() == Some(i)),
        };
        let family_done = family_side.iter().any(|&l| linked(l));
        let free = family_side.iter().copied().find(|&l| match l {
            Link::Husband => fam.husband_id().is_none(),
            Link::Wife => fam.wife_id().is_none(),
            _ => true,
        });
        let indi = self.get(individual).ok_or(EditError::StaleHandle)?;
        let individual_done = match individual_side {
            Link::ChildOf => indi.child_of().into_iter().any(|c| c.family() == Some(f)),
            _ => indi.spouse_of().into_iter().any(|c| c.family() == Some(f)),
        };
        if family_done && individual_done {
            return Err(EditError::AlreadyLinked(individual_side));
        }
        let family_link = match (family_done, free) {
            (true, _) => None,
            (false, Some(l)) => Some(l),
            (false, None) => return Err(EditError::Occupied(family_side[0])),
        };
        if let Some(l) = family_link {
            self.with_holder(family.into(), |h| h.link(l, i))?;
        }
        if !individual_done {
            self.with_holder(individual.into(), |h| h.link(individual_side, f))?;
        }
        Ok(())
    }

    fn unlink_both(
        &mut self,
        family: Handle<Family>,
        individual: Handle<Individual>,
        family_side: &[Link],
        individual_side: Link,
    ) -> Result<usize, EditError> {
        let (f, i) = self.ids_of(family, individual)?;
        let mut n = 0;
        for &l in family_side {
            n += self.with_holder(family.into(), |h| h.unlink(l, i))?;
        }
        n += self.with_holder(individual.into(), |h| h.unlink(individual_side, f))?;
        if n == 0 {
            return Err(EditError::NotLinked(individual_side));
        }
        Ok(n)
    }

    /// Runs `edit` on the holder, copied out, and stores it back (its
    /// pointers counted again) when `edit` succeeds.
    fn with_holder<R>(
        &mut self,
        holder: Holder,
        edit: impl FnOnce(HolderMut<'_>) -> Result<R, EditError>,
    ) -> Result<R, EditError> {
        match holder {
            Holder::Header => {
                let mut header = self.header.as_deref().cloned().ok_or(EditError::NoHeader)?;
                let result = edit(HolderMut::Header(&mut header));
                if result.is_ok() {
                    self.counted();
                    self.set_header(Some(header));
                }
                result
            }
            Holder::Record(h) => {
                let (kind, pos) = self
                    .handle_table()
                    .locate(h)
                    .ok_or(EditError::StaleHandle)?;
                let mut edit = Some(edit);
                let mut result = Err(EditError::StaleHandle);
                self.edit_record(kind, pos, &mut |holder| match edit.take() {
                    Some(e) => {
                        result = e(holder);
                        result.is_ok()
                    }
                    None => false,
                });
                result
            }
        }
    }
}

/// Copies the records of type `R` of `cx.from` into `cx.to`, in order.
fn copy_records<R: RecordRows>(cx: &mut CopyCx<'_>) {
    let len = R::records(cx.from).len();
    for pos in 0..len {
        let Some(&row) = R::records(cx.from).rows.get(pos) else {
            continue;
        };
        let at = u32::try_from(pos).unwrap_or(u32::MAX);
        let copied = R::copy_elem(&row, at, cx, at);
        R::records_mut(cx.to).rows.push(copied);
    }
}

/// A holder lent to [`Dataset::link`] and [`Dataset::unlink`], and to the
/// edits that rewrite or cut its pointers.
enum HolderMut<'a> {
    Header(&'a mut Header),
    Submitter(&'a mut Submitter),
    Submission(&'a mut Submission),
    Individual(&'a mut Individual),
    Family(&'a mut Family),
    Note(&'a mut SharedNote),
    Source(&'a mut Source),
    Repository(&'a mut Repository),
    Multimedia(&'a mut Multimedia),
    Other(&'a mut Node),
}

/// A list of structures each pointing somewhere: links added at its end,
/// removed by their pointer.
trait PointerList {
    type Item: Pointers;
    fn items(&self) -> &[Self::Item];
    fn add(&mut self, item: Self::Item);
    fn cut(&mut self, target: XrefId) -> usize;
}

impl<T: Pointers> PointerList for Vec<T> {
    type Item = T;
    fn items(&self) -> &[T] {
        self
    }
    fn add(&mut self, item: T) {
        self.push(item);
    }
    fn cut(&mut self, target: XrefId) -> usize {
        let before = self.len();
        self.retain(|i| i.own() != Some(target));
        before - self.len()
    }
}

impl<T: Pointers> PointerList for ThinVec<T> {
    type Item = T;
    fn items(&self) -> &[T] {
        self
    }
    fn add(&mut self, item: T) {
        self.push(item);
    }
    fn cut(&mut self, target: XrefId) -> usize {
        let before = self.len();
        self.retain(|i| i.own() != Some(target));
        before - self.len()
    }
}

/// Adds `item` (pointing to `target`) to `list`, unless an item points
/// there already.
fn add_to<L: PointerList>(
    list: &mut L,
    link: Link,
    target: XrefId,
    item: L::Item,
) -> Result<(), EditError> {
    if list.items().iter().any(|i| i.own() == Some(target)) {
        return Err(EditError::AlreadyLinked(link));
    }
    list.add(item);
    Ok(())
}

/// Sets `slot` to `item` (pointing to `target`), unless it points
/// elsewhere.
fn set_in<T: Pointers>(
    slot: &mut Option<T>,
    link: Link,
    target: XrefId,
    item: T,
) -> Result<(), EditError> {
    match slot.as_ref().and_then(Pointers::own) {
        Some(p) if p == target => Err(EditError::AlreadyLinked(link)),
        Some(_) => Err(EditError::Occupied(link)),
        None => {
            *slot = Some(item);
            Ok(())
        }
    }
}

fn cut_in<T: Pointers>(slot: &mut Option<T>, target: XrefId) -> usize {
    if slot.as_ref().and_then(Pointers::own) == Some(target) {
        *slot = None;
        1
    } else {
        0
    }
}

fn citation(source: XrefId) -> Citation {
    Citation {
        source: CitationSource::Pointer(source),
        ..Citation::default()
    }
}

fn multimedia_link(object: XrefId) -> MultimediaLink {
    MultimediaLink {
        object: Some(object),
        ..MultimediaLink::default()
    }
}

fn association(individual: XrefId) -> Association {
    Association {
        individual: Some(individual),
        ..Association::default()
    }
}

fn repository(repository: XrefId) -> RepositoryCitation {
    RepositoryCitation {
        repository: Some(repository),
        ..RepositoryCitation::default()
    }
}

/// An extension structure `tag @target@`.
fn extension(tag: TagId, target: XrefId) -> Node {
    Node {
        payload: Value::Pointer(target),
        ..Node::new(tag)
    }
}

/// The extension structures of a holder of tag `tag` pointing to `target`.
fn is_extension(n: &Node, tag: TagId, target: XrefId) -> bool {
    n.tag == tag && n.payload == Value::Pointer(target)
}

/// A field of a holder that holds pointers of one kind of link.
trait FieldMut {
    /// Adds a pointer to `target`.
    fn link(&mut self, link: Link, target: XrefId) -> Result<(), EditError>;
    /// Removes the pointers to `target`: how many.
    fn cut(&mut self, target: XrefId) -> usize;
}

/// A list of structures, each made by the function from its pointer.
struct Many<'a, L: PointerList>(&'a mut L, fn(XrefId) -> L::Item);

impl<L: PointerList> FieldMut for Many<'_, L> {
    fn link(&mut self, link: Link, target: XrefId) -> Result<(), EditError> {
        add_to(self.0, link, target, (self.1)(target))
    }
    fn cut(&mut self, target: XrefId) -> usize {
        self.0.cut(target)
    }
}

/// A structure that occurs once.
struct One<'a, T: Pointers>(&'a mut Option<T>, fn(XrefId) -> T);

impl<T: Pointers> FieldMut for One<'_, T> {
    fn link(&mut self, link: Link, target: XrefId) -> Result<(), EditError> {
        set_in(self.0, link, target, (self.1)(target))
    }
    fn cut(&mut self, target: XrefId) -> usize {
        cut_in(self.0, target)
    }
}

/// Extension structures of a tag.
struct Ext<'a>(&'a mut dyn ExtensionList, TagId);

impl FieldMut for Ext<'_> {
    fn link(&mut self, link: Link, target: XrefId) -> Result<(), EditError> {
        if self
            .0
            .nodes()
            .iter()
            .any(|n| is_extension(n, self.1, target))
        {
            return Err(EditError::AlreadyLinked(link));
        }
        self.0.push_node(extension(self.1, target));
        Ok(())
    }
    fn cut(&mut self, target: XrefId) -> usize {
        let tag = self.1;
        self.0.cut_nodes(&mut |n| is_extension(n, tag, target))
    }
}

/// A field of a detail that is not allocated: empty.
struct Absent;

impl FieldMut for Absent {
    fn link(&mut self, link: Link, _target: XrefId) -> Result<(), EditError> {
        Err(EditError::NotApplicable(link))
    }
    fn cut(&mut self, _target: XrefId) -> usize {
        0
    }
}

fn same(id: XrefId) -> XrefId {
    id
}

impl HolderMut<'_> {
    /// Runs `f` on the holder's field for `link`; `None` when the holder
    /// has none. Without `create`, a detail not allocated is not: its
    /// fields are empty.
    fn with_field<R>(
        self,
        link: Link,
        create: bool,
        f: impl FnOnce(&mut dyn FieldMut) -> R,
    ) -> Option<R> {
        use HolderMut as H;
        use Link as L;
        macro_rules! many {
            ($list:expr, $make:expr) => {
                Some(f(&mut Many($list, $make)))
            };
        }
        macro_rules! one {
            ($slot:expr, $make:expr) => {
                Some(f(&mut One($slot, $make)))
            };
        }
        macro_rules! detail {
            ($r:expr, |$d:ident| $list:expr, $make:expr) => {
                if create {
                    let $d = $r.detail_mut();
                    many!($list, $make)
                } else if let Some($d) = $r.detail.as_deref_mut() {
                    many!($list, $make)
                } else {
                    Some(f(&mut Absent))
                }
            };
        }
        match (self, link) {
            (h, L::Extension(tag)) => {
                let list: &mut dyn ExtensionList = match h {
                    H::Header(r) => &mut r.extra,
                    H::Submitter(r) => &mut r.extra,
                    H::Submission(r) => &mut r.extra,
                    H::Individual(r) => &mut r.extra,
                    H::Family(r) => &mut r.extra,
                    H::Note(r) => &mut r.extra,
                    H::Source(r) => &mut r.extra,
                    H::Repository(r) => &mut r.extra,
                    H::Multimedia(r) => &mut r.extra,
                    H::Other(r) => &mut r.children,
                };
                Some(f(&mut Ext(list, tag)))
            }
            (H::Individual(r), L::ChildOf) => many!(&mut r.child_of, ChildLink::new),
            (H::Individual(r), L::SpouseOf) => many!(&mut r.spouse_of, SpouseLink::new),
            (H::Individual(r), L::Citation) => many!(&mut r.citations, citation),
            (H::Individual(r), L::Note) => many!(&mut r.notes, Note::shared),
            (H::Individual(r), L::Alias) => detail!(r, |d| &mut d.aliases, IndividualRef::new),
            (H::Individual(r), L::Association) => {
                detail!(r, |d| &mut d.associations, association)
            }
            (H::Individual(r), L::Multimedia) => {
                detail!(r, |d| &mut d.multimedia, multimedia_link)
            }
            (H::Individual(r), L::Submitter) => detail!(r, |d| &mut d.submitters, same),
            (H::Individual(r), L::AncestorInterest) => {
                detail!(r, |d| &mut d.ancestor_interests, same)
            }
            (H::Individual(r), L::DescendantInterest) => {
                detail!(r, |d| &mut d.descendant_interests, same)
            }
            (H::Family(r), L::Husband) => one!(&mut r.husband, IndividualRef::new),
            (H::Family(r), L::Wife) => one!(&mut r.wife, IndividualRef::new),
            (H::Family(r), L::Child) => many!(&mut r.children, IndividualRef::new),
            (H::Family(r), L::Citation) => many!(&mut r.citations, citation),
            (H::Family(r), L::Note) => many!(&mut r.notes, Note::shared),
            (H::Family(r), L::Association) => detail!(r, |d| &mut d.associations, association),
            (H::Family(r), L::Multimedia) => detail!(r, |d| &mut d.multimedia, multimedia_link),
            (H::Family(r), L::Submitter) => detail!(r, |d| &mut d.submitters, same),
            (H::Source(r), L::Repository) => many!(&mut r.repositories, repository),
            (H::Source(r), L::Note) => many!(&mut r.notes, Note::shared),
            (H::Source(r), L::Multimedia) => many!(&mut r.multimedia, multimedia_link),
            (H::Repository(r), L::Note) => many!(&mut r.notes, Note::shared),
            (H::Multimedia(r), L::Note) => many!(&mut r.notes, Note::shared),
            (H::Multimedia(r), L::Citation) => many!(&mut r.citations, citation),
            (H::Submitter(r), L::Note) => many!(&mut r.notes, Note::shared),
            (H::Submitter(r), L::Multimedia) => many!(&mut r.multimedia, multimedia_link),
            (H::Submission(r), L::Submitter) => one!(&mut r.submitter, same),
            (H::Submission(r), L::Note) => many!(&mut r.notes, Note::shared),
            (H::Note(r), L::Citation) => many!(&mut r.citations, citation),
            (H::Header(r), L::Submitter) => one!(&mut r.submitter, same),
            (H::Header(r), L::Submission) => one!(&mut r.submission, same),
            (H::Header(r), L::Note) => many!(&mut r.notes, Note::shared),
            _ => None,
        }
    }

    fn link(self, link: Link, target: XrefId) -> Result<(), EditError> {
        self.with_field(link, true, |f| f.link(link, target))
            .unwrap_or(Err(EditError::NotApplicable(link)))
    }

    /// The holder's pointers, to rewrite or cut.
    fn pointers(&mut self) -> &mut dyn Pointers {
        match self {
            HolderMut::Header(r) => *r,
            HolderMut::Submitter(r) => *r,
            HolderMut::Submission(r) => *r,
            HolderMut::Individual(r) => *r,
            HolderMut::Family(r) => *r,
            HolderMut::Note(r) => *r,
            HolderMut::Source(r) => *r,
            HolderMut::Repository(r) => *r,
            HolderMut::Multimedia(r) => *r,
            HolderMut::Other(r) => *r,
        }
    }

    /// Rewrites every pointer to `f`'s.
    fn map(&mut self, f: &mut dyn FnMut(XrefId) -> XrefId) {
        self.pointers().map(f);
    }

    /// Removes the substructures whose own pointer `cut` refuses.
    fn retain(&mut self, cut: &mut Cut<'_>) {
        self.pointers().retain(cut);
    }

    /// Clears the holder's own pointer when `cut` refuses it.
    fn cut_own(&mut self, cut: &mut Cut<'_>) {
        self.pointers().cut_own(cut);
    }

    fn unlink(self, link: Link, target: XrefId) -> Result<usize, EditError> {
        self.with_field(link, false, |f| f.cut(target))
            .ok_or(EditError::NotApplicable(link))
    }
}

/// The untyped substructures of a holder: a structure's `extra`, an
/// untyped record's children.
trait ExtensionList {
    fn nodes(&self) -> &[Node];
    fn push_node(&mut self, node: Node);
    fn cut_nodes(&mut self, cut: &mut dyn FnMut(&Node) -> bool) -> usize;
}

impl ExtensionList for super::Extra {
    fn nodes(&self) -> &[Node] {
        self
    }
    fn push_node(&mut self, node: Node) {
        self.push(node);
    }
    fn cut_nodes(&mut self, cut: &mut dyn FnMut(&Node) -> bool) -> usize {
        let before = self.len();
        (**self).retain(|n| !cut(n));
        before - self.len()
    }
}

impl ExtensionList for Vec<Node> {
    fn nodes(&self) -> &[Node] {
        self
    }
    fn push_node(&mut self, node: Node) {
        self.push(node);
    }
    fn cut_nodes(&mut self, cut: &mut dyn FnMut(&Node) -> bool) -> usize {
        let before = self.len();
        self.retain(|n| !cut(n));
        before - self.len()
    }
}

impl Dataset {
    /// Adds the record at `placed` of `from` (a dataset a stream read) to
    /// this dataset, unchecked, as reading adds one: its rows are copied
    /// into this dataset's tables, its text into its store. A header
    /// becomes this dataset's (the version of an empty dataset with it);
    /// a second one an untyped record.
    pub(crate) fn adopt(
        &mut self,
        mut from: Dataset,
        placed: super::read::Placed,
        declared: Option<Box<str>>,
    ) {
        use super::read::Placed;
        match placed {
            Placed::Header => {
                let Some(header) = from.header.take().map(|h| *h) else {
                    return;
                };
                if self.header.is_some() {
                    let cx = super::driver::WriteCx::owned(&from.store, self.version, false);
                    let structure = super::driver::ToNodes::to_node(&header, "HEAD", &cx);
                    let node = Node::from_structure(&structure, &mut self.store);
                    self.place_unhandled(&node);
                    return;
                }
                // Through the tables, which move texts and identifiers.
                let idx = {
                    let mut cx = StoreCx {
                        tables: &mut from.tables,
                        store: &mut from.store,
                    };
                    header.store_one(&mut cx)
                };
                let copied = {
                    let mut cx = CopyCx {
                        from: &from.tables,
                        to: &mut self.tables,
                        moved: Moved::Into(super::relocate::Relocation::new(
                            &from.store,
                            &mut self.store,
                        )),
                    };
                    Header::copy_one(idx, &mut cx)
                };
                let header = Header::load_one(copied, &self.tables);
                self.tables.garbage += 1;
                if self.record_count() == 0 {
                    self.version = from.version;
                }
                self.declared_version = declared;
                self.set_header(Some(header));
            }
            Placed::Record(kind, pos) => {
                with_kind!(kind, |R| self.adopt_record::<R>(&from, pos));
            }
        }
    }

    /// Copies the record of `R` at `pos` of `from` after the last one of
    /// this dataset, and places it in the tables.
    fn adopt_record<R: RecordRows + RecordType>(&mut self, from: &Dataset, pos: usize) {
        let Some(&row) = R::records(&from.tables).rows.get(pos) else {
            return;
        };
        let to = R::records(&self.tables).len();
        let at = u32::try_from(to).unwrap_or(u32::MAX);
        let copied = {
            let mut cx = CopyCx {
                from: &from.tables,
                to: &mut self.tables,
                moved: Moved::Into(super::relocate::Relocation::new(
                    &from.store,
                    &mut self.store,
                )),
            };
            R::copy_elem(&row, u32::try_from(pos).unwrap_or(u32::MAX), &mut cx, at)
        };
        R::records_mut(&mut self.tables).rows.push(copied);
        self.tally(Lent::Record(R::KIND, to), 1);
        if let Some(handles) = self.index.handles.get_mut() {
            handles.alloc(R::KIND, to);
        }
    }
}
