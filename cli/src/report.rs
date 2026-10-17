//! What the reading commands print: the summary of a file, its
//! deviations, its producer, the profiles, records, name searches and
//! relatives, as text or JSON.

use std::fmt::Write as _;

use oxidged::model::{Dataset, Event, Individual, RecordRef, Relative, Sex, View};
use oxidged::profile::{self, Detection};
use oxidged::spec::Deviation;
use oxidged::{GedcomVersion, ReadReport};
use serde_json::{Value, json};

use crate::io::Loaded;

/// A text and its JSON, one of which is printed.
pub struct Out {
    pub text: String,
    pub json: Value,
}

impl Out {
    pub fn print(&self, json: bool) -> std::io::Result<()> {
        use std::io::Write as _;
        let mut out = std::io::stdout().lock();
        if json {
            let text = serde_json::to_string_pretty(&self.json).unwrap_or_default();
            writeln!(out, "{text}")?;
        } else {
            out.write_all(self.text.as_bytes())?;
        }
        out.flush()
    }
}

/// The identifier of a record, or a placeholder.
fn xref_text(data: &Dataset, id: Option<oxidged::model::XrefId>) -> String {
    id.map_or_else(
        || "(no identifier)".to_string(),
        |x| data.store().xref(x).to_string(),
    )
}

fn detection_json(d: &Detection) -> Value {
    json!({
        "id": d.profile().id(),
        "label": d.profile().label(),
        "version": d.producer_version().map(|v| v.to_string()),
        "score": d.score(),
        "known": d.is_known(),
        "evidence": d.evidence(),
        "hints": d.hints().iter().map(|h| h.id()).collect::<Vec<_>>(),
    })
}

pub fn deviation_json(d: &Deviation) -> Value {
    json!({ "line": d.line, "kind": format!("{:?}", d.kind), "detail": d.detail.to_string() })
}

/// The record counts of a dataset, by type.
fn counts(data: &Dataset) -> [(&'static str, &'static str, usize); 9] {
    [
        ("individuals", "individuals", data.individuals().len()),
        ("families", "families", data.families().len()),
        ("sources", "sources", data.sources().len()),
        ("repositories", "repositories", data.repositories().len()),
        ("multimedia objects", "multimedia", data.multimedia().len()),
        ("shared notes", "notes", data.notes().len()),
        ("submitters", "submitters", data.submitters().len()),
        ("submissions", "submissions", data.submissions().len()),
        ("other records", "other", data.extra().len()),
    ]
}

/// The version, encoding, producer, records and pointers of a dataset,
/// and what reading found.
pub fn info(loaded: &Loaded, mappings: bool) -> Out {
    let data = &loaded.data;
    let report: Option<&ReadReport> = loaded.report.as_ref();
    let mut text = String::new();
    let declared = data
        .declared_version()
        .map(|d| format!(" (declared {d})"))
        .unwrap_or_default();
    let _ = writeln!(text, "GEDCOM {}{declared}", data.version());
    let _ = writeln!(text, "format: {}", format_name(loaded));
    if let Some(r) = report {
        let encoding = r.encoding.map(|e| e.to_string());
        let _ = write!(text, "encoding: {}", encoding.as_deref().unwrap_or("text"));
        if let Some(c) = &r.declared_charset {
            let _ = write!(text, " (declared {c})");
        }
        text.push('\n');
        let d = &r.detection;
        let _ = write!(
            text,
            "producer: {} ({})",
            d.profile().id(),
            d.profile().label()
        );
        if let Some(v) = d.producer_version() {
            let _ = write!(text, " {v}");
        }
        text.push('\n');
    }
    for (name, _, count) in counts(data) {
        let _ = writeln!(text, "{name}: {count}");
    }
    if !loaded.media.is_empty() {
        let _ = writeln!(text, "media files: {}", loaded.media.len());
    }
    let dangling = data.dangling_references();
    let _ = writeln!(text, "pointers to no record: {}", dangling.len());
    if let Some(r) = report {
        let _ = writeln!(text, "deviations: {}", r.deviations.len());
        let m = r.interpretation.mappings();
        let _ = writeln!(text, "interpretation: {} rules applied", m.len());
        if mappings {
            for m in m {
                let _ = writeln!(
                    text,
                    "  {} {} {:?} ×{} (line {}) [{}]",
                    m.profile,
                    m.tag,
                    m.action,
                    m.count,
                    m.first_line,
                    m.rows.join(", ")
                );
            }
        }
    }
    let records: serde_json::Map<String, Value> = counts(data)
        .iter()
        .map(|(_, key, n)| ((*key).to_string(), json!(n)))
        .collect();
    let json = json!({
        "version": data.version().to_string(),
        "declared_version": data.declared_version(),
        "format": format_name(loaded),
        "encoding": report.and_then(|r| r.encoding).map(|e| e.to_string()),
        "declared_charset": report.and_then(|r| r.declared_charset.clone()),
        "producer": report.map(|r| detection_json(&r.detection)),
        "records": records,
        "media_files": loaded.media.len(),
        "dangling_pointers": dangling
            .iter()
            .map(|d| json!({
                "record": d.record.map(|x| data.store().xref(x).to_string()),
                "tag": d.tag.as_str(),
                "pointer": data.store().xref(d.pointer),
            }))
            .collect::<Vec<_>>(),
        "deviations": report.map(|r| r.deviations.len()),
        "interpretation": report.map(|r| r.interpretation.mappings().iter().map(|m| json!({
            "profile": m.profile,
            "tag": m.tag.as_ref(),
            "context": m.context,
            "action": format!("{:?}", m.action),
            "count": m.count,
            "first_line": m.first_line,
            "rows": m.rows,
        })).collect::<Vec<_>>()),
    });
    Out { text, json }
}

