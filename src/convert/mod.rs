//! Conversion between GEDCOM 5.5.1 and GEDCOM 7.x.
//!
//! [`GedcomWriter`] converts a dataset when it writes
//! it in a version of the other family (5.5.1, or 7.0 and 7.1), before the
//! conformance repair ([`crate::spec::conform`]) and the line rules; the
//! functions of this module give the same conversion as owned structures
//! ([`convert_structures`]) or as a dataset ([`convert_dataset`]). Within a
//! family nothing is converted: 7.0 and 7.1 differ in structures only,
//! which the repair handles.
//!
//! Values are always rewritten in the target's grammars (dates, ages,
//! times, enumeration spellings). The conversion adds what the migration
//! from 5.5.1 to 7.0 changes in structures, following the `FamilySearch`
//! migration guide and, where the guide leaves a choice, the gedcom7code
//! conversions (`test-files`, whose pairs are the oracle of the
//! conformance suite):
//!
//! | 5.5.1 | 7.x |
//! |---|---|
//! | `NOTE` record, `NOTE @N1@` | `SNOTE` record, `SNOTE @N1@` |
//! | Multimedia link holding its files (`OBJE` + `FILE`) | A new `OBJE` record; the link points to it and keeps its `TITL` |
//! | Source citation in words (`SOUR text`) | A new `SOUR` record with the text as its `NOTE`; the citation points to it, its `TEXT`s under `DATA` |
//! | `ASSO.RELA word` | `ASSO.ROLE`: the role the word names (`FRIEND`, `GODP`, …), else `OTHER`, with the word as `PHRASE` when it says more |
//! | `FONE`, `ROMN` with `TYPE` | `TRAN` with `LANG` (`kana` is `ja-Hrkt`, `romaji` `ja-Latn`, others private-use tags) |
//! | `LANG English` | `LANG en` (BCP 47); an unknown name `LANG und` + `PHRASE` |
//! | `FILE C:\photos\a b.jpg`, `FORM jpg`, `FORM.TYPE` | `FILE file:///C:/photos/a%20b.jpg`, `FORM image/jpeg`, `FORM.MEDI` |
//! | `RIN`, `AFN`, `RFN system:id`, `_FSFTID` | `EXID` with the registered `TYPE` (`https://gedcom.io/terms/v7/RIN#<HEAD.SOUR>`, …) |
//! | `ALIA` with a name | `NAME` with `TYPE AKA` |
//! | `_UID`, `EMAI`, `_EMAIL`, `_ASSO` | `UID`, `EMAIL`, `ASSO` |
//! | `@#DJULIAN@` in text; other escapes | `JULIAN`; left out (7.x has no escapes) |
//! | `AGE STILLBORN` | `AGE 0y` + `PHRASE`, and a `DEAT` at `AGE 0y` when the individual has none |
//! | A record named `@VOID@` | A new identifier, its pointers with it (`@VOID@` points to nothing in 7.x) |
//! | `SUBN` record, `HEAD.SUBN`, `HEAD.FILE`, `HEAD.CHAR`, `GEDC.FORM` | Left out |
//! | Extension tags of known meaning (`_LOC`, `_GOV`, …) | Declared in `HEAD.SCHMA` |
//!
//! Converting to 5.5.1 reverses each of these where 5.5.1 can say it
//! (`SNOTE` as `NOTE`, `EXID` as `RIN`/`AFN`/`RFN`, `ROLE` as `RELA`,
//! `TRAN` as `FONE`/`ROMN`, language tags as names, media types as formats,
//! `file` URLs as paths); what 5.5.1 has no place for (`UID`, `CREA`, `NO`,
//! `CROP`, `PHRASE`, an association under an event, …) is kept by the
//! conformance repair as extension structures. With
//! [`ConversionOptions::keep_obsolete`], the 5.5.1 structures 7.x drops are
//! kept as extensions that `HEAD.SCHMA` declares with their 5.5.1 URIs,
//! and converted back to them, so that 5.5.1 data survives a round trip
//! through 7.x.
//!
//! ```rust
//! use oxidged::convert::{convert_dataset, ConversionOptions};
//! use oxidged::model::Dataset;
//! use oxidged::GedcomVersion;
//!
//! let data = Dataset::parse(
//!     "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 ASSO @I2@\n2 RELA Godfather\n\
//!      0 @I2@ INDI\n0 TRLR\n",
//! );
//! let v7 = convert_dataset(&data, GedcomVersion::V7_0, &ConversionOptions::default());
//! let out = oxidged::GedcomWriter::new().write_to_string(&v7).unwrap();
//! assert!(out.contains("1 ASSO @I2@\n2 ROLE GODP\n3 PHRASE Godfather\n"));
//! ```

