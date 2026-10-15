//! Semantic equivalence of two GEDCOM streams.
//!
//! Both streams are read by the independent reader in `tree.rs`, put in a
//! canonical form and compared as multisets of `path = payload` lines. The
//! canonical form applies these rules (1–5 are the gedcom7code/test-files
//! README rules, 6–9 were added for mismatches found while reviewing):
//!
//! 1. An empty payload equals an absent one.
//! 2. Substructure order matters only among siblings with the same tag.
//! 3. Dates compare by meaning, not by version spelling: `GREGORIAN ` (7.0)
//!    and `@#DGREGORIAN@ ` (5.5.1) are dropped, the other 5.5.1 escapes
//!    equal the 7.0 calendar keywords (`@#DROMAN@`, `ROMAN` and `_ROMAN`
//!    alike), `B.C.` equals `BCE`, keywords and months compare without
//!    case, a dual year's suffix is two digits (`613/4` is `613/14`), and
//!    words outside parentheses are single-spaced.
//! 4. Extension tags declared in `HEAD.SCHMA` compare by URI.
//! 5. Xrefs are arbitrary: records are matched by content and by the content
//!    of what they point to (iterated, Weisfeiler-Lehman style), then by xref.
//! 6. `@@` is decoded per version before comparing (done by `tree.rs`).
//! 7. 5.5.1 enumeration payloads compare case-insensitively.
//! 8. Optionally, `GIVN a b` equals `GIVN a` + `GIVN b` (name pieces).
//! 9. Ages compare by meaning: `<1y` equals `< 1y`, units of any case and
//!    spacing are equal (`0 Y` is `0y`), a week is seven days (5.5.1 has
//!    no weeks), and a number alone counts years, as the test-files convert
//!    it.
//! 10. Times compare with zero-padded fields: `2:5` equals `02:05`.
//! 11. File paths compare as the path they name: a 7.x `FILE` is a
//!     percent-encoded URL string and its GEDZIP entry name, like a 5.5.1
//!     file reference, is not (7.0.17), so percent-encodings are decoded
//!     (`%3a`, `%3A` and `:` are equal), backslashes are slashes, and a
//!     `file` URL is its path (`file:///C:/a` is `C:/a`).
//!
//! Conversion mode (5.5.1 against its 7.0 conversion) also treats a NOTE
//! record referenced once, an inline NOTE and a single-use SNOTE as equal, and
//! ignores the HEAD lines a converter regenerates.

use super::tree::{self, Node, Payload, Version};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

#[derive(Clone, Debug)]
pub struct Options {
    /// HEAD paths ignored on both sides, such as `CHAR` or `GEDC/FORM`.
    pub ignore_head: Vec<String>,
    /// Rule 8.
    pub name_piece_lists: bool,
    /// Conversion mode (see the module documentation).
    pub conversion: bool,
}

impl Default for Options {
    /// Same-version comparison of a reader's input with a writer's output:
    /// the header lines that describe the bytes or the producing system are
    /// ignored, because the writer must regenerate them (their correctness is
    /// the output checker's job).
    fn default() -> Self {
        Options {
            ignore_head: ["CHAR", "GEDC/FORM", "SOUR", "DATE", "FILE", "SUBM", "DEST"]
                .map(String::from)
                .to_vec(),
            name_piece_lists: false,
            conversion: false,
        }
    }
}

impl Options {
    pub fn conversion() -> Self {
        Options {
            conversion: true,
            ..Options::default()
        }
    }

    pub fn exact_head() -> Self {
        Options {
            ignore_head: Vec::new(),
            ..Options::default()
        }
    }
}

/// Compares two streams; returns the differences as `- path = value` and
/// `+ path = value` lines, empty when equivalent.
pub fn compare(a: &str, b: &str, opts: &Options) -> Vec<String> {
    let (va, ta) = tree::parse(a);
    let (vb, tb) = tree::parse(b);
    compare_trees(&ta, va, &tb, vb, opts)
}

pub fn compare_trees(
    a: &[Node],
    va: Version,
    b: &[Node],
    vb: Version,
    opts: &Options,
) -> Vec<String> {
    let ca = canonical(a, va, opts);
    let cb = canonical(b, vb, opts);
    // Map b's xrefs onto a's: first by structural label, then by same xref.
    let la = labels(&ca);
    let lb = labels(&cb);
    let mut by_label: HashMap<(String, u64), Vec<String>> = HashMap::new();
    for r in &ca {
        if let Some(x) = &r.xref {
            by_label
                .entry((r.tag.clone(), la[x]))
                .or_default()
                .push(x.clone());
        }
    }
    let a_xrefs: HashSet<&String> = ca.iter().filter_map(|r| r.xref.as_ref()).collect();
    let mut map: HashMap<String, String> = HashMap::new();
    let mut used: HashSet<String> = HashSet::new();
    for r in &cb {
        if let Some(x) = &r.xref
            && let Some(cands) = by_label.get_mut(&(r.tag.clone(), lb[x]))
            && let Some(pos) = cands.iter().position(|c| !used.contains(c))
        {
            let c = cands.remove(pos);
            used.insert(c.clone());
            map.insert(x.clone(), c);
        }
    }
    for r in &cb {
        if let Some(x) = &r.xref
            && !map.contains_key(x)
            && a_xrefs.contains(x)
            && !used.contains(x)
        {
            used.insert(x.clone());
            map.insert(x.clone(), x.clone());
        }
    }
    let own: HashSet<String> = cb.iter().filter_map(|r| r.xref.clone()).collect();
    let cb: Vec<Node> = cb.into_iter().map(|r| rename(r, &map, &own)).collect();
    let fa = lines(&ca);
    let fb = lines(&cb);
    let mut out = Vec::new();
    for (k, n) in &fa {
        let m = fb.get(k).copied().unwrap_or(0);
        for _ in m..*n {
            out.push(format!("- {} = {:?}", k.0, k.1));
        }
    }
    for (k, n) in &fb {
        let m = fa.get(k).copied().unwrap_or(0);
        for _ in m..*n {
            out.push(format!("+ {} = {:?}", k.0, k.1));
        }
    }
    out
}

