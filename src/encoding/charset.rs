//! Single-byte character sets and ANSEL, both directions.
//!
//! Every byte of every table decodes to a character, so decoding never fails.
//! Encoding reports the characters a set cannot represent.

use super::{ansel_nfc, code_pages};

/// A single-byte character set: the lower half is ASCII in all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SingleByte {
    /// Windows-1252, also used to read ISO-8859-1 (a superset in practice).
    Cp1252,
    /// Windows-1250 (Central European).
    Cp1250,
    /// Windows-1251 (Cyrillic).
    Cp1251,
    /// ISO-8859-2 (Latin-2).
    Iso8859_2,
    /// ISO-8859-15 (Latin-9).
    Iso8859_15,
    /// IBM PC code page 437.
    Cp437,
    /// Mac OS Roman.
    MacRoman,
    /// ANSEL (ANSI/NISO Z39.47): combining marks precede their base.
    Ansel,
}

/// What one byte of a single-byte set stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decoded {
    /// A character that stands on its own.
    Char(char),
    /// An ANSEL non-spacing mark, which applies to the next character.
    Mark(char),
}

impl SingleByte {
    /// Decodes one byte.
    #[inline]
    pub(crate) fn decode(self, byte: u8) -> Decoded {
        if byte < 0x80 {
            return Decoded::Char(char::from(byte));
        }
        let upper = usize::from(byte & 0x7F);
        let table = match self {
            SingleByte::Cp1252 => &code_pages::CP1252,
            SingleByte::Cp1250 => &code_pages::CP1250,
            SingleByte::Cp1251 => &code_pages::CP1251,
            SingleByte::Iso8859_2 => &code_pages::ISO_8859_2,
            SingleByte::Iso8859_15 => &code_pages::ISO_8859_15,
            SingleByte::Cp437 => &code_pages::CP437,
            SingleByte::MacRoman => &code_pages::MAC_ROMAN,
            SingleByte::Ansel => {
                return match ansel_mark(byte) {
                    Some(mark) => Decoded::Mark(mark),
                    None => Decoded::Char(ansel_char(byte)),
                };
            }
        };
        // `upper` is below 128 by construction.
        Decoded::Char(
            table
                .get(upper)
                .copied()
                .unwrap_or(char::REPLACEMENT_CHARACTER),
        )
    }

    /// Encodes one character of a code page, or `None` when it has no byte.
    /// ANSEL goes through [`encode_ansel`] instead.
    pub(crate) fn encode(self, c: char) -> Option<u8> {
        if c.is_ascii() {
            return u8::try_from(u32::from(c)).ok();
        }
        let table = match self {
            SingleByte::Cp1252 => &code_pages::CP1252,
            SingleByte::Cp1250 => &code_pages::CP1250,
            SingleByte::Cp1251 => &code_pages::CP1251,
            SingleByte::Iso8859_2 => &code_pages::ISO_8859_2,
            SingleByte::Iso8859_15 => &code_pages::ISO_8859_15,
            SingleByte::Cp437 => &code_pages::CP437,
            SingleByte::MacRoman => &code_pages::MAC_ROMAN,
            SingleByte::Ansel => return None,
        };
        let i = table.iter().position(|&t| t == c)?;
        u8::try_from(i).ok().map(|i| i | 0x80)
    }
}

/// The ANSEL non-spacing mark of a byte (0xE0–0xFE), as its Unicode
/// combining character.
pub(crate) const fn ansel_mark(byte: u8) -> Option<char> {
    Some(match byte {
        0xE0 => '\u{0309}', // hook above
        0xE1 => '\u{0300}', // grave accent
        0xE2 => '\u{0301}', // acute accent
        0xE3 => '\u{0302}', // circumflex accent
        0xE4 => '\u{0303}', // tilde
        0xE5 => '\u{0304}', // macron
        0xE6 => '\u{0306}', // breve
        0xE7 => '\u{0307}', // dot above
        0xE8 => '\u{0308}', // umlaut (diaeresis)
        0xE9 => '\u{030C}', // hacek
        0xEA => '\u{030A}', // circle above (angstrom)
        0xEB => '\u{FE20}', // ligature, left half
        0xEC => '\u{FE21}', // ligature, right half
        0xED => '\u{0315}', // high comma, off center
        0xEE => '\u{030B}', // double acute accent
        0xEF => '\u{0310}', // candrabindu
        0xF0 => '\u{0327}', // cedilla
        0xF1 => '\u{0328}', // right hook, ogonek
        0xF2 => '\u{0323}', // dot below
        0xF3 => '\u{0324}', // double dot below
        0xF4 => '\u{0325}', // circle below
        0xF5 => '\u{0333}', // double underscore
        0xF6 => '\u{0332}', // underscore
        0xF7 => '\u{0326}', // left hook (comma below)
        0xF8 => '\u{031C}', // right cedilla
        0xF9 => '\u{032E}', // upadhmaniya (half circle below)
        0xFA => '\u{FE22}', // double tilde, left half
        0xFB => '\u{FE23}', // double tilde, right half
        0xFE => '\u{0313}', // high comma, centered
        _ => return None,
    })
}

