---
type: "Feature Reference"
title: "GEDZIP"
description: "Reading and writing GEDZIP archives, a GEDCOM 7.x dataset bundled with its media files, with the gedzip feature."
tags: [oxidged, gedzip, media]
generated: { by: human:maintainer, at: 2026-11-28T09:00:00Z }
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

## Streaming entries

An entry need not be held whole in memory. `open_media_file` (a media file,
found as `read_media_file` finds it) and `open_gedcom` (`gedcom.ged`) give a
`GedzipEntry`, an `io::Read` that decompresses as it is read;
`add_media_file_from` and `write_gedcom_from` write an entry from any
`io::Read` as it is read, and return its size. They name, refuse, compress
and limit entries as the functions that take or give bytes, which are made
of them (media of an already compressed format, JPEG XL among them, are
stored as they are, and the rest is deflated):

```rust
use oxidged::gedzip::{GedzipReader, GedzipWriter};
use oxidged::model::RecordRef;
use oxidged::{Dataset, GedcomStreamParser};
use std::io::{self, BufReader, Cursor, Read};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Write an archive with a media file of 4 MiB, read from a source as it is written.
    let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n");
    let mut writer = GedzipWriter::new(Cursor::new(Vec::new()))?;
    writer.write_dataset(&data)?;
    let mut scan = io::repeat(0x2A).take(4 << 20); // a file, a network stream…
    assert_eq!(writer.add_media_file_from("scans/register%201850.tif", &mut scan)?, 4 << 20);
    let archive = writer.finish()?.into_inner();

    // Copy the media file out, a block at a time, within a size limit.
    let mut reader = GedzipReader::new(Cursor::new(archive))?.max_entry_size(1 << 30);
    let mut entry = reader.open_media_file("scans/register%201850.tif")?;
    assert_eq!(entry.name(), "scans/register 1850.tif");
    assert_eq!(entry.size(), 4 << 20);
    assert_eq!(io::copy(&mut entry, &mut io::sink())?, 4 << 20);
    drop(entry); // It borrows the reader until it is dropped.

    // Read the dataset one record at a time.
    let parser = GedcomStreamParser::new(BufReader::new(reader.open_gedcom()?))?;
    for record in parser {
        if let RecordRef::Individual(person) = record?.record() {
            assert_eq!(person.full_name().as_deref(), Some("Ann Example"));
        }
    }
    Ok(())
}
```

A `GedzipEntry` borrows its reader until it is dropped, gives its `name()`
and the `size()` the archive declares, and checks the bytes against the
entry's CRC-32 at its end. An entry declaring more than `max_entry_size` is
refused when opened (`EntryTooLarge`); one holding more than it declares
fails with an `InvalidData` I/O error once past its declared size; reading
past the limit fails with an I/O error that converts into
`GedzipError::EntryTooLarge` and `GedcomError::Gedzip(EntryTooLarge)`.
`read_dataset` reads `gedcom.ged` through such an entry, as
`build_from_reader` reads, the bytes read becoming the dataset's store.
`write_dataset_with` writes the text into its entry as the `GedcomWriter`
produces it, except under `RepairPolicy::Error`, where it is produced
whole first so that a refusal leaves no `gedcom.ged`. When a source fails
while an entry is written, the error is returned and the entry is left
incomplete: the archive is to be discarded. An entry written holds less
than 4 GiB: writing more fails with an I/O error, the entries being written
without ZIP64 sizes.

---

`GedzipReader` also lists every entry (`file_names`, `contains_file`,
`len`), reads the dataset through a builder (`read_dataset`, with its size
limits and strict mode) or as bytes (`read_gedcom_bytes`) or a reader
(`open_gedcom`), reads media as bytes (`read_media_file`) or readers
(`open_media_file`), and refuses an entry larger than `max_entry_size`,
whatever size it declares. Entry names are decoded as UTF-8, else CP437.
`GedzipWriter` writes a dataset (`write_dataset`, `write_dataset_with` a
given writer) or GEDCOM bytes (`write_gedcom_bytes`) or a reader's
(`write_gedcom_from`), adds media (`add_media_file`, `add_media_files`,
`add_media_file_from`) and `finish`es. `read_gedzip`, `write_gedzip` and
`write_gedzip_with_media` do it in one call. `GedzipError` is
`#[non_exhaustive]`: `Zip`, `MissingGedcom`, `MissingMedia`,
`EntryTooLarge`, `Io`, `Write`, `ReservedName`.
