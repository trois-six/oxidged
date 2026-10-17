//! The typed model: a [`Dataset`] of records, every structure of GEDCOM
//! 5.5.1, 7.0 and 7.1 a Rust type.
//!
//! # Records and structures
//!
//! A [`Dataset`] holds the [`Header`] and the records, each type in file
//! order: [`Individual`], [`Family`], [`Source`], [`Repository`],
//! [`Multimedia`], [`Submitter`], [`Submission`] and [`SharedNote`].
//! Records of no type the model has (extension records, unknown records)
//! stay untyped [`Node`]s in [`Dataset::extra`].
//!
//! Each structure is a type ([`Event`], [`Name`], [`ChildLink`],
//! [`Ordinance`], [`Citation`], [`Place`], …) with one field per
//! substructure the specifications permit — an `Option` when every version
//! permits one, a list when one permits several — and `extra`, the
//! substructures it has no field for (extensions, unknown tags, a second
//! occurrence of a singleton, a structure whose shape does not fit its
//! type), in order, where they were. Enumeration values are enums
//! ([`Sex`], [`Pedigree`], [`Role`], …) whose `Unknown` variant keeps any
//! other value as written; the 7.x `PHRASE` that words a value is kept
//! with it ([`Phrased`]).
//!
//! # Reading
//!
//! Reading never fails and loses nothing: a file is decoded whatever its
//! encoding ([`crate::encoding`]), its lines are read by the lenient rules
//! of [`crate::tree`], and each record is typed as far as it fits. Values
//! are kept as written: dates and ages are read by the grammars of
//! [`crate::value`] on demand. [`Dataset::parse`] and
//! [`Dataset::from_bytes`] read text and bytes;
//! [`GedcomBuilder`](crate::GedcomBuilder) adds limits, strict mode and
//! GEDZIP archives; [`GedcomStreamParser`](crate::GedcomStreamParser) reads
//! one record at a time.
//!
//! # Tables and views
//!
//! A dataset keeps every structure but the header as a row of the table
//! of its type: rows of a fixed size, contiguous, that hold a structure
//! held once by its index in its table, a list as a run of its table, a
//! text as an 8-byte span of the store and a pointer as an interned id.
//! Reading fills the tables in place, a list's run reserved before its
//! items are read; no structure is allocated on its own, and a dataset
//! takes about twice the size of its input.
//!
//! The dataset's accessors give *views* of the rows ([`View`]): `Copy`
//! handles with a method per field, which give a substructure as a view, a
//! list as [`Items`], a text as a [`TextRef`] (which reads, compares and
//! prints without the store), a pointer as an [`XrefId`] and an
//! enumeration value as its type. The fields few occurrences of a
//! structure use, kept in a row of their own (the *detail*), are methods
//! of the view like the others. [`View::into_owned`] copies a structure
//! out as its owned type ([`Individual`], [`Event`], …), which is what
//! records are built and edited as.
//!
//! A view's accessors are inherent methods of `View<'_, T>` for each `T`,
//! so that they need no import and each type's documentation lists its
//! own. Several types share names (`xref`, `value`, `date`, …): a method
//! *path* such as `View::xref` names all of them and does not compile
//! (E0034), whatever clippy's pedantic `redundant_closure_for_method_calls`
//! suggests. Call the method instead, or name the type:
//!
//! ```rust
//! use oxidged::model::{Dataset, Individual, View};
//!
//! let data = Dataset::parse("0 HEAD\n0 @I1@ INDI\n0 @I2@ INDI\n0 TRLR\n");
//! let ids: Vec<_> = data.individuals().iter().map(|i| i.xref()).collect();
//! let same: Vec<_> = data.individuals().iter().map(View::<Individual>::xref).collect();
//! assert_eq!(ids, same);
//! ```
//!
//! Traits for the shared accessors would make the bare path compile, but
//! every call would then need the trait in scope, for a gain to
//! point-free style only; the accessors stay inherent (decided in PR13).
//!
//! # Texts and identifiers
//!
//! A dataset keeps the decoded text it was read from in its [`Store`], and
//! its rows point into it:
//!
//! - a text is a span of the input, or, for a payload reading rewrote
//!   (continuation lines joined, `@@` unescaped), a run of the store's
//!   pieces of it — no copy of the characters; text a program sets is
//!   appended to the store when its structure is stored. An owned [`Text`]
//!   (16 bytes) is the same, or owns its characters;
//! - identifiers and pointers are interned [`XrefId`]s (4 bytes), which the
//!   store maps to their text (`@I1@`) and back;
//! - the tags of untyped [`Node`]s are [`TagId`]s.
//!
//! [`TextRef`] reads a text of a view; [`Text::to_str`] and [`Store::xref`]
//! read an owned text and an identifier, given the dataset (or anything
//! that holds a store). On the owned types, the fields few occurrences of
//! a structure use are in its boxed *detail* ([`Event::detail`],
//! [`Individual::detail`], …), and lists that seldom repeat are
//! [`ThinVec`]s.
//!
//! # Editing
//!
//! A dataset is edited through its methods, which keep its references
//! whole (see [`Dataset`]):
//!
//! - every record has a [`Handle`], which stays valid until the record is
//!   removed, whatever else changes ([`Dataset::handle`] by identifier,
//!   [`Dataset::handles`] in order, [`Dataset::add`]);
//! - [`Dataset::add`] refuses an identifier another record has and
//!   pointers to no record; [`Dataset::remove`] refuses a record other
//!   structures point to, unless asked to remove them too
//!   ([`Removal::Detach`]) or to leave them dangling;
//! - [`Dataset::rename`] gives an identifier a new text, which its record
//!   and every pointer to it take at once; [`Dataset::redirect`] moves
//!   pointers from one record to another;
//! - [`Dataset::link`] and [`Dataset::unlink`] add and remove pointers of
//!   every kind ([`Link`]: family links, citations, notes, media,
//!   repositories, submitters, extension structures);
//!   [`Dataset::link_child`] and [`Dataset::link_spouse`] link a family and
//!   an individual both ways;
//! - [`Dataset::get_mut`] lends a record, copied out as its owned type, to
//!   edit any of its fields; when the loan ends ([`RecordMut`]), it is
//!   stored back as new rows and its pointers are counted again (the rows
//!   it leaves are reclaimed once they outnumber the others);
//! - [`Dataset::references`] counts the pointers to an identifier in
//!   constant time, [`Dataset::referrers`] lists their holders and
//!   [`Dataset::dangling_references`] the pointers to no record.
//!
//! ```rust
//! use oxidged::model::{Dataset, Family, Individual, Link, Removal, Source};
//! use oxidged::GedcomWriter;
//!
//! let mut data = Dataset::parse(
//!     "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
//!      0 @S1@ SOUR\n1 TITL Parish register\n0 TRLR\n",
//! );
//! let ann = data.handle::<Individual>("@I1@").unwrap();
//! let family = data.add(Family::default()).unwrap(); // given @F1@
//! data.link_spouse(family, ann).unwrap();
//! data.link(ann, Link::Citation, "@S1@").unwrap();
//! assert_eq!(data.references("@S1@"), 1);
//!
//! let source = data.handle::<Source>("@S1@").unwrap();
//! assert!(data.remove(source, Removal::Refuse).is_err()); // still cited
//! data.rename("@I1@", "@ANN@").unwrap();
//!
//! let out = GedcomWriter::new().write_to_string(&data).unwrap();
//! assert!(out.contains("0 @ANN@ INDI\n1 NAME Ann /Example/\n1 FAMS @F1@\n1 SOUR @S1@\n"));
//! assert!(out.contains("0 @F1@ FAM\n1 HUSB @ANN@\n"));
//! ```
//!
//! # Writing
//!
//! [`GedcomWriter`](crate::GedcomWriter) writes a dataset in its version or
//! another: values are converted to the target's grammars, the structures
//! are repaired to conform to its specification ([`crate::spec::conform`])
//! and every line follows its line rules.
//!
//! ```rust
//! use oxidged::model::{Dataset, NoteContent, Pedigree};
//! use oxidged::GedcomWriter;
//!
//! let data = Dataset::parse(
//!     "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NOTE Hello\n2 LANG en\n\
//!      1 FAMC @F1@\n2 PEDI OTHER\n3 PHRASE Guardianship\n0 @F1@ FAM\n0 TRLR\n",
//! );
//! let indi = data.individuals().first().unwrap();
//! let note = indi.notes().first().unwrap();
//! assert_eq!(note.text().unwrap(), "Hello");
//! assert_eq!(note.language().unwrap(), "en");
//!
//! let pedigree = indi.child_of().first().unwrap().pedigree().unwrap();
//! assert_eq!(pedigree.value(), Pedigree::Other);
//! // Copied out as owned types, to edit or keep.
//! let owned = indi.into_owned();
//! assert!(matches!(&owned.notes[0].content, NoteContent::Text(t) if t.to_str(&data) == "Hello"));
//!
//! let out = GedcomWriter::new().write_to_string(&data).unwrap();
//! assert!(out.contains("2 PEDI OTHER\n3 PHRASE Guardianship\n"));
//! ```

