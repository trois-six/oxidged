//! The grammars of the payload data types that are not dates, ages or
//! times (those come from [`crate::value`]), and the characters a payload
//! may hold.
//!
//! Each check returns `None` when the payload is valid, else why it is not.

use std::borrow::Cow;

use crate::GedcomVersion;
use crate::value::{AgeValue, DateExact, DatePeriod, DateValue, Grammar, Time};
use crate::version::VersionRules;
use crate::writer::{CONTROL, MAYBE_BANNED, TAB, special_bytes};

use super::schema::{EnumSet, Kind};

/// The grammar family of a version: 7.0 and 7.1 share theirs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    V551,
    V7,
}

impl Family {
    /// The family of a version's rules.
    pub(crate) fn of(rules: &VersionRules) -> Family {
        if rules.version.is_v7() {
            Family::V7
        } else {
            Family::V551
        }
    }

    /// The version whose grammars the family follows.
    pub(crate) fn version(self) -> GedcomVersion {
        match self {
            Family::V551 => GedcomVersion::V5_5_1,
            Family::V7 => GedcomVersion::V7_0,
        }
    }
}

/// Whether a character may not appear in a payload of the tree
/// ([`VersionRules::is_banned`]). Line breaks inside text are written as
/// `CONT` lines, so they are never banned here.
#[inline]
pub(crate) fn is_banned(c: char, rules: &VersionRules) -> bool {
    !matches!(c, '\n' | '\r') && rules.is_banned(c)
}

/// Whether `text` holds a character [`is_banned`] bans. Only a C0 control
/// but a line break, DEL, or the lead bytes of a C1 control (`C2`) or of
/// U+FFFE and U+FFFF (`EF`) can start one: text without them, nearly all
/// of it, is tested eight bytes at a time ([`special_bytes`]).
#[inline]
pub(crate) fn has_banned(text: &str, rules: &VersionRules) -> bool {
    has_banned_known(text, special_bytes(text), rules)
}

/// [`has_banned`] of a text whose [`special_bytes`] are `special`.
#[inline]
pub(crate) fn has_banned_known(text: &str, special: u8, rules: &VersionRules) -> bool {
    special & (TAB | CONTROL | MAYBE_BANNED) != 0 && text.chars().any(|c| is_banned(c, rules))
}

/// Whether `text` is a valid payload of `kind` ([`check`] finds nothing).
#[inline]
pub(crate) fn is_valid(kind: Kind, set: Option<&EnumSet>, text: &str, family: Family) -> bool {
    let simple = match kind {
        // 5.5.1 reads month names in any case (p. 21).
        Kind::Date => is_simple_date(text, false, family == Family::V551),
        Kind::DateExact => is_simple_date(text, true, family == Family::V551),
        _ => false,
    };
    if simple {
        debug_assert_eq!(check(kind, set, text, family), None, "{text:?}");
        return true;
    }
    // The grammars of values tell whether without saying why.
    let grammar = Grammar::from(family.version());
    let valid = match kind {
        Kind::Date => DateValue::is_strict(text, grammar),
        Kind::DateExact => DateExact::is_strict(text, grammar),
        Kind::DatePeriod => DatePeriod::is_strict(text, grammar),
        Kind::Time => Time::is_strict(text, grammar),
        Kind::Age
            if !(family == Family::V551 && matches!(text.as_bytes(), [b'<' | b'>', b' ', ..])) =>
        {
            AgeValue::is_strict(text, grammar)
        }
        _ => return check(kind, set, text, family).is_none(),
    };
    debug_assert_eq!(valid, check(kind, set, text, family).is_none(), "{text:?}");
    valid
}

