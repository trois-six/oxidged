//! An export written as the dataset is, record by record
//! ([`Exporter`](super::Exporter)).
//!
//! Of the folds of an export profile, most datasets need only those that
//! rewrite a value where it stands: their places in the target's format,
//! their citations of source records written inline. Those are made as
//! each structure is written ([`Rewrite`], a
//! [`RowRewrite`](crate::model::driver::RowRewrite)), while a [`Census`]
//! (a [`Looker`]) looks at every structure written for what any other
//! fold would change, and at every record the conformance pass repairs
//! (the folds that come after it look at what it made). The header, the
//! submitters and the submission, which the folds of the header read and
//! change, are folded as drafts first and typed again. When nothing else
//! is to change, the write is the export; otherwise the dataset is folded
//! whole, as drafts.

use std::any::TypeId;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::model::driver::{RowRewrite, RowWrite, WriteCx};
use crate::model::{Citation, Dataset, Place, RecordRef, XrefId};
use crate::tree::STANDARD_TAGS;
use crate::tree::{FlatPayload, PayloadRef, UNKNOWN, Xref, standard_index, standard_index_const};
use crate::writer::Looker;
use crate::writer::{LINE_FEED, special_bytes};

use super::export::{
    AssocStyle, Limits, Op, PlaceStyle, Placed, Step, UNION_EVENTS, Y_EVENTS, impossible_date,
    place_in,
};

/// Whether a direct export knows what the fold `op` changes ([`Census`]),
/// or makes it as the dataset is written ([`Rewrite`]).
pub(super) fn made_direct(op: &Op) -> bool {
    match op {
        Op::EventAssociations(style) => *style != AssocStyle::Reversed,
        Op::ProfessionFacts
        | Op::OccupationsMerged
        | Op::ParticleInSurname
        | Op::SurnamesPacked
        | Op::NicknamesJoined
        | Op::UsualNameQuoted
        | Op::NoColonInNames
        | Op::AsEven(_)
        | Op::EventsSaidY
        | Op::DeathSaidY
        | Op::OneBaptism
        | Op::UnionType
        | Op::EmptyPacs
        | Op::Godparents
        | Op::Adoptions
        | Op::ChildLinkSources
        | Op::InlineSources
        | Op::InlineMedia
        | Op::EscapedNotes
        | Op::OneHeadNote
        | Op::FieldsAsNotes(_)
        | Op::Places(_)
        | Op::AddressesAsNotes
        | Op::BadDates
        | Op::SexSaid
        | Op::ExtensionsAsNotes => true,
        _ => false,
    }
}

/// The words of a set of standard tags.
const WORDS: usize = STANDARD_TAGS.len().div_ceil(64);

/// Standard tags, by their index.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Tags([u64; WORDS]);

impl Tags {
    /// The tags `tags`, the standard ones.
    fn of(tags: &[&str]) -> Self {
        let mut set = Self::default();
        for t in tags {
            if let Some(i) = standard_index(t) {
                set.add(i);
            }
        }
        set
    }

    #[inline]
    fn add(&mut self, i: u16) {
        if let Some(w) = self.0.get_mut(usize::from(i) / 64) {
            *w |= 1 << (i % 64);
        }
    }

    #[inline]
    fn has(&self, i: u16) -> bool {
        self.0
            .get(usize::from(i) / 64)
            .is_some_and(|w| w & (1 << (i % 64)) != 0)
    }

    /// Whether one of `other` is among them.
    #[inline]
    fn meets(&self, other: &Tags) -> bool {
        self.0.iter().zip(&other.0).any(|(a, b)| a & b != 0)
    }
}

/// The index of a standard tag, at compile time.
const fn std_tag(tag: &str) -> u16 {
    match standard_index_const(tag) {
        Some(i) => i,
        // (Each is standard: a test sees to it.)
        None => u16::MAX,
    }
}

const NAME: u16 = std_tag("NAME");
const NOTE: u16 = std_tag("NOTE");
const PLAC: u16 = std_tag("PLAC");
const SOUR: u16 = std_tag("SOUR");
const FAMC: u16 = std_tag("FAMC");
const OCCU: u16 = std_tag("OCCU");
const CHIL: u16 = std_tag("CHIL");
const DATE: u16 = std_tag("DATE");
const INDI: u16 = std_tag("INDI");
const FAM: u16 = std_tag("FAM");
const REPO: u16 = std_tag("REPO");
const SEX: u16 = std_tag("SEX");
const HEAD: u16 = std_tag("HEAD");
const SUBM: u16 = std_tag("SUBM");
const SUBN: u16 = std_tag("SUBN");
const DEAT: u16 = std_tag("DEAT");
const HUSB: u16 = std_tag("HUSB");
const WIFE: u16 = std_tag("WIFE");
const BURI: u16 = std_tag("BURI");
const CREM: u16 = std_tag("CREM");
const NO: u16 = std_tag("NO");
const BAPM: u16 = std_tag("BAPM");
const CHR: u16 = std_tag("CHR");

