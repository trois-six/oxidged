//! The incremental decoder shared by in-memory decoding and [`DecodeReader`].
//!
//! Bytes go in as chunks of any size; text comes out. Partial UTF-8
//! sequences, an odd UTF-16 byte, a pending high surrogate and pending ANSEL
//! marks are carried from one chunk to the next, so the output does not
//! depend on where the chunks are cut.
//!
//! [`DecodeReader`]: super::DecodeReader

use super::charset::{Decoded, SingleByte, push_composed};
use super::detect::{Mode, continues, is_incomplete_sequence, utf8_width};

/// Decodes a byte stream in the given [`Mode`].
#[derive(Debug)]
pub(crate) struct Decoder {
    mode: Mode,
    /// Bytes of a sequence cut by the end of the previous chunk: a partial
    /// UTF-8 sequence, or the first byte of a UTF-16 unit.
    carry: [u8; 4],
    carry_len: usize,
    /// A high surrogate waiting for its low half.
    high_surrogate: Option<u16>,
    marks: Marks,
}

impl Decoder {
    pub(crate) fn new(mode: Mode) -> Self {
        Self {
            mode,
            carry: [0; 4],
            carry_len: 0,
            high_surrogate: None,
            marks: Marks::default(),
        }
    }

    /// Decodes the next chunk of input, appending to `out`.
    pub(crate) fn decode(&mut self, input: &[u8], out: &mut String) {
        match self.mode {
            Mode::Utf8 { fallback } => self.decode_utf8(input, fallback, out),
            Mode::Single(set) => self.decode_single(input, set, out),
            Mode::Utf16 { big_endian } => self.decode_utf16(input, big_endian, out),
        }
    }

    /// Flushes what the end of the input completes: a cut sequence decodes
    /// byte by byte, pending marks are written on their own.
    pub(crate) fn finish(&mut self, out: &mut String) {
        match self.mode {
            Mode::Utf8 { fallback } => {
                let carry = self.carry;
                for &b in carry.get(..self.carry_len).unwrap_or_default() {
                    self.push_fallback(fallback, b, out);
                }
            }
            Mode::Single(_) => {}
            Mode::Utf16 { .. } => {
                if self.high_surrogate.take().is_some() || self.carry_len > 0 {
                    self.marks.push(out, char::REPLACEMENT_CHARACTER);
                }
            }
        }
        self.carry_len = 0;
        self.marks.finish(out);
    }

    /// The length of `out` that later input can no longer change. Text past
    /// it is held back while an ANSEL mark at the end of a line waits to see
    /// whether the next line is a `CONC`.
    pub(crate) fn stable_len(&self, out: &str) -> usize {
        self.marks.hold.as_ref().map_or(out.len(), |h| h.insert_at)
    }

    /// Tells the decoder that the first `n` bytes of its output were removed
    /// (`n` is at most [`stable_len`](Self::stable_len)).
    pub(crate) fn drained(&mut self, n: usize) {
        if let Some(hold) = self.marks.hold.as_mut() {
            hold.insert_at = hold.insert_at.saturating_sub(n);
        }
    }

    fn push_fallback(&mut self, set: SingleByte, byte: u8, out: &mut String) {
        match set.decode(byte) {
            Decoded::Char(c) => self.marks.push(out, c),
            Decoded::Mark(m) => self.marks.push_mark(out, m),
        }
    }

    fn decode_utf8(&mut self, mut input: &[u8], fallback: SingleByte, out: &mut String) {
        if self.carry_len > 0 {
            // Complete the sequence cut by the previous chunk, if the next
            // bytes continue it.
            let lead = self.carry[0];
            while self.carry_len < utf8_width(lead) {
                match input.split_first() {
                    Some((&b, rest)) if continues(lead, self.carry_len - 1, b) => {
                        self.carry[self.carry_len] = b;
                        self.carry_len += 1;
                        input = rest;
                    }
                    _ => break,
                }
            }
            let carry = self.carry;
            let cut = carry.get(..self.carry_len).unwrap_or_default();
            if let Ok(s) = std::str::from_utf8(cut) {
                self.marks.push_str(out, s);
                self.carry_len = 0;
            } else if input.is_empty() && is_incomplete_sequence(cut) {
                return;
            } else {
                self.carry_len = 0;
                for &b in cut {
                    self.push_fallback(fallback, b, out);
                }
            }
        }
        // Valid text, the common case, is checked a word at a time.
        if let Ok(s) = std::str::from_utf8(input) {
            self.marks.push_str(out, s);
            return;
        }
        let mut rest_len = input.len();
        for chunk in input.utf8_chunks() {
            self.marks.push_str(out, chunk.valid());
            let invalid = chunk.invalid();
            rest_len -= chunk.valid().len() + invalid.len();
            if rest_len == 0 && is_incomplete_sequence(invalid) {
                self.carry_len = invalid.len();
                self.carry
                    .get_mut(..invalid.len())
                    .into_iter()
                    .for_each(|c| c.copy_from_slice(invalid));
            } else {
                for &b in invalid {
                    self.push_fallback(fallback, b, out);
                }
            }
        }
    }