/// Payloads and structures of `input` that appear nowhere in `output`
/// ("nothing lost", for leniency cases whose output is normalised).
///
/// The output of invalid input is repaired by the writer (the table of
/// `spec::conform`), so data counts as kept in the forms that table gives
/// it:
/// - tags compare without a leading underscore and case-insensitively, so
///   a structure relocated as an extension (repairs a, e, f) counts as
///   kept, and its payload is read as the standard tag's (`_AGE`, `_TIME`);
/// - enumeration values compare without case: 5.5.1 controlled values are
///   case-insensitive (p. 21), and repair b writes a value of the wrong
///   case in the version's own (`SEX m` is `SEX M`);
/// - a date kept as a 5.5.1 date phrase (`(7/11/1959)`, repair d) is its
///   text; a 7.x one is in its `PHRASE`, compared like any text.
///
/// Banned control characters are ignored; xrefs and pointers are not
/// compared.
pub fn lost(input: &str, output: &str) -> Vec<String> {
    let bag = |text: &str| {
        let (v, t) = tree::parse(text);
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for r in &t {
            if r.tag == "HEAD" || r.tag == "TRLR" {
                continue;
            }
            r.walk(&mut Vec::new(), &mut |path, n| {
                let tag = n.tag.trim_start_matches('_').to_ascii_uppercase();
                *m.entry(format!("tag {tag}")).or_default() += 1;
                if let Payload::Text(s) = &n.payload {
                    let mut s: String = s.chars().filter(|c| !is_banned(*c)).collect();
                    if tag == "DATE"
                        && let Some(phrase) = s.strip_prefix('(').and_then(|r| r.strip_suffix(')'))
                    {
                        s = phrase.to_string();
                    }
                    let mut s = normalise_value(&tag, &s, v);
                    let parent = path.len().checked_sub(2).and_then(|i| path.get(i));
                    if is_enum(&tag, parent.map(|p| p.trim_start_matches('_'))) {
                        s = s.to_ascii_uppercase();
                    }
                    if !s.is_empty() {
                        // A text may be re-split differently: compare by lines.
                        for part in s.split('\n') {
                            if !part.is_empty() {
                                *m.entry(format!("text {part}")).or_default() += 1;
                            }
                        }
                    }
                }
            });
        }
        m
    };
    let a = bag(input);
    let b = bag(output);
    let mut out = Vec::new();
    for (k, n) in &a {
        let m = b.get(k).copied().unwrap_or(0);
        if m < *n {
            out.push(format!("{k} (x{})", n - m));
        }
    }
    out
}

pub fn is_banned(c: char) -> bool {
    (c < ' ' && c != '\t' && c != '\n') || c == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&c)
}

fn canonical(records: &[Node], v: Version, opts: &Options) -> Vec<Node> {
    let mut schma: HashMap<String, String> = HashMap::new();
    if let Some(head) = records.iter().find(|r| r.tag == "HEAD")
        && let Some(s) = head.child("SCHMA")
    {
        for t in s.children_tagged("TAG") {
            if let Some((tag, uri)) = t.payload.as_str().split_once(' ') {
                schma.insert(tag.to_string(), uri.trim().to_string());
            }
        }
    }
    let mut out: Vec<Node> = records
        .iter()
        .filter(|r| r.tag != "TRLR")
        .map(|r| {
            let mut r = r.clone();
            if r.tag == "HEAD" {
                strip_head(&mut r, opts);
            }
            norm(&mut r, None, v, &schma, opts);
            r
        })
        .collect();
    if opts.conversion {
        inline_single_use_notes(&mut out);
    }
    // Rule 5: an xref nothing points to carries no information.
    let mut referenced = HashSet::new();
    for r in &out {
        r.walk(&mut Vec::new(), &mut |_, n| {
            if let Payload::Pointer(p) = &n.payload {
                referenced.insert(p.clone());
            }
        });
    }
    for r in &mut out {
        if r.xref.as_ref().is_some_and(|x| !referenced.contains(x)) {
            r.xref = None;
        }
    }
    out
}

