//! Notes, texts and their translations.

use crate::tree::{FlatPayload, Payload};

use super::citation::Citation;
use super::driver::{PayloadField, StdTag, WriteCx, gedcom_struct};
use super::enums::{NoteKind, Phrased};
use super::text::{Text, XrefId};

/// What a note holds: its own text, or a pointer to a shared note (5.5.1
/// `NOTE @N1@`, 7.x `SNOTE @N1@`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteContent {
    /// The note's text, continuations joined.
    Text(Text),
    /// A pointer to a shared note record.
    Shared(XrefId),
}

impl Default for NoteContent {
    fn default() -> Self {
        NoteContent::Text(Text::default())
    }
}

impl PayloadField for NoteContent {
    fn write(&self, cx: &WriteCx<'_>) -> Payload {
        match self {
            NoteContent::Text(t) => cx.text(t),
            NoteContent::Shared(id) => cx.pointer(*id),
        }
    }

    fn payload<'s>(&'s self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        match self {
            NoteContent::Text(t) => cx.text_flat(t),
            NoteContent::Shared(id) => cx.pointer_flat(*id),
        }
    }

    super::driver::pointer_or_text_raw!(NoteContent, Shared, Text);
}

impl super::pointers::Pointers for NoteContent {
    fn own(&self) -> Option<XrefId> {
        match self {
            NoteContent::Shared(id) => Some(*id),
            NoteContent::Text(_) => None,
        }
    }

    fn each(&self, f: &mut dyn FnMut(XrefId)) {
        if let NoteContent::Shared(id) = self {
            f(*id);
        }
    }

    fn map(&mut self, f: &mut dyn FnMut(XrefId) -> XrefId) {
        if let NoteContent::Shared(id) = self {
            *id = f(*id);
        }
    }
}

#[cfg(feature = "serde")]
impl super::serde::SerializeIn for NoteContent {
    fn serialize_in<S: ::serde::Serializer>(
        &self,
        store: &super::Store,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match self {
            NoteContent::Text(v) => super::serde::serialize_tagged("Text", v, store, s),
            NoteContent::Shared(v) => super::serde::serialize_tagged("Shared", v, store, s),
        }
    }
}

#[cfg(feature = "serde")]
impl super::serde::DeserializeIn for NoteContent {
    fn deserialize_in<'de, D: ::serde::Deserializer<'de>>(
        store: &mut super::Store,
        d: D,
    ) -> Result<Self, D::Error> {
        super::serde::deserialize_either(
            store,
            d,
            &["Text", "Shared"],
            NoteContent::Text,
            NoteContent::Shared,
        )
    }
}

/// A shared note is written `SNOTE` in 7.x, `NOTE` in 5.5.1.
fn note_tag(note: &Note, tag: StdTag, cx: &WriteCx<'_>) -> StdTag {
    match note.content {
        NoteContent::Shared(_) if cx.version.is_v7() => const { StdTag::new("SNOTE") },
        NoteContent::Shared(_) => const { StdTag::new("NOTE") },
        NoteContent::Text(_) => tag,
    }
}

/// [`note_tag`] of a row.
fn note_tag_row(note: &NoteRow, tag: StdTag, cx: &WriteCx<'_>) -> StdTag {
    match note.content.as_pointer() {
        Some(_) if cx.version.is_v7() => const { StdTag::new("SNOTE") },
        Some(_) => const { StdTag::new("NOTE") },
        None => tag,
    }
}

gedcom_struct! {
    /// A note (`NOTE`), or a pointer to a shared note (5.5.1 `NOTE @N1@`,
    /// 7.x `SNOTE @N1@`).
    ///
    /// 7.x gives a note text a media type (`MIME`), a language (`LANG`),
    /// translations (`TRAN`) and citations (`SOUR`); 7.1 says what kind of
    /// note it is (`KIND`).
    pub struct Note => NoteRow [tag = note_tag, note_tag_row] [chain = citations: Citation] {
        @payload
        /// The note's text, or the shared note it points to.
        content: NoteContent;
        @detail
        /// What few notes have besides their text: a media type, a
        /// language, translations, sources and kinds.
        NoteDetail => NoteDetailRow {
            /// The media type of the text (`MIME`), `text/plain` when absent;
            /// besides `text/html`, another `text/` type is an extension that
            /// needs no extension tag (7.0.17).
            "MIME" => mime: Option<Text>,
            /// The language of the text (`LANG`).
            "LANG" => language: Option<Text>,
            /// Translations of the text (`TRAN`).
            "TRAN" => translations: Vec<NoteTranslation>,
            /// Sources of the note (`SOUR`).
            "SOUR" => citations: Vec<Citation>,
            /// What kind of note this is (7.1 `KIND`).
            "KIND" => kinds: Vec<Phrased<NoteKind>>,
        }
    }
    spec {
        v551: ["NOTE_STRUCTURE.NOTE", "NOTE_STRUCTURE.NOTE#2"],
        v70: ["NOTE", "SNOTE"],
        v71: ["NOTE", "SNOTE"],
    }
}

impl Note {
    /// A note with this text.
    #[must_use]
    pub fn text(text: impl Into<Text>) -> Self {
        Self {
            content: NoteContent::Text(text.into()),
            ..Self::default()
        }
    }

    /// A pointer to a shared note.
    #[must_use]
    pub fn shared(xref: XrefId) -> Self {
        Self {
            content: NoteContent::Shared(xref),
            ..Self::default()
        }
    }
}

gedcom_struct! {
    /// A translation of a note (7.x `NOTE.TRAN`): the text in another
    /// language or media type.
    pub struct NoteTranslation => NoteTranslationRow {
        @payload
        /// The translated text.
        text: Text;
        /// Its media type (`MIME`).
        "MIME" => mime: Option<Text>,
        /// Its language (`LANG`).
        "LANG" => language: Option<Text>,
    }
    spec {
        v551: [],
        v70: ["NOTE-TRAN"],
        v71: ["NOTE-TRAN"],
    }
}

gedcom_struct! {
    /// Text from a source (`TEXT`), with its media type and language (7.x).
    pub struct SourceText => SourceTextRow {
        @payload
        /// The text.
        text: Text;
        /// Its media type (`MIME`).
        "MIME" => mime: Option<Text>,
        /// Its language (`LANG`).
        "LANG" => language: Option<Text>,
    }
    spec {
        v551: [
            "SOURCE_CITATION.SOUR#2.TEXT",
            "SOURCE_CITATION.SOUR.DATA.TEXT",
            "SOURCE_RECORD.SOUR.TEXT",
        ],
        v70: ["TEXT"],
        v71: ["TEXT"],
    }
}

gedcom_struct! {
    /// A translation of a header's title or description (7.1 `TRAN` of
    /// `HEAD.TITL` or `HEAD.DESC`).
    pub struct TextTranslation => TextTranslationRow {
        @payload
        /// The translated text.
        text: Text;
        /// Its language (`LANG`).
        "LANG" => language: Option<Text>,
    }
    spec {
        v551: [],
        v70: [],
        v71: ["TEXT-TRAN"],
    }
}

impl super::table::sealed::Field for NoteContent {
    type View<'a> = Self;
}

impl<'a> super::View<'a, Note> {
    /// The note's own text; `None` for a pointer to a shared note.
    #[must_use]
    pub fn text(self) -> Option<super::TextRef<'a>> {
        (self.row.content.as_pointer().is_none())
            .then(|| super::TextRef::new(self.store(), self.row.content))
    }

    /// The shared note it points to; `None` for a note of its own.
    #[must_use]
    pub fn shared(self) -> Option<XrefId> {
        self.row.content.as_pointer()
    }
}
