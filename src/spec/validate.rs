//! The structure validator: a tree against the tables of one version.
//!
//! It checks what a tree can show — every rule that does not depend on how
//! the lines were written. The line grammar itself (spacing, levels,
//! escapes, line length, continuations) is checked on the text by
//! [`super::lines`].

use std::collections::HashMap;

use super::graph::{Graph, Pointer};
use super::index::{SPEC_TAGS, SchemaIndex};
use super::payload::{self, Family};
use super::schema::{DATASET, Kind, Schema, StructId, tag_index};
use super::{Deviation, DeviationKind};
use crate::tree::Node;
use crate::tree::PayloadRef;
use crate::tree::node::STANDARD_TAGS;
use crate::version::{V551, VersionRules};

/// The largest 5.5.1 record (p. 10: "less than 32K").
pub(crate) const MAX_RECORD_BYTES: usize = 32_767;

/// What a record identifier refers to.
#[derive(Clone, Copy)]
struct Record<'a> {
    /// The record's structure type, when the version defines its tag.
    ty: Option<StructId>,
    tag: &'a str,
}

/// Options of a run.
#[derive(Clone, Copy)]
pub(crate) struct Options {
    /// Estimate the size of each 5.5.1 record as written. Off when the
    /// caller measures the real lines instead.
    pub(crate) record_size: bool,
}

pub(crate) struct Validator<'a> {
    rules: &'static VersionRules,
    schema: &'static Schema,
    family: Family,
    records: HashMap<&'a str, Record<'a>>,
    aliases: Aliases,
    out: Vec<Deviation>,
    /// The pointers between records 7.x rules on ([`super::graph`]).
    graph: Graph,
    /// The record being checked, its type, and its pointers the graph
    /// rules concern: line, tag and pointer.
    current: Option<(&'a str, StructId)>,
    pointers: Vec<(u32, &'a str, &'a str, bool)>,
}

/// The extension tags `HEAD.SCHMA` documents with a standard URI (7.x
/// §1.5.1): a tag that stands for a standard structure type, and a tag that
/// stands for a standard enumeration value, month or calendar.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct Aliases {
    pub(crate) structs: HashMap<String, StructId>,
    pub(crate) words: HashMap<String, String>,
}

impl Aliases {
    /// The aliases a header declares, for `schema`.
    pub(crate) fn of<'a, N: Node<'a>>(head: Option<N>, schema: &Schema) -> Aliases {
        let mut out = Aliases::default();
        if !schema.version.starts_with('7') {
            return out;
        }
        let Some(declarations) = head.and_then(|h| h.children().find(|c| c.tag() == "SCHMA"))
        else {
            return out;
        };
        for t in declarations.children().filter(|t| t.tag() == "TAG") {
            let Some((tag, uri)) = t.payload().as_str().and_then(|p| p.split_once(' ')) else {
                continue;
            };
            let uri = uri.trim();
            let name = uri
                .strip_prefix("https://gedcom.io/terms/v7/")
                .or_else(|| uri.strip_prefix("https://gedcom.io/terms/v7.1/"));
            let Some(name) = name else { continue };
            if let Some(word) = ["enum-", "month-", "cal-"]
                .iter()
                .find_map(|p| name.strip_prefix(p))
            {
                out.words.insert(tag.to_string(), word.to_string());
            } else if let Some(id) = (1..schema.structs.len())
                .filter_map(|i| StructId::try_from(i).ok())
                .find(|&i| {
                    // 7.1 names its types under v7.1/ and keeps reading the
                    // 7.0 URIs of the same names.
                    schema.uri(i) == uri
                        || schema.version == "7.1"
                            && uri.strip_prefix("https://gedcom.io/terms/v7/")
                                == Some(schema.name(i))
                })
            {
                out.structs.insert(tag.to_string(), id);
            }
        }
        out
    }

    /// `text` with every word that is an alias replaced by its standard
    /// tag.
    pub(crate) fn unalias<'t>(&self, text: &'t str) -> std::borrow::Cow<'t, str> {
        if self.words.is_empty() || !text.split(' ').any(|w| self.words.contains_key(w)) {
            return std::borrow::Cow::Borrowed(text);
        }
        let words: Vec<&str> = text
            .split(' ')
            .map(|w| self.words.get(w).map_or(w, String::as_str))
            .collect();
        std::borrow::Cow::Owned(words.join(" "))
    }
}

