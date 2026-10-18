//! The conformance repair pass: the repair table of the [module
//! documentation](super#conformance-repair), applied to owned structures.
//!
//! The pass walks every record with its structure type, repairing payloads
//! and substructures bottom-up: a structure that cannot be made valid where
//! it is becomes an extension structure (`_TAG`) in the same place, with
//! its payload and substructures. As that can change what a pointer points
//! to (a record that becomes an extension is no longer of its type), the
//! pass runs again until it changes nothing; each repair moves data towards
//! extensions only, so this ends, in two passes in practice.
//!
//! Most records need no repair. Each pass first looks at every record
//! without changing it ([`clean`]): a record the repair would leave as it is
//! stays as it was given — borrowed from a parsed tree, never copied — and
//! only the others are copied and repaired. A record found so stays so in
//! the following passes while what it depends on (the record each
//! identifier names, the renamed identifiers, the aliases) is unchanged.

mod clean;
mod cycles;
mod direct;
#[cfg(test)]
pub(crate) use direct::ID_PASSES;
pub(crate) mod streamed;

pub(crate) use direct::{Direct, IdTypes, Place, take_head};

use clean::Walk;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use super::index::{self, SchemaIndex};
use super::payload::{self, Family, is_ext_tag};
use super::schema::{DATASET, Kind, Schema, StructId, tag_index};
use super::tables::TAGS;
use super::validate::{
    Aliases, MAX_RECORD_BYTES, Pick, Picker, encoded_len, is_external_pointer, pick, xref_error,
};
use crate::GedcomVersion;
use crate::tree::node::identifiers;
use crate::tree::{Flat, FlatPayload, FlatRef, Node, Payload, PayloadRef, Structure, Tag, Xref};
use crate::version::VersionRules;
use crate::writer::{
    PLACEHOLDER, Repair, RepairKind, XrefIndex, complete_head, extension_tag, needs_submitter,
    new_xref, numbered_xref, stub_submitter,
};

/// The most passes [`conform`] makes; each one after the first only follows
/// up on records that became extensions.
const MAX_PASSES: usize = 8;

/// The longest pointer line to a note, as [`encoded_len`] counts it: a
/// two-digit level, `NOTE`, a 22-character 5.5.1 identifier and a two-byte
/// terminator. A 5.5.1 note no longer than it stays inline (repair j).
const POINTER_LINE: usize = 32;

/// The pointers looked up together ([`clean::Walk::pending`]): 2 MB.
const LOOKUP_BATCH: usize = 1 << 16;

/// Repairs `records` (a whole dataset: header, records and trailer, in
/// order) so that [`validate`](super::validate) finds nothing for
/// `version`, and returns what was changed, in the order it was changed.
///
/// Every repair is deterministic and keeps the data: what has no standard
/// place becomes an extension structure where it was. The [module
/// documentation](super#conformance-repair) lists every repair. The one
/// deviation that may remain is a 5.5.1 record over 32K made of structures
/// rather than notes, which nothing can move out of losslessly
/// ([`DeviationKind::RecordSize`]), and which 5.5.1 only recommends
/// against.
///
/// [`DeviationKind::RecordSize`]: super::DeviationKind::RecordSize
pub fn conform(records: &mut Vec<Structure>, version: GedcomVersion) -> Vec<Repair> {
    let mut recs: Vec<Rec<'_, &Structure>> = records.drain(..).map(Rec::owned).collect();
    let repairs = conform_records(&mut recs, version, None).repairs;
    records.extend(recs.into_iter().map(Rec::into_structure));
    repairs
}

/// The order in which a typed model writes the substructures of each
/// structure type: the substructures its fields hold, field after field in
/// their declared order, then those it keeps aside (`extra`), in their
/// order. A dataset of the model read back from what it wrote holds its
/// structures so, whatever their order in the file.
///
/// A record of such a model is written so: in that order. When the pass
/// repairs one, it puts its structures back in the order the model, reading
/// the record back, would write them ([`Conformer::model_order`]), as a
/// repair can change where they go: what becomes an extension goes aside,
/// what is added (a required substructure, a `PHRASE`, `HEAD.SCHMA`) or
/// made to fit (a pointer where text belongs, written as text) goes to its
/// field. A record that becomes an extension record goes after the others.
/// Writing what was written then writes the same lines. Without it (a
/// tree, whose order is the file's), the pass repairs in place.
#[derive(Debug)]
pub(crate) struct FieldOrder {
    /// For each structure type, where its fields are in `cells`: the first
    /// and the number.
    rows: &'static [(u16, u16)],
    /// The fields of each structure type ([`Field`]), by the indices of
    /// the standard tags they take (`crate::tree::STANDARD_TAGS`).
    cells: &'static [(u16, Field)],
}

/// A field of a typed structure, as [`FieldOrder`] needs it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Field {
    /// Its position among the fields.
    pub(crate) rank: u16,
    /// Whether it holds a value without substructures: a structure with
    /// substructures does not fit it.
    pub(crate) leaf: bool,
    /// Whether it holds any number of structures rather than one.
    pub(crate) many: bool,
}

impl FieldOrder {
    /// The order of the fields of each structure type (by [`StructId`]):
    /// `rows` gives where its fields are in `cells`, each the index of a
    /// standard tag a field takes, with the field.
    pub(crate) const fn new(rows: &'static [(u16, u16)], cells: &'static [(u16, Field)]) -> Self {
        Self { rows, cells }
    }

    /// The field of a `ty` structure that takes substructures with the
    /// standard tag `standard`.
    pub(crate) fn field(&self, ty: StructId, standard: u16) -> Option<Field> {
        let &(first, len) = self.rows.get(usize::from(ty))?;
        let (first, len) = (usize::from(first), usize::from(len));
        self.cells
            .get(first..first + len)?
            .iter()
            .find(|&&(t, _)| t == standard)
            .map(|&(_, field)| field)
    }
}

/// Where the typed model reads each substructure of a structure, one after
/// the other ([`Conformer::model_order`]): the fields that hold one
/// structure and have it, and the standard tags of substructures kept
/// aside, as bit sets.
#[derive(Clone, Default)]
struct ModelRead {
    full: [u64; 4],
    aside: [u64; 8],
    /// The standard tags of the structure's own substructures: an extension
    /// is not read back into a field that holds one structure when one of
    /// them has the field's tag, as the reader does not (`read_tag_of`).
    present: [u64; 8],
}

/// The word of a bit set holding bit `i`, and the bit.
fn bit(set: &mut [u64], i: u16) -> Option<(&mut u64, u64)> {
    let word = set.get_mut(usize::from(i / 64))?;
    Some((word, 1 << (i % 64)))
}

impl ModelRead {
    /// For the substructures of one structure, by their standard tags.
    fn of(tags: impl Iterator<Item = Option<u16>>) -> Self {
        let mut read = Self::default();
        for tag in tags.flatten() {
            if let Some((w, b)) = bit(&mut read.present, tag) {
                *w |= b;
            }
        }
        read
    }

    /// The rank of the field `c`, a substructure of a `ty` structure, goes
    /// to; `None` when it is kept aside.
    fn read<'a, N: Node<'a>>(
        &mut self,
        version: GedcomVersion,
        order: &FieldOrder,
        ty: StructId,
        c: N,
    ) -> Option<u16> {
        // An extension the reader reads back as its standard structure is
        // ordered as that structure.
        let (standard, read_back_as) = match c.standard_tag() {
            Some(standard) => (standard, false),
            None => (
                read_back(
                    version,
                    &[ty],
                    c.tag(),
                    ExtBody::of(c.payload()),
                    c.children().any(|g| g.standard_tag().is_none()),
                    |t| c.children().any(|g| g.tag() == t),
                )?,
                true,
            ),
        };
        let field = order.field(ty, standard);
        if read_back_as
            && field.is_some_and(|f| !f.many)
            && bit(&mut self.present, standard).is_some_and(|(w, b)| *w & b != 0)
        {
            return None;
        }
        let fits = field.filter(|f| {
            let blocked = bit(&mut self.aside, standard).is_none_or(|(w, b)| *w & b != 0);
            let full = !f.many && bit(&mut self.full, f.rank).is_none_or(|(w, b)| *w & b != 0);
            c.xref().is_none() && (!f.leaf || c.children().next().is_none()) && !blocked && !full
        });
        if let Some(f) = fits {
            if let (false, Some((w, b))) = (f.many, bit(&mut self.full, f.rank)) {
                *w |= b;
            }
            return Some(f.rank);
        }
        if let (Some(_), Some((w, b))) = (field, bit(&mut self.aside, standard)) {
            *w |= b;
        }
        None
    }
}

/// What [`conform_records`] did, and the identifiers it left.
pub(crate) struct Conformed<'n> {
    /// The repairs, in the order they were made.
    pub(crate) repairs: Vec<Repair>,
    xrefs: XrefIndex<'n, Option<StructId>>,
    /// The identifiers pointers hold, when a record has no identifier
    /// ([`Conformer::pointed`]).
    pointed: Option<HashSet<Box<str>>>,
    /// The records that write themselves kept once written
    /// ([`Rec::get`]).
    pub(crate) arena: Flat<'n>,
    /// The generation of what the records depend on as the pass ended
    /// ([`Rec::checked`]).
    pub(crate) state: u32,
}

/// What a pass found of a record it found clean ([`Rec::checked`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Checked {
    rtype: StructId,
    size: u32,
}

/// [`Rec::clean`] of a record known clean ([`Rec::known_clean`]) until
/// the first pass begins.
const KNOWN_CLEAN: u32 = u32::MAX;

impl Conformed<'_> {
    /// Whether a record has this identifier. Every record identifier is
    /// valid and unique once repaired.
    pub(crate) fn knows(&self, xref: &str) -> bool {
        self.xrefs.contains_key(xref)
    }

    /// Whether the records use `xref`: a record has it or a pointer holds
    /// it. An identifier given to a record without one is never one of
    /// these.
    pub(crate) fn uses(&self, xref: &str) -> bool {
        self.knows(xref) || self.pointed.as_ref().is_some_and(|p| p.contains(xref))
    }
}

/// [`conform`] on records as they are given ([`Rec`]): only the records
/// that need a repair are copied.
///
/// `order` is the order of the typed model the records come from, when
/// they do ([`FieldOrder`]).
pub(crate) fn conform_records<'n, N: Node<'n>>(
    records: &mut Vec<Rec<'n, N>>,
    version: GedcomVersion,
    order: Option<&'static FieldOrder>,
) -> Conformed<'n> {
    let mut c = Conformer::new(version);
    c.order = order;
    for pass in 0..MAX_PASSES {
        let before = c.repairs.len();
        c.pass(records, pass);
        if pass == 0 && c.family == Family::V551 {
            c.record_sizes(records);
        }
        // A pass that repaired nothing and added no record leaves nothing
        // for another pass to find, but the pointer rules between records
        // (7.x), whose repairs the next pass looks at.
        if c.repairs.len() == before && (pass > 0 || !c.added) && !c.pointer_rules(records) {
            break;
        }
    }
    // The writer gives an identifier to a standard record without one.
    if records
        .iter()
        .any(|r| r.xref().is_none() && crate::writer::record_prefix(r.tag()).is_some())
    {
        c.gather_pointed(records);
    }
    Conformed {
        repairs: c.repairs,
        xrefs: c.xrefs,
        pointed: c.pointed,
        arena: c.arena,
        state: c.state,
    }
}

/// Puts an owned record of a typed model, rewritten outside the pass (a
/// record converted to another version: `crate::convert`), in the order
/// the model writes it in `version` ([`FieldOrder`]), as the pass puts the
/// records it repairs.
pub(crate) fn model_order(
    record: &mut Structure,
    version: GedcomVersion,
    order: &'static FieldOrder,
) {
    let mut c = Conformer::new(version);
    c.order = Some(order);
    if let Some(ty) = c.record_of_standard(record.tag.as_str(), record.tag.standard_index()) {
        c.model_order(record, ty);
    }
}

/// A record of a dataset being repaired: as it was given (a node of a
/// parsed tree, a borrowed structure, a record that writes itself), as long
/// as the repair leaves it so, or an owned copy, which the repair changes.
pub(crate) struct Rec<'n, N> {
    body: Body<'n, N>,
    /// The tag and identifier of a record as given (read once: they are
    /// asked for pass after pass, and a tree finds an identifier by a
    /// search), and the tag's index among the standard tags.
    tag: &'n str,
    xref: Option<&'n str>,
    /// The identifier's id in the store of the typed dataset the record
    /// is of, when it writes itself.
    id: Option<crate::model::XrefId>,
    standard: Option<u16>,
    /// The generation of what the record depends on ([`Conformer::state`])
    /// under which it was found to need no repair; 0 when it was not.
    clean: u32,
    /// Its record type then (the dataset's type for none), which counts
    /// towards the records of that type the dataset permits.
    rtype: StructId,
    /// An upper bound of its size as written, from its last look (5.5.1
    /// records over 32K: [`Conformer::record_sizes`]); `u32::MAX` when
    /// unknown.
    size: u32,
    /// Where the structures of a record that writes itself start in the
    /// arena of kept records ([`Conformed::arena`]), kept from its first
    /// writing while the dataset is small ([`KEEP_BUILT`]).
    built: Option<u32>,
}

