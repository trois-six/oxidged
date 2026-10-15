//! Finding the producer of a file: its header first, then the extension tags
//! it uses.
//!
//! The header is read from the bytes before anything is decoded (only the
//! `HEAD` record, whatever its encoding), so that the producer's reading of
//! `CHAR` can choose the decoding. Each profile scores its signatures
//! ([`Sig`]) against the header's fields; the best profile scoring at least
//! [`THRESHOLD`] is the producer. The tags of the whole file then add the
//! weight of the profiles' fingerprint tags (at most [`FINGERPRINT_CAP`]
//! each): files pass through several programs, so a profile known only by
//! its tags becomes a *hint*, whose unambiguous rules apply too.

use std::fmt;

use crate::encoding::{GedcomEncoding, decode, detect_encoding};
use crate::tree::{Structure, Tree};

use super::{GENERIC, Profile, long_form};

/// The score a profile needs to be taken as the producer.
pub const THRESHOLD: u32 = 100;
/// The most the fingerprint tags of one profile add to its score.
pub const FINGERPRINT_CAP: u32 = 60;
/// How much of the input the header is looked for in.
const HEAD_WINDOW: usize = 64 * 1024;

/// A field of the header a signature tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    /// `HEAD.SOUR`, the system identifier.
    Sour,
    /// `HEAD.SOUR.NAME`.
    Name,
    /// `HEAD.SOUR.VERS`.
    Vers,
    /// `HEAD.SOUR.CORP`.
    Corp,
    /// `HEAD.SOUR.CORP.ADDR` (and its `WWW`).
    CorpAddr,
    /// `HEAD.SOUR.DATA`.
    Data,
    /// `HEAD.DEST`, when it names a system (not `ANY`, `GEDCOM`, …).
    Dest,
    /// `HEAD.CHAR`.
    Char,
    /// `HEAD.CHAR.VERS`.
    CharVers,
    /// `HEAD.GEDC.VERS`.
    GedcVers,
    /// `HEAD.FILE`.
    File,
    /// An extension tag anywhere in the header (the test's value is the
    /// tag).
    HeadTag,
    /// The first line as written, but for its terminator.
    FirstLine,
    /// A tag anywhere in the file: the fingerprint (scored once the file is
    /// read, at most [`FINGERPRINT_CAP`] per profile).
    Tag,
}

/// How a signature compares a field (ignoring case, around and repeated
/// spaces).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Test {
    /// The whole value.
    Eq(&'static str),
    /// The start of the value.
    Prefix(&'static str),
    /// Part of the value.
    Contains(&'static str),
}

/// One piece of evidence a profile looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sig {
    pub(crate) field: Field,
    pub(crate) test: Test,
    pub(crate) weight: u16,
}

/// When a rule applies, by the producer's version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Whatever the version.
    Always,
    /// Versions below this major version, or a system identifier equal to
    /// the text (Family Tree Maker for Windows is `FTW`, whatever its
    /// version).
    Below(u32, &'static str),
}

impl Gate {
    pub(crate) fn open(self, detection: &Detection) -> bool {
        match self {
            Gate::Always => true,
            Gate::Below(major, sour) => {
                detection
                    .head
                    .sour
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(sour))
                    || detection.version.is_some_and(|v| v.0 < major)
            }
        }
    }
}

/// The header fields detection reads.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeadFields {
    /// `HEAD.SOUR`, unless a placeholder (`0`, `APPROVED_SOURCE_NAME`, …).
    pub sour: Option<String>,
    /// `HEAD.SOUR.NAME`.
    pub name: Option<String>,
    /// `HEAD.SOUR.VERS`.
    pub vers: Option<String>,
    /// `HEAD.SOUR.CORP`.
    pub corp: Option<String>,
    /// `HEAD.SOUR.CORP.ADDR` or `WWW`.
    pub corp_addr: Option<String>,
    /// `HEAD.SOUR.DATA`.
    pub data: Option<String>,
    /// `HEAD.DEST`, when it names a system.
    pub dest: Option<String>,
    /// `HEAD.CHAR`.
    pub charset: Option<String>,
    /// `HEAD.CHAR.VERS`.
    pub charset_version: Option<String>,
    /// `HEAD.GEDC.VERS`.
    pub gedcom_version: Option<String>,
    /// `HEAD.FILE`.
    pub file: Option<String>,
    /// The extension tags of the header, in order, once each.
    pub tags: Vec<String>,
    /// The first line, without its terminator (and without a byte order
    /// mark, which [`has_bom`](Self::has_bom) tells).
    pub first_line: String,
    /// Whether the input starts with a byte order mark.
    pub has_bom: bool,
}

