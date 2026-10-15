//! The line emitter: the only code that writes a GEDCOM line.
//!
//! [`emit`] writes one structure — level, cross-reference identifier, tag and
//! payload — as one line plus the `CONT` and `CONC` lines its payload needs,
//! following the [`VersionRules`] of the target version. A [`LineSink`]
//! receives the lines, encodes them and buffers them for an [`io::Write`].
//!
//! Whatever it is given, the output follows the line grammar:
//!
//! - a level above the version's deepest is clamped to it;
//! - a tag outside the version's grammar, or a structure tagged `CONT` or
//!   `CONC`, is written as an extension tag (`_CONT`, `_FOO`);
//! - an identifier on a substructure is dropped, and a record identifier or a
//!   pointer outside the version's grammar is rewritten into it;
//! - text is escaped (5.5.1: every `@` except in an `@#…@` escape; 7.x: a
//!   leading `@` on each line), never written as a pointer;
//! - every line break of text (LF, CR LF or a lone CR) starts a `CONT` line;
//! - in 5.5.1, a line longer than the limit — level, identifier, tag,
//!   delimiters and terminator included — is continued with `CONC`, split
//!   between two characters that are not spaces and never before a
//!   combining mark (text with no such place in reach has a space written
//!   as a line break, `CONT`, instead); 7.x has no limit and no `CONC`;
//! - banned characters are removed.
//!
//! Each such rewrite is reported as a [`Repair`].

use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt::{self, Write as _};
use std::io;
use std::rc::Rc;

use super::{OutputEncoding, Repair, RepairKind, RepairPolicy, Unencodable, WriteError};
use crate::tree::PayloadRef;
use crate::version::{AtEscape, MAX_TAG_551, VersionRules, XrefGrammar};

/// The buffered bytes are handed to the writer once they exceed this size.
const FLUSH_AT: usize = 64 * 1024;

/// Where written lines go: an in-memory buffer, handed to an [`io::Write`]
/// in large blocks when there is one.
///
/// Lines are assembled as UTF-8 at the end of `text`. In UTF-8 output,
/// `text` is the output itself; otherwise each finished line is encoded into
/// `buf` and removed from `text`.
pub(crate) struct LineSink<'w> {
    writer: Option<&'w mut dyn io::Write>,
    /// The encoded output, for an encoding other than UTF-8.
    buf: Vec<u8>,
    /// The UTF-8 text: the output so far (UTF-8) or the open line (others).
    text: String,
    /// Where the open line starts in `text`.
    line_start: usize,
    eol: &'static str,
    /// The longest line, terminator included, when the version limits it.
    max_line: Option<usize>,
    encoding: OutputEncoding,
    policy: RepairPolicy,
    /// The number of the line being assembled, 1-based.
    line_no: u64,
    pub(crate) repairs: Vec<Repair>,
    /// The source line of the structure being written, 0 when unknown.
    pub(crate) source_line: u32,
    /// Where the lines of this sink start in `text`, when it writes after
    /// the text of another ([`LineSink::lend`]).
    base: usize,
    /// What looks at each structure written, when something does
    /// ([`Looker`]); the sinks of a record's lines share it.
    pub(crate) looker: Option<Rc<RefCell<dyn Looker>>>,
}

/// Tells `looker` of a structure written ([`Looker::structure`]): out of
/// the emitter's way, which most writes take without one.
#[cold]
#[inline(never)]
fn look(
    looker: &RefCell<dyn Looker>,
    level: usize,
    xref: Option<&str>,
    tag: &str,
    standard: Option<u16>,
    payload: PayloadRef<'_>,
) {
    looker
        .borrow_mut()
        .structure(level, xref, tag, standard, payload);
}

/// [`Looker::rollback`], out of the way.
#[cold]
#[inline(never)]
fn taken_back(looker: &RefCell<dyn Looker>) {
    looker.borrow_mut().rollback();
}

/// What looks at the structures a dataset's write writes, record after
/// record ([`crate::profile::Exporter`]): told of each structure as it
/// is written, before its payload is escaped and continued, of the lines
/// of a record taken back, and of each record's start and end.
pub(crate) trait Looker {
    /// A structure written: its level, identifier, tag (and the tag's
    /// index among the standard tags, when it is one) and payload.
    fn structure(
        &mut self,
        level: usize,
        xref: Option<&str>,
        tag: &str,
        standard: Option<u16>,
        payload: PayloadRef<'_>,
    );
    /// The lines of the record being written are taken back, all of
    /// them: it is written again.
    fn rollback(&mut self);
    /// Record `index` of the dataset's records (the header 0) starts.
    fn begin(&mut self, index: usize);
    /// Record `index` is written, repaired by the conformance pass when
    /// `repaired`: whether the write goes on.
    fn end(&mut self, index: usize, repaired: bool) -> bool;
}

impl<'w> LineSink<'w> {
    /// A sink writing to `writer`, or keeping everything in memory without
    /// one (see [`LineSink::into_bytes`]). `max_line` is the longest line,
    /// terminator included, when the version limits it.
    pub(crate) fn new(
        writer: Option<&'w mut dyn io::Write>,
        eol: &'static str,
        max_line: Option<usize>,
        encoding: OutputEncoding,
        policy: RepairPolicy,
    ) -> Self {
        // The buffer grows to its size as lines come: a small file needs
        // no large one.
        let capacity = if writer.is_some() { 4096 } else { 256 };
        let (text, buf) = if encoding == OutputEncoding::Utf8 {
            (String::with_capacity(capacity), Vec::new())
        } else {
            (String::with_capacity(256), Vec::with_capacity(capacity))
        };
        Self {
            writer,
            buf,
            text,
            line_start: 0,
            eol,
            max_line,
            encoding,
            policy,
            line_no: 1,
            repairs: Vec::new(),
            source_line: 0,
            base: 0,
            looker: None,
        }
    }