/// The ANSEL spacing character of a byte at or above 0x80 that is not a mark.
///
/// Bytes ANSEL leaves undefined decode to the Latin-1 character of the same
/// value, so their value survives.
fn ansel_char(byte: u8) -> char {
    match byte {
        0x8D | 0xFC => '\u{200D}', // zero width joiner (MARC-8 0x8D; 0xFC in some producers)
        0x8E | 0xFD => '\u{200C}', // zero width non-joiner
        0xA1 => '\u{0141}',        // Ł
        0xA2 => '\u{00D8}',        // Ø
        0xA3 => '\u{0110}',        // Đ
        0xA4 => '\u{00DE}',        // Þ
        0xA5 => '\u{00C6}',        // Æ
        0xA6 => '\u{0152}',        // Œ
        0xA7 => '\u{02B9}',        // ʹ soft sign
        0xA8 => '\u{00B7}',        // · middle dot
        0xA9 => '\u{266D}',        // ♭ musical flat
        0xAA => '\u{00AE}',        // ® registered
        0xAB => '\u{00B1}',        // ± plus or minus
        0xAC => '\u{01A0}',        // Ơ
        0xAD => '\u{01AF}',        // Ư
        0xAE => '\u{02BC}',        // ʼ alif
        0xB0 => '\u{02BB}',        // ʻ ayn
        0xB1 => '\u{0142}',        // ł
        0xB2 => '\u{00F8}',        // ø
        0xB3 => '\u{0111}',        // đ
        0xB4 => '\u{00FE}',        // þ
        0xB5 => '\u{00E6}',        // æ
        0xB6 => '\u{0153}',        // œ
        0xB7 => '\u{02BA}',        // ʺ hard sign
        0xB8 => '\u{0131}',        // ı dotless i
        0xB9 => '\u{00A3}',        // £
        0xBA => '\u{00F0}',        // ð
        0xBC => '\u{01A1}',        // ơ
        0xBD => '\u{01B0}',        // ư
        0xBE => '\u{25A1}',        // □ empty box
        0xBF => '\u{25A0}',        // ■ black box
        0xC0 => '\u{00B0}',        // ° degree
        0xC1 => '\u{2113}',        // ℓ script small l
        0xC2 => '\u{2117}',        // ℗ phonogram copyright
        0xC3 => '\u{00A9}',        // © copyright
        0xC4 => '\u{266F}',        // ♯ musical sharp
        0xC5 => '\u{00BF}',        // ¿
        0xC6 => '\u{00A1}',        // ¡
        0xC7 | 0xCF => '\u{00DF}', // ß (0xCF in GEDCOM 5.5.1 Appendix C, 0xC7 in MARC-8)
        0xC8 => '\u{20AC}',        // €
        other => char::from(other),
    }
}

