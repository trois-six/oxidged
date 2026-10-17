---
type: "Reference"
title: "GEDCOM Specifications"
description: "The GEDCOM versions oxidged reads and writes, the specifications bundled with it, and the related formats."
tags: [oxidged, specifications, gedcom-7, gedzip]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
sources:
  - id: gedcom-70
    resource: https://github.com/FamilySearch/GEDCOM/releases/tag/v7.0.18
    title: FamilySearch GEDCOM 7.0.18
  - id: gedcom-71
    resource: https://github.com/FamilySearch/GEDCOM/tree/v7.1
    title: FamilySearch GEDCOM 7.1 draft
  - id: gedcom-551
    resource: https://gedcom.io/specifications/ged551.pdf
    title: GEDCOM 5.5.1
---

# GEDCOM Specifications

> Part of the [oxidged documentation](index.md).
> See also: [GEDCOM 5.5.1 and 7.0](gedcom-7.md) · [Validation](validation.md)

## Versions

| Version | Status | In oxidged |
|---|---|---|
| 5.5.1 | The 1999 draft, published as the standard on 2019-11-15[^gedcom-551] | Read and written; any other 5.x declaration (`5.5`, `5.5.5`) is read as 5.5.1 |
| 7.0 | Released; its latest release is 7.0.18 (17 February 2026)[^gedcom-70] | Read and written, with the 7.0.18 tables and rules |
| 7.1 | Not released: FamilySearch's `v7.1` branch[^gedcom-71] | Read and written as a draft target |

The 7.0 rules include the URI, tag definition, latitude and longitude data
types, file paths as URL strings, the pointer rules between records (no
`ALIA` to its own record, no cycle of source, multimedia and shared note
records) and the GEDZIP entry names.

## The specifications in this directory

- `FamilySearchGEDCOMv7.pdf`: the FamilySearch GEDCOM Specification
  7.0.18, as FamilySearch publishes it: the `gedcom.pdf` asset of the
  v7.0.18 release (tag commit `2f0b863`, SHA-256
  `ec4475db84781878086bb3882d751c685fea6fa3d3d68b1ed90f581ec7dd832a`),
  Apache License 2.0, © 1984-2026 Intellectual Reserve, Inc. The crate's
  7.0 tables come from the same release (`tools/spec-tables/gen.py`).
- `ged551.pdf`: GEDCOM 5.5.1 (2019-11-15 release of the 1999 draft).

The 5.5.1 tables are transcribed from cacack/gedcom-go and corrected against
the PDF (`tools/spec-tables/551-errata.tsv`); the 7.1 tables come from the
`v7.1` branch at commit `c451779`. The licences are in `NOTICE`.

## Related formats

- [GEDZIP](https://gedcom.io/gedzip/), part of GEDCOM 7.0: a ZIP archive of a
  GEDCOM 7.x file (`gedcom.ged`) and the media files it references; see
  [GEDZIP](gedzip.md).
- [GEDCOM X](https://gedcomx.org/) is a different format, not read by
  oxidged; its specifications are at <https://github.com/FamilySearch/gedcomx>.

## Further reading

- <https://en.wikipedia.org/wiki/GEDCOM>
- <https://fr.geneawiki.com/wiki/Norme_Gedcom>
- <https://www.gedcom.org/versions.html>

[^gedcom-551]: GEDCOM 5.5.1
[^gedcom-70]: FamilySearch GEDCOM 7.0.18
[^gedcom-71]: FamilySearch GEDCOM 7.1 draft
