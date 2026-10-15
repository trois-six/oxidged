//! The conformance pass of a typed dataset, one record at a time.
//!
//! [`conform_records`](super::conform_records) looks at every record before
//! the writer writes the first: a repair can change what other records may
//! point to, or add records and header declarations, so the whole dataset
//! is repaired first, in passes. Nearly every dataset needs little of that:
//! the identifiers to rename and the header to complete (or repair) are
//! known before any record, and its records need no repair, or one that
//! changes nothing outside them.
//!
//! [`Direct`] does what concerns the dataset as a whole first
//! ([`Direct::prepare`]: the identifiers, the header), then checks each
//! record as the writer reaches it ([`Direct::record`]), repairing it in
//! place when the repair stays inside it. What it cannot do so — a missing
//! or misplaced header or trailer, a record that becomes an extension
//! record or one more than its type permits, a structure of a record
//! relocated under a `HEAD.SCHMA` declaration (or one using the tag of a
//! structure the header relocated), a 5.5.1 record over 32K — it answers
//! `None` for, and the writer starts again with the whole pass. The 7.x
//! pointer rules between records ([`crate::spec::graph`]) are decided
//! record by record, as the whole pass decides them: a record with a
//! pointer that breaks one gets `None` too. Either way
//! the output and the repairs are those of the whole pass: the records the
//! direct pass finds clean are those the whole pass finds clean, under the
//! same state, and the records it repairs it repairs with the same code.
//!
//! Record identifiers are looked up by their ids in the dataset's store
//! ([`IdTypes`]): no table of the dataset's identifiers is built.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::clean::Walk;
use super::cycles::{Found, streamed_pointers};
use super::{
    Aliases, Body, Conformer, DATASET, Family, FieldOrder, Named, Rec, RecRef, RepairKind,
    StructId, bump, encoded_len, extension_tags, insert_records,
};
use crate::GedcomVersion;
use crate::convert::Watch;
use crate::model::{Dataset, RecordKind, Store, XrefId};
use crate::spec::graph::{Graph, Pointer};
use crate::spec::validate::{MAX_RECORD_BYTES, xref_error};
use crate::tree::{Flat, FlatPayload, FlatRef, Node, Stream, Structure, Xref};
use crate::writer::{
    LineSink, complete_head, needs_submitter, new_xref, numbered_xref, stub_submitter,
};

#[cfg(test)]
thread_local! {
    /// The direct writes of this thread that looked at every record's
    /// identifier, the dataset's index not telling them valid and unique.
    pub(crate) static ID_PASSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// [`IdTypes::types`] of an identifier no record has.
const NO_RECORD: StructId = StructId::MAX;
/// [`IdTypes::types`] of an extension record's identifier.
const EXTENSION: StructId = StructId::MAX - 1;

/// The record each identifier of a typed dataset names, and its type, by
/// the identifier's id in the dataset's store.
pub(crate) struct IdTypes<'n> {
    store: &'n Store,
    /// By id: the record type, [`EXTENSION`] or [`NO_RECORD`].
    types: Types<'n>,
    /// The identifiers the pass gave records (renamed ones, a stub
    /// submitter), which the store may not have.
    made: HashMap<Box<str>, Option<StructId>>,
}

/// The types of the records identifiers name, by id.
enum Types<'n> {
    /// Found record by record ([`Conformer::rename`]).
    Owned(Vec<StructId>),
    /// Told by the index of the dataset the records are, whose
    /// identifiers are valid and unique
    /// ([`Dataset::xrefs_conform`]): nothing is built but `more`.
    Dataset {
        /// By id, where the record that has it is ([`Dataset::places`]).
        places: &'n [u32],
        /// By a place's kind bits, the type of its records; [`MORE`] for
        /// extension records.
        by_kind: [StructId; 16],
        /// The types of the extension records' identifiers, and of the
        /// identifiers given to records the dataset does not have (a stub
        /// submitter): few.
        more: Vec<(u32, StructId)>,
    },
}

/// [`Types::Dataset`]'s `by_kind` for extension records: their types are
/// in `more`.
const MORE: StructId = StructId::MAX - 2;

/// By version (5.5.1, 7.0, 7.1), the types of the typed records, in
/// [`RecordKind`] order: the same for every write.
static KIND_TYPES: [std::sync::OnceLock<[StructId; 8]>; 3] = [
    std::sync::OnceLock::new(),
    std::sync::OnceLock::new(),
    std::sync::OnceLock::new(),
];

