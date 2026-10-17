//! GEDCOM 7.0 and 7.1 specifics: version detection and rules, the header's
//! schema, shared notes (`SNOTE`), identifiers (`UID`, `EXID`) and creation
//! dates (`CREA`), events known not to have happened (`NO`), phrases
//! (`PHRASE`), sort dates (`SDATE`), `@VOID@` pointers, translations
//! (`TRAN`), cropping (`CROP`), Latter-day Saint ordinances (7.x `INIL`),
//! 7.1 structures and `@` escaping. Every name and value is fictitious.

use oxidged::model::{
    BirthKind, Dataset, EventKind, NoteContent, NoteKind, OrdinanceKind, OrdinanceStatus, Text,
};
use oxidged::version::detect_version;
use oxidged::{GedcomBuilder, GedcomVersion, GedcomWriter, RepairKind};

/// A 7.0 file with these records.
fn v7(records: &str) -> String {
    format!("0 HEAD\n1 GEDC\n2 VERS 7.0\n{records}0 TRLR\n")
}

fn text(data: &Dataset, t: &Text) -> String {
    t.to_str(data).into_owned()
}

fn write(data: &Dataset) -> String {
    GedcomWriter::new().write_to_string(data).expect("write")
}

fn write_as(data: &Dataset, version: GedcomVersion) -> String {
    GedcomWriter::new()
        .gedcom_version(version)
        .write_to_string(data)
        .expect("write")
}

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

#[test]
fn a_minimal_file_is_read_as_7_0() {
    let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR");
    assert_eq!(data.version(), GedcomVersion::V7_0);
    assert!(data.version().is_v7());
    assert!(!data.version().is_v5());
    assert_eq!(data.declared_version(), Some("7.0"));
    assert_eq!(data.record_count(), 1); // the header alone
}

#[test]
fn versions_are_detected_from_the_header() {
    for vers in [
        "7.0", "7.0.0", "7.0.1", "7.0.14", "7.0.16", "7.0.17", "7.0.18",
    ] {
        let content = format!("0 HEAD\n1 GEDC\n2 VERS {vers}\n0 TRLR");
        assert_eq!(detect_version(&content), GedcomVersion::V7_0, "{vers}");
        assert_eq!(Dataset::parse(content).declared_version(), Some(vers));
    }
    for vers in ["5.5", "5.5.0", "5.5.1"] {
        let content = format!("0 HEAD\n1 GEDC\n2 VERS {vers}\n2 FORM LINEAGE-LINKED\n0 TRLR");
        assert_eq!(detect_version(&content), GedcomVersion::V5_5_1, "{vers}");
    }
    let v71 = "0 HEAD\n1 GEDC\n2 VERS 7.1\n0 TRLR";
    assert_eq!(detect_version(v71), GedcomVersion::V7_1);
    assert_eq!(Dataset::parse(v71).version(), GedcomVersion::V7_1);
    // No header: 5.5.1's rules apply.
    assert_eq!(detect_version("0 @I1@ INDI\n0 TRLR"), GedcomVersion::V5_5_1);
}

#[test]
fn version_strings_and_rules() {
    for (vers, version) in [
        ("5.5.1", GedcomVersion::V5_5_1),
        ("5.5", GedcomVersion::V5_5_1),
        ("7.0", GedcomVersion::V7_0),
        ("7.0.14", GedcomVersion::V7_0),
        ("7.1", GedcomVersion::V7_1),
        // Anything that is not 7.x follows the 5.5.1 rules.
        ("6.0", GedcomVersion::V5_5_1),
    ] {
        assert_eq!(GedcomVersion::from_version_str(vers), version, "{vers}");
    }

    let v5 = GedcomVersion::V5_5_1;
    assert_eq!(v5.as_str(), "5.5.1");
    assert!(v5.rules().uses_conc());
    assert!(v5.rules().doubles_every_at_sign());
    assert!(v5.rules().has_head_char());
    assert_eq!(v5.rules().gedc_form(), Some("LINEAGE-LINKED"));

    for v7 in [GedcomVersion::V7_0, GedcomVersion::V7_1] {
        assert!(!v7.rules().uses_conc());
        assert!(!v7.rules().doubles_every_at_sign());
        assert!(!v7.rules().has_head_char());
        assert_eq!(v7.rules().gedc_form(), None);
    }
    assert_eq!(GedcomVersion::V7_0.as_str(), "7.0");
    assert_eq!(GedcomVersion::V7_1.as_str(), "7.1");
}

#[test]
fn a_5_5_1_file_keeps_its_version_and_its_note_records() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n\
         0 @I1@ INDI\n1 NAME Ann /Example/\n0 @U1@ SUBM\n1 NAME Bea Sample\n\
         0 @N1@ NOTE A note record\n0 TRLR",
    );
    assert_eq!(data.version(), GedcomVersion::V5_5_1);
    assert_eq!(data.individuals().len(), 1);
    assert_eq!(data.submitters().len(), 1);
    // A 5.5.1 NOTE record is a shared note, written SNOTE in 7.0.
    assert_eq!(
        text(&data, &data.notes().get(0).unwrap().into_owned().text),
        "A note record"
    );
    assert!(write(&data).contains("0 @N1@ NOTE A note record\n"));
    assert!(write_as(&data, GedcomVersion::V7_0).contains("0 @N1@ SNOTE A note record\n"));
}

