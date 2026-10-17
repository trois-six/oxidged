//! Ancestors, descendants and subsets of a dataset: the generations of
//! each relative, the families and records a subset keeps, the pointers it
//! cuts, and that a subset writes conformant GEDCOM. Every name is
//! fictitious.

use std::collections::BTreeSet;

use oxidged::model::{Dataset, Individual, Removal};
use oxidged::writer::RepairPolicy;
use oxidged::{GedcomVersion, GedcomWriter, IndexedDataset};

/// Three generations and what they cite:
///
/// - Gus (@I7@) and Hal (@I8@) are the parents of Bob (@I2@);
/// - Bob and Ann (@I1@) of Cal (@I3@) and Dan (@I4@);
/// - Cal and Eve (@I5@) of Fay (@I6@);
/// - Dan and Ivy (@I9@) have no child; Ivy is a friend (`ASSO`) of Fay,
///   with a citation of a source only that association cites.
const FILE: &str = "0 HEAD\n1 GEDC\n2 VERS 7.0\n1 SUBM @U1@\n\
    0 @U1@ SUBM\n1 NAME Example submitter\n\
    0 @I1@ INDI\n1 NAME Ann /Example/\n1 FAMS @F1@\n1 SOUR @S1@\n2 PAGE p. 1\n\
    0 @I2@ INDI\n1 NAME Bob /Example/\n1 FAMC @F0@\n1 FAMS @F1@\n1 OBJE @M1@\n\
    0 @I3@ INDI\n1 NAME Cal /Example/\n1 FAMC @F1@\n1 FAMS @F2@\n1 SNOTE @N1@\n\
    0 @I4@ INDI\n1 NAME Dan /Example/\n1 FAMC @F1@\n1 FAMS @F3@\n\
    0 @I5@ INDI\n1 NAME Eve /Sample/\n1 FAMS @F2@\n1 _PLACE @X1@\n\
    0 @I6@ INDI\n1 NAME Fay /Example/\n1 FAMC @F2@\n1 ASSO @I9@\n2 ROLE FRIEND\n2 SOUR @S3@\n\
    0 @I7@ INDI\n1 NAME Gus /Example/\n1 FAMS @F0@\n\
    0 @I8@ INDI\n1 NAME Hal /Example/\n1 FAMS @F0@\n\
    0 @I9@ INDI\n1 NAME Ivy /Sample/\n1 FAMS @F3@\n\
    0 @F0@ FAM\n1 HUSB @I7@\n1 WIFE @I8@\n1 CHIL @I2@\n\
    0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I1@\n1 CHIL @I3@\n1 CHIL @I4@\n1 MARR\n2 DATE 1950\n\
    0 @F2@ FAM\n1 HUSB @I3@\n1 WIFE @I5@\n1 CHIL @I6@\n\
    0 @F3@ FAM\n1 HUSB @I4@\n1 WIFE @I9@\n\
    0 @N1@ SNOTE A shared note\n1 SOUR @S2@\n\
    0 @S1@ SOUR\n1 TITL Parish register\n1 REPO @R1@\n\
    0 @S2@ SOUR\n1 TITL Family letters\n\
    0 @S3@ SOUR\n1 TITL School roll\n\
    0 @S4@ SOUR\n1 TITL Uncited\n\
    0 @R1@ REPO\n1 NAME Example archive\n\
    0 @M1@ OBJE\n1 FILE photo.jpg\n2 FORM image/jpeg\n\
    0 @X1@ _LOC\n1 NAME Sampleton\n\
    0 _DEFINITIONS\n1 _EVENT Example\n\
    0 TRLR\n";

fn data() -> Dataset {
    Dataset::parse(FILE)
}