impl Types<'_> {
    #[inline]
    fn get(&self, id: XrefId) -> Option<StructId> {
        match self {
            Types::Owned(types) => types.get(id.index()).copied(),
            Types::Dataset {
                places,
                by_kind,
                more,
            } => {
                let place = places.get(id.index()).copied().unwrap_or(0);
                let looked = |missing| {
                    let index = u32::try_from(id.index()).ok();
                    more.iter()
                        .find(|(i, _)| Some(*i) == index)
                        .map_or(missing, |&(_, ty)| ty)
                };
                Some(if place == 0 {
                    if more.is_empty() {
                        NO_RECORD
                    } else {
                        looked(NO_RECORD)
                    }
                } else {
                    let kind = (place >> crate::model::PLACE_KIND_SHIFT) as usize & 15;
                    match by_kind.get(kind).copied().unwrap_or(NO_RECORD) {
                        MORE => looked(EXTENSION),
                        ty => ty,
                    }
                })
            }
        }
    }

    /// Sets the type of `id`; whether the table has it.
    fn set(&mut self, id: XrefId, ty: StructId) -> bool {
        match self {
            Types::Owned(types) => match types.get_mut(id.index()) {
                Some(slot) => {
                    *slot = ty;
                    true
                }
                None => false,
            },
            Types::Dataset { more, .. } => {
                let Ok(index) = u32::try_from(id.index()) else {
                    return false;
                };
                more.retain(|(i, _)| *i != index);
                if ty != NO_RECORD {
                    more.push((index, ty));
                }
                true
            }
        }
    }
}

impl<'n> IdTypes<'n> {
    fn new(store: &'n Store) -> Self {
        Self {
            store,
            types: Types::Owned(vec![NO_RECORD; store.xref_count()]),
            made: HashMap::new(),
        }
    }

    /// The identifiers of `data`, whose records the pass checks one for
    /// one and whose identifiers are valid and unique: the record types
    /// come from its index as asked for, `type_of` giving the type of a
    /// record tag.
    fn of_dataset(
        data: &'n Dataset,
        version: GedcomVersion,
        type_of: impl Fn(&str) -> Option<StructId>,
    ) -> Self {
        let cache = match version {
            GedcomVersion::V5_5_1 => KIND_TYPES.first(),
            GedcomVersion::V7_0 => KIND_TYPES.get(1),
            _ => KIND_TYPES.get(2),
        };
        let typed = |note: &str| {
            ["SUBM", "SUBN", "INDI", "FAM", note, "SOUR", "REPO", "OBJE"]
                .map(|tag| type_of(tag).unwrap_or(EXTENSION))
        };
        let note = if version.is_v7() { "SNOTE" } else { "NOTE" };
        let kind_types = match cache {
            Some(cache) => *cache.get_or_init(|| typed(note)),
            None => typed(note),
        };
        let mut by_kind = [NO_RECORD; 16];
        for (slot, ty) in by_kind.iter_mut().zip(kind_types) {
            *slot = ty;
        }
        if let Some(slot) = by_kind.get_mut(RecordKind::Other.index()) {
            *slot = MORE;
        }
        // Extension records (and records that did not fit their type) are
        // few: their types are found now.
        let more = data
            .extra()
            .iter()
            .filter_map(|n| {
                let index = u32::try_from(n.xref()?.index()).ok()?;
                Some((index, type_of(n.tag()).unwrap_or(EXTENSION)))
            })
            .collect();
        Self {
            store: data.store(),
            types: Types::Dataset {
                places: data.places(),
                by_kind,
                more,
            },
            made: HashMap::new(),
        }
    }

    /// The record `xref` names, if one does.
    #[inline]
    pub(crate) fn get(&self, xref: &str) -> Option<Named> {
        if let Some(id) = self.store.find_xref(xref) {
            match self.types.get(id) {
                Some(NO_RECORD) | None => {}
                Some(EXTENSION) => return Some(Named(None)),
                Some(ty) => return Some(Named(Some(ty))),
            }
        }
        if self.made.is_empty() {
            return None;
        }
        self.made.get(xref).map(|&ty| Named(ty))
    }

    /// [`IdTypes::get`] of `xref`, whose id in the store is `id`.
    #[inline]
    pub(crate) fn get_id(&self, id: XrefId, xref: &str) -> Option<Named> {
        match self.types.get(id) {
            Some(NO_RECORD) | None if self.made.is_empty() => None,
            Some(NO_RECORD) | None => self.made.get(xref).map(|&ty| Named(ty)),
            Some(EXTENSION) => Some(Named(None)),
            Some(ty) => Some(Named(Some(ty))),
        }
    }

    /// Whether the dataset uses `xref`: a record has it or a pointer holds
    /// it (the store has every identifier of the dataset). A new identifier
    /// is never one of these, so that it names nothing a pointer named.
    pub(crate) fn uses(&self, xref: &str) -> bool {
        self.store.find_xref(xref).is_some() || self.made.contains_key(xref)
    }

