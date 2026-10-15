//! Conversions of payload texts: escape sequences, file references,
//! languages and identifiers.

use std::fmt::Write as _;

use super::tables::{LANGUAGE_ALIASES, LANGUAGES, MEDIA_TYPES};

/// A 5.5.1 text with its escape sequences (`@#…@`, p. 11) rewritten for
/// 7.x, which has none (§1.3): a calendar escape (`@#DJULIAN@`) becomes the
/// calendar's name, spaces as underscores (`JULIAN`), followed by a space;
/// any other escape, which no 5.5.1 form defines, is left out. As 5.5.1
/// asks of readers, the space after an escape is part of it. `None` when
/// the text holds no escape.
pub(super) fn without_escapes(text: &str) -> Option<String> {
    let mut rest = text;
    let mut out: Option<String> = None;
    while let Some(at) = rest.find("@#") {
        let Some(len) = rest.get(at + 2..).and_then(|r| r.find('@')) else {
            break;
        };
        let body = rest.get(at + 2..at + 2 + len).unwrap_or("");
        let o = out.get_or_insert_with(|| String::with_capacity(text.len()));
        o.push_str(rest.get(..at).unwrap_or(""));
        if let Some(calendar) = body.strip_prefix('D') {
            o.extend(calendar.chars().map(|c| if c == ' ' { '_' } else { c }));
            o.push(' ');
        }
        let after = rest.get(at + 2 + len + 1..).unwrap_or("");
        rest = after.strip_prefix(' ').unwrap_or(after);
    }
    let mut out = out?;
    out.push_str(rest);
    Some(out)
}

/// The characters a 7.x file reference writes as themselves; any other is
/// percent-encoded. Non-ASCII URL code points are kept, as a WHATWG valid
/// URL string admits them (§2.12, 7.0.17); `:`, `?`, `@` and `#`, which a
/// local path would read as a scheme, a query or a fragment, are not.
fn is_path_char(c: char) -> bool {
    !matches!(c, ':' | '?' | '@' | '#') && crate::spec::payload::is_url_code_point(c)
}

/// Appends `c`, percent-encoded unless it is a path character.
fn push_encoded(out: &mut String, c: char) {
    if is_path_char(c) {
        out.push(c);
    } else {
        percent_encode(out, c);
    }
}

/// Appends `c` percent-encoded, each byte of its UTF-8 as `%XX`.
fn percent_encode(out: &mut String, c: char) {
    let mut buf = [0; 4];
    for b in c.encode_utf8(&mut buf).bytes() {
        let _ = write!(out, "%{b:02X}");
    }
}

/// Whether `s` starts with a URL scheme and `//` (`https://…`), which a
/// file reference keeps as it is.
fn is_url(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        && rest.starts_with("//")
}

/// A URL (`https://…`) as a valid URL string: what the WHATWG URL
/// standard's serializer percent-encodes is (a space, `"`, `<`, `>`, `` ` ``,
/// `{`, `}`, `|`, `^`, `\\`, control characters, a second `#`, brackets
/// outside an IP literal host, a `%` that starts no encoding); valid
/// characters and existing `%XX` encodings stay as they are. `None` when
/// nothing needs encoding.
fn url_encoded(url: &str) -> Option<String> {
    // Where brackets may stand: the host of the authority.
    let host_end = url.find("//").map(|at| at + 2).map(|start| {
        start
            + url[start..]
                .find(['/', '?', '#'])
                .unwrap_or(url.len() - start)
    });
    let mut out = String::with_capacity(url.len() + 8);
    let mut fragment = false;
    for (i, c) in url.char_indices() {
        match c {
            '%' if url
                .get(i + 1..i + 3)
                .is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit())) =>
            {
                out.push('%');
            }
            '#' if !fragment => {
                fragment = true;
                out.push('#');
            }
            '[' | ']' if host_end.is_some_and(|end| i < end) => out.push(c),
            c if crate::spec::payload::is_url_code_point(c) => out.push(c),
            c => percent_encode(&mut out, c),
        }
    }
    (out != url).then_some(out)
}