// ---------------------------------------------------------------------------
// The header
// ---------------------------------------------------------------------------

#[test]
fn the_header_keeps_its_schema_and_source() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n1 SOUR SAMPLE_APP\n2 VERS 1.0\n2 NAME Sample Application\n\
         1 SCHMA\n2 TAG _CUSTOM https://example.com/custom\n2 TAG _OTHER https://example.com/other\n\
         0 TRLR",
    );
    let header = data.header().unwrap();
    assert_eq!(header.declared_version(&data).as_deref(), Some("7.0"));

    let source = header.source.as_ref().unwrap();
    assert_eq!(text(&data, &source.product), "SAMPLE_APP");
    assert_eq!(
        text(&data, source.name.as_ref().unwrap()),
        "Sample Application"
    );
    assert_eq!(text(&data, source.version.as_ref().unwrap()), "1.0");

    let tags: Vec<_> = header
        .schema
        .as_ref()
        .unwrap()
        .tags
        .iter()
        .map(|t| text(&data, t))
        .collect();
    assert_eq!(
        tags,
        [
            "_CUSTOM https://example.com/custom",
            "_OTHER https://example.com/other"
        ]
    );

    let out = write(&data);
    assert!(out.contains("1 SCHMA\n2 TAG _CUSTOM https://example.com/custom\n2 TAG _OTHER https://example.com/other\n"));
}

#[test]
fn the_7_1_header_has_a_title_and_a_description() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.1\n1 TITL The Example family\n2 LANG en\n\
         2 TRAN La famille Example\n3 LANG fr\n1 DESC Descendants of Ann Example\n0 TRLR\n",
    );
    let header = data.header().unwrap();
    let title = header.title.as_ref().unwrap();
    assert_eq!(text(&data, &title.text), "The Example family");
    assert_eq!(text(&data, title.language.as_ref().unwrap()), "en");
    assert_eq!(
        text(&data, &title.translations[0].text),
        "La famille Example"
    );
    assert_eq!(
        text(&data, &header.description.as_ref().unwrap().text),
        "Descendants of Ann Example"
    );
    assert!(
        write(&data).contains(
            "1 TITL The Example family\n2 LANG en\n2 TRAN La famille Example\n3 LANG fr\n"
        )
    );
}

// ---------------------------------------------------------------------------
// Shared notes
// ---------------------------------------------------------------------------

#[test]
fn shared_note_records() {
    let data = Dataset::parse(v7("0 @I1@ INDI\n1 NAME Ann /Example/\n\
         0 @N1@ SNOTE A shared note about the Example surname.\n\
         0 @N2@ SNOTE First line.\n1 CONT Second line.\n1 CONT Third line.\n"));
    assert_eq!(data.notes().len(), 2);
    // The header, one individual and two shared notes.
    assert_eq!(data.record_count(), 4);

    let n1 = data.find_note("@N1@").unwrap().into_owned();
    assert_eq!(data.store().xref(n1.xref.unwrap()), "@N1@");
    assert_eq!(
        text(&data, &n1.text),
        "A shared note about the Example surname."
    );

    let n2 = data.find_note("@N2@").unwrap().into_owned();
    assert_eq!(
        text(&data, &n2.text),
        "First line.\nSecond line.\nThird line."
    );

    let out = write(&data);
    assert!(out.contains("0 @N2@ SNOTE First line.\n1 CONT Second line.\n1 CONT Third line.\n"));
}

#[test]
fn shared_notes_keep_their_media_type_language_and_translations() {
    let data = Dataset::parse(v7(
        "0 @N1@ SNOTE <p>Some <b>HTML</b> content.</p>\n1 MIME text/html\n1 LANG en\n\
         1 TRAN <p>Du contenu <b>HTML</b>.</p>\n2 LANG fr\n",
    ));
    let note = data.find_note("@N1@").unwrap().into_owned();
    assert!(note.mime.as_ref().unwrap().eq_str(&data, "text/html"));
    assert!(note.language.as_ref().unwrap().eq_str(&data, "en"));
    let tran = &note.translations[0];
    assert_eq!(text(&data, &tran.text), "<p>Du contenu <b>HTML</b>.</p>");
    assert!(tran.language.as_ref().unwrap().eq_str(&data, "fr"));

    let out = write(&data);
    assert!(out.contains(
        "0 @N1@ SNOTE <p>Some <b>HTML</b> content.</p>\n1 MIME text/html\n1 LANG en\n\
         1 TRAN <p>Du contenu <b>HTML</b>.</p>\n2 LANG fr\n"
    ));
}