/// The most structures of records that write themselves kept once written
/// (about 6 MB): a small dataset is written once, a large one again for
/// each look, so that its memory stays bounded.
const KEEP_BUILT: usize = 1 << 16;

enum Body<'n, N> {
    Given(N),
    Built(&'n dyn Build<'n>),
    Owned(Box<Structure>),
}

/// A record that writes its structures when they are needed, each time:
/// a record of the typed model, written into a flat arena.
pub(crate) trait Build<'n> {
    /// The tag.
    fn tag(&self) -> &'n str;
    /// The identifier.
    fn xref(&self) -> Option<&'n str>;
    /// Whether it has neither payload nor substructure.
    fn is_empty(&self) -> bool;
    /// The identifier's id in the store of its dataset.
    fn xref_id(&self) -> Option<crate::model::XrefId> {
        None
    }
    /// The index of the tag among the standard tags.
    fn standard_tag(&self) -> Option<u16> {
        crate::tree::node::standard_index(self.tag())
    }
    /// Writes the record, and its substructures, into `out`.
    fn build(&self, out: &mut Flat<'n>);
    /// The version a typed header declares (`GEDC.VERS`, empty for none),
    /// as it writes it; `None` for another record, or a header that keeps
    /// structures aside.
    fn declared_version(&self) -> Option<Cow<'n, str>> {
        None
    }
    /// The record, when it is a typed header.
    fn typed_head(&self) -> Option<&'n crate::model::Header> {
        None
    }
    /// Writes the header ([`Build::typed_head`]) completed for `rules`, with
    /// the pointer `submitter` as its `SUBM` when it has none
    /// ([`crate::model::Header::write_completed_flat`]): `false` when it is
    /// none, or holds structures the completion keeps elsewhere.
    fn build_completed_head(
        &self,
        out: &mut Flat<'n>,
        rules: &crate::version::VersionRules,
        submitter: FlatPayload<'n>,
    ) -> bool {
        let _ = (out, rules, submitter);
        false
    }
    /// The record as an owned structure, when it has a cheaper way to it
    /// than its structures written into a flat arena (the header, which
    /// the writer completes): `None` for the others.
    fn owned(&self) -> Option<Structure> {
        None
    }
}

/// A record, borrowed: given, written on demand (or written already), or
/// owned.
pub(crate) enum RecRef<'b, 'n, N> {
    Given(N),
    Built(&'n dyn Build<'n>),
    Kept(FlatRef<'b>),
    Owned(&'b Structure),
}

/// Runs `$body` with `$n` bound to record `$rec` as a node: as given, as
/// owned, or written into the flat arena `$scratch`.
macro_rules! with_node {
    ($rec:expr, $scratch:expr, |$n:ident| $body:expr) => {
        match $rec {
            RecRef::Given($n) => $body,
            RecRef::Owned($n) => $body,
            RecRef::Kept($n) => $body,
            RecRef::Built(b) => {
                $scratch.clear();
                b.build(&mut $scratch);
                match $scratch.root() {
                    Some($n) => $body,
                    None => {
                        let empty = Structure::default();
                        let $n = &empty;
                        $body
                    }
                }
            }
        }
    };
}
pub(crate) use with_node;

impl<'n, N: Node<'n>> Rec<'n, N> {
    /// A record as it is given.
    pub(crate) fn given(node: N) -> Self {
        Self::of(
            node.tag(),
            node.xref(),
            node.standard_tag(),
            Body::Given(node),
        )
    }

    /// A record that writes itself when needed.
    pub(crate) fn built(record: &'n dyn Build<'n>) -> Self {
        let tag = record.tag();
        let standard = record.standard_tag();
        let mut r = Self::of(tag, record.xref(), standard, Body::Built(record));
        r.id = record.xref_id();
        r
    }

    /// An owned record.
    pub(crate) fn owned(s: Structure) -> Self {
        Self::of("", None, None, Body::Owned(Box::new(s)))
    }

    fn of(tag: &'n str, xref: Option<&'n str>, standard: Option<u16>, body: Body<'n, N>) -> Self {
        Self {
            body,
            tag,
            xref,
            id: None,
            standard,
            clean: 0,
            rtype: DATASET,
            size: u32::MAX,
            built: None,
        }
    }

    /// [`Build::declared_version`] of a record that writes itself.
    pub(crate) fn declared_version(&self) -> Option<Cow<'n, str>> {
        match &self.body {
            Body::Built(b) => b.declared_version(),
            Body::Given(_) | Body::Owned(_) => None,
        }
    }

    /// The record, borrowed; one kept is in `arena`.
    pub(crate) fn get<'b>(&'b self, arena: &'b Flat<'n>) -> RecRef<'b, 'n, N> {
        match (&self.body, self.built.and_then(|at| arena.root_at(at))) {
            (Body::Given(n), _) => RecRef::Given(*n),
            (Body::Built(_), Some(root)) => RecRef::Kept(root),
            (Body::Built(b), None) => RecRef::Built(*b),
            (Body::Owned(s), _) => RecRef::Owned(s),
        }
    }

    /// A record that writes itself and was not kept yet: written at the
    /// end of `arena`, typed as `arena` types (see [`Typing`]), then
    /// checked by `check`, and kept there while the arena holds fewer than
    /// [`KEEP_BUILT`] structures. `None` for any other record.
    fn write_typed<T>(
        &mut self,
        arena: &mut Flat<'n>,
        check: impl FnOnce(&Flat<'n>) -> T,
    ) -> Option<T> {
        let (Body::Built(b), None) = (&self.body, self.built) else {
            return None;
        };
        let start = arena.begin();
        b.build(arena);
        let checked = check(arena);
        if arena.len() <= KEEP_BUILT {
            self.built = u32::try_from(start).ok();
        } else {
            arena.truncate(start);
        }
        Some(checked)
    }

    pub(crate) fn tag<'b>(&'b self) -> &'b str
    where
        'n: 'b,
    {
        match &self.body {
            Body::Given(_) | Body::Built(_) => self.tag,
            Body::Owned(s) => s.tag.as_str(),
        }
    }

    pub(crate) fn xref<'b>(&'b self) -> Option<&'b str>
    where
        'n: 'b,
    {
        match &self.body {
            Body::Given(_) | Body::Built(_) => self.xref,
            Body::Owned(s) => s.xref.as_deref(),
        }
    }

    /// The index of the tag among the standard tags.
    fn standard_tag(&self) -> Option<u16> {
        match &self.body {
            Body::Given(_) | Body::Built(_) => self.standard,
            Body::Owned(s) => s.tag.standard_index(),
        }
    }

    fn line(&self) -> u32 {
        match &self.body {
            Body::Given(n) => n.line(),
            Body::Built(_) => 0,
            Body::Owned(s) => s.line,
        }
    }

    /// Whether the record has neither payload nor substructure.
    pub(crate) fn is_empty(&self) -> bool {
        match &self.body {
            Body::Given(n) => n.payload() == PayloadRef::None && n.children().next().is_none(),
            Body::Built(b) => b.is_empty(),
            Body::Owned(s) => s.payload == Payload::None && s.substructures.is_empty(),
        }
    }

    /// The record as an owned structure, as it is.
    pub(crate) fn to_structure(&self, arena: &Flat<'n>) -> Structure {
        if let Some(s) = match self.body {
            Body::Built(b) => b.owned(),
            Body::Given(_) | Body::Owned(_) => None,
        } {
            return s;
        }
        let mut scratch = Flat::default();
        with_node!(self.get(arena), scratch, |n| n.to_owned_structure())
    }

    /// The record, copied first if it is not owned yet.
    fn make_owned(&mut self, arena: &Flat<'n>) -> &mut Structure {
        if !matches!(self.body, Body::Owned(_)) {
            self.body = Body::Owned(Box::new(self.to_structure(arena)));
            self.built = None;
        }
        match &mut self.body {
            Body::Owned(s) => s,
            Body::Given(_) | Body::Built(_) => unreachable!("made owned above"),
        }
    }

    /// The record as an owned structure, taken out (an owned record is
    /// left empty).
    pub(crate) fn take_structure(&mut self, arena: &Flat<'n>) -> Structure {
        match &mut self.body {
            Body::Owned(s) => std::mem::take(s),
            Body::Given(_) | Body::Built(_) => self.to_structure(arena),
        }
    }

    /// What the pass found of a record it left as given and found clean
    /// as it ended (`state`: [`Conformed::state`]).
    pub(crate) fn checked(&self, state: u32) -> Option<Checked> {
        (matches!(self.body, Body::Given(_)) && self.clean == state).then_some(Checked {
            rtype: self.rtype,
            size: self.size,
        })
    }

    /// The record, known clean: a pass over the same records found it so
    /// (`checked`), and nothing it depends on changed since but what the
    /// pass looks at again (its identifiers and the types they name, the
    /// header). The pass looks at it again only once those change.
    pub(crate) fn known_clean(mut self, checked: Checked) -> Self {
        self.clean = KNOWN_CLEAN;
        self.rtype = checked.rtype;
        self.size = checked.size;
        self
    }

    /// The node of a record given that the pass left as it was; the record
    /// as an owned structure when the pass rewrote it.
    pub(crate) fn into_given(self) -> Result<N, Structure> {
        match self.body {
            Body::Given(n) => Ok(n),
            Body::Owned(s) => Err(*s),
            Body::Built(_) => Err(self.to_structure(&Flat::default())),
        }
    }

    /// The record as an owned structure.
    pub(crate) fn into_structure(self) -> Structure {
        match self.body {
            Body::Owned(s) => *s,
            Body::Given(_) | Body::Built(_) => self.to_structure(&Flat::default()),
        }
    }
}

struct Conformer<'n> {
    rules: &'static VersionRules,
    schema: &'static Schema,
    family: Family,
    repairs: Vec<Repair>,
    /// Every record identifier in use, with the type of the record it
    /// names as the pass found it (`None` for extension records).
    xrefs: XrefIndex<'n, Option<StructId>>,
    /// The same, by the ids of a typed dataset's store, in place of
    /// `xrefs` when its records are written one at a time ([`Direct`]).
    ids: Option<std::rc::Rc<IdTypes<'n>>>,
    /// The new identifier of each identifier that was invalid.
    renamed: HashMap<Box<str>, Box<str>>,
    /// Every identifier the records hold, pointers' included, gathered
    /// before the first new identifier is made
    /// ([`Conformer::gather_pointed`]).
    pointed: Option<HashSet<Box<str>>>,
    /// Extension tags of relocated standard structures, with their URIs.
    schma: Vec<(String, String)>,
    /// The tags from the record to the structure being repaired.
    path: Vec<Tag>,
    /// The submitter a synthesised 5.5.1 `HEAD.SUBM` points to.
    submitter: Option<Box<str>>,
    /// Records to add before the trailer.
    new_records: Vec<Structure>,
    /// The aliases `HEAD.SCHMA` declares (7.x).
    aliases: Aliases,
    /// Every extension tag of the dataset as the pass began, when a repair
    /// may need it (7.x relocations).
    used: Option<HashSet<String>>,
    /// The generation of what a record found clean depends on: the types
    /// in `xrefs`, `renamed` and `aliases`. It changes when they do.
    state: u32,
    /// The number of repairs when the last pass began.
    pass_start: usize,
    /// What the check needs of each type ([`SchemaIndex`]).
    tables: &'static SchemaIndex,
    /// The type of each substructure, by tag.
    picker: Picker,
    /// Room for a record that writes itself ([`Build`]), kept from pass to
    /// pass.
    scratch: Flat<'n>,
    /// The records that write themselves, written once and kept, one after
    /// the other (each typed as it is written: [`Typing`]).
    arena: Flat<'n>,
    /// Whether the last pass added records.
    added: bool,
    /// The order of the typed model the records come from, if they do.
    order: Option<&'static FieldOrder>,
    /// Whether pointers are taken to name records of their types, which
    /// the pass cannot see (records written one at a time:
    /// [`streamed`]).
    trust: bool,
    /// Whether the header can no longer declare extension tags: a
    /// relocated structure is then a plain extension structure.
    no_declare: bool,
}

/// What a record of the typed model checks of itself as it writes itself
/// into a [`Flat`] arena ([`Flat::open_typed`]), so that the check of the
/// pass does not walk it again: each structure is placed by its standard
/// tag under the type of its superstructure, and a payload made valid by
/// its type (an enumeration value, a date of the commonest shapes, a
/// number) is trusted; texts, pointers and anything untyped are left to
/// the check ([`Conformer::is_clean_typed`]).
#[derive(Clone, Copy)]
pub(crate) struct Typing {
    picker: Picker,
    rules: &'static VersionRules,
    schema: &'static Schema,
    tables: &'static SchemaIndex,
    v7: bool,
}

impl Typing {
    /// The tables of `version`.
    pub(crate) fn of(version: GedcomVersion) -> Typing {
        let rules = version.rules();
        Typing {
            picker: Picker::of(rules),
            rules,
            schema: rules.spec,
            tables: rules.index,
            v7: Family::of(rules) == Family::V7,
        }
    }

