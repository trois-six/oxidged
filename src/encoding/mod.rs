//! Character encodings: a decoder that never fails, and the encoder.
//!
//! GEDCOM 7.0 is always UTF-8. GEDCOM 5.5.1 declares its character set in
//! `HEAD.CHAR` (`ANSEL`, `UTF-8`, `UNICODE`, `ASCII`), and real files also
//! declare `ANSI`, `WINDOWS-1252`, `ISO-8859-1`, `ISO-8859-15`, `IBMPC` or
//! `MACINTOSH`, or declare one set and are written in another.
//!
//! [`decode`] trusts evidence over the declaration and never fails. The steps
//! run in this order; the first that applies wins:
//!
//! 1. a byte order mark selects UTF-8 or UTF-16 LE/BE;
//! 2. a NUL byte in one half of most of the first KiB's code units selects
//!    UTF-16 without a byte order mark;
//! 3. bytes that read as UTF-8 (multi-byte sequences outnumber the bytes that
//!    are not part of one, in the first 64 KiB that are not all ASCII) are
//!    UTF-8, whatever `CHAR` says — real ANSEL and
//!    code-page text practically never forms valid UTF-8 sequences; any
//!    stray byte is decoded with the declared single-byte set;
//! 4. otherwise the declared set applies: ANSEL (with its marks composed to
//!    Unicode NFC), Windows-1252 for `ANSI`, `WINDOWS-1252` and `CP1252`,
//!    Windows-1252 as well for `ISO-8859-1`/`LATIN1` (a superset that only
//!    differs on C1 control codes, which text files do not use), ISO-8859-15,
//!    code page 437 for `IBMPC` and Mac OS Roman for `MACINTOSH`;
//! 5. anything else falls back to Windows-1252.
//!
//! `CHAR` is read from the bytes of the `HEAD` record only, whatever its
//! length and line terminators, without copying the input.
//!
//! [`DecodeReader`] applies the same rules to a stream: it sniffs the first
//! 64 KiB (or the whole input if shorter), then decodes chunk by chunk.
//! [`decode`] chooses from the same window, so that both give the same text.
//! When the window is ASCII only, nothing can be decided yet, so the stream
//! decodes each valid UTF-8 sequence as UTF-8 and each other byte with the
//! declared set, while in-memory decoding lets the rest of the input choose;
//! the two can only differ on a file whose first non-ASCII byte lies beyond
//! the window and whose declared single-byte text happens to form valid UTF-8
//! sequences.
//!
//! # Example
//!
//! ```rust
//! use oxidged::encoding::{decode, GedcomEncoding};
//!
//! let decoded = decode(b"0 HEAD\n1 CHAR ANSI\n0 @I1@ INDI\n1 NAME Ren\xE9e\n0 TRLR\n");
//! assert_eq!(decoded.encoding, GedcomEncoding::Windows1252);
//! assert_eq!(decoded.declared.as_deref(), Some("ANSI"));
//! assert!(decoded.text.contains("Renée"));
//! ```

mod ansel_nfc;
mod charset;
mod code_pages;
mod decoder;
mod detect;
mod reader;

pub use reader::DecodeReader;

pub(crate) use charset::encode_ansel;
pub(crate) use decoder::Decoder;
pub(crate) use detect::{Mode, mode_for, sniff_input};

use charset::SingleByte;

/// A character encoding of a GEDCOM file.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GedcomEncoding {
    /// UTF-8, the only encoding of GEDCOM 7.0 (`CHAR UTF-8`).
    Utf8,
    /// UTF-16 little endian (`CHAR UNICODE`).
    Utf16Le,
    /// UTF-16 big endian (`CHAR UNICODE`).
    Utf16Be,
    /// 7-bit ASCII (`CHAR ASCII`).
    Ascii,
    /// ANSEL, ANSI/NISO Z39.47 (`CHAR ANSEL`), the GEDCOM 5.5.1 default.
    Ansel,
    /// ISO-8859-1, Latin-1 (`CHAR ISO-8859-1`, `LATIN1`); decoded as
    /// Windows-1252, its superset in practice.
    Iso8859_1,
    /// ISO-8859-15, Latin-9 (`CHAR ISO-8859-15`, `LATIN9`).
    Iso8859_15,
    /// Windows-1252 (`CHAR ANSI`, `WINDOWS-1252`, `CP1252`).
    Windows1252,
    /// Windows-1250, Central European (`CHAR WINDOWS-1250`, `CP1250`); also
    /// what byte statistics find under some `CHAR ANSI` files
    /// ([`crate::profile`]).
    Windows1250,
    /// Windows-1251, Cyrillic (`CHAR WINDOWS-1251`, `CP1251`).
    Windows1251,
    /// ISO-8859-2, Latin-2 (`CHAR ISO-8859-2`, `LATIN2`).
    Iso8859_2,
    /// IBM PC code page 437 (`CHAR IBMPC`, `CP437`).
    Cp437,
    /// Mac OS Roman (`CHAR MACINTOSH`, `MACROMAN`).
    MacRoman,
}

