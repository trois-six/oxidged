---
type: "Feature Reference"
title: "The Dataset"
description: "The typed model of every GEDCOM structure: how a dataset stores records, the views its accessors give, lookups and navigation, ancestors and descendants, subsets, indexed lookups and JSON."
tags: [oxidged, model, dataset, serde]
generated: { by: human:maintainer, at: 2026-10-17T13:36:18Z }
---

# The Dataset

> Part of the [oxidged documentation](index.md).
> See also: [Editing](editing.md) · [Streaming](streaming.md)

A dataset keeps the text it was read from once, and every structure as a
row of the table of its type, pointing into that text. Its accessors give
*views* of the rows: `Copy` handles with a method per field, which give a
substructure as a view, a list as `Items`, a text as a `TextRef` that reads
and compares without the store, a pointer as an `XrefId`
(`data.store().xref(id)` is its text). `view.into_owned()` copies a
structure out as its owned type (`Individual`, `Event`, …), which builds and
edits records ([Editing](editing.md)).

## Types

| Type | Description |
|------|-------------|
| `Dataset` | The header and the records by type, in file order, and `extra`: records of no type the model has; every structure in the table of its type |
| `View<'_, T>`, `Items<'_, T>`, `TextRef` | A structure where the dataset keeps it, a list of them, a text: what the dataset's accessors give |
| `Individual`, `Family`, `Source`, `Repository`, `Multimedia`, `Submitter`, `Submission`, `SharedNote` | The records (`INDI`, `FAM`, `SOUR`, `REPO`, `OBJE`, `SUBM`, 5.5.1 `SUBN`, 5.5.1 `NOTE` and 7.x `SNOTE`) |
| `Event`, `Name`, `ChildLink`, `SpouseLink`, `IndividualRef`, `Association`, `Citation`, `Note`, `Place`, `Date`, `Age`, `Ordinance`, … | Every substructure of 5.5.1, 7.0 and 7.1 |
| `Text`, `XrefId`, `Store` | Texts, interned identifiers and the store they resolve against |
| `Node` | A structure kept as read (an extension, an unknown tag) |
| `RecordRef` | A view of any record; `HeaderRef` for the header |

## Lookups and navigation

```rust
use oxidged::Dataset;

let data = Dataset::parse(
    "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 @I2@ INDI\n\
     1 NAME Bob /Example/\n0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I1@\n1 FAMC @F9@\n0 TRLR\n",
);
let ann = data.find_individual("@I1@").unwrap();
let family = data.families_as_spouse(ann.xref()).next().unwrap();
let bob = data.spouse(ann.xref(), family).unwrap();
assert_eq!(bob.full_name().as_deref(), Some("Bob Example"));
assert_eq!(data.search_individuals("example").count(), 2);
// Pointers to no record are a query, not an error.
assert_eq!(data.dangling_references().len(), 1);
```

`find`, `find_individual`, `find_family`, `find_source`,
`find_repository`, `find_multimedia`, `find_submitter` and `find_note`
take an identifier as text or as an `XrefId`; when several records have
one, the first is found. They take constant time: a dataset knows the
record of each identifier. `families_as_spouse` and `families_as_child`
search the families; `IndexedDataset` finds them in constant time too:

```rust
use oxidged::{Dataset, IndexedDataset};

let indexed = IndexedDataset::new(Dataset::parse(
    "0 HEAD\n0 @I1@ INDI\n0 @F1@ FAM\n1 CHIL @I1@\n0 TRLR\n",
));
assert!(indexed.find_individual("@I1@").is_some());
assert_eq!(indexed.families_as_child("@I1@").count(), 1);
```

Enumerations keep a value no variant names as written (`Unknown(Text)`) and
a 7.x `PHRASE` beside their value (`Phrased`); the values the crate returns
are `#[non_exhaustive]`. `Dataset::version` is the version a dataset is
written in by default, `declared_version` the declaration as written,
`record_count` its records, header and `extra` included.

## Ancestors and descendants

`ancestors(individual, generations)` and `descendants(individual,
generations)` list the relatives of an individual generation by
generation, up to `generations` (`None`: all of them), as `Relative`s: the
individual's view and its `generation` (1 for a parent or a child, 2 for a
grandparent or a grandchild, …).

- Families relate them: the parents of an individual are the partners
  (`HUSB`, then `WIFE`) of the families it is a child (`CHIL`) in, its
  children the children of the families it is a partner in, whatever the
  pedigree of the link.
- The walk is breadth first, in the order of the families and of their
  members; a relative reached twice (pedigree collapse, a cycle in the
  data) is listed once, at its nearest generation, and the individual
  itself never.
