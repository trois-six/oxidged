//! (c) The 7.x rules on pointers between records ([`crate::spec::graph`]):
//! an `ALIA` to its own record, and a pointer that closes a cycle between
//! source, multimedia and shared note records, become plain extension
//! structures where they are, with their pointers and substructures.

use super::{Conformer, Family, Pick, Rec, RecRef, RepairKind, StructId, node_at, with_node};
use crate::spec::graph::{Broken, Graph, Pointer};
use crate::spec::index::SchemaIndex;
use crate::tree::{Flat, Node, PayloadRef, Structure, Tag};

/// A pointer of a record the rules concern: where it is (the indices of
/// the substructures from the record down), what it names, and whether it
/// is an `ALIA`.
#[derive(Debug)]
pub(crate) struct Found {
    path: Vec<usize>,
    to: String,
    alias: bool,
}

impl Found {
    /// Where it is: the indices of the substructures from the record down.
    pub(crate) fn path(&self) -> &[usize] {
        &self.path
    }

    /// As the graph takes it.
    pub(crate) fn pointer(&self) -> Pointer<'_> {
        Pointer {
            to: &self.to,
            alias: self.alias,
        }
    }
}

/// The pointers the rules concern of a record written in stream mode
/// ([`Flat::stream_graph`]).
pub(crate) fn streamed_pointers<'f>(flat: &'f Flat<'_>, tables: &SchemaIndex) -> Vec<Pointer<'f>> {
    flat.stream_graph()
        .map(|(to, ty)| Pointer {
            to,
            alias: tables.is_alias(ty),
        })
        .collect()
}

impl<'n> Conformer<'n> {
    /// The pointers of record `n`, of type `rtype`, that the 7.x pointer
    /// rules concern, in the order of the record: those of its standard
    /// structures (and documented aliases of them), at any depth; an
    /// individual's `ALIA` is one of its own substructures.
    pub(crate) fn graph_pointers<'a, N: Node<'a>>(&self, n: N, rtype: StructId) -> Vec<Found> {
        let mut out = Vec::new();
        if self.family == Family::V7 && self.tables.is_graph_record(rtype) {
            let mut path = Vec::new();
            self.collect(n, rtype, rtype, &mut path, &mut out);
        }
        out
    }

    fn collect<'a, N: Node<'a>>(
        &self,
        n: N,
        ty: StructId,
        rtype: StructId,
        path: &mut Vec<usize>,
        out: &mut Vec<Found>,
    ) {
        let deep = !self.tables.is_individual(rtype);
        for (i, c) in n.children().enumerate() {
            let payload = c.payload();
            let is_ptr = matches!(payload, PayloadRef::Pointer(_));
            let cty = match self.picker.pick(ty, c, is_ptr) {
                Pick::Type(cty) => cty,
                Pick::Extension => match self.aliases.structs.get(c.tag()) {
                    Some(&aty) => aty,
                    None => continue,
                },
                _ => continue,
            };
            path.push(i);
            if let PayloadRef::Pointer(p) = payload
                && self.tables.is_graph_pointer(self.schema, rtype, cty)
            {
                out.push(Found {
                    path: path.clone(),
                    to: p.to_string(),
                    alias: self.tables.is_alias(cty),
                });
            }
            if deep {
                self.collect(c, cty, rtype, path, out);
            }
            path.pop();
        }
    }

    /// Makes the pointer structure at `path` of record `s` (identifier
    /// `xref`), which breaks a rule, a plain extension structure.
    pub(crate) fn cut(&mut self, s: &mut Structure, path: &[usize], broken: Broken, xref: &str) {
        let Some(c) = node_at(s, path) else { return };
        let tag = c.tag.as_str().to_string();
        let to = match &c.payload {
            crate::tree::Payload::Pointer(p) => p.as_str().to_string(),
            _ => String::new(),
        };
        let new = self.plain_tag(&tag);
        let line = c.line;
        c.tag = Tag::new(&new);
        self.repair(
            line,
            RepairKind::Cycle,
            format!("{}; written as {new}", broken.detail(&tag, &to, xref)),
        );
    }

    /// The pointer rules on the whole dataset, once every record is
    /// repaired: each record in order, its pointers that break a rule cut
    /// ([`Conformer::cut`]). Whether any was.
    pub(super) fn pointer_rules<N: Node<'n>>(&mut self, records: &mut [Rec<'n, N>]) -> bool {
        if self.family != Family::V7 {
            return false;
        }
        let mut graph = Graph::default();
        let mut scratch = Flat::default();
        let mut cut = false;
        for r in records.iter_mut() {
            let Some(rtype) = self.record_type_of(r.tag(), r.standard_tag()) else {
                continue;
            };
            if !self.tables.is_graph_record(rtype) {
                continue;
            }
            let found = with_node!(r.get(&self.arena), scratch, |n| self
                .graph_pointers(n, rtype));
            if found.is_empty() {
                continue;
            }
            let pointers: Vec<Pointer<'_>> = found.iter().map(Found::pointer).collect();
            let broken = graph.record(r.xref(), &pointers);
            if broken.is_empty() {
                continue;
            }
            let xref = r.xref().unwrap_or_default().to_string();
            (r.clean, r.size) = (0, u32::MAX);
            let s = r.make_owned(&self.arena);
            for (i, b) in broken {
                if let Some(f) = found.get(i) {
                    self.cut(s, &f.path, b, &xref);
                }
            }
            self.model_order(s, rtype);
            cut = true;
        }
        cut
    }
}