impl HeadFields {
    /// Reads the header of GEDCOM bytes: the `HEAD` record only, decoded as
    /// UTF-16 when the bytes are, otherwise byte for byte.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let window = bytes.get(..HEAD_WINDOW).unwrap_or(bytes);
        let utf16 = matches!(
            detect_encoding(window),
            GedcomEncoding::Utf16Le | GedcomEncoding::Utf16Be
        );
        let text = if utf16 {
            decode(window).text
        } else {
            let body = window.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(window);
            String::from_utf8_lossy(body).into_owned()
        };
        let mut fields = Self::from_text(&text);
        fields.has_bom = window.starts_with(b"\xEF\xBB\xBF")
            || window.starts_with(b"\xFF\xFE")
            || window.starts_with(b"\xFE\xFF");
        fields
    }

    /// Reads the header of decoded text.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        let first_line = text
            .split(['\r', '\n'])
            .next()
            .unwrap_or_default()
            .to_string();
        let head = head_region(text);
        let tree = Tree::parse(head);
        let mut fields = HeadFields {
            first_line,
            has_bom: false,
            ..HeadFields::default()
        };
        let Some(head) = tree
            .records()
            .find(|r| matches!(long_form(r.tag()).unwrap_or(r.tag()), "HEAD"))
        else {
            return fields;
        };
        let head = head.to_structure();
        let get = |s: &Structure, tag: &str| -> Option<Structure> {
            s.substructures
                .iter()
                .find(|c| long_form(c.tag.as_str()).unwrap_or(c.tag.as_str()) == tag)
                .cloned()
        };
        let value = |s: Option<&Structure>| {
            s.and_then(|s| s.payload.as_str().map(normalize))
                .filter(|v| !v.is_empty())
        };
        let sour = get(&head, "SOUR");
        fields.sour = value(sour.as_ref()).filter(|s| !is_placeholder(s));
        if let Some(sour) = &sour {
            fields.name = value(get(sour, "NAME").as_ref());
            fields.vers = value(get(sour, "VERS").as_ref());
            fields.data = value(get(sour, "DATA").as_ref());
            let corp = get(sour, "CORP");
            fields.corp = value(corp.as_ref());
            if let Some(corp) = &corp {
                fields.corp_addr =
                    value(get(corp, "ADDR").as_ref()).or_else(|| value(get(corp, "WWW").as_ref()));
            }
        }
        fields.dest = value(get(&head, "DEST").as_ref()).filter(|d| !is_vague_destination(d));
        let charset = get(&head, "CHAR");
        fields.charset = value(charset.as_ref());
        fields.charset_version = value(charset.as_ref().and_then(|c| get(c, "VERS")).as_ref());
        fields.gedcom_version = value(get(&head, "GEDC").and_then(|g| get(&g, "VERS")).as_ref());
        fields.file = value(get(&head, "FILE").as_ref());
        collect_tags(&head, &mut fields.tags);
        fields
    }

    fn field(&self, field: Field) -> Option<&str> {
        match field {
            Field::Sour => self.sour.as_deref(),
            Field::Name => self.name.as_deref(),
            Field::Vers => self.vers.as_deref(),
            Field::Corp => self.corp.as_deref(),
            Field::CorpAddr => self.corp_addr.as_deref(),
            Field::Data => self.data.as_deref(),
            Field::Dest => self.dest.as_deref(),
            Field::Char => self.charset.as_deref(),
            Field::CharVers => self.charset_version.as_deref(),
            Field::GedcVers => self.gedcom_version.as_deref(),
            Field::File => self.file.as_deref(),
            Field::FirstLine => Some(self.first_line.as_str()),
            Field::HeadTag | Field::Tag => None,
        }
    }
}

/// The text of the first record: lines up to the next level-0 line.
fn head_region(text: &str) -> &str {
    let mut start = 0;
    let mut first = true;
    for line in text.split_inclusive(['\n', '\r']) {
        let trimmed = line.trim_start_matches([' ', '\t', '\u{FEFF}']);
        let level_zero = trimmed.starts_with('0')
            && trimmed
                .get(1..2)
                .is_some_and(|c| c == " " || c == "\t" || c == "@");
        if level_zero && !first {
            return text.get(..start).unwrap_or(text);
        }
        if !trimmed.trim().is_empty() {
            first = false;
        }
        start += line.len();
    }
    text
}

fn collect_tags(s: &Structure, out: &mut Vec<String>) {
    for c in &s.substructures {
        let tag = c.tag.as_str();
        if tag.starts_with('_') && !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
        collect_tags(c, out);
    }
}

