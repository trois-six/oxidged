//! Output conformance checker, independent of oxidged.
//!
//! Every byte stream the writer produces in this suite goes through
//! [`check`]. It reports each deviation from the line grammar and from the
//! structure tables of the target version as an [`Issue`] with a stable rule
//! id; the known-gaps ratchet keys on those ids.
//!
//! Line rules (both versions unless stated): one terminator kind, a final
//! terminator, no blank lines or leading whitespace, single delimiters,
//! levels without leading zeros that rise by one at most, well-formed tags
//! and xrefs (7.0 `@[A-Z0-9_]+@`, never `@VOID@`; 5.5.1 at most 22
//! characters), xrefs on records only and unique, pointers that resolve,
//! `@` escaped (7.0: a leading `@`; 5.5.1: every `@` outside an `@#…@`
//! escape), no banned characters, 5.5.1 lines of at most 255 characters
//! including the terminator, 5.5.1 records of at most 32,767 bytes, no CONC
//! in 7.x, CONT/CONC directly under their structure and never split at a
//! space, HEAD first and TRLR last and empty, GEDC.VERS equal to the target,
//! 7.x without HEAD.CHAR and GEDC.FORM, 5.5.1 with `CHAR UTF-8` (the writer's
//! text is UTF-8) and `FORM LINEAGE-LINKED`.
//!
//! Structure rules, from `spec_tables.rs`: each standard tag allowed under its
//! superstructure, cardinalities, payload types (pointer targets, enumeration
//! values, dates, ages, times, languages, media types, file paths, names,
//! coordinates, integers, `Y`), and, given the input, a SCHMA declaration for
//! every documented extension tag the output uses; in 7.x, a substructure
//! with a payload or a substructure (§1.2: "A structure must have either a
//! non-empty payload or at least 1 substructure"; records, which pointers
//! name, are left to the writer); in 7.x (7.0.17), no `ALIA` to its own
//! record and no cycle of pointers between sources and multimedia objects
//! or shared notes.

use super::spec_tables::{Cal, Pay, Spec, Sub, V70, V71, V551};
use super::tree::{self, Node, Payload, Version};
use std::collections::{HashMap, HashSet};

/// The version a written stream must conform to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    V551,
    V70,
    V71,
}

impl Target {
    pub fn spec(self) -> &'static Spec {
        match self {
            Target::V551 => &V551,
            Target::V70 => &V70,
            Target::V71 => &V71,
        }
    }

    pub fn grammar(self) -> Version {
        match self {
            Target::V551 => Version::V551,
            Target::V70 | Target::V71 => Version::V70,
        }
    }

    pub fn is_v7(self) -> bool {
        self != Target::V551
    }

    pub fn vers(self) -> &'static str {
        self.spec().version
    }

    /// The target matching a declared `GEDC.VERS`.
    pub fn from_vers(vers: &str) -> Target {
        if vers.starts_with("7.1") {
            Target::V71
        } else if vers.starts_with('7') {
            Target::V70
        } else {
            Target::V551
        }
    }
}

/// One deviation in a written stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub rule: &'static str,
    pub line: usize,
    pub detail: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}: {}", self.line, self.rule, self.detail)
    }
}

/// Checks a written stream against `target`. `input`, when given, is the
/// stream the output was written from; it is used for the SCHMA rule only.
pub fn check(output: &str, target: Target, input: Option<&str>) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut push = |rule: &'static str, line: usize, detail: String| {
        issues.push(Issue { rule, line, detail });
    };
    let text = output.strip_prefix('\u{feff}').unwrap_or(output);
    let v = target.grammar();

    // Terminators.
    let mut kinds = HashSet::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\r' if b.get(i + 1) == Some(&b'\n') => {
                kinds.insert("CRLF");
                i += 1;
            }
            b'\r' => {
                kinds.insert("CR");
            }
            b'\n' => {
                kinds.insert("LF");
            }
            _ => {}
        }
        i += 1;
    }
    if kinds.len() > 1 {
        let mut k: Vec<_> = kinds.iter().copied().collect();
        k.sort_unstable();
        push("eol-mixed", 0, k.join(", "));
    }
    if !text.is_empty() && !text.ends_with(['\n', '\r']) {
        push("eol-final", 0, "the last line has no terminator".into());
    }
    let eol_len = if kinds.contains("CRLF") { 2 } else { 1 };

    // Lines.
    let lines = tree::split_lines(text);
    let mut prev_level: Option<u32> = None;
    let mut xrefs: HashMap<String, usize> = HashMap::new();
    let mut pointers: Vec<(usize, String)> = Vec::new();
    let mut record_start = 0usize;
    let mut record_bytes = 0usize;
    // (level, tag, raw payload, had a non-continuation child yet) of open lines.
    let mut open: Vec<(u32, String, String, bool)> = Vec::new();
    for (idx, raw) in lines.iter().enumerate() {
        let n = idx + 1;
        if raw.is_empty() {
            push("blank-line", n, String::new());
            continue;
        }
        if raw.starts_with([' ', '\t']) {
            push("leading-whitespace", n, (*raw).to_string());
        }
        if let Some(c) = raw.chars().find(|&c| is_banned(c)) {
            push("banned-char", n, format!("U+{:04X}", c as u32));
        }
        if v == Version::V551 && raw.chars().count() + eol_len > 255 {
            push(
                "line-length",
                n,
                format!("{} characters", raw.chars().count() + eol_len),
            );
        }
        let Some(l) = strict_line(raw.trim_start_matches([' ', '\t'])) else {
            push("line-syntax", n, (*raw).to_string());
            continue;
        };
        if l.leading_zero {
            push("level-leading-zero", n, (*raw).to_string());
        }
        if l.extra_delims {
            push("delimiter", n, (*raw).to_string());
        }
        if l.payload == Some("") {
            push("trailing-delimiter", n, (*raw).to_string());
        }
        match prev_level {
            None if l.level != 0 => push("level-jump", n, "first line is not level 0".into()),
            Some(p) if l.level > p + 1 => push("level-jump", n, format!("{p} to {}", l.level)),
            _ => {}
        }
        prev_level = Some(l.level);
        if !tag_ok(l.tag, v) {
            push("tag-syntax", n, l.tag.to_string());
        }
        if l.level == 0 {
            if v == Version::V551 && record_bytes > 32_767 {
                push("record-size", record_start, format!("{record_bytes} bytes"));
            }
            record_start = n;
            record_bytes = 0;
        }
        record_bytes += raw.len() + eol_len;
        if let Some(x) = l.xref {
            if l.level > 0 {
                push("xref-on-substructure", n, x.to_string());
            } else if matches!(l.tag, "HEAD" | "TRLR") {
                push("xref-forbidden", n, x.to_string());
            }
            if !xref_ok(x, v) {
                push("xref-syntax", n, x.to_string());
            }
            if let Some(first) = xrefs.insert(x.to_string(), n) {
                push("xref-duplicate", n, format!("{x} (first on line {first})"));
            }
        }
        // Continuations.
        while open.last().is_some_and(|o| o.0 >= l.level) {
            open.pop();
        }
        let is_cont = l.tag == "CONT" || l.tag == "CONC";
        if is_cont {
            if l.tag == "CONC" && target.is_v7() {
                push("conc", n, (*raw).to_string());
            }
            match open.last_mut() {
                Some(parent) if parent.0 + 1 == l.level => {
                    if parent.3 {
                        push("cont-position", n, "after another substructure".into());
                    }
                    if l.tag == "CONC" && v == Version::V551 {
                        let p = l.payload.unwrap_or("");
                        if p.starts_with(' ') || parent.2.ends_with(' ') {
                            push("conc-split-space", n, (*raw).to_string());
                        }
                    }
                    parent.2 = l.payload.unwrap_or("").to_string();
                }
                Some(parent) if CONT_TAGS.contains(&parent.1.as_str()) => {
                    push("cont-children", n, (*raw).to_string())
                }
                _ => push("cont-position", n, "no structure to continue".into()),
            }
        } else if let Some(parent) = open.last_mut() {
            if CONT_TAGS.contains(&parent.1.as_str()) {
                push("cont-children", n, (*raw).to_string());
            }
            parent.3 = true;
        }
        open.push((
            l.level,
            l.tag.to_string(),
            l.payload.unwrap_or("").to_string(),
            false,
        ));
        // Payload escapes and pointers.
        if let Some(p) = l.payload {
            if tree::is_pointer(p, v) && !is_cont {
                pointers.push((n, p.to_string()));
                if !(xref_ok(p, v) || (target.is_v7() && p == "@VOID@")) {
                    push("pointer-syntax", n, p.to_string());
                }
            } else if !escape_ok(p, v, is_cont) {
                push("escape", n, p.to_string());
            }
        }
    }
    if v == Version::V551 && record_bytes > 32_767 {
        push("record-size", record_start, format!("{record_bytes} bytes"));
    }
    for (n, p) in &pointers {
        // 5.5.1 network references (`@RESOURCE:ID@`, p.16) name a record
        // outside the transmission.
        let network = !target.is_v7() && p.contains([':', '!']);
        if !xrefs.contains_key(p) && !(target.is_v7() && p == "@VOID@") && !network {
            push("pointer-dangling", *n, p.clone());
        }
    }

    // Records and header.
    let records = tree::parse_with(text, v);
    check_records(&records, target, input, &mut issues);
    issues
}

