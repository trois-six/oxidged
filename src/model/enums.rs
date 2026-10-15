//! Every enumeration of GEDCOM 5.5.1, 7.0 and 7.1, typed.
//!
//! An enumeration value is read without ever failing: a value the
//! enumeration names, in either version's spelling and in any case
//! (5.5.1 p. 21: controlled values are case-insensitive), becomes its
//! variant; anything else — an extension value (`_X`), a 5.5.1 user value
//! of an open set, a 7.x value read from a 5.5.1 file, a misspelling —
//! becomes `Unknown` with the text as written. A variant is written in the
//! target version's spelling (5.5.1 `adopted`, 7.x `ADOPTED`; 5.5.1
//! `DNS/CAN`, 7.x `DNS_CAN`), or in the other version's when its own has
//! none; `Unknown` is written as read. The writer's conformance pass then
//! repairs what the target does not permit (7.x `OTHER` and a `PHRASE`, or
//! an extension structure).
//!
//! The 7.x `PHRASE` that qualifies a value is kept by [`Phrased`], the
//! structure of an enumeration payload with its phrase.
//!
//! Languages are not an enumeration here: 7.x tags them in BCP 47, an open
//! grammar, and 5.5.1's `LANGUAGE_ID` names are kept as text too.

use crate::spec::schema::Kind;
use crate::tree::{Flat, FlatPayload, Payload, Structure};
use crate::version::GedcomVersion;

use super::driver::{NodeRef, PayloadField, ReadCx, WriteCx, gedcom_struct};
use super::table::{Code, CopyCx, List, PhrasedRow, StoreCx, Tables, push_span};
use super::text::Text;

/// One value of an enumeration: its variant name and its spellings in
/// 5.5.1 and in 7.x, when the version has it.
#[derive(Clone, Copy, Debug)]
pub struct EnumValue {
    /// The variant.
    pub variant: &'static str,
    /// The 5.5.1 spelling.
    pub v551: Option<&'static str>,
    /// The 7.x spelling.
    pub v7: Option<&'static str>,
}

/// An enumeration type: its values and the sets of the specification
/// tables it stands for.
#[derive(Clone, Copy, Debug)]
pub struct EnumDesc {
    /// The type's name.
    pub name: &'static str,
    /// The 5.5.1 enumeration sets it stands for.
    pub v551: &'static [&'static str],
    /// The 7.x enumeration sets it stands for.
    pub v7: &'static [&'static str],
    /// The values the type names.
    pub values: &'static [EnumValue],
}

/// A value an enumeration names, written in a version.
pub(crate) trait Enumeration: Sized + Default {
    /// The type's description.
    const DESC: EnumDesc;
    /// The variant `s` names, in either spelling and any case; with a
    /// version, only among the variants that version has.
    fn known(s: &str, version: Option<GedcomVersion>) -> Option<Self>;
    /// The spelling of a known value in `version`.
    fn spelling(&self, version: GedcomVersion) -> Option<&'static str>;
    /// The text of an unknown value.
    fn unknown(&self) -> Option<&Text>;
    /// An unknown value.
    fn from_unknown(text: Text) -> Self;
    /// The index of the variant `s` names ([`Enumeration::known`]).
    fn known_index(s: &str, version: Option<GedcomVersion>) -> Option<usize>;
    /// The variant at `index` of `DESC.values`.
    fn variant(index: usize) -> Option<Self>;
    /// The index of a known value in `DESC.values`.
    fn index(&self) -> Option<usize>;
}

/// An enumeration value of a row, owned: `E::default()` (an empty unknown
/// value) for none.
pub(crate) fn enum_from_code<E: Enumeration>(code: Option<Code>, t: &Tables) -> E {
    let Some(code) = code else {
        return E::default();
    };
    match (code.variant(), code.text_index()) {
        (Some(i), _) => E::variant(i).unwrap_or_default(),
        (None, Some(i)) => {
            E::from_unknown(Text::from_span(t.spans.get(i).copied().unwrap_or_default()))
        }
        (None, None) => E::default(),
    }
}

/// An owned enumeration value, for a row: an empty unknown value is none.
pub(crate) fn enum_to_code<E: Enumeration>(value: &E, cx: &mut StoreCx<'_>) -> Option<Code> {
    match (value.index(), value.unknown()) {
        (Some(i), _) => Some(Code::known(i)),
        (None, Some(t)) if !t.is_empty() => {
            let span = t.to_span(cx.store);
            Some(push_span(&mut cx.tables.spans, span))
        }
        _ => None,
    }
}

/// [`enum_to_code`] of a value that is there: an empty unknown value is the
/// code of an empty text.
pub(crate) fn enum_to_code_present<E: Enumeration>(value: &E, cx: &mut StoreCx<'_>) -> Code {
    enum_to_code(value, cx)
        .unwrap_or_else(|| push_span(&mut cx.tables.spans, super::text::Span::EMPTY))
}

/// The spelling in `version` of the value at `index` ([`Enumeration::spelling`]).
fn spelling_at<E: Enumeration>(index: usize, version: GedcomVersion) -> Option<&'static str> {
    let v = E::DESC.values.get(index)?;
    if version.is_v7() {
        v.v7.or(v.v551)
    } else {
        v.v551.or(v.v7)
    }
}

/// [`write_enum_flat`] of a row's value.
pub(crate) fn enum_payload_raw<'s, E: Enumeration>(
    code: Option<Code>,
    cx: &WriteCx<'s>,
) -> FlatPayload<'s> {
    let Some(code) = code else {
        return FlatPayload::None;
    };
    match (code.variant(), code.text_index()) {
        (Some(i), _) => {
            spelling_at::<E>(i, cx.version).map_or(FlatPayload::None, WriteCx::static_flat)
        }
        (None, Some(i)) => cx.span_flat(cx.tables.spans.get(i).copied().unwrap_or_default()),
        (None, None) => FlatPayload::None,
    }
}

/// [`claim_enum`] of a row's value.
pub(crate) fn enum_claim_raw<E: Enumeration>(code: Option<Code>, cx: &WriteCx<'_>) -> Option<Kind> {
    let i = code?.variant()?;
    spelling_at::<E>(i, cx.version).map(|_| Kind::Enum)
}

/// Writes an enumeration payload.
pub(crate) fn write_enum<E: Enumeration>(value: &E, cx: &WriteCx<'_>) -> Payload {
    match (value.spelling(cx.version), value.unknown()) {
        (Some(s), _) => WriteCx::str(s),
        (None, Some(t)) => cx.text(t),
        (None, None) => Payload::None,
    }
}

/// A known value spelled in the target version is one of its set's
/// values: trusted ([`PayloadField::claim`]).
pub(crate) fn claim_enum<E: Enumeration>(value: &E, cx: &WriteCx<'_>) -> Option<Kind> {
    value.spelling(cx.version).map(|_| Kind::Enum)
}

/// [`write_enum`], borrowed.
pub(crate) fn write_enum_flat<'s, E: Enumeration>(
    value: &'s E,
    cx: &WriteCx<'s>,
) -> FlatPayload<'s> {
    match (value.spelling(cx.version), value.unknown()) {
        (Some(s), _) => WriteCx::static_flat(s),
        (None, Some(t)) => cx.text_flat(t),
        (None, None) => FlatPayload::None,
    }
}