/// Whether `text` is a Gregorian date of the commonest shapes, valid in
/// both versions without a full parse: `[ABT|CAL|EST|BEF|AFT ]` then
/// `YYYY`, `MON YYYY` or `D MON YYYY` (only the last, without a keyword,
/// when `exact`), single spaces, keywords and months in upper case (months
/// in any case when `any_case`, valid in 5.5.1 only), a year of one to four
/// digits from 1, a day of the month from 1. Anything else is left to the
/// grammar.
pub(crate) fn is_simple_date(text: &str, exact: bool, any_case: bool) -> bool {
    let b = text.as_bytes();
    // The keyword.
    let rest = match b {
        [b'A', b'B', b'T' | b'F', b' ', rest @ ..]
        | [b'C', b'A', b'L', b' ', rest @ ..]
        | [b'E', b'S', b'T', b' ', rest @ ..]
        | [b'B', b'E', b'F', b' ', rest @ ..]
            if !exact =>
        {
            rest
        }
        _ => b,
    };
    // A number of one to `max_len` digits, not starting with 0, and the
    // bytes after it.
    let number = |w: &[u8], max_len: usize| -> Option<u16> {
        if w.is_empty() || w.len() > max_len || w.first() == Some(&b'0') {
            return None;
        }
        w.iter().try_fold(0_u16, |n, &c| {
            c.is_ascii_digit().then(|| n * 10 + u16::from(c - b'0'))
        })
    };
    let leap = |y: u16| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    // The days of month `m` of year `y`.
    let days = |m: [u8; 3], y: u16| -> Option<u16> {
        let upper = m.map(|c| c.to_ascii_uppercase());
        if !any_case && upper != m {
            return None;
        }
        Some(match &upper {
            b"JAN" | b"MAR" | b"MAY" | b"JUL" | b"AUG" | b"OCT" | b"DEC" => 31,
            b"APR" | b"JUN" | b"SEP" | b"NOV" => 30,
            b"FEB" if leap(y) => 29,
            b"FEB" => 28,
            _ => return None,
        })
    };
    // `MON YYYY`: the days of the month.
    let month_year = |w: &[u8]| -> Option<u16> {
        let ([m0, m1, m2, b' '], year) = w.split_first_chunk::<4>().map(|(m, y)| (*m, y))? else {
            return None;
        };
        days([m0, m1, m2], number(year, 4)?)
    };
    match rest.iter().position(|&c| c == b' ') {
        // `YYYY`.
        None => !exact && number(rest, 4).is_some(),
        Some(at) => {
            let (first, after) = rest.split_at(at);
            let after = after.get(1..).unwrap_or_default();
            let digit = first.first().is_some_and(u8::is_ascii_digit);
            if let (Ok(&month), false) = (<&[u8; 3]>::try_from(first), digit) {
                // `MON YYYY`.
                !exact && number(after, 4).is_some_and(|y| days(month, y).is_some())
            } else {
                // `D MON YYYY`.
                number(first, 2)
                    .zip(month_year(after))
                    .is_some_and(|(d, max)| d <= max)
            }
        }
    }
}

/// Why `text` is not a valid payload of `kind` (an enumeration of `set`).
/// Pointers are checked by the caller.
pub(crate) fn check(
    kind: Kind,
    set: Option<&EnumSet>,
    text: &str,
    family: Family,
) -> Option<String> {
    let v = family.version();
    match kind {
        Kind::None => (!text.is_empty()).then(|| "this structure takes no payload".into()),
        Kind::Y => (!(text.is_empty() || text == "Y")).then(|| "only `Y` or nothing".into()),
        // Pointers are checked by the caller.
        Kind::Text | Kind::ListText | Kind::Pointer | Kind::NullablePointer => None,
        Kind::Int => (!is_digits(text)).then(|| "not a non-negative integer".into()),
        Kind::Name => name(text),
        Kind::Date => DateValue::parse_strict(text, v).err().map(|e| e.reason),
        Kind::DateExact => DateExact::parse_strict(text, v).err().map(|e| e.reason),
        Kind::DatePeriod => DatePeriod::parse_strict(text, v).err().map(|e| e.reason),
        Kind::Age => {
            // 5.5.1 shows the bound and the age with a space between them,
            // as its patterns show every part (p. 42); both spellings are
            // read as written.
            let text = match (family, text.as_bytes()) {
                (Family::V551, [b'<' | b'>', b' ', ..]) => {
                    Cow::Owned(format!("{}{}", &text[..1], &text[2..]))
                }
                _ => Cow::Borrowed(text),
            };
            AgeValue::parse_strict(&text, v).err().map(|e| e.reason)
        }
        Kind::Time => Time::parse_strict(text, v).err().map(|e| e.reason),
        // 5.5.1 languages are an enumeration; this kind is 7.x only.
        Kind::Lang => (!is_language_tag(text)).then(|| "not a BCP 47 language tag".into()),
        Kind::MediaType => (!is_media_type(text)).then(|| "not a media type".into()),
        Kind::FilePath => file_path(text, family),
        Kind::Uri => (!is_uri_reference(text)).then(|| "not a URI reference".into()),
        Kind::Lat => (!is_latitude(text, family)).then(|| "not a latitude".into()),
        Kind::Long => (!is_longitude(text, family)).then(|| "not a longitude".into()),
        Kind::TagDef => {
            (!is_tag_def(text)).then(|| "not an extension tag and a URI reference".into())
        }
        Kind::Enum => set.and_then(|s| enum_value(s, text, family)),
        Kind::ListEnum => set.and_then(|s| {
            list_items(text)
                .find_map(|item| enum_value(s, item, family))
                .or_else(|| {
                    text.is_empty()
                        .then(|| format!("not a value of {}", s.name))
                })
        }),
    }
}

/// The items of a list payload (7.x `List`: items separated by commas and
/// any spaces).
pub(crate) fn list_items(text: &str) -> impl Iterator<Item = &str> {
    text.split(',').map(|i| i.trim_matches(' '))
}