    fn decode_single(&mut self, input: &[u8], set: SingleByte, out: &mut String) {
        let mut rest = input;
        while !rest.is_empty() {
            // ASCII runs are copied as they are.
            let ascii = rest.iter().take_while(|b| b.is_ascii()).count();
            let (run, tail) = rest.split_at(ascii);
            if let Ok(run) = std::str::from_utf8(run) {
                self.marks.push_str(out, run);
            }
            let Some((&b, tail)) = tail.split_first() else {
                break;
            };
            self.push_fallback(set, b, out);
            rest = tail;
        }
    }

    fn decode_utf16(&mut self, input: &[u8], big_endian: bool, out: &mut String) {
        let mut input = input;
        if self.carry_len == 1 {
            let Some((&b, rest)) = input.split_first() else {
                return;
            };
            let pair = [self.carry[0], b];
            self.carry_len = 0;
            self.push_unit(unit(pair, big_endian), out);
            input = rest;
        }
        let (units, remainder) = input.as_chunks::<2>();
        for &pair in units {
            self.push_unit(unit(pair, big_endian), out);
        }
        if let [b] = remainder {
            self.carry[0] = *b;
            self.carry_len = 1;
        }
    }

    fn push_unit(&mut self, unit: u16, out: &mut String) {
        if let Some(high) = self.high_surrogate.take() {
            if (0xDC00..=0xDFFF).contains(&unit) {
                let c = 0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
                self.marks.push(
                    out,
                    char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER),
                );
                return;
            }
            self.marks.push(out, char::REPLACEMENT_CHARACTER);
        }
        if (0xD800..=0xDBFF).contains(&unit) {
            self.high_surrogate = Some(unit);
        } else {
            let c = char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER);
            self.marks.push(out, c);
        }
    }
}

fn unit(pair: [u8; 2], big_endian: bool) -> u16 {
    if big_endian {
        u16::from_be_bytes(pair)
    } else {
        u16::from_le_bytes(pair)
    }
}

/// ANSEL marks waiting for their base.
///
/// An ANSEL mark precedes its base. When a line ends with marks (a `CONC`
/// split between a mark and its letter), they are held across the line
/// terminator: if the next line is a `CONC`, they compose with the first
/// character of its payload; otherwise they are written on their own at the
/// end of the line they came from.
#[derive(Debug, Default)]
struct Marks {
    pending: Vec<char>,
    hold: Option<Hold>,
}

#[derive(Debug)]
struct Hold {
    marks: Vec<char>,
    /// Where the marks go if the next line is not a `CONC`: the end of the
    /// line they came from.
    insert_at: usize,
    state: Prefix,
    /// Characters of the next line seen so far.
    seen: u8,
}

/// Progress through the start of the line that follows held marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prefix {
    LineStart,
    Level,
    AfterLevel,
    Tag([u8; 4], u8),
    AwaitBase,
}

/// What the start of the next line says about held marks.
enum Verdict {
    /// Still reading the line's level and tag.
    Undecided,
    /// The character is the first of a `CONC` payload: the marks' base.
    Base,
    /// The line is not a `CONC` continuing the text.
    NotConc,
}

impl Hold {
    fn feed(&mut self, c: char) -> Verdict {
        self.seen = self.seen.saturating_add(1);
        if self.seen > 64 {
            return Verdict::NotConc;
        }
        let blank = c == ' ' || c == '\t';
        let eol = c == '\n' || c == '\r';
        self.state = match self.state {
            Prefix::LineStart if blank || eol => Prefix::LineStart,
            Prefix::LineStart | Prefix::Level if c.is_ascii_digit() => Prefix::Level,
            Prefix::Level | Prefix::AfterLevel if blank => Prefix::AfterLevel,
            Prefix::AfterLevel if c.is_ascii_alphabetic() => {
                Prefix::Tag([c.to_ascii_uppercase() as u8, 0, 0, 0], 1)
            }
            Prefix::Tag(tag, 4) if blank && &tag == b"CONC" => Prefix::AwaitBase,
            Prefix::Tag(mut tag, len) if c.is_ascii_alphabetic() && len < 4 => {
                if let Some(slot) = tag.get_mut(usize::from(len)) {
                    *slot = c.to_ascii_uppercase() as u8;
                }
                Prefix::Tag(tag, len + 1)
            }
            Prefix::AwaitBase if !eol => return Verdict::Base,
            _ => return Verdict::NotConc,
        };
        Verdict::Undecided
    }
}

impl Marks {
    #[inline]
    fn idle(&self) -> bool {
        self.pending.is_empty() && self.hold.is_none()
    }

    #[inline]
    fn push_str(&mut self, out: &mut String, s: &str) {
        if self.idle() {
            out.push_str(s);
            return;
        }
        for (i, c) in s.char_indices() {
            self.push(out, c);
            if self.idle() {
                out.push_str(s.get(i + c.len_utf8()..).unwrap_or_default());
                return;
            }
        }
    }

