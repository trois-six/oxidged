//! The export profiles. Each step cites the rows of the research's
//! `export.tsv` (`tests/profiles/export.tsv`) it follows; a test checks
//! that every row is followed by a profile, by the writer, or listed as
//! not yet.

use crate::writer::LineEnding;

use super::ExportProfile;
use super::dsl::{export_profile, step};
use super::export::{AssocStyle, MediaPaths, Op, PlaceStyle, Target};

/// Every export profile.
pub(crate) static PROFILES: &[&ExportProfile] = &[
    &GENERIC_551,
    &GENERIC_70,
    &GENEANET,
    &GENEWEB,
    &HEREDIS,
    &ANCESTRY,
    &FTM,
    &MYHERITAGE,
    &GENI,
    &FAMILYSEARCH,
    &PAF,
    &ANCESTRAL_QUEST,
    &ROOTSMAGIC,
    &LEGACY,
    &GRAMPS,
    &WEBTREES,
    &FAMILY_HISTORIAN,
    &MACFAMILYTREE,
    &ANCESTRIS,
    &GENEATIQUE,
    &TNG,
    &REUNION,
    &WIKITREE,
    &FINDMYPAST,
    &AHNENBLATT,
];

/// The attributes and events `GeneWeb`'s importer does not read, as `EVEN`
/// with the labels it knows (EXF-014, EXF-015, EXF-032).
const GENEWEB_UNREAD: &[(&str, &str)] = &[
    ("CHRA", "adult christening"),
    ("PROB", "probate"),
    ("CAST", "caste"),
    ("DSCR", "description"),
    ("IDNO", "identification number"),
    ("NATI", "nationality"),
    ("NCHI", "children count"),
    ("NMR", "marriage count"),
    ("RELI", "religion"),
    ("SSN", "social security number"),
    ("DIVF", "divorce filed"),
    ("MARS", "marriage settlement"),
    ("_SEPR", "separation"),
];

/// The attributes Ancestry does not import as such (EXC-005).
const ANCESTRY_ATTRIBUTES: &[(&str, &str)] = &[
    ("ALIA", "Alias"),
    ("IDNO", "ID number"),
    ("NCHI", "Children"),
    ("NMR", "Marriages"),
];

/// What `MyHeritage` does not import (EXC-040).
const MYHERITAGE_UNREAD: &[(&str, &str)] = &[
    ("AFN", "AFN"),
    ("ALIA", "Alias"),
    ("ANCI", "Ancestor interest"),
    ("DESI", "Descendant interest"),
    ("RFN", "Record file number"),
    ("SUBM", "Submitter"),
];

/// What `RootsMagic` does not hold natively (EXC-060).
const ROOTSMAGIC_UNREAD: &[(&str, &str)] = &[
    ("ALIA", "Alias"),
    ("ANCI", "Ancestor interest"),
    ("DESI", "Descendant interest"),
    ("IDNO", "ID number"),
    ("NCHI", "Children"),
    ("NMR", "Marriages"),
    ("RFN", "Record file number"),
    ("SUBM", "Submitter"),
];

/// What Legacy does not hold natively (EXC-077).
const LEGACY_UNREAD: &[(&str, &str)] = &[
    ("IDNO", "ID number"),
    ("ALIA", "Alias"),
    ("SUBM", "Submitter"),
];

/// What Ancestral Quest does not read as an attribute (EXC-090).
const AQ_UNREAD: &[(&str, &str)] = &[
    ("DSCR", "Description"),
    ("IDNO", "ID number"),
    ("NCHI", "Children"),
    ("NMR", "Marriages"),
    ("SUBM", "Submitter"),
];

/// What Findmypast does not hold natively (EXC-109).
const FINDMYPAST_UNREAD: &[(&str, &str)] = &[
    ("ALIA", "Alias"),
    ("ANCI", "Ancestor interest"),
    ("DESI", "Descendant interest"),
    ("SUBM", "Submitter"),
    ("RIN", "Record ID"),
];

/// What Family Tree Maker imports as facts (EXC-023).
const FTM_FACTS: &[(&str, &str)] = &[
    ("IDNO", "ID number"),
    ("NCHI", "Children"),
    ("NMR", "Marriages"),
];

