//! `oxidged date` and `oxidged age`: a payload read in every version's
//! grammar, written in each, and a date converted between calendars.

use std::fmt::Write as _;

use oxidged::GedcomVersion;
use oxidged::value::{AgeValue, Calendar, CalendarDate, DateValue};
use serde_json::{Value, json};

use crate::report::Out;

const VERSIONS: [GedcomVersion; 3] = [
    GedcomVersion::V5_5_1,
    GedcomVersion::V7_0,
    GedcomVersion::V7_1,
];

/// Whether `parse` accepts the text in each version, and how each writes
/// the lenient reading.
fn versions(
    text: &mut String,
    check: impl Fn(GedcomVersion) -> Result<(), String>,
    written: impl Fn(GedcomVersion) -> String,
) -> Value {
    let mut out = serde_json::Map::new();
    for v in VERSIONS {
        let ok = check(v);
        let w = written(v);
        match &ok {
            Ok(()) => {
                let _ = writeln!(text, "GEDCOM {v}: valid; written {w}");
            }
            Err(e) => {
                let _ = writeln!(text, "GEDCOM {v}: not valid ({e}); written {w}");
            }
        }
        out.insert(
            v.to_string(),
            json!({ "valid": ok.is_ok(), "error": ok.err(), "written": w }),
        );
    }
    Value::Object(out)
}

/// The calendar dates of a date value.
fn dates(value: &DateValue) -> Vec<&CalendarDate> {
    match value {
        DateValue::Date(d)
        | DateValue::Approximated(_, d)
        | DateValue::Before(d)
        | DateValue::After(d) => vec![d],
        DateValue::Between(a, b) => vec![a, b],
        DateValue::Period(p) => p.from.iter().chain(p.to.iter()).collect(),
        DateValue::Interpreted { date, .. } => vec![date],
        _ => Vec::new(),
    }
}

/// A calendar named on the command line.
pub fn calendar_named(name: &str) -> Option<Calendar> {
    Some(
        match name
            .to_ascii_lowercase()
            .replace(['-', '_', ' '], "")
            .as_str()
        {
            "gregorian" => Calendar::Gregorian,
            "julian" => Calendar::Julian,
            "hebrew" => Calendar::Hebrew,
            "french" | "frenchr" | "frenchrepublican" => Calendar::FrenchRepublican,
            _ => return None,
        },
    )
}

/// A date payload: how it reads, in which versions it is valid, how each
/// writes it, and each of its dates in other calendars.
pub fn date(input: &str, calendars: &[Calendar]) -> Out {
    let value = DateValue::parse(input);
    let mut text = format!("{input}\n  reads as: {value:?}\n");
    let versions = versions(
        &mut text,
        |v| {
            DateValue::parse_strict(input, v)
                .map(drop)
                .map_err(|e| e.to_string())
        },
        |v| value.to_gedcom(v),
    );
    let mut conversions = Vec::new();
    for d in dates(&value) {
        let mut entry = serde_json::Map::new();
        entry.insert("date".into(), json!(d.to_gedcom(GedcomVersion::V7_0)));
        let written = d.to_gedcom(GedcomVersion::V7_0);
        let jdn = match d.to_julian_day_number() {
            Ok(jdn) => jdn,
            Err(e) => {
                // A partial date has no day to convert.
                let _ = writeln!(text, "  {written}: not converted ({e})");
                entry.insert("error".into(), json!(e.to_string()));
                conversions.push(Value::Object(entry));
                continue;
            }
        };
        let weekday = d.weekday().map(|w| format!("{w:?}"));
        let _ = writeln!(
            text,
            "  {written}: julian day {jdn}{}",
            weekday
                .as_ref()
                .map(|w| format!(", {w}"))
                .unwrap_or_default()
        );
        entry.insert("julian_day".into(), json!(jdn));
        entry.insert("weekday".into(), json!(weekday));
        let mut converted = serde_json::Map::new();
        for c in calendars {
            if c == &d.calendar {
                continue;
            }
            match d.convert_to(c) {
                Ok(other) => {
                    let w = other.to_gedcom(GedcomVersion::V7_0);
                    let _ = writeln!(text, "    {}: {w}", c.gedcom7_tag());
                    converted.insert(c.gedcom7_tag().into_owned(), json!(w));
                }
                Err(e) => {
                    let _ = writeln!(text, "    {}: not converted ({e})", c.gedcom7_tag());
                    converted.insert(c.gedcom7_tag().into_owned(), Value::Null);
                }
            }
        }
        entry.insert("calendars".into(), Value::Object(converted));
        conversions.push(Value::Object(entry));
    }
    let json = json!({
        "input": input,
        "value": serde_json::to_value(&value).unwrap_or(Value::Null),
        "versions": versions,
        "dates": conversions,
    });
    Out { text, json }
}

/// An age payload: how it reads, in which versions it is valid and how
/// each writes it.
pub fn age(input: &str) -> Out {
    let value = AgeValue::parse(input);
    let mut text = format!("{input}\n  reads as: {value:?}\n");
    let versions = versions(
        &mut text,
        |v| {
            AgeValue::parse_strict(input, v)
                .map(drop)
                .map_err(|e| e.to_string())
        },
        |v| value.to_gedcom(v),
    );
    let json = json!({
        "input": input,
        "value": serde_json::to_value(&value).unwrap_or(Value::Null),
        "versions": versions,
    });
    Out { text, json }
}
