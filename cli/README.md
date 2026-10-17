# oxidged-cli

The `oxidged` command: inspect, validate, repair, convert, export, edit and
extract
[GEDCOM](https://en.wikipedia.org/wiki/GEDCOM) 5.5.1, 7.0 and 7.1 files,
built on the [`oxidged`](https://crates.io/crates/oxidged) library.

```bash
cargo install oxidged-cli
```

```bash
oxidged info family.ged                            # version, encoding, producer, records
oxidged validate family.ged                        # every deviation, with its line
oxidged write family.ged -g 7.0 -o family70.ged    # conformant GEDCOM 7.0
oxidged write family.ged -o family.gdz --media-dir photos/
oxidged export family.ged -p geneanet -o out.ged   # a file for an importer
oxidged ancestors family.ged @I1@ --generations 4
oxidged subset family.ged --descendants-of @I1@ -o branch.ged
oxidged edit rename family.ged @I1@ @ANN@ -o renamed.ged
oxidged --json info family.ged                     # any report as JSON
```

`oxidged --help` and `oxidged <command> --help` describe every option, and
the
[command-line documentation](https://github.com/trois-six/oxidged/blob/main/docs/cli.md)
every command. Exit codes: 0 success, 1 an input, output or edit error, 2 a
deviation from the specification, 3 a usage error.

Licensed under the [MIT License](license.md).
