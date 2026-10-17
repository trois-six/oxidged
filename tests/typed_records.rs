//! The typed records of the model (`oxidged::model`): header, individuals,
//! families, sources, repositories, multimedia objects, submitters,
//! submissions and shared notes, with their events, names, links and
//! ordinances; an ordinance status of either version's enumeration reads as
//! its variant, any other as `Unknown`, and both are written back with
//! their `DATE`. Every name and value is fictitious.

use oxidged::model::{
    Adoption, BirthKind, Dataset, EventKind, GedcomForm, NamePieceKind, NameType, OrdinanceKind,
    OrdinanceStatus, Pedigree, RecordRef, Restriction, Sex, Text,
};
use oxidged::{GedcomVersion, GedcomWriter};

fn text(data: &Dataset, t: &Text) -> String {
    t.to_str(data).into_owned()
}

fn opt(data: &Dataset, t: Option<&Text>) -> Option<String> {
    t.map(|t| text(data, t))
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

fn v7(body: &str) -> String {
    format!("0 HEAD\n1 GEDC\n2 VERS 7.0\n{body}0 TRLR\n")
}

fn v551(body: &str) -> String {
    format!(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n1 SOUR EXAMPLE\n1 SUBM @U1@\n{body}0 @U1@ SUBM\n1 NAME Example Submitter\n0 TRLR\n"
    )
}

fn assert_has(out: &str, lines: &str) {
    assert!(out.contains(lines), "missing\n{lines}\nin\n{out}");
}

/// The header: product and maker with every contact (D8, D9), recipient,
/// date and time, submitter and submission, file, copyright, language,
/// place form and note (D8: `HEAD.PLAC.FORM`, `HEAD.SUBN`).
#[test]
fn header_5() {
    let data = Dataset::parse(
        "0 HEAD\n1 SOUR EXAMPLE_APP\n2 VERS 2.1\n2 NAME Example App\n2 CORP Example Corp\n3 ADDR 1 Sample Road\n4 CITY Sampleton\n3 PHON 555-0100\n3 PHON 555-0101\n3 EMAIL a@@example.com\n3 FAX 555-0102\n3 WWW https://example.com\n2 DATA Example Data\n3 DATE 1 JAN 2000\n3 COPR Example copyright\n1 DEST OTHER_APP\n1 DATE 2 FEB 2001\n2 TIME 10:20:30\n1 SUBM @U1@\n1 SUBN @B1@\n1 FILE sample.ged\n1 COPR Copyright note\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n1 LANG English\n1 PLAC\n2 FORM City, County, Country\n1 NOTE A header note\n0 @U1@ SUBM\n1 NAME Example Submitter\n0 @B1@ SUBN\n1 FAMF family.ged\n0 TRLR\n",
    );
    let head = data.header().unwrap();
    let source = head.source.as_ref().unwrap();
    assert_eq!(text(&data, &source.product), "EXAMPLE_APP");
    let corp = source.corporation.as_ref().unwrap();
    assert_eq!(corp.phones.len(), 2);
    assert_eq!(text(&data, &corp.emails[0]), "a@example.com");
    assert_eq!(
        opt(&data, source.data.as_ref().unwrap().copyright.as_ref()),
        Some("Example copyright".into())
    );
    assert_eq!(
        opt(&data, head.date.as_ref().unwrap().time.as_ref()),
        Some("10:20:30".into())
    );
    assert!(head.submission.is_some());
    assert_eq!(
        opt(&data, head.place.as_ref().unwrap().form.as_ref()),
        Some("City, County, Country".into())
    );
    assert_eq!(
        head.gedcom.as_ref().unwrap().form,
        Some(GedcomForm::LineageLinked)
    );
    let out = write(&data);
    assert_has(
        &out,
        "2 CORP Example Corp\n3 ADDR 1 Sample Road\n4 CITY Sampleton\n3 PHON 555-0100\n3 PHON 555-0101\n3 EMAIL a@@example.com\n3 FAX 555-0102\n3 WWW https://example.com\n",
    );
    assert_has(&out, "1 SUBN @B1@\n");
    assert_has(&out, "1 PLAC\n2 FORM City, County, Country\n");
    assert_has(&out, "1 NOTE A header note\n");
    assert_has(&out, "0 @B1@ SUBN\n1 FAMF family.ged\n");
}

/// A form other than `LINEAGE-LINKED` and a second `FORM` or `CHAR` are
/// kept (D15f, TS8); the writer writes the form its version requires.
#[test]
fn header_form_is_lenient() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM Lineage-Linked\n2 FORM NOT LINEAGE-LINKED\n1 CHAR ANSEL\n1 CHAR UTF-8\n0 TRLR\n",
    );
    let head = data.header().unwrap();
    let gedc = head.gedcom.as_ref().unwrap();
    assert_eq!(gedc.form, Some(GedcomForm::LineageLinked));
    assert_eq!(gedc.extra.len(), 1, "the second FORM");
    assert_eq!(head.extra.len(), 1, "the second CHAR");
    let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM EVENT_ORIENTED\n0 TRLR\n");
    let form = data
        .header()
        .as_ref()
        .unwrap()
        .gedcom
        .as_ref()
        .unwrap()
        .form
        .as_ref();
    assert!(matches!(form, Some(GedcomForm::Unknown(t)) if t.to_str(&data) == "EVENT_ORIENTED"));
    assert_has(&write(&data), "2 FORM LINEAGE-LINKED\n");
}

