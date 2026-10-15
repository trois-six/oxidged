//! Writing a file one record at a time: [`GedcomStreamWriter`].

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::rc::Rc;

use super::emit::{LineSink, candidate_xref, is_valid_pointer, new_xref};
use super::head;
use super::xref::{XrefIndex, numbered_xref};
use super::{
    Credentials, Emitter, GedcomWriter, Pointers, Repair, RepairKind, RepairPolicy, WriteError,
    WriteReport, WriterConfig, drop_credentials, holds_credentials, map_pointers, put_flat,
    put_record, put_structure, record_prefix,
};
use crate::convert::{Converter, Entry, Fate, Fresh, Out, Watch, built_structure};
use crate::model::driver::{ToNodes, WriteCx};
use crate::model::write::Built;
use crate::model::{Dataset, Header};
use crate::model::{RecordRef, Store};
use crate::spec::conform::streamed::{Outcome, Streamed};
use crate::spec::conform::{FieldOrder, model_order};
use crate::tree::{PayloadRef, Structure, Tag, Xref};
use crate::version::{GedcomVersion, VersionRules, XrefGrammar};

/// The identifier of the stub submitter a 5.5.1 header without one points
/// to.
const STUB: &str = "@U1@";

/// Writes GEDCOM records one at a time, with memory bounded by one record.
///
/// [`GedcomStreamWriter`] writes the header, then each record as it is
/// given, then the trailer, with the conformance repair and the line
/// emitter of [`GedcomWriter`]: every line it writes follows the
/// version's grammar, and every record the version's structure rules. It
/// keeps one record and a block of output at a time, and the identifiers
/// of the records it wrote (16 bytes each for nearly all), so that they
/// stay unique.
///
/// # Differences from [`GedcomWriter`]
///
/// Records are written in the order they are given; [`GedcomWriter`]
/// writes a [`Dataset`] by record type. Given the records of a dataset in
/// the order [`Dataset::records`] yields them, the output is the one
/// [`GedcomWriter`] writes, except where knowing the records to come would
/// change it:
///
/// - **Identifiers.** A record identifier valid in the version and not
///   written yet is kept. An invalid one is written as the valid form
///   [`GedcomWriter`] gives it first (`@i1@` as `@I1@` in 7.x), with a
///   `_2`, `_3`… suffix when that form is taken, and so is a repeated
///   one; a standard record without one gets the first free `@I1@`,
///   `@F1@`…. Free means: not written yet, and not used in the store the
///   record comes with (a dataset's has every identifier of the dataset,
///   a [`StreamedRecord`](crate::StreamedRecord)'s those of the record),
///   as a record's or a pointer's. A later record holding that identifier
///   is then renamed, where [`GedcomWriter`], knowing every identifier
///   first, renames the earlier one.
/// - **Pointers.** A pointer is written as it is when it is valid, and in
///   the valid form of its identifier otherwise (the form a record with
///   that identifier gets, when no suffix was needed): pointers keep
///   naming the records they named. A pointer is taken to name a record
///   of the type its structure points to — that record may come later —
///   so a dangling pointer, or one to a record of another type, is not
///   repaired.
/// - **Header.** The header is written first, completed for the version:
///   a 5.5.1 header without `SUBM` points to a stub submitter `@U1@`,
///   written right after it ([`GedcomWriter`] points to the first
///   submitter of the dataset, when there is one). A structure that a
///   repair relocates in 7.x becomes a plain extension structure, the
///   header being written, where [`GedcomWriter`] declares its tag in
///   `HEAD.SCHMA`.
/// - **Large records.** The notes moved out of a 5.5.1 record over 32K
///   go to `NOTE` records written right after it.
/// - **Conversion.** Records of the other version family (5.5.1 written
///   as 7.x, or back) are converted as [`GedcomWriter`] converts them
///   ([`crate::convert`]), each found to need it as it is written. The
///   header, written first, cannot know which extension tags of known
///   meaning the records will use: converted to 7.x, it declares them all
///   in `HEAD.SCHMA` (at most nine `TAG` lines). The records a conversion
///   takes out of another (multimedia records of inline `OBJE`, source
///   records of `SOUR` citations in words) are written right after it,
///   with identifiers not written yet ([`GedcomWriter`] writes them after
///   the records of their type). A 5.5.1 record named `@VOID@` takes
///   another identifier, as do the pointers to it written after it; those
///   written before it stay `@VOID@`, which in 7.x names nothing.
///
/// A record that a repair or a conversion makes an extension record is
/// kept until the end and written before the trailer, where
/// [`GedcomWriter`] writes it too: such records (rare: a record type the
/// version does not have, a record that cannot be repaired otherwise, a
/// 5.5.1 submission kept in 7.x) are the only ones held.
///
/// Under [`RepairPolicy::Error`], the first repair is an error as soon as
/// it is found; what was written before stays written.
///
/// # Example
///
/// ```rust
/// use oxidged::{GedcomStreamParser, GedcomStreamWriter, GedcomVersion, WriterConfig};
/// use oxidged::model::RecordRef;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let file: &[u8] = b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 SUBM @U1@\n0 @U1@ SUBM\n1 NAME Ann\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n";
/// let mut records = GedcomStreamParser::new(file)?;
/// let head = records.next().unwrap()?;
/// let RecordRef::Header(header) = head.record() else { unreachable!() };
/// let config = WriterConfig { version: Some(GedcomVersion::V7_0), ..WriterConfig::default() };
/// let mut writer = GedcomStreamWriter::new(Vec::new(), config, header.header(), &head)?;
/// for record in records {
///     let record = record?;
///     writer.write_record(record.record())?;
/// }
/// let (bytes, report) = writer.finish()?;
/// let text = String::from_utf8(bytes)?;
/// assert!(text.contains("0 @I1@ INDI\n1 NAME Ann /Example/\n"));
/// assert!(text.ends_with("0 TRLR\n"));
/// assert!(report.repairs.is_empty());
/// # Ok(())
/// # }
/// ```
pub struct GedcomStreamWriter<W: io::Write> {
    writer: W,
    policy: RepairPolicy,
    rules: &'static VersionRules,
    /// The output not handed to `writer` yet, at most a block and a record.
    sink: LineSink<'static>,
    conform: Streamed,
    xrefs: StreamXrefs,
    /// The records a repair made extension records, with the identifiers
    /// they are written with: written last, as [`GedcomWriter`] writes
    /// them.
    held: Vec<(Structure, Option<String>)>,
    /// The conversion of records of the other version family.
    convert: Option<StreamConversion>,
    /// The records the conversion made extension records, with their
    /// identifiers: repaired and written last, as [`GedcomWriter`] writes
    /// them.
    held_converted: Vec<(Structure, Option<String>)>,
    /// Whether credentials are written ([`Credentials`]).
    credentials: Credentials,
}