/// Declares an enumeration: `Variant = 5.5.1 spelling / 7.x spelling`, `_`
/// where the version has none.
macro_rules! gedcom_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident (v551 = [$($set551:literal),*], v7 = [$($set7:literal),*]) {
            $(
                $(#[$vmeta:meta])*
                $variant:ident = $s551:tt / $s7:tt,
            )*
        }
    ) => {
        $(#[$meta])*
        #[non_exhaustive]
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)*
            /// A value this enumeration does not name, kept as written.
            Unknown(Text),
        }

        impl Default for $name {
            /// An empty unknown value.
            fn default() -> Self {
                Self::Unknown(Text::default())
            }
        }

        impl $name {
            /// The variants, in the order of `DESC.values`.
            const KNOWN: &'static [Self] = &[$(Self::$variant,)*];

            /// The value `s` names: a variant when `s` spells one in
            /// 5.5.1 or 7.x, in any case and with surrounding spaces;
            /// otherwise `Unknown`, owning `s`.
            #[must_use]
            pub fn parse(s: &str) -> Self {
                <Self as Enumeration>::known(s, None).unwrap_or_else(|| Self::Unknown(Text::new(s)))
            }

            /// The spelling of a known value in `version`: its own, or the
            /// other version's when it has none. `None` for `Unknown`.
            #[must_use]
            pub fn as_str(&self, version: GedcomVersion) -> Option<&'static str> {
                <Self as Enumeration>::spelling(self, version)
            }

            /// Whether the value is one the enumeration names.
            #[must_use]
            pub fn is_known(&self) -> bool {
                !matches!(self, Self::Unknown(_))
            }
        }

        impl Enumeration for $name {
            const DESC: EnumDesc = EnumDesc {
                name: stringify!($name),
                v551: &[$($set551),*],
                v7: &[$($set7),*],
                values: &[$(EnumValue {
                    variant: stringify!($variant),
                    v551: gedcom_enum!(@opt $s551),
                    v7: gedcom_enum!(@opt $s7),
                },)*],
            };

            fn known(s: &str, version: Option<GedcomVersion>) -> Option<Self> {
                find_known(Self::DESC.values, s, version).and_then(|i| Self::KNOWN.get(i).cloned())
            }

            fn spelling(&self, version: GedcomVersion) -> Option<&'static str> {
                let (v551, v7): (Option<&'static str>, Option<&'static str>) = match self {
                    $(Self::$variant => (gedcom_enum!(@opt $s551), gedcom_enum!(@opt $s7)),)*
                    Self::Unknown(_) => return None,
                };
                if version.is_v7() {
                    v7.or(v551)
                } else {
                    v551.or(v7)
                }
            }

            fn unknown(&self) -> Option<&Text> {
                match self {
                    Self::Unknown(t) => Some(t),
                    _ => None,
                }
            }

            fn from_unknown(text: Text) -> Self {
                Self::Unknown(text)
            }

            fn known_index(s: &str, version: Option<GedcomVersion>) -> Option<usize> {
                find_known(Self::DESC.values, s, version)
            }

            fn variant(index: usize) -> Option<Self> {
                Self::KNOWN.get(index).cloned()
            }

            fn index(&self) -> Option<usize> {
                match self {
                    Self::Unknown(_) => None,
                    known => Self::KNOWN.iter().position(|k| k == known),
                }
            }
        }

        impl PayloadField for $name {

            fn write(&self, cx: &WriteCx<'_>) -> Payload {
                write_enum(self, cx)
            }

            fn payload<'s>(&'s self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
                write_enum_flat(self, cx)
            }

            fn claim(&self, cx: &WriteCx<'_>) -> Option<Kind> {
                claim_enum(self, cx)
            }

            type Raw = Option<Code>;
            type View<'a> = Self;

            #[inline]
            fn read_raw(node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> Option<Option<Code>> {
                cx.enum_code(node, <Self as Enumeration>::known_index).map(Some)
            }

            #[inline]
            fn payload_raw<'s>(raw: &Option<Code>, cx: &WriteCx<'s>) -> FlatPayload<'s> {
                enum_payload_raw::<Self>(*raw, cx)
            }

            #[inline]
            fn claim_raw(raw: &Option<Code>, cx: &WriteCx<'_>) -> Option<Kind> {
                enum_claim_raw::<Self>(*raw, cx)
            }

            fn load_raw(raw: &Option<Code>, t: &Tables) -> Self {
                enum_from_code(*raw, t)
            }

            fn store_raw(&self, cx: &mut StoreCx<'_>) -> Option<Code> {
                enum_to_code(self, cx)
            }

            fn copy_raw(raw: &Option<Code>, cx: &mut CopyCx<'_>) -> Option<Code> {
                raw.map(|c| cx.code(c))
            }

            fn view_raw(raw: Option<Code>, data: &$crate::model::Dataset) -> Self {
                enum_from_code(raw, &data.tables)
            }
        }

        $crate::model::driver::leaf!($name);
        $crate::model::driver::leaf_item!([] $name, Option<$crate::model::table::Code>, $name);

        impl $crate::model::driver::LeafOne for $name {
            type One = $crate::model::table::Code;

            #[inline]
            fn one(raw: Option<$crate::model::table::Code>) -> Option<$crate::model::table::Code> {
                raw
            }

            #[inline]
            fn raw(one: $crate::model::table::Code) -> Option<$crate::model::table::Code> {
                Some(one)
            }

            fn store_one(&self, cx: &mut StoreCx<'_>) -> $crate::model::table::Code {
                enum_to_code_present(self, cx)
            }
        }

        impl $crate::model::table::sealed::Field for $name {
            type View<'a> = Self;
        }

        impl $crate::model::pointers::Pointers for $name {}

        #[cfg(feature = "serde")]
        impl $crate::model::serde::SerializeIn for $name {
            fn serialize_in<S: ::serde::Serializer>(
                &self,
                store: &$crate::model::Store,
                s: S,
            ) -> Result<S::Ok, S::Error> {
                match self {
                    $(Self::$variant => s.serialize_str(stringify!($variant)),)*
                    Self::Unknown(t) => $crate::model::serde::serialize_unknown(t, store, s),
                }
            }
        }

        #[cfg(feature = "serde")]
        impl $crate::model::serde::DeserializeIn for $name {
            fn deserialize_in<'de, D: ::serde::Deserializer<'de>>(
                _store: &mut $crate::model::Store,
                d: D,
            ) -> Result<Self, D::Error> {
                $crate::model::serde::deserialize_enum(
                    d,
                    &[$(stringify!($variant)),*],
                    |name| match name {
                        $(stringify!($variant) => Some(Self::$variant),)*
                        _ => None,
                    },
                    Self::Unknown,
                )
            }
        }
    };
    (@opt _) => { None };
    (@opt $l:literal) => { Some($l) };
}

/// The index of the value `s` spells, in either version and any case,
/// with surrounding spaces. With a version, a value that version does not
/// have is not recognised: a 5.5.1 file's `Other` stays as written rather
/// than becoming 7.x's `OTHER`, which 5.5.1 could not write back.
fn find_known(values: &[EnumValue], s: &str, version: Option<GedcomVersion>) -> Option<usize> {
    // Spaces around a value are not the value; a line break is (a value
    // continued is no value of the set, and writes back as continued).
    let edge = |b: Option<&u8>| b.is_some_and(|b| !b.is_ascii_alphanumeric());
    let s = if edge(s.as_bytes().first()) || edge(s.as_bytes().last()) {
        s.trim_matches(|c: char| c.is_whitespace() && c != '\n')
    } else {
        s
    };
    values.iter().position(|v| {
        let own = match version {
            Some(version) if version.is_v7() => v.v7.is_some(),
            Some(_) => v.v551.is_some(),
            None => true,
        };
        own && (v.v551.is_some_and(|x| x.eq_ignore_ascii_case(s))
            || v.v7.is_some_and(|x| x.eq_ignore_ascii_case(s)))
    })
}