/// The 7.x header: its schema, and 7.1's notes, title and description.
#[test]
fn header_7() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.1\n1 SCHMA\n2 TAG _LOC https://example.com/loc\n1 TITL A sample\n2 LANG en\n2 TRAN Un exemple\n3 LANG fr\n1 DESC Fictitious\n1 NOTE First\n1 NOTE Second\n0 TRLR\n",
    );
    let head = data.header().unwrap();
    assert_eq!(head.schema.as_ref().unwrap().tags.len(), 1);
    let title = head.title.as_ref().unwrap();
    assert_eq!(text(&data, &title.translations[0].text), "Un exemple");
    assert_eq!(head.notes.len(), 2);
    let out = write(&data);
    assert_has(&out, "1 SCHMA\n2 TAG _LOC https://example.com/loc\n");
    assert_has(
        &out,
        "1 TITL A sample\n2 LANG en\n2 TRAN Un exemple\n3 LANG fr\n",
    );
    assert_has(&out, "1 NOTE First\n1 NOTE Second\n");
}

/// Names keep their pieces as written, several of a kind (H6), in order,
/// and gain none: no `SURN` from the slashes (D23, L4, TS24); type with
/// phrase, translations (7.x) and variations (5.5.1, D8).
#[test]
fn names() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 NAME Ann Marie /Example/\n2 TYPE OTHER\n3 PHRASE Stage name\n2 GIVN Ann\n2 GIVN Marie\n2 NICK Annie\n2 TRAN Анна /Пример/\n3 LANG ru\n3 GIVN Анна\n1 NAME /Sample/\n",
    ));
    let names_record = data.find_individual("@I1@").unwrap().into_owned();
    let names = &names_record.names;
    assert_eq!(names.len(), 2);
    let name = &names[0];
    assert_eq!(name.givens().count(), 2);
    assert_eq!(opt(&data, name.given()), Some("Ann".into()));
    assert_eq!(name.surname(), None);
    assert_eq!(name.surname_in_value(&data).as_deref(), Some("Example"));
    assert_eq!(name.pieces[2].kind, NamePieceKind::Nickname);
    let kind = name.detail().kind.as_ref().unwrap();
    assert_eq!(kind.value, NameType::Other);
    assert_eq!(opt(&data, kind.phrase.as_ref()), Some("Stage name".into()));
    assert_eq!(name.detail().translations[0].pieces.len(), 1);
    let out = write(&data);
    assert_has(
        &out,
        "1 NAME Ann Marie /Example/\n2 GIVN Ann\n2 GIVN Marie\n2 NICK Annie\n2 TYPE OTHER\n3 PHRASE Stage name\n2 TRAN Анна /Пример/\n3 LANG ru\n3 GIVN Анна\n",
    );
    assert_has(&out, "1 NAME /Sample/\n");
    assert!(!out.contains("SURN"), "{out}");

    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Taro /Yamada/\n2 TYPE birth\n2 FONE Taro /Yamada/\n3 TYPE kana\n3 GIVN Taro\n2 ROMN Taro /Yamada/\n3 TYPE romaji\n2 NOTE A name note\n",
    ));
    let name_record = data.find_individual("@I1@").unwrap().into_owned();
    let name = &name_record.names[0];
    assert_eq!(name.detail().kind.as_ref().unwrap().value, NameType::Birth);
    assert_eq!(name.detail().phonetic[0].pieces.len(), 1);
    let out = write(&data);
    assert_has(
        &out,
        "2 TYPE birth\n2 FONE Taro /Yamada/\n3 TYPE kana\n3 GIVN Taro\n2 ROMN Taro /Yamada/\n3 TYPE romaji\n2 NOTE A name note\n",
    );
}

/// Events and attributes: their payloads (continued: D11), dates, places
/// and sources, and their detail: type, age, cause, agency, religion,
/// address and contacts, restriction, notes, media, associations; the
/// family of a birth or adoption with who adopted (and its phrase); 7.1's
/// birth kind; events known not to have happened (`NO`).
#[test]
fn events_and_attributes() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 BIRT\n2 DATE 1 JAN 1900\n2 PLAC Sampleton\n2 AGE 0y\n2 FAMC @F1@\n2 SOUR @S1@\n1 ADOP Y\n2 FAMC @F2@\n3 ADOP BOTH\n4 PHRASE Both guardians\n1 DSCR Brown hair,\n2 CONT green eyes\n2 TYPE Appearance\n1 EVEN\n2 TYPE Apprenticeship\n2 AGNC Sample Guild\n2 CAUS Training\n2 RELI None\n2 ADDR 1 Sample Road\n2 PHON 555-0100\n2 EMAIL a@example.com\n2 RESN CONFIDENTIAL, LOCKED\n2 NOTE An event note\n2 UID 0b0f2b0a-0000-4000-8000-000000000003\n2 SDATE 1910\n1 NO MARR\n2 DATE FROM 1900 TO 1950\n1 OCCU Miller\n0 @F1@ FAM\n0 @F2@ FAM\n0 @S1@ SOUR\n1 TITL Register\n",
    ));
    let indi = data.find_individual("@I1@").unwrap().into_owned();
    let events = &indi.events;
    assert_eq!(
        events.iter().map(|e| e.kind.clone()).collect::<Vec<_>>(),
        [
            EventKind::Birth,
            EventKind::Adoption,
            EventKind::PhysicalDescription,
            EventKind::Event,
            EventKind::Occupation,
        ]
    );
    let birth = &events[0];
    assert_eq!(
        text(&data, &birth.place.as_ref().unwrap().name),
        "Sampleton"
    );
    assert_eq!(birth.citations.len(), 1);
    assert_eq!(
        opt(&data, birth.detail().age.as_ref().map(|a| &a.value)),
        Some("0y".into())
    );
    assert!(birth.detail().family.as_ref().unwrap().family.is_some());
    let adoption = events[1].detail().family.as_ref().unwrap();
    let by = adoption.adopted_by.as_ref().unwrap();
    assert_eq!(by.value, Adoption::Both);
    assert_eq!(
        opt(&data, by.phrase.as_ref()),
        Some("Both guardians".into())
    );
    assert_eq!(text(&data, &events[2].value), "Brown hair,\ngreen eyes");
    let generic = events[3].detail();
    assert_eq!(
        opt(&data, generic.classification.as_ref()),
        Some("Apprenticeship".into())
    );
    assert_eq!(generic.phones.len(), 1);
    assert_eq!(
        generic.restriction.as_ref().unwrap().0,
        [Restriction::Confidential, Restriction::Locked]
    );
    assert_eq!(text(&data, &events[4].value), "Miller");
    let no = &indi.detail().non_events[0];
    assert_eq!(no.kind, EventKind::Marriage);
    let out = write(&data);
    assert_has(
        &out,
        "1 BIRT\n2 DATE 1 JAN 1900\n2 PLAC Sampleton\n2 SOUR @S1@\n2 AGE 0y\n2 FAMC @F1@\n",
    );
    assert_has(
        &out,
        "1 ADOP Y\n2 FAMC @F2@\n3 ADOP BOTH\n4 PHRASE Both guardians\n",
    );
    assert_has(
        &out,
        "1 DSCR Brown hair,\n2 CONT green eyes\n2 TYPE Appearance\n",
    );
    assert_has(&out, "2 RESN CONFIDENTIAL, LOCKED\n");
    assert_has(&out, "1 NO MARR\n2 DATE FROM 1900 TO 1950\n");
    assert_has(&out, "1 OCCU Miller\n");

    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.1\n0 @I1@ INDI\n1 BIRT\n2 KIND BORN_LIVE\n0 TRLR\n",
    );
    let birth_record = data.find_individual("@I1@").unwrap().into_owned();
    let birth = &birth_record.events[0];
    assert_eq!(birth.detail().birth_kinds, [BirthKind::BornLive]);
}

