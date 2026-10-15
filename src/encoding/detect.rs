//! Choosing how to decode a byte stream, from evidence first.
//!
//! The steps run in this order and the first that applies wins:
//!
//! 1. a byte order mark selects UTF-8 or UTF-16 LE/BE;
//! 2. a NUL pattern in the first KiB selects UTF-16;
//! 3. bytes that read as UTF-8 (at least one multi-byte sequence, and more of
//!    them than invalid bytes) are UTF-8, whatever `HEAD.CHAR` says; the
//!    bytes looked at are the first [`SNIFF_WINDOW`] ones, in memory as in a
//!    stream, unless those are ASCII only;
//! 4. otherwise the `HEAD.CHAR` declaration selects the single-byte set, and
//!    anything unknown (or a declaration that the bytes contradict, such as
//!    UTF-8 or UNICODE on bytes that are neither) falls back to Windows-1252.
//!
//! Nothing here fails: every input gets a decoding.

use super::GedcomEncoding;
use super::charset::SingleByte;

/// How the decoder turns bytes into characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// UTF-8; a byte that is not part of a valid sequence is decoded with
    /// `fallback`, so stray bytes keep their value.
    Utf8 { fallback: SingleByte },
    /// One character per byte (ANSEL: marks before their base).
    Single(SingleByte),
    /// UTF-16 code units.
    Utf16 { big_endian: bool },
}

/// The outcome of sniffing a window of the input.
#[derive(Clone, Debug)]
pub(crate) struct Sniff {
    /// How to decode.
    pub mode: Mode,
    /// What to report.
    pub encoding: GedcomEncoding,
    /// The `HEAD.CHAR` payload, when the window's bytes are ASCII-compatible.
    pub declared: Option<String>,
    /// Length of the byte order mark to skip.
    pub bom: usize,
}

/// How much of the input the decoding is chosen from: a stream reads this
/// much before it decodes anything.
pub(crate) const SNIFF_WINDOW: usize = 64 * 1024;

/// Decides how to decode `window`, the start of a stream (`complete` when it
/// is the whole input).
pub(crate) fn sniff(window: &[u8], complete: bool) -> Sniff {
    sniff_with(window, complete, None)
}

/// Decides how to decode `input`, held whole in memory, as a stream of it
/// decides ([`sniff`] of its first [`SNIFF_WINDOW`] bytes), so that both
/// give the same text. Only an ASCII window, which tells nothing, lets the
/// rest of the input choose between UTF-8 and the declared set.
pub(crate) fn sniff_input(input: &[u8]) -> Sniff {
    let window = input.get(..SNIFF_WINDOW).unwrap_or(input);
    // A stream knows it has the whole input when it ends before the window
    // is full.
    sniff_with(window, window.len() < SNIFF_WINDOW, Some(input))
}

/// [`sniff`] of `window`; `input` is the whole input, when it is known.
fn sniff_with(window: &[u8], complete: bool, input: Option<&[u8]>) -> Sniff {
    if let Some(sniff) = by_bom(window) {
        return sniff;
    }
    if let Some(big_endian) = utf16_by_nul_pattern(window) {
        return Sniff {
            mode: Mode::Utf16 { big_endian },
            encoding: if big_endian {
                GedcomEncoding::Utf16Be
            } else {
                GedcomEncoding::Utf16Le
            },
            declared: None,
            bom: 0,
        };
    }
    let declared = declared_char(window).map(|v| String::from_utf8_lossy(v).into_owned());
    let declared_encoding = declared.as_deref().and_then(GedcomEncoding::from_label);
    let fallback = declared_encoding.map_or(SingleByte::Cp1252, single_byte_for);
    let (mut multibyte, mut invalid) = utf8_evidence(window, complete);
    if let (Some(input), (0, 0), false) = (input, (multibyte, invalid), complete) {
        // An ASCII window (but for a sequence its end cuts): in memory, the
        // bytes from the first that is not ASCII on decide (a stream cannot
        // see them).
        let start = window
            .iter()
            .position(|b| !b.is_ascii())
            .unwrap_or(window.len());
        (multibyte, invalid) = utf8_evidence(input.get(start..).unwrap_or_default(), true);
    }

    let (mode, encoding) = if multibyte > invalid {
        (Mode::Utf8 { fallback }, GedcomEncoding::Utf8)
    } else if invalid > 0 {
        let encoding = match declared_encoding {
            Some(
                e @ (GedcomEncoding::Ansel
                | GedcomEncoding::Iso8859_1
                | GedcomEncoding::Iso8859_15
                | GedcomEncoding::Windows1252
                | GedcomEncoding::Windows1250
                | GedcomEncoding::Windows1251
                | GedcomEncoding::Iso8859_2
                | GedcomEncoding::Cp437
                | GedcomEncoding::MacRoman),
            ) => e,
            _ => GedcomEncoding::Windows1252,
        };
        (Mode::Single(fallback), encoding)
    } else {
        // ASCII only: every ASCII-compatible declaration is true of it. When
        // the window is not the whole input, later bytes are decoded as UTF-8
        // where they read as UTF-8 and with the declared set otherwise.
        let encoding = match declared_encoding {
            Some(GedcomEncoding::Utf16Le | GedcomEncoding::Utf16Be) | None => GedcomEncoding::Ascii,
            Some(e) => e,
        };
        (Mode::Utf8 { fallback }, encoding)
    };
    Sniff {
        mode,
        encoding,
        declared,
        bom: 0,
    }
}