/// The conversion of the records a [`GedcomStreamWriter`] writes, when they
/// are of the other version family than the output ([`crate::convert`]).
struct StreamConversion {
    conv: Converter,
    /// What it looks for in the records written as they are.
    watch: Rc<Watch>,
    order: &'static FieldOrder,
    target: GedcomVersion,
}

impl<W: io::Write> std::fmt::Debug for GedcomStreamWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GedcomStreamWriter")
            .field("version", &self.rules.version())
            .finish_non_exhaustive()
    }
}

impl<W: io::Write> GedcomStreamWriter<W> {
    /// Starts writing to `writer` with `config`: writes the header (whose
    /// texts and identifiers resolve against `store`), completed and
    /// repaired for the version. The version is the configured one, else
    /// the one the header declares, else 5.5.1; to write a dataset in its
    /// own version whatever its header declares, start with
    /// [`for_dataset`](Self::for_dataset).
    ///
    /// # Errors
    ///
    /// [`WriteError::Io`] when `writer` fails, [`WriteError::Unencodable`]
    /// when the output encoding cannot represent the header, and
    /// [`WriteError::NonConformant`] on a repair under
    /// [`RepairPolicy::Error`].
    pub fn new<S: AsRef<Store> + ?Sized>(
        writer: W,
        config: WriterConfig,
        header: &Header,
        store: &S,
    ) -> Result<Self, WriteError> {
        let store = store.as_ref();
        let from = header
            .declared_version(store)
            .map_or_else(GedcomVersion::default, |v| {
                GedcomVersion::from_version_str(&v)
            });
        Self::start(writer, config, header, store, from)
    }