/// Event fields shown by no importer of the Ancestry family (EXC-008,
/// EXC-024, EXC-062, EXC-091, EXC-097, EXC-110).
const AGE: &[(&str, &str)] = &[("AGE", "Age")];
const AGE_CONTACTS: &[(&str, &str)] = &[
    ("AGE", "Age"),
    ("PHON", "Phone"),
    ("EMAIL", "Email"),
    ("FAX", "Fax"),
    ("WWW", "Web"),
];
const GENEWEB_FIELDS: &[(&str, &str)] = &[("AGE", "Age")];

export_profile! {
    /// GEDCOM 5.5.1 as most importers read it best: UTF-8, LF, lines of at
    /// most 255 characters, `CONC` split inside words (REPORT §7.4).
    GENERIC_551 = "generic-551", "Any GEDCOM 5.5.1 importer", tier 0;
    target Target::V551;
    steps [];
}

export_profile! {
    /// GEDCOM 7.0.
    GENERIC_70 = "generic-70", "Any GEDCOM 7.0 importer", tier 0;
    target Target::V70;
    steps [];
}

export_profile! {
    /// Geneanet's online import (`GeneWeb` `ged2gwb`, `gnt` branch), after
    /// Geneanet's own pre-processing (GENEANET.md §9.3).
    GENEANET = "geneanet", "Geneanet", tier 1;
    target Target {
        max_bytes: Some(250_000_000),
        max_depth: Some(9),
        rows: &["EXF-044", "EXF-010", "EXF-218", "EXF-222"],
        ..Target::V551
    };
    steps [
        step!(Op::ProfessionFacts, "EXF-215"),
        step!(Op::OccupationsMerged, "EXF-013", "EXF-038"),
        step!(Op::ParticleInSurname, "EXF-001"),
        step!(Op::SurnamesPacked, "EXF-003", "EXF-037", "EXF-211", "EXF-002"),
        step!(Op::NicknamesJoined, "EXF-005", "EXF-040"),
        step!(Op::UsualNameQuoted, "EXF-006", "EXF-039", "EXF-217"),
        step!(Op::NoColonInNames, "EXF-008"),
        step!(Op::AsEven(GENEWEB_UNREAD), "EXF-014", "EXF-015", "EXF-032"),
        step!(Op::EventsSaidY, "EXF-016"),
        step!(Op::DeathSaidY, "EXF-017"),
        step!(Op::OneBaptism, "EXF-018"),
        step!(Op::UnionType, "EXF-031", "EXF-048", "EXF-216"),
        step!(Op::EmptyPacs, "EXF-205"),
        step!(Op::Godparents, "EXF-207", "EXF-047"),
        step!(Op::EventAssociations(AssocStyle::Note), "EXF-026", "EXF-206", "EXF-208", "EXF-028"),
        step!(Op::Adoptions, "EXF-030", "EXF-212"),
        step!(Op::ChildLinkSources, "EXF-049"),
        step!(Op::InlineSources, "EXF-019", "EXF-221", "EXF-011"),
        step!(Op::InlineMedia, "EXF-012", "EXF-042", "EXF-219"),
        step!(Op::EscapedNotes, "EXF-020", "EXF-210", "EXF-209"),
        step!(Op::OneHeadNote, "EXF-035", "EXF-213"),
        // The fields GeneWeb reads no place for go to notes first: the
        // places left are those the header's format then describes.
        step!(Op::FieldsAsNotes(GENEWEB_FIELDS), "EXF-036"),
        step!(Op::Places(PlaceStyle::Geneanet), "EXF-036", "EXF-046"),
        step!(Op::AddressesAsNotes, "EXF-045"),
        step!(Op::BadDates, "EXF-214"),
        step!(Op::SexSaid, "EXF-033"),
        step!(Op::ExtensionsAsNotes, "EXF-021", "EXF-220"),
    ];
}

