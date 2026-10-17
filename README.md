# oxidged

**Read any GEDCOM file, write conformant GEDCOM 5.5.1, 7.0 and 7.1**

[![Crates.io](https://img.shields.io/crates/v/oxidged.svg)](https://crates.io/crates/oxidged)
[![Documentation](https://docs.rs/oxidged/badge.svg)](https://docs.rs/oxidged)
[![CI](https://github.com/trois-six/oxidged/actions/workflows/ci.yml/badge.svg)](https://github.com/trois-six/oxidged/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

oxidged is a Rust library for [GEDCOM](https://en.wikipedia.org/wiki/GEDCOM),
the format genealogy programs exchange family trees in. It reads real-world
files as they are — any encoding, any line terminators, extensions, misplaced
or repeated structures, values outside the specification — and keeps all
their data, with no warning and no failure. It writes strictly conformant
GEDCOM 5.5.1, 7.0 or 7.1, whatever the data holds, repairing what the target
version does not permit and reporting each repair. A command-line tool,
`oxidged`, does the same from a shell.

## Highlights

- **Three versions**: GEDCOM 5.5.1, 7.0 (to its 7.0.18 release) and 7.1 (the
  draft), read and written, with a Rust type for every structure of each.
- **Lenient reading, conformant writing**: nothing read is lost — what fits
  no type is kept in place and written back — and every write is conformant
  and idempotent.
- **Conversion**: 5.5.1 to 7.x and back, as the FamilySearch migration guide
  says.
- **[Producer profiles](docs/profiles.md)**: the conventions of 34
  genealogy programs as data — the program that wrote a file detected, its
  character set and extension tags read as it meant them without loss — and
  exports shaped for 23 importers, every lossy fold reported.
- **Validation**: every deviation of a file from its specification, with its
  line; an opt-in strict mode; a repair that makes any tree conformant.
- **[Tested for conformance](docs/development.md#test-suites)**: the 82
  official test files of FamilySearch and gedcom7code, plus some 6,800 cases
  generated from the specifications or written by hand, run against an
  independent checker.
- **Editing**: add, remove, rename and link records with their references
  kept whole; ancestors, descendants and subsets of a tree.
- **Streaming**: read and write one record at a time, in bounded memory.
- **Encodings**: UTF-8, UTF-16, ANSEL, ASCII and eight single-byte code pages;
  decoding never fails.
- **GEDZIP, JSON and calendars**: archives with their media, serde
  serialization and calendar arithmetic, as optional features.
- **Fast**: about twice the input's size in memory, hundreds of megabytes a
  second ([Performance](docs/performance.md)).
- **[Command line](docs/cli.md)**: `oxidged` does the same from a shell —
  inspect, validate, convert, export, edit, extract branches, read dates —
  on GEDCOM, GEDZIP and JSON, with reports as text or JSON.

## Quick start

```bash
cargo add oxidged
```

Reading, writing and every feature are described, with examples, in the
[Quickstart](docs/quickstart.md) and the pages it leads to.

The command-line tool:

```bash
cargo install oxidged-cli

oxidged info family.ged                            # version, encoding, producer, records
oxidged validate family.ged                        # every deviation, with its line
oxidged write family.ged -g 7.0 -o family70.ged    # conformant GEDCOM 7.0
oxidged export family.ged -p geneanet -o out.ged   # a file for an importer
oxidged subset family.ged --descendants-of @I1@ -o branch.gdz
```

Every option is in [Command Line](docs/cli.md).

## Documentation

- [`docs/`](docs/index.md) describes what oxidged does, exhaustively: reading,
  the dataset, editing, writing, conversion, streaming, GEDZIP, validation,
  values, producer profiles, the command line and performance.
- [API documentation](https://docs.rs/oxidged) on docs.rs.
- Planned work is in the [Roadmap](docs/roadmap.md).

## Building from source

```bash
git clone https://github.com/trois-six/oxidged.git
cd oxidged
cargo build --workspace --release
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

The repository is a workspace of the library and the command-line tool
(`cli/`); the fuzz targets and the tools are packages of their own. The test
suites, the conformance corpora, fuzzing, the tools and the CI are described
in [Development](docs/development.md).

## Contributing

Contributions are welcome:

- bug reports and feature requests;
- exports of real programs (with fictitious data) to check the producer
  profiles against;
- test cases, documentation and performance improvements.

Please open issues or pull requests on
[github.com/trois-six/oxidged](https://github.com/trois-six/oxidged).

## Origin

oxidged is a fork of [`ged_io`](https://github.com/ge3224/ged_io) 0.17.0
(commit `08a055c`), redesigned: a new model, reader, writer, conversion,
validation, producer profiles and streaming pipeline, published under a new
name. ged_io was written by Robert Pirtle, Jacob Benison and its
contributors, and itself began as a fork of
[`pirtleshell/rust-gedcom`](https://github.com/pirtleshell/rust-gedcom); its
history is kept in this repository.

## License

This project is licensed under the [MIT License](license.md). The
specification tables derived from FamilySearch/GEDCOM are Apache-2.0, and
the 5.5.1 tables derived from gedcom-go are MIT; see [NOTICE](NOTICE).

## Acknowledgments

Thanks to the authors and contributors of
[`ged_io`](https://github.com/ge3224/ged_io) and
[`rust-gedcom`](https://github.com/pirtleshell/rust-gedcom), on whose work
oxidged is built.

GEDCOM is a specification maintained by [FamilySearch](https://www.familysearch.org/).