fn strip_head(head: &mut Node, opts: &Options) {
    let mut ignore: Vec<String> = opts.ignore_head.clone();
    if opts.conversion {
        ignore.extend(["GEDC/VERS", "SUBN", "LANG"].map(String::from));
    }
    for path in &ignore {
        let parts: Vec<&str> = path.split('/').collect();
        remove_path(head, &parts);
    }
    // SCHMA declarations compare by URI only (rule 4).
    if let Some(s) = head.children.iter_mut().find(|c| c.tag == "SCHMA") {
        for t in s.children.iter_mut().filter(|t| t.tag == "TAG") {
            if let Some((_, uri)) = t.payload.as_str().split_once(' ') {
                t.payload = Payload::Text(uri.trim().to_string());
            }
        }
    }
}

fn remove_path(n: &mut Node, parts: &[&str]) {
    match parts {
        [] => {}
        [last] => n.children.retain(|c| c.tag != *last),
        [first, rest @ ..] => {
            for c in n.children.iter_mut().filter(|c| c.tag == *first) {
                remove_path(c, rest);
            }
        }
    }
}

const NAME_PIECES: [&str; 6] = ["NPFX", "GIVN", "NICK", "SPFX", "SURN", "NSFX"];

/// The tags of enumeration payloads (`TYPE` under `NAME`, `FONE` or
/// `ROMN` only), for leniency.
fn is_enum(tag: &str, parent: Option<&str>) -> bool {
    match tag {
        "TYPE" => matches!(parent, Some("NAME" | "FONE" | "ROMN")),
        "PEDI" | "STAT" | "ADOP" | "MEDI" | "RESN" | "SEX" | "ROLE" | "QUAY" | "KIND" => true,
        _ => false,
    }
}
const ENUM_551: [&str; 8] = [
    "PEDI", "STAT", "ADOP", "MEDI", "RESN", "SEX", "ROLE", "TYPE",
];

fn norm(
    n: &mut Node,
    parent: Option<&str>,
    v: Version,
    schma: &HashMap<String, String>,
    opts: &Options,
) {
    if let Some(uri) = schma.get(&n.tag) {
        n.tag = format!("<{uri}>");
    }
    let tag = n.tag.clone();
    if let Payload::Text(s) = &n.payload {
        let mut s = normalise_value(&tag, s, v);
        if v == Version::V551
            && ENUM_551.contains(&tag.as_str())
            && (tag != "TYPE" || parent == Some("NAME"))
        {
            s = s.to_ascii_uppercase();
        }
        n.payload = if s.is_empty() {
            Payload::None
        } else {
            Payload::Text(s)
        };
    }
    if opts.conversion && n.tag == "NOTE" && parent.is_none() {
        n.tag = "SNOTE".into();
    }
    if opts.conversion && n.tag == "NOTE" && matches!(n.payload, Payload::Pointer(_)) {
        n.tag = "SNOTE".into();
    }
    for c in &mut n.children {
        norm(c, Some(&tag), v, schma, opts);
    }
    if opts.name_piece_lists && tag == "NAME" {
        let mut split = Vec::new();
        for c in std::mem::take(&mut n.children) {
            match (&c.payload, NAME_PIECES.contains(&c.tag.as_str())) {
                (Payload::Text(t), true) if t.contains(' ') => {
                    for part in t.split(' ').filter(|p| !p.is_empty()) {
                        let mut p = c.clone();
                        p.payload = Payload::Text(part.into());
                        split.push(p);
                    }
                }
                _ => split.push(c),
            }
        }
        n.children = split;
    }
    // Rule 2: stable sort keeps the order within a tag.
    n.children.sort_by(|a, b| a.tag.cmp(&b.tag));
}

/// Normalises one decoded payload (rules 3 and 9).
pub fn normalise_value(tag: &str, s: &str, v: Version) -> String {
    let mut s = s.to_string();
    if tag == "DATE" || tag == "SDATE" {
        s = normalise_date(&s);
        s = s.replace("@#DGREGORIAN@ ", "");
        if v == Version::V70 || s.starts_with("GREGORIAN ") || s.contains(" GREGORIAN ") {
            s = s.replace("GREGORIAN ", "");
        }
    }
    if tag == "AGE" {
        s = normalise_age(&s.replace("< ", "<").replace("> ", ">"));
    }
    if tag == "TIME" {
        s = normalise_time(&s);
    }
    if tag == "FILE" {
        s = normalise_file(&s);
    }
    s
}

