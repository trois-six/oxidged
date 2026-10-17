---
type: "Feature Reference"
title: "GEDZIP"
description: "Reading and writing GEDZIP archives, a GEDCOM 7.x dataset bundled with its media files, with the gedzip feature."
tags: [oxidged, gedzip, media]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
---

# GEDZIP

> Part of the [oxidged documentation](index.md).
> See also: [Reading](reading.md)

```rust,no_run
use oxidged::gedzip::{write_gedzip_with_media, GedzipReader};
use oxidged::GedcomBuilder;
use std::collections::HashMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read the dataset of an archive through the builder (limits, strict mode).
    let data = GedcomBuilder::new().build_from_gedzip(std::fs::File::open("family.gdz")?)?;

    // Extract its media files.
    let mut reader = GedzipReader::new(std::fs::File::open("family.gdz")?)?.max_entry_size(1 << 30);
    for name in reader.media_files().iter().map(|n| n.to_string()).collect::<Vec<_>>() {
        let bytes = reader.read_media_file(&name)?;
        println!("{name}: {} bytes", bytes.len());
    }

    // Write a new archive with media.
    let mut media = HashMap::new();
    media.insert("photos/portrait.jpg".to_string(), std::fs::read("portrait.jpg")?);
    std::fs::write("new.gdz", write_gedzip_with_media(&data, &media)?)?;
    Ok(())
}
```

An archive holds GEDCOM 7.x (§4): a 5.5.1 dataset is converted to 7.0 and
written in UTF-8 with a byte order mark. `add_media_file` refuses the names
GEDZIP reserves (`gedcom.ged`, `META-INF/…`, `MANIFEST.MF`); `media_files`
lists files, not directories; `find_entry` matches a `FILE` reference to its
entry by its exact name first, then decoded (`%20`), then leniently.

---

`GedzipReader` also lists every entry (`file_names`, `contains_file`,
`len`), reads the dataset through a builder (`read_dataset`, with its size
limits and strict mode) or as bytes (`read_gedcom_bytes`), and refuses an
entry larger than `max_entry_size` while reading it, whatever size it
declares. Entry names are decoded as UTF-8, else CP437. `GedzipWriter`
writes a dataset (`write_dataset`, `write_dataset_with` a given writer) or
GEDCOM bytes, adds media (`add_media_file`, `add_media_files`) and
`finish`es. `read_gedzip`, `write_gedzip` and `write_gedzip_with_media` do
it in one call. `GedzipError` is `#[non_exhaustive]`: `Zip`,
`MissingGedcom`, `MissingMedia`, `EntryTooLarge`, `Io`, `Write`,
`ReservedName`.