export_profile! {
    /// A local `GeneWeb` (master `ged2gwb`, `GeneWeb` 7.1): Geneanet's profile
    /// but for its differences (GENEANET.md §9.4).
    GENEWEB = "geneweb", "GeneWeb", tier 1;
    target Target {
        max_depth: Some(9),
        rows: &["EXF-010"],
        ..Target::V551
    };
    steps [
        step!(Op::ProfessionFacts, "EXF-215"),
        step!(Op::OccupationsMerged, "EXF-013"),
        step!(Op::ParticleInSurname, "EXF-001"),
        step!(Op::SurnamesPacked, "EXF-003", "EXF-002"),
        step!(Op::NicknamesJoined, "EXF-005"),
        step!(Op::UsualNameQuoted, "EXF-006"),
        step!(Op::NoColonInNames, "EXF-008"),
        step!(Op::AsEven(GENEWEB_UNREAD), "EXF-014", "EXF-015", "EXF-032"),
        step!(Op::EventsSaidY, "EXF-016"),
        step!(Op::DeathSaidY, "EXF-017"),
        step!(Op::OneBaptism, "EXF-018"),
        step!(Op::UnionType, "EXF-031", "EXF-048"),
        step!(Op::Godparents, "EXF-027", "EXF-207"),
        step!(Op::EventAssociations(AssocStyle::Note), "EXF-026", "EXF-206"),
        step!(Op::Adoptions, "EXF-030"),
        step!(Op::InlineSources, "EXF-019", "EXF-011"),
        step!(Op::InlineMedia, "EXF-012"),
        step!(Op::EscapedNotes, "EXF-020", "EXF-209"),
        step!(Op::OneHeadNote, "EXF-035"),
        step!(Op::FieldsAsNotes(GENEWEB_FIELDS), "EXF-036"),
        step!(Op::AddressesAsNotes, "EXF-036"),
        step!(Op::BadDates, "EXF-023"),
        step!(Op::SexSaid, "EXF-033"),
        step!(Op::ExtensionsAsNotes, "EXF-021"),
    ];
}

export_profile! {
    /// Heredis 2023 and later: 5.5.1 `.ged` (7.0 GEDZIP when media matter,
    /// with [`super::ExportOptions::version`]).
    HEREDIS = "heredis", "Heredis", tier 1;
    target Target {
        media: MediaPaths::Relative,
        rows: &["EXF-050", "EXF-051", "EXF-056"],
        ..Target::V551
    };
    steps [
        step!(Op::Places(PlaceStyle::Heredis), "EXF-052", "EXF-053"),
        step!(Op::Preferred(&["_PRIM"], false), "EXF-059", "EXF-060"),
        step!(Op::ExtensionsAsEvents, "EXF-054"),
    ];
}

export_profile! {
    /// Ancestry.com family tree upload.
    ANCESTRY = "ancestry", "Ancestry.com", tier 1;
    target Target {
        max_bytes: Some(500_000_000),
        rows: &["EXC-001"],
        ..Target::V551
    };
    steps [
        step!(Op::HomeFirst, "EXC-015"),
        step!(Op::FactsAsEven, "EXC-004"),
        step!(Op::AsEven(ANCESTRY_ATTRIBUTES), "EXC-005"),
        step!(Op::AssociationsAsEvents(false), "EXC-007"),
        step!(Op::FieldsAsNotes(AGE), "EXC-008"),
        step!(Op::AddressesInPlace, "EXC-009"),
        step!(Op::CausesAsNotes, "EXC-009"),
        step!(Op::RecordNotesAsEvent("Record Note"), "EXC-010"),
        step!(Op::RecordCitationsOnEvent("Whole Record Citation"), "EXC-011"),
        step!(Op::SourceRecords, "EXC-012"),
        step!(Op::SourceFieldsAsNote, "EXC-013"),
        step!(Op::RepositoryContactsAsNote, "EXC-014"),
        step!(Op::PlainNotes, "EXC-016"),
        step!(Op::ParentRelations, "EXC-017"),
        step!(Op::BareFactsNoted, "EXC-003"),
    ];
}

