//! Indexes of the specification tables, computed at compile time.
//!
//! The tables ([`super::tables`]) list each structure type's permitted
//! substructures, sorted by tag. What the validator, the conformance repair
//! and the writer ask of them most — the type of a `tag` substructure of a
//! `sup` structure, how many it may hold, what it requires — is answered
//! here without a search: one [`Cell`] per type and tag of its
//! substructures, found through a byte per tag of each type that has
//! substructures.
//!
//! Every index is a `static` built by constant evaluation from the tables,
//! so a program never spends time building one (a few kilobytes per
//! version).

use super::schema::{Kind, Schema, StructId};
use super::tables::{self, TAGS};
use crate::tree::node::STANDARD_TAGS;

/// [`Row::flags`]: a 7.x note translation (`NOTE-TRAN`), which says its
/// language or its media type.
pub(crate) const NOTE_TRAN: u8 = 1;
/// [`Row::flags`]: a 7.x media type (`MIME`) of a text.
pub(crate) const MIME: u8 = 2;
/// [`Row::flags`]: the 7.x alias pointer (`ALIA`), which may not point to
/// its own record (7.0.17).
pub(crate) const ALIAS: u8 = 4;
/// [`Row::flags`]: the 7.x source record, which with multimedia and shared
/// note records may form no pointer cycle (7.0.17).
pub(crate) const SOURCE: u8 = 8;
/// [`Row::flags`]: a 7.x multimedia or shared note record, which with
/// source records may form no pointer cycle (7.0.17).
pub(crate) const LINKED: u8 = 16;
/// [`Row::flags`]: the 7.x individual record, whose `ALIA` may not point
/// to itself (7.0.17).
pub(crate) const INDIVIDUAL: u8 = 32;

/// [`Row::slots`] of a type without substructures.
const NO_SLOTS: u16 = u16::MAX;

/// What a structure type's substructures are, by tag.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Row {
    /// Where its slots start in [`SchemaIndex::slots`], in rows of one
    /// byte per tag of `TAGS` ([`NO_SLOTS`]: none).
    slots: u16,
    /// The index in [`SchemaIndex::cells`] of its first cell.
    first: u16,
    /// The first and the number of its required substructures in
    /// [`SchemaIndex::required`].
    required: u16,
    nrequired: u8,
    /// [`NOTE_TRAN`], [`MIME`].
    flags: u8,
}

/// The substructures of one structure type with one tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    /// The form whose payload is a pointer, else the first: what a
    /// substructure with a pointer payload is (5.5.1 has a pointer form and
    /// a text form of some tags).
    pub(crate) pointer: StructId,
    /// The form whose payload is no pointer, else the first.
    pub(crate) other: StructId,
    /// The first form, in the order of the tables.
    pub(crate) first: StructId,
    /// The fewest occurrences.
    pub(crate) min: u8,
    /// The tag, an index into `TAGS`.
    pub(crate) tag: u8,
    /// The most occurrences; `u16::MAX`: no bound.
    pub(crate) max: u16,
}

/// The index of one version's tables.
#[derive(Debug)]
pub(crate) struct SchemaIndex {
    rows: &'static [Row],
    /// For each type with substructures, a byte per tag of `TAGS`: the
    /// index of its cell among the type's plus one, 0 for none.
    slots: &'static [u8],
    cells: &'static [Cell],
    /// The indices in `cells` of the required substructures of each type
    /// ([`Row::required`]).
    required: &'static [u16],
}

