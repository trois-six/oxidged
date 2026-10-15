//! A small GEDCOM line-tree reader, deliberately independent of oxidged.
//!
//! The suite compares what oxidged reads and writes against this reader, so it
//! must not share code with the crate under test. It reads any line terminator,
//! folds `CONT`/`CONC` into the payload they continue, decodes `@@` for the
//! file's version and types a payload as a pointer only when the raw payload
//! has the pointer shape.

/// The GEDCOM version family of a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Version {
    V551,
    V70,
}

/// A line payload, typed from the raw line.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Payload {
    None,
    Pointer(String),
    Text(String),
}

impl Payload {
    /// The payload as it would read on a line after decoding: pointers keep
    /// their `@` delimiters, text is the decoded text.
    pub fn as_str(&self) -> &str {
        match self {
            Payload::None => "",
            Payload::Pointer(p) | Payload::Text(p) => p,
        }
    }
}

/// One structure and its substructures.
#[derive(Clone, Debug)]
pub struct Node {
    pub tag: String,
    pub xref: Option<String>,
    pub payload: Payload,
    pub children: Vec<Node>,
    /// 1-based source line.
    pub line: usize,
}

impl Node {
    pub fn child(&self, tag: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.tag == tag)
    }

    pub fn children_tagged<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.tag == tag)
    }

    /// Calls `f` on this node and every descendant, depth first, with the tag
    /// path from the record (`INDI/BIRT/DATE`).
    pub fn walk<'a>(&'a self, path: &mut Vec<&'a str>, f: &mut dyn FnMut(&[&'a str], &'a Node)) {
        path.push(&self.tag);
        f(path, self);
        for c in &self.children {
            c.walk(path, f);
        }
        path.pop();
    }
}

/// A raw line, split on the 5.5.1/7.0 line grammar with lenient delimiters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawLine<'a> {
    pub level: u32,
    pub xref: Option<&'a str>,
    pub tag: &'a str,
    /// Everything after the single delimiter that follows the tag.
    pub payload: Option<&'a str>,
}

/// Splits on CR, LF, CRLF (and LFCR, which yields an empty line that callers skip).
pub fn split_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                out.push(&text[start..i]);
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            b'\n' => {
                out.push(&text[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        out.push(&text[start..]);
    }
    out
}

/// Parses one line. Leading whitespace and runs of spaces between the level,
/// the xref and the tag are accepted; exactly one space separates the tag from
/// the payload, so the payload keeps its own leading spaces.
pub fn parse_line(line: &str) -> Option<RawLine<'_>> {
    let s = line.trim_start_matches([' ', '\t', '\u{feff}']);
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let level: u32 = s[..digits].parse().ok()?;
    let mut rest = &s[digits..];
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    rest = rest.trim_start_matches([' ', '\t']);
    let mut xref = None;
    if rest.starts_with('@') {
        // A 5.5.1 xref may contain spaces (pointer_char is any non-`@`,
        // p.13): it ends at its closing `@`, else at the first space.
        let end = match rest[1..].find('@') {
            Some(at) if rest[at + 2..].starts_with([' ', '\t']) => at + 2,
            _ => rest.find([' ', '\t'])?,
        };
        xref = Some(&rest[..end]);
        rest = rest[end..].trim_start_matches([' ', '\t']);
    }
    let tag_end = rest.find([' ', '\t']).unwrap_or(rest.len());
    let tag = &rest[..tag_end];
    if tag.is_empty() {
        return None;
    }
    let payload = if tag_end < rest.len() {
        Some(&rest[tag_end + 1..])
    } else {
        None
    };
    Some(RawLine {
        level,
        xref,
        tag,
        payload,
    })
}

/// True when the raw payload is a pointer in the grammar of `version`.
///
/// * 5.5.1 (p.13): `@`, an alphanumeric, any run of non-`@` characters
///   (spaces included, as in `@NoTe ref@`), `@`.
/// * 7.0: `@`, one or more of `A-Z`, `0-9` and `_`, `@`; a payload such as
///   `@I 1@` is text (an invalid one, kept as read).
pub fn is_pointer(raw: &str, version: Version) -> bool {
    let Some(inner) = raw.strip_prefix('@').and_then(|r| r.strip_suffix('@')) else {
        return false;
    };
    let Some(first) = inner.chars().next() else {
        return false;
    };
    match version {
        Version::V551 => first.is_ascii_alphanumeric() && !inner.contains(['@', '\t']),
        Version::V70 => inner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
    }
}

/// Decodes the `@@` escapes of one line payload.
pub fn unescape(raw: &str, version: Version) -> String {
    match version {
        Version::V70 => match raw.strip_prefix("@@") {
            Some(rest) => format!("@{rest}"),
            None => raw.to_string(),
        },
        Version::V551 => raw.replace("@@", "@"),
    }
}

