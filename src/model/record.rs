//! The records of a dataset: individuals, families, sources, repositories,
//! multimedia objects, submitters, submissions and shared notes.
//!
//! A record that has an identifier keeps it (`xref`); a record without one
//! is given one by the writer. Records are read in any version, by tag: a
//! 5.5.1 `NOTE` record and a 7.x `SNOTE` record are both shared notes,
//! written with their version's tag.

use super::address::Address;
use super::citation::{Citation, RepositoryCitation};
use super::date::{ChangeDate, CreationDate, Period};
use super::driver::{StdTag, WriteCx, gedcom_struct};
use super::enums::{EnumList, EventKind, NoteKind, OrdinanceFlag, Phrased, Restriction, Sex};
use super::event::{Event, NonEvent};
use super::identifier::{Exid, Refn};
use super::lds::Ordinance;
use super::link::Association;
use super::link::{ChildLink, IndividualRef, SpouseLink};
use super::list::ThinVec;
use super::multimedia::{File, MultimediaLink};
use super::name::Name;
use super::note::{Note, NoteTranslation, SourceText};
use super::place::Place;
use super::text::{Store, Text, XrefId};

gedcom_struct! {
    /// An individual (`INDI`): names, sex, events and attributes, the
    /// families they are a child and a spouse in, notes, sources and when
    /// the record changed; the rest — ordinances, associations, aliases,
    /// identifiers, … — is in its [`detail`](Self::detail).
    ///
    /// Every link to a family is kept, in order, and so is every
    /// name piece, as written: none is derived from the name.
    ///
    /// ```rust
    /// use oxidged::model::{Dataset, Sex};
    ///
    /// let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 SEX f\n1 FAMC @F1@\n1 FAMC @F1@\n2 PEDI adopted\n1 REFN 7\n0 @F1@ FAM\n0 TRLR\n");
    /// let ann = data.individuals().first().unwrap();
    /// assert_eq!(ann.sex(), Some(Sex::Female)); // 5.5.1 controlled values ignore case
    /// assert_eq!(ann.child_of().len(), 2);
    /// assert_eq!(ann.refns().first().unwrap().value(), "7");
    /// ```
    pub struct Individual => IndividualRow {
        @xref
        /// The record's identifier.
        xref;
        /// The names (`NAME`), in order of preference.
        "NAME" => names: Vec<Name>,
        /// The sex (`SEX`).
        "SEX" => sex: Option<Sex>,
        /// Events and attributes, in order.
        "ADOP" | "BAPM" | "BARM" | "BASM" | "BIRT" | "BLES" | "BURI" | "CENS" | "CHR" | "CHRA"
            | "CONF" | "CREM" | "DEAT" | "EMIG" | "FCOM" | "GRAD" | "IMMI" | "NATU" | "ORDN"
            | "PROB" | "RETI" | "WILL" | "EVEN" | "CAST" | "DSCR" | "EDUC" | "IDNO" | "NATI"
            | "NCHI" | "NMR" | "OCCU" | "PROP" | "RELI" | "RESI" | "SSN" | "TITL" | "FACT"
            => events: Vec<Event>,
        /// The families the individual is a child in (`FAMC`).
        "FAMC" => child_of: ThinVec<ChildLink>,
        /// The families the individual is a spouse or partner in (`FAMS`).
        "FAMS" => spouse_of: ThinVec<SpouseLink>,
        /// Notes (`NOTE`, 7.x `SNOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
        /// Sources (`SOUR`).
        "SOUR" => citations: ThinVec<Citation>,
        /// When the record last changed (`CHAN`).
        "CHAN" => change: Option<ChangeDate>,
        @detail
        /// What fewer individuals have: a restriction, ordinances, events
        /// known not to have happened, associations, aliases, submitters,
        /// interests, media, identifiers and the creation date.
        IndividualDetail => IndividualDetailRow {
            /// Restrictions on the record (`RESN`).
            "RESN" => restriction: Option<EnumList<Restriction>>,
            /// Latter-day Saint ordinances (`BAPL`, `CONL`, `ENDL`, `INIL`,
            /// `SLGC`).
            "BAPL" | "CONL" | "ENDL" | "INIL" | "SLGC" => ordinances: Vec<Ordinance>,
            /// Events known not to have happened (7.x `NO`).
            "NO" => non_events: Vec<NonEvent>,
            /// Associated individuals (`ASSO`).
            "ASSO" => associations: Vec<Association>,
            /// Other records of the same individual (`ALIA`); in 7.x never
            /// this record itself, which the writer writes as `_ALIA`
            /// (7.0.17).
            "ALIA" => aliases: Vec<IndividualRef>,
            /// Submitters of the record (`SUBM`).
            "SUBM" => submitters: Vec<XrefId>,
            /// Submitters interested in the individual's ancestors (`ANCI`).
            "ANCI" => ancestor_interests: Vec<XrefId>,
            /// Submitters interested in the individual's descendants
            /// (`DESI`).
            "DESI" => descendant_interests: Vec<XrefId>,
            /// Media (`OBJE`).
            "OBJE" => multimedia: Vec<MultimediaLink>,
            /// User reference numbers (`REFN`).
            "REFN" => refns: Vec<Refn>,
            /// Unique identifiers (7.x `UID`).
            "UID" => uids: Vec<Text>,
            /// Identifiers in external systems (7.x `EXID`).
            "EXID" => exids: Vec<Exid>,
            /// The permanent record file number (5.5.1 `RFN`).
            "RFN" => record_file_number: Option<Text>,
            /// The ancestral file number (5.5.1 `AFN`).
            "AFN" => ancestral_file_number: Option<Text>,
            /// The automated record identifier (5.5.1 `RIN`).
            "RIN" => record_id: Option<Text>,
            /// When the record was created (7.x `CREA`).
            "CREA" => creation: Option<CreationDate>,
        }
    }
    spec {
        v551: ["INDIVIDUAL_RECORD.INDI"],
        v70: ["record-INDI"],
        v71: ["record-INDI"],
    }
}