mod rules;
pub(crate) mod tables;
pub(crate) mod text;

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::model::Dataset;
use crate::spec::conform::{Build, FieldOrder, Rec, model_order, record_rank};
use crate::tree::{Flat, Node, PayloadRef, STANDARD_TAGS, Structure, Xref};
use crate::{GedcomVersion, GedcomWriter};
use rules::Direction;
pub(crate) use rules::{Converter, Fate, Fresh, NONE, Out, POINTER, TEXT};

/// How a conversion between 5.5.1 and 7.x treats the data the target
/// version has no place for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ConversionOptions {
    /// Keep the 5.5.1 structures 7.x drops (the `SUBN` record and its
    /// pointer, `HEAD.FILE`, escape sequences in text) when converting to
    /// 7.x: as extensions `HEAD.SCHMA` declares with their 5.5.1 URIs
    /// (`_SUBMISSION`, `_SUBN`, `_FILE`), which converting back to 5.5.1
    /// restores. Off by default, as the migration guide drops them.
    pub keep_obsolete: bool,
}

impl ConversionOptions {
    /// The default options with [`keep_obsolete`](Self::keep_obsolete) set.
    #[must_use]
    pub fn keep_obsolete(mut self, keep: bool) -> Self {
        self.keep_obsolete = keep;
        self
    }
}

/// Converts the records of a dataset (header, records and trailer, in
/// order, as [`Tree::to_structures`](crate::tree::Tree::to_structures)
/// gives them) from version `from` to version `to`, as the writer converts
/// them (see the [module documentation](self)). Records it adds go before
/// the trailer. Within a version family it changes nothing.
///
/// The result is converted, not repaired: what `to` does not permit yet is
/// for [`spec::conform`](crate::spec::conform), which the writer runs next.
///
/// ```rust
/// use oxidged::convert::{convert_structures, ConversionOptions};
/// use oxidged::tree::Tree;
/// use oxidged::GedcomVersion;
///
/// let mut records = Tree::parse(
///     "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 LANG English\n0 @I1@ INDI\n1 RIN 42\n0 TRLR\n",
/// )
/// .to_structures();
/// convert_structures(
///     &mut records,
///     GedcomVersion::V5_5_1,
///     GedcomVersion::V7_0,
///     &ConversionOptions::default(),
/// );
/// assert_eq!(records[0].first("LANG").unwrap().text(), Some("en"));
/// let exid = records[1].first("EXID").unwrap();
/// assert_eq!(exid.text(), Some("42"));
/// assert_eq!(
///     exid.first("TYPE").unwrap().text(),
///     Some("https://gedcom.io/terms/v7/RIN")
/// );
/// ```
pub fn convert_structures(
    records: &mut Vec<Structure>,
    from: GedcomVersion,
    to: GedcomVersion,
    options: &ConversionOptions,
) {
    let refs: Vec<&Structure> = records.iter().collect();
    let Some(plan) = Plan::of_nodes(&refs, from, to, *options, true, None) else {
        return;
    };
    drop(refs);
    let owned = plan.into_owned(std::mem::take(records));
    *records = owned;
}

/// The dataset `data` converted to version `to` (see the [module
/// documentation](self)): what [`GedcomWriter`] writes of it in `to`, with
/// `options`, read back. Within a version family, the dataset as it is
/// written in `to` (values in its grammars, repairs made).
#[must_use]
pub fn convert_dataset(data: &Dataset, to: GedcomVersion, options: &ConversionOptions) -> Dataset {
    let writer = GedcomWriter::new()
        .gedcom_version(to)
        .conversion(*options)
        .bom(crate::Bom::Never);
    match writer.write_to_string(data) {
        Ok(text) => Dataset::parse(text),
        // The default repair policy repairs and never fails.
        Err(_) => data.clone(),
    }
}

/// Fresh identifiers, unused in the dataset: `@<prefix><n>@`, `n` from 1,
/// that no record has and no pointer holds, so that a new record never
/// takes what a pointer to no record named.
struct Alloc<'f> {
    /// The identifiers handed out.
    given: HashSet<Box<str>>,
    next: HashMap<String, u32>,
    /// Whether the dataset uses an identifier, as a record's or a pointer's.
    used: &'f dyn Fn(&str) -> bool,
}

impl Alloc<'_> {
    fn fresh(&mut self, prefix: &str) -> Box<str> {
        let n = self.next.entry(prefix.to_string()).or_insert(1);
        loop {
            let candidate: Box<str> = format!("@{prefix}{n}@").into();
            *n = n.saturating_add(1);
            if !(self.used)(&candidate) && self.given.insert(candidate.clone()) {
                return candidate;
            }
        }
    }
}

/// The identifiers handed to one record, kept to hand them again when the
/// record is converted anew.
struct Recording<'a, 'f> {
    alloc: &'a mut Alloc<'f>,
    names: Vec<Box<str>>,
}

impl Fresh for Recording<'_, '_> {
    fn fresh(&mut self, prefix: &str) -> Box<str> {
        let x = self.alloc.fresh(prefix);
        self.names.push(x.clone());
        x
    }
}

