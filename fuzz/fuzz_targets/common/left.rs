//! What the conformance repair documents it leaves in a written file
//! (`oxidged::spec`, repair j): a 5.5.1 record over 32K that holds no note
//! to move out, a header (whose note takes no pointer) or an extension
//! record. Shared by the targets that validate what the writer writes.

use oxidged::GedcomVersion;
use oxidged::spec::{Deviation, DeviationKind, validate_text};
use oxidged::tree::{Payload, Structure, parse_tree};

/// The bytes of the structure starting at the 1-based line `line` of
/// `lines`: its line and every line under it.
fn bytes_at(lines: &[&str], line: u32) -> usize {
    let level = |l: &str| l.split(' ').next().and_then(|n| n.parse::<usize>().ok());
    let start = line as usize - 1;
    let own = level(lines[start]);
    let end = lines[start + 1..]
        .iter()
        .position(|l| level(l) <= own)
        .map_or(lines.len(), |i| start + 1 + i);
    lines[start..end].iter().map(|l| l.len() + 1).sum()
}

/// Whether `s` holds an inline note (a `NOTE` with a text, under standard
/// structures only) longer than the pointer line that would replace it (32
/// bytes at most), which a 5.5.1 record points to instead when it is over
/// 32K.
fn holds_movable_note(lines: &[&str], s: &Structure) -> bool {
    s.substructures.iter().any(|c| {
        !c.tag.as_str().starts_with('_')
            && (c.tag == "NOTE"
                && matches!(c.payload, Payload::Text(_))
                && bytes_at(lines, c.line) > 32
                || holds_movable_note(lines, c))
    })
}

/// Whether a deviation is the one the conformance repair leaves (`spec`,
/// repair j): a 5.5.1 record over 32K that holds no note to move out, a
/// header (whose note takes no pointer) or an extension record.
fn is_left(d: &Deviation, version: GedcomVersion, lines: &[&str], records: &[Structure]) -> bool {
    d.kind == DeviationKind::RecordSize
        && version == GedcomVersion::V5_5_1
        && records.iter().find(|r| r.line == d.line).is_some_and(|r| {
            r.tag == "HEAD" || r.tag.as_str().starts_with('_') || !holds_movable_note(lines, r)
        })
}

/// The deviations the validator finds in `out`, written in `version`, but
/// those the conformance repair documents it leaves.
pub fn unexpected(out: &str, version: GedcomVersion) -> Vec<Deviation> {
    let mut deviations = validate_text(out);
    if !deviations.is_empty() {
        let records = parse_tree(out).to_structures();
        let lines: Vec<&str> = out.lines().collect();
        deviations.retain(|d| !is_left(d, version, &lines, &records));
    }
    deviations
}