impl GedcomEncoding {
    /// Reads a `HEAD.CHAR` value. Case, hyphens, underscores and spaces are
    /// ignored; `None` for a value no producer is known to use.
    ///
    /// ```rust
    /// use oxidged::GedcomEncoding;
    ///
    /// assert_eq!(GedcomEncoding::from_label("ANSI"), Some(GedcomEncoding::Windows1252));
    /// assert_eq!(GedcomEncoding::from_label("iso-8859-1"), Some(GedcomEncoding::Iso8859_1));
    /// assert_eq!(GedcomEncoding::from_label("EBCDIC"), None);
    /// ```
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        let mut key = [0_u8; 16];
        let mut len = 0;
        for b in label
            .trim()
            .bytes()
            .filter(|b| !matches!(b, b'-' | b'_' | b' '))
        {
            *key.get_mut(len)? = b.to_ascii_uppercase();
            len += 1;
        }
        Some(match key.get(..len)? {
            b"UTF8" => Self::Utf8,
            b"UNICODE" | b"UTF16" | b"UTF16LE" | b"UCS2" => Self::Utf16Le,
            b"UTF16BE" => Self::Utf16Be,
            b"ASCII" | b"USASCII" => Self::Ascii,
            b"ANSEL" | b"MARC8" => Self::Ansel,
            b"ISO88591" | b"LATIN1" | b"ISOLATIN1" | b"ISO8859" => Self::Iso8859_1,
            b"ISO885915" | b"LATIN9" => Self::Iso8859_15,
            b"ANSI" | b"WINDOWS1252" | b"CP1252" | b"WIN1252" | b"MSANSI" => Self::Windows1252,
            b"WINDOWS1250" | b"CP1250" | b"WIN1250" => Self::Windows1250,
            b"WINDOWS1251" | b"CP1251" | b"WIN1251" => Self::Windows1251,
            b"ISO88592" | b"LATIN2" | b"ISOLATIN2" => Self::Iso8859_2,
            b"IBMPC" | b"CP437" | b"IBM437" | b"DOS" | b"IBM" | b"OEM" => Self::Cp437,
            b"MACINTOSH" | b"MACROMAN" | b"MAC" => Self::MacRoman,
            _ => return None,
        })
    }
}

impl std::fmt::Display for GedcomEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            GedcomEncoding::Utf8 => "UTF-8",
            GedcomEncoding::Utf16Le => "UTF-16LE",
            GedcomEncoding::Utf16Be => "UTF-16BE",
            GedcomEncoding::Ascii => "ASCII",
            GedcomEncoding::Ansel => "ANSEL",
            GedcomEncoding::Iso8859_1 => "ISO-8859-1",
            GedcomEncoding::Iso8859_15 => "ISO-8859-15",
            GedcomEncoding::Windows1252 => "WINDOWS-1252",
            GedcomEncoding::Windows1250 => "WINDOWS-1250",
            GedcomEncoding::Windows1251 => "WINDOWS-1251",
            GedcomEncoding::Iso8859_2 => "ISO-8859-2",
            GedcomEncoding::Cp437 => "IBMPC",
            GedcomEncoding::MacRoman => "MACINTOSH",
        })
    }
}

/// Text decoded from bytes, with what was found about its encoding.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    /// The text, without a byte order mark. Line terminators are kept as
    /// they were.
    pub text: String,
    /// The encoding the bytes were decoded with.
    pub encoding: GedcomEncoding,
    /// The `HEAD.CHAR` payload as written, if any. It may contradict
    /// `encoding`: evidence wins.
    pub declared: Option<String>,
}

