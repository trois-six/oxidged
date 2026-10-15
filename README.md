# oxidged

**Read any GEDCOM file, write conformant GEDCOM 5.5.1, 7.0 and 7.1**

[![Crates.io](https://img.shields.io/crates/v/oxidged.svg)](https://crates.io/crates/oxidged)
[![Documentation](https://docs.rs/oxidged/badge.svg)](https://docs.rs/oxidged)
[![CI](https://github.com/trois-six/oxidged/actions/workflows/ci.yml/badge.svg)](https://github.com/trois-six/oxidged/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

---

## What is oxidged?

`oxidged` is a Rust library for reading and writing
[GEDCOM](https://en.wikipedia.org/wiki/GEDCOM) files, the standard for
exchanging genealogical data between family tree software.

It reads real-world files as they are — any encoding, any line
terminators, extensions, misplaced or repeated structures, values outside
the specification — and keeps all their data, with no warning and no
failure. It writes strictly conformant GEDCOM 5.5.1, 7.0 or 7.1, whatever
the data holds, repairing what the target version does not permit and
reporting each repair.

### Key Features

| Feature | Description |
|---------|-------------|
| **Three versions** | GEDCOM 5.5.1, 7.0 (to its latest release, 7.0.18) and 7.1 (FamilySearch's draft), read and written; every structure of each has a type |
| **Conversion** | 5.5.1 to 7.x and back, following the FamilySearch migration guide: multimedia and source records, roles, translations, languages, media types and file URIs, identifiers, dates and ages |
| **Read & write** | A lossless, lenient reader, and a writer conformant by construction; data that does not fit a type is kept, in place, and written back |
| **Streaming** | Read and write one record at a time, in any encoding and with any line terminators, memory bounded by the largest record |
| **Editing** | Add, remove, rename and link records with their references kept whole: stable handles, pointer counts, no dangling pointer unless asked |
| **Indexed lookups** | Records by identifier and families from individuals in constant time |
| **GEDZIP** | Read and write `.gdz` archives bundling a GEDCOM 7.x dataset with its media files, converting 5.5.1 (feature `gedzip`) |
| **Encodings** | UTF-8, UTF-16 LE/BE (with or without BOM), ANSEL (to Unicode NFC), ASCII, Windows-1252 (`ANSI`), ISO-8859-1, ISO-8859-15, IBM PC (cp437), Macintosh; decoding never fails; in memory and streaming |
| **JSON** | `Serialize` and `Deserialize` for the dataset with serde, for JSON and other self-describing formats (feature `serde`) |
| **Type safe** | A Rust type per structure, enumerations whose unknown values keep their text, typed identifiers and pointers |
| **Producer profiles** | 34 producers' conventions as data: the producer detected, its `CHAR` read its way, its extensions read as the standard says them without loss; exports shaped for 23 importers, conformant, every lossy fold reported |
| **Validation** | The 5.5.1, 7.0 and 7.1 specification tables: every deviation of a file with its line, an opt-in strict mode, and a repair pass that makes any tree conformant without losing data |
| **Values** | Date, age and time grammars of both versions, conversions between versions, and calendar arithmetic (feature `calendar`) |
| **Fast** | 1.5 to 3 times as fast as ged_io 0.17 to read and up to 4 times as fast to write, at about twice the input's size in memory (see [Performance](#performance)) |
| **Command line** | `oxidged` (package `oxidged-cli`): summarise, validate, repair, convert, re-encode, detect the producer and export for an importer (see [Command Line Tool](#command-line-tool)) |

---

## Installation

```bash
cargo add oxidged
```

or, in `Cargo.toml`:

```toml
[dependencies]
oxidged = "0.1"
```

The minimum supported Rust version is 1.88.

### Optional Features

| Feature | Adds | Dependencies |
|---------|------|--------------|
| `serde` | `Serialize` and `Deserialize` for the dataset (JSON and other self-describing formats) | `serde` |
| `gedzip` | GEDZIP archives (`.gdz`): read, write, media files (`oxidged::gedzip`) | `zip` (deflate only) |
| `calendar` | Calendar arithmetic: conversions between calendars, day numbers, weekdays | `calendrical_calculations`, `calendrier` |

```bash
cargo add oxidged --features serde,gedzip,calendar
```

None is enabled by default; everything else (reading, writing, conversion,
validation, streaming, editing, encodings, producer profiles) is always
built.

---

## Quick Start

### Read a file

```rust,no_run
use oxidged::GedcomBuilder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Any encoding: the bytes are decoded by what they hold and their CHAR.
    let data = GedcomBuilder::new().build_from_reader(std::fs::File::open("family.ged")?)?;

    println!("GEDCOM {}", data.version());
    for person in data.individuals() {
        let name = person.full_name().unwrap_or_default();
        let born = person.birth().and_then(|b| b.date()).map(|d| d.value().to_string());
        println!("{name} {}", born.unwrap_or_default());
    }
    Ok(())
}
```

A dataset keeps the text it was read from once, and every structure as a
row of the table of its type, pointing into that text. Its accessors give
*views* of the rows: `Copy` handles with a method per field, which give a
substructure as a view, a list as `Items`, a text as a `TextRef` that reads
and compares without the store, a pointer as an `XrefId`
(`data.store().xref(id)` is its text). `view.into_owned()` copies a
structure out as its owned type (`Individual`, `Event`, …), which builds and
edits records.

### Write a file

```rust,no_run
use oxidged::{Dataset, GedcomVersion, GedcomWriter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = Dataset::from_bytes(std::fs::read("input.ged")?);

    // In the dataset's own version (`Dataset::version`)…
    std::fs::write("output.ged", GedcomWriter::new().write_to_string(&data)?)?;

    // …or as GEDCOM 7.0, straight into a file, with the repairs it made.
    let file = std::io::BufWriter::new(std::fs::File::create("output70.ged")?);
    let report = GedcomWriter::new()
        .gedcom_version(GedcomVersion::V7_0)
        .write(file, &data)?;
    for repair in &report.repairs {
        println!("{repair}");
    }
    Ok(())
}
```

---

## Reading

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

### Builder options and strict mode

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
| `interpretation(level)` | `Lossless` | How far the producer's conventions are interpreted ([Producer profiles](#producer-profiles)): `None`, `Lossless` or `BestEffort` |
| `profile(Some(p))` | detected | Read every file as written by this producer instead of detecting it |

Inputs: `build_from_str`, `build_from_bytes` (a `Vec<u8>` of UTF-8 is not
copied), `build_from_bytes_with_encoding`, `build_from_reader` and
`build_from_gedzip`. `Dataset::parse` and `Dataset::from_bytes` read
without options (and interpret nothing). Reading fails only on I/O, a size
limit, strict mode or a GEDZIP container (`GedcomError`).

### Deferred report

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

## Producer profiles

Genealogy programs extend GEDCOM their own ways: extension tags
(`_MARNM`, `_FREL`, `_SDATE`), standard tags given another meaning, date
dialects, `CHAR ANSI` meaning one code page or another. `oxidged::profile`
describes them as data, from a compatibility study of about 40 programs:
each rule cites the row of the research it implements.

**Reading.** The builder detects the producer from the header (system
identifier, name, version, corporation, destination, character set, header
extension tags; scored) and from the extension tags the file uses (a
fingerprint, since files pass through several programs), then:

- decodes the bytes as the producer means its `CHAR` (`ANSI` is
  Windows-1252, but GeneWeb's `ANSI`, `ASCII` and `IBMPC` are Latin-1, and
  byte statistics find Windows-1250 under an `ANSI` label);
- interprets its conventions without losing anything (the default,
  `Interpretation::Lossless`): a rule whose standard form says all of it
  replaces the extension (`_FUN` → `EVEN` + `TYPE Funeral`, `_AKA` → a
  `NAME` of type AKA, `_SDATE` → `SDATE`); one that says part of it adds
  the standard form and keeps the extension (`_FREL`/`_MREL` → `PEDI` when
  both agree); inferred meanings apply only at `BestEffort`. Rules are
  keyed by producer, place and tag, never by the tag alone (`_STAT`,
  `_PRIM`, `_TYPE`… mean different things to different programs).
  Interpretation stays in the file's version family; what only 7.x can say
  (`UID`, `SDATE`, `CROP`, associations under events) is interpreted when
  the writer converts the dataset to 7.x.

Extension tags a profile knows and keeps are declared in a 7.x
`HEAD.SCHMA`: with their registered URI when there is one, else with the
URI oxidged publishes for them in [`docs/extensions.md`](docs/extensions.md).
The file is read once either way: a record no rule may change is typed
as it is read, one a rule changes is interpreted as it is read and typed
from what the rules made of it, its texts still those of the input (see
[Performance](#performance)).

**Writing.** `profile::Exporter` writes a dataset for an importer: always
conformant GEDCOM, its version, character set, line layout and limits
chosen for the importer, its data folded into the standard structures the
importer reads, redundant `_` extensions where the importer needs them
(`_FREL`/`_MREL`, `_PRIM`, `_PHOTO`, `_SHAR`), data left out only on
request (`ExportOptions`: media, private records, change dates), and the
program that wrote the data named in `HEAD.SOUR` (`HeadSource`: the
dataset's when the program set it, with the crate beside it in
`HEAD.SOUR._OXIDGED`; the crate's for a dataset read from a file). Every fold
that loses something, and every importer limit the output exceeds, is in
the `ExportReport`.

```rust
use oxidged::profile::Exporter;
use oxidged::Dataset;

let data = Dataset::parse(
    "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
     1 OCCU Weaver\n1 OCCU Miller\n0 TRLR\n",
);
let (text, report) = Exporter::named("geneanet").unwrap().write_to_string(&data).unwrap();
assert!(text.contains("1 OCCU Weaver, Miller\n")); // one profession field on Geneanet
assert!(text.contains("2 VERS 5.5.1\n")); // Geneanet reads no 7.0
for fold in report.lossy() {
    println!("{fold}"); // @I1@: 2 occupations made one (merged, lossy; EXF-013, EXF-038)
}
```

| Tier | Import profiles | Export profiles |
|------|-----------------|-----------------|
| 1 | `ftm` (Family Tree Maker), `ancestry`, `myheritage`, `geni`, `paf`, `ancestral-quest`, `familysearch`, `heredis`, `geneweb`, `geneanet` | `ancestry`, `ftm`, `myheritage`, `geni`, `familysearch`, `paf`, `ancestral-quest`, `geneanet`, `geneweb`, `heredis` |
| 2 | `rootsmagic`, `legacy`, `family-historian`, `gramps`, `macfamilytree`, `webtrees`, `gedcom-l` | `rootsmagic`, `legacy`, `gramps`, `webtrees`, `family-historian`, `macfamilytree` |
| 3 | `brothers-keeper`, `tmg`, `geneatique`, `ancestrologie`, `aldfaer`, `pro-gen`, `reunion`, `tng`, `wikitree`, `geditcom`, `ahnenblatt`, `gensdatapro`, `embla`, `stamboom`, `family-for-windows`, `famberry`, `genealogos` | `ancestris`, `geneatique`, `tng`, `reunion`, `wikitree`, `findmypast`, `ahnenblatt` |

Plus `generic` (any producer) and `oxidged` (what this crate exported,
read as written: no rule, no hint) for reading, and `generic-551`/`generic-70`
for writing. **The tier-1 profiles follow published and observed importer
behaviour, much of it dated: they are to be re-verified against recent
exports and imports** (`Profile::verification`); the others come from the
research alone.

---

## The dataset

| Type | Description |
|------|-------------|
| `Dataset` | The header and the records by type, in file order, and `extra`: records of no type the model has; every structure in the table of its type |
| `View<'_, T>`, `Items<'_, T>`, `TextRef` | A structure where the dataset keeps it, a list of them, a text: what the dataset's accessors give |
| `Individual`, `Family`, `Source`, `Repository`, `Multimedia`, `Submitter`, `Submission`, `SharedNote` | The records (`INDI`, `FAM`, `SOUR`, `REPO`, `OBJE`, `SUBM`, 5.5.1 `SUBN`, 5.5.1 `NOTE` and 7.x `SNOTE`) |
| `Event`, `Name`, `ChildLink`, `SpouseLink`, `IndividualRef`, `Association`, `Citation`, `Note`, `Place`, `Date`, `Age`, `Ordinance`, … | Every substructure of 5.5.1, 7.0 and 7.1 |
| `Text`, `XrefId`, `Store` | Texts, interned identifiers and the store they resolve against |
| `Node` | A structure kept as read (an extension, an unknown tag) |

Lookups and navigation:

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

---

## Editing

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

## Writing

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
dropped: data with no standard form becomes an extension
structure (`_TAG`), or 7.x `OTHER`/`@VOID@` with a `PHRASE`. Dates, ages and
times are written in the target's grammar (`@#DJULIAN@` and `JULIAN`,
`B.C.` and `BCE`, `INT … (…)` and `PHRASE`). Each repair is reported as a
`Repair`; `.on_nonconformant(RepairPolicy::Error)` makes the writer fail
instead. `.output_encoding(OutputEncoding::Ansel)` (or `Utf16Le`,
`Ascii`) writes 5.5.1 bytes in another encoding, with a matching `CHAR`;
`.line_ending(…)`, `.max_line_length(…)` and `.bom(…)` set the rest. The
credentials some programs put in their files (`_PASS`, an `_USER` that is
encrypted or a server login) are left out unless
`.credentials(Credentials::Written)`; the dataset keeps them.

### Conversion between 5.5.1 and 7.x

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

## Streaming

```rust,no_run
use oxidged::model::RecordRef;
use oxidged::{Dataset, GedcomStreamParser};

fn main() -> Result<(), oxidged::GedcomError> {
    let file = std::io::BufReader::new(std::fs::File::open("huge.ged")?);
    let mut kept = Dataset::default();
    for record in GedcomStreamParser::new(file)? {
        let record = record?;
        if let RecordRef::Individual(person) = record.record() {
            println!("{}", person.full_name().unwrap_or_default());
        }
        // Keep the records of interest, without reading the whole file.
        if matches!(record.record(), RecordRef::Header(_) | RecordRef::Family(_)) {
            kept.extend([record]);
        }
    }
    Ok(())
}
```

The streaming parser reads any encoding and any line terminator the
in-memory reader reads, with the same rules and types, decoding on the fly:
memory stays bounded by the largest record. Each record comes as a dataset
of its own, which keeps the record's text and rows; `record.record()` is its
view. `parser.nodes()` yields the records as lossless
structures instead; for a whole file as structures, nothing interpreted,
use `oxidged::tree::Tree` (in memory) or `oxidged::tree::TreeReader`
(streaming).

`GedcomStreamWriter` writes the other way: a header, then each record as
it is given (from a `GedcomStreamParser`, or a dataset), repaired and
converted for the target version as `GedcomWriter` does, in memory bounded
by one record and the identifiers written:

```rust,no_run
use oxidged::model::{Header, RecordRef};
use oxidged::{GedcomStreamParser, GedcomStreamWriter, GedcomVersion, WriterConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = std::io::BufReader::new(std::fs::File::open("huge.ged")?);
    let output = std::io::BufWriter::new(std::fs::File::create("huge-7.ged")?);
    let mut records = GedcomStreamParser::new(input)?;
    let head = records.next().ok_or("empty file")??;
    let default = Header::default();
    let header = match head.record() {
        RecordRef::Header(h) => h.header(),
        _ => &default,
    };
    let config = WriterConfig { version: Some(GedcomVersion::V7_0), ..WriterConfig::default() };
    let mut writer = GedcomStreamWriter::new(output, config, header, &head)?;
    for record in records {
        let record = record?;
        writer.write_record(record.record())?;
    }
    let (_, report) = writer.finish()?;
    eprintln!("{} repairs", report.repairs.len());
    Ok(())
}
```

Knowing one record at a time, it writes what `GedcomWriter` writes of the
same records but where the records to come would change it (a later
record named as an earlier one, the records a conversion adds, the
extension tags a 7.x header declares); its documentation lists each case.

A `Visitor` receives the records of a stream by their type and says
whether to go on (`ControlFlow`); `GedcomStreamParser::visit` stops at the
first break, leaving the rest unread:

```rust
use std::ops::ControlFlow;
use oxidged::model::{Individual, View};
use oxidged::stream::{GedcomStreamParser, StreamedRecord, Visitor};

struct FirstNamed;

impl Visitor for FirstNamed {
    type Break = String;
    fn individual(&mut self, i: View<'_, Individual>, _: &StreamedRecord) -> ControlFlow<String> {
        i.full_name().map_or(ControlFlow::Continue(()), ControlFlow::Break)
    }
}

let file: &[u8] = b"0 HEAD\n0 @I1@ INDI\n0 @I2@ INDI\n1 NAME Ann /Example/\n0 TRLR\n";
let mut parser = GedcomStreamParser::new(file).unwrap();
assert_eq!(parser.visit(&mut FirstNamed).unwrap(), ControlFlow::Break("Ann Example".into()));
```

---

## GEDZIP

```rust,no_run
use oxidged::gedzip::{write_gedzip_with_media, GedzipReader};
use oxidged::GedcomBuilder;
use std::collections::HashMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read the dataset of an archive through the builder (limits, strict mode).
    let data = GedcomBuilder::new().build_from_gedzip(std::fs::File::open("family.gdz")?)?;

    // Extract its media files.
    let mut reader = GedzipReader::new(std::fs::File::open("family.gdz")?)?.max_entry_size(1 << 30);
    for name in reader.media_files().iter().map(|n| n.to_string()).collect::<Vec<_>>() {
        let bytes = reader.read_media_file(&name)?;
        println!("{name}: {} bytes", bytes.len());
    }

    // Write a new archive with media.
    let mut media = HashMap::new();
    media.insert("photos/portrait.jpg".to_string(), std::fs::read("portrait.jpg")?);
    std::fs::write("new.gdz", write_gedzip_with_media(&data, &media)?)?;
    Ok(())
}
```

An archive holds GEDCOM 7.x (§4): a 5.5.1 dataset is converted to 7.0 and
written in UTF-8 with a byte order mark. `add_media_file` refuses the names
GEDZIP reserves (`gedcom.ged`, `META-INF/…`, `MANIFEST.MF`); `media_files`
lists files, not directories; `find_entry` matches a `FILE` reference to its
entry by its exact name first, then decoded (`%20`), then leniently.

---

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

## Validation and Conformance Repair

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

## Dates, Ages and Times

Dates, ages and times are kept exactly as written, and read on demand by
the grammars of both versions (`oxidged::value`), which never fail: wording
they do not understand is kept verbatim. `parse_strict` checks a payload
against one version's grammar; `Date::to_version` and `Age::to_version`
rewrite a value in the other version's syntax, as the writer does.

```rust
use oxidged::model::Date;
use oxidged::value::{Approximation, Calendar, DateValue};
use oxidged::{Dataset, GedcomVersion};

let store = Dataset::default();
let date = Date::new("ABT @#DJULIAN@ 1700");
let DateValue::Approximated(Approximation::About, d) = date.parse(&store) else { panic!() };
assert_eq!((d.calendar, d.year), (Calendar::Julian, Some(1700)));
assert_eq!(date.to_version(&store, GedcomVersion::V7_0).value.to_str(&store), "ABT JULIAN 1700");
```

- **Exact**: `15 MAR 1950`; **range**: `BET 1900 AND 1910`, `BEF 1900`,
  `AFT 1900`; **period**: `FROM 1900 TO 1910`; **approximate**: `ABT`,
  `CAL`, `EST`
- **Interpreted and phrases** (5.5.1): `INT 1900 (about 1900)`, `(unknown)`
- **Dual years** (5.5.1): `15 APR 1699/00`; **before the common era**:
  `44 B.C.`, `44 BCE`
- **Ages**: `25y 3m`, `< 8y`, `> 1y 400d`, `CHILD`; **times**:
  `12:34:56.789`, `02:50Z`

| Calendar | 5.5.1 | 7.0 | Arithmetic (`calendar` feature) |
|----------|-------|-----|:-------------------------------:|
| Gregorian | `@#DGREGORIAN@` | `GREGORIAN` | ✅ |
| Julian | `@#DJULIAN@` | `JULIAN` | ✅ |
| Hebrew | `@#DHEBREW@` | `HEBREW` | ✅ |
| French Republican | `@#DFRENCH R@` | `FRENCH_R` | ✅ |
| Roman, unknown | `@#DROMAN@`, `@#DUNKNOWN@` | `_ROMAN`, `_UNKNOWN` | - |
| Extension calendars | - | `_MYCAL` | - |

---

## Command Line Tool

The command-line tool is the package `oxidged-cli` (in `cli/`); its
command is `oxidged`.

```bash
# Install
cargo install oxidged-cli

# A summary: version, records, pointers to no record
oxidged family.ged

# One individual, or every individual whose surname contains EXAMPLE
oxidged --individual @I1@ family.ged
oxidged --individual-lastname example family.ged

# Check a file against the specification of the version it declares
oxidged --validate --validation-level strict family.ged
Validation: strict - errors: 2, warnings: 0
error: line 14: SEX "male": not a value of enumset-SEX
error: line 27: FAMC @F9@: no record has this identifier

# Rewrite any file as conformant GEDCOM 7.0 (repairs on standard error),
# converting a 5.5.1 file; --keep-obsolete keeps what 7.0 drops as extensions
oxidged --write 7.0 family.ged > family70.ged
oxidged --write 7.0 --keep-obsolete family.ged > family70.ged

# Repair a file in its own version; re-encode a 5.5.1 file
oxidged --sanitize family.ged > clean.ged
oxidged --sanitize --convert-encoding ANSEL family.ged > family-ansel.ged

# Which program wrote it; a file for an importer (lossy folds on standard error)
oxidged --detect family.ged
oxidged --profile geneanet family.ged > for-geneanet.ged
oxidged --list-profiles
```

`oxidged --help` lists every option. Exit codes: 0 success, 1 I/O error,
2 validation errors in strict mode, 3 usage error.

---

## Performance

Measured against the published ged_io 0.17.0, the crate oxidged was forked
from, with `tools/differential` (`perf`:
one crate and one operation per process, release build, on a performance
core of an Intel Core Ultra 7 265H). Small files: CPU cycles per
operation (`perf stat`, steady state over thousands of runs: free of the
clock's changes), instructions in parentheses:

| Input | Read ged_io 0.17 | Read oxidged | Write ged_io 0.17 | Write oxidged |
|-------|----------:|----------:|-----------:|-----------:|
| `tests/fixtures/simple.ged` (47 lines) | 25.8k (115k) | 17.5k (90k) | 10.0k (52k) | 9.5k (46k) |
| `tests/fixtures/sample.ged` (96 lines) | 52.4k (231k) | 29.8k (166k) | 17.9k (94k) | 21.7k (104k) |
| `tests/fixtures/conformance/maximal551.ged` (704 lines) | 484k (1.69M) | 280k (1.21M) | 116k (608k) | 127k (643k) |
| `tests/fixtures/washington.ged` (234 KB) | 8.12M (31.0M) | 3.59M (15.9M) | 2.26M (10.9M) | 2.16M (10.2M) |
| generated, 1,000 individuals (457 KB) | 13.1M (51.3M) | 4.95M (25.3M) | 3.32M (17.8M) | 2.82M (15.3M) |

ged_io 0.17 writes 653 lines of `maximal551.ged`, oxidged the 700 it holds;
written in the other version family (`dataset-converted`), oxidged converts
and repairs what ged_io 0.17 writes as it is: a dataset that needs nothing converted
costs no more than its write (generated: 14.6M instructions), one that
does several times more (`washington.ged`, whose individuals carry
`_UID` and `_FSFTID`: 43M).

Large files, wall time (median of 5) and peak resident set (Linux
`VmHWM`), with `tools/perf`:

| Generated input | Read ged_io 0.17 | Read oxidged | Write ged_io 0.17 | Write oxidged |
|-------|----------:|----------:|-----------:|-----------:|
| 20,000 individuals (9.4 MB) | 89 ms (106 MB/s) | 25 ms (380 MB/s) | 45 ms | 13 ms |
| 172,000 individuals (82 MB) | 1.10 s (75 MB/s) | 0.25 s (333 MB/s) | 0.48 s | 0.12 s |

| Peak memory, 82 MB input | ged_io 0.17 | oxidged |
|---|---:|---:|
| In memory (`GedcomBuilder::build_from_bytes`) | 1.9 GB (23× the input) | 161 MB (1.96×) |
| Streaming read (`GedcomStreamParser`) | — (UTF-8 only) | 4.5 MB, any encoding |
| Streaming write (`GedcomStreamParser` into `GedcomStreamWriter`, 147 MB/s) | — | 23 MB |

Producer profiles, on the 100,000 generated individuals (47 MB) and the
two variants `tools/perf generate` writes of them (`ftm`, `paf`), wall time
(median of 5) and instructions (`perf stat`, one operation) relative to
the plain pipeline, and peak resident set (`tools/perf`: `read-none`,
`read`, `write`, `write-v7`, `export`):

| Pipeline | Time | Instructions | Peak memory |
|---|---:|---:|---:|
| Read, `Interpretation::None` | 1× (as before profiles) | 1× | 2.2× the input |
| Read, default: no rule's tag in the file | 1.08× | 1.08× | 2.3× |
| Read, default: a PAF header, every `DATE` checked for a dialect, none found | 1.2× | 1.21× | 2.3× |
| Read, default: an `_MILT` (FTM) in every individual, every record rewritten | 1.5× | 1.49× | 2.8× |
| Write as 7.0 the FTM file (its events already read as such) | 1.35× the plain 7.0 write | 1.45× | 3.9× |
| Write as 7.0 a PAF file with an `_UID` in every individual (`UID` once converted) | 1.35× the plain 7.0 write | 1.40× | 3.4× |
| Export for Geneanet (27 folds, every individual and family changed) | 1.65× the plain write | 1.47× | 4.0× |

Reading interprets each record a rule may change as it reads it, and only
those: the others are typed as they are read, the records changed are typed
where they stand from what the rules made of them, their texts left in the
input (only the texts a rule writes are kept besides). A change that spans
records (a pedigree said in a family, read where the child is) is made
once every record is read, to the records it concerns only.

Writing a 5.5.1 dataset as 7.x converts and interprets each record that
needs it alone, as it is written: the others are written as they are. An
export writes the dataset as it is, record by record: the folds that
rewrite a value where it stands (places in the target's format, citations
of source records written inline) rewrite it as it is written, while
every structure written is looked at for what any other fold would
change; only the header, the submitters and the submission are folded
first. A dataset another fold changes is folded whole, as drafts, the
folds that read nothing another one beside them changes running record
after record.

A dataset keeps the decoded input once and every structure as a row of
the table of its type, pointing into it: the rows take about two thirds
of the input's size, and reading makes a few hundred allocations, not one
or more per structure. A written file is checked and repaired for its
version as it is written, one record at a time; the specification tables
are built at compile time.
`tools/perf check` runs every pipeline against the limits the CI checks,
and the profiles' against the plain ones;
Criterion benchmarks: `cargo bench --bench read`, `--bench write`,
`--bench memory`.

---

## Building from Source

```bash
git clone https://github.com/trois-six/oxidged.git
cd oxidged
cargo build --workspace --release
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo bench
```

The repository is a workspace of the library (the root) and the
command-line tool (`cli/`). The fuzz targets (`fuzz/`, with `cargo fuzz`)
and the tools (`tools/perf`, `tools/differential`) are packages of their
own, built with `--manifest-path`.

The conformance suite (`tests/conformance`) runs the vendored and
generated cases with every test; `tools/fetch-corpora.sh` fetches the
opt-in corpora for `cargo test --all-features --test conformance --
--ignored`, and `tools/differential` compares what oxidged keeps of
them with the published ged_io 0.17.0.

---

## Documentation

- [API Documentation](https://docs.rs/oxidged) - Full API reference
- [MIGRATION.md](MIGRATION.md) - Migrating from ged_io 0.17
- [GEDCOM 5.5.1 and 7.0](docs/gedcom-7.md) - The differences between the versions, and how oxidged handles them
- [CHANGELOG.md](CHANGELOG.md) - What changed, release by release
- [ROADMAP.md](ROADMAP.md) - Project roadmap and planned features
- [docs/extensions.md](docs/extensions.md) - The extension tags oxidged declares in GEDCOM 7.x headers
- GEDCOM specifications (bundled in this repo):
  - [GEDCOM 7.0.18 Specification (PDF)](docs/FamilySearchGEDCOMv7.pdf), the latest 7.0 release
  - [GEDCOM 5.5.1 Specification (PDF)](docs/ged551.pdf)

---

## Contributing

Contributions are welcome! Areas where help is appreciated:

- Bug reports and feature requests
- Exports of real programs (with fictitious data) to check the producer profiles against
- Additional test cases and edge cases
- Documentation improvements
- Performance optimizations

Please open issues or pull requests on
[github.com/trois-six/oxidged](https://github.com/trois-six/oxidged).

---

## Origin

oxidged is a fork of [`ged_io`](https://github.com/ge3224/ged_io) 0.17.0
(commit `08a055c`), redesigned: a new model, reader, writer, conversion,
validation, producer profiles and streaming pipeline, published under a new
name. ged_io was written by Robert Pirtle, Jacob Benison and its
contributors, and itself began as a fork of
[`pirtleshell/rust-gedcom`](https://github.com/pirtleshell/rust-gedcom); its
history is kept in this repository. [MIGRATION.md](MIGRATION.md) maps the
names and types of ged_io 0.17 to oxidged's.

---

## License

This project is licensed under the [MIT License](license.md). The
specification tables derived from FamilySearch/GEDCOM are Apache-2.0, and
the 5.5.1 tables derived from gedcom-go are MIT; see [NOTICE](NOTICE).

---

## Acknowledgments

Thanks to the authors and contributors of
[`ged_io`](https://github.com/ge3224/ged_io) and
[`rust-gedcom`](https://github.com/pirtleshell/rust-gedcom), on whose work
oxidged is built.

GEDCOM is a specification maintained by [FamilySearch](https://www.familysearch.org/).
