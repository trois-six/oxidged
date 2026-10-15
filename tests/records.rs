//! Records and their structures, read typed and written back: individuals,
//! families, sources, repositories, multimedia objects, submitters and
//! shared notes, with their events, addresses, associations, citations,
//! notes, places, identifiers and LDS ordinances. Each is written in
//! GEDCOM 5.5.1 and 7.0 and read back with nothing lost. Every name and
//! value is fictitious.

use oxidged::model::{
    Certainty, CitationSource, EnumList, EventKind, Individual, Medium, NoteContent, OrdinanceKind,
    OrdinanceStatus, Restriction, Role, Sex, Source, Text, XrefId,
};
use oxidged::tree::Structure;
use oxidged::{Dataset, GedcomStreamParser, GedcomVersion, GedcomWriter};

const V551: GedcomVersion = GedcomVersion::V5_5_1;
const V70: GedcomVersion = GedcomVersion::V7_0;

/// A 5.5.1 file of `body`, with the header and submitter 5.5.1 requires.
fn v551(body: &str) -> String {
    format!(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n1 SOUR SAMPLE\n1 SUBM @U0@\n{body}0 @U0@ SUBM\n1 NAME Sample Submitter\n0 TRLR\n"
    )
}

/// A 7.0 file of `body`.
fn v70(body: &str) -> String {
    format!("0 HEAD\n1 GEDC\n2 VERS 7.0\n{body}0 TRLR\n")
}

fn s(data: &Dataset, text: &Text) -> String {
    text.to_str(data).into_owned()
}

fn opt(data: &Dataset, text: Option<&Text>) -> Option<String> {
    text.map(|t| s(data, t))
}

fn xref(data: &Dataset, id: Option<XrefId>) -> Option<&str> {
    id.map(|id| data.store().xref(id))
}

fn write(data: &Dataset, version: GedcomVersion) -> String {
    GedcomWriter::new()
        .gedcom_version(version)
        .write_to_string(data)
        .expect("write")
}

/// The records of `structures`, the header left out (the writer completes
/// it).
fn records(structures: Vec<Structure>) -> Vec<Structure> {
    structures
        .into_iter()
        .filter(|s| s.tag.as_str() != "HEAD")
        .collect()
}

/// Writes `data` in `version` and reads it back: every record comes back as
/// the model holds it for that version. Returns the text written.
fn round_trip(data: &Dataset, version: GedcomVersion) -> String {
    let out = write(data, version);
    let back = records(Dataset::parse(out.as_str()).to_structures());
    let expected = records(data.to_structures_for(version));
    assert_eq!(back.len(), expected.len(), "written in {version:?}:\n{out}");
    for (back, expected) in back.iter().zip(&expected) {
        assert_eq!(back, expected, "written in {version:?}:\n{out}");
    }
    out
}

/// Writes `data` in `version`, where the writer repairs what the target
/// does not have (into extension structures, in place): read back and
/// written again, it says the same. Returns the text written.
fn write_stable(data: &Dataset, version: GedcomVersion) -> String {
    let out = write(data, version);
    let again = write(&Dataset::parse(out.as_str()), version);
    assert_eq!(
        sorted(Dataset::parse(again.as_str()).to_structures()),
        sorted(Dataset::parse(out.as_str()).to_structures()),
        "written again in {version:?}:\n{again}\nfirst written:\n{out}"
    );
    out
}

/// `structures` with the substructures of each sorted by tag, the order of
/// those of one tag kept: a structure read back holds the extension
/// structures the writer made after its fields.
fn sorted(structures: Vec<Structure>) -> Vec<Structure> {
    structures
        .into_iter()
        .map(|mut s| {
            s.substructures = sorted(s.substructures);
            s.substructures
                .sort_by(|a, b| a.tag.as_str().cmp(b.tag.as_str()));
            s
        })
        .collect()
}

#[track_caller]
fn assert_has(out: &str, lines: &str) {
    assert!(out.contains(lines), "missing\n{lines}\nin\n{out}");
}

// =============================================================================
// The dataset as a whole
// =============================================================================

#[test]
fn header_only_dataset_round_trips() {
    let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 5.5\n0 TRLR");
    for version in [V551, V70] {
        let writer = GedcomWriter::new().gedcom_version(version);
        assert_eq!(writer.config().version, Some(version));
        let out = writer.write_to_string(&data).unwrap();
        // Every line, the trailer included, ends with its terminator.
        assert!(out.ends_with("0 TRLR\n"), "{out}");
        assert!(Dataset::parse(out).header().is_some());
    }
}

#[test]
fn every_record_survives_a_round_trip() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Al /Example/\n1 SEX M\n1 BIRT\n2 DATE 1 JAN 1900\n0 @I2@ INDI\n1 NAME Ann /Example/\n1 SEX F\n0 @I3@ INDI\n1 NAME Cal /Example/\n0 @F1@ FAM\n1 HUSB @I1@\n1 WIFE @I2@\n1 CHIL @I3@\n1 MARR\n2 DATE 1 JUN 1925\n0 @F2@ FAM\n1 HUSB @I2@\n0 @S1@ SOUR\n1 TITL Birth register\n0 @R1@ REPO\n1 NAME Sample Archive\n0 @N1@ NOTE A shared note\n0 @M1@ OBJE\n1 FILE photo.jpg\n2 FORM jpg\n",
    ));
    assert_eq!(data.individuals().len(), 3);
    assert_eq!(data.families().len(), 2);
    assert_eq!(data.sources().len(), 1);
    assert_eq!(data.repositories().len(), 1);
    assert_eq!(data.notes().len(), 1);
    assert_eq!(data.multimedia().len(), 1);
    assert_eq!(data.submitters().len(), 1);

    round_trip(&data, V551);
    // 7.0 gives the file the media type of its format.
    let out = write_stable(&data, V70);
    assert_has(&out, "1 FILE photo.jpg\n2 FORM image/jpeg\n");
    assert!(!out.contains("_FORM"), "{out}");
    let back = Dataset::parse(out);
    assert_eq!(back.record_count(), data.record_count());
    assert_eq!(
        records(back.to_structures())[..9],
        records(data.to_structures_for(V70))[..9]
    );
}

#[test]
fn records_without_xref_get_distinct_ones() {
    let mut data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n0 @A@ INDI\n1 NAME Bea /Example/\n0 @B@ INDI\n1 NAME Cal /Example/\n0 @C@ SOUR\n1 TITL Register\n",
    ));
    // As records built in code have them: no xref.
    data.get_mut_at::<Individual>(1).unwrap().xref = None;
    data.get_mut_at::<Individual>(2).unwrap().xref = None;
    data.get_mut_at::<Source>(0).unwrap().xref = None;

    let out = write_stable(&data, V551);
    let back = Dataset::parse(out.as_str());
    let xrefs: Vec<_> = back
        .individuals()
        .iter()
        .map(|i| xref(&back, i.xref()).unwrap().to_string())
        .collect();
    // The existing xref is kept, the others are new and distinct.
    assert_eq!(xrefs, ["@I1@", "@I2@", "@I3@"], "{out}");
    assert_eq!(
        xref(&back, back.sources().get(0).unwrap().into_owned().xref),
        Some("@S1@")
    );
    // The caller's data is left as it was.
    assert!(
        data.individuals()
            .get(1)
            .unwrap()
            .into_owned()
            .xref
            .is_none()
    );
}

// =============================================================================
// Individuals and families
// =============================================================================