/// Families: partners with phrases (H7), a second husband kept in place
/// (D15d, R4, TS4), children with phrases and extensions, a 5.5.1 child
/// sealing under a child kept (SLGC under CHIL), family events with the
/// partners' ages, `NCHI` with its event detail (C2), `FACT`, sealings.
#[test]
fn families() {
    let data = Dataset::parse(v7(
        "0 @F1@ FAM\n1 HUSB @VOID@\n2 PHRASE Unknown father\n1 HUSB @I3@\n1 WIFE @I2@\n1 CHIL @I1@\n2 PHRASE Eldest\n2 _FREL Natural\n1 CHIL @VOID@\n2 PHRASE Stillborn child\n1 MARR\n2 DATE 1 JAN 1890\n2 HUSB\n3 AGE 25y\n2 WIFE\n3 AGE 22y\n1 NCHI 2\n2 HUSB\n3 AGE 40y\n2 NOTE Two children\n1 FACT Shared farm\n2 TYPE Property\n1 SLGS\n2 STAT COMPLETED\n3 DATE 2 FEB 2000\n0 @I1@ INDI\n0 @I2@ INDI\n0 @I3@ INDI\n",
    ));
    let fam = data.find_family("@F1@").unwrap().into_owned();
    let husband = fam.husband.as_ref().unwrap();
    assert_eq!(data.store().xref(husband.individual.unwrap()), "@VOID@");
    assert_eq!(
        opt(&data, husband.detail().phrase.as_ref()),
        Some("Unknown father".into())
    );
    assert_eq!(fam.extra.len(), 1, "the second HUSB");
    assert_eq!(fam.children.len(), 2);
    assert_eq!(fam.children[0].extra.len(), 1, "_FREL");
    let marriage = &fam.events[0];
    assert_eq!(
        opt(
            &data,
            marriage
                .detail()
                .wife
                .as_ref()
                .unwrap()
                .age
                .as_ref()
                .map(|a| &a.value)
        ),
        Some("22y".into())
    );
    let nchi = &fam.events[1];
    assert_eq!(nchi.kind, EventKind::ChildrenCount);
    assert_eq!(text(&data, &nchi.value), "2");
    assert!(nchi.detail().husband.is_some());
    assert_eq!(nchi.detail().notes.len(), 1);
    assert_eq!(fam.events[2].kind, EventKind::Fact);
    assert_eq!(
        fam.detail().ordinances[0].kind,
        OrdinanceKind::SpouseSealing
    );
    let out = write(&data);
    assert_has(&out, "1 HUSB @VOID@\n2 PHRASE Unknown father\n");
    assert_has(
        &out,
        "1 WIFE @I2@\n1 CHIL @I1@\n2 PHRASE Eldest\n2 _FREL Natural\n1 CHIL @VOID@\n2 PHRASE Stillborn child\n",
    );
    assert_has(
        &out,
        "1 MARR\n2 DATE 1 JAN 1890\n2 HUSB\n3 AGE 25y\n2 WIFE\n3 AGE 22y\n",
    );
    assert_has(&out, "1 NCHI 2\n2 HUSB\n3 AGE 40y\n2 NOTE Two children\n");
    assert_has(&out, "1 FACT Shared farm\n2 TYPE Property\n");
    // The second husband, which 7.0 does not permit, as an extension.
    assert_has(&out, "1 _HUSB @I3@\n");

    let data = Dataset::parse(v551(
        "0 @F1@ FAM\n1 HUSB @I1@\n1 CHIL @I2@\n2 SLGC\n3 DATE 1 JAN 1960\n3 TEMP SLAKE\n1 NCHI 3\n0 @I1@ INDI\n0 @I2@ INDI\n",
    ));
    let fam = data.find_family("@F1@").unwrap().into_owned();
    assert_eq!(fam.children[0].extra.len(), 1, "SLGC under CHIL");
    assert_eq!(fam.events[0].kind, EventKind::ChildrenCount);
    let out = write(&data);
    assert_has(
        &out,
        "1 CHIL @I2@\n2 _SLGC\n3 DATE 1 JAN 1960\n3 TEMP SLAKE\n",
    );
    assert_has(&out, "1 NCHI 3\n");
}

