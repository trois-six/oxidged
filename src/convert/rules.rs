//! The conversion rules, applied to one record at a time.
//!
//! Each record is converted on its own, from what the dataset-wide facts
//! (the header's source system, a 5.5.1 record named `@VOID@`, the
//! extension tags `HEAD.SCHMA` declares) and the identifiers it is handed
//! say; so a record converts the same way each time, which lets a writer
//! convert a large dataset one record at a time.

use std::collections::{BTreeMap, BTreeSet};

use super::ConversionOptions;
use super::tables::{
    EXID_TERMS, FAMILYSEARCH_PERSON, HEAD_FILE_URI, HEAD_SUBN_URI, KNOWN_EXTENSIONS, PHRASE_URI,
    RECORD_SUBN_URI, ROLE_NAMES, ROLE_WORDS, TRANSLATION_METHODS,
};
use super::text;
use crate::GedcomVersion;
use crate::tree::{Node, Payload, PayloadRef, Structure, Tag, Xref};
use crate::value::{AgeTexts, DateTexts};

/// The direction of a conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    /// GEDCOM 5.5.1 to 7.x.
    Up,
    /// GEDCOM 7.x to 5.5.1.
    Down,
}

/// What becomes of a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fate {
    /// Written, converted or not.
    Keep,
    /// Left out: 7.x has no place for it (a `SUBN` record).
    Remove,
}

/// Fresh identifiers for the records a conversion adds.
pub(crate) trait Fresh {
    /// A new identifier, unused in the dataset, starting with `prefix`.
    fn fresh(&mut self, prefix: &str) -> Box<str>;
}

/// What converting one record produced besides the record itself.
#[derive(Default)]
pub(crate) struct Out {
    /// Whether the record changed.
    pub(crate) changed: bool,
    /// The records it adds (multimedia and source records taken out of it).
    pub(crate) records: Vec<Structure>,
    /// The extension tags of well-known meaning it uses (7.x).
    pub(crate) known: BTreeSet<&'static str>,
}

/// The conversion of the records of one dataset from one version family
/// to the other.
#[derive(Clone, Debug)]
pub(crate) struct Converter {
    pub(crate) direction: Direction,
    target: GedcomVersion,
    options: ConversionOptions,
    /// Whether date, age and time payloads are converted too (structures
    /// read as they were written; the typed model converts its own).
    values: bool,
    /// The source system the header names (`HEAD.SOUR`), which 5.5.1 `RIN`
    /// numbers belong to.
    system: Option<String>,
    /// The identifier a 5.5.1 record named `@VOID@` takes in 7.x, where
    /// `@VOID@` points to nothing.
    void: Option<Box<str>>,
    /// The extension tags `HEAD.SCHMA` declares, with their URIs (7.x).
    declared: BTreeMap<String, String>,
}

/// `s` changed: `out` notes it.
macro_rules! changed {
    ($out:expr) => {
        $out.changed = true
    };
}

fn text_node(tag: &str, value: &str) -> Structure {
    Structure {
        payload: Payload::Text(value.into()),
        ..Structure::new(tag)
    }
}

fn set_text(s: &mut Structure, value: &str) {
    s.payload = if value.is_empty() {
        Payload::None
    } else {
        Payload::Text(value.into())
    };
}

fn retag(s: &mut Structure, tag: &str) {
    s.tag = Tag::new(tag);
}

fn is(s: &Structure, tag: &str) -> bool {
    s.tag.as_str() == tag
}

/// The text payload of the first `tag` substructure of `s`.
fn child_text<'s>(s: &'s Structure, tag: &str) -> Option<&'s str> {
    s.substructures
        .iter()
        .find(|c| is(c, tag))
        .and_then(Structure::text)
}

impl Converter {
    /// The conversion from `from` to `to`; `None` within one version family
    /// (5.5.1, or 7.0 and 7.1), which needs none. `values`: whether
    /// payloads are converted too.
    pub(crate) fn new(
        from: GedcomVersion,
        to: GedcomVersion,
        options: ConversionOptions,
        values: bool,
    ) -> Option<Self> {
        let direction = match (from.is_v7(), to.is_v7()) {
            (false, true) => Direction::Up,
            (true, false) => Direction::Down,
            _ => return None,
        };
        Some(Self {
            direction,
            target: to,
            options,
            values,
            system: None,
            void: None,
            declared: BTreeMap::new(),
        })
    }

    /// The version converted to.
    pub(crate) fn target(&self) -> GedcomVersion {
        self.target
    }

    /// Learns what the header says that records convert by: the source
    /// system, and the extension tags it declares.
    pub(crate) fn read_head(&mut self, head: &Structure) {
        self.system = child_text(head, "SOUR")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        self.declared = head
            .substructures
            .iter()
            .filter(|s| is(s, "SCHMA"))
            .flat_map(|s| s.substructures.iter().filter(|t| is(t, "TAG")))
            .filter_map(|t| t.text()?.split_once(' '))
            .map(|(tag, uri)| (tag.to_string(), uri.trim().to_string()))
            .collect();
    }

    /// Notes that a 5.5.1 record is named `@VOID@`: it takes `xref` in 7.x,
    /// and so do the pointers to it.
    pub(crate) fn rename_void(&mut self, xref: Box<str>) {
        self.void = Some(xref);
    }

    /// The identifier a record named `xref` takes; `None`: its own.
    pub(crate) fn record_xref(&self, xref: Option<&str>) -> Option<&str> {
        match (self.direction, xref) {
            (Direction::Up, Some(Xref::VOID)) => self.void.as_deref(),
            _ => None,
        }
    }

    /// The tag a record tagged `tag` takes, and whether it is written at
    /// all.
    pub(crate) fn record_tag(&self, tag: &str) -> (Option<&'static str>, Fate) {
        match (self.direction, tag) {
            (Direction::Up, "SUBN") if self.options.keep_obsolete => {
                (Some(SUBMISSION_TAG), Fate::Keep)
            }
            (Direction::Up, "SUBN") => (None, Fate::Remove),
            (Direction::Up, "NOTE") => (Some("SNOTE"), Fate::Keep),
            (Direction::Down, "SNOTE") => (Some("NOTE"), Fate::Keep),
            (Direction::Down, t) if self.declares(t, RECORD_SUBN_URI) => (Some("SUBN"), Fate::Keep),
            _ => (None, Fate::Keep),
        }
    }

