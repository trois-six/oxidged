//! Edit sequences keep a dataset whole: for any bytes — a file, then
//! after the first NUL byte the edits to run on it (the file's own bytes
//! when there is no NUL) — each edit leaves the count of pointers to every
//! identifier equal to a recount of the dataset's structures and lookups
//! agreeing with a search; dangling pointers only come from the edits
//! that may leave them (a removal that leaves them, an unchecked push, an
//! identifier set through a lent record); and the dataset is then written
//! conformant in every version (but for what the conformance repair
//! documents it leaves: `common/left.rs`), and stable through a read back.
//! Among the edits, a subset of a branch (an individual's ancestors or
//! descendants) replaces the dataset: it keeps only those individuals and
//! adds no dangling pointer.
#![no_main]

use std::collections::BTreeMap;

use libfuzzer_sys::fuzz_target;
use oxidged::model::{
    ChildLink, Citation, CitationSource, Dataset, Family, Holder, Individual, IndividualRef, Link,
    Node, Note, Record, RecordHandle, RecordKind, RecordType, Removal, SharedNote, Source, Value,
    XrefId,
};
use oxidged::tree::{Payload, Structure};
use oxidged::{GedcomVersion, GedcomWriter};

#[path = "common/left.rs"]
mod left;

/// The edits' bytes, read as small numbers.
struct Ops<'a>(&'a [u8]);

impl Ops<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }

    fn below(&mut self, n: usize) -> usize {
        match (self.byte(), n) {
            (_, 0) | (None, _) => 0,
            (Some(b), n) => usize::from(b) % n,
        }
    }
}

fn recount(data: &Dataset) -> BTreeMap<String, usize> {
    fn walk(s: &Structure, out: &mut BTreeMap<String, usize>) {
        if let Payload::Pointer(p) = &s.payload {
            *out.entry(p.as_str().to_string()).or_default() += 1;
        }
        for c in &s.substructures {
            walk(c, out);
        }
    }
    let mut out = BTreeMap::new();
    for s in data.to_structures() {
        walk(&s, &mut out);
    }
    out
}

fn check(data: &Dataset) {
    let expected = recount(data);
    for (xref, n) in &expected {
        assert_eq!(data.references(xref.as_str()), *n, "{xref}");
    }
    // The first record of each identifier, in one pass.
    let mut first = std::collections::HashMap::new();
    for record in data.records() {
        if let Some(id) = record.xref() {
            first.entry(id).or_insert(record);
        }
    }
    for (&id, record) in &first {
        let xref = data.store().xref(id);
        assert_eq!(
            data.references(id),
            expected.get(xref).copied().unwrap_or(0),
            "{xref}"
        );
        assert_eq!(data.find(id), Some(*record), "{xref}");
    }
    for (h, r) in data.records_with_handles() {
        assert_eq!(data.get_any(h), Some(r));
    }
}

fn any(data: &Dataset, ops: &mut Ops<'_>) -> Option<RecordHandle> {
    let all: Vec<RecordHandle> = data.records_with_handles().map(|(h, _)| h).collect();
    (!all.is_empty()).then(|| all[ops.below(all.len())])
}

fn xref_of<T: RecordType>(data: &Dataset, ops: &mut Ops<'_>) -> Option<XrefId> {
    let n = data.all::<T>().len();
    let h = (n > 0).then(|| data.handle_at::<T>(ops.below(n)))??;
    data.get_any(h.erase()).and_then(|r| r.xref())
}

fn remove(data: &mut Dataset, h: RecordHandle, how: Removal) {
    let _ = match h.kind() {
        RecordKind::Individual => data.remove(h.typed::<Individual>().unwrap(), how).map(drop),
        RecordKind::Family => data.remove(h.typed::<Family>().unwrap(), how).map(drop),
        RecordKind::Source => data.remove(h.typed::<Source>().unwrap(), how).map(drop),
        RecordKind::Note => data.remove(h.typed::<SharedNote>().unwrap(), how).map(drop),
        RecordKind::Other => data.remove(h.typed::<Node>().unwrap(), how).map(drop),
        RecordKind::Repository => data
            .remove(h.typed::<oxidged::model::Repository>().unwrap(), how)
            .map(drop),
        RecordKind::Multimedia => data
            .remove(h.typed::<oxidged::model::Multimedia>().unwrap(), how)
            .map(drop),
        RecordKind::Submitter => data
            .remove(h.typed::<oxidged::model::Submitter>().unwrap(), how)
            .map(drop),
        _ => Ok(()),
    };
}

