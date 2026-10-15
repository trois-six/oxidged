//! Editing a dataset with its references kept whole: handles, adding,
//! removing and renaming records, links of every kind, pointer counts and
//! dangling pointers. Every name is fictitious.

use oxidged::model::{
    Citation, CitationSource, Dataset, EditError, Family, Holder, Individual, IndividualRef, Link,
    Name, Node, Note, Record, RecordKind, Removal, Repository, SharedNote, Source, Submitter,
    Value,
};
use oxidged::tree::{Payload, Structure};
use oxidged::{GedcomVersion, GedcomWriter};

const FAMILY: &str = "0 HEAD\n1 GEDC\n2 VERS 7.0\n1 SUBM @U1@\n\
    0 @U1@ SUBM\n1 NAME Example submitter\n\
    0 @I1@ INDI\n1 NAME Ann /Example/\n1 FAMS @F1@\n1 SOUR @S1@\n2 PAGE p. 1\n2 SNOTE @N1@\n\
    0 @I2@ INDI\n1 NAME Bob /Example/\n1 FAMS @F1@\n1 _GODPARENT @I1@\n\
    0 @I3@ INDI\n1 NAME Cal /Example/\n1 FAMC @F1@\n1 ASSO @I1@\n2 ROLE FRIEND\n\
    0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I1@\n1 CHIL @I3@\n\
    0 @N1@ SNOTE A shared note\n\
    0 @S1@ SOUR\n1 TITL Parish register\n1 REPO @R1@\n\
    0 @R1@ REPO\n1 NAME Example archive\n\
    0 TRLR\n";

fn family() -> Dataset {
    Dataset::parse(FAMILY)
}

/// Every pointer of the dataset's structures, counted by identifier: an
/// oracle independent of the dataset's counts.
fn recount(data: &Dataset) -> std::collections::BTreeMap<String, usize> {
    fn walk(s: &Structure, out: &mut std::collections::BTreeMap<String, usize>) {
        if let Payload::Pointer(p) = &s.payload {
            *out.entry(p.as_str().to_string()).or_default() += 1;
        }
        for c in &s.substructures {
            walk(c, out);
        }
    }
    let mut out = std::collections::BTreeMap::new();
    for s in data.to_structures() {
        walk(&s, &mut out);
    }
    out
}

/// The dataset's counts agree with a recount, for every pointer and
/// every record's identifier.
fn assert_counts(data: &Dataset) {
    let expected = recount(data);
    for (xref, n) in &expected {
        assert_eq!(data.references(xref.as_str()), *n, "{xref}");
    }
    for record in data.records() {
        if let Some(id) = record.xref() {
            let xref = data.store().xref(id);
            let n = expected.get(xref).copied().unwrap_or(0);
            assert_eq!(data.references(id), n, "{xref}");
        }
    }
}

fn write(data: &Dataset) -> String {
    GedcomWriter::new().write_to_string(data).unwrap()
}

fn has(out: &str, line: &str) -> bool {
    out.contains(line)
}

#[test]
fn references_are_counted() {
    let data = family();
    assert_eq!(data.references("@I1@"), 3); // WIFE, _GODPARENT, ASSO
    assert_eq!(data.references("@F1@"), 3);
    assert_eq!(data.references("@S1@"), 1);
    assert_eq!(data.references("@N1@"), 1);
    assert_eq!(data.references("@R1@"), 1);
    assert_eq!(data.references("@U1@"), 1); // the header's
    assert_eq!(data.references("@X9@"), 0);
    assert_counts(&data);
    let i1_record = data.find_individual("@I1@").unwrap().into_owned();
    let i1 = i1_record.xref.unwrap();
    let mut holders = data.referrers(i1);
    holders.sort_by_key(|h| format!("{h:?}"));
    assert_eq!(holders.len(), 3);
}

