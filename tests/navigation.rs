//! The dataset-level API: record lookups, family relationships, name
//! search, record counts, dangling references, the constant-time
//! [`IndexedDataset`] (first record wins on a duplicate identifier,
//! reindexing after edits), and how the model clones and compares. Every
//! name and value is fictitious.

use oxidged::IndexedDataset;
use oxidged::model::{
    Dataset, Family, Individual, IndividualRef, Name, RecordRef, Removal, Repository, Sex, View,
    XrefId,
};

/// A family of three, their sources and the other record types.
const FAMILY: &str = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n\
    0 @I1@ INDI\n1 NAME John Robert /Example/ Jr.\n1 SEX M\n\
    1 BIRT\n2 DATE 1 JAN 1900\n2 PLAC Sampleton\n1 DEAT\n2 DATE 31 DEC 1980\n2 PLAC Exampleville\n\
    1 SOUR @S1@\n2 PAGE 42\n1 FAMS @F1@\n1 FAMS @F2@\n\
    0 @I2@ INDI\n1 NAME Jane /Example/\n1 SEX F\n1 FAMS @F1@\n\
    0 @I3@ INDI\n1 NAME Child /Example/\n1 FAMC @F1@\n1 FAMC @F3@\n\
    0 @I4@ INDI\n1 NAME Bob /Sample/\n1 FAMS @F2@\n\
    0 @F1@ FAM\n1 HUSB @I1@\n1 WIFE @I2@\n1 CHIL @I3@\n1 MARR\n2 DATE 15 JUN 1950\n\
    0 @F2@ FAM\n1 HUSB @I1@\n1 WIFE @I4@\n\
    0 @F3@ FAM\n1 WIFE @I9@\n1 CHIL @I3@\n\
    0 @S1@ SOUR\n1 TITL Census of Sampleton\n1 AUTH Sampleton Registry\n1 REPO @R1@\n\
    0 @R1@ REPO\n1 NAME Sampleton Archives\n\
    0 @M1@ OBJE\n1 FILE photo.jpg\n2 FORM jpg\n2 TITL Wedding photo\n\
    0 @U1@ SUBM\n1 NAME Ann Sample\n\
    0 @N1@ NOTE A shared note\n\
    0 @X1@ _LOC Sampleton\n\
    0 TRLR\n";

fn family() -> Dataset {
    Dataset::parse(FAMILY)
}

fn names<'a>(people: impl Iterator<Item = View<'a, Individual>>) -> Vec<String> {
    people.filter_map(View::full_name).collect()
}

fn xrefs<'a>(data: &Dataset, families: impl Iterator<Item = View<'a, Family>>) -> Vec<String> {
    families
        .filter_map(|f| f.xref())
        .map(|id| data.store().xref(id).to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

#[test]
fn records_are_found_by_identifier() {
    let data = family();
    let john = data.find_individual("@I1@").unwrap();
    assert_eq!(john.full_name().as_deref(), Some("John Robert Example Jr."));
    assert_eq!(
        data.find_family("@F1@").unwrap().husband_id(),
        data.store().find_xref("@I1@")
    );
    let source = data.find_source("@S1@").unwrap();
    assert_eq!(source.title().unwrap(), "Census of Sampleton");
    let repo = data.find_repository("@R1@").unwrap();
    assert_eq!(repo.name().unwrap(), "Sampleton Archives");
    assert!(data.find_multimedia("@M1@").is_some());
    assert!(data.find_submitter("@U1@").is_some());
    assert_eq!(data.find_note("@N1@").unwrap().text(), "A shared note");
    assert!(matches!(data.find("@X1@"), Some(RecordRef::Other(_))));
    assert!(matches!(data.find("@F2@"), Some(RecordRef::Family(_))));

    // Unknown identifiers, and identifiers of another record type.
    assert!(data.find_individual("@I999@").is_none());
    assert!(data.find_family("@F999@").is_none());
    assert!(data.find("@I9@").is_none()); // pointed to, but no record
    assert!(data.find_family("@I1@").is_none());
    assert!(data.find_individual("I1").is_none());
}

#[test]
fn every_kind_of_key_finds_the_same_record() {
    let data = family();
    let id = data.store().find_xref("@I2@").unwrap();
    let by_str = data.find_individual("@I2@").unwrap();
    assert_eq!(by_str.xref(), Some(id));
    // Views of the same record of the same dataset are equal.
    assert_eq!(data.find_individual(id), Some(by_str));
    assert_eq!(data.find_individual(Some(id)), Some(by_str));
    assert_eq!(data.find_individual(String::from("@I2@")), Some(by_str));
    assert!(data.find_individual(None::<XrefId>).is_none());
}

#[test]
fn the_first_record_of_a_duplicate_identifier_wins() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
         0 @I1@ INDI\n1 NAME Duplicate /Example/\n0 TRLR\n",
    );
    // Both are kept.
    assert_eq!(data.individuals().len(), 2);
    let found = data.find_individual("@I1@").unwrap();
    assert_eq!(found.full_name().as_deref(), Some("Ann Example"));
    let indexed = IndexedDataset::new(data.clone());
    let found = indexed.find_individual("@I1@").unwrap();
    assert_eq!(found.full_name().as_deref(), Some("Ann Example"));
    assert_eq!(
        indexed.find("@I1@").map(RecordRef::into_owned),
        data.find("@I1@").map(RecordRef::into_owned)
    );
}