const CONT_TAGS: [&str; 2] = ["CONT", "CONC"];

pub fn is_banned(c: char) -> bool {
    (c < ' ' && c != '\t')
        || c == '\u{7f}'
        || ('\u{80}'..='\u{9f}').contains(&c)
        || c == '\u{fffe}'
        || c == '\u{ffff}'
}

struct StrictLine<'a> {
    level: u32,
    leading_zero: bool,
    extra_delims: bool,
    xref: Option<&'a str>,
    tag: &'a str,
    payload: Option<&'a str>,
}

/// Splits a line on the exact grammar, noting (not rejecting) leading zeros
/// and extra delimiters so each gets its own rule.
fn strict_line(raw: &str) -> Option<StrictLine<'_>> {
    let digits = raw.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let level = raw[..digits].parse().ok()?;
    let leading_zero = digits > 1 && raw.starts_with('0');
    let mut rest = raw[digits..].strip_prefix(' ')?;
    let mut extra_delims = false;
    if rest.starts_with(' ') {
        extra_delims = true;
        rest = rest.trim_start_matches(' ');
    }
    let mut xref = None;
    if rest.starts_with('@') {
        let end = rest[1..].find('@')? + 2;
        xref = Some(&rest[..end]);
        rest = rest[end..].strip_prefix(' ')?;
        if rest.starts_with(' ') {
            extra_delims = true;
            rest = rest.trim_start_matches(' ');
        }
    }
    let end = rest.find(' ').unwrap_or(rest.len());
    let tag = &rest[..end];
    if tag.is_empty() {
        return None;
    }
    let payload = (end < rest.len()).then(|| &rest[end + 1..]);
    Some(StrictLine {
        level,
        leading_zero,
        extra_delims,
        xref,
        tag,
        payload,
    })
}

fn tag_ok(tag: &str, v: Version) -> bool {
    let b = tag.as_bytes();
    match v {
        // `stdTag = ucletter *tagchar`, `extTag = underscore 1*tagchar`,
        // `tagchar = ucletter / DIGIT / underscore` (7.0 §1.3).
        Version::V70 => {
            if let Some(rest) = tag.strip_prefix('_') {
                !rest.is_empty()
                    && rest
                        .bytes()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
            } else {
                b[0].is_ascii_uppercase()
                    && b.iter()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
            }
        }
        Version::V551 => {
            (b[0] == b'_' || b[0].is_ascii_alphabetic())
                && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
                && (b[0] == b'_' || !b.iter().any(u8::is_ascii_lowercase))
        }
    }
}

fn xref_ok(x: &str, v: Version) -> bool {
    let Some(inner) = x.strip_prefix('@').and_then(|s| s.strip_suffix('@')) else {
        return false;
    };
    if inner.is_empty() {
        return false;
    }
    match v {
        Version::V70 => {
            inner != "VOID"
                && inner
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        }
        // p.13: any non-`@` after a first alphanumeric, spaces included.
        Version::V551 => {
            x.chars().count() <= 22
                && inner.as_bytes()[0].is_ascii_alphanumeric()
                && !inner.contains(['@', '\t'])
        }
    }
}

/// True when a raw text payload is correctly escaped.
fn escape_ok(p: &str, v: Version, is_cont: bool) -> bool {
    match v {
        // 7.0: a payload starting with `@` must start with `@@`. A CONT line
        // has its own payload, so the rule applies to it too.
        Version::V70 => {
            let _ = is_cont;
            !p.starts_with('@') || p.starts_with("@@")
        }
        // 5.5.1: every `@` is doubled, except in an `@#…@` escape.
        Version::V551 => {
            let b = p.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'@' {
                    if b.get(i + 1) == Some(&b'@') {
                        i += 2;
                        continue;
                    }
                    if b.get(i + 1) == Some(&b'#') {
                        match p[i + 2..].find('@') {
                            Some(end) => {
                                i += end + 3;
                                continue;
                            }
                            None => return false,
                        }
                    }
                    return false;
                }
                i += 1;
            }
            true
        }
    }
}