    /// The type of a structure tagged with the standard tag `standard`
    /// under a structure of type `sup`, as [`Conformer::clean`] picks it;
    /// `None` when the tag has no place there, or when it is occurrence
    /// `count` of its tag there and the type takes fewer.
    #[inline]
    pub(crate) fn place(
        &self,
        sup: StructId,
        standard: Option<u16>,
        is_pointer: bool,
        count: u32,
    ) -> Option<StructId> {
        let (ty, tag) = self.picker.place(sup, standard?, is_pointer)?;
        if count > 1
            && self
                .picker
                .max(sup, tag)
                .is_some_and(|max| count > u32::from(max))
        {
            return None;
        }
        Some(ty)
    }

    /// Whether the payload of a structure of type `ty` is valid without a
    /// look ([`Conformer::clean_payload`]): none where none is valid, or a
    /// text made valid for `claim`, the kind of payload of its type. A
    /// typed enumeration value stands for the values of several sets (the
    /// statuses of each ordinance): it is looked up in its type's set.
    #[inline]
    pub(crate) fn trusts(
        &self,
        ty: StructId,
        payload: &FlatPayload<'_>,
        claim: Option<Kind>,
    ) -> bool {
        let (kind, arg) = self.schema.kind(ty);
        match payload {
            FlatPayload::None => match kind {
                Kind::Pointer => false,
                Kind::NullablePointer => true,
                kind => payload::empty_is_valid(kind),
            },
            FlatPayload::Text(..) if claim != Some(kind) => false,
            FlatPayload::Text(t, _) => match kind {
                Kind::Enum | Kind::ListEnum => {
                    let family = if self.v7 { Family::V7 } else { Family::V551 };
                    self.schema.enum_set(arg).is_some_and(|set| {
                        if kind == Kind::Enum {
                            clean::is_canonical(set, t, family)
                        } else {
                            clean::is_canonical_list(set, t, family)
                        }
                    })
                }
                _ => true,
            },
            FlatPayload::Pointer(..) => false,
        }
    }

    /// Whether the payload of a structure of type `ty`, `claim` its kind
    /// as the typed model made it valid ([`Typing::trusts`]) and `special`
    /// the [`special_bytes`] of a text, stays as it is
    /// ([`Conformer::clean_payload_known`] without documented aliases,
    /// which stream mode has none of); `None` for a pointer, which must
    /// name a record.
    ///
    /// [`special_bytes`]: crate::writer::special_bytes
    #[inline]
    pub(crate) fn check(
        &self,
        ty: StructId,
        payload: &FlatPayload<'_>,
        claim: Option<Kind>,
        special: u8,
    ) -> Option<bool> {
        let (kind, _) = self.schema.kind(ty);
        let raw = match payload {
            _ if matches!(kind, Kind::Pointer | Kind::NullablePointer) => {
                return match payload {
                    FlatPayload::None => Some(kind == Kind::NullablePointer),
                    FlatPayload::Pointer(..) => None,
                    FlatPayload::Text(..) => Some(false),
                };
            }
            FlatPayload::Pointer(..) => return Some(false),
            FlatPayload::Text(t, _) if claim == Some(kind) => {
                if self.trusts(ty, payload, claim) {
                    return Some(true);
                }
                t
            }
            FlatPayload::Text(t, _) => t,
            FlatPayload::None => "",
        };
        // Any text without a banned character is a text; nearly every
        // text has no byte that can start one.
        if matches!(kind, Kind::Text | Kind::ListText) {
            return Some(!payload::has_banned_known(raw, special, self.rules));
        }
        if raw.is_empty() && payload::empty_is_valid(kind) {
            return Some(true);
        }
        Some(clean::is_clean_value(
            self.schema,
            self.tables,
            self.rules,
            ty,
            raw,
            raw,
            Some(special),
        ))
    }

    /// Whether a pointer of a structure of type `ty` in a record of type
    /// `rtype` is one the 7.x pointer rules between records concern.
    #[inline]
    pub(crate) fn is_graph_pointer(&self, rtype: StructId, ty: StructId) -> bool {
        self.v7 && self.tables.is_graph_pointer(self.schema, rtype, ty)
    }

    /// The record type a pointer of a structure of type `ty` names.
    #[inline]
    pub(crate) fn pointer_target(&self, ty: StructId) -> StructId {
        self.schema.kind(ty).1
    }

    /// The required substructures of a type: the tag (an index in the
    /// tables' tags) and the fewest occurrences.
    #[inline]
    pub(crate) fn required(&self, ty: StructId) -> impl Iterator<Item = (u8, u8)> + use<> {
        self.tables.required(ty).map(|c| (c.tag, c.min))
    }

    /// The index in the tables' tags of a standard tag.
    #[inline]
    pub(crate) fn spec_tag(&self, standard: u16) -> Option<u8> {
        self.picker.spec_tag(standard)
    }

    /// Whether the version is 7.x, where a structure has a payload or a
    /// substructure.
    #[inline]
    pub(crate) fn v7(&self) -> bool {
        self.v7
    }

    /// Whether a type is a 7.x note translation, which says its language
    /// or its media type.
    #[inline]
    pub(crate) fn is_note_tran(&self, ty: StructId) -> bool {
        self.tables.is_note_tran(ty)
    }
}

/// A record an identifier names: its type, `None` for an extension record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Named(pub(crate) Option<StructId>);

/// What becomes of a structure after its repair.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fate {
    Keep,
    /// It becomes an extension structure; the repair is reported already.
    Extension,
    /// It holds nothing and goes; the repair is reported already.
    Remove,
}

fn text(tag: &str, value: &str) -> Structure {
    Structure {
        payload: Payload::Text(value.into()),
        ..Structure::new(tag)
    }
}

fn pointer(tag: &str, xref: &str) -> Structure {
    Structure {
        payload: Payload::Pointer(Xref::new(xref)),
        ..Structure::new(tag)
    }
}

/// File extensions and their media types; the first extension of a type
/// is its 5.5.1 file format (`image/jpeg` is `jpg`).
const FILE_TYPES: [(&[&str], &str); 16] = [
    (&["jpg", "jpeg"], "image/jpeg"),
    (&["png"], "image/png"),
    (&["gif"], "image/gif"),
    (&["bmp"], "image/bmp"),
    (&["tif", "tiff"], "image/tiff"),
    (&["webp"], "image/webp"),
    (&["svg"], "image/svg+xml"),
    (&["pdf"], "application/pdf"),
    (&["txt"], "text/plain"),
    (&["htm", "html"], "text/html"),
    (&["wav"], "audio/wav"),
    (&["mp3"], "audio/mpeg"),
    (&["ogg"], "audio/ogg"),
    (&["mp4"], "video/mp4"),
    (&["mpg", "mpeg"], "video/mpeg"),
    (&["avi"], "video/x-msvideo"),
];

/// The media type of a file from its extension; `application/octet-stream`
/// when unknown.
fn media_type(path: &str) -> &'static str {
    let ext = extension(path);
    FILE_TYPES
        .iter()
        .find(|(exts, _)| exts.contains(&ext.as_str()))
        .map_or("application/octet-stream", |&(_, media_type)| media_type)
}

/// The 5.5.1 file format of a file from its extension (p. 54): a format of
/// the list under its own spelling (`jpeg` is `jpg`), else the extension
/// (accepted practice: the list is open), else `unknown`.
fn multimedia_format(path: &str) -> String {
    let ext = extension(path);
    match ext.as_str() {
        "jpeg" => "jpg".into(),
        "tiff" => "tif".into(),
        "" => "unknown".into(),
        _ => ext,
    }
}

