---
type: "Feature Reference"
title: "Validation"
description: "The GEDCOM 5.5.1, 7.0 and 7.1 specification tables, the validator that reports every deviation with its line, and the conformance repair."
tags: [oxidged, validation, conformance, spec]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
---

# Validation

> Part of the [oxidged documentation](index.md).
> See also: [Writing](writing.md) · [GEDCOM Specifications](specifications.md)

`oxidged::spec` holds the specifications as data (generated from the
FamilySearch GEDCOM 7.0.18 tables, the 7.1 draft's and a 5.5.1 transcription checked
against the PDF; see `NOTICE`). It validates a file, a parsed tree or owned
structures, and repairs a tree so that it validates:

```rust
use oxidged::spec::{conform, validate, validate_bytes};
use oxidged::tree::parse_tree;
use oxidged::GedcomVersion;

let bytes = b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n0 TRLR\n";
for deviation in validate_bytes(bytes) {
    println!("{deviation}"); // line 5: SEX "male": not a value of enumset-SEX
}

let mut records = parse_tree("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n0 TRLR\n")
    .to_structures();
let repairs = conform(&mut records, GedcomVersion::V7_0); // SEX male becomes _SEX male
assert!(validate(&records, GedcomVersion::V7_0).is_empty());
```

Every rule the validator checks and every repair are listed in the
[`spec` module documentation](https://docs.rs/oxidged/latest/oxidged/spec/).

One deliberate exception to strict conformance: the 5.5.1 file formats
(`FILE.FORM`, `MULTIMEDIA_FORMAT`) are treated as an open list. The
specification lists only `bmp`, `gif`, `jpg`, `ole`, `pcx`, `tif` and `wav`;
`png`, `pdf` or `webp` are accepted and written as given, a media type
(`image/png`) is written as its file format (`png`), and a missing `FORM` is
derived from the file's extension. GEDCOM 7.0 output keeps media types.

---

## What is checked

The validator and the repair cover the encoding, the line syntax, levels,
identifiers, escapes, line lengths and continuations, record sizes, unknown
and misplaced tags, cardinalities, required substructures, payloads by data
type, enumeration values, pointer targets, dangling pointers, the header,
banned characters and, in 7.x, cycles of pointers between records
(`DeviationKind`). The 7.0 rules follow the 7.0.18 release: the `URI`,
`TagDef`, `Latitude` and `Longitude` data types, file paths as URL strings,
no `ALIA` to its own record, no cycle of source, multimedia and shared note
records. GEDCOM 7.1 follows FamilySearch's draft.

Strict reading (`GedcomBuilder::strict`) refuses a file with any deviation
([Reading](reading.md)); the writer repairs every one
([Writing](writing.md)).