fn format_name(loaded: &Loaded) -> &'static str {
    match loaded.format {
        crate::io::Format::Ged => "GEDCOM",
        crate::io::Format::Gdz => "GEDZIP",
        crate::io::Format::Json => "JSON",
    }
}

/// Every deviation of a file from its specification.
pub fn validate(deviations: &[Deviation]) -> Out {
    let mut text = String::new();
    for d in deviations {
        let _ = writeln!(text, "{d}");
    }
    match deviations.len() {
        0 => text.push_str("conformant\n"),
        1 => text.push_str("1 deviation\n"),
        n => {
            let _ = writeln!(text, "{n} deviations");
        }
    }
    let json = json!({
        "conformant": deviations.is_empty(),
        "deviations": deviations.iter().map(deviation_json).collect::<Vec<_>>(),
    });
    Out { text, json }
}

/// The program that produced a file.
pub fn detect(d: &Detection) -> Out {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "producer: {} ({})",
        d.profile().id(),
        d.profile().label()
    );
    if let Some(v) = d.producer_version() {
        let _ = writeln!(text, "version: {v}");
    }
    let _ = writeln!(
        text,
        "score: {}{}",
        d.score(),
        if d.is_known() {
            ""
        } else {
            " (not recognised)"
        }
    );
    for e in d.evidence() {
        let _ = writeln!(text, "evidence: {e}");
    }
    for h in d.hints() {
        let _ = writeln!(text, "hint: {} ({})", h.id(), h.label());
    }
    Out {
        text,
        json: detection_json(d),
    }
}