/// What a record's structures say that a fold looks for
/// ([`Seen::flags`]).
mod flag {
    /// `:` in a name or a structure under it.
    pub(super) const NAME_COLON: u32 = 1;
    /// `<` or `&` in a note.
    pub(super) const NOTE_MARKUP: u32 = 1 << 1;
    /// An event that says nothing (no payload, no substructure).
    pub(super) const BARE_EVENT: u32 = 1 << 2;
    /// A citation of a source record left as it is.
    pub(super) const CITATION: u32 = 1 << 3;
    /// A citation under a child link.
    pub(super) const FAMC_SOUR: u32 = 1 << 4;
    /// An impossible date.
    pub(super) const BAD_DATE: u32 = 1 << 5;
}

/// The records a [`Rule`] is about: persons, families, the others.
const PERSONS: u8 = 1;
const FAMILIES: u8 = 2;
const OTHERS: u8 = 4;
const ANY: u8 = PERSONS | FAMILIES | OTHERS;

/// When a fold may change a record, as the census tells it: a [`Rule`].
#[derive(Clone, Copy)]
enum When {
    /// One of these tags among its substructures, below them, anywhere.
    Top(Tags),
    Below(Tags),
    Anywhere(Tags),
    /// Both tags among its substructures.
    Both(u16, u16),
    /// An extension structure among its substructures, below them,
    /// anywhere.
    ExtensionTop,
    ExtensionBelow,
    Extension,
    /// One of these [`flag`]s.
    Flag(u32),
    /// At least two names, occupations, child links.
    Names,
    Occupations,
    ChildLinks,
    /// A burial or cremation without a death.
    Buried,
    /// A family that says no union, or the union it is not.
    Union,
    /// No sex.
    NoSex,
    /// A place the write did not rewrite as the place fold does.
    Places,
}

/// A fold changes a record of `records` ([`PERSONS`], …) when `when`.
#[derive(Clone, Copy)]
struct Rule {
    records: u8,
    when: When,
}