gedcom_struct! {
    /// A family (`FAM`): its partners (`HUSB`, `WIFE`; a second one of
    /// either is kept in `extra`, in place), children, events and
    /// attributes (`NCHI` with its event detail), notes, sources and
    /// when the record changed; the rest is in its
    /// [`detail`](Self::detail).
    pub struct Family => FamilyRow {
        @xref
        /// The record's identifier.
        xref;
        /// The husband or first partner (`HUSB`).
        "HUSB" => husband: Option<IndividualRef>,
        /// The wife or second partner (`WIFE`).
        "WIFE" => wife: Option<IndividualRef>,
        /// The children (`CHIL`), in order.
        "CHIL" => children: Vec<IndividualRef>,
        /// Events and attributes, in order.
        "ANUL" | "CENS" | "DIV" | "DIVF" | "ENGA" | "MARB" | "MARC" | "MARL" | "MARR" | "MARS"
            | "EVEN" | "RESI" | "NCHI" | "FACT" => events: Vec<Event>,
        /// Notes (`NOTE`, 7.x `SNOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
        /// Sources (`SOUR`).
        "SOUR" => citations: ThinVec<Citation>,
        /// When the record last changed (`CHAN`).
        "CHAN" => change: Option<ChangeDate>,
        @detail
        /// What fewer families have: a restriction, sealings, events known
        /// not to have happened, associations, submitters, media,
        /// identifiers and the creation date.
        FamilyDetail => FamilyDetailRow {
            /// Restrictions on the record (`RESN`).
            "RESN" => restriction: Option<EnumList<Restriction>>,
            /// Latter-day Saint sealings of the couple (`SLGS`).
            "SLGS" => ordinances: Vec<Ordinance>,
            /// Events known not to have happened (7.x `NO`).
            "NO" => non_events: Vec<NonEvent>,
            /// Associated individuals (7.x `ASSO`).
            "ASSO" => associations: Vec<Association>,
            /// Submitters of the record (`SUBM`).
            "SUBM" => submitters: Vec<XrefId>,
            /// Media (`OBJE`).
            "OBJE" => multimedia: Vec<MultimediaLink>,
            /// User reference numbers (`REFN`).
            "REFN" => refns: Vec<Refn>,
            /// Unique identifiers (7.x `UID`).
            "UID" => uids: Vec<Text>,
            /// Identifiers in external systems (7.x `EXID`).
            "EXID" => exids: Vec<Exid>,
            /// The automated record identifier (5.5.1 `RIN`).
            "RIN" => record_id: Option<Text>,
            /// When the record was created (7.x `CREA`).
            "CREA" => creation: Option<CreationDate>,
        }
    }
    spec {
        v551: ["FAM_RECORD.FAM"],
        v70: ["record-FAM"],
        v71: ["record-FAM"],
    }
}

