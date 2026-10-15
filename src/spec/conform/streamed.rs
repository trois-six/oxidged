//! The conformance repair of records written one at a time, with nothing
//! of the dataset at hand but the header
//! ([`GedcomStreamWriter`](crate::GedcomStreamWriter)).
//!
//! Each record is checked and repaired by the code of the whole pass
//! ([`conform_records`](super::conform_records)), under three differences
//! that come from not seeing the other records:
//!
//! - a pointer valid in the version's grammar is taken to name a record
//!   of its type, which may come later: dangling pointers and pointers to
//!   a record of another type are not repaired;
//! - a structure the whole pass relocates under a `HEAD.SCHMA`
//!   declaration (7.x) becomes a plain extension structure, the header
//!   being written already;
//! - the notes moved out of a 5.5.1 record over 32K go to `NOTE` records
//!   written right after it, instead of with the other notes.
//!
//! The records of each type are counted as they come, so that one more
//! than its type permits becomes an extension record as in the whole
//! pass; and the 7.x pointer rules between records
//! ([`crate::spec::graph`]) are decided as they come, as the whole pass
//! decides them in the same order.

use std::borrow::Cow;
use std::rc::Rc;

use super::clean::Walk;
use super::cycles::{Found, streamed_pointers};
use super::{Aliases, Conformer, DATASET, Family, StructId, Typing, bump, encoded_len};
use crate::GedcomVersion;
use crate::convert::Watch;
use crate::spec::conform::Build;
use crate::spec::graph::{Graph, Pointer};
use crate::spec::validate::MAX_RECORD_BYTES;
use crate::tree::Node;
use crate::tree::{Flat, Stream, StreamBuffers, Structure};
use crate::writer::LineSink;
use crate::writer::{Repair, RepairKind, complete_head, needs_submitter};

/// The state of records written one at a time: see the [module
/// documentation](self).
pub(crate) struct Streamed {
    version: GedcomVersion,
    /// The aliases the header declares (7.x).
    aliases: Aliases,
    /// The records of each standard type so far.
    counts: Vec<(u8, u32)>,
    /// The repairs, in order.
    pub(crate) repairs: Vec<Repair>,
    /// The room of stream mode, from record to record.
    buffers: Option<StreamBuffers>,
    /// The pointers between records kept so far, for the 7.x pointer
    /// rules ([`crate::spec::graph`]).
    graph: Graph,
}

/// A record as [`Streamed::record`] leaves it.
pub(crate) enum Outcome<'r> {
    /// Written as lines, at the end of the sink.
    Lines,
    /// As it wrote itself, typed, in this arena.
    Clean(Box<Flat<'r>>),
    /// Repaired, with the notes moved out of it.
    Repaired(Structure, Vec<Structure>),
    /// Not written: it needs converting first (see `watch` of
    /// [`Streamed::record`]).
    Convert,
}

