---
type: "Feature Reference"
title: "Reading"
description: "How oxidged reads any GEDCOM input without failing or losing data: decoding, line rules, typing, the builder's inputs and options, strict mode and the read report."
tags: [oxidged, reading, builder, encodings]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
---

# Reading

> Part of the [oxidged documentation](index.md).
> See also: [The Dataset](dataset.md) · [Producer Profiles](profiles.md)

Reading never fails and loses nothing. Bytes of any encoding are decoded
(a byte order mark, then a UTF-16 NUL pattern, then valid UTF-8 decide,
before the `HEAD.CHAR` declaration), and every line is read by fixed,
silent rules:

- CR, LF, CR LF and LF CR terminators, mixed or not; blank lines; tabs or
  several spaces between the parts of a line; leading zeros in levels;
  level jumps (the line nests in the deepest open structure of its record);
- `CONT` and `CONC` under any tag, at any level, also after substructures;
- identifiers on substructures, missing `HEAD` or `TRLR`, content after
  `TRLR`;
- extension and unknown tags, under their real parent at their real level.

Each record is then typed as far as it fits the model. What a type has no
field for — an extension, an unknown tag, a second occurrence of a
structure that occurs once, a structure of another shape — is kept in its
`extra`, in order, where it was, and written back. An enumeration value no
variant names is kept as written (`Pedigree::Unknown("stepchild")`), as is
any date, age or time: their grammars read them on demand.

```rust
use oxidged::model::{Dataset, Pedigree, Sex};

let data = Dataset::parse(
    "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 SEX f\n\
     1 FAMC @F1@\n2 PEDI stepchild\n1 _HOBBY Weaving\n0 @F1@ FAM\n0 TRLR\n",
);
let ann = data.individuals().first().unwrap();
assert_eq!(ann.sex(), Some(Sex::Female)); // 5.5.1 values ignore case
let pedigree = ann.child_of().first().unwrap().pedigree().unwrap().value();
assert!(matches!(pedigree, Pedigree::Unknown(t) if t.to_str(&data) == "stepchild"));
assert_eq!(ann.extra().first().unwrap().tag(), "_HOBBY"); // kept, in place
```

## Builder options and strict mode

```rust
use oxidged::{GedcomBuilder, GedcomError};

let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n0 TRLR\n";
let builder = GedcomBuilder::new().max_file_size(50_000_000);
assert!(builder.build_from_str(text).is_ok()); // lenient: read, value kept

match builder.clone().strict(true).build_from_str(text) {
    Err(GedcomError::NonConformant(deviations)) => {
        // line 5: SEX "male": not a value of enumset-SEX
        println!("{}", deviations[0]);
    }
    _ => unreachable!(),
}
```

| Option | Default | Description |
|--------|---------|-------------|
| `strict(bool)` | `false` | Refuse input that does not follow the specification of the version it declares, with every deviation (encoding, lines, structures, cardinalities, payloads, enumerations, pointers, header) |
| `max_file_size(bytes)` | none | Refuse input larger than this before reading it (for GEDZIP, its `gedcom.ged` as it is decompressed) |
| `interpretation(level)` | `Lossless` | How far the producer's conventions are interpreted ([Producer profiles](profiles.md)): `None`, `Lossless` or `BestEffort` |
| `profile(Some(p))` | detected | Read every file as written by this producer instead of detecting it |

Inputs: `build_from_str`, `build_from_bytes` (a `Vec<u8>` of UTF-8 is not
copied), `build_from_bytes_with_encoding`, `build_from_reader` and
`build_from_gedzip`. `Dataset::parse` and `Dataset::from_bytes` read
without options (and interpret nothing). Reading fails only on I/O, a size
limit, strict mode or a GEDZIP container (`GedcomError`).

## Read report

Reading says nothing. The `…_with_report` methods return, with the
dataset, what reading found: the producer, the encoding, every deviation
from the specification (what strict mode would refuse, nothing refused)
and what was interpreted.

```rust
use oxidged::GedcomBuilder;

let file = b"0 HEAD\n1 SOUR FTM\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n1 _FUN\n2 DATE 1900\n0 TRLR\n";
let (data, report) = GedcomBuilder::new().build_from_bytes_with_report(file.to_vec()).unwrap();
assert_eq!(report.detection.profile().id(), "ftm");
assert_eq!(report.deviations.len(), 1); // SEX male
assert_eq!(report.interpretation.rows(), ["VD-093", "X-020"]); // _FUN, a funeral
assert_eq!(data.individuals().first().unwrap().events().len(), 1);
```