#[test]
fn individual_record_round_trips() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 SEX F\n1 BIRT\n2 DATE 15 MAR 1950\n2 PLAC Sampleton, Sample County\n1 DEAT\n2 DATE 20 JUN 2020\n2 PLAC Otherville\n1 OCCU Weaver\n0 @I2@ INDI\n1 SEX M\n",
    ));
    let ann = &data.individuals().get(0).unwrap().into_owned();
    assert_eq!(s(&data, &ann.names[0].value), "Ann /Example/");
    assert_eq!(ann.sex, Some(Sex::Female));
    let birth = ann.birth().unwrap();
    assert_eq!(s(&data, &birth.date.as_ref().unwrap().value), "15 MAR 1950");
    assert_eq!(
        s(&data, &birth.place.as_ref().unwrap().name),
        "Sampleton, Sample County"
    );
    assert_eq!(
        s(&data, &ann.death().unwrap().date.as_ref().unwrap().value),
        "20 JUN 2020"
    );
    let occupation = ann.events_of(EventKind::Occupation).next().unwrap();
    assert_eq!(s(&data, &occupation.value), "Weaver");
    assert!(
        data.individuals()
            .get(1)
            .unwrap()
            .into_owned()
            .names
            .is_empty()
    );

    for version in [V551, V70] {
        let out = round_trip(&data, version);
        assert_has(
            &out,
            "0 @I1@ INDI\n1 NAME Ann /Example/\n1 SEX F\n1 BIRT\n2 DATE 15 MAR 1950\n2 PLAC Sampleton, Sample County\n1 DEAT\n",
        );
        assert_has(&out, "1 OCCU Weaver\n");
        assert_has(&out, "0 @I2@ INDI\n1 SEX M\n");
    }
}

#[test]
fn family_record_round_trips() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Al /Example/\n1 FAMS @F1@\n1 FAMS @F2@\n0 @I2@ INDI\n1 NAME Ann /Example/\n1 FAMS @F1@\n0 @I3@ INDI\n1 NAME Bea /Example/\n1 FAMC @F1@\n0 @I4@ INDI\n1 NAME Cal /Example/\n1 FAMC @F1@\n0 @F1@ FAM\n1 HUSB @I1@\n1 WIFE @I2@\n1 CHIL @I3@\n1 CHIL @I4@\n1 MARR\n2 DATE 1 JUN 2000\n2 PLAC Sampleton\n0 @F2@ FAM\n1 HUSB @I1@\n",
    ));
    let family = &data.families().get(0).unwrap().into_owned();
    assert_eq!(xref(&data, family.husband_id()), Some("@I1@"));
    assert_eq!(xref(&data, family.wife_id()), Some("@I2@"));
    let children: Vec<_> = data
        .children(data.families().first().unwrap())
        .filter_map(|c| c.full_name())
        .collect();
    assert_eq!(children, ["Bea Example", "Cal Example"]);
    assert_eq!(family.events[0].kind, EventKind::Marriage);
    assert!(
        data.families()
            .get(1)
            .unwrap()
            .into_owned()
            .children
            .is_empty()
    );
    assert_eq!(data.families_as_spouse("@I1@").count(), 2);

    for version in [V551, V70] {
        let out = round_trip(&data, version);
        assert_has(
            &out,
            "0 @F1@ FAM\n1 HUSB @I1@\n1 WIFE @I2@\n1 CHIL @I3@\n1 CHIL @I4@\n1 MARR\n2 DATE 1 JUN 2000\n2 PLAC Sampleton\n",
        );
        assert_has(&out, "0 @F2@ FAM\n1 HUSB @I1@\n");
    }
}

#[test]
fn individual_and_family_identifiers_round_trip() {
    let data = Dataset::parse(v551(
        "0 @U1@ SUBM\n1 NAME Other Submitter\n0 @I1@ INDI\n1 RESN confidential\n1 NAME Ann /Example/\n1 ALIA @I2@\n1 ANCI @U1@\n1 DESI @U1@\n1 AFN 1A2B-3C4\n1 REFN IND-1\n2 TYPE card\n1 RIN 101\n0 @I2@ INDI\n1 NAME Annie /Example/\n0 @F1@ FAM\n1 RESN locked\n1 WIFE @I1@\n1 NCHI 3\n1 REFN FAM-1\n2 TYPE box\n1 RIN 201\n",
    ));
    let ann_record = data.individuals().get(0).unwrap().into_owned();
    let ann = ann_record.detail();
    assert_eq!(
        ann.restriction,
        Some(EnumList(vec![Restriction::Confidential]))
    );
    assert_eq!(xref(&data, ann.aliases[0].individual), Some("@I2@"));
    assert_eq!(xref(&data, Some(ann.ancestor_interests[0])), Some("@U1@"));
    assert_eq!(xref(&data, Some(ann.descendant_interests[0])), Some("@U1@"));
    assert_eq!(
        opt(&data, ann.ancestral_file_number.as_ref()),
        Some("1A2B-3C4".into())
    );
    assert_eq!(s(&data, &ann.refns[0].value), "IND-1");
    assert_eq!(opt(&data, ann.refns[0].kind.as_ref()), Some("card".into()));
    assert_eq!(opt(&data, ann.record_id.as_ref()), Some("101".into()));
    let family = &data.families().get(0).unwrap().into_owned();
    assert_eq!(
        family.detail().restriction,
        Some(EnumList(vec![Restriction::Locked]))
    );
    assert_eq!(family.events[0].kind, EventKind::ChildrenCount);
    assert_eq!(s(&data, &family.events[0].value), "3");
    assert_eq!(
        opt(&data, family.detail().refns[0].kind.as_ref()),
        Some("box".into())
    );

    let out = round_trip(&data, V551);
    for expected in [
        "1 RESN confidential\n",
        "1 ALIA @I2@\n",
        "1 ANCI @U1@\n",
        "1 DESI @U1@\n",
        "1 AFN 1A2B-3C4\n",
        "1 REFN IND-1\n2 TYPE card\n",
        "1 RIN 101\n",
        "1 RESN locked\n",
        "1 NCHI 3\n",
        "1 REFN FAM-1\n2 TYPE box\n",
        "1 RIN 201\n",
    ] {
        assert_has(&out, expected);
    }

    let out = write_stable(&data, V70);
    for expected in [
        "1 RESN CONFIDENTIAL\n1 ALIA @I2@\n",
        "1 REFN IND-1\n2 TYPE card\n",
        "1 RESN LOCKED\n",
        "1 NCHI 3\n",
    ] {
        assert_has(&out, expected);
    }
    // 7.0 has no AFN nor RIN: they are external identifiers of the types
    // the migration guide names, a RIN of the system the header names.
    assert_has(&out, "1 ANCI @U1@\n1 DESI @U1@\n");
    assert_has(
        &out,
        "1 EXID 1A2B-3C4\n2 TYPE https://gedcom.io/terms/v7/AFN\n",
    );
    assert_has(
        &out,
        "1 EXID 101\n2 TYPE https://gedcom.io/terms/v7/RIN#SAMPLE\n",
    );
    assert_has(
        &out,
        "1 EXID 201\n2 TYPE https://gedcom.io/terms/v7/RIN#SAMPLE\n",
    );
}

#[test]
fn record_submitter_pointers_round_trip() {
    let data = Dataset::parse(v551(
        "0 @U1@ SUBM\n1 NAME Other Submitter\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 SUBM @U0@\n1 SUBM @U1@\n0 @F1@ FAM\n1 WIFE @I1@\n1 SUBM @U1@\n",
    ));
    let submitters = |ids: &[XrefId]| -> Vec<String> {
        ids.iter()
            .map(|&id| data.store().xref(id).to_string())
            .collect()
    };
    assert_eq!(
        submitters(
            &data
                .individuals()
                .get(0)
                .unwrap()
                .into_owned()
                .detail()
                .submitters
        ),
        ["@U0@", "@U1@"]
    );
    assert_eq!(
        submitters(
            &data
                .families()
                .get(0)
                .unwrap()
                .into_owned()
                .detail()
                .submitters
        ),
        ["@U1@"]
    );

    for version in [V551, V70] {
        let out = round_trip(&data, version);
        assert_has(&out, "1 SUBM @U0@\n1 SUBM @U1@\n");
        assert_has(&out, "1 WIFE @I1@\n1 SUBM @U1@\n");
    }
}

