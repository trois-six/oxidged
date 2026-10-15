//! Probes generated from the specification tables (`support/spec_tables.rs`).
//!
//! 1. Placement (`place551/…`, `place70/…`, `place71/…`): every standard
//!    substructure, in the shortest context from a record, with its required
//!    substructures, twice when it may repeat. The probed lines must come
//!    back at their place (else LOST, CARD, CHANGED or MOVED) and the output
//!    must be conformant.
//! 2. Enumerations (`enum…/…`): every value of every set, wherever the set
//!    is used, plus an extension value and (5.5.1) a mixed-case spelling.
//! 3. Calendars (`calendar/…`): every month and epoch of every calendar, and
//!    one period, range and approximation per calendar.
//! 4. Payload types (`payload/…`): valid samples (kept verbatim) and invalid
//!    samples (kept somewhere, written conformant) per data type.
//! 5. Checker self-test: misplaced, repeated and missing structures built from
//!    the tables must be reported by `checker.rs` (no oxidged involved).
//!
//! Every generated input is itself checked conformant first, so a generator
//! or checker mistake cannot pass for a oxidged gap.

use crate::support::adapter::{self, Target as WriteTarget};
use crate::support::checker::{self, Target};
use crate::support::ratchet::{self, Failure};
use crate::support::semantic;
use crate::support::spec_tables::{Pay, Spec, Sub};
use crate::support::tree::{self, Payload};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// A structure of a generated dataset.
#[derive(Clone, Debug)]
struct N {
    tag: String,
    xref: Option<String>,
    payload: Option<String>,
    children: Vec<N>,
}

impl N {
    fn new(tag: &str, payload: Option<String>) -> Self {
        N {
            tag: tag.to_string(),
            xref: None,
            payload,
            children: Vec::new(),
        }
    }

    fn render(&self, level: usize, out: &mut String) {
        out.push_str(&level.to_string());
        if let Some(x) = &self.xref {
            out.push(' ');
            out.push_str(x);
        }
        out.push(' ');
        out.push_str(&self.tag);
        if let Some(p) = &self.payload
            && !p.is_empty()
        {
            out.push(' ');
            out.push_str(p);
        }
        out.push('\n');
        for c in &self.children {
            c.render(level + 1, out);
        }
    }
}

pub struct Gen {
    pub target: Target,
    spec: &'static Spec,
    by_sup: HashMap<&'static str, Vec<&'static Sub>>,
    pays: HashMap<&'static str, Pay>,
    /// A required (superstructure type, tag) to leave out (checker self-test).
    skip: Option<(&'static str, &'static str)>,
}

/// One generated probe.
pub struct Probe {
    pub id: String,
    pub doc: String,
    /// `path = value` lines that must come back.
    pub want: Vec<(String, String)>,
}

fn xref_prefix(tag: &str) -> &'static str {
    match tag {
        "INDI" => "I",
        "FAM" => "F",
        "SOUR" => "S",
        "REPO" => "R",
        "OBJE" => "O",
        "SNOTE" | "NOTE" => "N",
        "SUBM" => "U",
        "SUBN" => "B",
        _ => "X",
    }
}

impl Gen {
    pub fn new(target: Target) -> Self {
        let spec = target.spec();
        let mut by_sup: HashMap<&str, Vec<&Sub>> = HashMap::new();
        for s in spec.subs {
            by_sup.entry(s.sup).or_default().push(s);
        }
        for v in by_sup.values_mut() {
            v.sort_by(|a, b| (a.tag, a.ty).cmp(&(b.tag, b.ty)));
        }
        let pays = spec.payloads.iter().copied().collect();
        Gen {
            target,
            spec,
            by_sup,
            pays,
            skip: None,
        }
    }

    fn v7(&self) -> bool {
        self.target.is_v7()
    }

    pub fn subs(&self, sup: &str) -> &[&'static Sub] {
        self.by_sup.get(sup).map_or(&[], Vec::as_slice)
    }

    pub fn pay(&self, ty: &str) -> Pay {
        self.pays.get(ty).copied().unwrap_or(Pay::None)
    }

