//! The line grammar, checked on the text itself: what the tree no longer
//! shows once read (spacing, level numbers, terminators, escapes, line
//! length, where continuations sit, record sizes) and the encoding of the
//! bytes.

use super::payload::Family;
use super::validate::MAX_RECORD_BYTES;
use super::{Deviation, DeviationKind};
use crate::encoding::{Decoded, GedcomEncoding};
use crate::tree::{find_eol, terminator_len};

/// The longest 5.5.1 line, terminator included (p. 11).
const MAX_LINE: usize = 255;

/// An open line, for the continuation rules.
struct Open<'t> {
    level: u32,
    tag: &'t str,
    /// The raw payload of the line, or of its last continuation.
    payload: &'t str,
    pointer: bool,
    /// Whether a substructure other than a continuation came already.
    has_subs: bool,
}

/// The parts of a structure line on the exact grammar (the identifier is
/// checked on the tree).
struct Parts<'t> {
    level: u32,
    tag: &'t str,
    /// The raw payload: `Some("")` when a delimiter follows the tag but
    /// nothing else.
    payload: Option<&'t str>,
}

fn dev(line: u32, kind: DeviationKind, detail: impl Into<Box<str>>) -> Deviation {
    Deviation {
        line,
        kind,
        detail: detail.into(),
    }
}

/// Splits `raw` (without terminator and leading whitespace) on the grammar
/// `Level D [Xref D] Tag [D LineVal]`, or says what breaks it.
fn split(raw: &str, family: Family) -> Result<Parts<'_>, (DeviationKind, String)> {
    let digits = raw.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return Err((
            DeviationKind::LineSyntax,
            "a line starts with its level".into(),
        ));
    }
    let (number, rest) = raw.split_at(digits);
    let level = number.parse::<u32>().unwrap_or(u32::MAX);
    if digits > 1 && number.starts_with('0') {
        return Err((
            DeviationKind::Level,
            format!("level {number:?} has a leading zero"),
        ));
    }
    if family == Family::V551 && digits > 2 {
        return Err((
            DeviationKind::Level,
            format!("level {number} has more than two digits"),
        ));
    }
    let delim = |rest: &'_ str, what: &str| -> Result<(), (DeviationKind, String)> {
        match rest.as_bytes() {
            [b' ', b' ' | b'\t', ..] | [b'\t', ..] => Err((
                DeviationKind::LineSyntax,
                format!("one space, and nothing else, before the {what}"),
            )),
            [b' ', _, ..] => Ok(()),
            _ => Err((DeviationKind::LineSyntax, format!("no {what}"))),
        }
    };
    delim(rest, "tag")?;
    let mut rest = rest.get(1..).unwrap_or("");
    if rest.starts_with('@') {
        // To the `@` that a delimiter follows (5.5.1 identifiers may hold
        // spaces), else to the first space.
        let close = rest
            .char_indices()
            .skip(1)
            .find(|&(i, c)| c == '@' && rest.get(i + 1..).is_none_or(|r| r.starts_with(' ')))
            .map(|(i, _)| i + 1);
        let end = close.unwrap_or_else(|| rest.find(' ').unwrap_or(rest.len()));
        let r = rest.get(end..).unwrap_or("");
        delim(r, "tag")?;
        rest = r.get(1..).unwrap_or("");
    }
    let end = rest.find([' ', '\t']).unwrap_or(rest.len());
    let (tag, after) = rest.split_at(end);
    let payload = match after.as_bytes() {
        [] => None,
        [b'\t', ..] => {
            return Err((
                DeviationKind::LineSyntax,
                "one space, and nothing else, after the tag".into(),
            ));
        }
        _ => Some(after.get(1..).unwrap_or("")),
    };
    Ok(Parts {
        level,
        tag,
        payload,
    })
}

/// Whether a raw payload has the pointer shape (the lexer's rule, so that
/// the text and the tree agree on what is a pointer).
fn is_pointer(raw: &str) -> bool {
    crate::tree::pointer(raw).is_some()
}

/// Why a raw text payload is not escaped as the version wants: 7.x doubles
/// a leading `@` (§1.3); 5.5.1 doubles every `@` that does not open an
/// escape `@#…@` (p. 12).
fn escape_error(raw: &str, family: Family) -> Option<&'static str> {
    match family {
        Family::V7 => {
            (raw.starts_with('@') && !raw.starts_with("@@")).then_some("a leading @ is written @@")
        }
        Family::V551 => {
            let b = raw.as_bytes();
            let mut i = 0;
            while let Some(&c) = b.get(i) {
                if c == b'@' {
                    match b.get(i + 1) {
                        Some(b'@') => i += 2,
                        Some(b'#') => match b
                            .get(i + 2..)
                            .and_then(|r| r.iter().position(|&x| x == b'@'))
                        {
                            Some(p) => i += p + 3,
                            None => return Some("an escape ends with @"),
                        },
                        _ => return Some("an @ in text is written @@"),
                    }
                } else {
                    i += 1;
                }
            }
            None
        }
    }
}