/// The rules that tell the census what the folds `steps` change; `media`:
/// the target rewrites media paths.
fn rules(steps: &[Step], media: bool) -> Vec<Rule> {
    let mut out = Vec::new();
    let mut rule = |records: u8, when: When| out.push(Rule { records, when });
    let both = PERSONS | FAMILIES;
    for s in steps {
        match s.fold {
            Op::ProfessionFacts => rule(PERSONS, When::Top(Tags::of(&["FACT"]))),
            Op::OccupationsMerged => rule(PERSONS, When::Occupations),
            Op::ParticleInSurname => rule(PERSONS, When::Anywhere(Tags::of(&["SPFX"]))),
            Op::SurnamesPacked => rule(PERSONS, When::Names),
            Op::NicknamesJoined => rule(PERSONS, When::Anywhere(Tags::of(&["NICK"]))),
            Op::UsualNameQuoted => rule(PERSONS, When::ExtensionBelow),
            Op::NoColonInNames => rule(PERSONS, When::Flag(flag::NAME_COLON)),
            Op::AsEven(map) => {
                let tags: Vec<&str> = map.iter().map(|(t, _)| *t).collect();
                rule(both, When::Top(Tags::of(&tags)));
                if tags.iter().any(|t| standard_index(t).is_none()) {
                    rule(both, When::ExtensionTop);
                }
            }
            Op::EventsSaidY => rule(both, When::Flag(flag::BARE_EVENT)),
            Op::DeathSaidY => rule(PERSONS, When::Buried),
            Op::OneBaptism => rule(PERSONS, When::Both(BAPM, CHR)),
            Op::UnionType => rule(FAMILIES, When::Union),
            Op::EmptyPacs => rule(FAMILIES, When::Top(Tags::of(&["EVEN"]))),
            Op::Godparents | Op::EventAssociations(_) => {
                rule(ANY, When::Anywhere(Tags::of(&["ASSO"])));
                rule(ANY, When::Extension);
            }
            Op::Adoptions => {
                rule(PERSONS, When::Below(Tags::of(&["PEDI"])));
                rule(PERSONS, When::ChildLinks);
            }
            Op::ChildLinkSources => rule(PERSONS, When::Flag(flag::FAMC_SOUR)),
            Op::InlineSources => rule(ANY, When::Flag(flag::CITATION)),
            Op::InlineMedia => rule(ANY, When::Anywhere(Tags::of(&["OBJE"]))),
            Op::EscapedNotes => {
                rule(ANY, When::Flag(flag::NOTE_MARKUP));
                rule(ANY, When::Anywhere(Tags::of(&["MIME"])));
            }
            Op::FieldsAsNotes(fields) => {
                let tags: Vec<&str> = fields.iter().map(|(t, _)| *t).collect();
                rule(both, When::Below(Tags::of(&tags)));
                if tags.iter().any(|t| standard_index(t).is_none()) {
                    rule(both, When::ExtensionBelow);
                }
                if tags.contains(&"AGE") {
                    rule(both, When::Below(Tags::of(&["HUSB", "WIFE"])));
                }
            }
            Op::Places(_) => rule(ANY, When::Places),
            Op::AddressesAsNotes => rule(both, When::Below(Tags::of(&["ADDR"]))),
            Op::BadDates => rule(ANY, When::Flag(flag::BAD_DATE)),
            Op::SexSaid => rule(PERSONS, When::NoSex),
            Op::ExtensionsAsNotes => rule(ANY, When::Extension),
            // Made by none: in the header (folded apart).
            Op::OneHeadNote => {}
            // The others are not made as the dataset is written
            // ([`made_direct`]).
            _ => rule(ANY, When::Flag(u32::MAX)),
        }
    }
    if media {
        rule(ANY, When::Anywhere(Tags::of(&["FILE"])));
    }
    out
}

/// The [`Rule`]s about one kind of record ([`Rule::records`]), merged:
/// what [`Census::changes`] looks at.
#[derive(Clone, Default)]
struct Masks {
    top: Tags,
    below: Tags,
    anywhere: Tags,
    flags: u32,
    /// The other checks, as [`check`] bits.
    checks: u16,
    both: Vec<(u16, u16)>,
}

/// The checks of [`Masks::checks`], one per [`When`] that is no set of
/// tags or flags.
mod check {
    pub(super) const EXT_TOP: u16 = 1;
    pub(super) const EXT_BELOW: u16 = 1 << 1;
    pub(super) const NAMES: u16 = 1 << 2;
    pub(super) const OCCUPATIONS: u16 = 1 << 3;
    pub(super) const CHILD_LINKS: u16 = 1 << 4;
    pub(super) const BURIED: u16 = 1 << 5;
    pub(super) const UNION: u16 = 1 << 6;
    pub(super) const NO_SEX: u16 = 1 << 7;
    pub(super) const PLACES: u16 = 1 << 8;
}

impl Masks {
    /// The rules of `rules` about the records of `records`.
    fn of(rules: &[Rule], records: u8) -> Self {
        let mut m = Self::default();
        let or = |a: &mut Tags, b: &Tags| a.0.iter_mut().zip(&b.0).for_each(|(x, y)| *x |= y);
        for r in rules.iter().filter(|r| r.records & records != 0) {
            match r.when {
                When::Top(t) => or(&mut m.top, &t),
                When::Below(t) => or(&mut m.below, &t),
                When::Anywhere(t) => or(&mut m.anywhere, &t),
                When::Both(a, b) => m.both.push((a, b)),
                When::ExtensionTop => m.checks |= check::EXT_TOP,
                When::ExtensionBelow => m.checks |= check::EXT_BELOW,
                When::Extension => m.checks |= check::EXT_TOP | check::EXT_BELOW,
                When::Flag(f) => m.flags |= f,
                When::Names => m.checks |= check::NAMES,
                When::Occupations => m.checks |= check::OCCUPATIONS,
                When::ChildLinks => m.checks |= check::CHILD_LINKS,
                When::Buried => m.checks |= check::BURIED,
                When::Union => m.checks |= check::UNION,
                When::NoSex => m.checks |= check::NO_SEX,
                When::Places => m.checks |= check::PLACES,
            }
        }
        m
    }
}

/// The deepest level whose tag the census keeps.
const LEVELS: usize = 16;