/// A 5.5.1 file reference (any text naming a file) as a 7.x file path, a
/// valid URL string (§2.12): a URL keeps its form, the characters a URL
/// string does not permit percent-encoded ([`url_encoded`]); a Windows drive path
/// (`C:\x`) becomes `file:///C:/x`, a network path (`\\host\x`)
/// `file://host/x`, an absolute path (`/x`) `file:///x`; backslashes
/// become slashes; characters a URL string does not permit, `%` not
/// starting an encoding, and `:` (which would start a scheme), are
/// percent-encoded.
/// `None` when the reference is a valid one already.
pub(crate) fn file_uri(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    if is_url(path) {
        return url_encoded(path);
    }
    let mut out = String::with_capacity(path.len() + 8);
    let bytes = path.as_bytes();
    let drive = bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes.get(1) == Some(&b':')
        && bytes.get(2).is_none_or(|b| matches!(b, b'\\' | b'/'));
    let mut rest = path;
    if drive {
        out.push_str("file:///");
        out.push_str(path.get(..2).unwrap_or(""));
        rest = path.get(2..).unwrap_or("");
    } else if let Some(host) = path
        .strip_prefix("\\\\")
        .filter(|r| !r.starts_with('\\'))
        .or_else(|| path.strip_prefix("//").filter(|r| !r.starts_with('/')))
    {
        out.push_str("file://");
        rest = host;
    } else if path.starts_with(['/', '\\']) {
        out.push_str("file://");
    }
    for (i, c) in rest.char_indices() {
        match c {
            '\\' => out.push('/'),
            '%' if rest
                .get(i + 1..i + 3)
                .is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit())) =>
            {
                out.push('%');
            }
            c => push_encoded(&mut out, c),
        }
    }
    (out != path).then_some(out)
}

/// The value of hexadecimal digit `b`.
fn hex(b: u8) -> Option<u8> {
    char::from(b)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
}

/// `s` with its percent-encodings decoded, as long as the result is UTF-8
/// and decodes no slash or control character (which would change the
/// path); `None` otherwise or when nothing is encoded.
fn percent_decoded(s: &str) -> Option<String> {
    if !s.contains('%') {
        return None;
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).copied().and_then(hex),
                b.get(i + 2).copied().and_then(hex),
            )
        {
            let byte = h * 16 + l;
            if byte == b'/' || byte < 0x20 || byte == 0x7f {
                return None;
            }
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// A 7.x file path written back as a 5.5.1 file reference: a `file` URL as
/// the local or network path it names (`file:///C:/x` is `C:\x`,
/// `file://host/x` is `\\host\x`, `file:///x` is `/x`), a relative
/// reference decoded (`a%20b` is `a b`); other URLs stay as they are.
/// `None` when the path is written as it is.
pub(super) fn file_path(uri: &str) -> Option<String> {
    let lower = uri.get(..7).map(str::to_ascii_lowercase);
    if lower.as_deref() == Some("file://") {
        let rest = uri.get(7..).unwrap_or("");
        let decoded = percent_decoded(rest).unwrap_or_else(|| rest.to_string());
        if let Some(local) = decoded.strip_prefix('/') {
            let b = local.as_bytes();
            let drive = b.first().is_some_and(u8::is_ascii_alphabetic) && b.get(1) == Some(&b':');
            if drive {
                return Some(local.replace('/', "\\"));
            }
            return Some(decoded);
        }
        return Some(format!("\\\\{}", decoded.replace('/', "\\")));
    }
    if is_url(uri) {
        return None;
    }
    percent_decoded(uri)
}

/// The BCP 47 tag of a 5.5.1 language name, matched without regard to case
/// (`English` is `en`); a name in neither list is `None`.
pub(super) fn language_tag(name: &str) -> Option<&'static str> {
    let name = name.trim();
    LANGUAGES
        .iter()
        .chain(LANGUAGE_ALIASES)
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, tag)| tag)
}

/// The 5.5.1 language name of a BCP 47 tag (`en` is `English`), matched
/// without regard to case; a tag no 5.5.1 name stands for is `None`.
pub(super) fn language_name(tag: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(_, t)| t.eq_ignore_ascii_case(tag.trim()))
        .map(|&(name, _)| name)
}