fn check_records(records: &[Node], target: Target, input: Option<&str>, issues: &mut Vec<Issue>) {
    let mut push = |rule: &'static str, line: usize, detail: String| {
        issues.push(Issue { rule, line, detail });
    };
    let spec = target.spec();
    match records.first() {
        Some(r) if r.tag == "HEAD" => {}
        Some(r) => push("head-first", r.line, r.tag.clone()),
        None => {
            push("head-first", 0, "empty output".into());
            return;
        }
    }
    match records.last() {
        Some(r) if r.tag == "TRLR" => {
            if let Some(c) = r.children.first() {
                push("trlr-children", c.line, c.tag.clone());
            }
        }
        Some(r) => push("trlr-last", r.line, r.tag.clone()),
        None => {}
    }
    for (tag, rule) in [("HEAD", "head-duplicate"), ("TRLR", "trlr-duplicate")] {
        for r in records.iter().filter(|r| r.tag == tag).skip(1) {
            push(rule, r.line, String::new());
        }
    }
    // Header rules.
    if let Some(head) = records.iter().find(|r| r.tag == "HEAD") {
        let gedc = head.child("GEDC");
        match gedc.and_then(|g| g.child("VERS")) {
            Some(vs) if vs.payload.as_str() == target.vers() => {}
            Some(vs) => push(
                "gedc-vers",
                vs.line,
                format!("{} instead of {}", vs.payload.as_str(), target.vers()),
            ),
            None => push("gedc-vers", head.line, "missing".into()),
        }
        let form = gedc.and_then(|g| g.child("FORM"));
        let chr = head.child("CHAR");
        if target.is_v7() {
            if let Some(c) = chr {
                push("v7-char", c.line, c.payload.as_str().to_string());
            }
            if let Some(f) = form {
                push("v7-gedc-form", f.line, f.payload.as_str().to_string());
            }
        } else {
            match chr {
                Some(c) if c.payload.as_str() == "UTF-8" => {}
                Some(c) => push(
                    "char-mismatch",
                    c.line,
                    format!("{} for UTF-8 output", c.payload.as_str()),
                ),
                None => {}
            }
            match form {
                Some(f) if f.payload.as_str() == "LINEAGE-LINKED" => {}
                Some(f) => push("gedc-form", f.line, f.payload.as_str().to_string()),
                None => {}
            }
        }
        if let Some(input) = input {
            let declared = schma(&tree::parse(input).1);
            let ours = schma(records);
            let mut used = HashSet::new();
            for r in records {
                r.walk(&mut Vec::new(), &mut |_, n| {
                    if n.tag.starts_with('_') {
                        used.insert(n.tag.clone());
                    }
                });
            }
            let mut missing: Vec<_> = declared
                .iter()
                .filter(|(t, uri)| used.contains(*t) && ours.get(*t) != Some(uri))
                .map(|(t, _)| t.clone())
                .collect();
            missing.sort();
            for t in missing {
                push("schma-missing", head.line, t);
            }
        }
    }
    // Structures.
    let record_types: HashMap<&str, &str> = records
        .iter()
        .filter_map(|r| {
            let x = r.xref.as_deref()?;
            let sub = spec
                .subs
                .iter()
                .find(|s| s.sup.is_empty() && s.tag == r.tag)?;
            // 7.x pointers name the record type, 5.5.1 pointers the record tag.
            Some((x, if target.is_v7() { sub.ty } else { sub.tag }))
        })
        .collect();
    let cx = Cx {
        spec,
        target,
        record_types,
    };
    if target.is_v7() {
        check_pointer_cycles(records, issues);
    }
    let mut root_counts: HashMap<&str, usize> = HashMap::new();
    for r in records {
        if r.tag.starts_with('_') {
            continue;
        }
        let cands: Vec<&Sub> = spec
            .subs
            .iter()
            .filter(|s| s.sup.is_empty() && s.tag == r.tag)
            .collect();
        match pick_typed(spec, &cands, &r.payload) {
            Some(sub) => {
                *root_counts.entry(sub.ty).or_default() += 1;
                check_node(r, sub.ty, true, &cx, issues);
            }
            None => issues.push(Issue {
                rule: "misplaced",
                line: r.line,
                detail: format!("record {}", r.tag),
            }),
        }
    }
    for s in spec.subs.iter().filter(|s| s.sup.is_empty()) {
        let n = root_counts.get(s.ty).copied().unwrap_or(0);
        if s.max > 0 && n > s.max as usize && s.tag != "TRLR" && s.tag != "HEAD" {
            issues.push(Issue {
                rule: "cardinality-max",
                line: 0,
                detail: format!("{} records: {n} > {}", s.tag, s.max),
            });
        }
    }
}

/// 7.x pointers between records (7.0.17): an individual's `ALIA` never
/// names the individual itself, and the pointers of sources to multimedia
/// objects and shared notes, and back, form no cycle. Any cycle is
/// reported once, at a record on it.
fn check_pointer_cycles(records: &[Node], issues: &mut Vec<Issue>) {
    let tag_of: HashMap<&str, &str> = records
        .iter()
        .filter_map(|r| Some((r.xref.as_deref()?, r.tag.as_str())))
        .collect();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for r in records {
        let Some(x) = r.xref.as_deref() else { continue };
        if r.tag == "INDI" {
            for a in r.children.iter().filter(|c| c.tag == "ALIA") {
                if let Payload::Pointer(p) = &a.payload
                    && p == x
                {
                    issues.push(Issue {
                        rule: "alia-self",
                        line: a.line,
                        detail: p.clone(),
                    });
                }
            }
        }
        let wanted: &[&str] = match r.tag.as_str() {
            "SOUR" => &["OBJE", "SNOTE"],
            "OBJE" | "SNOTE" => &["SOUR"],
            _ => continue,
        };
        fn pointers<'a>(n: &'a Node, out: &mut Vec<&'a str>) {
            for c in n.children.iter().filter(|c| !c.tag.starts_with('_')) {
                if let Payload::Pointer(p) = &c.payload {
                    out.push(p);
                }
                pointers(c, out);
            }
        }
        let mut found = Vec::new();
        pointers(r, &mut found);
        found.retain(|p| tag_of.get(p).is_some_and(|t| wanted.contains(t)));
        edges.insert(x, found);
    }
    // Depth-first, three colours: a pointer to a record on the path closes
    // a cycle.
    let mut colour: HashMap<&str, u8> = HashMap::new();
    for r in records {
        let Some(start) = r.xref.as_deref() else {
            continue;
        };
        if colour.contains_key(start) || !edges.contains_key(start) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(start, 0)];
        colour.insert(start, 1);
        while let Some(&mut (at, ref mut next)) = stack.last_mut() {
            let out = edges.get(at).map_or(&[][..], Vec::as_slice);
            if let Some(&to) = out.get(*next) {
                *next += 1;
                match colour.get(to) {
                    Some(1) => issues.push(Issue {
                        rule: "pointer-cycle",
                        line: r.line,
                        detail: format!("{at} -> {to}"),
                    }),
                    Some(_) => {}
                    None => {
                        colour.insert(to, 1);
                        stack.push((to, 0));
                    }
                }
            } else {
                colour.insert(at, 2);
                stack.pop();
            }
        }
    }
}