/// The identifiers a record was handed, handed again in order.
struct Replay<'a> {
    names: std::slice::Iter<'a, Box<str>>,
}

impl Fresh for Replay<'_> {
    fn fresh(&mut self, prefix: &str) -> Box<str> {
        // A record converts the same way each time, so asks as often.
        self.names
            .next()
            .cloned()
            .unwrap_or_else(|| format!("@{prefix}@").into())
    }
}

/// What becomes of one record.
enum Action {
    /// Written as it is.
    Keep,
    /// Converted, handed these identifiers: kept so while the records kept
    /// converted are few ([`KEEP_CONVERTED`]), else converted anew each
    /// time it is written.
    Convert(Box<[Box<str>]>, Option<Box<Structure>>),
    /// Left out.
    Remove,
}

/// The most structures of records a plan keeps converted (about 6 MB): a
/// small dataset is converted once, a large one again each time a record
/// is written, so that its memory stays bounded.
const KEEP_CONVERTED: usize = 1 << 16;

/// The extension structures converting renames to standard structures
/// holding a text (`_UID` as `UID`, [`rules::RENAMED_UP`]): the writer,
/// converting a 5.5.1 dataset to 7.x, writes each such structure that is a
/// text without substructure or `@` as that structure, where its parent's
/// type has a field of texts for it, as its record is written
/// ([`crate::model`]'s `WriteCx::adopt`): such a record is then written as
/// it is, not converted on its own as owned structures, and its lines are
/// those converting it writes.
#[derive(Debug)]
pub(crate) struct Adoptions(&'static [(&'static str, &'static str)]);

impl Adoptions {
    /// The standard tag the extension `tag` is written as.
    pub(crate) fn target(&self, tag: &str) -> Option<&'static str> {
        self.0.iter().find(|(t, _)| *t == tag).map(|&(_, to)| to)
    }
}

/// [`Adoptions`] of a conversion from 5.5.1 to 7.x.
pub(crate) static ADOPTED_UP: Adoptions = Adoptions(rules::RENAMED_UP);

/// The structures of `s`, itself included.
fn size(s: &Structure) -> usize {
    1 + s.substructures.iter().map(size).sum::<usize>()
}

/// The conversion of one dataset: what becomes of each record, the
/// converted header and the records added. Planned once, record by record;
/// a record that writes itself is then converted again each time the
/// writer writes it, so that the dataset is never copied.
pub(crate) struct Plan {
    conv: Converter,
    actions: Vec<Action>,
    /// The index and the converted structures of the header.
    head: Option<(usize, Structure)>,
    /// The records added (multimedia and source records), in order.
    added: Vec<Structure>,
    /// The order of the typed model the records come from, if they do.
    order: Option<&'static FieldOrder>,
    target: GedcomVersion,
}