impl Individual {
    /// The preferred name: the first one.
    #[must_use]
    pub fn name(&self) -> Option<&Name> {
        self.names.first()
    }

    /// The preferred name as written, without its surname slashes
    /// ([`Name::full`]).
    #[must_use]
    pub fn full_name<S: AsRef<Store> + ?Sized>(&self, store: &S) -> Option<String> {
        self.name().map(|n| n.full(store))
    }

    /// The events and attributes of a kind, in order.
    pub fn events_of(&self, kind: EventKind) -> impl Iterator<Item = &Event> {
        self.events.iter().filter(move |e| e.kind == kind)
    }

    /// The first birth (`BIRT`).
    #[must_use]
    pub fn birth(&self) -> Option<&Event> {
        self.events_of(EventKind::Birth).next()
    }

    /// The first death (`DEAT`).
    #[must_use]
    pub fn death(&self) -> Option<&Event> {
        self.events_of(EventKind::Death).next()
    }
}

impl Family {
    /// The husband or first partner's identifier.
    #[must_use]
    pub fn husband_id(&self) -> Option<XrefId> {
        self.husband.as_ref().and_then(|h| h.individual)
    }

    /// The wife or second partner's identifier.
    #[must_use]
    pub fn wife_id(&self) -> Option<XrefId> {
        self.wife.as_ref().and_then(|w| w.individual)
    }
}

gedcom_struct! {
    /// A source (`SOUR` record): its title, author, abbreviation and
    /// publication, its text, the data it records (`DATA`), the
    /// repositories holding it, notes, media, identifiers and its change
    /// and creation dates.
    pub struct Source => SourceRow {
        @xref
        /// The record's identifier.
        xref;
        /// The title (`TITL`).
        "TITL" => title: Option<Text>,
        /// The author (`AUTH`).
        "AUTH" => author: Option<Text>,
        /// A short title (`ABBR`).
        "ABBR" => abbreviation: Option<Text>,
        /// Publication facts (`PUBL`).
        "PUBL" => publication: Option<Text>,
        /// Text from the source (`TEXT`).
        "TEXT" => text: Option<SourceText>,
        /// What the source records (`DATA`).
        "DATA" => data: Option<SourceData>,
        /// Repositories holding the source (`REPO`).
        "REPO" => repositories: ThinVec<RepositoryCitation>,
        /// Notes (`NOTE`, 7.x `SNOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
        /// Media (`OBJE`).
        "OBJE" => multimedia: ThinVec<MultimediaLink>,
        /// Restrictions on the record (7.1 `RESN`).
        "RESN" => restriction: Option<EnumList<Restriction>>,
        /// User reference numbers (`REFN`).
        "REFN" => refns: ThinVec<Refn>,
        /// Unique identifiers (7.x `UID`).
        "UID" => uids: ThinVec<Text>,
        /// Identifiers in external systems (7.x `EXID`).
        "EXID" => exids: ThinVec<Exid>,
        /// The automated record identifier (5.5.1 `RIN`).
        "RIN" => record_id: Option<Text>,
        /// When the record last changed (`CHAN`).
        "CHAN" => change: Option<ChangeDate>,
        /// When the record was created (7.x `CREA`).
        "CREA" => creation: Option<CreationDate>,
    }
    spec {
        v551: ["SOURCE_RECORD.SOUR"],
        v70: ["record-SOUR"],
        v71: ["record-SOUR"],
    }
}