#[test]
fn snote_pointers_and_note_texts() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 SNOTE @N1@\n1 NOTE Own text\n2 MIME text/plain\n0 @N1@ SNOTE Shared\n",
    ));
    let notes_record = data.individuals().get(0).unwrap().into_owned();
    let notes = &notes_record.notes;
    let shared = data.store().find_xref("@N1@").unwrap();
    assert_eq!(notes[0].content, NoteContent::Shared(shared));
    assert!(matches!(&notes[1].content, NoteContent::Text(t) if t.eq_str(&data, "Own text")));
    assert!(
        notes[1]
            .detail()
            .mime
            .as_ref()
            .unwrap()
            .eq_str(&data, "text/plain")
    );
    assert!(data.dangling_references().is_empty());

    let out = write(&data);
    assert!(out.contains("1 SNOTE @N1@\n1 NOTE Own text\n2 MIME text/plain\n"));
    // 5.5.1 points with NOTE.
    assert!(write_as(&data, GedcomVersion::V5_5_1).contains("1 NOTE @N1@\n"));
}

#[test]
fn note_kinds_of_7_1() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.1\n0 @I1@ INDI\n1 NOTE To check\n2 KIND TODO\n\
         0 @N1@ SNOTE Collected\n1 KIND OTHER\n2 PHRASE Hearsay\n0 TRLR\n",
    );
    let note_record = data.individuals().get(0).unwrap().into_owned();
    let note = &note_record.notes[0];
    assert_eq!(note.detail().kinds[0].value, NoteKind::Todo);
    let kind_record = data.notes().get(0).unwrap().into_owned();
    let kind = &kind_record.kinds[0];
    assert_eq!(kind.value, NoteKind::Other);
    assert_eq!(text(&data, kind.phrase.as_ref().unwrap()), "Hearsay");
    assert!(write(&data).contains("1 KIND OTHER\n2 PHRASE Hearsay\n"));
}

// ---------------------------------------------------------------------------
// Identifiers and creation dates
// ---------------------------------------------------------------------------

#[test]
fn uid_exid_and_creation_dates() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 UID 0d7c1d2a-3b4e-4f60-8a9b-0c1d2e3f4a5b\n\
         1 EXID 123\n2 TYPE https://example.com/ids\n1 EXID 456\n\
         1 CREA\n2 DATE 1 JAN 2020\n3 TIME 12:30:00Z\n\
         0 @S1@ SOUR\n1 UID 1e2f3a4b-5c6d-4e7f-8a9b-0c1d2e3f4a5c\n1 CREA\n2 DATE 2 FEB 2021\n",
    ));
    let detail_record = data.individuals().get(0).unwrap().into_owned();
    let detail = detail_record.detail();
    assert_eq!(
        text(&data, &detail.uids[0]),
        "0d7c1d2a-3b4e-4f60-8a9b-0c1d2e3f4a5b"
    );
    assert_eq!(detail.exids.len(), 2);
    assert_eq!(text(&data, &detail.exids[0].value), "123");
    assert_eq!(
        text(&data, detail.exids[0].kind.as_ref().unwrap()),
        "https://example.com/ids"
    );
    assert!(detail.exids[1].kind.is_none());
    let created = detail.creation.as_ref().unwrap().date.as_ref().unwrap();
    assert_eq!(text(&data, &created.value), "1 JAN 2020");
    assert_eq!(text(&data, created.time.as_ref().unwrap()), "12:30:00Z");

    let source = data.find_source("@S1@").unwrap().into_owned();
    assert_eq!(source.uids.len(), 1);
    assert!(source.creation.is_some());

    let out = write(&data);
    assert!(out.contains("1 EXID 123\n2 TYPE https://example.com/ids\n1 EXID 456\n"));
    assert!(out.contains("1 CREA\n2 DATE 1 JAN 2020\n3 TIME 12:30:00Z\n"));
}

// ---------------------------------------------------------------------------
// Events: NO, SDATE, PHRASE
// ---------------------------------------------------------------------------

#[test]
fn individual_non_events() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 NO MARR\n2 DATE TO 1900\n3 PHRASE Before the war\n\
         2 NOTE Never married, per family records.\n",
    ));
    let non_events_record = data.individuals().get(0).unwrap().into_owned();
    let non_events = &non_events_record.detail().non_events;
    assert_eq!(non_events.len(), 1);
    let no = &non_events[0];
    assert_eq!(no.kind, EventKind::Marriage);
    let date = no.date.as_ref().unwrap();
    assert_eq!(text(&data, &date.value), "TO 1900");
    assert_eq!(text(&data, date.phrase.as_ref().unwrap()), "Before the war");
    assert_eq!(no.notes.len(), 1);
    assert!(
        data.individuals()
            .get(0)
            .unwrap()
            .into_owned()
            .events
            .is_empty()
    );

    assert!(write(&data).contains("1 NO MARR\n2 DATE TO 1900\n3 PHRASE Before the war\n"));
}

#[test]
fn family_non_events() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n0 @I2@ INDI\n0 @F1@ FAM\n1 HUSB @I1@\n1 WIFE @I2@\n\
         1 NO DIV\n2 NOTE The couple never divorced.\n",
    ));
    let no_record = data.families().get(0).unwrap().into_owned();
    let no = &no_record.detail().non_events[0];
    assert_eq!(no.kind, EventKind::Divorce);
    assert_eq!(no.notes.len(), 1);
    assert!(write(&data).contains("1 NO DIV\n2 NOTE The couple never divorced.\n"));
}