    /// Starts writing `data` to `writer` with `config`: writes its header
    /// (an empty one when it has none), completed and repaired for the
    /// version, as [`new`](Self::new) does. The version is the configured
    /// one, else the dataset's own ([`Dataset::version`]), the version
    /// [`GedcomWriter`] writes it in. Its records follow, given to
    /// [`write_record`](Self::write_record) but for the header (the first
    /// that [`Dataset::records`] yields, when it has one). A stream read by
    /// a [`GedcomStreamParser`](crate::GedcomStreamParser) starts so with
    /// the [`dataset`](crate::StreamedRecord::dataset) of its header, then
    /// writes the records that follow it.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn for_dataset(
        writer: W,
        mut config: WriterConfig,
        data: &Dataset,
    ) -> Result<Self, WriteError> {
        config.version.get_or_insert(data.version());
        let default = Header::default();
        let header = data.header().unwrap_or(&default);
        Self::start(writer, config, header, data.store(), data.version())
    }

    /// [`new`](Self::new), the records being of version `from` (what the
    /// output is converted from, when it is of the other family).
    fn start(
        writer: W,
        config: WriterConfig,
        header: &Header,
        store: &Store,
        from: GedcomVersion,
    ) -> Result<Self, WriteError> {
        let policy = config.on_nonconformant;
        let credentials = config.credentials;
        let gw = GedcomWriter::with_config(config);
        let rules = gw.config().version.unwrap_or(from).rules();
        let encoding = gw.encoding(rules);
        let mut sink = gw.sink(None, rules, encoding, policy);
        if gw.wants_bom(rules, encoding, false) {
            sink.bom();
        }
        let cx = WriteCx::owned(store, rules.version, true);
        let mut head = header.to_node("HEAD", &cx);
        let convert =
            Converter::new(from, rules.version, gw.config().conversion, false).map(|mut conv| {
                conv.read_head(&head);
                // The records to come may use any extension tag of known
                // meaning: the header declares them all.
                let known = conv.declarable();
                let order = crate::model::field_order(rules.version);
                if conv.head(&mut head, &known) {
                    model_order(&mut head, rules.version, order);
                }
                StreamConversion {
                    watch: Rc::new(Watch::new(conv.clone())),
                    conv,
                    order,
                    target: rules.version,
                }
            });
        let mut stubbed = false;
        let conform = Streamed::header(rules.version, &mut head, STUB, &mut stubbed);
        let mut this = Self {
            writer,
            policy,
            rules,
            sink,
            conform,
            xrefs: StreamXrefs::new(rules),
            held: Vec::new(),
            convert,
            held_converted: Vec::new(),
            credentials,
        };
        this.repairs_since(0)?;
        let stub = stubbed.then(|| {
            this.xrefs.take(STUB);
            head::stub_submitter(STUB.to_string())
        });
        map_pointers(&mut head, &mut this.xrefs);
        let charset = this.sink.charset();
        head::complete(&mut head, rules, charset, None);
        let mut out = Emitter {
            rules,
            sink: &mut this.sink,
        };
        put_structure(&mut out, &head)?;
        if let Some(stub) = stub {
            put_structure(&mut out, &stub)?;
        }
        this.sink.drain_to(&mut this.writer, false)?;
        Ok(this)
    }

    /// Writes `record` (of a [`StreamedRecord`](crate::StreamedRecord), or
    /// of any dataset), repaired for the version when it needs it. A
    /// header is written as a `_HEAD` extension record, a trailer with
    /// content as `_TRLR`; an empty trailer is left out, the writer writing
    /// its own.
    ///
    /// # Errors
    ///
    /// As [`GedcomStreamWriter::new`].
    pub fn write_record(&mut self, record: RecordRef<'_>) -> Result<(), WriteError> {
        let cx = WriteCx::of(record.dataset(), self.rules.version, true);
        let mark = self.conform.repairs.len();
        let credentials = self.credentials == Credentials::LeftOut
            && holds_credentials(record.dataset().store().other_tags());
        match record {
            RecordRef::Header(_) => {
                let mut s = record.to_structure(&cx);
                if credentials {
                    drop_credentials(&mut s);
                }
                s.tag = Tag::new("_HEAD");
                self.conform.repairs.push(Repair::new(
                    0,
                    RepairKind::Header,
                    "a second HEAD written as a _HEAD record",
                ));
                self.repairs_since(mark)?;
                self.put_owned(&s, &|x| cx.store.find_xref(x).is_some())?;
            }
            RecordRef::Other(n) if n.tag() == "TRLR" => {
                if !record.is_empty() || n.xref().is_some() {
                    let mut s = record.to_structure(&cx);
                    if credentials {
                        drop_credentials(&mut s);
                    }
                    s.tag = Tag::new("_TRLR");
                    s.xref = None;
                    self.conform.repairs.push(Repair::new(
                        0,
                        RepairKind::Header,
                        "a TRLR with content written as a _TRLR record",
                    ));
                    self.repairs_since(mark)?;
                    self.put_owned(&s, &|x| cx.store.find_xref(x).is_some())?;
                }
            }
            _ if credentials && self.wrote_without_credentials(record)? => {}
            _ => {
                let own = record.xref().map(|id| cx.store.xref(id));
                let tag = record.tag(self.rules.version);
                let uses = |x: &str| cx.store.find_xref(x).is_some();
                let xref = self.record_xref(own, tag, &uses)?;
                let mut converts = false;
                if let Some(c) = &mut self.convert {
                    if let (true, Some(x)) = (c.conv.names_void(own), &xref) {
                        // Pointers to it from here on take its name.
                        c.conv.rename_void(x.as_str().into());
                        c.watch = Rc::new(Watch::new(c.conv.clone()));
                    }
                    converts = c.watch.record(tag, own);
                }
                let built = Built::new(record, self.rules.version);
                if !converts {
                    let xrefs = &mut self.xrefs;
                    let outcome = self.conform.record(
                        &built,
                        xref.as_deref().map(|x| Cow::Owned(x.to_string())),
                        &mut self.sink,
                        &mut || xrefs.unique("@N1@", &uses),
                        self.convert.as_ref().map(|c| &c.watch),
                    );
                    self.repairs_since(mark)?;
                    if !matches!(outcome, Outcome::Convert) {
                        self.put_outcome(outcome, xref, tag)?;
                        return self.sink.drain_to(&mut self.writer, false);
                    }
                }
                self.convert_record(&built, xref, tag, &uses)?;
            }
        }
        self.sink.drain_to(&mut self.writer, false)
    }

    /// Writes `record` without the credentials it holds
    /// ([`Credentials`]), read again from its text without
    /// them; `false` (nothing written) when it holds none.
    fn wrote_without_credentials(&mut self, record: RecordRef<'_>) -> Result<bool, WriteError> {
        let version = record.dataset().version();
        let mut s = record.to_structure(&WriteCx::of(record.dataset(), version, true));
        if drop_credentials(&mut s) == 0 {
            return Ok(false);
        }
        let mut text = format!("0 HEAD\n1 GEDC\n2 VERS {}\n", version.as_str());
        s.write_gedcom(&mut text, 0, version);
        let without = crate::Dataset::parse(text);
        for r in without.records() {
            if !matches!(r, RecordRef::Header(_)) {
                self.write_record(r)?;
            }
        }
        Ok(true)
    }

    /// Converts `built`, a record tagged `tag` written with the identifier
    /// `xref`, and writes it, then the records the conversion takes out of
    /// it (multimedia and source records), right after it; their
    /// identifiers are none its store `uses`.
    fn convert_record(
        &mut self,
        built: &Built<'_>,
        xref: Option<String>,
        tag: &str,
        uses: &dyn Fn(&str) -> bool,
    ) -> Result<(), WriteError> {
        let Some(c) = &self.convert else {
            return Ok(());
        };
        let mut s = built_structure(built);
        let mut out = Out::default();
        let mut fresh = StreamFresh(&mut self.xrefs, uses);
        if c.conv.record(&mut s, &mut fresh, &mut out) == Fate::Remove {
            return Ok(());
        }
        model_order(&mut s, c.target, c.order);
        let mut added = std::mem::take(&mut out.records);
        for r in &mut added {
            model_order(r, c.target, c.order);
        }
        if !tag.starts_with('_') && s.tag.as_str().starts_with('_') {
            // An extension record now, written last.
            self.held_converted.push((s, xref));
        } else {
            self.put_converted(&s, xref, uses)?;
        }
        for r in &added {
            let xref = r.xref.as_ref().map(|x| x.as_str().to_string());
            self.put_converted(r, xref, uses)?;
        }
        Ok(())
    }

    /// Writes `s`, a record converted, with the identifier `xref`,
    /// repaired as any record; the identifiers of the notes a repair moves
    /// out of it are none its store `uses`.
    fn put_converted(
        &mut self,
        s: &Structure,
        xref: Option<String>,
        uses: &dyn Fn(&str) -> bool,
    ) -> Result<(), WriteError> {
        let mark = self.conform.repairs.len();
        let xrefs = &mut self.xrefs;
        let entry = Entry::<Built<'_>>::Owned(s);
        let outcome = self.conform.record(
            &entry,
            xref.as_deref().map(|x| Cow::Owned(x.to_string())),
            &mut self.sink,
            &mut || xrefs.unique("@N1@", uses),
            None,
        );
        self.repairs_since(mark)?;
        self.put_outcome(outcome, xref, s.tag.as_str())
    }

    /// Writes a record tagged `tag` as [`Streamed::record`] left it, with
    /// the identifier `xref`; one a repair made an extension record is
    /// held until the end.
    fn put_outcome(
        &mut self,
        outcome: Outcome<'_>,
        xref: Option<String>,
        tag: &str,
    ) -> Result<(), WriteError> {
        match outcome {
            // At the end of the sink already; a record to convert is
            // converted first, when a conversion is looked for.
            Outcome::Lines | Outcome::Convert => {}
            Outcome::Clean(flat) => {
                if let Some(root) = flat.root() {
                    let mut out = Emitter {
                        rules: self.rules,
                        sink: &mut self.sink,
                    };
                    put_flat(&mut out, &mut self.xrefs, xref.as_deref(), root)?;
                }
            }
            Outcome::Repaired(s, notes) => {
                if s.tag.as_str().starts_with('_') && !tag.starts_with('_') {
                    self.held.push((s, xref));
                    return Ok(());
                }
                self.put_with(&s, xref.as_deref())?;
                for note in &notes {
                    let xref = note.xref.as_ref().map(Xref::as_str);
                    self.put_with(note, xref)?;
                }
            }
        }
        Ok(())
    }

    /// Writes the trailer, hands every byte to the writer and flushes it.
    /// Returns the writer and the repairs made, those of the structures
    /// first, then those of the lines.
    ///
    /// # Errors
    ///
    /// [`WriteError::Io`] when the writer fails.
    pub fn finish(mut self) -> Result<(W, WriteReport), WriteError> {
        // Their stores are gone: what was written is all that is known.
        for (s, xref) in std::mem::take(&mut self.held_converted) {
            self.put_converted(&s, xref, &|_| false)?;
        }
        for (s, xref) in std::mem::take(&mut self.held) {
            self.put_with(&s, xref.as_deref())?;
        }
        let mut out = Emitter {
            rules: self.rules,
            sink: &mut self.sink,
        };
        out.sink.source_line = 0;
        out.put(0, None, "TRLR", PayloadRef::None)?;
        self.sink.drain_to(&mut self.writer, true)?;
        self.writer.flush()?;
        let mut repairs = std::mem::take(&mut self.conform.repairs);
        repairs.append(&mut self.sink.repairs);
        Ok((self.writer, WriteReport { repairs }))
    }

    /// Fails with the first repair made since `mark` under
    /// [`RepairPolicy::Error`].
    fn repairs_since(&self, mark: usize) -> Result<(), WriteError> {
        match self.conform.repairs.get(mark..).and_then(<[_]>::first) {
            Some(first) if self.policy == RepairPolicy::Error => {
                Err(WriteError::NonConformant(Box::new(first.clone())))
            }
            _ => Ok(()),
        }
    }

    /// The identifier written for a record with the identifier `xref` and
    /// the tag `tag`, the rename reported; a new one is none the record's
    /// store `uses`.
    fn record_xref(
        &mut self,
        xref: Option<&str>,
        tag: &str,
        uses: &dyn Fn(&str) -> bool,
    ) -> Result<Option<String>, WriteError> {
        let (new, repair) = self.xrefs.record(xref, record_prefix(tag), uses);
        if let Some(repair) = repair {
            self.sink.repair(repair)?;
        }
        Ok(new)
    }

    /// Writes an owned record, its identifier made by the writer, none
    /// its store `uses`.
    fn put_owned(&mut self, s: &Structure, uses: &dyn Fn(&str) -> bool) -> Result<(), WriteError> {
        let xref = self.record_xref(s.xref.as_ref().map(Xref::as_str), s.tag.as_str(), uses)?;
        self.put_with(s, xref.as_deref())
    }

    /// Writes an owned record with the identifier `xref`, which the writer
    /// made already.
    fn put_with(&mut self, s: &Structure, xref: Option<&str>) -> Result<(), WriteError> {
        let mut out = Emitter {
            rules: self.rules,
            sink: &mut self.sink,
        };
        put_record(&mut out, &mut self.xrefs, xref, s, &mut Vec::new())
    }
}