impl SchemaIndex {
    /// The substructures of `sup` tagged `TAGS[tag]`.
    #[inline]
    pub(crate) fn cell(&self, sup: StructId, tag: u8) -> Option<&'static Cell> {
        let row = self.rows.get(usize::from(sup))?;
        let slot = *self
            .slots
            .get(usize::from(row.slots) * TAGS.len() + usize::from(tag))
            .filter(|_| usize::from(tag) < TAGS.len())?;
        let at = usize::from(slot).checked_sub(1)?;
        self.cells.get(usize::from(row.first) + at)
    }

    /// The type of a `TAGS[tag]` substructure of `sup`, with a pointer
    /// payload or not ([`super::validate`]'s `choose`).
    #[inline]
    pub(crate) fn pick(&self, sup: StructId, tag: u8, is_pointer: bool) -> Option<StructId> {
        self.cell(sup, tag)
            .map(|c| if is_pointer { c.pointer } else { c.other })
    }

    /// The required substructures of `ty`: their cells, in the order of
    /// the tables.
    #[inline]
    pub(crate) fn required(&self, ty: StructId) -> impl Iterator<Item = &'static Cell> + use<> {
        let (start, len) = self.rows.get(usize::from(ty)).map_or((0, 0), |r| {
            (usize::from(r.required), usize::from(r.nrequired))
        });
        let cells = self.cells;
        self.required
            .get(start..start + len)
            .unwrap_or(&[])
            .iter()
            .filter_map(move |&i| cells.get(usize::from(i)))
    }

    /// Whether `ty` is a 7.x note translation.
    #[inline]
    pub(crate) fn is_note_tran(&self, ty: StructId) -> bool {
        self.flags(ty) & NOTE_TRAN != 0
    }

    /// Whether `ty` is a 7.x media type.
    #[inline]
    pub(crate) fn is_mime(&self, ty: StructId) -> bool {
        self.flags(ty) & MIME != 0
    }

    /// Whether `ty` is the 7.x alias pointer `ALIA`.
    #[inline]
    pub(crate) fn is_alias(&self, ty: StructId) -> bool {
        self.flags(ty) & ALIAS != 0
    }

    /// Whether `ty` is the 7.x individual record.
    #[inline]
    pub(crate) fn is_individual(&self, ty: StructId) -> bool {
        self.flags(ty) & INDIVIDUAL != 0
    }

    /// Whether a record of type `rtype` has pointers the 7.x pointer rules
    /// concern ([`crate::spec::graph`]): an individual (its `ALIA`), a
    /// source, a multimedia or a shared note record.
    #[inline]
    pub(crate) fn is_graph_record(&self, rtype: StructId) -> bool {
        self.flags(rtype) & (SOURCE | LINKED | INDIVIDUAL) != 0
    }

    /// Whether a structure of type `ty`, in a record of type `rtype`, has a
    /// pointer the 7.x pointer rules concern: an `ALIA`, or a pointer of a
    /// source record to a multimedia or shared note record, or of one of
    /// those to a source record.
    #[inline]
    pub(crate) fn is_graph_pointer(&self, schema: &Schema, rtype: StructId, ty: StructId) -> bool {
        if self.is_alias(ty) {
            return true;
        }
        let from = self.flags(rtype) & (SOURCE | LINKED);
        if from == 0 {
            return false;
        }
        let (kind, target) = schema.kind(ty);
        if !matches!(kind, Kind::Pointer | Kind::NullablePointer) {
            return false;
        }
        let to = self.flags(target) & (SOURCE | LINKED);
        from == SOURCE && to == LINKED || from == LINKED && to == SOURCE
    }

    #[inline]
    fn flags(&self, ty: StructId) -> u8 {
        self.rows.get(usize::from(ty)).map_or(0, |r| r.flags)
    }
}

/// The index in `TAGS` of each tag of `STANDARD_TAGS`, `u8::MAX` for none.
// Below 255: checked; constants can only convert with `as`.
#[allow(clippy::cast_possible_truncation)]
pub(crate) static SPEC_TAGS: [u8; STANDARD_TAGS.len()] = {
    let mut table = [u8::MAX; STANDARD_TAGS.len()];
    let mut i = 0;
    while i < STANDARD_TAGS.len() {
        let mut t = 0;
        while t < TAGS.len() {
            if eq(STANDARD_TAGS[i].as_bytes(), TAGS[t].as_bytes()) {
                assert!(t < u8::MAX as usize, "too many tags");
                table[i] = t as u8;
            }
            t += 1;
        }
        i += 1;
    }
    table
};