    /// A sink for the lines of one record, written as `self` writes them,
    /// in UTF-8 and in memory, where a line that needs a repair fails
    /// ([`RepairPolicy::Error`]).
    pub(crate) fn record_lines(&self) -> LineSink<'static> {
        let mut lines = LineSink::new(
            None,
            self.eol,
            self.max_line,
            OutputEncoding::Utf8,
            RepairPolicy::Error,
        );
        lines.looker.clone_from(&self.looker);
        lines
    }

    /// Hands the bytes written so far, encoded, to `writer`, and forgets
    /// them, when they are more than a block (or `all` of them): for a
    /// sink without a writer that writes to one in blocks.
    pub(crate) fn drain_to(
        &mut self,
        writer: &mut dyn io::Write,
        all: bool,
    ) -> Result<(), WriteError> {
        if self.encoding == OutputEncoding::Utf8 {
            if all || self.text.len() >= FLUSH_AT {
                writer.write_all(self.text.as_bytes())?;
                self.text.clear();
                self.line_start = 0;
            }
        } else if all || self.buf.len() >= FLUSH_AT {
            writer.write_all(&self.buf)?;
            self.buf.clear();
        }
        Ok(())
    }

    /// The output encoding.
    pub(crate) fn encoding(&self) -> OutputEncoding {
        self.encoding
    }

    /// The UTF-8 text written so far (UTF-8 output) or of the open line.
    #[cfg(debug_assertions)]
    pub(crate) fn text(&self) -> &str {
        self.text.get(self.base..).unwrap_or("")
    }

    /// Lends the text written so far to `lines`, a sink of
    /// [`LineSink::record_lines`], which writes its lines after it until
    /// [`LineSink::take_back`]: a record's lines are written in place, not
    /// copied. For a sink without a writer, in UTF-8.
    pub(crate) fn lend(&mut self, lines: &mut LineSink<'_>) {
        debug_assert!(self.writer.is_none() && self.encoding == OutputEncoding::Utf8);
        lines.text.clear();
        std::mem::swap(&mut self.text, &mut lines.text);
        lines.base = lines.text.len();
        lines.line_start = lines.base;
        lines.line_no = 1;
    }

    /// Takes back the text lent to `lines` ([`LineSink::lend`]), with the
    /// lines written there since when `keep`, without them otherwise.
    pub(crate) fn take_back(&mut self, lines: &mut LineSink<'_>, keep: bool) {
        if keep {
            self.line_no += lines.line_no - 1;
        } else {
            lines.text.truncate(lines.base);
            if let Some(looker) = &self.looker {
                taken_back(looker);
            }
        }
        std::mem::swap(&mut self.text, &mut lines.text);
        self.line_start = self.text.len();
        lines.clear();
        lines.base = 0;
        lines.line_start = 0;
    }

    /// Empties a sink without a writer (of the lines written since
    /// [`LineSink::lend`], for one lent text).
    pub(crate) fn clear(&mut self) {
        self.text.truncate(self.base);
        self.buf.clear();
        self.line_start = self.base;
        self.line_no = 1;
        self.repairs.clear();
        self.source_line = 0;
    }

    /// Appends the lines of `lines` (a sink of [`LineSink::record_lines`]),
    /// as if each was written here.
    pub(crate) fn append(&mut self, lines: &LineSink<'_>) -> Result<(), WriteError> {
        debug_assert_eq!(lines.line_start, lines.text.len());
        if self.encoding != OutputEncoding::Utf8 {
            // Encoded line by line, as they are written here.
            for line in lines.text.split_inclusive(self.eol) {
                self.text
                    .push_str(line.strip_suffix(self.eol).unwrap_or(line));
                self.end_line()?;
            }
            return Ok(());
        }
        self.text.push_str(&lines.text);
        self.line_start = self.text.len();
        self.line_no += lines.line_no - 1;
        if self.writer.is_some() && self.text.len() >= FLUSH_AT {
            self.flush()?;
        }
        Ok(())
    }

    /// A sink that keeps everything in memory, LF-terminated in UTF-8, for
    /// `rules` (tests).
    #[cfg(test)]
    pub(crate) fn buffer(rules: &VersionRules, policy: RepairPolicy) -> Self {
        Self::new(None, "\n", rules.max_line_len, OutputEncoding::Utf8, policy)
    }

    /// Room for `n` more bytes of UTF-8 text kept in memory (no writer):
    /// the output of a dataset is about the size of its input.
    pub(crate) fn reserve(&mut self, n: usize) {
        if self.writer.is_none() && self.encoding == OutputEncoding::Utf8 {
            self.text.reserve(n);
        }
    }

    /// The `HEAD.CHAR` payload naming the output encoding.
    pub(crate) fn charset(&self) -> &'static str {
        self.encoding.char_label()
    }

    /// Writes the byte order mark of the output encoding.
    pub(crate) fn bom(&mut self) {
        match self.encoding {
            OutputEncoding::Utf8 => self.text.push('\u{feff}'),
            OutputEncoding::Utf16Le => self.buf.extend_from_slice(&[0xFF, 0xFE]),
            OutputEncoding::Ansel | OutputEncoding::Ascii => {}
        }
    }

    /// Records a repair of the structure being written (its source line is
    /// [`LineSink::source_line`]).
    pub(crate) fn repair_here(
        &mut self,
        kind: RepairKind,
        detail: String,
    ) -> Result<(), WriteError> {
        self.repair(Repair::new(self.source_line, kind, detail))
    }

    /// Records a repair, or fails with it under [`RepairPolicy::Error`].
    pub(crate) fn repair(&mut self, repair: Repair) -> Result<(), WriteError> {
        match self.policy {
            RepairPolicy::Repair => {
                self.repairs.push(repair);
                Ok(())
            }
            RepairPolicy::Error => Err(WriteError::NonConformant(Box::new(repair))),
        }
    }

    /// The number of the line being assembled.
    pub(crate) fn line_no(&self) -> u64 {
        self.line_no
    }

    /// The length of the open line so far, in bytes.
    #[inline]
    fn line_len(&self) -> usize {
        self.text.len() - self.line_start
    }

    /// Ends the open line: adds its terminator, and encodes it for an
    /// encoding other than UTF-8.
    #[inline]
    fn end_line(&mut self) -> Result<(), WriteError> {
        if self.eol == "\n" {
            self.text.push('\n');
        } else {
            self.text.push_str(self.eol);
        }
        if self.encoding != OutputEncoding::Utf8 {
            return self.encode_line();
        }
        self.line_start = self.text.len();
        self.line_no += 1;
        // Text kept in memory, without a writer, is never handed over.
        if self.writer.is_some() && self.text.len() >= FLUSH_AT {
            self.flush()?;
        }
        Ok(())
    }

    /// [`LineSink::end_line`] in an encoding other than UTF-8: encodes the
    /// line, terminator included, into the output bytes.
    #[inline(never)]
    fn encode_line(&mut self) -> Result<(), WriteError> {
        let line = &self.text[self.line_start..];
        match self.encoding {
            // Never asked: UTF-8 is the text itself.
            OutputEncoding::Utf8 => {}
            OutputEncoding::Utf16Le => {
                for unit in line.encode_utf16() {
                    self.buf.extend_from_slice(&unit.to_le_bytes());
                }
            }
            OutputEncoding::Ascii => {
                if let Some(character) = line.chars().find(|c| !c.is_ascii()) {
                    return Err(self.unencodable(character));
                }
                self.buf.extend_from_slice(line.as_bytes());
            }
            OutputEncoding::Ansel => {
                let mut lost = 0;
                let bytes = crate::encoding::encode_ansel(line, &mut lost);
                if lost > 0 {
                    let character = line
                        .chars()
                        .find(|&c| {
                            let mut one = 0;
                            crate::encoding::encode_ansel(c.encode_utf8(&mut [0; 4]), &mut one);
                            one > 0
                        })
                        .unwrap_or(char::REPLACEMENT_CHARACTER);
                    return Err(self.unencodable(character));
                }
                self.buf.extend_from_slice(&bytes);
            }
        }
        self.text.clear();
        self.line_start = 0;
        self.line_no += 1;
        if self.buf.len() >= FLUSH_AT {
            self.flush()?;
        }
        Ok(())
    }

    fn unencodable(&self, character: char) -> WriteError {
        WriteError::Unencodable(Box::new(Unencodable {
            encoding: self.encoding,
            character,
            line: self.line_no,
        }))
    }

    /// Hands the buffered bytes to the writer, if there is one.
    pub(crate) fn flush(&mut self) -> Result<(), WriteError> {
        if let Some(writer) = self.writer.as_mut() {
            if self.encoding == OutputEncoding::Utf8 {
                writer.write_all(self.text.as_bytes())?;
                self.text.clear();
                self.line_start = 0;
            } else {
                writer.write_all(&self.buf)?;
                self.buf.clear();
            }
        }
        Ok(())
    }

    /// Flushes and flushes the writer.
    pub(crate) fn finish(&mut self) -> Result<(), WriteError> {
        self.flush()?;
        if let Some(writer) = self.writer.as_mut() {
            writer.flush()?;
        }
        Ok(())
    }

    /// The bytes written, encoded, for a sink without a writer.
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        if self.encoding == OutputEncoding::Utf8 {
            self.text.into_bytes()
        } else {
            self.buf
        }
    }

    /// The text written, for a UTF-8 sink without a writer.
    pub(crate) fn into_string(self) -> String {
        debug_assert_eq!(self.encoding, OutputEncoding::Utf8);
        self.text
    }

    /// Starts a line: `{level}[ {xref}] {tag}`.
    #[inline]
    fn start(&mut self, level: usize, xref: Option<&str>, tag: &str) {
        match u8::try_from(level) {
            Ok(digit @ 0..=9) => self.text.push(char::from(b'0' + digit)),
            _ => {
                let _ = write!(self.text, "{level}");
            }
        }
        if let Some(xref) = xref {
            self.text.push(' ');
            self.text.push_str(xref);
        }
        self.text.push(' ');
        self.text.push_str(tag);
    }
}