/// The identifiers of the records written one at a time: see
/// [`GedcomStreamWriter`].
pub(crate) struct StreamXrefs {
    rules: &'static VersionRules,
    /// Every record identifier written.
    written: XrefIndex<'static, ()>,
    /// The identifier each invalid identifier was first written as.
    renamed: HashMap<Box<str>, Box<str>>,
    /// The last number given to records without an identifier, by prefix.
    counters: HashMap<Box<str>, usize>,
}

impl StreamXrefs {
    fn new(rules: &'static VersionRules) -> Self {
        Self {
            rules,
            written: XrefIndex::default(),
            renamed: HashMap::new(),
            counters: HashMap::new(),
        }
    }

    /// Notes `xref` as written.
    fn take(&mut self, xref: &str) {
        self.written.insert(Cow::Owned(xref.to_string()), ());
    }

    /// `candidate`, or `candidate` with the first `_n` suffix not written
    /// and not one the store `uses`: taken.
    fn unique(&mut self, candidate: &str, uses: &dyn Fn(&str) -> bool) -> String {
        let mut new = candidate.to_string();
        let mut n = 1_usize;
        while self.written.contains_key(&new) || uses(&new) {
            n += 1;
            new = numbered_xref(self.rules, candidate, n);
        }
        self.take(&new);
        new
    }