#[test]
fn record_uid_and_exid_per_version() {
    let data = Dataset::parse(v70(
        "0 @I1@ INDI\n1 UID 0d7a3c9e-0000-4000-8000-000000000003\n1 EXID 42\n2 TYPE https://example.org/people\n0 @F1@ FAM\n1 UID 0d7a3c9e-0000-4000-8000-000000000004\n0 @S1@ SOUR\n1 TITL A source\n1 UID 0d7a3c9e-0000-4000-8000-000000000001\n1 EXID 123\n2 TYPE https://example.org/sources\n0 @U1@ SUBM\n1 NAME Sample Submitter\n1 UID 0d7a3c9e-0000-4000-8000-000000000002\n",
    ));
    let ann_record = data.individuals().get(0).unwrap().into_owned();
    let ann = ann_record.detail();
    assert_eq!(
        s(&data, &ann.uids[0]),
        "0d7a3c9e-0000-4000-8000-000000000003"
    );
    assert_eq!(s(&data, &ann.exids[0].value), "42");
    assert_eq!(
        opt(&data, ann.exids[0].kind.as_ref()),
        Some("https://example.org/people".into())
    );
    assert_eq!(
        data.families()
            .get(0)
            .unwrap()
            .into_owned()
            .detail()
            .uids
            .len(),
        1
    );
    assert_eq!(
        s(
            &data,
            &data.sources().get(0).unwrap().into_owned().exids[0].value
        ),
        "123"
    );
    assert_eq!(data.submitters().get(0).unwrap().into_owned().uids.len(), 1);

    let out = round_trip(&data, V70);
    for expected in [
        "0 @I1@ INDI\n1 UID 0d7a3c9e-0000-4000-8000-000000000003\n1 EXID 42\n2 TYPE https://example.org/people\n",
        "0 @F1@ FAM\n1 UID 0d7a3c9e-0000-4000-8000-000000000004\n",
        "1 UID 0d7a3c9e-0000-4000-8000-000000000001\n1 EXID 123\n",
        "1 UID 0d7a3c9e-0000-4000-8000-000000000002\n",
    ] {
        assert_has(&out, expected);
    }

    // UID and EXID do not exist in GEDCOM 5.5.1: they are kept as
    // extensions.
    let out = write_stable(&data, V551);
    for expected in [
        "0 @I1@ INDI\n1 _UID 0d7a3c9e-0000-4000-8000-000000000003\n1 _EXID 42\n2 TYPE https://example.org/people\n",
        "0 @F1@ FAM\n1 _UID 0d7a3c9e-0000-4000-8000-000000000004\n",
        "1 _UID 0d7a3c9e-0000-4000-8000-000000000001\n1 _EXID 123\n",
        "1 _UID 0d7a3c9e-0000-4000-8000-000000000002\n",
    ] {
        assert_has(&out, expected);
    }
    assert!(!out.contains(" UID ") && !out.contains(" EXID "), "{out}");
}

#[test]
fn association_role_per_version() {
    let data = Dataset::parse(v70(
        "0 @I1@ INDI\n1 ASSO @I2@\n2 ROLE GODP\n1 ASSO @I3@\n2 ROLE OTHER\n3 PHRASE Best man\n2 SOUR @S1@\n3 PAGE Folio 3\n1 ASSO @VOID@\n2 PHRASE Unnamed neighbour\n2 ROLE NGHBR\n0 @I2@ INDI\n0 @I3@ INDI\n0 @S1@ SOUR\n1 TITL Register\n",
    ));
    let assoc_record = data.individuals().get(0).unwrap().into_owned();
    let assoc = &assoc_record.detail().associations;
    assert_eq!(assoc[0].role.as_ref().unwrap().value, Role::Godparent);
    let best_man = assoc[1].role.as_ref().unwrap();
    assert_eq!(best_man.value, Role::Other);
    assert_eq!(
        opt(&data, best_man.phrase.as_ref()),
        Some("Best man".into())
    );
    assert_eq!(assoc[1].citations.len(), 1);
    assert_eq!(
        opt(&data, assoc[2].phrase.as_ref()),
        Some("Unnamed neighbour".into())
    );

    let out = round_trip(&data, V70);
    for expected in [
        "1 ASSO @I2@\n2 ROLE GODP\n",
        "1 ASSO @I3@\n2 ROLE OTHER\n3 PHRASE Best man\n2 SOUR @S1@\n3 PAGE Folio 3\n",
        "1 ASSO @VOID@\n2 PHRASE Unnamed neighbour\n2 ROLE NGHBR\n",
    ] {
        assert_has(&out, expected);
    }

    // 5.5.1 has no ROLE: the role is the relation, in words.
    let out = write_stable(&data, V551);
    assert_has(&out, "1 ASSO @I2@\n2 RELA godparent\n");
    assert_has(
        &out,
        "1 ASSO @I3@\n2 RELA Best man\n2 SOUR @S1@\n3 PAGE Folio 3\n",
    );
    // Nor an association with no individual.
    assert!(
        out.contains("_ASSO") && out.contains("2 PHRASE Unnamed neighbour\n"),
        "{out}"
    );
    assert!(!out.contains("ROLE"), "{out}");
}

#[test]
fn association_relation_per_version() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 ASSO @I2@\n2 RELA Godfather\n0 @I2@ INDI\n",
    ));
    let assoc_record = data.individuals().get(0).unwrap().into_owned();
    let assoc = &assoc_record.detail().associations[0];
    assert_eq!(xref(&data, assoc.individual), Some("@I2@"));
    assert_eq!(
        opt(&data, assoc.relation.as_ref()),
        Some("Godfather".into())
    );

    let out = round_trip(&data, V551);
    assert_has(&out, "1 ASSO @I2@\n2 RELA Godfather\n");

    // 7.0 has no RELA: the relation is the role it names, in its words.
    let out = write_stable(&data, V70);
    assert_has(&out, "1 ASSO @I2@\n2 ROLE GODP\n3 PHRASE Godfather\n");
    assert!(!out.contains("RELA"), "{out}");
}

#[test]
fn event_address_contacts_and_associations_round_trip() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 BIRT\n2 DATE 1 JAN 1900\n2 ADDR 1 Example Road\n3 CITY Sampleton\n2 PHON +00 000 001\n2 EMAIL clinic@@example.org\n1 OCCU Weaver\n2 ADDR 2 Example Road\n2 PHON +00 000 002\n2 WWW https://mill.example.org\n2 ASSO @I2@\n3 RELA Employer\n0 @I2@ INDI\n1 NAME Bea /Example/\n",
    ));
    let birth_record = data.individuals().get(0).unwrap().into_owned();
    let birth = birth_record.events[0].detail();
    let address = birth.address.as_ref().unwrap();
    assert_eq!(s(&data, &address.value), "1 Example Road");
    assert_eq!(opt(&data, address.city.as_ref()), Some("Sampleton".into()));
    assert_eq!(s(&data, &birth.phones[0]), "+00 000 001");
    assert_eq!(s(&data, &birth.emails[0]), "clinic@example.org");
    let occupation_record = data.individuals().get(0).unwrap().into_owned();
    let occupation = occupation_record.events[1].detail();
    assert_eq!(
        s(&data, &occupation.websites[0]),
        "https://mill.example.org"
    );
    assert_eq!(occupation.associations.len(), 1);

    let out = write_stable(&data, V551);
    for expected in [
        // GEDCOM 5.5.1 doubles every `@` of text.
        "2 ADDR 1 Example Road\n3 CITY Sampleton\n2 PHON +00 000 001\n2 EMAIL clinic@@example.org\n",
        "2 ADDR 2 Example Road\n2 PHON +00 000 002\n2 WWW https://mill.example.org\n",
        // 5.5.1 events have no associations: it is kept as an extension.
        "2 _ASSO @I2@\n3 RELA Employer\n",
    ] {
        assert_has(&out, expected);
    }

    let out = write_stable(&data, V70);
    assert_has(
        &out,
        "2 ADDR 1 Example Road\n3 CITY Sampleton\n2 PHON +00 000 001\n2 EMAIL clinic@example.org\n",
    );
    assert_has(&out, "2 WWW https://mill.example.org\n");
    assert_has(&out, "2 ASSO @I2@\n");
    assert!(out.contains("Employer"), "{out}");
}

