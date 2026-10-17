---
type: "Roadmap"
title: "Roadmap"
description: "The work planned for oxidged: the command-line tool, the producer profiles, GEDCOM 7.1 and the known conformance gaps."
tags: [oxidged, roadmap]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
---

# Roadmap

> Part of the [oxidged documentation](index.md).
> See also: [Development](development.md)

## Command-line tool

`oxidged` becomes a set of subcommands built on `clap`, exposing what the
library does: reading options (encoding, strict mode, producer,
interpretation, read report), every writer and export option, GEDZIP and JSON
input and output, the lossless tree, encodings, producer profiles and their
interpretation, lookups and navigation, editing, subsets of a dataset,
streaming, dates and ages, reports in JSON, shell completions and a manual
page. The command line of version 0.2 is replaced, not kept beside it.

## Producer profiles

The tier-1 profiles follow published and observed importer behaviour, much of
it dated: each is to be checked against recent exports and imports of its
program (`Profile::verification`).

## GEDCOM 7.1

oxidged follows FamilySearch's `v7.1` branch as a draft; once 7.1 is
released, its tables are pinned to the release, as 7.0's are.

## Known conformance gaps

The cases `tests/fixtures/conformance/known_gaps.tsv` lists: 5.5.1 records
over the size limit that no note can bring under it, shared notes in pointer
cycles, and a few lossy round trips of 7.x extensions.

## Excluded

- Locale-aware string handling: Rust strings are UTF-8, and display is the
  application's.
- A SAX-like tag parser: `GedcomStreamParser` (typed) and `tree::TreeReader`
  (lossless structures) cover it.