#[test]
fn sort_dates() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 BIRT\n2 DATE BEF 1820\n2 SDATE 1818\n3 TIME 06:00\n",
    ));
    let birth_record = data.individuals().get(0).unwrap().into_owned();
    let birth = birth_record.birth().unwrap();
    assert_eq!(text(&data, &birth.date.as_ref().unwrap().value), "BEF 1820");
    let sdate = birth.detail().sort_date.as_ref().unwrap();
    assert_eq!(text(&data, &sdate.value), "1818");
    assert_eq!(text(&data, sdate.detail().time.as_ref().unwrap()), "06:00");

    assert!(write(&data).contains("2 DATE BEF 1820\n2 SDATE 1818\n3 TIME 06:00\n"));
}

#[test]
fn date_phrases() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 BIRT\n2 DATE 15 MAR 1820\n3 PHRASE The Ides of March, 1820\n",
    ));
    let ann = data.individuals().get(0).unwrap().into_owned();
    let date = ann.birth().unwrap().date.as_ref().unwrap();
    assert_eq!(text(&data, &date.value), "15 MAR 1820");
    assert_eq!(
        text(&data, date.detail().phrase.as_ref().unwrap()),
        "The Ides of March, 1820"
    );

    assert!(write(&data).contains("2 DATE 15 MAR 1820\n3 PHRASE The Ides of March, 1820\n"));
    // 5.5.1 words the date as an interpreted date.
    assert!(
        write_as(&data, GedcomVersion::V5_5_1)
            .contains("2 DATE INT 15 MAR 1820 (The Ides of March, 1820)\n")
    );
}

#[test]
fn enumeration_phrases() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 FAMC @F1@\n2 PEDI OTHER\n3 PHRASE Guardianship\n\
         0 @F1@ FAM\n1 CHIL @I1@\n",
    ));
    let ann = data.individuals().get(0).unwrap().into_owned();
    let pedigree = ann.child_of[0].detail().pedigree.as_ref().unwrap();
    assert_eq!(pedigree.value, oxidged::model::Pedigree::Other);
    assert_eq!(
        text(&data, pedigree.phrase.as_ref().unwrap()),
        "Guardianship"
    );
    assert!(write(&data).contains("2 PEDI OTHER\n3 PHRASE Guardianship\n"));
}

#[test]
fn birth_kinds_of_7_1() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.1\n0 @I1@ INDI\n1 BIRT\n2 KIND BORN_LIVE\n0 TRLR\n",
    );
    let birth_record = data.individuals().get(0).unwrap().into_owned();
    let birth = birth_record.birth().unwrap();
    assert_eq!(birth.detail().birth_kinds, [BirthKind::BornLive]);
    assert!(write(&data).contains("1 BIRT\n2 KIND BORN_LIVE\n"));
}

// ---------------------------------------------------------------------------
// @VOID@ pointers
// ---------------------------------------------------------------------------

#[test]
fn void_pointers_point_to_nothing() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 FAMC @VOID@\n1 ASSO @VOID@\n2 PHRASE A neighbour\n2 ROLE WITN\n\
         0 @F1@ FAM\n1 HUSB @VOID@\n2 PHRASE Unknown father\n1 WIFE @I1@\n",
    ));
    let void = data.store().find_xref("@VOID@").unwrap();
    let indi = &data.individuals().get(0).unwrap().into_owned();
    assert_eq!(indi.child_of[0].family, Some(void));
    let asso = &indi.detail().associations[0];
    assert_eq!(asso.individual, Some(void));
    assert_eq!(text(&data, asso.phrase.as_ref().unwrap()), "A neighbour");

    let family = &data.families().get(0).unwrap().into_owned();
    assert_eq!(family.husband_id(), Some(void));
    assert_eq!(
        text(
            &data,
            family
                .husband
                .as_ref()
                .unwrap()
                .detail()
                .phrase
                .as_ref()
                .unwrap()
        ),
        "Unknown father"
    );
    // @VOID@ names no record: no parent, no dangling reference.
    assert_eq!(data.parents(data.families().first().unwrap()).count(), 1);
    assert!(data.find("@VOID@").is_none());
    assert!(data.dangling_references().is_empty());

    let out = write(&data);
    assert!(out.contains("1 HUSB @VOID@\n2 PHRASE Unknown father\n"));
    assert!(out.contains("1 FAMC @VOID@\n"));
}

// ---------------------------------------------------------------------------
// Translations and multimedia
// ---------------------------------------------------------------------------

