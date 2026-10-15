//! Conversion between 5.5.1 and 7.x is total, conformant, stable and keeps
//! the data: for any bytes, the dataset written as 5.5.1, read back and
//! written as 7.0 or 7.1 (with and without the obsolete structures kept)
//! validates and is written again as it was; written back as 5.5.1, every
//! free text of the 5.5.1 form is still there; the lossless tree converts
//! to output that validates; and converting a dataset twice changes nothing
//! more than converting it once.
#![no_main]

use std::collections::HashMap;

use libfuzzer_sys::fuzz_target;
use oxidged::convert::{ConversionOptions, convert_dataset, convert_structures};
use oxidged::spec::{DeviationKind, validate_text};
use oxidged::tree::{Payload, Structure, Tree};
use oxidged::{Bom, Dataset, GedcomVersion, GedcomWriter};

const V551: GedcomVersion = GedcomVersion::V5_5_1;

fn write(data: &Dataset, version: GedcomVersion, options: ConversionOptions) -> String {
    GedcomWriter::new()
        .gedcom_version(version)
        .conversion(options)
        .bom(Bom::Never)
        .write_to_string(data)
        .expect("repairs, never fails")
}

/// The deviations of `out` but the 5.5.1 records over 32K no note can
/// bring under the limit.
fn deviations(out: &str) -> Vec<String> {
    validate_text(out)
        .into_iter()
        .filter(|d| d.kind != DeviationKind::RecordSize)
        .map(|d| d.to_string())
        .collect()
}

/// The free texts of `out` (notes, texts, pages, titles, names, places, a
/// source in words), outside the header, as a multiset: what a conversion
/// moves but never rewrites.
fn texts(out: &str) -> HashMap<String, usize> {
    fn walk(s: &Structure, out: &mut HashMap<String, usize>) {
        let tag = s.tag.as_str().trim_start_matches('_');
        if let Payload::Text(t) = &s.payload {
            let free = matches!(
                tag,
                "NOTE"
                    | "SNOTE"
                    | "TEXT"
                    | "PAGE"
                    | "TITL"
                    | "AUTH"
                    | "PUBL"
                    | "ABBR"
                    | "PLAC"
                    | "SOUR"
            );
            if free && !t.is_empty() {
                *out.entry(t.to_string()).or_insert(0) += 1;
            }
        }
        for c in &s.substructures {
            walk(c, out);
        }
    }
    let mut map = HashMap::new();
    for r in Tree::parse(out)
        .to_structures()
        .iter()
        .filter(|r| r.tag != "HEAD")
    {
        walk(r, &mut map);
    }
    map
}

fuzz_target!(|data: &[u8]| {
    let dataset = Dataset::from_bytes(data);
    let keep = ConversionOptions::default().keep_obsolete(true);
    let p = write(&dataset, V551, keep);
    let v551 = Dataset::parse(p.as_str());
    for v7 in [GedcomVersion::V7_0, GedcomVersion::V7_1] {
        for options in [ConversionOptions::default(), keep] {
            let a = write(&v551, v7, options);
            let left = deviations(&a);
            assert!(left.is_empty(), "{v7}: {left:?}\n{a}");
            let read = Dataset::parse(a.as_str());
            assert!(write(&read, v7, options) == a, "{v7}: not stable\n{a}");
            if options == keep {
                // 5.5.1 → 7.x → 5.5.1 keeps every free text.
                let q = write(&read, V551, keep);
                let (before, after) = (texts(&p), texts(&q));
                for (t, n) in &before {
                    assert!(
                        after.get(t).copied().unwrap_or(0) >= *n,
                        "{v7}: {t:?} lost\n{p}\n{q}"
                    );
                }
            }
        }
        // The lossless tree, converted.
        let mut out = Vec::new();
        GedcomWriter::new()
            .gedcom_version(v7)
            .bom(Bom::Never)
            .write_tree(&mut out, &Tree::parse(p.as_str()))
            .expect("repairs, never fails");
        let out = String::from_utf8(out).expect("UTF-8");
        let left = deviations(&out);
        assert!(left.is_empty(), "{v7}: {left:?}\n{out}");
        let mut records = Tree::from_bytes(data).to_structures();
        convert_structures(&mut records, dataset.version(), v7, &keep);
        // Converting a converted dataset again changes nothing.
        let once = convert_dataset(&v551, v7, &keep);
        let twice = convert_dataset(&once, v7, &keep);
        assert!(once.to_structures() == twice.to_structures(), "{v7}: twice");
    }
});
