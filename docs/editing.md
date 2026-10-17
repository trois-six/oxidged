---
type: "Feature Reference"
title: "Editing"
description: "Adding, removing, renaming and linking records with their references kept whole: handles, pointer counts and checked edits."
tags: [oxidged, editing, integrity]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
---

# Editing

> Part of the [oxidged documentation](index.md).
> See also: [The Dataset](dataset.md)

A dataset keeps its references whole as it is edited. Its records are
read through it (`data.individuals()`, `data.families()`, …) and changed
through it: it knows the record of every identifier and counts the
pointers to it, so that it can refuse what would break a reference and
find any record in constant time.

```rust
use oxidged::model::{Dataset, EditError, Family, Individual, Link, Removal, Source};
use oxidged::{GedcomVersion, GedcomWriter};

let mut data = Dataset::parse(
    "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
     0 @S1@ SOUR\n1 TITL Parish register\n0 TRLR\n",
);
// Handles stay valid whatever else changes, until their record goes.
let ann = data.handle::<Individual>("@I1@").unwrap();
let bob = data.add(Individual::default()).unwrap(); // given @I2@
let family = data.add(Family::default()).unwrap(); // given @F1@
data.link_spouse(family, ann).unwrap(); // HUSB and FAMS, both ways
data.link_spouse(family, bob).unwrap(); // WIFE
data.link(ann, Link::Citation, "@S1@").unwrap(); // any kind of pointer
assert_eq!(data.references("@S1@"), 1); // constant time

// Removing a record other structures point to is refused, unless asked.
let source = data.handle::<Source>("@S1@").unwrap();
assert!(matches!(
    data.remove(source, Removal::Refuse),
    Err(EditError::StillReferenced { references: 1, .. })
));
data.remove(source, Removal::Detach).unwrap(); // and Ann's citation with it
assert!(data.dangling_references().is_empty());

// Renaming moves every pointer at once.
data.rename("@I1@", "@ANN@").unwrap();
// Any field can be edited through a lent record, a copy stored back (its
// pointers counted again) when the guard is dropped.
data.get_mut(ann).unwrap().names[0] = oxidged::model::Name::new("Anna /Example/");

let out = GedcomWriter::new().write_to_string(&data).unwrap();
assert!(out.contains("0 @F1@ FAM\n1 HUSB @ANN@\n1 WIFE @I2@\n"));
assert!(out.contains("0 @ANN@ INDI\n1 NAME Anna /Example/\n1 FAMS @F1@\n"));
```

| Operation | Does |
|---|---|
| `add::<T>(record)` | Adds a record and returns its `Handle<T>`; refuses an identifier another record has and pointers to no record (but 7.x `@VOID@`); gives a record without identifier a free one (`@I3@`) |
| `push(record)` | Adds a record unchecked, as reading does (a repeated identifier, pointers to no record) |
| `remove(handle, Removal)` | Removes a record and releases its pointers; while other structures point to it, `Refuse`s, `Detach`es them (removing each with its substructures) or `LeaveDangling` |
| `rename(xref, new)` | Gives an identifier a new text, which its record and every pointer take at once; refuses a name a record has or pointers hold |
| `link` / `unlink(holder, Link, target)` | Adds or removes a pointer of any kind from a record or the header: `FAMC`, `FAMS`, `HUSB`, `WIFE`, `CHIL`, `ALIA`, `ASSO`, `SOUR`, `NOTE`/`SNOTE`, `OBJE`, `REPO`, `SUBM`, `ANCI`, `DESI`, `HEAD.SUBN`, extension structures (`_TAG @X1@`); checks the target's type |
| `link_child`, `link_spouse` (and `unlink_…`) | Link a family and an individual both ways |
| `redirect(from, to)`, `detach(xref)` | Move every pointer from one record to another (to merge two); remove every pointer to one |
| `get(handle)`, `get_any(handle)` | The record of a handle, as a view |
| `get_mut(handle)`, `get_mut_at::<T>(i)`, `header_mut()`, `edit_all::<T>(f)` | Lend records to edit any field, pointer fields and extensions included: a copy, stored back when the guard is dropped |
| `references(xref)`, `referrers(xref)`, `dangling_references()` | Count the pointers to an identifier, list their holders, list the pointers to no record |

Identifiers and pointers are 4-byte interned ids, so the integrity of a
dataset is a few tables keyed by id rather than counted pointers: where
the record of each identifier is (built as the file is read), the count
of pointers to each (built the first time it is asked for, then kept by
every edit) and the handles (built the first time one is asked for).
Reading and writing never pay for the counts or the handles; the writer
uses the identifier table to skip its check of every record's
identifier when the dataset's are valid and unique.

---