fn schma(records: &[Node]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Some(s) = records
        .iter()
        .find(|r| r.tag == "HEAD")
        .and_then(|h| h.child("SCHMA"))
    {
        for t in s.children_tagged("TAG") {
            if let Some((tag, uri)) = t.payload.as_str().split_once(' ') {
                m.insert(tag.to_string(), uri.trim().to_string());
            }
        }
    }
    m
}

struct Cx<'a> {
    spec: &'static Spec,
    target: Target,
    record_types: HashMap<&'a str, &'static str>,
}

fn payload_of(spec: &Spec, ty: &str) -> Pay {
    spec.payloads
        .iter()
        .find(|(t, _)| *t == ty)
        .map_or(Pay::None, |(_, p)| *p)
}

/// Among alternative definitions of one tag (5.5.1 pointer and text forms),
/// the one whose payload shape matches.
fn pick_typed<'s>(spec: &Spec, cands: &[&'s Sub], payload: &Payload) -> Option<&'s Sub> {
    let is_ptr = matches!(payload, Payload::Pointer(_));
    cands
        .iter()
        .find(|s| matches!(payload_of(spec, s.ty), Pay::Ptr(_)) == is_ptr)
        .or_else(|| cands.first())
        .copied()
}

fn check_node(n: &Node, ty: &'static str, record: bool, cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let spec = cx.spec;
    check_payload(n, ty, payload_of(spec, ty), cx, issues);
    if cx.target.is_v7() && !record && n.payload.as_str().is_empty() && n.children.is_empty() {
        issues.push(Issue {
            rule: "empty",
            line: n.line,
            detail: format!("{} has no payload and no substructure", n.tag),
        });
    }
    let allowed: Vec<&Sub> = spec.subs.iter().filter(|s| s.sup == ty).collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for c in &n.children {
        if c.tag.starts_with('_') || CONT_TAGS.contains(&c.tag.as_str()) {
            continue;
        }
        let cands: Vec<&Sub> = allowed.iter().copied().filter(|s| s.tag == c.tag).collect();
        match pick_typed(spec, &cands, &c.payload) {
            Some(sub) => {
                // Alternatives of one tag share the cardinality budget.
                *counts.entry(sub.tag).or_default() += 1;
                check_node(c, sub.ty, false, cx, issues);
            }
            // HEAD.CHAR and GEDC.FORM in 7.x have rules of their own.
            None if cx.target.is_v7()
                && matches!(
                    (n.tag.as_str(), c.tag.as_str()),
                    ("HEAD", "CHAR") | ("GEDC", "FORM")
                ) => {}
            None => issues.push(Issue {
                rule: "misplaced",
                line: c.line,
                detail: format!("{} under {}", c.tag, n.tag),
            }),
        }
    }
    let mut seen = HashSet::new();
    for s in &allowed {
        if !seen.insert(s.tag) {
            continue;
        }
        let n_found = counts.get(s.tag).copied().unwrap_or(0);
        let max = allowed
            .iter()
            .filter(|a| a.tag == s.tag)
            .map(|a| a.max)
            .max()
            .unwrap_or(0);
        let min = allowed
            .iter()
            .filter(|a| a.tag == s.tag)
            .map(|a| a.min)
            .max()
            .unwrap_or(0);
        if max > 0 && n_found > max as usize {
            issues.push(Issue {
                rule: "cardinality-max",
                line: n.line,
                detail: format!("{} under {}: {n_found} > {max}", s.tag, n.tag),
            });
        }
        if n_found < min as usize {
            issues.push(Issue {
                rule: if n.tag == "HEAD" {
                    "head-required"
                } else {
                    "cardinality-min"
                },
                line: n.line,
                detail: format!("{} under {}: missing", s.tag, n.tag),
            });
        }
    }
}

fn check_payload(n: &Node, ty: &str, pay: Pay, cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let v7 = cx.target.is_v7();
    let mut bad = |rule: &'static str, why: &str| {
        issues.push(Issue {
            rule,
            line: n.line,
            detail: format!("{} {:?}: {why}", n.tag, n.payload.as_str()),
        });
    };
    let text = match &n.payload {
        Payload::Pointer(p) => {
            match pay {
                Pay::Ptr(to) => {
                    if let Some(actual) = cx.record_types.get(p.as_str())
                        && *actual != to
                    {
                        bad("pointer-target", &format!("points to {actual}, not {to}"));
                    }
                }
                _ => bad(
                    "payload-pointer",
                    "a pointer where the type is not a pointer",
                ),
            }
            return;
        }
        Payload::Text(t) => t.as_str(),
        Payload::None => "",
    };
    let _ = ty;
    match pay {
        Pay::None => {
            if !text.is_empty() {
                bad("payload-unexpected", "this structure takes no payload");
            }
        }
        Pay::Y => {
            if !(text.is_empty() || text == "Y") {
                bad("payload-y", "only Y or nothing");
            }
        }
        Pay::Ptr(_) => {
            if !(n.payload == Payload::None && !v7) {
                bad("payload-pointer", "text where a pointer is required");
            }
        }
        Pay::Int => {
            if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                bad("payload-int", "not a non-negative integer");
            }
        }
        Pay::Enum(set) => {
            if !enum_ok(cx.spec, set, text, v7) {
                bad("payload-enum", &format!("not in {set}"));
            }
        }
        Pay::ListEnum(set) => {
            if !text.split(',').all(|i| enum_ok(cx.spec, set, i.trim(), v7)) {
                bad("payload-enum", &format!("not a list of {set}"));
            }
        }
        Pay::Date | Pay::DateExact | Pay::DatePeriod => {
            let ok = if v7 {
                date70(text, pay, cx.spec)
            } else {
                date551(text, pay, cx.spec)
            };
            if !ok {
                bad("payload-date", "not in the date grammar");
            }
        }
        Pay::Age => {
            if !age_ok(text, v7) {
                bad("payload-age", "not in the age grammar");
            }
        }
        Pay::Time => {
            if !time_ok(text, v7) {
                bad("payload-time", "not in the time grammar");
            }
        }
        Pay::Lang => {
            if v7 && !lang_ok(text) {
                bad("payload-lang", "not a BCP 47 language tag");
            }
        }
        Pay::MediaType => {
            if !media_type_ok(text) {
                bad("payload-media-type", "not a media type");
            }
        }
        Pay::FilePath => {
            if text.contains([' ', '\\']) || v7 && !url_string_ok(text) {
                bad("payload-file-path", "not a URI reference");
            }
        }
        Pay::Name => {
            if !name_ok(text) {
                bad("payload-name", "slashes must come in one pair");
            }
        }
        Pay::Lat | Pay::Long => {
            if !coord_ok(text, pay == Pay::Lat, v7) {
                bad("payload-coordinate", "not a coordinate");
            }
        }
        Pay::TagDef => {
            let ok = text.split_once(' ').is_some_and(|(t, u)| {
                t.starts_with('_') && tag_ok(t, Version::V70) && uri_reference_ok(u)
            });
            if !ok {
                bad("payload-tagdef", "not `_TAG URI`");
            }
        }
        Pay::Uri => {
            if !uri_reference_ok(text) {
                bad("payload-uri", "not a URI");
            }
        }
        Pay::Text | Pay::ListText => {}
    }
}