/// Checks the lines of `text` (decoded, terminators kept) for `family`.
pub(crate) fn check(text: &str, family: Family) -> Vec<Deviation> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bytes = text.as_bytes();
    let mut checker = Checker {
        family,
        out: Vec::new(),
        prev_level: None,
        open: Vec::new(),
        record_start: 0,
        record_bytes: 0,
    };
    let mut pos = 0;
    let mut line_no = 0_u32;
    while pos < bytes.len() {
        line_no = line_no.saturating_add(1);
        let rest = bytes.get(pos..).unwrap_or_default();
        let (end, term) = match find_eol(rest) {
            Some(k) => (pos + k, rest.get(k..).map_or(1, terminator_len)),
            None => (bytes.len(), 0),
        };
        let line = text.get(pos..end).unwrap_or("");
        let terminator = text.get(end..end + term).unwrap_or("");
        pos = end + term;
        checker.line(line_no, line, terminator);
    }
    checker.record_size();
    checker.out
}

/// The state of [`check`] between lines.
struct Checker<'t> {
    family: Family,
    out: Vec<Deviation>,
    prev_level: Option<u32>,
    open: Vec<Open<'t>>,
    record_start: u32,
    record_bytes: usize,
}

impl<'t> Checker<'t> {
    fn push(&mut self, line: u32, kind: DeviationKind, detail: impl Into<Box<str>>) {
        self.out.push(dev(line, kind, detail));
    }

    /// One line and its terminator (empty for a last line without one).
    fn line(&mut self, line_no: u32, line: &'t str, terminator: &str) {
        let family = self.family;
        if terminator.is_empty() {
            self.push(
                line_no,
                DeviationKind::LineSyntax,
                "the last line has no terminator",
            );
        }
        if family == Family::V7 && terminator == "\n\r" {
            self.push(
                line_no,
                DeviationKind::LineSyntax,
                "LF CR is not a line terminator of GEDCOM 7",
            );
        }
        let trimmed = line.trim_start_matches([' ', '\t', '\u{feff}']);
        if trimmed.is_empty() {
            // 5.5.1 readers ignore white space and extra terminators
            // before a line (p. 11); 7.x has no blank line (§1.3).
            if family == Family::V7 {
                self.push(line_no, DeviationKind::LineSyntax, "a blank line");
            }
            return;
        }
        if trimmed.len() != line.len() && family == Family::V7 {
            self.push(
                line_no,
                DeviationKind::LineSyntax,
                "nothing comes before the level",
            );
        }
        if family == Family::V551 {
            let length = line.chars().count() + terminator.len();
            if length > MAX_LINE {
                self.push(
                    line_no,
                    DeviationKind::LineLength,
                    format!("{length} characters, over {MAX_LINE}"),
                );
            }
        }
        match split(trimmed, family) {
            Ok(parts) => self.structure(line_no, &parts, line.len() + terminator.len()),
            Err((kind, why)) => self.push(line_no, kind, why),
        }
    }

    /// A line that follows the line grammar, `size` bytes long with its
    /// terminator.
    fn structure(&mut self, line_no: u32, parts: &Parts<'t>, size: usize) {
        match self.prev_level {
            None if parts.level != 0 => self.push(
                line_no,
                DeviationKind::Level,
                "the first line is a record, at level 0",
            ),
            Some(p) if parts.level > p.saturating_add(1) => self.push(
                line_no,
                DeviationKind::Level,
                format!("level {} after level {p}", parts.level),
            ),
            _ => {}
        }
        self.prev_level = Some(parts.level);
        if parts.level == 0 {
            self.record_size();
            self.record_start = line_no;
            self.record_bytes = 0;
        }
        self.record_bytes += size;
        if parts.tag.is_empty() {
            self.push(line_no, DeviationKind::LineSyntax, "a line has a tag");
            return;
        }
        if parts.payload == Some("") {
            self.push(
                line_no,
                DeviationKind::LineSyntax,
                "a space after the tag is followed by a payload",
            );
        }
        continuation(&mut self.out, &mut self.open, parts, line_no, self.family);
        if let Some(raw) = parts.payload.filter(|p| !p.is_empty()) {
            let cont = matches!(parts.tag, "CONT" | "CONC");
            if (cont || !is_pointer(raw))
                && let Some(why) = escape_error(raw, self.family)
            {
                self.push(
                    line_no,
                    DeviationKind::Escape,
                    format!("{} {raw:?}: {why}", parts.tag),
                );
            }
        }
    }