    /// Gives `xref` (whose id in the store is `id`, when known) to a record
    /// of type `ty` unless a record has it: whether it did.
    fn take(&mut self, xref: &str, id: Option<XrefId>, ty: Option<StructId>) -> bool {
        let id = id.or_else(|| self.store.find_xref(xref));
        let known = match id {
            Some(id) => self.get_id(id, xref),
            None => self.made.get(xref).map(|&ty| Named(ty)),
        };
        if known.is_some() {
            return false;
        }
        let set = id.is_some_and(|id| self.types.set(id, ty.unwrap_or(EXTENSION)));
        if !set {
            self.made.insert(xref.into(), ty);
        }
        true
    }

    /// Takes `xref` back from the record [`IdTypes::take`] gave it to.
    fn forget(&mut self, xref: &str) {
        match self.store.find_xref(xref) {
            Some(id) => {
                self.types.set(id, NO_RECORD);
            }
            None => {
                self.made.remove(xref);
            }
        }
    }
}

impl IdTypes<'_> {
    /// Whether a pointer stays as it is, written in stream mode
    /// ([`Conformer::clean_pointer`] and [`Conformer::resolves`] with no
    /// renamed identifier): `@VOID@` in 7.x, in 5.5.1 a pointer to a
    /// substructure or another file, or one naming a record of type `want`
    /// (`None`: of any type). `id` is its id in the store, when known.
    #[inline]
    pub(crate) fn resolves(
        &self,
        p: &str,
        id: Option<XrefId>,
        want: Option<StructId>,
        family: Family,
    ) -> bool {
        if (family == Family::V7 && p == "@VOID@")
            || crate::spec::validate::is_external_pointer(p, family)
        {
            return true;
        }
        let named = match id {
            Some(id) => self.get_id(id, p),
            None => self.get(p),
        };
        named.is_some_and(|n| want.is_none() || n.0 == want)
    }
}

/// Where [`Direct::record`] left a record: written in the pass's arena
/// ([`Direct::root`]), or in the record itself (owned: the stub
/// submitter, or a repaired record).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// Written as lines already ([`Direct::lines`]).
    Lines,
    Arena,
    Record,
}

/// The conformance pass of a typed dataset, record by record. See the
/// [module documentation](self).
pub(crate) struct Direct<'n> {
    c: Conformer<'n>,
    ids: Rc<IdTypes<'n>>,
    /// The records of each standard type so far, which the dataset
    /// permits a number of.
    counts: Vec<(u8, u32)>,
    /// The record being written; in stream mode ([`Flat::set_stream`])
    /// when [`Direct::prepare`] set it.
    arena: Flat<'n>,
    /// The extension tags the header's repair declared in `HEAD.SCHMA`
    /// for the structures it relocated: the whole pass declares them only
    /// when no record uses them.
    relocated: Vec<String>,
    /// Whether the arena's stream holds the header's lines
    /// ([`Direct::head_lines`]).
    head_lines: bool,
    /// The stub submitter record to write first ([`Direct::take_first`]).
    first: Option<Structure>,
    /// What a conversion looks for in the records, when the dataset is
    /// written in another version family without one ([`Direct::watch`]).
    watch: Option<Rc<Watch>>,
    /// The pointers between records kept so far, for the 7.x pointer
    /// rules ([`crate::spec::graph`]).
    graph: Graph,
    /// Whether the last record [`Direct::record`] answered `None` of needs
    /// converting, and nothing else ([`Direct::wants_converting`]).
    converts: bool,
}