/// Trims and collapses runs of spaces.
fn normalize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// System identifiers that identify nothing (REPORT §3.2).
fn is_placeholder(sour: &str) -> bool {
    [
        "0",
        "APPROVED_SOURCE_NAME",
        "REGISTERED_SOURCE_NAME",
        "UNSPECIFIED",
        "NOTEPAD++",
    ]
    .iter()
    .any(|p| p.eq_ignore_ascii_case(sour))
}

/// Destinations that select no dialect (TJ-058).
fn is_vague_destination(dest: &str) -> bool {
    [
        "ANY", "GEDCOM", "GED55", "GED551", "OTHER", "USER", "DISKETTE",
    ]
    .iter()
    .any(|p| p.eq_ignore_ascii_case(dest))
}

fn passes(test: Test, value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    match test {
        Test::Eq(t) => value == t.to_ascii_lowercase(),
        Test::Prefix(t) => value.starts_with(&t.to_ascii_lowercase()),
        Test::Contains(t) => value.contains(&t.to_ascii_lowercase()),
    }
}

fn test_text(test: Test) -> &'static str {
    match test {
        Test::Eq(t) | Test::Prefix(t) | Test::Contains(t) => t,
    }
}

/// The tags the profiles' fingerprints look for, as they spell them: the
/// only tags of a file [`Detection::refine`] reads.
pub(crate) fn fingerprint_tags() -> &'static [&'static str] {
    static TAGS: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    TAGS.get_or_init(|| {
        let mut tags: Vec<&'static str> = super::import_profiles()
            .iter()
            .flat_map(|p| p.detect.iter())
            .filter(|s| s.field == Field::Tag)
            .map(|s| test_text(s.test))
            .collect();
        tags.sort_unstable();
        tags.dedup();
        tags
    })
}

/// The fingerprint tag `tag` is (ASCII case ignored), as the fingerprint
/// spells it.
pub(crate) fn fingerprint(tag: &str) -> Option<&'static str> {
    if !tag.starts_with('_') {
        return None;
    }
    fingerprint_tags()
        .iter()
        .copied()
        .find(|t| t.eq_ignore_ascii_case(tag))
}

/// The fingerprint tags the lines of `text` have as tags, once each, as
/// far as a quick look tells: the lines with an `_` where the lexer finds
/// a tag (what [`Detection::refine`] reads, without lexing the file).
pub(crate) fn fingerprints_in(text: &str) -> Vec<&'static str> {
    let bytes = text.as_bytes();
    let eol = |b: &u8| matches!(b, b'\n' | b'\r');
    let mut found: Vec<&'static str> = Vec::new();
    // The last tag looked at, and what it is: files repeat their tags.
    let mut last: Option<(&str, Option<&'static str>)> = None;
    let mut from = 0;
    while let Some(at) = text
        .get(from..)
        .and_then(|rest| rest.find('_'))
        .map(|p| from + p)
    {
        let start = bytes
            .get(..at)
            .and_then(|before| before.iter().rposition(eol))
            .map_or(0, |p| p + 1);
        let end = bytes
            .get(at..)
            .and_then(|after| after.iter().position(eol))
            .map_or(bytes.len(), |p| at + p);
        if let Some(line) = text.get(start..end)
            && let crate::tree::Line::Structure { tag, .. } = crate::tree::lex_line(line)
            && tag.0 == at - start
        {
            let tag = line.get(tag.0..tag.1).unwrap_or_default();
            let print = match last {
                Some((t, print)) if t == tag => print,
                _ => fingerprint(tag),
            };
            last = Some((tag, print));
            if let Some(print) = print.filter(|p| !found.contains(p)) {
                found.push(print);
            }
        }
        from = end.max(at + 1);
    }
    found
}

/// A producer version: up to three numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProducerVersion(pub u32, pub u32, pub u32);

impl fmt::Display for ProducerVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

impl ProducerVersion {
    /// The version a header gives: in parentheses in `VERS` (Family Tree
    /// Maker writes `Family Tree Maker (22.2.5)`), else the first number in
    /// `VERS`, else the first number in the system identifier
    /// (`HEREDIS 14 PC`, `PAF 2.1`).
    fn of(head: &HeadFields) -> Option<Self> {
        let vers = head.vers.as_deref().unwrap_or_default();
        let in_parens = vers
            .find('(')
            .and_then(|i| vers.get(i + 1..))
            .and_then(Self::parse);
        in_parens
            .or_else(|| Self::parse(vers))
            .or_else(|| head.sour.as_deref().and_then(Self::parse))
    }