/// The 5.5.1 file format of a media type: the extension of a known type
/// (`application/pdf` is `pdf`, `image/jpeg` is `jpg`), else the subtype
/// without its `x-` or `vnd.` prefix and `+` suffix (`image/x-icon` is
/// `icon`), else the type, else `unknown`; lower case.
fn file_format_of_media_type(media_type: &str) -> String {
    let lower = media_type.to_ascii_lowercase();
    let mt = lower.split(';').next().unwrap_or("").trim();
    if let Some((exts, _)) = FILE_TYPES.iter().find(|&&(_, t)| t == mt) {
        return exts.first().copied().unwrap_or("unknown").to_string();
    }
    let (ty, sub) = mt.split_once('/').unwrap_or((mt, ""));
    let sub = sub.split('+').next().unwrap_or("");
    let sub = sub
        .strip_prefix("x-")
        .or_else(|| sub.strip_prefix("vnd."))
        .unwrap_or(sub);
    let token = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            .collect()
    };
    [token(sub), token(ty)]
        .into_iter()
        .find(|t| !t.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn extension(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or("");
    let name = name.split(['?', '#']).next().unwrap_or("");
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

impl<'n> Conformer<'n> {
    fn repair(&mut self, line: u32, kind: RepairKind, detail: String) {
        self.repairs.push(Repair {
            line,
            kind,
            detail: detail.into(),
        });
    }

    /// The path of the structure being repaired, ending with `tag`.
    fn here(&self, tag: &str) -> String {
        let mut p = String::new();
        for t in &self.path {
            p.push_str(t.as_str());
            p.push('.');
        }
        p.push_str(tag);
        p
    }

    fn new(version: GedcomVersion) -> Self {
        let rules = version.rules();
        let schema = rules.spec;
        let family = Family::of(rules);
        let mut c = Conformer {
            rules,
            schema,
            family,
            repairs: Vec::new(),
            xrefs: XrefIndex::default(),
            ids: None,
            renamed: HashMap::new(),
            pointed: None,
            schma: Vec::new(),
            path: Vec::new(),
            submitter: None,
            new_records: Vec::new(),
            aliases: Aliases::default(),
            used: None,
            state: 1,
            pass_start: 0,
            tables: rules.index,
            picker: Picker::of(rules),
            scratch: Flat::default(),
            arena: Flat::default(),
            added: false,
            order: None,
            trust: false,
            no_declare: false,
        };
        c.arena.set_typing(Some(Typing::of(version)));
        c
    }

    fn pass<N: Node<'n>>(&mut self, records: &mut Vec<Rec<'n, N>>, pass: usize) {
        // Whether nothing was repaired since the last pass began.
        let quiet = pass > 0 && self.repairs.len() == self.pass_start;
        self.pass_start = self.repairs.len();
        self.header_and_trailer(records);
        let quiet = quiet && self.repairs.len() == self.pass_start;
        let mut scratch = std::mem::take(&mut self.scratch);
        let head = records.first().filter(|r| r.tag() == "HEAD");
        let aliases = match head.map(|h| h.get(&self.arena)) {
            Some(h) => with_node!(h, scratch, |h| Aliases::of(Some(h), self.schema)),
            None => Aliases::of(None::<&Structure>, self.schema),
        };
        let aliases_changed = aliases != self.aliases;
        self.aliases = aliases;
        // The identifiers and the types they name only change with a
        // repair: after a pass that repaired nothing, they are as they were
        // (the records it added are in `xrefs` already).
        if !quiet || aliases_changed {
            let renamed = self.renamed.len();
            let previous = std::mem::take(&mut self.xrefs);
            self.xrefs(records);
            if pass == 0
                || aliases_changed
                || previous != self.xrefs
                || self.renamed.len() != renamed
            {
                self.state += 1;
            }
        }
        if pass == 0 {
            // The records known clean are clean under what they depend on
            // as the pass begins.
            for r in records.iter_mut().filter(|r| r.clean == KNOWN_CLEAN) {
                r.clean = self.state;
            }
        }
        self.submitter = self.submitter.take().or_else(|| {
            records
                .iter()
                .find(|r| r.tag() == "SUBM" && r.xref().is_some())
                .and_then(|r| r.xref().map(Box::from))
        });
        // The extension tags of the header before it is completed (as the
        // pass begins), for relocations.
        let head_tags = if self.family == Family::V7 {
            records
                .first()
                .filter(|r| r.tag() == "HEAD")
                .map(|h| extension_tags(h, &self.arena))
        } else {
            None
        };
        // Completing the header twice changes nothing: after a quiet pass,
        // it is complete.
        if !quiet {
            self.complete_head(records);
        }

        // Which records need a repair, without changing any.
        let head_ty = self.record_of("HEAD");
        let dirty = self.find_dirty(records, head_ty, &mut scratch);
        let any_dirty = !dirty.is_empty();
        // A relocation (7.x) picks an extension tag no record uses yet.
        self.used = (self.family == Family::V7 && any_dirty).then(|| {
            let mut used: HashSet<String> = head_tags.unwrap_or_default();
            for r in records.iter().skip(1) {
                used.extend(extension_tags(r, &self.arena));
            }
            used
        });

        let extensions = self.repair_dirty(records, head_ty, dirty);
        if self.order.is_some() && !extensions.is_empty() {
            // Records that became extension records go after the others,
            // before the trailer, where the typed model writes them.
            let end = records
                .iter()
                .rposition(|r| r.tag() == "TRLR")
                .unwrap_or(records.len());
            let mut moved = Vec::with_capacity(extensions.len());
            for &i in extensions.iter().rev() {
                moved.push(records.remove(i));
            }
            moved.reverse();
            let at = end.saturating_sub(moved.len());
            records.splice(at..at, moved);
        }
        self.path.clear();
        self.added = !self.new_records.is_empty();
        if self.added {
            let added = std::mem::take(&mut self.new_records);
            insert_records(records, added);
        }
        if self.family == Family::V7 {
            self.declare(records);
        }
        self.used = None;
        self.scratch = scratch;
    }

    /// Which records need a repair, found without changing any; their
    /// pointers are looked up in batches. In record order.
    fn find_dirty<N: Node<'n>>(
        &mut self,
        records: &mut [Rec<'n, N>],
        head_ty: Option<StructId>,
        scratch: &mut Flat<'n>,
    ) -> Vec<usize> {
        let mut dirty: Vec<usize> = Vec::new();
        let mut pending = Vec::with_capacity(LOOKUP_BATCH.min(4 * records.len()));
        let mut failed = Vec::new();
        let mut arena = std::mem::take(&mut self.arena);
        // Room for records of a dozen structures, as most are, once one
        // writes itself.
        let room = records.len().saturating_mul(12).min(KEEP_BUILT);
        for (i, r) in records.iter_mut().enumerate() {
            if arena.len() == 0 && matches!(r.body, Body::Built(_)) {
                arena.reserve(room);
            }
            let clean = match r.tag() {
                "TRLR" => true,
                "HEAD" if i == 0 && r.clean == self.state => true,
                "HEAD" if i == 0 => {
                    let mut walk = Walk::default();
                    let clean = match head_ty {
                        Some(ty) => with_node!(r.get(&arena), *scratch, |h| self
                            .is_clean_walked(h, ty, &mut walk)),
                        None => true,
                    };
                    if clean {
                        r.clean = self.state;
                        r.size = u32::try_from(walk.size).unwrap_or(u32::MAX);
                    }
                    clean
                }
                _ if r.clean == self.state => true,
                _ => {
                    let mark = pending.len();
                    let mut walk = Walk {
                        size: 0,
                        pending: Some(&mut pending),
                        record: u32::try_from(i).unwrap_or(u32::MAX),
                        rtype: DATASET,
                    };
                    // A record that writes itself is written once, typed:
                    // when it is so, only what needs a look is looked at.
                    let written =
                        r.write_typed(&mut arena, |flat| self.is_clean_written(flat, &mut walk));
                    let clean = written.unwrap_or_else(|| {
                        let xref = r.xref();
                        with_node!(r.get(&arena), *scratch, |n| self
                            .is_clean_record(n, xref, &mut walk))
                    });
                    let (size, rtype) = (walk.size, walk.rtype);
                    if clean {
                        r.clean = self.state;
                        r.rtype = rtype;
                        r.size = u32::try_from(size).unwrap_or(u32::MAX);
                    } else {
                        pending.truncate(mark);
                    }
                    if pending.len() >= LOOKUP_BATCH {
                        self.look_up(&mut pending, &mut failed);
                    }
                    clean
                }
            };
            if !clean {
                dirty.push(i);
            }
        }
        self.arena = arena;
        self.look_up(&mut pending, &mut failed);
        if !failed.is_empty() {
            for &i in &failed {
                if let Some(r) = records.get_mut(i as usize) {
                    r.clean = 0;
                }
            }
            dirty.extend(failed.iter().map(|&i| i as usize));
            dirty.sort_unstable();
            dirty.dedup();
        }
        dirty
    }

    /// The repairs, in record order: the dirty records repaired, the others
    /// counted.
    ///
    /// Returns the records that became extension records.
    fn repair_dirty<N: Node<'n>>(
        &mut self,
        records: &mut [Rec<'n, N>],
        head_ty: Option<StructId>,
        dirty: Vec<usize>,
    ) -> Vec<usize> {
        let mut extensions = Vec::new();
        let mut counts: Vec<(u8, u32)> = Vec::new();
        let mut next_dirty = dirty.into_iter().peekable();
        for (i, r) in records.iter_mut().enumerate() {
            let is_dirty = next_dirty.next_if_eq(&i).is_some();
            match r.tag() {
                "TRLR" => continue,
                "HEAD" if i == 0 => {
                    if let (Some(head), true) = (head_ty, is_dirty) {
                        self.path.clear();
                        r.clean = 0;
                        let r = r.make_owned(&self.arena);
                        let _ = self.fix(r, head, true);
                        self.model_order(r, head);
                    }
                    continue;
                }
                _ => {}
            }
            self.path.clear();
            let was_extension = r.tag().starts_with('_');
            if is_dirty {
                (r.clean, r.size) = (0, u32::MAX);
                self.record(r.make_owned(&self.arena), &mut counts);
                if !was_extension && r.tag().starts_with('_') {
                    extensions.push(i);
                }
                continue;
            }
            #[cfg(debug_assertions)]
            self.assert_clean(r);
            // A clean record of a standard type still counts towards the
            // records of its type the dataset permits.
            if r.rtype != DATASET {
                let t = self.schema.tag_id(r.rtype).unwrap_or(0);
                let n = bump(&mut counts, t);
                if self
                    .picker
                    .max(DATASET, t)
                    .is_some_and(|max| n > u32::from(max))
                {
                    if let Some((_, n)) = counts.iter_mut().find(|(k, _)| *k == t) {
                        *n -= 1;
                    }
                    (r.clean, r.size) = (0, u32::MAX);
                    self.record(r.make_owned(&self.arena), &mut counts);
                    if !was_extension && r.tag().starts_with('_') {
                        extensions.push(i);
                    }
                }
            }
        }
        extensions
    }

    /// Checks, in debug builds, that a record found clean is one the repair
    /// leaves as it is.
    #[cfg(debug_assertions)]
    fn assert_clean<N: Node<'n>>(&mut self, r: &Rec<'n, N>) {
        let original = r.to_structure(&self.arena);
        let mut copy = original.clone();
        let before = self.repairs.len();
        let mut counts = Vec::new();
        self.record(&mut copy, &mut counts);
        self.path.clear();
        assert!(
            self.repairs.len() == before && copy == original,
            "a record found clean is repaired: {:?}",
            self.repairs.get(before..)
        );
    }

    /// Whether a record of the typed model, just written into `flat`
    /// (typed as it was written: [`Typing`]), needs no repair: when it is
    /// typed, only what needs a look is looked at.
    fn is_clean_written(&self, flat: &Flat<'n>, walk: &mut Walk<'_>) -> bool {
        let typed = flat
            .typed()
            .then(|| self.is_clean_typed(flat, walk))
            .flatten();
        // What the typed record trusts, the walk of any record finds so too
        // (its pointers are looked up with the others).
        #[cfg(debug_assertions)]
        if typed == Some(true) {
            let mut deferred = Vec::new();
            let mut check = Walk {
                pending: Some(&mut deferred),
                ..Walk::default()
            };
            assert!(
                flat.root()
                    .is_some_and(|n| self.is_clean_record(n, n.xref(), &mut check)),
                "a record of the typed model trusted as written is not clean: {:?}",
                flat.root().map(Node::to_owned_structure)
            );
        }
        // One that is not typed is clean when the repair also leaves its
        // order as it is.
        typed.unwrap_or_else(|| {
            flat.root()
                .is_some_and(|n| self.is_clean_record(n, n.xref(), walk) && self.in_model_order(n))
        })
    }

    /// (i) Completes the header as the writer does for the version
    /// ([`crate::writer`]): `GEDC.VERS` (and in 5.5.1 `GEDC.FORM`,
    /// `CHAR`, `SOUR` and `SUBM`, with a submitter record when there is
    /// none), without an identifier; 7.x drops `CHAR` and `GEDC.FORM`,
    /// which describe the bytes and the form of the input. The writer
    /// rewrites the header of every file it writes, so this is no repair.
    fn complete_head<N: Node<'n>>(&mut self, records: &mut [Rec<'n, N>]) {
        let Some(head) = records.first_mut().filter(|r| r.tag() == "HEAD") else {
            return;
        };
        head.clean = 0;
        let needs = needs_submitter(head.make_owned(&self.arena), self.rules);
        if needs && self.submitter.is_none() {
            // A stub submitter is made.
            self.gather_pointed(records);
        }
        let Some(head) = records.first_mut() else {
            return;
        };
        let head = head.make_owned(&self.arena);
        let submitter = needs.then(|| {
            if let Some(x) = &self.submitter {
                return x.to_string();
            }
            let x = self.fresh("U", self.record_of("SUBM"));
            self.new_records.push(stub_submitter(x.clone()));
            self.submitter = Some(x.as_str().into());
            x
        });
        complete_head(head, self.rules, "UTF-8", submitter);
        // Completing changes the header's substructures and those of its
        // `GEDC` only; a repair puts the rest in order.
        if let Some(head_ty) = self.record_of("HEAD") {
            self.model_order_to(head, head_ty, 2);
        }
    }

    /// The first `@<prefix>n@` no record and no pointer holds
    /// ([`Conformer::gather_pointed`] first); taken, for a record of type
    /// `ty`.
    fn fresh(&mut self, prefix: &str, ty: Option<StructId>) -> String {
        let mut n = 0_usize;
        loop {
            n += 1;
            let candidate = format!("@{prefix}{n}@");
            if !self.is_used(&candidate) {
                self.xrefs.insert(Cow::Owned(candidate.clone()), ty);
                return candidate;
            }
        }
    }

    fn record(&mut self, r: &mut Structure, counts: &mut Vec<(u8, u32)>) {
        let is_ptr = matches!(r.payload, Payload::Pointer(_));
        let tag = r.tag.as_str().to_string();
        match pick(self.rules, DATASET, &tag, is_ptr) {
            Pick::Type(ty) => {
                if self.fix(r, ty, true) == Fate::Extension {
                    self.make_extension(r);
                    return;
                }
                let t = self.schema.tag_id(ty).unwrap_or(0);
                let n = bump(counts, t);
                if let Some(max) = self.picker.max(DATASET, t)
                    && n > u32::from(max)
                {
                    self.repair(
                            r.line,
                            RepairKind::Repeated,
                            format!("a {tag} record more than the {max} permitted, written as an extension record"),
                        );
                    self.make_extension(r);
                    return;
                }
                self.model_order(r, ty);
            }
            // A documented alias of a standard record is that record.
            Pick::Extension => match self.aliases.structs.get(&tag).copied() {
                Some(ty) if self.fix(r, ty, true) == Fate::Extension => self.make_extension(r),
                Some(_) => {}
                None => self.extension(r),
            },
            _ => {
                let new = self.plain_tag(&tag);
                self.repair(
                    r.line,
                    RepairKind::Misplaced,
                    format!(
                        "{tag:?} is not a record of GEDCOM {}: written as {new}",
                        self.schema.version
                    ),
                );
                self.make_extension(r);
            }
        }
    }

    /// (i) One `HEAD` first, one empty `TRLR` last.
    fn header_and_trailer<N: Node<'n>>(&mut self, records: &mut Vec<Rec<'n, N>>) {
        match records.iter().position(|r| r.tag() == "HEAD") {
            None => {
                records.insert(0, Rec::owned(Structure::new("HEAD")));
                self.repair(0, RepairKind::Header, "HEAD added".into());
            }
            Some(0) => {}
            Some(at) => {
                let head = records.remove(at);
                self.repair(head.line(), RepairKind::Header, "HEAD moved first".into());
                records.insert(0, head);
            }
        }
        for r in records.iter_mut().skip(1).filter(|r| r.tag() == "HEAD") {
            r.make_owned(&self.arena).tag = Tag::new("_HEAD");
            self.repair(
                r.line(),
                RepairKind::Header,
                "a second HEAD written as a _HEAD record".into(),
            );
        }
        if let Some(head) = records.first_mut().filter(|h| h.xref().is_some())
            && let Some(x) = head.make_owned(&self.arena).xref.take()
        {
            self.repair(
                head.line(),
                RepairKind::Xref,
                format!("identifier {x} of HEAD left out"),
            );
        }
        let trailers: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.tag() == "TRLR")
            .map(|(i, _)| i)
            .collect();
        let well_formed = trailers.len() == 1
            && trailers.first() == Some(&(records.len() - 1))
            && records
                .last()
                .is_some_and(|t| t.is_empty() && t.xref().is_none());
        if well_formed {
            return;
        }
        let mut line = 0;
        for &i in trailers.iter().rev() {
            let Some(t) = records.get_mut(i) else {
                continue;
            };
            line = t.line();
            if t.is_empty() {
                records.remove(i);
            } else {
                let t = t.make_owned(&self.arena);
                t.tag = Tag::new("_TRLR");
                t.xref = None;
                self.repair(
                    t.line,
                    RepairKind::Header,
                    "a TRLR with content written as a _TRLR record".into(),
                );
            }
        }
        records.push(Rec::owned(Structure::new("TRLR")));
        self.repair(
            line,
            RepairKind::Header,
            "one empty TRLR written last".into(),
        );
    }

    /// (h) Valid, unique identifiers on records; invalid ones renamed.
    /// Fills `xrefs` with every identifier and the type of its record.
    fn xrefs<N: Node<'n>>(&mut self, records: &mut [Rec<'n, N>]) {
        self.xrefs.clear();
        self.xrefs.reserve(records.len());
        let mut keep = Vec::with_capacity(records.len());
        for r in records.iter() {
            let tag = r.tag();
            let ok = match (tag, &r.body) {
                ("HEAD" | "TRLR", _) => true,
                (_, Body::Given(_) | Body::Built(_)) => r
                    .xref
                    .is_none_or(|x| self.take(Cow::Borrowed(x), tag, r.standard)),
                (_, Body::Owned(s)) => s.xref.as_deref().is_none_or(|x| {
                    self.take(Cow::Owned(x.to_string()), tag, s.tag.standard_index())
                }),
            };
            keep.push(ok);
        }
        for r in &self.new_records {
            if let Some(x) = r.xref.as_deref() {
                let ty = self.record_type(r.tag.as_str());
                self.xrefs.insert(Cow::Owned(x.to_string()), ty);
            }
        }
        if keep.contains(&false) {
            self.gather_pointed(records);
        }
        for (r, ok) in records.iter_mut().zip(keep) {
            if ok {
                continue;
            }
            let r = r.make_owned(&self.arena);
            let Some(old) = r.xref.take() else { continue };
            let ty = self.record_type(r.tag.as_str());
            let new = self.unique(&new_xref(self.rules, &old), ty);
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

    /// Whether a record or a pointer holds `xref`.
    fn is_used(&self, xref: &str) -> bool {
        debug_assert!(self.pointed.is_some(), "pointers not gathered");
        self.xrefs.contains_key(xref) || self.pointed.as_ref().is_some_and(|p| p.contains(xref))
    }

    /// Gathers, once, every identifier the records hold, pointers'
    /// included: a new identifier is never one of them, so that it names
    /// nothing a pointer named (a pointer to no record keeps naming none).
    fn gather_pointed<N: Node<'n>>(&mut self, records: &[Rec<'n, N>]) {
        if self.pointed.is_some() {
            return;
        }
        let mut set = HashSet::new();
        let mut add = |x: &str| {
            set.insert(Box::from(x));
        };
        let mut scratch = std::mem::take(&mut self.scratch);
        for r in records {
            with_node!(r.get(&self.arena), scratch, |n| identifiers(n, &mut add));
        }
        for r in &self.new_records {
            identifiers(r, &mut add);
        }
        self.scratch = scratch;
        self.pointed = Some(set);
    }

    /// Takes `xref`, the valid identifier of a record tagged `tag`, unless
    /// a record holds it already.
    fn take(&mut self, xref: Cow<'n, str>, tag: &str, standard: Option<u16>) -> bool {
        if xref_error(&xref, self.rules).is_some() {
            return false;
        }
        let ty = self.record_type_of(tag, standard);
        self.xrefs.insert_new(xref, ty)
    }

    /// `candidate`, or `candidate` with the first `_2`, `_3`… suffix no
    /// record and no pointer holds ([`Conformer::gather_pointed`] first);
    /// taken, for a record of type `ty`.
    fn unique(&mut self, candidate: &str, ty: Option<StructId>) -> String {
        let mut new = candidate.to_string();
        let mut n = 1_usize;
        while self.is_used(&new) {
            n += 1;
            new = numbered_xref(self.rules, candidate, n);
        }
        self.xrefs.insert(Cow::Owned(new.clone()), ty);
        new
    }

    /// The type of the record tagged `tag`: a standard one, or one an alias
    /// stands for.
    fn record_type(&self, tag: &str) -> Option<StructId> {
        self.record_type_of(tag, crate::tree::node::standard_index(tag))
    }

    /// [`Conformer::record_type`] of a tag whose standard index is known.
    fn record_type_of(&self, tag: &str, standard: Option<u16>) -> Option<StructId> {
        self.record_of_standard(tag, standard)
            .or_else(|| self.aliases.structs.get(tag).copied())
    }

    /// The type of the record tagged `tag` the version defines
    /// ([`Schema::record`], from a table).
    fn record_of(&self, tag: &str) -> Option<StructId> {
        self.record_of_standard(tag, crate::tree::node::standard_index(tag))
    }

    fn record_of_standard(&self, tag: &str, standard: Option<u16>) -> Option<StructId> {
        match standard.and_then(|t| self.picker.spec_tag(t)) {
            Some(t) => self.tables.cell(DATASET, t).map(|c| c.first),
            None => self.schema.record(tag),
        }
    }

    /// The extension tag a structure tagged `tag` is written with when it
    /// cannot stay standard: `_TAG`, and as many `_` more as it takes not to
    /// be an alias of a standard structure.
    fn plain_tag(&self, tag: &str) -> String {
        let mut new = extension_tag(self.rules, tag);
        while self.aliases.structs.contains_key(&new) || self.schma.iter().any(|(t, _)| *t == new) {
            new.push('_');
        }
        new
    }

    /// Makes `s` an extension structure: its tag, then its content.
    fn make_extension(&mut self, s: &mut Structure) {
        s.tag = Tag::new(&self.plain_tag(s.tag.as_str()));
        self.extension_content(s, true);
    }

    /// (e, f) Moves a standard structure of type `ty` (valid as such) out
    /// of a place that does not permit it: in 7.x as an extension tag that
    /// `HEAD.SCHMA` declares with the type's URI, when that tag is free;
    /// otherwise as a plain extension. Returns the tag and how it was
    /// declared, for the repair.
    fn relocate(&mut self, s: &mut Structure, ty: Option<StructId>) -> (String, String) {
        let tag = s.tag.as_str().to_string();
        let candidate = extension_tag(self.rules, &tag);
        if let (Family::V7, Some(ty)) = (self.family, ty) {
            let uri = self.schema.uri(ty);
            let ours = self.aliases.structs.get(&candidate) == Some(&ty)
                || self.schma.iter().any(|(t, u)| *t == candidate && *u == uri);
            let free = !self.no_declare
                && !self.aliases.structs.contains_key(&candidate)
                && !self.used.as_ref().is_some_and(|u| u.contains(&candidate))
                && !self.schma.iter().any(|(t, _)| *t == candidate);
            if ours || free {
                if free {
                    self.schma.push((candidate.clone(), uri.clone()));
                }
                s.tag = Tag::new(&candidate);
                return (candidate, format!(" (declared as {uri})"));
            }
        }
        self.make_extension(s);
        (s.tag.as_str().to_string(), String::new())
    }

    /// An extension structure: tags in the grammar, no identifier below the
    /// record, pointers that name a record, no banned character.
    fn extension(&mut self, s: &mut Structure) {
        self.extension_content(s, true);
    }

    fn extension_content(&mut self, s: &mut Structure, top: bool) {
        if !top {
            if !self.rules.is_valid_tag(s.tag.as_str()) {
                let new = extension_tag(self.rules, s.tag.as_str());
                self.repair(
                    s.line,
                    RepairKind::Misplaced,
                    format!("{:?} is not a tag: written as {new}", s.tag.as_str()),
                );
                s.tag = Tag::new(&new);
            }
            self.drop_xref(s);
        }
        match &s.payload {
            Payload::Pointer(p) => {
                let p = p.as_str().to_string();
                if self.resolve(s, &p).is_err() {
                    self.repair(
                        s.line,
                        RepairKind::Pointer,
                        format!("{} {p} names no record: kept as text", s.tag.as_str()),
                    );
                    s.payload = Payload::Text(p.into());
                }
            }
            Payload::Text(_) => self.characters(s),
            Payload::None => {}
        }
        for c in &mut s.substructures {
            self.extension_content(c, false);
        }
    }

    /// Puts the substructures of `s`, a repaired structure of type `ty` of
    /// a typed model's record, and those of its substructures, in the order
    /// the model writes them once it reads them back ([`FieldOrder`]). The
    /// model reads a substructure into the field its tag selects unless a
    /// substructure of that tag went aside before it, it has an identifier,
    /// it has substructures and the field holds a value without any, or the
    /// field holds one structure and has it; anything else goes aside
    /// (`model::driver::read_row_into`, [`ModelRead`]). `s` is repaired: every
    /// payload fits its field. Substructures already in that order, as
    /// nearly all are, stay where they are.
    fn model_order(&self, s: &mut Structure, ty: StructId) {
        self.model_order_to(s, ty, usize::MAX);
    }

    /// [`Conformer::model_order`] of `s` and its substructures down to
    /// `levels` levels (1: `s`'s own substructures).
    fn model_order_to(&self, s: &mut Structure, ty: StructId, levels: usize) {
        let Some(order) = self.order else {
            return;
        };
        let present = ModelRead::of(s.substructures.iter().map(Node::standard_tag));
        let mut read = present.clone();
        let (mut last, mut aside) = (0, false);
        let in_order = s.substructures.iter().all(|c| {
            let Some(rank) = read.read(self.rules.version, order, ty, c) else {
                aside = true;
                return true;
            };
            let ok = !aside && rank >= last;
            last = rank;
            ok
        });
        if !in_order {
            let mut read = present.clone();
            let mut fields: Vec<(u16, Structure)> = Vec::with_capacity(s.substructures.len());
            let mut kept: Vec<Structure> = Vec::new();
            for c in std::mem::take(&mut s.substructures) {
                match read.read(self.rules.version, order, ty, &c) {
                    Some(rank) => fields.push((rank, c)),
                    None => kept.push(c),
                }
            }
            // Stable: the substructures of a field keep their order.
            fields.sort_by_key(|&(rank, _)| rank);
            s.substructures.extend(fields.into_iter().map(|(_, c)| c));
            s.substructures.append(&mut kept);
        }
        if levels <= 1 {
            return;
        }
        let mut read = present;
        for c in &mut s.substructures {
            if read.read(self.rules.version, order, ty, &*c).is_none() || c.substructures.is_empty()
            {
                continue;
            }
            let is_ptr = matches!(c.payload, Payload::Pointer(_));
            if let Pick::Type(cty) = self.picker.pick(ty, &*c, is_ptr) {
                self.model_order_to(c, cty, levels - 1);
            }
        }
    }

    /// Whether record `n` is in the order [`Conformer::model_order`] puts
    /// it in, its substructures too: always without a [`FieldOrder`].
    fn in_model_order<N: Node<'n>>(&self, n: N) -> bool {
        let (Some(order), Some(ty)) = (
            self.order,
            self.record_of_standard(n.tag(), n.standard_tag()),
        ) else {
            return true;
        };
        self.in_order_under(order, n, ty)
    }

    fn in_order_under<'a, M: Node<'a>>(&self, order: &FieldOrder, n: M, ty: StructId) -> bool {
        let present = ModelRead::of(n.children().map(Node::standard_tag));
        let mut read = present.clone();
        let (mut last, mut aside) = (0, false);
        let in_order = n.children().all(|c| {
            let Some(rank) = read.read(self.rules.version, order, ty, c) else {
                aside = true;
                return true;
            };
            let ok = !aside && rank >= last;
            last = rank;
            ok
        });
        if !in_order {
            return false;
        }
        let mut read = present;
        n.children().all(|c| {
            if read.read(self.rules.version, order, ty, c).is_none()
                || c.children().next().is_none()
            {
                return true;
            }
            let is_ptr = matches!(c.payload(), PayloadRef::Pointer(_));
            match self.picker.pick(ty, c, is_ptr) {
                Pick::Type(cty) => self.in_order_under(order, c, cty),
                _ => true,
            }
        })
    }

    fn drop_xref(&mut self, s: &mut Structure) {
        if let Some(x) = s.xref.take() {
            self.repair(
                s.line,
                RepairKind::Xref,
                format!(
                    "identifier {x} of {} left out: only records have one",
                    s.tag.as_str()
                ),
            );
        }
    }

    /// Follows a renamed identifier. `Ok(Some(type))` for a record of a
    /// standard type, `Ok(None)` for an extension record, `@VOID@` (7.x) or a
    /// 5.5.1 pointer to another file; `Err` for a pointer to nothing.
    fn resolve(&mut self, s: &mut Structure, p: &str) -> Result<Option<StructId>, ()> {
        // `@VOID@` points to nothing in 7.x, even when a record had it (and
        // was renamed).
        if self.trust || (self.family == Family::V7 && p == "@VOID@") {
            return Ok(None);
        }
        let p = match self.renamed.get(p) {
            Some(new) => {
                let new = new.to_string();
                s.payload = Payload::Pointer(Xref::new(new.as_str()));
                new
            }
            None => p.to_string(),
        };
        if self.family == Family::V7 && p == "@VOID@" || is_external_pointer(&p, self.family) {
            return Ok(None);
        }
        self.target(&p).map(|n| n.0).ok_or(())
    }

    /// The record `xref` names, if one does.
    #[inline]
    fn target(&self, xref: &str) -> Option<Named> {
        match &self.ids {
            Some(ids) => ids.get(xref),
            None => self.xrefs.get(xref).map(|&ty| Named(ty)),
        }
    }

    /// (h) Leaves out banned characters (5.5.1 tabs become spaces).
    fn characters(&mut self, s: &mut Structure) {
        let Payload::Text(t) = &s.payload else { return };
        let (family, rules) = (self.family, self.rules);
        if !t.chars().any(|c| payload::is_banned(c, rules)) {
            return;
        }
        let mut removed = 0;
        let clean: String = t
            .chars()
            .filter_map(|c| match c {
                '\t' if family == Family::V551 => Some(' '),
                c if payload::is_banned(c, rules) => {
                    removed += 1;
                    None
                }
                c => Some(c),
            })
            .collect();
        let what = if removed == 0 {
            "tabs written as spaces".to_string()
        } else {
            format!("{removed} banned character(s) left out")
        };
        let here = self.here(s.tag.as_str());
        self.repair(s.line, RepairKind::Characters, format!("{here}: {what}"));
        s.payload = Payload::Text(clean.into());
    }

    /// Whether `ty` takes a `tag` substructure (in its text form).
    fn takes(&self, ty: StructId, tag: &str) -> Option<StructId> {
        let tag = tag_index(tag)?;
        self.schema
            .subs_tagged(ty, tag)
            .find(|s| {
                !matches!(
                    self.schema.kind(s.id).0,
                    Kind::Pointer | Kind::NullablePointer
                )
            })
            .map(|s| s.id)
    }

    /// Repairs a structure of type `ty` and its substructures.
    fn fix(&mut self, s: &mut Structure, ty: StructId, record: bool) -> Fate {
        if !record {
            self.drop_xref(s);
        }
        if self.payload(s, ty) == Fate::Extension {
            return Fate::Extension;
        }
        let children = std::mem::take(&mut s.substructures);
        let mut kept = Vec::with_capacity(children.len());
        let mut counts: Vec<(u8, u32)> = Vec::new();
        // A standard tag is an index: no copy of its text.
        self.path.push(s.tag.clone());
        for mut c in children {
            if self.child(&mut c, ty, &mut counts) {
                kept.push(c);
            }
        }
        self.path.pop();
        s.substructures = kept;
        if self.required(s, ty, &counts) == Fate::Extension {
            return Fate::Extension;
        }
        self.special(s, ty);
        // 7.x §1.2: a structure has a payload or a substructure.
        if self.family == Family::V7
            && !record
            && s.substructures.is_empty()
            && s.payload.as_str().is_none_or(str::is_empty)
        {
            let here = self.here(s.tag.as_str());
            if self.schema.kind(ty).0 == Kind::Y {
                s.payload = Payload::Text("Y".into());
                self.repair(
                    s.line,
                    RepairKind::Empty,
                    format!("{here}: empty, written as Y"),
                );
            } else {
                self.repair(
                    s.line,
                    RepairKind::Empty,
                    format!("{here}: empty, left out"),
                );
                return Fate::Remove;
            }
        }
        Fate::Keep
    }

    /// Repairs `c`, a substructure of a structure of type `ty`, counting the
    /// standard ones in `counts`. Returns whether it stays.
    fn child(&mut self, c: &mut Structure, ty: StructId, counts: &mut Vec<(u8, u32)>) -> bool {
        let schema = self.schema;
        let is_ptr = matches!(c.payload, Payload::Pointer(_));
        let tag = c.tag.as_str().to_string();
        match self.picker.pick(ty, &*c, is_ptr) {
            Pick::Type(cty) => {
                // A substructure that needs nothing is left as it is.
                let fate = if self.is_clean(&*c, cty, false) {
                    Fate::Keep
                } else {
                    self.fix(c, cty, false)
                };
                match fate {
                    Fate::Remove => return false,
                    Fate::Extension => self.make_extension(c),
                    Fate::Keep => {
                        let t = schema.tag_id(cty).unwrap_or(0);
                        let n = bump(counts, t);
                        if let Some(max) = self.picker.max(ty, t).filter(|&m| n > u32::from(m)) {
                            let here = self.here(&tag);
                            let (new, declared) = self.relocate(c, Some(cty));
                            self.repair(
                            c.line,
                            RepairKind::Repeated,
                            format!("{here}: more than the {max} permitted, written as {new}{declared}"),
                        );
                        }
                    }
                }
            }
            // A documented alias of a standard structure is that structure,
            // wherever it is (7.x §1.5.1: relocated).
            Pick::Extension => match self.aliases.structs.get(&tag).copied() {
                Some(aty) if self.is_clean(&*c, aty, false) => {}
                Some(aty) => match self.fix(c, aty, false) {
                    Fate::Keep => {}
                    Fate::Remove => return false,
                    Fate::Extension => self.make_extension(c),
                },
                None if self.is_clean_extension(&*c) => {}
                None => self.extension(c),
            },
            other => {
                let here = self.here(&tag);
                let why = match other {
                    Pick::Misplaced => "not permitted here",
                    Pick::Continuation => "a continuation is not a structure",
                    Pick::Unknown => "not a tag of the version",
                    _ => "not a tag",
                };
                // A standard structure that one type only can stand for
                // keeps its meaning where it moves, when it is valid.
                let only = (other == Pick::Misplaced && self.family == Family::V7)
                    .then(|| self.only_type(&tag))
                    .flatten();
                let fate = only.map(|oty| (oty, self.fix(c, oty, false)));
                let (new, declared) = match fate {
                    Some((_, Fate::Remove)) => return false,
                    Some((oty, Fate::Keep)) => self.relocate(c, Some(oty)),
                    Some((_, Fate::Extension)) | None => {
                        self.make_extension(c);
                        (c.tag.as_str().to_string(), String::new())
                    }
                };
                self.repair(
                    c.line,
                    RepairKind::Misplaced,
                    format!("{here}: {why}, written as {new}{declared}"),
                );
            }
        }
        true
    }

    /// The structure type a tag names when the version has exactly one with
    /// that tag.
    fn only_type(&self, tag: &str) -> Option<StructId> {
        let t = tag_index(tag)?;
        let mut found = None;
        for id in 1..self.schema.structs.len() {
            let Ok(id) = StructId::try_from(id) else {
                break;
            };
            if self.schema.tag_id(id) == Some(t) {
                if found.is_some() {
                    return None;
                }
                found = Some(id);
            }
        }
        found
    }

    /// Adds the `HEAD.SCHMA.TAG` declarations of relocated structures.
    fn declare<N: Node<'n>>(&mut self, records: &mut [Rec<'n, N>]) {
        if self.schma.is_empty() {
            return;
        }
        let Some(head) = records.first_mut().filter(|h| h.tag() == "HEAD") else {
            return;
        };
        head.clean = 0;
        let head = head.make_owned(&self.arena);
        let at = head
            .substructures
            .iter()
            .position(|s| s.tag == "SCHMA")
            .unwrap_or_else(|| {
                head.substructures.push(Structure::new("SCHMA"));
                head.substructures.len() - 1
            });
        let Some(schma) = head.substructures.get_mut(at) else {
            return;
        };
        let declared: HashSet<String> = schma
            .substructures
            .iter()
            .filter(|t| t.tag == "TAG")
            .filter_map(|t| t.text()?.split(' ').next().map(str::to_string))
            .collect();
        for (tag, uri) in &self.schma {
            if !declared.contains(tag) {
                schma
                    .substructures
                    .push(text("TAG", &format!("{tag} {uri}")));
            }
        }
        if let Some(head_ty) = self.record_of("HEAD") {
            self.model_order(head, head_ty);
        }
    }

    /// The payload of a structure of type `ty`.
    fn payload(&mut self, s: &mut Structure, ty: StructId) -> Fate {
        let (kind, arg) = self.schema.kind(ty);
        let here = self.here(s.tag.as_str());
        if matches!(kind, Kind::Pointer | Kind::NullablePointer) {
            return self.pointer_payload(s, ty, arg, &here);
        }
        if let Payload::Pointer(p) = &s.payload {
            let p = p.as_str().to_string();
            self.repair(
                s.line,
                RepairKind::Pointer,
                format!("{here} {p}: a pointer where text belongs, written as text"),
            );
            s.payload = Payload::Text(p.into());
        }
        self.characters(s);
        let raw = s.text().unwrap_or("").to_string();
        let set = matches!(kind, Kind::Enum | Kind::ListEnum)
            .then(|| self.schema.enum_set(arg))
            .flatten();
        match (kind, set) {
            (Kind::Enum, Some(set)) if !raw.is_empty() => {
                return self.enumeration(s, ty, set, &raw, &here);
            }
            (Kind::ListEnum, Some(set)) if !raw.is_empty() => {
                return self.list(s, set, &raw, &here);
            }
            _ => {}
        }
        let error = if raw.is_empty() && payload::empty_is_valid(kind) {
            None
        } else if matches!(kind, Kind::Date | Kind::DateExact | Kind::DatePeriod) {
            payload::check(kind, set, &self.aliases.unalias(&raw), self.family)
        } else {
            payload::check(kind, set, &raw, self.family)
        };
        let mime_text = self.family == Family::V7
            && self.schema.name(ty) == "MIME"
            && !raw.is_empty()
            && !raw.to_ascii_lowercase().starts_with("text/");
        match error.or_else(|| mime_text.then(|| "the media type of a text is text/…".into())) {
            Some(why) => self.invalid_payload(s, ty, &raw, &why, &here),
            None => Fate::Keep,
        }
    }

    /// (d) A payload outside its type's grammar, `why`.
    fn invalid_payload(
        &mut self,
        s: &mut Structure,
        ty: StructId,
        raw: &str,
        why: &str,
        here: &str,
    ) -> Fate {
        let kind = self.schema.kind(ty).0;
        let line = s.line;
        match kind {
            // A date or an age: PHRASE in 7.x, a date phrase in 5.5.1.
            Kind::Date | Kind::DatePeriod | Kind::Age if self.family == Family::V7 => {
                if self.takes(ty, "PHRASE").is_some() && s.first("PHRASE").is_none() {
                    s.payload = Payload::None;
                    s.substructures.push(text("PHRASE", raw));
                    self.repair(
                        line,
                        RepairKind::Payload,
                        format!("{here} {raw:?}: {why}; moved to PHRASE"),
                    );
                    return Fate::Keep;
                }
            }
            Kind::Date => {
                let phrase = format!("({raw})");
                if payload::check(kind, None, &phrase, self.family).is_none() {
                    self.repair(
                        line,
                        RepairKind::Payload,
                        format!("{here} {raw:?}: {why}; written as the date phrase {phrase}"),
                    );
                    s.payload = Payload::Text(phrase.into());
                    return Fate::Keep;
                }
            }
            // 7.x file paths are valid URL strings (§2.12, 7.0.17): a path
            // that is not one is written as conversion writes a 5.5.1 file
            // reference, percent-encoded (a local path as a `file` URL).
            Kind::FilePath if self.family == Family::V7 => {
                if let Some(fixed) = crate::convert::text::file_uri(raw)
                    .filter(|f| payload::check(kind, None, f, self.family).is_none())
                {
                    self.repair(
                        line,
                        RepairKind::Payload,
                        format!("{here} {raw:?}: {why}; written as {fixed}"),
                    );
                    s.payload = Payload::Text(fixed.into());
                    return Fate::Keep;
                }
            }
            // 7.x coordinates (§2.15, §2.16, 7.0.17): the canonical form of
            // the same degrees, when it is valid.
            Kind::Lat | Kind::Long if self.family == Family::V7 => {
                if let Some(fixed) = payload::canonical_coordinate(kind, raw, self.family) {
                    self.repair(
                        line,
                        RepairKind::Payload,
                        format!("{here} {raw:?}: {why}; written as {fixed}"),
                    );
                    s.payload = Payload::Text(fixed.into());
                    return Fate::Keep;
                }
            }
            // An event with text: `Y` and a note; text where none belongs:
            // a note.
            Kind::Y | Kind::None if self.takes(ty, "NOTE").is_some() => {
                let (payload, to) = if kind == Kind::Y {
                    (Payload::Text("Y".into()), "Y")
                } else {
                    (Payload::None, "nothing")
                };
                s.payload = payload;
                s.substructures.push(text("NOTE", raw));
                self.repair(
                    line,
                    RepairKind::Payload,
                    format!("{here} {raw:?}: {why}; payload written as {to}, the text as a NOTE"),
                );
                return Fate::Keep;
            }
            _ => {}
        }
        let new = self.plain_tag(s.tag.as_str());
        self.repair(
            line,
            RepairKind::Payload,
            format!("{here} {raw:?}: {why}; written as {new}"),
        );
        Fate::Extension
    }

    /// (a, b) An enumeration value.
    fn enumeration(
        &mut self,
        s: &mut Structure,
        ty: StructId,
        set: &super::schema::EnumSet,
        raw: &str,
        here: &str,
    ) -> Fate {
        if let Some(fixed) = canonical(set, raw, self.family) {
            if fixed != raw {
                self.repair(
                    s.line,
                    RepairKind::EnumCase,
                    format!("{here} {raw:?} written as {fixed}"),
                );
                s.payload = Payload::Text(fixed.into());
            }
            return Fate::Keep;
        }
        // The 5.5.1 file formats are open (accepted practice): a word is
        // written as it is, a media type as its file format (`image/png`
        // as `png`).
        if self.family == Family::V551 && set.name == payload::OPEN_551_FORMATS {
            if payload::is_open_format(raw) {
                return Fate::Keep;
            }
            if raw.contains('/') {
                let fixed = file_format_of_media_type(raw);
                self.repair(
                    s.line,
                    RepairKind::EnumValue,
                    format!("{here} {raw:?}: a media type; written as the file format {fixed}"),
                );
                s.payload = Payload::Text(fixed.into());
                return Fate::Keep;
            }
        }
        if self.family == Family::V7
            && set.values.contains(&"OTHER")
            && self.takes(ty, "PHRASE").is_some()
            && s.first("PHRASE").is_none()
        {
            s.payload = Payload::Text("OTHER".into());
            s.substructures.push(text("PHRASE", raw));
            self.repair(
                s.line,
                RepairKind::EnumValue,
                format!(
                    "{here} {raw:?}: not a value of {}; written as OTHER with a PHRASE",
                    set.name
                ),
            );
            return Fate::Keep;
        }
        let new = extension_tag(self.rules, s.tag.as_str());
        self.repair(
            s.line,
            RepairKind::EnumValue,
            format!(
                "{here} {raw:?}: not a value of {}; written as {new}",
                set.name
            ),
        );
        Fate::Extension
    }

    /// (a, b) A list of enumeration values.
    fn list(
        &mut self,
        s: &mut Structure,
        set: &super::schema::EnumSet,
        raw: &str,
        here: &str,
    ) -> Fate {
        let mut items = Vec::new();
        for item in payload::list_items(raw).filter(|i| !i.is_empty()) {
            let Some(v) = canonical(set, item, self.family) else {
                let new = self.plain_tag(s.tag.as_str());
                self.repair(
                    s.line,
                    RepairKind::EnumValue,
                    format!(
                        "{here} {raw:?}: {item:?} is not a value of {}; written as {new}",
                        set.name
                    ),
                );
                return Fate::Extension;
            };
            items.push(v);
        }
        if items.is_empty() {
            let new = self.plain_tag(s.tag.as_str());
            self.repair(
                s.line,
                RepairKind::EnumValue,
                format!("{here} {raw:?}: no value; written as {new}"),
            );
            return Fate::Extension;
        }
        let fixed = items.join(", ");
        if fixed != raw {
            self.repair(
                s.line,
                RepairKind::EnumCase,
                format!("{here} {raw:?} written as {fixed}"),
            );
            s.payload = Payload::Text(fixed.into());
        }
        Fate::Keep
    }

    /// (c) The payload of a pointer structure.
    fn pointer_payload(
        &mut self,
        s: &mut Structure,
        ty: StructId,
        target: u16,
        here: &str,
    ) -> Fate {
        let kind = self.schema.kind(ty).0;
        let v7 = self.family == Family::V7;
        let line = s.line;
        let new = self.plain_tag(s.tag.as_str());
        let phrase_ok = self.takes(ty, "PHRASE").is_some() && s.first("PHRASE").is_none();
        let to_void = |s: &mut Structure, phrase: Option<String>| {
            s.payload = Payload::Pointer(Xref::new(Xref::VOID));
            if let Some(p) = phrase {
                s.substructures.push(text("PHRASE", &p));
            }
        };
        match s.payload.clone() {
            Payload::Pointer(_) if self.trust => Fate::Keep,
            Payload::Pointer(p) => {
                let p = p.as_str().to_string();
                match self.resolve(s, &p) {
                    Ok(Some(t)) if t == target => Fate::Keep,
                    Ok(None) if v7 && s.pointer().is_some_and(Xref::is_void) => Fate::Keep,
                    Ok(None) if is_external_pointer(&p, self.family) => Fate::Keep,
                    Ok(_) => {
                        let what = self.schema.tag(target);
                        self.repair(
                            line,
                            RepairKind::Pointer,
                            format!("{here} {p}: not a {what} record; written as {new}"),
                        );
                        Fate::Extension
                    }
                    Err(()) if v7 && phrase_ok => {
                        to_void(s, Some(p.clone()));
                        self.repair(
                            line,
                            RepairKind::Pointer,
                            format!("{here} {p}: names no record; written as @VOID@ with a PHRASE"),
                        );
                        Fate::Keep
                    }
                    Err(()) => {
                        s.payload = Payload::Text(p.as_str().into());
                        self.repair(
                            line,
                            RepairKind::Pointer,
                            format!("{here} {p}: names no record; written as {new} with the text"),
                        );
                        Fate::Extension
                    }
                }
            }
            Payload::Text(t) if !t.is_empty() => {
                self.characters(s);
                let t = s.text().unwrap_or("").to_string();
                if v7 && phrase_ok {
                    to_void(s, Some(t.clone()));
                    self.repair(line, RepairKind::Pointer, format!("{here} {t:?}: text where a pointer belongs; written as @VOID@ with a PHRASE"));
                    Fate::Keep
                } else {
                    self.repair(
                        line,
                        RepairKind::Pointer,
                        format!("{here} {t:?}: text where a pointer belongs; written as {new}"),
                    );
                    Fate::Extension
                }
            }
            _ if kind == Kind::NullablePointer => {
                s.payload = Payload::None;
                Fate::Keep
            }
            _ if v7 => {
                to_void(s, None);
                self.repair(
                    line,
                    RepairKind::Pointer,
                    format!("{here}: no pointer; written as @VOID@"),
                );
                Fate::Keep
            }
            _ => {
                self.repair(
                    line,
                    RepairKind::Pointer,
                    format!("{here}: no pointer; written as {new}"),
                );
                Fate::Extension
            }
        }
    }

    /// (g) Required substructures.
    fn required(&mut self, s: &mut Structure, ty: StructId, counts: &[(u8, u32)]) -> Fate {
        let schema = self.schema;
        for cell in self.tables.required(ty) {
            let (t, min, sub) = (cell.tag, cell.min, cell.first);
            let found = counts.iter().find(|(k, _)| *k == t).map_or(0, |(_, n)| *n);
            if found >= u32::from(min) {
                continue;
            }
            let tag = schema.tag(sub);
            let here = self.here(s.tag.as_str());
            let Some(made) = self.synthesise(s, sub) else {
                let new = self.plain_tag(s.tag.as_str());
                self.repair(
                    s.line,
                    RepairKind::Required,
                    format!("{here} lacks {tag}, which nothing can stand for; written as {new}"),
                );
                return Fate::Extension;
            };
            let shown = made.to_gedcom(0, self.rules.version);
            let shown = shown
                .lines()
                .next()
                .unwrap_or("")
                .trim_start_matches("0 ")
                .to_string();
            self.repair(
                0,
                RepairKind::Required,
                format!("{here} lacks {tag}: {shown} added"),
            );
            s.substructures.push(made);
        }
        Fate::Keep
    }

    /// The minimal structure that stands for a missing required `sub` of
    /// `parent`, from the documented list ([`synthesisable`]); `None` when
    /// there is none.
    fn synthesise(&mut self, parent: &Structure, sub: StructId) -> Option<Structure> {
        let schema = self.schema;
        let v7 = self.family == Family::V7;
        if !synthesisable(schema, v7, parent.tag.as_str(), sub) {
            return None;
        }
        let tag = schema.tag(sub);
        let parent_tag = parent.tag.as_str();
        Some(match (parent_tag, tag) {
            (_, "FORM") if parent_tag == "FILE" || parent_tag == "TRAN" => {
                let file = parent.text().unwrap_or("");
                if v7 {
                    text("FORM", media_type(file))
                } else {
                    text("FORM", &multimedia_format(file))
                }
            }
            ("TRAN", "LANG") if v7 => text("LANG", "und"),
            ("ASSO", "ROLE") if v7 => text("ROLE", "OTHER"),
            ("SLGC", "FAMC") if v7 => pointer("FAMC", Xref::VOID),
            _ => text(tag, PLACEHOLDER),
        })
    }

    /// Rules the tables cannot state: a 7.x note translation says its
    /// language or its media type.
    fn special(&mut self, s: &mut Structure, ty: StructId) {
        if self.family == Family::V7
            && self.schema.name(ty) == "NOTE-TRAN"
            && !s
                .substructures
                .iter()
                .any(|c| c.tag == "MIME" || c.tag == "LANG")
        {
            let here = self.here(s.tag.as_str());
            self.repair(
                0,
                RepairKind::Required,
                format!("{here} lacks MIME and LANG: LANG und added"),
            );
            s.substructures.push(text("LANG", "und"));
        }
    }

    /// (j) Moves inline notes out of `r`, a 5.5.1 record of type `ty` over
    /// 32K (`size` bytes as written), longest first, until it fits or no
    /// note longer than the pointer line it would leave is left, each to
    /// new `NOTE` records ([`Conformer::move_note`]) whose identifiers
    /// `fresh` makes: the new records. A record still over the limit then
    /// holds no text whose moving out would shrink it.
    fn move_notes(
        &mut self,
        r: &mut Structure,
        ty: StructId,
        mut size: usize,
        fresh: &mut dyn FnMut(&mut Self) -> String,
    ) -> Vec<Structure> {
        let mut notes = Vec::new();
        movable_notes(self.rules, r, ty, &mut Vec::new(), &mut notes);
        // The bytes each move saves at least: the note's lines, less the
        // pointer line left in its place (`n NOTE @N…@`, at most
        // `POINTER_LINE` bytes).
        let saved = |n: &(usize, Vec<usize>)| n.0.saturating_sub(POINTER_LINE);
        // Longest first; between equals, file order.
        notes.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let tag = r.tag.as_str().to_string();
        let mut added = Vec::new();
        // The pointers to the further parts of notes split, inserted after
        // the first part's once every note is moved (last path first, so
        // that the paths stay right).
        let mut further: Vec<(Vec<usize>, Vec<Structure>)> = Vec::new();
        for note in &notes {
            // Shorter notes save nothing.
            if size <= MAX_RECORD_BYTES || note.0 <= POINTER_LINE {
                break;
            }
            size = size.saturating_sub(saved(note));
            let Some(at) = node_at(r, &note.1) else {
                continue;
            };
            let pointers = self.move_note(at, &tag, &mut added, fresh);
            if !pointers.is_empty() {
                further.push((note.1.clone(), pointers));
            }
        }
        insert_after(r, further);
        added
    }

    /// (j) Moves inline notes out of 5.5.1 records over 32K, longest first,
    /// until the record fits or no note whose moving shrinks it is left.
    fn record_sizes<N: Node<'n>>(&mut self, records: &mut Vec<Rec<'n, N>>) {
        let mut over = Vec::new();
        let mut scratch = std::mem::take(&mut self.scratch);
        for (i, rec) in records.iter().enumerate() {
            let Some(ty) = self.record_of_standard(rec.tag(), rec.standard_tag()) else {
                continue;
            };
            // The bound the last look found, when there is one.
            if rec.size as usize <= MAX_RECORD_BYTES {
                continue;
            }
            let size = with_node!(rec.get(&self.arena), scratch, |n| encoded_len(
                n,
                0,
                self.family
            ));
            if size > MAX_RECORD_BYTES {
                over.push((i, ty, size));
            }
        }
        self.scratch = scratch;
        if over.is_empty() {
            return;
        }
        self.gather_pointed(records);
        let mut added = Vec::new();
        for (i, ty, size) in over {
            let Some(rec) = records.get_mut(i) else {
                continue;
            };
            let r = rec.make_owned(&self.arena);
            let note_ty = self.record_of("NOTE");
            let moved = self.move_notes(r, ty, size, &mut |c: &mut Self| c.unique("@N1@", note_ty));
            if moved.is_empty() {
                continue;
            }
            added.extend(moved);
            (rec.clean, rec.size) = (0, u32::MAX);
        }
        if !added.is_empty() {
            insert_records(records, added);
        }
    }

    /// Moves the inline note `at` of a `tag` record over 32K to new `NOTE`
    /// records (`added`) with the identifiers `fresh` makes, one, or several when the note is over 32K itself
    /// ([`note_parts`]): `at` points to the first, and the pointers to the
    /// others, to go after it, are returned.
    fn move_note(
        &mut self,
        at: &mut Structure,
        tag: &str,
        added: &mut Vec<Structure>,
        fresh: &mut dyn FnMut(&mut Self) -> String,
    ) -> Vec<Structure> {
        let line = at.line;
        let text = match std::mem::take(&mut at.payload) {
            Payload::Text(t) => t,
            other => {
                at.payload = other;
                return Vec::new();
            }
        };
        let parts = note_parts(&text);
        let mut xrefs: Vec<String> = Vec::with_capacity(parts.len());
        for _ in &parts {
            xrefs.push(fresh(self));
        }
        let mut substructures = std::mem::take(&mut at.substructures);
        at.payload = Payload::Pointer(Xref::new(xrefs.first().map_or("@N1@", String::as_str)));
        for (part, xref) in parts.into_iter().zip(&xrefs) {
            added.push(Structure {
                xref: Some(Xref::new(xref.as_str())),
                payload: Payload::Text(part.into()),
                // What the note holds goes with its first part.
                substructures: std::mem::take(&mut substructures),
                ..Structure::new("NOTE")
            });
        }
        let detail = if xrefs.len() > 1 {
            format!(
                "a {tag} record over 32K: a note over 32K moved to the NOTE records {}, split at line breaks",
                xrefs.join(", ")
            )
        } else {
            format!(
                "a {tag} record over 32K: a note moved to the NOTE record {}",
                xrefs.join("")
            )
        };
        self.repair(line, RepairKind::RecordSize, detail);
        xrefs
            .iter()
            .skip(1)
            .map(|x| Structure {
                payload: Payload::Pointer(Xref::new(x.as_str())),
                ..Structure::new("NOTE")
            })
            .collect()
    }
}