// =============================================================================
// Notes
// =============================================================================

#[test]
fn every_note_of_a_structure_is_kept() {
    // GEDCOM allows any number of NOTE structures in each of these places;
    // none may collapse to the last one.
    let data = Dataset::parse(v551(
        "0 @U1@ SUBM\n1 NAME Other Submitter\n1 NOTE Submitter note one\n1 NOTE Submitter note two\n0 @I1@ INDI\n1 NAME Ann /Example/\n2 NOTE Name note one\n2 NOTE Name note two\n1 BIRT\n2 DATE 1 JAN 1900\n2 NOTE Event note one\n2 NOTE Event note two\n1 OCCU Weaver\n2 NOTE Attribute note one\n2 NOTE Attribute note two\n1 FAMC @F1@\n2 NOTE Link note one\n2 NOTE Link note two\n1 ASSO @I2@\n2 RELA Godmother\n2 NOTE Association note one\n2 NOTE Association note two\n1 NOTE Person note one\n1 NOTE Person note two\n0 @I2@ INDI\n1 NAME Bea /Example/\n0 @F1@ FAM\n1 CHIL @I1@\n0 @M1@ OBJE\n1 FILE photo.jpg\n2 FORM jpg\n1 NOTE Media note one\n1 NOTE Media note two\n",
    ));
    let ann = &data.individuals().get(0).unwrap().into_owned();
    assert_eq!(ann.notes.len(), 2);
    assert_eq!(ann.names[0].detail().notes.len(), 2);
    assert_eq!(ann.events[0].detail().notes.len(), 2);
    assert_eq!(ann.events[1].detail().notes.len(), 2);
    assert_eq!(ann.child_of[0].detail().notes.len(), 2);
    assert_eq!(ann.detail().associations[0].notes.len(), 2);
    assert_eq!(
        data.multimedia().get(0).unwrap().into_owned().notes.len(),
        2
    );
    let submitter = data.find_submitter("@U1@").unwrap().into_owned();
    assert_eq!(submitter.notes.len(), 2);

    for out in [round_trip(&data, V551), write_stable(&data, V70)] {
        for owner in [
            "Submitter",
            "Name",
            "Event",
            "Attribute",
            "Link",
            "Association",
            "Person",
            "Media",
        ] {
            for nth in ["one", "two"] {
                assert_has(&out, &format!("NOTE {owner} note {nth}\n"));
            }
        }
    }
}

#[test]
fn shared_note_pointers_per_version() {
    // GEDCOM 7.0 points at a shared note with SNOTE; GEDCOM 5.5.1 with
    // NOTE @N1@. Both are pointers, written in the form of the target.
    let v7_source = v70(
        "0 @F1@ FAM\n1 SNOTE @N1@\n0 @S1@ SOUR\n1 TITL Register\n1 SNOTE @N1@\n0 @N1@ SNOTE Shared text\n",
    );
    let v551_source = v551(
        "0 @F1@ FAM\n1 NOTE @N1@\n0 @S1@ SOUR\n1 TITL Register\n1 NOTE @N1@\n0 @N1@ NOTE Shared text\n",
    );

    for source in [v7_source, v551_source] {
        let data = Dataset::parse(source);
        for note in [
            &data.families().get(0).unwrap().into_owned().notes[0],
            &data.sources().get(0).unwrap().into_owned().notes[0],
        ] {
            let NoteContent::Shared(id) = note.content else {
                panic!("not a pointer: {note:?}");
            };
            assert_eq!(data.store().xref(id), "@N1@");
            assert_eq!(
                s(&data, &data.find_note(id).unwrap().into_owned().text),
                "Shared text"
            );
        }

        let out = write_stable(&data, V70);
        assert_eq!(out.matches("1 SNOTE @N1@\n").count(), 2, "{out}");
        assert_has(&out, "0 @N1@ SNOTE Shared text\n");
        assert!(!out.contains(" NOTE @N1@"), "{out}");

        let out = write_stable(&data, V551);
        assert_eq!(out.matches("1 NOTE @N1@\n").count(), 2, "{out}");
        assert_has(&out, "0 @N1@ NOTE Shared text\n");
        assert!(!out.contains("SNOTE"), "{out}");
    }
}

#[test]
fn inline_note_with_at_signs_is_not_a_pointer() {
    // Escaped as 5.5.1 requires, or not.
    for note in ["@@Sampleton@@", "@Sampleton@"] {
        let data = Dataset::parse(v551(&format!(
            "0 @F1@ FAM\n1 NOTE Married at {note} farm\n"
        )));
        let NoteContent::Text(text) =
            &data.families().get(0).unwrap().into_owned().notes[0].content
        else {
            panic!("read as a pointer");
        };
        assert_eq!(s(&data, text), "Married at @Sampleton@ farm");

        let out = round_trip(&data, V551);
        assert_has(&out, "1 NOTE Married at @@Sampleton@@ farm\n");
        let out = round_trip(&data, V70);
        assert_has(&out, "1 NOTE Married at @Sampleton@ farm\n");
    }
}

#[test]
fn stream_parser_reads_a_5_5_1_note_record() {
    let text = v551("0 @F1@ FAM\n1 NOTE @N1@\n0 @N1@ NOTE Shared text\n");
    let data: Dataset = GedcomStreamParser::new(text.as_bytes())
        .unwrap()
        .collect::<Result<Dataset, _>>()
        .unwrap();
    let NoteContent::Shared(id) = data.families().get(0).unwrap().into_owned().notes[0].content
    else {
        panic!("not a pointer");
    };
    assert_eq!(
        s(&data, &data.find_note(id).unwrap().into_owned().text),
        "Shared text"
    );
}

#[test]
fn multi_line_shared_note_record_round_trips() {
    let data = Dataset::parse(v551(
        "0 @N1@ NOTE First line\n1 CONT Second line\n0 @I1@ INDI\n1 NOTE @N1@\n",
    ));
    assert_eq!(
        s(&data, &data.notes().get(0).unwrap().into_owned().text),
        "First line\nSecond line"
    );

    for version in [V551, V70] {
        let out = round_trip(&data, version);
        // Every line of the file is a GEDCOM line.
        for line in out.lines() {
            assert!(
                line.starts_with(|c: char| c.is_ascii_digit()),
                "not a GEDCOM line: {line:?} in\n{out}"
            );
        }
        assert_has(&out, " First line\n1 CONT Second line\n");
        let back = Dataset::parse(out);
        assert_eq!(
            s(&back, &back.notes().get(0).unwrap().into_owned().text),
            "First line\nSecond line"
        );
        assert_eq!(
            xref(&back, back.notes().get(0).unwrap().into_owned().xref),
            Some("@N1@")
        );
    }
}

// =============================================================================
// Sources, citations and repositories
// =============================================================================