#[test]
fn handles_survive_edits() {
    let mut data = family();
    let cal = data.handle::<Individual>("@I3@").unwrap();
    let ann = data.handle::<Individual>("@I1@").unwrap();
    assert_eq!(data.position(cal), Some(2));
    // Removing a record before it moves it, not its handle.
    let bob = data.handle::<Individual>("@I2@").unwrap();
    data.remove(bob, Removal::Detach).unwrap();
    assert_eq!(data.position(cal), Some(1));
    assert_eq!(
        data.get(cal).unwrap().full_name().as_deref(),
        Some("Cal Example")
    );
    // A removed record's handle finds nothing, not even its successor.
    assert!(data.get(bob).is_none());
    assert_eq!(
        data.remove(bob, Removal::Refuse),
        Err(EditError::StaleHandle)
    );
    let dan = data.add(Individual::default()).unwrap();
    assert_ne!(dan.erase(), bob.erase());
    assert!(data.get(bob).is_none());
    assert!(data.get(dan).is_some());
    // Renaming keeps it too.
    data.rename("@I1@", "@ANN@").unwrap();
    assert_eq!(data.handle::<Individual>("@ANN@"), Some(ann));
    assert!(data.handle::<Individual>("@I1@").is_none());
    let handles: Vec<_> = data.handles::<Individual>().collect();
    assert_eq!(handles, [ann, cal, dan]);
    assert_eq!(data.handle_at::<Individual>(1), Some(cal));
    assert_eq!(cal.erase().kind(), RecordKind::Individual);
    assert!(cal.erase().typed::<Family>().is_none());
}

#[test]
fn lookups_are_by_index_and_find_the_first() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @X1@ FAM\n0 @X1@ INDI\n1 NAME First\n\
         0 @X1@ INDI\n1 NAME Second\n0 TRLR\n",
    );
    // Individuals come before families in record order; each lookup by
    // type finds the first of its type.
    let Some(oxidged::model::RecordRef::Individual(first)) = data.find("@X1@") else {
        panic!()
    };
    assert_eq!(first.names().first().unwrap().full(), "First");
    assert_eq!(data.find_individual("@X1@"), Some(first));
    assert!(data.find_family("@X1@").is_some());
    assert!(data.find_source("@X1@").is_none());
}

#[test]
fn add_checks_identifiers_and_pointers() {
    let mut data = family();
    // A record without an identifier gets a free one.
    let h = data.add(Individual::default()).unwrap();
    let id = data.get(h).unwrap().xref().unwrap();
    assert_eq!(data.store().xref(id), "@I4@");
    // An identifier another record has.
    let i1 = data.pointer("@I1@").unwrap();
    let dup = Individual {
        xref: Some(i1),
        ..Individual::default()
    };
    assert_eq!(data.add(dup), Err(EditError::DuplicateXref("@I1@".into())));
    // A pointer to no record.
    let nowhere = data.store_mut().intern_xref("@F9@").unwrap();
    let mut child = Individual::default();
    child.child_of.push(oxidged::model::ChildLink::new(nowhere));
    assert_eq!(
        data.add(child.clone()),
        Err(EditError::UnknownXref("@F9@".into()))
    );
    // 7.x `@VOID@` points nowhere by definition.
    let void = data.store_mut().intern_xref("@VOID@").unwrap();
    child.child_of = vec![oxidged::model::ChildLink::new(void)].into();
    data.add(child).unwrap();
    // An id of no store of this dataset.
    let mut other = Dataset::new(GedcomVersion::V7_0);
    for i in 0..50 {
        other.store_mut().intern_xref(&format!("@Z{i}@"));
    }
    let foreign = other.store_mut().intern_xref("@FAR@").unwrap();
    let far = Individual {
        xref: Some(foreign),
        ..Individual::default()
    };
    assert_eq!(data.add(far), Err(EditError::ForeignXref));
    // A record may point to itself.
    let me = data.store_mut().intern_xref("@ME@").unwrap();
    let mut alias = Individual {
        xref: Some(me),
        ..Individual::default()
    };
    alias.detail_mut().aliases.push(IndividualRef::new(me));
    let h = data.add(alias).unwrap();
    assert_eq!(data.references(me), 1);
    // And its removal is not refused for it.
    data.remove(h, Removal::Refuse).unwrap();
    assert_eq!(data.references(me), 0);
    assert_counts(&data);
    assert!(data.dangling_references().is_empty());
}