/// Links to families: every one kept, duplicates and `@VOID@` included
/// (D14, H5), with pedigree, status and their phrases, and notes.
#[test]
fn family_links() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 FAMC @F1@\n2 PEDI BIRTH\n1 FAMC @F1@\n2 PEDI ADOPTED\n2 STAT CHALLENGED\n3 PHRASE Doubtful\n1 FAMC @VOID@\n2 PEDI OTHER\n3 PHRASE Raised by a neighbour\n1 FAMS @F2@\n2 NOTE A spouse note\n1 FAMS @F2@\n0 @F1@ FAM\n0 @F2@ FAM\n",
    ));
    let indi = data.find_individual("@I1@").unwrap().into_owned();
    assert_eq!(indi.child_of.len(), 3);
    assert_eq!(indi.spouse_of.len(), 2);
    assert_eq!(
        indi.child_of[1].detail().pedigree.as_ref().unwrap().value,
        Pedigree::Adopted
    );
    assert_eq!(indi.spouse_of[0].notes.len(), 1);
    let out = write(&data);
    assert_has(
        &out,
        "1 FAMC @F1@\n2 PEDI BIRTH\n1 FAMC @F1@\n2 PEDI ADOPTED\n2 STAT CHALLENGED\n3 PHRASE Doubtful\n1 FAMC @VOID@\n2 PEDI OTHER\n3 PHRASE Raised by a neighbour\n",
    );
    assert_has(&out, "1 FAMS @F2@\n2 NOTE A spouse note\n1 FAMS @F2@\n");
    let out = write_as(&data, GedcomVersion::V5_5_1);
    assert_has(
        &out,
        "1 FAMC @F1@\n2 PEDI birth\n1 FAMC @F1@\n2 PEDI adopted\n2 STAT challenged\n",
    );
}

/// Ordinances of an individual: every kind, with date, temple, place,
/// status and its date, the family of a child's sealing, notes, sources.
#[test]
fn ordinances() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 BAPL\n2 DATE 1 JAN 1900\n2 TEMP SLAKE\n2 PLAC Sample Temple\n2 STAT COMPLETED\n3 DATE 2 FEB 1990\n2 NOTE A note\n1 CONL\n2 STAT DNS/CAN\n3 DATE 3 MAR 1991\n1 ENDL\n2 STAT PRE-1970\n3 DATE 4 APR 1992\n1 SLGC\n2 FAMC @F1@\n2 STAT BIC\n3 DATE 5 MAY 1993\n0 @F1@ FAM\n",
    ));
    let ords_record = data.find_individual("@I1@").unwrap().into_owned();
    let ords = &ords_record.detail().ordinances;
    assert_eq!(
        ords.iter().map(|o| o.kind).collect::<Vec<_>>(),
        [
            OrdinanceKind::Baptism,
            OrdinanceKind::Confirmation,
            OrdinanceKind::Endowment,
            OrdinanceKind::ChildSealing,
        ]
    );
    assert_eq!(opt(&data, ords[0].temple.as_ref()), Some("SLAKE".into()));
    assert_eq!(
        ords[1].status.as_ref().unwrap().value,
        OrdinanceStatus::DoNotSealCanceled
    );
    assert!(ords[3].family.is_some());
    let out = write(&data);
    assert_has(
        &out,
        "1 BAPL\n2 DATE 1 JAN 1900\n2 TEMP SLAKE\n2 PLAC Sample Temple\n2 STAT COMPLETED\n3 DATE 2 FEB 1990\n2 NOTE A note\n",
    );
    assert_has(&out, "1 SLGC\n2 STAT BIC\n3 DATE 5 MAY 1993\n2 FAMC @F1@\n");
    let out = write_as(&data, GedcomVersion::V7_0);
    assert_has(&out, "2 STAT DNS_CAN\n");
    assert_has(&out, "2 STAT PRE_1970\n");
}

/// Repeatable identifiers and interests of a record (D9): `REFN` with its
/// own `TYPE`, `ANCI`, `DESI`, `SUBM`, `ALIA` with its phrase; the 5.5.1
/// record numbers; the creation date (7.x); the change date's notes (D8).
#[test]
fn individual_detail() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 RESN locked\n1 REFN A-1\n2 TYPE user\n1 REFN A-2\n1 ANCI @U1@\n1 ANCI @U2@\n1 DESI @U1@\n1 SUBM @U1@\n1 ALIA @I2@\n1 RFN 42\n1 AFN 43\n1 RIN 44\n1 OBJE @O1@\n1 ASSO @I2@\n2 RELA Godfather\n1 CHAN\n2 DATE 1 JAN 2000\n2 NOTE Checked\n0 @I2@ INDI\n0 @U2@ SUBM\n1 NAME Other Submitter\n0 @O1@ OBJE\n1 FILE a.jpg\n2 FORM jpg\n",
    ));
    let indi = data.find_individual("@I1@").unwrap().into_owned();
    let detail = indi.detail();
    assert_eq!(detail.refns.len(), 2);
    assert_eq!(detail.refns[1].kind, None, "the type stays with its number");
    assert_eq!(detail.ancestor_interests.len(), 2);
    assert_eq!(detail.aliases.len(), 1);
    assert_eq!(
        opt(&data, detail.record_file_number.as_ref()),
        Some("42".into())
    );
    assert_eq!(indi.change.as_ref().unwrap().notes.len(), 1);
    let out = write(&data);
    assert_has(&out, "1 REFN A-1\n2 TYPE user\n1 REFN A-2\n");
    assert_has(&out, "1 ANCI @U1@\n1 ANCI @U2@\n1 DESI @U1@\n");
    assert_has(&out, "1 CHAN\n2 DATE 1 JAN 2000\n2 NOTE Checked\n");
    assert_has(&out, "1 RFN 42\n1 AFN 43\n1 RIN 44\n");
    assert_has(&out, "1 RESN locked\n");
}