#[test]
fn source_record_round_trips() {
    let data = Dataset::parse(v551(
        "0 @S1@ SOUR\n1 DATA\n2 EVEN BIRT, DEAT\n3 DATE FROM 1850 TO 1900\n3 PLAC Sampleton\n2 EVEN MARR\n2 AGNC Sample Registry Office\n2 NOTE About the recorded data\n1 AUTH Sample Clerk\n1 TITL Parish register of Sampleton\n1 ABBR Sampleton PR\n1 PUBL Sample Press, 1901\n1 TEXT First line of the transcript\n2 CONT second line\n1 REPO @R1@\n1 REFN SRC-1\n2 TYPE shelf\n1 RIN 42\n1 NOTE About the source\n1 OBJE @M1@\n1 CHAN\n2 DATE 1 JAN 2020\n0 @R1@ REPO\n1 NAME Sample Archive\n0 @M1@ OBJE\n1 FILE register.jpg\n2 FORM jpg\n",
    ));
    let source = &data.sources().get(0).unwrap().into_owned();
    assert_eq!(
        opt(&data, source.title.as_ref()),
        Some("Parish register of Sampleton".into())
    );
    assert_eq!(
        opt(&data, source.author.as_ref()),
        Some("Sample Clerk".into())
    );
    assert_eq!(
        opt(&data, source.abbreviation.as_ref()),
        Some("Sampleton PR".into())
    );
    assert_eq!(
        s(&data, &source.text.as_ref().unwrap().text),
        "First line of the transcript\nsecond line"
    );
    let recorded = source.data.as_ref().unwrap();
    // DATA.NOTE belongs to the data, not to the record.
    assert_eq!(recorded.notes.len(), 1);
    assert_eq!(source.notes.len(), 1);
    // An EVEN without substructures does not swallow the AGNC after it.
    assert_eq!(recorded.events.len(), 2);
    assert_eq!(
        recorded.events[0].kinds,
        EnumList(vec![EventKind::Birth, EventKind::Death])
    );
    assert_eq!(
        opt(&data, recorded.agency.as_ref()),
        Some("Sample Registry Office".into())
    );
    assert_eq!(
        opt(&data, source.refns[0].kind.as_ref()),
        Some("shelf".into())
    );
    assert_eq!(xref(&data, source.repositories[0].repository), Some("@R1@"));

    let out = round_trip(&data, V551);
    for expected in [
        "1 DATA\n2 EVEN BIRT, DEAT\n3 DATE FROM 1850 TO 1900\n3 PLAC Sampleton\n2 EVEN MARR\n",
        "2 AGNC Sample Registry Office\n2 NOTE About the recorded data\n",
        "1 AUTH Sample Clerk\n",
        "1 ABBR Sampleton PR\n",
        "1 PUBL Sample Press, 1901\n",
        "1 TEXT First line of the transcript\n2 CONT second line\n",
        "1 REFN SRC-1\n2 TYPE shelf\n",
        "1 RIN 42\n",
        "1 OBJE @M1@\n",
        "1 CHAN\n2 DATE 1 JAN 2020\n",
    ] {
        assert_has(&out, expected);
    }

    let out = write_stable(&data, V70);
    assert_has(
        &out,
        "1 DATA\n2 EVEN BIRT, DEAT\n3 DATE FROM 1850 TO 1900\n3 PLAC Sampleton\n2 EVEN MARR\n",
    );
    // 7.0 has no RIN: an external identifier.
    assert_has(
        &out,
        "1 EXID 42\n2 TYPE https://gedcom.io/terms/v7/RIN#SAMPLE\n",
    );
    assert_has(
        &out,
        "1 TEXT First line of the transcript\n2 CONT second line\n",
    );
}

#[test]
fn source_citation_substructures_round_trip() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 BIRT\n2 DATE 1 JAN 1900\n2 SOUR @S1@\n3 PAGE Folio 12\n3 EVEN BIRT\n4 ROLE CHIL\n3 DATA\n4 DATE 2 JAN 1900\n4 TEXT First excerpt\n4 TEXT Second excerpt\n3 OBJE @M1@\n3 NOTE Citation note one\n3 NOTE Citation note two\n3 QUAY 3\n1 SOUR Parish register of Sampleton,\n2 CONT baptisms 1890-1910\n2 TEXT Quoted line\n2 NOTE Free-text citation note\n0 @S1@ SOUR\n1 TITL Parish register\n0 @M1@ OBJE\n1 FILE register.jpg\n2 FORM jpg\n",
    ));
    let ann = &data.individuals().get(0).unwrap().into_owned();
    let cited = &ann.events[0].citations[0];
    assert!(matches!(cited.source, CitationSource::Pointer(id) if data.store().xref(id) == "@S1@"));
    assert_eq!(opt(&data, cited.page.as_ref()), Some("Folio 12".into()));
    let detail = cited.detail();
    let event = detail.event.as_ref().unwrap();
    assert_eq!(event.kind, EventKind::Birth);
    assert_eq!(event.role.as_ref().unwrap().value, Role::Child);
    assert_eq!(detail.notes.len(), 2);
    assert_eq!(detail.data.as_ref().unwrap().texts.len(), 2);
    assert_eq!(detail.quality, Some(Certainty::Primary));
    assert_eq!(detail.multimedia.len(), 1);
    let free = &ann.citations[0];
    let CitationSource::Description(description) = &free.source else {
        panic!("not a description");
    };
    assert_eq!(
        s(&data, description),
        "Parish register of Sampleton,\nbaptisms 1890-1910"
    );
    assert_eq!(free.detail().texts.len(), 1);
    assert_eq!(free.detail().notes.len(), 1);

    let out = round_trip(&data, V551);
    for expected in [
        "2 SOUR @S1@\n3 PAGE Folio 12\n",
        "3 EVEN BIRT\n4 ROLE CHIL\n",
        "4 TEXT First excerpt\n4 TEXT Second excerpt\n",
        "3 OBJE @M1@\n",
        "3 NOTE Citation note one\n",
        "3 NOTE Citation note two\n",
        "3 QUAY 3\n",
        "1 SOUR Parish register of Sampleton,\n2 CONT baptisms 1890-1910\n",
        "2 TEXT Quoted line\n",
        "2 NOTE Free-text citation note\n",
    ] {
        assert_has(&out, expected);
    }

    let out = write_stable(&data, V70);
    assert_has(&out, "3 EVEN BIRT\n4 ROLE CHIL\n");
    assert_has(&out, "3 QUAY 3\n");
    // The described source is not lost.
    assert!(out.contains("Parish register of Sampleton,"), "{out}");
}

#[test]
fn citation_with_free_text_description_round_trips() {
    // A SOUR citation with a free-text description (a URL) instead of a
    // pointer to a SOUR record must not be dropped nor read as a pointer.
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 BIRT\n2 DATE 1 JAN 1900\n2 SOUR https://example.com/records/123\n",
    ));
    let citation_record = data.individuals().get(0).unwrap().into_owned();
    let citation = &citation_record.events[0].citations[0];
    assert!(matches!(&citation.source,
        CitationSource::Description(t) if t.eq_str(&data, "https://example.com/records/123")));

    let out = round_trip(&data, V551);
    assert_has(&out, "2 SOUR https://example.com/records/123\n");
    let back = Dataset::parse(out);
    assert!(
        matches!(&back.individuals().get(0).unwrap().into_owned().events[0].citations[0].source,
        CitationSource::Description(t) if t.eq_str(&back, "https://example.com/records/123"))
    );
}

#[test]
fn repository_citation_call_numbers_round_trip() {
    let data = Dataset::parse(v551(
        "0 @S1@ SOUR\n1 TITL Parish register\n1 REPO @R1@\n2 NOTE Shelved in the reading room\n2 CALN 111\n3 MEDI Book\n2 CALN 222\n3 MEDI Film\n2 CALN 333\n1 REPO @R2@\n2 MEDI manuscript\n0 @R1@ REPO\n1 NAME Sample Archive\n0 @R2@ REPO\n1 NAME Sample Library\n",
    ));
    let call_numbers = |data: &Dataset| -> Vec<(String, Option<Medium>)> {
        data.sources().get(0).unwrap().into_owned().repositories[0]
            .call_numbers
            .iter()
            .map(|c| {
                (
                    s(data, &c.value),
                    c.medium.as_ref().map(|m| m.value.clone()),
                )
            })
            .collect()
    };
    // Every CALN is kept, each with its own MEDI (5.5.1 values ignore case).
    let expected = vec![
        ("111".to_string(), Some(Medium::Book)),
        ("222".to_string(), Some(Medium::Film)),
        ("333".to_string(), None),
    ];
    assert_eq!(call_numbers(&data), expected);
    assert_eq!(
        data.sources().get(0).unwrap().into_owned().repositories[0]
            .notes
            .len(),
        1
    );
    let second_record = data.sources().get(0).unwrap().into_owned();
    let second = &second_record.repositories[1];
    assert_eq!(xref(&data, second.repository), Some("@R2@"));
    assert!(second.call_numbers.is_empty());
    assert_eq!(data.store().tag(second.extra[0].tag), "MEDI");

    let out = write_stable(&data, V551);
    assert_has(
        &out,
        "1 REPO @R1@\n2 CALN 111\n3 MEDI book\n2 CALN 222\n3 MEDI film\n2 CALN 333\n2 NOTE Shelved in the reading room\n",
    );
    // A MEDI without a call number is not one of 5.5.1's structures.
    assert_has(&out, "1 REPO @R2@\n2 _MEDI manuscript\n");
    assert_eq!(call_numbers(&Dataset::parse(out)), expected);
}