export_profile! {
    /// Family Tree Maker (2017 to 2019 and later).
    FTM = "ftm", "Family Tree Maker", tier 1;
    target Target {
        max_name: Some(40),
        max_children: Some(99),
        rows: &["EXC-036", "EXC-034"],
        ..Target::V551
    };
    steps [
        step!(Op::FlatHead, "EXC-033"),
        step!(Op::PrefixAsTitle, "EXC-021"),
        step!(Op::AssociationsAsEvents(true), "EXC-022"),
        step!(Op::AsFact(FTM_FACTS), "EXC-023"),
        step!(Op::FieldsAsNotes(AGE), "EXC-024"),
        step!(Op::AddressesAsNotes, "EXC-025"),
        step!(Op::CausesAsNotes, "EXC-026"),
        step!(Op::ParentRelations, "EXC-017", "EXC-027"),
        step!(Op::RecordCitationsOnEvent("Whole Record Citation"), "EXC-028"),
        step!(Op::Photo, "EXC-029"),
        step!(Op::SourceFieldsAsNote, "EXC-030"),
        step!(Op::ContactsJoined, "EXC-031"),
        step!(Op::NamesWithSlashes, "EXC-019", "EXC-020"),
    ];
}

export_profile! {
    /// `MyHeritage` (Family Tree Builder and the site).
    MYHERITAGE = "myheritage", "MyHeritage", tier 1;
    target Target {
        max_name: Some(120),
        rows: &["EXC-050"],
        ..Target::V551
    };
    steps [
        step!(Op::FlatHead, "EXC-049"),
        step!(Op::NameValueComplete, "EXC-037"),
        step!(Op::UsualNameAsNick, "EXC-038"),
        step!(Op::FactsAsEven, "EXC-039"),
        step!(Op::AsEven(MYHERITAGE_UNREAD), "EXC-040", "EXC-048"),
        step!(Op::AssociationsAsEvents(false), "EXC-041"),
        step!(Op::AddressesInPlace, "EXC-042"),
        step!(Op::CausesAsNotes, "EXC-043"),
        step!(Op::FieldsAsNotes(AGE_CONTACTS), "EXC-043"),
        step!(Op::IdentifiersAsNote(&["REFN", "RIN"]), "EXC-044"),
        step!(Op::InterpretedDatesAsNote, "EXC-045"),
        step!(Op::Preferred(&["_PRIM", "_PRIM_CUTOUT"], false), "EXC-046", "EXC-051"),
    ];
}

export_profile! {
    /// Geni.com.
    GENI = "geni", "Geni.com", tier 1;
    target Target {
        max_individuals: Some(5_000),
        rows: &["EXC-106"],
        ..Target::V551
    };
    steps [];
}

export_profile! {
    /// `FamilySearch` Genealogies upload: 5.5.1 only, 100 MB.
    FAMILYSEARCH = "familysearch", "FamilySearch", tier 1;
    target Target {
        max_bytes: Some(100_000_000),
        rows: &["EXC-052", "EXC-053"],
        ..Target::V551
    };
    steps [
        step!(Op::AddressesInPlace, "EXC-054"),
    ];
}

export_profile! {
    /// Personal Ancestral File 5.2.
    PAF = "paf", "Personal Ancestral File", tier 1;
    target Target {
        bom: true,
        line_ending: LineEnding::CrLf,
        max_name: Some(120),
        rows: &["EXC-098", "EXC-100", "EXC-099"],
        ..Target::V551
    };
    steps [
        step!(Op::NamesInAka, "EXC-096"),
        step!(Op::FieldsAsNotes(AGE), "EXC-097"),
        step!(Op::PafUids, "EXC-071"),
        step!(Op::Preferred(&["_PRIM"], false), "EXC-067"),
        step!(Op::Retag(&[("SDATE", "_SDATE")]), "EXC-064"),
    ];
}

export_profile! {
    /// Ancestral Quest 15 and 16.
    ANCESTRAL_QUEST = "ancestral-quest", "Ancestral Quest", tier 1;
    target Target {
        bom: true,
        rows: &["EXC-095"],
        ..Target::V551
    };
    steps [
        step!(Op::FlatHead, "EXC-094"),
        step!(Op::UsualNameAsNote, "EXC-089"),
        step!(Op::AsEven(AQ_UNREAD), "EXC-090"),
        step!(Op::AssociationsAsEvents(false), "EXC-091"),
        step!(Op::FieldsAsNotes(AGE_CONTACTS), "EXC-091"),
        step!(Op::AddressesInPlace, "EXC-091"),
        step!(Op::IdentifiersAsNote(&["REFN"]), "EXC-092"),
        step!(Op::NoteSourcesLifted, "EXC-092"),
        step!(Op::Preferred(&["_PRIM"], true), "EXC-093"),
        step!(Op::PafUids, "EXC-071"),
    ];
}