#[test]
fn records_without_identifiers_and_extension_records() {
    let mut data = Dataset::new(GedcomVersion::V7_0);
    let f = data.add(Family::default()).unwrap();
    let s = data.add(Source::default()).unwrap();
    let n = data.add(SharedNote::default()).unwrap();
    let r = data.add(Repository::default()).unwrap();
    let u = data.add(Submitter::default()).unwrap();
    let ids: Vec<String> = [
        data.get(f).unwrap().xref(),
        data.get(s).unwrap().xref(),
        data.get(n).unwrap().xref(),
        data.get(r).unwrap().xref(),
        data.get(u).unwrap().xref(),
    ]
    .iter()
    .map(|x| data.store().xref(x.unwrap()).to_string())
    .collect();
    assert_eq!(ids, ["@F1@", "@S1@", "@N1@", "@R1@", "@U1@"]);
    // An extension record keeps no identifier unless it has one.
    let tag = data.store_mut().intern_tag("_LOC");
    let loc = data.add(Node::new(tag)).unwrap();
    assert_eq!(data.get(loc).unwrap().xref(), None);
    assert_eq!(data.extra().len(), 1);
}

#[test]
fn removal_refuses_detaches_or_leaves_dangling() {
    let mut data = family();
    let ann = data.handle::<Individual>("@I1@").unwrap();
    assert_eq!(
        data.remove(ann, Removal::Refuse),
        Err(EditError::StillReferenced {
            xref: "@I1@".into(),
            references: 3
        })
    );
    assert_eq!(data.individuals().len(), 3);

    let mut detached = data.clone();
    let removed = detached.remove(ann, Removal::Detach).unwrap();
    // The record removed keeps its pointers.
    assert_eq!(removed.citations.len(), 1);
    assert!(detached.dangling_references().is_empty());
    let out = write(&detached);
    assert!(!out.contains("@I1@"), "{out}");
    assert!(has(&out, "0 @F1@ FAM\n1 HUSB @I2@\n1 CHIL @I3@\n"), "{out}");
    // Its pointers are released: the source it cited is free to go.
    assert_eq!(detached.references("@S1@"), 0);
    assert_eq!(detached.references("@N1@"), 0);
    let source = detached.handle::<Source>("@S1@").unwrap();
    detached.remove(source, Removal::Refuse).unwrap();
    assert_counts(&detached);

    let mut dangling = data.clone();
    dangling.remove(ann, Removal::LeaveDangling).unwrap();
    let left = dangling.dangling_references();
    assert_eq!(left.len(), 3);
    assert!(
        left.iter()
            .all(|d| dangling.store().xref(d.pointer) == "@I1@")
    );
    assert_eq!(dangling.references("@I1@"), 3);
    assert_counts(&dangling);
}

#[test]
fn a_duplicate_identifier_keeps_its_pointers_resolved() {
    let mut data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME First\n0 @I1@ INDI\n1 NAME Second\n\
         0 @F1@ FAM\n1 CHIL @I1@\n0 TRLR\n",
    );
    // The first goes: the pointers now name the second, nothing dangles.
    let first = data.handle::<Individual>("@I1@").unwrap();
    data.remove(first, Removal::Refuse).unwrap();
    assert_eq!(
        data.find_individual("@I1@").unwrap().into_owned().names[0].full(&data),
        "Second"
    );
    let second = data.handle::<Individual>("@I1@").unwrap();
    assert!(data.remove(second, Removal::Refuse).is_err());
}

