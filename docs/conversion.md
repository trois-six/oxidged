---
type: "Feature Reference"
title: "Conversion"
description: "How a dataset is converted between GEDCOM 5.5.1 and 7.x, as the FamilySearch migration guide says, when it is written or on demand."
tags: [oxidged, conversion, gedcom-7]
generated: { by: human:maintainer, at: 2026-10-17T22:12:42Z }
---

# Conversion

> Part of the [oxidged documentation](index.md).
> See also: [Writing](writing.md) · [GEDCOM 5.5.1 and 7.0](gedcom-7.md)

Written in a version of the other family, a dataset is converted first
(`oxidged::convert`), as the FamilySearch migration guide says and, where it
leaves a choice, as the gedcom7code conversions do:

| 5.5.1 | 7.x |
|---|---|
| `NOTE` record, `NOTE @N1@` | `SNOTE` record, `SNOTE @N1@` |
| Multimedia link holding its files | A new `OBJE` record; the link points to it and keeps its title |
| Source citation in words (`SOUR text`) | A new `SOUR` record with the text as its `NOTE`; the citation's `TEXT`s under `DATA` |
| `ASSO.RELA Godfather` | `ASSO.ROLE GODP` + `PHRASE Godfather`; a word no role names is `OTHER` + `PHRASE` |
| `FONE`/`ROMN` with `TYPE` | `TRAN` with `LANG` (`kana` is `ja-Hrkt`, `romaji` `ja-Latn`, …) |
| `LANG English` | `LANG en` (an unknown name `und` + its phrase) |
| `FILE C:\photos\a b.jpg`, `FORM jpg`, `FORM.TYPE photo` | `FILE file:///C:/photos/a%20b.jpg`, `FORM image/jpeg`, `FORM.MEDI PHOTO` |
| `RIN`, `AFN`, `RFN system:id`, `_FSFTID` | `EXID` with its registered `TYPE` |
| `@#DJULIAN@` in text, `30 JAN 1648/49`, `AGE STILLBORN` | `JULIAN`, `30 JAN 1649` + `PHRASE`, `AGE 0y` + `PHRASE` (and a death at that age) |
| `SUBN`, `HEAD.FILE`, `HEAD.CHAR`, `GEDC.FORM`; a record named `@VOID@` | Left out; renamed, with its pointers |
| Extension tags of known meaning (`_LOC`, `_GOV`, …) | Declared in `HEAD.SCHMA` |

Converting to 5.5.1 reverses each of these where 5.5.1 can say it; what it
has no place for is kept as extension structures. With
`.conversion(ConversionOptions::default().keep_obsolete(true))`, the 5.5.1
structures 7.x drops are kept as extensions `HEAD.SCHMA` declares with their
5.5.1 URIs, and restored when converting back, so that 5.5.1 data survives a
round trip through 7.x.

The reader reads the writer's own repairs back as the structures they stand
for, so that a dataset written to a version and read again is the dataset
that was written. An extension `_TAG` the conformance repair made of a
standard `TAG` is read as `TAG`, in the field the standard tag fills, when
all of these hold: its value is none, or a pointer that the standard `TAG`
takes (a text value is kept); it has no extension substructure; it stands
directly under a structure whose typed field takes `TAG`; and the version
does not permit `TAG` there in the form its value selects, or permits it but
`TAG` lacks a required part nothing can stand for. The header, a structure
with a text value and a structure with an extension substructure are read as
written. Written back, the dataset repairs to the same extensions in the
version it was written to, byte for byte, and to the standard tags in 7.x.

The structures read back, by the version the repair is made for:

| Version | Under | Extension of a standard structure (its repair) |
|---|---|---|
| 5.5.1 | `INDI` | `_INIL` (no 5.5.1 place); `_CREA` (no 5.5.1 place); `_SLGC` without its `FAMC`; `_CHAN` without its `DATE`; `_OBJE` without its `FILE` |
| 5.5.1 | `FAM`, `OBJE`, `SOUR`, `NOTE`, `REPO`, `SUBM`, `SUBN` | `_CHAN` without its `DATE`; `_CREA` (no 5.5.1 place); `_OBJE` without its `FILE` (`SOUR`, `SUBM`, `FAM`) |
| 5.5.1 | `PLAC` | `_MAP` without its `LATI` and `LONG` |
| 5.5.1 | the object link of `INDI`, `FAM`, `SOUR`, `SUBM` | `_CROP` (no 5.5.1 place) |
| 5.5.1 | a family event (`HUSB`, `WIFE`) and its `OBJE` | `_HUSB` and `_WIFE` without their `AGE`; `_OBJE` without its `FILE` |
| 5.5.1 | a note | `_SOUR` (no 5.5.1 place) |
| 5.5.1 | an individual event that does not take `FAMC` (`EVEN`) | `_FAMC` (a pointer to the family) |
| 7.0, 7.1 | `INDI`, `FAM`, `OBJE`, `SOUR`, `NOTE`, `REPO`, `SUBM`, `SUBN` | `_CHAN` and `_CREA` without their `DATE` |
| 7.0, 7.1 | `PLAC` | `_MAP` without its `LATI` and `LONG` |
| 7.0, 7.1 | a family event (`HUSB`, `WIFE`) | `_HUSB` and `_WIFE` without their `AGE` |
| 7.0, 7.1 | an individual event that does not take `FAMC` (`EVEN`) | `_FAMC` (a pointer to the family) |

Only the structures whose value is none or a pointer are listed: a repaired
structure with a text value (a date, a text such as `_UID`, `_RFN`) is kept
as an extension, as the writer leaves it. A pointer is read back as well
where the standard tag takes one: an event's `FAMC` that `EVEN` does not
permit is written `_FAMC @F1@` and reads back as the family of the event. A
pointer the writer repaired for naming a record of another type (`_OBJE`
naming a family) is not read back: the writer repaired it for its target,
not for its place, and the standard tag is permitted there. The header is
read as written, its extensions (`_SCHMA` among them) keeping their place,
since the header is completed by its own rules.

```rust
use oxidged::convert::{convert_dataset, ConversionOptions};
use oxidged::{Dataset, GedcomVersion, GedcomWriter};

let data = Dataset::parse(
    "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 OBJE\n2 FILE photo.jpg\n3 FORM jpg\n\
     2 TITL Portrait\n0 TRLR\n",
);
let out = GedcomWriter::new()
    .gedcom_version(GedcomVersion::V7_0)
    .write_to_string(&data)
    .unwrap();
assert!(out.contains("1 OBJE @M1@\n2 TITL Portrait\n"));
assert!(out.contains("0 @M1@ OBJE\n1 FILE photo.jpg\n2 FORM image/jpeg\n"));

// Or as a dataset of the other version.
let v7 = convert_dataset(&data, GedcomVersion::V7_0, &ConversionOptions::default());
assert_eq!(v7.multimedia().len(), 1);
```

---

A 5.5.1 note over 32K is split at line breaks into consecutive records, so
that every record it is moved out of fits the 5.5.1 record size. `convert`
also exposes `convert_structures`, which converts lossless structures.