#[test]
fn name_and_file_translations() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n2 TRAN Анна /Пример/\n3 LANG ru\n3 GIVN Анна\n\
         0 @M1@ OBJE\n1 FILE media/portrait.jpg\n2 FORM image/jpeg\n\
         2 TRAN media/portrait-small.jpg\n3 FORM image/jpeg\n",
    ));
    let name_record = data.individuals().get(0).unwrap().into_owned();
    let name = name_record.name().unwrap();
    let tran = &name.detail().translations[0];
    assert_eq!(text(&data, &tran.value), "Анна /Пример/");
    assert!(tran.language.as_ref().unwrap().eq_str(&data, "ru"));
    assert_eq!(text(&data, &tran.pieces[0].value), "Анна");

    let file_record = data.find_multimedia("@M1@").unwrap().into_owned();

    let file = &file_record.files[0];
    assert_eq!(text(&data, &file.path), "media/portrait.jpg");
    let small = &file.translations[0];
    assert_eq!(text(&data, &small.path), "media/portrait-small.jpg");
    assert!(
        small
            .form
            .as_ref()
            .unwrap()
            .format
            .eq_str(&data, "image/jpeg")
    );

    let out = write(&data);
    assert!(out.contains("2 TRAN Анна /Пример/\n3 LANG ru\n3 GIVN Анна\n"));
    assert!(out.contains("2 TRAN media/portrait-small.jpg\n3 FORM image/jpeg\n"));
}

#[test]
fn crop_of_a_multimedia_link() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 OBJE @M1@\n2 CROP\n3 TOP 10\n3 LEFT 15\n3 HEIGHT 50\n3 WIDTH 40\n\
         2 TITL Portrait\n\
         0 @M1@ OBJE\n1 FILE photo.jpg\n2 FORM image/jpeg\n",
    ));
    let link_record = data.individuals().get(0).unwrap().into_owned();
    let link = &link_record.detail().multimedia[0];
    assert_eq!(link.object, data.store().find_xref("@M1@"));
    let crop = link.crop.as_ref().unwrap();
    assert_eq!(
        (crop.top, crop.left, crop.height, crop.width),
        (Some(10), Some(15), Some(50), Some(40))
    );
    assert_eq!(text(&data, link.title.as_ref().unwrap()), "Portrait");

    assert!(write(&data).contains(
        "1 OBJE @M1@\n2 CROP\n3 TOP 10\n3 LEFT 15\n3 HEIGHT 50\n3 WIDTH 40\n2 TITL Portrait\n"
    ));
}

#[test]
fn crop_values_that_are_not_pixels_are_kept_aside() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 OBJE @M1@\n2 CROP\n3 TOP 10.5\n3 LEFT 3\n0 @M1@ OBJE\n1 FILE a.jpg\n\
         2 FORM image/jpeg\n2 CROP\n",
    ));
    let ann = data.individuals().get(0).unwrap().into_owned();
    let crop = ann.detail().multimedia[0].crop.as_ref().unwrap();
    assert_eq!(crop.top, None);
    assert_eq!(crop.left, Some(3));
    assert_eq!(crop.extra.len(), 1);
    assert_eq!(data.store().tag(crop.extra[0].tag), "TOP");
    // A CROP under a record's FILE has no field: it is kept where it was.
    let file_record = data.multimedia().get(0).unwrap().into_owned();
    let file = &file_record.files[0];
    assert_eq!(data.store().tag(file.extra[0].tag), "CROP");
}

// ---------------------------------------------------------------------------
// Latter-day Saint ordinances
// ---------------------------------------------------------------------------

#[test]
fn individual_ordinances_of_5_5_1() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
         1 BAPL\n2 DATE 15 MAR 1990\n2 TEMP SLAKE\n2 STAT COMPLETED\n\
         1 CONL\n2 DATE 16 MAR 1990\n2 TEMP SLAKE\n\
         1 ENDL\n2 DATE 17 MAR 1991\n2 TEMP SLAKE\n2 STAT COMPLETED\n0 TRLR",
    );
    let ordinances_record = data.individuals().get(0).unwrap().into_owned();
    let ordinances = &ordinances_record.detail().ordinances;
    let kinds: Vec<_> = ordinances.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [
            OrdinanceKind::Baptism,
            OrdinanceKind::Confirmation,
            OrdinanceKind::Endowment
        ]
    );
    let bapl = &ordinances[0];
    assert_eq!(
        text(&data, &bapl.date.as_ref().unwrap().value),
        "15 MAR 1990"
    );
    assert!(bapl.temple.as_ref().unwrap().eq_str(&data, "SLAKE"));
    assert_eq!(
        bapl.status.as_ref().unwrap().value,
        OrdinanceStatus::Completed
    );
    assert!(ordinances[1].status.is_none());
}

#[test]
fn initiatory_is_read_in_7_0() {
    let data = Dataset::parse(v7("0 @I1@ INDI\n1 NAME Bea /Example/\n\
         1 BAPL\n2 DATE 10 JAN 2000\n2 TEMP SLAKE\n\
         1 CONL\n2 DATE 10 JAN 2000\n2 TEMP SLAKE\n\
         1 INIL\n2 DATE 15 FEB 2001\n2 TEMP SLAKE\n2 STAT COMPLETED\n3 DATE 16 FEB 2001\n\
         1 ENDL\n2 DATE 15 FEB 2001\n2 TEMP SLAKE\n"));
    let ordinances_record = data.individuals().get(0).unwrap().into_owned();
    let ordinances = &ordinances_record.detail().ordinances;
    assert_eq!(ordinances.len(), 4);
    let inil = &ordinances[2];
    assert_eq!(inil.kind, OrdinanceKind::Initiatory);
    assert_eq!(inil.kind.tag(), "INIL");
    assert_eq!(
        text(&data, &inil.date.as_ref().unwrap().value),
        "15 FEB 2001"
    );
    assert!(inil.temple.as_ref().unwrap().eq_str(&data, "SLAKE"));
    let status = inil.status.as_ref().unwrap();
    assert_eq!(status.value, OrdinanceStatus::Completed);
    assert_eq!(
        text(&data, &status.date.as_ref().unwrap().value),
        "16 FEB 2001"
    );
}