#[test]
fn rename_moves_every_pointer() {
    let mut data = family();
    let before = data.references("@I1@");
    let id = data.rename("@I1@", "@ANN@").unwrap();
    assert_eq!(data.store().xref(id), "@ANN@");
    assert_eq!(data.references("@ANN@"), before);
    let out = write(&data);
    assert!(!out.contains("@I1@"), "{out}");
    for line in [
        "0 @ANN@ INDI\n",
        "1 WIFE @ANN@\n",
        "1 _GODPARENT @ANN@\n",
        "1 ASSO @ANN@\n",
    ] {
        assert!(has(&out, line), "{line}{out}");
    }
    assert_eq!(
        data.rename("@ANN@", "@I2@"),
        Err(EditError::DuplicateXref("@I2@".into()))
    );
    assert_eq!(
        data.rename("@ANN@", "ANN"),
        Err(EditError::InvalidXref("ANN".into()))
    );
    assert_eq!(
        data.rename("@ANN@", "@A B@"),
        Err(EditError::InvalidXref("@A B@".into()))
    );
    // A name only dangling pointers hold would take them.
    data.store_mut().intern_xref("@GHOST@");
    let mut g = Individual::default();
    g.detail_mut().aliases.push(IndividualRef::new(
        data.store_mut().intern_xref("@GHOST@").unwrap(),
    ));
    data.push(Record::Individual(Box::new(g)));
    assert_eq!(
        data.rename("@ANN@", "@GHOST@"),
        Err(EditError::XrefInUse("@GHOST@".into()))
    );
    // The old name is free again: a new record may take it.
    let h = data
        .add(Individual {
            xref: data.store().find_xref("@I1@"),
            ..Individual::default()
        })
        .unwrap();
    assert_eq!(
        data.store().xref(data.get(h).unwrap().xref().unwrap()),
        "@I1@"
    );
    assert_counts(&data);
}

#[test]
fn renaming_to_an_invalid_identifier_is_repaired_on_write() {
    let mut data = family();
    data.rename("@I1@", "@ann@").unwrap();
    // Not a 7.0 identifier: the writer renames it, and its pointers.
    let out = write(&data);
    assert!(oxidged::spec::validate_text(&out).is_empty(), "{out}");
    assert!(!out.contains("@ann@"), "{out}");
    data.rename("@ann@", "@ANN@").unwrap();
    let out = write(&data);
    assert!(out.contains("0 @ANN@ INDI\n"), "{out}");
}

#[test]
fn links_of_every_kind() {
    let mut data = family();
    let ann = data.handle::<Individual>("@I1@").unwrap();
    let fam = data.handle::<Family>("@F1@").unwrap();
    let src = data.handle::<Source>("@S1@").unwrap();
    let note = data.handle::<SharedNote>("@N1@").unwrap();
    let sub = data.handle::<Submitter>("@U1@").unwrap();
    let obje = data.add(oxidged::model::Multimedia::default()).unwrap();
    let obje_id = data.get(obje).unwrap().xref().unwrap();
    let ext = data.store_mut().intern_tag("_WITNESS");
    let cases: &[(Holder, Link, &str)] = &[
        (ann.into(), Link::ChildOf, "@F1@"),
        (ann.into(), Link::Alias, "@I3@"),
        (ann.into(), Link::Association, "@I2@"),
        (ann.into(), Link::Note, "@N1@"),
        (ann.into(), Link::Submitter, "@U1@"),
        (ann.into(), Link::AncestorInterest, "@U1@"),
        (ann.into(), Link::DescendantInterest, "@U1@"),
        (ann.into(), Link::Extension(ext), "@I2@"),
        (fam.into(), Link::Association, "@I3@"),
        (fam.into(), Link::Citation, "@S1@"),
        (fam.into(), Link::Note, "@N1@"),
        (fam.into(), Link::Submitter, "@U1@"),
        (src.into(), Link::Note, "@N1@"),
        (note.into(), Link::Citation, "@S1@"),
        (sub.into(), Link::Note, "@N1@"),
        (Holder::Header, Link::Note, "@N1@"),
    ];
    for &(holder, link, target) in cases {
        let before = data.references(target);
        data.link(holder, link, target).unwrap();
        assert_eq!(data.references(target), before + 1, "{link:?}");
        assert_eq!(
            data.link(holder, link, target),
            Err(EditError::AlreadyLinked(link)),
            "{link:?}"
        );
    }
    data.link(ann, Link::Multimedia, obje_id).unwrap();
    data.link(src, Link::Multimedia, obje_id).unwrap();
    data.link(sub, Link::Multimedia, obje_id).unwrap();
    assert_eq!(data.references(obje_id), 3);
    assert_counts(&data);
    let out = write(&data);
    assert!(oxidged::spec::validate_text(&out).is_empty(), "{out}");
    assert!(has(&out, "1 _WITNESS @I2@\n"), "{out}");

    // Type checks and inapplicable links.
    assert!(matches!(
        data.link(ann, Link::ChildOf, "@I2@"),
        Err(EditError::WrongType {
            expected: RecordKind::Family,
            found: RecordKind::Individual,
            ..
        })
    ));
    assert_eq!(
        data.link(fam, Link::ChildOf, "@F1@"),
        Err(EditError::NotApplicable(Link::ChildOf))
    );
    assert_eq!(
        data.link(ann, Link::ChildOf, "@F9@"),
        Err(EditError::UnknownXref("@F9@".into()))
    );
    assert_eq!(
        data.link(fam, Link::Husband, "@I3@"),
        Err(EditError::Occupied(Link::Husband))
    );

    // Unlinking removes every one, with its substructures' pointers.
    for &(holder, link, target) in cases {
        let before = data.references(target);
        assert_eq!(data.unlink(holder, link, target), Ok(1), "{link:?}");
        assert_eq!(data.references(target), before - 1, "{link:?}");
        assert_eq!(
            data.unlink(holder, link, target),
            Err(EditError::NotLinked(link))
        );
    }
    // A citation goes with the note pointer under it.
    assert_eq!(data.references("@N1@"), 1);
    assert_eq!(data.unlink(ann, Link::Citation, "@S1@"), Ok(1));
    assert_eq!(data.references("@N1@"), 0);
    assert_counts(&data);
}