- Only pointers to an individual's record count: a `CHIL` to no record is
  skipped.

Each call is linear in the size of the dataset; `IndexedDataset` walks its
index instead, in time linear in the relatives and their families.

```rust
use oxidged::Dataset;

let data = Dataset::parse(
    "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n0 @I2@ INDI\n0 @I3@ INDI\n\
     0 @F1@ FAM\n1 HUSB @I2@\n1 CHIL @I1@\n0 @F2@ FAM\n1 WIFE @I3@\n1 CHIL @I2@\n0 TRLR\n",
);
let ancestors = data.ancestors("@I1@", None);
assert_eq!(ancestors.len(), 2);
assert_eq!(ancestors[1].generation, 2);
assert_eq!(data.descendants("@I3@", Some(1)).len(), 1);
```

## Subsets

`subset(individuals)` copies the part of a dataset some individuals need
into a new dataset, the dataset itself unchanged:

| Kept | Rule |
|---|---|
| Individuals | Those with the given identifiers; identifiers of no individual are ignored |
| Families | Those with a kept partner, or with two or more kept children |
| Other records | Those the header, the kept individuals and the kept families point to, at any depth (sources and their repositories, shared notes, multimedia objects, submitters, submissions, extension records), and the untyped records without an identifier |
| The header | As it is, with the version, the declaration and the producer |

Every structure pointing to a record left out — a family's `HUSB`, `WIFE`
or `CHIL`, an individual's `FAMC`, `FAMS`, `ALIA` or `ASSO`, an extension
structure — is removed with its substructures, as `Removal::Detach` removes
them: the subset points to no record the dataset has and it has not, and
pointers that pointed to no record stay as they were. The records keep
their order and identifiers. A subset is a dataset like any other, to edit
or write; taking it is linear in the size of the dataset.

With `ancestors` and `descendants`, it extracts a branch:

```rust
use oxidged::{Dataset, GedcomWriter};

let data = Dataset::parse(
    "0 HEAD\n1 GEDC\n2 VERS 7.0\n\
     0 @I1@ INDI\n1 NAME Ann /Example/\n1 FAMC @F1@\n\
     0 @I2@ INDI\n1 NAME Bob /Example/\n1 FAMS @F1@\n1 SOUR @S1@\n\
     0 @I3@ INDI\n1 NAME Cal /Example/\n1 FAMC @F1@\n\
     0 @F1@ FAM\n1 HUSB @I2@\n1 CHIL @I1@\n1 CHIL @I3@\n\
     0 @S1@ SOUR\n1 TITL Parish register\n0 @S2@ SOUR\n0 TRLR\n",
);
// Bob and his descendants, without Cal.
let mut chosen: Vec<_> = data
    .descendants("@I2@", None)
    .iter()
    .map(|r| r.individual.xref())
    .collect();
chosen.push(data.find_individual("@I2@").unwrap().xref());
chosen.retain(|&x| x != data.find_individual("@I3@").unwrap().xref());
let branch = data.subset(chosen);
let out = GedcomWriter::new().write_to_string(&branch).unwrap();
assert!(out.contains("0 @F1@ FAM\n1 HUSB @I2@\n1 CHIL @I1@\n0 @S1@ SOUR\n"));
assert!(!out.contains("@I3@") && !out.contains("@S2@"));
```

## JSON

With the `serde` feature, a `Dataset` serializes as a map of its records,
each structure a map of its non-empty fields, texts and identifiers as
strings, enumeration values by name (`"Female"`) or as written
(`{"Unknown": "x"}`); it deserializes back into a dataset.

What may nest to any depth is listed flat, so that the JSON of any dataset
nests a few levels deep and reads back within the nesting limit of
`serde_json` (128): a structure kept untyped (`extra`) lists its
`descendants`, each with its `level` below it, and a list of notes (or of
citations) gives each note and citation held in another after it, as
`[depth, {…}]` (a citation at depth 1 of a list of notes, a note at depth
2, and so on).

```rust
fn main() -> Result<(), serde_json::Error> {
    let data = oxidged::Dataset::parse("0 HEAD\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n");
    let json = serde_json::to_string(&data)?;
    assert_eq!(
        json,
        r#"{"version":"5.5.1","header":{},"individuals":[{"xref":"@I1@","names":[{"value":"Ann /Example/"}]}]}"#
    );
    let back: oxidged::Dataset = serde_json::from_str(&json)?;
    assert_eq!(back.to_structures(), data.to_structures());
    Ok(())
}
```

---

`StreamedRecord` (`Serialize`), the values of `oxidged::value` and the
structures of `oxidged::tree` serialize too.
