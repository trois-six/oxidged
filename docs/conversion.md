---
type: "Feature Reference"
title: "Conversion"
description: "How a dataset is converted between GEDCOM 5.5.1 and 7.x, as the FamilySearch migration guide says, when it is written or on demand."
tags: [oxidged, conversion, gedcom-7]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
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