gedcom_enum! {
    /// Who adopted a child (`ADOP`): 5.5.1 `ADOPTED_BY_WHICH_PARENT`, 7.x
    /// `enumset-ADOP`.
    pub enum Adoption (v551 = ["ADOPTED_BY_WHICH_PARENT"], v7 = ["enumset-ADOP"]) {
        /// Adopted by the husband (`HUSB`).
        Husband = "HUSB" / "HUSB",
        /// Adopted by the wife (`WIFE`).
        Wife = "WIFE" / "WIFE",
        /// Adopted by both (`BOTH`).
        Both = "BOTH" / "BOTH",
    }
}

gedcom_enum! {
    /// The quality of a source's evidence (`QUAY`): 5.5.1
    /// `CERTAINTY_ASSESSMENT`, 7.x `enumset-QUAY`.
    pub enum Certainty (v551 = ["CERTAINTY_ASSESSMENT"], v7 = ["enumset-QUAY"]) {
        /// `0`: unreliable evidence or estimated data.
        Unreliable = "0" / "0",
        /// `1`: questionable reliability of evidence.
        Questionable = "1" / "1",
        /// `2`: secondary evidence.
        Secondary = "2" / "2",
        /// `3`: direct and primary evidence.
        Primary = "3" / "3",
    }
}

gedcom_enum! {
    /// The character set a 5.5.1 file declares (`HEAD.CHAR`); 7.x is
    /// always UTF-8 and has no such structure.
    pub enum CharacterSet (v551 = ["CHARACTER_SET"], v7 = []) {
        /// ANSEL (ANSI Z39.47).
        Ansel = "ANSEL" / _,
        /// UTF-8.
        Utf8 = "UTF-8" / _,
        /// UTF-16 (`UNICODE`).
        Unicode = "UNICODE" / _,
        /// ASCII.
        Ascii = "ASCII" / _,
    }
}

gedcom_enum! {
    /// How sure a child-to-family link is (`FAMC.STAT`): 5.5.1
    /// `CHILD_LINKAGE_STATUS`, 7.x `enumset-FAMC-STAT`.
    pub enum ChildStatus (v551 = ["CHILD_LINKAGE_STATUS"], v7 = ["enumset-FAMC-STAT"]) {
        /// Linking the child to the family is suspect.
        Challenged = "challenged" / "CHALLENGED",
        /// The link has been disproven.
        Disproven = "disproven" / "DISPROVEN",
        /// The link has been proven.
        Proven = "proven" / "PROVEN",
    }
}

gedcom_enum! {
    /// The form of a 5.5.1 file (`HEAD.GEDC.FORM`).
    pub enum GedcomForm (v551 = ["GEDCOM_FORM"], v7 = []) {
        /// `LINEAGE-LINKED`.
        LineageLinked = "LINEAGE-LINKED" / _,
    }
}

gedcom_enum! {
    /// The status of a Latter-day Saint ordinance (`STAT`): the 5.5.1
    /// `LDS_*_DATE_STATUS` sets together, 7.x `enumset-ord-STAT`.
    pub enum OrdinanceStatus (
            v551 = [
                "LDS_BAPTISM_DATE_STATUS",
                "LDS_CHILD_SEALING_DATE_STATUS",
                "LDS_ENDOWMENT_DATE_STATUS",
                "LDS_SPOUSE_SEALING_DATE_STATUS"
            ],
            v7 = ["enumset-ord-STAT"]
        ) {
        /// Born in the covenant.
        BornInCovenant = "BIC" / "BIC",
        /// The sealing was cancelled.
        Canceled = "CANCELED" / "CANCELED",
        /// Died before eight years old.
        Child = "CHILD" / "CHILD",
        /// Completed.
        Completed = "COMPLETED" / "COMPLETED",
        /// Do not seal.
        DoNotSeal = "DNS" / "DNS",
        /// Do not seal, previous sealing cancelled (5.5.1 `DNS/CAN`, 7.x
        /// `DNS_CAN`).
        DoNotSealCanceled = "DNS/CAN" / "DNS_CAN",
        /// Excluded from this ordinance.
        Excluded = "EXCLUDED" / "EXCLUDED",
        /// Died before less than one year old.
        Infant = "INFANT" / "INFANT",
        /// Completed before 1970 (5.5.1 `PRE-1970`, 7.x `PRE_1970`).
        Pre1970 = "PRE-1970" / "PRE_1970",
        /// Stillborn.
        Stillborn = "STILLBORN" / "STILLBORN",
        /// Submitted, not yet completed.
        Submitted = "SUBMITTED" / "SUBMITTED",
        /// Not cleared.
        Uncleared = "UNCLEARED" / "UNCLEARED",
    }
}

gedcom_enum! {
    /// The format of a 5.5.1 multimedia file (`FORM`); 7.x gives a media
    /// type instead, which [`FileForm`](super::FileForm) keeps as text.
    pub enum MultimediaFormat (v551 = ["MULTIMEDIA_FORMAT"], v7 = []) {
        /// Bitmap.
        Bmp = "bmp" / _,
        /// GIF.
        Gif = "gif" / _,
        /// JPEG.
        Jpg = "jpg" / _,
        /// OLE.
        Ole = "ole" / _,
        /// PCX.
        Pcx = "pcx" / _,
        /// TIFF.
        Tif = "tif" / _,
        /// WAV audio.
        Wav = "wav" / _,
    }
}

gedcom_enum! {
    /// The kind of a name (`NAME.TYPE`): 5.5.1 `NAME_TYPE` (an open set),
    /// 7.x `enumset-NAME-TYPE`.
    pub enum NameType (v551 = ["NAME_TYPE"], v7 = ["enumset-NAME-TYPE"]) {
        /// Also known as.
        Aka = "aka" / "AKA",
        /// Name given at or near birth.
        Birth = "birth" / "BIRTH",
        /// Name assumed at immigration.
        Immigrant = "immigrant" / "IMMIGRANT",
        /// Maiden name.
        Maiden = "maiden" / "MAIDEN",
        /// Name assumed at marriage.
        Married = "married" / "MARRIED",
        /// Name used professionally (7.x).
        Professional = _ / "PROFESSIONAL",
        /// Another kind, which a `PHRASE` describes (7.x).
        Other = _ / "OTHER",
    }
}

gedcom_enum! {
    /// Whether ordinances of a 5.5.1 submission are to be processed
    /// (`SUBN.ORDI`).
    pub enum OrdinanceFlag (v551 = ["ORDINANCE_PROCESS_FLAG"], v7 = []) {
        /// `yes`.
        Yes = "yes" / _,
        /// `no`.
        No = "no" / _,
    }
}

gedcom_enum! {
    /// How a child belongs to a family (`PEDI`): 5.5.1
    /// `PEDIGREE_LINKAGE_TYPE`, 7.x `enumset-PEDI`.
    pub enum Pedigree (v551 = ["PEDIGREE_LINKAGE_TYPE"], v7 = ["enumset-PEDI"]) {
        /// Adopted.
        Adopted = "adopted" / "ADOPTED",
        /// Birth parents.
        Birth = "birth" / "BIRTH",
        /// Foster child.
        Foster = "foster" / "FOSTER",
        /// Sealed to the family.
        Sealing = "sealing" / "SEALING",
        /// Another relationship, which a `PHRASE` describes (7.x).
        Other = _ / "OTHER",
    }
}