/// Rule 11: a file reference as the path it names.
fn normalise_file(s: &str) -> String {
    let s = s.replace('\\', "/");
    let lower = s.to_ascii_lowercase();
    let s = if let Some(rest) = lower.strip_prefix("file:///").map(|_| &s[8..]) {
        let b = rest.as_bytes();
        let drive = b.first().is_some_and(u8::is_ascii_alphabetic) && b.get(1) == Some(&b':');
        if drive {
            rest.to_string()
        } else {
            format!("/{rest}")
        }
    } else if lower.starts_with("file://") {
        format!("//{}", &s[7..])
    } else {
        s
    };
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |at: usize| b.get(at).and_then(|&h| char::from(h).to_digit(16));
        if let (b'%', Some(hi), Some(lo)) = (c, hex(i + 1), hex(i + 2)) {
            out.push(u8::try_from(hi * 16 + lo).unwrap_or(b'%'));
            i += 3;
        } else {
            out.push(c);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The keywords, calendars, months and epochs of dates, which compare
/// without case (5.5.1 p. 21).
const DATE_WORDS: &[&str] = &[
    "ABT",
    "CAL",
    "EST",
    "BEF",
    "AFT",
    "BET",
    "AND",
    "FROM",
    "TO",
    "INT",
    "GREGORIAN",
    "JULIAN",
    "HEBREW",
    "FRENCH_R",
    "ROMAN",
    "UNKNOWN",
    "BCE",
    "B.C.",
    "JAN",
    "FEB",
    "MAR",
    "APR",
    "MAY",
    "JUN",
    "JUL",
    "AUG",
    "SEP",
    "OCT",
    "NOV",
    "DEC",
    "VEND",
    "BRUM",
    "FRIM",
    "NIVO",
    "PLUV",
    "VENT",
    "GERM",
    "FLOR",
    "PRAI",
    "MESS",
    "THER",
    "FRUC",
    "COMP",
    "TSH",
    "CSH",
    "KSL",
    "TVT",
    "SHV",
    "ADR",
    "ADS",
    "NSN",
    "IYR",
    "SVN",
    "TMZ",
    "AAV",
    "ELL",
];

/// A 5.5.1 dual year with its other year's last two digits: `613/4` is the
/// year 613 or 614, written `613/14`. `None` for any other word.
fn dual_year(word: &str) -> Option<String> {
    let (year, digits) = word.split_once('/')?;
    if digits.is_empty() || digits.len() > 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: u64 = year.parse().ok()?;
    let modulus = 10u64.pow(u32::try_from(digits.len()).ok()?);
    let mut other = year - year % modulus + digits.parse::<u64>().ok()?;
    if other <= year {
        other += modulus;
    }
    Some(format!("{year}/{:02}", other % 100))
}

/// Rule 3: the words of a date outside parentheses, single-spaced, with
/// keywords and months upper case, dual years with two digits, and 5.5.1
/// escapes and `B.C.` in their 7.0 spelling.
fn normalise_date(s: &str) -> String {
    let (head, phrase) = s.split_at(s.find('(').unwrap_or(s.len()));
    let mut words: Vec<String> = Vec::new();
    let mut rest = head.trim();
    while !rest.is_empty() {
        // An escape is one word even with its space: `@#DFRENCH R@`.
        let end = if let Some(escape) = rest.strip_prefix("@#") {
            escape.find('@').map_or(rest.len(), |at| at + 3)
        } else {
            rest.find(char::is_whitespace).unwrap_or(rest.len())
        };
        let word = rest[..end].to_string();
        rest = rest[end..].trim_start();
        let upper = word.to_ascii_uppercase();
        let word = if DATE_WORDS.contains(&upper.as_str()) || upper.starts_with("@#D") {
            upper
        } else {
            dual_year(&word).unwrap_or(word)
        };
        let word = match word.as_str() {
            "@#DJULIAN@" => "JULIAN".to_string(),
            "@#DHEBREW@" => "HEBREW".to_string(),
            "@#DFRENCH R@" => "FRENCH_R".to_string(),
            "@#DROMAN@" | "ROMAN" => "_ROMAN".to_string(),
            "@#DUNKNOWN@" | "UNKNOWN" => "_UNKNOWN".to_string(),
            "B.C." => "BCE".to_string(),
            _ => word,
        };
        words.push(word);
    }
    let mut out = words.join(" ");
    if !phrase.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(phrase);
    }
    out
}

/// Rule 9: an age's `<n><unit>` parts, lower case, weeks as days, in the
/// order years, months, days; anything else as it is.
fn normalise_age(s: &str) -> String {
    let (bound, body) = match s.strip_prefix(['<', '>']) {
        Some(body) => (&s[..1], body),
        None => ("", s),
    };
    let mut counts = [None::<u64>; 3];
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let mut rest = compact.as_str();
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let Ok(n) = rest[..digits].parse::<u64>() else {
            return s.to_string();
        };
        // A number without unit counts years, as the test-files convert it.
        let unit = match rest[digits..].chars().next() {
            None if counts == [None; 3] => 'y',
            Some(unit) => unit,
            None => return s.to_string(),
        };
        if digits == rest.len() {
            counts[0] = Some(n);
            break;
        }
        let (slot, n) = match unit.to_ascii_lowercase() {
            'y' => (0, n),
            'm' => (1, n),
            'w' => (2, n.saturating_mul(7)),
            'd' => (2, n),
            _ => return s.to_string(),
        };
        counts[slot] = Some(counts[slot].unwrap_or(0).saturating_add(n));
        rest = &rest[digits + 1..];
    }
    let parts: Vec<String> = counts
        .iter()
        .zip(['y', 'm', 'd'])
        .filter_map(|(n, unit)| n.map(|n| format!("{n}{unit}")))
        .collect();
    if parts.is_empty() {
        return s.to_string();
    }
    format!("{bound}{}", parts.join(" "))
}