/// What the census saw of the record being written.
#[derive(Clone, Default)]
struct Seen {
    /// The record's tag: its index when standard.
    tag: Option<u16>,
    /// The standard tags of its substructures, and of the structures below
    /// them.
    top: Tags,
    below: Tags,
    /// Whether an extension structure is among its substructures, below
    /// them.
    ext_top: bool,
    ext_below: bool,
    /// The names, occupations and child links among its substructures,
    /// and children.
    names: u32,
    occupations: u32,
    child_links: u32,
    children: u32,
    /// The deepest level.
    depth: usize,
    /// The lengths, in characters, of its names (of a person) longer than
    /// the importer takes.
    long_names: Vec<usize>,
    /// The places, with a text.
    places: u32,
    flags: u32,
    /// The tag of the structure open at each level (`None`: an extension).
    open: [Option<u16>; LEVELS],
    /// The level of the extension structure the structures written are
    /// under, when they are.
    extension: Option<usize>,
    /// The substructure of the record written last, while it may be one
    /// that says nothing: its tag.
    bare: Option<u16>,
}

impl Seen {
    /// Which [`Rule::records`] the record is.
    fn records(&self) -> u8 {
        match self.tag {
            Some(INDI) => PERSONS,
            Some(FAM) => FAMILIES,
            _ => OTHERS,
        }
    }
}

/// What the hooks counted of a record as it was written last
/// ([`Rewrite`]).
#[derive(Clone, Copy, Default)]
struct Rewritten {
    /// The places the place fold looked at.
    places: u32,
    /// Of them, changed, guessed and left as they are for having too many
    /// jurisdictions.
    changed: u32,
    guessed: u32,
    skipped: u32,
    /// The citations written inline.
    inline: u32,
    /// The places left in a form other than the target's.
    unfit: u32,
}

/// What the census found of the whole write.
#[derive(Default)]
pub(super) struct Found {
    /// Whether a record needs a fold the write does not make, or was
    /// repaired: the dataset is folded whole.
    pub(super) whole: bool,
    /// Per record whose citations were written inline: its identifier and
    /// how many.
    pub(super) inline: Vec<(Option<Xref>, u32)>,
    /// The places changed, guessed and left.
    pub(super) changed: usize,
    pub(super) guessed: usize,
    pub(super) skipped: usize,
}

/// Looks at what an export's write writes ([`Looker`]): what a fold
/// would change of each record. See the [module documentation](self).
pub(super) struct Census {
    /// The rules, by the records they are about: persons, families, the
    /// others.
    masks: [Masks; 3],
    /// The events a fold says `Y` of when they say nothing.
    said: Tags,
    /// The family events that say a union.
    union: Tags,
    /// The importer's limits, found record after record.
    limits: Limits<'static>,
    max_name: Option<usize>,
    /// The identifier of the record being written.
    xref: Option<String>,
    rewritten: Rc<Cell<Rewritten>>,
    /// The record being written, when one is.
    seen: Option<Seen>,
    found: Found,
}

impl Census {
    /// A census for the steps `steps` of an export to `v7` (7.x when
    /// true); `media`: whether it rewrites media paths; `limits`: the
    /// importer's limits, the records before counted; `hooks`: the folds
    /// made as the dataset is written.
    pub(super) fn new(
        steps: &[Step],
        v7: bool,
        media: bool,
        limits: Limits<'static>,
        hooks: &Rewrite<'_>,
    ) -> Self {
        // 5.5.1 says `Y` for these four only (7.x for every event).
        let said = if v7 {
            Tags::of(Y_EVENTS)
        } else {
            Tags::of(&["BIRT", "CHR", "DEAT", "MARR"])
        };
        Self {
            masks: {
                let rules = rules(steps, media);
                [
                    Masks::of(&rules, PERSONS),
                    Masks::of(&rules, FAMILIES),
                    Masks::of(&rules, OTHERS),
                ]
            },
            said,
            union: Tags::of(UNION_EVENTS),
            max_name: limits.max_name(),
            limits,
            xref: None,
            rewritten: Rc::clone(&hooks.rewritten),
            seen: None,
            found: Found::default(),
        }
    }

    /// What it found, and the limits.
    pub(super) fn into_found(self) -> (Found, Limits<'static>) {
        (self.found, self.limits)
    }