/// Writes one structure — without its substructures — as a line and the
/// `CONT`/`CONC` lines of its payload. See the [module documentation](self).
pub(crate) fn emit(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    xref: Option<&str>,
    tag: &str,
    payload: PayloadRef<'_>,
) -> Result<(), WriteError> {
    emit_tagged(rules, sink, level, xref, tag, false, payload)
}

/// [`emit`], told whether `tag` is a standard tag (one of every version's
/// grammar, which only `CONT` and `CONC` do not leave as it is).
pub(crate) fn emit_tagged(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    xref: Option<&str>,
    tag: &str,
    standard: bool,
    payload: PayloadRef<'_>,
) -> Result<(), WriteError> {
    emit_known(rules, sink, level, xref, tag, standard, payload, None)
}

/// [`emit_tagged`], the [`special_bytes`] of a text payload known when
/// `special` is; for a pointer, `Some(_)` says it is one the writer mapped
/// ([`XrefMap::pointer`](super::xref::XrefMap::pointer)), valid.
#[allow(clippy::too_many_arguments)] // The parts of one line, as `emit_tagged` takes them.
pub(crate) fn emit_known(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    xref: Option<&str>,
    tag: &str,
    standard: bool,
    payload: PayloadRef<'_>,
    special: Option<u8>,
) -> Result<(), WriteError> {
    if let Some(looker) = &sink.looker {
        let index = crate::tree::standard_index(tag);
        look(looker, level, xref, tag, index, payload);
    }
    emit_seen(rules, sink, level, xref, tag, standard, payload, special)
}

/// [`emit_known`] of a structure the [`Looker`] was told of.
#[allow(clippy::too_many_arguments)] // As `emit_known`.
fn emit_seen(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    xref: Option<&str>,
    tag: &str,
    standard: bool,
    payload: PayloadRef<'_>,
    special: Option<u8>,
) -> Result<(), WriteError> {
    let level = if level > rules.max_level {
        level_repair(rules, sink, level)?
    } else {
        level
    };
    let tag = if standard && !matches!(tag, "CONT" | "CONC") {
        Cow::Borrowed(tag)
    } else {
        structure_tag(rules, tag)
    };
    if let Cow::Owned(to) = &tag {
        repair(
            sink,
            RepairKind::Misplaced,
            format_args!("tag {tag:?} written as {to}"),
        )?;
    }
    let xref = match xref {
        Some(x) if level > 0 => {
            repair(
                sink,
                RepairKind::Xref,
                format_args!("identifier {x} of a substructure left out"),
            )?;
            None
        }
        Some(x) if !rules.is_valid_xref(x) => {
            let to = candidate_xref(rules, x);
            repair(
                sink,
                RepairKind::Xref,
                format_args!("identifier {x} written as {to}"),
            )?;
            Some(Cow::Owned(to))
        }
        Some(x) => Some(Cow::Borrowed(x)),
        None => None,
    };
    sink.start(level, xref.as_deref(), &tag);
    match payload {
        PayloadRef::None => sink.end_line(),
        PayloadRef::Pointer(p) => {
            sink.text.push(' ');
            if special.is_some() || is_valid_pointer(rules, p) {
                debug_assert!(is_valid_pointer(rules, p), "{p}");
                sink.text.push_str(p);
            } else {
                let to = candidate_xref(rules, p);
                sink.text.push_str(&to);
                repair(
                    sink,
                    RepairKind::Xref,
                    format_args!("pointer {p} written as {to}"),
                )?;
            }
            sink.end_line()
        }
        PayloadRef::Text(text) => {
            let special = special.unwrap_or_else(|| special_bytes(text));
            emit_text_special(rules, sink, level, text, special)
        }
    }
}