/// Where a record goes in the order the typed model writes records in
/// (`model::Dataset::records`): the header, submitters, submissions,
/// individuals, families, shared notes, sources, repositories, multimedia
/// objects, other records, the trailer.
pub(crate) fn record_rank(tag: &str) -> u8 {
    match tag {
        "HEAD" => 0,
        "SUBM" => 1,
        "SUBN" => 2,
        "INDI" => 3,
        "FAM" => 4,
        "NOTE" | "SNOTE" => 5,
        "SOUR" => 6,
        "REPO" => 7,
        "OBJE" => 8,
        "TRLR" => 10,
        _ => 9,
    }
}

/// Inserts the records a repair made (a stub submitter, notes moved out of
/// a 5.5.1 record), in order, each before the first record of a later
/// [`record_rank`]: where the typed model, which keeps records by type,
/// writes them once read back, and where a tree, which keeps records in
/// file order, keeps them. Either way, writing what was written again
/// changes nothing.
fn insert_records<'n, N: Node<'n>>(records: &mut Vec<Rec<'n, N>>, mut added: Vec<Structure>) {
    // Room for them only: doubling the records would cost more.
    records.reserve_exact(added.len());
    // Stable: records of a rank keep their order.
    added.sort_by_key(|s| record_rank(s.tag.as_str()));
    let mut added = added.into_iter().peekable();
    while let Some(first) = added.next() {
        let rank = record_rank(first.tag.as_str());
        let mut run = vec![first];
        while let Some(s) = added.next_if(|s| record_rank(s.tag.as_str()) == rank) {
            run.push(s);
        }
        let at = records
            .iter()
            .position(|r| record_rank(r.tag()) > rank)
            .unwrap_or(records.len());
        records.splice(at..at, run.into_iter().map(Rec::owned));
    }
}

