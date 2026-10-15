//! Source and repository citations.

use crate::tree::{FlatPayload, Payload};

use super::date::Date;
use super::driver::{PayloadField, WriteCx, gedcom_struct, pointer_or_text_raw};
use super::enums::{Certainty, EventKind, Medium, Phrased, Role};
use super::list::ThinVec;
use super::multimedia::MultimediaLink;
use super::note::{Note, SourceText};
use super::text::{Text, XrefId};

/// What a citation cites: a source record, or (5.5.1) a source described
/// in its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CitationSource {
    /// A pointer to a source record (7.x: `@VOID@` when there is none).
    Pointer(XrefId),
    /// A description of the source (5.5.1 `SOUR <SOURCE_DESCRIPTION>`).
    Description(Text),
}

impl Default for CitationSource {
    fn default() -> Self {
        CitationSource::Description(Text::default())
    }
}

impl PayloadField for CitationSource {
    fn write(&self, cx: &WriteCx<'_>) -> Payload {
        match self {
            CitationSource::Pointer(id) => cx.pointer(*id),
            CitationSource::Description(t) => cx.text(t),
        }
    }

    fn payload<'s>(&'s self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        match self {
            CitationSource::Pointer(id) => cx.pointer_flat(*id),
            CitationSource::Description(t) => cx.text_flat(t),
        }
    }

    pointer_or_text_raw!(CitationSource, Pointer, Description);
}

impl super::pointers::Pointers for CitationSource {
    fn own(&self) -> Option<XrefId> {
        match self {
            CitationSource::Pointer(id) => Some(*id),
            CitationSource::Description(_) => None,
        }
    }

    fn each(&self, f: &mut dyn FnMut(XrefId)) {
        if let CitationSource::Pointer(id) = self {
            f(*id);
        }
    }

    fn map(&mut self, f: &mut dyn FnMut(XrefId) -> XrefId) {
        if let CitationSource::Pointer(id) = self {
            *id = f(*id);
        }
    }
}

#[cfg(feature = "serde")]
impl super::serde::SerializeIn for CitationSource {
    fn serialize_in<S: ::serde::Serializer>(
        &self,
        store: &super::Store,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match self {
            CitationSource::Pointer(v) => super::serde::serialize_tagged("Pointer", v, store, s),
            CitationSource::Description(v) => {
                super::serde::serialize_tagged("Description", v, store, s)
            }
        }
    }
}

#[cfg(feature = "serde")]
impl super::serde::DeserializeIn for CitationSource {
    fn deserialize_in<'de, D: ::serde::Deserializer<'de>>(
        store: &mut super::Store,
        d: D,
    ) -> Result<Self, D::Error> {
        super::serde::deserialize_either(
            store,
            d,
            &["Pointer", "Description"],
            CitationSource::Pointer,
            CitationSource::Description,
        )
    }
}

gedcom_struct! {
    /// A source citation (`SOUR`): the source, where in it (`PAGE`), what
    /// it says (`DATA`), the event it records (`EVEN`), how reliable it is
    /// (`QUAY`), and media and notes.
    pub struct Citation => CitationRow [chain = notes: Note] {
        @payload
        /// The source cited.
        source: CitationSource;
        /// Where in the source (`PAGE`).
        "PAGE" => page: Option<Text>,
        @detail
        /// What fewer citations have: what the source says, the event it
        /// records, its quality, texts, media and notes.
        CitationDetail => CitationDetailRow {
            /// What the source says (`DATA`).
            "DATA" => data: Option<CitationData>,
            /// The event the source records (`EVEN`).
            "EVEN" => event: Option<CitedEvent>,
            /// How reliable the evidence is (`QUAY`).
            "QUAY" => quality: Option<Certainty>,
            /// Text from a described source (5.5.1 `TEXT`).
            "TEXT" => texts: Vec<SourceText>,
            /// Media (`OBJE`).
            "OBJE" => multimedia: Vec<MultimediaLink>,
            /// Notes (`NOTE`, 7.x `SNOTE`).
            "NOTE" | "SNOTE" => notes: Vec<Note>,
        }
    }
    spec {
        v551: ["SOURCE_CITATION.SOUR", "SOURCE_CITATION.SOUR#2"],
        v70: ["SOUR"],
        v71: ["SOUR"],
    }
}

gedcom_struct! {
    /// What a cited source says (`DATA` of a citation): when it was
    /// recorded and its text.
    pub struct CitationData => CitationDataRow {
        /// When the entry was recorded (`DATE`).
        "DATE" => date: Option<Date>,
        /// The text of the entry (`TEXT`).
        "TEXT" => texts: Vec<SourceText>,
    }
    spec {
        v551: ["SOURCE_CITATION.SOUR.DATA"],
        v70: ["SOUR-DATA"],
        v71: ["SOUR-DATA"],
    }
}

gedcom_struct! {
    /// The event a cited source records (`EVEN` of a citation) and the
    /// role of the person in it.
    pub struct CitedEvent => CitedEventRow {
        @payload
        /// The event or attribute type (5.5.1: any text, kept in
        /// `Unknown` when it is not a tag).
        kind: EventKind;
        /// The event in words (7.x `PHRASE`).
        "PHRASE" => phrase: Option<Text>,
        /// The role of the person cited (`ROLE`).
        "ROLE" => role: Option<Phrased<Role>>,
    }
    spec {
        v551: ["SOURCE_CITATION.SOUR.EVEN"],
        v70: ["SOUR-EVEN"],
        v71: ["SOUR-EVEN"],
    }
}

gedcom_struct! {
    /// A repository citation (`REPO` of a source): the repository, the
    /// call numbers and notes. 5.5.1 allows it without a pointer.
    pub struct RepositoryCitation => RepositoryCitationRow {
        @payload
        /// The repository (7.x: `@VOID@` when unknown).
        repository: Option<XrefId>;
        /// Call numbers (`CALN`).
        "CALN" => call_numbers: Vec<CallNumber>,
        /// Notes (`NOTE`, 7.x `SNOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
    }
    spec {
        v551: ["SOURCE_REPOSITORY_CITATION.REPO"],
        v70: ["REPO"],
        v71: ["REPO"],
    }
}

gedcom_struct! {
    /// A call number in a repository (`CALN`) and the medium of the item
    /// (`MEDI`).
    pub struct CallNumber => CallNumberRow {
        @payload
        /// The call number.
        value: Text;
        /// The medium (`MEDI`), with a phrase in 7.x.
        "MEDI" => medium: Option<Phrased<Medium>>,
    }
    spec {
        v551: ["SOURCE_REPOSITORY_CITATION.REPO.CALN"],
        v70: ["CALN"],
        v71: ["CALN"],
    }
}

impl super::table::sealed::Field for CitationSource {
    type View<'a> = Self;
}
