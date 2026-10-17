---
type: "Command Reference"
title: "Command Line"
description: "The oxidged command of the oxidged-cli package: its commands, their reading and writing options, their text and JSON outputs, and its exit codes."
tags: [oxidged, cli]
generated: { by: human:maintainer, at: 2026-10-17T14:06:10Z }
---

# Command Line

> Part of the [oxidged documentation](index.md).
> See also: [Reading](reading.md) · [Writing](writing.md) · [Producer Profiles](profiles.md)

The command-line tool is the package `oxidged-cli` (in `cli/`); its
command is `oxidged`, a set of subcommands over the library:

```bash
cargo install oxidged-cli

oxidged info family.ged                            # version, encoding, producer, records
oxidged validate family.ged                        # every deviation, with its line
oxidged write family.ged -g 7.0 -o family70.ged    # conformant GEDCOM 7.0
oxidged write family.ged -o family.gdz --media-dir photos/
oxidged export family.ged -p geneanet -o out.ged   # a file for an importer
oxidged ancestors family.ged @I1@ --generations 4
oxidged subset family.ged --descendants-of @I1@ -o branch.ged
oxidged edit rename family.ged @I1@ @ANN@ | oxidged edit remove - @S9@ -o out.ged
oxidged date "ABT @#DJULIAN@ 1 MAR 1700"
```

`oxidged --help` and `oxidged <command> --help` describe every option;
`oxidged man` prints the manual page.

## Commands