fn enum_ok(spec: &Spec, set: &str, value: &str, v7: bool) -> bool {
    let Some((_, values, open)) = spec.enums.iter().find(|(s, _, _)| *s == set) else {
        return true;
    };
    if *open {
        return true;
    }
    if v7 {
        values.contains(&value) || (value.starts_with('_') && tag_ok(value, Version::V70))
    } else if set == "MULTIMEDIA_FORMAT" {
        // The crate's one accepted practice (src/spec/mod.rs): the 5.5.1
        // file formats are open, a list value in its spelling or any other
        // word, but not a media type.
        values.contains(&value)
            || (!value.is_empty()
                && !value.contains('/')
                && !values.iter().any(|v| v.eq_ignore_ascii_case(value)))
    } else {
        // Readers match 5.5.1 values case-insensitively; a writer uses the
        // spec's spelling.
        values.contains(&value)
    }
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn ext_tag(s: &str) -> bool {
    s.len() > 1 && s.starts_with('_') && tag_ok(s, Version::V70)
}

/// GEDCOM 7 `DateValue`, `DateExact` or `DatePeriod` (grammar.abnf).
pub fn date70(s: &str, kind: Pay, spec: &Spec) -> bool {
    let t: Vec<&str> = if s.is_empty() {
        Vec::new()
    } else {
        s.split(' ').collect()
    };
    if t.iter().any(|x| x.is_empty()) {
        return false;
    }
    let date = |t: &[&str]| date70_simple(t, spec);
    match kind {
        Pay::DateExact => t.len() == 3 && digits(t[0]) && date(&t),
        Pay::DatePeriod => period70(&t, &date),
        _ => {
            if t.is_empty() || period70(&t, &date) {
                return true;
            }
            match t.first().copied() {
                Some("BET") => t
                    .iter()
                    .position(|x| *x == "AND")
                    .is_some_and(|i| date(&t[1..i]) && date(&t[i + 1..])),
                Some("AFT" | "BEF" | "ABT" | "CAL" | "EST") => date(&t[1..]),
                _ => date(&t),
            }
        }
    }
}

fn period70(t: &[&str], date: &dyn Fn(&[&str]) -> bool) -> bool {
    match t.first().copied() {
        None => true,
        Some("TO") => date(&t[1..]),
        Some("FROM") => match t.iter().position(|x| *x == "TO") {
            Some(i) => date(&t[1..i]) && date(&t[i + 1..]),
            None => date(&t[1..]),
        },
        _ => false,
    }
}

fn date70_simple(t: &[&str], spec: &Spec) -> bool {
    let mut t = t;
    let mut cal: Option<&Cal> = spec.calendars.iter().find(|c| c.tag == "GREGORIAN");
    let mut ext_cal = false;
    if let Some(first) = t.first() {
        if let Some(c) = spec.calendars.iter().find(|c| c.tag == *first) {
            cal = Some(c);
            t = &t[1..];
        } else if ext_tag(first) {
            ext_cal = true;
            cal = None;
            t = &t[1..];
        }
    }
    // Optional epoch at the end.
    let mut epoch_ok = true;
    if let Some(last) = t.last()
        && !digits(last)
    {
        epoch_ok = ext_cal || cal.is_some_and(|c| c.epochs.contains(last)) || ext_tag(last);
        t = &t[..t.len() - 1];
    }
    let month_ok = |m: &str| ext_cal || ext_tag(m) || cal.is_some_and(|c| c.months.contains(&m));
    epoch_ok
        && match t {
            [y] => digits(y),
            [m, y] => month_ok(m) && digits(y),
            [d, m, y] => digits(d) && month_ok(m) && digits(y),
            _ => false,
        }
}

/// GEDCOM 5.5.1 `DATE_VALUE`, `DATE_EXACT` or `DATE_PERIOD` (pp. 45–47).
pub fn date551(s: &str, kind: Pay, spec: &Spec) -> bool {
    if kind == Pay::DateExact {
        let t: Vec<&str> = s.split(' ').collect();
        return t.len() == 3 && digits(t[0]) && t[0].len() <= 2 && date551_simple(s, spec);
    }
    if kind == Pay::Date {
        if s.starts_with('(') && s.ends_with(')') {
            return true;
        }
        if let Some(rest) = s.strip_prefix("INT ") {
            return rest
                .find(" (")
                .is_some_and(|i| rest.ends_with(')') && date551_simple(&rest[..i], spec));
        }
    }
    let period = |s: &str| -> bool {
        if let Some(rest) = s.strip_prefix("FROM ") {
            match rest.find(" TO ") {
                Some(i) => date551_simple(&rest[..i], spec) && date551_simple(&rest[i + 4..], spec),
                None => date551_simple(rest, spec),
            }
        } else if let Some(rest) = s.strip_prefix("TO ") {
            date551_simple(rest, spec)
        } else {
            false
        }
    };
    if kind == Pay::DatePeriod {
        return period(s);
    }
    if period(s) {
        return true;
    }
    if let Some(rest) = s.strip_prefix("BET ") {
        return rest.find(" AND ").is_some_and(|i| {
            date551_simple(&rest[..i], spec) && date551_simple(&rest[i + 5..], spec)
        });
    }
    for k in ["BEF ", "AFT ", "ABT ", "CAL ", "EST "] {
        if let Some(rest) = s.strip_prefix(k) {
            return date551_simple(rest, spec);
        }
    }
    date551_simple(s, spec)
}

fn date551_simple(s: &str, spec: &Spec) -> bool {
    let mut s = s;
    let mut cal = "GREGORIAN";
    if let Some(rest) = s.strip_prefix("@#D") {
        let Some(end) = rest.find("@ ") else {
            return false;
        };
        cal = &rest[..end];
        if !spec.calendars.iter().any(|c| c.tag == cal) {
            return false;
        }
        s = &rest[end + 2..];
    }
    let Some(c) = spec.calendars.iter().find(|c| c.tag == cal) else {
        return false;
    };
    if c.months.is_empty() {
        // ROMAN and UNKNOWN: no grammar is defined.
        return !s.is_empty();
    }
    let s = s
        .strip_suffix(" B.C.")
        .or_else(|| s.strip_suffix("B.C."))
        .unwrap_or(s);
    let t: Vec<&str> = s.split(' ').collect();
    let year = |y: &str| {
        if cal == "GREGORIAN" {
            match y.split_once('/') {
                Some((a, b)) => digits(a) && b.len() == 2 && digits(b),
                None => digits(y),
            }
        } else {
            digits(y)
        }
    };
    match t.as_slice() {
        [y] => year(y),
        [m, y] => c.months.contains(m) && year(y),
        [d, m, y] => digits(d) && d.len() <= 2 && c.months.contains(m) && year(y),
        _ => false,
    }
}

/// 7.0 `Age` or 5.5.1 `AGE_AT_EVENT`.
pub fn age_ok(s: &str, v7: bool) -> bool {
    // 7.0: `Age = [[ageBound D] ageDuration]`, so an empty age (with a PHRASE) is valid.
    if v7 && s.is_empty() {
        return true;
    }
    let mut s = s;
    if let Some(rest) = s.strip_prefix(['<', '>']) {
        s = if v7 {
            match rest.strip_prefix(' ') {
                Some(r) => r,
                None => return false,
            }
        } else {
            rest.strip_prefix(' ').unwrap_or(rest)
        };
    }
    // 5.5.1 controlled values ignore case (p. 21): its keywords and units
    // (test-files `5/age-valid.ged`: `child`, `0Y`, `<0D`).
    let lower = s.to_ascii_lowercase();
    let s = if v7 { s } else { lower.as_str() };
    if !v7 && matches!(s, "child" | "infant" | "stillborn") {
        return true;
    }
    let units: &[char] = if v7 {
        &['y', 'm', 'w', 'd']
    } else {
        &['y', 'm', 'd']
    };
    let mut next = 0;
    let mut any = false;
    for part in s.split(' ') {
        let Some(u) = part.chars().last() else {
            return false;
        };
        let Some(pos) = units.iter().position(|&x| x == u) else {
            return false;
        };
        if pos < next || !digits(&part[..part.len() - 1]) {
            return false;
        }
        next = pos + 1;
        any = true;
    }
    any
}

pub fn time_ok(s: &str, v7: bool) -> bool {
    let s = if v7 {
        s.strip_suffix('Z').unwrap_or(s)
    } else {
        s
    };
    let (hms, frac) = match s.split_once('.') {
        Some((a, f)) => (a, Some(f)),
        None => (s, None),
    };
    let p: Vec<&str> = hms.split(':').collect();
    let two =
        |x: &str, max: u32| x.len() == 2 && digits(x) && x.parse::<u32>().is_ok_and(|n| n < max);
    let ok = match p.as_slice() {
        [h, m] => {
            frac.is_none()
                && (1..=2).contains(&h.len())
                && digits(h)
                && h.parse::<u32>().is_ok_and(|n| n < 24)
                && two(m, 60)
        }
        [h, m, sec] => {
            (1..=2).contains(&h.len())
                && digits(h)
                && h.parse::<u32>().is_ok_and(|n| n < 24)
                && two(m, 60)
                && two(sec, 60)
        }
        _ => false,
    };
    ok && frac.is_none_or(digits)
}

fn lang_ok(s: &str) -> bool {
    let mut parts = s.split('-');
    let first = parts.next().unwrap_or("");
    (2..=8).contains(&first.len())
        && first.bytes().all(|b| b.is_ascii_alphabetic())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

fn media_type_ok(s: &str) -> bool {
    let main = s.split(';').next().unwrap_or("").trim();
    let token = |t: &str| {
        !t.is_empty()
            && t.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&b))
    };
    main.split_once('/')
        .is_some_and(|(a, b)| token(a) && token(b))
}