/// Validates `records` (a whole dataset, header and trailer included).
pub(crate) fn run<'a, N, I>(
    records: I,
    rules: &'static VersionRules,
    options: Options,
) -> Vec<Deviation>
where
    N: Node<'a>,
    I: Iterator<Item = N> + Clone,
{
    let schema = rules.spec;
    let family = Family::of(rules);
    let head = records.clone().next().filter(|r| r.tag() == "HEAD");
    let mut v = Validator {
        rules,
        schema,
        family,
        records: HashMap::new(),
        aliases: Aliases::of(head, schema),
        out: Vec::new(),
        graph: Graph::default(),
        current: None,
        pointers: Vec::new(),
    };
    v.index(records.clone());
    v.dataset(&records);
    for r in records {
        v.record(r, options);
    }
    v.out.sort_by_key(|d| d.line);
    v.out
}

fn dev(line: u32, kind: DeviationKind, detail: String) -> Deviation {
    Deviation {
        line,
        kind,
        detail: detail.into(),
    }
}

/// Why `xref` (delimiters included) is not an identifier of the version
/// ([`VersionRules::is_valid_xref`]): 7.x `@` `[A-Z0-9_]+` `@` but not
/// `@VOID@` (§1.3); 5.5.1 `@`, a letter, digit or `_`, then any
/// non-control characters but `@`, `@`, at most 22 characters in all
/// (pp. 11, 13, 17).
pub(crate) fn xref_error(xref: &str, rules: &VersionRules) -> Option<String> {
    if rules.is_valid_xref(xref) {
        return None;
    }
    let Some(id) = xref
        .strip_prefix('@')
        .and_then(|x| x.strip_suffix('@'))
        .filter(|id| !id.is_empty())
    else {
        return Some("an identifier is enclosed in `@`".into());
    };
    match Family::of(rules) {
        Family::V7 => {
            if id == "VOID" {
                Some("`@VOID@` is not an identifier".into())
            } else {
                Some(
                    id.chars()
                        .find(|c| !(c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_'))
                        .map_or_else(
                            || "not an identifier".to_string(),
                            |c| format!("{c:?} is not allowed in an identifier"),
                        ),
                )
            }
        }
        Family::V551 => {
            if xref.chars().count() > 22 {
                Some("an identifier has at most 22 characters".into())
            } else if !id.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                Some("an identifier starts with a letter or a digit".into())
            } else {
                Some(
                    id.chars()
                        .find(|&c| c == '@' || c.is_control())
                        .map_or_else(
                            || "not an identifier".to_string(),
                            |c| format!("{c:?} is not allowed in an identifier"),
                        ),
                )
            }
        }
    }
}

/// Whether a 5.5.1 pointer names a substructure (`@I1!2@`) or a record of
/// another file (`@A:B@`, p. 16): such pointers need no record here. Only
/// an identifier of the 5.5.1 grammar is one: `@!@` names nothing, and is
/// checked (and repaired) as any pointer, which the writer's renaming of
/// an invalid pointer (to `@X_@`, no longer of that form) relies on.
pub(crate) fn is_external_pointer(pointer: &str, family: Family) -> bool {
    family == Family::V551 && pointer.contains(['!', ':']) && V551.is_valid_xref(pointer)
}

/// What a tag is under a superstructure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    /// A standard structure of this type.
    Type(StructId),
    /// An extension structure (`_TAG`).
    Extension,
    /// A tag outside the grammar.
    BadTag,
    /// A tag the version does not define.
    Unknown,
    /// `CONT` or `CONC` as a structure.
    Continuation,
    /// A standard tag the superstructure does not permit.
    Misplaced,
}

/// The type of a `tag` substructure of `sup`, with a pointer payload or not.
/// Among a pointer form and a text form of one tag (5.5.1), the one the
/// payload has.
pub(crate) fn pick(rules: &VersionRules, sup: StructId, tag: &str, is_pointer: bool) -> Pick {
    if !rules.is_valid_tag(tag) {
        return Pick::BadTag;
    }
    if tag.starts_with('_') {
        return Pick::Extension;
    }
    let Some(index) = tag_index(tag) else {
        return Pick::Unknown;
    };
    pick_tagged(rules.spec, sup, index, tag, is_pointer)
}

