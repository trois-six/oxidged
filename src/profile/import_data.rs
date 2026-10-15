//! The import profiles. Each rule cites the rows of the compatibility
//! catalogue (`tests/profiles/catalogue.tsv`) it implements; a test checks
//! that every cited row exists and that every lossless row with a 7.0
//! target is implemented here, by the reader, or listed as not yet.

use crate::encoding::GedcomEncoding::{Iso8859_1, MacRoman, Windows1252};

use super::detect::Gate;
use super::dsl::{import_profile, rule, sig};
use super::interpret::{CropForm, Label, NameKind, NameValue, Op};
use super::{FAMILYSEARCH_PERSON, Profile};

/// The events and attributes of persons and families, as a rule context
/// (`INDI|FAM.BIRT|…`), followed by `$rest`: where event details
/// (associations, contacts, privacy) belong.
macro_rules! events {
    ($rest:literal) => {
        concat!(
            "INDI|FAM.ADOP|ANUL|BAPM|BARM|BASM|BIRT|BLES|BURI|CENS|CHR|CHRA|CONF|CREM|DEAT|DIV|",
            "DIVF|EMIG|ENGA|EVEN|FCOM|GRAD|IMMI|MARB|MARC|MARL|MARR|MARS|NATU|ORDN|PROB|RETI|",
            "WILL|RESI|OCCU|EDUC|FACT|CAST|DSCR|IDNO|NATI|NCHI|NMR|PROP|RELI|SSN|TITL",
            $rest
        )
    };
}

/// Every import profile: the generic ones first.
pub(crate) static PROFILES: &[&Profile] = &[
    &GENERIC,
    &OXIDGED,
    &GEDCOM_L,
    &FTM,
    &ANCESTRY,
    &MYHERITAGE,
    &GENI,
    &PAF,
    &ANCESTRAL_QUEST,
    &FAMILYSEARCH,
    &HEREDIS,
    &GENEWEB,
    &GENEANET,
    &ROOTSMAGIC,
    &LEGACY,
    &FAMILY_HISTORIAN,
    &GRAMPS,
    &MACFAMILYTREE,
    &WEBTREES,
    &BROTHERS_KEEPER,
    &TMG,
    &GENEATIQUE,
    &ANCESTROLOGIE,
    &ALDFAER,
    &PRO_GEN,
    &REUNION,
    &TNG,
    &WIKITREE,
    &GEDITCOM,
    &AHNENBLATT,
    &GENSDATAPRO,
    &EMBLA,
    &STAMBOOM,
    &FAMILY_FOR_WINDOWS,
    &FAMBERRY,
    &GENEALOGOS,
];

/// Date words of any producer, English and French: `ABT.`, `CIR`, `ENV`,
/// `SEPT`, months in any case or in full, French Republican month names.
const DATES: Op = Op::DateWords;

import_profile! {
    /// This crate (`HEAD.SOUR oxidged`, as its exports write, or `ged_io`,
    /// as its pre-release builds wrote): what it wrote is read as written. Its data was interpreted when it was read; its
    /// extensions are those it kept, its repairs and the redundant ones an
    /// importer needs, none another producer's to interpret again.
    OXIDGED = "oxidged", "oxidged", tier 0, parent None;
    // An export of a program that names itself in `HEAD.SOUR` names the
    // crate in `HEAD.SOUR._OXIDGED` (`profile::HeadSource`).
    detect [
        sig!(Sour, Eq("oxidged"), 200),
        sig!(Sour, Eq("ged_io"), 200),
        sig!(HeadTag, Eq("_OXIDGED"), 200),
    ];
    charset [];
    stats false;
    rules [];
}