/// The 7.x media type of a 5.5.1 file format (`jpg` is `image/jpeg`); a
/// format of no known type `f` is `application/x-f`. A media type already
/// is kept (`None`), as is an empty format.
pub(super) fn media_type(format: &str) -> Option<String> {
    let format = format.trim();
    if format.is_empty() || format.contains('/') {
        return None;
    }
    let lower = format.to_ascii_lowercase();
    if let Some(&(_, mt)) = MEDIA_TYPES.iter().find(|(f, _)| *f == lower) {
        return Some(mt.to_string());
    }
    let token: String = lower
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "!#$&-^_.+".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    Some(format!("application/x-{token}"))
}

/// The 5.5.1 file format of a 7.x media type, the inverse of
/// [`media_type`]: `image/jpeg` is `jpg`, `application/x-f` is `f`. `None`
/// for any other media type (which the conformance repair names by its
/// subtype).
pub(super) fn file_format(media_type: &str) -> Option<String> {
    let lower = media_type.trim().to_ascii_lowercase();
    let main = lower.split(';').next().unwrap_or("").trim();
    if let Some(&(f, _)) = MEDIA_TYPES.iter().find(|(_, t)| *t == main) {
        return Some(f.to_string());
    }
    main.strip_prefix("application/x-")
        .filter(|f| !f.is_empty())
        .map(str::to_string)
}

/// `s` percent-encoded for a URI fragment (RFC 3986 §3.5).
pub(super) fn fragment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || "-._~!$&'()*+,;=:@/?".contains(c) {
            out.push(c);
        } else {
            percent_encode(&mut out, c);
        }
    }
    out
}