gedcom_struct! {
    /// What a source records (`DATA` of a source record): the events, the
    /// agency responsible, notes.
    pub struct SourceData => SourceDataRow {
        /// The events recorded (`EVEN`).
        "EVEN" => events: ThinVec<RecordedEvents>,
        /// The agency responsible (`AGNC`).
        "AGNC" => agency: Option<Text>,
        /// Notes (`NOTE`, 7.x `SNOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
    }
    spec {
        v551: ["SOURCE_RECORD.SOUR.DATA"],
        v70: ["DATA"],
        v71: ["DATA"],
    }
}

gedcom_struct! {
    /// Events a source records (`EVEN` of a source's `DATA`): their kinds,
    /// the period and the place they cover.
    pub struct RecordedEvents => RecordedEventsRow {
        @payload
        /// The kinds of event, comma-separated (`BIRT, DEAT`).
        kinds: EnumList<EventKind>;
        /// The period covered (`DATE`).
        "DATE" => date: Option<Period>,
        /// The place covered (`PLAC`), as a full place in 7.x.
        "PLAC" => place: Option<Place>,
    }
    spec {
        v551: ["SOURCE_RECORD.SOUR.DATA.EVEN"],
        v70: ["DATA-EVEN"],
        v71: ["DATA-EVEN"],
    }
}

gedcom_struct! {
    /// A repository (`REPO` record): its name, address and contacts,
    /// notes, identifiers and change and creation dates.
    pub struct Repository => RepositoryRow {
        @xref
        /// The record's identifier.
        xref;
        /// The name (`NAME`).
        "NAME" => name: Option<Text>,
        /// The address (`ADDR`).
        "ADDR" => address: Option<Address>,
        /// Telephone numbers (`PHON`).
        "PHON" => phones: ThinVec<Text>,
        /// E-mail addresses (`EMAIL`).
        "EMAIL" => emails: ThinVec<Text>,
        /// Fax numbers (`FAX`).
        "FAX" => faxes: ThinVec<Text>,
        /// Web pages (`WWW`).
        "WWW" => websites: ThinVec<Text>,
        /// Notes (`NOTE`, 7.x `SNOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
        /// Restrictions on the record (7.1 `RESN`).
        "RESN" => restriction: Option<EnumList<Restriction>>,
        /// User reference numbers (`REFN`).
        "REFN" => refns: ThinVec<Refn>,
        /// Unique identifiers (7.x `UID`).
        "UID" => uids: ThinVec<Text>,
        /// Identifiers in external systems (7.x `EXID`).
        "EXID" => exids: ThinVec<Exid>,
        /// The automated record identifier (5.5.1 `RIN`).
        "RIN" => record_id: Option<Text>,
        /// When the record last changed (`CHAN`).
        "CHAN" => change: Option<ChangeDate>,
        /// When the record was created (7.x `CREA`).
        "CREA" => creation: Option<CreationDate>,
    }
    spec {
        v551: ["REPOSITORY_RECORD.REPO"],
        v70: ["record-REPO"],
        v71: ["record-REPO"],
    }
}

gedcom_struct! {
    /// A multimedia object (`OBJE` record): its files, every one kept, notes, sources, identifiers and change and creation dates.
    pub struct Multimedia => MultimediaRow {
        @xref
        /// The record's identifier.
        xref;
        /// The files (`FILE`).
        "FILE" => files: ThinVec<File>,
        /// Restrictions on the record (`RESN`).
        "RESN" => restriction: Option<EnumList<Restriction>>,
        /// Notes (`NOTE`, 7.x `SNOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
        /// Sources (`SOUR`).
        "SOUR" => citations: ThinVec<Citation>,
        /// User reference numbers (`REFN`).
        "REFN" => refns: ThinVec<Refn>,
        /// Unique identifiers (7.x `UID`).
        "UID" => uids: ThinVec<Text>,
        /// Identifiers in external systems (7.x `EXID`).
        "EXID" => exids: ThinVec<Exid>,
        /// The automated record identifier (5.5.1 `RIN`).
        "RIN" => record_id: Option<Text>,
        /// When the record last changed (`CHAN`).
        "CHAN" => change: Option<ChangeDate>,
        /// When the record was created (7.x `CREA`).
        "CREA" => creation: Option<CreationDate>,
    }
    spec {
        v551: ["MULTIMEDIA_RECORD.OBJE"],
        v70: ["record-OBJE"],
        v71: ["record-OBJE"],
    }
}