    /// The record tag a pointer type points to.
    fn record_tag(&self, rec: &str) -> &'static str {
        if self.v7() {
            self.subs("")
                .iter()
                .find(|s| s.ty == rec)
                .map_or("INDI", |s| s.tag)
        } else {
            self.subs("")
                .iter()
                .find(|s| s.tag == rec)
                .map_or("INDI", |s| s.tag)
        }
    }

    fn enum_values(&self, set: &str) -> &'static [&'static str] {
        self.spec
            .enums
            .iter()
            .find(|(s, _, _)| *s == set)
            .map_or(&[], |(_, v, _)| v)
    }

    /// A valid payload for type `ty` (tag `tag` under `parent`), distinct
    /// for `i` = 0 and 1.
    pub fn sample(&self, ty: &str, tag: &str, parent: &str, i: usize) -> Option<String> {
        let v7 = self.v7();
        let pick = |a: &str, b: &str| Some(if i == 0 { a } else { b }.to_string());
        // Values with a meaning of their own.
        match (parent, tag) {
            ("GEDC", "VERS") => return Some(self.target.vers().to_string()),
            ("GEDC", "FORM") => return Some("LINEAGE-LINKED".into()),
            ("HEAD", "CHAR") => return Some("UTF-8".into()),
            ("CHAR", "VERS") => return Some("1.0".into()),
            _ => {}
        }
        match self.pay(ty) {
            Pay::None => None,
            Pay::Y => Some("Y".into()),
            Pay::Text => Some(format!("Value {tag} {}", ["A", "B"][i % 2])),
            Pay::ListText => pick("Alpha, Beta", "Gamma, Delta"),
            Pay::Int => pick("1", "2"),
            Pay::Ptr(rec) => {
                let t = self.record_tag(rec);
                Some(format!(
                    "@{}{}@",
                    xref_prefix(t),
                    if t == "SUBN" { 1 } else { i + 1 }
                ))
            }
            Pay::Enum(set) | Pay::ListEnum(set) => {
                let vals = self.enum_values(set);
                vals.get(i % vals.len().max(1)).map(|s| (*s).to_string())
            }
            Pay::Date => pick("1 JAN 1900", "2 FEB 1901"),
            Pay::DateExact => pick("1 JAN 2000", "2 FEB 2001"),
            Pay::DatePeriod => pick("FROM 1900 TO 1910", "FROM 1920 TO 1930"),
            Pay::Age => pick("1y", "2y"),
            Pay::Time => pick("10:00:00", "11:00:00"),
            Pay::Lang if v7 => pick("en", "fr"),
            Pay::Lang => pick("English", "French"),
            // MIME gives the media type of a text (7.x §MIME).
            Pay::MediaType if tag == "MIME" => pick("text/plain", "text/html"),
            Pay::MediaType => pick("image/jpeg", "text/plain"),
            Pay::FilePath => pick("media/a.jpg", "media/b.jpg"),
            Pay::Name => pick("Ann /Sample/", "Bea /Example/"),
            Pay::Uri => pick("https://example.com/a", "https://example.com/b"),
            Pay::Lat => pick("N1.5", "S2.5"),
            Pay::Long => pick("E1.5", "W2.5"),
            Pay::TagDef => pick("_EXTA https://example.com/a", "_EXTB https://example.com/b"),
        }
    }

    /// The required substructures of a structure of type `ty`.
    fn required(&self, ty: &str, tag: &str, depth: usize) -> Vec<N> {
        let mut out: Vec<N> = Vec::new();
        if depth > 8 {
            return out;
        }
        for s in self.subs(ty) {
            if s.min >= 1 && !out.iter().any(|n| n.tag == s.tag) && self.skip != Some((ty, s.tag)) {
                let mut n = N::new(s.tag, self.sample(s.ty, s.tag, tag, 0));
                n.children = self.required(s.ty, s.tag, depth + 1);
                out.push(n);
            }
        }
        // A note translation says its language or its media type (7.x
        // §NOTE-TRAN), which the tables cannot state.
        if ty == "NOTE-TRAN" && self.skip.is_none() {
            out.push(N::new("LANG", Some("en".into())));
        }
        // A 7.x structure has a payload or a substructure (§1.2): one
        // without either (records aside) gets its first substructure but a
        // phrase.
        let record = self.subs("").iter().any(|s| s.ty == ty);
        if self.v7()
            && out.is_empty()
            && self.pay(ty) == Pay::None
            && !record
            && let Some(s) = self.subs(ty).iter().find(|s| s.tag != "PHRASE")
        {
            let mut n = N::new(s.tag, self.sample(s.ty, s.tag, tag, 0));
            n.children = self.required(s.ty, s.tag, depth + 1);
            out.push(n);
        }
        out
    }

    /// Shortest tag path from the dataset root to every structure type.
    pub fn paths(&self) -> HashMap<&'static str, Vec<(&'static str, &'static str)>> {
        let mut best: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        best.insert("", Vec::new());
        let mut q = VecDeque::from([""]);
        while let Some(cur) = q.pop_front() {
            for s in self.subs(cur) {
                if !best.contains_key(s.ty) {
                    let mut p = best[cur].clone();
                    p.push((s.tag, s.ty));
                    best.insert(s.ty, p);
                    q.push_back(s.ty);
                }
            }
        }
        best
    }

    fn head(&self) -> N {
        let head_ty = self
            .subs("")
            .iter()
            .find(|s| s.tag == "HEAD")
            .map_or("HEAD", |s| s.ty);
        let mut h = N::new("HEAD", None);
        h.children = self.required(head_ty, "HEAD", 0);
        h
    }

    /// Two records of every kind (one SUBN), each with its required parts.
    fn background(&self) -> Vec<N> {
        let mut out = Vec::new();
        for s in self.subs("") {
            if matches!(s.tag, "HEAD" | "TRLR") {
                continue;
            }
            let n = if s.max == 1 { 1 } else { 2 };
            for i in 0..n {
                let mut r = N::new(s.tag, self.sample(s.ty, s.tag, "", i));
                r.xref = Some(format!("@{}{}@", xref_prefix(s.tag), i + 1));
                r.children = self.required(s.ty, s.tag, 0);
                out.push(r);
            }
        }
        out
    }

    fn render(head: &N, records: &[N]) -> String {
        let mut s = String::new();
        head.render(0, &mut s);
        for r in records {
            r.render(0, &mut s);
        }
        s.push_str("0 TRLR\n");
        s
    }

    /// A dataset with `values` (one per instance) of `sub` under the
    /// shortest path to `sub.sup`.
    pub fn build(
        &self,
        sub: &Sub,
        values: &[Option<String>],
        paths: &HashMap<&str, Vec<(&str, &str)>>,
    ) -> Option<(String, Vec<(String, String)>)> {
        let mut head = self.head();
        let mut records = self.background();
        let mut want = Vec::new();
        if sub.sup.is_empty() {
            if matches!(sub.tag, "HEAD" | "TRLR") {
                return None;
            }
            // The SUBN record may occur once: probe it in place of the background one.
            if sub.max == 1 {
                records.retain(|r| r.tag != sub.tag);
            }
            for (i, v) in values.iter().enumerate() {
                let mut r = N::new(sub.tag, v.clone());
                let x = format!("@P{i}@");
                r.xref = Some(x.clone());
                r.children = self.required(sub.ty, sub.tag, 0);
                want.push((format!("{}{x}", sub.tag), v.clone().unwrap_or_default()));
                records.insert(i, r);
            }
            if sub.max == 1 {
                // Keep pointers to it resolving.
                let doc = Self::render(&head, &records)
                    .replace(&format!("@{}1@", xref_prefix(sub.tag)), "@P0@");
                return Some((doc, want));
            }
            return Some((Self::render(&head, &records), want));
        }
        let path = paths.get(sub.sup)?;
        let (first_tag, first_ty) = *path.first()?;
        let mut prefix: Vec<String> = Vec::new();
        let mut root = if first_tag == "HEAD" {
            prefix.push("HEAD".into());
            std::mem::replace(&mut head, N::new("HEAD", None))
        } else {
            let mut r = N::new(first_tag, self.sample(first_ty, first_tag, "", 0));
            r.xref = Some("@P0@".into());
            r.children = self.required(first_ty, first_tag, 0);
            prefix.push(format!("{first_tag}@P0@"));
            r
        };
        {
            let mut cur = &mut root;
            let mut parent_tag = first_tag;
            for &(tag, ty) in &path[1..] {
                prefix.push(tag.to_string());
                let idx = match cur.children.iter().position(|c| c.tag == tag) {
                    Some(i) => i,
                    None => {
                        let mut n = N::new(tag, self.sample(ty, tag, parent_tag, 0));
                        n.children = self.required(ty, tag, 0);
                        cur.children.push(n);
                        cur.children.len() - 1
                    }
                };
                cur = &mut cur.children[idx];
                parent_tag = tag;
            }
            let p = prefix.join("/");
            match cur.children.iter().position(|c| c.tag == sub.tag) {
                Some(i) if sub.max == 1 => {
                    cur.children[i].payload = values[0].clone();
                    want.push((
                        format!("{p}/{}", sub.tag),
                        values[0].clone().unwrap_or_default(),
                    ));
                }
                _ => {
                    for v in values {
                        let mut n = N::new(sub.tag, v.clone());
                        n.children = self.required(sub.ty, sub.tag, 0);
                        cur.children.push(n);
                        want.push((format!("{p}/{}", sub.tag), v.clone().unwrap_or_default()));
                    }
                }
            }
        }
        if first_tag == "HEAD" {
            head = root;
            return Some((Self::render(&head, &records), want));
        }
        // A record that may occur once (SUBN) replaces the background one.
        let single = self
            .subs("")
            .iter()
            .any(|s| s.tag == first_tag && s.max == 1);
        if single {
            records.retain(|r| r.tag != first_tag);
        }
        records.insert(0, root);
        let mut doc = Self::render(&head, &records);
        if single {
            doc = doc.replace(&format!("@{}1@", xref_prefix(first_tag)), "@P0@");
        }
        Some((doc, want))
    }

    pub fn family(&self, kind: &str) -> String {
        let v = match self.target {
            Target::V551 => "551",
            Target::V70 => "70",
            Target::V71 => "71",
        };
        format!("{kind}{v}")
    }

    fn write_target(&self) -> WriteTarget {
        WriteTarget::Same
    }
}