impl<'n> Direct<'n> {
    /// Does what the whole pass does of `records` (a typed dataset's: the
    /// header first, then the records, then the trailer) before looking at
    /// any record: renames the identifiers, completes the header (adding
    /// the stub submitter it adds) and repairs it. `None` when the dataset
    /// needs the whole pass.
    ///
    /// With `sink` (UTF-8 output), the records that write themselves are
    /// written as lines straight away, as `sink` writes them, when their
    /// structures need no look of the check's walk ([`Flat`] stream mode).
    /// The documented aliases of standard structures (7.x `HEAD.SCHMA`)
    /// need it: none may be declared. A typed header that needs neither
    /// repair nor stub submitter is written so, completed
    /// ([`Direct::head_lines`]).
    ///
    /// With `whole`, the dataset `records` are the records of one for one
    /// (the header aside): when its identifiers are valid and unique, its
    /// index tells the record of each, and no table of them is built.
    pub(crate) fn prepare<N: Node<'n>>(
        records: &mut Vec<Rec<'n, N>>,
        version: GedcomVersion,
        order: Option<&'static FieldOrder>,
        store: &'n Store,
        sink: Option<&LineSink<'_>>,
        whole: Option<&'n Dataset>,
    ) -> Option<Self> {
        let mut c = Conformer::new(version);
        c.order = order;
        well_formed(records)?;
        // The records the header documents as aliases of standard ones (7.x
        // `HEAD.SCHMA`) are records of those types, as the whole pass finds.
        let head = records.first()?.get(&c.arena);
        let mut scratch = Flat::default();
        c.aliases = super::with_node!(head, scratch, |h| Aliases::of(Some(h), c.schema));
        let mut ids = if let Some(data) = whole.filter(|d| d.xrefs_conform(c.rules)) {
            IdTypes::of_dataset(data, version, |tag| c.record_type(tag))
        } else {
            #[cfg(test)]
            ID_PASSES.with(|n| n.set(n.get() + 1));
            let mut ids = IdTypes::new(store);
            c.rename(records, &mut ids);
            ids
        };
        let mut arena = Flat::default();
        arena.set_typing(Some(super::Typing::of(version)));
        let mut stub = None;
        let submitter = match sink {
            Some(sink) => {
                // Pointers are checked by the check's walk until the
                // header is.
                arena.set_stream(Some(Stream::new(c.rules, sink.record_lines(), None)));
                c.head_submitter(records, &mut ids, &mut stub)
            }
            None => None,
        };
        let ids = Rc::new(ids);
        c.ids = Some(Rc::clone(&ids));
        let head_lines =
            submitter.is_some_and(|submitter| c.stream_head(records, &mut arena, submitter));
        let mut first = None;
        let (ids, relocated) = if head_lines {
            // The stub goes first, as the whole pass inserts it, unless
            // other submitter records come first.
            match stub {
                Some(stub) if records.get(1).is_some_and(|r| r.tag() != "SUBM") => {
                    first = Some(stub);
                }
                Some(stub) => insert_records(records, vec![stub]),
                None => {}
            }
            (ids, Vec::new())
        } else {
            c.ids = None;
            let mut ids = Rc::into_inner(ids)?;
            if let Some(x) = stub.and_then(|s| s.xref) {
                ids.forget(x.as_str());
            }
            // The extension tags of the header as given, which a relocation
            // does not take.
            let head_tags = (c.family == Family::V7)
                .then(|| records.first().map(|h| extension_tags(h, &c.arena)))
                .flatten();
            let mut stub = None;
            c.complete_direct(records, &mut ids, &mut stub)?;
            let ids = Rc::new(ids);
            c.ids = Some(Rc::clone(&ids));
            c.used = head_tags;
            let relocated = c.check_head(records)?;
            c.used = None;
            if let Some(stub) = stub {
                insert_records(records, vec![stub]);
            }
            (ids, relocated)
        };
        if !c.streams() {
            arena.set_stream(None);
        }
        // Pointers are looked up as they are written, unless renamed
        // identifiers make the check's walk follow them.
        arena.set_stream_targets(c.renamed.is_empty().then(|| Rc::clone(&ids)));
        Some(Self {
            c,
            ids,
            counts: Vec::new(),
            arena,
            relocated,
            head_lines,
            first,
            watch: None,
            graph: Graph::default(),
            converts: false,
        })
    }

    /// Looks out for records that `watch` says need converting: such a
    /// record makes [`Direct::record`] answer `None` (the dataset is to be
    /// converted first).
    pub(crate) fn watch(&mut self, watch: Rc<Watch>) {
        self.arena.set_stream_watch(Some(Rc::clone(&watch)));
        self.watch = Some(watch);
    }

    /// The arena of the records kept, which [`Rec::to_structure`] reads.
    pub(crate) fn c_arena(&self) -> &Flat<'n> {
        &self.c.arena
    }

    /// Whether the record [`Direct::record`] last answered `None` of needs
    /// converting, and nothing else seen yet: converted alone
    /// ([`crate::convert::Watch::convert_alone`]), it is to be checked
    /// again ([`Direct::record_converted`]).
    pub(crate) fn wants_converting(&self) -> bool {
        self.converts
    }

    /// [`Direct::record`] of a record just converted (or found to need
    /// no converting), which needs no more.
    pub(crate) fn record_converted<N: Node<'n>>(
        &mut self,
        index: usize,
        r: &mut Rec<'n, N>,
        xref: Option<Cow<'n, str>>,
    ) -> Option<Place> {
        let watch = self.watch.take();
        self.arena.set_stream_watch(None);
        let place = self.record(index, r, xref);
        self.arena.set_stream_watch(watch.clone());
        self.watch = watch;
        place
    }

    /// The stub submitter record to write first, after the header, when
    /// [`Direct::prepare`] completed the header with it and kept it out
    /// of the records (sparing their move).
    pub(crate) fn take_first(&mut self) -> Option<Structure> {
        self.first.take()
    }

    /// The lines of the header, completed, when [`Direct::prepare`] wrote
    /// it so: to write before any record.
    pub(crate) fn head_lines(&mut self) -> Option<&mut LineSink<'static>> {
        if self.head_lines {
            self.head_lines = false;
            self.arena.stream_lines()
        } else {
            None
        }
    }

    /// The lines of a record left as [`Place::Lines`].
    pub(crate) fn lines(&mut self) -> Option<&mut LineSink<'static>> {
        self.arena.stream_lines()
    }

    /// The identifiers of the dataset's records, to tell the writer which
    /// are taken.
    pub(crate) fn ids(&self) -> Rc<IdTypes<'n>> {
        Rc::clone(&self.ids)
    }

    /// Checks record `index` of the dataset, written into the pass's arena
    /// when it writes itself, and repairs it when the repair stays inside
    /// it: where it is then. `None` when the dataset needs the whole pass.
    ///
    /// A record that writes itself is first written as lines, with the
    /// identifier `xref` (stream mode, [`Direct::prepare`]): when its structures are
    /// all typed and its lines need no repair, it stays so
    /// ([`Place::Lines`]).
    pub(crate) fn record<N: Node<'n>>(
        &mut self,
        index: usize,
        r: &mut Rec<'n, N>,
        xref: Option<Cow<'n, str>>,
    ) -> Option<Place> {
        self.converts = false;
        let mut walk = Walk {
            size: 0,
            pending: None,
            record: u32::try_from(index).unwrap_or(u32::MAX),
            rtype: DATASET,
        };
        // The record's own conversion: by its tag and identifier as given.
        if self
            .watch
            .as_ref()
            .is_some_and(|w| w.record(r.tag(), r.xref))
        {
            return None;
        }
        if let (true, Body::Built(b)) = (self.arena.is_streaming(), &r.body) {
            self.arena.begin_stream(xref);
            b.build(&mut self.arena);
            if self.arena.stream_converts() {
                self.converts = true;
                return None;
            }
            let clean = !self.arena.spoiled()
                && self.c.is_clean_streamed(&self.arena, &mut walk)
                && !(self.c.family == Family::V551 && walk.size > MAX_RECORD_BYTES);
            if clean {
                if !self.count(walk.rtype) {
                    return None;
                }
                #[cfg(debug_assertions)]
                self.c.assert_clean(r);
                if self.c.family == Family::V7
                    && self.c.tables.is_graph_record(walk.rtype)
                    && !self
                        .graph
                        .keep_if_clean(r.xref(), &streamed_pointers(&self.arena, self.c.tables))
                {
                    return None;
                }
                return Some(Place::Lines);
            }
        }
        // As structures, for the check's walk and the repair.
        let stream = self.arena.take_stream();
        let place = self.record_nodes(index, r);
        self.arena.set_stream(stream);
        place
    }

    /// Whether record `r`, as given (written in the arena when it writes
    /// itself), uses an extension tag the header relocated a structure
    /// to: the whole pass then relocates it otherwise.
    fn uses_relocated<N: Node<'n>>(&self, r: &Rec<'n, N>) -> bool {
        if self.relocated.is_empty() {
            return false;
        }
        let tags = match r.body {
            Body::Built(_) => self
                .arena
                .root()
                .map(|root| {
                    let mut out = HashSet::new();
                    for (_, n) in root.preorder() {
                        if n.tag().starts_with('_') {
                            out.insert(n.tag().to_string());
                        }
                    }
                    out
                })
                .unwrap_or_default(),
            Body::Given(_) | Body::Owned(_) => extension_tags(r, &self.c.arena),
        };
        self.relocated.iter().any(|t| tags.contains(t))
    }

    /// Whether the watch converts or interprets record `r`.
    fn rewrites<'m, M: Node<'m>>(&self, r: M) -> bool {
        self.watch
            .as_ref()
            .is_some_and(|w| w.needs(r) || w.interprets(r))
    }

    /// [`Direct::record`] of a record written as structures.
    fn record_nodes<N: Node<'n>>(&mut self, index: usize, r: &mut Rec<'n, N>) -> Option<Place> {
        let mut walk = Walk {
            size: 0,
            pending: None,
            record: u32::try_from(index).unwrap_or(u32::MAX),
            rtype: DATASET,
        };
        let c = &self.c;
        let (clean, place) = match &r.body {
            Body::Built(b) => {
                self.arena.clear();
                b.build(&mut self.arena);
                let root = self.arena.root();
                if root.is_some_and(|root| self.rewrites(root)) {
                    self.converts = true;
                    return None;
                }
                if self.uses_relocated(r) {
                    return None;
                }
                (c.is_clean_written(&self.arena, &mut walk), Place::Arena)
            }
            Body::Owned(s) => {
                if self.rewrites(&**s) {
                    self.converts = true;
                    return None;
                }
                (
                    c.is_clean_record(&**s, s.xref.as_deref(), &mut walk),
                    Place::Record,
                )
            }
            Body::Given(n) => {
                if self.rewrites(*n) {
                    self.converts = true;
                    return None;
                }
                (c.is_clean_record(*n, n.xref(), &mut walk), Place::Record)
            }
        };
        if !matches!(r.body, Body::Built(_)) && self.uses_relocated(r) {
            return None;
        }
        if clean {
            #[cfg(debug_assertions)]
            self.c.assert_clean(r);
            if !self.count(walk.rtype) {
                return None;
            }
            if !self.graph_rules(r, place, walk.rtype) {
                return None;
            }
            if self.c.family == Family::V551 && walk.size > MAX_RECORD_BYTES {
                let size = match (place, r.get(&self.c.arena)) {
                    (Place::Arena, _) => self
                        .arena
                        .root()
                        .map_or(0, |n| encoded_len(n, 0, self.c.family)),
                    (_, RecRef::Owned(s)) => encoded_len(s, 0, self.c.family),
                    (_, RecRef::Given(n)) => encoded_len(n, 0, self.c.family),
                    (_, RecRef::Kept(n)) => encoded_len(n, 0, self.c.family),
                    (_, RecRef::Built(_)) => usize::MAX,
                };
                if size > MAX_RECORD_BYTES && self.c.record_of(r.tag()).is_some() {
                    return None;
                }
            }
            return Some(place);
        }
        // Repaired as the whole pass repairs it; it must stay a record of
        // its type and need nothing outside it.
        let was_extension = r.tag().starts_with('_');
        let c = &mut self.c;
        c.path.clear();
        let s = r.make_owned(&c.arena);
        c.record(s, &mut self.counts);
        c.path.clear();
        if (!was_extension && s.tag.as_str().starts_with('_'))
            || !c.schma.is_empty()
            || !c.new_records.is_empty()
        {
            return None;
        }
        // The pass's next round finds it clean.
        let mut walk = Walk::default();
        if !c.is_clean_record(&*s, s.xref.as_deref(), &mut walk) {
            return None;
        }
        if c.family == Family::V551
            && encoded_len(&*s, 0, c.family) > MAX_RECORD_BYTES
            && c.record_of(s.tag.as_str()).is_some()
        {
            return None;
        }
        let rtype = c
            .record_type_of(s.tag.as_str(), s.tag.standard_index())
            .unwrap_or(DATASET);
        if !self.graph_rules(r, Place::Record, rtype) {
            return None;
        }
        Some(Place::Record)
    }

    /// The 7.x pointer rules between records ([`crate::spec::graph`]) for
    /// record `r`, of type `rtype`, left at `place`: its pointers kept, or
    /// `false` when one breaks a rule, which the whole pass repairs.
    fn graph_rules<N: Node<'n>>(&mut self, r: &Rec<'n, N>, place: Place, rtype: StructId) -> bool {
        let c = &self.c;
        if c.family != Family::V7 || !c.tables.is_graph_record(rtype) {
            return true;
        }
        let found = match place {
            Place::Arena => self
                .arena
                .root()
                .map(|n| c.graph_pointers(n, rtype))
                .unwrap_or_default(),
            Place::Record | Place::Lines => {
                let mut scratch = Flat::default();
                super::with_node!(r.get(&c.arena), scratch, |n| c.graph_pointers(n, rtype))
            }
        };
        let pointers: Vec<Pointer<'_>> = found.iter().map(Found::pointer).collect();
        self.graph.keep_if_clean(r.xref(), &pointers)
    }

    /// Counts a clean record of type `rtype` towards the records of its
    /// type the dataset permits: `false` when it is one too many.
    fn count(&mut self, rtype: StructId) -> bool {
        if rtype == DATASET {
            return true;
        }
        let t = self.c.schema.tag_id(rtype).unwrap_or(0);
        let n = bump(&mut self.counts, t);
        self.c
            .picker
            .max(DATASET, t)
            .is_none_or(|max| n <= u32::from(max))
    }

    /// The record [`Direct::record`] left in the arena.
    pub(crate) fn root(&self) -> Option<FlatRef<'_>> {
        self.arena.root()
    }

    /// The repairs made, in order.
    pub(crate) fn into_repairs(self) -> Vec<crate::writer::Repair> {
        self.c.repairs
    }
}