/// The single-byte set used for a declared encoding's bytes above 0x7F.
const fn single_byte_for(encoding: GedcomEncoding) -> SingleByte {
    match encoding {
        GedcomEncoding::Ansel => SingleByte::Ansel,
        GedcomEncoding::Iso8859_15 => SingleByte::Iso8859_15,
        GedcomEncoding::Windows1250 => SingleByte::Cp1250,
        GedcomEncoding::Windows1251 => SingleByte::Cp1251,
        GedcomEncoding::Iso8859_2 => SingleByte::Iso8859_2,
        GedcomEncoding::Cp437 => SingleByte::Cp437,
        GedcomEncoding::MacRoman => SingleByte::MacRoman,
        GedcomEncoding::Utf8
        | GedcomEncoding::Utf16Le
        | GedcomEncoding::Utf16Be
        | GedcomEncoding::Ascii
        | GedcomEncoding::Iso8859_1
        | GedcomEncoding::Windows1252 => SingleByte::Cp1252,
    }
}

/// The decoding mode that forces `encoding`.
pub(crate) const fn mode_for(encoding: GedcomEncoding) -> Mode {
    match encoding {
        GedcomEncoding::Utf16Le => Mode::Utf16 { big_endian: false },
        GedcomEncoding::Utf16Be => Mode::Utf16 { big_endian: true },
        GedcomEncoding::Utf8 | GedcomEncoding::Ascii => Mode::Utf8 {
            fallback: SingleByte::Cp1252,
        },
        other => Mode::Single(single_byte_for(other)),
    }
}

fn by_bom(window: &[u8]) -> Option<Sniff> {
    let (mode, encoding, bom) = if window.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let fallback = SingleByte::Cp1252;
        (Mode::Utf8 { fallback }, GedcomEncoding::Utf8, 3)
    } else if window.starts_with(&[0xFF, 0xFE]) {
        let mode = Mode::Utf16 { big_endian: false };
        (mode, GedcomEncoding::Utf16Le, 2)
    } else if window.starts_with(&[0xFE, 0xFF]) {
        let mode = Mode::Utf16 { big_endian: true };
        (mode, GedcomEncoding::Utf16Be, 2)
    } else {
        return None;
    };
    let rest = window.get(bom..).unwrap_or_default();
    Some(Sniff {
        mode,
        encoding,
        declared: if bom == 3 {
            declared_char(rest).map(|v| String::from_utf8_lossy(v).into_owned())
        } else {
            None
        },
        bom,
    })
}

/// UTF-16 without a BOM: GEDCOM text starts with ASCII lines, so one byte of
/// each unit is NUL. Looks at the first KiB; `Some(true)` for big endian.
fn utf16_by_nul_pattern(window: &[u8]) -> Option<bool> {
    let head = window.get(..window.len().min(1024)).unwrap_or_default();
    let (mut even, mut odd, mut pairs) = (0_usize, 0_usize, 0_usize);
    for [a, b] in head.as_chunks::<2>().0 {
        pairs += 1;
        even += usize::from(*a == 0);
        odd += usize::from(*b == 0);
    }
    if pairs < 2 {
        return None;
    }
    // At least half the units have the NUL on one side and almost none on the other.
    if odd * 2 >= pairs && even * 10 <= pairs {
        Some(false)
    } else if even * 2 >= pairs && odd * 10 <= pairs {
        Some(true)
    } else {
        None
    }
}

/// Counts the multi-byte UTF-8 sequences and the bytes that are not part of
/// any valid sequence. An incomplete sequence at the end of an incomplete
/// window is not counted as invalid.
fn utf8_evidence(window: &[u8], complete: bool) -> (usize, usize) {
    if let Ok(text) = std::str::from_utf8(window) {
        return (usize::from(!text.is_ascii()), 0);
    }
    let (mut multibyte, mut invalid) = (0, 0);
    let mut rest_len = window.len();
    for chunk in window.utf8_chunks() {
        let valid = chunk.valid();
        multibyte += valid.bytes().filter(|&b| b >= 0xC0).count();
        rest_len -= valid.len() + chunk.invalid().len();
        let at_end = rest_len == 0;
        if !(at_end && !complete && is_incomplete_sequence(chunk.invalid())) {
            invalid += chunk.invalid().len();
        }
    }
    (multibyte, invalid)
}

/// Whether `bytes` is a proper prefix of a valid UTF-8 sequence.
pub(crate) fn is_incomplete_sequence(bytes: &[u8]) -> bool {
    let Some((&lead, rest)) = bytes.split_first() else {
        return false;
    };
    let width = utf8_width(lead);
    width > 1 && bytes.len() < width && rest.iter().enumerate().all(|(i, &b)| continues(lead, i, b))
}