    /// Whether `HEAD.SCHMA` declares `tag` as `uri`.
    fn declares(&self, tag: &str, uri: &str) -> bool {
        tag.starts_with('_') && self.declared.get(tag).is_some_and(|u| u == uri)
    }

    /// Whether a structure tagged `tag` is a 7.x `PHRASE`, standard or
    /// relocated as an extension.
    fn is_phrase(&self, tag: &str) -> bool {
        tag == "PHRASE" || self.declares(tag, PHRASE_URI)
    }

    /// Whether converting record `r` may change it, or the dataset (a
    /// record it adds, an extension it declares): what
    /// [`Converter::record`] looks at, found without copying the record. A
    /// record it says no of converts to itself; one it says yes of may too.
    pub(crate) fn needs<'n, N: Node<'n>>(&self, r: N) -> bool {
        let tag = r.tag();
        self.record_tag(tag) != (None, Fate::Keep)
            || self.record_xref(r.xref()).is_some()
            || self.triggers(r, "", "", false)
            || r.children().any(|c| self.needs_under(c, tag, ""))
    }

    /// [`Converter::needs`] of a substructure `n` under `parent` (itself
    /// under `grand`), and of its substructures.
    fn needs_under<'n, N: Node<'n>>(&self, n: N, parent: &str, grand: &str) -> bool {
        self.triggers(n, parent, grand, true)
            || n.children().any(|c| self.needs_under(c, n.tag(), parent))
    }

    /// Whether a rule applies to structure `n` under `parent` and `grand`
    /// (`sub`: a substructure, not a record).
    fn triggers<'n, N: Node<'n>>(&self, n: N, parent: &str, grand: &str, sub: bool) -> bool {
        self.triggers_at(n.tag(), n.payload(), parent, grand, sub)
    }

    /// [`Converter::triggers`] of a structure tagged `tag` with `payload`.
    pub(crate) fn triggers_at(
        &self,
        tag: &str,
        payload: PayloadRef<'_>,
        parent: &str,
        grand: &str,
        sub: bool,
    ) -> bool {
        let (text, pointer) = match payload {
            PayloadRef::Text(t) => (Some(t), None),
            PayloadRef::Pointer(p) => (None, Some(p)),
            PayloadRef::None => (None, None),
        };
        if KNOWN_EXTENSIONS.iter().any(|(t, _)| *t == tag)
            || tag == "_SDATE"
            || self.values && matches!(tag, "DATE" | "SDATE" | "AGE")
        {
            return true;
        }
        match self.direction {
            Direction::Up => {
                if pointer.is_some_and(|p| p == Xref::VOID) && self.void.is_some()
                    || text.is_some_and(|t| {
                        !self.options.keep_obsolete && has_escape(t) && !is_value(tag, parent)
                    })
                {
                    return true;
                }
                if !sub {
                    return false;
                }
                match tag {
                    "_UID" | "EMAI" | "_EMAIL" | "_ASSO" | "ASSO" | "RIN" | "AFN" | "RFN"
                    | "_FSFTID" | "_FID" | "FSFTID" => true,
                    "FONE" | "ROMN" => matches!(parent, "NAME" | "PLAC"),
                    "TYPE" => parent == "FORM" && grand == "FILE",
                    "NOTE" => pointer.is_some(),
                    "ALIA" => parent == "INDI" && text.is_some(),
                    "OBJE" | "SOUR" => pointer.is_none(),
                    "LANG" | "_LANG" => {
                        text.is_some_and(|t| text::language_tag(t).is_some() || !is_language_tag(t))
                    }
                    "FILE" => parent == "OBJE" && text.is_some_and(|t| text::file_uri(t).is_some()),
                    "FORM" => {
                        parent == "FILE" && text.is_some_and(|t| text::media_type(t).is_some())
                    }
                    "ROLE" => parent == "EVEN" && text.is_some_and(|t| t.trim().starts_with('(')),
                    "AGE" => text.is_some_and(|t| {
                        let t = t.trim();
                        t.eq_ignore_ascii_case("STILLBORN") || t.eq_ignore_ascii_case("0y")
                    }),
                    _ => false,
                }
            }
            Direction::Down => {
                if !sub {
                    return false;
                }
                text == Some("OTHER")
                    || matches!(parent, "INDI" | "FAM")
                        && text == Some("Y")
                        && Y_EVENTS.contains(&tag)
                    || match tag {
                        "SNOTE" => pointer.is_some(),
                        "EXID" | "ASSO" | "_ASSO" => true,
                        "TRAN" => matches!(parent, "NAME" | "PLAC"),
                        "LANG" | "_LANG" => {
                            text.is_some_and(|t| t == "und" || text::language_name(t).is_some())
                        }
                        "FILE" => {
                            parent == "OBJE" && text.is_some_and(|t| text::file_path(t).is_some())
                        }
                        "FORM" | "ROLE" => matches!(parent, "FILE" | "EVEN"),
                        _ => false,
                    }
            }
        }
    }

    /// The payloads ([`NONE`], [`TEXT`], [`POINTER`], as bits) with which
    /// a rule may apply to a structure tagged `tag`, whatever its
    /// superstructures: [`Converter::triggers_at`] is false of a structure
    /// of another tag or payload unless [`Converter::may_apply_to`] its
    /// payload.
    pub(crate) fn may_apply(&self, tag: &str) -> u8 {
        const ANY: u8 = NONE | TEXT | POINTER;
        if KNOWN_EXTENSIONS.iter().any(|(t, _)| *t == tag)
            || tag == "_SDATE"
            || self.values && matches!(tag, "DATE" | "SDATE" | "AGE")
        {
            return ANY;
        }
        match self.direction {
            Direction::Up => match tag {
                "_UID" | "EMAI" | "_EMAIL" | "_ASSO" | "ASSO" | "RIN" | "AFN" | "RFN"
                | "_FSFTID" | "_FID" | "FSFTID" | "FONE" | "ROMN" | "TYPE" => ANY,
                "NOTE" => POINTER,
                "OBJE" | "SOUR" => NONE | TEXT,
                "ALIA" | "LANG" | "_LANG" | "FILE" | "FORM" | "ROLE" | "AGE" => TEXT,
                _ => 0,
            },
            Direction::Down => match tag {
                "SNOTE" => POINTER,
                "EXID" | "ASSO" | "_ASSO" | "TRAN" | "FORM" | "ROLE" => ANY,
                "LANG" | "_LANG" | "FILE" => TEXT,
                _ => 0,
            },
        }
    }

