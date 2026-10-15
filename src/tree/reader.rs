//! Reading a stream record by record.

use std::collections::VecDeque;
use std::io::{self, BufRead};

use super::lexer::{
    Builder, Escaping, Line, TERMINATOR_LOOKAHEAD, TagInterner, find_eol, head_version, lex_line,
    starts_stray, terminator_len,
};
use super::{Structure, arena};
use crate::encoding::{DecodeReader, GedcomEncoding};
use crate::version::GedcomVersion;

/// Splits a stream of text into records, as the in-memory reader does: a
/// record starts at a level-0 line, or at the first line that is not blank.
///
/// Each record comes back as its lines with LF terminators (blank lines
/// included, so that line numbers stay right) and the source line number of
/// its first line. Memory is bounded by the largest record.
pub(crate) struct RecordSplitter<R> {
    inner: R,
    /// The text read and not yet split: checked once as it is read, not
    /// line by line.
    buf: String,
    /// The bytes of a character cut at the end of the last bytes read.
    carry: Vec<u8>,
    /// Start of the unread part of `buf`.
    pos: usize,
    /// Bytes after `pos` known to hold no line terminator.
    scanned: usize,
    eof: bool,
    /// Lines read so far.
    line_no: u32,
    /// The level-0 line that starts the next record, and its number.
    pending: Option<(String, u32)>,
    line: String,
}

/// Appends `bytes` to `buf`, invalid UTF-8 replaced as
/// [`String::from_utf8_lossy`] replaces it, but for a character cut at the
/// end: its bytes are kept in `carry`, for the bytes that follow.
fn push_lossy(buf: &mut String, carry: &mut Vec<u8>, bytes: &[u8]) {
    if !carry.is_empty() {
        let mut joined = std::mem::take(carry);
        joined.extend_from_slice(bytes);
        return push_lossy(buf, carry, &joined);
    }
    let mut bytes = bytes;
    loop {
        match std::str::from_utf8(bytes) {
            Ok(text) => {
                buf.push_str(text);
                return;
            }
            Err(e) => {
                let (valid, rest) = bytes.split_at(e.valid_up_to());
                buf.push_str(std::str::from_utf8(valid).unwrap_or_default());
                match e.error_len() {
                    None => {
                        carry.extend_from_slice(rest);
                        return;
                    }
                    Some(n) => {
                        buf.push(char::REPLACEMENT_CHARACTER);
                        bytes = rest.get(n..).unwrap_or_default();
                    }
                }
            }
        }
    }
}