#[test]
fn header_links() {
    let mut data = family();
    assert_eq!(
        data.link(Holder::Header, Link::Submitter, "@U1@"),
        Err(EditError::AlreadyLinked(Link::Submitter))
    );
    assert_eq!(data.unlink(Holder::Header, Link::Submitter, "@U1@"), Ok(1));
    assert_eq!(data.references("@U1@"), 0);
    data.header_mut().unwrap().submitter = Some(data.pointer("@U1@").unwrap());
    assert_eq!(data.references("@U1@"), 1);
    let mut empty = Dataset::new(GedcomVersion::V7_0);
    assert_eq!(
        empty.link(Holder::Header, Link::Note, "@N1@"),
        Err(EditError::UnknownXref("@N1@".into()))
    );
    empty.add(SharedNote::default()).unwrap();
    assert_eq!(
        empty.link(Holder::Header, Link::Note, "@N1@"),
        Err(EditError::NoHeader)
    );
    assert!(
        empty
            .set_header(Some(oxidged::model::Header::default()))
            .is_none()
    );
    empty.link(Holder::Header, Link::Note, "@N1@").unwrap();
    assert_eq!(empty.references("@N1@"), 1);
    assert!(empty.set_header(None).is_some());
    assert_eq!(empty.references("@N1@"), 0);
}

#[test]
fn children_and_spouses_both_ways() {
    let mut data = Dataset::new(GedcomVersion::V7_0);
    let fam = data.add(Family::default()).unwrap();
    let ann = data.add(Individual::default()).unwrap();
    let bob = data.add(Individual::default()).unwrap();
    let cal = data.add(Individual::default()).unwrap();
    let dan = data.add(Individual::default()).unwrap();
    data.link_spouse(fam, ann).unwrap();
    data.link_spouse(fam, bob).unwrap();
    assert_eq!(
        data.link_spouse(fam, dan),
        Err(EditError::Occupied(Link::Husband))
    );
    assert_eq!(
        data.link_spouse(fam, ann),
        Err(EditError::AlreadyLinked(Link::SpouseOf))
    );
    data.link_child(fam, cal).unwrap();
    assert_eq!(
        data.link_child(fam, cal),
        Err(EditError::AlreadyLinked(Link::ChildOf))
    );
    let out = write(&data);
    assert!(
        has(&out, "0 @F1@ FAM\n1 HUSB @I1@\n1 WIFE @I2@\n1 CHIL @I3@\n"),
        "{out}"
    );
    assert!(has(&out, "0 @I1@ INDI\n1 FAMS @F1@\n"), "{out}");
    assert!(has(&out, "0 @I3@ INDI\n1 FAMC @F1@\n"), "{out}");
    let f = data.get(fam).unwrap();
    assert_eq!(data.parents(f).count(), 2);
    assert_eq!(data.children(f).count(), 1);
    // One side already there: the other is added.
    data.unlink(cal, Link::ChildOf, "@F1@").unwrap();
    data.link_child(fam, cal).unwrap();
    assert_eq!(data.get(cal).unwrap().child_of().len(), 1);
    assert_eq!(data.unlink_child(fam, cal), Ok(2));
    assert_eq!(
        data.unlink_child(fam, cal),
        Err(EditError::NotLinked(Link::ChildOf))
    );
    assert_eq!(data.unlink_spouse(fam, ann), Ok(2));
    assert!(data.get(fam).unwrap().husband().is_none());
    data.link_spouse(fam, dan).unwrap();
    assert_eq!(
        data.get(fam).unwrap().husband_id(),
        data.get(dan).unwrap().xref()
    );
    assert_counts(&data);
    assert!(data.dangling_references().is_empty());
}