#[test]
fn call_number_medium_per_version() {
    let data = Dataset::parse(v70(
        "0 @S1@ SOUR\n1 TITL Parish register\n1 REPO @R1@\n2 CALN 111\n3 MEDI OTHER\n4 PHRASE Parish register\n2 CALN 222\n3 MEDI Microfilm reel\n2 CALN 333\n3 MEDI BOOK\n0 @R1@ REPO\n1 NAME Sample Archive\n",
    ));
    let call_numbers_record = data.sources().get(0).unwrap().into_owned();
    let call_numbers = &call_numbers_record.repositories[0].call_numbers;
    let medium = call_numbers[0].medium.as_ref().unwrap();
    assert_eq!(medium.value, Medium::Other);
    assert_eq!(
        opt(&data, medium.phrase.as_ref()),
        Some("Parish register".into())
    );
    assert_eq!(call_numbers[2].medium.as_ref().unwrap().value, Medium::Book);

    let out = write_stable(&data, V70);
    for expected in [
        "2 CALN 111\n3 MEDI OTHER\n4 PHRASE Parish register\n",
        "2 CALN 222\n3 MEDI OTHER\n4 PHRASE Microfilm reel\n",
        "2 CALN 333\n3 MEDI BOOK\n",
    ] {
        assert_has(&out, expected);
    }

    // 5.5.1 has no medium but its own: the others, in words, are kept as
    // extensions.
    let out = write_stable(&data, V551);
    for expected in [
        "2 CALN 111\n3 _MEDI Parish register\n",
        "2 CALN 222\n3 _MEDI Microfilm reel\n",
        "2 CALN 333\n3 MEDI book\n",
    ] {
        assert_has(&out, expected);
    }
}

#[test]
fn repository_record_round_trips() {
    let data = Dataset::parse(v551(
        "0 @R1@ REPO\n1 NAME Sample Archive\n1 ADDR 1 Example Road\n2 CITY Sampleton\n2 STAE Sample State\n2 CTRY Exampleland\n1 PHON +00 000 000\n1 PHON +00 000 001\n1 EMAIL archive@@example.org\n1 FAX +00 000 002\n1 WWW https://archive.example.org\n1 NOTE Closed on Mondays\n1 REFN ARCH-1\n2 TYPE catalogue\n1 RIN 12\n1 CHAN\n2 DATE 1 JAN 2020\n",
    ));
    let repo = &data.repositories().get(0).unwrap().into_owned();
    assert_eq!(
        opt(&data, repo.name.as_ref()),
        Some("Sample Archive".into())
    );
    let address = repo.address.as_ref().unwrap();
    assert_eq!(
        opt(&data, address.state.as_ref()),
        Some("Sample State".into())
    );
    assert_eq!(repo.phones.len(), 2);
    assert_eq!(s(&data, &repo.emails[0]), "archive@example.org");
    assert_eq!(
        opt(&data, repo.refns[0].kind.as_ref()),
        Some("catalogue".into())
    );
    assert_eq!(
        s(
            &data,
            &repo.change.as_ref().unwrap().date.as_ref().unwrap().value
        ),
        "1 JAN 2020"
    );

    let out = round_trip(&data, V551);
    for expected in [
        "0 @R1@ REPO\n1 NAME Sample Archive\n1 ADDR 1 Example Road\n2 CITY Sampleton\n2 STAE Sample State\n2 CTRY Exampleland\n",
        "1 PHON +00 000 000\n1 PHON +00 000 001\n",
        // GEDCOM 5.5.1 doubles every `@` of text.
        "1 EMAIL archive@@example.org\n",
        "1 FAX +00 000 002\n",
        "1 WWW https://archive.example.org\n",
        "1 NOTE Closed on Mondays\n",
        "1 REFN ARCH-1\n2 TYPE catalogue\n",
        "1 RIN 12\n",
        "1 CHAN\n2 DATE 1 JAN 2020\n",
    ] {
        assert_has(&out, expected);
    }

    let out = write_stable(&data, V70);
    assert_has(&out, "1 EMAIL archive@example.org\n");
    assert_has(&out, "1 REFN ARCH-1\n2 TYPE catalogue\n");
}

// =============================================================================
// Submitters and multimedia
// =============================================================================

#[test]
fn submitter_record_round_trips() {
    let data = Dataset::parse(v551(
        "0 @U1@ SUBM\n1 NAME Other Submitter\n1 ADDR 1 Example Road\n2 CITY Sampleton\n1 PHON +00 000 000\n1 EMAIL submitter@@example.org\n1 FAX +00 000 001\n1 WWW https://example.org\n1 OBJE @M1@\n1 OBJE\n2 FILE portrait.jpg\n3 FORM jpg\n2 TITL Portrait\n1 LANG English\n1 LANG French\n1 RFN 1234\n1 RIN 56\n1 NOTE Submitted for the sample project\n1 CHAN\n2 DATE 1 JAN 2020\n0 @M1@ OBJE\n1 FILE logo.jpg\n2 FORM jpg\n",
    ));
    let submitter = data.find_submitter("@U1@").unwrap().into_owned();
    assert_eq!(
        opt(&data, submitter.name.as_ref()),
        Some("Other Submitter".into())
    );
    assert_eq!(xref(&data, submitter.multimedia[0].object), Some("@M1@"));
    assert_eq!(
        s(&data, &submitter.multimedia[1].files[0].path),
        "portrait.jpg"
    );
    let languages: Vec<_> = submitter.languages.iter().map(|l| s(&data, l)).collect();
    assert_eq!(languages, ["English", "French"]);
    assert_eq!(
        opt(&data, submitter.record_file_number.as_ref()),
        Some("1234".into())
    );

    let out = round_trip(&data, V551);
    for expected in [
        "1 PHON +00 000 000\n",
        // GEDCOM 5.5.1 doubles every `@` of text.
        "1 EMAIL submitter@@example.org\n",
        "1 FAX +00 000 001\n",
        "1 WWW https://example.org\n",
        "1 OBJE @M1@\n",
        "1 OBJE\n2 FILE portrait.jpg\n3 FORM jpg\n2 TITL Portrait\n",
        "1 LANG English\n1 LANG French\n",
        "1 RFN 1234\n",
        "1 RIN 56\n",
    ] {
        assert_has(&out, expected);
    }

    let out = write_stable(&data, V70);
    assert_has(&out, "1 EMAIL submitter@example.org\n");
    assert_has(&out, "1 OBJE @M1@\n");
    assert!(out.contains("portrait.jpg"), "{out}");
}