gedcom_struct! {
    /// A submitter (`SUBM` record): name, address and contacts, languages
    /// (every one kept), media, notes, identifiers and change and
    /// creation dates.
    pub struct Submitter => SubmitterRow {
        @xref
        /// The record's identifier.
        xref;
        /// The name (`NAME`).
        "NAME" => name: Option<Text>,
        /// The address (`ADDR`).
        "ADDR" => address: Option<Address>,
        /// Telephone numbers (`PHON`).
        "PHON" => phones: ThinVec<Text>,
        /// E-mail addresses (`EMAIL`).
        "EMAIL" => emails: ThinVec<Text>,
        /// Fax numbers (`FAX`).
        "FAX" => faxes: ThinVec<Text>,
        /// Web pages (`WWW`).
        "WWW" => websites: ThinVec<Text>,
        /// Languages (`LANG`), in order of preference.
        "LANG" => languages: ThinVec<Text>,
        /// Media (`OBJE`).
        "OBJE" => multimedia: ThinVec<MultimediaLink>,
        /// Notes (`NOTE`, 7.x `SNOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
        /// Restrictions on the record (7.1 `RESN`).
        "RESN" => restriction: Option<EnumList<Restriction>>,
        /// User reference numbers (7.x `REFN`).
        "REFN" => refns: ThinVec<Refn>,
        /// Unique identifiers (7.x `UID`).
        "UID" => uids: ThinVec<Text>,
        /// Identifiers in external systems (7.x `EXID`).
        "EXID" => exids: ThinVec<Exid>,
        /// The permanent record file number (5.5.1 `RFN`).
        "RFN" => record_file_number: Option<Text>,
        /// The automated record identifier (5.5.1 `RIN`).
        "RIN" => record_id: Option<Text>,
        /// When the record last changed (`CHAN`).
        "CHAN" => change: Option<ChangeDate>,
        /// When the record was created (7.x `CREA`).
        "CREA" => creation: Option<CreationDate>,
    }
    spec {
        v551: ["SUBMITTER_RECORD.SUBM"],
        v70: ["record-SUBM"],
        v71: ["record-SUBM"],
    }
}

gedcom_struct! {
    /// A submission to a 5.5.1 processing system (`SUBN` record): the
    /// submitter, the family file and temple, how many generations of
    /// ancestors and descendants, whether to process ordinances, notes and
    /// the change date. Every substructure is kept.
    pub struct Submission => SubmissionRow {
        @xref
        /// The record's identifier.
        xref;
        /// The submitter (`SUBM`).
        "SUBM" => submitter: Option<XrefId>,
        /// The family file's name (`FAMF`).
        "FAMF" => family_file: Option<Text>,
        /// The temple (`TEMP`).
        "TEMP" => temple: Option<Text>,
        /// Generations of ancestors (`ANCE`).
        "ANCE" => ancestors: Option<Text>,
        /// Generations of descendants (`DESC`).
        "DESC" => descendants: Option<Text>,
        /// Whether ordinances are to be processed (`ORDI`).
        "ORDI" => ordinance_process: Option<OrdinanceFlag>,
        /// Notes (`NOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
        /// The automated record identifier (`RIN`).
        "RIN" => record_id: Option<Text>,
        /// When the record last changed (`CHAN`).
        "CHAN" => change: Option<ChangeDate>,
    }
    spec {
        v551: ["SUBMISSION_RECORD.SUBN"],
        v70: [],
        v71: [],
    }
}