    /// Whether a fold changes the record `s` saw.
    fn changes(&self, s: &Seen) -> bool {
        let m = match s.records() {
            PERSONS => &self.masks[0],
            FAMILIES => &self.masks[1],
            _ => &self.masks[2],
        };
        let on = |c: u16| m.checks & c != 0;
        s.top.meets(&m.top)
            || s.below.meets(&m.below)
            || s.top.meets(&m.anywhere)
            || s.below.meets(&m.anywhere)
            || s.flags & m.flags != 0
            || (on(check::EXT_TOP) && s.ext_top)
            || (on(check::EXT_BELOW) && s.ext_below)
            || (on(check::NAMES) && s.names >= 2)
            || (on(check::OCCUPATIONS) && s.occupations >= 2)
            || (on(check::CHILD_LINKS) && s.child_links >= 2)
            || (on(check::BURIED) && !s.top.has(DEAT) && (s.top.has(BURI) || s.top.has(CREM)))
            || (on(check::UNION)
                && (s.top.has(NO)
                    || s.ext_top
                    || ((s.top.has(HUSB) || s.top.has(WIFE)) && !s.top.meets(&self.union))))
            || (on(check::NO_SEX) && !s.top.has(SEX))
            || (on(check::PLACES) && s.places != self.rewritten.get().places)
            || m.both.iter().any(|&(a, b)| s.top.has(a) && s.top.has(b))
    }

    /// Notes the structure of standard tag `standard` (`None`: another
    /// tag), identified as `xref`, with `payload` at `level` of the record
    /// being written.
    fn look(
        &mut self,
        level: usize,
        xref: Option<&str>,
        standard: Option<u16>,
        payload: PayloadRef<'_>,
    ) {
        let Some(s) = self.seen.as_mut() else {
            return;
        };
        let text = match payload {
            PayloadRef::Text(t) => Some(t),
            _ => None,
        };
        if level == 0 {
            s.tag = standard;
            match &mut self.xref {
                Some(x) => {
                    x.clear();
                    x.push_str(xref.unwrap_or_default());
                }
                None => self.xref = xref.map(str::to_string),
            }
            // A shared note's text.
            if standard == Some(NOTE) && text.is_some_and(markup) {
                s.flags |= flag::NOTE_MARKUP;
            }
            return;
        }
        // The substructure before said nothing when this one is not below
        // it.
        if let Some(bare) = s.bare.take()
            && level == 1
            && self.said.has(bare)
        {
            s.flags |= flag::BARE_EVENT;
        }
        if s.extension.is_some_and(|l| level <= l) {
            s.extension = None;
        }
        s.depth = s.depth.max(level);
        let parent = s.open.get(level - 1).copied().flatten();
        if let Some(slot) = s.open.get_mut(level) {
            *slot = standard;
        }
        match standard {
            Some(i) if level == 1 => {
                s.top.add(i);
                match i {
                    NAME => s.names += 1,
                    OCCU => s.occupations += 1,
                    FAMC => s.child_links += 1,
                    CHIL => s.children += 1,
                    _ => {}
                }
                if matches!(payload, PayloadRef::None) {
                    s.bare = Some(i);
                }
            }
            Some(i) => s.below.add(i),
            None => {
                if level == 1 {
                    s.ext_top = true;
                } else {
                    s.ext_below = true;
                }
                if s.extension.is_none() {
                    s.extension = Some(level);
                }
            }
        }
        if let Some(t) = text {
            let named = if level == 1 {
                standard == Some(NAME)
            } else {
                s.open.get(1).copied().flatten() == Some(NAME)
            };
            if named && t.as_bytes().contains(&b':') {
                s.flags |= flag::NAME_COLON;
            }
            match standard {
                Some(NAME) if level == 1 && s.tag == Some(INDI) => {
                    if let Some(max) = self.max_name.filter(|&max| t.len() > max) {
                        let len = t.chars().count();
                        if len > max {
                            s.long_names.push(len);
                        }
                    }
                }
                Some(NOTE) if markup(t) => s.flags |= flag::NOTE_MARKUP,
                Some(DATE) if may_be_impossible(t) && impossible_date(t) => {
                    s.flags |= flag::BAD_DATE;
                }
                Some(PLAC) => s.places += 1,
                _ => {}
            }
        }
        if standard == Some(SOUR) {
            if matches!(payload, PayloadRef::Pointer(_)) && !matches!(s.tag, Some(SOUR | REPO)) {
                s.flags |= flag::CITATION;
            }
            if level == 2 && parent == Some(FAMC) {
                s.flags |= flag::FAMC_SOUR;
            }
        }
    }
}