const LINKS: [Link; 15] = [
    Link::ChildOf,
    Link::SpouseOf,
    Link::Husband,
    Link::Wife,
    Link::Child,
    Link::Alias,
    Link::Association,
    Link::Citation,
    Link::Note,
    Link::Multimedia,
    Link::Repository,
    Link::Submitter,
    Link::AncestorInterest,
    Link::DescendantInterest,
    Link::Submission,
];

/// One edit; whether it may leave pointers dangling.
fn edit(data: &mut Dataset, ops: &mut Ops<'_>, fresh: &mut usize) -> bool {
    match ops.below(13) {
        0 => {
            let mut indi = Individual::default();
            if let Some(f) = xref_of::<Family>(data, ops) {
                indi.child_of.push(ChildLink::new(f));
            }
            if let Some(s) = xref_of::<Source>(data, ops) {
                indi.citations.push(Citation {
                    source: CitationSource::Pointer(s),
                    ..Citation::default()
                });
            }
            data.add(indi).expect("pointers to records");
            false
        }
        1 => {
            let fam = Family {
                husband: xref_of::<Individual>(data, ops).map(IndividualRef::new),
                ..Family::default()
            };
            data.add(fam).expect("pointers to records");
            false
        }
        2 | 3 => {
            let (Some(holder), Some(target)) = (any(data, ops), any(data, ops)) else {
                return false;
            };
            let holder = if ops.below(8) == 0 {
                Holder::Header
            } else {
                holder.into()
            };
            let link = if ops.below(8) == 0 {
                Link::Extension(data.store_mut().intern_tag("_LINK"))
            } else {
                LINKS[ops.below(LINKS.len())]
            };
            let target = data.get_any(target).and_then(|r| r.xref());
            if ops.below(2) == 0 {
                let _ = data.link(holder, link, target);
            } else {
                let _ = data.unlink(holder, link, target);
            }
            false
        }
        4 => {
            let (Some(f), Some(i)) = (
                data.handle_at::<Family>(0),
                data.handle_at::<Individual>(ops.below(data.individuals().len().max(1))),
            ) else {
                return false;
            };
            let _ = match ops.below(4) {
                0 => data.link_child(f, i).map(|()| 0),
                1 => data.link_spouse(f, i).map(|()| 0),
                2 => data.unlink_child(f, i),
                _ => data.unlink_spouse(f, i),
            };
            false
        }
        5 => {
            let Some(h) = any(data, ops) else {
                return false;
            };
            let how = [Removal::Refuse, Removal::Detach, Removal::LeaveDangling][ops.below(3)];
            remove(data, h, how);
            how == Removal::LeaveDangling
        }
        6 => {
            let Some(id) = any(data, ops)
                .and_then(|h| data.get_any(h))
                .and_then(|r| r.xref())
            else {
                return false;
            };
            *fresh += 1;
            let new = match ops.below(3) {
                0 => format!("@x{fresh}@"),
                1 => format!("@X{fresh}@"),
                _ => "@I1@".to_string(),
            };
            let _ = data.rename(id, &new);
            false
        }
        7 => {
            let (Some(a), Some(b)) = (
                xref_of::<Individual>(data, ops),
                xref_of::<Individual>(data, ops),
            ) else {
                return false;
            };
            let _ = data.redirect(a, b);
            false
        }
        8 => {
            let Some(id) = any(data, ops)
                .and_then(|h| data.get_any(h))
                .and_then(|r| r.xref())
            else {
                return false;
            };
            data.detach(id);
            false
        }
        9 => {
            let n = data.individuals().len();
            if n == 0 {
                return false;
            }
            let at = ops.below(n);
            let note = xref_of::<SharedNote>(data, ops);
            let other = xref_of::<Individual>(data, ops);
            let tag = data.store_mut().intern_tag("_SEEN");
            let choice = ops.below(5);
            let forget = ops.below(8) == 0;
            let mut guard = data.get_mut_at::<Individual>(at).unwrap();
            let dangles = match choice {
                0 => {
                    guard.citations.clear();
                    guard.extra.clear();
                    false
                }
                1 => {
                    if let Some(n) = note {
                        guard.notes.push(Note::shared(n));
                    }
                    false
                }
                2 => {
                    if let Some(o) = other {
                        guard.extra.push(Node {
                            payload: Value::Pointer(o),
                            ..Node::new(tag)
                        });
                    }
                    false
                }
                3 => {
                    // Its identifier changed: pointers to the old one may
                    // dangle.
                    guard.xref = other;
                    true
                }
                _ => {
                    guard.child_of.clear();
                    false
                }
            };
            if forget {
                std::mem::forget(guard);
            }
            dangles
        }
        10 => {
            let id = data.store_mut().intern_xref("@GONE@");
            let mut indi = Individual::default();
            if let Some(id) = id {
                indi.child_of.push(ChildLink::new(id));
            }
            data.push(Record::Individual(Box::new(indi)));
            true
        }
        11 => {
            // A branch: the relatives of an individual, some generations
            // up or down, with it.
            let Some(start) = xref_of::<Individual>(data, ops) else {
                return false;
            };
            let depth = match ops.below(4) {
                0 => None,
                d => u32::try_from(d).ok(),
            };
            let mut chosen: Vec<_> = if ops.below(2) == 0 {
                data.ancestors(start, depth)
            } else {
                data.descendants(start, depth)
            }
            .iter()
            .filter_map(|r| r.individual.xref())
            .collect();
            chosen.push(start);
            let subset = data.subset(chosen.iter().copied());
            // Only chosen individuals are kept, and the dataset is unchanged.
            let names = |d: &Dataset, id: XrefId| d.store().xref(id).to_string();
            let chosen: std::collections::BTreeSet<_> =
                chosen.iter().map(|&id| names(data, id)).collect();
            for i in subset.individuals() {
                let id = i.xref().expect("a kept individual has an identifier");
                assert!(chosen.contains(&names(&subset, id)));
            }
            *data = subset;
            false
        }
        _ => {
            let submitter = xref_of::<oxidged::model::Submitter>(data, ops);
            if let Some(mut h) = data.header_mut() {
                h.submitter = submitter;
            }
            false
        }
    }
}