    /// Every extension tag of known meaning converting records may use
    /// ([`Out::known`]): what a header written before the records declares
    /// to have them declared.
    pub(crate) fn declarable(&self) -> BTreeSet<&'static str> {
        let mut tags: BTreeSet<&'static str> = KNOWN_EXTENSIONS.iter().map(|&(t, _)| t).collect();
        tags.insert(PHRASE_TAG);
        if self.options.keep_obsolete {
            tags.insert(SUBMISSION_TAG);
        }
        tags
    }

    /// Whether a record named `xref` is the 5.5.1 record named `@VOID@`
    /// that takes another identifier in 7.x ([`Converter::rename_void`]),
    /// and none was given yet.
    pub(crate) fn names_void(&self, xref: Option<&str>) -> bool {
        self.direction == Direction::Up && self.void.is_none() && xref == Some(Xref::VOID)
    }

    /// The [`special_bytes`](crate::writer::special_bytes) of a text with
    /// which a rule may apply to it whatever its tag
    /// ([`Converter::may_apply_to`]): an `@` in 5.5.1.
    pub(crate) fn special_mask(&self) -> u8 {
        if self.direction == Direction::Up && !self.options.keep_obsolete {
            crate::writer::AT_SIGN
        } else {
            0
        }
    }

    /// Whether a rule may apply to a pointer whatever its tag
    /// ([`Converter::may_apply_to`]): `@VOID@` a record takes the name of.
    pub(crate) fn watches_pointers(&self) -> bool {
        self.direction == Direction::Up && self.void.is_some()
    }

    /// Whether a rule may apply to a text of `len` bytes whatever its tag,
    /// by its length ([`Converter::may_apply_to`]): `OTHER` and `Y` in 7.x.
    pub(crate) fn watches_len(&self, len: usize) -> bool {
        self.direction == Direction::Down && matches!(len, 1 | 5)
    }

    /// Whether a rule may apply to a structure with `payload` whatever its
    /// tag (see [`Converter::may_apply`]): in 5.5.1, a text with an `@`
    /// (`special`, its [`special_bytes`](crate::writer::special_bytes),
    /// says whether it has one), which may hold an escape, or the
    /// `@VOID@` pointer a record takes the name of; in 7.x, `OTHER` and
    /// `Y`.
    pub(crate) fn may_apply_to(&self, payload: PayloadRef<'_>, special: u8) -> bool {
        match (self.direction, payload) {
            (Direction::Up, PayloadRef::Text(_)) => {
                !self.options.keep_obsolete && special & crate::writer::AT_SIGN != 0
            }
            (Direction::Up, PayloadRef::Pointer(p)) => self.void.is_some() && p == Xref::VOID,
            (Direction::Down, PayloadRef::Text(t)) => t == "OTHER" || t == "Y",
            (_, PayloadRef::None) | (Direction::Down, PayloadRef::Pointer(_)) => false,
        }
    }

    /// Whether converting `r` changes it (for the checks of
    /// [`Converter::needs`]).
    pub(crate) fn changes(&self, r: &Structure) -> bool {
        struct Any;
        impl Fresh for Any {
            fn fresh(&mut self, prefix: &str) -> Box<str> {
                format!("@{prefix}@").into()
            }
        }
        let mut r = r.clone();
        let mut out = Out::default();
        let fate = self.record(&mut r, &mut Any, &mut out);
        fate == Fate::Remove || out.changed || !out.records.is_empty() || !out.known.is_empty()
    }

    /// Converts a record other than the header. `fresh` names the records
    /// it adds, which `out` collects.
    pub(crate) fn record(&self, r: &mut Structure, fresh: &mut dyn Fresh, out: &mut Out) -> Fate {
        let (tag, fate) = self.record_tag(r.tag.as_str());
        if fate == Fate::Remove {
            changed!(out);
            return fate;
        }
        if let Some(tag) = tag {
            retag(r, tag);
            if tag == SUBMISSION_TAG {
                out.known.insert(SUBMISSION_TAG);
            }
            changed!(out);
        }
        if let Some(x) = self.record_xref(r.xref.as_deref()) {
            r.xref = Some(Xref::new(x));
            changed!(out);
        }
        if let Some(&(t, _)) = KNOWN_EXTENSIONS.iter().find(|(t, _)| *t == r.tag.as_str()) {
            out.known.insert(t);
        }
        let mut cx = Cx {
            fresh,
            out,
            head: false,
            record: r.tag.as_str().to_string(),
            stillborn: false,
        };
        self.payload(r, "", &mut cx);
        self.children(r, "", &mut cx);
        if self.direction == Direction::Up && cx.stillborn && is(r, "INDI") {
            Self::stillborn_death(r, cx.out);
        }
        Fate::Keep
    }

    /// Converts the header, once every other record is converted:
    /// `known` are the extension tags of well-known meaning they use.
    pub(crate) fn head(&self, h: &mut Structure, known: &BTreeSet<&'static str>) -> bool {
        struct NoFresh;
        impl Fresh for NoFresh {
            fn fresh(&mut self, prefix: &str) -> Box<str> {
                format!("@{prefix}@").into()
            }
        }
        let mut out = Out::default();
        let mut declare: Vec<(String, &str)> = Vec::new();
        match self.direction {
            Direction::Up => {
                let keep = self.options.keep_obsolete;
                let mut dropped = false;
                h.substructures.retain_mut(|s| match s.tag.as_str() {
                    // The character set describes the bytes of the 5.5.1
                    // file; 7.x has neither a file name nor a submission.
                    t if t == "CHAR" || !keep && matches!(t, "FILE" | "SUBN") => {
                        dropped = true;
                        false
                    }
                    "GEDC" => {
                        let before = s.substructures.len();
                        s.substructures.retain(|g| !is(g, "FORM"));
                        dropped |= s.substructures.len() != before;
                        true
                    }
                    "FILE" | "SUBN" => {
                        let (tag, uri) = if is(s, "FILE") {
                            ("_FILE", HEAD_FILE_URI)
                        } else {
                            ("_SUBN", HEAD_SUBN_URI)
                        };
                        retag(s, tag);
                        declare.push((tag.to_string(), uri));
                        true
                    }
                    _ => true,
                });
                if dropped {
                    changed!(out);
                }
                for &tag in known {
                    if let Some(uri) = known_uri(tag) {
                        declare.push((tag.to_string(), uri));
                    }
                }
            }
            Direction::Down => {
                for s in &mut h.substructures {
                    match s.tag.as_str() {
                        t if self.declares(t, HEAD_FILE_URI) => {
                            retag(s, "FILE");
                            changed!(out);
                        }
                        t if self.declares(t, HEAD_SUBN_URI) => {
                            retag(s, "SUBN");
                            changed!(out);
                        }
                        _ => {}
                    }
                }
                // The declarations of what is standard again, and of what
                // converting to 7.x declares again, go.
                let undeclared = [HEAD_FILE_URI, HEAD_SUBN_URI, RECORD_SUBN_URI, PHRASE_URI];
                for schma in h.substructures.iter_mut().filter(|s| is(s, "SCHMA")) {
                    let before = schma.substructures.len();
                    schma.substructures.retain(|t| {
                        !(is(t, "TAG")
                            && t.text().and_then(|v| v.split_once(' ')).is_some_and(
                                |(tag, uri)| {
                                    let uri = uri.trim();
                                    undeclared.contains(&uri) || known_uri(tag) == Some(uri)
                                },
                            ))
                    });
                    if schma.substructures.len() != before {
                        changed!(out);
                    }
                }
                let before = h.substructures.len();
                h.substructures
                    .retain(|s| !(is(s, "SCHMA") && s.substructures.is_empty()));
                if h.substructures.len() != before {
                    changed!(out);
                }
            }
        }
        // The other conversions apply to the header as to any record, but
        // for the source citations it does not hold (`HEAD.SOUR` names the
        // system).
        let mut fresh = NoFresh;
        let mut cx = Cx {
            fresh: &mut fresh,
            out: &mut out,
            head: true,
            record: "HEAD".to_string(),
            stillborn: false,
        };
        self.children(h, "", &mut cx);
        if !declare.is_empty() {
            declare_tags(h, &declare);
            changed!(out);
        }
        out.changed
    }

    /// Converts the substructures of `s` (whose superstructure is tagged
    /// `parent`), and theirs.
    fn children(&self, s: &mut Structure, parent: &str, cx: &mut Cx<'_>) {
        // A standard tag is cloned without a copy of its text.
        let tag = s.tag.clone();
        let tag = tag.as_str();
        for c in &mut s.substructures {
            self.payload(c, tag, cx);
            match self.direction {
                Direction::Up => self.up(c, tag, parent, cx),
                Direction::Down => self.down(c, tag, cx),
            }
            if !c.substructures.is_empty() {
                self.children(c, tag, cx);
            }
        }
    }

    /// Converts the payload of `s` (under `parent`) itself: identifiers
    /// renamed, text escapes (5.5.1), dates and ages.
    fn payload(&self, s: &mut Structure, parent: &str, cx: &mut Cx<'_>) {
        match &mut s.payload {
            Payload::Pointer(p) => {
                if let (Direction::Up, Some(void)) = (self.direction, &self.void)
                    && p.as_str() == Xref::VOID
                {
                    *p = Xref::new(&**void);
                    changed!(cx.out);
                }
            }
            Payload::Text(t) => {
                if self.direction == Direction::Up
                    && !self.options.keep_obsolete
                    && !is_value(s.tag.as_str(), parent)
                    && let Some(new) = text::without_escapes(t)
                {
                    *t = new.into();
                    changed!(cx.out);
                }
            }
            Payload::None => {}
        }
        if self.direction == Direction::Up && is(s, "AGE") && is_stillborn(s, self) {
            cx.stillborn = true;
        }
        match s.tag.as_str() {
            "DATE" | "SDATE" if self.values => self.date(s, cx),
            "AGE" if self.values => self.age(s, cx),
            // A sort date kept as an extension in 5.5.1, which no typed
            // structure converts.
            "_SDATE" => self.date(s, cx),
            _ => {}
        }
    }

    /// A date payload, its `TIME` and `PHRASE`, in the target's grammar.
    fn date(&self, s: &mut Structure, cx: &mut Cx<'_>) {
        let take = |s: &Structure, tag: &str| child_text(s, tag).map(str::to_string);
        let before = DateTexts {
            value: s.text().map(str::to_string),
            time: take(s, "TIME"),
            phrase: s
                .substructures
                .iter()
                .find(|c| self.is_phrase(c.tag.as_str()))
                .and_then(|c| c.text().map(str::to_string)),
        };
        if s.substructures.iter().filter(|c| is(c, "TIME")).count() > 1 {
            return;
        }
        let after = before.to_version(self.target);
        if after == before {
            return;
        }
        set_text(s, after.value.as_deref().unwrap_or(""));
        set_leaf(s, "TIME", after.time.as_deref(), |t| t == "TIME");
        set_leaf(s, "PHRASE", after.phrase.as_deref(), |t| self.is_phrase(t));
        changed!(cx.out);
    }

    /// An age payload and its `PHRASE` in the target's grammar.
    fn age(&self, s: &mut Structure, cx: &mut Cx<'_>) {
        let before = AgeTexts {
            value: s.text().map(str::to_string),
            phrase: s
                .substructures
                .iter()
                .find(|c| self.is_phrase(c.tag.as_str()))
                .and_then(|c| c.text().map(str::to_string)),
        };
        let after = before.to_version(self.target);
        if after == before {
            return;
        }
        set_text(s, after.value.as_deref().unwrap_or(""));
        set_leaf(s, "PHRASE", after.phrase.as_deref(), |t| self.is_phrase(t));
        changed!(cx.out);
    }

    /// The 7.x death a 5.5.1 stillborn individual has (the migration
    /// guide: a `DEAT` at `AGE 0y`), unless it has one.
    fn stillborn_death(r: &mut Structure, out: &mut Out) {
        if r.substructures.iter().any(|c| is(c, "DEAT")) {
            return;
        }
        let mut age = text_node("AGE", "0y");
        age.substructures.push(text_node("PHRASE", "Stillborn"));
        let mut death = Structure::new("DEAT");
        death.substructures.push(age);
        r.substructures.push(death);
        changed!(out);
    }

    /// The 5.5.1 to 7.x rules for structure `c` under `parent` (itself
    /// under `grand`).
    fn up(&self, c: &mut Structure, parent: &str, grand: &str, cx: &mut Cx<'_>) {
        let tag = c.tag.as_str();
        // Renamed structures (the gedcom7code converter's list).
        let anywhere = RENAMED_UP.iter().find(|(from, _)| *from == tag);
        let renamed = match tag {
            _ if anywhere.is_some() => anywhere.map(|&(_, to)| to),
            // An association kept as an extension because it has no
            // pointer stays one.
            "_ASSO" if matches!(c.payload, Payload::Pointer(_)) => Some("ASSO"),
            "TYPE" if parent == "FORM" && grand == "FILE" => Some("MEDI"),
            "NOTE" if matches!(c.payload, Payload::Pointer(_)) => Some("SNOTE"),
            "FONE" | "ROMN" if matches!(parent, "NAME" | "PLAC") => {
                Self::translation(c);
                Some("TRAN")
            }
            _ => None,
        };
        if let Some(t) = renamed {
            retag(c, t);
            changed!(cx.out);
        }
        if let Some(&(t, _)) = KNOWN_EXTENSIONS.iter().find(|(t, _)| *t == c.tag.as_str()) {
            cx.out.known.insert(t);
        }
        match c.tag.as_str() {
            "ROLE" if parent == "EVEN" => {
                // A 5.5.1 role descriptor, `(word)`: `OTHER` and the word.
                if let Some(word) = c
                    .text()
                    .map(str::trim)
                    .and_then(|t| t.strip_prefix('('))
                    .and_then(|t| t.strip_suffix(')'))
                    .map(str::to_string)
                {
                    set_text(c, "OTHER");
                    c.substructures.insert(0, text_node("PHRASE", &word));
                    changed!(cx.out);
                }
            }
            "RIN" | "AFN" | "RFN" | "_FSFTID" | "_FID" | "FSFTID" if !cx.head => {
                self.exid(c);
                changed!(cx.out);
            }
            "ASSO" => Self::relation(c, cx),
            "ALIA" if parent == "INDI" => {
                // A name, as some 5.5.1 files write an alias (on one line):
                // a name of type AKA.
                if let Some(t) = c.text().filter(|t| !t.contains(['\n', '\r'])) {
                    let name = t.to_string();
                    retag(c, "NAME");
                    set_text(c, &name);
                    c.substructures.insert(0, text_node("TYPE", "AKA"));
                    changed!(cx.out);
                }
            }
            "OBJE" if !matches!(c.payload, Payload::Pointer(_)) && !cx.head => {
                self.inline_object(c, cx);
            }
            "SOUR" if matches!(c.payload, Payload::Text(_)) && !cx.head => {
                Self::text_source(c, cx);
            }
            "LANG" | "_LANG" => {
                if let Some(t) = c.text() {
                    if let Some(tag) = text::language_tag(t) {
                        set_text(c, tag);
                        changed!(cx.out);
                    } else if !is_language_tag(t) && is(c, "LANG") {
                        // 7.x has no phrase for a language: an extension
                        // that HEAD.SCHMA declares as one says it.
                        let raw = t.to_string();
                        set_text(c, "und");
                        c.substructures.push(text_node(PHRASE_TAG, &raw));
                        cx.out.known.insert(PHRASE_TAG);
                        changed!(cx.out);
                    }
                }
            }
            "FILE" if parent == "OBJE" => {
                if let Some(uri) = c.text().and_then(text::file_uri) {
                    set_text(c, &uri);
                    changed!(cx.out);
                }
            }
            "FORM" if parent == "FILE" => {
                if let Some(mt) = c.text().and_then(text::media_type) {
                    set_text(c, &mt);
                    changed!(cx.out);
                }
            }
            _ => {}
        }
    }

    /// A 5.5.1 phonetic or romanized variation (`FONE`, `ROMN`) as a 7.x
    /// translation: its `TYPE` (the method) becomes its language (`LANG`).
    fn translation(c: &mut Structure) {
        let phonetic = is(c, "FONE");
        let at = c.substructures.iter().position(|s| is(s, "TYPE"));
        let method = at
            .and_then(|i| c.substructures.get(i))
            .and_then(Structure::text)
            .map_or("", str::trim)
            .to_string();
        let known = TRANSLATION_METHODS
            .iter()
            .find(|(m, p, _)| *p == phonetic && m.eq_ignore_ascii_case(&method))
            .map(|&(_, _, tag)| tag.to_string());
        // A method no table names is a private-use subtag, when it can be
        // one; the method is kept beside (`_TYPE`) when the subtag does
        // not say it exactly.
        let private = |prefix: &str, fallback: &str| {
            let subtags: Vec<&str> = method
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|s| !s.is_empty())
                .collect();
            let fits = !subtags.is_empty() && subtags.iter().all(|s| s.len() <= 8);
            let words = subtags.join("-").to_ascii_lowercase();
            if fits {
                (format!("{prefix}-{words}"), words != method)
            } else {
                (fallback.to_string(), !method.is_empty())
            }
        };
        let (lang, keep_method) = match known {
            Some(tag) => (tag, false),
            None if phonetic => private("und-x-phonetic", "und-x-phonetic"),
            None => private("und-Latn-x", "und-Latn"),
        };
        let lang_node = text_node("LANG", &lang);
        match at {
            Some(i) if keep_method => {
                // A method no language tag can say stays, as an extension.
                if let Some(t) = c.substructures.get_mut(i) {
                    retag(t, "_TYPE");
                }
                c.substructures.insert(i, lang_node);
            }
            Some(i) => {
                if let Some(t) = c.substructures.get_mut(i) {
                    *t = lang_node;
                }
            }
            None => c.substructures.insert(0, lang_node),
        }
    }

    /// A 5.5.1 identifier (`RIN`, `AFN`, `RFN`) or a `FamilySearch` Family
    /// Tree identifier (`_FSFTID`) as a 7.x `EXID` with its `TYPE` (the
    /// migration guide; GEDCOM-registries `exid-types`).
    fn exid(&self, c: &mut Structure) {
        let value = c.text().unwrap_or("").trim().to_string();
        let (value, ty) = match c.tag.as_str() {
            "AFN" => (value, format!("{EXID_TERMS}AFN")),
            "RIN" => (
                value,
                match &self.system {
                    Some(system) => format!("{EXID_TERMS}RIN#{}", text::fragment(system)),
                    None => format!("{EXID_TERMS}RIN"),
                },
            ),
            "RFN" => match value.split_once(':') {
                Some((system, id)) if !system.is_empty() && !id.is_empty() => (
                    id.to_string(),
                    format!("{EXID_TERMS}RFN#{}", text::fragment(system)),
                ),
                _ => (value, format!("{EXID_TERMS}RFN")),
            },
            _ => (value, FAMILYSEARCH_PERSON.to_string()),
        };
        retag(c, "EXID");
        set_text(c, &value);
        c.substructures.insert(0, text_node("TYPE", &ty));
    }

    /// A 5.5.1 association: its relation (`RELA`) as a 7.x `ROLE`, a role
    /// value when the word names one (with the word as `PHRASE` when it
    /// says more), else `OTHER` with the word as `PHRASE`; a 5.5 record
    /// type (`TYPE INDI`) goes, as 7.x associations are with individuals.
    fn relation(c: &mut Structure, cx: &mut Cx<'_>) {
        let before = c.substructures.len();
        c.substructures
            .retain(|s| !(is(s, "TYPE") && s.text().is_some_and(|t| t.trim() == "INDI")));
        if c.substructures.len() != before {
            changed!(cx.out);
        }
        if c.substructures.iter().any(|s| is(s, "ROLE")) {
            return;
        }
        let Some(rela) = c.substructures.iter_mut().find(|s| is(s, "RELA")) else {
            return;
        };
        let word = rela.text().unwrap_or("").trim().to_string();
        let role = ROLE_WORDS
            .iter()
            .find(|(w, _)| w.eq_ignore_ascii_case(&word))
            .map_or("OTHER", |&(_, r)| r);
        retag(rela, "ROLE");
        set_text(rela, role);
        if !word.is_empty() && !word.eq_ignore_ascii_case(role) {
            rela.substructures.insert(0, text_node("PHRASE", &word));
        }
        changed!(cx.out);
    }

    /// A 5.5.1 multimedia link that holds its files: a new multimedia
    /// record holds them (and the rest), the link points to it and keeps
    /// its title. A 5.5 link's `FORM` moves into its file.
    fn inline_object(&self, c: &mut Structure, cx: &mut Cx<'_>) {
        if !c.substructures.iter().any(|s| is(s, "FILE")) {
            return;
        }
        // 5.5: `OBJE.FORM` beside `OBJE.FILE`.
        if let Some(form_at) = c.substructures.iter().position(|s| is(s, "FORM")) {
            let files = c.substructures.iter().filter(|s| is(s, "FILE")).count();
            let file_at = c.substructures.iter().position(|s| is(s, "FILE"));
            let has_form = file_at
                .and_then(|i| c.substructures.get(i))
                .is_some_and(|f| f.substructures.iter().any(|s| is(s, "FORM")));
            if files == 1 && !has_form {
                let form = c.substructures.remove(form_at);
                if let Some(file) = c.substructures.iter_mut().find(|s| is(s, "FILE")) {
                    file.substructures.insert(0, form);
                }
            }
        }
        let xref = cx.fresh.fresh("M");
        let mut record = Structure {
            xref: Some(Xref::new(&*xref)),
            ..Structure::new("OBJE")
        };
        let (kept, moved): (Vec<_>, Vec<_>) = std::mem::take(&mut c.substructures)
            .into_iter()
            .partition(|s| is(s, "TITL"));
        record.substructures = moved;
        c.substructures = kept;
        c.payload = Payload::Pointer(Xref::new(&*xref));
        // The record's own structures, converted as a record's.
        self.children(&mut record, "", cx);
        cx.out.records.push(record);
        changed!(cx.out);
    }

    /// A 5.5.1 source citation that describes its source in words: a new
    /// source record holds the description (as a `NOTE`), the citation
    /// points to it, and its texts (`TEXT`) go under `DATA`.
    fn text_source(c: &mut Structure, cx: &mut Cx<'_>) {
        let Payload::Text(description) = std::mem::take(&mut c.payload) else {
            return;
        };
        let xref = cx.fresh.fresh("S");
        let mut record = Structure {
            xref: Some(Xref::new(&*xref)),
            ..Structure::new("SOUR")
        };
        record.substructures.push(text_node("NOTE", &description));
        c.payload = Payload::Pointer(Xref::new(&*xref));
        if let Some(first) = c.substructures.iter().position(|s| is(s, "TEXT")) {
            let mut texts = Vec::new();
            let mut i = first;
            while i < c.substructures.len() {
                if c.substructures.get(i).is_some_and(|s| is(s, "TEXT")) {
                    texts.push(c.substructures.remove(i));
                } else {
                    i += 1;
                }
            }
            if let Some(data) = c.substructures.iter_mut().find(|s| is(s, "DATA")) {
                data.substructures.extend(texts);
            } else {
                let mut data = Structure::new("DATA");
                data.substructures = texts;
                c.substructures.insert(first, data);
            }
        }
        cx.out.records.push(record);
        changed!(cx.out);
    }

    /// The 7.x to 5.5.1 rules for structure `c` under `parent`.
    fn down(&self, c: &mut Structure, parent: &str, cx: &mut Cx<'_>) {
        // An enumeration value 7.x says `OTHER` with a phrase: the phrase,
        // which 5.5.1 writes as the value (a role in an event in
        // parentheses), or which the conformance repair keeps as an
        // extension where the value is not one of a closed set.
        if (c.text() == Some("OTHER") && !is(c, "ROLE") || is(c, "ROLE") && parent == "EVEN")
            && let Some(i) = c
                .substructures
                .iter()
                .position(|p| self.is_phrase(p.tag.as_str()))
            && c.text() == Some("OTHER")
        {
            let phrase = c.substructures.remove(i);
            let word = phrase.text().unwrap_or("").to_string();
            let word = if is(c, "ROLE") {
                format!("({word})")
            } else {
                word
            };
            set_text(c, &word);
            changed!(cx.out);
        }
        // 7.x says an event took place with `Y`; 5.5.1 does so for these
        // events only, and by the event structure itself for the others.
        if matches!(parent, "INDI" | "FAM")
            && c.text() == Some("Y")
            && Y_EVENTS.contains(&c.tag.as_str())
            && !matches!(c.tag.as_str(), "BIRT" | "CHR" | "DEAT" | "MARR")
        {
            c.payload = Payload::None;
            changed!(cx.out);
        }
        match c.tag.as_str() {
            "SNOTE" if matches!(c.payload, Payload::Pointer(_)) => {
                retag(c, "NOTE");
                changed!(cx.out);
            }
            "EXID" => {
                if Self::identifier(c) {
                    changed!(cx.out);
                }
            }
            "ASSO" | "_ASSO" => {
                if self.role(c) {
                    changed!(cx.out);
                }
            }
            "TRAN" if matches!(parent, "NAME" | "PLAC") => {
                if Self::variation(c) {
                    changed!(cx.out);
                }
            }
            "LANG" | "_LANG" => {
                let phrase = c
                    .substructures
                    .iter()
                    .position(|s| self.is_phrase(s.tag.as_str()));
                match (c.text(), phrase) {
                    (Some("und"), Some(i)) => {
                        let p = c.substructures.remove(i);
                        set_text(c, p.text().unwrap_or(""));
                        changed!(cx.out);
                    }
                    (Some(t), _) => {
                        if let Some(name) = text::language_name(t) {
                            set_text(c, name);
                            changed!(cx.out);
                        }
                    }
                    _ => {}
                }
            }
            "FILE" if parent == "OBJE" => {
                if let Some(path) = c.text().and_then(text::file_path) {
                    set_text(c, &path);
                    changed!(cx.out);
                }
            }
            "FORM" if parent == "FILE" => {
                if let Some(format) = c.text().and_then(text::file_format) {
                    set_text(c, &format);
                    changed!(cx.out);
                }
                // A record's file names its medium `TYPE` in 5.5.1.
                if cx.record == "OBJE" {
                    for m in c.substructures.iter_mut().filter(|m| is(m, "MEDI")) {
                        retag(m, "TYPE");
                        changed!(cx.out);
                    }
                }
            }
            _ => {}
        }
    }

    /// A 7.x `EXID` that stands for a 5.5.1 identifier, as that identifier
    /// (the inverse of [`Converter::exid`]). Whether it was.
    fn identifier(c: &mut Structure) -> bool {
        let Some(ty) = child_text(c, "TYPE").map(str::trim) else {
            return false;
        };
        let value = c.text().unwrap_or("").to_string();
        let (tag, value) = if ty == FAMILYSEARCH_PERSON {
            ("_FSFTID", value)
        } else if let Some(rest) = ty.strip_prefix(EXID_TERMS) {
            match rest.split_once('#') {
                None if rest == "AFN" => ("AFN", value),
                None if rest == "RIN" => ("RIN", value),
                None if rest == "RFN" => ("RFN", value),
                Some(("RIN", _)) => ("RIN", value),
                Some(("RFN", system)) => ("RFN", format!("{}:{value}", text::unfragment(system))),
                _ => return false,
            }
        } else {
            return false;
        };
        if c.substructures.iter().filter(|s| is(s, "TYPE")).count() != 1 {
            return false;
        }
        retag(c, tag);
        set_text(c, &value);
        c.substructures.retain(|s| !is(s, "TYPE"));
        true
    }

    /// A 7.x association's role as a 5.5.1 relation (`RELA`): its phrase,
    /// or the word of its value. Whether it changed.
    fn role(&self, c: &mut Structure) -> bool {
        if c.substructures.iter().any(|s| is(s, "RELA")) {
            return false;
        }
        let Some(role) = c.substructures.iter_mut().find(|s| is(s, "ROLE")) else {
            return false;
        };
        let phrase = role
            .substructures
            .iter()
            .position(|s| self.is_phrase(s.tag.as_str()));
        let value = role.text().unwrap_or("").trim().to_string();
        let word = match phrase {
            Some(i) => role
                .substructures
                .remove(i)
                .text()
                .unwrap_or("")
                .to_string(),
            None => ROLE_NAMES
                .iter()
                .find(|(r, _)| *r == value)
                .map_or_else(|| value.clone(), |&(_, w)| w.to_string()),
        };
        retag(role, "RELA");
        set_text(role, &word);
        true
    }

    /// A 7.x name or place translation whose language is a 5.5.1 method
    /// as that phonetic or romanized variation (`FONE`, `ROMN`, the
    /// inverse of [`Converter::translation`]). Whether it was.
    fn variation(c: &mut Structure) -> bool {
        let Some(at) = c.substructures.iter().position(|s| is(s, "LANG")) else {
            return false;
        };
        let lang = c
            .substructures
            .get(at)
            .and_then(Structure::text)
            .unwrap_or("")
            .trim()
            .to_string();
        let lower = lang.to_ascii_lowercase();
        let kept = c.substructures.iter().position(|s| is(s, "_TYPE"));
        let (phonetic, method) = if let Some(&(m, p, _)) = TRANSLATION_METHODS
            .iter()
            .find(|(_, _, t)| t.eq_ignore_ascii_case(&lang))
        {
            (p, Some(m.to_string()))
        } else if lower == "und-x-phonetic" {
            (true, None)
        } else if lower == "und-latn" {
            (false, None)
        } else if let Some(m) = lower.strip_prefix("und-x-phonetic-") {
            (true, Some(m.to_string()))
        } else if let Some(m) = lower.strip_prefix("und-latn-x-") {
            (false, Some(m.to_string()))
        } else {
            return false;
        };
        let method = match kept {
            Some(i) => c.substructures.remove(i).text().map(str::to_string),
            None => method,
        };
        retag(c, if phonetic { "FONE" } else { "ROMN" });
        let at = c
            .substructures
            .iter()
            .position(|s| is(s, "LANG"))
            .unwrap_or(at);
        match method {
            Some(m) => {
                if let Some(l) = c.substructures.get_mut(at) {
                    *l = text_node("TYPE", &m);
                }
            }
            None => {
                c.substructures.remove(at);
            }
        }
        true
    }
}