#[test]
fn edits_through_a_lent_record_are_counted() {
    let mut data = family();
    let ann = data.handle::<Individual>("@I1@").unwrap();
    let s1 = data.pointer("@S1@").unwrap();
    let tag = data.store_mut().intern_tag("_SEEN_IN");
    {
        let mut a = data.get_mut(ann).unwrap();
        a.citations.clear(); // its SOUR @S1@, with SNOTE @N1@ under it
        a.notes.push(Note::shared(s1)); // a wrong type: kept, counted
        a.extra.push(Node {
            payload: Value::Pointer(s1),
            ..Node::new(tag)
        });
    }
    assert_eq!(data.references("@S1@"), 2);
    assert_eq!(data.references("@N1@"), 0);
    assert_counts(&data);
    // The identifier too: a duplicate is kept as reading keeps one.
    let i2 = data.pointer("@I2@").unwrap();
    data.get_mut(ann).unwrap().xref = Some(i2);
    assert_eq!(
        data.find_individual("@I2@").unwrap().into_owned().names[0].full(&data),
        "Ann Example"
    );
    assert!(data.find_individual("@I1@").is_none());
    let out = write(&data);
    assert!(oxidged::spec::validate_text(&out).is_empty(), "{out}");
    // Every individual, in turn.
    data.edit_all::<Individual>(|i| i.names.push(Name::new("Also /Known/")));
    assert!(data.individuals().iter().all(|i| i.names().len() == 2));
    assert_counts(&data);
}

#[test]
fn a_forgotten_guard_changes_nothing() {
    let mut data = family();
    let before = write(&data);
    let ann = data.handle::<Individual>("@I1@").unwrap();
    let mut guard = data.get_mut(ann).unwrap();
    guard.citations.clear();
    std::mem::forget(guard);
    // The guard held a copy: the record and its counts are as they were.
    assert_eq!(data.references("@S1@"), 1);
    assert_eq!(data.references("@I1@"), 3);
    assert!(data.find_individual("@I1@").is_some());
    assert!(data.dangling_references().is_empty());
    assert_eq!(write(&data), before);
    assert_counts(&data);
    // Edits go on, another loan of the same record included.
    data.get_mut(ann).unwrap().citations.clear();
    assert_eq!(data.references("@S1@"), 0);
    assert_counts(&data);
    let source = data.handle::<Source>("@S1@").unwrap();
    data.remove(source, Removal::Detach).unwrap();
    assert_counts(&data);
}

#[test]
fn redirect_merges_two_records() {
    let mut data = family();
    let i2 = data.pointer("@I2@").unwrap();
    assert_eq!(data.redirect("@I1@", i2), Ok(3));
    assert_eq!(data.references("@I1@"), 0);
    assert_eq!(data.references("@I2@"), 4); // its HUSB, and the three

    let ann = data.handle::<Individual>("@I1@").unwrap();
    data.remove(ann, Removal::Refuse).unwrap();
    assert!(data.dangling_references().is_empty());
    assert_eq!(
        data.redirect("@I2@", "@NONE@"),
        Err(EditError::UnknownXref("@NONE@".into()))
    );
    assert_counts(&data);
}

#[test]
fn detach_removes_every_pointer_to_a_record() {
    let mut data = family();
    assert_eq!(data.detach("@I1@"), 3);
    assert_eq!(data.references("@I1@"), 0);
    assert_eq!(data.detach("@I1@"), 0);
    let out = write(&data);
    assert!(!has(&out, "@I1@\n"), "{out}");
    assert!(has(&out, "0 @I1@ INDI\n"), "{out}");
    assert_counts(&data);
}