export_profile! {
    /// `RootsMagic` 8 to 11.
    ROOTSMAGIC = "rootsmagic", "RootsMagic", tier 2;
    target Target {
        max_name: Some(120),
        rows: &["EXC-072", "EXC-074"],
        ..Target::V551
    };
    steps [
        step!(Op::NameValueComplete, "EXC-058"),
        step!(Op::UsualNameAsNote, "EXC-058"),
        step!(Op::EventAssociations(AssocStyle::Shar), "EXC-063", "EXC-059"),
        step!(Op::AssociationsAsEvents(false), "EXC-059"),
        step!(Op::AsEven(ROOTSMAGIC_UNREAD), "EXC-060"),
        step!(Op::FactsAsEven, "EXC-061"),
        step!(Op::FieldsAsNotes(AGE_CONTACTS), "EXC-062"),
        step!(Op::CausesAsNotes, "EXC-062"),
        step!(Op::Retag(&[("SDATE", "_SDATE")]), "EXC-064"),
        step!(Op::SourceRecords, "EXC-065"),
        step!(Op::Preferred(&["_PRIM"], false), "EXC-067"),
        step!(Op::NamesWithSlashes, "EXC-069"),
        step!(Op::PafUids, "EXC-071"),
    ];
}

export_profile! {
    /// Legacy Family Tree 9 and 10.
    LEGACY = "legacy", "Legacy Family Tree", tier 2;
    target Target {
        max_children: Some(60),
        rows: &["EXC-075", "EXC-088"],
        ..Target::V551
    };
    steps [
        step!(Op::HomeFirst, "EXC-076"),
        step!(Op::EventAssociations(AssocStyle::Shar), "EXC-079"),
        step!(Op::AsEven(LEGACY_UNREAD), "EXC-077"),
        step!(Op::AssociationsAsEvents(false), "EXC-077"),
        step!(Op::FactsAsEven, "EXC-078"),
        step!(Op::ContactsJoined, "EXC-080"),
        step!(Op::NoteSourcesLifted, "EXC-081"),
        step!(Op::MarriageStatus, "EXC-082"),
        step!(Op::Preferred(&["_PRIM"], false), "EXC-085"),
        step!(Op::PafUids, "EXC-086"),
    ];
}

export_profile! {
    /// Gramps 5.x and 6.0.
    GRAMPS = "gramps", "Gramps", tier 2;
    target Target {
        media: MediaPaths::Relative,
        rows: &["EXF-067", "EXF-068", "EXF-072", "EXF-079", "EXF-082"],
        ..Target::V551
    };
    steps [
        step!(Op::OnePlace, "EXF-070"),
        step!(Op::CitationRefnToSource, "EXF-073"),
        step!(Op::NoteSourcesLifted, "EXF-073"),
        step!(Op::EventAssociations(AssocStyle::Witn), "EXF-075"),
        step!(Op::UnionsAsMarriage, "EXF-081"),
    ];
}

export_profile! {
    /// webtrees 2.2 (and the GEDCOM-L programs).
    WEBTREES = "webtrees", "webtrees", tier 2;
    target Target {
        media: MediaPaths::Relative,
        rows: &["EXF-083", "EXF-084", "EXF-086", "EXF-087", "EXF-090", "EXF-092"],
        ..Target::V551
    };
    steps [
        step!(Op::ShortXrefs(20), "EXF-089"),
        step!(Op::XreflessRecords, "EXF-088"),
        step!(Op::EventAssociations(AssocStyle::GedcomL), "EXF-093"),
    ];
}

export_profile! {
    /// Family Historian 7 and 8: GEDCOM 7.0.
    FAMILY_HISTORIAN = "family-historian", "Family Historian", tier 2;
    target Target {
        rows: &["EXC-126"],
        ..Target::V70
    };
    steps [
        step!(Op::EventAssociations(AssocStyle::Shar), "EXC-128", "EXC-127"),
    ];
}