/// What a citation names of a source record ([`Rewrite::with_source`]):
/// its text and [`special_bytes`]; `None` when the fold writes it otherwise.
type Source = Option<(String, u8)>;

/// Whether `t` has `<` or `&`.
#[inline]
fn markup(t: &str) -> bool {
    t.contains('<') || t.contains('&')
}

/// Whether a date may name a day its month does not have
/// ([`impossible_date`]): a day of 0 or over 28 before a month.
fn may_be_impossible(t: &str) -> bool {
    let b = t.as_bytes();
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        // A number starting a word.
        let starts = i == 0 || b.get(i - 1).is_some_and(|p| !p.is_ascii_digit());
        if !(c.is_ascii_digit() && starts) {
            i += 1;
            continue;
        }
        let (mut n, mut digits) = (0_u32, 0);
        while let Some(d) = b.get(i).filter(|d| d.is_ascii_digit()) {
            n = n.saturating_mul(10).saturating_add(u32::from(d - b'0'));
            digits += 1;
            i += 1;
        }
        // Not 1 to 28 (or longer than a day), then more.
        if (n == 0 || n > 28 || digits > 3) && b.get(i).is_some() {
            return true;
        }
    }
    false
}

impl Looker for Census {
    fn structure(
        &mut self,
        level: usize,
        xref: Option<&str>,
        _tag: &str,
        standard: Option<u16>,
        payload: PayloadRef<'_>,
    ) {
        self.look(level, xref, standard, payload);
    }

    // The record is seen again from its start.
    fn rollback(&mut self) {
        if self.seen.is_some() {
            self.seen = Some(Seen::default());
        }
    }

    fn begin(&mut self, _index: usize) {
        self.seen = Some(Seen::default());
    }

    fn end(&mut self, _index: usize, repaired: bool) -> bool {
        let Some(mut s) = self.seen.take() else {
            return true;
        };
        if let Some(bare) = s.bare.take()
            && self.said.has(bare)
        {
            s.flags |= flag::BARE_EVENT;
        }
        // The header, the submitters and the submission were folded apart.
        if matches!(s.tag, Some(HEAD | SUBM | SUBN)) {
            return true;
        }
        // A place left out of the target's format: the header names no
        // format then (the whole fold tells).
        let r = self.rewritten.get();
        if repaired || r.unfit > 0 || self.changes(&s) {
            self.found.whole = true;
            return false;
        }
        let found = &mut self.found;
        found.changed += r.changed as usize;
        found.guessed += r.guessed as usize;
        found.skipped += r.skipped as usize;
        let xref = self.xref.as_deref().filter(|x| !x.is_empty());
        if r.inline > 0 {
            found.inline.push((xref.map(Xref::new), r.inline));
        }
        let children = (s.tag == Some(FAM)).then_some(s.children as usize);
        self.limits
            .add(xref, s.tag == Some(INDI), s.depth, s.long_names, children);
        true
    }
}

/// The folds an export makes as each structure is written
/// ([`RowRewrite`]): places in the target's format, citations of source
/// records inline. See the [module documentation](self).
pub(super) struct Rewrite<'d> {
    data: &'d Dataset,
    /// The place fold: its format, and the jurisdictions the header names
    /// (as slots of the target's) and whether each is known.
    places: Option<(PlaceStyle, Vec<Option<usize>>, bool)>,
    /// The jurisdictions of the target's format, as its slots.
    identity: Vec<Option<usize>>,
    /// Whether citations of source records are written inline.
    inline: bool,
    /// Whether the record being written is one the folds rewrite.
    rewrites: Cell<bool>,
    /// Whether the record being written may have its citations inline.
    cites: Cell<bool>,
    /// By identifier, the text of the source record cited (its page
    /// aside) and its [`special_bytes`]: `Some(None)` when it names no
    /// source record the fold writes so.
    sources: RefCell<Vec<Option<Source>>>,
    rewritten: Rc<Cell<Rewritten>>,
}

