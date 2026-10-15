//! The declarations of the typed model, for the coverage ledger, and the
//! structures a dataset kept untyped although the model has a type for
//! them.

use super::dataset::Dataset;
use super::driver::{FieldDesc, Fields, SpecNames, Struct};
use super::enums::{LdsStatus, PHRASED_FIELDS};
use super::header::{
    Charset, Corporation, GedcomInfo, Header, HeaderPlace, HeaderSource, HeaderSourceData,
    HeaderText, Schema,
};
use super::record::{
    Family, Individual, Multimedia, RecordedEvents, Repository, SharedNote, Source, SourceData,
    Submission, Submitter,
};
use super::text::{Store, TagId};
use super::{
    Address, Age, Association, CallNumber, ChangeDate, ChildLink, Citation, CitationData,
    CitedEvent, CreationDate, Crop, Date, Event, EventFamily, EventSpouse, ExactDate, Exid, File,
    FileForm, FileTranslation, IndividualRef, Map, MultimediaLink, Name, NameTranslation, NonEvent,
    Note, NoteTranslation, Ordinance, Period, PhoneticName, PhoneticVariation, Place,
    PlaceTranslation, Refn, RepositoryCitation, RomanizedName, RomanizedVariation, SourceText,
    SpouseLink, TextTranslation,
};
use crate::GedcomVersion;
use crate::spec::conform::{Field, FieldOrder};
use crate::spec::index::name_is;
use crate::spec::schema::Schema as Tables;
use crate::spec::tables;

/// The typed structures and the structure types each stands for.
macro_rules! structures {
    ($($ty:ty),* $(,)?; phrased: $($p:ident: [$($v70:literal),*] [$($v71:literal),*];)*) => {
        /// Every typed structure: its name, its structure types and its
        /// fields, for the coverage ledger. An enumeration value with its
        /// phrase ([`Phrased`](super::Phrased)) stands for the 7.x types
        /// listed with it.
        pub(crate) const STRUCTURES: &[(&str, SpecNames, &[FieldDesc])] = &[
            $((stringify!($ty), <$ty as Struct>::SPEC, <$ty as Struct>::FIELDS),)*
            $((
                concat!("Phrased<", stringify!($p), ">"),
                SpecNames { v551: &[], v70: &[$($v70),*], v71: &[$($v71),*] },
                PHRASED_FIELDS,
            ),)*
        ];
    };
}

structures!(
    Address,
    Age,
    Association,
    CallNumber,
    ChangeDate,
    Charset,
    ChildLink,
    Citation,
    CitationData,
    CitedEvent,
    Corporation,
    CreationDate,
    Crop,
    Date,
    Event,
    EventFamily,
    EventSpouse,
    ExactDate,
    Exid,
    Family,
    File,
    FileForm,
    FileTranslation,
    GedcomInfo,
    Header,
    HeaderPlace,
    HeaderSource,
    HeaderSourceData,
    HeaderText,
    Individual,
    IndividualRef,
    LdsStatus,
    Map,
    Multimedia,
    MultimediaLink,
    Name,
    NameTranslation,
    NonEvent,
    Note,
    NoteTranslation,
    Ordinance,
    Period,
    PhoneticName,
    PhoneticVariation,
    Place,
    PlaceTranslation,
    RecordedEvents,
    Refn,
    Repository,
    RepositoryCitation,
    RomanizedName,
    RomanizedVariation,
    Schema,
    SharedNote,
    Source,
    SourceData,
    SourceText,
    SpouseLink,
    Submission,
    Submitter,
    TextTranslation;
    phrased:
    Adoption: ["FAMC-ADOP"] ["FAMC-ADOP"];
    ChildStatus: ["FAMC-STAT"] ["FAMC-STAT"];
    Medium: ["MEDI"] ["MEDI"];
    NameType: ["NAME-TYPE"] ["NAME-TYPE"];
    Pedigree: ["PEDI"] ["PEDI"];
    Role: ["ROLE"] ["ROLE"];
    NoteKind: [] ["NOTE-KIND"];
);

/// The tags of the records the model types.
const RECORD_TAGS: &[&str] = &[
    "HEAD", "INDI", "FAM", "SOUR", "REPO", "OBJE", "SUBM", "SUBN", "NOTE", "SNOTE",
];