/// Whether two byte strings are equal, in a constant.
const fn eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Whether the name of type `id` is `name`, in a constant.
pub(crate) const fn name_is(s: &Schema, id: usize, name: &str) -> bool {
    let d = s.const_def(id);
    let names = s.names.as_bytes();
    let name = name.as_bytes();
    if d.name_len != name.len() || d.name + d.name_len > names.len() {
        return false;
    }
    let mut i = 0;
    while i < name.len() {
        if names[d.name + i] != name[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The number of types of `s` with substructures, of (type, tag) pairs,
/// and of required ones.
const fn counts(s: &Schema) -> (usize, usize, usize) {
    let (mut rows, mut cells, mut required) = (0, 0, 0);
    let mut id = 0;
    while id < s.structs.len() {
        let d = s.const_def(id);
        if d.nsubs > 0 {
            rows += 1;
        }
        let mut i = 0;
        while i < d.nsubs {
            let sub = s.subs[d.subs + i];
            let tag = s.const_def(sub.id as usize).tag;
            let new = i == 0 || s.const_def(s.subs[d.subs + i - 1].id as usize).tag != tag;
            if new {
                cells += 1;
                // Required when one form of the tag is.
                let mut j = i;
                let mut min = 0;
                while j < d.nsubs && s.const_def(s.subs[d.subs + j].id as usize).tag == tag {
                    if s.subs[d.subs + j].min > min {
                        min = s.subs[d.subs + j].min;
                    }
                    j += 1;
                }
                if min > 0 {
                    required += 1;
                }
            }
            i += 1;
        }
        id += 1;
    }
    (rows, cells, required)
}

const NO_ROW: Row = Row {
    slots: NO_SLOTS,
    first: 0,
    required: 0,
    nrequired: 0,
    flags: 0,
};

const NO_CELL: Cell = Cell {
    pointer: 0,
    other: 0,
    first: 0,
    min: 0,
    tag: 0,
    max: 0,
};

/// The [`Row::flags`] of type `id` of `schema`: the 7.x types some rules
/// name.
const fn flags_of(schema: &Schema, id: usize) -> u8 {
    if schema.version.as_bytes()[0] != b'7' {
        return 0;
    }
    let named = [
        ("NOTE-TRAN", NOTE_TRAN),
        ("MIME", MIME),
        ("ALIA", ALIAS),
        ("record-SOUR", SOURCE),
        ("record-OBJE", LINKED),
        ("record-SNOTE", LINKED),
        ("record-INDI", INDIVIDUAL),
    ];
    let mut flags = 0;
    let mut i = 0;
    while i < named.len() {
        if name_is(schema, id, named[i].0) {
            flags |= named[i].1;
        }
        i += 1;
    }
    flags
}

/// The rows, slots, cells and required cells of `s`: `R` types, `D` of
/// them with substructures, `C` cells and `Q` required ones ([`counts`]).
// Indices are below the table sizes, checked to fit; constants can only
// convert with `as`.
#[allow(clippy::cast_possible_truncation)]
const fn build<const R: usize, const S: usize, const C: usize, const Q: usize>(
    schema: &Schema,
) -> ([Row; R], [u8; S], [Cell; C], [u16; Q]) {
    assert!(R == schema.structs.len() && C < u16::MAX as usize && S.is_multiple_of(TAGS.len()));
    let mut rows = [NO_ROW; R];
    let mut slots = [0_u8; S];
    let mut d_rows = 0;
    let mut cells = [NO_CELL; C];
    let mut required = [0_u16; Q];
    let (mut next, mut req) = (0, 0);
    let mut id = 0;
    while id < R {
        let d = schema.const_def(id);
        let mut row = NO_ROW;
        row.required = req as u16;
        row.flags = flags_of(schema, id);
        let first_cell = next;
        row.first = next as u16;
        if d.nsubs > 0 {
            row.slots = d_rows as u16;
            d_rows += 1;
        }
        let mut i = 0;
        while i < d.nsubs {
            let sub = schema.subs[d.subs + i];
            let sd = schema.const_def(sub.id as usize);
            let tag = sd.tag;
            let new = i == 0
                || schema
                    .const_def(schema.subs[d.subs + i - 1].id as usize)
                    .tag
                    != tag;
            if new {
                assert!(
                    i == 0
                        || schema
                            .const_def(schema.subs[d.subs + i - 1].id as usize)
                            .tag
                            < tag,
                    "the tables sort each type's substructures by tag"
                );
                assert!(next - first_cell < u8::MAX as usize);
                slots[row.slots as usize * TAGS.len() + tag as usize] =
                    (next - first_cell + 1) as u8;
                cells[next] = Cell {
                    first: sub.id,
                    tag,
                    ..NO_CELL
                };
                next += 1;
            }
            let cell = &mut cells[next - 1];
            let is_pointer = matches!(sd.kind, Kind::Pointer | Kind::NullablePointer);
            if is_pointer && cell.pointer == 0 {
                cell.pointer = sub.id;
            }
            if !is_pointer && cell.other == 0 {
                cell.other = sub.id;
            }
            if sub.min > cell.min {
                cell.min = sub.min;
            }
            cell.max = if cell.max == u16::MAX || sub.max == 0 {
                u16::MAX
            } else if (sub.max as u16) > cell.max {
                sub.max as u16
            } else {
                cell.max
            };
            i += 1;
        }
        let mut k = first_cell;
        while k < next {
            if cells[k].pointer == 0 {
                cells[k].pointer = cells[k].first;
            }
            if cells[k].other == 0 {
                cells[k].other = cells[k].first;
            }
            if cells[k].min > 0 {
                required[req] = k as u16;
                req += 1;
            }
            k += 1;
        }
        row.nrequired = (req - row.required as usize) as u8;
        rows[id] = row;
        id += 1;
    }
    assert!(next == C && req == Q && d_rows * TAGS.len() == S);
    (rows, slots, cells, required)
}

/// Declares the index of one version's tables.
macro_rules! index {
    ($name:ident, $schema:path) => {
        pub(crate) static $name: SchemaIndex = {
            const R: usize = $schema.structs.len();
            const N: (usize, usize, usize) = counts(&$schema);
            static TABLES: ([Row; R], [u8; N.0 * TAGS.len()], [Cell; N.1], [u16; N.2]) =
                build(&$schema);
            SchemaIndex {
                rows: &TABLES.0,
                slots: &TABLES.1,
                cells: &TABLES.2,
                required: &TABLES.3,
            }
        };
    };
}

index!(V551, tables::V551);
index!(V70, tables::V70);
index!(V71, tables::V71);

/// The index of `schema`, one of the tables' versions.
pub(crate) fn of(schema: &Schema) -> Option<&'static SchemaIndex> {
    [
        (&tables::V551, &V551),
        (&tables::V70, &V70),
        (&tables::V71, &V71),
    ]
    .into_iter()
    .find(|(s, _)| std::ptr::eq(*s, schema))
    .map(|(_, index)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::schema::DATASET;

    /// The index answers what a search of the tables answers, for every
    /// type and tag of every version.
    #[test]
    fn the_index_is_the_tables() {
        for schema in [&tables::V551, &tables::V70, &tables::V71] {
            let index = of(schema).unwrap();
            for sup in 0..schema.structs.len() {
                let sup = StructId::try_from(sup).unwrap();
                let mut required = Vec::new();
                for tag in 0..TAGS.len() {
                    let tag = u8::try_from(tag).unwrap();
                    let subs: Vec<_> = schema.subs_tagged(sup, tag).collect();
                    let cell = index.cell(sup, tag);
                    assert_eq!(cell.is_none(), subs.is_empty(), "{sup} {tag}");
                    let Some(cell) = cell else { continue };
                    let is_ptr =
                        |id| matches!(schema.kind(id).0, Kind::Pointer | Kind::NullablePointer);
                    let choose = |ptr: bool| {
                        subs.iter()
                            .find(|s| is_ptr(s.id) == ptr)
                            .or(subs.first())
                            .map(|s| s.id)
                    };
                    assert_eq!(Some(cell.pointer), choose(true));
                    assert_eq!(Some(cell.other), choose(false));
                    assert_eq!(cell.first, subs[0].id);
                    assert_eq!(cell.tag, tag);
                    let (min, max) = crate::spec::validate::bounds(schema, sup, tag);
                    assert_eq!(cell.min, min);
                    assert_eq!(cell.max, max.map_or(u16::MAX, u16::from));
                    if min > 0 {
                        required.push(tag);
                    }
                }
                let found: Vec<u8> = index.required(sup).map(|c| c.tag).collect();
                assert_eq!(found, required, "{sup}");
                let name = schema.name(sup);
                assert_eq!(
                    index.is_note_tran(sup),
                    schema.version != "5.5.1" && name == "NOTE-TRAN"
                );
                assert_eq!(
                    index.is_mime(sup),
                    schema.version != "5.5.1" && name == "MIME"
                );
                let v7 = schema.version != "5.5.1";
                assert_eq!(index.is_alias(sup), v7 && name == "ALIA");
                assert_eq!(
                    index.is_graph_record(sup),
                    v7 && matches!(
                        name,
                        "record-INDI" | "record-SOUR" | "record-OBJE" | "record-SNOTE"
                    ),
                    "{name}"
                );
            }
            assert!(index.cell(DATASET, 200).is_none());
        }
        for (i, tag) in STANDARD_TAGS.iter().enumerate() {
            let t = SPEC_TAGS[i];
            assert_eq!(
                (t != u8::MAX).then_some(t),
                crate::spec::schema::tag_index(tag)
            );
        }
    }
}