/// Rule 10: a time's numeric fields zero-padded to two digits.
fn normalise_time(s: &str) -> String {
    let (clock, rest) = s.split_at(s.find(['.', 'Z', 'z']).unwrap_or(s.len()));
    if !clock
        .split(':')
        .all(|f| !f.is_empty() && f.len() <= 2 && f.bytes().all(|b| b.is_ascii_digit()))
    {
        return s.to_string();
    }
    let padded: Vec<String> = clock.split(':').map(|f| format!("{f:0>2}")).collect();
    format!("{}{}", padded.join(":"), rest.to_ascii_uppercase())
}

/// Conversion rule: a NOTE/SNOTE record pointed to exactly once is inlined.
fn inline_single_use_notes(records: &mut Vec<Node>) {
    let mut uses: HashMap<String, usize> = HashMap::new();
    for r in records.iter() {
        r.walk(&mut Vec::new(), &mut |_, n| {
            if let Payload::Pointer(p) = &n.payload {
                *uses.entry(p.clone()).or_default() += 1;
            }
        });
    }
    let single: HashMap<String, Node> = records
        .iter()
        .filter(|r| r.tag == "SNOTE")
        .filter_map(|r| r.xref.clone().map(|x| (x, r)))
        .filter(|(x, _)| uses.get(x) == Some(&1))
        .map(|(x, r)| (x, r.clone()))
        .collect();
    if single.is_empty() {
        return;
    }
    records.retain(|r| r.xref.as_ref().is_none_or(|x| !single.contains_key(x)));
    fn inline(n: &mut Node, single: &HashMap<String, Node>) {
        for c in &mut n.children {
            if c.tag == "SNOTE"
                && let Payload::Pointer(p) = &c.payload
                && let Some(rec) = single.get(p)
            {
                c.tag = "NOTE".into();
                c.payload = rec.payload.clone();
                c.children.extend(rec.children.iter().cloned());
                c.children.sort_by(|a, b| a.tag.cmp(&b.tag));
            }
            inline(c, single);
        }
    }
    for r in records.iter_mut() {
        inline(r, &single);
    }
}

/// Weisfeiler-Lehman labels of the records that have an xref: a record's
/// label hashes its content, the labels of the records it points to and the
/// labels of the records that point to it (with the pointing tag).
fn labels(records: &[Node]) -> HashMap<String, u64> {
    let mut incoming: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        r.walk(&mut Vec::new(), &mut |path, n| {
            if let Payload::Pointer(p) = &n.payload {
                incoming
                    .entry(p.clone())
                    .or_default()
                    .push((i, path.join("/")));
            }
        });
    }
    let mut lab: HashMap<String, u64> = HashMap::new();
    let mut by_index: Vec<u64> = vec![0; records.len()];
    for _ in 0..4 {
        let mut next = HashMap::new();
        let mut next_idx = vec![0; records.len()];
        for (i, r) in records.iter().enumerate() {
            let mut h = DefaultHasher::new();
            hash_node(r, &lab, &mut h);
            if let Some(x) = &r.xref {
                let mut inc: Vec<(u64, &str)> = incoming
                    .get(x)
                    .map(|v| v.iter().map(|(j, p)| (by_index[*j], p.as_str())).collect())
                    .unwrap_or_default();
                inc.sort_unstable();
                inc.hash(&mut h);
            }
            let l = h.finish();
            next_idx[i] = l;
            if let Some(x) = &r.xref {
                next.insert(x.clone(), l);
            }
        }
        lab = next;
        by_index = next_idx;
    }
    lab
}

fn hash_node(n: &Node, lab: &HashMap<String, u64>, h: &mut DefaultHasher) {
    n.tag.hash(h);
    match &n.payload {
        Payload::Pointer(p) => match lab.get(p) {
            Some(l) => ("ptr", l).hash(h),
            None if lab.is_empty() => "ptr".hash(h),
            None => ("dangling", p).hash(h),
        },
        p => p.hash(h),
    }
    n.children.len().hash(h);
    for c in &n.children {
        hash_node(c, lab, h);
    }
}

/// Renames b's xrefs onto a's. An unmatched b record keeps its xref with a
/// `'`; a pointer to no record of b (dangling, `@VOID@`, network) is kept.
fn rename(mut n: Node, map: &HashMap<String, String>, own: &HashSet<String>) -> Node {
    if let Some(x) = &n.xref {
        n.xref = Some(map.get(x).cloned().unwrap_or_else(|| format!("{x}'")));
    }
    if let Payload::Pointer(p) = &n.payload {
        if let Some(m) = map.get(p) {
            n.payload = Payload::Pointer(m.clone());
        } else if own.contains(p) {
            n.payload = Payload::Pointer(format!("{p}'"));
        }
    }
    n.children = n
        .children
        .into_iter()
        .map(|c| rename(c, map, own))
        .collect();
    n
}