    /// The first run of digits and dots in `text`.
    fn parse(text: &str) -> Option<Self> {
        let start = text.find(|c: char| c.is_ascii_digit())?;
        let rest = text.get(start..)?;
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let mut parts = rest.get(..end)?.split('.').map(|p| p.parse::<u32>().ok());
        let major = parts.next()??;
        let minor = parts.next().flatten().unwrap_or(0);
        let patch = parts.next().flatten().unwrap_or(0);
        Some(Self(major, minor, patch))
    }
}

/// What detection found: the producer's profile, how sure, and the
/// evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    pub(crate) profile: &'static Profile,
    pub(crate) score: u32,
    pub(crate) version: Option<ProducerVersion>,
    pub(crate) hints: Vec<&'static Profile>,
    pub(crate) head: HeadFields,
    pub(crate) evidence: Vec<String>,
    /// Whether a profile was chosen by the caller, not detected.
    pub(crate) chosen: bool,
}

impl Detection {
    /// Detects the producer of GEDCOM bytes from their header alone (see
    /// the [`profile` documentation](crate::profile)).
    #[must_use]
    pub fn from_head(bytes: &[u8]) -> Self {
        Self::of_head(HeadFields::from_bytes(bytes))
    }

    /// Detects the producer from header fields.
    #[must_use]
    pub fn of_head(head: HeadFields) -> Self {
        let mut best: Option<(&'static Profile, u32, Vec<String>)> = None;
        for &profile in super::import_profiles() {
            let (score, evidence) = score_head(profile, &head);
            if score >= THRESHOLD && best.as_ref().is_none_or(|(_, s, _)| score > *s) {
                best = Some((profile, score, evidence));
            }
        }
        let version = ProducerVersion::of(&head);
        let (profile, score, evidence) = best.unwrap_or((&GENERIC, 0, Vec::new()));
        Self {
            profile,
            score,
            version,
            hints: Vec::new(),
            head,
            evidence,
            chosen: false,
        }
    }

    /// A detection that names `profile` whatever the file says (its header
    /// is still read for the producer's version).
    #[must_use]
    pub fn chosen(profile: &'static Profile, bytes: &[u8]) -> Self {
        let head = HeadFields::from_bytes(bytes);
        Self {
            profile,
            score: 0,
            version: ProducerVersion::of(&head),
            hints: Vec::new(),
            head,
            evidence: vec![format!("chosen: {}", profile.id)],
            chosen: true,
        }
    }

    /// Adds the evidence of the tags a file uses (each once): the
    /// fingerprint. A profile they make score at least [`THRESHOLD`] above
    /// the header's choice becomes the producer (unless the producer was
    /// chosen); others they point to become hints.
    pub fn refine<'t>(&mut self, tags: impl IntoIterator<Item = &'t str>) {
        let tags: Vec<&str> = tags.into_iter().collect();
        let mut hints = Vec::new();
        let mut best: Option<(&'static Profile, u32, Vec<String>)> = None;
        for &profile in super::import_profiles() {
            let (head, mut evidence) = score_head(profile, &self.head);
            let mut print = 0_u32;
            for sig in profile.detect.iter().filter(|s| s.field == Field::Tag) {
                let wanted = test_text(sig.test);
                if tags.iter().any(|t| t.eq_ignore_ascii_case(wanted)) {
                    print += u32::from(sig.weight);
                    evidence.push(format!("tag {wanted}"));
                }
            }
            let score = head + print.min(FINGERPRINT_CAP);
            // A profile the tags point to, or (when the producer was
            // chosen) one the header names: the file holds its tags.
            let named = print > 0 && head < THRESHOLD || self.chosen && head >= THRESHOLD;
            if named && !std::ptr::eq(profile, self.profile) {
                hints.push(profile);
            }
            if score >= THRESHOLD && best.as_ref().is_none_or(|(_, s, _)| score > *s) {
                best = Some((profile, score, evidence));
            }
        }
        if let Some((profile, score, evidence)) = best
            && !self.chosen
            && (score > self.score || std::ptr::eq(profile, self.profile))
        {
            self.profile = profile;
            self.score = score;
            self.evidence = evidence;
        }
        hints.retain(|h| !self.chain().any(|p| std::ptr::eq(p, *h)));
        // What this crate wrote holds no other producer's extensions to
        // interpret again.
        if self.profile.id == "oxidged" {
            hints.clear();
        }
        self.hints = hints;
    }

    /// The producer's profile ([`super::profile`] `"generic"` when none
    /// scored enough).
    #[must_use]
    pub fn profile(&self) -> &'static Profile {
        self.profile
    }

