# Changelog

All notable changes to this project are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.0] — 2026-10-15

### Changed

- The workspace (the library, `oxidged-cli`, the fuzz targets and the
  tools) is written in Rust edition 2024 instead of 2021, and formatted in
  rustfmt's 2024 style. The MSRV stays 1.88, the public API and every
  output are unchanged: `Detection::chain` keeps its 2021 lifetime capture
  through `use<>`.

## [0.1.0] — 2026-10-15

The first release of oxidged: a Rust library that reads any GEDCOM file
leniently, without losing data and without warnings, and writes strictly
conformant GEDCOM 5.5.1, 7.0 or 7.1, idempotently; with a command-line
tool, `oxidged` (package `oxidged-cli`).

oxidged is a fork of [ged_io](https://github.com/ge3224/ged_io) 0.17.0
(commit `08a055c`), redesigned and published under a new name. Everything
below is relative to ged_io 0.17.0; [MIGRATION.md](MIGRATION.md) maps its
names and types to oxidged's.

### Packaging

- The library is the crate `oxidged`; the command-line tool is the
  package `oxidged-cli` (`cargo install oxidged-cli`), whose binary is
  `oxidged` (ged_io's library package built the `ged_io` binary). The
  repository is a workspace of both.
- Features: `serde` (it was `json`; `serde_json` is no longer a
  dependency), `gedzip`, `calendar`. None is on by default.
- MSRV 1.88.
- When the crate supplies `HEAD.SOUR` itself, it writes `oxidged`, its
  version and `NAME oxidged`.

### Reading

- Reading never fails and keeps all data: CR, LF, CR LF, LF CR and mixed
  terminators, blank lines, lines without a level, level jumps,
  `CONT`/`CONC` anywhere, extension and unknown tags under any structure,
  unknown records, repeated structures, identifiers on substructures,
  unknown enumeration values and content after `TRLR` are read, every
  line's data kept. What a type has no field for is kept in its `extra`,
  in place, and written back. Repeated structures, `REFN` and `EXID` types,
  every `PHRASE`, name pieces, contacts, notes, citations and the other
  substructures 0.17 dropped or overwrote are kept; no `SURN` is derived
  from a name.
- `GedcomBuilder` returns a `Dataset`, takes `&self`, and reads from a
  string, bytes, bytes in a given encoding, a reader or a GEDZIP archive
  (`build_from_str`, `build_from_bytes`, `build_from_bytes_with_encoding`,
  `build_from_reader`, `build_from_gedzip`); an owned `String` or UTF-8
  `Vec<u8>` is kept as the dataset's text without a copy. Its options:
  `strict` (refuse input that does not follow its specification, with
  every deviation: `GedcomError::NonConformant`), `max_file_size`,
  `interpretation` and `profile`. `ParserConfig` is gone.
- Deferred report: `build_from_bytes_with_report`,
  `build_from_str_with_report` and `build_from_reader_with_report` return
  the dataset and a `ReadReport` (producer, encoding, every deviation from
  the specification, what was interpreted); the other methods say nothing.
- Encodings, decoded by evidence (byte order mark, UTF-16 NUL pattern,
  valid UTF-8, then `HEAD.CHAR`), in memory and streaming (`DecodeReader`),
  never failing: UTF-8, UTF-16 LE/BE with or without BOM, ANSEL (composed to
  Unicode NFC), ASCII, Windows-1250, Windows-1251, Windows-1252 (`ANSI`),
  ISO-8859-1, ISO-8859-2, ISO-8859-15, IBM PC (cp437; also the labels
  `IBM` and `OEM`), Macintosh; `ISO8859` is Latin-1.
  `encoding::encode_to_bytes` is `encoding::encode`, failing with
  `EncodeError`.
- `oxidged::tree`: a lossless tree of any file (`Tree`, `parse_tree`,
  `TreeReader`, `Structure`, `Xref`, `Tag`), replacing the `tokenizer` and
  `parser` modules.
- `oxidged::value`: date, age and time grammars of GEDCOM 5.5.1 and 7.0,
  lenient and strict, which never fail (wording they do not understand is
  kept); conversions between versions (`Date::to_version`,
  `Age::to_version`), `Date::normalize`; calendar arithmetic with the
  `calendar` feature (Gregorian, Julian, Hebrew, French Republican: day
  numbers, conversions, ordering, weekdays). A dual year converted to 7.x
  takes the later year only from January to March and converts back to
  the 5.5.1 date it came from. The grammars do not allocate to check a
  value.
- `GedcomError` is `#[non_exhaustive]`: `Io`, `FileTooLarge`,
  `NonConformant`, `Gedzip`.
- No panic or hang from any input (Hebrew dates, levels over 255, version
  detection, …); every entry point is fuzzed.

### Writing and conformance

- `GedcomWriter` writes a `Dataset` in its version or another (5.5.1, 7.0,
  7.1), to any `io::Write` (`write`), a string (`write_to_string`,
  `write_to_string_with_report`), a tree or structures (`write_tree`,
  `write_structures`). The output is conformant to the target: line rules
  (255 characters in 5.5.1, the whole line counted; `CONC` never splitting
  before a combining mark; no `CONC` in 7.x), `@` escapes of each version,
  valid and unique identifiers (never `@VOID@`), the header the version
  requires, values in the target's grammar, terminated last line, no raw
  control characters, no line break that injects a record.
- With no version configured (`WriterConfig::version` is `None`, the
  default), a dataset is written in its own version, `Dataset::version`:
  the one its file declares, or the one it was made with
  (`Dataset::new`), whether it has a header or not. Pre-release builds
  wrote a dataset without a header declaring a version as 5.5.1, a
  `Dataset::new(GedcomVersion::V7_0)` among them. Records, trees and
  streamed headers, which hold no dataset, are written in the version
  their `HEAD.GEDC.VERS` declares, 5.5.1 when it declares none.
- Before the lines, structures are repaired for the target
  (`spec::conform`): an enumeration value outside its set, text where a
  pointer belongs, a value outside its grammar, a structure the version
  does not permit or permits once, a missing required substructure, and
  in 7.x an `ALIA` to its own record or a pointer closing a cycle of
  source, multimedia and shared note records. Nothing is dropped: data
  with no standard form becomes an extension structure, or 7.x
  `OTHER`/`@VOID@` with a `PHRASE`. Every repair is reported
  (`WriteReport`, `Repair`, `RepairKind`); `RepairPolicy::Error` makes the
  writer fail instead.
- Writing is idempotent: writing what was read back from written output
  gives the same bytes, in every version.
- Options: `OutputEncoding` (5.5.1 in UTF-8, UTF-16 LE, ANSEL or ASCII,
  with a matching `CHAR`), `LineEnding`, `Bom`, `max_line_length`,
  `Credentials` (the credentials a file may hold, `_PASS` and an encrypted
  or server-login `_USER`, are left out unless asked for; the dataset keeps
  them).
- 5.5.1 file formats (`FILE.FORM`) are an open list, by decision: any word
  is accepted and written as given, a media type is written as its file
  format, a missing `FORM` is derived from the file's extension. 7.x keeps
  media types.
- `oxidged::spec`: the GEDCOM 5.5.1, 7.0 (release 7.0.18) and 7.1 (draft)
  specification tables, generated and checked in; a validator
  (`validate`, `validate_text`, `validate_bytes`: every deviation with its
  line, `Deviation`, `DeviationKind`) and the conformance repair. The
  GEDCOM 7.0.17 and 7.0.18 changes are implemented: the `URI`, `TagDef`,
  `Latitude` and `Longitude` data types, file paths as URL strings, the
  pointer rules between records.

### Model and editing

- `oxidged::model`: a type for every structure of GEDCOM 5.5.1, 7.0 and 7.1
  (`Dataset`, `Header`, `Individual`, `Family`, `Source`, `Repository`,
  `Multimedia`, `Submitter`, `Submission`, `SharedNote`, `Event`, `Name`,
  `ChildLink`, `SpouseLink`, `Association`, `Citation`, `Note`, `Place`,
  `Date`, `Age`, `Ordinance`, …), replacing `types`, `GedcomData`,
  `UserDefinedTag` and `HasEvents`. Enumerations keep unknown values
  (`Unknown(Text)`) and 7.x `PHRASE`s; values the crate returns are
  `#[non_exhaustive]`.
- A dataset keeps its decoded input once and every structure as a row of
  the table of its type, texts as spans of the input (`Text`, `Store`),
  identifiers and pointers interned (`XrefId`). Its accessors give views:
  `View<'_, T>` (a `Copy` handle with a method per field), `Items<'_, T>`
  (a list) and `TextRef` (a text that reads and compares without the
  store); `RecordRef` is a view of any record; `into_owned` copies a
  structure out as its owned type.
- Lookups (`find`, `find_individual`, `find_family`, … by text or
  `XrefId`) in constant time; navigation (`families_as_spouse`,
  `families_as_child`, `parents`, `children`, `spouse`,
  `search_individuals`); `dangling_references`. `IndexedDataset` finds the
  families of individuals in constant time; `data_mut` reindexes.
- Referential integrity and editing: `Handle<T>` and `RecordHandle`,
  valid across every edit until their record is removed; `add` (refuses a
  duplicate identifier and pointers to no record, gives a record without
  identifier a free one), `push` (unchecked, as reading), `remove` with
  `Removal` (`Refuse`, `Detach`, `LeaveDangling`), `rename`, `redirect`,
  `detach`; `link`/`unlink` of every kind of pointer (`Link`),
  `link_child`/`link_spouse` and their unlinks both ways; `get_mut`,
  `get_mut_at`, `header_mut` and `edit_all` lend records (`RecordMut`)
  whose pointers are counted again when the loan ends; `references`,
  `referrers`; `EditError`. The counts and handles are built the first
  time they are asked for: reading and writing never pay for them.
- `serde` feature: `Serialize` and `Deserialize` for `Dataset` (texts and
  identifiers as strings, nesting bounded so any dataset reads back within
  `serde_json`'s limit), `Serialize` for `StreamedRecord`; the JSON shape
  follows the new model.
- Removed with the old model: `Gedcom::new(…).parse_data()`, `util`, the
  `Display` and `ImprovedDebug` impls, `GedcomData::stats`,
  `appears_to_be_v7`.

### Conversion between GEDCOM 5.5.1 and 7.x

- `oxidged::convert` (`convert_dataset`, `convert_structures`,
  `ConversionOptions`), run by the writer when the target is of the other
  family, as the FamilySearch migration guide says: multimedia links
  holding files and source citations in words become records; `NOTE`
  records `SNOTE`; `ASSO.RELA` the `ROLE` it names with a `PHRASE`;
  `FONE`/`ROMN` `TRAN` with a language; language names BCP 47 tags; file
  formats media types and file paths URIs (percent-encoded); `RIN`, `AFN`,
  `RFN` and `_FSFTID` `EXID`s of the registered types; escapes in text
  calendar names; `AGE STILLBORN` a death at `0y`; `SUBN`, `HEAD.FILE`,
  `HEAD.CHAR` and `GEDC.FORM` left out; a 5.5.1 record named `@VOID@`
  renamed with its pointers; extension tags of known meaning declared in
  `HEAD.SCHMA`. Each is reversed when converting to 5.5.1.
  `keep_obsolete` keeps what 7.x drops as declared extensions, restored in
  5.5.1.
- A 5.5.1 note over 32K is split at line breaks into consecutive records,
  so that every record it is moved out of fits.

### GEDZIP

- `gedzip` feature: `GedzipReader` (`read_dataset` through the builder's
  limits and strict mode, `media_files`, `read_media_file`, `find_entry`,
  `max_entry_size`), `GedzipWriter`, `read_gedzip`, `write_gedzip`,
  `write_gedzip_with_media`. An archive holds GEDCOM 7.x: a 5.5.1 dataset
  is converted to 7.0. `add_media_file` refuses the names GEDZIP reserves
  (`GedzipError::ReservedName`) and stores a `FILE` payload under its
  decoded name; directories are not listed as media; `find_entry`
  prefers an exact, then a decoded name. `GedzipError` is
  `#[non_exhaustive]`.

### Producer profiles

- `oxidged::profile`: 36 import profiles (`generic`, `oxidged` for this
  crate's own output, read as written, and 34 producers) and 25 export
  profiles (two generic, 23 importers), written as compile-checked data,
  each rule citing the row of the compatibility research it implements.
- Reading detects the producer (`Detection`: the header scored, refined by
  a fingerprint of the extension tags used), decodes `CHAR` as the
  producer means it, and interprets its conventions without loss by
  default (`Interpretation::{None, Lossless, BestEffort}`), keyed by
  producer, place and tag; `InterpretReport` lists what was done.
  `Dataset::producer`, `Dataset::interpretation`.
- `Exporter` writes for an importer, always conformant (`ExportOptions`:
  media, private records, change dates, credentials, `HeadSource`), and
  returns an `ExportReport` of every fold, lossy or not, and of the
  importer limits exceeded. An export names the program that wrote the
  data in `HEAD.SOUR` when the program set it, with the crate beside it in
  `HEAD.SOUR._OXIDGED`; the crate for a dataset read from a file.
- Extension tags a profile keeps are declared in a 7.x `HEAD.SCHMA` with
  their registered URI or the one oxidged documents in
  [`docs/extensions.md`](docs/extensions.md).
- The `oxidged` profile recognises the crate's output by `HEAD.SOUR
  oxidged`, `HEAD.SOUR ged_io` (written by pre-release builds) or the
  `_OXIDGED` marker.

### Streaming

- `GedcomStreamParser` yields `StreamedRecord`s (a dataset of one record:
  `record()`, `dataset()`, `into_parts()`), reads any encoding and line
  terminator the in-memory reader reads, decoding on the fly, gives correct
  line numbers and stays in memory bounded by the largest record;
  `nodes()` yields lossless structures; `Dataset` collects and extends
  from streamed records.
- `stream::Visitor` and `GedcomStreamParser::visit`: records by type,
  `ControlFlow` to stop, the rest left unread.
- `GedcomStreamWriter`: a header, then records one at a time, then the
  trailer, with the conformance repair, conversion and line rules of
  `GedcomWriter`, in memory bounded by one record and the identifiers
  written; its documentation lists where it differs from writing the whole
  dataset. `GedcomStreamWriter::for_dataset` starts with a dataset's
  header, in its version unless one is configured, as `GedcomWriter`
  writes it; `GedcomStreamWriter::new` with a header and a store.

### Performance

- Reading is 1.5 to 3 times as fast as ged_io 0.17 and writing up to 4
  times, measured with `tools/differential` (README, Performance).
- A dataset takes about twice the size of its input (ged_io 0.17: about
  23 times on an 82 MB file); reading makes a few hundred allocations,
  not one or more per structure. Streaming reads in a few megabytes, any
  encoding.
- The specification tables and indexes are built at compile time; a
  dataset whose identifiers are valid and unique is written without a
  look at every record's identifier.
- Producer profiles cost about 1.5 times the instructions of the plain
  pipeline at most (an export 1.5 times the write, a 7.0 write of a PAF
  file 1.4 times the plain one), within their memory budgets.

### Tooling

- A conformance suite (`tests/conformance`): vendored, written and
  generated cases for 5.5.1, 7.0 and 7.1, with a known-gaps ratchet and
  opt-in corpora (`tools/fetch-corpora.sh`, pinned by commit and SHA-256).
- Fuzz targets (`fuzz/`, `cargo fuzz`) over every entry point, the
  decoder and lexer, the writer, the validator and repair, the typed
  model, the conversion, edit sequences and the producer profiles.
- `tools/perf`: time, throughput and peak memory of every pipeline, with
  the limits the CI checks (`tools/perf check`); `tools/differential`:
  what this crate and the published ged_io 0.17.0 keep of the same files,
  and their speed; `tools/spec-tables`: the generator of the
  specification tables.
- CI: tests on stable, beta and nightly, MSRV, each feature alone, Linux,
  Windows and macOS, peak memory and throughput, security audit; nightly
  fuzzing and corpora.
- Command line (`oxidged`): a summary of a file, one individual,
  individuals by name, `--validate` (lenient or strict), `--write <5.5.1|7.0|7.1>`
  (`--keep-obsolete`), `--sanitize`, `--convert-encoding`, `--profile`,
  `--detect`, `--list-profiles`, `--interpretation`.

[Unreleased]: https://github.com/trois-six/oxidged/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/trois-six/oxidged/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/trois-six/oxidged/releases/tag/v0.1.0