impl Plan {
    /// Plans the conversion of `n` records, `tag`, `xref` and `owned` giving
    /// each one's tag, identifier and structures, `used` whether the
    /// records use an identifier (a record's or a pointer's). `None` when
    /// the versions need none. Without `scan`, the records are not looked
    /// at (the header only is converted); without `maybe_void`, no record
    /// is named `@VOID@`.
    #[allow(clippy::too_many_arguments)]
    fn new<'a>(
        n: usize,
        tag: impl Fn(usize) -> &'a str,
        xref: impl Fn(usize) -> Option<&'a str>,
        used: &dyn Fn(&str) -> bool,
        head_of: impl FnOnce(usize) -> Structure,
        mut prepare: impl FnMut(usize, &Converter) -> Option<Structure>,
        from: GedcomVersion,
        to: GedcomVersion,
        options: ConversionOptions,
        values: bool,
        order: Option<&'static FieldOrder>,
        scan: bool,
        maybe_void: bool,
    ) -> Option<Self> {
        let mut conv = Converter::new(from, to, options, values)?;
        let mut alloc = Alloc {
            given: HashSet::new(),
            next: HashMap::new(),
            used,
        };
        let void = maybe_void && (0..n).any(|i| xref(i) == Some(Xref::VOID));
        let head_at = (0..n).find(|&i| tag(i) == "HEAD");
        let mut head = head_at.map(|i| (i, head_of(i)));
        if let Some((_, h)) = &head {
            conv.read_head(h);
        }
        if void && conv.direction == Direction::Up {
            conv.rename_void(alloc.fresh("VOID_"));
        }
        let mut actions = Vec::with_capacity(if scan { n } else { 0 });
        let mut added = Vec::new();
        let mut known = BTreeSet::new();
        // The structures of the records kept converted.
        let mut kept = 0;
        // Without a scan, no record is looked at: none has an action.
        for i in (0..n).filter(|_| scan) {
            if Some(i) == head_at || tag(i) == "TRLR" {
                actions.push(Action::Keep);
                continue;
            }
            // Most records need nothing: they are not copied.
            let Some(mut s) = prepare(i, &conv) else {
                actions.push(Action::Keep);
                continue;
            };
            let mut fresh = Recording {
                alloc: &mut alloc,
                names: Vec::new(),
            };
            let mut out = Out::default();
            let fate = conv.record(&mut s, &mut fresh, &mut out);
            let names = fresh.names;
            actions.push(match fate {
                Fate::Remove => Action::Remove,
                Fate::Keep if out.changed => {
                    let n = size(&s);
                    let keep = kept + n <= KEEP_CONVERTED;
                    let converted = keep.then(|| {
                        kept += n;
                        if let Some(order) = order {
                            model_order(&mut s, to, order);
                        }
                        Box::new(s)
                    });
                    Action::Convert(names.into_boxed_slice(), converted)
                }
                Fate::Keep => Action::Keep,
            });
            known.append(&mut out.known);
            added.append(&mut out.records);
        }
        // The records added convert as records (they hold converted
        // structures already); what is left is to place them.
        if let Some((_, h)) = &mut head {
            if !conv.head(h, &known) {
                head = None;
            } else if let Some(order) = order {
                model_order(h, to, order);
            }
        }
        if let Some(order) = order {
            for r in &mut added {
                model_order(r, to, order);
            }
        }
        Some(Self {
            conv,
            actions,
            head,
            added,
            order,
            target: to,
        })
    }

    /// Plans the conversion of records given as nodes (a parsed tree,
    /// owned structures).
    pub(crate) fn of_nodes<'n, N: Node<'n>>(
        records: &[N],
        from: GedcomVersion,
        to: GedcomVersion,
        options: ConversionOptions,
        values: bool,
        order: Option<&'static FieldOrder>,
    ) -> Option<Self> {
        let get = |i: usize| records.get(i).copied();
        // Gathered with the first identifier asked for, as few datasets
        // need any.
        let identifiers = std::cell::OnceCell::new();
        let used = |x: &str| {
            identifiers
                .get_or_init(|| {
                    let mut set = HashSet::new();
                    for r in records {
                        crate::tree::node::identifiers(*r, &mut |x| {
                            set.insert(x);
                        });
                    }
                    set
                })
                .contains(x)
        };
        Self::new(
            records.len(),
            |i| get(i).map_or("", Node::tag),
            |i| get(i).and_then(Node::xref),
            &used,
            |i| get(i).map(Node::to_owned_structure).unwrap_or_default(),
            |i, conv| get(i).and_then(|r| needed(conv, r)),
            from,
            to,
            options,
            values,
            order,
            true,
            true,
        )
    }

    /// Plans the conversion of the records of `data` to `to`, `built` (the
    /// records as they write themselves, then the trailer), whose values
    /// are converted already. Which records need a look is found from the
    /// records written once each, and only those are copied.
    ///
    /// Without `scan`, no record is looked at: the plan converts the
    /// header only, for a writer that finds whether a record needs
    /// converting as it writes it ([`Watch`]).
    pub(crate) fn of_dataset<'d, B: Build<'d>>(
        data: &'d Dataset,
        built: &[B],
        to: GedcomVersion,
        options: ConversionOptions,
        order: Option<&'static FieldOrder>,
        scan: bool,
    ) -> Option<Self> {
        let mut scratch = Flat::default();
        // The store has every identifier of the dataset.
        let used = |x: &str| data.store().find_xref(x).is_some();
        Self::new(
            built.len(),
            |i| built.get(i).map_or("", Build::tag),
            |i| built.get(i).and_then(Build::xref),
            &used,
            |i| built.get(i).map(built_structure).unwrap_or_default(),
            |i, conv| {
                // Written once, looked at, copied only when it needs it.
                scratch.clear();
                built.get(i)?.build(&mut scratch);
                let root = scratch.root()?;
                if conv.needs(root) {
                    return Some(root.to_owned_structure());
                }
                debug_assert!(
                    !conv.changes(&root.to_owned_structure()),
                    "a conversion needs() missed: {:?}",
                    root.to_owned_structure()
                );
                None
            },
            data.version(),
            to,
            options,
            false,
            order,
            scan,
            data.store().find_xref(Xref::VOID).is_some(),
        )
    }

    /// The look for the records this plan would convert, for a writer
    /// that writes them as they are ([`Plan::of_dataset`] without a scan).
    pub(crate) fn watch(&self) -> Watch {
        let mut watch = Watch::new(self.conv.clone());
        watch.order = self.order;
        watch
    }

    /// The header converted, and its index among the records, when the
    /// conversion changes it.
    pub(crate) fn head(&self) -> Option<(usize, &Structure)> {
        self.head.as_ref().map(|(i, h)| (*i, h))
    }

    /// Converts record `index` (anew), as planned.
    fn convert(&self, index: usize, s: &mut Structure) {
        let Some(Action::Convert(names, converted)) = self.actions.get(index) else {
            return;
        };
        if let Some(c) = converted {
            s.clone_from(c);
            return;
        }
        let mut fresh = Replay {
            names: names.iter(),
        };
        let mut out = Out::default();
        let _ = self.conv.record(s, &mut fresh, &mut out);
        if let Some(order) = self.order {
            model_order(s, self.target, order);
        }
    }

    /// The records kept (`kept`, in order, then the trailer, of record
    /// ranks `ranks`) with the
    /// records added and those made extension records (`last`): in the
    /// typed model's order (records added after the records of their type,
    /// extension records last), else before the trailer.
    fn arrange<'p, T>(
        &'p self,
        kept: Vec<T>,
        mut last: Vec<T>,
        ranks: &[u8],
        added: impl Fn(&'p Structure) -> T,
    ) -> Vec<T> {
        let end = ranks
            .iter()
            .rposition(|&r| r == record_rank("TRLR"))
            .unwrap_or(ranks.len());
        let at = |r: &Structure| match self.order {
            Some(_) => {
                let rank = record_rank(r.tag.as_str());
                ranks
                    .get(..end)
                    .unwrap_or(&[])
                    .iter()
                    .rposition(|&k| k <= rank)
                    .map_or(0, |i| i + 1)
            }
            None => end,
        };
        let mut placed: Vec<(usize, &'p Structure)> =
            self.added.iter().map(|r| (at(r), r)).collect();
        // Records added at one place go by type, in order (stable).
        placed.sort_by_key(|&(p, r)| (p, self.order.map(|_| record_rank(r.tag.as_str()))));
        let mut new = placed.into_iter().peekable();
        let mut out = Vec::with_capacity(kept.len() + self.added.len() + last.len());
        for (i, k) in kept.into_iter().enumerate() {
            while let Some((_, r)) = new.next_if(|(p, _)| *p <= i) {
                out.push(added(r));
            }
            if i == end {
                out.append(&mut last);
            }
            out.push(k);
        }
        out.extend(new.map(|(_, r)| added(r)));
        out.append(&mut last);
        out
    }

    /// Whether record `index`, tagged `tag`, is a standard record the
    /// conversion makes an extension record, which a typed model writes
    /// after the others.
    fn becomes_extension(&self, index: usize, tag: &str) -> bool {
        matches!(self.actions.get(index), Some(Action::Convert(..)))
            && !tag.starts_with('_')
            && self
                .conv
                .record_tag(tag)
                .0
                .is_some_and(|t| t.starts_with('_'))
    }

    /// The records converted, as owned structures.
    fn into_owned(self, records: Vec<Structure>) -> Vec<Structure> {
        let mut kept: Vec<Structure> = Vec::with_capacity(records.len() + self.added.len());
        let mut last = Vec::new();
        for (i, mut r) in records.into_iter().enumerate() {
            match self.actions.get(i) {
                Some(Action::Remove) => continue,
                _ if self.head.as_ref().is_some_and(|(h, _)| *h == i) => {
                    if let Some((_, h)) = &self.head {
                        r = h.clone();
                    }
                }
                Some(Action::Convert(..)) => {
                    let ext = self.order.is_some() && self.becomes_extension(i, r.tag.as_str());
                    self.convert(i, &mut r);
                    if ext {
                        last.push(r);
                        continue;
                    }
                }
                _ => {}
            }
            kept.push(r);
        }
        let ranks = ranks(kept.iter().map(|r| r.tag.as_str()));
        self.arrange(kept, last, &ranks, Structure::clone)
    }

    /// The records given as nodes, converted, for the writer: those that
    /// change owned, the others as they are.
    pub(crate) fn recs<'n, N: Node<'n>>(&self, records: &[N]) -> Vec<Rec<'n, N>> {
        let mut kept: Vec<Rec<'n, N>> = Vec::with_capacity(records.len() + self.added.len());
        let mut last = Vec::new();
        for (i, &r) in records.iter().enumerate() {
            match self.actions.get(i) {
                Some(Action::Remove) => {}
                _ if self.head.as_ref().is_some_and(|(h, _)| *h == i) => {
                    if let Some((_, h)) = &self.head {
                        kept.push(Rec::owned(h.clone()));
                    }
                }
                Some(Action::Convert(..)) => {
                    let ext = self.order.is_some() && self.becomes_extension(i, r.tag());
                    let mut s = r.to_owned_structure();
                    self.convert(i, &mut s);
                    if ext {
                        last.push(Rec::owned(s));
                    } else {
                        kept.push(Rec::owned(s));
                    }
                }
                _ => kept.push(Rec::given(r)),
            }
        }
        let ranks = ranks(kept.iter().map(Rec::tag));
        self.arrange(kept, last, &ranks, |s| Rec::owned(s.clone()))
    }

    /// The records that write themselves, converted, for the writer: in
    /// the order the typed model writes them.
    pub(crate) fn entries<'a, B: Build<'a>>(&'a self, records: &'a [B]) -> Vec<Entry<'a, B>> {
        let mut kept: Vec<Entry<'a, B>> = Vec::with_capacity(records.len() + self.added.len());
        let mut last = Vec::new();
        for (i, r) in records.iter().enumerate() {
            match self.actions.get(i) {
                Some(Action::Remove) => {}
                _ if self.head.as_ref().is_some_and(|(h, _)| *h == i) => {
                    if let Some((_, h)) = &self.head {
                        kept.push(Entry::Owned(h));
                    }
                }
                Some(Action::Convert(_, converted)) => {
                    let e = match converted {
                        Some(c) => Entry::Owned(c),
                        None => Entry::Converted {
                            record: r,
                            plan: self,
                            index: i,
                        },
                    };
                    if self.becomes_extension(i, r.tag()) {
                        last.push(e);
                    } else {
                        kept.push(e);
                    }
                }
                _ => kept.push(Entry::Plain(r)),
            }
        }
        let ranks = ranks(kept.iter().map(Build::tag));
        self.arrange(kept, last, &ranks, Entry::Owned)
    }
}