/// [`emit_known`] of a structure whose tag is in the version's grammar and
/// whose identifier, on a record only, is valid; its text payload has the
/// [`special_bytes`] `special`. A pointer is written as it is: the caller
/// makes sure that it names a record (whose identifier is valid), `@VOID@`
/// in 7.x or, in 5.5.1, a substructure or another file, or throws the line
/// away.
#[inline]
#[allow(clippy::too_many_arguments)] // A line, and its tag's index.
pub(crate) fn emit_typed(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    xref: Option<&str>,
    tag: &str,
    standard: Option<u16>,
    payload: PayloadRef<'_>,
    special: u8,
) -> Result<(), WriteError> {
    if let Some(looker) = &sink.looker {
        look(looker, level, xref, tag, standard, payload);
    }
    if level > rules.max_level || matches!(tag, "CONT" | "CONC") || (xref.is_some() && level > 0) {
        return emit_unusual(rules, sink, level, xref, tag, payload, special);
    }
    debug_assert!(rules.is_valid_tag(tag) && xref.is_none_or(|x| rules.is_valid_xref(x)));
    // Nearly every line: a level of one digit, no identifier, and a payload
    // written as it is that fits the line (no line break, no `@`, nothing
    // banned), in UTF-8.
    let payload_len = match payload {
        PayloadRef::None => Some(0),
        PayloadRef::Pointer(p) if !p.is_empty() => Some(p.len()),
        PayloadRef::Text(t) if special == 0 => Some(t.len()),
        PayloadRef::Pointer(_) | PayloadRef::Text(_) => None,
    };
    if let (Some(payload_len), Ok(digit @ 0..=9), None, OutputEncoding::Utf8) =
        (payload_len, u8::try_from(level), xref, sink.encoding)
    {
        let len = 2 + tag.len() + payload_len + usize::from(payload_len > 0) + sink.eol.len();
        if sink.max_line.is_none_or(|max| len <= max) {
            let text = &mut sink.text;
            text.reserve(len);
            text.push(char::from(b'0' + digit));
            text.push(' ');
            text.push_str(tag);
            if let Some(p) = payload.as_str().filter(|p| !p.is_empty()) {
                text.push(' ');
                text.push_str(p);
            }
            text.push_str(sink.eol);
            sink.line_start = sink.text.len();
            sink.line_no += 1;
            if sink.writer.is_some() && sink.text.len() >= FLUSH_AT {
                sink.flush()?;
            }
            return Ok(());
        }
    }
    emit_typed_text(rules, sink, level, xref, tag, payload, special)
}

/// [`emit_typed`] of a line it does not write in one go: a level of two
/// digits, an identifier, a text to escape, continue or wrap.
#[inline(never)]
fn emit_typed_text(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    xref: Option<&str>,
    tag: &str,
    payload: PayloadRef<'_>,
    special: u8,
) -> Result<(), WriteError> {
    sink.start(level, xref, tag);
    match payload {
        PayloadRef::None => sink.end_line(),
        PayloadRef::Pointer(p) => {
            sink.text.push(' ');
            sink.text.push_str(p);
            sink.end_line()
        }
        PayloadRef::Text(text) => emit_text_special(rules, sink, level, text, special),
    }
}

/// [`emit_typed`] of a line the version's rules change: too deep, tagged
/// `CONT` or `CONC`, or with an identifier below the record.
#[cold]
#[inline(never)]
fn emit_unusual(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    xref: Option<&str>,
    tag: &str,
    payload: PayloadRef<'_>,
    special: u8,
) -> Result<(), WriteError> {
    emit_seen(rules, sink, level, xref, tag, true, payload, Some(special))
}

/// Reports a repair of the line being written: `output line n: what`.
#[cold]
#[inline(never)]
fn repair(
    sink: &mut LineSink<'_>,
    kind: RepairKind,
    what: fmt::Arguments<'_>,
) -> Result<(), WriteError> {
    let detail = format!("output line {}: {what}", sink.line_no());
    sink.repair_here(kind, detail)
}

/// The deepest level, for a structure deeper: reported.
#[cold]
#[inline(never)]
fn level_repair(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
) -> Result<usize, WriteError> {
    repair(
        sink,
        RepairKind::Level,
        format_args!(
            "a structure at level {level} written at level {}",
            rules.max_level
        ),
    )?;
    Ok(rules.max_level)
}

/// Writes the text payload of a started line, with its `CONT` and `CONC`
/// lines.
#[cfg(test)]
fn emit_text(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    text: &str,
) -> Result<(), WriteError> {
    emit_text_special(rules, sink, level, text, special_bytes(text))
}

/// [`emit_text`] of a text whose [`special_bytes`] are `special`.
fn emit_text_special(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    text: &str,
    special: u8,
) -> Result<(), WriteError> {
    // Nearly all text is lines of characters that are never banned (LF
    // line breaks at most): each line is escaped and wrapped as it is.
    let at_signs = special & AT_SIGN != 0;
    match special & !AT_SIGN {
        0 => plain_line(rules, sink, level, text, at_signs),
        LINE_FEED => {
            for (i, part) in text.split('\n').enumerate() {
                if i > 0 {
                    sink.start(level + 1, None, "CONT");
                }
                plain_line(rules, sink, level, part, at_signs)?;
            }
            Ok(())
        }
        _ => emit_text_general(rules, sink, level, text),
    }
}

/// [`emit_text`] of any text: line breaks of every kind, tabs and banned
/// characters.
fn emit_text_general(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    text: &str,
) -> Result<(), WriteError> {
    let whole = scan(text);
    let tabs_allowed = !rules.is_banned('\t');
    let mut first = true;
    let (mut banned, mut tabs) = (0_usize, 0_usize);
    let mut rest = text;
    loop {
        // One line of the text: up to a CR, an LF or a CR LF.
        let end = if whole.breaks {
            rest.bytes().position(|b| b == b'\r' || b == b'\n')
        } else {
            None
        };
        let (part, next) = match end {
            Some(at) => {
                let skip = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
                (&rest[..at], Some(&rest[at + skip..]))
            }
            None => (rest, None),
        };
        if !first {
            sink.start(level + 1, None, "CONT");
        }
        first = false;
        let found = if whole.breaks { scan(part) } else { whole };
        let suspect = found.suspect || (found.tabs && !tabs_allowed);
        let part: Cow<'_, str> = if suspect && has_banned(rules, part) {
            let mut kept = String::with_capacity(part.len());
            for c in part.chars() {
                if c == '\t' && !tabs_allowed {
                    // 5.5.1 has no tab: the nearest character it has.
                    kept.push(' ');
                    tabs += 1;
                } else if rules.is_banned(c) {
                    banned += 1;
                } else {
                    kept.push(c);
                }
            }
            Cow::Owned(kept)
        } else {
            Cow::Borrowed(part)
        };
        if !part.is_empty() {
            if let Some(max) = sink.max_line {
                wrapped(rules, sink, level, &part, Some(found.at_signs), max)?;
            } else {
                sink.text.push(' ');
                escape_into(rules.at_escape, &part, &mut sink.text);
            }
        }
        sink.end_line()?;
        match next {
            Some(next) => rest = next,
            None => break,
        }
    }
    if banned + tabs > 0 {
        let mut what = Vec::new();
        if banned > 0 {
            what.push(format!("{banned} banned character(s) left out"));
        }
        if tabs > 0 {
            what.push(format!("{tabs} tab(s) written as spaces"));
        }
        let detail = format!("output line {}: {}", sink.line_no() - 1, what.join(", "));
        sink.repair_here(RepairKind::Characters, detail)?;
    }
    Ok(())
}

/// What one pass over the bytes of `text` finds.
#[derive(Clone, Copy)]
struct Scan {
    /// A byte that can start a banned character.
    suspect: bool,
    /// A line break (CR or LF).
    breaks: bool,
    /// A tab (banned in 5.5.1 only).
    tabs: bool,
    /// The number of `@`: an upper bound of what escaping adds.
    at_signs: usize,
}