/// A source record: what it records, with every event's period and full
/// place (H7), its repositories, text, media and notes.
#[test]
fn sources() {
    let data = Dataset::parse(v7(
        "0 @S1@ SOUR\n1 TITL Parish register\n1 AUTH Sample parish\n1 ABBR Register\n1 PUBL Unpublished\n1 DATA\n2 EVEN BIRT, DEAT\n3 DATE FROM 1800 TO 1850\n3 PLAC Sampleton\n4 FORM City\n4 MAP\n5 LATI N1.5\n5 LONG E2.5\n2 AGNC Sample parish\n2 NOTE Some pages missing\n1 TEXT Transcribed\n2 MIME text/plain\n1 REPO @R1@\n2 CALN 12\n1 OBJE @O1@\n1 CREA\n2 DATE 1 JAN 2020\n0 @R1@ REPO\n1 NAME Sample archive\n1 PHON 555-0100\n1 PHON 555-0101\n0 @O1@ OBJE\n1 FILE a.jpg\n2 FORM image/jpeg\n",
    ));
    let source = &data.sources().get(0).unwrap().into_owned();
    let recorded = &source.data.as_ref().unwrap().events[0];
    // 5.5.1 may name no event (found by the `model` fuzz target).
    let empty = Dataset::parse(v551("0 @S1@ SOUR\n1 DATA\n2 EVEN\n"));
    assert!(
        empty
            .sources()
            .get(0)
            .unwrap()
            .into_owned()
            .data
            .as_ref()
            .unwrap()
            .events[0]
            .kinds
            .0
            .is_empty()
    );
    assert!(oxidged::model::ledger::untyped(&empty).is_empty());
    assert_eq!(recorded.kinds.0, [EventKind::Birth, EventKind::Death]);
    let place = recorded.place.as_ref().unwrap();
    assert!(place.detail().map.is_some());
    assert_eq!(
        data.repositories()
            .get(0)
            .unwrap()
            .into_owned()
            .phones
            .len(),
        2
    );
    let out = write(&data);
    assert_has(
        &out,
        "1 DATA\n2 EVEN BIRT, DEAT\n3 DATE FROM 1800 TO 1850\n3 PLAC Sampleton\n4 FORM City\n4 MAP\n5 LATI N1.5\n5 LONG E2.5\n2 AGNC Sample parish\n2 NOTE Some pages missing\n",
    );
    assert_has(&out, "1 TEXT Transcribed\n2 MIME text/plain\n");
    assert_has(&out, "1 REPO @R1@\n2 CALN 12\n");
    assert_has(&out, "1 CREA\n2 DATE 1 JAN 2020\n");
}

/// Multimedia records keep every file (D9, M6) and every source (M6); a
/// 5.5.1 record's `FORM.TYPE` is read and written as `TYPE`, a link's
/// `FORM.MEDI` as `MEDI` (D12).
#[test]
fn multimedia_records() {
    let data = Dataset::parse(v551(
        "0 @O1@ OBJE\n1 FILE a.jpg\n2 FORM jpg\n3 TYPE photo\n2 TITL First\n1 FILE b.tif\n2 FORM tif\n1 SOUR @S1@\n2 PAGE 1\n1 SOUR @S1@\n2 PAGE 2\n1 REFN M-1\n0 @I1@ INDI\n1 OBJE\n2 FILE c.jpg\n3 FORM jpg\n4 MEDI photo\n0 @S1@ SOUR\n1 TITL Album\n",
    ));
    let obje = &data.multimedia().get(0).unwrap().into_owned();
    assert_eq!(obje.files.len(), 2);
    assert_eq!(obje.citations.len(), 2);
    assert!(obje.files[0].form.as_ref().unwrap().medium_type.is_some());
    let out = write(&data);
    assert_has(
        &out,
        "0 @O1@ OBJE\n1 FILE a.jpg\n2 FORM jpg\n3 TYPE photo\n2 TITL First\n1 FILE b.tif\n2 FORM tif\n",
    );
    assert_has(&out, "1 SOUR @S1@\n2 PAGE 1\n1 SOUR @S1@\n2 PAGE 2\n");
    assert_has(&out, "1 OBJE\n2 FILE c.jpg\n3 FORM jpg\n4 MEDI photo\n");
}

/// Submitters keep every language (D9) and contact; a submission every
/// substructure (D8).
#[test]
fn submitters_and_submissions() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n1 SOUR EXAMPLE\n1 SUBM @U1@\n1 SUBN @B1@\n0 @U1@ SUBM\n1 NAME Example Submitter\n1 LANG English\n1 LANG French\n1 LANG German\n1 EMAIL a@@example.com\n1 RFN 7\n0 @B1@ SUBN\n1 SUBM @U1@\n1 FAMF family.ged\n1 TEMP SLAKE\n1 ANCE 2\n1 DESC 3\n1 ORDI yes\n1 NOTE A submission note\n1 RIN 9\n1 CHAN\n2 DATE 1 JAN 2000\n0 TRLR\n",
    );
    assert_eq!(
        data.submitters()
            .get(0)
            .unwrap()
            .into_owned()
            .languages
            .len(),
        3
    );
    let subn = &data.submissions().get(0).unwrap().into_owned();
    assert!(subn.ordinance_process.is_some());
    let out = write(&data);
    assert_has(&out, "1 LANG English\n1 LANG French\n1 LANG German\n");
    assert_has(
        &out,
        "0 @B1@ SUBN\n1 SUBM @U1@\n1 FAMF family.ged\n1 TEMP SLAKE\n1 ANCE 2\n1 DESC 3\n1 ORDI yes\n1 NOTE A submission note\n1 RIN 9\n1 CHAN\n2 DATE 1 JAN 2000\n",
    );
}