/// The ANSEL byte of a spacing character, if ANSEL has one.
fn ansel_byte(c: char) -> Option<u8> {
    if c.is_ascii() {
        return u8::try_from(u32::from(c)).ok();
    }
    Some(match c {
        '\u{0141}' => 0xA1,
        '\u{00D8}' => 0xA2,
        '\u{0110}' => 0xA3,
        '\u{00DE}' => 0xA4,
        '\u{00C6}' => 0xA5,
        '\u{0152}' => 0xA6,
        '\u{02B9}' => 0xA7,
        '\u{00B7}' => 0xA8,
        '\u{266D}' => 0xA9,
        '\u{00AE}' => 0xAA,
        '\u{00B1}' => 0xAB,
        '\u{01A0}' => 0xAC,
        '\u{01AF}' => 0xAD,
        '\u{02BC}' => 0xAE,
        '\u{02BB}' => 0xB0,
        '\u{0142}' => 0xB1,
        '\u{00F8}' => 0xB2,
        '\u{0111}' => 0xB3,
        '\u{00FE}' => 0xB4,
        '\u{00E6}' => 0xB5,
        '\u{0153}' => 0xB6,
        '\u{02BA}' => 0xB7,
        '\u{0131}' => 0xB8,
        '\u{00A3}' => 0xB9,
        '\u{00F0}' => 0xBA,
        '\u{01A1}' => 0xBC,
        '\u{01B0}' => 0xBD,
        '\u{25A1}' => 0xBE,
        '\u{25A0}' => 0xBF,
        '\u{00B0}' => 0xC0,
        '\u{2113}' => 0xC1,
        '\u{2117}' => 0xC2,
        '\u{00A9}' => 0xC3,
        '\u{266F}' => 0xC4,
        '\u{00BF}' => 0xC5,
        '\u{00A1}' => 0xC6,
        '\u{00DF}' => 0xCF,
        '\u{20AC}' => 0xC8,
        '\u{200D}' => 0x8D,
        '\u{200C}' => 0x8E,
        _ => return None,
    })
}

/// The ANSEL byte of a combining mark, if ANSEL has one.
fn ansel_mark_byte(mark: char) -> Option<u8> {
    (0xE0..=0xFE).find(|&b| ansel_mark(b) == Some(mark))
}

/// Encodes text to ANSEL.
///
/// Precomposed characters are decomposed with the generated canonical table
/// and written as their marks followed by the base, so ANSEL written by this
/// function decodes back to the same NFC text. A character ANSEL cannot
/// represent becomes `?`; `unencodable` counts them.
pub(crate) fn encode_ansel(text: &str, unencodable: &mut usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut marks: Vec<u8> = Vec::new();
    while let Some(c) = chars.next() {
        marks.clear();
        let mut base = c;
        // Peel the precomposed marks off, innermost last.
        while let Some((starter, mark)) = decompose(base) {
            match ansel_mark_byte(mark) {
                Some(b) => marks.insert(0, b),
                None => break,
            }
            base = starter;
        }
        // Marks that follow the character in the text apply to it as well.
        while let Some(&next) = chars.peek() {
            match ansel_mark_byte(next) {
                Some(b) => {
                    marks.push(b);
                    chars.next();
                }
                None => break,
            }
        }
        if let Some(b) = ansel_byte(base) {
            out.extend_from_slice(&marks);
            out.push(b);
        } else if ansel_mark_byte(base).is_some() {
            // A mark with no base before it: keep it on its own.
            out.extend_from_slice(&marks);
            out.extend(ansel_mark_byte(base));
        } else {
            *unencodable += 1;
            out.push(b'?');
        }
    }
    out
}

/// The canonical (starter, mark) pair of a composite reachable from ANSEL.
pub(crate) fn decompose(c: char) -> Option<(char, char)> {
    ansel_nfc::DECOMPOSE
        .binary_search_by_key(&c, |&(composite, _, _)| composite)
        .ok()
        .and_then(|i| ansel_nfc::DECOMPOSE.get(i))
        .map(|&(_, starter, mark)| (starter, mark))
}

/// The primary composite of a starter and a mark, if Unicode has one.
fn compose_pair(starter: char, mark: char) -> Option<char> {
    ansel_nfc::COMPOSE
        .binary_search_by(|&(s, m, _)| (s, m).cmp(&(starter, mark)))
        .ok()
        .and_then(|i| ansel_nfc::COMPOSE.get(i))
        .map(|&(_, _, composite)| composite)
}