/// The extension tags of a record and its substructures.
fn extension_tags<'n, N: Node<'n>>(r: &Rec<'n, N>, arena: &Flat<'n>) -> HashSet<String> {
    fn walk<'a, M: Node<'a>>(n: M, out: &mut HashSet<String>) {
        let mut stack = vec![n];
        while let Some(n) = stack.pop() {
            let tag = n.tag();
            if tag.starts_with('_') && !out.contains(tag) {
                out.insert(tag.to_string());
            }
            stack.extend(n.children());
        }
    }
    let mut out = HashSet::new();
    let mut scratch = Flat::default();
    with_node!(r.get(arena), scratch, |n| walk(n, &mut out));
    out
}

/// Whether the repair can stand for a missing required `sub` of a structure
/// tagged `parent` (the list of [`Conformer::synthesise`]): a `FORM` of a
/// `FILE` or `TRAN`, a documented 7.x substructure, or a placeholder for a
/// leaf of a text or open enumeration type. Nothing else is invented.
pub(crate) fn synthesisable(schema: &Schema, v7: bool, parent: &str, sub: StructId) -> bool {
    match (parent, schema.tag(sub)) {
        (_, "FORM") if parent == "FILE" || parent == "TRAN" => true,
        ("TRAN", "LANG") | ("ASSO", "ROLE") | ("SLGC", "FAMC") if v7 => true,
        _ => {
            let (kind, arg) = schema.kind(sub);
            let open = kind == Kind::Enum && schema.enum_set(arg).is_some_and(|e| e.open);
            // A placeholder only stands for a leaf: a structure with
            // required parts of its own (a FILE needs its FORM) is not
            // invented.
            let leaf = schema.subs(sub).iter().all(|s| s.min == 0);
            (kind == Kind::Text || open) && leaf
        }
    }
}

