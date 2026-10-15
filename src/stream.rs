//! Reading a file one record at a time, with memory bounded by the largest
//! record.
//!
//! [`GedcomStreamParser`] reads GEDCOM bytes of any encoding from any
//! [`BufRead`], with the reading rules of the in-memory reader, and yields
//! each record typed, as a [`StreamedRecord`]: the [`Record`] and the
//! [`Store`] its texts point into, which holds that record's text alone.
//! [`GedcomStreamParser::nodes`] yields lossless structures instead, as
//! [`TreeReader`] does.
//!
//! ```rust
//! use oxidged::model::RecordRef;
//! use oxidged::GedcomStreamParser;
//!
//! # fn main() -> Result<(), oxidged::GedcomError> {
//! let file: &[u8] = b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 @F1@ FAM\n0 TRLR\n";
//! let mut names = Vec::new();
//! for record in GedcomStreamParser::new(file)? {
//!     let record = record?;
//!     if let RecordRef::Individual(indi) = record.record() {
//!         names.extend(indi.full_name());
//!     }
//! }
//! assert_eq!(names, ["Ann Example"]);
//! # Ok(())
//! # }
//! ```
//!
//! A dataset collects streamed records (`FromIterator`, `Extend`), so that
//! a stream can be filtered into a smaller dataset without reading the
//! whole file at once.
//!
//! A [`Visitor`] receives each record by its type and says whether to go
//! on ([`ControlFlow`]): [`GedcomStreamParser::visit`] stops at the first
//! [`ControlFlow::Break`], leaving the rest of the input unread.
//!
//! ```rust
//! use std::ops::ControlFlow;
//! use oxidged::model::{Individual, View};
//! use oxidged::stream::{GedcomStreamParser, StreamedRecord, Visitor};
//!
//! /// Finds the first individual named in a file.
//! struct FirstNamed;
//!
//! impl Visitor for FirstNamed {
//!     type Break = String;
//!     fn individual(&mut self, i: View<'_, Individual>, _: &StreamedRecord) -> ControlFlow<String> {
//!         match i.full_name() {
//!             Some(name) => ControlFlow::Break(name),
//!             None => ControlFlow::Continue(()),
//!         }
//!     }
//! }
//!
//! # fn main() -> Result<(), oxidged::GedcomError> {
//! let file: &[u8] = b"0 HEAD\n0 @I1@ INDI\n0 @I2@ INDI\n1 NAME Ann /Example/\n0 @I3@ INDI\n0 TRLR\n";
//! let mut parser = GedcomStreamParser::new(file)?;
//! assert_eq!(parser.visit(&mut FirstNamed)?, ControlFlow::Break("Ann Example".to_string()));
//! // The parser goes on from there.
//! assert_eq!(parser.count(), 1);
//! # Ok(())
//! # }
//! ```

use std::collections::VecDeque;
use std::io::BufRead;
use std::ops::ControlFlow;

use crate::GedcomError;
use crate::encoding::GedcomEncoding;
use crate::model::read::{Placed, RecordReader};
use crate::model::{
    Dataset, Family, HeaderRef, Individual, Multimedia, Node, Record, RecordRef, Repository,
    SharedNote, Source, Store, Submission, Submitter, View,
};
use crate::tree::{RecordSource, TreeReader};
use crate::version::GedcomVersion;

/// A record read by a [`GedcomStreamParser`]: a dataset of that record
/// alone, whose store holds its text.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamedRecord {
    data: Dataset,
    placed: Placed,
    line: u32,
    /// On a header, the `HEAD.GEDC.VERS` payload as written, as the parser
    /// read it (the typed header holds no `VERS` that has substructures).
    declared: Option<Box<str>>,
}