/// What converting one record needs as it goes.
struct Cx<'a> {
    fresh: &'a mut dyn Fresh,
    out: &'a mut Out,
    /// Whether the record is the header.
    head: bool,
    /// The record's tag.
    record: String,
    /// Whether an age of the record says stillborn.
    stillborn: bool,
}

/// Whether a structure tagged `tag` under `parent` holds a value or the
/// words of a value (a date's `PHRASE`, which keeps the 5.5.1 wording,
/// escapes included, as the date was converted from it): text whose
/// escapes are not converted as text.
fn is_value(tag: &str, parent: &str) -> bool {
    matches!(tag, "DATE" | "SDATE" | "_SDATE" | "AGE" | "TIME")
        || matches!(parent, "DATE" | "SDATE" | "_SDATE" | "AGE" | "TIME")
}

/// Whether an age structure says stillborn: 5.5.1 `STILLBORN`, or `0y`
/// with that phrase.
fn is_stillborn(s: &Structure, c: &Converter) -> bool {
    let value = s.text().unwrap_or("").trim();
    value.eq_ignore_ascii_case("STILLBORN")
        || value.eq_ignore_ascii_case("0y")
            && s.substructures.iter().any(|p| {
                c.is_phrase(p.tag.as_str())
                    && p.text()
                        .is_some_and(|t| t.trim().eq_ignore_ascii_case("stillborn"))
            })
}

