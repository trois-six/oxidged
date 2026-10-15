//! The specification tables, read from the crate's own generated files
//! (`src/spec/tables.rs`, with the types of `src/spec/schema.rs`), and laid
//! out flat for the suite: one [`Sub`] row per permitted substructure, the
//! payload type of each structure type, the enumeration sets and the
//! calendars.
//!
//! Only the facts are shared with the crate. The checker and the probe
//! generator that read them are the suite's own code, independent of the
//! crate's validator (`src/spec/validate.rs`).

use std::sync::LazyLock;

use super::schema::{DATASET, Kind, Schema, StructId};
use super::tables;

/// One permitted substructure: `tag` of type `ty` under a structure of type
/// `sup` (`""` for records), with `min..=max` occurrences (`max == 0`: no
/// upper bound).
#[derive(Clone, Copy, Debug)]
pub struct Sub {
    pub sup: &'static str,
    pub tag: &'static str,
    pub ty: &'static str,
    pub min: u8,
    pub max: u8,
}

/// The payload type of a structure type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pay {
    None,
    /// `Y` or empty.
    Y,
    Text,
    Int,
    /// Pointer to a record of the given structure type (7.0) or tag (5.5.1).
    Ptr(&'static str),
    /// A value of the given enumeration set.
    Enum(&'static str),
    ListEnum(&'static str),
    ListText,
    Date,
    DateExact,
    DatePeriod,
    Age,
    Time,
    Lang,
    MediaType,
    FilePath,
    Name,
    Uri,
    Lat,
    Long,
    TagDef,
}

/// A calendar with its month tags and epoch markers.
#[derive(Clone, Copy, Debug)]
pub struct Cal {
    pub tag: &'static str,
    pub months: &'static [&'static str],
    pub epochs: &'static [&'static str],
}

/// The tables of one version. `enums` rows are (set, values, open): an open
/// set also admits user-defined values.
#[derive(Debug)]
pub struct Spec {
    pub version: &'static str,
    pub subs: &'static [Sub],
    pub payloads: &'static [(&'static str, Pay)],
    pub enums: &'static [(&'static str, &'static [&'static str], bool)],
    pub calendars: &'static [Cal],
}

pub static V551: LazyLock<Spec> = LazyLock::new(|| flat(&tables::V551));
pub static V70: LazyLock<Spec> = LazyLock::new(|| flat(&tables::V70));
pub static V71: LazyLock<Spec> = LazyLock::new(|| flat(&tables::V71));

fn ids(schema: &Schema) -> impl Iterator<Item = StructId> + use<> {
    (0..schema.structs.len()).filter_map(|i| StructId::try_from(i).ok())
}

/// The flat view of a schema. Built once per version and kept for the
/// whole run.
fn flat(schema: &'static Schema) -> Spec {
    let v7 = schema.version.starts_with('7');
    let mut subs = Vec::new();
    let mut payloads = Vec::new();
    for id in ids(schema) {
        for s in schema.subs(id) {
            subs.push(Sub {
                sup: schema.name(id),
                tag: schema.tag(s.id),
                ty: schema.name(s.id),
                min: s.min,
                max: s.max,
            });
        }
        if id == DATASET {
            continue;
        }
        let (kind, arg) = schema.kind(id);
        let set = || schema.enum_set(arg).map_or("", |e| e.name);
        let pay = match kind {
            Kind::None => Pay::None,
            Kind::Y => Pay::Y,
            Kind::Text => Pay::Text,
            Kind::ListText => Pay::ListText,
            Kind::Int => Pay::Int,
            Kind::Name => Pay::Name,
            Kind::Date => Pay::Date,
            Kind::DateExact => Pay::DateExact,
            Kind::DatePeriod => Pay::DatePeriod,
            Kind::Age => Pay::Age,
            Kind::Time => Pay::Time,
            Kind::Lang => Pay::Lang,
            Kind::MediaType => Pay::MediaType,
            Kind::FilePath => Pay::FilePath,
            Kind::Uri => Pay::Uri,
            Kind::Lat => Pay::Lat,
            Kind::Long => Pay::Long,
            Kind::TagDef => Pay::TagDef,
            // 7.x pointers name the record type, 5.5.1 pointers the record tag.
            Kind::Pointer | Kind::NullablePointer => Pay::Ptr(if v7 {
                schema.name(arg)
            } else {
                schema.tag(arg)
            }),
            Kind::Enum => Pay::Enum(set()),
            Kind::ListEnum => Pay::ListEnum(set()),
        };
        payloads.push((schema.name(id), pay));
    }
    let enums = schema
        .enums
        .iter()
        .map(|e| (e.name, e.values, e.open))
        .collect::<Vec<_>>();
    let calendars = schema
        .calendars
        .iter()
        .map(|c| Cal {
            tag: c.tag,
            months: c.months,
            epochs: c.epochs,
        })
        .collect::<Vec<_>>();
    Spec {
        version: schema.version,
        subs: subs.leak(),
        payloads: payloads.leak(),
        enums: enums.leak(),
        calendars: calendars.leak(),
    }
}