/// Shared notes: a 5.5.1 `NOTE` record and a 7.x `SNOTE` record, with
/// their change and creation dates, translations and sources (D8, H7),
/// each written with its version's tag.
#[test]
fn shared_notes() {
    let data = Dataset::parse(v7(
        "0 @N1@ SNOTE A shared note\n2 CONT continued\n1 MIME text/plain\n1 LANG en\n1 TRAN Une note\n2 LANG fr\n1 SOUR @S1@\n1 CHAN\n2 DATE 1 JAN 2000\n1 CREA\n2 DATE 1 JAN 1999\n0 @S1@ SOUR\n1 TITL Register\n",
    ));
    let note = &data.notes().get(0).unwrap().into_owned();
    assert!(note.text.eq_str(&data, "A shared note\ncontinued"));
    assert!(note.change.is_some() && note.creation.is_some());
    let out = write(&data);
    assert_has(
        &out,
        "0 @N1@ SNOTE A shared note\n1 CONT continued\n1 MIME text/plain\n1 LANG en\n1 TRAN Une note\n2 LANG fr\n1 SOUR @S1@\n1 CHAN\n2 DATE 1 JAN 2000\n1 CREA\n2 DATE 1 JAN 1999\n",
    );
    let out = write_as(&data, GedcomVersion::V5_5_1);
    assert_has(&out, "0 @N1@ NOTE A shared note\n1 CONT continued\n");

    let data = Dataset::parse(v551(
        "0 @N1@ NOTE Shared\n1 REFN 7\n1 CHAN\n2 DATE 1 JAN 2000\n",
    ));
    assert_eq!(data.notes().get(0).unwrap().into_owned().refns.len(), 1);
    assert_has(
        &write(&data),
        "0 @N1@ NOTE Shared\n1 REFN 7\n1 CHAN\n2 DATE 1 JAN 2000\n",
    );
}

/// Records of no type the model has (extension and unknown records, a
/// trailer with content, a second header) are kept in order (D15e); a
/// payload a record cannot hold is kept aside.
#[test]
fn other_records() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @L1@ _LOC Sampleton\n1 _POP 100\n0 @X1@ FOO bar\n0 INDI with a payload\n0 TRLR\n1 _AFTER trailer content\n0 @I1@ INDI\n",
    );
    // The record after TRLR is read; so is the one with a payload, kept
    // aside.
    assert_eq!(data.individuals().len(), 2);
    assert_eq!(data.extra().len(), 3);
    let tags: Vec<&str> = data.extra().iter().map(|n| n.tag()).collect();
    assert_eq!(tags, ["_LOC", "FOO", "TRLR"]);
    assert!(matches!(data.find("@L1@"), Some(RecordRef::Other(_))));
    let out = write(&data);
    assert_has(&out, "0 @L1@ _LOC Sampleton\n1 _POP 100\n");
    assert_has(&out, "1 _AFTER trailer content\n");
    assert_eq!(oxidged::model::ledger::untyped(&data), ["INDI"]);
}

/// Sex: any case in 5.5.1 (D15a), 7.x's `X`, an unknown value kept; no
/// `FACT` is added under it (L5).
#[test]
fn sex() {
    let data = Dataset::parse(v551("0 @I1@ INDI\n1 SEX f\n0 @I2@ INDI\n1 SEX N\n"));
    assert_eq!(
        data.individuals().get(0).unwrap().into_owned().sex,
        Some(Sex::Female)
    );
    assert!(
        matches!(&data.individuals().get(1).unwrap().into_owned().sex, Some(Sex::Unknown(t)) if t.to_str(&data) == "N")
    );
    let out = write(&data);
    assert_has(&out, "1 SEX F\n");
    assert!(!out.contains("FACT"), "{out}");
    let data = Dataset::parse(v7("0 @I1@ INDI\n1 SEX X\n"));
    assert_eq!(
        data.individuals().get(0).unwrap().into_owned().sex,
        Some(Sex::Nonbinary)
    );
}

/// Records are built by programs with struct literals and constructors
/// that set nothing twice (§4.10), and written conformant.
#[test]
fn building_records() {
    use oxidged::model::{Event, Individual, Name};
    let mut data = Dataset::parse(v7(""));
    let id = data.store_mut().intern_xref("@I9@").unwrap();
    let mut birth = Event::new(EventKind::Birth);
    birth.detail_mut().classification = Some("Home birth".into());
    data.add(Individual {
        xref: Some(id),
        names: vec![Name::new("Ann /Example/")],
        sex: Some(Sex::Female),
        events: vec![birth],
        ..Individual::default()
    })
    .unwrap();
    let out = write(&data);
    assert_has(
        &out,
        "0 @I9@ INDI\n1 NAME Ann /Example/\n1 SEX F\n1 BIRT\n2 TYPE Home birth\n",
    );
}

