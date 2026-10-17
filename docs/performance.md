---
type: "Benchmark Report"
title: "Performance"
description: "Time, throughput and peak memory of oxidged's pipelines, measured against ged_io 0.17.0, and how they are measured and checked."
tags: [oxidged, performance, benchmarks]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
---

# Performance

> Part of the [oxidged documentation](index.md).
> See also: [Development](development.md)

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
| Read, `Interpretation::None` | 1× (the reference) | 1× | 2.2× the input |
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