/// Bits of [`special_bytes`]: a line feed.
pub(crate) const LINE_FEED: u8 = 1;
/// A carriage return.
pub(crate) const CARRIAGE_RETURN: u8 = 2;
/// A tab.
pub(crate) const TAB: u8 = 4;
/// Any other C0 control.
pub(crate) const CONTROL: u8 = 8;
/// DEL, or the lead byte of a C1 control (`C2`) or of U+FFFE and U+FFFF
/// (`EF`).
pub(crate) const MAYBE_BANNED: u8 = 16;
/// An `@`, which text may have to escape.
pub(crate) const AT_SIGN: u8 = 32;

/// The [`special_bytes`] class of each byte.
static CLASSES: [u8; 256] = {
    let mut table = [0_u8; 256];
    let mut c = 0;
    while c < 0x20 {
        table[c] = CONTROL;
        c += 1;
    }
    table[b'\n' as usize] = LINE_FEED;
    table[b'\r' as usize] = CARRIAGE_RETURN;
    table[b'\t' as usize] = TAB;
    table[0x7F] = MAYBE_BANNED;
    table[0xC2] = MAYBE_BANNED;
    table[0xEF] = MAYBE_BANNED;
    table[b'@' as usize] = AT_SIGN;
    table
};

/// What `text` holds of the bytes that can start a line break, a tab or a
/// banned character, or that escaping changes (`@`), as bits. Eight bytes
/// are tested at a time, and only a word that holds one is looked at byte
/// by byte; the last bytes are looked up one at a time.
#[inline]
pub(crate) fn special_bytes(text: &str) -> u8 {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    // Whether a byte of the word is zero; whether one is below `n` (at
    // most 128). Exact, as booleans.
    let zero = |v: u64| v.wrapping_sub(ONES) & !v & HIGH;
    let below = |v: u64, n: u64| v.wrapping_sub(ONES * n) & !v & HIGH;
    let class = |c: &u8| CLASSES.get(usize::from(*c)).copied().unwrap_or(0);
    let word = |w: &[u8; 8]| {
        let x = u64::from_le_bytes(*w);
        let hit = below(x, 0x20)
            | zero(x ^ (ONES * 0x7F))
            | zero(x ^ (ONES * 0xC2))
            | zero(x ^ (ONES * 0xEF))
            | zero(x ^ (ONES * u64::from(b'@')));
        if hit == 0 {
            0
        } else {
            w.iter().fold(0, |f, c| f | class(c))
        }
    };
    let (words, rest) = text.as_bytes().as_chunks::<8>();
    let found = words.iter().fold(0, |f, w| f | word(w));
    rest.iter().fold(found, |f, c| f | class(c))
}

/// Writes one line of text without special bytes ([`special_bytes`]) on
/// the started line, escaped and wrapped, and ends it.
#[inline]
/// `at_signs`: whether it may hold an `@` (none: it is written as it is).
fn plain_line(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    text: &str,
    at_signs: bool,
) -> Result<(), WriteError> {
    if !text.is_empty() {
        match sink.max_line {
            Some(max) => {
                wrapped(rules, sink, level, text, (!at_signs).then_some(0), max)?;
            }
            None if !at_signs => {
                sink.text.push(' ');
                sink.text.push_str(text);
            }
            None => {
                sink.text.push(' ');
                escape_into(rules.at_escape, text, &mut sink.text);
            }
        }
    }
    sink.end_line()
}

/// Scans `text` once, in blocks the compiler vectorises.
fn scan(text: &str) -> Scan {
    let (mut flags, mut at_signs) = (0_u8, 0_usize);
    for chunk in text.as_bytes().chunks(128) {
        let (mut f, mut a) = (0_u8, 0_u8);
        for &c in chunk {
            f |= (u8::from(c < 0x20) & u8::from(c != b'\t'))
                | u8::from(c == 0x7F)
                | u8::from(c == 0xC2)
                | u8::from(c == 0xEF)
                | (u8::from(c == b'\r' || c == b'\n') << 1)
                | (u8::from(c == b'\t') << 2);
            a += u8::from(c == b'@');
        }
        flags |= f;
        at_signs += usize::from(a);
    }
    Scan {
        // A line break is a control character too: tell them apart later.
        suspect: flags & 1 != 0,
        breaks: flags & 2 != 0,
        tabs: flags & 4 != 0,
        at_signs,
    }
}

/// Whether `text` holds a character the version bans (see
/// [`VersionRules::is_banned`]), from its UTF-8 bytes: a C0 control (tab in
/// 5.5.1 only), DEL, a C1 control (`C2 80`–`C2 9F`), U+FFFE or U+FFFF
/// (`EF BF BE`, `EF BF BF`).
fn has_banned(rules: &VersionRules, text: &str) -> bool {
    let tab_banned = rules.is_banned('\t');
    let b = text.as_bytes();
    b.iter().enumerate().any(|(i, &c)| match c {
        b'\t' => tab_banned,
        0..=0x1F | 0x7F => true,
        0xC2 => b.get(i + 1).is_some_and(|n| (0x80..=0x9F).contains(n)),
        0xEF => b.get(i + 1) == Some(&0xBF) && matches!(b.get(i + 2), Some(0xBE | 0xBF)),
        _ => false,
    })
}

/// Writes `part` (no line break, not empty) on the started line and as many
/// `CONC` lines as the line length requires; the last line is left open.
/// `at_signs` is the number of `@` of `part`, when known.
fn wrapped(
    rules: &VersionRules,
    sink: &mut LineSink<'_>,
    level: usize,
    part: &str,
    at_signs: Option<usize>,
    max: usize,
) -> Result<(), WriteError> {
    // Most payloads fit: an upper bound of their escaped length tells (an
    // `@` is at most doubled, so twice the length is a bound too).
    let fixed = sink.line_len() + 1 + part.len() + sink.eol.len();
    let fits = fixed + part.len() <= max
        || fixed + at_signs.unwrap_or_else(|| part.bytes().filter(|&b| b == b'@').count()) <= max;
    if fits {
        sink.text.push(' ');
        if at_signs == Some(0) {
            sink.text.push_str(part);
        } else {
            escape_into(rules.at_escape, part, &mut sink.text);
        }
        return Ok(());
    }
    let mut pos = 0;
    let mut broken = 0_usize;
    loop {
        let room = max
            .saturating_sub(sink.line_len() + 1 + sink.eol.len())
            .max(1);
        let ansel = sink.encoding == OutputEncoding::Ansel;
        let cut = cut_point(rules.at_escape, part, pos, room, ansel);
        // A `CONC` line never starts or ends with a space (readers trim
        // them). Text with no two characters in reach that are not spaces
        // (`a b c …`, a run of spaces) has no such cut: a space next to the
        // cut is written as a line break instead (`CONT`).
        let space = if cut == part.len() {
            None
        } else if part[..cut].ends_with(' ') {
            Some(cut - 1)
        } else if part[cut..].starts_with(' ') {
            Some(cut)
        } else {
            None
        };
        let end = space.unwrap_or(cut);
        if end > pos {
            sink.text.push(' ');
            escape_into(rules.at_escape, &part[pos..end], &mut sink.text);
        }
        if cut == part.len() {
            break;
        }
        sink.end_line()?;
        if let Some(space) = space {
            broken += 1;
            sink.start(level + 1, None, "CONT");
            pos = space + 1;
        } else {
            sink.start(level + 1, None, "CONC");
            pos = cut;
        }
        if pos == part.len() {
            break;
        }
    }
    if broken > 0 {
        let detail = format!(
            "output line {}: {broken} space(s) written as line breaks, the text having no place to continue with CONC",
            sink.line_no()
        );
        sink.repair_here(RepairKind::Characters, detail)?;
    }
    Ok(())
}