/// The rank of each record of a dataset in the typed model's order
/// (`record_rank`): a header or a trailer elsewhere than first or last is an
/// extension record, as the model reads it.
fn ranks<'t>(tags: impl ExactSizeIterator<Item = &'t str>) -> Vec<u8> {
    let last = tags.len().saturating_sub(1);
    tags.enumerate()
        .map(|(i, tag)| match tag {
            "HEAD" if i != 0 => record_rank("_HEAD"),
            "TRLR" if i != last => record_rank("_TRLR"),
            tag => record_rank(tag),
        })
        .collect()
}

/// What a writer looks out for in the records it writes, to find whether a
/// dataset needs converting without a pass of its own: a dataset that
/// needs none (nearly every dataset written in its own version family
/// needs nothing but its header converted) is written as it is, its
/// records looked at as they are written; one that does is planned
/// ([`Plan`]) and written again.
///
/// Whether a record needs converting is [`Converter::needs`]; the writer
/// asks it of each structure it writes ([`Watch::typed`]), with a filter
/// by tag and payload that spares nearly every structure the look.
pub(crate) struct Watch {
    conv: Converter,
    /// The order of the typed model the records come from, if they do.
    order: Option<&'static FieldOrder>,
    /// The rules a 5.5.1 dataset's producer is interpreted with again in
    /// 7.x, what only 7.x says ([`crate::profile`]), each record as it is
    /// written ([`Watch::interprets`]).
    interp: Option<crate::profile::RuleSet>,
    /// By the index of a standard tag: the payloads with which a rule may
    /// apply to a structure of that tag ([`Converter::may_apply`]).
    tags: [u8; STANDARD_TAGS.len()],
    /// [`Converter::special_mask`].
    special: u8,
    /// [`Converter::watches_pointers`].
    pointers: bool,
}