/// Whether `s` is a BCP 47 language tag as files write them: a primary
/// subtag of two or three letters (ISO 639) or a private-use or
/// grandfathered prefix (`x`, `i`), then subtags of one to eight letters or
/// digits. A word of more letters (`Hmong`) is a language name.
fn is_language_tag(s: &str) -> bool {
    let mut parts = s.split('-');
    let first = parts.next().unwrap_or("");
    let primary = (2..=3).contains(&first.len()) && first.bytes().all(|b| b.is_ascii_alphabetic())
        || matches!(first, "x" | "X" | "i" | "I");
    let mut rest = parts.peekable();
    primary
        && (rest.peek().is_some() || first.len() > 1)
        && rest.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// The URI of an extension tag a conversion writes or keeps: a well-known
/// extension, or the 5.5.1 submission record kept.
fn known_uri(tag: &str) -> Option<&'static str> {
    match tag {
        SUBMISSION_TAG => return Some(RECORD_SUBN_URI),
        PHRASE_TAG => return Some(PHRASE_URI),
        _ => {}
    }
    KNOWN_EXTENSIONS
        .iter()
        .find(|(t, _)| *t == tag)
        .map(|&(_, uri)| uri)
}

/// The events of individuals and families whose 7.x payload says that they
/// took place (`[Y|<NULL>]`).
const Y_EVENTS: &[&str] = &[
    "ADOP", "ANUL", "BAPM", "BARM", "BASM", "BIRT", "BLES", "BURI", "CENS", "CHR", "CHRA", "CONF",
    "CREM", "DEAT", "DIV", "DIVF", "EMIG", "ENGA", "FCOM", "GRAD", "IMMI", "MARB", "MARC", "MARL",
    "MARR", "MARS", "NATU", "ORDN", "PROB", "RETI", "WILL",
];