/// The length of the sequence a lead byte starts (0 for a byte that cannot lead).
pub(crate) const fn utf8_width(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 0,
    }
}

/// Whether `byte` may follow `lead` at position `i` (0-based, after the lead).
pub(crate) const fn continues(lead: u8, i: usize, byte: u8) -> bool {
    match (lead, i) {
        (0xE0, 0) => matches!(byte, 0xA0..=0xBF),
        (0xED, 0) => matches!(byte, 0x80..=0x9F),
        (0xF0, 0) => matches!(byte, 0x90..=0xBF),
        (0xF4, 0) => matches!(byte, 0x80..=0x8F),
        _ => matches!(byte, 0x80..=0xBF),
    }
}

/// The payload of `HEAD.CHAR`, read from the bytes of the `HEAD` record only.
///
/// Any line terminator counts, the tag is matched case-insensitively and
/// nothing is copied. Returns `None` when the input does not start with a
/// `HEAD` record or the record has no `CHAR` line.
pub(crate) fn declared_char(bytes: &[u8]) -> Option<&[u8]> {
    let mut lines = bytes
        .split(|&b| b == b'\n' || b == b'\r')
        .filter_map(split_line);
    let (level, tag, _) = lines.next()?;
    if level != 0 || !tag.eq_ignore_ascii_case(b"HEAD") {
        return None;
    }
    for (level, tag, payload) in lines {
        if level == 0 {
            break;
        }
        if level == 1 && tag.eq_ignore_ascii_case(b"CHAR") {
            return Some(payload.trim_ascii());
        }
    }
    None
}

/// Splits a raw line into level, tag and payload; `None` for a blank line or
/// a line without a level.
fn split_line(line: &[u8]) -> Option<(u32, &[u8], &[u8])> {
    let line = trim_start(line);
    let digits = line.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let level = line.get(..digits)?.iter().fold(0_u32, |acc, &d| {
        acc.saturating_mul(10).saturating_add(u32::from(d - b'0'))
    });
    let mut rest = trim_start(line.get(digits..)?);
    if rest.first() == Some(&b'@') {
        let end = rest.iter().position(|&b| b == b' ' || b == b'\t')?;
        rest = trim_start(rest.get(end..)?);
    }
    let end = rest
        .iter()
        .position(|&b| b == b' ' || b == b'\t')
        .unwrap_or(rest.len());
    let tag = rest.get(..end)?;
    let payload = rest.get(end + 1..).unwrap_or_default();
    Some((level, tag, payload))
}

fn trim_start(bytes: &[u8]) -> &[u8] {
    let n = bytes
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    bytes.get(n..).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_is_found_anywhere_in_head_with_any_terminator() {
        let head = b"0 HEAD\r1 SOUR x\r2 VERS 1\r1 char  ansel \r0 TRLR\r";
        assert_eq!(declared_char(head), Some(&b"ansel"[..]));
        let not_in_head = b"0 HEAD\n0 @I1@ INDI\n1 CHAR ANSEL\n";
        assert_eq!(declared_char(not_in_head), None);
        assert_eq!(declared_char(b"1 CHAR ANSEL\n"), None);
    }

    #[test]
    fn utf16_without_bom_is_found_by_its_nuls() {
        let le: Vec<u8> = "0 HEAD\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let be: Vec<u8> = "0 HEAD\n"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        assert_eq!(utf16_by_nul_pattern(&le), Some(false));
        assert_eq!(utf16_by_nul_pattern(&be), Some(true));
        assert_eq!(utf16_by_nul_pattern(b"0 HEAD\n"), None);
    }

    #[test]
    fn utf8_evidence_beats_the_declaration() {
        let s = sniff("0 HEAD\n1 CHAR ANSEL\n1 NOTE é\n".as_bytes(), true);
        assert_eq!(s.encoding, GedcomEncoding::Utf8);
        let s = sniff(b"0 HEAD\n1 CHAR ANSI\n1 NOTE \xE9\n", true);
        assert_eq!(s.encoding, GedcomEncoding::Windows1252);
        assert_eq!(s.mode, Mode::Single(SingleByte::Cp1252));
        let s = sniff(b"0 HEAD\n1 CHAR UTF-8\n1 NOTE \xE9\n", true);
        assert_eq!(s.encoding, GedcomEncoding::Windows1252);
    }

    #[test]
    fn an_incomplete_window_ends_in_the_middle_of_a_sequence() {
        let s = sniff("0 HEAD\n1 NOTE é".as_bytes().split_last().unwrap().1, false);
        assert_eq!(s.encoding, GedcomEncoding::Ascii);
        assert_eq!(
            s.mode,
            Mode::Utf8 {
                fallback: SingleByte::Cp1252
            }
        );
        assert!(is_incomplete_sequence(&[0xE2, 0x82]));
        assert!(!is_incomplete_sequence(&[0xE0, 0x80]));
        assert!(!is_incomplete_sequence(&[0x80]));
    }
}