---

## Inputs and errors

| Input | Method |
|---|---|
| A string (kept without a copy when owned) | `GedcomBuilder::build_from_str`, `Dataset::parse` |
| Bytes in any encoding (a UTF-8 `Vec<u8>` kept without a copy) | `build_from_bytes`, `Dataset::from_bytes` |
| Bytes in an encoding given by the caller | `build_from_bytes_with_encoding` |
| Any `io::Read` | `build_from_reader` |
| A GEDZIP archive (feature `gedzip`) | `build_from_gedzip` ([GEDZIP](gedzip.md)) |
| The same, with what reading found | `build_from_str_with_report`, `build_from_bytes_with_report`, `build_from_reader_with_report` |

A builder is `Clone` and reusable (`build_*` take `&self`); its getters are
`is_strict`, `file_size_limit` and `interpretation_level`.

`GedcomError` is `#[non_exhaustive]`: `Io` (an `io::Error`), `FileTooLarge {
size, max }`, `NonConformant` (every deviation, in strict mode) and `Gedzip`
(feature `gedzip`). Content never makes reading fail.

## Encodings

Decoding trusts evidence over the declaration and never fails. The first
step that applies wins:

1. a byte order mark selects UTF-8 or UTF-16 LE/BE;
2. a NUL byte in one half of most of the first KiB's code units selects
   UTF-16 without a byte order mark;
3. bytes that read as UTF-8 are UTF-8, whatever `CHAR` says (a stray byte
   is decoded with the declared single-byte set);
4. otherwise the declared set applies, as the producer means it
   ([Producer Profiles](profiles.md));
5. anything else is read as Windows-1252.

No byte is lost: the five bytes Windows-1252 leaves undefined (0x81, 0x8D,
0x8F, 0x90, 0x9D) decode to the C1 control of the same value, as the WHATWG
Encoding Standard does, and ANSEL's undefined bytes to the Latin-1 character
of the same value.

`CHAR` is read from the bytes of the `HEAD` record only.

| Encoding | `CHAR` labels (case, `-` and `_` ignored) |
|---|---|
| UTF-8 (with or without BOM) | `UTF-8` |
| UTF-16 little endian, with or without BOM | `UNICODE`, `UTF-16`, `UTF-16LE`, `UCS-2` |
| UTF-16 big endian | `UTF-16BE` |
| ANSEL, composed to Unicode NFC | `ANSEL`, `MARC-8` |
| ASCII | `ASCII`, `US-ASCII` |
| Windows-1252 | `ANSI`, `WINDOWS-1252`, `CP1252`, `WIN1252`, `MS-ANSI` |
| Windows-1250 (also found by byte statistics under `ANSI`) | `WINDOWS-1250`, `CP1250`, `WIN1250` |
| Windows-1251 | `WINDOWS-1251`, `CP1251`, `WIN1251` |
| ISO-8859-1 (read as Windows-1252, its superset outside C1 controls) | `ISO-8859-1`, `LATIN1`, `ISO-LATIN-1`, `ISO8859` |
| ISO-8859-2 | `ISO-8859-2`, `LATIN2`, `ISO-LATIN-2` |
| ISO-8859-15 | `ISO-8859-15`, `LATIN9` |
| IBM PC (code page 437) | `IBMPC`, `CP437`, `IBM437`, `DOS`, `IBM`, `OEM` |
| Macintosh (Mac OS Roman) | `MACINTOSH`, `MACROMAN`, `MAC` |

`oxidged::encoding` exposes the same machinery: `decode` (the text, the
encoding found and the one declared), `decode_owned`, `decode_as` (a given
encoding), `detect_encoding`, `GedcomEncoding::from_label`, `encode` (text to
bytes, failing with `EncodeError` on a character the encoding lacks) and
`DecodeReader`, which decodes a stream on the fly.

## The lossless tree

`oxidged::tree` reads any file as structures without typing or interpreting
anything: `Tree` (`parse`, `from_bytes`, `records`, `version`,
`declared_version`, `to_structures`, `to_gedcom`), `parse_tree`, the
streaming `TreeReader`, `Structure`, `Xref` and `Tag`. It is what the typed
reader is built on, and what [Validation](validation.md) works on.
