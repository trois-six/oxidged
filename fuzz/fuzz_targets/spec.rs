//! The validator and the conformance repair are total and agree: for any
//! bytes, validation never panics, and for every version the repaired tree
//! validates (but for 5.5.1 records over 32K that hold no note to move), a
//! second repair changes nothing, and the written output needs no repair
//! and is written again as it was: write(read(write(x))) == write(x).
#![no_main]

use libfuzzer_sys::fuzz_target;
use oxidged::spec::{DeviationKind, conform, validate, validate_bytes};
use oxidged::tree::Tree;
use oxidged::{Bom, GedcomVersion, GedcomWriter};

fuzz_target!(|data: &[u8]| {
    let _ = validate_bytes(data);
    let records = Tree::from_bytes(data).to_structures();
    for version in [
        GedcomVersion::V5_5_1,
        GedcomVersion::V7_0,
        GedcomVersion::V7_1,
    ] {
        let mut t = records.clone();
        let _ = validate(&t, version);
        conform(&mut t, version);
        let left: Vec<_> = validate(&t, version)
            .into_iter()
            .filter(|d| d.kind != DeviationKind::RecordSize)
            .collect();
        assert!(left.is_empty(), "{version}: {left:?}");
        assert!(conform(&mut t, version).is_empty(), "{version}");
        // What the writer writes, read back, needs no repair, and is
        // written again as it was: write(read(write(x))) == write(x).
        let write = |records: &[oxidged::tree::Structure]| {
            let mut out = Vec::new();
            GedcomWriter::new()
                .gedcom_version(version)
                .bom(Bom::Never)
                .write_structures(&mut out, records)
                .expect("repairs, never fails");
            String::from_utf8(out).expect("UTF-8")
        };
        let out = write(&records);
        let mut back = Tree::parse(out.as_str()).to_structures();
        let repairs = conform(&mut back, version);
        assert!(repairs.is_empty(), "{version}: {repairs:?}\n{out}");
        assert!(write(&back) == out, "{version}: not stable\n{out}");
    }
});