/// `path = value` lines of a stream, records keyed by tag and xref.
fn flat(text: &str) -> BTreeMap<(String, String), usize> {
    let (v, recs) = tree::parse(text);
    let mut out = BTreeMap::new();
    for r in &recs {
        let root = match &r.xref {
            Some(x) => format!("{}{x}", r.tag),
            None => r.tag.clone(),
        };
        fn go(
            n: &tree::Node,
            path: &str,
            v: tree::Version,
            out: &mut BTreeMap<(String, String), usize>,
        ) {
            let val = match &n.payload {
                Payload::None => String::new(),
                p => semantic::normalise_value(&n.tag, p.as_str(), v),
            };
            *out.entry((path.to_string(), val)).or_default() += 1;
            for c in &n.children {
                go(c, &format!("{path}/{}", c.tag), v, out);
            }
        }
        go(r, &root, v, &mut out);
    }
    out
}

/// LOST, CARD, CHANGED or MOVED for the wanted lines, or None when all came back.
fn classify(src: &str, out: &str, want: &[(String, String)]) -> Option<(&'static str, String)> {
    let o = flat(out);
    let s = flat(src);
    let mut need: BTreeMap<(String, String), usize> = BTreeMap::new();
    for w in want {
        *need.entry(w.clone()).or_default() += 1;
    }
    let total: usize = need.values().sum();
    let got: usize = need
        .iter()
        .map(|(k, n)| (*n).min(o.get(k).copied().unwrap_or(0)))
        .sum();
    if got == total {
        return None;
    }
    if got > 0 {
        return Some(("CARD", format!("{got}/{total} kept")));
    }
    let paths: Vec<&String> = want.iter().map(|w| &w.0).collect();
    let same: Vec<String> = o
        .iter()
        .filter(|((p, _), _)| paths.contains(&p))
        .map(|((_, v), _)| format!("{v:?}"))
        .collect();
    if same.len() >= total.min(1) && !same.is_empty() {
        return Some(("CHANGED", format!("now {}", same.join(", "))));
    }
    let vals: Vec<&String> = want
        .iter()
        .map(|w| &w.1)
        .filter(|v| !v.is_empty() && v.as_str() != "Y")
        .collect();
    let moved: Vec<String> = o
        .keys()
        .filter(|(p, v)| vals.contains(&v) && !s.contains_key(&(p.clone(), v.clone())))
        .map(|(p, _)| p.clone())
        .collect();
    if !moved.is_empty() {
        return Some(("MOVED", format!("to {}", moved.join(", "))));
    }
    Some((
        "LOST",
        format!(
            "{} not written",
            want.iter()
                .map(|w| format!("{} = {:?}", w.0, w.1))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    ))
}

/// Runs one probe: parse, write, re-read, classify and check the output.
fn run_probe(g: &Gen, check: &str, p: &Probe, kind_lenient: bool, failures: &mut Vec<Failure>) {
    let m = match adapter::read(p.doc.as_bytes()) {
        Ok(m) => m,
        Err(e) => {
            let class = if e.starts_with("PANIC") {
                "PANIC"
            } else {
                "FATAL"
            };
            failures.push(Failure::new(&p.id, "parse", class, e));
            return;
        }
    };
    // A valid input is read into the types the model has.
    let untyped = adapter::untyped(&m);
    if !kind_lenient && !untyped.is_empty() {
        failures.push(Failure::new(&p.id, "typed", "UNTYPED", untyped.join(", ")));
    }
    let out = match adapter::write(&m, g.write_target()) {
        Ok(o) => o,
        Err(e) => {
            failures.push(Failure::new(&p.id, "write", "FATAL", e));
            return;
        }
    };
    if kind_lenient {
        let lost = semantic::lost(&p.doc, &out);
        if !lost.is_empty() {
            failures.push(Failure::new(&p.id, check, "LOST", lost.join("; ")));
        }
    } else if let Some((class, detail)) = classify(&p.doc, &out, &p.want) {
        failures.push(Failure::new(&p.id, check, class, detail));
    }
    if let Err(e) = adapter::read(out.as_bytes()) {
        failures.push(Failure::new(&p.id, "reparse", "FATAL", e));
    }
    for i in checker::check(&out, g.target, Some(&p.doc)) {
        failures.push(Failure::new(
            &p.id,
            format!("output:{}", i.rule),
            "NONCONFORMANT",
            i.to_string(),
        ));
    }
}

fn sup_name(sup: &str) -> &str {
    if sup.is_empty() { "ROOT" } else { sup }
}

/// Every placement probe of a version.
pub fn placement_probes(g: &Gen) -> Vec<Probe> {
    let paths = g.paths();
    let fam = g.family("place");
    let mut out = Vec::new();
    let mut sups: Vec<&&str> = g.by_sup.keys().collect();
    sups.sort();
    for sup in sups {
        if !paths.contains_key(*sup) {
            continue;
        }
        for sub in g.subs(sup) {
            let n = if sub.max == 1 { 1 } else { 2 };
            let parent = sub.sup.rsplit(['-', '.']).next().unwrap_or("");
            let values: Vec<Option<String>> = (0..n)
                .map(|i| g.sample(sub.ty, sub.tag, parent, i))
                .collect();
            if let Some((doc, mut want)) = g.build(sub, &values, &paths) {
                // HEAD.CHAR describes the input bytes: the writer regenerates it.
                if parent == "CHAR" {
                    want.clear();
                }
                out.push(Probe {
                    id: format!("{fam}/{}/{}", sup_name(sub.sup), sub.ty),
                    doc,
                    want,
                });
            }
        }
    }
    out
}

/// How an enumeration probe is judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Valid input: the value comes back as written.
    Exact,
    /// Invalid input: the value is kept somewhere, the output is conformant.
    Kept,
    /// A 5.5.1 value in the wrong case: it comes back in the spec's spelling.
    Canonical,
}