/// [`pick`] for a node: a standard tag (valid in every version, and no
/// extension) is found by its index instead of its text.
pub(crate) fn pick_node<'a, N: Node<'a>>(
    rules: &VersionRules,
    sup: StructId,
    n: N,
    is_pointer: bool,
) -> Pick {
    Picker::of(rules).pick(sup, n, is_pointer)
}

/// [`pick`] with the tables of one version at hand.
#[derive(Clone, Copy)]
pub(crate) struct Picker {
    rules: &'static VersionRules,
    index: &'static SchemaIndex,
    spec_tags: &'static [u8; STANDARD_TAGS.len()],
}

impl Picker {
    pub(crate) fn of(rules: &VersionRules) -> Picker {
        let rules = rules.version.rules();
        Picker {
            rules,
            index: rules.index,
            spec_tags: &SPEC_TAGS,
        }
    }

    /// The most substructures tagged `TAGS[tag]` a `sup` takes
    /// ([`bounds`]).
    #[inline]
    pub(crate) fn max(self, sup: StructId, tag: u8) -> Option<u8> {
        self.index
            .cell(sup, tag)
            .map_or(Some(0), |c| u8::try_from(c.max).ok())
    }

    /// [`pick_node`].
    #[inline]
    pub(crate) fn pick<'a, N: Node<'a>>(self, sup: StructId, n: N, is_pointer: bool) -> Pick {
        match n.standard_tag().and_then(|t| self.spec_tag(t)) {
            Some(index) => picked(self.index.pick(sup, index, is_pointer), n.tag()),
            None => pick(self.rules, sup, n.tag(), is_pointer),
        }
    }

    /// [`Picker::pick`] of a standard tag, by its index among the tree's
    /// standard tags: the type and the tag's index in the tables, or `None`
    /// when the tag is no structure of `sup` in the version.
    #[inline]
    pub(crate) fn place(
        self,
        sup: StructId,
        standard: u16,
        is_pointer: bool,
    ) -> Option<(StructId, u8)> {
        let index = self.spec_tag(standard)?;
        let ty = self.index.pick(sup, index, is_pointer)?;
        Some((ty, index))
    }

    /// The index in the tables' tags of a tag of the tree's standard table.
    #[inline]
    pub(crate) fn spec_tag(self, standard: u16) -> Option<u8> {
        self.spec_tags
            .get(usize::from(standard))
            .copied()
            .filter(|&t| t != u8::MAX)
    }
}

/// [`pick`] of a tag the tables know, `TAGS[index]`.
fn pick_tagged(schema: &Schema, sup: StructId, index: u8, tag: &str, is_pointer: bool) -> Pick {
    let found = match super::index::of(schema) {
        Some(table) => table.pick(sup, index, is_pointer),
        None => choose(schema, sup, index, is_pointer),
    };
    picked(found, tag)
}

/// What [`choose`] found, as a [`Pick`] of `tag`.
fn picked(found: Option<StructId>, tag: &str) -> Pick {
    match found {
        Some(id) => Pick::Type(id),
        None if matches!(tag, "CONT" | "CONC") => Pick::Continuation,
        None => Pick::Misplaced,
    }
}

/// The substructure type of `sup` tagged `TAGS[index]`: among its forms
/// (5.5.1 has a pointer form and a text form of some tags), the first whose
/// payload is a pointer exactly when `is_pointer`, else the first.
fn choose(schema: &Schema, sup: StructId, index: u8, is_pointer: bool) -> Option<StructId> {
    let mut found = None;
    for s in schema.subs_tagged(sup, index) {
        let ptr = matches!(schema.kind(s.id).0, Kind::Pointer | Kind::NullablePointer);
        if ptr == is_pointer {
            return Some(s.id);
        }
        found = found.or(Some(s.id));
    }
    found
}

/// How many substructures tagged `tag` (an index into `TAGS`) `sup` takes:
/// at least the first, at most the second (`None`: no bound). Alternative
/// forms of one tag share the bounds.
pub(crate) fn bounds(schema: &Schema, sup: StructId, tag: u8) -> (u8, Option<u8>) {
    let mut min = 0;
    let mut max = Some(0);
    for s in schema.subs_tagged(sup, tag) {
        min = min.max(s.min);
        max = match (max, s.max) {
            (_, 0) | (None, _) => None,
            (Some(m), x) => Some(m.max(x)),
        };
    }
    (min, max)
}