gedcom_enum! {
    /// The method of a 5.5.1 phonetic variation (`FONE.TYPE`), an open set.
    pub enum PhoneticType (v551 = ["PHONETIC_TYPE"], v7 = []) {
        /// Korean hangul.
        Hangul = "hangul" / _,
        /// Japanese kana.
        Kana = "kana" / _,
    }
}

gedcom_enum! {
    /// The method of a 5.5.1 romanized variation (`ROMN.TYPE`), an open
    /// set.
    pub enum RomanizedType (v551 = ["ROMANIZED_TYPE"], v7 = []) {
        /// Chinese pinyin.
        Pinyin = "pinyin" / _,
        /// Japanese romaji.
        Romaji = "romaji" / _,
        /// Chinese Wade-Giles.
        WadeGiles = "wadegiles" / _,
    }
}

gedcom_enum! {
    /// A restriction on a record or structure (`RESN`): 5.5.1
    /// `RESTRICTION_NOTICE`, 7.x `enumset-RESN` (a list of them).
    pub enum Restriction (v551 = ["RESTRICTION_NOTICE"], v7 = ["enumset-RESN"]) {
        /// Not to be distributed.
        Confidential = "confidential" / "CONFIDENTIAL",
        /// Not to be changed.
        Locked = "locked" / "LOCKED",
        /// Private: details removed.
        Privacy = "privacy" / "PRIVACY",
    }
}

gedcom_enum! {
    /// A role in an event or association (`ROLE`): 5.5.1 `ROLE_IN_EVENT`
    /// (an open set), 7.x `enumset-ROLE`.
    pub enum Role (v551 = ["ROLE_IN_EVENT"], v7 = ["enumset-ROLE"]) {
        /// Child.
        Child = "CHIL" / "CHIL",
        /// Religious official (7.x).
        Clergy = _ / "CLERGY",
        /// Father.
        Father = "FATH" / "FATH",
        /// Friend (7.x).
        Friend = _ / "FRIEND",
        /// Godparent (7.x).
        Godparent = _ / "GODP",
        /// Husband.
        Husband = "HUSB" / "HUSB",
        /// Mother.
        Mother = "MOTH" / "MOTH",
        /// Several roles (7.x).
        Multiple = _ / "MULTIPLE",
        /// Neighbour (7.x).
        Neighbor = _ / "NGHBR",
        /// Officiator (7.x).
        Officiator = _ / "OFFICIATOR",
        /// Another role, which a `PHRASE` describes (7.x).
        Other = _ / "OTHER",
        /// Parent (7.x).
        Parent = _ / "PARENT",
        /// Spouse.
        Spouse = "SPOU" / "SPOU",
        /// Wife.
        Wife = "WIFE" / "WIFE",
        /// Witness (7.x).
        Witness = _ / "WITN",
    }
}

gedcom_enum! {
    /// Sex (`SEX`): 5.5.1 `SEX_VALUE`, 7.x `enumset-SEX`.
    pub enum Sex (v551 = ["SEX_VALUE"], v7 = ["enumset-SEX"]) {
        /// Female (`F`).
        Female = "F" / "F",
        /// Male (`M`).
        Male = "M" / "M",
        /// Cannot be determined from the sources (`U`).
        Undetermined = "U" / "U",
        /// Neither only male nor only female (7.x `X`).
        Nonbinary = _ / "X",
    }
}

gedcom_enum! {
    /// The medium of a source (`MEDI`): 5.5.1 `SOURCE_MEDIA_TYPE`, 7.x
    /// `enumset-MEDI`.
    pub enum Medium (v551 = ["SOURCE_MEDIA_TYPE"], v7 = ["enumset-MEDI"]) {
        /// Audio recording.
        Audio = "audio" / "AUDIO",
        /// Bound book.
        Book = "book" / "BOOK",
        /// Card file.
        Card = "card" / "CARD",
        /// A digital artifact, such as a computer file or a scan.
        Electronic = "electronic" / "ELECTRONIC",
        /// Microfiche.
        Fiche = "fiche" / "FICHE",
        /// Microfilm.
        Film = "film" / "FILM",
        /// Magazine.
        Magazine = "magazine" / "MAGAZINE",
        /// Manuscript.
        Manuscript = "manuscript" / "MANUSCRIPT",
        /// Map.
        Map = "map" / "MAP",
        /// Newspaper.
        Newspaper = "newspaper" / "NEWSPAPER",
        /// Photo.
        Photo = "photo" / "PHOTO",
        /// Tombstone.
        Tombstone = "tombstone" / "TOMBSTONE",
        /// Video recording.
        Video = "video" / "VIDEO",
        /// Another medium, which a `PHRASE` describes (7.x).
        Other = _ / "OTHER",
    }
}

gedcom_enum! {
    /// An event or attribute type, by its tag: what a citation's `EVEN`
    /// or a source's `DATA.EVEN` names, and what `NO` denies (7.x
    /// `enumset-EVENATTR`, which includes `enumset-EVEN`). 5.5.1 writes the
    /// same tags as text (its `ATTRIBUTE_TYPE` set lists the attributes).
    pub enum EventKind (v551 = ["ATTRIBUTE_TYPE"], v7 = ["enumset-EVENATTR", "enumset-EVEN"]) {
        /// `ADOP`.
        Adoption = "ADOP" / "ADOP",
        /// `ANUL`.
        Annulment = "ANUL" / "ANUL",
        /// `BAPM`.
        Baptism = "BAPM" / "BAPM",
        /// `BARM`.
        BarMitzvah = "BARM" / "BARM",
        /// `BASM`.
        BasMitzvah = "BASM" / "BASM",
        /// `BIRT`.
        Birth = "BIRT" / "BIRT",
        /// `BLES`.
        Blessing = "BLES" / "BLES",
        /// `BURI`.
        Burial = "BURI" / "BURI",
        /// `CAST`.
        Caste = "CAST" / "CAST",
        /// `CENS`.
        Census = "CENS" / "CENS",
        /// `CHR`.
        Christening = "CHR" / "CHR",
        /// `CHRA`.
        AdultChristening = "CHRA" / "CHRA",
        /// `CONF`.
        Confirmation = "CONF" / "CONF",
        /// `CREM`.
        Cremation = "CREM" / "CREM",
        /// `DEAT`.
        Death = "DEAT" / "DEAT",
        /// `DIV`.
        Divorce = "DIV" / "DIV",
        /// `DIVF`.
        DivorceFiled = "DIVF" / "DIVF",
        /// `DSCR`.
        PhysicalDescription = "DSCR" / "DSCR",
        /// `EDUC`.
        Education = "EDUC" / "EDUC",
        /// `EMIG`.
        Emigration = "EMIG" / "EMIG",
        /// `ENGA`.
        Engagement = "ENGA" / "ENGA",
        /// `EVEN`.
        Event = "EVEN" / "EVEN",
        /// `FACT`.
        Fact = "FACT" / "FACT",
        /// `FCOM`.
        FirstCommunion = "FCOM" / "FCOM",
        /// `GRAD`.
        Graduation = "GRAD" / "GRAD",
        /// `IDNO`.
        IdNumber = "IDNO" / "IDNO",
        /// `IMMI`.
        Immigration = "IMMI" / "IMMI",
        /// `MARB`.
        MarriageBann = "MARB" / "MARB",
        /// `MARC`.
        MarriageContract = "MARC" / "MARC",
        /// `MARL`.
        MarriageLicense = "MARL" / "MARL",
        /// `MARR`.
        Marriage = "MARR" / "MARR",
        /// `MARS`.
        MarriageSettlement = "MARS" / "MARS",
        /// `NATI`.
        Nationality = "NATI" / "NATI",
        /// `NATU`.
        Naturalization = "NATU" / "NATU",
        /// `NCHI`.
        ChildrenCount = "NCHI" / "NCHI",
        /// `NMR`.
        MarriageCount = "NMR" / "NMR",
        /// `OCCU`.
        Occupation = "OCCU" / "OCCU",
        /// `ORDN`.
        Ordination = "ORDN" / "ORDN",
        /// `PROB`.
        Probate = "PROB" / "PROB",
        /// `PROP`.
        Property = "PROP" / "PROP",
        /// `RELI`.
        Religion = "RELI" / "RELI",
        /// `RESI`.
        Residence = "RESI" / "RESI",
        /// `RETI`.
        Retirement = "RETI" / "RETI",
        /// `SSN`.
        SocialSecurityNumber = "SSN" / "SSN",
        /// `TITL`.
        Title = "TITL" / "TITL",
        /// `WILL`.
        Will = "WILL" / "WILL",
    }
}