#[test]
fn record_counts_include_the_header() {
    let data = family();
    // The header and 13 records; the trailer is not a record.
    assert_eq!(data.record_count(), 14);
    assert_eq!(data.records().count(), 14);
    assert!(matches!(data.records().next(), Some(RecordRef::Header(_))));
    assert_eq!(data.extra().len(), 1);

    let empty = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 TRLR");
    assert_eq!(empty.record_count(), 1);
    assert_eq!(empty.declared_version(), Some("5.5.1"));
    assert_eq!(Dataset::default().record_count(), 0);
}

// ---------------------------------------------------------------------------
// Relationships
// ---------------------------------------------------------------------------

#[test]
fn families_of_an_individual() {
    let data = family();
    let f = |x| xrefs(&data, data.families_as_spouse(x));
    assert_eq!(f("@I1@"), ["@F1@", "@F2@"]);
    assert_eq!(f("@I4@"), ["@F2@"]);
    assert!(f("@I999@").is_empty());

    let c = |x| xrefs(&data, data.families_as_child(x));
    assert_eq!(c("@I3@"), ["@F1@", "@F3@"]);
    assert!(c("@I1@").is_empty());
}

#[test]
fn parents_children_and_spouses() {
    let data = family();
    let f1 = data.find_family("@F1@").unwrap();
    assert_eq!(
        names(data.parents(f1)),
        ["John Robert Example Jr.", "Jane Example"]
    );
    assert_eq!(names(data.children(f1)), ["Child Example"]);
    assert_eq!(
        data.spouse("@I1@", f1).and_then(View::full_name).as_deref(),
        Some("Jane Example")
    );
    assert_eq!(
        data.spouse("@I2@", f1).and_then(View::full_name).as_deref(),
        Some("John Robert Example Jr.")
    );
    assert!(data.spouse("@I3@", f1).is_none()); // a child, not a partner

    // Partners and children without a record are left out.
    let f3 = data.find_family("@F3@").unwrap();
    assert_eq!(data.parents(f3).count(), 0);
    assert_eq!(data.children(f3).count(), 1);
    assert!(data.spouse("@I9@", f3).is_none());
}

#[test]
fn names_are_searched_ignoring_case_and_slashes() {
    let data = family();
    let search = |q| names(data.search_individuals(q));
    assert_eq!(search("example").len(), 3);
    assert_eq!(search("EXAMPLE").len(), 3);
    assert_eq!(search("jane"), ["Jane Example"]);
    assert_eq!(search("jane example"), ["Jane Example"]);
    assert_eq!(search("bob sample"), ["Bob Sample"]);
    assert!(search("/example/").is_empty());
    assert!(search("xyz").is_empty());
}

