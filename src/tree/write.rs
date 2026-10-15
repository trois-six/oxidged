//! Writing structures back as lines: the inverse of the reader.

use std::borrow::Cow;
use std::fmt::Write as _;

use super::PayloadRef;
use super::lexer::Escaping;

/// Writes one structure as a line (and `CONT` lines for the newlines of its
/// text), LF-terminated.
///
/// A structure with neither tag nor identifier came from a line without a
/// level number, and is written back as that bare line.
pub(crate) fn line(
    out: &mut String,
    level: usize,
    xref: Option<&str>,
    tag: &str,
    payload: PayloadRef<'_>,
    escaping: Escaping,
) {
    let text = match payload {
        PayloadRef::Text(t) if t.contains('\r') => {
            Some(Cow::Owned(t.replace("\r\n", "\n").replace('\r', "\n")))
        }
        PayloadRef::Text(t) => Some(Cow::Borrowed(t)),
        PayloadRef::None | PayloadRef::Pointer(_) => None,
    };
    if tag.is_empty() && xref.is_none() {
        // The line as it was; what joined it later continues with `CONT`
        // (a bare line could be blank, and blank lines are skipped).
        if let Some(text) = text {
            let mut parts = text.split('\n');
            out.push_str(parts.next().unwrap_or_default());
            out.push('\n');
            continuation_lines(out, level, parts, escaping);
        }
        return;
    }
    let _ = write!(out, "{level}");
    if let Some(xref) = xref {
        out.push(' ');
        out.push_str(xref);
    }
    out.push(' ');
    out.push_str(tag);
    match (payload, text) {
        (PayloadRef::Pointer(p), _) => {
            out.push(' ');
            out.push_str(p);
            out.push('\n');
        }
        (_, Some(text)) => {
            let mut parts = text.split('\n');
            if let Some(first) = parts.next().filter(|f| !f.is_empty()) {
                if xref.is_some_and(|x| !is_closed(x)) {
                    // An identifier without its closing `@` (`@`, `@a`)
                    // would take the text up to an `@` in it, as an
                    // identifier holding spaces: the text continues on a
                    // `CONC` line, which joins it back.
                    let _ = write!(out, "\n{} CONC", level + 1);
                }
                out.push(' ');
                escape_into(first, escaping, out);
            }
            out.push('\n');
            continuation_lines(out, level, parts, escaping);
        }
        _ => out.push('\n'),
    }
}

/// Whether an identifier ends with the `@` that closes it, so that the
/// lexer ends it there whatever follows on its line.
fn is_closed(xref: &str) -> bool {
    xref.len() >= 2 && xref.ends_with('@')
}

/// Writes each part as a `CONT` line one level below `level`.
fn continuation_lines<'a>(
    out: &mut String,
    level: usize,
    parts: impl Iterator<Item = &'a str>,
    escaping: Escaping,
) {
    for part in parts {
        let _ = write!(out, "{} CONT", level + 1);
        if !part.is_empty() {
            out.push(' ');
            escape_into(part, escaping, out);
        }
        out.push('\n');
    }
}

/// Escapes the `@` of one line of text: 7.0 doubles a leading `@`; 5.5.1
/// doubles every `@` except those of an escape sequence (`@#DJULIAN@`).
fn escape_into(text: &str, escaping: Escaping, out: &mut String) {
    match escaping {
        Escaping::V70 => {
            if text.starts_with('@') {
                out.push('@');
            }
            out.push_str(text);
        }
        Escaping::V551 => {
            let mut rest = text;
            while let Some(at) = rest.find('@') {
                out.push_str(rest.get(..at).unwrap_or_default());
                let after = rest.get(at + 1..).unwrap_or_default();
                if let Some(close) = after.strip_prefix('#').and_then(|a| a.find('@')) {
                    // `@#` + escape text + `@`, kept as it is.
                    let end = at + close + 3;
                    out.push_str(rest.get(at..end).unwrap_or_default());
                    rest = rest.get(end..).unwrap_or_default();
                } else {
                    out.push_str("@@");
                    rest = after;
                }
            }
            out.push_str(rest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esc(text: &str, e: Escaping) -> String {
        let mut s = String::new();
        escape_into(text, e, &mut s);
        s
    }

    #[test]
    fn escapes() {
        assert_eq!(esc("@@@@ has four", Escaping::V70), "@@@@@ has four");
        assert_eq!(esc("a@b", Escaping::V70), "a@b");
        assert_eq!(esc("a@b @c", Escaping::V551), "a@@b @@c");
        assert_eq!(esc("@#DJULIAN@ 1700", Escaping::V551), "@#DJULIAN@ 1700");
        assert_eq!(esc("@#open", Escaping::V551), "@@#open");
    }

    /// An identifier without its closing `@` keeps the text of its line
    /// apart: written on the line, an `@` in it would close the identifier
    /// (`1 @ n \t@` reads as the identifier `@ n \t@`, without a tag).
    #[test]
    fn an_unclosed_identifier_keeps_its_text_apart() {
        for input in [
            "0 HEAD\n1 GEDC\n2 VERS 7\n1\t@\tn\n1\tCONC\t\t@",
            "0 HEAD\n1 @a NOTE\n2 CONC x @\n2 CONT y @",
            "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 @ NOTE\n2 CONC a@ b",
        ] {
            let tree = crate::tree::Tree::parse(input);
            let dump = tree.to_gedcom();
            assert_eq!(
                crate::tree::parse_tree(&dump).to_structures(),
                tree.to_structures(),
                "{input:?}\n{dump}"
            );
        }
        let tree = crate::tree::Tree::parse("0 HEAD\n1 @ n\n1 CONC \t@\n");
        assert_eq!(tree.to_gedcom(), "0 HEAD\n1 @ n\n2 CONC \t@@\n"); // 5.5.1 doubles `@`
    }

    #[test]
    fn text_lines() {
        let mut out = String::new();
        line(
            &mut out,
            1,
            None,
            "NOTE",
            PayloadRef::Text("\na\n\nb"),
            Escaping::V70,
        );
        assert_eq!(out, "1 NOTE\n2 CONT a\n2 CONT\n2 CONT b\n");
        out.clear();
        line(
            &mut out,
            0,
            Some("@I1@"),
            "INDI",
            PayloadRef::None,
            Escaping::V70,
        );
        line(
            &mut out,
            1,
            None,
            "",
            PayloadRef::Text("bare line\n\n@x"),
            Escaping::V70,
        );
        assert_eq!(out, "0 @I1@ INDI\nbare line\n2 CONT\n2 CONT @@x\n");
    }
}
