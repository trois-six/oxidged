---
type: "Feature Reference"
title: "Streaming"
description: "Reading and writing one record at a time in bounded memory: the stream parser, the visitor and the stream writer."
tags: [oxidged, streaming, visitor]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
---

# Streaming

> Part of the [oxidged documentation](index.md).
> See also: [Reading](reading.md) · [Writing](writing.md)

```rust,no_run
use oxidged::model::RecordRef;
use oxidged::{Dataset, GedcomStreamParser};

fn main() -> Result<(), oxidged::GedcomError> {
    let file = std::io::BufReader::new(std::fs::File::open("huge.ged")?);
    let mut kept = Dataset::default();
    for record in GedcomStreamParser::new(file)? {
        let record = record?;
        if let RecordRef::Individual(person) = record.record() {
            println!("{}", person.full_name().unwrap_or_default());
        }
        // Keep the records of interest, without reading the whole file.
        if matches!(record.record(), RecordRef::Header(_) | RecordRef::Family(_)) {
            kept.extend([record]);
        }
    }
    Ok(())
}
```

The streaming parser reads any encoding and any line terminator the
in-memory reader reads, with the same rules and types, decoding on the fly:
memory stays bounded by the largest record. Each record comes as a dataset
of its own, which keeps the record's text and rows; `record.record()` is its
view. `parser.nodes()` yields the records as lossless
structures instead; for a whole file as structures, nothing interpreted,
use `oxidged::tree::Tree` (in memory) or `oxidged::tree::TreeReader`
(streaming).

`GedcomStreamWriter` writes the other way: a header, then each record as
it is given (from a `GedcomStreamParser`, or a dataset), repaired and
converted for the target version as `GedcomWriter` does, in memory bounded
by one record and the identifiers written:

```rust,no_run
use oxidged::model::{Header, RecordRef};
use oxidged::{GedcomStreamParser, GedcomStreamWriter, GedcomVersion, WriterConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = std::io::BufReader::new(std::fs::File::open("huge.ged")?);
    let output = std::io::BufWriter::new(std::fs::File::create("huge-7.ged")?);
    let mut records = GedcomStreamParser::new(input)?;
    let head = records.next().ok_or("empty file")??;
    let default = Header::default();
    let header = match head.record() {
        RecordRef::Header(h) => h.header(),
        _ => &default,
    };
    let config = WriterConfig { version: Some(GedcomVersion::V7_0), ..WriterConfig::default() };
    let mut writer = GedcomStreamWriter::new(output, config, header, &head)?;
    for record in records {
        let record = record?;
        writer.write_record(record.record())?;
    }
    let (_, report) = writer.finish()?;
    eprintln!("{} repairs", report.repairs.len());
    Ok(())
}
```

Knowing one record at a time, it writes what `GedcomWriter` writes of the
same records but where the records to come would change it (a later
record named as an earlier one, the records a conversion adds, the
extension tags a 7.x header declares); its documentation lists each case.

A `Visitor` receives the records of a stream by their type and says
whether to go on (`ControlFlow`); `GedcomStreamParser::visit` stops at the
first break, leaving the rest unread:

```rust
use std::ops::ControlFlow;
use oxidged::model::{Individual, View};
use oxidged::stream::{GedcomStreamParser, StreamedRecord, Visitor};

struct FirstNamed;

impl Visitor for FirstNamed {
    type Break = String;
    fn individual(&mut self, i: View<'_, Individual>, _: &StreamedRecord) -> ControlFlow<String> {
        i.full_name().map_or(ControlFlow::Continue(()), ControlFlow::Break)
    }
}

let file: &[u8] = b"0 HEAD\n0 @I1@ INDI\n0 @I2@ INDI\n1 NAME Ann /Example/\n0 TRLR\n";
let mut parser = GedcomStreamParser::new(file).unwrap();
assert_eq!(parser.visit(&mut FirstNamed).unwrap(), ControlFlow::Break("Ann Example".into()));
```

---

A `StreamedRecord` gives its `record()`, its `dataset()`, its `line()` and
`version()`, and `into_parts()`; a `Dataset` collects and extends from
streamed records (`FromIterator`, `Extend`). The parser also gives the
`encoding()`, `version()` and `declared_version()` of the stream.