#[test]
fn multimedia_record_round_trips() {
    let data = Dataset::parse(v551(
        "0 @S1@ SOUR\n1 TITL Cemetery survey\n0 @M1@ OBJE\n1 FILE headstone.jpg\n2 FORM jpg\n3 TYPE tombstone\n2 TITL Headstone, front\n1 REFN MEDIA-0042\n2 TYPE Archive number\n1 RIN 12345\n1 NOTE Photographed on site.\n1 SOUR @S1@\n2 PAGE Plot 12\n1 CHAN\n2 DATE 1 JAN 2020\n",
    ));
    let media = &data.multimedia().get(0).unwrap().into_owned();
    let file = &media.files[0];
    assert_eq!(s(&data, &file.path), "headstone.jpg");
    assert_eq!(
        opt(&data, file.title.as_ref()),
        Some("Headstone, front".into())
    );
    let form = file.form.as_ref().unwrap();
    assert_eq!(s(&data, &form.format), "jpg");
    assert_eq!(form.medium_type.as_deref(), Some(&Medium::Tombstone));
    assert_eq!(
        opt(&data, media.refns[0].kind.as_ref()),
        Some("Archive number".into())
    );
    assert_eq!(opt(&data, media.record_id.as_ref()), Some("12345".into()));
    assert_eq!(media.notes.len(), 1);
    assert_eq!(
        opt(&data, media.citations[0].page.as_ref()),
        Some("Plot 12".into())
    );

    let out = round_trip(&data, V551);
    for expected in [
        "1 FILE headstone.jpg\n2 FORM jpg\n3 TYPE tombstone\n2 TITL Headstone, front\n",
        "1 REFN MEDIA-0042\n2 TYPE Archive number\n",
        "1 RIN 12345\n",
        "1 NOTE Photographed on site.\n",
        "1 SOUR @S1@\n2 PAGE Plot 12\n",
        "1 CHAN\n2 DATE 1 JAN 2020\n",
    ] {
        assert_has(&out, expected);
    }

    let out = write_stable(&data, V70);
    assert_has(&out, "1 FILE headstone.jpg\n");
    assert_has(&out, "1 SOUR @S1@\n2 PAGE Plot 12\n");
    assert!(out.contains("Headstone, front"), "{out}");
}

#[test]
fn multimedia_form_written_beside_its_file_is_kept() {
    // Some exporters write FORM as a sibling of FILE rather than under it.
    let data = Dataset::parse(v551(
        "0 @M1@ OBJE\n1 FILE headstone.jpg\n1 FORM jpeg\n2 TYPE tombstone\n",
    ));
    let media = &data.multimedia().get(0).unwrap().into_owned();
    assert!(media.files[0].form.is_none());
    assert_eq!(data.store().tag(media.extra[0].tag), "FORM");

    // Kept in place as an extension; the file gets the FORM 5.5.1 requires.
    let out = write_stable(&data, V551);
    assert_has(
        &out,
        "1 FILE headstone.jpg\n2 FORM jpg\n1 _FORM jpeg\n2 TYPE tombstone\n",
    );
    let out = write_stable(&data, V70);
    assert!(out.contains("FORM jpeg\n2 TYPE tombstone\n"), "{out}");
}

#[test]
fn multimedia_link_files_round_trip() {
    // The TITL of a FILE is distinct from the OBJE's own TITL.
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 OBJE\n2 FILE photo.jpg\n3 FORM jpg\n4 MEDI photo\n3 TITL Ann at the beach\n2 TITL Beach\n",
    ));
    let link_record = data.individuals().get(0).unwrap().into_owned();
    let link = &link_record.detail().multimedia[0];
    assert_eq!(link.object, None);
    assert_eq!(opt(&data, link.title.as_ref()), Some("Beach".into()));
    let file = &link.files[0];
    assert_eq!(s(&data, &file.path), "photo.jpg");
    assert_eq!(
        opt(&data, file.title.as_ref()),
        Some("Ann at the beach".into())
    );
    let form = file.form.as_ref().unwrap();
    assert_eq!(form.medium.as_ref().unwrap().value, Medium::Photo);

    // A link's FILE has no TITL in 5.5.1: it is kept as an extension.
    let out = write_stable(&data, V551);
    assert_has(
        &out,
        "1 OBJE\n2 FILE photo.jpg\n3 FORM jpg\n4 MEDI photo\n3 _TITL Ann at the beach\n2 TITL Beach\n",
    );
    let out = write_stable(&data, V70);
    assert!(
        out.contains("Ann at the beach") && out.contains("Beach\n"),
        "{out}"
    );
}

#[test]
fn multimedia_link_pointer_round_trips() {
    // `1 OBJE @M1@` is a link to a record, not an inline object.
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 OBJE @M1@\n0 @M1@ OBJE\n1 FILE headstone.jpg\n2 FORM jpg\n3 TYPE tombstone\n",
    ));
    let link_record = data.individuals().get(0).unwrap().into_owned();
    let link = &link_record.detail().multimedia[0];
    assert_eq!(xref(&data, link.object), Some("@M1@"));
    assert!(link.files.is_empty());

    for out in [round_trip(&data, V551), write_stable(&data, V70)] {
        assert_has(&out, "1 OBJE @M1@\n");
        let back = Dataset::parse(out);
        assert!(
            back.find_multimedia("@M1@")
                .is_some_and(|m| !m.files().is_empty())
        );
    }
}

#[test]
fn file_value_with_an_at_sign_is_not_a_pointer() {
    // Escaped as 5.5.1 requires, or not.
    for file in ["photo@@2x.jpg", "photo@2x.jpg"] {
        let data = Dataset::parse(v551(&format!(
            "0 @I1@ INDI\n1 NAME Ann /Example/\n1 OBJE\n2 FILE {file}\n3 FORM jpg\n"
        )));
        let link_record = data.individuals().get(0).unwrap().into_owned();
        let link = &link_record.detail().multimedia[0];
        assert_eq!(link.object, None);
        assert_eq!(s(&data, &link.files[0].path), "photo@2x.jpg");

        let out = round_trip(&data, V551);
        assert_has(&out, "2 FILE photo@@2x.jpg\n");
    }
}

#[test]
fn multimedia_crop_round_trips() {
    // CROP belongs to a multimedia link; one under a FILE is kept as an
    // extension, and so is a width that is not a whole number.
    let data = Dataset::parse(v70(
        "0 @I1@ INDI\n1 OBJE @M1@\n2 CROP\n3 TOP 10\n3 LEFT 20\n3 HEIGHT 50\n3 WIDTH 25\n2 TITL Group photo, detail\n0 @M1@ OBJE\n1 FILE group-photo.jpg\n2 FORM image/jpeg\n2 CROP\n3 TOP 10\n3 WIDTH 25.5\n",
    ));
    let link_record = data.individuals().get(0).unwrap().into_owned();
    let link = &link_record.detail().multimedia[0];
    let crop = link.crop.as_ref().unwrap();
    assert_eq!(
        (crop.top, crop.left, crop.height, crop.width),
        (Some(10), Some(20), Some(50), Some(25))
    );

    let out = write_stable(&data, V70);
    assert_has(
        &out,
        "1 OBJE @M1@\n2 CROP\n3 TOP 10\n3 LEFT 20\n3 HEIGHT 50\n3 WIDTH 25\n2 TITL Group photo, detail\n",
    );
    assert_has(&out, "2 _CROP\n3 TOP 10\n3 _WIDTH 25.5\n");
}

// =============================================================================
// LDS ordinances
// =============================================================================

#[test]
fn lds_ordinance_place_round_trips_in_5_5_1() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 BAPL\n2 DATE 15 MAR 1990\n2 TEMP SLAKE\n2 PLAC Sampleton\n2 STAT COMPLETED\n3 DATE 1 JAN 2000\n1 SLGC\n2 PLAC Otherville\n2 FAMC @F1@\n1 FAMC @F1@\n0 @F1@ FAM\n1 CHIL @I1@\n1 SLGS\n2 DATE 2 APR 1991\n2 PLAC Sampleton\n",
    ));
    let ordinances_record = data.individuals().get(0).unwrap().into_owned();
    let ordinances = &ordinances_record.detail().ordinances;
    let place = |i: usize| s(&data, &ordinances[i].place.as_ref().unwrap().name);
    assert_eq!(ordinances[0].kind, OrdinanceKind::Baptism);
    assert_eq!(place(0), "Sampleton");
    assert_eq!(place(1), "Otherville");
    assert_eq!(xref(&data, ordinances[1].family), Some("@F1@"));
    let sealing_record = data.families().get(0).unwrap().into_owned();
    let sealing = &sealing_record.detail().ordinances[0];
    assert_eq!(sealing.kind, OrdinanceKind::SpouseSealing);
    assert_eq!(s(&data, &sealing.place.as_ref().unwrap().name), "Sampleton");
    // The place does not end up anywhere else.
    assert!(
        data.individuals()
            .get(0)
            .unwrap()
            .into_owned()
            .extra
            .is_empty()
    );
    assert!(ordinances[0].extra.is_empty());

    for version in [V551, V70] {
        let out = round_trip(&data, version);
        assert_has(
            &out,
            "1 BAPL\n2 DATE 15 MAR 1990\n2 TEMP SLAKE\n2 PLAC Sampleton\n",
        );
        assert_has(&out, "1 SLGC\n2 PLAC Otherville\n");
        assert_has(&out, "1 SLGS\n2 DATE 2 APR 1991\n2 PLAC Sampleton\n");
    }
}