impl Streamed {
    /// Completes and repairs `head`, the header of records of `version`,
    /// as the whole pass does, pointing to the record `stub` when it needs
    /// a submitter: then `stubbed` is set, for the stub to be written.
    pub(crate) fn header(
        version: GedcomVersion,
        head: &mut Structure,
        stub: &str,
        stubbed: &mut bool,
    ) -> Self {
        let mut this = Self {
            version,
            aliases: Aliases::default(),
            counts: Vec::new(),
            repairs: Vec::new(),
            buffers: None,
            graph: Graph::default(),
        };
        let mut c = this.conformer();
        if let Some(x) = head.xref.take() {
            c.repair(
                head.line,
                RepairKind::Xref,
                format!("identifier {x} of HEAD left out"),
            );
        }
        c.aliases = Aliases::of(Some(&*head), c.schema);
        let head_ty = c.record_of("HEAD");
        let mut complete = |c: &Conformer<'_>, head: &mut Structure| {
            let submitter = needs_submitter(head, c.rules).then(|| {
                *stubbed = true;
                stub.to_string()
            });
            complete_head(head, c.rules, "UTF-8", submitter);
            if let Some(ty) = head_ty {
                c.model_order_to(head, ty, 2);
            }
        };
        complete(&c, head);
        if let Some(ty) = head_ty
            && !c.is_clean(&*head, ty, true)
        {
            c.path.clear();
            let _ = c.fix(head, ty, true);
            c.model_order(head, ty);
            complete(&c, head);
        }
        this.aliases = Aliases::of(Some(&*head), c.schema);
        this.repairs = c.repairs;
        this
    }

    /// A conformance pass for one record.
    fn conformer<'r>(&self) -> Conformer<'r> {
        let mut c = Conformer::new(self.version);
        c.order = Some(crate::model::field_order(self.version));
        c.aliases = self.aliases.clone();
        c.trust = true;
        c.no_declare = true;
        c
    }

    /// Writes `record` typed and checks it; repairs it when it needs it.
    /// It is first written as lines at the end of `sink`, with the
    /// identifier `xref` ([`Outcome::Lines`]); when that needs the
    /// structures, it is written into an arena. `fresh` makes the
    /// identifier of a `NOTE` record a note moves to. With `watch`, a
    /// record that needs converting is left as it is ([`Outcome::Convert`]).
    pub(crate) fn record<'r>(
        &mut self,
        record: &dyn Build<'r>,
        xref: Option<Cow<'r, str>>,
        sink: &mut LineSink<'_>,
        fresh: &mut dyn FnMut() -> String,
        watch: Option<&Rc<Watch>>,
    ) -> Outcome<'r> {
        let mut c = self.conformer();
        let typing = Typing::of(self.version);
        // Lines go straight to UTF-8 output; another encoding encodes them
        // from the structures.
        let utf8 = sink.encoding() == crate::writer::OutputEncoding::Utf8;
        if utf8 && c.aliases.structs.is_empty() && c.aliases.words.is_empty() {
            let mut buffers = self
                .buffers
                .take()
                .unwrap_or_else(|| StreamBuffers::new(sink.record_lines()));
            // The record's lines go right after the output, kept when
            // they are its own.
            sink.lend(buffers.lines_mut());
            let mut flat = Flat::default();
            flat.set_typing(Some(typing));
            flat.set_stream(Some(Stream::with_buffers(c.rules, buffers, None)));
            flat.set_stream_watch(watch.cloned());
            flat.begin_stream(xref);
            record.build(&mut flat);
            let converts = flat.stream_converts();
            let mut walk = Walk::default();
            let clean = !converts
                && !flat.spoiled()
                && c.is_clean_streamed(&flat, &mut walk)
                && (c.family != Family::V551 || walk.size <= MAX_RECORD_BYTES);
            let mut keep = clean && self.count(&c, walk.rtype);
            if keep
                && c.family == Family::V7
                && c.tables.is_graph_record(walk.rtype)
                && !self
                    .graph
                    .keep_if_clean(record.xref(), &streamed_pointers(&flat, c.tables))
            {
                self.uncount(&c, walk.rtype);
                keep = false;
            }
            self.buffers = flat.take_stream().map(Stream::into_buffers);
            if let Some(b) = &mut self.buffers {
                sink.take_back(b.lines_mut(), keep);
            }
            if converts {
                return Outcome::Convert;
            }
            if keep {
                return Outcome::Lines;
            }
        }
        let mut flat = Flat::default();
        flat.set_typing(Some(typing));
        record.build(&mut flat);
        if let (Some(w), Some(root)) = (watch, flat.root())
            && w.needs(root)
        {
            return Outcome::Convert;
        }
        let mut walk = Walk::default();
        let clean = c.is_clean_written(&flat, &mut walk);
        if clean {
            let over = !self.count(&c, walk.rtype);
            let big = c.family == Family::V551
                && walk.size > MAX_RECORD_BYTES
                && flat
                    .root()
                    .is_some_and(|n| encoded_len(n, 0, c.family) > MAX_RECORD_BYTES);
            if !over && !big {
                let found = flat
                    .root()
                    .map(|n| c.graph_pointers(n, walk.rtype))
                    .unwrap_or_default();
                let pointers: Vec<Pointer<'_>> = found.iter().map(Found::pointer).collect();
                if self.graph.keep_if_clean(record.xref(), &pointers) {
                    return Outcome::Clean(Box::new(flat));
                }
            }
            if !over {
                // Counted again by the repair.
                self.uncount(&c, walk.rtype);
            }
        }
        let mut s = flat
            .root()
            .map(Node::to_owned_structure)
            .unwrap_or_default();
        c.path.clear();
        c.record(&mut s, &mut self.counts);
        self.cut_graph(&mut c, &mut s, record.xref());
        let notes = Self::notes(&mut c, &mut s, fresh);
        self.repairs.append(&mut c.repairs);
        Outcome::Repaired(s, notes)
    }

    /// The 7.x pointer rules between records for the repaired record `s`
    /// (identifier `xref`): its pointers that break one cut, the others
    /// kept.
    fn cut_graph(&mut self, c: &mut Conformer<'_>, s: &mut Structure, xref: Option<&str>) {
        let Some(rtype) = c.record_type_of(s.tag.as_str(), s.tag.standard_index()) else {
            return;
        };
        let found = c.graph_pointers(&*s, rtype);
        if found.is_empty() {
            return;
        }
        let pointers: Vec<Pointer<'_>> = found.iter().map(Found::pointer).collect();
        let broken = self.graph.record(xref, &pointers);
        if broken.is_empty() {
            return;
        }
        let x = xref.unwrap_or_default();
        for (i, b) in broken {
            if let Some(f) = found.get(i) {
                c.cut(s, f.path(), b, x);
            }
        }
        c.model_order(s, rtype);
    }

    /// Counts a clean record of type `rtype` towards the records of its
    /// type the dataset permits: `false`, and not counted, when it is one
    /// too many.
    fn count(&mut self, c: &Conformer<'_>, rtype: StructId) -> bool {
        if rtype == DATASET {
            return true;
        }
        let t = c.schema.tag_id(rtype).unwrap_or(0);
        let n = bump(&mut self.counts, t);
        if c.picker
            .max(DATASET, t)
            .is_some_and(|max| n > u32::from(max))
        {
            self.uncount(c, rtype);
            return false;
        }
        true
    }

    /// Takes back a count of [`Streamed::count`].
    fn uncount(&mut self, c: &Conformer<'_>, rtype: StructId) {
        let t = c.schema.tag_id(rtype).unwrap_or(0);
        if let Some((_, n)) = self.counts.iter_mut().find(|(k, _)| *k == t) {
            *n -= 1;
        }
    }

    /// (j) The notes moved out of `s` when it is a 5.5.1 record over 32K.
    fn notes(
        c: &mut Conformer<'_>,
        s: &mut Structure,
        fresh: &mut dyn FnMut() -> String,
    ) -> Vec<Structure> {
        let Some(ty) = c.record_of_standard(s.tag.as_str(), s.tag.standard_index()) else {
            return Vec::new();
        };
        if c.family != Family::V551 {
            return Vec::new();
        }
        let size = encoded_len(&*s, 0, c.family);
        if size <= MAX_RECORD_BYTES {
            return Vec::new();
        }
        c.move_notes(s, ty, size, &mut |_| fresh())
    }
}