/// Enumeration probes.
pub fn enum_probes(g: &Gen) -> Vec<(Probe, Mode)> {
    let paths = g.paths();
    let fam = g.family("enum");
    let mut out = Vec::new();
    for s in g.spec.subs {
        let set = match g.pay(s.ty) {
            Pay::Enum(set) | Pay::ListEnum(set) => set,
            _ => continue,
        };
        // The writer sets the character set and the form of the header from
        // its own output: they are not data to keep.
        if !paths.contains_key(s.sup)
            || matches!(s.ty, "HEADER.HEAD.CHAR" | "HEADER.HEAD.GEDC.FORM")
        {
            continue;
        }
        let (vals, open) = g
            .spec
            .enums
            .iter()
            .find(|(n, _, _)| *n == set)
            .map_or((&[][..], false), |(_, v, o)| (*v, *o));
        let mut tries: Vec<(String, Mode)> = vals
            .iter()
            .map(|v| ((*v).to_string(), Mode::Exact))
            .collect();
        tries.push((
            "_EXTVAL".into(),
            if !g.v7() && !open {
                Mode::Kept
            } else {
                Mode::Exact
            },
        ));
        if !g.v7()
            && let Some(first) = vals.first()
        {
            let mixed: String = first
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    if i == 0 {
                        c.to_ascii_uppercase()
                    } else {
                        c.to_ascii_lowercase()
                    }
                })
                .collect();
            if mixed != *first {
                tries.push((mixed, Mode::Canonical));
            }
        }
        for (v, mode) in tries {
            if let Some((doc, mut want)) = g.build(s, &[Some(v.clone())], &paths) {
                if mode == Mode::Canonical {
                    for w in &mut want {
                        w.1 = vals[0].to_string();
                    }
                }
                out.push((
                    Probe {
                        id: format!("{fam}/{}/{}/{v}", sup_name(s.sup), s.ty),
                        doc,
                        want,
                    },
                    mode,
                ));
            }
        }
    }
    out
}