impl<'d> Rewrite<'d> {
    /// The folds of `steps` made as `data` is written, with `mapping`: the
    /// jurisdictions the header names, and whether each is known, as the
    /// place fold reads them.
    pub(super) fn new(
        data: &'d Dataset,
        steps: &[Step],
        mapping: (Vec<Option<usize>>, bool),
    ) -> Self {
        let style = steps.iter().find_map(|s| match s.fold {
            Op::Places(style) => Some(style),
            _ => None,
        });
        let places = style.map(|s| (s, mapping.0, mapping.1));
        let inline = steps.iter().any(|s| matches!(s.fold, Op::InlineSources));
        let identity = style.map_or_else(Vec::new, |s| {
            (0..s.form().split(',').count()).map(Some).collect()
        });
        Self {
            data,
            places,
            identity,
            inline,
            rewrites: Cell::new(false),
            cites: Cell::new(false),
            sources: RefCell::new(Vec::new()),
            rewritten: Rc::new(Cell::new(Rewritten::default())),
        }
    }

    /// The place `text`, as the place fold leaves it: `None` when as it
    /// is.
    fn place(&self, text: &str, r: &mut Rewritten) -> Option<String> {
        let (style, mapping, known) = self.places.as_ref()?;
        r.places += 1;
        if text.trim_start().starts_with('[') {
            if !style.fits(text) {
                r.unfit += 1;
            }
            return None;
        }
        let (mapping, known) = if !known && style.fits(text) {
            (self.identity.as_slice(), true)
        } else {
            (mapping.as_slice(), *known)
        };
        let new = match place_in(text, mapping, known, *style) {
            Placed::Mapped(new) => new,
            Placed::Guessed(new) => {
                r.guessed += 1;
                new
            }
            Placed::TooMany => {
                r.skipped += 1;
                r.unfit += 1;
                return None;
            }
        };
        if new == text {
            return None;
        }
        r.changed += 1;
        Some(new)
    }

    /// `f` of the text of the source record `id` as a citation names it,
    /// its page aside; `None` when `id` names no source record the fold
    /// writes so.
    fn with_source<R>(&self, id: XrefId, f: impl FnOnce(Option<(&str, u8)>) -> R) -> R {
        let at = id.index();
        if !matches!(self.sources.borrow().get(at), Some(Some(_))) {
            let text = self.source_text_of(id).map(|t| {
                let special = special_bytes(&t);
                (t, special)
            });
            let mut sources = self.sources.borrow_mut();
            if sources.len() <= at {
                sources.resize(at + 1, None);
            }
            if let Some(slot) = sources.get_mut(at) {
                *slot = Some(text);
            }
        }
        let sources = self.sources.borrow();
        f(sources
            .get(at)
            .and_then(|s| s.as_ref())
            .and_then(Option::as_ref)
            .map(|(t, s)| (t.as_str(), *s)))
    }

    fn source_text_of(&self, id: XrefId) -> Option<String> {
        let data = self.data;
        let Some(RecordRef::Source(s)) = data.find_record(id) else {
            return None;
        };
        // A source record with structures the model keeps untyped may name
        // itself otherwise.
        if !s.extra().is_empty() {
            return None;
        }
        let mut out = String::new();
        let mut part = |p: &str, flat: bool| {
            if p.trim().is_empty() {
                return;
            }
            if !out.is_empty() {
                out.push_str(", ");
            }
            if flat && p.contains('\n') {
                out.push_str(&p.replace('\n', " "));
            } else {
                out.push_str(p);
            }
        };
        let title = s
            .title()
            .filter(|t| !t.is_empty())
            .or_else(|| s.abbreviation().filter(|t| !t.is_empty()));
        if let Some(t) = title {
            part(&t.to_str(), true);
        }
        if let Some(a) = s.author().filter(|t| !t.is_empty()) {
            part(&a.to_str(), true);
        }
        for repo in s.repositories() {
            if let Some(r) = repo.repository() {
                match data.find_record(r) {
                    Some(RecordRef::Repository(r)) => {
                        if !r.extra().is_empty() {
                            return None;
                        }
                        if let Some(name) = r.name().filter(|t| !t.is_empty()) {
                            part(&name.to_str(), false);
                        }
                    }
                    None => {}
                    // A pointer to another record names it otherwise.
                    Some(_) => return None,
                }
            }
            if let Some(caln) = repo.call_numbers().first() {
                part(&caln.value().to_str(), false);
            }
        }
        Some(out)
    }