#[test]
fn push_is_unchecked() {
    let mut data = family();
    let i1 = data.pointer("@I1@").unwrap();
    let f9 = data.store_mut().intern_xref("@F9@").unwrap();
    let mut twin = Individual {
        xref: Some(i1),
        ..Individual::default()
    };
    twin.child_of.push(oxidged::model::ChildLink::new(f9));
    let h = data.push(Record::Individual(Box::new(twin))).unwrap();
    assert_eq!(h.kind(), RecordKind::Individual);
    assert_eq!(data.dangling_references().len(), 1);
    assert_eq!(
        data.find_individual("@I1@")
            .unwrap()
            .into_owned()
            .names
            .len(),
        1
    );
    // Written conformant all the same: the duplicate renamed, the pointer
    // to no record kept.
    let out = write(&data);
    assert!(oxidged::spec::validate_text(&out).is_empty(), "{out}");
    assert_counts(&data);
}

#[test]
fn citations_as_users_build_them() {
    let mut data = family();
    let s1 = data.pointer("@S1@").unwrap();
    let mut eve = Individual::default();
    eve.citations.push(Citation {
        source: CitationSource::Pointer(s1),
        ..Citation::default()
    });
    data.add(eve).unwrap();
    assert_eq!(data.references(s1), 2);
}

#[test]
fn errors_display() {
    let e = EditError::StillReferenced {
        xref: "@I1@".into(),
        references: 2,
    };
    assert_eq!(e.to_string(), "@I1@: 2 pointers to the record");
    let w = EditError::WrongType {
        xref: "@I1@".into(),
        expected: RecordKind::Family,
        found: RecordKind::Individual,
    };
    assert_eq!(w.to_string(), "@I1@: an individual, not a family");
}

/// An untyped record whose own payload points to a record removed with
/// its pointers loses that payload, and keeps the rest (found by fuzzing:
/// a family pointed to by an unknown record's payload).
#[test]
fn detaching_clears_an_untyped_records_payload() {
    let mut data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @X1@ _FAMILY_OF @F1@\n1 _NOTE kept\n\
         0 @F1@ FAM\n0 TRLR\n",
    );
    assert_eq!(data.references("@F1@"), 1);
    let family = data.handle::<Family>("@F1@").unwrap();
    assert!(data.remove(family, Removal::Refuse).is_err());
    data.remove(family, Removal::Detach).unwrap();
    assert!(data.dangling_references().is_empty());
    assert_eq!(data.references("@F1@"), 0);
    let out = write(&data);
    assert!(has(&out, "0 @X1@ _FAMILY_OF\n1 _NOTE kept\n"), "{out}");
    assert_counts(&data);
}

#[test]
fn a_structure_aside_that_fits_its_field_once_edited_is_written_in_order() {
    // The name holds a note, which its field has no room for: the reading
    // keeps it aside, after the record's notes.
    let mut data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n1 SUBM @U1@\n\
         0 @U1@ SUBM\n1 NAME Example submitter\n2 NOTE @N1@\n1 NOTE @N2@\n\
         0 @N1@ NOTE A note\n0 @N2@ NOTE Another note\n0 TRLR\n",
    );
    // Removing the note detaches it: the name fits its field again.
    let note = data.handle::<SharedNote>("@N1@").unwrap();
    data.remove(note, Removal::Detach).unwrap();
    for version in [GedcomVersion::V5_5_1, GedcomVersion::V7_0] {
        let writer = GedcomWriter::new().gedcom_version(version);
        let out = writer.write_to_string(&data).unwrap();
        let name = out.find("1 NAME Example submitter").unwrap();
        let notes = out
            .find("1 NOTE @N2@")
            .or_else(|| out.find("1 SNOTE @N2@"))
            .unwrap();
        assert!(name < notes, "{version}: {out}");
        let again = writer
            .write_to_string(&Dataset::parse(out.as_str()))
            .unwrap();
        assert_eq!(again, out, "{version}");
    }
}