/// Bytes to text, by evidence only: a BOM, a UTF-16 NUL pattern, valid UTF-8,
/// or Latin-1 as the last resort. Good enough for the suite's own fixtures and
/// for oxidged output, which is always UTF-8.
pub fn decode_bytes(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| {
                if le {
                    u16::from_le_bytes(c)
                } else {
                    u16::from_be_bytes(c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    if bytes.len() >= 2 && bytes[0] == b'0' && bytes[1] == 0 {
        return utf16(bytes, true);
    }
    if bytes.len() >= 2 && bytes[0] == 0 && bytes[1] == b'0' {
        return utf16(bytes, false);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

/// The version declared by `HEAD.GEDC.VERS` (7.x is 7.0; anything else 5.5.1).
pub fn detect_version(text: &str) -> Version {
    if declared_vers(text).is_some_and(|v| v.starts_with('7')) {
        Version::V70
    } else {
        Version::V551
    }
}

/// The raw `HEAD.GEDC.VERS` payload, if any.
pub fn declared_vers(text: &str) -> Option<String> {
    let mut in_head = false;
    let mut in_gedc = false;
    for line in split_lines(text) {
        let Some(l) = parse_line(line) else { continue };
        match l.level {
            0 => {
                if in_head {
                    break;
                }
                in_head = l.tag == "HEAD";
            }
            1 => in_gedc = in_head && l.tag == "GEDC",
            2 if in_gedc && l.tag == "VERS" => {
                return Some(l.payload.unwrap_or("").trim().to_string());
            }
            _ => {}
        }
    }
    None
}

/// Reads a whole stream into records, with its declared version.
pub fn parse(text: &str) -> (Version, Vec<Node>) {
    let v = detect_version(text);
    (v, parse_with(text, v))
}

/// Reads a whole stream into records.
///
/// Recovery is silent: a line without a level is appended to the previous
/// text payload; `CONT`/`CONC` anywhere continue the payload of their parent
/// (or of the nearest open structure); a level jump attaches to the deepest
/// open structure of the current record.
pub fn parse_with(text: &str, version: Version) -> Vec<Node> {
    let mut records: Vec<Node> = Vec::new();
    // Path of child indexes from the current record to the deepest open node,
    // with each node's level.
    let mut stack: Vec<(u32, usize)> = Vec::new();
    for (i, raw) in split_lines(text).into_iter().enumerate() {
        let lineno = i + 1;
        // Blank lines, and a level with nothing after it, carry no data.
        if raw.trim().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Some(l) = parse_line(raw) else {
            if let Some(n) = deepest(&mut records, &stack) {
                append_text(n, &format!("\n{raw}"));
            }
            continue;
        };
        if (l.tag == "CONT" || l.tag == "CONC") && !records.is_empty() && l.level > 0 {
            let text = l.payload.map(|p| unescape(p, version)).unwrap_or_default();
            // A continuation belongs to the open structure one level up
            // (else to the deepest one). When that structure carries no
            // payload but the previous sibling carries text, the line was
            // written one level too high (`1 NOTE a` / `1 CONT b`): the
            // spec gives the text no other home, so it continues the sibling.
            let depth = stack
                .iter()
                .rposition(|&(lv, _)| lv + 1 == l.level)
                .map_or(stack.len(), |d| d + 1);
            let sibling = stack.get(depth).filter(|&&(lv, _)| lv == l.level).is_some()
                && node_ref(&records, &stack[..depth]).is_some_and(|n| n.payload == Payload::None)
                && node_ref(&records, &stack[..=depth])
                    .is_some_and(|n| matches!(n.payload, Payload::Text(_)));
            let target = if sibling {
                &stack[..=depth]
            } else {
                &stack[..depth]
            };
            let sep = if l.tag == "CONT" { "\n" } else { "" };
            if let Some(n) = node_at(&mut records, target) {
                append_text(n, &format!("{sep}{text}"));
            }
            continue;
        }
        let payload = match l.payload {
            None => Payload::None,
            Some(p) if is_pointer(p, version) => Payload::Pointer(p.to_string()),
            Some(p) => Payload::Text(unescape(p, version)),
        };
        let node = Node {
            tag: l.tag.to_string(),
            xref: l.xref.map(str::to_string),
            payload,
            children: Vec::new(),
            line: lineno,
        };
        if l.level == 0 || records.is_empty() {
            records.push(node);
            stack.clear();
            stack.push((0, records.len() - 1));
            continue;
        }
        while stack.len() > 1 && stack.last().is_some_and(|&(lv, _)| lv >= l.level) {
            stack.pop();
        }
        let parent = node_at(&mut records, &stack).expect("stack points to a node");
        parent.children.push(node);
        let idx = parent.children.len() - 1;
        stack.push((l.level, idx));
    }
    records
}

fn node_at<'a>(records: &'a mut [Node], stack: &[(u32, usize)]) -> Option<&'a mut Node> {
    let (&(_, first), rest) = stack.split_first()?;
    let mut n = records.get_mut(first)?;
    for &(_, i) in rest {
        n = n.children.get_mut(i)?;
    }
    Some(n)
}

fn node_ref<'a>(records: &'a [Node], stack: &[(u32, usize)]) -> Option<&'a Node> {
    let (&(_, first), rest) = stack.split_first()?;
    let mut n = records.get(first)?;
    for &(_, i) in rest {
        n = n.children.get(i)?;
    }
    Some(n)
}

fn deepest<'a>(records: &'a mut [Node], stack: &[(u32, usize)]) -> Option<&'a mut Node> {
    node_at(records, stack)
}

