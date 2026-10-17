/*!
Reads and writes GEDCOM, the genealogical data format, in versions 5.5.1,
7.0 and 7.1.

- **Reading never fails and loses nothing.** Bytes of any encoding are
  decoded ([`encoding`]), any line is read by fixed, lenient rules
  ([`tree`]), and every record is typed as far as it fits the model
  ([`model`]): what a type has no field for is kept, where it was. An
  opt-in strict mode refuses input that does not follow its specification
  ([`GedcomBuilder::strict`]).
- **Writing is conformant.** [`GedcomWriter`] writes a dataset in its
  version or another, converting it between 5.5.1 and 7.x as the migration
  guide says ([`convert`]), repairing what the target does not permit
  ([`spec::conform`]) and emitting every line by the target's line rules.
- **Producer profiles.** [`profile`] recognises the program that wrote
  a file, reads its `CHAR` and its extensions as it meant them (without
  losing anything), and writes files shaped for an importer, conformant,
  reporting every lossy fold.
- **Streaming and indexing.** [`GedcomStreamParser`] reads one record at a
  time in bounded memory; [`IndexedDataset`] finds records and families in
  constant time; `oxidged::gedzip` reads and writes GEDZIP archives (feature
  `gedzip`).
- **The specifications as data.** [`spec`] validates a file against the
  tables of its version; [`value`] reads and converts dates, ages and
  times.

```rust
use oxidged::{Dataset, GedcomVersion, GedcomWriter};

let data = Dataset::parse(
    "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 BIRT\n\
     2 DATE ABT @#DJULIAN@ 1700\n0 TRLR\n",
);
let ann = data.individuals().first().unwrap();
assert_eq!(ann.full_name().as_deref(), Some("Ann Example"));
let birth = ann.birth().and_then(|b| b.date()).unwrap();
assert_eq!(birth.value(), "ABT @#DJULIAN@ 1700");

let out = GedcomWriter::new()
    .gedcom_version(GedcomVersion::V7_0)
    .write_to_string(&data)
    .unwrap();
assert!(out.contains("2 DATE ABT JULIAN 1700\n"));
```

# Features

- `serde`: `Serialize` and `Deserialize` for [`Dataset`] (its texts
  and identifiers as strings), [`StreamedRecord`] (`Serialize`), the
  values of [`value`] and the structures of [`tree`], in self-describing
  formats such as JSON.
- `gedzip`: GEDZIP archives (`oxidged::gedzip`).
- `calendar`: conversions between calendars and day numbers
  ([`value::CalendarDate`]).
*/
#![cfg_attr(not(test), deny(clippy::cargo, clippy::pedantic, clippy::panic))]
#![deny(clippy::all)]
#![deny(missing_docs)]

pub mod builder;
pub mod convert;
pub mod encoding;
pub mod error;
#[cfg(feature = "gedzip")]
pub mod gedzip;
pub mod indexed;
pub mod model;
pub mod profile;
pub mod spec;
pub mod stream;
pub mod tree;
pub mod value;
pub mod version;
pub mod writer;

pub use builder::{GedcomBuilder, ReadReport};
pub use encoding::GedcomEncoding;
pub use error::GedcomError;
pub use indexed::IndexedDataset;
pub use model::Dataset;
pub use stream::{GedcomStreamParser, StreamedRecord, Visitor};
pub use version::{GedcomVersion, VersionRules};
/// The Rust examples of the documentation in `docs/`, compiled and run as
/// documentation tests (some need the `serde` and `gedzip` features).
#[cfg(all(doctest, feature = "serde", feature = "gedzip"))]
mod docs {
    #[doc = include_str!("../docs/quickstart.md")]
    pub struct Quickstart;
    #[doc = include_str!("../docs/reading.md")]
    pub struct Reading;
    #[doc = include_str!("../docs/dataset.md")]
    pub struct Dataset;
    #[doc = include_str!("../docs/editing.md")]
    pub struct Editing;
    #[doc = include_str!("../docs/writing.md")]
    pub struct Writing;
    #[doc = include_str!("../docs/conversion.md")]
    pub struct Conversion;
    #[doc = include_str!("../docs/streaming.md")]
    pub struct Streaming;
    #[doc = include_str!("../docs/gedzip.md")]
    pub struct Gedzip;
    #[doc = include_str!("../docs/validation.md")]
    pub struct Validation;
    #[doc = include_str!("../docs/values.md")]
    pub struct Values;
    #[doc = include_str!("../docs/profiles.md")]
    pub struct Profiles;
    #[doc = include_str!("../docs/gedcom-7.md")]
    pub struct GedcomSeven;
}

pub use writer::{
    Bom, Credentials, GedcomStreamWriter, GedcomWriter, LineEnding, OutputEncoding, Repair,
    RepairKind, RepairPolicy, Unencodable, WriteError, WriteReport, WriterConfig,
};
