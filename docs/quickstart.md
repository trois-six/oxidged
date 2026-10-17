---
type: "Quickstart Guide"
title: "oxidged Quickstart"
description: "Installing oxidged and its optional features, then reading and writing a first file."
tags: [oxidged, quickstart, installation]
generated: { by: human:maintainer, at: 2026-10-17T14:44:00Z }
---

# oxidged Quickstart

> Part of the [oxidged documentation](index.md).
> See also: [Reading](reading.md) · [Writing](writing.md)

## Installation

```bash
cargo add oxidged
```

or, in `Cargo.toml`:

```toml
[dependencies]
oxidged = "0.3"
```

The minimum supported Rust version is 1.88.

## Optional features

| Feature | Adds | Dependencies |
|---------|------|--------------|
| `serde` | `Serialize` and `Deserialize` for the dataset (JSON and other self-describing formats): [The Dataset](dataset.md#json) | `serde` |
| `gedzip` | GEDZIP archives (`.gdz`): read, write, media files (`oxidged::gedzip`): [GEDZIP](gedzip.md) | `zip` (deflate only) |
| `calendar` | Calendar arithmetic: conversions between calendars, day numbers, weekdays: [Dates, Ages and Times](values.md) | `calendrical_calculations`, `calendrier` |

```bash
cargo add oxidged --features serde,gedzip,calendar
```

None is enabled by default; everything else (reading, writing, conversion,
validation, streaming, editing, encodings, producer profiles) is always
built.

## Read a file

```rust,no_run
use oxidged::GedcomBuilder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Any encoding: the bytes are decoded by what they hold and their CHAR.
    let data = GedcomBuilder::new().build_from_reader(std::fs::File::open("family.ged")?)?;

    println!("GEDCOM {}", data.version());
    for person in data.individuals() {
        let name = person.full_name().unwrap_or_default();
        let born = person.birth().and_then(|b| b.date()).map(|d| d.value().to_string());
        println!("{name} {}", born.unwrap_or_default());
    }
    Ok(())
}
```

A dataset keeps the text it was read from once, and every structure as a
row of the table of its type, pointing into that text. Its accessors give
*views* of the rows: `Copy` handles with a method per field, which give a
substructure as a view, a list as `Items`, a text as a `TextRef` that reads
and compares without the store, a pointer as an `XrefId`
(`data.store().xref(id)` is its text). `view.into_owned()` copies a
structure out as its owned type (`Individual`, `Event`, …), which builds and
edits records.

## Write a file

```rust,no_run
use oxidged::{Dataset, GedcomVersion, GedcomWriter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = Dataset::from_bytes(std::fs::read("input.ged")?);

    // In the dataset's own version (`Dataset::version`)…
    std::fs::write("output.ged", GedcomWriter::new().write_to_string(&data)?)?;

    // …or as GEDCOM 7.0, straight into a file, with the repairs it made.
    let file = std::io::BufWriter::new(std::fs::File::create("output70.ged")?);
    let report = GedcomWriter::new()
        .gedcom_version(GedcomVersion::V7_0)
        .write(file, &data)?;
    for repair in &report.repairs {
        println!("{repair}");
    }
    Ok(())
}
```

---

## Where to go next

- [Reading](reading.md): every input, the builder's options, strict mode and
  the read report.
- [The Dataset](dataset.md) and [Editing](editing.md): reading and changing
  the records.
- [Writing](writing.md) and [Conversion](conversion.md): conformant output in
  any version.
- [Command Line](cli.md): the same from a shell.