    /// The citation `s` written inline, when it is one the fold writes as
    /// a text alone (a pointer to a source record, and its page), with its
    /// [`special_bytes`].
    fn citation(&self, s: &dyn RowWrite, cx: &WriteCx<'_>) -> Option<(String, u8)> {
        let FlatPayload::Pointer(_, Some(id)) = s.write_payload(cx) else {
            return None;
        };
        // Its page only (field 0).
        let mut other = false;
        s.holds(cx.tables, &mut |f| other |= f != 0);
        if other {
            return None;
        }
        let mut page: Option<(Cow<'_, str>, u8)> = None;
        s.field_texts(0, cx.tables, &mut |span| {
            if page.is_none() {
                page = Some((cx.store.span_str(span), span.special()));
            }
        });
        let page = page.filter(|(p, _)| !p.trim().is_empty());
        let (mut text, mut special) = self.with_source(id, |source| {
            source.map(|(source, special)| {
                let room = source.len() + page.as_ref().map_or(0, |(p, _)| p.len() + 2);
                let mut text = String::with_capacity(room.max(6));
                text.push_str(source);
                (text, special)
            })
        })?;
        if let Some((page, page_special)) = page {
            // Its line breaks as spaces.
            special = if page_special & LINE_FEED == 0 && special != UNKNOWN {
                special | page_special
            } else {
                UNKNOWN
            };
            if !text.is_empty() {
                text.push_str(", ");
            }
            if page.contains('\n') {
                text.push_str(&page.replace('\n', " "));
            } else {
                text.push_str(&page);
            }
        }
        if text.is_empty() {
            text.push_str("Source");
        }
        Some((text, special))
    }
}

impl RowRewrite for Rewrite<'_> {
    fn tags(&self) -> [Option<u16>; 2] {
        [
            self.places.is_some().then_some(PLAC),
            self.inline.then_some(SOUR),
        ]
    }

    fn record(&self, tag: &str) {
        let folded = matches!(tag, "HEAD" | "SUBM" | "SUBN");
        self.rewrites.set(!folded);
        self.cites.set(!folded && !matches!(tag, "SOUR" | "REPO"));
        self.rewritten.set(Rewritten::default());
    }

    fn rewrite(&self, s: &dyn RowWrite, extra: bool, cx: &WriteCx<'_>) -> Option<(String, u8)> {
        if !self.rewrites.get() || extra {
            return None;
        }
        let ty = s.struct_type();
        if ty == TypeId::of::<Place>() {
            let mut held = false;
            s.holds(cx.tables, &mut |_| held = true);
            if held {
                return None;
            }
            let FlatPayload::Text(text, special) = s.write_payload(cx) else {
                return None;
            };
            let mut r = self.rewritten.get();
            let new = self.place(&text, &mut r);
            self.rewritten.set(r);
            // Of its characters, with commas, spaces and brackets.
            let special = if special == 0 { 0 } else { UNKNOWN };
            return new.map(|n| (n, special));
        }
        if ty == TypeId::of::<Citation>() && self.inline && self.cites.get() {
            let text = self.citation(s, cx)?;
            let mut r = self.rewritten.get();
            r.inline += 1;
            self.rewritten.set(r);
            return Some(text);
        }
        None
    }
}

/// `n` in decimal digits, after `out`.
pub(super) fn push_count(out: &mut String, n: u32) {
    let mut digits = [0_u8; 10];
    let mut at = digits.len();
    let mut n = n;
    loop {
        at -= 1;
        if let Some(d) = digits.get_mut(at) {
            *d = b'0' + (n % 10) as u8;
        }
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for &d in digits.get(at..).unwrap_or_default() {
        out.push(char::from(d));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_census_tags_are_standard() {
        for t in [
            NAME, NOTE, PLAC, SOUR, FAMC, OCCU, CHIL, DATE, INDI, FAM, REPO, SEX, HEAD, SUBM, SUBN,
            DEAT, HUSB, WIFE, BURI, CREM, NO, BAPM, CHR,
        ] {
            assert_ne!(t, u16::MAX);
        }
    }

    #[test]
    fn counts_are_written_in_digits() {
        for n in [0, 7, 10, 4_294_967_295] {
            let mut s = String::new();
            push_count(&mut s, n);
            assert_eq!(s, n.to_string());
        }
    }

    #[test]
    fn impossible_dates_are_looked_at() {
        for d in [
            "31 FEB 1900",
            "BET 30 FEB 1900 AND 1 MAR 1900",
            "0 JAN 1800",
            "31\u{a0}APR 1900",
        ] {
            assert!(!impossible_date(d) || may_be_impossible(d), "{d}");
            assert!(may_be_impossible(d), "{d}");
        }
        assert!(!may_be_impossible("1 JAN 1800"));
        assert!(!may_be_impossible("ABT 1850"));
    }
}