import_profile! {
    /// Any producer: the extension tags whose meaning every producer that
    /// writes them agrees on.
    GENERIC = "generic", "Any producer", tier 0, parent None;
    detect [];
    charset [];
    stats true;
    rules [
        rule!("**", "_UID", Op::V7("UID"), Lossless, "X-001", "TJ-071", "VD-028", "VD-033", "VD-052", "VD-068", "VD-095"),
        rule!("INDI", "_FSFTID", Op::Exid { uri: FAMILYSEARCH_PERSON, strip: "" }, Lossless, "X-002", "VD-034", "FR-040"),
        rule!("INDI", "_FID", Op::Exid { uri: FAMILYSEARCH_PERSON, strip: "" }, Lossless, "VD-109"),
        rule!("INDI.NAME", "_AKA", Op::Name(NameKind::Aka, NameValue::List), Lossless, "X-012"),
        rule!("INDI.NAME", "_AKAN", Op::Name(NameKind::Aka, NameValue::AsIs), Lossless, "X-012", "VD-115"),
        rule!("INDI", "_AKA", Op::Name(NameKind::Aka, NameValue::List), Lossless, "X-012"),
        rule!("INDI.NAME", "_MARNM", Op::Name(NameKind::Married, NameValue::Surname), Lossless, "VD-069", "X-010"),
        rule!("INDI.NAME", "_MARN", Op::Name(NameKind::Married, NameValue::Surname), Partial, "X-011"),
        rule!("INDI.NAME", "_MAR", Op::Name(NameKind::Married, NameValue::Surname), Partial, "X-011"),
        rule!("INDI.NAME", "_FORMERNAME", Op::Name(NameKind::Other("Former name"), NameValue::Surname), Partial, "VD-070"),
        rule!("INDI.NAME", "_ADPN", Op::Name(NameKind::Other("Adopted name"), NameValue::AsIs), Lossless, "X-013", "VD-115"),
        rule!("INDI.NAME", "_BIRN", Op::Name(NameKind::Birth, NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_CURN", Op::Name(NameKind::Other("Current name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_FARN", Op::Name(NameKind::Other("Farm name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_RELN", Op::Name(NameKind::Other("Religious name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_SLDN", Op::Name(NameKind::Other("Soldier name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_CENN", Op::Name(NameKind::Other("Census name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_OTHN", Op::Name(NameKind::Other("Other name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_CALL", Op::Name(NameKind::Other("Called name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_FKAN", Op::Name(NameKind::Other("Formerly known as"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_FRKA", Op::Name(NameKind::Other("Formerly known as"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_GERN", Op::Name(NameKind::Other("German name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_INDG", Op::Name(NameKind::Other("Indigenous name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_INDN", Op::Name(NameKind::Other("Indian name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_LNCH", Op::Name(NameKind::Other("Long name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_SHON", Op::Name(NameKind::Other("Short name"), NameValue::AsIs), Lossless, "X-014"),
        rule!("INDI.NAME", "_HEBN", Op::Translation("he"), Lossless, "X-014"),
        rule!("INDI.NAME", "_HNM", Op::Translation("he"), Lossless, "X-014"),
        rule!("INDI.NAME,**.PLAC", "_HEB", Op::Translation("he"), Lossless, "X-109"),
        rule!("INDI", "_FUN", Op::Event { tag: "EVEN", label: Label::Text("Funeral"), y: false }, Lossless, "X-020", "VD-093"),
        rule!("INDI", "_FNRL", Op::Event { tag: "EVEN", label: Label::Text("Funeral"), y: false }, Lossless, "X-021"),
        rule!("INDI", "_MILT", Op::Event { tag: "EVEN", label: Label::Text("Military service"), y: false }, Lossless, "X-027", "VD-085", "VD-093"),
        rule!("INDI", "_MILI", Op::Event { tag: "EVEN", label: Label::Text("Military"), y: false }, Lossless, "X-028"),
        rule!("INDI", "_MILTID", Op::Event { tag: "IDNO", label: Label::Text("Military ID"), y: false }, Lossless, "X-029", "VD-093"),
        rule!("INDI", "_MISN", Op::Event { tag: "EVEN", label: Label::Text("Mission"), y: false }, Lossless, "X-026", "VD-093"),
        rule!("INDI", "_ELEC", Op::Event { tag: "EVEN", label: Label::Text("Elected"), y: false }, Lossless, "X-030", "VD-093"),
        rule!("INDI", "_EXCM", Op::Event { tag: "EVEN", label: Label::Text("Excommunication"), y: false }, Lossless, "X-031", "VD-093"),
        rule!("INDI", "_MDCL", Op::Event { tag: "FACT", label: Label::Text("Medical"), y: false }, Lossless, "X-033", "VD-093"),
        rule!("INDI", "_HEIG", Op::Event { tag: "FACT", label: Label::Text("Height"), y: false }, Lossless, "X-035", "VD-093"),
        rule!("INDI", "_WEIG", Op::Event { tag: "FACT", label: Label::Text("Weight"), y: false }, Lossless, "X-035", "VD-093"),
        rule!("INDI", "_EYEC", Op::Event { tag: "FACT", label: Label::Text("Eye color"), y: false }, Lossless, "X-036"),
        rule!("INDI", "_HAIR", Op::Event { tag: "FACT", label: Label::Text("Hair color"), y: false }, Lossless, "X-036"),
        rule!("INDI", "_DNA", Op::Event { tag: "FACT", label: Label::Text("DNA"), y: false }, Lossless, "X-037", "VD-093"),
        rule!("INDI", "_CIRC", Op::Event { tag: "EVEN", label: Label::Text("Circumcision"), y: false }, Lossless, "X-038", "VD-093"),
        rule!("INDI", "_BRTM", Op::Event { tag: "EVEN", label: Label::Text("Brit milah"), y: false }, Lossless, "X-038"),
        rule!("INDI", "_YART", Op::Event { tag: "EVEN", label: Label::Text("Yahrzeit"), y: false }, Lossless, "X-041"),
        rule!("INDI", "_HOL", Op::Event { tag: "EVEN", label: Label::Text("Holocaust"), y: false }, Lossless, "X-110"),
        rule!("INDI", "_EMPLOY", Op::Event { tag: "EVEN", label: Label::Text("Employment"), y: false }, Lossless, "X-022", "VD-093"),
        rule!("INDI", "_DEG", Op::Event { tag: "EVEN", label: Label::Text("Degree"), y: false }, Lossless, "X-032", "VD-093"),
        rule!("INDI", "_DCAUSE", Op::IntoAs("DEAT", "CAUS"), Partial, "X-034", "VD-093"),
        rule!("INDI", "_INIT", Op::V7("INIL"), Lossless, "X-024", "LDS-002"),
        rule!("INDI", "WAC", Op::V7("INIL"), Lossless, "LDS-002", "VD-036"),
        rule!("FAM", "_SEPR", Op::Event { tag: "EVEN", label: Label::Text("Separation"), y: false }, Lossless, "X-066", "VD-093"),
        rule!("**", "_SDATE", Op::V7("SDATE"), Lossless, "X-136", "VD-008"),
        rule!(events!(""), "_GODP", Op::Assoc("GODP"), Partial, "X-072", "VD-118"),
        rule!(events!(""), "_WITN", Op::Assoc("WITN"), Partial, "X-073", "VD-060"),
        rule!(events!(",FAM"), "_ASSO", Op::Shared, Lossless, "X-074"),
        rule!("INDI", "_PHOTO", Op::PreferPointer, Lossless, "X-047", "VD-090"),
        rule!(events!(",REPO|SUBM|_LOC,HEAD.SOUR.CORP,**.ADDR"), "_EMAIL", Op::Rename("EMAIL"), Lossless, "X-126", "X-168", "VD-025", "TJ-077"),
        rule!(events!(",REPO|SUBM|_LOC,HEAD.SOUR.CORP"), "_E-MAIL", Op::Rename("EMAIL"), Lossless, "X-168"),
        rule!(events!(",REPO|SUBM|_LOC,HEAD.SOUR.CORP"), "_EMAI", Op::Rename("EMAIL"), Lossless, "X-168"),
        rule!(events!(",REPO|SUBM|_LOC,HEAD.SOUR.CORP"), "EMAI", Op::Rename("EMAIL"), Lossless, "X-168"),
        rule!(events!(",REPO|SUBM|_LOC,HEAD.SOUR.CORP"), "EMAL", Op::Rename("EMAIL"), Lossless, "X-168", "X-046"),
        rule!(events!(",REPO|SUBM|_LOC,HEAD.SOUR.CORP"), "_WEB", Op::Rename("WWW"), Lossless, "X-126", "X-168", "VD-025"),
        rule!(events!(",REPO|SUBM|_LOC,HEAD.SOUR.CORP"), "_URL", Op::Rename("WWW"), Lossless, "X-168"),
        rule!("REPO|SUBM,HEAD.SOUR.CORP", "URL", Op::Rename("WWW"), Lossless, "X-168", "TJ-076"),
        rule!("**.SOUR", "_JUST", Op::ToNote("Justification"), Partial, "X-078", "VD-091"),
        rule!("**.SOUR", "_LINK", Op::ToNote("Link"), Partial, "X-079", "VD-091"),
        rule!("**.SOUR", "_FOOT", Op::ToNote("Footnote"), Partial, "X-080", "VD-092"),
        rule!("INDI|FAM|SOUR|REPO|OBJE|NOTE|SUBM", "_UPD", Op::Change, Partial, "X-100", "VD-067"),
        rule!("", "_TODO", Op::Known, Lossless, "X-094"),
        rule!("", "_EVENT_DEFN", Op::Known, Lossless, "X-116", "VD-043"),
        rule!("", "_PLAC_DEFN", Op::Known, Lossless, "VD-044"),
        rule!("", "_HASHTAG_DEFN", Op::Known, Lossless, "VD-059"),
        rule!("", "_EVDEF", Op::Known, Lossless, "X-117", "VD-032"),
        rule!("", "_TASK", Op::Known, Lossless, "X-120"),
        rule!("", "_LOC", Op::Known, Lossless, "X-121"),
        rule!("", "_PLAC", Op::Known, Lossless, "X-122", "VD-005"),
        rule!("**", "*", Op::LongForm, Lossless, "X-167", "TJ-035"),
        rule!("", "INDI|FAM|SOUR|REPO|OBJE|NOTE|SUBM", Op::RecordXref, Lossless, "FR-100"),
    ];
}

import_profile! {
    /// The GEDCOM-L programs (Ages!, AHN, PC-AHNEN, `GEN_DO!`, …): the
    /// German addendum to 5.5.1.
    GEDCOM_L = "gedcom-l", "GEDCOM-L programs (Ages!, Ahnenblatt, GEN_DO!, PC-AHNEN)", tier 2, parent Some("generic");
    detect [
        sig!(Sour, Eq("AGES"), 100),
        sig!(Sour, Eq("AHN"), 100),
        sig!(Sour, Eq("PCAHNEN"), 100),
        sig!(Sour, Prefix("GEN_DO"), 100),
        sig!(GedcVers, Contains("EL"), 60),
        sig!(HeadTag, Eq("_ADDENDUM"), 30),
        sig!(Tag, Eq("_RUFNAME"), 10),
        sig!(Tag, Eq("_GOV"), 10),
        sig!(Tag, Eq("_LOC"), 10),
        sig!(Tag, Eq("_GODP"), 10),
    ];
    charset [];
    stats false;
    rules [
        rule!("INDI.NAME", "_RUFNAME", Op::Known, Lossless, "X-015", "VD-099"),
        rule!("**.PLAC", "_GOV", Op::Exid { uri: "https://gov.genealogy.net/", strip: "" }, Lossless, "X-003"),
        rule!("**.PLAC,_LOC", "_MAIDENHEAD", Op::Known, Lossless, "X-143"),
        rule!("**.PLAC,_LOC", "_POST", Op::Known, Lossless, "X-143"),
        rule!("**.ADDR", "_NAME", Op::Known, Lossless, "X-144"),
        rule!("HEAD.GEDC.VERS", "_ADDENDUM", Op::Known, Lossless, "X-115"),
        rule!("HEAD", "_SCHEMA", Op::Known, Lossless, "X-114"),
        rule!("INDI", "_BUERGERORT", Op::PlaceEvent("Bürgerort"), Lossless, "X-145"),
        rule!("INDI", "_HEIM", Op::PlaceEvent("Heimatort"), Lossless, "X-145"),
        rule!("FAM", "_STAT", Op::NoEvent("MARR"), Partial, "X-062"),
        rule!("INDI", "AGE", Op::IntoAs("DEAT", "AGE"), Lossless, "X-165"),
        rule!("**", "AGE", Op::AgeWords, Lossless, "X-164"),
    ];
}

import_profile! {
    /// Family Tree Maker: Family Tree Maker for Windows (`FTW`, to 16), the
    /// Ancestry era (17 to 22.0) and `MacKiev`'s (22.0.1 on).
    FTM = "ftm", "Family Tree Maker", tier 1, parent Some("generic");
    detect [
        sig!(Sour, Eq("FTM"), 100),
        sig!(Sour, Eq("FTW"), 100),
        sig!(Name, Prefix("Family Tree Maker"), 80),
        sig!(Vers, Prefix("Family Tree Maker"), 40),
        sig!(Corp, Contains("Learning Company"), 40),
        sig!(Corp, Contains("Generations Network"), 40),
        sig!(Corp, Contains("MacKiev"), 40),
        sig!(Corp, Contains("Banner Blue"), 40),
        sig!(Dest, Eq("FTM"), 60),
        sig!(FirstLine, Prefix("0 HEADER"), 30),
        sig!(Tag, Eq("_FREL"), 10),
        sig!(Tag, Eq("_MREL"), 10),
        sig!(Tag, Eq("_MILT"), 10),
        sig!(Tag, Eq("_PHOTO"), 10),
    ];
    charset [];
    stats false;
    rules [
        rule!("FAM.CHIL", "_FREL", Op::Pedigree, Partial, "X-056", "VD-048"),
        rule!("FAM.CHIL", "_MREL", Op::Known, Lossless, "X-056"),
        rule!("INDI", "_ORIG", Op::Event { tag: "FACT", label: Label::Text("Origin"), y: false }, Lossless, "X-023", "VD-093"),
        rule!("INDI", "_DEST", Op::Event { tag: "FACT", label: Label::Text("Destination"), y: false }, Lossless, "X-023", "VD-093"),
        rule!("INDI", "_DETS", Op::Event { tag: "EVEN", label: Label::Text("Death of one spouse"), y: false }, Lossless, "X-067"),
        rule!("FAM", "_DETS", Op::Event { tag: "EVEN", label: Label::Text("Death of one spouse"), y: false }, Lossless, "X-067"),
        rule!("FAM", "_MSTAT", Op::Known, Lossless, "X-065"),
        rule!("FAM", "_MEND", Op::Known, Lossless, "X-065"),
        rule!("INDI", "_NAMS", Op::Known, Lossless, "X-039"),
        rule!("INDI", "_ORDI", Op::Known, Lossless, "X-025"),
        rule!("**.SOUR", "_APID", Op::Known, Lossless, "X-005", "VD-077"),
        rule!("INDI", "ADDR", Op::Residence, Lossless, "X-149"; Gate::Below(17, "FTW")),
        rule!("INDI.OCCU|RELI", "PLAC", Op::PlaceAsValue, Inferred, "X-150"; Gate::Below(17, "FTW")),
        rule!("INDI|FAM", "*", Op::SchemaFact, Lossless, "X-113", "VD-094"; Gate::Below(17, "FTW")),
        rule!("HEAD", "_SCHEMA", Op::Known, Lossless, "X-113", "TJ-038"),
        rule!("**", "DATE", DATES, Lossless, "X-161", "TJ-046"),
    ];
}

import_profile! {
    /// Ancestry.com family trees (online exports).
    ANCESTRY = "ancestry", "Ancestry.com", tier 1, parent Some("generic");
    detect [
        sig!(Sour, Prefix("Ancestry.com"), 100),
        sig!(Sour, Eq("ANCESTRY"), 100),
        sig!(Name, Prefix("Ancestry.com"), 80),
        sig!(Corp, Prefix("Ancestry.com"), 40),
        sig!(HeadTag, Eq("_TREE"), 30),
        sig!(Tag, Eq("_APID"), 10),
        sig!(Tag, Eq("_CROP"), 10),
    ];
    charset [];
    stats false;
    rules [
        rule!("**.SOUR", "_APID", Op::Known, Lossless, "X-005", "VD-077"),
        rule!("SOUR", "_APID", Op::Known, Lossless, "X-005"),
        rule!("FAM.*.SOUR", "_HPID", Op::Known, Lossless, "VD-078"),
        rule!("FAM.*.SOUR", "_WPID", Op::Known, Lossless, "VD-078"),
        rule!("HEAD.SOUR", "_TREE", Op::Known, Lossless, "X-092", "VD-076"),
        rule!("INDI", "_WLNK", Op::Known, Lossless, "VD-084"),
        rule!("OBJE", "_CLON", Op::Known, Lossless, "VD-082"),
        rule!("OBJE", "_CREA", Op::Creation, Partial, "X-093", "VD-081"),
        rule!("INDI.OBJE", "_CROP", Op::Crop(CropForm::Ancestry), Lossless, "VD-083"),
        rule!("INDI.OBJE", "_PRIM", Op::Prefer, Lossless, "VD-083", "VD-037"),
        rule!("INDI|FAM.**.SOUR.DATA", "NOTE", Op::Up, Lossless, "X-090"),
        rule!("SOUR", "NOTE", Op::Html, Lossless, "VD-087"),
        rule!("**", "DATE", DATES, Lossless, "VD-086"),
    ];
}

import_profile! {
    /// `MyHeritage` (Family Tree Builder and the site).
    MYHERITAGE = "myheritage", "MyHeritage", tier 1, parent Some("generic");
    detect [
        sig!(Sour, Eq("MYHERITAGE"), 100),
        sig!(Name, Prefix("MyHeritage"), 80),
        sig!(Corp, Prefix("MyHeritage"), 40),
        sig!(HeadTag, Eq("_RTLSAVE"), 30),
        sig!(HeadTag, Eq("_RINS"), 30),
        sig!(HeadTag, Eq("_PROJECT_GUID"), 30),
        sig!(HeadTag, Eq("_EXPORTED_FROM_SITE_ID"), 30),
    ];
    charset [];
    stats false;
    rules [
        rule!("HEAD.SOUR", "_RTLSAVE", Op::Known, Lossless, "VD-061"),
        rule!("HEAD", "_RINS", Op::Known, Lossless, "X-101", "VD-062"),
        rule!("HEAD", "_PROJECT_GUID", Op::Known, Lossless, "X-101", "VD-063"),
        rule!("HEAD", "_EXPORTED_FROM_SITE_ID", Op::Known, Lossless, "X-101", "VD-063"),
        rule!("HEAD", "_UID", Op::Known, Lossless, "X-101", "VD-063"),
        rule!("HEAD", "_DESCRIPTION_AWARE", Op::Known, Lossless, "VD-064"),
        rule!("HEAD", "_FACTS_DEFRAGGED", Op::Known, Lossless, "VD-064"),
        rule!("", "_PUBLISH", Op::Known, Lossless, "X-103", "VD-066"),
        rule!("", "_ALBUM", Op::Known, Lossless, "X-112"),
        rule!("OBJE,INDI.OBJE", "_FILESIZE", Op::Known, Lossless, "X-053"),
        rule!("OBJE,INDI.OBJE", "_PHOTO_RIN", Op::Known, Lossless, "X-053"),
        rule!("OBJE,INDI.OBJE", "_PLACE", Op::Known, Lossless, "X-053"),
        rule!("INDI.OBJE,INDI.*.OBJE", "_PRIM", Op::Prefer, Inferred, "VD-072"),
        rule!("INDI.OBJE,INDI.*.OBJE", "_CUTOUT", Op::Known, Lossless, "VD-072"),
        rule!("INDI.OBJE,INDI.*.OBJE", "_PRIM_CUTOUT", Op::Known, Lossless, "VD-072"),
        rule!("INDI|FAM.**.SOUR.DATA", "TEXT", Op::Html, Lossless, "VD-075"),
        rule!("INDI|FAM", "_UPD", Op::Change, Partial, "VD-067", "X-100"),
        rule!("**", "DATE", DATES, Lossless, "X-161"),
    ];
}

import_profile! {
    /// Geni.com.
    GENI = "geni", "Geni.com", tier 1, parent Some("generic");
    detect [
        sig!(Sour, Prefix("Geni"), 100),
        sig!(Name, Prefix("Geni"), 80),
        sig!(FirstLine, Prefix(" "), 10),
    ];
    charset [];
    stats false;
    rules [
        rule!(events!(",REPO|SUBM"), "_EMAIL", Op::Rename("EMAIL"), Lossless, "VD-025"),
    ];
}

import_profile! {
    /// Personal Ancestral File.
    PAF = "paf", "Personal Ancestral File", tier 1, parent Some("generic");
    detect [
        sig!(Sour, Prefix("PAF"), 100),
        sig!(Name, Prefix("Personal Ancestral File"), 80),
        sig!(Corp, Contains("Latter-day Saints"), 40),
    ];
    charset [];
    stats false;
    rules [
        rule!("INDI.NAME", "_AKA", Op::Name(NameKind::Aka, NameValue::List), Partial, "TJ-072", "X-012"),
        rule!("INDI.OBJE,OBJE.FILE,OBJE", "_PRIM", Op::Prefer, Lossless, "X-048"),
        rule!("INDI.OBJE,OBJE", "_SCBK", Op::Known, Lossless, "X-049"),
        rule!("INDI.OBJE", "_SSHOW", Op::Known, Lossless, "VD-097"),
        rule!("INDI.FAMC", "_PRIMARY", Op::Prefer, Lossless, "X-060", "VD-096"),
        rule!("INDI.DEAT", "DATE", DATES, Lossless, "TJ-045"),
        rule!("**", "DATE", DATES, Lossless, "X-161", "TJ-049"),
        rule!("SUBM", "URL", Op::Rename("WWW"), Lossless, "TJ-076"),
    ];
}

import_profile! {
    /// Ancestral Quest.
    ANCESTRAL_QUEST = "ancestral-quest", "Ancestral Quest", tier 1, parent Some("paf");
    detect [
        sig!(Sour, Prefix("AncestQuest"), 100),
        sig!(Name, Prefix("Ancestral Quest"), 80),
        sig!(Corp, Contains("Incline Software"), 40),
    ];
    charset [];
    stats false;
    rules [];
}

import_profile! {
    /// `FamilySearch` and the GEDCOM 7 tools of its ecosystem (the GEDCOM X
    /// to GEDCOM 7 converter).
    FAMILYSEARCH = "familysearch", "FamilySearch", tier 1, parent Some("generic");
    detect [
        sig!(Sour, Prefix("FamilySearch"), 100),
        sig!(Name, Prefix("FamilySearch"), 80),
        sig!(Sour, Eq("xto7"), 100),
    ];
    charset [];
    stats false;
    rules [
        rule!("INDI.NAME", "_RUFNAM", Op::Known, Lossless, "VD-132"),
        rule!("INDI.NAME,**.PLAC", "_LANG", Op::Known, Lossless, "VD-132"),
    ];
}

import_profile! {
    /// Heredis (BSD Concept, then Heredis SCOP).
    HEREDIS = "heredis", "Heredis", tier 1, parent Some("generic");
    detect [
        sig!(Sour, Prefix("HEREDIS"), 100),
        sig!(Name, Prefix("HEREDIS"), 80),
        sig!(Corp, Prefix("BSD Concept"), 40),
        sig!(Corp, Prefix("Heredis"), 40),
        sig!(HeadTag, Eq("_GUID"), 30),
        sig!(Tag, Eq("_FIL"), 10),
        sig!(Tag, Eq("_UST"), 10),
        sig!(Tag, Eq("_FNA"), 10),
    ];
    charset [];
    stats false;
    rules [
        rule!("HEAD.SOUR.CORP", "WEB", Op::Rename("WWW"), Lossless, "FR-002", "TJ-064"),
        rule!("HEAD", "_GUID", Op::Known, Lossless, "FR-003"),
        rule!("INDI", "_FIL", Op::PedigreeWord, Partial, "FR-012", "VD-124"),
        rule!("FAM", "_UST", Op::Known, Lossless, "FR-013"),
        rule!("**", "_FNA", Op::Known, Lossless, "FR-014", "X-142"),
        rule!("**.SOUR", "_QUAL", Op::Known, Lossless, "FR-015"),
        rule!("SOUR", "QUAY", Op::Known, Lossless, "X-088"),
        rule!("SOUR", "_ARCH", Op::Known, Lossless, "FR-018"),
        rule!("SOUR", "REPO", Op::RepositoryRecord, Lossless, "FR-019"),
        rule!("**.OBJE", "_PRIM", Op::Prefer, Inferred, "FR-021"),
        rule!("OBJE", "_DATE", Op::Known, Lossless, "FR-022"),
        rule!("INDI|FAM|SOUR|REPO|OBJE|NOTE", "_CREA", Op::Creation, Lossless, "X-104", "FR-039"),
        rule!("INDI|FAM|SOUR|REPO|OBJE|NOTE", "_ETI", Op::Known, Lossless, "FR-039"),
        rule!("INDI.NATI|EDUC|RELI|OCCU", "CAUS", Op::ToParentPayload, Lossless, "FR-032"),
        rule!("INDI|FAM", "EVEN", Op::EvenType(&[("Residence", "RESI")]), Lossless, "FR-033"),
        rule!("INDI|FAM.EVEN", "TYPE", Op::SecondType, Lossless, "FR-034"),
        rule!("INDI", "ACQU", Op::Event { tag: "EVEN", label: Label::Text("Acquisition"), y: false }, Lossless, "FR-035"),
        rule!("INDI", "SEX", Op::Values(&[("?", "U")]), Lossless, "FR-036"),
        rule!("INDI", "SIGN", Op::Known, Lossless, "FR-037", "X-142"),
        rule!("INDI", "_SEC", Op::Known, Lossless, "FR-038"),
        rule!("INDI", "_CLS", Op::Known, Lossless, "FR-038"),
        rule!("INDI", "_ULS", Op::Known, Lossless, "FR-038"),
        rule!("INDI", "_FNF", Op::Known, Lossless, "FR-038"),
        rule!("INDI|FAM.*", "_RECH", Op::Known, Lossless, "FR-041"),
        rule!("**.PLAC", "_SUBMAP", Op::Known, Lossless, "FR-043"),
        rule!("**", "AGE", Op::AgeWords, Lossless, "FR-030"),
        rule!("**", "DATE", DATES, Lossless, "FR-102", "FR-105", "FR-106"),
    ];
}

import_profile! {
    /// `GeneWeb` (`gwb2ged`, master and the `GeneWeb` 7.x line).
    GENEWEB = "geneweb", "GeneWeb", tier 1, parent Some("generic");
    detect [
        sig!(Sour, Eq("GeneWeb"), 100),
        sig!(Name, Eq("gwb2ged"), 80),
        sig!(Corp, Eq("INRIA"), 40),
        sig!(Data, Contains(".gwb"), 30),
        sig!(Tag, Eq("_GWID"), 10),
    ];
    charset [
        ("ANSI", Iso8859_1),
        ("ASCII", Iso8859_1),
        ("IBMPC", Iso8859_1),
    ];
    stats true;
    rules [
        rule!("INDI", "_GWID", Op::Known, Lossless, "FR-073"),
        rule!("INDI", "NAME", Op::AliasNames, Lossless, "FR-077"),
        rule!("INDI.NAME", "SURN", Op::SurnameAliases, Inferred, "FR-076"),
        rule!("INDI.NAME", "GIVN", Op::PublicName, Inferred, "FR-074"),
        rule!("", "INDI", Op::Default("SEX", "U"), Partial, "FR-078"),
        rule!("INDI", "ASSO", Op::Dedupe, Lossless, "FR-206"),
        rule!("INDI", "ASSO|_ASSO", Op::FamilyWitness, Lossless, "FR-089"),
        rule!(events!(",INDI"), "ASSO", Op::RoleWords(GENEWEB_ROLES), Lossless, "FR-087", "FR-088", "FR-107"),
        rule!("INDI", "ADOP", Op::Adoption, Lossless, "FR-096", "FR-204", "FR-108"),
        rule!("FAM", "EVEN", Op::Unmarried, Inferred, "FR-094"),
        rule!("**", "DATE", Op::NegativeYear, Lossless, "FR-208"),
    ];
}

/// The witness kinds `GeneWeb` writes in `RELA` (FR-087, FR-088, EXF-206).
const GENEWEB_ROLES: &[(&str, &str, &str)] = &[
    ("GODF", "GODP", "godfather"),
    ("GODM", "GODP", "godmother"),
    ("GODP", "GODP", ""),
    ("Witness", "WITN", ""),
    ("Civil officer", "OFFICIATOR", "Civil officer"),
    ("Registry officer", "OFFICIATOR", "Registry officer"),
    ("Religious officer", "CLERGY", "Religious officer"),
    ("Officiant", "OFFICIATOR", "Officiant"),
    ("Informant", "OTHER", "Informant"),
];

import_profile! {
    /// Geneanet (`GeneWeb`'s `gnt` branch; header `SOUR Geneanet` since
    /// 2024-10, before that `GeneWeb`'s, with `HEAD.FILE base.ged.tmp`).
    GENEANET = "geneanet", "Geneanet", tier 1, parent Some("geneweb");
    detect [
        sig!(Sour, Eq("Geneanet"), 100),
        // Before 2024-10, Geneanet exports carried GeneWeb's header: they
        // tell themselves by `HEAD.FILE` (GENEANET.md §9.1).
        sig!(Sour, Eq("GeneWeb"), 100),
        sig!(Name, Eq("gwb2ged"), 80),
        sig!(Corp, Eq("INRIA"), 40),
        sig!(Corp, Eq("Geneanet"), 40),
        sig!(CorpAddr, Contains("geneanet.org"), 60),
        sig!(File, Eq("base.ged.tmp"), 60),
        sig!(Name, Eq("GeneWeb"), 30),
    ];
    charset [];
    stats true;
    rules [
        rule!("HEAD.SOUR", "DATA", Op::Known, Lossless, "VD-129"),
    ];
}

import_profile! {
    /// `RootsMagic`.
    ROOTSMAGIC = "rootsmagic", "RootsMagic", tier 2, parent Some("generic");
    detect [
        sig!(Sour, Prefix("RootsMagic"), 100),
        sig!(Name, Prefix("RootsMagic"), 80),
        sig!(Corp, Prefix("RootsMagic"), 40),
        sig!(Tag, Eq("_TMPLT"), 10),
        sig!(Tag, Eq("_SUBQ"), 10),
    ];
    charset [];
    stats false;
    rules [
        rule!(events!(""), "_SHAR", Op::Shared, Lossless, "VD-041"),
        rule!("SOUR", "_SUBQ", Op::Known, Lossless, "X-081", "VD-029"),
        rule!("SOUR", "_BIBL", Op::Known, Lossless, "X-081", "VD-029"),
        rule!("**.SOUR", "_TMPLT", Op::Known, Lossless, "X-082", "VD-030", "TJ-078"),
        rule!("**.SOUR", "_QUAL", Op::Known, Lossless, "VD-031"),
        rule!("INDI", "_AMTID", Op::Known, Lossless, "VD-035"),
        rule!("INDI", "_WEBTAG", Op::Known, Lossless, "X-134", "VD-039"),
        rule!("INDI", "_COLOR", Op::Known, Lossless, "VD-040"),
        rule!("INDI|FAM.OBJE", "_PRIM", Op::Prefer, Lossless, "VD-037"),
        rule!("INDI|FAM.OBJE,OBJE", "_SCBK", Op::Known, Lossless, "VD-038"),
        rule!("**", "NOTE", Op::Html, Lossless, "VD-042"),
        rule!("**", "DATE", DATES, Lossless, "X-161", "TJ-049"),
    ];
}

import_profile! {
    /// Legacy Family Tree.
    LEGACY = "legacy", "Legacy Family Tree", tier 2, parent Some("generic");
    detect [
        sig!(Sour, Eq("Legacy"), 100),
        sig!(Name, Prefix("Legacy"), 80),
        sig!(Corp, Prefix("Millennia"), 40),
    ];
    charset [];
    stats false;
    rules [
        rule!("FAM.CHIL", "_FREL", Op::Pedigree, Partial, "VD-048"),
        rule!("FAM.CHIL", "_MREL", Op::Known, Lossless, "VD-048"),
        rule!("FAM.CHIL|HUSB|WIFE", "_PREF", Op::PreferFamily, Lossless, "X-059", "VD-047"),
        rule!("FAM.CHIL", "_STAT", Op::Known, Lossless, "X-061", "VD-050"),
        rule!("FAM", "_STAT", Op::NoEvent("MARR"), Partial, "VD-009", "X-062"),
        rule!("FAM", "_NONE", Op::Fixed("NCHI", "0"), Lossless, "X-064", "VD-049"),
        rule!(events!(",INDI|FAM"), "_PRIV", Op::Privacy, Lossless, "VD-046", "X-095"),
        rule!("INDI.OBJE,OBJE", "_PRIM", Op::Prefer, Lossless, "VD-037"),
        rule!("INDI.OBJE,OBJE", "_SCBK", Op::Known, Lossless, "VD-038"),
        rule!("INDI.OBJE,OBJE", "_DATE", Op::Known, Lossless, "X-051", "VD-038"),
        rule!("INDI.OBJE,OBJE", "_TYPE", Op::Known, Lossless, "X-050", "VD-038"),
        rule!("INDI.OBJE,OBJE", "_SOUND", Op::Known, Lossless, "X-054"),
        rule!("INDI|FAM.**.SOUR", "DATE", Op::Into("DATA"), Lossless, "X-084"),
        rule!("**.SOUR|PLAC", "_VERI", Op::Known, Lossless, "X-083", "VD-054"),
        rule!("SOUR", "_ITALIC", Op::Known, Lossless, "X-086", "VD-056"),
        rule!("SOUR", "_PAREN", Op::Known, Lossless, "X-086", "VD-056"),
        rule!("SOUR", "_QUOTED", Op::Known, Lossless, "X-086", "VD-056"),
        rule!("INDI|FAM|SOUR,**.PLAC|ADDR", "_TAG", Op::Known, Lossless, "X-098", "VD-051"),
        rule!("INDI", "_URL", Op::Known, Lossless, "X-133", "VD-053"),
        rule!("INDI", "_PPEXCLUDE", Op::Known, Lossless, "VD-058"),
        rule!("FAM.MARR,INDI.EVEN", "_HTITL", Op::Known, Lossless, "X-132"),
        rule!("INDI|FAM.*", "_OVER", Op::Known, Lossless, "X-132", "VD-057"),
        rule!("**.ADDR", "_SORT", Op::Known, Lossless, "X-131", "VD-055"),
        rule!("**", "DATE", DATES, Lossless, "X-161", "TJ-049"),
    ];
}

import_profile! {
    /// Family Historian (Calico Pie).
    FAMILY_HISTORIAN = "family-historian", "Family Historian", tier 2, parent Some("generic");
    detect [
        sig!(Sour, Eq("FAMILY_HISTORIAN"), 100),
        sig!(Name, Prefix("Family Historian"), 80),
        sig!(Corp, Prefix("Calico Pie"), 40),
        sig!(HeadTag, Eq("_ROOT"), 30),
        sig!(Tag, Eq("_SHAR"), 10),
        sig!(Tag, Eq("_ATTR"), 10),
        sig!(Tag, Eq("_FLGS"), 10),
    ];
    charset [];
    stats false;
    rules [
        rule!("INDI|FAM", "_ATTR", Op::Event { tag: "FACT", label: Label::None, y: false }, Lossless, "X-123", "VD-001"),
        rule!(events!(""), "_SHAR", Op::Shared, Lossless, "X-075", "VD-002"),
        rule!(events!(""), "_SHAN", Op::Shared, Lossless, "X-076", "VD-003"),
        rule!("INDI.FAMC", "_PEDI", Op::PedigreeWord, Partial, "X-130", "VD-024"),
        rule!("**.OBJE", "_AREA", Op::Crop(CropForm::Area), Partial, "X-127", "VD-023"),
        rule!("INDI.NAME", "_USED", Op::Known, Lossless, "X-016", "VD-010"),
        rule!("INDI", "_FLGS", Op::Known, Lossless, "X-124", "VD-006"),
        rule!("INDI.*", "_SENT", Op::Known, Lossless, "X-129", "VD-026"),
        rule!("HEAD", "_ROOT", Op::Known, Lossless, "X-125", "VD-013"),
        rule!("HEAD", "_LIST", Op::Known, Lossless, "X-125", "VD-015"),
        rule!("HEAD", "_USED", Op::Known, Lossless, "VD-011"),
        rule!("HEAD", "_UID", Op::Known, Lossless, "VD-012"),
        rule!("HEAD", "_PCIT", Op::Known, Lossless, "VD-016"),
        rule!("HEAD.GEDC", "_VAR", Op::Known, Lossless, "VD-014"),
        rule!("SOUR.TITL", "_AUTO", Op::Known, Lossless, "VD-027"),
        rule!("SOUR", "_TYPE", Op::Known, Lossless, "VD-019", "X-050"),
        rule!("", "_SRCT", Op::Known, Lossless, "VD-018"),
        rule!("", "_RNOT", Op::Known, Lossless, "VD-021"),
        rule!("INDI", "_SEX", Op::Rename("SEX"), Lossless, "TJ-069"),
    ];
}

import_profile! {
    /// Gramps.
    GRAMPS = "gramps", "Gramps", tier 2, parent Some("generic");
    detect [
        sig!(Sour, Eq("Gramps"), 100),
        sig!(Name, Eq("Gramps"), 80),
    ];
    charset [];
    stats false;
    rules [
        rule!("INDI.FAMC", "_FREL", Op::Pedigree, Partial, "X-057", "VD-098"),
        rule!("INDI.FAMC", "_MREL", Op::Known, Lossless, "X-057"),
        rule!("INDI.FAMC", "_PRIMARY", Op::Prefer, Lossless, "X-060"),
        rule!("INDI.NAME", "_RUFNAME", Op::Known, Lossless, "X-015", "VD-099"),
        rule!("INDI|FAM.*", "ATTR", Op::Known, Lossless, "X-158"),
    ];
}

import_profile! {
    /// `MacFamilyTree` (Synium Software).
    MACFAMILYTREE = "macfamilytree", "MacFamilyTree", tier 2, parent Some("generic");
    detect [
        sig!(Sour, Prefix("SyniumFamilyTree"), 100),
        sig!(Sour, Prefix("MacFamilyTree"), 100),
        // Before 8.3.1 the system identifier was empty (TJ-059).
        sig!(Name, Prefix("MacFamilyTree"), 100),
        sig!(Corp, Prefix("Synium"), 40),
    ];
    charset [];
    stats false;
    rules [
        rule!("", "_PLAC", Op::Known, Lossless, "X-119", "VD-111"),
        rule!("INDI", "_FID", Op::Exid { uri: FAMILYSEARCH_PERSON, strip: "" }, Lossless, "VD-109"),
    ];
}

import_profile! {
    /// webtrees and `PhpGedView`.
    WEBTREES = "webtrees", "webtrees and PhpGedView", tier 2, parent Some("gedcom-l");
    detect [
        sig!(Sour, Prefix("webtrees"), 100),
        sig!(Sour, Prefix("PhpGedView"), 100),
        sig!(Name, Prefix("webtrees"), 80),
        sig!(Name, Prefix("PhpGedView"), 80),
        sig!(Tag, Eq("_WT_USER"), 10),
        sig!(Tag, Eq("_PGVU"), 10),
    ];
    charset [];
    stats false;
    rules [
        rule!("**.CHAN", "_PGVU", Op::Known, Lossless, "X-105"),
        rule!("**.CHAN", "_WT_USER", Op::Known, Lossless, "X-105"),
        rule!("INDI", "_PGV_OBJS", Op::PreferPointer, Lossless, "X-106"),
        rule!("INDI", "_WT_OBJE_SORT", Op::PreferPointer, Lossless, "X-106"),
        rule!("INDI|FAM", "COMM", Op::Rename("NOTE"), Lossless, "X-107"),
        rule!("OBJE", "_THUM", Op::Known, Lossless, "X-055"),
        rule!("SOUR", "_DBID", Op::Known, Lossless, "X-111"),
    ];
}

import_profile! {
    /// Brother's Keeper.
    BROTHERS_KEEPER = "brothers-keeper", "Brother's Keeper", tier 3, parent Some("generic");
    detect [
        sig!(Sour, Eq("BROSKEEP"), 100),
        sig!(Dest, Prefix("BROSKEEP"), 60),
    ];
    charset [("IBMPC", Windows1252)];
    stats false;
    rules [
        rule!("INDI", "_INTE", Op::Event { tag: "BURI", label: Label::Text("Interment"), y: false }, Lossless, "X-040"),
        rule!("INDI", "_NLIV", Op::Fixed("DEAT", "Y"), Lossless, "X-042"),
        rule!("INDI", "_NMAR", Op::Fixed("NMR", "0"), Lossless, "X-043"),
        rule!("FAM", "_NMR", Op::NoEvent("MARR"), Lossless, "X-063", "VD-114"),
        rule!("FAM", "_MARRIED", Op::NoEvent("MARR"), Inferred, "VD-114"),
        rule!("FAM", "_COML", Op::Event { tag: "MARR", label: Label::Text("Common law"), y: false }, Lossless, "X-068", "VD-114"),
        rule!("FAM", "_MARI", Op::Event { tag: "EVEN", label: Label::Text("Marriage intention"), y: false }, Lossless, "X-068"),
        rule!("FAM", "_MBON", Op::Event { tag: "EVEN", label: Label::Text("Marriage bond"), y: false }, Lossless, "X-068"),
        rule!("FAM", "_MSTAT", Op::Known, Lossless, "VD-114"),
        rule!("SOUR", "_ITALIC", Op::Known, Lossless, "VD-056"),
    ];
}

import_profile! {
    /// The Master Genealogist.
    TMG = "tmg", "The Master Genealogist", tier 3, parent Some("generic");
    detect [
        sig!(Sour, Eq("TMG"), 100),
        sig!(Name, Prefix("The Master Genealogist"), 80),
    ];
    charset [];
    stats false;
    rules [
        rule!(events!(""), "_SHAR", Op::Shared, Lossless, "VD-121"),
        rule!("INDI.NAME", "_DATE", Op::Known, Lossless, "X-018"),
        rule!("**", "DATE", DATES, Lossless, "X-148", "VD-122"),
    ];
}

import_profile! {
    /// Généatique (CDIP).
    GENEATIQUE = "geneatique", "Généatique", tier 3, parent Some("generic");
    detect [
        sig!(Sour, Prefix("GENEATIQUE"), 100),
        sig!(Dest, Eq("ANSTFILE"), 30),
        sig!(Char, Eq("OEM"), 30),
    ];
    charset [];
    stats false;
    rules [
        rule!("INDI|FAM.*", "_ACT", Op::Known, Lossless, "X-140", "FR-047"),
        rule!("OBJE", "FORM", Op::IntoSibling("FILE"), Lossless, "X-141"),
        rule!("OBJE", "TITL", Op::IntoSibling("FILE"), Lossless, "X-141"),
        rule!("INDI", "NICK", Op::IntoSibling("NAME"), Lossless, "FR-051"),
        rule!("INDI|FAM|SOUR|REPO|OBJE|NOTE", "_DCT", Op::Creation, Lossless, "FR-052"),
        rule!("HEAD,HEAD.SOUR.CORP,SUBM|REPO,INDI.*", "_MAIL", Op::Rename("EMAIL"), Lossless, "FR-053"),
        rule!("INDI,INDI.*", "_ORG", Op::Known, Lossless, "FR-054"),
        rule!("INDI|FAM", "_EVT", Op::Event { tag: "EVEN", label: Label::Payload, y: false }, Lossless, "FR-055"),
        rule!("FAM", "_PCS", Op::Event { tag: "EVEN", label: Label::Text("PACS"), y: false }, Lossless, "FR-056"),
        rule!("FAM", "PACS", Op::Event { tag: "EVEN", label: Label::Text("PACS"), y: false }, Lossless, "FR-056"),
        rule!("**", "DATE", DATES, Lossless, "FR-102", "FR-105", "FR-106"),
    ];
}

import_profile! {
    /// Ancestrologie.
    ANCESTROLOGIE = "ancestrologie", "Ancestrologie", tier 3, parent Some("generic");
    detect [sig!(Sour, Prefix("Ancestrologie"), 100)];
    charset [];
    stats false;
    rules [
        rule!("INDI", "INHU", Op::Rename("BURI"), Lossless, "FR-067"),
        rule!("SOUR", "EVEN", Op::Into("DATA"), Lossless, "FR-068"),
        rule!("**", "DATE", DATES, Lossless, "FR-102"),
    ];
}

import_profile! {
    /// Aldfaer.
    ALDFAER = "aldfaer", "Aldfaer", tier 3, parent Some("generic");
    detect [sig!(Sour, Prefix("ALDFAER"), 100)];
    charset [];
    stats false;
    rules [
        rule!("FAM", "MARR_CIVIL", Op::Event { tag: "MARR", label: Label::Text("Civil"), y: false }, Lossless, "X-069"),
        rule!("FAM", "MARR_RELIGIOUS", Op::Event { tag: "MARR", label: Label::Text("Religious"), y: false }, Lossless, "X-069"),
        rule!("FAM", "MARR_PARTNERS", Op::Event { tag: "EVEN", label: Label::Text("Registered partnership"), y: false }, Lossless, "X-069"),
        rule!("FAM", "MARR_UNKNOWN", Op::Event { tag: "MARR", label: Label::None, y: false }, Lossless, "X-069"),
        rule!("FAM", "_ALDFAER_NOREL", Op::NoEvent("MARR"), Inferred, "X-070"),
        rule!("INDI.BIRT|DEAT", "_ALDFAER_TIME", Op::TimeIntoDate, Lossless, "X-137"),
        rule!("INDI.BIRT|DEAT", "_LENGTH", Op::Known, Lossless, "X-138"),
        rule!("INDI.BIRT|DEAT", "_WEIGHT", Op::Known, Lossless, "X-138"),
        rule!("INDI.NAME", "_SURNAS", Op::Rename("SURN"), Partial, "X-017"),
        rule!("INDI", "_PRIVACY", Op::Privacy, Inferred, "X-097"),
        rule!("HEAD", "SUBM", Op::SubmitterRecord, Lossless, "X-139"),
        rule!("INDI", "_BOLD", Op::Known, Lossless, "X-099"),
        rule!("INDI", "_ITALIC", Op::Known, Lossless, "X-099"),
        rule!("INDI", "_UNDERLINE", Op::Known, Lossless, "X-099"),
        rule!("INDI", "_COLOR", Op::Known, Lossless, "X-099"),
        rule!("INDI", "_NEW", Op::Known, Lossless, "X-099"),
    ];
}

import_profile! {
    /// PRO-GEN.
    PRO_GEN = "pro-gen", "PRO-GEN", tier 3, parent Some("generic");
    detect [sig!(Sour, Prefix("PRO-GEN"), 100)];
    charset [];
    stats false;
    rules [
        rule!("INDI.BIRT|DEAT", "TIME", Op::TimeIntoDate, Lossless, "X-137"),
        rule!("**.SOUR", "REFN", Op::Known, Lossless, "X-089"),
    ];
}

import_profile! {
    /// Reunion (Leister Productions).
    REUNION = "reunion", "Reunion", tier 3, parent Some("generic");
    detect [
        sig!(Sour, Eq("Reunion"), 100),
        sig!(Corp, Prefix("Leister"), 40),
    ];
    charset [];
    stats false;
    rules [
        rule!("INDI", "_PURC", Op::Event { tag: "EVEN", label: Label::Text("Purchase"), y: false }, Lossless, "X-045"),
        rule!("INDI", "_SALE", Op::Event { tag: "EVEN", label: Label::Text("Sale"), y: false }, Lossless, "X-045"),
        rule!("INDI", "_LEGA", Op::Event { tag: "EVEN", label: Label::Text("Legacy"), y: false }, Lossless, "X-045"),
        rule!("INDI", "CITN", Op::Event { tag: "FACT", label: Label::Text("Citizenship"), y: false }, Lossless, "X-046"),
        rule!("INDI", "EMAL", Op::Known, Lossless, "X-046"),
        rule!("INDI", "_MDCL", Op::Event { tag: "FACT", label: Label::Text("Medical"), y: false }, Lossless, "X-033"),
        rule!("FAM", "CLAW", Op::Event { tag: "MARR", label: Label::Text("Common law"), y: false }, Inferred, "VD-113"),
    ];
}

import_profile! {
    /// TNG (The Next Generation of Genealogy Sitebuilding).
    TNG = "tng", "TNG", tier 3, parent Some("generic");
    detect [
        sig!(Sour, Prefix("The Next Generation"), 100),
        sig!(Sour, Prefix("TNG"), 100),
        sig!(Tag, Eq("_LIVING"), 10),
    ];
    charset [];
    stats false;
    rules [
        rule!("INDI", "_PRIVATE", Op::Privacy, Partial, "X-096"),
        rule!("INDI", "_LIVING", Op::Known, Lossless, "X-096"),
    ];
}

import_profile! {
    /// `WikiTree`.
    WIKITREE = "wikitree", "WikiTree", tier 3, parent Some("generic");
    detect [
        sig!(Sour, Prefix("WikiTree"), 100),
        sig!(Name, Prefix("WikiTree"), 80),
        sig!(Corp, Prefix("Interesting.com"), 40),
    ];
    charset [];
    stats false;
    rules [
        rule!("INDI", "WWW", Op::ExidUrl { prefix: "https://www.wikitree.com/wiki/", uri: "https://www.wikitree.com/wiki/" }, Lossless, "VD-104"),
        rule!("INDI", "_BIO", Op::ChildNote("TEXT"), Lossless, "VD-106"),
        rule!("INDI.NAME", "_PGVN", Op::Rename("NICK"), Partial, "VD-105"),
        rule!("INDI.NAME", "_MIDN", Op::Rename("GIVN"), Partial, "VD-105"),
    ];
}

import_profile! {
    /// `GEDitCOM` (RSAC Software).
    GEDITCOM = "geditcom", "GEDitCOM", tier 3, parent Some("generic");
    detect [
        sig!(Sour, Prefix("GEDitCOM"), 100),
        sig!(Corp, Prefix("RSAC"), 40),
        sig!(CharVers, Contains("MacOS"), 30),
        sig!(HeadTag, Eq("_HME"), 30),
    ];
    charset [("ASCII", MacRoman)];
    stats false;
    rules [rule!("HEAD", "_HME", Op::Known, Lossless, "VD-130")];
}

import_profile! {
    /// Ahnenblatt.
    AHNENBLATT = "ahnenblatt", "Ahnenblatt", tier 3, parent Some("gedcom-l");
    detect [
        sig!(Sour, Prefix("AHNENBLATT"), 100),
        sig!(HeadTag, Eq("_NAVM"), 30),
    ];
    charset [];
    stats false;
    rules [rule!("HEAD", "_NAVM", Op::Known, Lossless, "VD-117")];
}

import_profile! {
    /// `GensDataPro`.
    GENSDATAPRO = "gensdatapro", "GensDataPro", tier 3, parent Some("generic");
    detect [sig!(Sour, Prefix("GensDataPro"), 100)];
    charset [];
    stats false;
    rules [rule!("INDI|FAM.*.DATE", "NOTE", Op::Up, Lossless, "X-156")];
}

import_profile! {
    /// Embla Family Treasures.
    EMBLA = "embla", "Embla Family Treasures", tier 3, parent Some("generic");
    detect [sig!(Sour, Prefix("Embla"), 100)];
    charset [];
    stats false;
    rules [rule!("**", "*", Op::CaseFold, Lossless, "TJ-063")];
}

import_profile! {
    /// `StamboomNederland`.
    STAMBOOM = "stamboom", "StamboomNederland", tier 3, parent Some("generic");
    detect [sig!(Sour, Eq("SNL"), 100), sig!(Sour, Prefix("Stamboom"), 100)];
    charset [];
    stats false;
    rules [rule!("INDI", "SEX", Op::Values(&[("", "U")]), Lossless, "TJ-066")];
}

import_profile! {
    /// Family For Windows.
    FAMILY_FOR_WINDOWS = "family-for-windows", "Family For Windows", tier 3, parent Some("generic");
    detect [sig!(Sour, Prefix("Family For Windows"), 100), sig!(Sour, Eq("FFW"), 100)];
    charset [];
    stats false;
    rules [
        rule!("INDI", "SEX", Op::Values(&[("Male", "M"), ("Female", "F"), ("Unknown", "U")]), Lossless, "TJ-075"),
    ];
}

import_profile! {
    /// Famberry (2015): `CHAR MACINTOSH` over Latin-1 bytes.
    FAMBERRY = "famberry", "Famberry", tier 3, parent Some("generic");
    detect [sig!(Sour, Prefix("Famberry"), 100)];
    charset [("MACINTOSH", Iso8859_1)];
    stats false;
    rules [];
}

import_profile! {
    /// Genealogos, which writes `0 INDI @I1@`.
    GENEALOGOS = "genealogos", "Genealogos", tier 3, parent Some("generic");
    detect [sig!(Sour, Prefix("Genealogos"), 100)];
    charset [];
    stats false;
    rules [];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_named_once_and_parents_exist() {
        let mut ids: Vec<&str> = PROFILES.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), len, "duplicate profile names");
        for p in PROFILES {
            if let Some(parent) = p.parent {
                assert!(
                    super::super::profile(parent).is_some(),
                    "{}: {parent}",
                    p.id
                );
            }
        }
    }
}