gedcom_enum! {
    /// Whether a child was born alive (7.1 `BIRT.KIND`, `enumset-BIRT-KIND`).
    pub enum BirthKind (v551 = [], v7 = ["enumset-BIRT-KIND"]) {
        /// Born dead.
        BornDead = _ / "BORN_DEAD",
        /// Born alive.
        BornLive = _ / "BORN_LIVE",
    }
}

gedcom_enum! {
    /// What a note is about (7.1 `NOTE.KIND`, `enumset-NOTE-KIND`).
    pub enum NoteKind (v551 = [], v7 = ["enumset-NOTE-KIND"]) {
        /// Data transcribed or abstracted.
        Data = _ / "DATA",
        /// Another kind, which a `PHRASE` describes.
        Other = _ / "OTHER",
        /// Reasoning behind a conclusion.
        Reasoning = _ / "REASONING",
        /// Research notes.
        Research = _ / "RESEARCH",
        /// The scope of a work.
        Scope = _ / "SCOPE",
        /// A story.
        Story = _ / "STORY",
        /// Work to do.
        Todo = _ / "TODO",
    }
}

/// An event or attribute structure keeps its tag as its kind.
impl super::driver::TagField for EventKind {
    fn from_tag(tag: &'static str) -> Option<Self> {
        Self::known(tag, None)
    }

    fn tag(&self) -> &'static str {
        // A kind read from a tag is known; one a program left unknown is
        // written as a generic event.
        self.as_str(GedcomVersion::V7_0).unwrap_or("EVEN")
    }

    type Raw = Option<Code>;

    fn raw_of_tag(tag: &'static str) -> Option<Option<Code>> {
        /// By the index of a standard tag, the variant (plus one) whose
        /// 5.5.1 or 7.x spelling it is, the first: what
        /// [`Enumeration::known_index`] finds of the tag, found without a
        /// search.
        #[allow(clippy::cast_possible_truncation)] // Fewer variants than a u16 counts: asserted.
        const BY_TAG: [u16; crate::tree::STANDARD_TAGS.len()] = {
            let values = EventKind::DESC.values;
            assert!(values.len() < u16::MAX as usize);
            let mut by_tag = [0_u16; crate::tree::STANDARD_TAGS.len()];
            let mut i = 0;
            while i < values.len() {
                let spellings = [values[i].v551, values[i].v7];
                let mut k = 0;
                while k < spellings.len() {
                    if let Some(s) = spellings[k]
                        && let Some(t) = crate::tree::standard_index_const(s)
                        && by_tag[t as usize] == 0
                    {
                        by_tag[t as usize] = i as u16 + 1;
                    }
                    k += 1;
                }
                i += 1;
            }
            by_tag
        };
        let at = crate::tree::standard_index(tag)?;
        let variant = BY_TAG.get(usize::from(at)).copied().filter(|&v| v > 0)?;
        Some(Some(Code::known(usize::from(variant - 1))))
    }

    fn std_tag_raw(raw: &Option<Code>, _t: &Tables) -> super::driver::StdTag {
        /// The tag of each variant, and of a generic event.
        const TAGS: [super::driver::StdTag; EventKind::DESC.values.len() + 1] = {
            let values = EventKind::DESC.values;
            let mut tags = [super::driver::StdTag::new("EVEN"); EventKind::DESC.values.len() + 1];
            let mut i = 0;
            while i < values.len() {
                tags[i] = super::driver::StdTag::new(match (values[i].v7, values[i].v551) {
                    (Some(t), _) | (None, Some(t)) => t,
                    (None, None) => "EVEN",
                });
                i += 1;
            }
            tags
        };
        let i = raw.and_then(Code::variant).unwrap_or(TAGS.len() - 1);
        TAGS.get(i)
            .copied()
            .unwrap_or(const { super::driver::StdTag::new("EVEN") })
    }

    fn load_raw(raw: &Option<Code>, t: &Tables) -> Self {
        enum_from_code(*raw, t)
    }

    fn store_raw(&self, cx: &mut StoreCx<'_>) -> Option<Code> {
        enum_to_code(self, cx)
    }

    fn copy_raw(raw: &Option<Code>, cx: &mut CopyCx<'_>) -> Option<Code> {
        raw.map(|c| cx.code(c))
    }
}

/// Every enumeration type, for the coverage ledger.
pub(crate) const ENUMS: &[EnumDesc] = &[
    Adoption::DESC,
    BirthKind::DESC,
    Certainty::DESC,
    CharacterSet::DESC,
    ChildStatus::DESC,
    EventKind::DESC,
    GedcomForm::DESC,
    Medium::DESC,
    MultimediaFormat::DESC,
    NameType::DESC,
    NoteKind::DESC,
    OrdinanceFlag::DESC,
    OrdinanceStatus::DESC,
    Pedigree::DESC,
    PhoneticType::DESC,
    Restriction::DESC,
    Role::DESC,
    RomanizedType::DESC,
    Sex::DESC,
];

/// A comma-separated list of enumeration values (`RESN CONFIDENTIAL,
/// LOCKED`): each item typed, written `A, B`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnumList<E>(pub Vec<E>);

impl<E: Enumeration> PayloadField for EnumList<E> {
    fn write(&self, cx: &WriteCx<'_>) -> Payload {
        // An item as it reads back: without the characters the target
        // bans (which the writer leaves out), then read as reading does
        // (`read`): a known value once they are gone (`CHR` and a
        // control character) in the target's spelling, else the text
        // without the spaces around it.
        let rules = cx.version.rules();
        let items: Vec<String> = self
            .0
            .iter()
            .map(|e| match (e.spelling(cx.version), e.unknown()) {
                (Some(s), _) => s.to_string(),
                (None, Some(t)) => {
                    let kept: String = t
                        .to_str(cx.store)
                        .chars()
                        .filter(|&c| !rules.is_banned(c))
                        .collect();
                    match E::known(&kept, Some(cx.version)).and_then(|e| e.spelling(cx.version)) {
                        Some(s) => s.to_string(),
                        None => kept.trim_matches(' ').to_string(),
                    }
                }
                (None, None) => String::new(),
            })
            .collect();
        WriteCx::str(&items.join(", "))
    }

    fn claim(&self, cx: &WriteCx<'_>) -> Option<Kind> {
        // Known values, spelled in the target version and joined by `, `.
        self.0
            .iter()
            .all(|e| e.spelling(cx.version).is_some())
            .then_some(Kind::ListEnum)
    }