/// The payload bit of [`Converter::may_apply`] of `payload`.
pub(crate) fn payload_bit(payload: PayloadRef<'_>) -> u8 {
    match payload {
        PayloadRef::None => rules::NONE,
        PayloadRef::Text(_) => rules::TEXT,
        PayloadRef::Pointer(_) => rules::POINTER,
    }
}

impl Watch {
    /// The look for what `conv` converts.
    pub(crate) fn new(conv: Converter) -> Self {
        let mut tags = [0; STANDARD_TAGS.len()];
        for (slot, tag) in tags.iter_mut().zip(STANDARD_TAGS.iter()) {
            *slot = conv.may_apply(tag);
        }
        Self {
            special: conv.special_mask(),
            pointers: conv.watches_pointers(),
            conv,
            order: None,
            interp: None,
            tags,
        }
    }

    /// Interprets each record written with `rules` too.
    pub(crate) fn interpreting(&mut self, rules: crate::profile::RuleSet) {
        // The filter of the stream lets through every structure of a tag a
        // rule may rewrite ([`Watch::may_apply`]).
        for (i, kinds) in (0_u16..).zip(self.tags.iter_mut()) {
            if rules.may_rewrite_standard(i) {
                *kinds = rules::NONE | rules::TEXT | rules::POINTER;
            }
        }
        self.interp = Some(rules);
    }

    /// Whether the watch interprets ([`Watch::interpreting`]).
    #[inline]
    pub(crate) fn has_interpretation(&self) -> bool {
        self.interp.is_some()
    }

    /// Whether interpretation may change a structure tagged `tag` (of
    /// index `standard` among the standard tags) with `payload` (and an
    /// identifier when `has_xref`), a record when `record`, wherever it is:
    /// the look of a record streamed, which [`Watch::interprets`] makes
    /// precise.
    #[inline]
    pub(crate) fn interprets_node(
        &self,
        tag: &str,
        standard: Option<u16>,
        payload: PayloadRef<'_>,
        has_xref: bool,
        record: bool,
    ) -> bool {
        self.interp.as_ref().is_some_and(|rules| {
            rules.may_rewrite_anywhere(tag, standard, payload, has_xref, record)
        })
    }