/// The 5.5.1 set treated as open: the one deliberate exception to strict
/// conformance (see the [module documentation](super#accepted-practice)).
/// `MULTIMEDIA_FORMAT` (p. 54) lists seven formats of 1999 (`bmp`, `gif`,
/// `jpg`, `ole`, `pcx`, `tif`, `wav`); files name `png`, `pdf` and the
/// like, and no other 5.5.1 structure says what a file is.
pub(crate) const OPEN_551_FORMATS: &str = "MULTIMEDIA_FORMAT";

/// Whether `value` is a 5.5.1 file format as it is written ([`open_format`]).
pub(crate) fn is_open_format(value: &str) -> bool {
    !value.is_empty() && !value.contains('/')
}

/// Why `value` is no 5.5.1 file format: any word is one, as written, but a
/// media type (`image/png`), which the repair writes as its file format.
fn open_format(value: &str) -> Option<String> {
    if value.is_empty() {
        Some("an empty file format".into())
    } else if value.contains('/') {
        Some("a media type, not a 5.5.1 file format".into())
    } else {
        None
    }
}

/// Why `value` is not a value of `set`. 5.5.1 values are matched without
/// regard to case (p. 21); 7.x values are exact, and any extension tag is a
/// value of every set (§1.5.2).
pub(crate) fn enum_value(set: &EnumSet, value: &str, family: Family) -> Option<String> {
    if set.open && !value.is_empty() {
        return None;
    }
    if family == Family::V551 && set.name == OPEN_551_FORMATS {
        return open_format(value);
    }
    let ok = match family {
        Family::V551 => set.values.iter().any(|v| v.eq_ignore_ascii_case(value)),
        Family::V7 => set.values.contains(&value) || is_ext_tag(value),
    };
    (!ok).then(|| format!("not a value of {}", set.name))
}

/// Whether `s` is one or more ASCII digits.
pub(crate) fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// A 7.x extension tag: `_` and one or more `A`–`Z`, digits or `_`.
pub(crate) fn is_ext_tag(s: &str) -> bool {
    s.strip_prefix('_').is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    })
}

/// 7.x `PersonalName` and 5.5.1 `NAME_PERSONAL`: no control character,
/// and either no slash or one pair of slashes around the surname.
fn name(text: &str) -> Option<String> {
    if text.chars().any(char::is_control) {
        return Some("a name holds no control character".into());
    }
    match text.matches('/').count() {
        0 | 2 => None,
        _ => Some("the surname takes one pair of slashes".into()),
    }
}

/// BCP 47 syntax: a primary subtag of 2 to 8 letters (or `x`/`i` for
/// private-use and grandfathered tags), then subtags of 1 to 8 letters or
/// digits, separated by hyphens.
fn is_language_tag(s: &str) -> bool {
    let mut parts = s.split('-');
    let first = parts.next().unwrap_or("");
    let primary = (2..=8).contains(&first.len()) && first.bytes().all(|b| b.is_ascii_alphabetic())
        || matches!(first, "x" | "X" | "i" | "I");
    let mut rest = parts.peekable();
    primary
        && (rest.peek().is_some() || first.len() > 1)
        && rest.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// RFC 6838 `type "/" subtype` with optional `; name=value` parameters.
fn is_media_type(s: &str) -> bool {
    let restricted = |t: &str| {
        let b = t.as_bytes();
        (1..=127).contains(&b.len())
            && b.first().is_some_and(u8::is_ascii_alphanumeric)
            && b.iter()
                .all(|c| c.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(c))
    };
    let mut parts = s.split(';');
    let main = parts.next().unwrap_or("");
    let main_ok = main
        .split_once('/')
        .is_some_and(|(t, sub)| restricted(t) && restricted(sub));
    main_ok
        && parts.all(|p| {
            p.trim_start_matches(' ')
                .split_once('=')
                .is_some_and(|(n, v)| restricted(n) && !v.is_empty() && !v.contains(' '))
        })
}

/// Whether `c` is a URL code point of the WHATWG URL standard, which a
/// valid URL string writes as itself: an ASCII letter or digit, one of
/// `!$&'()*+,-./:;=?@_~`, or a character from U+00A0 to U+10FFFD that is
/// not a noncharacter (7.x §2.12: a file path "can contain non-ASCII
/// characters that are permitted in a valid URL string").
pub(crate) fn is_url_code_point(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || "!$&'()*+,-./:;=?@_~".contains(c);
    }
    let n = u32::from(c);
    n >= 0xA0 && !(0xFDD0..=0xFDEF).contains(&n) && n & 0xFFFE != 0xFFFE
}