/// The end of the next piece of `s` from `pos`: the whole rest when its
/// escaped form fits in `room` bytes, else the last split point that fits —
/// between two escape units, preferably between two characters that are not
/// spaces and not before a combining mark, else between two that are not
/// spaces — and at least one unit.
///
/// In ANSEL, where a combining mark comes before its base, a mark must stay
/// on the line of its base: a cut next to a space is then preferred to one
/// before a mark.
fn cut_point(escape: AtEscape, s: &str, pos: usize, room: usize, ansel: bool) -> usize {
    let bytes = s.as_bytes();
    // Forward, byte by byte: the furthest unit boundary that fits, and the
    // escape sequences on the way, which are not cut. Without an `@` in
    // reach, that is simply `room` bytes on.
    let mut cost = 0;
    let mut at = pos;
    let mut escapes: Vec<(usize, usize)> = Vec::new();
    let reach = (pos + room + 1).min(s.len());
    if !bytes[pos..reach].contains(&b'@') {
        at = reach.min(pos + room);
        if at >= s.len() {
            return s.len();
        }
        cost = room + 1; // skip the scan
    }
    while at < s.len() && cost <= room {
        let (len, out) = if bytes[at] == b'@' {
            match escape {
                AtEscape::LeadingOnly => (1, if at == pos { 2 } else { 1 }),
                AtEscape::AllAtSigns => match escape_sequence(&s[at..]) {
                    Some(len) => (len, len),
                    None => (1, 2),
                },
            }
        } else {
            (1, 1)
        };
        if at > pos && cost + out > room {
            break;
        }
        if len > 1 {
            escapes.push((at, at + len));
        }
        cost += out;
        at += len;
    }
    if at >= s.len() {
        return s.len();
    }
    // A cut inside a character moves back to its start; at least one
    // character goes on the line.
    let mut limit = at;
    while !s.is_char_boundary(limit) {
        limit -= 1;
    }
    if limit == pos {
        return pos + s[pos..].chars().next().map_or(1, char::len_utf8);
    }
    // Backward from there: the best place to cut.
    let inside = |p: usize| escapes.iter().any(|&(start, end)| start < p && p < end);
    let (mut fallback, mut spaced) = (None, None);
    let mut after = s[limit..].chars().next();
    for (p, before) in s[pos..limit].char_indices().rev() {
        let cut = pos + p + before.len_utf8();
        if let Some(a) = after.filter(|_| !inside(cut)) {
            match split_quality(before, a) {
                Split::Good => return cut,
                Split::BeforeMark => fallback = fallback.or(Some(cut)),
                Split::AtSpace => spaced = spaced.or(Some(cut)),
                Split::Other => {}
            }
        }
        after = Some(before);
    }
    let second = if ansel { spaced.or(fallback) } else { fallback };
    second.unwrap_or(limit)
}

/// The longest escape sequence kept as one: GEDCOM 5.5.1 defines only
/// calendar escapes such as `@#DFRENCH R@`.
const MAX_ESCAPE: usize = 32;

/// The length of the `@#…@` escape sequence `s` starts with, if any: `@#`,
/// then at most [`MAX_ESCAPE`] bytes up to the closing `@`.
fn escape_sequence(s: &str) -> Option<usize> {
    let body = s.strip_prefix("@#")?;
    body.bytes()
        .take(MAX_ESCAPE)
        .position(|b| b == b'@')
        .map(|end| end + 3)
}

/// How good a place byte `at` of `s` is to cut text at.
enum Split {
    /// Between two characters that are not spaces or tabs (readers trim
    /// them at the ends of lines), the second not a combining mark.
    Good,
    /// Between two characters that are not spaces, before a combining mark
    /// (which a reader joining the lines puts back on its base).
    BeforeMark,
    /// Next to a space or tab, and not before a combining mark.
    AtSpace,
    /// Anywhere else.
    Other,
}

fn split_quality(before: char, after: char) -> Split {
    let space = |c: char| c == ' ' || c == '\t';
    let mark = after >= '\u{300}' && is_combining(after);
    match (space(before) || space(after), mark) {
        (false, false) => Split::Good,
        (false, true) => Split::BeforeMark,
        (true, false) => Split::AtSpace,
        (true, true) => Split::Other,
    }
}

/// Whether `c` is a combining mark of the common combining blocks.
fn is_combining(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}'
        | '\u{0483}'..='\u{0489}'
        | '\u{0591}'..='\u{05BD}'
        | '\u{0610}'..='\u{061A}'
        | '\u{064B}'..='\u{065F}'
        | '\u{0900}'..='\u{0903}'
        | '\u{093A}'..='\u{094F}'
        | '\u{1AB0}'..='\u{1AFF}'
        | '\u{1DC0}'..='\u{1DFF}'
        | '\u{200C}'..='\u{200D}'
        | '\u{20D0}'..='\u{20FF}'
        | '\u{302A}'..='\u{302F}'
        | '\u{3099}'..='\u{309A}'
        | '\u{FE00}'..='\u{FE0F}'
        | '\u{FE20}'..='\u{FE2F}')
}

/// Appends `text` (one line, no line break) escaped for the version.
#[inline]
pub(crate) fn escape_into(escape: AtEscape, text: &str, out: &mut String) {
    match escape {
        AtEscape::LeadingOnly => {
            if text.starts_with('@') {
                out.push('@');
            }
            out.push_str(text);
        }
        AtEscape::AllAtSigns => {
            let mut rest = text;
            while let Some(at) = rest.find('@') {
                out.push_str(&rest[..at]);
                let from_at = &rest[at..];
                if let Some(len) = escape_sequence(from_at) {
                    out.push_str(&from_at[..len]);
                    rest = &from_at[len..];
                } else {
                    out.push_str("@@");
                    rest = &from_at[1..];
                }
            }
            out.push_str(rest);
        }
    }
}

