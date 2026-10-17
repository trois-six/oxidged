---
type: "Command Reference"
title: "Command Line"
description: "The oxidged command of the oxidged-cli package: its options, outputs and exit codes."
tags: [oxidged, cli]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
---

# Command Line

> Part of the [oxidged documentation](index.md).
> See also: [Producer Profiles](profiles.md) · [Validation](validation.md)

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

## Options

| Option | Effect |
|---|---|
| `<file.ged>` | A summary: the version, the records by type, the pointers to no record |
| `--individual <XREF>` | One individual: names, sex, birth, death, parents, partners, children |
| `--individual-lastname <NAME>`, `--individual-firstname <NAME>` | The individuals whose surname or given names contain the text (case-insensitive; both: those matching both) |
| `--validate [--validation-level strict\|lenient]` | Every deviation from the specification of the declared version, with its line; errors in strict mode (exit code 2), warnings otherwise |
| `--write <5.5.1\|7.0\|7.1> [--keep-obsolete]` | The file as conformant GEDCOM of that version on standard output, converted between 5.5.1 and 7.x, the repairs on standard error |
| `--sanitize` | The same in the file's own version |
| `--convert-encoding <UTF-8\|UTF-16\|ANSEL\|ASCII>` | With `--write` or `--sanitize` (implied): the 5.5.1 output encoding |
| `--profile <NAME>` | The file exported for an importer, the folds, limits and repairs on standard error |
| `--detect` | The program that produced the file, with the evidence and hints |
| `--list-profiles` | The import and export profiles |
| `--interpretation <none\|lossless\|best-effort>` | How a producer's conventions are read |
| `-h`, `--help` | The help |

Any encoding is read. Written files leave out the credentials a file may
hold (`_PASS`, an encrypted or server-login `_USER`).