#[test]
fn individual_helpers() {
    let data = family();
    let john = data.find_individual("@I1@").unwrap();
    let jane = data.find_individual("@I2@").unwrap();
    assert_eq!(john.sex(), Some(Sex::Male));
    assert_eq!(jane.sex(), Some(Sex::Female));
    assert!(data.find_individual("@I3@").unwrap().sex().is_none());

    let name = john.name().unwrap();
    assert_eq!(name.value(), "John Robert /Example/ Jr.");
    assert_eq!(name.surname_in_value().as_deref(), Some("Example"));

    let birth = john.birth().unwrap();
    assert_eq!(birth.date().unwrap().value(), "1 JAN 1900");
    assert_eq!(birth.place().unwrap().name(), "Sampleton");
    let death = john.death().unwrap();
    assert_eq!(death.date().unwrap().value(), "31 DEC 1980");
    assert_eq!(death.place().unwrap().name(), "Exampleville");

    assert_eq!(john.events().len(), 2);
    assert!(jane.events().is_empty());
    assert!(jane.birth().is_none());
    assert_eq!(john.citations().len(), 1);
    assert!(jane.citations().is_empty());
}

// ---------------------------------------------------------------------------
// Dangling references
// ---------------------------------------------------------------------------

#[test]
fn dangling_references_are_listed_in_record_order() {
    let data = family();
    let dangling = data.dangling_references();
    let found: Vec<_> = dangling
        .iter()
        .map(|d| {
            (
                d.record.map(|r| data.store().xref(r).to_string()),
                d.tag.as_str().to_string(),
                data.store().xref(d.pointer).to_string(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [(
            Some("@F3@".to_string()),
            "WIFE".to_string(),
            "@I9@".to_string()
        )]
    );
}

#[test]
fn dangling_references_of_every_kind_but_void() {
    // Within a record, in the order the model writes its structures.
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n1 SUBM @U9@\n\
         0 @I1@ INDI\n1 FAMC @F9@\n1 SOUR @S9@\n1 SNOTE @N9@\n1 OBJE @M9@\n1 FAMS @VOID@\n\
         0 @S1@ SOUR\n1 REPO @R9@\n0 TRLR\n",
    );
    let found: Vec<_> = data
        .dangling_references()
        .iter()
        .map(|d| format!("{} {}", d.tag.as_str(), data.store().xref(d.pointer)))
        .collect();
    assert_eq!(
        found,
        [
            "SUBM @U9@",
            "FAMC @F9@",
            "SNOTE @N9@",
            "SOUR @S9@",
            "OBJE @M9@",
            "REPO @R9@"
        ]
    );
    assert!(data.dangling_references()[0].record.is_none()); // the header
    assert!(
        Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n")
            .dangling_references()
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// IndexedDataset
// ---------------------------------------------------------------------------

#[test]
fn the_index_agrees_with_the_linear_lookups() {
    let data = family();
    let indexed = IndexedDataset::from(data.clone());
    for xref in [
        "@I1@",
        "@I2@",
        "@I3@",
        "@I4@",
        "@F1@",
        "@F2@",
        "@F3@",
        "@S1@",
        "@R1@",
        "@M1@",
        "@U1@",
        "@N1@",
        "@X1@",
        "@I9@", // pointed to, but no record: its families are found alike
        "@nothing@",
    ] {
        assert_eq!(
            indexed.find(xref).map(RecordRef::into_owned),
            data.find(xref).map(RecordRef::into_owned),
            "{xref}"
        );
        assert_eq!(
            indexed.find_individual(xref).map(|v| v.into_owned()),
            data.find_individual(xref).map(|v| v.into_owned()),
            "{xref}"
        );
        assert_eq!(
            indexed.find_family(xref).map(|v| v.into_owned()),
            data.find_family(xref).map(|v| v.into_owned()),
            "{xref}"
        );
        assert_eq!(
            indexed.find_source(xref).map(|v| v.into_owned()),
            data.find_source(xref).map(|v| v.into_owned()),
            "{xref}"
        );
        assert_eq!(
            indexed.find_repository(xref).map(|v| v.into_owned()),
            data.find_repository(xref).map(|v| v.into_owned()),
            "{xref}"
        );
        assert_eq!(
            indexed.find_multimedia(xref).map(|v| v.into_owned()),
            data.find_multimedia(xref).map(|v| v.into_owned()),
            "{xref}"
        );
        assert_eq!(
            indexed.find_submitter(xref).map(|v| v.into_owned()),
            data.find_submitter(xref).map(|v| v.into_owned()),
            "{xref}"
        );
        assert_eq!(
            indexed.find_note(xref).map(|v| v.into_owned()),
            data.find_note(xref).map(|v| v.into_owned()),
            "{xref}"
        );
        let owned = |f: View<'_, Family>| f.into_owned();
        let spouse: Vec<_> = indexed.families_as_spouse(xref).map(owned).collect();
        assert_eq!(
            spouse,
            data.families_as_spouse(xref).map(owned).collect::<Vec<_>>(),
            "{xref}"
        );
        let child: Vec<_> = indexed.families_as_child(xref).map(owned).collect();
        assert_eq!(
            child,
            data.families_as_child(xref).map(owned).collect::<Vec<_>>(),
            "{xref}"
        );
    }
    // The index is of a clone: its families are of the clone.
    for family in indexed.families() {
        assert_eq!(
            indexed.parents(family).collect::<Vec<_>>(),
            indexed.data().parents(family).collect::<Vec<_>>()
        );
        assert_eq!(
            indexed.children(family).collect::<Vec<_>>(),
            indexed.data().children(family).collect::<Vec<_>>()
        );
        for xref in ["@I1@", "@I2@", "@I4@"] {
            assert_eq!(
                indexed.spouse(xref, family),
                indexed.data().spouse(xref, family)
            );
        }
    }
    // A dataset still, through Deref and data().
    assert_eq!(indexed.individuals().len(), 4);
    assert_eq!(indexed.data(), &data);
    assert_eq!(indexed.into_inner(), data);
}

#[test]
fn an_individual_who_is_both_partners_has_the_family_once() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n0 @F1@ FAM\n1 HUSB @I1@\n1 WIFE @I1@\n0 TRLR\n",
    );
    let indexed = IndexedDataset::new(data);
    assert_eq!(indexed.families_as_spouse("@I1@").count(), 1);
    assert_eq!(indexed.data().families_as_spouse("@I1@").count(), 1);
}

#[test]
fn edits_through_data_mut_reindex() {
    let mut indexed = IndexedDataset::new(family());
    {
        let mut data = indexed.data_mut();
        let id = data.store_mut().intern_xref("@I7@").unwrap();
        let mut eve = Individual {
            xref: Some(id),
            ..Individual::default()
        };
        eve.names.push(Name::new("Eve /Example/"));
        data.add(eve).unwrap();
        // Eve replaces Bob in @F2@, and joins @F1@ as a child.
        let f2 = data.find_family("@F2@").unwrap().xref();
        let at = data.families().iter().position(|f| f.xref() == f2).unwrap();
        data.get_mut_at::<Family>(at).unwrap().wife = Some(IndividualRef::new(id));
        data.get_mut_at::<Family>(0)
            .unwrap()
            .children
            .push(IndividualRef::new(id));
        // A removed record is no longer found.
        for repository in data.handles::<Repository>().collect::<Vec<_>>() {
            data.remove(repository, Removal::Detach).unwrap();
        }
    }
    let eve = indexed.find_individual("@I7@").unwrap();
    assert_eq!(eve.full_name().as_deref(), Some("Eve Example"));
    assert_eq!(indexed.families_as_spouse("@I7@").count(), 1);
    assert_eq!(indexed.families_as_child("@I7@").count(), 1);
    assert_eq!(indexed.families_as_spouse("@I4@").count(), 0);
    let f1 = indexed.find_family("@F1@").unwrap();
    assert_eq!(indexed.children(f1).count(), 2);
    assert!(indexed.find_repository("@R1@").is_none());
    assert!(indexed.find("@R1@").is_none());
}

#[test]
fn an_unedited_guard_changes_nothing() {
    let mut indexed = IndexedDataset::new(family());
    drop(indexed.data_mut());
    assert!(indexed.find_individual("@I1@").is_some());
    assert_eq!(indexed.families_as_spouse("@I1@").count(), 2);
}

// ---------------------------------------------------------------------------
// Clone and PartialEq
// ---------------------------------------------------------------------------

#[test]
fn a_cloned_dataset_equals_its_original() {
    let data = family();
    let cloned = data.clone();
    assert_eq!(data, cloned);
    assert_eq!(cloned.individuals().len(), 4);
    // Its texts resolve against its own store.
    let john = cloned.find_individual("@I1@").unwrap();
    assert_eq!(john.full_name().as_deref(), Some("John Robert Example Jr."));
    assert_eq!(cloned.to_structures(), data.to_structures());
}

#[test]
fn cloned_records_equal_their_originals() {
    let data = family();
    // Records copied out, as owned types, clone and compare.
    assert_eq!(data.header().clone(), data.header());
    let first = |r: Option<RecordRef<'_>>| r.unwrap().into_owned();
    for record in [
        first(data.individuals().first().map(RecordRef::Individual)),
        first(data.families().first().map(RecordRef::Family)),
        first(data.sources().first().map(RecordRef::Source)),
        first(data.repositories().first().map(RecordRef::Repository)),
        first(data.multimedia().first().map(RecordRef::Multimedia)),
        first(data.submitters().first().map(RecordRef::Submitter)),
        first(data.notes().first().map(RecordRef::Note)),
        first(data.extra().first().map(RecordRef::Other)),
    ] {
        assert_eq!(record.clone(), record);
    }
    let individuals = data.individuals().to_vec();
    assert_eq!(individuals.len(), data.individuals().len());
    assert!(
        data.individuals()
            .iter()
            .any(|i| i.into_owned() == individuals[2])
    );
}

#[test]
fn a_modified_clone_differs_and_leaves_the_original_alone() {
    let mut data = family();
    let original = data.individuals().first().unwrap().into_owned();
    let mut modified = original.clone();
    let id = data.store_mut().intern_xref("@I8@").unwrap();
    modified.xref = Some(id);
    modified.sex = Some(Sex::Undetermined);
    assert_ne!(original, modified);
    assert_eq!(data.individuals().first().unwrap().into_owned(), original);
    assert_eq!(data.store().xref(original.xref.unwrap()), "@I1@");

    let mut changed = data.clone();
    changed.get_mut_at::<Individual>(0).unwrap().names.clear();
    assert_ne!(changed, data);
}

#[test]
fn equal_input_reads_to_equal_datasets() {
    let at = |d: &Dataset, i: usize| d.individuals().get(i).unwrap().into_owned();
    let fam = |d: &Dataset, i: usize| d.families().get(i).unwrap().into_owned();
    assert_eq!(family(), family());
    assert_eq!(at(&family(), 1), at(&family(), 1));
    assert_eq!(fam(&family(), 0), fam(&family(), 0));

    let other = Dataset::parse(FAMILY.replace("1 SEX F", "1 SEX M"));
    assert_ne!(other, family());
    assert_ne!(at(&other, 1), at(&family(), 1));
    let other = Dataset::parse(FAMILY.replace("1 WIFE @I4@", "1 WIFE @I3@"));
    assert_ne!(fam(&other, 1), fam(&family(), 1));
}

#[test]
fn texts_compare_as_held_so_records_of_other_datasets_compare_by_their_structures() {
    // The same individual at another place of another file holds other
    // spans: compare such records by their structures.
    let a =
        Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n");
    let b = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n1 LANG en\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n",
    );
    fn first(d: &Dataset) -> View<'_, Individual> {
        d.individuals().first().unwrap()
    }
    let structure = |d: &Dataset| RecordRef::Individual(first(d)).to_structure_in(d.version());
    assert_eq!(structure(&a), structure(&b));
    assert_eq!(first(&a).full_name(), first(&b).full_name());
    // Copied out, they hold spans of their own stores: they differ.
    assert_ne!(first(&a).into_owned(), first(&b).into_owned());
}