#[test]
fn sealings_of_children_and_spouses() {
    let data = Dataset::parse(v7("0 @I1@ INDI\n1 NAME Cid /Example/\n\
         1 SLGC\n2 DATE 20 MAR 1995\n2 TEMP SLAKE\n2 FAMC @F1@\n2 STAT COMPLETED\n3 DATE 21 MAR 1995\n\
         0 @F1@ FAM\n1 CHIL @I1@\n\
         1 SLGS\n2 DATE 25 DEC 1990\n2 TEMP SLAKE\n2 STAT COMPLETED\n3 DATE 26 DEC 1990\n"));
    let slgc_record = data.individuals().get(0).unwrap().into_owned();
    let slgc = &slgc_record.detail().ordinances[0];
    assert_eq!(slgc.kind, OrdinanceKind::ChildSealing);
    assert_eq!(slgc.family, data.store().find_xref("@F1@"));
    assert_eq!(
        slgc.status.as_ref().unwrap().value,
        OrdinanceStatus::Completed
    );

    let slgs_record = data.families().get(0).unwrap().into_owned();

    let slgs = &slgs_record.detail().ordinances[0];
    assert_eq!(slgs.kind, OrdinanceKind::SpouseSealing);
    assert_eq!(
        text(&data, &slgs.date.as_ref().unwrap().value),
        "25 DEC 1990"
    );
    assert_eq!(
        slgs.status.as_ref().unwrap().value,
        OrdinanceStatus::Completed
    );
}

#[test]
fn ordinances_round_trip() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 NAME Dee /Example/\n1 BAPL\n2 DATE 1 JAN 2000\n2 TEMP SLAKE\n\
         1 INIL\n2 DATE 2 JAN 2001\n2 TEMP SLAKE\n2 STAT COMPLETED\n3 DATE 3 JAN 2001\n",
    ));
    let out = write(&data);
    assert!(out.contains("1 BAPL\n2 DATE 1 JAN 2000\n2 TEMP SLAKE\n"));
    assert!(out.contains(
        "1 INIL\n2 DATE 2 JAN 2001\n2 TEMP SLAKE\n2 STAT COMPLETED\n3 DATE 3 JAN 2001\n"
    ));

    let reread = Dataset::parse(out);
    assert_eq!(reread.to_structures(), data.to_structures());
    let kinds: Vec<_> = reread
        .individuals()
        .get(0)
        .unwrap()
        .into_owned()
        .detail()
        .ordinances
        .iter()
        .map(|o| o.kind)
        .collect();
    assert_eq!(kinds, [OrdinanceKind::Baptism, OrdinanceKind::Initiatory]);
}

// ---------------------------------------------------------------------------
// @ escaping
// ---------------------------------------------------------------------------

#[test]
fn only_a_leading_at_sign_is_doubled_in_7_0() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 NOTE @@ref and ann@example.com\n1 NOTE a@@b\n",
    ));
    let notes_record = data.individuals().get(0).unwrap().into_owned();
    let notes = &notes_record.notes;
    assert!(
        matches!(&notes[0].content, NoteContent::Text(t) if t.eq_str(&data, "@ref and ann@example.com"))
    );
    // Past the first character, @@ is two at signs in 7.0.
    assert!(matches!(&notes[1].content, NoteContent::Text(t) if t.eq_str(&data, "a@@b")));

    let out = write(&data);
    assert!(out.contains("1 NOTE @@ref and ann@example.com\n1 NOTE a@@b\n"));
    // 5.5.1 doubles every at sign.
    assert!(
        write_as(&data, GedcomVersion::V5_5_1)
            .contains("1 NOTE @@ref and ann@@example.com\n1 NOTE a@@@@b\n")
    );
}

#[test]
fn every_doubled_at_sign_is_one_in_5_5_1() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NOTE ann@@example.com\n0 TRLR\n",
    );
    let note_record = data.individuals().get(0).unwrap().into_owned();
    let note = &note_record.notes[0];
    assert!(matches!(&note.content, NoteContent::Text(t) if t.eq_str(&data, "ann@example.com")));
    assert!(write_as(&data, GedcomVersion::V7_0).contains("1 NOTE ann@example.com\n"));
}

// ---------------------------------------------------------------------------
// Strictness
// ---------------------------------------------------------------------------