/// The tag written for a structure: the tag itself when it follows the
/// version's grammar and is not `CONT` or `CONC` (which only continue a
/// payload), else an extension tag made from it.
pub(crate) fn structure_tag<'t>(rules: &VersionRules, tag: &'t str) -> Cow<'t, str> {
    if rules.is_valid_tag(tag) && !matches!(tag, "CONT" | "CONC") {
        Cow::Borrowed(tag)
    } else {
        Cow::Owned(extension_tag(rules, tag))
    }
}

/// An extension tag made from any tag: `_` and the tag's letters, digits and
/// underscores (upper case in 7.x), anything else as `_`.
pub(crate) fn extension_tag(rules: &VersionRules, tag: &str) -> String {
    let v7 = rules.xref == XrefGrammar::V7;
    let body = tag.strip_prefix('_').unwrap_or(tag);
    let mut out = String::with_capacity(body.len() + 1);
    out.push('_');
    for c in body.chars() {
        out.push(match c {
            'a'..='z' if v7 => c.to_ascii_uppercase(),
            'A'..='Z' | 'a'..='z' | '0'..='9' | '_' => c,
            _ => '_',
        });
    }
    if out.len() == 1 {
        out.push('X');
    }
    if !v7 {
        out.truncate(MAX_TAG_551);
    }
    out
}

/// Whether a pointer can be written as it is: a valid identifier (in
/// 5.5.1 also a substructure `@I1!2@` or network `@A:B@` pointer, p. 16),
/// or `@VOID@` in 7.x.
pub(crate) fn is_valid_pointer(rules: &VersionRules, p: &str) -> bool {
    rules.is_valid_xref(p) || (rules.xref == XrefGrammar::V7 && p == "@VOID@")
}

/// An identifier in the version's grammar made from any identifier: the
/// same for valid ones, otherwise its characters with the invalid ones
/// replaced (`@i 1@` becomes `@I_1@` in 7.x). New 5.5.1 identifiers keep to
/// a stricter subset of the 5.5.1 grammar — visible ASCII, no `!` or `:`
/// (the substructure and network forms) — while valid 5.5.1 identifiers
/// that use spaces, `!` or `:` are written as they are. The result is
/// deterministic; callers that know every identifier make it unique.
pub(crate) fn candidate_xref(rules: &VersionRules, xref: &str) -> String {
    if rules.is_valid_xref(xref) {
        return xref.to_string();
    }
    new_xref(rules, xref)
}