    fn payload<'s>(&'s self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        // One known value is its static spelling; a list is joined.
        match self.0.as_slice() {
            [] => FlatPayload::None,
            [e] if e.spelling(cx.version).is_some() => write_enum_flat(e, cx),
            _ => self.write(cx).into(),
        }
    }

    type Raw = List<Code>;
    type View<'a> = Self;

    fn read_raw(node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> Option<List<Code>> {
        if node.is_pointer() {
            return None;
        }
        if node.has_no_payload() {
            return Some(List::EMPTY);
        }
        let payload = cx.payload_str(node);
        let count = payload.split(',').count();
        let start = super::table::reserve(&mut cx.tables.codes, count, None);
        for (i, item) in payload.split(',').enumerate() {
            let code = if let Some(k) = E::known_index(item, Some(cx.version)) {
                Code::known(k)
            } else {
                let span = cx.sub_span(node, item.trim_matches(' '));
                cx.unknown_code(span)
            };
            if let Some(slot) = cx.tables.codes.get_mut(start as usize + i) {
                *slot = Some(code);
            }
        }
        Some(List::new(start, u32::try_from(count).unwrap_or(u32::MAX)))
    }

    fn payload_raw<'s>(raw: &List<Code>, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        match raw.of(&cx.tables.codes) {
            [] => FlatPayload::None,
            [code] if enum_claim_raw::<E>(*code, cx).is_some() => enum_payload_raw::<E>(*code, cx),
            _ => Self::load_raw(raw, cx.tables).write(cx).into(),
        }
    }

    fn claim_raw(raw: &List<Code>, cx: &WriteCx<'_>) -> Option<Kind> {
        raw.of(&cx.tables.codes)
            .iter()
            .all(|c| enum_claim_raw::<E>(*c, cx).is_some())
            .then_some(Kind::ListEnum)
    }

    fn load_raw(raw: &List<Code>, t: &Tables) -> Self {
        Self(
            raw.of(&t.codes)
                .iter()
                .map(|c| enum_from_code(*c, t))
                .collect(),
        )
    }

    fn store_raw(&self, cx: &mut StoreCx<'_>) -> List<Code> {
        if self.0.is_empty() {
            return List::EMPTY;
        }
        let start = super::table::reserve(&mut cx.tables.codes, self.0.len(), None);
        for (i, e) in self.0.iter().enumerate() {
            let code = enum_to_code(e, cx);
            if let Some(slot) = cx.tables.codes.get_mut(start as usize + i) {
                *slot = code;
            }
        }
        List::new(start, u32::try_from(self.0.len()).unwrap_or(u32::MAX))
    }

    fn copy_raw(raw: &List<Code>, cx: &mut CopyCx<'_>) -> List<Code> {
        if raw.is_empty() {
            return List::EMPTY;
        }
        let start = super::table::reserve(&mut cx.to.codes, raw.len as usize, None);
        for i in 0..raw.len {
            let code = cx
                .from
                .codes
                .get((raw.start + i) as usize)
                .copied()
                .flatten();
            let copied = code.map(|c| cx.code(c));
            if let Some(slot) = cx.to.codes.get_mut((start + i) as usize) {
                *slot = copied;
            }
        }
        List::new(start, raw.len)
    }

    fn view_raw(raw: List<Code>, data: &super::Dataset) -> Self {
        Self::load_raw(&raw, &data.tables)
    }
}

super::driver::leaf_item!([E: Enumeration] EnumList<E>, List<Code>, EnumList<E>);
super::driver::leaf_one_same!([E: Enumeration] EnumList<E>);

impl<E> super::table::sealed::Field for EnumList<E> {
    type View<'a> = Self;
}

impl<E: Enumeration> super::driver::ToNodes for EnumList<E> {
    const LEAF: bool = true;

    fn to_node(&self, tag: &'static str, cx: &WriteCx<'_>) -> Structure {
        super::driver::write_leaf(self, tag, cx)
    }

    fn to_flat<'s>(&'s self, tag: super::driver::StdTag, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        super::driver::write_leaf_flat(self, tag, cx, out);
    }
}

/// The structure of an enumeration value with the 7.x `PHRASE` that words
/// it (`PEDI OTHER` + `PHRASE Guardianship`): `PEDI`, `FAMC.STAT`,
/// `ADOP`, `ROLE`, `NAME.TYPE`, `MEDI`, 7.1 `NOTE.KIND`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Phrased<E> {
    /// The value.
    pub value: E,
    /// The value in words (7.x).
    pub phrase: Option<Text>,
    /// Substructures no field holds.
    pub extra: super::Extra,
}

/// The fields of [`Phrased`]: `PHRASE`.
pub(crate) const PHRASED_FIELDS: &[super::driver::FieldDesc] = &[super::driver::FieldDesc {
    tags: &["PHRASE"],
    name: "phrase",
    many: false,
    leaf: true,
}];

/// The field of each standard tag in [`Phrased`].
static PHRASED_LOOKUP: super::driver::Lookup = super::driver::lookup(PHRASED_FIELDS);

impl<E: Enumeration + Clone + PartialEq + std::fmt::Debug> super::driver::Struct for Phrased<E> {
    const FIELDS: &'static [super::driver::FieldDesc] = PHRASED_FIELDS;
    const LOOKUP: super::driver::Lookup = super::driver::lookup(PHRASED_FIELDS);
    const SPEC: super::driver::SpecNames = super::driver::SpecNames {
        v551: &[],
        v70: &[],
        v71: &[],
    };
}

impl<E: Enumeration> super::driver::Fields for Phrased<E> {
    fn fields(&self) -> &'static [super::driver::FieldDesc] {
        PHRASED_FIELDS
    }

    fn lookup(&self) -> &'static super::driver::Lookup {
        &PHRASED_LOOKUP
    }

    fn write_xref(&self, _cx: &WriteCx<'_>) -> Option<crate::tree::Xref> {
        None
    }

    fn walk(&self, _f: &mut dyn FnMut(&'static str, &dyn super::driver::Fields)) {}

    fn write_extra(
        &self,
        _field: usize,
        _node: &super::Node,
        _tag: &'static str,
        _cx: &WriteCx<'_>,
    ) -> Option<Structure> {
        None
    }

    fn extra(&self) -> &super::Extra {
        &self.extra
    }

    fn write_payload(&self, cx: &WriteCx<'_>) -> Payload {
        write_enum(&self.value, cx)
    }

    fn write_fields(&self, cx: &WriteCx<'_>, out: &mut Vec<Structure>) {
        super::driver::Slot::write(&self.phrase, "PHRASE", cx, out);
    }

    fn write_payload_flat<'s>(&'s self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        write_enum_flat(&self.value, cx)
    }

    fn payload_claim(&self, cx: &WriteCx<'_>) -> Option<Kind> {
        claim_enum(&self.value, cx)
    }

    fn write_fields_flat<'s>(&'s self, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        super::driver::Slot::to_flat(
            &self.phrase,
            const { super::driver::StdTag::new("PHRASE") },
            cx,
            out,
        );
    }
}

impl<E: Enumeration> super::driver::ToNodes for Phrased<E> {
    fn to_node(&self, tag: &'static str, cx: &WriteCx<'_>) -> Structure {
        super::driver::write(self, tag, cx)
    }

    fn to_flat<'s>(&'s self, tag: super::driver::StdTag, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        super::driver::write_flat(self, tag, cx, out, None);
    }

    fn visit(
        &self,
        tag: &'static str,
        f: &mut dyn FnMut(&'static str, &dyn super::driver::Fields),
    ) {
        f(tag, self);
    }
}