impl StreamedRecord {
    /// The record.
    #[must_use]
    pub fn record(&self) -> RecordRef<'_> {
        let record = match self.placed {
            Placed::Header => self.data.header_ref().map(RecordRef::Header),
            Placed::Record(kind, pos) => self.data.record_at(kind, pos),
        };
        match record {
            Some(r) => r,
            // The parser makes a streamed record of a record it placed.
            None => unreachable!("a streamed record holds its record"),
        }
    }

    /// The dataset of the record alone.
    #[must_use]
    pub fn dataset(&self) -> &Dataset {
        &self.data
    }

    /// The store the record's texts and identifiers resolve against.
    #[must_use]
    pub fn store(&self) -> &Store {
        self.data.store()
    }

    /// The version of the file.
    #[must_use]
    pub fn version(&self) -> GedcomVersion {
        self.data.version()
    }

    /// The line the record starts on.
    #[must_use]
    pub fn line(&self) -> u32 {
        self.line
    }

    /// The record, copied out as its owned type, and its store.
    #[must_use]
    pub fn into_parts(self) -> (Record, Store) {
        let record = self.record().into_owned();
        (record, self.data.store)
    }
}

impl AsRef<Store> for StreamedRecord {
    fn as_ref(&self) -> &Store {
        self.data.store()
    }
}

/// Reads GEDCOM bytes one record at a time: see the [module
/// documentation](self).
///
/// The input may be in any encoding the in-memory reader reads, with any
/// line terminators: it is decoded on the fly ([`DecodeReader`]), and each
/// record is read by the same rules and types as [`Dataset::parse`] reads
/// it. Memory is bounded by the largest record and a 64 KiB buffer.
///
/// Reading fails only when the input does: an I/O error ends the stream.
///
/// [`DecodeReader`]: crate::encoding::DecodeReader
pub struct GedcomStreamParser<R> {
    source: RecordSource<R>,
    reader: RecordReader,
    /// Records read from the last text and not yet yielded, with the line
    /// of that text.
    pending: VecDeque<(Placed, u32)>,
    /// The dataset of the pending records.
    data: Option<Dataset>,
    done: bool,
}

impl<R: BufRead> GedcomStreamParser<R> {
    /// Starts reading; this reads the first 64 KiB to choose the decoding.
    ///
    /// # Errors
    ///
    /// [`GedcomError::Io`] when `reader` fails.
    pub fn new(reader: R) -> Result<Self, GedcomError> {
        Ok(Self {
            source: RecordSource::new(reader)?,
            reader: RecordReader::new(),
            pending: VecDeque::new(),
            data: None,
            done: false,
        })
    }

    /// The encoding the input is decoded with.
    #[must_use]
    pub fn encoding(&self) -> GedcomEncoding {
        self.source.encoding()
    }

    /// The version the file declares, once the record that tells it (the
    /// first one, normally `HEAD`) has been read.
    #[must_use]
    pub fn version(&self) -> GedcomVersion {
        self.source.version()
    }

    /// The `HEAD.GEDC.VERS` payload as written, once read.
    #[must_use]
    pub fn declared_version(&self) -> Option<&str> {
        self.source.declared_version()
    }

    /// The records not read yet, as lossless structures (see
    /// [`TreeReader`]) instead of typed records.
    #[must_use]
    pub fn nodes(self) -> TreeReader<R> {
        let version = self.source.version();
        let pending = match &self.data {
            Some(data) => self
                .pending
                .iter()
                .filter_map(|(placed, _)| match *placed {
                    Placed::Header => data.header_ref().map(RecordRef::Header),
                    Placed::Record(kind, pos) => data.record_at(kind, pos),
                })
                .map(|r| r.to_structure_in(version))
                .collect(),
            None => VecDeque::new(),
        };
        TreeReader::from_source(self.source, pending)
    }

    fn read_next(&mut self) -> Result<Option<StreamedRecord>, GedcomError> {
        loop {
            if let Some((placed, line)) = self.pending.pop_front() {
                // The last record of a text takes its dataset; the others,
                // seldom any, a copy.
                let mut data = if self.pending.is_empty() {
                    self.data.take().unwrap_or_default()
                } else {
                    self.data.clone().unwrap_or_default()
                };
                data.version = self.source.version();
                let declared = match placed {
                    Placed::Header => self.source.declared_version().map(Into::into),
                    Placed::Record(..) => None,
                };
                return Ok(Some(StreamedRecord {
                    data,
                    placed,
                    line,
                    declared,
                }));
            }
            let mut text = String::new();
            let Some(line) = self.source.next_record(&mut text)? else {
                return Ok(None);
            };
            self.reader
                .set_version(self.source.version(), self.source.escaping());
            let mut records = Vec::new();
            let data = self.reader.read(text, line, &mut records);
            self.pending.extend(records.into_iter().map(|r| (r, line)));
            self.data = Some(data);
        }
    }
}