/// A shared note is written `SNOTE` in 7.x, `NOTE` in 5.5.1.
fn shared_note_tag(_note: &SharedNote, _tag: StdTag, cx: &WriteCx<'_>) -> StdTag {
    if cx.version.is_v7() {
        const { StdTag::new("SNOTE") }
    } else {
        const { StdTag::new("NOTE") }
    }
}

/// [`shared_note_tag`] of a row.
fn shared_note_tag_row(_note: &SharedNoteRow, _tag: StdTag, cx: &WriteCx<'_>) -> StdTag {
    if cx.version.is_v7() {
        const { StdTag::new("SNOTE") }
    } else {
        const { StdTag::new("NOTE") }
    }
}

gedcom_struct! {
    /// A note that structures point to (5.5.1 `NOTE` record, 7.x `SNOTE`
    /// record): its text, media type, language, translations and sources,
    /// identifiers, change and creation dates, and 7.1's kind.
    pub struct SharedNote => SharedNoteRow [tag = shared_note_tag, shared_note_tag_row] {
        @xref
        /// The record's identifier.
        xref;
        @payload
        /// The text.
        text: Text;
        /// The media type of the text (7.x `MIME`): `text/plain` or
        /// `text/html`, or another `text/` type, an extension that needs no
        /// extension tag (7.0.17).
        "MIME" => mime: Option<Text>,
        /// The language of the text (7.x `LANG`).
        "LANG" => language: Option<Text>,
        /// Translations (7.x `TRAN`).
        "TRAN" => translations: ThinVec<NoteTranslation>,
        /// Sources (`SOUR`).
        "SOUR" => citations: ThinVec<Citation>,
        /// What kind of note it is (7.1 `KIND`).
        "KIND" => kinds: ThinVec<Phrased<NoteKind>>,
        /// Restrictions on the record (7.1 `RESN`).
        "RESN" => restriction: Option<EnumList<Restriction>>,
        /// User reference numbers (`REFN`).
        "REFN" => refns: ThinVec<Refn>,
        /// Unique identifiers (7.x `UID`).
        "UID" => uids: ThinVec<Text>,
        /// Identifiers in external systems (7.x `EXID`).
        "EXID" => exids: ThinVec<Exid>,
        /// The automated record identifier (5.5.1 `RIN`).
        "RIN" => record_id: Option<Text>,
        /// When the record last changed (`CHAN`).
        "CHAN" => change: Option<ChangeDate>,
        /// When the record was created (7.x `CREA`).
        "CREA" => creation: Option<CreationDate>,
    }
    spec {
        v551: ["NOTE_RECORD.NOTE"],
        v70: ["record-SNOTE"],
        v71: ["record-SNOTE"],
    }
}

impl<'a> super::table::View<'a, Individual> {
    /// The preferred name: the first one.
    #[must_use]
    pub fn name(self) -> Option<super::table::View<'a, Name>> {
        self.names().first()
    }

    /// The preferred name as written, without its surname slashes
    /// ([`Name::full`]).
    #[must_use]
    pub fn full_name(self) -> Option<String> {
        self.name().map(super::table::View::<Name>::full)
    }

    /// The events and attributes of a kind, in order.
    pub fn events_of(
        self,
        kind: EventKind,
    ) -> impl Iterator<Item = super::table::View<'a, Event>> + 'a {
        self.events().into_iter().filter(move |e| e.kind() == kind)
    }

    /// The first birth (`BIRT`).
    #[must_use]
    pub fn birth(self) -> Option<super::table::View<'a, Event>> {
        self.events_of(EventKind::Birth).next()
    }

    /// The first death (`DEAT`).
    #[must_use]
    pub fn death(self) -> Option<super::table::View<'a, Event>> {
        self.events_of(EventKind::Death).next()
    }
}

impl super::table::View<'_, Family> {
    /// The husband or first partner's identifier.
    #[must_use]
    pub fn husband_id(self) -> Option<XrefId> {
        self.husband()
            .and_then(super::table::View::<IndividualRef>::individual)
    }

    /// The wife or second partner's identifier.
    #[must_use]
    pub fn wife_id(self) -> Option<XrefId> {
        self.wife()
            .and_then(super::table::View::<IndividualRef>::individual)
    }
}