/// Whether byte `c` is an RFC 3986 character: unreserved, a sub-delim,
/// `:`, `@`, `/`, `?`, `#`, `[`, `]`, or the `%` of a percent-encoding.
fn is_rfc3986_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?#[]%".contains(&c)
}

/// Whether every `%` of `s` starts a percent-encoding (two hexadecimal
/// digits).
fn percents_ok(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().all(|(i, &c)| {
        c != b'%'
            || b.get(i + 1..i + 3)
                .is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit))
    })
}

/// The scheme of a URI reference, when it has one (RFC 3986 §3.1).
fn scheme(s: &str) -> Option<&str> {
    let (scheme, _) = s.split_once(':')?;
    let b = scheme.as_bytes();
    (b.first().is_some_and(u8::is_ascii_alphabetic)
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || b"+-.".contains(c)))
    .then_some(scheme)
}

/// The authority of a URI reference (`//host` after the scheme, if any)
/// and where it ends, when it has one.
fn authority(s: &str) -> Option<(usize, usize)> {
    let after_scheme = scheme(s).map_or(0, |sc| sc.len() + 1);
    let rest = s.get(after_scheme..)?.strip_prefix("//")?;
    let start = after_scheme + 2;
    let len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some((start, start + len))
}

/// Whether `[` and `]` appear only in the authority of `s`, around an IP
/// literal host, as RFC 3986 and the WHATWG URL standard admit them.
fn brackets_ok(s: &str) -> bool {
    if !s.contains(['[', ']']) {
        return true;
    }
    let Some((start, end)) = authority(s) else {
        return false;
    };
    let host = &s[start..end];
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let outside = s[..start].contains(['[', ']']) || s[end..].contains(['[', ']']);
    !outside
        && host.starts_with('[')
        && host.matches('[').count() == 1
        && host.matches(']').count() == 1
        && host
            .find(']')
            .is_some_and(|at| host[at + 1..].is_empty() || host[at + 1..].starts_with(':'))
}

/// RFC 3986 `URI-reference` (§4.1), the 7.0.17 `URI` data type (7.x
/// §2.13): ASCII only, every character an RFC 3986 one, percent-encodings
/// whole, `#` at most once (before the fragment), brackets only around an
/// IP literal host, and, without a scheme, no `:` in the first path
/// segment (`path-noscheme`). A relative reference is one (it "should be
/// avoided" in shared datasets, which is no grammar rule).
pub(crate) fn is_uri_reference(s: &str) -> bool {
    if !s.bytes().all(is_rfc3986_char) || !percents_ok(s) || s.matches('#').count() > 1 {
        return false;
    }
    if !brackets_ok(s) {
        return false;
    }
    if scheme(s).is_none() && authority(s).is_none() {
        let first = s.split(['/', '?', '#']).next().unwrap_or("");
        if first.contains(':') {
            return false;
        }
    }
    true
}

/// 7.x `FilePath` (§2.12): a "valid URL string" of the WHATWG URL standard
/// that is an `ftp`, `http`, `https` or `file` URL, or a relative URL that
/// does not start with `/`, has no `..` segment, no backslash (escaped or
/// not), no query and no fragment. Every character is a URL code point
/// ([`is_url_code_point`], non-ASCII ones included) or the `%` of a
/// percent-encoding; `[` and `]` only around an IP literal host. A `file`
/// URL names a host without a port (RFC 8089). 5.5.1
/// `MULTIMEDIA_FILE_REFERENCE` is any text.
fn file_path(s: &str, family: Family) -> Option<String> {
    if family == Family::V551 || s.is_empty() {
        return None;
    }
    let chars_ok = s
        .chars()
        .all(|c| is_url_code_point(c) || matches!(c, '%' | '#' | '[' | ']'))
        && percents_ok(s)
        && s.matches('#').count() <= 1
        && brackets_ok(s);
    if !chars_ok {
        return Some("not a valid URL string".into());
    }
    if let Some(scheme) = scheme(s) {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(scheme.as_str(), "ftp" | "http" | "https" | "file") {
            return Some(format!("the {scheme} scheme is not supported"));
        }
        if scheme == "file" {
            let rest = s.get(5..).unwrap_or("");
            if let Some(auth) = rest.strip_prefix("//") {
                let host = auth.split('/').next().unwrap_or("");
                if host.contains([':', '@']) {
                    return Some("a file URL names a host and nothing else".into());
                }
            }
        }
        return None;
    }
    let lower = s.to_ascii_lowercase();
    if s.starts_with('/') {
        Some("a local file path does not start with /".into())
    } else if s.split('/').any(|seg| seg == "..") {
        Some("a local file path has no .. segment".into())
    } else if lower.contains("%5c") {
        Some("a local file path has no backslash".into())
    } else if s.contains(['?', '#']) {
        Some("a local file path has no query or fragment".into())
    } else if s.split('/').next().is_some_and(|seg| seg.contains(':')) {
        Some("not a valid URL string".into())
    } else {
        None
    }
}