/// Writes `base` followed by `marks` in Unicode Normalization Form C.
///
/// The base is decomposed, its marks and `marks` are put in canonical order
/// (a stable sort by combining class), and each mark that is not blocked is
/// composed into the starter. For every base in ANSEL's repertoire the result
/// equals `NFC(base + marks)`; the generator checks this against the Unicode
/// Character Database.
pub(crate) fn push_composed(out: &mut String, base: char, marks: &[char]) {
    // Decompose the base: composites reachable from ANSEL are at most a few
    // marks deep.
    let mut starter = base;
    let mut head = ['\0'; 4];
    let mut depth = 0;
    while let Some(slot) = head.get_mut(depth) {
        let Some((s, m)) = decompose(starter) else {
            break;
        };
        *slot = m;
        starter = s;
        depth += 1;
    }
    let head = &mut head[..depth];
    head.reverse();

    let mut inline = ['\0'; 16];
    let mut spill = Vec::new();
    let total = depth + marks.len();
    let all: &mut [char] = if total <= inline.len() {
        &mut inline[..total]
    } else {
        spill.resize(total, '\0');
        &mut spill
    };
    all[..depth].copy_from_slice(head);
    all[depth..].copy_from_slice(marks);
    // Canonical ordering: a stable sort by combining class.
    all.sort_by_key(|&m| ansel_nfc::ccc(m));

    // Marks that do not compose stay after the starter, in order; they are
    // compacted to the front of `all` as the loop goes (`kept <= i`).
    let mut last_class: Option<u8> = None;
    let mut kept = 0;
    for i in 0..all.len() {
        let m = all[i];
        let class = ansel_nfc::ccc(m);
        let blocked = last_class.is_some_and(|last| last >= class);
        if !blocked && let Some(composite) = compose_pair(starter, m) {
            starter = composite;
            continue;
        }
        all[kept] = m;
        kept += 1;
        last_class = Some(class);
    }
    out.push(starter);
    out.extend(&all[..kept]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composed(base: char, marks: &[char]) -> String {
        let mut s = String::new();
        push_composed(&mut s, base, marks);
        s
    }

    #[test]
    fn ansel_marks_have_a_class() {
        for b in 0xE0..=0xFE_u8 {
            if let Some(m) = ansel_mark(b) {
                assert!(ansel_nfc::ccc(m) > 0, "mark {m:?} of byte {b:#X}");
            }
        }
    }

    #[test]
    fn composes_single_and_stacked_marks() {
        assert_eq!(composed('e', &['\u{0301}']), "é");
        // Circumflex then acute on a: ấ (U+1EA5).
        assert_eq!(composed('a', &['\u{0302}', '\u{0301}']), "\u{1EA5}");
        // Dot below sorts before the circumflex: ậ (U+1EAD) either way.
        assert_eq!(composed('a', &['\u{0302}', '\u{0323}']), "\u{1EAD}");
        assert_eq!(composed('a', &['\u{0323}', '\u{0302}']), "\u{1EAD}");
        // ANSEL ơ with an acute: ớ (U+1EDB).
        assert_eq!(composed('\u{01A1}', &['\u{0301}']), "\u{1EDB}");
        // Ơ with an ogonek: NFC recomposes O + ogonek, the horn stays apart.
        assert_eq!(composed('\u{01A0}', &['\u{0328}']), "\u{01EA}\u{031B}");
        // No composite: the mark stays after its base.
        assert_eq!(composed('q', &['\u{0301}']), "q\u{0301}");
        // Two marks of one class: the second is blocked.
        assert_eq!(composed('e', &['\u{0301}', '\u{0301}']), "é\u{0301}");
    }

    #[test]
    fn ansel_round_trips_precomposed_text() {
        let text = "Łódź Ærø Nguyễn ấp Œuvre ß €";
        let mut lost = 0;
        let bytes = encode_ansel(text, &mut lost);
        assert_eq!(lost, 0);
        let mut decoded = String::new();
        let mut pending: Vec<char> = Vec::new();
        for &b in &bytes {
            match SingleByte::Ansel.decode(b) {
                Decoded::Mark(m) => pending.push(m),
                Decoded::Char(c) => {
                    push_composed(&mut decoded, c, &pending);
                    pending.clear();
                }
            }
        }
        assert_eq!(decoded, text);
    }

    #[test]
    fn code_pages_round_trip_their_upper_half() {
        for set in [
            SingleByte::Cp1252,
            SingleByte::Cp1250,
            SingleByte::Cp1251,
            SingleByte::Iso8859_2,
            SingleByte::Iso8859_15,
            SingleByte::Cp437,
            SingleByte::MacRoman,
        ] {
            for b in 0x80..=0xFF_u8 {
                let Decoded::Char(c) = set.decode(b) else {
                    panic!("{set:?} {b:#X} is a mark")
                };
                assert_eq!(set.encode(c), Some(b), "{set:?} {b:#X} {c:?}");
            }
        }
    }
}
