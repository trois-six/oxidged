---
type: "Development Guide"
title: "Development"
description: "Building and checking oxidged: the workspace, the test suites, the conformance corpora, fuzzing, the tools and the CI workflows."
tags: [oxidged, development, testing, fuzzing, ci]
generated: { by: human:maintainer, at: 2026-10-18T09:30:00Z }
---

# Development

> Part of the [oxidged documentation](index.md).
> See also: [Performance](performance.md) · [Roadmap](roadmap.md)

## Workspace

| Package | Directory | Role |
|---|---|---|
| `oxidged` | the root | The library |
| `oxidged-cli` | `cli/` | The `oxidged` command ([Command Line](cli.md)) |
| `oxidged-fuzz` | `fuzz/` | The `cargo fuzz` targets; a workspace of its own |
| `oxidged-perf` | `tools/perf` | Time, throughput and peak memory of every pipeline; a workspace of its own |
| `oxidged-differential` | `tools/differential` | What oxidged and the published ged_io 0.17.0 keep of the same files, and their speed; a workspace of its own |

The workspace is in Rust edition 2024; the minimum supported Rust version is
1.88.

## Checks

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p oxidged --all-targets --no-default-features -- -D warnings
cargo test --workspace --all-features
cargo test -p oxidged --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo +1.88 test --workspace --all-features
```

The command-line tool enables the library's `serde`, `gedzip` and `calendar`
features, which cargo unifies across the workspace: the library without
features is checked on its own (`-p oxidged`). `cargo fmt --check` also runs
in `fuzz/`, `tools/perf` and
`tools/differential` (`--manifest-path`), and the CI tests each feature on its
own with `cargo hack --workspace --each-feature`.

The Rust examples of this documentation are compiled and run as
documentation tests (`src/lib.rs`), and `tests/docs.rs` checks that `docs/` is
a conformant OKF v0.2 bundle: the frontmatter of every document, the index,
and every relative link.

## Test suites

- `tests/*.rs`: reading, writing, the typed model, editing, streaming,
  encodings, conversion, GEDZIP, JSON, producer profiles, validation and the
  documentation, with property tests (idempotence, bounded inputs).
- `tests/conformance`: the conformance suite. It runs vendored cases
  (`tests/fixtures/conformance`, licences in its `LICENSES.md`), cases
  written for the suite and cases generated from the specification tables,
  against an output checker independent of the crate. Every expected failure
  is listed in `tests/fixtures/conformance/known_gaps.tsv`, a ratchet: a gap
  that closes must leave the list, a new one fails the suite.
- Opt-in corpora: `tools/fetch-corpora.sh` fetches files that are not
  vendored (their licences do not allow it), each pinned to a commit and
  checked against its SHA-256 (`tests/fixtures/corpora.lock.tsv`); then
  `cargo test --all-features --test conformance -- --ignored`.
- Producer profiles: `tests/profiles.rs` reads the producers' fixtures and
  writes the export fixtures; `BLESS=1` rewrites the expected outputs after a
  deliberate change, and regenerates [Extension Tags](extensions.md).

Every name, place and value in the tests is fictitious.

## Fuzzing

`cargo +nightly fuzz run <target>` in `fuzz/`:

| Target | Property |
|---|---|
| `entry_points` | Every reading entry point is total on any bytes |
| `tree` | The decoder and the lexer are total and lossless |
| `writer` | Any output re-parses, follows the line rules and is written again the same |
| `spec` | The validator and the conformance repair: a repaired tree validates |
| `model` | The typed model reads any input and writes stably |
| `convert` | Conversion between 5.5.1 and 7.x is conformant and stable |
| `editing` | Edit sequences keep a dataset whole: counts, lookups, no dangling pointer unless asked |
| `profiles` | Any input read with any profile at any level is conformant and idempotent, and every export is |

## Tools

- `tools/perf`: `generate` writes a generated dataset (plain, or as FTM or
  PAF write it), `run` measures one pipeline, `check` runs every pipeline
  against the CI's limits ([Performance](performance.md)).
- `tools/differential`: compares what oxidged and ged_io 0.17.0 keep of the
  same files (reviewed differences in `allow.tsv`), and their speed (`perf`).
- `tools/spec-tables/gen.py`: generates the specification tables
  (`src/spec/tables.rs`) from the pinned FamilySearch and gedcom-go inputs.
- `tools/encoding`: generates the code page tables and the ANSEL
  composition table from Python's codec and Unicode tables.
- Criterion benchmarks: `cargo bench --bench read`, `--bench write`,
  `--bench memory`.

## Continuous integration

| Workflow | Jobs |
|---|---|
| `ci.yml` (pushes to `main`, pull requests, tags, manual runs) | Formatting, clippy, tests and documentation on stable, beta and nightly; the MSRV; each feature alone; Linux, Windows and macOS; peak memory and throughput (`tools/perf check`); `cargo audit` of the lockfile; on a `v*` tag, the release. A change limited to the documentation (`docs/`, Markdown files) runs none of these jobs, a tag or a manual run runs them all (the `changes` job decides); the `CI` job, the check to require, passes when each succeeded or was skipped |
| `nightly.yml` (daily when `main` moved since the previous run, cancelled runs aside — its `changes` job skips the others; manual runs always run) | Five minutes of fuzzing per target, the crashing inputs kept as artifacts; the opt-in corpora, the corpus-wide validation and repair, and the performance smoke test in a release build |
| `release.yml` | The GitHub release and the `oxidged` binaries for Linux, Windows and macOS (x86_64 and ARM64) |