/// `N`/`S` (or `E`/`W`), then a decimal number of degrees up to `max`,
/// whose whole part has at most `digits` digits, and whose fraction, when
/// there is a `.`, has one digit or more.
///
/// 7.x (§2.15, §2.16, 7.0.17 with the 7.0.18 correction): `("N" / "S")
/// upto90 [ "." 1*digit ]` with at most two digits before the point, and
/// `("E" / "W") upto180 [ "." 1*digit ]` with at most three, the degrees
/// "limited by definition" to 90 and 180. 5.5.1 `PLACE_LATITUDE` and
/// `PLACE_LONGITUDE` (p. 58) state no grammar beyond their example; up to
/// three digits are read for both.
fn is_coordinate(s: &str, dirs: [char; 2], max: u32, digits: usize) -> bool {
    let Some(rest) = s.strip_prefix(dirs) else {
        return false;
    };
    let (int, frac) = rest.split_once('.').unwrap_or((rest, "0"));
    is_digits(int)
        && is_digits(frac)
        && int.len() <= digits
        && int
            .parse::<u32>()
            .is_ok_and(|d| d < max || d == max && frac.bytes().all(|b| b == b'0'))
}

/// Whether `s` is a latitude of `family`.
fn is_latitude(s: &str, family: Family) -> bool {
    let digits = if family == Family::V7 { 2 } else { 3 };
    is_coordinate(s, ['N', 'S'], 90, digits)
}

/// Whether `s` is a longitude of `family`.
fn is_longitude(s: &str, _family: Family) -> bool {
    is_coordinate(s, ['E', 'W'], 180, 3)
}

/// A 7.x coordinate that is valid once written in its canonical form: the
/// direction in upper case, no leading zero before the degrees but one.
/// `None` when that form is the text itself or is no valid coordinate.
pub(crate) fn canonical_coordinate(kind: Kind, text: &str, family: Family) -> Option<String> {
    let mut chars = text.chars();
    let dir = chars.next()?.to_ascii_uppercase();
    let rest = chars.as_str();
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    let trimmed = int.trim_start_matches('0');
    let int = if trimmed.is_empty() && !int.is_empty() {
        "0"
    } else {
        trimmed
    };
    let fixed = match frac {
        Some(f) => format!("{dir}{int}.{f}"),
        None => format!("{dir}{int}"),
    };
    let ok = match kind {
        Kind::Lat => is_latitude(&fixed, family),
        Kind::Long => is_longitude(&fixed, family),
        _ => false,
    };
    (ok && fixed != text).then_some(fixed)
}

/// 7.x `TagDef` (§2.14, 7.0.17): an extension tag, a space, and a URI
/// reference ([`is_uri_reference`]).
fn is_tag_def(s: &str) -> bool {
    s.split_once(' ')
        .is_some_and(|(tag, uri)| is_ext_tag(tag) && !uri.is_empty() && is_uri_reference(uri))
}