| Command | What it does |
|---|---|
| `info FILE [--mappings]` | The version and its declaration, the format, the encoding found and the one declared, the producer, the records by type, the media files of an archive, the pointers to no record, the deviations and the interpretation rules applied (each one with `--mappings`) |
| `validate FILE` | Every deviation of a GEDCOM or GEDZIP file from the specification of the version it declares, with its line ([Validation](validation.md)); exit code 2 when there is one |
| `write FILE [--stream]` | The file written as conformant GEDCOM, GEDZIP or JSON, converted, re-encoded and repaired as the writing options say; with `--stream`, GEDCOM read and written one record at a time, in memory bounded by the largest record ([Streaming](streaming.md)) |
| `export FILE -p PROFILE` | The file written for an importer ([Producer Profiles](profiles.md)) |
| `detect FILE` | The program that produced a file: its profile, version, score, evidence and hints |
| `profiles [ID]` | The producer and importer profiles; with `ID`, one of them: tier, verification, parent, rules, catalogue rows and documented extension tags of a producer, version, encoding, byte order mark and limits of an importer |
| `show FILE XREF… [--gedcom]` | Records: an individual described (names, sex, birth, death, parents, partners and children), a family's partners and children, any other record as GEDCOM; with `--gedcom`, every one as GEDCOM |
| `search FILE [QUERY] [--surname S] [--given G]` | The individuals one of whose full names contains `QUERY`, a surname `S` and a given name `G` (each case-insensitive; all those given must match) |
| `ancestors FILE XREF [--generations N]` | The ancestors of an individual, generation by generation ([The Dataset](dataset.md#ancestors-and-descendants)) |
| `descendants FILE XREF [--generations N]` | Its descendants |
| `subset FILE [-i XREF]… [--ancestors-of XREF]… [--descendants-of XREF]… [--generations N]` | The part of the file these individuals need, written as `write` writes ([The Dataset](dataset.md#subsets)): the individuals given, those with their ancestors or descendants (up to `N` generations) |
| `edit rename FILE OLD NEW` | A record's identifier renamed, every pointer to it following |
| `edit remove FILE XREF… [--pointers refuse\|detach\|leave-dangling]` | Records removed; a record pointed to is refused by default, its pointers removed (`detach`) or left pointing nowhere |
| `edit redirect FILE FROM TO [--remove]` | Every pointer to `FROM` pointed to `TO`; with `--remove`, the record of `FROM` removed after (a merge) |
| `edit detach FILE XREF` | Every structure pointing to `XREF` removed, with its substructures |
| `edit link FILE HOLDER KIND TARGET`, `edit unlink …` | A pointer of `KIND` from `HOLDER` (a record, or `HEAD` for the header) to `TARGET` added or removed: `child` and `spouse` link a family and an individual both ways (`CHIL` and `FAMC`, `HUSB` or `WIFE` and `FAMS`); `child-of`, `spouse-of`, `husband`, `wife`, `family-child`, `alias`, `association`, `citation`, `note`, `multimedia`, `repository`, `submitter`, `ancestor-interest`, `descendant-interest` and `submission` one side ([Editing](editing.md)) |
| `tree FILE` | The lossless tree of a GEDCOM or GEDZIP file: its structures as read, nothing typed or interpreted |
| `encoding FILE` | The encoding a file's bytes are decoded with, the one its `HEAD.CHAR` declares, its byte order mark, and whether they agree |
| `decode FILE [--encoding E] [-o OUT]` | The file's text decoded to UTF-8, as oxidged reads it |
| `date VALUE [--calendar C]…` | A date payload: how it reads, whether 5.5.1, 7.0 and 7.1 accept it and how each writes it, and each of its dates as a Julian day number, a weekday and in other calendars (`gregorian`, `julian`, `hebrew`, `french-republican`; all by default) ([Dates, Ages and Times](values.md)) |
| `age VALUE` | An age payload: how it reads, whether each version accepts it and how each writes it |
| `completions SHELL` | A completion script for `bash`, `elvish`, `fish`, `powershell` or `zsh` |
| `man [--out-dir DIR]` | The manual page on standard output, or a page per command in `DIR` |

Edits and subsets write their result as `write` writes it; each edit reads
its input from a file or from standard input, so edits chain through
pipes.

## Reading options

Every command that reads a file (but `detect`, `encoding` and `decode`,
which read its bytes) takes these options:

| Option | Effect |
|---|---|
| `FILE` | The file; `-` reads standard input |
| `--from ged\|gdz\|json` | The input's format; by default its extension's (`.ged`, `.gdz` or `.zip`, `.json`), else what its bytes look like (a ZIP archive, a JSON object, else GEDCOM) |
| `--encoding E` | Decode the bytes in this encoding, whatever they declare (any `CHAR` label of [Reading](reading.md#encodings)) |
| `--strict` | Refuse an input with any deviation from its specification (exit code 2, every deviation listed) |
| `--max-size BYTES` | Refuse an input larger than this |
| `--producer ID` | Read the input as this producer wrote it, instead of detecting it |
| `--interpretation none\|lossless\|best-effort` | How a producer's conventions are read (default `lossless`) |

A GEDZIP input is read from its `gedcom.ged`, its media files kept for a
GEDZIP output; a JSON input is a dataset as `write --to json` writes it.

## Writing options

`write`, `subset` and every `edit` take these options:

| Option | Effect |
|---|---|
| `-o, --output FILE` | The file to write; standard output by default or with `-` |
| `--to ged\|gdz\|json` | The output's format; by default the output's extension's, else GEDCOM |
| `-g, --gedcom-version 5.5.1\|7.0\|7.1` | The version written (default: the dataset's), converting 5.5.1 and 7.x into each other ([Conversion](conversion.md)); a GEDZIP archive holds 7.x (7.0 by default for a 5.5.1 dataset) |
| `--output-encoding UTF-8\|UTF-16\|ANSEL\|ASCII` | The encoding of 5.5.1 output (7.x is always UTF-8) |
| `--bom auto\|always\|never` | The byte order mark |
| `--line-ending lf\|crlf\|cr` | The line terminator |
| `--max-line-length N` | The longest 5.5.1 line (1 to 255) |
| `--keep-obsolete` | Writing 7.x from 5.5.1: keep what 7.x drops (`SUBN`, `HEAD.FILE`, escapes) as declared extensions |
| `--credentials` | Write the credentials a file may hold (`_PASS`, an encrypted or server-login `_USER`), left out by default |
| `--no-repair` | Fail with exit code 2, writing nothing, instead of repairing what the version does not permit |
| `--media-dir DIR` | GEDZIP output: add the media files the dataset's relative `FILE` paths name under `DIR` (a warning for each one missing) |

The repairs are listed on standard error (`repair: …`). `export` takes
`-o`, `-g` and the [export options](profiles.md#exports): `--omit-media`,
`--omit-private`, `--omit-change-dates`, `--home XREF`, `--media-root PATH`,
`--credentials` and `--head-source auto|library|dataset`; it lists the
lossy folds (`lossy: …`), the importer limits exceeded (`limit: …`) and
the repairs.

## JSON

With `--json` (before or after the command), the commands that report
print a JSON document on standard output instead of text: `info`,
`validate`, `detect`, `profiles`, `show`, `search`, `ancestors`,
`descendants`, `tree` (the version and the structures), `encoding`, `date`
and `age` (with the value as the library serializes it). The commands that
write a file print their report (repairs, folds, limits) as one JSON
object on standard error.

```bash
oxidged validate family.ged --json
{
  "conformant": false,
  "deviations": [
    {
      "detail": "SEX \"male\": not a value of SEX_VALUE",
      "kind": "EnumValue",
      "line": 11
    }
  ]
}
```

## Exit codes

| Code | When |
|---|---|
| 0 | Success (and for `--help` and `--version`) |
| 1 | An input that cannot be read (a missing file, an archive without `gedcom.ged`, invalid JSON, a missing record or individual), an output that cannot be written, an edit refused |
| 2 | A deviation from the specification: `validate` found one, `--strict` refused the input, `--no-repair` refused a repair |
| 3 | A usage error: an unknown command, option, value, profile, encoding or calendar, a missing argument, options that do not go together |
