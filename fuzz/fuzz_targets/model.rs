//! The typed model is total and its output conformant and stable: for any
//! bytes, reading never panics; for every version, the written output
//! follows the version's specification (the crate's validator finds
//! nothing but what the conformance repair documents it leaves: a 5.5.1
//! record over 32K holding no note to move out), reads back with every structure
//! the model types read typed (`ledger::untyped`), and writing what was
//! read back gives the same bytes: write(read(write(x))) == write(x).
#![no_main]

use libfuzzer_sys::fuzz_target;
use oxidged::model::{Dataset, ledger};
use oxidged::{GedcomVersion, GedcomWriter};

#[path = "common/left.rs"]
mod left;

fuzz_target!(|data: &[u8]| {
    let dataset = Dataset::from_bytes(data);
    let _ = format!("{dataset:?}");
    let _ = dataset.to_structures();
    for version in [
        GedcomVersion::V5_5_1,
        GedcomVersion::V7_0,
        GedcomVersion::V7_1,
    ] {
        let writer = GedcomWriter::new().gedcom_version(version);
        let Ok(out) = writer.write_to_string(&dataset) else {
            continue;
        };
        let deviations = left::unexpected(&out, version);
        assert!(deviations.is_empty(), "{version}: {deviations:?}");
        let again = Dataset::parse(out.as_str());
        let untyped = ledger::untyped(&again);
        assert!(untyped.is_empty(), "{version}: {untyped:?}");
        let twice = writer
            .write_to_string(&again)
            .expect("a written dataset writes");
        // Writing is idempotent: what was written, read back and written
        // again, is the same bytes.
        assert!(twice == out, "{version}: not stable\n{out}\n{twice}");
    }
});