    /// Whether interpretation may change record `r` (or a record beside
    /// it, or the header): what [`Watch::rewrite_alone`] then does.
    pub(crate) fn interprets<'n, N: Node<'n>>(&self, r: N) -> bool {
        fn walk<'n, N: Node<'n>>(
            rules: &crate::profile::RuleSet,
            n: N,
            path: &mut Vec<&'n str>,
        ) -> bool {
            if rules.may_rewrite(
                n.tag(),
                n.standard_tag(),
                path,
                || n.payload(),
                n.xref().is_some(),
                true,
                true,
            ) {
                return true;
            }
            path.push(n.tag());
            let found = n.children().any(|c| walk(rules, c, path));
            path.pop();
            found
        }
        self.interp
            .as_ref()
            .is_some_and(|rules| walk(rules, r, &mut Vec::new()))
    }

    /// Converts record `s` alone ([`Watch::convert_alone`]), then
    /// interprets it alone, when the watch interprets: whether it changed
    /// (as written, in the model's order); `None` when either reaches
    /// outside it.
    pub(crate) fn rewrite_alone(&self, s: &mut Structure) -> Option<bool> {
        let converted = self.convert_alone(s)?;
        let Some(rules) = &self.interp else {
            return Some(converted);
        };
        let target = self.conv.target();
        let mut interp = crate::profile::interpret::Interpreter::new(rules, target);
        let mut drafts = vec![crate::profile::draft::Draft::of_structure(s)];
        let rewrite = interp.record(&mut drafts, &|_| false);
        if rewrite.promoted || rewrite.placeholder || interp.has_pending() || drafts.len() != 1 {
            return None;
        }
        if !rewrite.changed {
            return Some(converted);
        }
        let mut interpreted = drafts
            .first()
            .map(crate::profile::draft::Draft::to_structure)?;
        drop(drafts);
        if let Some(order) = self.order {
            model_order(&mut interpreted, target, order);
        }
        *s = interpreted;
        Some(true)
    }

    /// Converts record `s` on its own, as a plan converts it, when the
    /// conversion changes nothing outside it: no record added, no
    /// extension the header is to declare, the record kept with its tag and
    /// identifier. Whether it changed (written as it was when not, as a
    /// plan keeps it); `None` (`s` then in any state) when the conversion
    /// is not its own, for the dataset to be planned whole.
    pub(crate) fn convert_alone(&self, s: &mut Structure) -> Option<bool> {
        /// Names no record: a record the conversion adds is not local.
        struct Nothing(bool);
        impl Fresh for Nothing {
            fn fresh(&mut self, prefix: &str) -> Box<str> {
                self.0 = true;
                format!("@{prefix}@").into()
            }
        }
        let (tag, xref) = (s.tag.clone(), s.xref.clone());
        let mut fresh = Nothing(false);
        let mut out = Out::default();
        let fate = self.conv.record(s, &mut fresh, &mut out);
        if fate != Fate::Keep
            || fresh.0
            || !out.records.is_empty()
            || !out.known.is_empty()
            || s.tag != tag
            || s.xref != xref
        {
            return None;
        }
        if !out.changed {
            return Some(false);
        }
        if let Some(order) = self.order {
            model_order(s, self.conv.target(), order);
        }
        Some(true)
    }

    /// Whether a rule may apply to a structure of the standard tag
    /// `standard` (`None`: any other tag) with a payload of kind `kind`
    /// (`len` bytes of text with the special bytes `special`): the filter
    /// of [`Watch::typed`], which spares nearly every structure the look.
    #[inline]
    pub(crate) fn may_apply(
        &self,
        standard: Option<u16>,
        kind: u8,
        len: usize,
        special: u8,
    ) -> bool {
        let kinds = match standard {
            Some(i) => self.tags.get(usize::from(i)).copied().unwrap_or(0),
            None => rules::NONE | rules::TEXT | rules::POINTER,
        };
        kinds & kind != 0
            || match kind {
                rules::TEXT => special & self.special != 0 || self.conv.watches_len(len),
                rules::POINTER => self.pointers,
                _ => false,
            }
    }

    /// Whether a record tagged `tag` and named `xref` converts as a record
    /// (retagged, left out or renamed).
    pub(crate) fn record(&self, tag: &str, xref: Option<&str>) -> bool {
        self.conv.record_tag(tag) != (None, Fate::Keep) || self.conv.record_xref(xref).is_some()
    }

    /// Whether a rule applies to a structure tagged `tag` with `payload`
    /// under structures tagged `parent` and `grand` (`sub`: not the
    /// record), once [`Watch::may_apply`] says one may:
    /// [`Converter::triggers_at`].
    pub(crate) fn typed(
        &self,
        tag: &str,
        payload: PayloadRef<'_>,
        parent: &str,
        grand: &str,
        sub: bool,
    ) -> bool {
        self.conv.triggers_at(tag, payload, parent, grand, sub)
    }