/// The paths (`INDI/BIRT/DATE`) of the structures of `data` kept untyped
/// in an `extra` although a field of their superstructure takes their tag
/// (they did not fit their type, or repeated a singleton), or with a
/// payload or identifier their type has no place for. A conformant file
/// has none, but for leaves (values without substructures) that carry
/// extension substructures, which stay whole. Only the first of a tag is
/// listed: the later ones follow it in `extra` to keep their order.
pub(crate) fn untyped(data: &Dataset) -> Vec<String> {
    let mut out = Vec::new();
    let mut path = Vec::new();
    for record in data.records() {
        // Copied out as owned types, which the driver walks.
        let owned = record.into_owned();
        let cx_tag = match &owned {
            super::Record::Other(node) => {
                let tag = data.store.tag(node.tag);
                if RECORD_TAGS.contains(&tag) {
                    out.push(tag.to_string());
                }
                continue;
            }
            super::Record::Header(r) => ("HEAD", &**r as &dyn Fields),
            super::Record::Individual(r) => ("INDI", &**r as &dyn Fields),
            super::Record::Family(r) => ("FAM", &**r as &dyn Fields),
            super::Record::Source(r) => ("SOUR", &**r as &dyn Fields),
            super::Record::Repository(r) => ("REPO", &**r as &dyn Fields),
            super::Record::Multimedia(r) => ("OBJE", &**r as &dyn Fields),
            super::Record::Submitter(r) => ("SUBM", &**r as &dyn Fields),
            super::Record::Submission(r) => ("SUBN", &**r as &dyn Fields),
            super::Record::Note(r) => ("SNOTE", &**r as &dyn Fields),
        };
        path.push(cx_tag.0);
        check(cx_tag.1, &data.store, &mut path, &mut out);
        path.pop();
    }
    out
}

fn check(s: &dyn Fields, store: &Store, path: &mut Vec<&'static str>, out: &mut Vec<String>) {
    let (lookup, fields) = (s.lookup(), s.fields());
    let mut seen = Vec::new();
    for node in s.extra() {
        // A payload or identifier the type had no place for.
        if node.tag == TagId::ASIDE {
            out.push(path.join("/"));
            continue;
        }
        let field = lookup.get(node.tag.get() as usize).copied().unwrap_or(0);
        let Some(desc) = usize::from(field)
            .checked_sub(1)
            .and_then(|f| fields.get(f))
        else {
            continue;
        };
        if seen.contains(&node.tag) {
            continue;
        }
        seen.push(node.tag);
        // A leaf has no `extra`: with substructures of its own, it is kept
        // whole where it is.
        if !desc.leaf || node.children.is_empty() {
            out.push(format!("{}/{}", path.join("/"), store.tag(node.tag)));
        }
    }
    s.walk(&mut |tag, child| {
        path.push(tag);
        check(child, store, path, out);
        path.pop();
    });
}

/// The order in which the model writes the substructures of each structure
/// type of `version` ([`FieldOrder`]): the fields of the typed structure
/// that stands for it, in their declared order, and their shapes. Computed
/// at compile time.
pub(crate) fn field_order(version: GedcomVersion) -> &'static FieldOrder {
    match version {
        GedcomVersion::V5_5_1 => &ORDER_551,
        GedcomVersion::V7_0 => &ORDER_70,
        GedcomVersion::V7_1 => &ORDER_71,
    }
}

/// The spec names of a typed structure for the version of `schema`.
const fn spec_names(spec: &SpecNames, schema: &Tables) -> &'static [&'static str] {
    match schema.version.as_bytes() {
        b"5.5.1" => spec.v551,
        b"7.0" => spec.v70,
        _ => spec.v71,
    }
}

/// The typed structure (an index into [`STRUCTURES`]) that stands for type
/// `id` of `schema`: the first that names it and has fields with tags.
const fn structure_of(schema: &Tables, id: usize) -> Option<usize> {
    let mut e = 0;
    while e < STRUCTURES.len() {
        let names = spec_names(&STRUCTURES[e].1, schema);
        let mut n = 0;
        while n < names.len() {
            if name_is(schema, id, names[n]) && tag_count(STRUCTURES[e].2) > 0 {
                return Some(e);
            }
            n += 1;
        }
        e += 1;
    }
    None
}

/// The number of tags the fields take.
const fn tag_count(fields: &[FieldDesc]) -> usize {
    let (mut n, mut f) = (0, 0);
    while f < fields.len() {
        n += fields[f].tags.len();
        f += 1;
    }
    n
}

/// The number of (type, tag) cells of the order of `schema`.
const fn order_len(schema: &Tables) -> usize {
    let (mut n, mut id) = (0, 0);
    while id < schema.structs.len() {
        if let Some(e) = structure_of(schema, id) {
            n += tag_count(STRUCTURES[e].2);
        }
        id += 1;
    }
    n
}