impl<'n, N> Rec<'n, N> {
    /// Whether the record writes itself (and was not repaired).
    pub(crate) fn is_built(&self) -> bool {
        matches!(self.body, Body::Built(_))
    }

    /// The identifier of a record that writes itself, as the dataset has
    /// it.
    pub(crate) fn built_xref(&self) -> Option<&'n str> {
        self.xref
    }

    /// The record, when it is owned.
    pub(crate) fn as_owned(&self) -> Option<&Structure> {
        match &self.body {
            Body::Owned(s) => Some(s),
            Body::Given(_) | Body::Built(_) => None,
        }
    }
}

impl<'n> Rec<'n, &'n Structure> {
    /// The record, when it is a structure: owned, or given as one.
    pub(crate) fn as_structure(&self) -> Option<&Structure> {
        match &self.body {
            Body::Owned(s) => Some(s),
            Body::Given(s) => Some(s),
            Body::Built(_) => None,
        }
    }
}

/// Whether `records` has one header first, without an identifier, one
/// empty trailer last, and neither in between: the whole pass changes
/// nothing of them then ([`Conformer::header_and_trailer`]).
fn well_formed<'n, N: Node<'n>>(records: &[Rec<'n, N>]) -> Option<()> {
    let last = records.len().checked_sub(1)?;
    let (head, trailer) = (records.first()?, records.get(last)?);
    let ok = head.tag() == "HEAD"
        && head.xref().is_none()
        && trailer.tag() == "TRLR"
        && trailer.is_empty()
        && trailer.xref().is_none()
        && records
            .get(1..last)?
            .iter()
            .all(|r| !matches!(r.tag(), "HEAD" | "TRLR"));
    ok.then_some(())
}

impl<'n> Conformer<'n> {
    /// (h) [`Conformer::xrefs`] of the records of a typed dataset, by the
    /// ids of its store: the first record holding a valid identifier keeps
    /// it, the others are renamed (and pointers to an invalid one follow).
    fn rename<N: Node<'n>>(&mut self, records: &mut [Rec<'n, N>], ids: &mut IdTypes<'n>) {
        let last = records.len().saturating_sub(1);
        let mut renamed = Vec::new();
        for (i, r) in records.iter().enumerate() {
            let Some(x) = r.xref().filter(|_| i > 0 && i < last) else {
                continue;
            };
            let ty = self.record_type_of(r.tag(), r.standard_tag());
            // Valid as the store found it, for an identifier of the store.
            let valid = match r.id {
                Some(id) => ids.store.is_valid_xref(id, self.rules),
                None => xref_error(x, self.rules).is_none(),
            };
            debug_assert_eq!(valid, xref_error(x, self.rules).is_none(), "{x}");
            if !valid || !ids.take(x, r.id, ty) {
                renamed.push(i);
            }
        }
        for i in renamed {
            let Some(r) = records.get_mut(i) else {
                continue;
            };
            let r = r.make_owned(&self.arena);
            let Some(old) = r.xref.take() else { continue };
            let ty = self.record_type(r.tag.as_str());
            let candidate = new_xref(self.rules, &old);
            let mut new = candidate.clone();
            let mut n = 1_usize;
            while ids.uses(&new) {
                n += 1;
                new = numbered_xref(self.rules, &candidate, n);
            }
            ids.take(&new, None, ty);
            if xref_error(&old, self.rules).is_some() && !self.renamed.contains_key(old.as_str()) {
                self.renamed
                    .insert(old.as_str().into(), new.as_str().into());
            }
            self.repair(
                r.line,
                RepairKind::Xref,
                format!("identifier {old} written as {new}"),
            );
            r.xref = Some(Xref::new(new));
        }
    }

    /// The `SUBM` pointer the typed header of `records` is completed
    /// with ([`FlatPayload::None`] when it needs none), to write it as lines
    /// ([`Conformer::stream_head`]): the first submitter record's, else a
    /// stub submitter's (in `stub`, its identifier taken), as
    /// [`Conformer::complete_direct`] finds it. `None` when the header is
    /// not typed, declares extension tags (7.x `HEAD.SCHMA`) or the
    /// submitter was renamed.
    fn head_submitter<N: Node<'n>>(
        &self,
        records: &[Rec<'n, N>],
        ids: &mut IdTypes<'n>,
        stub: &mut Option<Structure>,
    ) -> Option<FlatPayload<'n>> {
        let Some(Body::Built(b)) = records.first().map(|r| &r.body) else {
            return None;
        };
        let h = b.typed_head()?;
        if h.schema.is_some() {
            return None;
        }
        if !self.rules.head_sour_subm || h.submitter.is_some() {
            return Some(FlatPayload::None);
        }
        let first = records
            .iter()
            .find(|r| r.tag() == "SUBM" && r.xref().is_some());
        match first {
            Some(
                r @ Rec {
                    body: Body::Built(_),
                    xref: Some(x),
                    ..
                },
            ) => Some(FlatPayload::Pointer(Cow::Borrowed(x), r.id)),
            Some(_) => None,
            None => {
                let x = (1..usize::MAX)
                    .map(|n| format!("@U{n}@"))
                    .find(|x| !ids.uses(x))?;
                ids.take(&x, None, self.record_of("SUBM"));
                *stub = Some(stub_submitter(x.clone()));
                Some(FlatPayload::Pointer(Cow::Owned(x), None))
            }
        }
    }

    /// The typed header of `records`, completed with `submitter`
    /// ([`Conformer::head_submitter`]), written as lines into `arena` (in
    /// stream mode) and checked as it is: `true` when it needs no repair,
    /// what [`Conformer::check_head`] would find clean.
    fn stream_head<N: Node<'n>>(
        &self,
        records: &[Rec<'n, N>],
        arena: &mut Flat<'n>,
        submitter: FlatPayload<'n>,
    ) -> bool {
        let Some(Body::Built(b)) = records.first().map(|r| &r.body) else {
            return false;
        };
        arena.begin_stream(None);
        if !b.build_completed_head(arena, self.rules, submitter) {
            return false;
        }
        let mut walk = Walk::default();
        !arena.spoiled()
            && self.is_clean_streamed(arena, &mut walk)
            && !(self.family == Family::V551 && walk.size > MAX_RECORD_BYTES)
    }

    /// (i) [`Conformer::complete_head`] of a typed dataset: the stub
    /// submitter to add, if any, in `stub`. `None` when no identifier is
    /// left for it.
    fn complete_direct<N: Node<'n>>(
        &mut self,
        records: &mut [Rec<'n, N>],
        ids: &mut IdTypes<'n>,
        stub: &mut Option<Structure>,
    ) -> Option<()> {
        let submitter = records
            .iter()
            .find(|r| r.tag() == "SUBM" && r.xref().is_some())
            .and_then(|r| r.xref().map(str::to_string));
        let head_ty = self.record_of("HEAD");
        let subm_ty = self.record_of("SUBM");
        let head = records.first_mut()?.make_owned(&self.arena);
        let submitter = if !needs_submitter(head, self.rules) {
            None
        } else if let Some(x) = submitter {
            Some(x)
        } else {
            let x = (1..usize::MAX)
                .map(|n| format!("@U{n}@"))
                .find(|x| !ids.uses(x))?;
            ids.take(&x, None, subm_ty);
            *stub = Some(stub_submitter(x.clone()));
            Some(x)
        };
        complete_head(head, self.rules, "UTF-8", submitter);
        if let Some(ty) = head_ty {
            self.model_order_to(head, ty, 2);
        }
        Some(())
    }

    /// The header of `records`, checked as a record and, when it needs it,
    /// repaired as the whole pass repairs it (with the declarations of the
    /// structures it relocates), then completed and looked at again, as
    /// the pass's next round does. The extension tags it relocated
    /// structures to; `None` when the dataset needs the whole pass.
    fn check_head<N: Node<'n>>(&mut self, records: &mut [Rec<'n, N>]) -> Option<Vec<String>> {
        let Some(ty) = self.record_of("HEAD") else {
            return Some(Vec::new());
        };
        let mut walk = Walk::default();
        let head = records.first_mut()?.make_owned(&self.arena);
        if self.is_clean_walked(&*head, ty, &mut walk) {
            self.head_size(head, &walk)?;
            return Some(Vec::new());
        }
        self.path.clear();
        let _ = self.fix(head, ty, true);
        self.model_order(head, ty);
        self.path.clear();
        if !self.new_records.is_empty() {
            return None;
        }
        let relocated: Vec<String> = self.schma.iter().map(|(t, _)| t.clone()).collect();
        self.declare(records);
        let head = records.first_mut()?.make_owned(&self.arena);
        let aliases = Aliases::of(Some(&*head), self.schema);
        // The identifiers were typed with the aliases as given: a repair
        // that drops one (a `SCHMA` with a payload made `_SCHMA`) needs
        // the whole pass, which types them again.
        if self
            .aliases
            .structs
            .iter()
            .any(|(tag, ty)| aliases.structs.get(tag) != Some(ty))
        {
            return None;
        }
        self.aliases = aliases;
        complete_head(head, self.rules, "UTF-8", None);
        self.model_order_to(head, ty, 2);
        let mut walk = Walk::default();
        if needs_submitter(head, self.rules) || !self.is_clean_walked(&*head, ty, &mut walk) {
            return None;
        }
        self.head_size(head, &walk)?;
        // Relocations in records would need tags no record uses.
        self.schma.clear();
        Some(relocated)
    }

    /// Whether records can be checked as they are written in stream mode:
    /// no documented alias of a standard structure or value (7.x
    /// `HEAD.SCHMA`), which the check's walk follows.
    fn streams(&self) -> bool {
        self.aliases.structs.is_empty() && self.aliases.words.is_empty()
    }

    /// (j) A 5.5.1 header over 32K: `None`, for the whole pass.
    fn head_size(&self, head: &Structure, walk: &Walk<'_>) -> Option<()> {
        let over = self.family == Family::V551
            && walk.size > MAX_RECORD_BYTES
            && encoded_len(head, 0, self.family) > MAX_RECORD_BYTES;
        (!over).then_some(())
    }
}

/// The header of `records`, owned, for the writer.
pub(crate) fn take_head<'n, N: Node<'n>>(records: &mut [Rec<'n, N>]) -> Option<Structure> {
    match &mut records.first_mut()?.body {
        Body::Owned(s) => Some(std::mem::take(s)),
        Body::Given(_) | Body::Built(_) => None,
    }
}