/// Enumeration values hold no pointer.
impl<E> super::pointers::Pointers for EnumList<E> {}

/// A value with its phrase: the pointers of its extensions.
impl<E> super::pointers::Pointers for Phrased<E> {
    fn each(&self, f: &mut dyn FnMut(super::XrefId)) {
        self.extra.each(f);
    }

    fn map(&mut self, f: &mut dyn FnMut(super::XrefId) -> super::XrefId) {
        self.extra.map(f);
    }

    fn retain(&mut self, cut: &mut super::pointers::Cut<'_>) {
        self.extra.retain(cut);
    }
}

#[cfg(feature = "serde")]
impl<E: super::serde::SerializeIn> super::serde::SerializeIn for EnumList<E> {
    fn serialize_in<S: ::serde::Serializer>(
        &self,
        store: &super::Store,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        self.0.serialize_in(store, s)
    }
}

#[cfg(feature = "serde")]
impl<E: super::serde::DeserializeIn> super::serde::DeserializeIn for EnumList<E> {
    fn deserialize_in<'de, D: ::serde::Deserializer<'de>>(
        store: &mut super::Store,
        d: D,
    ) -> Result<Self, D::Error> {
        Vec::deserialize_in(store, d).map(Self)
    }
}

#[cfg(feature = "serde")]
impl<E: super::serde::SerializeIn> super::serde::SerializeIn for Phrased<E> {
    fn serialize_in<S: ::serde::Serializer>(
        &self,
        store: &super::Store,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        use super::serde::In;
        use ::serde::ser::SerializeMap as _;
        let count = 1 + usize::from(self.phrase.is_some()) + usize::from(!self.extra.is_empty());
        let mut map = s.serialize_map(Some(count))?;
        map.serialize_entry("value", &In(store, &self.value))?;
        if let Some(phrase) = &self.phrase {
            map.serialize_entry("phrase", &In(store, phrase))?;
        }
        if !self.extra.is_empty() {
            map.serialize_entry("extra", &In(store, &self.extra))?;
        }
        map.end()
    }
}

#[cfg(feature = "serde")]
impl<E: super::serde::DeserializeIn + Default> super::serde::DeserializeIn for Phrased<E> {
    fn deserialize_in<'de, D: ::serde::Deserializer<'de>>(
        store: &mut super::Store,
        d: D,
    ) -> Result<Self, D::Error> {
        /// The visitor of the fields.
        struct V<'s, E>(&'s mut super::Store, std::marker::PhantomData<E>);
        impl<'de, E: super::serde::DeserializeIn + Default> ::serde::de::Visitor<'de> for V<'_, E> {
            type Value = Phrased<E>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an enumeration value and its phrase")
            }
            fn visit_map<A: ::serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Phrased<E>, A::Error> {
                use super::serde::Seed;
                let store = self.0;
                let mut out = Phrased::new(E::default());
                while let Some(key) = map.next_key::<super::serde::Str<'de>>()? {
                    match &*key.0 {
                        "value" => out.value = map.next_value_seed(Seed::new(&mut *store))?,
                        "phrase" => out.phrase = map.next_value_seed(Seed::new(&mut *store))?,
                        "extra" => out.extra = map.next_value_seed(Seed::new(&mut *store))?,
                        _ => super::serde::skip(&mut map)?,
                    }
                }
                Ok(out)
            }
        }
        d.deserialize_map(V(store, std::marker::PhantomData))
    }
}

impl<E> Phrased<E> {
    /// A value without a phrase.
    pub fn new(value: E) -> Self {
        Self {
            value,
            phrase: None,
            extra: super::Extra::default(),
        }
    }
}

gedcom_struct! {
    /// The status of a Latter-day Saint ordinance (`STAT`) and the date it
    /// was set (5.5.1 `LDS_*.STAT`, 7.x `ord-STAT`).
    pub struct LdsStatus => LdsStatusRow {
        @payload
        /// The status.
        value: OrdinanceStatus;
        /// When the status was set (`DATE`, with an optional `TIME`).
        "DATE" => date: Option<super::ExactDate>,
    }
    spec {
        v551: [
            "LDS_INDIVIDUAL_ORDINANCE.BAPL.STAT",
            "LDS_INDIVIDUAL_ORDINANCE.ENDL.STAT",
            "LDS_INDIVIDUAL_ORDINANCE.SLGC.STAT",
            "LDS_SPOUSE_SEALING.SLGS.STAT",
        ],
        v70: ["ord-STAT"],
        v71: ["ord-STAT"],
    }
}

// ---- Values with their phrases in a dataset's tables --------------------

impl<E> super::table::sealed::Stored for Phrased<E> {
    type Row = PhrasedRow;

    fn empty() -> &'static PhrasedRow {
        static EMPTY: PhrasedRow = PhrasedRow {
            value: None,
            phrase: None,
        };
        &EMPTY
    }
}

impl<E> super::table::HasTable for Phrased<E> {
    fn table(t: &Tables) -> &super::table::Table<PhrasedRow> {
        &t.phrased
    }
    fn table_mut(t: &mut Tables) -> &mut super::table::Table<PhrasedRow> {
        &mut t.phrased
    }
}

/// A value with its phrase being read into a row.
struct PhrasedBuilding<E> {
    row: PhrasedRow,
    value: std::marker::PhantomData<fn() -> E>,
}

impl<E: Enumeration> super::driver::RowFields for PhrasedBuilding<E> {
    fn fields(&self) -> &'static [super::driver::FieldDesc] {
        PHRASED_FIELDS
    }

    fn lookup(&self) -> &'static super::driver::Lookup {
        &PHRASED_LOOKUP
    }

    fn read_tag(&mut self, _node: NodeRef<'_>) -> bool {
        true
    }

    fn read_xref(&mut self, node: NodeRef<'_>, _cx: &mut ReadCx<'_>) -> bool {
        !node.has_xref()
    }

    fn read_payload(&mut self, node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> bool {
        match cx.enum_code(node, E::known_index) {
            Some(code) => {
                self.row.value = Some(code);
                true
            }
            None => false,
        }
    }

    fn reserve(&mut self, _field: usize, _count: u32, _cx: &mut ReadCx<'_>) {}

    fn accept(
        &mut self,
        field: usize,
        node: NodeRef<'_>,
        reserved: u32,
        cx: &mut ReadCx<'_>,
    ) -> bool {
        field == 0
            && <Option<Text> as super::table::RowSlot>::accept(
                &mut self.row.phrase,
                node,
                reserved,
                cx,
            )
    }

    fn finish(&mut self, _field: usize, _reserved: u32, _cx: &mut ReadCx<'_>) {}
}

/// What writing a value with its phrase needs: the row and its type.
struct PhrasedWrite<'s, E> {
    row: &'s PhrasedRow,
    value: std::marker::PhantomData<fn() -> E>,
}

impl<E: Enumeration + 'static> super::driver::RowWrite for PhrasedWrite<'_, E> {
    fn as_dyn(&self) -> &dyn super::driver::RowWrite {
        self
    }

    fn write_xref<'s>(&self, _cx: &WriteCx<'s>) -> Option<std::borrow::Cow<'s, str>> {
        None
    }

    fn write_payload<'s>(&self, cx: &WriteCx<'s>) -> FlatPayload<'s> {
        enum_payload_raw::<E>(self.row.value, cx)
    }

    fn payload_claim(&self, cx: &WriteCx<'_>) -> Option<Kind> {
        enum_claim_raw::<E>(self.row.value, cx)
    }

    fn write_fields<'s>(&self, cx: &WriteCx<'s>, out: &mut Flat<'s>) {
        <Option<Text> as super::table::RowSlot>::write(
            &self.row.phrase,
            const { super::driver::StdTag::new("PHRASE") },
            cx,
            out,
        );
    }

    fn lookup(&self) -> &'static super::driver::Lookup {
        &PHRASED_LOOKUP
    }

    fn write_extra(
        &self,
        _field: usize,
        _node: &super::table::NodeRow,
        _tag: &'static str,
        _cx: &WriteCx<'_>,
    ) -> Option<Structure> {
        None
    }

    fn field_descs(&self) -> &'static [super::driver::FieldDesc] {
        &[]
    }

    fn write_fields_with<'s>(
        &self,
        cx: &WriteCx<'s>,
        out: &mut Flat<'s>,
        after: &mut dyn FnMut(usize, &mut Flat<'s>),
    ) {
        self.write_fields(cx, out);
        after(0, out);
    }

    fn field_texts(
        &self,
        _field: usize,
        _t: &Tables,
        _f: &mut dyn FnMut(super::text::Span),
    ) -> Option<usize> {
        None
    }

    fn struct_type(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Phrased<E>>()
    }

    fn holds(&self, _t: &Tables, f: &mut dyn FnMut(usize)) {
        if self.row.phrase.is_some() {
            f(0);
        }
    }
}

