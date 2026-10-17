//! The only module of the suite that touches oxidged's API.
//!
//! Every test reads and writes through these functions and asserts on
//! behaviour: what the model keeps (searched as text), what the writer emits
//! and what reads back. When the crate's types or entry points change, this
//! file is the one to update; the cases, tables and checks stay as they are.

use oxidged::model::{Dataset, RecordRef};
use oxidged::{GedcomBuilder, GedcomVersion, GedcomWriter, LineEnding};
use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

/// A parsed dataset, opaque to the tests.
pub type Model = Dataset;

thread_local! {
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f`, turning a panic into `Err("PANIC: …")` without printing it.
/// Panics outside `guard` (failing assertions) still print as usual.
pub fn guard<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let default = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if !QUIET.with(Cell::get) {
                default(info);
            }
        }));
    });
    QUIET.with(|q| q.set(true));
    let r = panic::catch_unwind(AssertUnwindSafe(f));
    QUIET.with(|q| q.set(false));
    match r {
        Ok(r) => r,
        Err(e) => {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_default();
            Err(format!("PANIC: {msg}"))
        }
    }
}

/// The reader of the suite: lenient, and without producer interpretation,
/// which rewrites a producer's conventions as the standard says them (its
/// own tests are `tests/profiles.rs`): the suite checks what the reader
/// keeps as written.
fn lenient() -> GedcomBuilder {
    GedcomBuilder::new().interpretation(oxidged::profile::Interpretation::None)
}

/// Reads bytes with the default (lenient) configuration, interpreting
/// nothing.
pub fn read(bytes: &[u8]) -> Result<Model, String> {
    guard(|| lenient().build_from_bytes(bytes).map_err(|e| e.to_string()))
}

/// Reads bytes and reports dangling pointers as an error (the reference
/// check the documentation describes for application backends).
pub fn read_checking_references(bytes: &[u8]) -> Result<Model, String> {
    guard(|| {
        let data = lenient()
            .build_from_bytes(bytes)
            .map_err(|e| e.to_string())?;
        match data.dangling_references().first() {
            Some(d) => Err(format!("dangling pointer {}", data.store().xref(d.pointer))),
            None => Ok(data),
        }
    })
}

/// The structures the model types but read untyped (it found them not to
/// fit their type): none for a conformant input.
pub fn untyped(m: &Model) -> Vec<String> {
    oxidged::model::ledger::untyped(m)
}

/// Target of a write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The version the data declares (5.5.1 when it declares none).
    Same,
    V551,
    V70,
    V71,
}

/// Writes the model with the default configuration for `target`.
pub fn write(m: &Model, target: Target) -> Result<String, String> {
    write_with(m, target, None)
}

fn writer(target: Target, eol: Option<&str>) -> GedcomWriter {
    let mut w = GedcomWriter::new();
    w = match target {
        Target::Same => w,
        Target::V551 => w.gedcom_version(GedcomVersion::V5_5_1),
        Target::V70 => w.gedcom_version(GedcomVersion::V7_0),
        Target::V71 => w.gedcom_version(GedcomVersion::V7_1),
    };
    if let Some(eol) = eol {
        w = w.line_ending(match eol {
            "\r\n" => LineEnding::CrLf,
            "\r" => LineEnding::Cr,
            _ => LineEnding::Lf,
        });
    }
    w
}

/// Writes with an explicit line terminator.
pub fn write_with(m: &Model, target: Target, eol: Option<&str>) -> Result<String, String> {
    guard(|| {
        writer(target, eol)
            .write_to_string(m)
            .map_err(|e| e.to_string())
    })
}

/// Writes the model for `target`, keeping the 5.5.1 structures 7.x drops
/// as declared extensions when converting (`keep_obsolete`).
pub fn write_keeping(m: &Model, target: Target) -> Result<String, String> {
    guard(|| {
        writer(target, None)
            .conversion(oxidged::convert::ConversionOptions::default().keep_obsolete(true))
            .write_to_string(m)
            .map_err(|e| e.to_string())
    })
}

/// The model as an archive holds it: GEDCOM 7.x, a 5.5.1 model converted
/// to 7.0.
#[cfg(feature = "gedzip")]
pub fn as_archived(m: &Model) -> Model {
    if m.version().is_v7() {
        m.clone()
    } else {
        oxidged::convert::convert_dataset(
            m,
            GedcomVersion::V7_0,
            &oxidged::convert::ConversionOptions::default(),
        )
    }
}

/// Converts the lossless tree of `text` (not the typed model) to `target`
/// and writes it.
pub fn write_tree(text: &str, target: Target) -> Result<String, String> {
    guard(|| {
        let tree = oxidged::tree::Tree::parse(text);
        let mut out = Vec::new();
        writer(target, None)
            .bom(oxidged::Bom::Never)
            .write_tree(&mut out, &tree)
            .map_err(|e| e.to_string())?;
        String::from_utf8(out).map_err(|e| e.to_string())
    })
}

/// Writes the model as GEDCOM 5.5.1 bytes in ANSEL (`CHAR ANSEL`).
pub fn write_ansel(m: &Model) -> Result<Vec<u8>, String> {
    guard(|| {
        let mut bytes = Vec::new();
        GedcomWriter::new()
            .gedcom_version(GedcomVersion::V5_5_1)
            .output_encoding(oxidged::OutputEncoding::Ansel)
            .write(&mut bytes, m)
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    })
}

/// The whole model as text, for "the model keeps this value" checks: the
/// structures it writes hold everything it keeps.
pub fn dump(m: &Model) -> String {
    format!("{:?}", m.to_structures())
}

/// The records of the model as text, header excluded.
pub fn dump_records(m: &Model) -> String {
    let mut records = m.to_structures();
    records.retain(|r| r.tag != "HEAD");
    format!("{records:?}")
}

/// True when `needle` appears in the model. A needle is matched against the
/// model's text form, where a newline reads `\n` and a quote `\"`.
pub fn model_contains(m: &Model, needle: &str) -> bool {
    let d = dump(m);
    let quoted = format!("{needle:?}");
    d.contains(needle) || d.contains(&quoted[1..quoted.len() - 1])
}

/// Number of records of every kind, header excluded.
pub fn record_count(m: &Model) -> usize {
    m.records()
        .filter(|r| match r {
            RecordRef::Header(_) => false,
            RecordRef::Other(n) => !["HEAD", "TRLR"].contains(&n.tag()),
            _ => true,
        })
        .count()
}

/// Reads with the streaming parser, one record at a time. Returns the text
/// form of each record.
pub fn read_streaming(bytes: &[u8]) -> Result<Vec<String>, String> {
    guard(|| {
        let parser = oxidged::GedcomStreamParser::new(std::io::Cursor::new(bytes))
            .map_err(|e| e.to_string())?;
        parser
            .map(|r| {
                r.map(|rec| {
                    let s = rec.record().to_structure_in(rec.version());
                    format!("{s:?}")
                })
                .map_err(|e| e.to_string())
            })
            .collect()
    })
}

/// Looks an xref up through the indexed (O(1)) view; returns the record's
/// text form.
pub fn indexed_find(m: Model, xrefs: &[&str]) -> Vec<Option<String>> {
    let idx = oxidged::IndexedDataset::new(m);
    xrefs
        .iter()
        .map(|x| idx.find(*x).map(|r| format!("{r:?}")))
        .collect()
}

/// Serialises the model to JSON and back; returns the JSON text and whether
/// the dataset read back holds the same structures.
#[cfg(feature = "serde")]
pub fn json_round_trip(m: &Model) -> Result<(String, bool), String> {
    guard(|| {
        let json = serde_json::to_string(m).map_err(|e| e.to_string())?;
        let back: Model = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        Ok((json, back.to_structures() == m.to_structures()))
    })
}

/// Writes a GEDZIP archive with media files, reads it back and returns the
/// model and the media names found in the archive.
#[cfg(feature = "gedzip")]
pub fn gedzip_round_trip(
    m: &Model,
    media: &[(&str, &[u8])],
) -> Result<(Model, Vec<String>), String> {
    guard(|| {
        let files: std::collections::HashMap<String, Vec<u8>> = media
            .iter()
            .map(|(n, b)| ((*n).to_string(), b.to_vec()))
            .collect();
        let bytes =
            oxidged::gedzip::write_gedzip_with_media(m, &files).map_err(|e| e.to_string())?;
        let mut reader = oxidged::gedzip::GedzipReader::new(std::io::Cursor::new(bytes.as_slice()))
            .map_err(|e| e.to_string())?;
        let names = reader
            .media_files()
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let back = reader.read_dataset(&lenient()).map_err(|e| e.to_string())?;
        Ok((back, names))
    })
}

/// Reads a GEDZIP archive.
#[cfg(feature = "gedzip")]
pub fn read_gedzip(bytes: &[u8]) -> Result<Model, String> {
    guard(|| oxidged::gedzip::read_gedzip(bytes).map_err(|e| e.to_string()))
}

/// The crate's validator on a written or input stream: each deviation as
/// (rule, line, detail), the rule being the `DeviationKind` name.
pub fn validate(text: &str) -> Vec<(String, u32, String)> {
    oxidged::spec::validate_text(text)
        .into_iter()
        .map(|d| (format!("{:?}", d.kind), d.line, d.detail.to_string()))
        .collect()
}