fn xrefs(data: &Dataset, relatives: &[oxidged::model::Relative<'_>]) -> Vec<(String, u32)> {
    relatives
        .iter()
        .map(|r| {
            let xref = r.individual.xref().unwrap();
            (data.store().xref(xref).to_string(), r.generation)
        })
        .collect()
}

/// The identifiers of the records of a dataset, in order.
fn records(data: &Dataset) -> Vec<String> {
    data.records()
        .filter_map(|r| r.xref())
        .map(|id| data.store().xref(id).to_string())
        .collect()
}

fn write(data: &Dataset, version: GedcomVersion) -> String {
    GedcomWriter::new()
        .gedcom_version(version)
        .on_nonconformant(RepairPolicy::Error)
        .write_to_string(data)
        .unwrap()
}

#[test]
fn ancestors_and_descendants_by_generation() {
    let data = data();
    assert_eq!(
        xrefs(&data, &data.ancestors("@I6@", None)),
        [
            ("@I3@".into(), 1),
            ("@I5@".into(), 1),
            ("@I2@".into(), 2),
            ("@I1@".into(), 2),
            ("@I7@".into(), 3),
            ("@I8@".into(), 3),
        ]
    );
    assert_eq!(
        xrefs(&data, &data.descendants("@I7@", Some(2))),
        [("@I2@".into(), 1), ("@I3@".into(), 2), ("@I4@".into(), 2)]
    );
    let indexed = IndexedDataset::new(data.clone());
    assert_eq!(
        xrefs(&data, &indexed.descendants("@I7@", None)),
        xrefs(&data, &data.descendants("@I7@", None))
    );
}

#[test]
fn a_subset_keeps_the_families_and_records_its_individuals_need() {
    let data = data();
    let fay = data.find_individual("@I6@").unwrap();
    let line = data.subset(
        data.ancestors("@I6@", Some(2))
            .iter()
            .map(|r| r.individual.xref())
            .chain([fay.xref()]),
    );
    // Fay, her parents and grandparents; the families with a partner of
    // them (not Dan's, whose partners are left out); what they cite.
    assert_eq!(
        records(&line),
        [
            "@U1@", "@I1@", "@I2@", "@I3@", "@I5@", "@I6@", "@F1@", "@F2@", "@N1@", "@S1@", "@S2@",
            "@R1@", "@M1@", "@X1@",
        ]
    );
    let out = write(&line, GedcomVersion::V7_0);
    // Pointers to the records left out are cut, with their substructures:
    // Bob's family as a child, Dan as a child, Fay's association with Ivy
    // and the citation it held.
    assert!(out.contains("0 @I2@ INDI\n1 NAME Bob /Example/\n1 FAMS @F1@\n1 OBJE @M1@\n"));
    assert!(out.contains("0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I1@\n1 CHIL @I3@\n1 MARR\n"));
    assert!(out.contains("0 @I6@ INDI\n1 NAME Fay /Example/\n1 FAMC @F2@\n0 @F1@ FAM\n"));
    for gone in [
        "@I4@", "@I7@", "@I9@", "@F0@", "@F3@", "@S3@", "@S4@", "ASSO",
    ] {
        assert!(!out.contains(gone), "{gone}");
    }
    // The untyped record without identifier stays.
    assert!(out.contains("0 _DEFINITIONS\n"));
    assert!(line.dangling_references().is_empty());
    write(&line, GedcomVersion::V5_5_1);
    // The dataset itself is unchanged.
    assert_eq!(data, Dataset::parse(FILE));
}

#[test]
fn families_are_kept_by_a_partner_or_two_children() {
    let data = data();
    // Cal and Dan, siblings whose parents are left out: their family
    // stays for the two of them, without its partners.
    let siblings = data.subset(["@I3@", "@I4@"]);
    let out = write(&siblings, GedcomVersion::V7_0);
    assert!(out.contains("0 @F1@ FAM\n1 CHIL @I3@\n1 CHIL @I4@\n1 MARR\n"));
    // Fay alone: her parents' family has one kept member, a child, and goes.
    let fay = data.subset(["@I6@"]);
    assert_eq!(fay.families().len(), 0);
    assert!(write(&fay, GedcomVersion::V7_0).contains("0 @I6@ INDI\n1 NAME Fay /Example/\n0 "));
    // Dan alone: his own family stays, with him as its only partner.
    let dan = data.subset(["@I4@"]);
    assert!(write(&dan, GedcomVersion::V7_0).contains("0 @F3@ FAM\n1 HUSB @I4@\n0 "));
}

#[test]
fn every_individual_keeps_every_reached_record() {
    let data = data();
    let all: Vec<_> = data.individuals().into_iter().map(|i| i.xref()).collect();
    let whole = data.subset(all);
    // Only the source nothing cites goes.
    let mut expected = records(&data);
    expected.retain(|x| x != "@S4@");
    assert_eq!(records(&whole), expected);
    let mut without = data.clone();
    let uncited = without.handle::<oxidged::model::Source>("@S4@").unwrap();
    without.remove(uncited, Removal::Refuse).unwrap();
    assert_eq!(whole, without);
}

#[test]
fn an_empty_selection_keeps_the_header_and_what_it_needs() {
    let data = data();
    let none = data.subset(std::iter::empty::<&str>());
    assert_eq!(records(&none), ["@U1@"]);
    let out = write(&none, GedcomVersion::V5_5_1);
    assert!(out.contains("1 SUBM @U1@\n"));
    // Identifiers of no individual are ignored.
    assert_eq!(
        records(&data.subset(["@F1@", "@S1@", "@nothing@"])),
        ["@U1@"]
    );
}

#[test]
fn pointers_that_were_dangling_stay_and_duplicates_are_kept_together() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n\
         0 @I1@ INDI\n1 NAME Ann /Example/\n1 FAMC @F9@\n\
         0 @I1@ INDI\n1 NAME Ann /Duplicate/\n\
         0 @I2@ INDI\n1 NAME Bob /Example/\n1 ALIA @I1@\n\
         0 TRLR\n",
    );
    let ann = data.subset(["@I1@"]);
    assert_eq!(ann.individuals().len(), 2);
    let dangling: BTreeSet<_> = ann
        .dangling_references()
        .iter()
        .map(|d| ann.store().xref(d.pointer).to_string())
        .collect();
    assert_eq!(dangling, BTreeSet::from(["@F9@".to_string()]));
    // Bob is kept and his alias of a kept individual stays.
    let bob = data.subset(["@I2@", "@I1@"]);
    let out = GedcomWriter::new().write_to_string(&bob).unwrap();
    assert!(out.contains("1 ALIA @I1@\n"));
    // Bob alone loses it.
    let out = GedcomWriter::new()
        .write_to_string(&data.subset(["@I2@"]))
        .unwrap();
    assert!(!out.contains("ALIA"));
}

#[test]
fn a_subset_can_be_edited() {
    let data = data();
    let mut fay = data.subset(["@I6@"]);
    // Its counts and handles are its own.
    assert_eq!(fay.references("@I6@"), 0);
    assert_eq!(fay.references("@U1@"), 1);
    let handle = fay.handle::<Individual>("@I6@").unwrap();
    fay.get_mut(handle).unwrap().names.clear();
    let added = fay.add(Individual::default()).unwrap();
    assert!(fay.get(added).is_some());
    assert_eq!(fay.individuals().len(), 2);
    let out = write(&fay, GedcomVersion::V7_0);
    assert!(
        out.contains("0 @I6@ INDI\n0 @I"),
        "Fay without her name, then the new record"
    );
}