/// Decodes GEDCOM bytes to text. Never fails: see the [module
/// documentation](self) for how the encoding is chosen.
#[must_use]
pub fn decode(bytes: &[u8]) -> Decoded {
    let sniff = sniff_input(bytes);
    let body = bytes.get(sniff.bom..).unwrap_or_default();
    let text = match sniff.mode {
        Mode::Utf8 { .. } => {
            std::str::from_utf8(body).map_or_else(|_| decode_body(body, sniff.mode), str::to_owned)
        }
        mode => decode_body(body, mode),
    };
    decoded(text, sniff)
}

/// Decodes GEDCOM bytes it owns: like [`decode`], but UTF-8 input becomes
/// the text without a copy, which halves the peak memory of reading a large
/// UTF-8 file.
#[must_use]
pub fn decode_owned(mut bytes: Vec<u8>) -> Decoded {
    let sniff = sniff_input(&bytes);
    let text = match sniff.mode {
        Mode::Utf8 { .. } => {
            bytes.drain(..sniff.bom.min(bytes.len()));
            String::from_utf8(bytes).unwrap_or_else(|e| decode_body(e.as_bytes(), sniff.mode))
        }
        mode => decode_body(bytes.get(sniff.bom..).unwrap_or_default(), mode),
    };
    decoded(text, sniff)
}

fn decoded(text: String, sniff: detect::Sniff) -> Decoded {
    let declared = sniff.declared.or_else(|| {
        matches!(sniff.mode, Mode::Utf16 { .. })
            .then(|| {
                detect::declared_char(text.as_bytes())
                    .map(|v| String::from_utf8_lossy(v).into_owned())
            })
            .flatten()
    });
    Decoded {
        text,
        encoding: sniff.encoding,
        declared,
    }
}

/// What a reader asks for single-byte bytes: given the `HEAD.CHAR` payload
/// and the bytes, the encoding to decode them with, if not the default.
pub(crate) type ChooseEncoding<'a> = dyn Fn(Option<&str>, &[u8]) -> Option<GedcomEncoding> + 'a;

/// Decodes GEDCOM bytes it owns as [`decode_owned`] does, except that when
/// the bytes are neither UTF-8 nor UTF-16 — when the declared single-byte
/// set would apply — `choose` is asked first, with the `HEAD.CHAR` payload
/// and the bytes after any byte order mark: the encoding it names (a
/// single-byte one) is used instead. This is how a producer profile reads
/// `CHAR ANSI` as its producer meant it ([`crate::profile`]).
pub(crate) fn decode_owned_with(bytes: Vec<u8>, choose: &ChooseEncoding<'_>) -> Decoded {
    let sniff = sniff_input(&bytes);
    if matches!(sniff.mode, Mode::Single(_)) {
        let body = bytes.get(sniff.bom..).unwrap_or_default();
        if let Some(encoding) = choose(sniff.declared.as_deref(), body).filter(|e| {
            !matches!(
                e,
                GedcomEncoding::Utf8 | GedcomEncoding::Utf16Le | GedcomEncoding::Utf16Be
            )
        }) {
            return Decoded {
                text: decode_body(body, mode_for(encoding)),
                encoding,
                declared: sniff.declared,
            };
        }
    }
    decode_owned(bytes)
}

/// Whether single-byte text reads better as Windows-1250 (Central European)
/// than as Windows-1252: some producers label either `ANSI`. Only the bytes
/// that are a letter in one code page and a symbol, a control or nothing in
/// the other decide, and only when they stand inside a word; Windows-1250
/// must win by at least two such bytes and be clearly ahead.
#[must_use]
pub(crate) fn reads_as_cp1250(body: &[u8]) -> bool {
    let (mut cp1250, mut cp1252) = (0_usize, 0_usize);
    let letter = |b: u8, set: SingleByte| match set.decode(b) {
        charset::Decoded::Char(c) => c.is_alphabetic(),
        charset::Decoded::Mark(_) => false,
    };
    for (i, &b) in body.iter().enumerate() {
        if b < 0x80 {
            continue;
        }
        let (in_1250, in_1252) = (letter(b, SingleByte::Cp1250), letter(b, SingleByte::Cp1252));
        if in_1250 == in_1252 {
            continue;
        }
        let near = |j: Option<usize>| {
            j.and_then(|j| body.get(j))
                .is_some_and(|&n| n.is_ascii_alphabetic() || n >= 0x80)
        };
        if !(near(i.checked_sub(1)) || near(i.checked_add(1))) {
            continue;
        }
        if in_1250 {
            cp1250 += 1;
        } else {
            cp1252 += 1;
        }
    }
    cp1250 >= 2 && cp1250 > cp1252.saturating_mul(4)
}