#[test]
fn lds_ordinance_place_structure_round_trips_in_7_0() {
    let data = Dataset::parse(v70(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 INIL\n2 DATE 15 MAR 1990\n2 TEMP SLAKE\n2 PLAC Sampleton, Sample County\n3 FORM City, County\n3 MAP\n4 LATI N10.5\n4 LONG W20.25\n2 STAT COMPLETED\n3 DATE 1 JAN 2000\n",
    ));
    let ordinance_record = data.individuals().get(0).unwrap().into_owned();
    let ordinance = &ordinance_record.detail().ordinances[0];
    assert_eq!(ordinance.kind, OrdinanceKind::Initiatory);
    let place = ordinance.place.as_ref().unwrap();
    assert_eq!(s(&data, &place.name), "Sampleton, Sample County");
    assert_eq!(
        opt(&data, place.detail().form.as_ref()),
        Some("City, County".into())
    );
    assert_eq!(
        opt(
            &data,
            place.detail().map.as_ref().unwrap().latitude.as_ref()
        ),
        Some("N10.5".into())
    );

    let out = round_trip(&data, V70);
    assert_has(
        &out,
        "2 PLAC Sampleton, Sample County\n3 FORM City, County\n3 MAP\n4 LATI N10.5\n4 LONG W20.25\n",
    );
}

#[test]
fn lds_status_date_round_trips_in_5_5_1() {
    // The status date does not replace the ordinance's own date.
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 BAPL\n2 DATE 15 MAR 1990\n2 TEMP SLAKE\n2 STAT COMPLETED\n3 DATE 1 JAN 2000\n1 FAMC @F1@\n0 @F1@ FAM\n1 CHIL @I1@\n1 SLGS\n2 STAT CANCELED\n3 DATE 2 APR 2001\n2 DATE 2 APR 1991\n",
    ));
    let baptism_record = data.individuals().get(0).unwrap().into_owned();
    let baptism = &baptism_record.detail().ordinances[0];
    assert_eq!(
        s(&data, &baptism.date.as_ref().unwrap().value),
        "15 MAR 1990"
    );
    let status = baptism.status.as_ref().unwrap();
    assert_eq!(status.value, OrdinanceStatus::Completed);
    assert_eq!(s(&data, &status.date.as_ref().unwrap().value), "1 JAN 2000");

    let sealing_record = data.families().get(0).unwrap().into_owned();

    let sealing = &sealing_record.detail().ordinances[0];
    assert_eq!(
        s(&data, &sealing.date.as_ref().unwrap().value),
        "2 APR 1991"
    );
    let status = sealing.status.as_ref().unwrap();
    assert_eq!(status.value, OrdinanceStatus::Canceled);
    assert_eq!(s(&data, &status.date.as_ref().unwrap().value), "2 APR 2001");

    let out = round_trip(&data, V551);
    assert_has(
        &out,
        "1 BAPL\n2 DATE 15 MAR 1990\n2 TEMP SLAKE\n2 STAT COMPLETED\n3 DATE 1 JAN 2000\n",
    );
    assert_has(&out, "2 STAT CANCELED\n3 DATE 2 APR 2001\n");
    assert_has(&out, "2 DATE 2 APR 1991\n");
}

#[test]
fn lds_status_date_and_time_round_trip_in_7_0() {
    let data = Dataset::parse(v70(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 INIL\n2 DATE 15 MAR 1990\n2 TEMP SLAKE\n2 STAT COMPLETED\n3 DATE 1 JAN 2000\n4 TIME 10:15:00\n",
    ));
    let initiatory_record = data.individuals().get(0).unwrap().into_owned();
    let initiatory = &initiatory_record.detail().ordinances[0];
    assert_eq!(
        s(&data, &initiatory.date.as_ref().unwrap().value),
        "15 MAR 1990"
    );
    let status_date = initiatory.status.as_ref().unwrap().date.as_ref().unwrap();
    assert_eq!(s(&data, &status_date.value), "1 JAN 2000");
    assert_eq!(
        opt(&data, status_date.time.as_ref()),
        Some("10:15:00".into())
    );

    let out = round_trip(&data, V70);
    assert_has(
        &out,
        "2 STAT COMPLETED\n3 DATE 1 JAN 2000\n4 TIME 10:15:00\n",
    );
}

#[test]
fn lds_status_date_goes_with_its_status() {
    let mut data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 ENDL\n2 DATE 15 MAR 1990\n2 STAT COMPLETED\n3 DATE 1 JAN 2000\n",
    ));
    data.get_mut_at::<Individual>(0)
        .unwrap()
        .detail_mut()
        .ordinances[0]
        .status = None;

    let out = round_trip(&data, V551);
    assert_has(&out, "1 ENDL\n2 DATE 15 MAR 1990\n");
    assert!(!out.contains("STAT") && !out.contains("3 DATE"), "{out}");
}

// =============================================================================
// Places
// =============================================================================

#[test]
fn place_notes_and_citations_round_trip() {
    let data = Dataset::parse(v551(
        "0 @I1@ INDI\n1 NAME Ann /Example/\n1 BIRT\n2 PLAC Sampleton\n3 MAP\n4 LATI N1.5\n4 LONG E2.5\n3 NOTE Then a hamlet of Otherville\n3 SOUR @S1@\n4 PAGE Gazetteer, p. 7\n1 RESI\n2 PLAC Otherville\n3 NOTE Moved here in 1920\n0 @S1@ SOUR\n1 TITL Sample gazetteer\n",
    ));
    let place_record = data.individuals().get(0).unwrap().into_owned();
    let place = place_record.events[0].place.as_ref().unwrap();
    assert_eq!(place.detail().notes.len(), 1);
    // A SOUR under PLAC is not one of its fields: it is kept in place, and
    // written as an extension.
    assert_eq!(data.store().tag(place.extra[0].tag), "SOUR");

    for version in [V551, V70] {
        let out = write_stable(&data, version);
        assert_has(&out, "3 NOTE Then a hamlet of Otherville\n");
        assert_has(&out, "2 PLAC Otherville\n3 NOTE Moved here in 1920\n");
        assert!(out.contains("SOUR @S1@\n4 PAGE Gazetteer, p. 7\n"), "{out}");
    }
}

#[test]
fn place_exid_per_version() {
    let data = Dataset::parse(v70(
        "0 @I1@ INDI\n1 BIRT\n2 PLAC Sampleton\n3 EXID 4242\n4 TYPE https://example.org/places\n",
    ));
    let place_record = data.individuals().get(0).unwrap().into_owned();
    let place = place_record.events[0].place.as_ref().unwrap();
    assert_eq!(s(&data, &place.detail().exids[0].value), "4242");

    let out = round_trip(&data, V70);
    assert_has(
        &out,
        "2 PLAC Sampleton\n3 EXID 4242\n4 TYPE https://example.org/places\n",
    );
    // 5.5.1 has no EXID: it is kept as an extension.
    let out = write_stable(&data, V551);
    assert_has(
        &out,
        "2 PLAC Sampleton\n3 _EXID 4242\n4 TYPE https://example.org/places\n",
    );
}