/// The import and export profiles, or one of them.
pub fn profiles(id: Option<&str>) -> Option<Out> {
    let Some(id) = id else {
        let mut text = String::from("Producers (read, --producer):\n");
        for p in profile::import_profiles() {
            let _ = writeln!(text, "  {:<20} tier {}  {}", p.id(), p.tier(), p.label());
        }
        text.push_str("Importers (export --profile):\n");
        for p in profile::export_profiles() {
            let _ = writeln!(
                text,
                "  {:<20} tier {}  {}, GEDCOM {}",
                p.id(),
                p.tier(),
                p.label(),
                p.target().version()
            );
        }
        let json = json!({
            "producers": profile::import_profiles().iter().map(|p| json!({
                "id": p.id(), "label": p.label(), "tier": p.tier(),
            })).collect::<Vec<_>>(),
            "importers": profile::export_profiles().iter().map(|p| json!({
                "id": p.id(), "label": p.label(), "tier": p.tier(),
                "version": p.target().version().to_string(),
            })).collect::<Vec<_>>(),
        });
        return Some(Out { text, json });
    };
    let import = profile::profile(id);
    let export = profile::export_profile(id);
    if import.is_none() && export.is_none() {
        return None;
    }
    let mut text = String::new();
    let mut json = serde_json::Map::new();
    if let Some(p) = import {
        let _ = writeln!(text, "producer {} ({})", p.id(), p.label());
        let _ = writeln!(text, "  tier: {}", p.tier());
        let _ = writeln!(text, "  verification: {:?}", p.verification());
        if let Some(parent) = p.parent() {
            let _ = writeln!(text, "  inherits: {}", parent.id());
        }
        let _ = writeln!(text, "  rules: {}", p.rule_count());
        let _ = writeln!(text, "  catalogue rows: {}", p.rows().join(", "));
        let tags: Vec<_> = p.documented_tags().collect();
        for t in &tags {
            let _ = writeln!(text, "  extension {} in {}: {}", t.tag, t.context, t.uri);
        }
        json.insert(
            "producer".into(),
            json!({
                "id": p.id(),
                "label": p.label(),
                "tier": p.tier(),
                "verification": format!("{:?}", p.verification()),
                "parent": p.parent().map(|p| p.id()),
                "rules": p.rule_count(),
                "rows": p.rows(),
                "extensions": tags.iter().map(|t| json!({
                    "tag": t.tag, "context": t.context, "uri": t.uri, "registered": t.registered,
                })).collect::<Vec<_>>(),
            }),
        );
    }
    if let Some(p) = export {
        let t = p.target();
        let _ = writeln!(text, "importer {} ({})", p.id(), p.label());
        let _ = writeln!(text, "  tier: {}", p.tier());
        let _ = writeln!(text, "  verification: {:?}", p.verification());
        let _ = writeln!(text, "  writes: GEDCOM {}, {}", t.version(), t.encoding());
        let _ = writeln!(
            text,
            "  byte order mark: {}",
            if t.bom() { "yes" } else { "no" }
        );
        if let Some(max) = t.max_bytes() {
            let _ = writeln!(text, "  largest file: {max} bytes");
        }
        if let Some(max) = t.max_individuals() {
            let _ = writeln!(text, "  most individuals: {max}");
        }
        let _ = writeln!(text, "  catalogue rows: {}", p.rows().join(", "));
        json.insert(
            "importer".into(),
            json!({
                "id": p.id(),
                "label": p.label(),
                "tier": p.tier(),
                "verification": format!("{:?}", p.verification()),
                "version": t.version().to_string(),
                "encoding": t.encoding().to_string(),
                "bom": t.bom(),
                "max_bytes": t.max_bytes(),
                "max_individuals": t.max_individuals(),
                "rows": p.rows(),
            }),
        );
    }
    Some(Out {
        text,
        json: Value::Object(json),
    })
}

/// An event's date and place.
fn event_json(event: Option<View<'_, Event>>) -> Value {
    event.map_or(Value::Null, |e| {
        json!({
            "date": e.date().map(|d| d.value().to_string()),
            "place": e.place().map(|p| p.name().to_string()),
        })
    })
}

fn event_text(label: &str, event: View<'_, Event>) -> String {
    let mut line = format!("  {label}:");
    if let Some(date) = event.date() {
        let _ = write!(line, " {}", date.value());
    }
    if let Some(place) = event.place() {
        let _ = write!(line, ", {}", place.name());
    }
    line.push('\n');
    line
}

fn sex(individual: View<'_, Individual>) -> Option<&'static str> {
    individual.sex().map(|s| match s {
        Sex::Male => "male",
        Sex::Female => "female",
        _ => "other or unknown",
    })
}

/// A one-line label for an individual: identifier, name and years.
pub fn label(data: &Dataset, individual: View<'_, Individual>) -> String {
    let mut line = format!(
        "{} {}",
        xref_text(data, individual.xref()),
        individual.full_name().unwrap_or_else(|| "(no name)".into())
    );
    let year = |e: Option<View<'_, Event>>| {
        e.and_then(|e| e.date())
            .map(|d| d.value().to_string())
            .unwrap_or_default()
    };
    let (born, died) = (year(individual.birth()), year(individual.death()));
    if !born.is_empty() || !died.is_empty() {
        let _ = write!(line, " ({born} – {died})");
    }
    line
}

/// The summary of an individual as JSON.
pub fn individual_json(data: &Dataset, individual: View<'_, Individual>) -> Value {
    json!({
        "xref": individual.xref().map(|x| data.store().xref(x)),
        "name": individual.full_name(),
        "names": individual.names().into_iter().map(|n| n.full()).collect::<Vec<_>>(),
        "sex": sex(individual),
        "birth": event_json(individual.birth()),
        "death": event_json(individual.death()),
    })
}