/// Decodes bytes with a given encoding, skipping its byte order mark.
///
/// Never fails: a byte that is not valid in `encoding` is decoded as
/// Windows-1252 (UTF-8, ASCII) or as U+FFFD (UTF-16, which has no byte-wise
/// fallback).
#[must_use]
pub fn decode_as(bytes: &[u8], encoding: GedcomEncoding) -> String {
    let bom: &[u8] = match encoding {
        GedcomEncoding::Utf8 | GedcomEncoding::Ascii => &[0xEF, 0xBB, 0xBF],
        GedcomEncoding::Utf16Le => &[0xFF, 0xFE],
        GedcomEncoding::Utf16Be => &[0xFE, 0xFF],
        _ => &[],
    };
    let body = bytes.strip_prefix(bom).unwrap_or(bytes);
    decode_body(body, mode_for(encoding))
}

/// Detects the encoding of GEDCOM bytes: the encoding [`decode`] reports.
#[must_use]
pub fn detect_encoding(bytes: &[u8]) -> GedcomEncoding {
    sniff_input(bytes).encoding
}

fn decode_body(body: &[u8], mode: Mode) -> String {
    let mut decoder = Decoder::new(mode);
    let mut out = String::with_capacity(body.len() + body.len() / 8);
    decoder.decode(body, &mut out);
    decoder.finish(&mut out);
    out
}

/// A character an encoding cannot represent: why [`encode`] failed.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncodeError {
    /// The encoding.
    pub encoding: GedcomEncoding,
    /// The first character it cannot represent.
    pub character: char,
    /// Its byte offset in the text.
    pub offset: usize,
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} cannot represent {:?} (U+{:04X}) at byte {}",
            self.encoding,
            self.character,
            u32::from(self.character),
            self.offset
        )
    }
}

impl std::error::Error for EncodeError {}