export_profile! {
    /// `MacFamilyTree` 10 and 11: GEDCOM 7.0, media in a sibling folder.
    MACFAMILYTREE = "macfamilytree", "MacFamilyTree", tier 2;
    target Target {
        media: MediaPaths::Relative,
        rows: &["EXC-125"],
        ..Target::V70
    };
    steps [];
}

export_profile! {
    /// Ancestris (GEDCOM 5.5.1 import wizard).
    ANCESTRIS = "ancestris", "Ancestris", tier 3;
    target Target {
        rows: &["EXF-100", "EXF-098"],
        ..Target::V551
    };
    steps [
        step!(Op::Places(PlaceStyle::Heredis), "EXF-094"),
        step!(Op::EventsSaidY, "EXF-095"),
        step!(Op::FactsAsEven, "EXF-096"),
        step!(Op::EventAssociations(AssocStyle::Reversed), "EXF-097"),
        step!(Op::CitationRefnToSource, "EXF-099"),
    ];
}

export_profile! {
    /// Généatique 2024.
    GENEATIQUE = "geneatique", "Généatique", tier 3;
    target Target {
        rows: &["EXF-061", "EXF-062", "EXF-065", "EXF-066"],
        ..Target::V551
    };
    steps [
        step!(Op::PlainNotes, "EXF-064"),
    ];
}

export_profile! {
    /// TNG (The Next Generation of Genealogy Sitebuilding).
    TNG = "tng", "TNG", tier 3;
    target Target {
        rows: &["EXC-114", "EXC-116", "EXC-120", "EXC-121"],
        ..Target::V551
    };
    steps [
        step!(Op::FactsAsEven, "EXC-115"),
        step!(Op::ParentRelations, "EXC-117"),
        step!(Op::FaxAsPhone, "EXC-118"),
        step!(Op::Preferred(&["_PRIM"], false), "EXC-116"),
    ];
}

export_profile! {
    /// Reunion 9 to 14.
    REUNION = "reunion", "Reunion", tier 3;
    target Target {
        rows: &["EXC-122", "EXC-124"],
        ..Target::V551
    };
    steps [
        step!(Op::Preferred(&["_PRIM"], false), "EXC-122"),
        step!(Op::Retag(&[("UID", "_UID")]), "EXC-123"),
    ];
}

export_profile! {
    /// `WikiTree`'s `GEDCOMpare`.
    WIKITREE = "wikitree", "WikiTree", tier 3;
    target Target {
        max_individuals: Some(5_000),
        rows: &["EXC-101", "EXC-105"],
        ..Target::V551
    };
    steps [];
}

export_profile! {
    /// Findmypast.
    FINDMYPAST = "findmypast", "Findmypast", tier 3;
    target Target {
        max_bytes: Some(140_000_000),
        max_individuals: Some(120_000),
        rows: &["EXC-107"],
        ..Target::V551
    };
    steps [
        step!(Op::NameValueComplete, "EXC-108"),
        step!(Op::PrefixAsTitle, "EXC-108"),
        step!(Op::UsualNameAsNote, "EXC-108"),
        step!(Op::AssociationsAsEvents(false), "EXC-109"),
        step!(Op::AsEven(FINDMYPAST_UNREAD), "EXC-109"),
        step!(Op::FactsAsEven, "EXC-109"),
        step!(Op::FieldsAsNotes(AGE), "EXC-110"),
        step!(Op::CausesAsNotes, "EXC-110"),
        step!(Op::RecordNotesAsEvent("Family Story"), "EXC-111"),
        step!(Op::RecordCitationsOnEvent("Story"), "EXC-111"),
        step!(Op::PlainNotes, "EXC-016"),
    ];
}

export_profile! {
    /// Ahnenblatt 3.
    AHNENBLATT = "ahnenblatt", "Ahnenblatt", tier 3;
    target Target {
        rows: &["EXC-130"],
        ..Target::V551
    };
    steps [
        step!(Op::FlatHead, "EXC-129"),
        step!(Op::PafUids, "EXC-131"),
        step!(Op::GodparentText, "EXC-132"),
    ];
}