/// `path = payload` lines with their multiplicity; a path element carries
/// the index of the structure among its same-tag siblings.
fn lines(records: &[Node]) -> BTreeMap<(String, String), usize> {
    let mut out = BTreeMap::new();
    let mut rec_seen: HashMap<String, usize> = HashMap::new();
    for r in records {
        let root = match &r.xref {
            Some(x) => format!("{}{}", r.tag, x),
            None => {
                let k = rec_seen.entry(r.tag.clone()).or_default();
                *k += 1;
                format!("{}#{}", r.tag, *k - 1)
            }
        };
        emit(r, &root, &mut out);
    }
    out
}

fn emit(n: &Node, path: &str, out: &mut BTreeMap<(String, String), usize>) {
    *out.entry((path.to_string(), n.payload.as_str().to_string()))
        .or_default() += 1;
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for c in &n.children {
        let k = seen.entry(&c.tag).or_default();
        let p = if *k == 0 {
            format!("{path}/{}", c.tag)
        } else {
            format!("{path}/{}[{}]", c.tag, k)
        };
        *k += 1;
        emit(c, &p, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H7: &str = "0 HEAD\n1 GEDC\n2 VERS 7.0\n";
    const H5: &str = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n";

    fn eq(a: &str, b: &str) -> bool {
        compare(a, b, &Options::exact_head()).is_empty()
    }

    #[test]
    fn rule1_empty_payload_is_absent() {
        assert!(eq(
            &format!("{H7}0 @I1@ INDI\n1 BIRT \n0 TRLR"),
            &format!("{H7}0 @I1@ INDI\n1 BIRT\n0 TRLR")
        ));
    }

    #[test]
    fn rule2_order_matters_within_a_tag_only() {
        let a = format!("{H7}0 @I1@ INDI\n1 SEX M\n1 NOTE a\n1 NOTE b\n0 TRLR");
        let b = format!("{H7}0 @I1@ INDI\n1 NOTE a\n1 NOTE b\n1 SEX M\n0 TRLR");
        let c = format!("{H7}0 @I1@ INDI\n1 NOTE b\n1 NOTE a\n1 SEX M\n0 TRLR");
        assert!(eq(&a, &b));
        assert!(!eq(&a, &c));
    }

    #[test]
    fn rule3_gregorian_prefix() {
        assert!(eq(
            &format!("{H7}0 @I1@ INDI\n1 BIRT\n2 DATE GREGORIAN 1930\n0 TRLR"),
            &format!("{H7}0 @I1@ INDI\n1 BIRT\n2 DATE 1930\n0 TRLR")
        ));
        assert!(eq(
            &format!("{H5}0 @I1@ INDI\n1 BIRT\n2 DATE @#DGREGORIAN@ 1930\n0 TRLR"),
            &format!("{H5}0 @I1@ INDI\n1 BIRT\n2 DATE 1930\n0 TRLR")
        ));
        assert!(!eq(
            &format!("{H5}0 @I1@ INDI\n1 BIRT\n2 DATE @#DJULIAN@ 1930\n0 TRLR"),
            &format!("{H5}0 @I1@ INDI\n1 BIRT\n2 DATE 1930\n0 TRLR")
        ));
    }

    #[test]
    fn rule4_extension_tags_compare_by_uri() {
        let a = "0 HEAD\n1 GEDC\n2 VERS 7.0\n1 SCHMA\n2 TAG _X https://example.com/t\n0 @X1@ _X payload\n0 TRLR";
        let b = "0 HEAD\n1 GEDC\n2 VERS 7.0\n1 SCHMA\n2 TAG _TYPE https://example.com/t\n0 @X1@ _TYPE payload\n0 TRLR";
        assert!(eq(a, b));
        let c = format!("{H7}0 @X1@ _TYPE payload\n0 TRLR");
        assert!(!eq(a, &c));
    }

    #[test]
    fn rule5_xrefs_are_arbitrary() {
        let a = format!("{H7}0 @I1@ INDI\n1 FAMS @F1@\n0 @F1@ FAM\n1 HUSB @I1@\n0 TRLR");
        let b = format!("{H7}0 @X9@ FAM\n1 HUSB @P2@\n0 @P2@ INDI\n1 FAMS @X9@\n0 TRLR");
        assert!(eq(&a, &b));
        // Two look-alike people: the links decide which is which.
        let a = format!(
            "{H7}0 @I1@ INDI\n1 SEX M\n0 @I2@ INDI\n1 SEX M\n0 @F1@ FAM\n1 HUSB @I1@\n0 TRLR"
        );
        let b =
            format!("{H7}0 @A@ INDI\n1 SEX M\n0 @B@ INDI\n1 SEX M\n0 @F1@ FAM\n1 HUSB @B@\n0 TRLR");
        assert!(eq(&a, &b));
        let c =
            format!("{H7}0 @A@ INDI\n1 SEX M\n0 @B@ INDI\n1 SEX F\n0 @F1@ FAM\n1 HUSB @B@\n0 TRLR");
        assert!(!eq(&a, &c));
    }

    #[test]
    fn records_without_xref_may_gain_one() {
        let a = format!("{H7}0 INDI\n1 SEX M\n0 TRLR");
        let b = format!("{H7}0 @I1@ INDI\n1 SEX M\n0 TRLR");
        assert!(eq(&a, &b));
    }

    #[test]
    fn rule6_at_signs_decode_per_version() {
        let a = format!("{H7}0 @N1@ SNOTE @@x@@y\n0 TRLR");
        let b = format!("{H5}0 @N1@ NOTE @@x@@y\n0 TRLR");
        let (va, ta) = tree::parse(&a);
        let (vb, tb) = tree::parse(&b);
        assert_eq!(ta[1].payload, Payload::Text("@x@@y".into()));
        assert_eq!(tb[1].payload, Payload::Text("@x@y".into()));
        assert!(!compare_trees(&ta, va, &tb, vb, &Options::conversion()).is_empty());
    }

    #[test]
    fn rule7_551_enums_ignore_case() {
        let a = format!("{H5}0 @I1@ INDI\n1 FAMC @F1@\n2 PEDI birth\n0 @F1@ FAM\n0 TRLR");
        let b = format!("{H5}0 @I1@ INDI\n1 FAMC @F1@\n2 PEDI BIRTH\n0 @F1@ FAM\n0 TRLR");
        assert!(eq(&a, &b));
        let a7 = a.replace("5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8", "7.0");
        let b7 = b.replace("5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8", "7.0");
        assert!(!eq(&a7, &b7));
    }

    /// The three name-pieces samples of ArmidaleSoftware/gedcom7 (MIT,
    /// `Tests/samples/name-pieces-*.ged` at dabc9a9), inlined.
    fn armidale_name_pieces(kind: &str) -> String {
        let pieces = ["NPFX", "GIVN", "NICK", "SPFX", "SURN", "NSFX"];
        let stems = ["npfx", "givn", "nick", "spfx", "surn", "nsfx"];
        let mut s = format!(
            "{H7}0 @I1@ INDI\n1 NAME npfx1 npfx2 givn1 givn2 nick1 nick2 /spfx1 spfx2 surn1 surn2/ nsfx1 nsfx2\n"
        );
        for (p, st) in pieces.iter().zip(stems) {
            match kind {
                "single" => s += &format!("2 {p} {st}1 {st}2\n"),
                "multiple" => s += &format!("2 {p} {st}1\n2 {p} {st}2\n"),
                _ => s += &format!("2 {p} {st}1\n2 {p} {st}1\n"),
            }
        }
        s + "0 TRLR\n"
    }

    #[test]
    fn rule8_name_piece_lists() {
        let opts = Options {
            name_piece_lists: true,
            ..Options::exact_head()
        };
        let single = armidale_name_pieces("single");
        let multiple = armidale_name_pieces("multiple");
        let mismatch = armidale_name_pieces("mismatch");
        assert!(compare(&single, &multiple, &opts).is_empty());
        assert!(!compare(&single, &mismatch, &opts).is_empty());
        assert!(!compare(&multiple, &mismatch, &opts).is_empty());
        assert!(!compare(&single, &multiple, &Options::exact_head()).is_empty());
    }

    #[test]
    fn rule9_age_spacing() {
        assert!(eq(
            &format!("{H5}0 @I1@ INDI\n1 DEAT\n2 AGE <1y\n0 TRLR"),
            &format!("{H5}0 @I1@ INDI\n1 DEAT\n2 AGE < 1y\n0 TRLR")
        ));
    }

    /// ArmidaleSoftware/gedcom7 `note.ged` and `snote.ged` (MIT), inlined.
    #[test]
    fn conversion_single_use_note_equals_inline_note() {
        let note = format!("{H7}0 @I1@ INDI\n1 NOTE A single-use note record\n0 TRLR\n");
        let snote = format!(
            "{H7}0 @I1@ INDI\n1 SNOTE @N1@\n0 @N1@ SNOTE A single-use note record\n0 TRLR\n"
        );
        assert!(compare(&note, &snote, &Options::conversion()).is_empty());
        assert!(!compare(&note, &snote, &Options::exact_head()).is_empty());
        let shared = format!(
            "{H7}0 @I1@ INDI\n1 SNOTE @N1@\n0 @I2@ INDI\n1 SNOTE @N1@\n0 @N1@ SNOTE A shared note\n0 TRLR\n"
        );
        let inlined = format!(
            "{H7}0 @I1@ INDI\n1 NOTE A shared note\n0 @I2@ INDI\n1 NOTE A shared note\n0 TRLR\n"
        );
        assert!(!compare(&shared, &inlined, &Options::conversion()).is_empty());
    }

    #[test]
    fn conversion_551_note_record_equals_snote() {
        let a = format!(
            "{H5}0 @I1@ INDI\n1 NOTE @N1@\n0 @I2@ INDI\n1 NOTE @N1@\n0 @N1@ NOTE Shared\n1 CONC  text\n0 TRLR"
        );
        let b = format!(
            "{H7}0 @I1@ INDI\n1 SNOTE @N1@\n0 @I2@ INDI\n1 SNOTE @N1@\n0 @N1@ SNOTE Shared text\n0 TRLR"
        );
        assert_eq!(
            compare(&a, &b, &Options::conversion()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn diff_lines_name_the_path() {
        let a = format!("{H7}0 @I1@ INDI\n1 BIRT\n2 DATE 1900\n0 TRLR");
        let b = format!("{H7}0 @I1@ INDI\n1 BIRT\n0 TRLR");
        let d = compare(&a, &b, &Options::default());
        assert!(
            d.iter()
                .any(|l| l.starts_with("- INDI#0/BIRT/DATE = \"1900\"")),
            "{d:?}"
        );
    }

    #[test]
    fn dates_ages_and_times_compare_by_meaning() {
        let date = |s: &str| normalise_value("DATE", s, Version::V551);
        assert_eq!(date("abt @#DJULIAN@ 1700"), date("ABT JULIAN 1700"));
        assert_eq!(date("@#DFRENCH R@ 1 vend 2"), date("FRENCH_R 1 VEND 2"));
        assert_eq!(date("ROMAN 28"), date("_ROMAN 28"));
        assert_eq!(date("20 B.C."), date("20  BCE"));
        assert_eq!(date("INT 1 jan 1900 (in jan)"), "INT 1 JAN 1900 (in jan)");
        assert_eq!(date("26 AUG 613/4"), date("26 AUG 613/14"));
        assert_eq!(date("Value DATE A"), "Value DATE A");
        assert_ne!(date("1 JAN 1900"), date("2 JAN 1900"));
        let age = |s: &str| normalise_value("AGE", s, Version::V551);
        assert_eq!(age("1w"), age("7d"));
        assert_eq!(age("< 0 Y"), age("<0y"));
        assert_eq!(age("11m99y"), age("99y 11m"));
        assert_eq!(age("majeur"), "majeur");
        assert_eq!(age("79"), age("79y"));
        assert_eq!(age("< 8"), age("<8y"));
        assert_ne!(age("1y"), age("1m"));
        let file = |s: &str| normalise_value("FILE", s, Version::V70);
        assert_eq!(file("https%3a/x%5b"), file("https%3A/x%5B"));
        assert_eq!(file("a%3a"), file("a:"));
        assert_eq!(
            file("C:\\photos\\a b.jpg"),
            file("file:///C:/photos/a%20b.jpg")
        );
        assert_eq!(file("/srv/a.jpg"), file("file:///srv/a.jpg"));
        assert_eq!(file("\\\\host\\a.jpg"), file("file://host/a.jpg"));
        assert_ne!(file("a.jpg"), file("b.jpg"));
        let time = |s: &str| normalise_value("TIME", s, Version::V70);
        assert_eq!(time("2:5"), time("02:05"));
        assert_eq!(time("2:50:00.00z"), time("02:50:00.00Z"));
        assert_eq!(time("noon"), "noon");
    }

    #[test]
    fn lost_finds_missing_payloads_and_structures() {
        let a = format!("{H7}0 @I1@ INDI\n1 NOTE a\u{7}b\n1 _X\n2 _Y kept\n0 TRLR");
        let b = format!("{H7}0 @I1@ INDI\n1 NOTE ab\n1 X\n0 TRLR");
        assert_eq!(lost(&a, &b), ["tag Y (x1)", "text kept (x1)"]);
    }

    #[test]
    fn lost_reads_the_repaired_forms() {
        // Relocated as an extension; an enumeration value of another case.
        let a = format!("{H5}0 @I1@ INDI\n1 SEX m\n1 FAMC @F1@\n2 PEDI stepchild\n0 TRLR");
        let b = format!("{H5}0 @I1@ INDI\n1 SEX M\n1 FAMC @F1@\n2 _PEDI stepchild\n0 TRLR");
        assert!(lost(&a, &b).is_empty(), "{:?}", lost(&a, &b));
        // A 5.5.1 date phrase, a 7.x date in its phrase, an invalid time
        // as an extension.
        let a = format!("{H5}0 @I1@ INDI\n1 BIRT\n2 DATE 7/11/1959\n0 TRLR");
        let b = format!("{H5}0 @I1@ INDI\n1 BIRT\n2 DATE (7/11/1959)\n0 TRLR");
        assert!(lost(&a, &b).is_empty());
        let a = format!("{H7}0 @I1@ INDI\n1 BIRT\n2 DATE 7/11/1959\n3 TIME 2:60\n0 TRLR");
        let b =
            format!("{H7}0 @I1@ INDI\n1 BIRT\n2 DATE\n3 PHRASE 7/11/1959\n3 _TIME 2:60\n0 TRLR");
        assert!(lost(&a, &b).is_empty(), "{:?}", lost(&a, &b));
        // A value of another case is not another value elsewhere.
        let a = format!("{H5}0 @I1@ INDI\n1 NOTE m\n0 TRLR");
        let b = format!("{H5}0 @I1@ INDI\n1 NOTE M\n0 TRLR");
        assert_eq!(lost(&a, &b), ["text m (x1)"]);
    }
}