/// Encodes text to bytes in `encoding`: the inverse of [`decode_as`].
/// UTF-16 output starts with a byte order mark; ANSEL output writes
/// precomposed letters as a mark and a base.
///
/// ```rust
/// use oxidged::encoding::{decode_as, encode, GedcomEncoding};
///
/// let bytes = encode("Renée", GedcomEncoding::Ansel).unwrap();
/// assert_eq!(decode_as(&bytes, GedcomEncoding::Ansel), "Renée");
/// let error = encode("Zoë", GedcomEncoding::Ascii).unwrap_err();
/// assert_eq!((error.character, error.offset), ('ë', 2));
/// ```
///
/// # Errors
///
/// [`EncodeError`] when the text holds a character the encoding cannot
/// represent (ASCII, ANSEL and the single-byte sets).
pub fn encode(text: &str, encoding: GedcomEncoding) -> Result<Vec<u8>, EncodeError> {
    let unencodable = |offset: usize, character: char| EncodeError {
        encoding,
        character,
        offset,
    };
    let single = match encoding {
        GedcomEncoding::Utf8 => return Ok(text.as_bytes().to_vec()),
        GedcomEncoding::Utf16Le | GedcomEncoding::Utf16Be => {
            let big_endian = encoding == GedcomEncoding::Utf16Be;
            let mut bytes = Vec::with_capacity(2 + text.len() * 2);
            for unit in std::iter::once(0xFEFF).chain(text.encode_utf16()) {
                bytes.extend_from_slice(&if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
            return Ok(bytes);
        }
        GedcomEncoding::Ascii => {
            return match text.char_indices().find(|(_, c)| !c.is_ascii()) {
                None => Ok(text.as_bytes().to_vec()),
                Some((offset, c)) => Err(unencodable(offset, c)),
            };
        }
        GedcomEncoding::Ansel => {
            let mut lost = 0;
            let bytes = charset::encode_ansel(text, &mut lost);
            if lost == 0 {
                return Ok(bytes);
            }
            // The first character ANSEL cannot hold, decomposed or not.
            let (offset, c) = text
                .char_indices()
                .find(|(_, c)| {
                    let mut lost = 0;
                    charset::encode_ansel(c.encode_utf8(&mut [0; 4]), &mut lost);
                    lost > 0
                })
                .unwrap_or((0, char::REPLACEMENT_CHARACTER));
            return Err(unencodable(offset, c));
        }
        GedcomEncoding::Iso8859_1 | GedcomEncoding::Windows1252 => SingleByte::Cp1252,
        GedcomEncoding::Iso8859_15 => SingleByte::Iso8859_15,
        GedcomEncoding::Windows1250 => SingleByte::Cp1250,
        GedcomEncoding::Windows1251 => SingleByte::Cp1251,
        GedcomEncoding::Iso8859_2 => SingleByte::Iso8859_2,
        GedcomEncoding::Cp437 => SingleByte::Cp437,
        GedcomEncoding::MacRoman => SingleByte::MacRoman,
    };
    text.char_indices()
        .map(|(offset, c)| single.encode(c).ok_or_else(|| unencodable(offset, c)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str, big_endian: bool, bom: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        if bom {
            bytes.extend_from_slice(if big_endian {
                &[0xFE, 0xFF]
            } else {
                &[0xFF, 0xFE]
            });
        }
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&if big_endian {
                unit.to_be_bytes()
            } else {
                unit.to_le_bytes()
            });
        }
        bytes
    }

    #[test]
    fn owned_decoding_matches() {
        let inputs: [&[u8]; 4] = [
            b"\xEF\xBB\xBF0 HEAD\n1 NOTE \xC3\xA9\n",
            b"0 HEAD\n1 CHAR ANSEL\n1 NOTE \xE2e\n",
            b"\xFF\xFE0\x00",
            b"",
        ];
        for input in inputs {
            assert_eq!(decode_owned(input.to_vec()), decode(input));
        }
    }

    #[test]
    fn byte_order_marks_decide() {
        let d = decode(b"\xEF\xBB\xBF0 HEAD\n");
        assert_eq!(
            (d.encoding, d.text.as_str()),
            (GedcomEncoding::Utf8, "0 HEAD\n")
        );
        let d = decode(&utf16("0 HEAD\n1 CHAR UNICODE\n", false, true));
        assert_eq!(d.encoding, GedcomEncoding::Utf16Le);
        assert_eq!(d.declared.as_deref(), Some("UNICODE"));
        let d = decode(&utf16("0 HEAD\n", true, true));
        assert_eq!(
            (d.encoding, d.text.as_str()),
            (GedcomEncoding::Utf16Be, "0 HEAD\n")
        );
    }

    #[test]
    fn utf16_without_bom_is_decoded() {
        let text = "0 HEAD\n1 CHAR UNICODE\n0 @I1@ INDI\n1 NAME Zoë /Example/\n0 TRLR\n";
        let d = decode(&utf16(text, false, false));
        assert_eq!(
            (d.encoding, d.text.as_str()),
            (GedcomEncoding::Utf16Le, text)
        );
        let d = decode(&utf16(text, true, false));
        assert_eq!(
            (d.encoding, d.text.as_str()),
            (GedcomEncoding::Utf16Be, text)
        );
    }

    #[test]
    fn declarations_select_the_single_byte_set() {
        let cases: [(&[u8], GedcomEncoding, &str); 7] = [
            (
                b"0 HEAD\n1 CHAR ANSI\n1 NOTE 5\x80 \x93q\x94\n",
                GedcomEncoding::Windows1252,
                "5€ “q”",
            ),
            (
                b"0 HEAD\n1 CHAR ASCII\n1 NOTE caf\xE9\n",
                GedcomEncoding::Windows1252,
                "café",
            ),
            (
                b"0 HEAD\n1 CHAR LATIN1\n1 NOTE caf\xE9\n",
                GedcomEncoding::Iso8859_1,
                "café",
            ),
            (
                b"0 HEAD\n1 CHAR ISO-8859-15\n1 NOTE 10\xA4\n",
                GedcomEncoding::Iso8859_15,
                "10€",
            ),
            (
                b"0 HEAD\n1 CHAR IBMPC\n1 NOTE \x82t\x82\n",
                GedcomEncoding::Cp437,
                "été",
            ),
            (
                b"0 HEAD\n1 CHAR MACINTOSH\n1 NOTE \x8Et\x8E\n",
                GedcomEncoding::MacRoman,
                "été",
            ),
            (
                b"0 HEAD\n1 CHAR ANSEL\n1 NOTE \xE2et\xE2e\n",
                GedcomEncoding::Ansel,
                "été",
            ),
        ];
        for (bytes, encoding, note) in cases {
            let d = decode(bytes);
            assert_eq!(d.encoding, encoding, "{note}");
            assert!(
                d.text.ends_with(&format!("1 NOTE {note}\n")),
                "{:?}",
                d.text
            );
        }
    }

    #[test]
    fn valid_utf8_wins_over_the_declaration() {
        for label in ["ANSEL", "ANSI", "UNICODE", "ASCII"] {
            let text = format!("0 HEAD\n1 CHAR {label}\n1 NOTE Renée\n");
            let d = decode(text.as_bytes());
            assert_eq!(
                (d.encoding, d.text.as_str()),
                (GedcomEncoding::Utf8, text.as_str())
            );
        }
    }

    #[test]
    fn ascii_reports_the_declared_set() {
        assert_eq!(detect_encoding(b"0 HEAD\n0 TRLR\n"), GedcomEncoding::Ascii);
        assert_eq!(
            detect_encoding(b"0 HEAD\n1 CHAR UTF-8\n"),
            GedcomEncoding::Utf8
        );
        assert_eq!(
            detect_encoding(b"0 HEAD\n1 CHAR ANSEL\n"),
            GedcomEncoding::Ansel
        );
    }

    #[test]
    fn decode_as_forces_the_set() {
        assert_eq!(decode_as(b"Jos\xE9", GedcomEncoding::Iso8859_1), "José");
        assert_eq!(decode_as(b"Jos\xE9", GedcomEncoding::Utf8), "José");
        assert_eq!(
            decode_as(&utf16("Zoë", false, true), GedcomEncoding::Utf16Le),
            "Zoë"
        );
    }

    #[test]
    fn encoding_round_trips() {
        let text = "0 HEAD\n1 NAME Zoë /Exämple/\n";
        for encoding in [
            GedcomEncoding::Utf8,
            GedcomEncoding::Utf16Le,
            GedcomEncoding::Utf16Be,
            GedcomEncoding::Iso8859_1,
            GedcomEncoding::Iso8859_15,
            GedcomEncoding::Windows1252,
            GedcomEncoding::Windows1250,
            GedcomEncoding::Iso8859_2,
            GedcomEncoding::Cp437,
            GedcomEncoding::MacRoman,
            GedcomEncoding::Ansel,
        ] {
            let bytes = encode(text, encoding).unwrap();
            assert_eq!(decode_as(&bytes, encoding), text, "{encoding}");
        }
        let cyrillic = "0 HEAD\n1 NAME \u{418}\u{432}\u{430}\u{43d} /\u{41f}\u{440}\u{438}\u{43c}\u{435}\u{440}/\n";
        let bytes = encode(cyrillic, GedcomEncoding::Windows1251).unwrap();
        assert_eq!(decode_as(&bytes, GedcomEncoding::Windows1251), cyrillic);
        assert!(encode("Zoë", GedcomEncoding::Ascii).is_err());
        assert!(encode("中", GedcomEncoding::Ansel).is_err());
    }

    #[test]
    fn labels() {
        for (label, encoding) in [
            ("UTF-8", GedcomEncoding::Utf8),
            ("unicode", GedcomEncoding::Utf16Le),
            ("Windows-1252", GedcomEncoding::Windows1252),
            ("windows-1250", GedcomEncoding::Windows1250),
            ("IBM PC", GedcomEncoding::Cp437),
            ("Macintosh", GedcomEncoding::MacRoman),
            ("Latin_9", GedcomEncoding::Iso8859_15),
        ] {
            assert_eq!(GedcomEncoding::from_label(label), Some(encoding), "{label}");
        }
        assert_eq!(
            GedcomEncoding::from_label("A-VERY-LONG-UNKNOWN-LABEL"),
            None
        );
        assert_eq!(GedcomEncoding::Cp437.to_string(), "IBMPC");
    }
}