fn placement(target: Target) {
    let g = Gen::new(target);
    let probes = placement_probes(&g);
    let mut failures = Vec::new();
    for p in &probes {
        run_probe(&g, "place", p, false, &mut failures);
    }
    ratchet::verify(&g.family("place"), probes.len(), failures);
}

fn enumerations(target: Target) {
    let g = Gen::new(target);
    let probes = enum_probes(&g);
    let mut failures = Vec::new();
    for (p, mode) in &probes {
        run_probe(&g, "enum", p, *mode == Mode::Kept, &mut failures);
    }
    ratchet::verify(&g.family("enum"), probes.len(), failures);
}

#[test]
fn placement_551() {
    placement(Target::V551);
}

#[test]
fn placement_70() {
    placement(Target::V70);
}

#[test]
fn placement_71() {
    placement(Target::V71);
}

#[test]
fn enumerations_551() {
    enumerations(Target::V551);
}

#[test]
fn enumerations_70() {
    enumerations(Target::V70);
}

#[test]
fn enumerations_71() {
    enumerations(Target::V71);
}

/// What the crate's validator finds in a generated input, but for one rule
/// the probes break on purpose: placing a structure alone, with neither a
/// payload nor a substructure (`1 BAPL`), which GEDCOM 7 §1.2 forbids and
/// the checker leaves to the validator.
fn validator_issues(doc: &str) -> Vec<(String, u32, String)> {
    adapter::validate(doc)
        .into_iter()
        .filter(|(_, _, detail)| !detail.ends_with("has neither a payload nor a substructure"))
        .collect()
}

