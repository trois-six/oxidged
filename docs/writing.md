---
type: "Feature Reference"
title: "Writing"
description: "How oxidged writes strictly conformant GEDCOM 5.5.1, 7.0 or 7.1: line rules, the header, the conformance repair, output options and the write report."
tags: [oxidged, writing, conformance]
generated: { by: human:maintainer, at: 2026-10-17T20:10:00Z }
---

# Writing

> Part of the [oxidged documentation](index.md).
> See also: [Conversion](conversion.md) · [Validation](validation.md)

The writer emits conformant lines in GEDCOM 5.5.1, 7.0 or 7.1, whatever
the data holds:

| Rule | 5.5.1 | 7.0 and 7.1 |
|---|---|---|
| Line length | At most 255 characters, the whole line counted; longer payloads continue with `CONC`, never split at a space or before a combining mark (text with no such place has a space written as a line break, `CONT`) | No limit, no `CONC` |
| Line breaks in text (LF, CR LF, CR) | `CONT` lines | `CONT` lines |
| `@` in text | Every `@` doubled; `@#…@` escapes kept | A leading `@` doubled |
| Identifiers | Valid (at most 22 characters) and unique | `@[A-Z0-9_]+@`, unique, never `@VOID@` |
| Header | `GEDC.VERS`, `GEDC.FORM LINEAGE-LINKED`, `CHAR` naming the output encoding, `SOUR`, `SUBM` | `GEDC.VERS` only, no `FORM` or `CHAR` |
| Last line | Terminated | Terminated |

Before the lines, the structures are repaired for the target version
(`oxidged::spec::conform`): an enumeration value outside its set, text
where a pointer belongs, a date outside the grammar, a structure the
version does not permit or permits once, a missing required substructure,
and in 7.x an `ALIA` to its own record or a pointer that closes a cycle of
source, multimedia and shared note records (GEDCOM 7.0.17). Nothing is
dropped but what the version cannot hold: data with no standard form becomes
an extension structure (`_TAG`), or 7.x `OTHER`/`@VOID@` with a `PHRASE`; a
7.x structure with neither a value nor a substructure (`1 BAPL` alone) is
not permitted (GEDCOM 7.0 §1.2), so it is left out, and reported as `Empty`. Dates, ages and
times are written in the target's grammar (`@#DJULIAN@` and `JULIAN`,
`B.C.` and `BCE`, `INT … (…)` and `PHRASE`). Each repair is reported as a
`Repair`; `.on_nonconformant(RepairPolicy::Error)` makes the writer fail
instead. `.output_encoding(OutputEncoding::Ansel)` (or `Utf16Le`,
`Ascii`) writes 5.5.1 bytes in another encoding, with a matching `CHAR`;
`.line_ending(…)`, `.max_line_length(…)` and `.bom(…)` set the rest. The
credentials some programs put in their files (`_PASS`, an `_USER` that is
encrypted or a server login) are left out unless
`.credentials(Credentials::Written)`; the dataset keeps them.

## Writer options

| Option | Values | Default |
|---|---|---|
| `gedcom_version` | `GedcomVersion::V5_5_1`, `V7_0`, `V7_1` | The dataset's version |
| `output_encoding` | `OutputEncoding::Utf8`, `Utf16Le`, `Ansel`, `Ascii` (5.5.1 only; 7.x is UTF-8) | `Utf8` |
| `bom` | `Bom::Auto`, `Always`, `Never` | `Auto` |
| `line_ending` | `LineEnding::Lf`, `CrLf`, `Cr` | `Lf` |
| `max_line_length` | the 5.5.1 limit, at most 255 | 255 |
| `on_nonconformant` | `RepairPolicy::Repair`, `Error` | `Repair` |
| `conversion` | `ConversionOptions` ([Conversion](conversion.md)) | default |
| `credentials` | `Credentials::LeftOut`, `Written` | `LeftOut` |

`GedcomWriter::with_config(WriterConfig)` takes them all at once.

## Outputs and reports

| Output | Method |
|---|---|
| Any `io::Write` | `write` (returns the `WriteReport`) |
| A string | `write_to_string`, `write_to_string_with_report` |
| A lossless tree, or owned structures | `write_tree`, `write_structures` |
| One record at a time | `GedcomStreamWriter` ([Streaming](streaming.md)) |

A `WriteReport` lists every `Repair` (`kind`, `detail`); a `WriteError` is
an I/O error, a character the output encoding cannot hold (`Unencodable`)
or, under `RepairPolicy::Error`, the first repair needed.

Writing is idempotent: writing what was read back from written output gives
the same bytes, in every version. When the dataset names no `HEAD.SOUR`,
the 5.5.1 header names the crate (`SOUR oxidged`, its version, `NAME
oxidged`).