/// Substructures of one tag keep their order, the first being the
/// preferred one, even when one of them cannot be typed (a value with an
/// extension under it, a second singleton): it and the later ones of its
/// tag are kept after the typed ones, in order.
#[test]
fn same_tag_order_is_kept() {
    let data = Dataset::parse(v551(
        "0 @U2@ SUBM\n1 NAME Second Submitter\n1 LANG English\n2 _SCRIPT Latin\n1 LANG French\n1 LANG German\n0 @I1@ INDI\n1 SOUR @S1@\n2 PAGE 1\n2 PAGE 2\n2 PAGE 3\n0 @S1@ SOUR\n1 TITL Register\n",
    ));
    let subm = &data.submitters().get(0).unwrap().into_owned();
    assert!(subm.languages.is_empty());
    assert_eq!(subm.extra.len(), 3);
    // A leaf with an extension stays whole; a repeated singleton is
    // untyped, the first of its tag only.
    assert_eq!(oxidged::model::ledger::untyped(&data), ["INDI/SOUR/PAGE"]);
    let out = write(&data);
    assert_has(
        &out,
        "1 LANG English\n2 _SCRIPT Latin\n1 LANG French\n1 LANG German\n",
    );
    assert_has(&out, "2 PAGE 1\n2 _PAGE 2\n2 _PAGE 3\n");
    let again = Dataset::parse(&out);
    assert_eq!(write(&again), out);
}

/// A value kept untyped (a second age of an event) is written in the
/// target's grammar like the typed one, so that what reads back typed
/// writes alike (found by the `model` fuzz target).
#[test]
fn untyped_values_are_written_like_typed_ones() {
    let data = Dataset::parse(
        "0 @I1@ INDI\n1 EVEN\n2 AGE t\n2 AGE > 0m\n2 DATE 1 JAN 1900\n2 DATE @#DJULIAN@ 2 FEB 1900\n",
    );
    for version in [
        GedcomVersion::V5_5_1,
        GedcomVersion::V7_0,
        GedcomVersion::V7_1,
    ] {
        let out = write_as(&data, version);
        let again = write_as(&Dataset::parse(&out), version);
        let lines = |s: &str| {
            let mut l: Vec<String> = s.lines().map(String::from).collect();
            l.sort();
            l
        };
        assert_eq!(lines(&again), lines(&out), "{version}\n{out}\n{again}");
    }
    let out = write_as(&data, GedcomVersion::V7_0);
    assert_has(&out, "JULIAN 2 FEB 1900");
}

/// A conformant dataset is written without a repair: the trailer, which the
/// dataset does not keep, is written as the writer writes one, and the
/// owned structures end with it.
#[test]
fn a_conformant_dataset_writes_without_repairs() {
    let data = Dataset::parse(
        "0 HEAD\n1 SOUR Sample\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n1 SUBM @U1@\n0 @U1@ SUBM\n1 NAME Tester\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n",
    );
    let report = GedcomWriter::new().write(std::io::sink(), &data).unwrap();
    assert!(report.repairs.is_empty(), "{:?}", report.repairs);
    let records = data.to_structures();
    assert_eq!(records.last().map(|s| s.tag.as_str()), Some("TRLR"));
    assert_eq!(records.iter().filter(|s| s.tag == "TRLR").count(), 1);
}

/// An LDS ordinance status is never dropped.
#[test]
fn lds_status_outside_the_enumeration_round_trips_in_5_5_1() {
    let source = "\
0 HEAD
1 GEDC
2 VERS 5.5.1
2 FORM LINEAGE-LINKED
1 CHAR UTF-8
0 @I1@ INDI
1 NAME Ann /Example/
1 BAPL
2 DATE 15 MAR 1990
2 STAT EXCLUDED
3 DATE 1 JAN 2000
1 ENDL
2 STAT COMPLETED
0 TRLR";
    let data = Dataset::parse(source);

    let ordinances_record = data.find_individual("@I1@").unwrap().into_owned();

    let ordinances = &ordinances_record.detail().ordinances;
    assert_eq!(ordinances[0].kind, OrdinanceKind::Baptism);
    assert_eq!(ordinances[1].kind, OrdinanceKind::Endowment);
    let excluded = ordinances[0].status.as_ref().unwrap();
    let completed = ordinances[1].status.as_ref().unwrap();
    assert_eq!(excluded.value, OrdinanceStatus::Excluded);
    assert_eq!(
        excluded.date.as_ref().unwrap().value.to_str(&data),
        "1 JAN 2000"
    );
    assert_eq!(completed.value, OrdinanceStatus::Completed);

    let written = GedcomWriter::new().write_to_string(&data).unwrap();
    assert!(
        written.contains("1 BAPL\n2 DATE 15 MAR 1990\n2 STAT EXCLUDED\n3 DATE 1 JAN 2000\n"),
        "{written}"
    );
    // A 5.5.1 status requires its DATE: without one, the conformant writer
    // keeps it as an extension structure, value included.
    assert!(written.contains("1 ENDL\n2 _STAT COMPLETED\n"), "{written}");
    let read = Dataset::parse(&written);
    let back_record = read.find_individual("@I1@").unwrap().into_owned();
    let back = back_record.detail().ordinances[0].status.as_ref().unwrap();
    assert_eq!(back.value, excluded.value);
}

#[test]
fn lds_extension_status_round_trips_in_7_0() {
    let source = "\
0 HEAD
1 GEDC
2 VERS 7.0
0 @F1@ FAM
1 SLGS
2 STAT _PENDING
3 DATE 2 APR 2001
0 TRLR";
    let data = Dataset::parse(source);

    let slgs_record = data.find_family("@F1@").unwrap().into_owned();

    let slgs = &slgs_record.detail().ordinances[0];
    assert_eq!(slgs.kind, OrdinanceKind::SpouseSealing);
    let sealing = slgs.status.as_ref().unwrap();
    assert!(matches!(&sealing.value, OrdinanceStatus::Unknown(t) if t.to_str(&data) == "_PENDING"));

    let written = GedcomWriter::new().write_to_string(&data).unwrap();
    assert!(
        written.contains("1 SLGS\n2 STAT _PENDING\n3 DATE 2 APR 2001\n"),
        "{written}"
    );
    let read = Dataset::parse(&written);
    let back_record = read.find_family("@F1@").unwrap().into_owned();
    let back = back_record.detail().ordinances[0].status.as_ref().unwrap();
    assert!(matches!(&back.value, OrdinanceStatus::Unknown(t) if t.to_str(&read) == "_PENDING"));
}