fn name_ok(s: &str) -> bool {
    let n = s.matches('/').count();
    n == 0 || n == 2
}

/// 7.x (7.0.17 as 7.0.18 corrected it): `N`/`S` and at most two digits of
/// degrees up to 90, `E`/`W` and at most three up to 180, then `.` and
/// one digit or more; 5.5.1 shows no grammar but its example.
fn coord_ok(s: &str, lat: bool, v7: bool) -> bool {
    let (dirs, max, width) = if lat {
        (['N', 'S'], 90.0, 2)
    } else {
        (['E', 'W'], 180.0, 3)
    };
    let Some(rest) = s.strip_prefix(dirs) else {
        return false;
    };
    let (int, frac) = rest.split_once('.').unwrap_or((rest, "0"));
    (!v7 || int.len() <= width)
        && digits(int)
        && digits(frac)
        && rest.parse::<f64>().is_ok_and(|x| x <= max)
}

/// An RFC 3986 URI reference, loosely: printable ASCII but space, a
/// backtick and the characters of `"<>\^{|}`, at most one `#`, no `:` in a
/// first segment that is no scheme. Not empty.
fn uri_reference_ok(s: &str) -> bool {
    let chars = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_graphic() && !b"\"<>\\^`{|}".contains(&b))
        && s.matches('#').count() <= 1;
    let first = s.split(['/', '?', '#']).next().unwrap_or("");
    let scheme = first.split_once(':').is_some_and(|(sc, _)| {
        sc.starts_with(|c: char| c.is_ascii_alphabetic())
            && sc
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    chars && (scheme || !first.contains(':'))
}

/// A WHATWG valid URL string, loosely: no ASCII control, space, backtick
/// or character of `"<>\^{|}`, no C1 control or noncharacter, `%` before
/// two hex digits.
fn url_string_ok(s: &str) -> bool {
    let b = s.as_bytes();
    s.chars().all(|c| {
        let n = u32::from(c);
        !(c.is_ascii_control()
            || " \"<>\\^`{|}".contains(c)
            || (0x80..0xA0).contains(&n)
            || (0xFDD0..=0xFDEF).contains(&n)
            || n & 0xFFFE == 0xFFFE)
    }) && b.iter().enumerate().all(|(i, &c)| {
        c != b'%'
            || b.get(i + 1..i + 3)
                .is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit))
    })
}