fn append_text(n: &mut Node, s: &str) {
    n.payload = match std::mem::replace(&mut n.payload, Payload::None) {
        Payload::None => Payload::Text(s.trim_start_matches('\n').to_string()),
        Payload::Text(t) => Payload::Text(t + s),
        // Text continuing a pointer: keep both, the pointer first.
        Payload::Pointer(p) => Payload::Text(p + s),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_terminator_splits() {
        assert_eq!(split_lines("a\rb\r\nc\nd"), ["a", "b", "c", "d"]);
        assert_eq!(split_lines("a\n\rb"), ["a", "", "b"]);
    }

    #[test]
    fn payload_keeps_its_leading_spaces() {
        let l = parse_line("1 NOTE  two").unwrap();
        assert_eq!(l.payload, Some(" two"));
        let l = parse_line("  01   @I1@  INDI").unwrap();
        assert_eq!(
            (l.level, l.xref, l.tag, l.payload),
            (1, Some("@I1@"), "INDI", None)
        );
    }

    #[test]
    fn continuation_and_escapes_fold() {
        let t = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @N1@ NOTE a@@b\n1 CONC c\n1 CONT d\n0 TRLR\n";
        let (v, recs) = parse(t);
        assert_eq!(v, Version::V551);
        assert_eq!(recs[1].payload, Payload::Text("a@bc\nd".into()));
        let t = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @N1@ SNOTE @@a@@b\n0 TRLR";
        let (v, recs) = parse(t);
        assert_eq!(v, Version::V70);
        assert_eq!(recs[1].payload, Payload::Text("@a@@b".into()));
    }

    #[test]
    fn pointer_shape_is_strict() {
        for v in [Version::V551, Version::V70] {
            assert!(is_pointer("@I1@", v));
            assert!(!is_pointer("@@I1@", v));
            assert!(!is_pointer("@#DJULIAN@", v));
            assert!(!is_pointer("@I1@ x", v));
            assert!(!is_pointer("@@", v));
        }
        assert!(is_pointer("@NoTe ref@", Version::V551));
        assert!(!is_pointer("@NoTe ref@", Version::V70));
        assert!(is_pointer("@I_1@", Version::V70));
    }

    #[test]
    fn xrefs_with_spaces_are_read_whole() {
        let recs = parse_with(
            "0 @NoTe ref@ NOTE text\n0 @I1@ INDI\n1 NOTE @NoTe ref@",
            Version::V551,
        );
        assert_eq!(recs[0].xref.as_deref(), Some("@NoTe ref@"));
        assert_eq!(recs[0].payload, Payload::Text("text".into()));
        assert_eq!(
            recs[1].children[0].payload,
            Payload::Pointer("@NoTe ref@".into())
        );
    }

    #[test]
    fn a_continuation_one_level_too_high_continues_the_sibling() {
        let recs = parse_with(
            "0 @I1@ INDI\n1 NOTE a\n1 CONT b\n1 CONC c\n1 SEX F",
            Version::V70,
        );
        assert_eq!(recs[0].payload, Payload::None);
        assert_eq!(recs[0].children[0].payload, Payload::Text("a\nbc".into()));
        assert_eq!(recs[0].children.len(), 2);
        // A record with text keeps its continuation, as the grammar says.
        let recs = parse_with("0 @N1@ NOTE a\n1 SOUR @S1@\n1 CONT b", Version::V551);
        assert_eq!(recs[0].payload, Payload::Text("a\nb".into()));
        // Nothing to continue but the record: the text is still kept there.
        let recs = parse_with("0 @I1@ INDI\n1 CONT orphan", Version::V70);
        assert_eq!(recs[0].payload, Payload::Text("orphan".into()));
    }

    #[test]
    fn level_jump_stays_in_its_record() {
        let t = "0 @I1@ INDI\n1 BIRT\n0 @I2@ INDI\n2 _FOO bar\n1 SEX F";
        let recs = parse_with(t, Version::V70);
        assert_eq!(recs[0].children.len(), 1);
        assert_eq!(recs[1].children.len(), 2);
    }
}