/// The types of `version` the spec names `names` stand for that a structure
/// of the tag `tag` is read in, `pointer` saying whether its payload is a
/// pointer. A row of the reader stands for several types, of several tags
/// (the event row for `BIRT` and for `EVEN`), and of one tag in several
/// forms (a source citation pointing to a source, or a 5.5.1 citation of a
/// text): only those with the parent's tag are the parent's, and of those,
/// the form its payload selects, as the conformance pass picks it, so that
/// the reader and the pass read a repaired substructure back alike. When no
/// form fits the payload, every type of the tag is kept.
pub(crate) fn type_ids(
    version: GedcomVersion,
    names: &[&str],
    tag: &str,
    pointer: bool,
) -> Vec<StructId> {
    let schema = version.rules().spec;
    let of_tag: Vec<StructId> = names
        .iter()
        .filter_map(|name| {
            (1..schema.structs.len())
                .filter_map(|id| StructId::try_from(id).ok())
                .find(|&id| index::name_is(schema, usize::from(id), name))
        })
        .filter(|&id| schema.tag(id) == tag)
        .collect();
    let of_form: Vec<StructId> = of_tag
        .iter()
        .copied()
        .filter(|&id| matches!(schema.kind(id).0, Kind::Pointer | Kind::NullablePointer) == pointer)
        .collect();
    if of_form.is_empty() { of_tag } else { of_form }
}