/// Whether `text` holds `@#`, the start of a 5.5.1 escape.
/// The 5.5.1 tags 7.x says with another tag whatever their place, payload
/// and substructures (the gedcom7code converter's list): converting up
/// renames them, and nothing else of them but their texts' escapes
/// (`@#…@`).
pub(crate) const RENAMED_UP: &[(&str, &str)] =
    &[("_UID", "UID"), ("EMAI", "EMAIL"), ("_EMAIL", "EMAIL")];

fn has_escape(text: &str) -> bool {
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        rest = rest.get(at + 1..).unwrap_or("");
        if rest.starts_with('#') {
            return true;
        }
    }
    false
}

/// [`Converter::may_apply`]: no payload.
pub(crate) const NONE: u8 = 1;
/// [`Converter::may_apply`]: a text.
pub(crate) const TEXT: u8 = 2;
/// [`Converter::may_apply`]: a pointer.
pub(crate) const POINTER: u8 = 4;

/// The tag of a phrase where 7.x permits none (a language's name), which
/// `HEAD.SCHMA` declares as a `PHRASE`.
const PHRASE_TAG: &str = "_PHRASE";

/// The tag of a 5.5.1 submission record kept in 7.x
/// ([`ConversionOptions::keep_obsolete`]); not `_SUBN`, which the header's
/// pointer to it takes, as `HEAD.SCHMA` gives each tag one meaning.
const SUBMISSION_TAG: &str = "_SUBMISSION";