/// An individual: names, sex, birth and death, parents, partners and
/// children.
pub fn describe(data: &Dataset, individual: View<'_, Individual>) -> (String, Value) {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "{} {}",
        xref_text(data, individual.xref()),
        individual.full_name().unwrap_or_else(|| "(no name)".into())
    );
    for other in individual.names().iter().skip(1) {
        let _ = writeln!(text, "  also: {}", other.full());
    }
    if let Some(sex) = sex(individual) {
        let _ = writeln!(text, "  sex: {sex}");
    }
    if let Some(birth) = individual.birth() {
        text.push_str(&event_text("born", birth));
    }
    if let Some(death) = individual.death() {
        text.push_str(&event_text("died", death));
    }
    let mut json = individual_json(data, individual);
    let mut parents_json = Vec::new();
    for family in data.families_as_child(individual.xref()) {
        let parents: Vec<_> = data.parents(family).collect();
        if !parents.is_empty() {
            let names: Vec<String> = parents.iter().map(|p| label(data, *p)).collect();
            let _ = writeln!(text, "  child of: {}", names.join(" and "));
        }
        parents_json.push(json!({
            "family": xref_text(data, family.xref()),
            "parents": parents.iter().map(|p| individual_json(data, *p)).collect::<Vec<_>>(),
        }));
    }
    let mut unions_json = Vec::new();
    for family in data.families_as_spouse(individual.xref()) {
        let spouse = data.spouse(individual.xref(), family);
        if let Some(spouse) = spouse {
            let _ = writeln!(text, "  partner: {}", label(data, spouse));
        }
        let children: Vec<_> = data.children(family).collect();
        for child in &children {
            let _ = writeln!(text, "  child: {}", label(data, *child));
        }
        unions_json.push(json!({
            "family": xref_text(data, family.xref()),
            "partner": spouse.map(|s| individual_json(data, s)),
            "children": children.iter().map(|c| individual_json(data, *c)).collect::<Vec<_>>(),
        }));
    }
    if let Value::Object(m) = &mut json {
        m.insert("child_of".into(), Value::Array(parents_json));
        m.insert("unions".into(), Value::Array(unions_json));
    }
    (text, json)
}

/// A record as GEDCOM lines of the dataset's version.
pub fn gedcom_of(record: RecordRef<'_>, version: GedcomVersion) -> String {
    record.to_structure_in(version).to_gedcom(0, version)
}

/// The kind of a record, in words.
pub fn kind_name(record: RecordRef<'_>) -> &'static str {
    match record {
        RecordRef::Header(_) => "header",
        RecordRef::Individual(_) => "individual",
        RecordRef::Family(_) => "family",
        RecordRef::Source(_) => "source",
        RecordRef::Repository(_) => "repository",
        RecordRef::Multimedia(_) => "multimedia",
        RecordRef::Submitter(_) => "submitter",
        RecordRef::Submission(_) => "submission",
        RecordRef::Note(_) => "shared note",
        _ => "other",
    }
}

/// Relatives by generation.
pub fn relatives(data: &Dataset, relatives: &[Relative<'_>]) -> Out {
    let mut text = String::new();
    for r in relatives {
        let _ = writeln!(text, "{} {}", r.generation, label(data, r.individual));
    }
    let json = Value::Array(
        relatives
            .iter()
            .map(|r| {
                let mut v = individual_json(data, r.individual);
                if let Value::Object(m) = &mut v {
                    m.insert("generation".into(), json!(r.generation));
                }
                v
            })
            .collect(),
    );
    Out { text, json }
}

/// The given names and surnames of an individual's names: their pieces,
/// or the name's words, its surname between slashes.
pub fn name_parts(individual: View<'_, Individual>) -> (Vec<String>, Vec<String>) {
    let mut given = Vec::new();
    let mut surname = Vec::new();
    for name in individual.names() {
        given.extend(name.givens().map(|g| g.to_string()));
        surname.extend(name.surnames().map(|s| s.to_string()));
        let value = name.value().to_str();
        if let Some(s) = name.surname_in_value() {
            surname.push(s);
        }
        let before = value.split('/').next().unwrap_or_default().trim();
        if !before.is_empty() {
            given.push(before.to_string());
        }
    }
    (given, surname)
}