super::table::listed!([E: Enumeration + 'static] Phrased<E>, PhrasedRow, super::table::View<'a, Phrased<E>>);

impl<E: Enumeration + 'static> super::table::Item for Phrased<E> {
    type One = super::table::Idx<Phrased<E>>;

    fn elems(t: &Tables) -> &[PhrasedRow] {
        &t.phrased.rows
    }

    fn elems_mut(t: &mut Tables) -> &mut Vec<PhrasedRow> {
        &mut t.phrased.rows
    }

    fn fill() -> PhrasedRow {
        PhrasedRow::default()
    }

    fn read_elem(node: NodeRef<'_>, at: u32, cx: &mut ReadCx<'_>) -> Option<PhrasedRow> {
        let mut b = PhrasedBuilding::<E> {
            row: PhrasedRow::default(),
            value: std::marker::PhantomData,
        };
        let extra = super::driver::read_row_into(&mut b, node, cx)?;
        if !extra.is_empty() {
            cx.tables.phrased.set_extra(at, extra);
        }
        Some(b.row)
    }

    fn read_one(node: NodeRef<'_>, cx: &mut ReadCx<'_>) -> Option<Self::One> {
        super::table::read_one_row::<Self>(node, cx)
    }

    fn write_elem<'s>(
        e: &PhrasedRow,
        at: u32,
        tag: super::driver::StdTag,
        cx: &WriteCx<'s>,
        out: &mut Flat<'s>,
    ) {
        let w = PhrasedWrite::<E> {
            row: e,
            value: std::marker::PhantomData,
        };
        let extra = cx.tables.phrased.extra(at);
        super::driver::write_row_flat(&w, extra, tag, cx, out, None);
    }

    fn write_one<'s>(
        o: &Self::One,
        tag: super::driver::StdTag,
        cx: &WriteCx<'s>,
        out: &mut Flat<'s>,
    ) {
        if let Some(e) = cx.tables.phrased.rows.get(o.get() as usize) {
            Self::write_elem(e, o.get(), tag, cx, out);
        }
    }

    fn load_elem(e: &PhrasedRow, at: u32, t: &Tables) -> Self {
        Self {
            value: enum_from_code(e.value, t),
            phrase: e.phrase.map(Text::from_span),
            extra: super::node::load_extra(t.phrased.extra(at), t),
        }
    }

    fn load_one(o: Self::One, t: &Tables) -> Self {
        t.phrased.rows.get(o.get() as usize).map_or_else(
            || Phrased::new(E::default()),
            |e| Self::load_elem(e, o.get(), t),
        )
    }

    fn store_elem(&self, at: u32, cx: &mut StoreCx<'_>) -> PhrasedRow {
        let row = PhrasedRow {
            value: enum_to_code(&self.value, cx),
            phrase: self.phrase.as_ref().map(|p| p.to_span(cx.store)),
        };
        let extra = super::node::store_extra(&self.extra, cx);
        if !extra.is_empty() {
            cx.tables.phrased.set_extra(at, extra);
        }
        row
    }

    fn store_one(&self, cx: &mut StoreCx<'_>) -> Self::One {
        super::table::store_one_row(self, cx)
    }

    fn each_elem(_e: &PhrasedRow, at: u32, t: &Tables, f: &mut dyn FnMut(super::XrefId)) {
        super::node::each_extra(t.phrased.extra(at), t, f);
    }

    fn each_one(o: Self::One, t: &Tables, f: &mut dyn FnMut(super::XrefId)) {
        super::node::each_extra(t.phrased.extra(o.get()), t, f);
    }

    fn copy_elem(e: &PhrasedRow, at: u32, cx: &mut CopyCx<'_>, to: u32) -> PhrasedRow {
        let row = PhrasedRow {
            value: e.value.map(|c| cx.code(c)),
            phrase: cx.opt_span(e.phrase),
        };
        let extra = cx.from.phrased.extra(at);
        if !extra.is_empty() {
            let copied = super::node::copy_extra(extra, cx);
            cx.to.phrased.set_extra(to, copied);
        }
        row
    }

    fn copy_one(o: Self::One, cx: &mut CopyCx<'_>) -> Self::One {
        super::table::copy_one_row::<Self>(o, cx)
    }

    fn view_elem<'a>(e: &'a PhrasedRow, at: u32, data: &'a super::Dataset) -> Self::View<'a> {
        super::table::View::new(data, e, at)
    }

    fn view_one(o: Self::One, data: &super::Dataset) -> Self::View<'_> {
        super::table::view_one_row::<Self>(o, data)
    }
}

/// The views of a value with its phrase, for each enumeration.
macro_rules! phrased_view {
    ($($e:ty),* $(,)?) => {$(
        impl<'a> super::table::View<'a, Phrased<$e>> {
            /// The value.
            #[must_use]
            pub fn value(self) -> $e {
                enum_from_code(self.row.value, self.tables())
            }

            /// The value in words (7.x).
            #[must_use]
            pub fn phrase(self) -> Option<super::TextRef<'a>> {
                self.row
                    .phrase
                    .map(|p| super::TextRef::new(self.store(), p))
            }

            /// The substructures it keeps untyped.
            #[must_use]
            pub fn extra(self) -> super::Items<'a, super::Node> {
                let list = self.tables().phrased.extra(self.at);
                super::Items::new(self.dataset(), list.of(&self.tables().nodes), list.start)
            }
        }

        impl std::fmt::Debug for super::table::View<'_, Phrased<$e>> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("Phrased")
                    .field("value", &self.value())
                    .field("phrase", &self.phrase())
                    .finish()
            }
        }
    )*};
}

phrased_view!(
    Pedigree,
    ChildStatus,
    Adoption,
    Role,
    NameType,
    Medium,
    NoteKind
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::driver::TagField;

    #[test]
    fn event_kinds_are_found_by_tag_as_by_spelling() {
        for tag in crate::tree::STANDARD_TAGS {
            let searched = EventKind::known_index(tag, None).map(|i| Some(Code::known(i)));
            assert_eq!(EventKind::raw_of_tag(tag), searched, "{tag}");
        }
    }
}