    /// Whether a rule may apply to a structure tagged `tag` with `payload`,
    /// its superstructures unknown: [`Watch::typed`] without the last look,
    /// for the record's [`Converter::needs`] to tell.
    #[inline(never)]
    pub(crate) fn may(&self, tag: &str, payload: PayloadRef<'_>, special: u8) -> bool {
        self.conv.may_apply(tag) & payload_bit(payload) != 0
            || self.conv.may_apply_to(payload, special)
    }

    /// Whether converting record `r` may change it ([`Converter::needs`]).
    pub(crate) fn needs<'n, N: Node<'n>>(&self, r: N) -> bool {
        self.conv.needs(r)
    }
}

/// The structures of record `r`, when converting it may change something
/// ([`Converter::needs`]).
fn needed<'n, N: Node<'n>>(conv: &Converter, r: N) -> Option<Structure> {
    if conv.needs(r) {
        return Some(r.to_owned_structure());
    }
    debug_assert!(
        !conv.changes(&r.to_owned_structure()),
        "a conversion needs() missed: {:?}",
        r.to_owned_structure()
    );
    None
}

/// The structures of a record that writes itself.
pub(crate) fn built_structure<'n, B: Build<'n>>(b: &B) -> Structure {
    if let Some(s) = b.owned() {
        return s;
    }
    let mut scratch = Flat::default();
    b.build(&mut scratch);
    scratch
        .root()
        .map(Node::to_owned_structure)
        .unwrap_or_default()
}

/// A record of a dataset being converted, for the writer: as it is,
/// converted each time it is written, or owned (the header, the records
/// added).
pub(crate) enum Entry<'a, B> {
    Plain(&'a B),
    Converted {
        record: &'a B,
        plan: &'a Plan,
        index: usize,
    },
    Owned(&'a Structure),
}

impl<'a, B: Build<'a>> Build<'a> for Entry<'a, B> {
    fn tag(&self) -> &'a str {
        match self {
            Entry::Plain(r) => r.tag(),
            Entry::Converted { record, plan, .. } => {
                let tag = record.tag();
                plan.conv.record_tag(tag).0.unwrap_or(tag)
            }
            Entry::Owned(s) => s.tag.as_str(),
        }
    }

    fn xref(&self) -> Option<&'a str> {
        match self {
            Entry::Plain(r) => r.xref(),
            Entry::Converted { record, plan, .. } => {
                let xref = record.xref();
                plan.conv.record_xref(xref).or(xref)
            }
            Entry::Owned(s) => s.xref.as_deref(),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Entry::Plain(r) | Entry::Converted { record: r, .. } => r.is_empty(),
            Entry::Owned(s) => {
                s.payload == crate::tree::Payload::None && s.substructures.is_empty()
            }
        }
    }

    fn xref_id(&self) -> Option<crate::model::XrefId> {
        match self {
            Entry::Plain(r) => r.xref_id(),
            Entry::Converted { .. } | Entry::Owned(_) => None,
        }
    }

    fn standard_tag(&self) -> Option<u16> {
        match self {
            Entry::Plain(r) => r.standard_tag(),
            Entry::Converted { .. } | Entry::Owned(_) => crate::tree::standard_index(self.tag()),
        }
    }

    fn declared_version(&self) -> Option<std::borrow::Cow<'a, str>> {
        match self {
            Entry::Plain(r) => r.declared_version(),
            Entry::Converted { .. } | Entry::Owned(_) => None,
        }
    }

    fn typed_head(&self) -> Option<&'a crate::model::Header> {
        match self {
            Entry::Plain(r) => r.typed_head(),
            Entry::Converted { .. } | Entry::Owned(_) => None,
        }
    }

    fn build_completed_head(
        &self,
        out: &mut Flat<'a>,
        rules: &crate::version::VersionRules,
        submitter: crate::tree::FlatPayload<'a>,
    ) -> bool {
        match self {
            Entry::Plain(r) => r.build_completed_head(out, rules, submitter),
            Entry::Converted { .. } | Entry::Owned(_) => false,
        }
    }

    fn build(&self, out: &mut Flat<'a>) {
        match self {
            Entry::Plain(r) => r.build(out),
            Entry::Converted {
                record,
                plan,
                index,
            } => {
                let mut s = built_structure(*record);
                plan.convert(*index, &mut s);
                out.push_structure(s);
            }
            Entry::Owned(s) => out.push_typed(s),
        }
    }

    fn owned(&self) -> Option<Structure> {
        match self {
            Entry::Plain(r) => r.owned(),
            Entry::Converted { .. } => None,
            Entry::Owned(s) => Some((*s).clone()),
        }
    }
}

#[cfg(test)]
mod tests;