/// The generator only produces conformant datasets (lenient enumeration
/// probes excepted): a probe that fails is a oxidged gap, not a test mistake.
#[test]
fn generated_inputs_are_conformant() {
    let mut bad = Vec::new();
    let mut n = 0;
    for t in [Target::V551, Target::V70, Target::V71] {
        let g = Gen::new(t);
        for p in placement_probes(&g) {
            n += 1;
            for i in checker::check(&p.doc, t, None) {
                bad.push(format!("{}: {i}", p.id));
            }
            for (rule, line, detail) in validator_issues(&p.doc) {
                bad.push(format!(
                    "{}: validator: line {line}: {rule}: {detail}",
                    p.id
                ));
            }
        }
        for (p, mode) in enum_probes(&g) {
            n += 1;
            if mode == Mode::Exact {
                for i in checker::check(&p.doc, t, None) {
                    bad.push(format!("{}: {i}", p.id));
                }
                for (rule, line, detail) in validator_issues(&p.doc) {
                    bad.push(format!(
                        "{}: validator: line {line}: {rule}: {detail}",
                        p.id
                    ));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} issues in {n} generated inputs:\n{}",
        bad.len(),
        bad[..bad.len().min(60)].join("\n")
    );
}

/// The header and trailer of a minimal conformant dataset around `body`.
fn dataset(t: Target, body: &str) -> String {
    match t {
        Target::V551 => format!(
            "0 HEAD\n1 SOUR EXAMPLE_APP\n1 SUBM @U1@\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n{body}0 @U1@ SUBM\n1 NAME Example Submitter\n0 TRLR\n"
        ),
        _ => format!("0 HEAD\n1 GEDC\n2 VERS {}\n{body}0 TRLR\n", t.vers()),
    }
}

fn short(t: Target) -> &'static str {
    match t {
        Target::V551 => "551",
        Target::V70 => "70",
        Target::V71 => "71",
    }
}

/// Every month and epoch of every calendar, and the period, range and
/// approximation forms, as `INDI.BIRT.DATE`.
pub fn calendar_values(t: Target) -> Vec<(String, String)> {
    let spec = t.spec();
    let mut v: Vec<(String, String)> = Vec::new();
    for c in spec.calendars {
        let (prefix, key) = if t == Target::V551 {
            (format!("@#D{}@ ", c.tag), c.tag.replace(' ', "_"))
        } else {
            (format!("{} ", c.tag), c.tag.to_string())
        };
        if c.months.is_empty() {
            if c.tag == "UNKNOWN" {
                v.push((format!("{key}/year"), format!("{prefix}1900")));
            }
            continue;
        }
        for m in c.months {
            v.push((format!("{key}/{m}"), format!("{prefix}1 {m} 1900")));
        }
        for e in c.epochs {
            v.push((
                format!("{key}/epoch"),
                format!("{prefix}1900 {e}").replace(" B.C.", " B.C."),
            ));
        }
        let d1 = format!("{prefix}1900");
        let d2 = format!("{prefix}1910");
        for (k, f) in [
            ("from-to", format!("FROM {d1} TO {d2}")),
            ("to", format!("TO {d2}")),
            ("between", format!("BET {d1} AND {d2}")),
            ("before", format!("BEF {d1}")),
            ("after", format!("AFT {d1}")),
            ("about", format!("ABT {d1}")),
            ("calculated", format!("CAL {d1}")),
            ("estimated", format!("EST {d1}")),
        ] {
            v.push((format!("{key}/{k}"), f));
        }
    }
    if t == Target::V551 {
        v.push(("GREGORIAN/dual-year".into(), "1 JAN 1699/00".into()));
        v.push(("interpreted".into(), "INT 1900 (about then)".into()));
        v.push(("phrase".into(), "(sometime in spring)".into()));
    } else {
        v.push(("extension".into(), "_CAL 1 _MON 1900".into()));
    }
    v
}

#[test]
fn calendars() {
    let mut failures = Vec::new();
    let mut n = 0;
    for t in [Target::V551, Target::V70] {
        let g = Gen::new(t);
        for (key, value) in calendar_values(t) {
            n += 1;
            let doc = dataset(t, &format!("0 @I1@ INDI\n1 BIRT\n2 DATE {value}\n"));
            let want = semantic::normalise_value("DATE", &value, t.grammar());
            let p = Probe {
                id: format!("calendar/{}/{key}", short(t)),
                doc,
                want: vec![("INDI@I1@/BIRT/DATE".into(), want)],
            };
            run_probe(&g, "date", &p, false, &mut failures);
        }
    }
    ratchet::verify("calendar", n, failures);
}

/// (kind, body with `{}` for the value, path of the value, valid, invalid).
type Samples = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

/// Payload samples per data type, after the intents of the
/// ArmidaleSoftware/gedcom7 validator tests (valid and invalid lists).
pub fn payload_samples(t: Target) -> Vec<Samples> {
    if t == Target::V551 {
        return vec![
            (
                "age",
                "0 @I1@ INDI\n1 DEAT\n2 AGE {}\n",
                "INDI@I1@/DEAT/AGE",
                &[
                    "79y",
                    "1y 2m 3d",
                    "<1y",
                    "> 79y",
                    "CHILD",
                    "INFANT",
                    "STILLBORN",
                    "2m",
                    "30d",
                ],
                &["79", "1w", "about ten"],
            ),
            (
                "date",
                "0 @I1@ INDI\n1 DEAT\n2 DATE {}\n",
                "INDI@I1@/DEAT/DATE",
                &[
                    "1 JAN 1900",
                    "JAN 1900",
                    "1900",
                    "1699/00",
                    "1 JAN 1699/00",
                    "44 B.C.",
                    "@#DJULIAN@ 1 JAN 1700",
                    "@#DHEBREW@ 1 TSH 5000",
                    "@#DFRENCH R@ 1 VEND 1",
                    "INT 1900 (about then)",
                    "(sometime)",
                    "ABT 1900",
                    "BET 1900 AND 1910",
                    "FROM 1900 TO 1910",
                ],
                &[
                    "1 jan 1900",
                    "1900 BCE",
                    "BET 1900",
                    "1699/2000",
                    "JULIAN 1700",
                    "25/3/1934",
                ],
            ),
            (
                "time",
                "0 @I1@ INDI\n1 CHAN\n2 DATE 1 JAN 2000\n3 TIME {}\n",
                "INDI@I1@/CHAN/DATE/TIME",
                &["13:57:24.80", "10:00"],
                &["25:00", "10h00"],
            ),
            (
                "latitude",
                "0 @I1@ INDI\n1 BIRT\n2 PLAC Sampleton\n3 MAP\n4 LATI {}\n4 LONG E1.5\n",
                "INDI@I1@/BIRT/PLAC/MAP/LATI",
                &["N18.150944", "S0.5"],
                &["18.15", "N 18"],
            ),
            (
                "pedigree",
                "0 @I1@ INDI\n1 FAMC @F1@\n2 PEDI {}\n0 @F1@ FAM\n1 CHIL @I1@\n",
                "INDI@I1@/FAMC/PEDI",
                &["adopted", "birth", "foster", "sealing"],
                &["stepchild"],
            ),
        ];
    }
    vec![
        (
            "age",
            "0 @I1@ INDI\n1 DEAT\n2 AGE {}\n",
            "INDI@I1@/DEAT/AGE",
            &[
                "79y",
                "79y 1d",
                "79y 1w",
                "79y 1w 1d",
                "79y 1m",
                "79y 1m 1d",
                "79y 1m 1w",
                "79y 1m 1w 1d",
                "79m",
                "1m 1d",
                "1m 1w",
                "1m 1w 1d",
                "79w",
                "79w 1d",
                "79d",
                "> 79y",
                "< 79y 1m 1w 1d",
            ],
            &[
                "invalid",
                "d",
                "79",
                "1d 1m",
                "<>1y",
                ">79y",
                "<79y 1m 1w 1d",
            ],
        ),
        (
            "date",
            "0 @I1@ INDI\n1 DEAT\n2 DATE {}\n",
            "INDI@I1@/DEAT/DATE",
            &[
                "1 JAN 2000",
                "JAN 2000",
                "2000",
                "2000 BCE",
                "ABT 2000",
                "CAL 2000",
                "EST 2000",
                "BEF 2000",
                "AFT 2000",
                "BET 1999 AND 2000",
                "FROM 1999",
                "TO 2000",
                "FROM 1999 TO 2000",
                "JULIAN 1 JAN 1700",
                "HEBREW 1 TSH 5000",
                "FRENCH_R 1 VEND 1",
                "_CAL 1 _M 2000",
            ],
            &[
                "1 jan 2000",
                "BET 2000",
                "1999/00",
                "BET 1999 TO 2000",
                "JAN",
                "@#DJULIAN@ 1700",
                "ABT ABT 2000",
            ],
        ),
        (
            "date-period",
            "0 @S1@ SOUR\n1 DATA\n2 EVEN MARR\n3 DATE {}\n",
            "SOUR@S1@/DATA/EVEN/DATE",
            &["FROM 1900", "TO 1910", "FROM 1900 TO 1910"],
            &["1900", "BET 1900 AND 1910", "ABT 1900"],
        ),
        (
            "date-exact",
            "0 @I1@ INDI\n1 CHAN\n2 DATE {}\n",
            "INDI@I1@/CHAN/DATE",
            &["1 JAN 2000", "31 DEC 1999"],
            &["JAN 2000", "2000", "ABT 1 JAN 2000", "1 Jan 2000"],
        ),
        (
            "time",
            "0 @I1@ INDI\n1 CHAN\n2 DATE 1 JAN 2000\n3 TIME {}\n",
            "INDI@I1@/CHAN/DATE/TIME",
            &["02:50", "2:50:00.00Z", "23:59:59", "0:00"],
            &["24:00:00", "2:5", "2:60", "2:50 PM"],
        ),
        (
            "language",
            "0 @I1@ INDI\n1 NOTE Text\n2 LANG {}\n",
            "INDI@I1@/NOTE/LANG",
            &["und", "mul", "en", "en-US", "und-Latn-pinyin"],
            &["-", "und-", "-und", "en US"],
        ),
        (
            "file-path",
            "0 @O1@ OBJE\n1 FILE {}\n2 FORM image/jpeg\n",
            "OBJE@O1@/FILE",
            &[
                "media/filename",
                "http://www.example.com/path/filename",
                "file://host.example.com/path/to/file",
                "file:///path/to/file",
            ],
            &[
                "http://www.example.com/path???/file name",
                "c:\\directory\\filename",
                "http:\\\\host/path/file",
            ],
        ),
        (
            "media-type",
            "0 @O1@ OBJE\n1 FILE media/a\n2 FORM {}\n",
            "OBJE@O1@/FILE/FORM",
            &["text/plain", "image/jpeg", "application/x-example"],
            &["invalid media type", "text/", "/text", "text"],
        ),
        (
            "name",
            "0 @I1@ INDI\n1 NAME {}\n",
            "INDI@I1@/NAME",
            &["John /Smith/ Jr.", "Ann", "/Example/"],
            &["/", "a/b/c/d"],
        ),
        (
            "integer",
            "0 @F1@ FAM\n1 NCHI {}\n",
            "FAM@F1@/NCHI",
            &["0", "12"],
            &["-1", "two", "1.5"],
        ),
        (
            "y-or-null",
            "0 @I1@ INDI\n1 BIRT {}\n",
            "INDI@I1@/BIRT",
            &["Y"],
            &["N", "yes"],
        ),
        (
            "latitude",
            "0 @I1@ INDI\n1 BIRT\n2 PLAC Sampleton\n3 MAP\n4 LATI {}\n4 LONG E1.5\n",
            "INDI@I1@/BIRT/PLAC/MAP/LATI",
            &["N12.5", "S0", "N90"],
            &["12.5", "N91", "N 12"],
        ),
        (
            "longitude",
            "0 @I1@ INDI\n1 BIRT\n2 PLAC Sampleton\n3 MAP\n4 LATI N1.5\n4 LONG {}\n",
            "INDI@I1@/BIRT/PLAC/MAP/LONG",
            &["E180", "W0.25"],
            &["E181", "0.25"],
        ),
        (
            "enum",
            "0 @I1@ INDI\n1 SEX {}\n",
            "INDI@I1@/SEX",
            &["M", "F", "X", "U"],
            &["male", "m"],
        ),
        (
            "list-enum",
            "0 @I1@ INDI\n1 RESN {}\n",
            "INDI@I1@/RESN",
            &["CONFIDENTIAL", "LOCKED", "PRIVACY", "CONFIDENTIAL, LOCKED"],
            &["SECRET"],
        ),
        (
            "uri",
            "0 @I1@ INDI\n1 EXID 123\n2 TYPE {}\n",
            "INDI@I1@/EXID/TYPE",
            &["https://example.com/id"],
            &["not a uri"],
        ),
    ]
}

#[test]
fn payload_types() {
    let mut failures = Vec::new();
    let mut n = 0;
    for t in [Target::V551, Target::V70] {
        let g = Gen::new(t);
        for (kind, body, path, valid, invalid) in payload_samples(t) {
            for (i, (value, ok)) in valid
                .iter()
                .map(|v| (*v, true))
                .chain(invalid.iter().map(|v| (*v, false)))
                .enumerate()
            {
                n += 1;
                let p = Probe {
                    id: format!(
                        "payload/{}/{kind}/{}{i}",
                        short(t),
                        if ok { "valid" } else { "invalid" }
                    ),
                    doc: dataset(t, &body.replace("{}", value)),
                    want: vec![(
                        path.to_string(),
                        semantic::normalise_value(
                            path.rsplit('/').next().unwrap(),
                            value,
                            t.grammar(),
                        ),
                    )],
                };
                run_probe(&g, "payload", &p, !ok, &mut failures);
            }
        }
    }
    ratchet::verify("payload", n, failures);
}

/// Invalid samples the crate's validator accepts, and why.
const VALIDATOR_READS_AS_VALID: &[(&str, &str, &str)] = &[
    // 5.5.1 controlled values are case-insensitive (p. 21), months
    // included; the checker reads months as tags, exact.
    ("551", "date", "1 jan 1900"),
];

/// The valid samples pass the checker and the crate's validator, and the
/// invalid ones do not: the lists and both sets of grammars agree.
#[test]
fn payload_samples_agree_with_the_checker() {
    let mut wrong = Vec::new();
    for t in [Target::V551, Target::V70] {
        for (kind, body, _, valid, invalid) in payload_samples(t) {
            for v in valid.iter() {
                let doc = dataset(t, &body.replace("{}", v));
                let issues = checker::check(&doc, t, None);
                if !issues.is_empty() {
                    wrong.push(format!("{} {kind} valid {v:?}: {:?}", short(t), issues));
                }
                let found = adapter::validate(&doc);
                if !found.is_empty() {
                    wrong.push(format!(
                        "{} {kind} valid {v:?}: validator {found:?}",
                        short(t)
                    ));
                }
            }
            for v in invalid.iter() {
                let doc = dataset(t, &body.replace("{}", v));
                if checker::check(&doc, t, None).is_empty() {
                    wrong.push(format!("{} {kind} invalid {v:?} passes", short(t)));
                }
                if adapter::validate(&doc).is_empty()
                    && !VALIDATOR_READS_AS_VALID.contains(&(short(t), kind, v))
                {
                    wrong.push(format!(
                        "{} {kind} invalid {v:?} passes the validator",
                        short(t)
                    ));
                }
            }
        }
        for (key, value) in calendar_values(t) {
            let doc = dataset(t, &format!("0 @I1@ INDI\n1 BIRT\n2 DATE {value}\n"));
            let issues = checker::check(&doc, t, None);
            if !issues.is_empty() {
                wrong.push(format!("{} calendar {key} {value:?}: {issues:?}", short(t)));
            }
            let found = adapter::validate(&doc);
            if !found.is_empty() {
                wrong.push(format!(
                    "{} calendar {key} {value:?}: validator {found:?}",
                    short(t)
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Whether the crate's validator reports one of `rules` on `doc`.
fn validator_reports(doc: &str, rules: &[&str]) -> bool {
    adapter::validate(doc)
        .iter()
        .any(|(rule, _, _)| rules.contains(&rule.as_str()))
}

/// The checker and the crate's validator both report misplaced, repeated
/// and missing structures, for a deterministic sample of the table rows of
/// every version.
#[test]
fn checker_self_test() {
    let mut missed = Vec::new();
    let mut n = 0;
    for t in [Target::V551, Target::V70, Target::V71] {
        let mut g = Gen::new(t);
        let paths = g.paths();
        let rows: Vec<&Sub> = t
            .spec()
            .subs
            .iter()
            .filter(|s| {
                !s.sup.is_empty() && paths.contains_key(s.sup) && !matches!(s.tag, "CONT" | "CONC")
            })
            .collect();
        for (k, sub) in rows.iter().enumerate().filter(|(k, _)| k % 7 == 0) {
            let _ = k;
            let alternatives = rows
                .iter()
                .filter(|r| r.sup == sub.sup && r.tag == sub.tag)
                .count();
            let parent = sub.sup.rsplit(['-', '.']).next().unwrap_or("");
            let sample = |i| g.sample(sub.ty, sub.tag, parent, i);
            // Misplaced: the tag under a structure that takes no substructure.
            if let Some((doc, _)) = g.build(sub, &[sample(0)], &paths) {
                n += 1;
                let host = "0 @R1@ REPO\n1 NAME Value NAME A\n";
                let doc = doc.replacen(host, &format!("{host}2 {} x\n", sub.tag), 1);
                if !checker::check(&doc, t, None)
                    .iter()
                    .any(|i| i.rule == "misplaced")
                {
                    missed.push(format!("{} misplaced {}", short(t), sub.tag));
                }
                if !validator_reports(&doc, &["Misplaced", "Continuation"]) {
                    missed.push(format!("{} validator: misplaced {}", short(t), sub.tag));
                }
            }
            // Repeated beyond the maximum.
            if sub.max == 1 && alternatives == 1 {
                let mut many = **sub;
                many.max = 0;
                if let Some((doc, _)) = g.build(&many, &[sample(0), sample(1)], &paths) {
                    n += 1;
                    let issues = checker::check(&doc, t, None);
                    if !issues.iter().any(|i| i.rule == "cardinality-max") {
                        missed.push(format!(
                            "{} repeated {}/{}: {issues:?}",
                            short(t),
                            sub.sup,
                            sub.tag
                        ));
                    }
                    if !validator_reports(&doc, &["Cardinality"]) {
                        missed.push(format!(
                            "{} validator: repeated {}/{}",
                            short(t),
                            sub.sup,
                            sub.tag
                        ));
                    }
                }
            }
            // Required but missing.
            if sub.min >= 1 && alternatives == 1 && !(sub.tag == "VERS" && parent == "GEDC") {
                g.skip = Some((sub.sup, sub.tag));
                if let Some((doc, _)) = g.build(sub, &[], &paths) {
                    n += 1;
                    let issues = checker::check(&doc, t, None);
                    if !issues
                        .iter()
                        .any(|i| matches!(i.rule, "cardinality-min" | "head-required"))
                    {
                        missed.push(format!(
                            "{} missing {}/{}: {issues:?}",
                            short(t),
                            sub.sup,
                            sub.tag
                        ));
                    }
                    if !validator_reports(&doc, &["MissingRequired"]) {
                        missed.push(format!(
                            "{} validator: missing {}/{}",
                            short(t),
                            sub.sup,
                            sub.tag
                        ));
                    }
                }
                g.skip = None;
            }
        }
    }
    assert!(n > 100, "{n} self-test documents");
    assert!(
        missed.is_empty(),
        "{} of {n} not reported:\n{}",
        missed.len(),
        missed.join("\n")
    );
}