fuzz_target!(|bytes: &[u8]| {
    let (file, ops) = match bytes.iter().position(|&b| b == 0) {
        Some(at) => (&bytes[..at], &bytes[at + 1..]),
        None => (bytes, bytes),
    };
    let mut data = Dataset::from_bytes(file);
    check(&data);
    let mut ops = Ops(ops);
    let mut fresh = 0;
    let mut dangling = data.dangling_references().len();
    for _ in 0..32 {
        if ops.0.is_empty() {
            break;
        }
        let asked = edit(&mut data, &mut ops, &mut fresh);
        check(&data);
        let now = data.dangling_references().len();
        assert!(asked || now <= dangling, "{dangling} -> {now} dangling");
        dangling = now;
    }
    for version in [
        GedcomVersion::V5_5_1,
        GedcomVersion::V7_0,
        GedcomVersion::V7_1,
    ] {
        let writer = GedcomWriter::new().gedcom_version(version);
        let Ok(out) = writer.write_to_string(&data) else {
            continue;
        };
        let deviations = left::unexpected(&out, version);
        assert!(deviations.is_empty(), "{version}: {deviations:?}");
        let again = writer
            .write_to_string(&Dataset::parse(out.as_str()))
            .expect("a written dataset writes");
        assert!(again == out, "{version}: not stable");
    }
});