/// Receives the records of a stream ([`GedcomStreamParser::visit`]) by
/// their type, and says whether to go on.
///
/// Every method has a default that goes on, so that a visitor implements
/// the records it wants; [`record`](Self::record) dispatches each record to
/// the method of its type, and can be implemented instead to see them all.
/// Each method also receives the [`StreamedRecord`], whose store the views
/// resolve against.
pub trait Visitor {
    /// What a visit stops with.
    type Break;

    /// Any record: dispatches it to the method of its type.
    fn record(&mut self, record: &StreamedRecord) -> ControlFlow<Self::Break> {
        match record.record() {
            RecordRef::Header(h) => self.header(h, record),
            RecordRef::Individual(r) => self.individual(r, record),
            RecordRef::Family(r) => self.family(r, record),
            RecordRef::Source(r) => self.source(r, record),
            RecordRef::Repository(r) => self.repository(r, record),
            RecordRef::Multimedia(r) => self.multimedia(r, record),
            RecordRef::Submitter(r) => self.submitter(r, record),
            RecordRef::Submission(r) => self.submission(r, record),
            RecordRef::Note(r) => self.note(r, record),
            RecordRef::Other(r) => self.other(r, record),
        }
    }

    /// The header.
    fn header(
        &mut self,
        _header: HeaderRef<'_>,
        _record: &StreamedRecord,
    ) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }

    /// An individual.
    fn individual(
        &mut self,
        _individual: View<'_, Individual>,
        _record: &StreamedRecord,
    ) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }

    /// A family.
    fn family(
        &mut self,
        _family: View<'_, Family>,
        _record: &StreamedRecord,
    ) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }

    /// A source.
    fn source(
        &mut self,
        _source: View<'_, Source>,
        _record: &StreamedRecord,
    ) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }

    /// A repository.
    fn repository(
        &mut self,
        _repository: View<'_, Repository>,
        _record: &StreamedRecord,
    ) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }

    /// A multimedia record.
    fn multimedia(
        &mut self,
        _multimedia: View<'_, Multimedia>,
        _record: &StreamedRecord,
    ) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }

    /// A submitter.
    fn submitter(
        &mut self,
        _submitter: View<'_, Submitter>,
        _record: &StreamedRecord,
    ) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }

    /// A 5.5.1 submission.
    fn submission(
        &mut self,
        _submission: View<'_, Submission>,
        _record: &StreamedRecord,
    ) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }

    /// A shared note.
    fn note(
        &mut self,
        _note: View<'_, SharedNote>,
        _record: &StreamedRecord,
    ) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }

    /// A record of no type the model has (an extension record).
    fn other(
        &mut self,
        _record_node: View<'_, Node>,
        _record: &StreamedRecord,
    ) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
}

impl<R: BufRead> GedcomStreamParser<R> {
    /// Gives `visitor` every record not read yet, in file order, until it
    /// breaks: what it broke with, or [`ControlFlow::Continue`] at the end
    /// of the input. The parser can go on after a break.
    ///
    /// # Errors
    ///
    /// [`GedcomError::Io`] when the input fails.
    pub fn visit<V: Visitor + ?Sized>(
        &mut self,
        visitor: &mut V,
    ) -> Result<ControlFlow<V::Break>, GedcomError> {
        for record in self.by_ref() {
            let record = record?;
            if let ControlFlow::Break(b) = visitor.record(&record) {
                return Ok(ControlFlow::Break(b));
            }
        }
        Ok(ControlFlow::Continue(()))
    }
}

impl<R: BufRead> Iterator for GedcomStreamParser<R> {
    type Item = Result<StreamedRecord, GedcomError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let next = self.read_next().transpose();
        if !matches!(next, Some(Ok(_))) {
            self.done = true;
        }
        next
    }
}