/// The rows and cells of a [`FieldOrder`].
type OrderTables<const R: usize, const C: usize> = ([(u16, u16); R], [(u16, Field); C]);

/// The rows and cells of [`FieldOrder`] for `schema`: `R` types, `C`
/// cells.
// Ranks and indices are below 65,536 (checked); constants convert with
// `as` only.
#[allow(clippy::cast_possible_truncation)]
const fn order<const R: usize, const C: usize>(schema: &Tables) -> OrderTables<R, C> {
    let none = Field {
        rank: 0,
        leaf: false,
        many: false,
    };
    let mut rows = [(0_u16, 0_u16); R];
    let mut cells = [(0_u16, none); C];
    assert!(C < u16::MAX as usize);
    let (mut c, mut id) = (0, 0);
    while id < R {
        let first = c;
        if let Some(e) = structure_of(schema, id) {
            let fields = STRUCTURES[e].2;
            let mut f = 0;
            while f < fields.len() {
                let field = Field {
                    rank: f as u16,
                    leaf: fields[f].leaf,
                    many: fields[f].many,
                };
                let mut t = 0;
                while t < fields[f].tags.len() {
                    let standard = crate::tree::standard_index_const(fields[f].tags[t]);
                    assert!(standard.is_some(), "a field tag is not a standard tag");
                    if let Some(standard) = standard {
                        cells[c] = (standard, field);
                        c += 1;
                    }
                    t += 1;
                }
                f += 1;
            }
        }
        rows[id] = (first as u16, (c - first) as u16);
        id += 1;
    }
    assert!(c == C);
    (rows, cells)
}

/// Declares the [`FieldOrder`] of one version's tables.
macro_rules! order {
    ($name:ident, $schema:path) => {
        static $name: FieldOrder = {
            const R: usize = $schema.structs.len();
            const C: usize = order_len(&$schema);
            static TABLES: OrderTables<R, C> = order(&$schema);
            FieldOrder::new(&TABLES.0, &TABLES.1)
        };
    };
}

order!(ORDER_551, tables::V551);
order!(ORDER_70, tables::V70);
order!(ORDER_71, tables::V71);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::schema::StructId;
    use std::collections::HashMap;

    /// The order computed at compile time is the one the typed model's
    /// declarations give: for each structure type, the fields of the first
    /// typed structure that names it, each of its tags with the field.
    #[test]
    fn the_field_order_is_the_declarations() {
        for version in [
            GedcomVersion::V5_5_1,
            GedcomVersion::V7_0,
            GedcomVersion::V7_1,
        ] {
            let schema = version.rules().spec;
            let by_name: HashMap<&str, usize> = (0..schema.structs.len())
                .map(|i| (schema.name(StructId::try_from(i).unwrap()), i))
                .collect();
            let mut expected: Vec<Vec<(u16, u16, bool, bool)>> =
                vec![Vec::new(); schema.structs.len()];
            for (_, spec, fields) in STRUCTURES {
                for name in spec_names(spec, schema) {
                    let Some(slot) = by_name.get(name).and_then(|&i| expected.get_mut(i)) else {
                        continue;
                    };
                    if !slot.is_empty() {
                        continue;
                    }
                    for (rank, field) in (0_u16..).zip(fields.iter()) {
                        for tag in field.tags {
                            let standard = crate::tree::node::standard_index(tag).unwrap();
                            slot.push((standard, rank, field.leaf, field.many));
                        }
                    }
                }
            }
            let order = field_order(version);
            for (ty, cells) in expected.iter().enumerate() {
                let ty = StructId::try_from(ty).unwrap();
                for &(standard, rank, leaf, many) in cells {
                    let f = order.field(ty, standard).unwrap();
                    // The first field taking a tag is the one found.
                    if cells.iter().find(|c| c.0 == standard).unwrap().1 == rank {
                        assert_eq!(
                            (f.rank, f.leaf, f.many),
                            (rank, leaf, many),
                            "{version} {ty}"
                        );
                    }
                }
                let count = (0..crate::tree::node::STANDARD_TAGS.len())
                    .filter(|&s| order.field(ty, u16::try_from(s).unwrap()).is_some())
                    .count();
                let distinct: std::collections::HashSet<u16> = cells.iter().map(|c| c.0).collect();
                assert_eq!(count, distinct.len(), "{version} {ty}");
            }
        }
    }
}
