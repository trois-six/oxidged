//! Producer profiles are total, conformant and idempotent: any bytes read
//! with any import profile (or the one detected) at any interpretation
//! level; and for the file the writer makes of them (identifiers unique,
//! extensions kept), read the same way, the dataset written as 5.5.1 and
//! 7.0 validates,
//! and reading that output back the same way and writing it again gives
//! the same text; exported with every export profile, the output validates
//! and, read back as any file is by default (its producer, oxidged,
//! detected), exporting it again gives the same text.
#![no_main]

use libfuzzer_sys::fuzz_target;
use oxidged::profile::{Exporter, Interpretation, export_profiles, import_profiles};
use oxidged::spec::{DeviationKind, validate_text};
use oxidged::{Bom, Dataset, GedcomBuilder, GedcomVersion, GedcomWriter};

/// The deviations of `out` but the 5.5.1 records over 32K no note can
/// bring under the limit.
fn deviations(out: &str) -> Vec<String> {
    validate_text(out)
        .into_iter()
        .filter(|d| d.kind != DeviationKind::RecordSize)
        .map(|d| d.to_string())
        .collect()
}

fn write(data: &Dataset, version: GedcomVersion) -> String {
    GedcomWriter::new()
        .gedcom_version(version)
        .bom(Bom::Never)
        .write_to_string(data)
        .expect("repairs, never fails")
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, bytes)) = data.split_first() else {
        return;
    };
    let profiles = import_profiles();
    let profile = profiles
        .get(usize::from(selector & 0x3F))
        .copied()
        .filter(|_| selector & 0x40 != 0);
    let level = if selector & 0x80 != 0 {
        Interpretation::BestEffort
    } else {
        Interpretation::Lossless
    };
    let builder = GedcomBuilder::new().profile(profile).interpretation(level);
    // Total on any bytes (the charset a producer means included).
    let _ = builder.build_from_bytes(bytes);
    // The properties hold of files the writer wrote: their identifiers
    // unique, their lines in the grammar (as a producer's export is, give
    // or take its extensions, which writing keeps).
    let base = write(&Dataset::from_bytes(bytes), GedcomVersion::V5_5_1);
    let Ok(read) = builder.build_from_str(base.as_str()) else {
        return;
    };
    for version in [GedcomVersion::V5_5_1, GedcomVersion::V7_0] {
        let out = write(&read, version);
        let left = deviations(&out);
        assert!(left.is_empty(), "{version}: {left:?}\n{out}");
        let again = builder.build_from_str(out.as_str()).expect("lenient");
        let out2 = write(&again, version);
        assert!(
            out2 == out,
            "{version}: not idempotent\n{out}\n----\n{out2}"
        );
    }
    let exported = Dataset::parse(write(&read, GedcomVersion::V7_0));
    for p in export_profiles() {
        let exporter = Exporter::new(p);
        let (out, _) = exporter
            .write_to_string(&exported)
            .expect("repairs, never fails");
        let left = deviations(&out);
        assert!(left.is_empty(), "{}: {left:?}\n{out}", p.id());
        let again = GedcomBuilder::new()
            .build_from_str(out.as_str())
            .expect("lenient");
        let (out2, _) = exporter
            .write_to_string(&again)
            .expect("repairs, never fails");
        assert!(
            out2 == out,
            "{}: not idempotent\n{out}\n----\n{out2}",
            p.id()
        );
    }
});
