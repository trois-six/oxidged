# oxidged-cli

The `oxidged` command: inspect, validate, repair, convert and export
[GEDCOM](https://en.wikipedia.org/wiki/GEDCOM) 5.5.1, 7.0 and 7.1 files,
built on the [`oxidged`](https://crates.io/crates/oxidged) library.

```bash
cargo install oxidged-cli
```

```bash
# A summary: version, records, pointers to no record
oxidged family.ged

# One individual, or every individual whose surname contains EXAMPLE
oxidged --individual @I1@ family.ged
oxidged --individual-lastname example family.ged

# Check a file against the specification of the version it declares
oxidged --validate --validation-level strict family.ged

# Rewrite any file as conformant GEDCOM 7.0 (repairs on standard error),
# converting a 5.5.1 file; --keep-obsolete keeps what 7.0 drops as extensions
oxidged --write 7.0 family.ged > family70.ged

# Repair a file in its own version; re-encode a 5.5.1 file
oxidged --sanitize family.ged > clean.ged
oxidged --sanitize --convert-encoding ANSEL family.ged > family-ansel.ged

# Which program wrote it; a file for an importer (lossy folds on standard error)
oxidged --detect family.ged
oxidged --profile geneanet family.ged > for-geneanet.ged
oxidged --list-profiles
```

`oxidged --help` lists every option; the
[command-line documentation](https://github.com/trois-six/oxidged/blob/main/docs/cli.md)
describes each. Exit codes: 0 success, 1 I/O error,
2 validation errors in strict mode, 3 usage error.

Licensed under the [MIT License](license.md).