pub(crate) mod driver;

mod address;
mod citation;
mod dataset;
mod date;
mod edit;
mod enums;
mod event;
mod header;
mod identifier;
pub(crate) mod kinship;
mod lds;
mod ledger_impl;
mod link;
mod list;
mod multimedia;
mod name;
mod node;
mod note;
mod place;
pub(crate) mod pointers;
pub(crate) mod read;
mod record;
pub(crate) mod relocate;
#[cfg(feature = "serde")]
pub(crate) mod serde;
pub(crate) mod table;
mod text;
pub(crate) mod write;

pub(crate) use ledger_impl::field_order;

pub use address::Address;
pub use citation::{
    CallNumber, Citation, CitationData, CitationDetail, CitationSource, CitedEvent,
    RepositoryCitation,
};
pub use dataset::{DanglingReference, Dataset, HeaderRef, Record, RecordRef, XrefKey};
pub use date::{Age, ChangeDate, CreationDate, Date, DateDetail, ExactDate, Period};
pub(crate) use edit::PLACE_KIND_SHIFT;
pub use edit::{
    EditError, Editable, Handle, Holder, Link, RecordHandle, RecordKind, RecordMut, RecordType,
    Removal,
};
pub(crate) use enums::Enumeration;
pub use enums::{
    Adoption, BirthKind, Certainty, CharacterSet, ChildStatus, EnumList, EventKind, GedcomForm,
    LdsStatus, Medium, MultimediaFormat, NameType, NoteKind, OrdinanceFlag, OrdinanceStatus,
    Pedigree, PhoneticType, Phrased, Restriction, Role, RomanizedType, Sex,
};
pub use event::{Event, EventDetail, EventFamily, EventSpouse, NonEvent};
pub use header::{
    Charset, Corporation, GedcomInfo, Header, HeaderPlace, HeaderSource, HeaderSourceData,
    HeaderText, Schema,
};
pub use identifier::{Exid, Refn};
pub use kinship::Relative;
pub use lds::{Ordinance, OrdinanceKind};
pub use link::{
    Association, ChildLink, ChildLinkDetail, IndividualRef, IndividualRefDetail, SpouseLink,
};
pub use list::ThinVec;
pub use multimedia::{Crop, File, FileForm, FileTranslation, MultimediaLink};
pub use name::{
    Name, NameDetail, NamePiece, NamePieceKind, NameTranslation, PhoneticName, RomanizedName,
};
pub use node::{Extra, Node, Value};
pub use note::{Note, NoteContent, NoteDetail, NoteTranslation, SourceText, TextTranslation};
pub use place::{Map, PhoneticVariation, Place, PlaceDetail, PlaceTranslation, RomanizedVariation};
pub use record::{
    Family, FamilyDetail, Individual, IndividualDetail, Multimedia, RecordedEvents, Repository,
    SharedNote, Source, SourceData, Submission, Submitter,
};
pub use table::{Field, FieldView, Items, ItemsIter, Listed, Stored, View};
pub use text::{Store, TagId, Text, TextRef, XrefId};

/// The declarations of the typed model, for the coverage ledger
/// (`tests/spec_coverage.rs`), which checks them against the specification
/// tables.
#[doc(hidden)]
pub mod ledger {
    pub use super::driver::{FieldDesc, SpecNames};
    pub use super::enums::{EnumDesc, EnumValue};

    /// Every typed structure: its name, the structure types it stands for
    /// and its fields.
    #[must_use]
    pub fn structures() -> &'static [(&'static str, SpecNames, &'static [FieldDesc])] {
        super::ledger_impl::STRUCTURES
    }

    /// Every enumeration type.
    #[must_use]
    pub fn enumerations() -> &'static [EnumDesc] {
        super::enums::ENUMS
    }

    /// The paths of the structures of `data` kept untyped although a field
    /// takes their tag: they did not fit their type.
    #[must_use]
    pub fn untyped(data: &super::Dataset) -> Vec<String> {
        super::ledger_impl::untyped(data)
    }
}