/// The payload kinds for which an empty payload is still a value.
pub(crate) fn empty_is_valid(kind: Kind) -> bool {
    !matches!(
        kind,
        Kind::Int | Kind::Enum | Kind::ListEnum | Kind::Lang | Kind::MediaType
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["John Smith", "John /Smith/", "John /Smith/ Jr.", "//", ""] {
            assert!(name(ok).is_none(), "{ok:?}");
        }
        for bad in ["/", "a/b/c/d", "a\tb"] {
            assert!(name(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn languages() {
        for ok in ["und", "mul", "en", "en-US", "und-Latn-pinyin", "x-private"] {
            assert!(is_language_tag(ok), "{ok:?}");
        }
        for bad in [" ", "-", "und-", "-und", "en US", "", "x", "toolongtag"] {
            assert!(!is_language_tag(bad), "{bad:?}");
        }
    }

    #[test]
    fn media_types() {
        for ok in [
            "application/x-other",
            "text/plain",
            "text/html; charset=utf-8",
        ] {
            assert!(is_media_type(ok), "{ok:?}");
        }
        for bad in [
            "",
            "invalid media type",
            "text/",
            "/text",
            "text/a/b",
            "text",
        ] {
            assert!(!is_media_type(bad), "{bad:?}");
        }
    }

    #[test]
    fn file_paths() {
        for ok in [
            "media/filename",
            "https://example.com/1/16/Portrait_\u{e9}t\u{e9}.jpg",
            "http://www.contoso.com/path/filename",
            "file://host.example.com/path/to/file",
            "file:///path/to/file",
            "foo",
        ] {
            assert!(file_path(ok, Family::V7).is_none(), "{ok:?}");
        }
        for bad in [
            "http://www.contoso.com/path???/file name",
            "c:\\\\directory\\filename",
            "file://c:/directory/filename",
            "http:\\\\\\host/path/file",
            "2013.05.29_14:33:41",
            "/abs/path",
            "media/../x",
            "media/a%5Cb",
            "mailto:someone@example.com",
        ] {
            assert!(file_path(bad, Family::V7).is_some(), "{bad:?}");
        }
        assert!(file_path("c:\\dir\\file", Family::V551).is_none());
    }

    /// 7.x coordinates follow the ABNF of 7.0.17 as 7.0.18 corrected it
    /// (§2.15, §2.16); 5.5.1 reads up to three digits of degrees in both.
    #[test]
    fn coordinates() {
        let lat = |t: &str| is_latitude(t, Family::V7);
        let long = |t: &str| is_longitude(t, Family::V7);
        // The examples of the specification.
        assert!(lat("N18.150944"));
        assert!(long("E168.150944"));
        for ok in ["S90", "N90.000", "N0", "N05", "N89.9", "S8.1"] {
            assert!(lat(ok), "{ok:?}");
        }
        for bad in [
            "N90.5", "N91", "N090", "N005", "N18.", "N.5", "18.15", "n18.15", "N 18", "N18,15",
            "E18", "N+18", "N-18", "N1e2", "",
        ] {
            assert!(!lat(bad), "{bad:?}");
        }
        for ok in [
            "W180", "E180.0", "E0", "E05", "E099", "E100", "E179.99", "W9", "E17",
        ] {
            assert!(long(ok), "{ok:?}");
        }
        // 7.0.17 spelled the longitude with `N`/`S`, which 7.0.18 corrected.
        for bad in [
            "W200", "E181", "E180.5", "E0180", "E1800", "N18", "S18", "E18.", "e18", "",
        ] {
            assert!(!long(bad), "{bad:?}");
        }
        // 5.5.1 is as it was.
        assert!(is_latitude("N090", Family::V551));
        assert!(!is_latitude("N090", Family::V7));
        assert!(check(Kind::Lat, None, "N005", Family::V551).is_none());
        assert_eq!(
            check(Kind::Lat, None, "N005", Family::V7).as_deref(),
            Some("not a latitude")
        );
    }

    #[test]
    fn canonical_coordinates() {
        let fix = |kind, t: &str| canonical_coordinate(kind, t, Family::V7);
        assert_eq!(fix(Kind::Lat, "N005").as_deref(), Some("N5"));
        assert_eq!(fix(Kind::Lat, "n18.150944").as_deref(), Some("N18.150944"));
        assert_eq!(fix(Kind::Lat, "S000.5").as_deref(), Some("S0.5"));
        assert_eq!(fix(Kind::Long, "w0168.15").as_deref(), Some("W168.15"));
        // Valid already, or not a coordinate in any form.
        assert_eq!(fix(Kind::Lat, "N18"), None);
        assert_eq!(fix(Kind::Lat, "N090.5"), None);
        assert_eq!(fix(Kind::Long, "E181"), None);
        assert_eq!(fix(Kind::Lat, "X18"), None);
        assert_eq!(fix(Kind::Lat, ""), None);
    }

    /// The 7.0.17 `URI` data type: an RFC 3986 URI reference (§2.13).
    #[test]
    fn uri_references() {
        for ok in [
            "https://gedcom.io/terms/v7/record-INDI",
            "http://www.familysearch.org/ark:/61903/",
            "urn:uuid:3c1d8c58-6e0b-4b0f-a2f9-1d8e3f0b7c11",
            "https://[::1]:8080/a?b=c#d",
            "mailto:someone@example.com",
            "relative/path",
            "./a:b",
            "#fragment",
            "?query",
            "//host/path",
            "a%20b",
            "",
        ] {
            assert!(is_uri_reference(ok), "{ok:?}");
        }
        for bad in [
            "has space",
            "https://example.com/caf\u{e9}",
            "a%2",
            "a%zz",
            "a#b#c",
            "1a:b",
            "https://example.com/[x]",
            "x[1]",
            "a\\b",
            "<https://example.com>",
        ] {
            assert!(!is_uri_reference(bad), "{bad:?}");
        }
        assert_eq!(
            check(Kind::Uri, None, "relative/type", Family::V7),
            None,
            "a relative URI is a URI reference"
        );
    }

    /// The 7.0.17 `TagDef` data type: `extTag D URI-reference` (§2.14).
    #[test]
    fn tag_definitions() {
        for ok in [
            "_SKYPEID http://xmlns.com/foaf/0.1/skypeID",
            "_MEMBER http://xmlns.com/foaf/0.1/member",
            "_LOC https://gedcom.io/terms/v7.1/record-LOC",
            "_X relative/uri",
        ] {
            assert!(is_tag_def(ok), "{ok:?}");
        }
        for bad in [
            "SKYPEID http://xmlns.com/foaf/0.1/skypeID",
            "_SKYPEID",
            "_SKYPEID ",
            "_SKYPEID  http://xmlns.com/foaf/0.1/skypeID",
            "_SKYPEID http://xmlns.com/foaf/0.1/skype ID",
            "_skype http://example.com/",
            "_X 1a:b",
        ] {
            assert!(!is_tag_def(bad), "{bad:?}");
        }
    }

    /// File paths are WHATWG valid URL strings (§2.12, 7.0.17): non-ASCII
    /// URL code points as themselves, anything else percent-encoded.
    #[test]
    fn file_paths_are_url_strings() {
        for ok in [
            "photos/\u{e9}t\u{e9}.jpg",
            "photos/\u{65e5}\u{672c}.png",
            "photos/a%20b.jpg",
            "photos/%E2%82%AC.jpg",
            "https://[2001:db8::1]/a.jpg",
            "https://example.com/a.jpg#page=2",
            "file:///C:/photos/a%20b.jpg",
        ] {
            assert!(file_path(ok, Family::V7).is_none(), "{ok:?}");
        }
        for bad in [
            "photos/a b.jpg",
            "photos/a\u{fdd0}.jpg",
            "photos/a\u{1fffe}.jpg",
            "photos/a\u{9f}.jpg",
            "photos/a[1].jpg",
            "photos/a%2.jpg",
            "photos/a|b.jpg",
            "photos/a\"b.jpg",
            "https://example.com/a#b#c",
        ] {
            assert!(file_path(bad, Family::V7).is_some(), "{bad:?}");
        }
        for c in ['a', '-', '~', '\u{a0}', '\u{10fffd}', '\u{fffd}'] {
            assert!(is_url_code_point(c), "{c:?}");
        }
        for c in [
            ' ',
            '"',
            '#',
            '%',
            '<',
            '[',
            '\\',
            '^',
            '`',
            '{',
            '|',
            '\u{9f}',
            '\u{fdef}',
            '\u{ffff}',
            '\u{10ffff}',
        ] {
            assert!(!is_url_code_point(c), "{c:?}");
        }
    }

    #[test]
    fn banned_characters() {
        use crate::version::{V70, V551};
        assert!(is_banned('\u{7}', &V70));
        assert!(!is_banned('\t', &V70));
        assert!(is_banned('\t', &V551));
        assert!(!is_banned('\n', &V551));
        assert!(!is_banned('\r', &V70));
        assert!(is_banned('\u{85}', &V70));
        assert!(!is_banned('é', &V551));
    }

    #[test]
    fn banned_characters_by_bytes() {
        use crate::version::{V70, V551};
        for rules in [&V551, &V70] {
            for c in [
                'a', 'é', '\t', '\n', '\r', '\u{7}', '\u{7f}', '\u{85}', '\u{a0}', '\u{fffd}',
            ]
            .into_iter()
            .chain(['\u{fffe}', '\u{ffff}', '\u{10000}'])
            {
                let text = format!("ab{c}cd\nef");
                assert_eq!(
                    has_banned(&text, rules),
                    text.chars().any(|c| is_banned(c, rules)),
                    "{c:?}"
                );
            }
        }
    }

    /// [`is_simple_date`] by words, as it was first written: the reference
    /// of the byte-wise one.
    fn simple_date_by_words(text: &str, exact: bool, any_case: bool) -> bool {
        let number = |w: &[u8], max_len: usize| -> Option<u16> {
            if w.is_empty() || w.len() > max_len || w.first() == Some(&b'0') {
                return None;
            }
            w.iter().try_fold(0_u16, |n, &c| {
                c.is_ascii_digit().then(|| n * 10 + u16::from(c - b'0'))
            })
        };
        let leap =
            |y: u16| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
        let days = |m: &[u8], y: u16| -> Option<u16> {
            let upper = match *m {
                [x, y, z] => [x, y, z].map(|c| c.to_ascii_uppercase()),
                _ => return None,
            };
            if !any_case && upper != m {
                return None;
            }
            Some(match &upper {
                b"JAN" | b"MAR" | b"MAY" | b"JUL" | b"AUG" | b"OCT" | b"DEC" => 31,
                b"APR" | b"JUN" | b"SEP" | b"NOV" => 30,
                b"FEB" if leap(y) => 29,
                b"FEB" => 28,
                _ => return None,
            })
        };
        let mut words = text.as_bytes().split(|&c| c == b' ');
        let mut first = words.next();
        if !exact && matches!(first, Some(b"ABT" | b"CAL" | b"EST" | b"BEF" | b"AFT")) {
            first = words.next();
        }
        match (first, words.next(), words.next(), words.next()) {
            (Some(y), None, None, None) => !exact && number(y, 4).is_some(),
            (Some(m), Some(y), None, None) => {
                !exact && number(y, 4).is_some_and(|y| days(m, y).is_some())
            }
            (Some(d), Some(m), Some(y), None) => number(y, 4).is_some_and(|y| {
                number(d, 2)
                    .zip(days(m, y))
                    .is_some_and(|(d, max)| d <= max)
            }),
            _ => false,
        }
    }

    /// The byte-wise test of simple dates answers as the test by words.
    #[test]
    fn simple_dates_by_bytes() {
        let parts = [
            "",
            " ",
            "ABT",
            "abt",
            "BET",
            "1",
            "01",
            "9",
            "29",
            "31",
            "32",
            "123",
            "JAN",
            "FEB",
            "Feb",
            "fEB",
            "XYZ",
            "1900",
            "2000",
            "1999",
            "0",
            "10000",
            "@#DJULIAN@",
            "1700",
        ];
        let mut n = 0;
        for a in parts {
            for b in parts {
                for c in parts {
                    for d in ["", " ", " 1900", "  1900", " X"] {
                        for text in [
                            format!("{a} {b} {c}{d}"),
                            format!("{a}{b} {c}{d}"),
                            format!("{a} {b}{c}{d}"),
                            format!("{a}{d}"),
                            format!("{a} {b}{d}"),
                        ] {
                            for exact in [false, true] {
                                for any_case in [false, true] {
                                    assert_eq!(
                                        is_simple_date(&text, exact, any_case),
                                        simple_date_by_words(&text, exact, any_case),
                                        "{text:?} {exact} {any_case}"
                                    );
                                    n += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(n > 100_000, "{n}");
    }

    /// The dates the fast path takes are exactly those of the grammar that
    /// it takes: every one valid in both versions.
    #[test]
    fn simple_dates_are_valid() {
        let months = [
            "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
            "Jan", "oct", "fEb", "FOO", "Foo",
        ];
        let mut taken = 0;
        for keyword in [
            "", "ABT ", "CAL ", "EST ", "BEF ", "AFT ", "abt ", "BET ", "FROM ",
        ] {
            for year in [
                "1", "4", "99", "100", "400", "1600", "1700", "1900", "1999", "2000", "2024",
                "9999", "0", "01", "10000",
            ] {
                let mut texts = vec![format!("{keyword}{year}")];
                for month in months {
                    texts.push(format!("{keyword}{month} {year}"));
                    for day in ["0", "1", "9", "01", "28", "29", "30", "31", "32"] {
                        texts.push(format!("{keyword}{day} {month} {year}"));
                        texts.push(format!("{keyword}{day}  {month} {year}"));
                    }
                }
                for text in texts {
                    for (kind, exact) in [(Kind::Date, false), (Kind::DateExact, true)] {
                        if is_simple_date(&text, exact, true) {
                            assert_eq!(
                                check(kind, None, &text, Family::V551),
                                None,
                                "{text:?} {kind:?}"
                            );
                            // Written as it is in 5.5.1, its month in upper
                            // case in 7.x.
                            let date = crate::value::DateTexts {
                                value: Some(text.clone()),
                                time: None,
                                phrase: None,
                            };
                            let upper = crate::value::DateTexts {
                                value: Some(text.to_ascii_uppercase()),
                                ..date.clone()
                            };
                            assert_eq!(date.to_version(GedcomVersion::V5_5_1), date, "{text:?}");
                            for version in [GedcomVersion::V7_0, GedcomVersion::V7_1] {
                                assert_eq!(date.to_version(version), upper, "{text:?} {version}");
                            }
                        }
                        if !is_simple_date(&text, exact, false) {
                            continue;
                        }
                        taken += 1;
                        for family in [Family::V551, Family::V7] {
                            assert_eq!(check(kind, None, &text, family), None, "{text:?} {kind:?}");
                        }
                        // Written alike in every version (the conversion
                        // of the typed model skips them).
                        let date = crate::value::DateTexts {
                            value: Some(text.clone()),
                            time: None,
                            phrase: None,
                        };
                        for version in [
                            GedcomVersion::V5_5_1,
                            GedcomVersion::V7_0,
                            GedcomVersion::V7_1,
                        ] {
                            assert_eq!(date.to_version(version), date, "{text:?} {version}");
                        }
                    }
                }
            }
        }
        assert!(taken > 5000, "{taken}");
        assert!(!is_simple_date("29 FEB 1900", false, false));
        assert!(is_simple_date("29 FEB 2000", true, false));
        assert!(!is_simple_date("ABT 1 JAN 1900", true, false));
        assert!(!is_simple_date("2 Oct 1822", false, false));
        assert!(is_simple_date("2 Oct 1822", true, true));
    }
}