/// Whether the conformance repair writes the standard substructure `tag` of
/// a structure of one of the types `sups` as an extension `_TAG`: when some
/// type of `sups` has `tag` and lacks a required substructure nothing can
/// stand for, or when no type of `sups` permits `tag` at all (a standard tag
/// not permitted here, [`RepairKind::Misplaced`]). The form of `tag` is the
/// one its payload selects: `pointer`, whether the substructure's payload is
/// a pointer (the pointer form of a tag has its own required parts). `has`
/// tells which substructure tags the substructure holds. The header is
/// completed by its own rules, never read back.
pub(crate) fn repaired_as_extension(
    version: GedcomVersion,
    sups: &[StructId],
    tag: &str,
    pointer: bool,
    has: impl Fn(&str) -> bool,
) -> bool {
    let rules: &VersionRules = version.rules();
    let schema = rules.spec;
    let Some(index) = index::of(schema) else {
        return false;
    };
    if sups
        .iter()
        .any(|&sup| matches!(schema.name(sup), "HEADER.HEAD" | "HEAD"))
    {
        return false;
    }
    let v7 = version != GedcomVersion::V5_5_1;
    let mut permitted = false;
    for &sup in sups {
        match pick(rules, sup, tag, pointer) {
            Pick::Misplaced => {}
            Pick::Type(cty) => {
                permitted = true;
                let lacks = index.required(cty).any(|cell| {
                    let name = TAGS[usize::from(cell.tag)];
                    !has(name) && !synthesisable(schema, v7, tag, cell.first)
                });
                if lacks {
                    return true;
                }
            }
            _ => return false,
        }
    }
    !sups.is_empty() && !permitted
}

/// The payload of an extension substructure, as [`read_back`] asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExtBody {
    /// No payload.
    None,
    /// A pointer.
    Pointer,
    /// Text.
    Text,
}

impl ExtBody {
    /// The payload kind of `payload`.
    pub(crate) fn of(payload: PayloadRef<'_>) -> Self {
        match payload {
            PayloadRef::None => Self::None,
            PayloadRef::Pointer(_) => Self::Pointer,
            PayloadRef::Text(_) => Self::Text,
        }
    }
}

/// The standard tag an extension substructure `ext` (`_TAG`) of a structure
/// of one of the types `sups` stands for, when the reader reads it back as
/// that substructure: the writer's repair made it of a standard `TAG` that
/// has no extension substructure ([`repaired_as_extension`]) and whose
/// payload is either none or a pointer, the kind the standard `TAG` takes
/// ([`takes_pointer`]); a text payload is never read back. The caller
/// checks the field that takes the tag. `body`: the payload of the
/// substructure; `nested`: whether it has an extension substructure; `has`:
/// the tags of its standard substructures.
///
/// The reader ([`crate::model`]) and the conformance pass's order of
/// substructures ask this one question, so that a repaired structure is
/// read and ordered alike.
pub(crate) fn read_back(
    version: GedcomVersion,
    sups: &[StructId],
    ext: &str,
    body: ExtBody,
    nested: bool,
    has: impl Fn(&str) -> bool,
) -> Option<u16> {
    if nested {
        return None;
    }
    let tag = ext.strip_prefix('_')?;
    let standard = crate::tree::standard_index(tag)?;
    match body {
        ExtBody::None => {}
        ExtBody::Pointer if takes_pointer(version, tag) => {}
        ExtBody::Pointer | ExtBody::Text => return None,
    }
    let pointer = body == ExtBody::Pointer;
    repaired_as_extension(version, sups, tag, pointer, has).then_some(standard)
}

/// Whether the standard substructure `tag` takes a pointer payload in
/// `version`: among the structure types of the version with that tag, some
/// has a pointer payload and none has a text payload. The answer is the
/// tag's own, not a parent's: a misplaced tag has no type under the parent
/// that could say. A tag with a pointer form and a text form (5.5.1 keeps
/// some) takes no pointer read back.
fn takes_pointer(version: GedcomVersion, tag: &str) -> bool {
    let schema = version.rules().spec;
    let mut pointer = false;
    for id in (1..schema.structs.len()).filter_map(|id| StructId::try_from(id).ok()) {
        if schema.tag(id) != tag {
            continue;
        }
        match schema.kind(id).0 {
            Kind::Pointer | Kind::NullablePointer => pointer = true,
            Kind::None => {}
            _ => return false,
        }
    }
    pointer
}

/// The spelling of `value` in `set`: 5.5.1 matches without regard to case
/// (p. 21) and writes the specification's spelling; 7.x upper-cases (and
/// keeps extension values, upper-cased). `None` when no value matches.
fn canonical(set: &super::schema::EnumSet, value: &str, family: Family) -> Option<String> {
    if let Some(v) = set.values.iter().find(|v| v.eq_ignore_ascii_case(value)) {
        return Some((*v).to_string());
    }
    // An open set's own value is kept as it is.
    if set.open {
        return Some(value.to_string());
    }
    if family == Family::V7 {
        let upper = value.to_ascii_uppercase();
        if is_ext_tag(&upper) {
            return Some(upper);
        }
    }
    None
}

/// Counts one more occurrence of tag `t`; returns the count.
fn bump(counts: &mut Vec<(u8, u32)>, t: u8) -> u32 {
    if let Some((_, n)) = counts.iter_mut().find(|(k, _)| *k == t) {
        *n += 1;
        *n
    } else {
        counts.push((t, 1));
        1
    }
}

/// The structure at `path` (indices of substructures) under `r`.
/// The text of a note, in parts that each fit a 5.5.1 record under 32K
/// once written (with its continuation lines): split at line breaks, a line
/// longer than a part between two characters that are not spaces (where
/// `CONC` would split it too), else between any two characters. A text
/// that fits is one part.
fn note_parts(text: &str) -> Vec<String> {
    /// The bytes of text a part holds: room for the lines `CONT` and `CONC`
    /// add (about one in twenty bytes) under the limit.
    const PART: usize = MAX_RECORD_BYTES * 7 / 8;
    if text.len() <= PART {
        return vec![text.to_string()];
    }
    let mut parts = Vec::new();
    let mut current: Option<String> = None;
    for mut line in text.split('\n') {
        loop {
            match &mut current {
                Some(c) if c.len() + 1 + line.len() <= PART => {
                    c.push('\n');
                    c.push_str(line);
                    break;
                }
                Some(_) => parts.extend(current.take()),
                None if line.len() <= PART => {
                    current = Some(line.to_string());
                    break;
                }
                None => {
                    let (head, tail) = line.split_at(cut(line, PART));
                    parts.push(head.to_string());
                    line = tail;
                }
            }
        }
    }
    parts.extend(current);
    parts
}

/// Where to cut `line`, longer than `max` bytes: the last character
/// boundary at most `max` between two characters that are not spaces, else
/// the last character boundary at most `max` (never 0).
fn cut(line: &str, max: usize) -> usize {
    let bytes = line.as_bytes();
    let boundary = (1..=max.min(line.len()))
        .rev()
        .find(|&i| line.is_char_boundary(i))
        .unwrap_or(line.len());
    (max / 2..=boundary)
        .rev()
        .find(|&i| {
            line.is_char_boundary(i)
                && bytes.get(i - 1).is_some_and(|&b| b != b' ')
                && bytes.get(i).is_some_and(|&b| b != b' ')
        })
        .unwrap_or(boundary)
}

/// Inserts each run of structures of `further` after the structure its path
/// names in `r`, the last path first, so that the paths stay right.
fn insert_after(r: &mut Structure, mut further: Vec<(Vec<usize>, Vec<Structure>)>) {
    further.sort_by(|a, b| b.0.cmp(&a.0));
    for (path, run) in further {
        let Some((&last, parent)) = path.split_last() else {
            continue;
        };
        if let Some(p) = node_at(r, parent) {
            let at = (last + 1).min(p.substructures.len());
            p.substructures.splice(at..at, run);
        }
    }
}

fn node_at<'s>(r: &'s mut Structure, path: &[usize]) -> Option<&'s mut Structure> {
    let mut cur = r;
    for &i in path {
        cur = cur.substructures.get_mut(i)?;
    }
    Some(cur)
}

/// The inline notes under `s` (of type `ty`) whose superstructure also
/// takes a note pointer: (size as written, path).
fn movable_notes(
    rules: &VersionRules,
    s: &Structure,
    ty: StructId,
    path: &mut Vec<usize>,
    out: &mut Vec<(usize, Vec<usize>)>,
) {
    let (schema, family) = (rules.spec, Family::of(rules));
    let takes_pointer = matches!(
        pick(rules, ty, "NOTE", true),
        Pick::Type(id) if schema.kind(id).0 == Kind::Pointer
    );
    for (i, c) in s.substructures.iter().enumerate() {
        let is_ptr = matches!(c.payload, Payload::Pointer(_));
        let Pick::Type(cty) = pick(rules, ty, c.tag.as_str(), is_ptr) else {
            continue;
        };
        path.push(i);
        if c.tag == "NOTE" && takes_pointer && c.text().is_some() {
            out.push((encoded_len(c, path.len(), family), path.clone()));
        } else {
            movable_notes(rules, c, cty, path, out);
        }
        path.pop();
    }
}