/// Sets the single leaf substructure `tag` of `s` (found by `matches`) to
/// `value`, adding or removing it.
fn set_leaf(s: &mut Structure, tag: &str, value: Option<&str>, matches: impl Fn(&str) -> bool) {
    let at = s.substructures.iter().position(|c| matches(c.tag.as_str()));
    match (at, value) {
        (Some(i), Some(v)) => {
            if let Some(c) = s.substructures.get_mut(i) {
                set_text(c, v);
            }
        }
        (Some(i), None) => {
            s.substructures.remove(i);
        }
        (None, Some(v)) => s.substructures.push(text_node(tag, v)),
        (None, None) => {}
    }
}

/// Adds `HEAD.SCHMA` declarations, those of tags it declares already
/// excepted.
fn declare_tags(h: &mut Structure, tags: &[(String, &str)]) {
    let at = if let Some(i) = h.substructures.iter().position(|s| is(s, "SCHMA")) {
        i
    } else {
        h.substructures.push(Structure::new("SCHMA"));
        h.substructures.len() - 1
    };
    let Some(schma) = h.substructures.get_mut(at) else {
        return;
    };
    for (tag, uri) in tags {
        let declared = schma.substructures.iter().any(|t| {
            is(t, "TAG") && t.text().and_then(|v| v.split(' ').next()) == Some(tag.as_str())
        });
        if !declared {
            schma
                .substructures
                .push(text_node("TAG", &format!("{tag} {uri}")));
        }
    }
}
