# GEDCOM 5.5.1 and GEDCOM 7.0

This guide explains the key differences between GEDCOM 5.5.1 and GEDCOM 7.0, and how oxidged reads and writes each. (To move code from the `ged_io` crate to oxidged, see [MIGRATION.md](../MIGRATION.md).)

## Key Differences

### 1. Encoding

| Feature | GEDCOM 5.5.1 | GEDCOM 7.0 |
|---------|--------------|------------|
| Character Encoding | Multiple (ANSEL, ASCII, UTF-8, UNICODE) | UTF-8 only |
| `CHAR` tag | Required | Removed |
| BOM | Not specified | Optional UTF-8 BOM recommended |

**Migration Note:** If you're creating GEDCOM 7.0 files, always use UTF-8 encoding. The `CHAR` tag should not be included.

```rust
use oxidged::version::GedcomVersion;

let rules = GedcomVersion::V7_0.rules();
assert!(!rules.has_head_char()); // 7.0 is UTF-8 only: no CHAR
```

### 2. Line Continuation

| Feature | GEDCOM 5.5.1 | GEDCOM 7.0 |
|---------|--------------|------------|
| `CONT` tag | Supported | Supported |
| `CONC` tag | Supported | **Removed** |

**Migration Note:** GEDCOM 7.0 removes the `CONC` tag. Use only `CONT` for multi-line text. The library handles this automatically when writing.

```rust
use oxidged::version::GedcomVersion;

assert!(GedcomVersion::V5_5_1.rules().uses_conc());
assert!(!GedcomVersion::V7_0.rules().uses_conc());
```

### 3. @ Sign Escaping

| Feature | GEDCOM 5.5.1 | GEDCOM 7.0 |
|---------|--------------|------------|
| Escaping Rule | All `@` doubled (`@@`) | Only leading `@` doubled |

**Migration Note:** Reading unescapes `@@` by the rule of the file's
version, and the writer escapes text by the rule of the version it writes:

```rust
use oxidged::model::{Dataset, Note};
use oxidged::{GedcomVersion, GedcomWriter};

let mut data = Dataset::new(GedcomVersion::V5_5_1);
let id = data.store_mut().intern_xref("@I1@").unwrap();
let mut person = oxidged::model::Individual { xref: Some(id), ..Default::default() };
person.notes.push(Note::text("@home and ann@example.com"));
data.add(person).unwrap();

// GEDCOM 5.5.1: every @ doubled
let v551 = GedcomWriter::new().write_to_string(&data).unwrap();
assert!(v551.contains("1 NOTE @@home and ann@@example.com\n"));

// GEDCOM 7.0: only a leading @ doubled
let v70 = GedcomWriter::new().gedcom_version(GedcomVersion::V7_0).write_to_string(&data).unwrap();
assert!(v70.contains("1 NOTE @@home and ann@example.com\n"));
```

### 4. New Record Types

#### Shared Notes (`SNOTE`)

GEDCOM 7.0 introduces `SNOTE` records for notes that can be referenced by multiple structures:

```rust
use oxidged::Dataset;

let gedcom_7 = "\
    0 HEAD\n\
    1 GEDC\n\
    2 VERS 7.0\n\
    0 @N1@ SNOTE This note can be referenced by multiple records.\n\
    1 MIME text/plain\n\
    1 LANG en\n\
    0 TRLR";

let data = Dataset::parse(gedcom_7);
assert_eq!(data.notes().len(), 1);
let note = data.find_note("@N1@").unwrap();
assert!(note.text().to_str().contains("referenced"));
```

A 5.5.1 `NOTE` record is a shared note too, written `NOTE` in 5.5.1 and
`SNOTE` in 7.0.

#### Schema (`SCHMA`)

GEDCOM 7.0 formalizes extension tags via the `SCHMA` structure:

```rust
use oxidged::Dataset;

let gedcom_7 = "\
    0 HEAD\n\
    1 GEDC\n\
    2 VERS 7.0\n\
    1 SCHMA\n\
    2 TAG _CUSTOM http://example.com/gedcom-extensions/custom\n\
    0 TRLR";

let data = Dataset::parse(gedcom_7);
let schema = data.header().unwrap().schema.as_ref().unwrap();
assert_eq!(
    schema.tags[0].to_str(&data),
    "_CUSTOM http://example.com/gedcom-extensions/custom"
);
```

### 5. New Substructures

#### Sort Date (`SDATE`)

A date used for sorting when the actual date is vague:

```text
1 BIRT
2 DATE BEF 1820
2 SDATE 1818
```

#### Non-Events (`NO`)

Asserts that an event did NOT occur (distinct from unknown):

```text
0 @I1@ INDI
1 NO MARR
2 NOTE Never married per family records.
```

#### Phrases (`PHRASE`)

Free-text representation of dates:

```text
1 BIRT
2 DATE 15 MAR 1820
3 PHRASE The Ides of March, in the year 1820
```

#### Creation Date (`CREA`)

Records when a structure was first created (vs `CHAN` for last modified):

```text
0 @I1@ INDI
1 CREA
2 DATE 15 MAR 2020
```

#### Image Cropping (`CROP`)

Defines a region of an image to display:

```text
1 FILE photo.jpg
2 CROP
3 TOP 10
3 LEFT 15
3 HEIGHT 50
3 WIDTH 40
```

### 6. LDS Ordinances

#### New: `INIL` (Initiatory)

GEDCOM 7.0 adds the `INIL` tag for LDS initiatory ordinances:

```rust
use oxidged::Dataset;
use oxidged::model::OrdinanceKind;

let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 INIL\n2 DATE 15 MAR 1990\n2 TEMP SLAKE\n0 TRLR\n");
let ordinance = data.individuals().first().unwrap().ordinances().first().unwrap();
assert_eq!(ordinance.kind(), OrdinanceKind::Initiatory);
assert_eq!(ordinance.kind().tag(), "INIL");
```

Written as 5.5.1, which has no `INIL`, it is kept as the extension `_INIL`.

**Available LDS Ordinance Types:**

| Tag | Type | Records | GEDCOM 7.0 Only |
|-----|------|---------|-----------------|
| `BAPL` | Baptism | Individual | No |
| `CONL` | Confirmation | Individual | No |
| `INIL` | Initiatory | Individual | **Yes** |
| `ENDL` | Endowment | Individual | No |
| `SLGC` | Sealing to Parents | Individual | No |
| `SLGS` | Sealing to Spouse | Family | No |

### 7. Removed Structures

| Structure | Status in 7.0 |
|-----------|--------------|
| `SUBN` (Submission record) | Removed |
| `CHAR` (Character encoding) | Removed |
| `CONC` (Concatenation) | Removed |

## Version Detection

The library automatically detects the GEDCOM version:

```rust
use oxidged::version::detect_version;

let content = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n";
let version = detect_version(content);

// 7.x declarations read as 7.0 or 7.1; anything else follows 5.5.1.
if version.is_v7() {
    println!("GEDCOM {version} file");
} else {
    println!("GEDCOM 5.5.1 file");
}

// The writing rules of the version
let rules = version.rules();
println!("longest line: {:?}", rules.max_line_length());
```

## Checking Version Programmatically

```rust
use oxidged::Dataset;

let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @N1@ SNOTE Shared\n0 TRLR\n");

if data.version().is_v7() {
    for note in data.notes() {
        println!("Shared note: {}", note.text());
    }
}
// The declaration as written (`7.0`, `7.0.14`, `5.5`, …)
assert_eq!(data.declared_version(), Some("7.0"));
```

## Writing Version-Specific Files

```rust
use oxidged::{Dataset, GedcomVersion, GedcomWriter};

let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n");

// Write in the dataset's own version (`Dataset::version`): 5.5.1 here
let output_551 = GedcomWriter::new().write_to_string(&data).unwrap();
assert!(output_551.contains("2 VERS 5.5.1\n"));

// Write as GEDCOM 7.0
let output_70 = GedcomWriter::new()
    .gedcom_version(GedcomVersion::V7_0)
    .write_to_string(&data)
    .unwrap();
assert!(output_70.contains("2 VERS 7.0\n"));
```

## Best Practices for Migration

1. **Version Detection First**: Always detect the version before processing
2. **Graceful Degradation**: Handle missing 7.0 features when reading 5.5.1 files
3. **UTF-8 Always**: Use UTF-8 encoding for all new files
4. **Test Round-Trips**: Verify data integrity when converting between versions
5. **Handle Shared Notes**: If converting to 7.0, consider extracting common notes to `SNOTE` records
6. **Document Extensions**: Use `SCHMA` to document any custom tags in 7.0 files

## Common Migration Patterns

### Converting Inline Notes to Shared Notes

```rust
use oxidged::model::{Note, SharedNote};
use oxidged::{Dataset, GedcomVersion};

let mut data = Dataset::new(GedcomVersion::V7_0);
// Create a shared note from common text, and point to it.
let id = data.store_mut().intern_xref("@N1@").unwrap();
data.add(SharedNote {
    xref: Some(id),
    text: "Common note text used in multiple places".into(),
    ..Default::default()
})
.unwrap();
let mut person = oxidged::model::Individual::default();
person.notes.push(Note::shared(id));
data.add(person).unwrap(); // checked: the note it points to is there
assert_eq!(data.references(id), 1);
assert!(data.dangling_references().is_empty());
```

### Handling CONC in 7.0

Reading joins `CONC` continuations into the text; writing GEDCOM 7.0 never
splits a payload, so 7.0 output has no `CONC` at all:

```rust
use oxidged::{Dataset, GedcomVersion, GedcomWriter};

let text = format!("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NOTE {}\n2 CONC {}\n0 TRLR\n", "a".repeat(200), "b".repeat(200));
let data = Dataset::parse(text);
let writer = GedcomWriter::new().gedcom_version(GedcomVersion::V7_0);
// Long text stays on one line; line breaks become CONT lines.
let out = writer.write_to_string(&data).unwrap();
assert!(!out.contains("CONC"));
```

## Additional Resources

- [GEDCOM 7.0 Specification](https://gedcom.io/specifications/FamilySearchGEDCOMv7.html)
- [GEDCOM 5.5.1 Specification](https://gedcom.io/specifications/ged551.pdf)
- [oxidged Documentation](https://docs.rs/oxidged)

---

*This guide is part of the oxidged documentation.*