/// An event list item that the target's banned characters hide is written
/// as reading it back gives it: `CHR`, followed by a control character and
/// a no-break space, is the known `CHR` once the control character is left
/// out, not `CHR` and the no-break space, which the next read would turn
/// into `CHR` (fuzz `model`).
#[test]
fn list_items_are_written_as_they_read_back() {
    let data = Dataset::parse(v551("0 @S1@ SOUR\n1 DATA\n2 EVEN BIRT, CHR\u{8}\u{a0}\n"));
    let out = write_as(&data, GedcomVersion::V5_5_1);
    assert_has(&out, "2 EVEN BIRT, CHR\n");
    assert_eq!(
        write_as(&Dataset::parse(out.as_str()), GedcomVersion::V5_5_1),
        out
    );
}

/// A 5.5.1 multimedia record whose files are of formats outside the 5.5.1
/// list stays a record, its pointers pointers (accepted practice: the list
/// is open); a 7.x media type is written in 5.5.1 as its file format.
#[test]
fn file_formats_outside_the_551_list_are_kept() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 OBJE @M1@\n0 @M1@ OBJE\n1 FILE scan.pdf\n2 FORM pdf\n1 FILE photo.png\n",
    ));
    let out = write_as(&data, GedcomVersion::V5_5_1);
    assert_has(&out, "1 OBJE @M1@\n");
    assert_has(
        &out,
        "0 @M1@ OBJE\n1 FILE scan.pdf\n2 FORM pdf\n1 FILE photo.png\n2 FORM png\n",
    );
    assert!(
        !out.contains("_OBJE") && !out.contains("_FILE") && !out.contains("_FORM"),
        "{out}"
    );
    assert_eq!(
        write_as(&Dataset::parse(out.as_str()), GedcomVersion::V5_5_1),
        out
    );

    let data = Dataset::parse(v7("0 @M1@ OBJE\n1 FILE photo.png\n2 FORM image/png\n"));
    let out = write_as(&data, GedcomVersion::V5_5_1);
    assert_has(&out, "1 FILE photo.png\n2 FORM png\n");
}

/// The writer repairs a standard structure the version does not permit, or
/// that lacks a required part nothing can stand for, into an extension of
/// its tag (`_INIL` for an `INIL` in 5.5.1, `_SLGC` for an `SLGC` without
/// its `FAMC`). Read back, such an extension is the structure it stands for,
/// in the field the standard tag fills: the dataset written as 5.5.1 reads
/// as the one that was written, writes back to 5.5.1 unchanged, and writes
/// to 7.x with the standard tags.
#[test]
fn repaired_ordinances_read_back_as_ordinances() {
    let data = Dataset::parse(v7(
        "0 @I1@ INDI\n1 INIL\n2 DATE 1 JAN 1900\n1 SLGC\n2 DATE 2 FEB 1901\n",
    ));
    let written = write_as(&data, GedcomVersion::V5_5_1);
    assert_has(&written, "1 _INIL\n2 DATE 1 JAN 1900\n");
    assert_has(&written, "1 _SLGC\n2 DATE 2 FEB 1901\n");

    let back = Dataset::parse(&written);
    let record = back.find_individual("@I1@").unwrap().into_owned();
    let ords = &record.detail().ordinances;
    assert_eq!(
        ords.iter().map(|o| o.kind).collect::<Vec<_>>(),
        [OrdinanceKind::Initiatory, OrdinanceKind::ChildSealing]
    );
    assert!(ords[1].family.is_none());
    assert_eq!(write_as(&back, GedcomVersion::V5_5_1), written);
    let v7_out = write_as(&back, GedcomVersion::V7_0);
    assert_has(&v7_out, "1 INIL\n2 DATE 1 JAN 1900\n");
    assert_has(&v7_out, "1 SLGC\n2 DATE 2 FEB 1901\n2 FAMC @VOID@\n");

    // Read directly from 5.5.1 text, the same structures are typed too.
    let direct = Dataset::parse(v551("0 @I1@ INDI\n1 _INIL\n2 DATE 1 JAN 1900\n"));
    let record = direct.find_individual("@I1@").unwrap().into_owned();
    assert_eq!(
        record.detail().ordinances[0].kind,
        OrdinanceKind::Initiatory
    );
}

/// The reading is no wider than the writer's repair: an extension of a tag
/// the version permits where it stands is an extension, and stays one.
#[test]
fn extension_of_a_permitted_ordinance_stays_an_extension() {
    let data = Dataset::parse(v7("0 @I1@ INDI\n1 _INIL\n2 DATE 1 JAN 1900\n"));
    let record = data.find_individual("@I1@").unwrap().into_owned();
    assert!(record.detail().ordinances.is_empty());
    assert_has(&write(&data), "1 _INIL\n2 DATE 1 JAN 1900\n");
}

/// A structure only 7.x has, written to 5.5.1 as its extension, reads back as
/// the structure it stands for: a creation date (`CREA`) of an individual.
#[test]
fn repaired_creation_date_reads_back_as_creation_date() {
    let data = Dataset::parse(v7("0 @I1@ INDI\n1 CREA\n2 DATE 1 JAN 1900\n"));
    let written = write_as(&data, GedcomVersion::V5_5_1);
    assert_has(&written, "1 _CREA\n2 DATE 1 JAN 1900\n");

    let back = Dataset::parse(&written);
    let record = back.find_individual("@I1@").unwrap().into_owned();
    assert!(record.detail().creation.is_some());
    assert_eq!(write_as(&back, GedcomVersion::V5_5_1), written);
    assert_has(
        &write_as(&back, GedcomVersion::V7_0),
        "1 CREA\n2 DATE 1 JAN 1900\n",
    );
}