    /// The 5.5.1 size rule for the record that ends here.
    fn record_size(&mut self) {
        if self.family == Family::V551 && self.record_bytes > MAX_RECORD_BYTES {
            let bytes = self.record_bytes;
            self.push(
                self.record_start,
                DeviationKind::RecordSize,
                format!("a record of {bytes} bytes, over 32K"),
            );
        }
    }
}

/// The continuation rules: `CONT` (and in 5.5.1 `CONC`) lines come right
/// under the line they continue, before its substructures, have no
/// substructures of their own, never continue a pointer, and a `CONC` never
/// splits at a space (5.5.1 p. 85). GEDCOM 7 has no `CONC` (§1.4).
fn continuation<'t>(
    out: &mut Vec<Deviation>,
    open: &mut Vec<Open<'t>>,
    parts: &Parts<'t>,
    line_no: u32,
    family: Family,
) {
    while open.last().is_some_and(|o| o.level >= parts.level) {
        open.pop();
    }
    let payload = parts.payload.unwrap_or("");
    let cont = matches!(parts.tag, "CONT" | "CONC");
    if !cont {
        if let Some(parent) = open.last_mut() {
            if matches!(parent.tag, "CONT" | "CONC") {
                out.push(dev(
                    line_no,
                    DeviationKind::Continuation,
                    format!("{} has no substructure", parent.tag),
                ));
            }
            parent.has_subs = true;
        }
        open.push(Open {
            level: parts.level,
            tag: parts.tag,
            payload,
            pointer: is_pointer(payload),
            has_subs: false,
        });
        return;
    }
    if parts.tag == "CONC" && family == Family::V7 {
        out.push(dev(
            line_no,
            DeviationKind::Continuation,
            "GEDCOM 7 has no CONC",
        ));
    }
    match open.last_mut() {
        Some(parent) if parent.level.saturating_add(1) == parts.level => {
            if matches!(parent.tag, "CONT" | "CONC") {
                out.push(dev(
                    line_no,
                    DeviationKind::Continuation,
                    format!("{} has no substructure", parent.tag),
                ));
            } else if parent.has_subs {
                out.push(dev(
                    line_no,
                    DeviationKind::Continuation,
                    format!("{} comes before the other substructures", parts.tag),
                ));
            } else if parent.pointer {
                out.push(dev(
                    line_no,
                    DeviationKind::Continuation,
                    "a pointer has no continuation",
                ));
            }
            if parts.tag == "CONC"
                && family == Family::V551
                && (payload.starts_with(' ') || parent.payload.ends_with(' '))
            {
                out.push(dev(
                    line_no,
                    DeviationKind::Continuation,
                    "CONC never splits a value at a space",
                ));
            }
            parent.payload = payload;
        }
        Some(parent) if matches!(parent.tag, "CONT" | "CONC") => out.push(dev(
            line_no,
            DeviationKind::Continuation,
            format!("{} has no substructure", parent.tag),
        )),
        _ => out.push(dev(
            line_no,
            DeviationKind::Continuation,
            format!("{} continues nothing", parts.tag),
        )),
    }
    // A continuation is a leaf: later lines nest under its parent.
    open.push(Open {
        level: parts.level,
        tag: parts.tag,
        payload,
        pointer: false,
        has_subs: false,
    });
}

/// The encoding rules: 7.x is UTF-8 (§1.1); a 5.5.1 file is in the
/// character set its `HEAD.CHAR` names (p. 44), checked against what the
/// bytes turned out to be.
pub(crate) fn encoding(decoded: &Decoded, family: Family) -> Option<Deviation> {
    let enc = decoded.encoding;
    let utf8 = matches!(enc, GedcomEncoding::Utf8);
    let utf16 = matches!(enc, GedcomEncoding::Utf16Le | GedcomEncoding::Utf16Be);
    // ASCII bytes are UTF-8 (and ANSEL, and ASCII) whatever the detection
    // called them.
    let ascii = !utf16 && decoded.text.is_ascii();
    match family {
        Family::V7 => (!(utf8 || ascii)).then(|| {
            dev(
                0,
                DeviationKind::Encoding,
                format!("GEDCOM 7 is UTF-8, these bytes are {enc:?}"),
            )
        }),
        Family::V551 => {
            let declared = decoded.declared.as_deref()?.trim().to_ascii_uppercase();
            let ok = match declared.as_str() {
                "UTF-8" => utf8 || ascii,
                "UNICODE" => utf16,
                "ANSEL" => matches!(enc, GedcomEncoding::Ansel) || ascii,
                "ASCII" => ascii,
                // A character set 5.5.1 does not name is reported as a value
                // of HEAD.CHAR.
                _ => true,
            };
            (!ok).then(|| {
                dev(
                    0,
                    DeviationKind::Encoding,
                    format!("HEAD.CHAR says {declared}, the bytes are {enc:?}"),
                )
            })
        }
    }
}