impl<R: BufRead> RecordSplitter<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            inner,
            buf: String::with_capacity(64 * 1024),
            carry: Vec::new(),
            pos: 0,
            scanned: 0,
            eof: false,
            line_no: 0,
            pending: None,
            line: String::new(),
        }
    }

    pub(crate) fn get_ref(&self) -> &R {
        &self.inner
    }

    /// Reads the next line, without its terminator, into `self.line`.
    fn next_line(&mut self) -> io::Result<bool> {
        self.line.clear();
        loop {
            let avail = self.buf.as_bytes().get(self.pos..).unwrap_or_default();
            let found = avail
                .get(self.scanned..)
                .and_then(find_eol)
                .map(|k| self.scanned + k);
            match found {
                Some(k) if self.eof || k + TERMINATOR_LOOKAHEAD <= avail.len() => {
                    let term = avail.get(k..).map_or(1, terminator_len);
                    // A terminator is ASCII: the line ends on a character.
                    self.line
                        .push_str(self.buf.get(self.pos..self.pos + k).unwrap_or_default());
                    self.pos += k + term;
                    self.scanned = 0;
                    return Ok(true);
                }
                Some(k) => self.scanned = k,
                None if self.eof => {
                    if avail.is_empty() {
                        return Ok(false);
                    }
                    self.line
                        .push_str(self.buf.get(self.pos..).unwrap_or_default());
                    self.pos = self.buf.len();
                    self.scanned = 0;
                    return Ok(true);
                }
                None => self.scanned = avail.len(),
            }
            self.fill()?;
        }
    }

    fn fill(&mut self) -> io::Result<()> {
        if self.pos > 0 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        loop {
            match self.inner.fill_buf() {
                Ok([]) => {
                    self.eof = true;
                    // A character cut by the end of the input.
                    if !self.carry.is_empty() {
                        self.carry.clear();
                        self.buf.push(char::REPLACEMENT_CHARACTER);
                    }
                    return Ok(());
                }
                Ok(chunk) => {
                    let len = chunk.len();
                    push_lossy(&mut self.buf, &mut self.carry, chunk);
                    self.inner.consume(len);
                    return Ok(());
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// Reads the next record into `record`; returns the line number of its
    /// first line, or `None` at the end of the input.
    pub(crate) fn next_record(&mut self, record: &mut String) -> io::Result<Option<u32>> {
        record.clear();
        let first = if let Some((line, number)) = self.pending.take() {
            record.push_str(&line);
            record.push('\n');
            self.line = line;
            number
        } else {
            loop {
                if !self.next_line()? {
                    return Ok(None);
                }
                self.line_no = self.line_no.saturating_add(1);
                if !matches!(lex_line(&self.line), Line::Blank | Line::LevelOnly) {
                    record.push_str(&self.line);
                    record.push('\n');
                    break self.line_no;
                }
            }
        };
        while self.next_line()? {
            self.line_no = self.line_no.saturating_add(1);
            if matches!(lex_line(&self.line), Line::Structure { level: 0, .. }) {
                self.pending = Some((std::mem::take(&mut self.line), self.line_no));
                break;
            }
            record.push_str(&self.line);
            record.push('\n');
        }
        Ok(Some(first))
    }
}

/// The records of a stream of GEDCOM bytes of any encoding, as texts, and
/// the version its first record declares: what [`TreeReader`] and the
/// [`GedcomStreamParser`](crate::GedcomStreamParser) read.
pub(crate) struct RecordSource<R> {
    splitter: RecordSplitter<DecodeReader<R>>,
    /// A record read ahead to find the version, and its first line.
    next: Option<(String, u32)>,
    escaping: Option<Escaping>,
    vers: Option<Box<str>>,
}

impl<R: BufRead> RecordSource<R> {
    /// Starts reading; this reads the first 64 KiB to choose the decoding.
    pub(crate) fn new(reader: R) -> io::Result<Self> {
        Ok(Self {
            splitter: RecordSplitter::new(DecodeReader::new(reader)?),
            next: None,
            escaping: None,
            vers: None,
        })
    }

    pub(crate) fn encoding(&self) -> GedcomEncoding {
        self.splitter.get_ref().encoding()
    }

    pub(crate) fn version(&self) -> GedcomVersion {
        self.vers
            .as_deref()
            .map_or(GedcomVersion::V5_5_1, GedcomVersion::from_version_str)
    }

    pub(crate) fn declared_version(&self) -> Option<&str> {
        self.vers.as_deref()
    }

    /// How the stream escapes `@`, once its first record is read.
    pub(crate) fn escaping(&self) -> Escaping {
        self.escaping.unwrap_or(Escaping::V551)
    }

    /// Reads the next record's text into `record`; the line number of its
    /// first line, or `None` at the end of the input.
    pub(crate) fn next_record(&mut self, record: &mut String) -> io::Result<Option<u32>> {
        let first_line = match self.next.take() {
            Some((text, line)) => {
                *record = text;
                line
            }
            None => match self.splitter.next_record(record)? {
                Some(line) => line,
                None => return Ok(None),
            },
        };
        if self.escaping.is_none() {
            // The version comes from the first record that starts with a
            // structure line. Stray lines before it form one record of their
            // own: read the next record to know how to read them.
            if starts_stray(record) {
                let mut next = String::new();
                if let Some(line) = self.splitter.next_record(&mut next)? {
                    let both = format!("{record}{next}");
                    self.vers = head_version(&both).map(Box::from);
                    self.next = Some((next, line));
                }
            } else {
                self.vers = head_version(record).map(Box::from);
            }
            self.escaping = Some(Escaping::of(self.vers.as_deref()));
        }
        Ok(Some(first_line))
    }
}

/// Reads GEDCOM bytes of any encoding as a stream of records, each a
/// lossless [`Structure`] tree, with memory bounded by the largest record.
///
/// It decodes with [`DecodeReader`] and reads with the same rules as
/// [`Tree`](super::Tree): the records it yields are those of
/// `Tree::from_bytes` on the same input, line numbers included.
///
/// ```rust
/// use oxidged::tree::TreeReader;
///
/// let bytes: &[u8] = b"0 HEAD\r\n1 GEDC\r\n2 VERS 5.5.1\r\n0 @N1@ NOTE a@@b\r\n1 CONC c\r\n0 TRLR\r\n";
/// let records = TreeReader::new(bytes)?.collect::<Result<Vec<_>, _>>()?;
/// assert_eq!(records[1].text(), Some("a@bc"));
/// assert_eq!(records[1].line, 4);
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct TreeReader<R> {
    source: RecordSource<R>,
    /// Records read already, to yield first.
    pending: VecDeque<Structure>,
    builder: Builder,
    tags: TagInterner,
    record: String,
}

impl<R: BufRead> TreeReader<R> {
    /// Starts reading; this reads the first 64 KiB to choose the decoding.
    ///
    /// # Errors
    ///
    /// Only the I/O errors of `reader`.
    pub fn new(reader: R) -> io::Result<Self> {
        Ok(Self::from_source(
            RecordSource::new(reader)?,
            VecDeque::new(),
        ))
    }

    /// Reads the rest of `source`, after the `pending` records.
    pub(crate) fn from_source(source: RecordSource<R>, pending: VecDeque<Structure>) -> Self {
        Self {
            source,
            pending,
            builder: Builder::new(Escaping::V551, 1),
            tags: TagInterner::default(),
            record: String::new(),
        }
    }

    /// The encoding the input is decoded with.
    #[must_use]
    pub fn encoding(&self) -> GedcomEncoding {
        self.source.encoding()
    }

    /// The version the file declares, once the record that tells it (the
    /// first one, normally `HEAD`) has been read. See [`Tree::version`].
    ///
    /// [`Tree::version`]: super::Tree::version
    #[must_use]
    pub fn version(&self) -> GedcomVersion {
        self.source.version()
    }

    /// The `HEAD.GEDC.VERS` payload as written, once read.
    #[must_use]
    pub fn declared_version(&self) -> Option<&str> {
        self.source.declared_version()
    }

    fn read_record(&mut self) -> io::Result<Option<Structure>> {
        if let Some(s) = self.pending.pop_front() {
            return Ok(Some(s));
        }
        let Some(first_line) = self.source.next_record(&mut self.record)? else {
            return Ok(None);
        };
        self.builder.reset(first_line);
        self.builder.set_escaping(self.source.escaping());
        self.builder.read(&self.record, &mut self.tags);
        let view = arena::View {
            text: &self.record,
            side: &self.builder.side,
            nodes: &self.builder.nodes,
            xrefs: &self.builder.xrefs,
            tags: &self.tags.others,
        };
        Ok(Some(view.to_structure(0)))
    }
}

impl<R: BufRead> Iterator for TreeReader<R> {
    type Item = io::Result<Structure>;

    fn next(&mut self) -> Option<io::Result<Structure>> {
        self.read_record().transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    fn records(text: &str, capacity: usize) -> Vec<(u32, String)> {
        let mut splitter = RecordSplitter::new(BufReader::with_capacity(capacity, text.as_bytes()));
        let mut out = Vec::new();
        let mut record = String::new();
        while let Some(line) = splitter.next_record(&mut record).unwrap() {
            out.push((line, record.clone()));
        }
        out
    }

    #[test]
    fn splits_like_the_line_iterator() {
        let text = "\r\n0 HEAD\r1 X\n\r\n0 @I1@ INDI\n\n1 NAME a\n\r0 TRLR";
        for capacity in [1, 2, 3, 5, 64] {
            assert_eq!(
                records(text, capacity),
                [
                    (2, "0 HEAD\n1 X\n\n".to_string()),
                    (5, "0 @I1@ INDI\n\n1 NAME a\n".to_string()),
                    (8, "0 TRLR\n".to_string())
                ],
                "capacity {capacity}"
            );
        }
    }

    /// Bytes that are not UTF-8 are replaced as `from_utf8_lossy` replaces
    /// them, wherever the reads cut them.
    #[test]
    fn invalid_bytes_replaced_whatever_the_reads() {
        let bytes: &[u8] =
            b"0 HEAD\n1 NOTE a\xC3\xA9b\xFFc\xE2\x82\n0 @I1@ INDI\n1 NAME \xF0\x9F\x98\x80\xF0\x9F";
        let mut lines = String::from_utf8_lossy(bytes).into_owned();
        lines.push('\n');
        for capacity in [1, 2, 3, 4, 64] {
            let mut splitter = RecordSplitter::new(BufReader::with_capacity(capacity, bytes));
            let mut all = String::new();
            let mut record = String::new();
            while splitter.next_record(&mut record).unwrap().is_some() {
                all.push_str(&record);
            }
            assert_eq!(all, lines, "capacity {capacity}");
        }
    }
}