impl Extend<StreamedRecord> for Dataset {
    /// Adds streamed records to the dataset: their texts move into its
    /// store. The first header sets the dataset's version (an empty
    /// dataset's) and becomes its header; another is kept in `extra`.
    fn extend<I: IntoIterator<Item = StreamedRecord>>(&mut self, iter: I) {
        for streamed in iter {
            let StreamedRecord {
                data,
                placed,
                declared,
                ..
            } = streamed;
            self.adopt(data, placed, declared);
        }
    }
}

impl FromIterator<StreamedRecord> for Dataset {
    fn from_iter<I: IntoIterator<Item = StreamedRecord>>(iter: I) -> Self {
        let mut data = Dataset::default();
        data.extend(iter);
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RecordRef;

    const FILE: &str = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 NOTE a@@b\n2 CONC c\n1 _X y\n0 @F1@ FAM\n1 HUSB @I1@\n0 _LOC Here\n0 TRLR\n";

    fn stream(bytes: &[u8]) -> Vec<StreamedRecord> {
        GedcomStreamParser::new(bytes)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn records_typed_one_at_a_time() {
        let records = stream(FILE.as_bytes());
        assert_eq!(records.len(), 4);
        assert!(matches!(records[0].record(), RecordRef::Header(_)));
        let RecordRef::Individual(indi) = records[1].record() else {
            panic!()
        };
        assert_eq!(records[1].line(), 4);
        assert_eq!(indi.full_name().as_deref(), Some("Ann Example"));
        assert_eq!(indi.extra().first().unwrap().tag(), "_X");
        assert!(matches!(records[3].record(), RecordRef::Other(_)));
    }

    #[test]
    fn collected_equals_read_in_memory() {
        let whole = Dataset::parse(FILE);
        let collected: Dataset = stream(FILE.as_bytes()).into_iter().collect();
        assert_eq!(collected.to_structures(), whole.to_structures());
        assert_eq!(collected.version(), whole.version());
        assert_eq!(collected.declared_version(), Some("5.5.1"));
        let indi = collected.find_individual("@I1@").unwrap();
        assert_eq!(collected.families_as_spouse(indi.xref()).count(), 1);
    }

    #[test]
    fn a_version_with_substructures_is_declared_as_in_memory() {
        // A `VERS` with an extension under it is no typed version: the
        // declared one is what the parser read, as in memory.
        let text = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n3 _X Note on the version\n2 FORM LINEAGE-LINKED\n0 TRLR\n";
        let whole = Dataset::parse(text);
        let collected: Dataset = stream(text.as_bytes()).into_iter().collect();
        assert_eq!(whole.declared_version(), Some("5.5.1"));
        assert_eq!(collected.declared_version(), whole.declared_version());
        assert_eq!(collected.to_structures(), whole.to_structures());
    }

    #[test]
    fn any_encoding_and_terminator() {
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(
                FILE.replace('\n', "\r")
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes),
            )
            .collect();
        let records = stream(&utf16);
        assert_eq!(records.len(), 4);
        let RecordRef::Individual(indi) = records[1].record() else {
            panic!()
        };
        assert_eq!(indi.notes().len(), 1);
    }

    #[test]
    fn nodes_after_typed_records() {
        let mut parser = GedcomStreamParser::new(FILE.as_bytes()).unwrap();
        assert!(parser.next().is_some());
        let rest: Vec<_> = parser.nodes().collect::<Result<_, _>>().unwrap();
        let tags: Vec<_> = rest.iter().map(|s| s.tag.as_str().to_string()).collect();
        assert_eq!(tags, ["INDI", "FAM", "_LOC", "TRLR"]);
        assert_eq!(rest[0].first("NOTE").unwrap().text(), Some("a@bc"));
    }

    #[test]
    fn empty_and_trailer_only() {
        assert!(stream(b"").is_empty());
        assert!(stream(b"0 TRLR\n").is_empty());
    }

    #[test]
    fn io_errors_end_the_stream() {
        struct Failing;
        impl std::io::Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("broken"))
            }
        }
        let parser = GedcomStreamParser::new(std::io::BufReader::new(Failing));
        assert!(matches!(parser, Err(GedcomError::Io(_))));
    }
}