/// A URI fragment decoded ([`fragment`]'s inverse); as it is when it does
/// not decode.
pub(super) fn unfragment(s: &str) -> String {
    percent_decoded(s).unwrap_or_else(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test files' `atsign` conversions (gedcom7code, Unlicense).
    #[test]
    fn escapes_become_calendar_names_or_go() {
        let cases = [
            ("@#DJULIAN@date escape", "JULIAN date escape"),
            ("@#DJULIAN@ date escape", "JULIAN date escape"),
            ("@#DJULIAN@  date escape", "JULIAN  date escape"),
            ("non-leading @#DJULIAN@ date", "non-leading JULIAN date"),
            ("@#DJULIAN AND SUCH@ date", "JULIAN_AND_SUCH date"),
            ("obsolete @#OLD FORM@ escape", "obsolete escape"),
            ("obsolete@#OLD FORM@ escape", "obsoleteescape"),
            ("obsolete@#OLD FORM@  escape", "obsolete escape"),
            ("obsolete @#OLD FORM@escape", "obsolete escape"),
            (
                "@all in @one@thing @#DWITH DATES@ , @#OBSOLETE@ etc",
                "@all in @one@thing WITH_DATES , etc",
            ),
        ];
        for (from, to) in cases {
            assert_eq!(without_escapes(from).as_deref(), Some(to), "{from}");
        }
        assert_eq!(without_escapes("no escape @ here"), None);
        assert_eq!(without_escapes("open @#escape"), None);
    }

    /// The test files' `filename-1` conversions, and back.
    #[test]
    fn file_references_become_uris_and_back() {
        let cases = [
            ("/unix/absolute", "file:///unix/absolute"),
            ("c:\\windows\\absolute", "file:///c:/windows/absolute"),
            ("\\\\windows\\server", "file://windows/server"),
            ("a\\relative\\path", "a/relative/path"),
            (
                "most/paths?get#escaped[like]this",
                "most/paths%3Fget%23escaped%5Blike%5Dthis",
            ),
            (
                "spaces are fine/even with slashes ? and so on",
                "spaces%20are%20fine/even%20with%20slashes%20%3F%20and%20so%20on",
            ),
            (
                "https:/not.a.url/even-though-similar",
                "https%3A/not.a.url/even-though-similar",
            ),
            ("100%25 done", "100%25%20done"),
            ("50% off", "50%25%20off"),
            // Non-ASCII URL code points stay as they are (7.0.17); a
            // noncharacter is no URL code point.
            ("photos/\u{e9}t\u{e9} 1.jpg", "photos/\u{e9}t\u{e9}%201.jpg"),
            ("a\u{fdd0}.jpg", "a%EF%B7%90.jpg"),
        ];
        for (from, to) in cases {
            assert_eq!(file_uri(from).as_deref(), Some(to), "{from}");
        }
        assert_eq!(file_uri("a/relative/path"), None);
        assert_eq!(file_uri("https://leave.alone?with=args#and-frags"), None);
        // A URL keeps its form; what a valid URL string does not permit is
        // percent-encoded, existing encodings and IP literals kept.
        for (from, to) in [
            (
                "https://example.org/a b.jpg",
                "https://example.org/a%20b.jpg",
            ),
            (
                "https://example.org/a%20b c.jpg",
                "https://example.org/a%20b%20c.jpg",
            ),
            (
                "http://example.org/p?q=a b#frag ment",
                "http://example.org/p?q=a%20b#frag%20ment",
            ),
            ("https://example.org/x#a#b", "https://example.org/x#a%23b"),
            ("https://[::1]/a[1].jpg", "https://[::1]/a%5B1%5D.jpg"),
            (
                "https://example.org/{x}|\"y\"<z>",
                "https://example.org/%7Bx%7D%7C%22y%22%3Cz%3E",
            ),
            (
                "https://example.org/100% done",
                "https://example.org/100%25%20done",
            ),
            (
                "ftp://example.org/\u{e9}t\u{e9} 1.jpg",
                "ftp://example.org/\u{e9}t\u{e9}%201.jpg",
            ),
        ] {
            let fixed = file_uri(from);
            assert_eq!(fixed.as_deref(), Some(to), "{from}");
            assert!(
                crate::spec::payload::check(
                    crate::spec::schema::Kind::FilePath,
                    None,
                    to,
                    crate::spec::payload::Family::V7
                )
                .is_none(),
                "{to}"
            );
            assert_eq!(file_uri(to), None, "{to}");
        }

        assert_eq!(
            file_path("file:///unix/absolute").as_deref(),
            Some("/unix/absolute")
        );
        assert_eq!(
            file_path("file:///c:/windows/absolute").as_deref(),
            Some("c:\\windows\\absolute")
        );
        assert_eq!(
            file_path("file://windows/server").as_deref(),
            Some("\\\\windows\\server")
        );
        assert_eq!(file_path("a%20b/c%3Fd").as_deref(), Some("a b/c?d"));
        assert_eq!(file_path("https://x.example/a%20b"), None);
        assert_eq!(file_path("a/b"), None);
        // An encoded slash stays encoded: decoding it would change the path.
        assert_eq!(file_path("a%2Fb"), None);
    }

    #[test]
    fn languages_both_ways() {
        assert_eq!(language_tag("English"), Some("en"));
        assert_eq!(language_tag("ENGlish"), Some("en"));
        assert_eq!(language_tag("serbo_croa"), Some("sh"));
        assert_eq!(language_tag("Catalan_Spn"), Some("ca-ES"));
        assert_eq!(language_tag("Hmong"), None);
        assert_eq!(language_name("EN"), Some("English"));
        assert_eq!(language_name("ca-ES"), Some("Catalan_Spn"));
        assert_eq!(language_name("zh"), None);
        assert_eq!(LANGUAGES.len(), 86);
    }

    #[test]
    fn media_types_both_ways() {
        assert_eq!(media_type("jpg").as_deref(), Some("image/jpeg"));
        assert_eq!(media_type("WAV").as_deref(), Some("audio/wav"));
        assert_eq!(media_type("other").as_deref(), Some("application/x-other"));
        assert_eq!(media_type("image/png"), None);
        assert_eq!(file_format("image/jpeg").as_deref(), Some("jpg"));
        assert_eq!(file_format("application/x-other").as_deref(), Some("other"));
        assert_eq!(
            file_format("application/x-oleobject").as_deref(),
            Some("ole")
        );
        assert_eq!(file_format("video/webm"), None);
    }

    #[test]
    fn fragments_round_trip() {
        assert_eq!(fragment("Example App 2"), "Example%20App%202");
        assert_eq!(unfragment(&fragment("Ex#ample é")), "Ex#ample é");
    }
}