impl<'a> Validator<'a> {
    fn push(&mut self, line: u32, kind: DeviationKind, detail: String) {
        self.out.push(dev(line, kind, detail));
    }

    fn index<N: Node<'a>>(&mut self, records: impl Iterator<Item = N>) {
        for r in records {
            let Some(x) = r.xref() else { continue };
            let tag = r.tag();
            if matches!(tag, "HEAD" | "TRLR") {
                self.push(
                    r.line(),
                    DeviationKind::Xref,
                    format!("{tag} has no identifier"),
                );
                continue;
            }
            if let Some(why) = xref_error(x, self.rules) {
                self.push(r.line(), DeviationKind::Xref, format!("{x}: {why}"));
            }
            let ty = self
                .schema
                .record(tag)
                .or_else(|| self.aliases.structs.get(tag).copied());
            if self.records.insert(x, Record { ty, tag }).is_some() {
                self.push(
                    r.line(),
                    DeviationKind::Xref,
                    format!("{x} identifies another record already"),
                );
            }
        }
    }

    /// Header and trailer placement, and the version the header names.
    fn dataset<N: Node<'a>>(&mut self, records: &(impl Iterator<Item = N> + Clone)) {
        let mut first = true;
        let mut trailer = false;
        let mut last_line = 0;
        for r in records.clone() {
            let tag = r.tag();
            last_line = r.line();
            if trailer {
                let what = if tag == "TRLR" {
                    "a second TRLR".to_string()
                } else {
                    format!("{tag} after TRLR")
                };
                self.push(r.line(), DeviationKind::Header, what);
            }
            match tag {
                "HEAD" if !first => self.push(
                    r.line(),
                    DeviationKind::Header,
                    "HEAD is the first record, and the only one".into(),
                ),
                "HEAD" => {}
                "TRLR" if !trailer => {
                    trailer = true;
                    if r.children().next().is_some() {
                        self.push(
                            r.line(),
                            DeviationKind::Header,
                            "TRLR has no substructure".into(),
                        );
                    }
                    if r.payload() != PayloadRef::None {
                        self.push(
                            r.line(),
                            DeviationKind::Header,
                            "TRLR has no payload".into(),
                        );
                    }
                    if first {
                        self.push(
                            r.line(),
                            DeviationKind::Header,
                            "HEAD is the first record".into(),
                        );
                    }
                }
                _ if first => self.push(
                    r.line(),
                    DeviationKind::Header,
                    "HEAD is the first record".into(),
                ),
                _ => {}
            }
            first = false;
        }
        if first {
            self.push(0, DeviationKind::Header, "no HEAD record".into());
        }
        if !trailer {
            self.push(last_line, DeviationKind::Header, "no TRLR record".into());
        }
        if let Some(head) = records.clone().next().filter(|r| r.tag() == "HEAD") {
            self.header(head);
        }
    }

    /// `HEAD.GEDC.VERS` names the version the tables are for.
    fn header<N: Node<'a>>(&mut self, head: N) {
        let vers = head
            .children()
            .find(|c| c.tag() == "GEDC")
            .and_then(|g| g.children().find(|c| c.tag() == "VERS"));
        // A missing GEDC or VERS is a cardinality deviation of its own.
        let Some(vers) = vers else { return };
        let text = vers.payload().as_str().unwrap_or("");
        let want = self.schema.version;
        let ok = match self.family {
            Family::V551 => text == want,
            // 7.0 and 7.0.x (§2.3: the patch number is optional).
            Family::V7 => text.strip_prefix(want).is_some_and(|rest| {
                rest.is_empty() || rest.strip_prefix('.').is_some_and(payload::is_digits)
            }),
        };
        if !ok {
            self.push(
                vers.line(),
                DeviationKind::Header,
                format!("GEDC.VERS {text:?} does not name GEDCOM {want}"),
            );
        }
    }

    fn record<N: Node<'a>>(&mut self, r: N, options: Options) {
        let tag = r.tag();
        if tag == "TRLR" {
            return;
        }
        let ty = self.pick(DATASET, r);
        match ty {
            Ok(ty) => {
                self.begin_graph(r, ty);
                self.structure(r, ty, "", true);
                self.end_graph();
                if options.record_size && self.family == Family::V551 {
                    let size = encoded_len(r, 0, self.family);
                    if size > MAX_RECORD_BYTES {
                        self.push(
                            r.line(),
                            DeviationKind::RecordSize,
                            format!("{tag} record of about {size} bytes, over 32K"),
                        );
                    }
                }
            }
            Err(None) => match self.aliases.structs.get(tag).copied() {
                Some(ty) => {
                    self.begin_graph(r, ty);
                    self.structure(r, ty, "", true);
                    self.end_graph();
                }
                None => self.extension(r),
            },
            Err(Some((kind, why))) => {
                self.push(r.line(), kind, why);
                self.extension(r);
            }
        }
    }

    /// Starts collecting the pointers of record `r`, of type `ty`, that
    /// the 7.x pointer rules concern.
    fn begin_graph<N: Node<'a>>(&mut self, r: N, ty: StructId) {
        self.pointers.clear();
        self.current = (self.family == Family::V7 && self.rules.index.is_graph_record(ty))
            .then(|| r.xref().map(|x| (x, ty)))
            .flatten();
    }

    /// A substructure `n` of type `ty` of the current record: collected
    /// when it is a pointer the rules concern, to a record of the type it
    /// points to (others are deviations of their own).
    fn graph_pointer<N: Node<'a>>(&mut self, n: N, ty: StructId) {
        let Some((_, rtype)) = self.current else {
            return;
        };
        let PayloadRef::Pointer(p) = n.payload() else {
            return;
        };
        let index = self.rules.index;
        if !index.is_graph_pointer(self.schema, rtype, ty) {
            return;
        }
        let want = self.schema.kind(ty).1;
        if self.records.get(p).is_some_and(|r| r.ty == Some(want)) {
            self.pointers
                .push((n.line(), n.tag(), p, index.is_alias(ty)));
        }
    }

    /// Decides the rules for the current record's pointers.
    fn end_graph(&mut self) {
        let Some((xref, _)) = self.current.take() else {
            return;
        };
        let pointers: Vec<Pointer<'_>> = self
            .pointers
            .iter()
            .map(|&(_, _, to, alias)| Pointer { to, alias })
            .collect();
        for (i, broken) in self.graph.record(Some(xref), &pointers) {
            if let Some(&(line, tag, p, _)) = self.pointers.get(i) {
                self.out
                    .push(dev(line, DeviationKind::Cycle, broken.detail(tag, p, xref)));
            }
        }
    }

    /// The type of `n` as a substructure of `sup`: `Err(None)` for an
    /// extension, `Err(Some(..))` for a tag that does not belong there.
    #[allow(clippy::type_complexity)] // A one-off private result.
    fn pick<N: Node<'a>>(
        &self,
        sup: StructId,
        n: N,
    ) -> Result<StructId, Option<(DeviationKind, String)>> {
        let tag = n.tag();
        let parent = if sup == DATASET {
            "the dataset"
        } else {
            self.schema.tag(sup)
        };
        let is_pointer = matches!(n.payload(), PayloadRef::Pointer(_));
        match pick_node(self.rules, sup, n, is_pointer) {
            Pick::Type(id) => Ok(id),
            Pick::Extension => Err(None),
            Pick::BadTag => Err(Some((
                DeviationKind::UnknownTag,
                format!("{tag:?} is not a tag"),
            ))),
            Pick::Unknown => Err(Some((
                DeviationKind::UnknownTag,
                format!("{tag} is not a tag of GEDCOM {}", self.schema.version),
            ))),
            Pick::Continuation => Err(Some((
                DeviationKind::Continuation,
                format!("{tag} continues a payload and is not a structure of {parent}"),
            ))),
            Pick::Misplaced if sup == DATASET => Err(Some((
                DeviationKind::Misplaced,
                format!("{tag} is not a record"),
            ))),
            Pick::Misplaced => Err(Some((
                DeviationKind::Misplaced,
                format!("{tag} is not a substructure of {parent}"),
            ))),
        }
    }

    /// A structure of a known type and its substructures.
    fn structure<N: Node<'a>>(&mut self, n: N, ty: StructId, _parent: &str, record: bool) {
        let tag = n.tag();
        if !record && let Some(x) = n.xref() {
            self.push(
                n.line(),
                DeviationKind::Xref,
                format!("{x}: a substructure has no identifier"),
            );
        }
        self.payload(n, ty);
        let schema = self.schema;
        let subs = schema.subs(ty);
        // Occurrences per permitted tag (a pointer form and a text form of
        // one tag share the budget).
        let mut counts: Vec<(u8, u32)> = Vec::new();
        let mut children = 0;
        for c in n.children() {
            children += 1;
            match self.pick(ty, c) {
                Ok(cty) => {
                    if let Some(t) = schema.tag_id(cty) {
                        match counts.iter_mut().find(|(k, _)| *k == t) {
                            Some((_, n)) => *n += 1,
                            None => counts.push((t, 1)),
                        }
                    }
                    self.graph_pointer(c, cty);
                    self.structure(c, cty, tag, false);
                }
                // A documented alias of a standard structure is that
                // structure, wherever it is (7.x §1.5.1: relocated).
                Err(None) => match self.aliases.structs.get(c.tag()).copied() {
                    Some(aty) => {
                        self.graph_pointer(c, aty);
                        self.structure(c, aty, tag, false);
                    }
                    None => self.extension(c),
                },
                Err(Some((kind, why))) => {
                    self.push(c.line(), kind, why);
                    self.extension(c);
                }
            }
        }
        let mut seen: Vec<u8> = Vec::new();
        for s in subs {
            let Some(t) = schema.tag_id(s.id) else {
                continue;
            };
            if seen.contains(&t) {
                continue;
            }
            seen.push(t);
            let (min, max) = bounds(schema, ty, t);
            let found = counts.iter().find(|(k, _)| *k == t).map_or(0, |(_, n)| *n);
            let sub_tag = schema.tag(s.id);
            if let Some(max) = max.filter(|&m| found > u32::from(m)) {
                self.push(
                    n.line(),
                    DeviationKind::Cardinality,
                    format!("{tag} has {found} {sub_tag}, at most {max}"),
                );
            }
            if found < u32::from(min) {
                self.push(
                    n.line(),
                    DeviationKind::MissingRequired,
                    format!("{tag} lacks {sub_tag}"),
                );
            }
        }
        self.special(n, ty);
        // 7.x §1.2: a structure has a payload or a substructure. Records are
        // left out: an empty record still stands for an entity others point
        // to, and the header and trailer are pseudo-structures.
        if self.family == Family::V7
            && !record
            && children == 0
            && n.payload().as_str().is_none_or(str::is_empty)
        {
            self.push(
                n.line(),
                DeviationKind::Payload,
                format!("{tag} has neither a payload nor a substructure"),
            );
        }
    }

    /// Rules the tables cannot state.
    fn special<N: Node<'a>>(&mut self, n: N, ty: StructId) {
        if self.family != Family::V7 {
            return;
        }
        let name = self.schema.name(ty);
        // §NOTE-TRAN: a translation of a note says its language or its
        // media type.
        if name == "NOTE-TRAN" && !n.children().any(|c| matches!(c.tag(), "MIME" | "LANG")) {
            self.push(
                n.line(),
                DeviationKind::MissingRequired,
                "TRAN lacks both MIME and LANG".into(),
            );
        }
        // §MIME: the media type of a text is a text type.
        if name == "MIME" {
            let text = n.payload().as_str().unwrap_or("");
            if !text.is_empty() && !text.to_ascii_lowercase().starts_with("text/") {
                self.push(
                    n.line(),
                    DeviationKind::Payload,
                    format!("MIME {text:?}: the media type of a text is text/…"),
                );
            }
        }
    }

    /// The payload of a structure of type `ty`.
    fn payload<N: Node<'a>>(&mut self, n: N, ty: StructId) {
        let tag = n.tag();
        let (kind, arg) = self.schema.kind(ty);
        match (kind, n.payload()) {
            (Kind::Pointer | Kind::NullablePointer, PayloadRef::Pointer(p)) => {
                self.pointer(n, p, Some(arg));
            }
            (Kind::Pointer, PayloadRef::None) => self.push(
                n.line(),
                DeviationKind::Payload,
                format!("{tag} takes a pointer"),
            ),
            (Kind::NullablePointer, PayloadRef::None) => {}
            (Kind::Pointer | Kind::NullablePointer, PayloadRef::Text(t)) => self.push(
                n.line(),
                DeviationKind::Payload,
                format!("{tag} {t:?}: a pointer is required"),
            ),
            (_, PayloadRef::Pointer(p)) => {
                self.pointer(n, p, None);
                self.push(
                    n.line(),
                    DeviationKind::Payload,
                    format!("{tag} {p}: a pointer where the payload is not one"),
                );
            }
            (_, payload) => {
                let raw = payload.as_str().unwrap_or("");
                self.characters(n, raw);
                let text = match kind {
                    Kind::Date | Kind::DateExact | Kind::DatePeriod => self.aliases.unalias(raw),
                    _ => std::borrow::Cow::Borrowed(raw),
                };
                let text = text.as_ref();
                if text.is_empty() && payload::empty_is_valid(kind) {
                    return;
                }
                let set = matches!(kind, Kind::Enum | Kind::ListEnum)
                    .then(|| self.schema.enum_set(arg))
                    .flatten();
                if let Some(why) = payload::check(kind, set, text, self.family) {
                    let dk = if matches!(kind, Kind::Enum | Kind::ListEnum) {
                        DeviationKind::EnumValue
                    } else {
                        DeviationKind::Payload
                    };
                    self.push(n.line(), dk, format!("{tag} {text:?}: {why}"));
                }
            }
        }
    }

    fn characters<N: Node<'a>>(&mut self, n: N, text: &str) {
        if let Some(c) = text.chars().find(|&c| payload::is_banned(c, self.rules)) {
            self.push(
                n.line(),
                DeviationKind::Character,
                format!(
                    "{}: U+{:04X} may not appear in GEDCOM {}",
                    n.tag(),
                    u32::from(c),
                    self.schema.version
                ),
            );
        }
    }

    /// A pointer: its grammar, its record, and that record's type when the
    /// structure's type says which (`expect`).
    fn pointer<N: Node<'a>>(&mut self, n: N, p: &str, expect: Option<u16>) {
        let tag = n.tag();
        if self.family == Family::V7 && p == "@VOID@" {
            return;
        }
        if is_external_pointer(p, self.family) {
            return;
        }
        if let Some(why) = xref_error(p, self.rules) {
            self.push(n.line(), DeviationKind::Xref, format!("{tag} {p}: {why}"));
        }
        match self.records.get(p) {
            None => self.push(
                n.line(),
                DeviationKind::DanglingPointer,
                format!("{tag} {p}: no record has this identifier"),
            ),
            Some(r) => {
                if let Some(want) = expect
                    && r.ty != Some(want)
                {
                    self.push(
                        n.line(),
                        DeviationKind::PointerTarget,
                        format!(
                            "{tag} {p} points to a {} record, not to a {} record",
                            r.tag,
                            self.schema.tag(want)
                        ),
                    );
                }
            }
        }
    }

    /// An extension structure: only what holds everywhere is checked (tags,
    /// identifiers, pointers, characters); its substructures mean what its
    /// definer says (7.x §1.5).
    fn extension<N: Node<'a>>(&mut self, n: N) {
        let mut stack = vec![(n, true)];
        while let Some((s, top)) = stack.pop() {
            if !top {
                if !self.rules.is_valid_tag(s.tag()) {
                    self.push(
                        s.line(),
                        DeviationKind::UnknownTag,
                        format!("{:?} is not a tag", s.tag()),
                    );
                }
                if let Some(x) = s.xref() {
                    self.push(
                        s.line(),
                        DeviationKind::Xref,
                        format!("{x}: a substructure has no identifier"),
                    );
                }
            }
            match s.payload() {
                PayloadRef::Pointer(p) => self.pointer(s, p, None),
                PayloadRef::Text(t) => self.characters(s, t),
                PayloadRef::None => {}
            }
            stack.extend(s.children().map(|c| (c, false)));
        }
    }
}

/// The size of a record as written in `family`, an upper bound: CR LF
/// terminators, `@` doubled, a `CONT` line per line break and a `CONC` line
/// per 200 bytes of payload.
pub(crate) fn encoded_len<'a, N: Node<'a>>(n: N, level: usize, family: Family) -> usize {
    let digits = if level >= 10 { 2 } else { 1 };
    let mut size = digits + 1 + n.tag().len() + 2 + n.xref().map_or(0, |x| x.len() + 1);
    if let Some(p) = n.payload().as_str() {
        let at = if family == Family::V551 {
            p.matches('@').count()
        } else {
            0
        };
        let breaks = p.matches('\n').count();
        let conc = (p.len() + at) / 200;
        size += 1 + p.len() + at + (breaks + conc) * (digits + 8);
    }
    size + n
        .children()
        .map(|c| encoded_len(c, level + 1, family))
        .sum::<usize>()
}
