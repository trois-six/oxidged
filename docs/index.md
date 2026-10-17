---
okf_version: "0.2"
---

# Getting started

* [oxidged Quickstart](quickstart.md) - Installing oxidged and its optional features, then reading and writing a first file.
* [Command Line](cli.md) - The oxidged command of the oxidged-cli package: its commands, their reading and writing options, their text and JSON outputs, and its exit codes.

# Library

* [Reading](reading.md) - How oxidged reads any GEDCOM input without failing or losing data: decoding, line rules, typing, the builder's inputs and options, strict mode and the read report.
* [The Dataset](dataset.md) - The typed model of every GEDCOM structure: how a dataset stores records, the views its accessors give, lookups and navigation, ancestors and descendants, subsets, indexed lookups and JSON.
* [Editing](editing.md) - Adding, removing, renaming and linking records with their references kept whole: handles, pointer counts and checked edits.
* [Writing](writing.md) - How oxidged writes strictly conformant GEDCOM 5.5.1, 7.0 or 7.1: line rules, the header, the conformance repair, output options and the write report.
* [Conversion](conversion.md) - How a dataset is converted between GEDCOM 5.5.1 and 7.x, as the FamilySearch migration guide says, when it is written or on demand.
* [Streaming](streaming.md) - Reading and writing one record at a time in bounded memory: the stream parser, the visitor and the stream writer.
* [GEDZIP](gedzip.md) - Reading and writing GEDZIP archives, a GEDCOM 7.x dataset bundled with its media files, with the gedzip feature.
* [Validation](validation.md) - The GEDCOM 5.5.1, 7.0 and 7.1 specification tables, the validator that reports every deviation with its line, and the conformance repair.
* [Dates, Ages and Times](values.md) - The date, age and time grammars of GEDCOM 5.5.1 and 7.0, conversions between versions, and calendar arithmetic with the calendar feature.
* [Producer Profiles](profiles.md) - How oxidged detects the program that wrote a file, reads its conventions without loss, and exports files shaped for an importer.
* [Extension Tags](extensions.md) - The producers' extension tags oxidged keeps and declares in GEDCOM 7.x headers, one anchor per producer and tag, which the URIs it declares point to.

# GEDCOM

* [GEDCOM Specifications](specifications.md) - The GEDCOM versions oxidged reads and writes, the specifications bundled with it, and the related formats.
* [GEDCOM 5.5.1 and 7.0](gedcom-7.md) - The differences between GEDCOM 5.5.1 and 7.0, and how oxidged reads, checks and writes each version.

# Project

* [Performance](performance.md) - Time, throughput and peak memory of oxidged's pipelines, measured against ged_io 0.17.0, and how they are measured and checked.
* [Development](development.md) - Building and checking oxidged: the workspace, the test suites, the conformance corpora, fuzzing, the tools and the CI workflows.
* [Roadmap](roadmap.md) - The work planned for oxidged: the producer profiles, GEDCOM 7.1 and the known conformance gaps.