/// How often each (superstructure type, structure type) pair of the tables
/// occurs in a stream, typed the way [`check`] types it. The count is per
/// superstructure instance maximum, so `2` means "repeated somewhere".
pub fn coverage(text: &str, target: Target) -> HashMap<(&'static str, &'static str), usize> {
    let spec = target.spec();
    let records = tree::parse_with(text, target.grammar());
    let mut out: HashMap<(&'static str, &'static str), usize> = HashMap::new();
    fn walk(
        n: &Node,
        ty: &'static str,
        spec: &'static Spec,
        out: &mut HashMap<(&'static str, &'static str), usize>,
    ) {
        let mut here: HashMap<&'static str, usize> = HashMap::new();
        for c in &n.children {
            let cands: Vec<&Sub> = spec
                .subs
                .iter()
                .filter(|s| s.sup == ty && s.tag == c.tag)
                .collect();
            if let Some(sub) = pick_typed(spec, &cands, &c.payload) {
                *here.entry(sub.ty).or_default() += 1;
                walk(c, sub.ty, spec, out);
            }
        }
        for (t, k) in here {
            let e = out.entry((ty, t)).or_default();
            *e = (*e).max(k);
        }
    }
    let mut roots: HashMap<&'static str, usize> = HashMap::new();
    for r in &records {
        let cands: Vec<&Sub> = spec
            .subs
            .iter()
            .filter(|s| s.sup.is_empty() && s.tag == r.tag)
            .collect();
        if let Some(sub) = pick_typed(spec, &cands, &r.payload) {
            *roots.entry(sub.ty).or_default() += 1;
            walk(r, sub.ty, spec, &mut out);
        }
    }
    for (t, k) in roots {
        out.insert(("", t), k);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(out: &str, t: Target) -> Vec<&'static str> {
        let mut r: Vec<_> = check(out, t, None).into_iter().map(|i| i.rule).collect();
        r.sort_unstable();
        r.dedup();
        r
    }

    const OK7: &str = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n";
    const OK5: &str = "0 HEAD\n1 SOUR test\n1 SUBM @U1@\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n0 @U1@ SUBM\n1 NAME Sub\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n";

    #[test]
    fn minimal_streams_are_clean() {
        assert_eq!(check(OK7, Target::V70, None), []);
        assert_eq!(check(OK5, Target::V551, None), []);
    }

    #[test]
    fn pointer_rules_between_records() {
        let doc = |body: &str| format!("0 HEAD\n1 GEDC\n2 VERS 7.0\n{body}0 TRLR\n");
        let self_alias = doc("0 @I1@ INDI\n1 ALIA @I1@\n");
        assert_eq!(rules(&self_alias, Target::V70), ["alia-self"]);
        let cycle = doc(
            "0 @S1@ SOUR\n1 OBJE @O1@\n0 @O1@ OBJE\n1 FILE a.jpg\n2 FORM image/jpeg\n1 SOUR @S1@\n",
        );
        assert_eq!(rules(&cycle, Target::V70), ["pointer-cycle"]);
        let cut = cycle.replace("1 SOUR @S1@", "1 _SOUR @S1@");
        assert_eq!(rules(&cut, Target::V70), Vec::<&str>::new());
        let notes = doc("0 @N1@ SNOTE x\n1 SOUR @S1@\n0 @S1@ SOUR\n1 SNOTE @N1@\n");
        assert_eq!(rules(&notes, Target::V70), ["pointer-cycle"]);
    }

    #[test]
    fn line_rules() {
        assert_eq!(
            rules(&OK7.replace("1 NAME", "1  NAME"), Target::V70),
            ["delimiter"]
        );
        assert_eq!(
            rules(&OK7.replace("1 NAME", "01 NAME"), Target::V70),
            ["level-leading-zero"]
        );
        assert_eq!(
            rules(&OK7.replace("0 TRLR\n", "0 TRLR"), Target::V70),
            ["eol-final"]
        );
        assert_eq!(
            rules(&OK7.replacen('\n', "\r\n", 1), Target::V70),
            ["eol-mixed"]
        );
        assert_eq!(
            rules(&OK7.replace("0 @I1@", "\n0 @I1@"), Target::V70),
            ["blank-line"]
        );
        assert_eq!(
            rules(&OK7.replace("1 NAME", " 1 NAME"), Target::V70),
            ["leading-whitespace"]
        );
        assert_eq!(
            rules(&OK7.replace("Ann", "A\u{7}nn"), Target::V70),
            ["banned-char"]
        );
        assert_eq!(
            rules(
                &OK7.replace("1 NAME Ann /Example/", "2 NAME Ann /Example/"),
                Target::V70
            ),
            ["level-jump"]
        );
        assert!(rules(&OK7.replace("1 NAME", "1 name"), Target::V70).contains(&"tag-syntax"));
        assert!(rules(&OK7.replace("1 NAME", "1 _name"), Target::V70).contains(&"tag-syntax"));
        assert!(!rules(&OK7.replace("1 NAME", "1 _NAME"), Target::V70).contains(&"tag-syntax"));
    }

    #[test]
    fn xref_rules() {
        assert_eq!(
            rules(&OK7.replace("@I1@", "@VOID@"), Target::V70),
            ["xref-syntax"]
        );
        assert_eq!(
            rules(&OK7.replace("@I1@", "@i1@"), Target::V70),
            ["xref-syntax"]
        );
        assert_eq!(
            rules(&OK5.replace("@I1@", "@i#1@"), Target::V551),
            Vec::<&str>::new()
        );
        assert_eq!(
            rules(
                &OK5.replace("@I1@", "@I1234567890123456789012@"),
                Target::V551
            ),
            ["xref-syntax"]
        );
        assert_eq!(
            rules(&OK7.replace("1 NAME", "1 @X1@ NAME"), Target::V70),
            ["xref-on-substructure"]
        );
        let dup = OK7.replace("0 TRLR", "0 @I1@ INDI\n0 TRLR");
        assert_eq!(rules(&dup, Target::V70), ["xref-duplicate"]);
        let dangling = OK7.replace("0 TRLR", "0 @F1@ FAM\n1 HUSB @I9@\n1 WIFE @VOID@\n0 TRLR");
        assert_eq!(rules(&dangling, Target::V70), ["pointer-dangling"]);
        let wrong = OK7.replace("0 TRLR", "0 @F1@ FAM\n1 HUSB @F1@\n0 TRLR");
        assert_eq!(rules(&wrong, Target::V70), ["pointer-target"]);
    }

    #[test]
    fn escape_rules() {
        let n7 = |p: &str| OK7.replace("0 TRLR", &format!("0 @N1@ SNOTE {p}\n0 TRLR"));
        assert_eq!(rules(&n7("@@ leading"), Target::V70), Vec::<&str>::new());
        assert_eq!(rules(&n7("a@b"), Target::V70), Vec::<&str>::new());
        assert_eq!(rules(&n7("@ leading"), Target::V70), ["escape"]);
        let n5 = |p: &str| OK5.replace("0 TRLR", &format!("0 @N1@ NOTE {p}\n0 TRLR"));
        assert_eq!(rules(&n5("a@@b"), Target::V551), Vec::<&str>::new());
        assert_eq!(
            rules(&n5("@#DJULIAN@ 1700"), Target::V551),
            Vec::<&str>::new()
        );
        assert_eq!(rules(&n5("a@b"), Target::V551), ["escape"]);
        assert_eq!(
            rules(&n5("@NoTe ref@"), Target::V551),
            ["payload-pointer", "pointer-dangling"]
        );
    }

    #[test]
    fn length_and_continuation_rules() {
        let long = OK5.replace(
            "0 TRLR",
            &format!("0 @N1@ NOTE {}\n0 TRLR", "x".repeat(250)),
        );
        assert_eq!(rules(&long, Target::V551), ["line-length"]);
        let split = OK5.replace("0 TRLR", "0 @N1@ NOTE a \n1 CONC b\n0 TRLR");
        assert_eq!(rules(&split, Target::V551), ["conc-split-space"]);
        let conc7 = OK7.replace("0 TRLR", "0 @N1@ SNOTE a\n1 CONC b\n0 TRLR");
        assert_eq!(rules(&conc7, Target::V70), ["conc"]);
        let late = OK7.replace("0 TRLR", "0 @N1@ SNOTE a\n1 LANG en\n1 CONT b\n0 TRLR");
        assert_eq!(rules(&late, Target::V70), ["cont-position"]);
        let cont = format!("1 CONT {}\n", "x".repeat(120)).repeat(300);
        let big = OK5.replace("0 TRLR\n", &format!("0 @N1@ NOTE a\n{cont}0 TRLR\n"));
        assert_eq!(rules(&big, Target::V551), ["record-size"]);
    }

    #[test]
    fn header_rules() {
        assert_eq!(
            rules(
                &OK7.replace("2 VERS 7.0", "2 VERS 7.0\n2 FORM LINEAGE-LINKED"),
                Target::V70
            ),
            ["v7-gedc-form"]
        );
        assert_eq!(
            rules(
                &OK7.replace("2 VERS 7.0", "2 VERS 7.0\n1 CHAR UTF-8"),
                Target::V70
            ),
            ["v7-char"]
        );
        assert_eq!(
            rules(&OK5.replace("CHAR UTF-8", "CHAR ANSEL"), Target::V551),
            ["char-mismatch"]
        );
        assert_eq!(
            rules(OK7, Target::V551),
            ["cardinality-min", "gedc-vers", "head-required"]
        );
        assert_eq!(
            rules(&OK7.replace("0 TRLR\n", ""), Target::V70),
            ["trlr-last"]
        );
        assert_eq!(
            rules(&OK7.replace("0 TRLR\n", "0 TRLR\n1 _X y\n"), Target::V70),
            ["trlr-children"]
        );
    }

    #[test]
    fn schma_rule_needs_the_input() {
        let input = OK7
            .replace(
                "2 VERS 7.0",
                "2 VERS 7.0\n1 SCHMA\n2 TAG _X https://example.com/x",
            )
            .replace("1 NAME", "1 _X y\n1 NAME");
        let output = OK7.replace("1 NAME", "1 _X y\n1 NAME");
        assert_eq!(
            check(&output, Target::V70, Some(&input))
                .iter()
                .map(|i| i.rule)
                .collect::<Vec<_>>(),
            ["schma-missing"]
        );
        assert_eq!(check(&input, Target::V70, Some(&input)), []);
    }

    #[test]
    fn payload_rules() {
        let ind = |l: &str| OK7.replace("1 NAME Ann /Example/", l);
        assert_eq!(rules(&ind("1 SEX M"), Target::V70), Vec::<&str>::new());
        assert_eq!(rules(&ind("1 SEX _X"), Target::V70), Vec::<&str>::new());
        assert_eq!(rules(&ind("1 SEX male"), Target::V70), ["payload-enum"]);
        assert_eq!(rules(&ind("1 BIRT Y"), Target::V70), Vec::<&str>::new());
        assert_eq!(rules(&ind("1 BIRT yes"), Target::V70), ["payload-y"]);
        assert_eq!(rules(&ind("1 NAME a/b/c/d"), Target::V70), ["payload-name"]);
        assert_eq!(
            rules(&ind("1 RESN CONFIDENTIAL, LOCKED"), Target::V70),
            Vec::<&str>::new()
        );
        assert_eq!(rules(&ind("1 RESN SECRET"), Target::V70), ["payload-enum"]);
        let p5 = |l: &str| OK5.replace("1 NAME Ann /Example/", l);
        assert_eq!(rules(&p5("1 SEX m"), Target::V551), ["payload-enum"]);
        assert_eq!(
            rules(&p5("1 FAMC @I1@\n2 PEDI stepchild"), Target::V551),
            ["payload-enum", "pointer-target"]
        );
    }

    #[test]
    fn date_age_time_grammars() {
        let s7 = &V70;
        for ok in [
            "",
            "1900",
            "1 JAN 1900",
            "JULIAN 44 BCE",
            "HEBREW 1 TSH 5000",
            "_CAL 1 _M 2000",
            "BET 1900 AND 1910",
            "FROM 1900 TO 1910",
            "TO 1900",
            "ABT JAN 1900",
            "AFT 1900 BCE",
        ] {
            assert!(date70(ok, Pay::Date, s7), "{ok}");
        }
        for bad in [
            "1 jan 1900",
            "1 JAN",
            "BET 1900",
            "JAN",
            "1 JAN",
            "FRENCH_R 1 JAN 1900",
            "1900 BCE BCE",
            "GREGORIAN  1900",
        ] {
            assert!(!date70(bad, Pay::Date, s7), "{bad}");
        }
        assert!(date70("1 JAN 2000", Pay::DateExact, s7));
        assert!(!date70("JAN 2000", Pay::DateExact, s7));
        let s5 = &V551;
        for ok in [
            "1900",
            "1 JAN 1699/00",
            "@#DJULIAN@ 1 JAN 1700",
            "@#DFRENCH R@ 1 VEND 1",
            "44 B.C.",
            "INT 1900 (about then)",
            "(phrase)",
            "BET 1900 AND 1910",
            "@#DUNKNOWN@ anything",
        ] {
            assert!(date551(ok, Pay::Date, s5), "{ok}");
        }
        for bad in [
            "1 JAN 1699/2000",
            "@#DJULIAN@ 1 VEND 1700",
            "BET 1900",
            "1900 BCE",
            "jan 1900",
        ] {
            assert!(!date551(bad, Pay::Date, s5), "{bad}");
        }
        for ok in ["1y", "< 1y", "> 79y 1m 1w 1d", "1y 30m 100w 400d", "0d"] {
            assert!(age_ok(ok, true), "{ok}");
        }
        for bad in ["79", ">79y", "1d 1m", "CHILD", "y"] {
            assert!(!age_ok(bad, true), "{bad}");
        }
        assert!(age_ok("<1y", false) && age_ok("CHILD", false) && !age_ok("1w", false));
        assert!(time_ok("2:50:00.00Z", true) && time_ok("23:59", true));
        assert!(!time_ok("24:00:00", true) && !time_ok("2:5", true) && !time_ok("2:60", true));
    }
}