    fn push(&mut self, out: &mut String, c: char) {
        if let Some(hold) = self.hold.as_mut() {
            match hold.feed(c) {
                Verdict::Undecided => {
                    out.push(c);
                    return;
                }
                Verdict::Base => {
                    let marks = std::mem::take(&mut hold.marks);
                    self.hold = None;
                    push_composed(out, c, &marks);
                    return;
                }
                Verdict::NotConc => self.release(out),
            }
        }
        if self.pending.is_empty() {
            out.push(c);
        } else if c == '\n' || c == '\r' {
            self.hold = Some(Hold {
                marks: std::mem::take(&mut self.pending),
                insert_at: out.len(),
                state: Prefix::LineStart,
                seen: 0,
            });
            out.push(c);
        } else {
            push_composed(out, c, &self.pending);
            self.pending.clear();
        }
    }

    fn push_mark(&mut self, out: &mut String, mark: char) {
        if let Some(hold) = self.hold.as_mut() {
            if hold.state == Prefix::AwaitBase {
                // The `CONC` payload starts with more marks: all of them
                // apply to its first letter.
                self.pending = std::mem::take(&mut hold.marks);
                self.hold = None;
            } else {
                self.release(out);
            }
        }
        self.pending.push(mark);
    }

    /// Writes held marks at the end of the line they came from.
    fn release(&mut self, out: &mut String) {
        if let Some(hold) = self.hold.take() {
            let marks: String = hold.marks.iter().collect();
            if out.is_char_boundary(hold.insert_at) {
                out.insert_str(hold.insert_at, &marks);
            } else {
                out.push_str(&marks);
            }
        }
    }

    fn finish(&mut self, out: &mut String) {
        self.release(out);
        out.extend(self.pending.drain(..));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(mode: Mode, input: &[u8]) -> String {
        let mut d = Decoder::new(mode);
        let mut out = String::new();
        d.decode(input, &mut out);
        d.finish(&mut out);
        out
    }

    fn decode_split(mode: Mode, input: &[u8], cut: usize) -> String {
        let mut d = Decoder::new(mode);
        let mut out = String::new();
        let (a, b) = input.split_at(cut);
        d.decode(a, &mut out);
        d.decode(b, &mut out);
        d.finish(&mut out);
        out
    }

    const ANSEL: Mode = Mode::Single(SingleByte::Ansel);
    const UTF8: Mode = Mode::Utf8 {
        fallback: SingleByte::Cp1252,
    };

    #[test]
    fn ansel_marks_compose_with_their_base() {
        assert_eq!(decode_all(ANSEL, b"Jos\xE2e"), "José");
        assert_eq!(decode_all(ANSEL, b"\xE3\xE2a"), "\u{1EA5}");
        assert_eq!(decode_all(ANSEL, b"\xA1\xE2od\xE2z"), "Łódź");
    }

    #[test]
    fn ansel_mark_across_conc_composes_with_the_next_line() {
        let input = b"1 NOTE Jos\xE2\r\n2 CONC e here\r\n";
        assert_eq!(
            decode_all(ANSEL, input),
            "1 NOTE Jos\r\n2 CONC \u{e9} here\r\n"
        );
        for cut in 0..input.len() {
            assert_eq!(decode_split(ANSEL, input, cut), decode_all(ANSEL, input));
        }
    }

    #[test]
    fn ansel_mark_before_another_line_stays_on_its_line() {
        let input = b"1 NOTE x\xE2\n2 CONT e\n";
        assert_eq!(decode_all(ANSEL, input), "1 NOTE x\u{301}\n2 CONT e\n");
        assert_eq!(decode_all(ANSEL, b"x\xE2"), "x\u{301}");
    }

    #[test]
    fn utf8_with_stray_bytes_keeps_them() {
        assert_eq!(decode_all(UTF8, "é\x00".as_bytes()), "é\0");
        assert_eq!(decode_all(UTF8, b"caf\xC3\xA9 5\x80"), "café 5€");
    }

    #[test]
    fn chunk_boundaries_do_not_change_the_output() {
        let samples: [(Mode, &[u8]); 4] = [
            (UTF8, "a€b😀c\u{e9}".as_bytes()),
            (UTF8, b"\xE0\x80\xC3\xA9\xF0\x9F\x98x\xED\xA0\x80"),
            (
                Mode::Utf16 { big_endian: false },
                b"a\0\x3D\xD8\x00\xDEb\0\x00\xD8c",
            ),
            (ANSEL, b"\xE2e\xE3\xE2a\n\xE2\n2 CONC \xE8o"),
        ];
        for (mode, input) in samples {
            let whole = decode_all(mode, input);
            for cut in 0..=input.len() {
                assert_eq!(decode_split(mode, input, cut), whole, "{mode:?} cut {cut}");
            }
        }
    }
}