    /// The score of the producer's profile.
    #[must_use]
    pub fn score(&self) -> u32 {
        self.score
    }

    /// Whether a producer was recognised (or chosen).
    #[must_use]
    pub fn is_known(&self) -> bool {
        self.chosen || self.score >= THRESHOLD
    }

    /// The producer's version, as the header gives it.
    #[must_use]
    pub fn producer_version(&self) -> Option<ProducerVersion> {
        self.version
    }

    /// Profiles the fingerprint points to besides the producer's: only
    /// their unambiguous rules apply.
    #[must_use]
    pub fn hints(&self) -> &[&'static Profile] {
        &self.hints
    }

    /// The header fields read.
    #[must_use]
    pub fn head(&self) -> &HeadFields {
        &self.head
    }

    /// What matched, in words.
    #[must_use]
    pub fn evidence(&self) -> &[String] {
        &self.evidence
    }

    /// The producer's profile and its parents, nearest first.
    pub fn chain(&self) -> impl Iterator<Item = &'static Profile> + use<> {
        let mut next = Some(self.profile);
        std::iter::from_fn(move || {
            let p = next?;
            next = p.parent.and_then(super::profile);
            Some(p)
        })
    }
}

impl fmt::Display for Detection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.profile.id, self.profile.label)?;
        if let Some(v) = self.version {
            write!(f, " {v}")?;
        }
        if self.chosen {
            f.write_str(", chosen")?;
        } else {
            write!(f, ", score {}", self.score)?;
        }
        if !self.hints.is_empty() {
            let hints: Vec<&str> = self.hints.iter().map(|h| h.id).collect();
            write!(f, ", hints: {}", hints.join(", "))?;
        }
        Ok(())
    }
}

/// The header score of `profile`, and what matched.
fn score_head(profile: &Profile, head: &HeadFields) -> (u32, Vec<String>) {
    let mut score = 0_u32;
    let mut evidence = Vec::new();
    // One signature per field counts: the heaviest that matches.
    let mut seen: Vec<(Field, u16)> = Vec::new();
    for sig in profile.detect {
        let hit = match sig.field {
            Field::Tag => false,
            Field::HeadTag => head
                .tags
                .iter()
                .any(|t| t.eq_ignore_ascii_case(test_text(sig.test))),
            field => head.field(field).is_some_and(|v| passes(sig.test, v)),
        };
        if !hit {
            continue;
        }
        let repeatable = sig.field == Field::HeadTag;
        if !repeatable {
            if let Some(at) = seen.iter().position(|(f, _)| *f == sig.field) {
                let (_, w) = seen.get(at).copied().unwrap_or((sig.field, 0));
                if w >= sig.weight {
                    continue;
                }
                score -= u32::from(w);
                seen.remove(at);
            }
            seen.push((sig.field, sig.weight));
        }
        score += u32::from(sig.weight);
        evidence.push(format!("{:?} {:?}", sig.field, test_text(sig.test)));
    }
    (score, evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_fields() {
        let head = HeadFields::from_bytes(
            b"\xEF\xBB\xBF0 HEAD\r\n1 SOUR FTM\r\n2 NAME Family Tree Maker for Windows\r\n\
              2 VERS Family Tree Maker (22.2.5)\r\n2 CORP Ancestry.com\r\n1 DEST GED55\r\n\
              1 CHAR UTF-8\r\n1 _TREE x\r\n0 @I1@ INDI\r\n0 TRLR\r\n",
        );
        assert_eq!(head.sour.as_deref(), Some("FTM"));
        assert_eq!(head.corp.as_deref(), Some("Ancestry.com"));
        assert_eq!(head.dest, None);
        assert_eq!(head.tags, ["_TREE"]);
        assert!(head.has_bom);
        assert_eq!(ProducerVersion::of(&head), Some(ProducerVersion(22, 2, 5)));
        let utf16: Vec<u8> = "0 HEAD\n1 SOUR GeneWeb\n0 TRLR\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            HeadFields::from_bytes(&utf16).sour.as_deref(),
            Some("GeneWeb")
        );
        let long = HeadFields::from_text("0 HEADER\n1 SOURCE FTW\n2 VERSION 4.0\n0 TRLR\n");
        assert_eq!(long.sour.as_deref(), Some("FTW"));
        assert_eq!(long.first_line, "0 HEADER");
        assert_eq!(
            HeadFields::from_text("0 HEAD\n1 SOUR APPROVED_SOURCE_NAME\n").sour,
            None
        );
    }
}