#[test]
fn structures_of_the_other_version_are_kept_and_strict_mode_refuses_them() {
    // NO is a 7.x structure; a 5.5.1 file that has one keeps it.
    let text = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n\
                1 SOUR SAMPLE\n1 SUBM @U1@\n0 @U1@ SUBM\n1 NAME Bea Sample\n\
                0 @I1@ INDI\n1 NO MARR\n0 TRLR\n";
    let data = GedcomBuilder::new().build_from_str(text).unwrap();
    assert_eq!(
        data.individuals()
            .get(0)
            .unwrap()
            .into_owned()
            .detail()
            .non_events
            .len(),
        1
    );
    assert!(
        GedcomBuilder::new()
            .strict(true)
            .build_from_str(text)
            .is_err()
    );
    // The same structure in 7.0 is conformant.
    let v70 = v7("0 @I1@ INDI\n1 NO MARR\n");
    assert!(
        GedcomBuilder::new()
            .strict(true)
            .build_from_str(v70)
            .is_ok()
    );
}

#[test]
fn structures_5_5_1_lacks_are_written_to_it_as_extensions() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 INIL\n2 TEMP SLAKE\n1 NO MARR\n1 UID abc\n1 FAMC @VOID@\n",
    ));
    let (out, report) = GedcomWriter::new()
        .gedcom_version(GedcomVersion::V5_5_1)
        .write_to_string_with_report(&data)
        .unwrap();
    // The repaired INIL is ordered where the typed model puts it, ahead of
    // the extensions the model keeps.
    assert!(out.contains(
        "0 @I1@ INDI\n1 _INIL\n2 TEMP SLAKE\n1 _FAMC @@VOID@@\n1 _NO MARR\n1 _UID abc\n"
    ));
    let kinds: Vec<_> = report.repairs.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            RepairKind::Pointer,
            RepairKind::Misplaced,
            RepairKind::Misplaced,
            RepairKind::Misplaced
        ]
    );
}

// ---------------------------------------------------------------------------
// GEDCOM 7.0.17 and 7.0.18
// ---------------------------------------------------------------------------

/// A source and the image it shows citing each other, a shared note citing
/// the source that points to it, and an individual its own alias: 7.0.17
/// prohibits each. Reading keeps them (nothing follows pointers in a loop);
/// the writer cuts the pointer that closes each cycle, in the order it
/// writes the records, and the output reads back and writes as it is.
#[test]
fn pointer_cycles_7_0_17_are_read_and_cut_on_write() {
    use oxidged::spec::{DeviationKind, validate_text};
    let input = v7(
        "0 @I1@ INDI\n1 ALIA @I1@\n1 ALIA @I2@\n0 @I2@ INDI\n1 ALIA @I1@\n\
         0 @N1@ SNOTE Transcribed from the register\n1 SOUR @S1@\n\
         0 @S1@ SOUR\n1 TITL Register\n1 SNOTE @N1@\n1 OBJE @O1@\n\
         0 @O1@ OBJE\n1 FILE scan.jpg\n2 FORM image/jpeg\n1 SOUR @S1@\n2 PAGE 4\n",
    );
    let cycles: Vec<u32> = validate_text(&input)
        .iter()
        .filter(|d| d.kind == DeviationKind::Cycle)
        .map(|d| d.line)
        .collect();
    assert_eq!(cycles, [5, 13, 18]);
    let data = Dataset::parse(&input);
    assert_eq!(
        data.individuals()
            .get(0)
            .unwrap()
            .into_owned()
            .detail()
            .aliases
            .len(),
        2
    );
    let index = oxidged::IndexedDataset::new(data.clone());
    assert!(index.find_source("@S1@").is_some());

    let (out, report) = GedcomWriter::new()
        .write_to_string_with_report(&data)
        .unwrap();
    assert!(
        out.contains("0 @I1@ INDI\n1 ALIA @I2@\n1 _ALIA @I1@\n"),
        "{out}"
    );
    assert!(out.contains("0 @I2@ INDI\n1 ALIA @I1@\n"), "{out}");
    // The typed model writes shared notes, then sources, then media: the
    // source's pointers close both cycles.
    assert!(
        out.contains("0 @N1@ SNOTE Transcribed from the register\n1 SOUR @S1@\n"),
        "{out}"
    );
    assert!(
        out.contains("0 @S1@ SOUR\n1 TITL Register\n1 OBJE @O1@\n1 _SNOTE @N1@\n"),
        "{out}"
    );
    assert!(
        out.contains("0 @O1@ OBJE\n1 FILE scan.jpg\n2 FORM image/jpeg\n1 _SOUR @S1@\n2 PAGE 4\n"),
        "{out}"
    );
    let kinds: Vec<_> = report.repairs.iter().map(|r| r.kind).collect();
    assert_eq!(kinds, [RepairKind::Cycle; 3], "{:?}", report.repairs);
    assert!(validate_text(&out).is_empty(), "{:?}", validate_text(&out));
    let again = Dataset::parse(&out);
    let (twice, report) = GedcomWriter::new()
        .write_to_string_with_report(&again)
        .unwrap();
    assert_eq!(twice, out);
    assert!(report.repairs.is_empty(), "{:?}", report.repairs);
    // 5.5.1 has neither rule.
    let out = write_as(&data, GedcomVersion::V5_5_1);
    assert!(
        out.contains("1 ALIA @I1@\n") && !out.contains("_ALIA"),
        "{out}"
    );
}