    /// The first `@<prefix>n@` not written and not one the store `uses`:
    /// taken.
    pub(crate) fn fresh(&mut self, prefix: &str, uses: &dyn Fn(&str) -> bool) -> String {
        let n = self.counters.entry(prefix.into()).or_insert(0);
        let mut next = *n;
        let new = loop {
            next += 1;
            let candidate = format!("@{prefix}{next}@");
            if !self.written.contains_key(&candidate) && !uses(&candidate) {
                break candidate;
            }
        };
        *n = next;
        self.take(&new);
        new
    }

    /// The identifier of a record with the identifier `xref`, of a kind
    /// whose generated identifiers start with `prefix`: kept when it is
    /// valid and not written yet, renamed otherwise (with the repair) to
    /// one the store does not use.
    fn record(
        &mut self,
        xref: Option<&str>,
        prefix: Option<&'static str>,
        uses: &dyn Fn(&str) -> bool,
    ) -> (Option<String>, Option<Repair>) {
        let Some(x) = xref else {
            return (prefix.map(|p| self.fresh(p, uses)), None);
        };
        let valid = self.rules.is_valid_xref(x);
        if valid && !self.written.contains_key(x) {
            self.take(x);
            return (Some(x.to_string()), None);
        }
        let new = self.unique(&new_xref(self.rules, x), uses);
        if !valid && !self.renamed.contains_key(x) {
            self.renamed.insert(x.into(), new.as_str().into());
        }
        let repair = Repair::new(
            0,
            RepairKind::Xref,
            format!("identifier {x} written as {new}"),
        );
        (Some(new), Some(repair))
    }
}

/// The identifiers of the records a conversion adds, the first not
/// written and not used in the store of the record converted
/// ([`StreamXrefs::fresh`]).
struct StreamFresh<'x>(&'x mut StreamXrefs, &'x dyn Fn(&str) -> bool);

impl Fresh for StreamFresh<'_> {
    fn fresh(&mut self, prefix: &str) -> Box<str> {
        self.0.fresh(prefix, self.1).into()
    }
}

impl Pointers for StreamXrefs {
    fn pointer<'p>(&mut self, pointer: &'p str) -> Cow<'p, str> {
        if self.rules.xref == XrefGrammar::V7 && pointer == "@VOID@" {
            return Cow::Borrowed(pointer);
        }
        if let Some(new) = self.renamed.get(pointer) {
            return Cow::Owned(new.to_string());
        }
        if is_valid_pointer(self.rules, pointer) {
            return Cow::Borrowed(pointer);
        }
        Cow::Owned(candidate_xref(self.rules, pointer))
    }
}
