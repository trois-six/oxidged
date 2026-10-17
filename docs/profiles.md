---
type: "Feature Reference"
title: "Producer Profiles"
description: "How oxidged detects the program that wrote a file, reads its conventions without loss, and exports files shaped for an importer."
tags: [oxidged, profiles, interpretation, export]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
---

# Producer Profiles

> Part of the [oxidged documentation](index.md).
> See also: [Extension Tags](extensions.md) · [Reading](reading.md)

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
URI oxidged publishes for them in [Extension Tags](extensions.md).
The file is read once either way: a record no rule may change is typed
as it is read, one a rule changes is interpreted as it is read and typed
from what the rules made of it, its texts still those of the input (see
[Performance](performance.md)).

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

## Detection

`profile::detect(bytes)` (or the read report's `detection`) gives a
`Detection`: the profile chosen (`profile()`, `is_known()`), its `score()`,
the `evidence()` it rests on, the producer's version
(`producer_version()`), the header fields read (`head()`), the chain of
profiles it inherits from (`chain()`) and `hints()`: other producers whose
tags the file uses, whose unambiguous extension rules apply too.

`import_profiles()`, `profile(id)`, `export_profiles()` and
`export_profile(id)` list and find the profiles. A `Profile` has an `id`, a
`label`, a `tier`, a `verification`, a `parent`, its `rule_count` and
catalogue `rows`, the encoding it means by a `CHAR` label (`charset`) and
the extension tags it documents (`documented_tags`). An `ExportProfile`
names its `target`: version, encoding, BOM, line ending, maximum line
length, media paths, size and individual limits.

## Interpretation

`Interpretation::None` reads the file as written, `Lossless` (the default)
applies only the rules whose standard form keeps all of the data,
`BestEffort` adds the rules that infer a meaning. `profile::interpret`
applies them to structures; `InterpretReport` lists what was done
(`mappings`, `with_action`, `for_row`, `rows`, `changed`), each mapping with
its `Action` (`Replaced`, `Added`, `Reordered`, `Kept`) and the catalogue
rows it implements. `Dataset::producer` and `Dataset::interpretation` keep
what reading found.

## Exports

`Exporter::new(profile)` or `Exporter::named(id)` writes a dataset for an
importer (`write`, `write_to_string`) and returns an `ExportReport`: every
fold (`folds`; `lossy()`, `is_lossless()`, `for_row`, `for_record`), each
importer limit the output exceeds (`limits`) and the writer's repairs.
`ExportOptions`:

| Option | Effect |
|---|---|
| `version` | Write this version instead of the profile's |
| `omit_media` | Leave out multimedia links and records |
| `omit_private` | Leave out the records whose `RESN` says `confidential` or `privacy`, and the pointers to them |
| `omit_change_dates` | Leave out `CHAN` |
| `home` | The individual written first (the home person of some importers) |
| `media_root` | The folder media paths are made relative to |
| `credentials` | Whether the credentials a file may hold are written (left out by default, reported as a lossy fold) |
| `source` | Which system `HEAD.SOUR` names (`HeadSource`) |

`HeadSource::Auto` (the default) names the program when it set the
dataset's `HEAD.SOUR` (the dataset was not read from a file, or the program
changed it): its `VERS`, `NAME` and `CORP` are kept, and the crate is named
beside it in `HEAD.SOUR._OXIDGED`, declared in a 7.x `HEAD.SCHMA`. A
dataset read from a file and written again names the crate: `HEAD.SOUR
oxidged`, its version and `NAME oxidged`, reported as a relabelling fold,
since the producer of the file read did not export this one.
`HeadSource::Library` always names the crate, `HeadSource::Dataset` always
the dataset's.

## The crate's own output

The `oxidged` import profile recognises what the crate wrote (`HEAD.SOUR
oxidged` or `ged_io`, or the `_OXIDGED` marker) and reads it as written: no
rule, no hint.

## Extension tags

The extension tags a profile keeps are declared in a 7.x `HEAD.SCHMA`, with
their registered URI when there is one and otherwise with the URI oxidged
publishes for them: `profile::docs_uri(profile, tag)`, an anchor of
[Extension Tags](extensions.md) (`profile::EXTENSION_DOCS`).