/// The examples 7.0.17 added: the ages of the partners at a family event
/// (`HUSB.AGE`, `WIFE.AGE`, §HUSB, §WIFE), a `PHRASE` beside a value other
/// than `OTHER` (§NAME-TYPE: a pen name), a note in a `text/` media type
/// of its own (§MIME: an extension that needs no extension tag), and the
/// coordinates and tag definitions of the new data types. Each is read
/// typed and written as it was, and validates.
#[test]
fn the_7_0_17_examples_read_and_write_as_they_are() {
    use oxidged::model::NameType;
    use oxidged::spec::validate_text;
    let input = v7(
        "0 @I1@ INDI\n1 NAME Mark /Twain/\n2 TYPE PROFESSIONAL\n3 PHRASE Pen\n\
         1 RESI\n2 PLAC Sampletown\n3 MAP\n4 LATI N18.150944\n4 LONG E168.150944\n\
         1 EXID 123\n2 TYPE https://registry.example.invalid/ids\n\
         0 @I2@ INDI\n\
         0 @F1@ FAM\n1 HUSB @I1@\n1 WIFE @I2@\n1 MARR\n2 HUSB\n3 AGE 32y\n\
         0 @N1@ SNOTE # Census\n1 MIME text/markdown\n",
    )
    .replace(
        "2 VERS 7.0\n",
        "2 VERS 7.0\n1 SCHMA\n2 TAG _SKYPEID http://xmlns.com/foaf/0.1/skypeID\n",
    );
    assert!(
        validate_text(&input).is_empty(),
        "{:?}",
        validate_text(&input)
    );
    let data = Dataset::parse(&input);
    let name_record = data.find_individual("@I1@").unwrap().into_owned();
    let name = &name_record.names[0];
    let kind = name.detail().kind.as_ref().unwrap();
    assert_eq!(kind.value, NameType::Professional);
    assert_eq!(text(&data, kind.phrase.as_ref().unwrap()), "Pen");
    let marriage_record = data.find_family("@F1@").unwrap().into_owned();
    let marriage = &marriage_record.events[0];
    let husband = marriage.detail().husband.as_ref().unwrap();
    assert_eq!(text(&data, &husband.age.as_ref().unwrap().value), "32y");
    assert!(marriage.detail().wife.is_none());
    let note = &data.notes().get(0).unwrap().into_owned();
    assert_eq!(text(&data, note.mime.as_ref().unwrap()), "text/markdown");
    let (out, report) = GedcomWriter::new()
        .write_to_string_with_report(&data)
        .unwrap();
    assert!(report.repairs.is_empty(), "{:?}", report.repairs);
    for lines in [
        "1 NAME Mark /Twain/\n2 TYPE PROFESSIONAL\n3 PHRASE Pen\n",
        "3 MAP\n4 LATI N18.150944\n4 LONG E168.150944\n",
        "1 EXID 123\n2 TYPE https://registry.example.invalid/ids\n",
        "1 MARR\n2 HUSB\n3 AGE 32y\n",
        "0 @N1@ SNOTE # Census\n1 MIME text/markdown\n",
        "2 TAG _SKYPEID http://xmlns.com/foaf/0.1/skypeID\n",
    ] {
        assert!(out.contains(lines), "{lines:?} in {out}");
    }
}

/// A web-address file with characters a URL string does not permit (a
/// space) is percent-encoded when written in 7.x, as local paths are, and
/// its multimedia record stays a record (it used to become `_OBJE`). The
/// encoded address reads back and writes again unchanged.
#[test]
fn web_file_addresses_are_percent_encoded() {
    let v551 = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @M1@ OBJE\n1 FILE https://example.org/a b.jpg\n\
                2 FORM jpg\n0 @M2@ OBJE\n1 FILE photos/a b.jpg\n2 FORM jpg\n0 TRLR\n";
    let data = oxidged::Dataset::parse(v551);
    let (out, report) = GedcomWriter::new()
        .gedcom_version(GedcomVersion::V7_0)
        .write_to_string_with_report(&data)
        .unwrap();
    assert!(
        out.contains("1 FILE https://example.org/a%20b.jpg\n"),
        "{out}"
    );
    assert!(out.contains("1 FILE photos/a%20b.jpg\n"), "{out}");
    assert!(!out.contains("_OBJE") && !out.contains("_FILE"), "{out}");
    assert!(
        report
            .repairs
            .iter()
            .all(|r| r.kind != RepairKind::Misplaced),
        "{:?}",
        report.repairs
    );
    assert!(oxidged::spec::validate_text(&out).is_empty());
    let back = oxidged::Dataset::parse(out.clone());
    let again = GedcomWriter::new()
        .gedcom_version(GedcomVersion::V7_0)
        .write_to_string(&back)
        .unwrap();
    assert_eq!(again, out);
    // A 7.0 file that holds such an address is repaired the same way.
    let v70 = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @M1@ OBJE\n1 FILE https://example.org/a b.jpg\n\
               2 FORM image/jpeg\n0 TRLR\n";
    let out = GedcomWriter::new()
        .write_to_string(&oxidged::Dataset::parse(v70))
        .unwrap();
    assert!(
        out.contains("0 @M1@ OBJE\n1 FILE https://example.org/a%20b.jpg\n"),
        "{out}"
    );
}