/// A new identifier made from any identifier, valid or not, in the stricter
/// subset [`candidate_xref`] describes: what a duplicate is renamed from.
pub(crate) fn new_xref(rules: &VersionRules, xref: &str) -> String {
    let id = xref.strip_prefix('@').unwrap_or(xref);
    let id = id.strip_suffix('@').unwrap_or(id);
    let mut out = String::with_capacity(id.len() + 3);
    out.push('@');
    match rules.xref {
        XrefGrammar::V7 => {
            for c in id.chars() {
                out.push(match c {
                    'a'..='z' => c.to_ascii_uppercase(),
                    'A'..='Z' | '0'..='9' | '_' => c,
                    _ => '_',
                });
            }
            if out.len() == 1 {
                out.push('X');
            }
            if out == "@VOID" {
                out.push('_');
            }
        }
        XrefGrammar::V551 { max_len } => {
            if !id.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                out.push('X');
            }
            for c in id.chars() {
                if out.len() + 1 >= max_len {
                    break;
                }
                out.push(if c.is_ascii_graphic() && !matches!(c, '@' | '!' | ':') {
                    c
                } else {
                    '_'
                });
            }
        }
    }
    out.push('@');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::{V70, V551};

    fn lines(rules: &VersionRules, level: usize, tag: &str, payload: PayloadRef<'_>) -> String {
        let mut sink = LineSink::buffer(rules, RepairPolicy::Repair);
        emit(rules, &mut sink, level, None, tag, payload).unwrap();
        sink.into_string()
    }

    #[test]
    fn escapes() {
        let esc = |e, t: &str| {
            let mut s = String::new();
            escape_into(e, t, &mut s);
            s
        };
        assert_eq!(
            esc(AtEscape::LeadingOnly, "@@@@ has four"),
            "@@@@@ has four"
        );
        assert_eq!(esc(AtEscape::LeadingOnly, "a@b"), "a@b");
        assert_eq!(esc(AtEscape::AllAtSigns, "a@b @c"), "a@@b @@c");
        assert_eq!(
            esc(AtEscape::AllAtSigns, "@#DJULIAN@ 1700"),
            "@#DJULIAN@ 1700"
        );
        assert_eq!(esc(AtEscape::AllAtSigns, "@#open"), "@@#open");
    }

    #[test]
    fn text_lines_and_breaks() {
        assert_eq!(
            lines(&V70, 1, "NOTE", PayloadRef::Text("\r\na\r\rb\n@c")),
            "1 NOTE\n2 CONT a\n2 CONT\n2 CONT b\n2 CONT @@c\n"
        );
        assert_eq!(
            lines(&V551, 1, "NOTE", PayloadRef::Text("x@y\u{7}")),
            "1 NOTE x@@y\n"
        );
        assert_eq!(lines(&V70, 1, "NOTE", PayloadRef::Text("")), "1 NOTE\n");
    }

    #[test]
    fn pointers_are_never_escaped() {
        assert_eq!(
            lines(&V551, 1, "FAMC", PayloadRef::Pointer("@F1@")),
            "1 FAMC @F1@\n"
        );
        assert_eq!(
            lines(&V70, 1, "FAMC", PayloadRef::Pointer("@VOID@")),
            "1 FAMC @VOID@\n"
        );
        assert_eq!(
            lines(&V70, 1, "FAMC", PayloadRef::Pointer("@f 1@")),
            "1 FAMC @F_1@\n"
        );
    }

    #[test]
    fn tags() {
        assert_eq!(structure_tag(&V70, "_x"), "_X");
        assert_eq!(structure_tag(&V551, "_x"), "_x");
        assert_eq!(structure_tag(&V551, "CONT"), "_CONT");
        assert_eq!(structure_tag(&V70, "mili"), "_MILI");
        assert_eq!(structure_tag(&V70, ""), "_X");
        assert_eq!(structure_tag(&V551, "NA-ME"), "_NA_ME");
        assert_eq!(structure_tag(&V551, &"A".repeat(40)).len(), 31);
    }

    #[test]
    fn xrefs() {
        assert_eq!(candidate_xref(&V70, "@i1@"), "@I1@");
        assert_eq!(candidate_xref(&V70, "@VOID@"), "@VOID_@");
        assert_eq!(candidate_xref(&V70, "@@"), "@X@");
        assert_eq!(candidate_xref(&V551, "@#I1@"), "@X#I1@");
        assert_eq!(candidate_xref(&V551, "@_I1@"), "@_I1@");
        assert_eq!(candidate_xref(&V551, "@#a b:c@"), "@X#a_b_c@");
        // Valid 5.5.1 identifiers with spaces, `!` or `:` stay (p. 13).
        assert_eq!(candidate_xref(&V551, "@NoTe ref@"), "@NoTe ref@");
        assert_eq!(new_xref(&V551, "@NoTe ref@"), "@NoTe_ref@");
        assert_eq!(new_xref(&V551, "@I1!2@"), "@I1_2@");
        assert_eq!(new_xref(&V70, "@I1@"), "@I1@");
        let long = candidate_xref(&V551, "@IABCDEFGHIJKLMNOPQRSTUVWXYZ0123@");
        assert!(V551.is_valid_xref(&long), "{long}");
        assert!(is_valid_pointer(&V551, "@I1!2@"));
        assert!(is_valid_pointer(&V551, "@NoTe ref@"));
        assert!(is_valid_pointer(&V551, "@NET:I1@"));
        assert!(!is_valid_pointer(&V70, "@I1!2@"));
    }

    #[test]
    fn long_lines_are_continued_within_the_limit() {
        let text = "word ".repeat(120);
        let out = lines(&V551, 3, "PAGE", PayloadRef::Text(text.trim_end()));
        assert!(out.lines().count() > 2, "{out}");
        for line in out.lines() {
            assert!(line.len() < 255, "{line}");
            if let Some(piece) = line.strip_prefix("4 CONC ") {
                assert!(!piece.starts_with(' '), "{line}");
            }
        }
        assert!(!out.lines().any(|l| l.ends_with(' ')), "{out}");
        let joined: String = out
            .lines()
            .map(|l| l.splitn(3, ' ').nth(2).unwrap_or(""))
            .collect();
        assert_eq!(joined, text.trim_end());
    }

    #[test]
    fn text_with_no_place_for_conc_breaks_at_a_space() {
        // Single letters between spaces, then a run of spaces: no two
        // characters in reach of a cut are both other than spaces.
        for text in [
            "a ".repeat(200),
            format!("x{}y", " ".repeat(600)),
            format!("{}@{}", " ".repeat(300), " ".repeat(300)),
        ] {
            let mut sink = LineSink::buffer(&V551, RepairPolicy::Repair);
            emit(&V551, &mut sink, 0, None, "NOTE", PayloadRef::Text(&text)).unwrap();
            let repairs = sink.repairs.len();
            let out = sink.into_string();
            let mut previous = "";
            for line in out.lines() {
                assert!(line.len() < 255, "{line}");
                if let Some(piece) = line.strip_prefix("1 CONC ") {
                    assert!(!piece.starts_with(' ') && !previous.ends_with(' '), "{out}");
                }
                previous = line;
            }
            assert_eq!(repairs, 1, "{out}");
            // Every space the text keeps is still there: the ones written as
            // line breaks are the only change.
            let back = crate::tree::Tree::parse(out.clone()).to_structures();
            let value = back[0].text().unwrap_or("");
            assert_eq!(value.replace('\n', " "), text, "{out}");
        }
    }

    #[test]
    fn splits_never_break_an_escape_or_before_a_mark() {
        let text = format!("{}@@{}", "a".repeat(240), "e\u{301}".repeat(60));
        let out = lines(&V551, 1, "NOTE", PayloadRef::Text(&text));
        for line in out.lines() {
            assert!(line.len() < 255, "{line}");
            let piece = line.splitn(3, ' ').nth(2).unwrap_or("");
            assert!(!piece.starts_with('\u{301}'), "{line}");
            assert_eq!(piece.matches('@').count() % 2, 0, "{line}");
        }
    }

    #[test]
    fn seven_never_wraps() {
        let text = "x".repeat(1000);
        assert_eq!(
            lines(&V70, 1, "NOTE", PayloadRef::Text(&text)),
            format!("1 NOTE {text}\n")
        );
    }

    /// Eight bytes at a time, the classes are those of each byte.
    #[test]
    fn special_bytes_by_words() {
        let reference = |t: &str| {
            t.bytes().fold(0, |f, c| {
                f | match c {
                    b'\n' => LINE_FEED,
                    b'\r' => CARRIAGE_RETURN,
                    b'\t' => TAB,
                    0..=0x1F => CONTROL,
                    0x7F | 0xC2 | 0xEF => MAYBE_BANNED,
                    b'@' => AT_SIGN,
                    _ => 0,
                }
            })
        };
        for c in (0..=0x7F_u8)
            .map(char::from)
            .chain(['\u{80}', '\u{9f}', 'é', '\u{fffe}', '\u{ffff}', '€'])
        {
            for at in 0..20 {
                let text = format!("{}{c}{}", "a".repeat(at), "b".repeat(20 - at));
                assert_eq!(special_bytes(&text), reference(&text), "{c:?} at {at}");
            }
        }
    }

    /// The paths for common text write what the general one writes.
    #[test]
    fn plain_text_paths_write_as_the_general_one() {
        let pieces = [
            "a",
            "word ",
            "@",
            "@@",
            "@#DJULIAN@",
            "\n",
            "é",
            "e\u{301}",
            " ",
            "\t",
            "\r",
            "\u{7}",
            "ç",
            "z",
        ];
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..4000 {
            let mut text = String::new();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = (seed % 90) as usize;
            let mut x = seed;
            for _ in 0..len {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                text.push_str(pieces[(x >> 33) as usize % pieces.len()]);
            }
            for rules in [&V551, &V70] {
                for max in [None, Some(40), Some(255)] {
                    let sink = |rules: &VersionRules| {
                        let mut sink = LineSink::buffer(rules, RepairPolicy::Repair);
                        if rules.max_line_len.is_some() {
                            sink.max_line = max;
                        }
                        sink
                    };
                    let (mut fast, mut general) = (sink(rules), sink(rules));
                    fast.start(1, None, "NOTE");
                    emit_text(rules, &mut fast, 1, &text).unwrap();
                    general.start(1, None, "NOTE");
                    emit_text_general(rules, &mut general, 1, &text).unwrap();
                    assert_eq!(fast.repairs, general.repairs, "{text:?}");
                    assert_eq!(fast.into_string(), general.into_string(), "{text:?}");
                }
            }
        }
    }

    #[test]
    fn levels_are_clamped() {
        let mut sink = LineSink::buffer(&V551, RepairPolicy::Repair);
        emit(&V551, &mut sink, 150, None, "_X", PayloadRef::Text("a\nb")).unwrap();
        assert_eq!(sink.repairs.len(), 1);
        assert_eq!(sink.into_string(), "98 _X a\n99 CONT b\n");
        let mut sink = LineSink::buffer(&V551, RepairPolicy::Error);
        assert!(emit(&V551, &mut sink, 150, None, "_X", PayloadRef::None).is_err());
    }
}
