//! Import interpretation: a producer's way of saying something rewritten as
//! the standard says it, never losing what was read.
//!
//! Rules are keyed by (profile, context, tag), never by the tag alone: the
//! same tag means different things to different producers (`_STAT`,
//! `_PRIM`, `_DATE`, `_TYPE`, `_ORIG`, `_SHAR`, `_FIL`), so these apply only
//! for the producer the file was detected as, never from a hint.
//!
//! A rule's [`Fidelity`] says what it may do:
//!
//! - **lossless** rules replace what they read by the standard structure
//!   (`_FUN` becomes `EVEN` + `TYPE Funeral`);
//! - **partial** rules add the standard structure and keep what they read,
//!   in place (it stays in the dataset's `extra`);
//! - **inferred** rules, whose meaning the research could not confirm,
//!   apply only at [`Interpretation::BestEffort`], and keep what they read
//!   too.
//!
//! Interpretation targets the version family of the file: in 5.5.1, a
//! meaning only 7.x can say (`UID`, `SDATE`, `CROP`, `NO`, an association
//! under an event, …) is left as it is, and the writer interprets it when
//! it converts the dataset to 7.x. Applying interpretation twice changes
//! nothing more: an added structure is never added again where an equal one
//! is.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::hash::BuildHasherDefault;

use std::borrow::Cow;

use crate::GedcomVersion;
use crate::model::driver::NodeRef;
use crate::tree::{
    Escaping, PayloadRef, STANDARD_TAGS, Structure, Tag, TagHasher, Xref, standard_index,
};
use crate::value::DateValue;

use super::detect::{Detection, Gate};
use super::draft::{Draft, DraftTag, Value};
use super::util::{
    child, child_mut, child_text, fresh_xref, given_part, has, insert_new, is, is_yes, long_form,
    name_value, payload_str, pointer_node, push_new, retag, set_text, suffix_part, surname_part,
    text_node,
};
use super::{EXTENSION_DOCS, Profile};

/// How far reading interprets a producer's conventions.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Interpretation {
    /// Not at all: the dataset holds what the file says, as it says it.
    None,
    /// Only what keeps every datum read (the default): lossless rules
    /// replace, partial rules add and keep.
    #[default]
    Lossless,
    /// Also the inferred and opt-in rules (such as `GeneWeb`'s `unmarried`
    /// as `NO MARR`), which add and keep too.
    BestEffort,
}

/// What a rule may do with what it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fidelity {
    /// Replace: the standard structure says all of it.
    Lossless,
    /// Add the standard structure, keep what was read.
    Partial,
    /// As partial, only at [`Interpretation::BestEffort`].
    Inferred,
}

/// One rule of an import profile.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rule {
    /// Where the tag is: the tags from the record down to its parent, `.`
    /// separated; `*` stands for one tag, `**` for any number, `A|B` for
    /// either; empty for a record.
    pub(crate) ctx: &'static str,
    /// The tag read.
    pub(crate) tag: &'static str,
    pub(crate) op: Op,
    pub(crate) fidelity: Fidelity,
    /// The rows of the compatibility catalogue it implements.
    pub(crate) rows: &'static [&'static str],
    pub(crate) gate: Gate,
}

/// A standard name type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameKind {
    Aka,
    Birth,
    Married,
    /// `OTHER` with the phrase (5.5.1: the phrase as the type).
    Other(&'static str),
}

impl NameKind {
    fn type_node<'s>(self, v7: bool) -> Draft<'s> {
        let (v7_word, v551) = match self {
            NameKind::Aka => ("AKA", "aka"),
            NameKind::Birth => ("BIRTH", "birth"),
            NameKind::Married => ("MARRIED", "married"),
            NameKind::Other(phrase) => ("OTHER", phrase),
        };
        if v7 {
            let mut t = text_node("TYPE", v7_word);
            if let NameKind::Other(phrase) = self {
                t.substructures.push(text_node("PHRASE", phrase));
            }
            t
        } else {
            text_node("TYPE", v551)
        }
    }
}

/// Where an `EVEN`/`FACT` gets its `TYPE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Label {
    /// None added.
    None,
    /// This text.
    Text(&'static str),
    /// The payload read (which is then not the payload).
    Payload,
    /// The label the header's `_SCHEMA` gives the tag (Family Tree Maker
    /// for Windows).
    Schema,
}

/// What a rule does.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Op {
    /// A documented extension, kept as it is (and declared in a 7.x
    /// header's `SCHMA`).
    Known,
    /// Another tag, the payload and substructures kept.
    Rename(&'static str),
    /// Another tag, the payload replaced: `_NMAR` as `NMR 0`.
    Fixed(&'static str, &'static str),
    /// An event or attribute: `tag` + `TYPE`; `y` sets the payload `Y`
    /// when the structure read has none.
    Event {
        tag: &'static str,
        label: Label,
        y: bool,
    },
    /// A name under `NAME` (or `INDI`) as a `NAME` of its own.
    Name(NameKind, NameValue),
    /// A name as a 7.x translation of its parent (`NAME`, `PLAC`) in a
    /// language.
    Translation(&'static str),
    /// An identifier as a 7.x `EXID` with this `TYPE`, without `strip` at
    /// its start; in 5.5.1 a `FamilySearch` person identifier is written
    /// `_FSFTID`, which the converter knows.
    Exid {
        uri: &'static str,
        strip: &'static str,
    },
    /// A 7.x tag for a 5.5.1 extension: `UID` for `_UID`, `SDATE` for
    /// `_SDATE` (a date). Nothing in 5.5.1.
    V7(&'static str),
    /// An event's witness or godparent named by text or pointed to: 7.x
    /// `ASSO` + `ROLE` (+ `PHRASE`).
    Assoc(&'static str),
    /// An association whose `ROLE` or `RELA` words its role: 7.x `ASSO` +
    /// `ROLE` from the word (`OTHER` + `PHRASE` when no role says it).
    Shared,
    /// A 7.x `ROLE OTHER` + `PHRASE word` (or a 5.5.1 `RELA word`) whose
    /// word the producer uses for a standard role.
    RoleWords(&'static [(&'static str, &'static str, &'static str)]),
    /// `_FREL`/`_MREL` under `FAM.CHIL` or `INDI.FAMC`: `PEDI` when both
    /// say the same.
    Pedigree,
    /// A pedigree said by a value (`_PEDI Adopted (father)`, Heredis
    /// `_FIL`): `PEDI` from the first word that names one, the value as
    /// `PHRASE` (7.x).
    PedigreeWord,
    /// A preferred-item flag (`_PRIM Y`): its structure goes first among
    /// its like (7.x drops the flag, the order saying it).
    Prefer,
    /// A pointer to the preferred multimedia record (`_PHOTO @M1@`), or the
    /// order of several (`_WT_OBJE_SORT`): the `OBJE` links in that order.
    PreferPointer,
    /// A preferred family (`_PREF Y` under `FAM.HUSB`, `WIFE`, `CHIL`): the
    /// family goes first among the person's.
    PreferFamily,
    /// A privacy flag: `RESN PRIVACY` on its structure.
    Privacy,
    /// A last-change stamp (`_UPD 12 JAN 2015 10:20:30 GMT -0500`):
    /// `CHAN` + `DATE` + `TIME`.
    Change,
    /// A creation stamp: 7.x `CREA` + `DATE`.
    Creation,
    /// A crop rectangle: 7.x `CROP`.
    Crop(CropForm),
    /// A flag that an event did not happen: 7.x `NO` with the tag.
    NoEvent(&'static str),
    /// A label and a text as a `NOTE` under the same structure.
    ToNote(&'static str),
    /// Payload values the producer spells its own way (whole, any case).
    Values(&'static [(&'static str, &'static str)]),
    /// The payload of the parent, which the producer wrote in this
    /// substructure (Heredis `NATI` + `CAUS`).
    ToParentPayload,
    /// A time under an event: 7.x `DATE.TIME`.
    TimeIntoDate,
    /// The structure moved into a substructure of its parent (`DATA`).
    Into(&'static str),
    /// The structure moved up beside its parent.
    Up,
    /// A residence written as an individual's address: `RESI` + `ADDR`.
    Residence,
    /// An occupation written in `PLAC`: the payload.
    PlaceAsValue,
    /// A second `TYPE` under `EVEN` is its payload (Heredis).
    SecondType,
    /// `EVEN` + `TYPE <word>` naming a standard event: that event.
    EvenType(&'static [(&'static str, &'static str)]),
    /// Identical repeated substructures: the first kept.
    Dedupe,
    /// `GeneWeb`'s `INDI.ASSO @F@` (a marriage witness) as 7.x `MARR.ASSO`
    /// `ROLE WITN` in the family.
    FamilyWitness,
    /// An adoption (`ADOP` + `FAMC @F@`) also linked as `FAMC @F@ PEDI
    /// ADOPTED` and `CHIL` of the family.
    Adoption,
    /// Negative years as `B.C.` (5.5.1) or `BCE` (7.x).
    NegativeYear,
    /// Date words of a dialect: `ENV`, `CIR`, `ABT.`, `SEPT`, French
    /// Republican month names, `EITHER … OR …`, `Stillborn`.
    DateWords,
    /// Age words of a dialect: `65 y 10 m`, `65-10-25`, `11 mois`.
    AgeWords,
    /// `GeneWeb` surname aliases in `SURN a,b`: a `NAME` of type AKA each.
    SurnameAliases,
    /// `GeneWeb`'s public name in `GIVN`: a `NAME` of type AKA.
    PublicName,
    /// The second and later `NAME`s without a `TYPE`: type AKA.
    AliasNames,
    /// `GeneWeb`'s `EVEN TYPE unmarried`: 7.x `NO MARR`.
    Unmarried,
    /// A repository named in words under a source: a `REPO` record.
    RepositoryRecord,
    /// A record written `0 INDI @I1@`: its identifier.
    RecordXref,
    /// A tag written in its long form (`BIRTH`): the tag.
    LongForm,
    /// Family Tree Maker for Windows' `_SCHEMA` facts: `EVEN` + `TYPE`.
    SchemaFact,
    /// An event at a place given as the payload: `EVEN` + `TYPE` + `PLAC`.
    PlaceEvent(&'static str),
    /// A URL that names a record of a site: a 7.x `EXID` with the site's
    /// `TYPE` (the URL without its prefix).
    ExidUrl {
        prefix: &'static str,
        uri: &'static str,
    },
    /// A structure holding its text in a substructure (`WikiTree` `_BIO` +
    /// `TEXT`): a `NOTE` with that text.
    ChildNote(&'static str),
    /// Text written as HTML (or HTML escaped once more): 7.x `MIME
    /// text/html`, the escaping undone.
    Html,
    /// The structure moved into the first sibling with this tag, if any.
    IntoSibling(&'static str),
    /// The structure moved into a substructure of its parent with this tag
    /// (made when there is none), retagged.
    IntoAs(&'static str, &'static str),
    /// Tags in mixed case (`Data`) in upper case, when that is a standard
    /// tag.
    CaseFold,
    /// A substructure this record should have: added when missing
    /// (`GeneWeb` writes no `SEX` for an unknown sex).
    Default(&'static str, &'static str),
    /// A submitter written inside the header: a `SUBM` record.
    SubmitterRecord,
}

/// How the value of a name read becomes a `NAME` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameValue {
    /// As it is (`Johnny /Doe/`, or a name without surname).
    AsIs,
    /// A bare value is a surname: `/value/` (a value with slashes is a
    /// whole name).
    Surname,
    /// Several whole names (each with its surname slashes) separated by
    /// commas or semicolons (PAF `_AKA`), else one name as it is.
    List,
}

/// The shape of a crop rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CropForm {
    /// Ancestry: `_CROP` + `_LEFT`, `_TOP`, `_WDTH`, `_HGHT`.
    Ancestry,
    /// Family Historian: `_AREA {left,top,right,bottom}`.
    Area,
}

/// What an interpretation did, by rule.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    /// The structure read was replaced by its standard form.
    Replaced,
    /// The standard form was added; the structure read is kept.
    Added,
    /// Structures were put in the order the producer meant.
    Reordered,
    /// A documented extension, kept as it is.
    Kept,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Action::Replaced => "replaced",
            Action::Added => "added",
            Action::Reordered => "reordered",
            Action::Kept => "kept",
        })
    }
}

/// What one rule did, over the whole input.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    /// The profile of the rule.
    pub profile: &'static str,
    /// The catalogue rows it implements.
    pub rows: &'static [&'static str],
    /// Where the tag was: the rule's context.
    pub context: &'static str,
    /// The tag read.
    pub tag: Box<str>,
    /// What was done.
    pub action: Action,
    /// How many times.
    pub count: usize,
    /// The first source line it applied at (0 when unknown).
    pub first_line: u32,
    /// The first record it applied in.
    pub first_record: Option<Xref>,
}

impl fmt::Display for Mapping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}.{} {} ×{} ({})",
            self.profile,
            self.context,
            self.tag,
            self.action,
            self.count,
            self.rows.join(", ")
        )?;
        if self.first_line > 0 {
            write!(f, ", first at line {}", self.first_line)?;
        }
        Ok(())
    }
}

/// What interpretation did: a query, not a warnings channel. Empty when
/// nothing was interpreted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InterpretReport {
    mappings: Vec<Mapping>,
    /// Whether anything was changed (not only kept).
    changed: bool,
}

impl InterpretReport {
    /// Every rule that applied, in the order each first did.
    #[must_use]
    pub fn mappings(&self) -> &[Mapping] {
        &self.mappings
    }

    /// Whether nothing was interpreted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }

    /// Whether interpretation changed anything (beyond keeping documented
    /// extensions).
    #[must_use]
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// How many structures were interpreted.
    #[must_use]
    pub fn count(&self) -> usize {
        self.mappings.iter().map(|m| m.count).sum()
    }

    /// The rules that did `action`.
    pub fn with_action(&self, action: Action) -> impl Iterator<Item = &Mapping> {
        self.mappings.iter().filter(move |m| m.action == action)
    }

    /// The rules that implement catalogue row `row`.
    pub fn for_row<'a>(&'a self, row: &'a str) -> impl Iterator<Item = &'a Mapping> {
        self.mappings.iter().filter(move |m| m.rows.contains(&row))
    }

    /// The catalogue rows that applied, once each, sorted.
    #[must_use]
    pub fn rows(&self) -> Vec<&'static str> {
        let rows: BTreeSet<&'static str> = self
            .mappings
            .iter()
            .flat_map(|m| m.rows.iter().copied())
            .collect();
        rows.into_iter().collect()
    }

    fn note(
        &mut self,
        rule: &Applied,
        tag: &str,
        action: Action,
        line: u32,
        record: Option<&Xref>,
    ) {
        self.changed |= action != Action::Kept;
        if let Some(m) = self
            .mappings
            .iter_mut()
            .find(|m| std::ptr::eq(m.rows, rule.rule.rows) && m.action == action && &*m.tag == tag)
        {
            m.count += 1;
            return;
        }
        self.mappings.push(Mapping {
            profile: rule.profile.id,
            rows: rule.rule.rows,
            context: rule.rule.ctx,
            tag: tag.into(),
            action,
            count: 1,
            first_line: line,
            first_record: record.cloned(),
        });
    }
}

/// The pointer a link to a record that interpretation adds holds until
/// the record is placed and named.
pub(crate) const PLACEHOLDER: &str = "@\u{0}@";

/// Tags whose meaning depends on the producer: never interpreted from a
/// hint (REPORT §3, §6.2).
const AMBIGUOUS: &[&str] = &[
    "_STAT", "_PRIM", "_DATE", "_TYPE", "_ORIG", "_SHAR", "_FIL", "_PREF", "_PRIMARY",
];

/// A rule with the profile it comes from.
#[derive(Clone, Copy)]
struct Applied {
    profile: &'static Profile,
    rule: &'static Rule,
    /// Its context, parsed ([`RuleSet::contexts`]).
    ctx: u16,
}

/// The rules that may apply to a file, by tag.
pub(crate) struct RuleSet {
    by_tag: HashMap<&'static str, Vec<Applied>, BuildHasherDefault<TagHasher>>,
    /// The rules of [`RuleSet::by_tag`] on standard tags, by the tag's
    /// index among them: found without hashing the tag.
    by_standard: Box<[Vec<Applied>]>,
    /// Rules on any tag (long forms, `_SCHEMA` facts).
    any: Vec<Applied>,
    /// Those of [`RuleSet::any`] that may apply to a standard tag (not the
    /// long forms, nor the tags in another case).
    any_standard: Vec<Applied>,
    /// The rules' contexts, parsed.
    contexts: Vec<Context>,
}

/// A rule's context ([`Rule::ctx`]), parsed: alternatives of segments.
#[derive(Debug)]
struct Context(Box<[Box<[Segment]>]>);

/// A segment of a [`Context`].
#[derive(Debug)]
enum Segment {
    /// `*`: one tag.
    One,
    /// `**`: any number of tags.
    Any,
    /// `A|B`: one of these tags.
    Tags(Box<[&'static str]>),
}

impl Context {
    fn parse(ctx: &'static str) -> Self {
        Self(
            ctx.split(',')
                .map(|alt| {
                    if alt.is_empty() {
                        return Box::default();
                    }
                    alt.split('.')
                        .map(|seg| match seg {
                            "*" => Segment::One,
                            "**" => Segment::Any,
                            tags => Segment::Tags(tags.split('|').collect()),
                        })
                        .collect()
                })
                .collect(),
        )
    }

    /// Whether the tags `path` (from the record down) are in the context.
    #[cfg(test)]
    fn matches<T: AsRef<str>>(&self, path: &[T]) -> bool {
        Self::matches_in(&self.0, path)
    }

    /// Whether the tags `path` are in the context of alternatives `alts`.
    fn matches_in<T: AsRef<str>>(alts: &[Box<[Segment]>], path: &[T]) -> bool {
        fn go<T: AsRef<str>>(segments: &[Segment], path: &[T]) -> bool {
            match segments.split_first() {
                None => path.is_empty(),
                Some((Segment::Any, rest)) => {
                    (0..=path.len()).any(|k| go(rest, path.get(k..).unwrap_or(&[])))
                }
                Some((seg, rest)) => match path.split_first() {
                    None => false,
                    Some((tag, tail)) => {
                        let fits = match seg {
                            Segment::Tags(tags) => tags.contains(&tag.as_ref()),
                            _ => true,
                        };
                        fits && go(rest, tail)
                    }
                },
            }
        }
        alts.iter().any(|alt| go(alt, path))
    }
}

impl RuleSet {
    /// The rules of the producer's profile chain, then the unambiguous rules
    /// of the hints, for `level` and the producer's version.
    pub(crate) fn of(detection: &Detection, level: Interpretation) -> Self {
        let mut set = RuleSet {
            by_tag: HashMap::default(),
            by_standard: Box::default(),
            any: Vec::new(),
            any_standard: Vec::new(),
            contexts: Vec::new(),
        };
        if level == Interpretation::None {
            return set;
        }
        let mut add = |profile: &'static Profile, hint: bool| {
            for rule in profile.rules {
                if rule.fidelity == Fidelity::Inferred && level != Interpretation::BestEffort {
                    continue;
                }
                // From a hint, only the extension tags every producer
                // agrees on.
                if hint
                    && (AMBIGUOUS.contains(&rule.tag)
                        || rule.fidelity == Fidelity::Inferred
                        || !rule.tag.starts_with('_'))
                {
                    continue;
                }
                if !rule.gate.open(detection) {
                    continue;
                }
                let ctx = u16::try_from(set.contexts.len()).unwrap_or(u16::MAX);
                set.contexts.push(Context::parse(rule.ctx));
                let applied = Applied { profile, rule, ctx };
                if rule.tag == "*" {
                    set.any.push(applied);
                } else {
                    for tag in rule.tag.split('|') {
                        set.by_tag.entry(tag).or_default().push(applied);
                        // The writer's repair writes an unknown tag `X` as
                        // `_X`: a rule on `_X` reads `X` too, so that reading
                        // what was written interprets nothing more.
                        if let Some(bare) = tag.strip_prefix('_').filter(|b| {
                            !b.is_empty() && !Tag::new(b).is_standard() && !b.starts_with('_')
                        }) {
                            set.by_tag.entry(bare).or_default().push(applied);
                        }
                    }
                }
            }
        };
        for profile in detection.chain() {
            add(profile, false);
        }
        for &hint in detection.hints() {
            add(hint, true);
        }
        let mut by_standard = vec![Vec::new(); STANDARD_TAGS.len()];
        for (tag, rules) in &set.by_tag {
            if let Some(slot) =
                standard_index(tag).and_then(|i| by_standard.get_mut(usize::from(i)))
            {
                slot.clone_from(rules);
            }
        }
        set.by_standard = by_standard.into_boxed_slice();
        set.any_standard = set
            .any
            .iter()
            .filter(|a| !matches!(a.rule.op, Op::LongForm | Op::CaseFold))
            .copied()
            .collect();
        set
    }

    /// Whether no rule applies.
    pub(crate) fn is_empty(&self) -> bool {
        self.by_tag.is_empty() && self.any.is_empty()
    }

    /// Whether a file whose text is `input` and whose tags are `tags` (the
    /// tags that are not standard, once each) gives a rule anything to do:
    /// a rule on such a tag, or a rule on a standard tag that `input` has
    /// with a value the rule may change (a record written `0 INDI @I1@`
    /// only with `untyped`, a record of a standard tag the model could not
    /// type). Documented extensions, which are only kept, count only with
    /// `known` (to report them, or declare them in 7.x); the rules that
    /// only say something in 7.x only with `v7`.
    pub(crate) fn may_apply<'t>(
        &self,
        store: &crate::model::Store,
        tags: impl Iterator<Item = &'t str>,
        untyped: bool,
        known: bool,
        v7: bool,
    ) -> bool {
        let input = store.input();
        let relevant = |a: &Applied| match a.rule.op {
            Op::Known => known,
            Op::RecordXref => untyped,
            op => v7 || !op.v7_only(),
        };
        let tags: Vec<&str> = tags.collect();
        let any = self
            .any
            .iter()
            .filter(|a| relevant(a))
            .any(|a| match a.rule.op {
                Op::LongForm => tags.iter().any(|t| long_form(t).is_some()),
                Op::CaseFold => tags
                    .iter()
                    .any(|t| Tag::new(&t.to_ascii_uppercase()).is_standard()),
                Op::SchemaFact => tags.contains(&"_SCHEMA"),
                _ => true,
            });
        if any
            || tags.iter().any(|t| {
                self.by_tag
                    .get(t)
                    .is_some_and(|rules| rules.iter().any(relevant))
            })
        {
            return true;
        }
        // The rules on standard tags: the lines of their tags.
        self.by_tag.iter().any(|(tag, rules)| {
            Tag::new(tag).is_standard()
                && rules.iter().any(relevant)
                // (A quicker look first: most of these tags are rare.)
                && store.read_tag(tag).unwrap_or_else(|| input.contains(tag))
                && lines_of(input, tag).any(|value| {
                    rules
                        .iter()
                        .any(|a| relevant(a) && a.rule.op.may_change(value, v7))
                })
        })
    }

    /// Whether some rule only says something in 7.x (left for the writer
    /// to apply when it converts a 5.5.1 dataset).
    pub(crate) fn has_v7_only(&self) -> bool {
        self.by_tag.values().flatten().any(|a| a.rule.op.v7_only())
    }

    /// The rules for `tag` under `path`, in order of precedence.
    fn candidates<'a, T: AsRef<str>>(
        &'a self,
        tag: &str,
        path: &'a [T],
    ) -> impl Iterator<Item = Applied> + use<'a, T> {
        let standard = standard_index(tag);
        // A standard tag is no long form, nor in another case.
        let any = if standard.is_some() {
            &self.any_standard
        } else {
            &self.any
        };
        self.rules_of(tag, standard)
            .iter()
            .chain(any)
            .filter(move |a| Context::matches_in(self.context(a), path))
            .copied()
    }

    /// The rules on `tag` (whose index among the standard tags is
    /// `standard`), the rules on any tag aside.
    #[inline]
    fn rules_of(&self, tag: &str, standard: Option<u16>) -> &[Applied] {
        match standard {
            Some(i) => self
                .by_standard
                .get(usize::from(i))
                .map_or(&[], Vec::as_slice),
            None => self.by_tag.get(tag).map_or(&[], Vec::as_slice),
        }
    }

    #[inline]
    fn context(&self, a: &Applied) -> &[Box<[Segment]>] {
        self.contexts.get(usize::from(a.ctx)).map_or(&[], |c| &c.0)
    }

    /// Whether a rule may change a structure tagged `tag` (of index
    /// `standard` among the standard tags) under the tags `path` (from the
    /// record down), with `payload` and an identifier when `has_xref`: what
    /// the record's interpretation would find, found without copying it.
    /// Documented extensions, which are only kept, count only with
    /// `known`.
    #[allow(clippy::too_many_arguments)] // The parts of a node of any form.
    pub(crate) fn may_rewrite<'p, T: AsRef<str>>(
        &self,
        tag: &str,
        standard: Option<u16>,
        path: &[T],
        payload: impl Fn() -> PayloadRef<'p>,
        has_xref: bool,
        v7: bool,
        known: bool,
    ) -> bool {
        let rules = self.rules_of(tag, standard);
        // A standard tag is no long form, nor in another case.
        let any = if standard.is_some() {
            &self.any_standard
        } else {
            &self.any
        };
        if rules.is_empty() && any.is_empty() {
            return false;
        }
        any_of(rules, any, |a| {
            let op = a.rule.op;
            // What only 7.x says, 5.5.1 leaves as it is (but a
            // `FamilySearch` identifier, which is written `_FSFTID`).
            let inert = op.v7_only()
                && !v7
                && !matches!(op, Op::Exid { uri, .. } if uri == super::FAMILYSEARCH_PERSON);
            !inert
                && (known || !matches!(op, Op::Known))
                && Context::matches_in(self.context(a), path)
                && op.may_apply_to_parts(tag, payload(), has_xref, path.is_empty(), v7)
        })
    }

    /// Whether some rule may read a structure tagged `tag` (of index
    /// `standard` among the standard tags), wherever it is: what
    /// [`RuleSet::may_rewrite`] looks at further.
    /// `record`: of a record, else of a substructure.
    pub(crate) fn reads(
        &self,
        tag: &str,
        standard: Option<u16>,
        record: bool,
        v7: bool,
        known: bool,
    ) -> bool {
        let any = if standard.is_some() {
            &self.any_standard
        } else {
            &self.any
        };
        any_of(self.rules_of(tag, standard), any, |a| {
            let op = a.rule.op;
            let inert = op.v7_only()
                && !v7
                && !matches!(op, Op::Exid { uri, .. } if uri == super::FAMILYSEARCH_PERSON);
            let alts = self.context(a);
            let placed = if record {
                Context::matches_in::<&str>(alts, &[])
            } else {
                alts.iter().any(|alt| !alt.is_empty())
            };
            placed && !inert && (known || !matches!(op, Op::Known))
        })
    }

    /// [`RuleSet::may_rewrite`] in 7.x, documented extensions counted, of
    /// a structure under any tags: of a record when `record`, else of a
    /// substructure wherever a rule's context may put it.
    pub(crate) fn may_rewrite_anywhere(
        &self,
        tag: &str,
        standard: Option<u16>,
        payload: PayloadRef<'_>,
        has_xref: bool,
        record: bool,
    ) -> bool {
        let rules = self.rules_of(tag, standard);
        let any = if standard.is_some() {
            &self.any_standard
        } else {
            &self.any
        };
        if rules.is_empty() && any.is_empty() {
            return false;
        }
        any_of(rules, any, |a| {
            let alts = self.context(a);
            let placed = if record {
                Context::matches_in::<&str>(alts, &[])
            } else {
                alts.iter().any(|alt| !alt.is_empty())
            };
            placed
                && a.rule
                    .op
                    .may_apply_to_parts(tag, payload, has_xref, record, true)
        })
    }

    /// Whether a rule may rewrite a structure of the standard tag of index
    /// `standard`, whatever its payload and place.
    pub(crate) fn may_rewrite_standard(&self, standard: u16) -> bool {
        !self.any_standard.is_empty()
            || self
                .by_standard
                .get(usize::from(standard))
                .is_some_and(|rules| !rules.is_empty())
    }

    /// Whether some rule changes records other than the one it reads
    /// (pedigrees, families' order, witnesses, adoptions).
    pub(crate) fn spans_records(&self) -> bool {
        self.by_tag.values().flatten().chain(&self.any).any(|a| {
            matches!(
                a.rule.op,
                Op::Pedigree | Op::PreferFamily | Op::FamilyWitness | Op::Adoption
            )
        })
    }

    /// Whether some rule reads Family Tree Maker's `HEAD._SCHEMA` labels.
    pub(crate) fn reads_schema(&self) -> bool {
        self.by_tag.values().flatten().chain(&self.any).any(|a| {
            matches!(
                a.rule.op,
                Op::SchemaFact
                    | Op::Event {
                        label: Label::Schema,
                        ..
                    }
            )
        })
    }
}

/// Whether `f` holds of one of `rules`, or of `any` (the rules on any
/// tag): two loops, which every look at a structure's rules runs.
#[inline]
fn any_of(rules: &[Applied], any: &[Applied], mut f: impl FnMut(&Applied) -> bool) -> bool {
    rules.iter().any(&mut f) || any.iter().any(f)
}

/// The values (first lines) of the lines of `input` tagged `tag`, as far
/// as a quick look can tell: a search for the tag, rare in most files,
/// beats reading every line.
fn lines_of<'a>(input: &'a str, tag: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    let delimiter = |c: char| c == ' ' || c == '\t';
    let line_end = |c: char| c == '\n' || c == '\r';
    input.match_indices(tag).filter_map(move |(at, _)| {
        let after = input.get(at + tag.len()..)?;
        if !after.is_empty() && !after.starts_with(delimiter) && !after.starts_with(line_end) {
            return None;
        }
        // What precedes the tag on its line: a level, then an identifier.
        let before = input.get(..at)?;
        if !before.ends_with(delimiter) {
            return None;
        }
        let line = before
            .rfind(line_end)
            .map_or(before, |k| before.get(k + 1..).unwrap_or(""));
        let mut words = line.split(delimiter).filter(|w| !w.is_empty());
        let level = words.next()?.trim_start_matches('\u{feff}');
        if level.is_empty() || !level.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        match (words.next(), words.next()) {
            (None, _) => {}
            (Some(xref), None) if xref.starts_with('@') => {}
            _ => return None,
        }
        let value = after.split(line_end).next().unwrap_or("");
        Some(value.strip_prefix(delimiter).unwrap_or(value))
    })
}

impl Op {
    /// Whether the op may change `node` (under `path`): false only when it
    /// surely leaves it as it is, as the op itself would find.
    fn may_apply_to(self, node: &Draft<'_>, path: &[DraftTag<'_>], v7: bool) -> bool {
        self.may_apply_to_parts(
            node.tag.as_str(),
            node.payload.borrowed(),
            node.xref.is_some(),
            path.is_empty(),
            v7,
        )
    }

    /// [`Op::may_apply_to`] of a structure tagged `tag` with `payload`
    /// and an identifier when `has_xref`, a record when `record`.
    #[inline]
    fn may_apply_to_parts(
        self,
        tag: &str,
        payload: PayloadRef<'_>,
        has_xref: bool,
        record: bool,
        v7: bool,
    ) -> bool {
        match self {
            Op::LongForm => long_form(tag).is_some(),
            Op::CaseFold => {
                tag.bytes().any(|b| b.is_ascii_lowercase())
                    && Tag::new(&tag.to_ascii_uppercase()).is_standard()
            }
            Op::RecordXref => matches!(payload, PayloadRef::Pointer(_)) && !has_xref && record,
            op => op.may_change(payload.as_str().unwrap_or_default(), v7),
        }
    }

    /// Whether the op may change a structure whose first line has
    /// `value`: false only when it surely leaves it as it is.
    fn may_change(self, value: &str, v7: bool) -> bool {
        match self {
            Op::Values(map) => map
                .iter()
                .any(|&(from, to)| from.eq_ignore_ascii_case(value.trim()) && to != value.trim()),
            Op::NegativeYear => negative_years(value, v7).is_some(),
            Op::AgeWords => age_words(value).is_some(),
            // (A date of the commonest shapes is in the grammar's words, as
            // `plain_date` finds it, with no word to look up.)
            Op::DateWords => {
                !crate::spec::payload::is_simple_date(value, false, false)
                    && (value.trim().eq_ignore_ascii_case("stillborn")
                        || date_dialect(value, v7).is_some())
            }
            // Markup, or markup escaped once more.
            Op::Html => value.contains(['<', '&']),
            _ => true,
        }
    }

    /// Whether what [`Op::may_change`] answers depends on the payload
    /// alone, the op's kind aside (no data of its own).
    fn by_payload(self) -> bool {
        matches!(
            self,
            Op::DateWords | Op::AgeWords | Op::NegativeYear | Op::Html
        )
    }

    /// Whether [`apply`] leaves a structure as it is, without a trace, in
    /// 5.5.1, before it looks at anything.
    fn skips_in_551(self) -> bool {
        matches!(
            self,
            Op::V7(_)
                | Op::Assoc(_)
                | Op::Shared
                | Op::Creation
                | Op::NoEvent(_)
                | Op::TimeIntoDate
                | Op::FamilyWitness
                | Op::Unmarried
                | Op::Translation(_)
        )
    }

    /// Whether the op does nothing in 5.5.1.
    fn v7_only(self) -> bool {
        matches!(
            self,
            Op::V7(_)
                | Op::Assoc(_)
                | Op::Shared
                | Op::Creation
                | Op::Crop(_)
                | Op::NoEvent(_)
                | Op::TimeIntoDate
                | Op::FamilyWitness
                | Op::Unmarried
                | Op::Exid { .. }
                | Op::RoleWords(_)
                | Op::Translation(_)
                | Op::ExidUrl { .. }
                | Op::Html
        )
    }
}

/// Interprets records (header, records and trailer, as
/// [`Tree::to_structures`](crate::tree::Tree::to_structures) gives them) in
/// `version` with the rules `detection` and `level` select, and reports
/// what was done (see the [`profile` documentation](crate::profile)).
pub fn interpret(
    records: &mut Vec<Structure>,
    version: GedcomVersion,
    detection: &Detection,
    level: Interpretation,
) -> InterpretReport {
    let rules = RuleSet::of(detection, level);
    let mut drafts: Vec<Draft<'_>> = records.iter().map(Draft::of_structure).collect();
    let report = interpret_with(&mut drafts, version, &rules);
    let interpreted: Vec<Structure> = drafts.iter().map(Draft::to_structure).collect();
    drop(drafts);
    *records = interpreted;
    report
}

pub(crate) fn interpret_with(
    records: &mut Vec<Draft<'_>>,
    version: GedcomVersion,
    rules: &RuleSet,
) -> InterpretReport {
    let mut cx = Cx {
        v7: version.is_v7(),
        rules,
        report: InterpretReport::default(),
        deferred: Vec::new(),
        record: None,
        declare: BTreeSet::new(),
        schema: schema_labels(records),
        peers: Vec::new(),
        settled: false,
        defined: None,
        promoted: false,
    };
    if rules.is_empty() {
        return cx.report;
    }
    let mut root = Draft {
        substructures: std::mem::take(records),
        ..Draft::default()
    };
    let mut path = Vec::new();
    walk(&mut root, &mut path, &mut cx, true);
    *records = root.substructures;
    let deferred = std::mem::take(&mut cx.deferred);
    apply_deferred(records, deferred, &mut cx.report, cx.v7, None);
    if cx.v7 && declare(records, &cx.declare) {
        cx.report.changed = true;
    }
    cx.report
}

/// Interpretation of a file one record at a time, as it is read: what
/// goes from one record to the next (the report, the changes that span
/// records, the extensions to declare). Each record is interpreted alone
/// ([`Interpreter::record`]), as [`interpret_with`] interprets it among
/// the file's records, but that a record is a structure of its own: no
/// rule compares it with the others. The changes that span records wait
/// for [`Interpreter::finish`].
pub(crate) struct Interpreter<'r> {
    rules: &'r RuleSet,
    v7: bool,
    report: InterpretReport,
    deferred: Vec<(Applied, Deferred)>,
    declare: BTreeSet<(Box<str>, String)>,
    schema: Vec<(String, String, String)>,
    /// Room for [`Interpreter::record_in_arena`], kept from record to
    /// record.
    scratch: InArenaScratch,
}

/// What [`Interpreter::record_in_arena`] keeps its structures in, record
/// after record: the structures a rule may change (each with its depth,
/// where its substructures end, its place among the record's
/// substructures, its index in the record, its rules in `found`).
#[derive(Default)]
struct InArenaScratch {
    waiting: Vec<(u8, u32, usize, usize, std::ops::Range<usize>)>,
    found: Vec<Applied>,
}

/// The deepest structure whose tags above [`Interpreter::record_in_arena`]
/// keeps: one deeper is read as a draft.
const IN_ARENA_DEPTH: usize = 32;

/// What interpreting one record did ([`Interpreter::record`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rewrite {
    /// The record changed (it and what it lifted beside it are to be read
    /// anew).
    pub(crate) changed: bool,
    /// It links to a record [`Interpreter::finish`] adds and names.
    pub(crate) placeholder: bool,
    /// It goes first among the records of its tag.
    pub(crate) promoted: bool,
}

impl<'r> Interpreter<'r> {
    /// Interpretation with `rules`, of a file of `version`.
    pub(crate) fn new(rules: &'r RuleSet, version: GedcomVersion) -> Self {
        Self {
            rules,
            v7: version.is_v7(),
            report: InterpretReport::default(),
            deferred: Vec::new(),
            declare: BTreeSet::new(),
            schema: Vec::new(),
            scratch: InArenaScratch::default(),
        }
    }

    /// The rules.
    pub(crate) fn rules(&self) -> &'r RuleSet {
        self.rules
    }

    /// Whether some change spans records, or some extension is to be
    /// declared: what [`Interpreter::finish`] does.
    pub(crate) fn has_pending(&self) -> bool {
        !self.deferred.is_empty() || self.declares()
    }

    /// The identifiers of the records the changes that span records concern
    /// (with repeats).
    pub(crate) fn targets(&self) -> impl Iterator<Item = &str> {
        self.deferred.iter().flat_map(|(_, d)| {
            let (a, b): (Option<&Xref>, Option<&Xref>) = match d {
                Deferred::Pedigree { child, .. } => (Some(child), None),
                Deferred::FamilyFirst { person, .. } => (Some(person), None),
                Deferred::Witness { family, .. } => (Some(family), None),
                Deferred::Adopted { child, family } => (Some(child), Some(family)),
                Deferred::Record(_) => (None, None),
            };
            a.into_iter().chain(b).map(Xref::as_str)
        })
    }

    /// Interprets the record `records` holds, alone: it may become several
    /// (what it lifted beside it). `defined`: whether a record before it
    /// has an identifier.
    pub(crate) fn record<'s>(
        &mut self,
        records: &mut Vec<Draft<'s>>,
        defined: &dyn Fn(&str) -> bool,
    ) -> Rewrite {
        let changed = changes(&self.report);
        let added = self.added_records();
        #[cfg(debug_assertions)]
        let before = records.clone();
        let mut cx = Cx {
            v7: self.v7,
            rules: self.rules,
            report: std::mem::take(&mut self.report),
            deferred: std::mem::take(&mut self.deferred),
            record: None,
            declare: std::mem::take(&mut self.declare),
            schema: std::mem::take(&mut self.schema),
            peers: Vec::new(),
            settled: false,
            defined: Some(defined),
            promoted: false,
        };
        let mut root = Draft {
            substructures: std::mem::take(records),
            ..Draft::default()
        };
        walk(&mut root, &mut Vec::new(), &mut cx, true);
        *records = root.substructures;
        self.report = cx.report;
        self.deferred = cx.deferred;
        self.declare = cx.declare;
        self.schema = cx.schema;
        let rewrite = Rewrite {
            changed: changes(&self.report) != changed,
            placeholder: self.added_records() != added,
            promoted: cx.promoted,
        };
        // Every change is reported: a record nothing was reported of is as
        // it was read.
        #[cfg(debug_assertions)]
        debug_assert!(rewrite.changed || *records == before, "{before:?}");
        rewrite
    }

    /// Interprets the record at `root` of the lexer's arena where it is, as
    /// [`Interpreter::record`] would interpret it, when every rule that
    /// applies to it renames, retypes or makes an event of a substructure
    /// of the record ([`Op::Rename`], [`Op::Fixed`], [`Op::Event`]), or
    /// keeps a structure as it is ([`Op::Known`]): only the substructures
    /// a rule rewrites are copied, as drafts; the others stay where they
    /// were read. The record's structures are looked at once, in the order
    /// read: the rules that may change each are found then, and whether
    /// the record is to be interpreted at all (what the reader asks before
    /// interpreting one, `wanted`); they are applied as each structure's
    /// own substructures are done with, as [`walk`] applies them. Nothing
    /// is reported until [`Interpreter::commit`]. `read` tells what the
    /// arena is.
    pub(crate) fn record_in_arena<'a>(
        &mut self,
        root: NodeRef<'a>,
        read: &InArena<'a, '_>,
    ) -> InPlace<'a> {
        let mut look = Look {
            rules: self.rules,
            v7: self.v7,
            read,
            root,
            wanted: false,
            emulated: true,
            notes: Vec::new(),
            edits: Vec::new(),
        };
        let nodes = root.subtree();
        let top = nodes.first().map_or(0, |n| n.depth);
        // The structures some rule may change, each with its depth, where
        // its substructures end, its place among the record's
        // substructures, and its rules (in `found`): few, found by their
        // tags alone.
        let InArenaScratch { waiting, found } = &mut self.scratch;
        waiting.clear();
        found.clear();
        // (Made for the first structure a rule may change: most records
        // have none.)
        let mut path: Option<[&'a str; IN_ARENA_DEPTH]> = None;
        for (at, raw) in nodes.iter().enumerate() {
            // (The record is the first, and the only one at its depth.)
            let bit = if at == 0 {
                InArena::RECORD
            } else {
                InArena::BELOW
            };
            if read
                .tried
                .get(raw.tag as usize)
                .is_some_and(|t| t & bit == 0)
            {
                continue;
            }
            let depth = raw.depth.wrapping_sub(top);
            let Some(n) = root.below(at) else { break };
            // Most structures a rule reads are values no rule changes (a
            // date in no dialect): told by the rules' ops alone, wherever
            // the structure is.
            if !look.may_change(n, depth == 0) {
                continue;
            }
            // The tags above it, and its place: by the structures before it.
            let path = path.get_or_insert([""; IN_ARENA_DEPTH]);
            let Some(above_path) = path.get_mut(..usize::from(depth)) else {
                look.emulated = false;
                look.wanted = true;
                break;
            };
            let mut want = depth;
            let mut ordinal = 0;
            for above in nodes.get(..at).unwrap_or_default().iter().rev() {
                let d = above.depth.wrapping_sub(top);
                if d == 1 && depth == 1 {
                    ordinal += 1;
                }
                if d < want {
                    want = d;
                    if let Some(slot) = above_path.get_mut(usize::from(d)) {
                        *slot = tag_of(above.tag, read.others);
                    }
                }
            }
            let from = found.len();
            look.rules_for(n, above_path, found);
            if found.len() > from {
                waiting.push((depth, raw.end, ordinal, at, from..found.len()));
            }
            if !look.emulated && look.wanted {
                break;
            }
        }
        if !look.wanted {
            // Nothing to do: nothing to report either.
            return InPlace {
                wanted: false,
                done: None,
            };
        }
        // As `walk` applies them: a structure's substructures first, then
        // the structure; the record's substructures in order.
        waiting.sort_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
        for (depth, _, ordinal, at, rules) in waiting.drain(..) {
            if let Some(n) = root.below(at) {
                look.apply(depth, ordinal, n, found.get(rules).unwrap_or_default());
            }
        }
        let (wanted, emulated) = (look.wanted, look.emulated);
        InPlace {
            wanted,
            done: emulated.then_some(Done {
                notes: Notes(look.notes),
                edits: look.edits,
            }),
        }
    }

    /// Reports what [`Interpreter::record_in_arena`] did of a record whose
    /// identifier is `xref` (`defined`: whether a record before it has
    /// it).
    pub(crate) fn commit(
        &mut self,
        notes: Notes<'_>,
        xref: Option<&str>,
        defined: &dyn Fn(&str) -> bool,
    ) {
        let record = xref.filter(|x| !defined(x)).map(Xref::new);
        for (applied, tag, action, line) in notes.0 {
            self.report
                .note(&applied, tag, action, line, record.as_ref());
            if action == Action::Kept && tag.starts_with('_') {
                let uri = registered_uri(tag)
                    .map_or_else(|| docs_uri(applied.profile.id, tag), str::to_string);
                self.declare.insert((tag.into(), uri));
            }
        }
    }

    /// Learns what a header says that interprets the records after it:
    /// Family Tree Maker's `_SCHEMA` labels.
    pub(crate) fn read_head(&mut self, head: &Draft<'_>) {
        let labels = schema_labels(std::slice::from_ref(head));
        self.schema.extend(labels);
    }

    fn added_records(&self) -> usize {
        self.deferred
            .iter()
            .filter(|(_, d)| matches!(d, Deferred::Record(_)))
            .count()
    }

    /// Makes the changes that span records in `records` — the first record
    /// of each identifier [`Interpreter::targets`] names, and each record
    /// that links to a record to add ([`Rewrite::placeholder`]), in the
    /// file's order — adding the new records after them, and declares the
    /// extensions kept in `head` (7.x). `taken`: whether a record of the
    /// file has an identifier. What was done is reported.
    pub(crate) fn finish<'s>(
        mut self,
        records: &mut Vec<Draft<'s>>,
        head: Option<&mut Draft<'s>>,
        taken: &dyn Fn(&str) -> bool,
    ) -> InterpretReport {
        let deferred = std::mem::take(&mut self.deferred);
        apply_deferred(records, deferred, &mut self.report, self.v7, Some(taken));
        if let Some(head) = head.filter(|_| self.v7)
            && declare(std::slice::from_mut(head), &self.declare)
        {
            self.report.changed = true;
        }
        self.report
    }

    /// Whether extensions kept are to be declared in the header (7.x).
    pub(crate) fn declares(&self) -> bool {
        self.v7 && !self.declare.is_empty()
    }

    /// The report so far, when nothing is pending.
    pub(crate) fn into_report(self) -> InterpretReport {
        self.report
    }
}

/// The text of the tag of id `id` in a lexer's arena whose tags that are
/// not standard are `others`.
fn tag_of(id: u32, others: &[Box<str>]) -> &str {
    let id = id as usize;
    match STANDARD_TAGS.get(id) {
        Some(tag) => tag,
        None => others.get(id - STANDARD_TAGS.len()).map_or("", |t| t),
    }
}

/// What a rule did to a substructure of a record interpreted where the
/// lexer left it ([`Interpreter::record_in_arena`]), by its place among
/// the record's substructures as read.
pub(crate) enum Edit<'a> {
    /// The substructure replaced by the one the rule made, or left out
    /// (`None`: one equal to it is there).
    Replace(usize, Option<Box<Draft<'a>>>),
    /// The structure the rule made, after the substructure.
    After(usize, Box<Draft<'a>>),
}

/// A substructure of a record as rules left it so far: as read, or made.
#[derive(Clone, Copy)]
enum Sibling<'s, 'a> {
    Read(NodeRef<'a>),
    New(&'s Draft<'a>),
}

/// What [`Interpreter::record_in_arena`] needs of the lexer's arena.
pub(crate) struct InArena<'a, 'x> {
    /// The tags of the arena that are not standard.
    pub(crate) others: &'a [Box<str>],
    /// The file's escaping.
    pub(crate) escaping: Escaping,
    /// Whether the documented extensions kept count: reported, or declared
    /// in 7.x.
    pub(crate) known: bool,
    /// By the id of a tag in the arena: whether some rule may apply to a
    /// structure of that tag, of a record (bit [`InArena::RECORD`]) or below
    /// one ([`InArena::BELOW`]); the others are not looked at. A tag past
    /// it may be tried anywhere.
    pub(crate) tried: &'x [u8],
}

impl InArena<'_, '_> {
    /// [`InArena::tried`] of a record.
    pub(crate) const RECORD: u8 = 4;
    /// [`InArena::tried`] below a record.
    pub(crate) const BELOW: u8 = 8;
}

/// What interpreting a record where it was read found
/// ([`Interpreter::record_in_arena`]).
pub(crate) struct InPlace<'a> {
    /// Whether a rule may change a structure of the record, as the reader
    /// asks before interpreting it (`Pass::wants`).
    pub(crate) wanted: bool,
    /// What the rules did, when only those it interprets so applied.
    pub(crate) done: Option<Done<'a>>,
}

/// The rules' work on a record interpreted where it was read: what to
/// report ([`Interpreter::commit`]), and what changed of its
/// substructures, in their order.
pub(crate) struct Done<'a> {
    pub(crate) notes: Notes<'a>,
    pub(crate) edits: Vec<Edit<'a>>,
}

/// What the rules did to the structures of a record, to report.
pub(crate) struct Notes<'a>(Vec<Note<'a>>);

/// What a rule did to a structure: the rule, the tag, the action and the
/// line.
type Note<'a> = (Applied, &'a str, Action, u32);

/// The interpretation of a record where the lexer left it
/// ([`Interpreter::record_in_arena`]).
struct Look<'r, 'a, 'x> {
    rules: &'r RuleSet,
    v7: bool,
    read: &'x InArena<'a, 'x>,
    root: NodeRef<'a>,
    /// Whether a rule may change a structure ([`InPlace::wanted`]).
    wanted: bool,
    /// Whether the rules that applied so far are those interpreted so.
    emulated: bool,
    notes: Vec<Note<'a>>,
    /// What the rules changed of the record's substructures, in their
    /// order.
    edits: Vec<Edit<'a>>,
}

impl<'a> Look<'_, 'a, '_> {
    /// Whether a rule on the tag of `n` (a record when `record`) may change
    /// it, wherever it is: [`Op::may_apply_to_parts`] of each, its context
    /// aside.
    fn may_change(&self, n: NodeRef<'a>, record: bool) -> bool {
        let rules = self.rules;
        let tag = n.tag_text(self.read.others);
        let standard = n
            .standard_tag()
            .and_then(|_| u16::try_from(n.tag_id()).ok());
        let any = if standard.is_some() {
            &rules.any_standard
        } else {
            &rules.any
        };
        let mut text = None;
        // An op that looks at the payload alone, found not to change it:
        // the rules after with the same op are not asked again (rules of
        // several contexts often have one).
        let mut refused = None;
        any_of(rules.rules_of(tag, standard), any, |a| {
            let op = a.rule.op;
            let kind = op.by_payload().then(|| std::mem::discriminant(&op));
            if kind.is_some() && kind == refused {
                return false;
            }
            // (The tag and the kind of payload tell of these.)
            let kind_only = matches!(op, Op::LongForm | Op::CaseFold | Op::RecordXref);
            let text: &str = if kind_only {
                ""
            } else {
                text.get_or_insert_with(|| n.payload_str(self.read.escaping))
            };
            let payload = if n.is_pointer() {
                PayloadRef::Pointer(text)
            } else if n.has_no_payload() {
                PayloadRef::None
            } else {
                PayloadRef::Text(text)
            };
            let may = op.may_apply_to_parts(tag, payload, n.has_xref(), record, self.v7);
            if !may && kind.is_some() {
                refused = kind;
            }
            may
        })
    }

    /// The rules for `n` under `path` that may change it, added to `found`
    /// in the order they are tried; notes whether the record is wanted, as
    /// [`RuleSet::may_rewrite`] finds it.
    fn rules_for(&mut self, n: NodeRef<'a>, path: &[&str], found: &mut Vec<Applied>) {
        let rules = self.rules;
        let tag = n.tag_text(self.read.others);
        let mut text = None;
        for a in rules.candidates(tag, path) {
            let text = text.get_or_insert_with(|| n.payload_str(self.read.escaping));
            let payload = if n.is_pointer() {
                PayloadRef::Pointer(text)
            } else if n.has_no_payload() {
                PayloadRef::None
            } else {
                PayloadRef::Text(text)
            };
            let op = a.rule.op;
            if !op.may_apply_to_parts(tag, payload, n.has_xref(), path.is_empty(), self.v7) {
                continue;
            }
            let inert = op.v7_only()
                && !self.v7
                && !matches!(op, Op::Exid { uri, .. } if uri == super::FAMILYSEARCH_PERSON);
            self.wanted |= !inert && (self.read.known || !matches!(op, Op::Known));
            found.push(a);
        }
    }

    /// Applies `rules`, those that may change `n` (`depth` deep, substructure
    /// `k` of the record as read when one deep), as [`walk`] tries them:
    /// the first that applies wins. A structure below the record's
    /// substructures, or the record itself, may only be kept as it is.
    fn apply(&mut self, depth: u8, k: usize, n: NodeRef<'a>, rules: &[Applied]) {
        if !self.emulated {
            return;
        }
        let line = n.line();
        let tag = n.tag_text(self.read.others);
        let mut text = None;
        for &a in rules {
            if !self.v7 && a.rule.op.skips_in_551() {
                continue;
            }
            if depth != 1 {
                if matches!(a.rule.op, Op::Known) {
                    self.notes.push((a, tag, Action::Kept, line));
                } else {
                    self.emulated = false;
                }
                return;
            }
            let (others, escaping) = (self.read.others, self.read.escaping);
            let read = || Draft::of_arena(n, others, escaping);
            let new = match a.rule.op {
                Op::Known => {
                    self.notes.push((a, tag, Action::Kept, line));
                    return;
                }
                Op::Rename(to) => {
                    let text = text.get_or_insert_with(|| n.payload_str(escaping));
                    if !renames(to, text, self.v7) {
                        continue;
                    }
                    let mut new = read();
                    retag(&mut new, to);
                    new
                }
                Op::Fixed(to, value) => {
                    let mut new = read();
                    retag(&mut new, to);
                    set_text(&mut new, value);
                    new.substructures.clear();
                    new
                }
                Op::Event { tag: to, label, y } if label != Label::Schema => {
                    let node = read();
                    let ty = label_of(&node, label);
                    event_of(node, to, ty, label == Label::Payload, y)
                }
                _ => {
                    self.emulated = false;
                    return;
                }
            };
            if self.place(n, k, new, a) {
                return;
            }
            // An equal one is there already: no trace, the next rule.
        }
    }

    /// The record's substructures as the rules left them so far, each
    /// with its place among those read (a structure made, that of the one
    /// it replaces or follows).
    fn siblings(&self, mut f: impl FnMut(usize, Sibling<'_, 'a>)) {
        let mut edits = self.edits.iter().peekable();
        for (k, c) in self.root.children().enumerate() {
            let mut kept = true;
            while let Some(e) = edits.next_if(|e| match e {
                Edit::Replace(at, _) | Edit::After(at, _) => *at == k,
            }) {
                match e {
                    Edit::Replace(_, new) => {
                        kept = false;
                        if let Some(d) = new {
                            f(k, Sibling::New(d));
                        }
                    }
                    Edit::After(_, d) => {
                        if kept {
                            f(k, Sibling::Read(c));
                            kept = false;
                        }
                        f(k, Sibling::New(d));
                    }
                }
            }
            if kept {
                f(k, Sibling::Read(c));
            }
        }
    }

    /// [`place`] of `new`, made of `n`, the record's substructure `k` as
    /// read: whether a rule applied (`false`: `new` adds nothing and leaves
    /// no trace).
    fn place(&mut self, n: NodeRef<'a>, k: usize, new: Draft<'a>, a: Applied) -> bool {
        let read = self.read;
        let want = new.tag.as_str().trim_start_matches('_');
        // A standard tag read is told by its id: those of `new`'s tag and
        // of `want`, when standard.
        let standard = u32::try_from(STANDARD_TAGS.len()).unwrap_or(u32::MAX);
        let new_id = standard_index(new.tag.as_str()).map(u32::from);
        let want_id = standard_index(want).map(u32::from);
        // Whether a sibling `c` holds `new` (`said`: as `holds` finds,
        // else equal).
        let like = |c: Sibling<'_, 'a>, said: bool| -> bool {
            let fits = match c {
                Sibling::Read(r) if r.tag_id() < standard => {
                    Some(r.tag_id()) == if said { want_id } else { new_id }
                }
                _ => {
                    let tag = match c {
                        Sibling::Read(r) => r.tag_text(read.others),
                        Sibling::New(d) => d.tag.as_str(),
                    };
                    if said {
                        tag.trim_start_matches('_') == want
                    } else {
                        tag == new.tag.as_str()
                    }
                }
            };
            if !fits {
                return false;
            }
            let read_draft;
            let d = match c {
                Sibling::Read(r) => {
                    read_draft = Draft::of_arena(r, read.others, read.escaping);
                    &read_draft
                }
                Sibling::New(d) => d,
            };
            if said {
                holds(std::iter::once(d), &new)
            } else {
                *d == new
            }
        };
        let lossless = a.rule.fidelity == Fidelity::Lossless;
        let mut said = false;
        let mut equal = false;
        self.siblings(|j, c| {
            let other = j != k || matches!(c, Sibling::New(_));
            if lossless && other {
                said = said || like(c, true);
            }
            // (Lossless, the substructure itself is left out first.)
            if !lossless || other {
                equal = equal || like(c, false);
            }
        });
        let tag = n.tag_text(read.others);
        if lossless {
            // Said already: what was read stays.
            if said {
                return true;
            }
            let at = self.edits.partition_point(
                |e| matches!(e, Edit::Replace(j, _) | Edit::After(j, _) if *j < k),
            );
            self.edits
                .insert(at, Edit::Replace(k, (!equal).then(|| Box::new(new))));
            self.notes.push((a, tag, Action::Replaced, n.line()));
            return true;
        }
        if equal {
            return false;
        }
        let at = self
            .edits
            .partition_point(|e| matches!(e, Edit::Replace(j, _) | Edit::After(j, _) if *j < k));
        self.edits.insert(at, Edit::After(k, Box::new(new)));
        self.notes.push((a, tag, Action::Added, n.line()));
        true
    }
}

/// How many changes `report` reports (what is kept as it is aside).
fn changes(report: &InterpretReport) -> usize {
    report
        .mappings
        .iter()
        .filter(|m| m.action != Action::Kept)
        .map(|m| m.count)
        .sum()
}

/// The state of an interpretation.
struct Cx<'r, 's> {
    v7: bool,
    rules: &'r RuleSet,
    report: InterpretReport,
    deferred: Vec<(Applied, Deferred)>,
    record: Option<Xref>,
    /// The extension tags kept, to declare in 7.x (tag, URI).
    declare: BTreeSet<(Box<str>, String)>,
    /// Family Tree Maker for Windows' fact labels: (record tag, tag, label).
    schema: Vec<(String, String, String)>,
    /// The siblings of the structure being walked that what its
    /// substructures lift may repeat.
    peers: Vec<Draft<'s>>,
    /// Whether the rule just tried found its meaning said already.
    settled: bool,
    /// One record at a time ([`Interpreter`]): whether a record before the
    /// one interpreted has an identifier.
    defined: Option<&'r dyn Fn(&str) -> bool>,
    /// One record at a time: whether the record went first among the
    /// records of its tag.
    promoted: bool,
}

/// A change in another record, made once every record is read.
enum Deferred {
    /// `PEDI` on the child's link to the family.
    Pedigree {
        child: Xref,
        family: Xref,
        pedigree: Draft<'static>,
    },
    /// The family first among a person's (`FAMS`, or `FAMC` as child).
    FamilyFirst {
        person: Xref,
        family: Xref,
        as_child: bool,
    },
    /// A marriage witness in the family.
    Witness { family: Xref, assoc: Draft<'static> },
    /// The child of an adoptive family, linked both ways.
    Adopted { child: Xref, family: Xref },
    /// A new record.
    Record(Draft<'static>),
}

/// What applying a rule to the substructure at an index did.
struct Outcome<'s> {
    /// How many structures now stand where it stood (it, its replacements
    /// and the siblings added after it).
    slot: usize,
    /// Structures for beside the parent.
    lift: Vec<Draft<'s>>,
    /// Whether the parent goes first among its like.
    promote: bool,
}

impl Outcome<'_> {
    fn skip() -> Self {
        Self {
            slot: 1,
            lift: Vec::new(),
            promote: false,
        }
    }
}

/// Interprets the substructures of `parent` (whose tags from the record
/// down are `path`): returns what goes beside `parent`, and whether
/// `parent` goes first among its like.
fn walk<'s>(
    parent: &mut Draft<'s>,
    path: &mut Vec<DraftTag<'s>>,
    cx: &mut Cx<'_, 's>,
    root: bool,
) -> (Vec<Draft<'s>>, bool) {
    let mut lifted_up = Vec::new();
    let mut promote_up = false;
    // The identifiers of the records before the one walked (those up to
    // `seen_upto`).
    let mut seen: HashSet<Cow<'s, str>, BuildHasherDefault<TagHasher>> = HashSet::default();
    let mut seen_upto = 0;
    let mut i = 0;
    while i < parent.substructures.len() {
        if let (true, Some(defined)) = (root, cx.defined) {
            cx.record = parent
                .substructures
                .get(i)
                .and_then(|r| r.xref.as_deref())
                .filter(|x| !defined(x))
                .map(Xref::new);
        } else if root {
            // A record whose identifier an earlier one has: the writer
            // renames it, so changes across records skip it (pointers name
            // the first).
            for r in parent.substructures.get(seen_upto..i).unwrap_or_default() {
                if let Some(x) = &r.xref {
                    seen.insert(x.clone());
                }
            }
            seen_upto = seen_upto.max(i);
            cx.record = parent
                .substructures
                .get(i)
                .and_then(|r| r.xref.as_deref())
                .filter(|x| !seen.contains(*x))
                .map(Xref::new);
        }
        // The siblings of the same tag, which what a substructure lifts
        // may repeat (names under a name).
        let peers = match parent.substructures.get(i) {
            Some(c) if is(c, "NAME") && !c.substructures.is_empty() => parent
                .substructures
                .iter()
                .filter(|p| is(p, "NAME") || is(p, "_NAME"))
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        let (lift, promote) = match parent.substructures.get_mut(i) {
            Some(c) if !c.substructures.is_empty() => {
                path.push(c.tag.clone());
                let saved = std::mem::replace(&mut cx.peers, peers);
                let out = walk(c, path, cx, false);
                cx.peers = saved;
                path.pop();
                out
            }
            _ => (Vec::new(), false),
        };
        let mut n = 0;
        for s in lift {
            if insert_new(parent, i + 1 + n, s) {
                n += 1;
            }
        }
        if promote {
            cx.promoted |= root && cx.defined.is_some();
            promote_child(parent, i);
        }
        let tag = match parent.substructures.get(i) {
            Some(c) => c.tag.clone(),
            None => break,
        };
        // The first rule that applies wins; one that does not leaves no
        // trace in the report or the deferred changes.
        let mut outcome = Outcome::skip();
        let rules = cx.rules;
        for applied in rules.candidates(tag.as_str(), path) {
            let before = (
                cx.report.count(),
                cx.report.mappings.len(),
                cx.deferred.len(),
            );
            cx.settled = false;
            outcome = apply(applied, parent, i, path, cx);
            // A rule that applied, or found its meaning said already, ends
            // the search: another producer's rule never overrides it.
            if cx.settled
                || (
                    cx.report.count(),
                    cx.report.mappings.len(),
                    cx.deferred.len(),
                ) != before
            {
                break;
            }
        }
        lifted_up.extend(outcome.lift);
        promote_up |= outcome.promote;
        i += outcome.slot + n;
    }
    (lifted_up, promote_up)
}

/// Moves the substructure at `i` before the first of its tag.
fn promote_child(parent: &mut Draft<'_>, i: usize) {
    let Some(tag) = parent.substructures.get(i).map(|c| c.tag.clone()) else {
        return;
    };
    if let Some(first) = parent.substructures.iter().position(|c| c.tag == tag)
        && first < i
    {
        let moved = parent.substructures.remove(i);
        parent.substructures.insert(first, moved);
    }
}

/// The rule's verdict on the structure at `i`: replace it by `new` when
/// lossless, else add `new` after it (unless an equal one is there).
fn place<'s>(
    parent: &mut Draft<'s>,
    i: usize,
    new: Vec<Draft<'s>>,
    applied: Applied,
    cx: &mut Cx<'_, 's>,
) -> Outcome<'s> {
    let lossless = applied.rule.fidelity == Fidelity::Lossless;
    let (tag, line) = match parent.substructures.get(i) {
        Some(c) => (c.tag.as_str().to_string(), c.line),
        None => return Outcome::skip(),
    };
    if lossless {
        // Said already: what was read stays (applying again changes
        // nothing).
        // (The structure read itself does not count: its tag is the
        // extension the rule reads.)
        let others = || {
            parent
                .substructures
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != i)
                .map(|(_, c)| c)
        };
        if new.iter().all(|s| holds(others(), s)) {
            cx.settled = true;
            return Outcome::skip();
        }
        parent.substructures.remove(i);
        let mut slot = 0;
        for s in new {
            if insert_new(parent, i + slot, s) {
                slot += 1;
            }
        }
        cx.report
            .note(&applied, &tag, Action::Replaced, line, cx.record.as_ref());
        Outcome {
            slot,
            ..Outcome::skip()
        }
    } else {
        let mut slot = 1;
        for s in new {
            if insert_new(parent, i + slot, s) {
                slot += 1;
            }
        }
        if slot > 1 {
            cx.report
                .note(&applied, &tag, Action::Added, line, cx.record.as_ref());
        }
        Outcome {
            slot,
            ..Outcome::skip()
        }
    }
}

/// Like [`place`], for structures that go beside the parent.
fn lift<'s>(
    parent: &mut Draft<'s>,
    i: usize,
    new: Vec<Draft<'s>>,
    applied: Applied,
    cx: &mut Cx<'_, 's>,
) -> Outcome<'s> {
    let (tag, line) = match parent.substructures.get(i) {
        Some(c) => (c.tag.as_str().to_string(), c.line),
        None => return Outcome::skip(),
    };
    // Said already beside the parent (or by the parent: a name of type
    // AKA holding its own `_AKA`): nothing to do.
    if new.iter().all(|s| holds(&cx.peers, s)) {
        cx.settled = true;
        return Outcome::skip();
    }
    let lossless = applied.rule.fidelity == Fidelity::Lossless;
    let action = if lossless {
        parent.substructures.remove(i);
        Action::Replaced
    } else {
        Action::Added
    };
    cx.report
        .note(&applied, &tag, action, line, cx.record.as_ref());
    Outcome {
        slot: usize::from(!lossless),
        lift: new,
        promote: false,
    }
}

/// Edits the structure at `i` in place when lossless, else adds the edited
/// copy after it.
fn edit<'s>(
    parent: &mut Draft<'s>,
    i: usize,
    applied: Applied,
    cx: &mut Cx<'_, 's>,
    f: impl FnOnce(&mut Draft<'s>) -> bool,
) -> Outcome<'s> {
    let Some(mut copy) = parent.substructures.get(i).cloned() else {
        return Outcome::skip();
    };
    if !f(&mut copy) {
        return Outcome::skip();
    }
    place(parent, i, vec![copy], applied, cx)
}

fn kept<'s>(parent: &Draft<'s>, i: usize, applied: Applied, cx: &mut Cx<'_, 's>) -> Outcome<'s> {
    if let Some(c) = parent.substructures.get(i) {
        let tag = c.tag.as_str().to_string();
        cx.report
            .note(&applied, &tag, Action::Kept, c.line, cx.record.as_ref());
        if c.tag.is_extension() {
            let uri = registered_uri(&tag)
                .map_or_else(|| docs_uri(applied.profile.id, &tag), str::to_string);
            cx.declare.insert((tag.into(), uri));
        }
    }
    Outcome::skip()
}

/// The URI of an extension tag documented elsewhere than by oxidged: the
/// GEDCOM-L addendum's.
pub(crate) fn registered_uri(tag: &str) -> Option<&'static str> {
    crate::convert::tables::KNOWN_EXTENSIONS
        .iter()
        .chain(&[("_RUFNAME", "http://genealogy.net/GEDCOM#_RUFNAME")])
        .find(|(t, _)| *t == tag)
        .map(|&(_, uri)| uri)
}

/// The documentation URI oxidged publishes for a producer's extension
/// tag: `<docs>#<profile>-<TAG>`.
#[must_use]
pub fn docs_uri(profile: &str, tag: &str) -> String {
    format!("{EXTENSION_DOCS}#{profile}-{tag}")
}

fn noted<'s>(parent: &Draft<'s>, i: usize, applied: Applied, cx: &mut Cx<'_, 's>, action: Action) {
    if let Some(c) = parent.substructures.get(i) {
        let tag = c.tag.as_str().to_string();
        cx.report
            .note(&applied, &tag, action, c.line, cx.record.as_ref());
    }
}

/// Whether an [`Op::Rename`] rule renames a structure to `to`, with
/// `payload`: a sex the version has no value for (5.5.1 `_SEX X`) stays an
/// extension, which the writer would make it again.
fn renames(to: &str, payload: &str, v7: bool) -> bool {
    let version = if v7 {
        GedcomVersion::V7_0
    } else {
        GedcomVersion::V5_5_1
    };
    to != "SEX"
        || <crate::model::Sex as crate::model::Enumeration>::known(payload, Some(version)).is_some()
}

/// The type label an [`Op::Event`] rule gives the event it makes of
/// `node`, but from the header's schema ([`Label::Schema`]).
fn label_of(node: &Draft<'_>, label: Label) -> Option<Cow<'static, str>> {
    match label {
        Label::None | Label::Schema => None,
        Label::Text(t) => Some(Cow::Borrowed(t)),
        Label::Payload => {
            let t = payload_str(node).trim();
            (!t.is_empty()).then(|| Cow::Owned(t.to_string()))
        }
    }
}

/// The event or attribute `tag` an [`Op::Event`] rule makes of `node`,
/// with the type label `ty` (its payload left out when it is the label,
/// `payload`), said `Y` when `y` and nothing more is said.
fn event_of<'s>(
    mut new: Draft<'s>,
    tag: &'static str,
    ty: Option<Cow<'s, str>>,
    payload: bool,
    y: bool,
) -> Draft<'s> {
    retag(&mut new, tag);
    if payload {
        new.payload = Value::None;
    }
    if let Some(ty) = ty
        && !has(&new, "TYPE")
    {
        // First, the others after.
        let mut substructures = Vec::with_capacity(new.substructures.len() + 1);
        substructures.push(Draft {
            tag: DraftTag::new("TYPE"),
            payload: if ty.is_empty() {
                Value::None
            } else {
                Value::Text(ty)
            },
            ..Draft::default()
        });
        substructures.append(&mut new.substructures);
        new.substructures = substructures;
    }
    // A standard event says `Y` or nothing: a text goes to a note.
    if !matches!(tag, "EVEN" | "FACT")
        && let Some(t) = new.text().map(str::to_string)
        && !t.eq_ignore_ascii_case("y")
    {
        new.payload = Value::None;
        new.substructures.push(text_node("NOTE", &t));
    }
    if tag == "FACT" && new.text().is_none() {
        retag(&mut new, "EVEN");
    }
    if y && matches!(new.payload, Value::None) && new.substructures.is_empty() {
        set_text(&mut new, "Y");
    }
    new
}

#[allow(clippy::too_many_lines)]
fn apply<'s>(
    applied: Applied,
    parent: &mut Draft<'s>,
    i: usize,
    path: &[DraftTag<'s>],
    cx: &mut Cx<'_, 's>,
) -> Outcome<'s> {
    let v7 = cx.v7;
    // What surely leaves the structure as it is, told before it is copied
    // (the rules on any tag look at every structure).
    if !parent
        .substructures
        .get(i)
        .is_some_and(|s| applied.rule.op.may_apply_to(s, path, v7))
    {
        return Outcome::skip();
    }
    let Some(node) = parent.substructures.get(i).cloned() else {
        return Outcome::skip();
    };
    match applied.rule.op {
        Op::Known => kept(parent, i, applied, cx),
        Op::Rename(to) => {
            if !renames(to, payload_str(&node), v7) {
                return Outcome::skip();
            }
            edit(parent, i, applied, cx, |s| {
                retag(s, to);
                true
            })
        }
        Op::Fixed(to, value) => edit(parent, i, applied, cx, |s| {
            retag(s, to);
            set_text(s, value);
            s.substructures.clear();
            true
        }),
        Op::Event { tag, label, y } => {
            let ty = match label {
                Label::Schema => {
                    let record = path.first().map_or("", DraftTag::as_str);
                    let found = cx
                        .schema
                        .iter()
                        .find(|(r, t, _)| r == record && t == node.tag.as_str())
                        .map(|(_, _, l)| Cow::Owned(l.clone()));
                    if found.is_none() {
                        return Outcome::skip();
                    }
                    found
                }
                label => label_of(&node, label),
            };
            let new = event_of(node, tag, ty, label == Label::Payload, y);
            place(parent, i, vec![new], applied, cx)
        }
        Op::Name(kind, value) => name(parent, i, applied, path, cx, kind, value),
        Op::Translation(lang) => {
            let value = payload_str(&node).trim();
            if !v7 || value.is_empty() {
                return Outcome::skip();
            }
            let mut tran = text_node("TRAN", value);
            tran.substructures.push(text_node("LANG", lang));
            place(parent, i, vec![tran], applied, cx)
        }
        Op::PlaceEvent(label) => {
            let value = payload_str(&node).trim().to_string();
            let mut even = Draft::new("EVEN");
            even.substructures.push(text_node("TYPE", label));
            if !value.is_empty() && !has(&node, "PLAC") {
                even.substructures.push(text_node("PLAC", &value));
            }
            even.substructures.extend(
                node.substructures
                    .iter()
                    .filter(|s| !is(s, "TYPE"))
                    .cloned(),
            );
            place(parent, i, vec![even], applied, cx)
        }
        Op::ExidUrl { prefix, uri } => {
            let value = payload_str(&node).trim();
            let lower = value.to_ascii_lowercase();
            let Some(id) = lower
                .starts_with(&prefix.to_ascii_lowercase())
                .then(|| value.get(prefix.len()..))
                .flatten()
                .filter(|id| !id.is_empty() && !id.contains('/'))
            else {
                return Outcome::skip();
            };
            if !v7 {
                return Outcome::skip();
            }
            let mut exid = text_node("EXID", id);
            exid.substructures.push(text_node("TYPE", uri));
            place(parent, i, vec![exid], applied, cx)
        }
        Op::ChildNote(tag) => {
            let Some(text) = child_text(&node, tag).map(str::to_string) else {
                return Outcome::skip();
            };
            let mut biography = text_node("NOTE", text.trim_start_matches('\n'));
            biography
                .substructures
                .extend(node.substructures.iter().filter(|s| !is(s, tag)).cloned());
            place(parent, i, vec![biography], applied, cx)
        }
        Op::Html => {
            let text = payload_str(&node).to_string();
            let unescaped = unescape_html(&text);
            let html = |t: &str| {
                [
                    "<p>", "<br", "<pre>", "<b>", "<i>", "<u>", "<a ", "<div", "</",
                ]
                .iter()
                .any(|m| t.to_ascii_lowercase().contains(m))
            };
            if !v7
                || (has(&node, "MIME") || has(&node, "_MIME"))
                || !(html(&text) || html(&unescaped))
            {
                return Outcome::skip();
            }
            edit(parent, i, applied, cx, |s| {
                if html(&unescaped) && !html(&text) {
                    set_text(s, &unescaped);
                }
                s.substructures.insert(0, text_node("MIME", "text/html"));
                true
            })
        }
        Op::IntoSibling(tag) => {
            let Some(at) = parent.substructures.iter().position(|c| is(c, tag)) else {
                return Outcome::skip();
            };
            let moved = parent.substructures.remove(i);
            let at = if at > i { at - 1 } else { at };
            noted_tag(&moved, applied, cx, Action::Replaced);
            if let Some(target) = parent.substructures.get_mut(at) {
                target.substructures.push(moved);
            }
            Outcome {
                slot: 0,
                ..Outcome::skip()
            }
        }
        Op::IntoAs(container, tag) => {
            if parent
                .substructures
                .iter()
                .any(|c| is(c, container) && has(c, tag))
            {
                return Outcome::skip();
            }
            let mut moved = node.clone();
            retag(&mut moved, tag);
            let lossless = applied.rule.fidelity == Fidelity::Lossless;
            if lossless {
                parent.substructures.remove(i);
            }
            if let Some(c) = child_mut(parent, container) {
                c.substructures.push(moved);
            } else {
                let mut c = Draft::new(container);
                c.substructures.push(moved);
                parent.substructures.push(c);
            }
            noted_tag(
                &node,
                applied,
                cx,
                if lossless {
                    Action::Replaced
                } else {
                    Action::Added
                },
            );
            Outcome {
                slot: usize::from(!lossless),
                ..Outcome::skip()
            }
        }
        Op::CaseFold => {
            let tag = node.tag.as_str();
            // The standard tag, as the table spells it.
            let Some(upper) = standard_index(&tag.to_ascii_uppercase())
                .and_then(|i| STANDARD_TAGS.get(usize::from(i)).copied())
            else {
                return Outcome::skip();
            };
            if upper == tag {
                return Outcome::skip();
            }
            edit(parent, i, applied, cx, |s| {
                retag(s, upper);
                true
            })
        }
        Op::Default(tag, value) => {
            // Present, or kept as an extension (5.5.1 `_SEX X`).
            if has(&node, tag)
                || node
                    .substructures
                    .iter()
                    .any(|c| c.tag.as_str().strip_prefix('_') == Some(tag))
            {
                return Outcome::skip();
            }
            if let Some(n) = parent.substructures.get_mut(i) {
                let at = n
                    .substructures
                    .iter()
                    .position(|c| !is(c, "NAME"))
                    .unwrap_or(n.substructures.len());
                n.substructures.insert(at, text_node(tag, value));
            }
            noted_tag(&node, applied, cx, Action::Added);
            Outcome::skip()
        }
        Op::SubmitterRecord => {
            if !matches!(node.payload, Value::None | Value::Text(_))
                || node.substructures.is_empty()
            {
                return Outcome::skip();
            }
            let mut record = node.clone();
            if let Value::Text(t) = &node.payload {
                record.substructures.insert(0, text_node("NAME", t.trim()));
            }
            record.payload = Value::None;
            let mut link = Draft::new("SUBM");
            link.payload = Value::Pointer(Cow::Borrowed(PLACEHOLDER));
            cx.deferred
                .push((applied, Deferred::Record(record.into_owned())));
            place(parent, i, vec![link], applied, cx)
        }
        Op::Exid { uri, strip } => {
            let value = payload_str(&node).trim();
            let value = value
                .strip_prefix(strip)
                .unwrap_or(value)
                .trim()
                .to_string();
            if value.is_empty() {
                return Outcome::skip();
            }
            if v7 {
                let mut exid = text_node("EXID", &value);
                exid.substructures.push(text_node("TYPE", uri));
                exid.substructures.extend(
                    node.substructures
                        .iter()
                        .filter(|s| !is(s, "TYPE"))
                        .cloned(),
                );
                place(parent, i, vec![exid], applied, cx)
            } else if uri == super::FAMILYSEARCH_PERSON && node.tag.as_str() != "_FSFTID" {
                let mut fs = node.clone();
                retag(&mut fs, "_FSFTID");
                set_text(&mut fs, &value);
                place(parent, i, vec![fs], applied, cx)
            } else {
                Outcome::skip()
            }
        }
        Op::V7(tag) => {
            if !v7 {
                return Outcome::skip();
            }
            if tag == "SDATE"
                && DateValue::parse_strict(payload_str(&node), GedcomVersion::V7_0).is_err()
            {
                return Outcome::skip();
            }
            edit(parent, i, applied, cx, |s| {
                retag(s, tag);
                true
            })
        }
        Op::Assoc(role) => {
            if !v7 {
                return Outcome::skip();
            }
            let mut assoc = Draft::new("ASSO");
            let mut role_node = text_node("ROLE", role);
            match &node.payload {
                Value::Pointer(p) => assoc.payload = Value::Pointer(p.clone()),
                Value::Text(t) if !t.trim().is_empty() && !t.trim_start().starts_with('@') => {
                    assoc.payload = Value::Pointer(Cow::Borrowed(Xref::VOID));
                    assoc.substructures.push(text_node("PHRASE", t.trim()));
                }
                _ => return Outcome::skip(),
            }
            // A `TYPE` or `ROLE` under it words the role more precisely.
            if let Some(word) = child_text(&node, "ROLE").or_else(|| child_text(&node, "TYPE")) {
                let (r, phrase) = role_of(word);
                if r != "OTHER" || role == "OTHER" {
                    set_text(&mut role_node, r);
                }
                if let Some(p) = phrase {
                    role_node.substructures.push(text_node("PHRASE", &p));
                }
            }
            assoc.substructures.push(role_node);
            assoc.substructures.extend(
                node.substructures
                    .iter()
                    .filter(|s| !matches!(s.tag.as_str(), "ROLE" | "TYPE" | "RELA"))
                    .cloned(),
            );
            place(parent, i, vec![assoc], applied, cx)
        }
        Op::Shared => {
            // A pointer the writer's repair kept as text (`@I9@` to no
            // record) stays as it is.
            if !v7
                || matches!(node.payload, Value::None)
                || node.text().is_some_and(|t| t.trim_start().starts_with('@'))
            {
                return Outcome::skip();
            }
            let mut assoc = node.clone();
            retag(&mut assoc, "ASSO");
            if let Value::Text(t) = &node.payload {
                assoc.payload = Value::Pointer(Cow::Borrowed(Xref::VOID));
                assoc.substructures.insert(0, text_node("PHRASE", t.trim()));
            }
            // A 7.x role already (`ROLE WITN` + `PHRASE`) stays as it is.
            let standard = child(&node, "ROLE").filter(|r| {
                let v = payload_str(r).trim();
                role_of(v).0 == v
            });
            if standard.is_none() {
                let word = child_text(&node, "ROLE")
                    .or_else(|| child_text(&node, "RELA"))
                    .map(str::to_string);
                assoc
                    .substructures
                    .retain(|s| !matches!(s.tag.as_str(), "ROLE" | "RELA"));
                let (role, phrase) = word.as_deref().map_or(("WITN", None), role_of);
                let mut role_node = text_node("ROLE", role);
                if let Some(p) = phrase {
                    role_node.substructures.push(text_node("PHRASE", &p));
                }
                assoc.substructures.push(role_node);
            }
            place(parent, i, vec![assoc], applied, cx)
        }
        Op::RoleWords(words) => edit(parent, i, applied, cx, |s| role_words(s, words, v7)),
        Op::Pedigree => pedigree(parent, i, applied, path, cx),
        Op::PedigreeWord => {
            let value = payload_str(&node).to_string();
            let Some(pedi) = pedigree_of(&value, v7) else {
                return Outcome::skip();
            };
            if path.last().is_some_and(|t| t.as_str() == "FAMC") {
                if has(parent, "PEDI") {
                    return Outcome::skip();
                }
                place(parent, i, vec![pedi], applied, cx)
            } else {
                // Heredis `_FIL` under INDI: the individual's one family.
                let famcs: Vec<usize> = parent
                    .substructures
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| is(c, "FAMC"))
                    .map(|(k, _)| k)
                    .collect();
                let [at] = famcs.as_slice() else {
                    return Outcome::skip();
                };
                let at = *at;
                if parent.substructures.get(at).is_some_and(|f| has(f, "PEDI")) {
                    return Outcome::skip();
                }
                if let Some(famc) = parent.substructures.get_mut(at) {
                    famc.substructures.push(pedi);
                }
                noted(parent, i, applied, cx, Action::Added);
                Outcome::skip()
            }
        }
        Op::Prefer => {
            if !is_yes(&node) {
                return Outcome::skip();
            }
            noted(parent, i, applied, cx, Action::Reordered);
            if v7 {
                parent.substructures.remove(i);
                Outcome {
                    slot: 0,
                    lift: Vec::new(),
                    promote: true,
                }
            } else {
                Outcome {
                    promote: true,
                    ..Outcome::skip()
                }
            }
        }
        Op::PreferPointer => prefer_pointer(parent, i, applied, cx),
        Op::PreferFamily => {
            if !is_yes(&node) {
                return Outcome::skip();
            }
            let (Some(record), Some(Value::Pointer(person))) =
                (cx.record.clone(), Some(parent.payload.clone()))
            else {
                return Outcome::skip();
            };
            let as_child = is(parent, "CHIL");
            cx.deferred.push((
                applied,
                Deferred::FamilyFirst {
                    person: Xref::new(&*person),
                    family: record,
                    as_child,
                },
            ));
            Outcome::skip()
        }
        Op::Privacy => {
            if !is_yes(&node) {
                return Outcome::skip();
            }
            let word = if v7 { "PRIVACY" } else { "privacy" };
            if let Some(resn) = child_mut(parent, "RESN") {
                let current = payload_str(resn).to_string();
                if current.to_ascii_uppercase().contains("PRIVACY") {
                    return Outcome::skip();
                }
                let joined = if current.trim().is_empty() {
                    word.to_string()
                } else {
                    format!("{current}, {word}")
                };
                set_text(resn, &joined);
                if applied.rule.fidelity == Fidelity::Lossless {
                    parent.substructures.remove(i);
                    noted_tag(&node, applied, cx, Action::Replaced);
                    return Outcome {
                        slot: 0,
                        ..Outcome::skip()
                    };
                }
                noted_tag(&node, applied, cx, Action::Added);
                return Outcome::skip();
            }
            place(parent, i, vec![text_node("RESN", word)], applied, cx)
        }
        Op::Change => {
            if said(parent, "CHAN") {
                return Outcome::skip();
            }
            let Some((date, time)) = stamp(payload_str(&node), v7) else {
                return Outcome::skip();
            };
            let mut date_node = text_node("DATE", &date);
            if let Some(t) = time {
                date_node.substructures.push(text_node("TIME", &t));
            }
            let mut chan = Draft::new("CHAN");
            chan.substructures.push(date_node);
            place(parent, i, vec![chan], applied, cx)
        }
        Op::Creation => {
            if !v7 || said(parent, "CREA") {
                return Outcome::skip();
            }
            let Some((date, time)) = stamp(payload_str(&node), true) else {
                return Outcome::skip();
            };
            let mut date_node = text_node("DATE", &date);
            if let Some(t) = time {
                date_node.substructures.push(text_node("TIME", &t));
            }
            let mut crea = Draft::new("CREA");
            crea.substructures.push(date_node);
            place(parent, i, vec![crea], applied, cx)
        }
        Op::Crop(form) => {
            // The repair's `_CROP` (with `TOP`, …) is one already; Ancestry's
            // (with `_TOP`, …) is what this reads.
            let repaired = parent.substructures.iter().any(|c| {
                is(c, "_CROP")
                    && c.substructures
                        .iter()
                        .any(|d| matches!(d.tag.as_str(), "TOP" | "LEFT" | "WIDTH" | "HEIGHT"))
            });
            if !v7 || has(parent, "CROP") || repaired {
                return Outcome::skip();
            }
            let Some(crop) = crop_of(&node, form) else {
                return Outcome::skip();
            };
            place(parent, i, vec![crop], applied, cx)
        }
        Op::NoEvent(tag) => {
            if !v7
                || matches!(node.payload, Value::Text(ref t) if t.trim().eq_ignore_ascii_case("N"))
            {
                return Outcome::skip();
            }
            if has(parent, tag)
                || parent
                    .substructures
                    .iter()
                    .any(|c| (is(c, "NO") || is(c, "_NO")) && payload_str(c).trim() == tag)
            {
                return Outcome::skip();
            }
            if let Value::Text(t) = &node.payload {
                let t = t.to_ascii_uppercase();
                if !(t.contains("NEVER")
                    || t.contains("NOT")
                    || t == "Y"
                    || t.contains("UNMARRIED"))
                {
                    return Outcome::skip();
                }
            }
            place(parent, i, vec![text_node("NO", tag)], applied, cx)
        }
        Op::ToNote(label) => {
            let text = payload_str(&node).trim();
            if text.is_empty() {
                return Outcome::skip();
            }
            let labelled = text_node("NOTE", &format!("{label}: {text}"));
            place(parent, i, vec![labelled], applied, cx)
        }
        Op::Values(map) => {
            let value = payload_str(&node).trim().to_string();
            let Some(&(_, to)) = map
                .iter()
                .find(|(from, _)| from.eq_ignore_ascii_case(&value))
            else {
                return Outcome::skip();
            };
            if to == value {
                return Outcome::skip();
            }
            edit(parent, i, applied, cx, |s| {
                set_text(s, to);
                true
            })
        }
        Op::ToParentPayload => {
            if !matches!(parent.payload, Value::None) {
                return Outcome::skip();
            }
            let value = payload_str(&node).trim().to_string();
            if value.is_empty() || !node.substructures.is_empty() {
                return Outcome::skip();
            }
            set_text(parent, &value);
            noted(parent, i, applied, cx, Action::Replaced);
            parent.substructures.remove(i);
            Outcome {
                slot: 0,
                ..Outcome::skip()
            }
        }
        Op::TimeIntoDate => {
            if !v7 {
                return Outcome::skip();
            }
            let Some(date) = child_mut(parent, "DATE") else {
                return Outcome::skip();
            };
            if has(date, "TIME") {
                return Outcome::skip();
            }
            let mut time = node.clone();
            retag(&mut time, "TIME");
            date.substructures.push(time);
            noted(parent, i, applied, cx, Action::Replaced);
            parent.substructures.remove(i);
            Outcome {
                slot: 0,
                ..Outcome::skip()
            }
        }
        Op::Into(container) => {
            let moved = parent.substructures.remove(i);
            let tag = moved.tag.as_str().to_string();
            let line = moved.line;
            if let Some(c) = child_mut(parent, container) {
                c.substructures.push(moved);
            } else {
                let mut c = Draft::new(container);
                c.substructures.push(moved);
                parent.substructures.push(c);
            }
            cx.report
                .note(&applied, &tag, Action::Replaced, line, cx.record.as_ref());
            Outcome {
                slot: 0,
                ..Outcome::skip()
            }
        }
        Op::Up => lift(parent, i, vec![node], applied, cx),
        Op::Residence => {
            let mut resi = Draft::new("RESI");
            let mut addr = node.clone();
            retag(&mut addr, "ADDR");
            resi.substructures.push(addr);
            place(parent, i, vec![resi], applied, cx)
        }
        Op::PlaceAsValue => {
            if !matches!(parent.payload, Value::None) {
                return Outcome::skip();
            }
            let value = payload_str(&node).trim().to_string();
            if value.is_empty() {
                return Outcome::skip();
            }
            set_text(parent, &value);
            noted(parent, i, applied, cx, Action::Added);
            Outcome::skip()
        }
        Op::SecondType => {
            let types = parent
                .substructures
                .iter()
                .filter(|c| is(c, "TYPE"))
                .count();
            let first_type = parent.substructures.iter().position(|c| is(c, "TYPE"));
            if types != 2 || !matches!(parent.payload, Value::None) || first_type == Some(i) {
                return Outcome::skip();
            }
            let value = payload_str(&node).trim().to_string();
            set_text(parent, &value);
            noted(parent, i, applied, cx, Action::Replaced);
            parent.substructures.remove(i);
            Outcome {
                slot: 0,
                ..Outcome::skip()
            }
        }
        Op::EvenType(map) => {
            let Some(ty) = child_text(&node, "TYPE").map(str::trim) else {
                return Outcome::skip();
            };
            let Some(&(_, tag)) = map.iter().find(|(w, _)| w.eq_ignore_ascii_case(ty)) else {
                return Outcome::skip();
            };
            if !matches!(node.payload, Value::None) {
                return Outcome::skip();
            }
            edit(parent, i, applied, cx, |s| {
                retag(s, tag);
                if let Some(at) = s.substructures.iter().position(|c| is(c, "TYPE")) {
                    s.substructures.remove(at);
                }
                true
            })
        }
        Op::Dedupe => {
            if parent.substructures.iter().take(i).any(|c| *c == node) {
                noted(parent, i, applied, cx, Action::Replaced);
                parent.substructures.remove(i);
                return Outcome {
                    slot: 0,
                    ..Outcome::skip()
                };
            }
            Outcome::skip()
        }
        Op::FamilyWitness => {
            if !v7 {
                return Outcome::skip();
            }
            let (Some(Value::Pointer(family)), Some(person)) =
                (Some(node.payload.clone()), cx.record.clone())
            else {
                return Outcome::skip();
            };
            if !child_text(&node, "TYPE").is_some_and(|t| t.trim().eq_ignore_ascii_case("FAM")) {
                return Outcome::skip();
            }
            let word = child_text(&node, "RELA")
                .or_else(|| child(&node, "ROLE").and_then(|r| child_text(r, "PHRASE")))
                .or_else(|| child_text(&node, "ROLE"))
                .unwrap_or("witness");
            let (role, phrase) = role_of(word);
            let mut assoc = pointer_node("ASSO", &person);
            let mut role_node = text_node("ROLE", role);
            if let Some(p) = phrase {
                role_node.substructures.push(text_node("PHRASE", &p));
            }
            assoc.substructures.push(role_node);
            assoc.substructures.extend(
                node.substructures
                    .iter()
                    .filter(|s| !matches!(s.tag.as_str(), "TYPE" | "RELA" | "ROLE"))
                    .cloned(),
            );
            cx.deferred.push((
                applied,
                Deferred::Witness {
                    family: Xref::new(&*family),
                    assoc: assoc.into_owned(),
                },
            ));
            Outcome::skip()
        }
        Op::Adoption => {
            let (Some(child), Some(famc)) = (cx.record.clone(), child(&node, "FAMC")) else {
                return Outcome::skip();
            };
            let Value::Pointer(family) = famc.payload.clone() else {
                return Outcome::skip();
            };
            cx.deferred.push((
                applied,
                Deferred::Adopted {
                    child,
                    family: Xref::new(&*family),
                },
            ));
            Outcome::skip()
        }
        Op::NegativeYear => {
            let value = payload_str(&node).to_string();
            let Some(fixed) = negative_years(&value, v7) else {
                return Outcome::skip();
            };
            edit(parent, i, applied, cx, |s| {
                set_text(s, &fixed);
                true
            })
        }
        Op::DateWords => date_words(parent, i, applied, path, cx),
        Op::AgeWords => {
            let value = payload_str(&node).to_string();
            let Some(fixed) = age_words(&value) else {
                return Outcome::skip();
            };
            edit(parent, i, applied, cx, |s| {
                set_text(s, &fixed);
                if v7 && !has(s, "PHRASE") {
                    s.substructures.push(text_node("PHRASE", value.trim()));
                }
                true
            })
        }
        Op::SurnameAliases => {
            let Some(name) = path
                .last()
                .filter(|t| t.as_str() == "NAME")
                .map(|_| parent.clone())
            else {
                return Outcome::skip();
            };
            let value = payload_str(&name).to_string();
            let main = surname_part(&value).unwrap_or_default().to_string();
            let given = given_part(&value).to_string();
            let mut new = Vec::new();
            for alias in payload_str(&node).split(',').map(str::trim) {
                if alias.is_empty() || alias.eq_ignore_ascii_case(&main) {
                    continue;
                }
                let mut n = text_node("NAME", &name_value(&given, alias, ""));
                n.substructures.push(NameKind::Aka.type_node(v7));
                new.push(n);
            }
            if new.is_empty() {
                return Outcome::skip();
            }
            noted(parent, i, applied, cx, Action::Added);
            Outcome {
                lift: new,
                ..Outcome::skip()
            }
        }
        Op::PublicName => {
            let value = payload_str(parent).to_string();
            let public = payload_str(&node).trim().to_string();
            let given = given_part(&value);
            if public.is_empty() || public == given {
                return Outcome::skip();
            }
            let surname = surname_part(&value).unwrap_or_default();
            let mut n = text_node("NAME", &name_value(&public, surname, suffix_part(&value)));
            n.substructures.push(NameKind::Aka.type_node(v7));
            noted(parent, i, applied, cx, Action::Added);
            Outcome {
                lift: vec![n],
                ..Outcome::skip()
            }
        }
        Op::AliasNames => {
            let first = parent.substructures.iter().position(|c| is(c, "NAME"));
            if first == Some(i) || has(&node, "TYPE") {
                return Outcome::skip();
            }
            edit(parent, i, applied, cx, |s| {
                s.substructures.insert(0, NameKind::Aka.type_node(v7));
                true
            })
        }
        Op::Unmarried => {
            if !v7
                || !child_text(&node, "TYPE")
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("unmarried"))
                || has(parent, "MARR")
                || parent
                    .substructures
                    .iter()
                    .any(|c| is(c, "NO") && payload_str(c) == "MARR")
            {
                return Outcome::skip();
            }
            place(parent, i, vec![text_node("NO", "MARR")], applied, cx)
        }
        Op::RepositoryRecord => {
            let Value::Text(name) = node.payload.clone() else {
                return Outcome::skip();
            };
            let name = name.trim().to_string();
            if name.is_empty() {
                return Outcome::skip();
            }
            let mut record = Draft::new("REPO");
            record.substructures.push(text_node("NAME", &name));
            // The identifier is given when the record is placed.
            let mut link = node.clone();
            link.payload = Value::Pointer(Cow::Borrowed(PLACEHOLDER));
            cx.deferred
                .push((applied, Deferred::Record(record.into_owned())));
            place(parent, i, vec![link], applied, cx)
        }
        Op::RecordXref => {
            let Value::Pointer(x) = node.payload.clone() else {
                return Outcome::skip();
            };
            if node.xref.is_some() || !path.is_empty() {
                return Outcome::skip();
            }
            edit(parent, i, applied, cx, |s| {
                s.xref = Some(x);
                s.payload = Value::None;
                true
            })
        }
        Op::LongForm => {
            let Some(short) = long_form(node.tag.as_str()) else {
                return Outcome::skip();
            };
            edit(parent, i, applied, cx, |s| {
                retag(s, short);
                true
            })
        }
        Op::SchemaFact => {
            let record = path.first().map_or("", DraftTag::as_str);
            let Some(label) = cx
                .schema
                .iter()
                .find(|(r, t, _)| r == record && t == node.tag.as_str())
                .map(|(_, _, l)| l.clone())
            else {
                return Outcome::skip();
            };
            let mut new = node.clone();
            retag(&mut new, "EVEN");
            if !has(&new, "TYPE") {
                new.substructures.insert(0, text_node("TYPE", &label));
            }
            place(parent, i, vec![new], applied, cx)
        }
    }
}

/// Whether `list` holds `s`, or `s` as a writer may have written it: its
/// tags made extensions (`_NAME`), its values in the other version's case
/// (`TYPE aka`, `TYPE AKA`), its substructures in another order, and
/// perhaps with more substructures (a name holding its own `_AKA`).
fn holds<'a, 's: 'a>(list: impl IntoIterator<Item = &'a Draft<'s>>, s: &Draft<'_>) -> bool {
    fn tag<'t>(t: &'t DraftTag<'_>) -> &'t str {
        t.as_str().trim_start_matches('_')
    }
    fn similar(a: &Draft<'_>, b: &Draft<'_>) -> bool {
        tag(&a.tag) == tag(&b.tag)
            && match (a.payload.as_str(), b.payload.as_str()) {
                // A date the 5.5.1 writer made a phrase, `(…)`.
                (Some(x), Some(y)) if tag(&a.tag) == "DATE" => x
                    .trim_matches(['(', ')'])
                    .eq_ignore_ascii_case(y.trim_matches(['(', ')'])),
                (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
                (x, y) => x == y,
            }
            && a.substructures.len() >= b.substructures.len()
            && {
                // In any order: the writer orders substructures its way.
                let mut used = vec![false; a.substructures.len()];
                b.substructures.iter().all(|y| {
                    let found = a
                        .substructures
                        .iter()
                        .enumerate()
                        .find(|(k, x)| !used.get(*k).copied().unwrap_or(true) && similar(x, y));
                    match found {
                        Some((k, _)) => {
                            if let Some(u) = used.get_mut(k) {
                                *u = true;
                            }
                            true
                        }
                        None => false,
                    }
                })
            }
    }
    list.into_iter().any(|c| similar(c, s))
}

/// Whether `parent` holds a `tag`, or one the writer's repair made an
/// extension of (`_CHAN` + `DATE`): applying again must not add another.
fn said(parent: &Draft<'_>, tag: &str) -> bool {
    parent.substructures.iter().any(|c| {
        is(c, tag) || (c.tag.as_str().strip_prefix('_') == Some(tag) && !c.substructures.is_empty())
    })
}

fn noted_tag<'s>(node: &Draft<'s>, applied: Applied, cx: &mut Cx<'_, 's>, action: Action) {
    let tag = node.tag.as_str().to_string();
    cx.report
        .note(&applied, &tag, action, node.line, cx.record.as_ref());
}

/// A name under `NAME` (or a name-like structure under `INDI`) as a `NAME`.
fn name<'s>(
    parent: &mut Draft<'s>,
    i: usize,
    applied: Applied,
    path: &[DraftTag<'s>],
    cx: &mut Cx<'_, 's>,
    kind: NameKind,
    how: NameValue,
) -> Outcome<'s> {
    let Some(node) = parent.substructures.get(i).cloned() else {
        return Outcome::skip();
    };
    let Value::Text(value) = &node.payload else {
        return Outcome::skip();
    };
    let value = value.trim();
    if value.is_empty() {
        return Outcome::skip();
    }
    let values: Vec<String> = match how {
        NameValue::AsIs => vec![value.to_string()],
        NameValue::Surname if value.contains('/') => vec![value.to_string()],
        NameValue::Surname => vec![name_value("", value, "")],
        NameValue::List => {
            let parts: Vec<&str> = value
                .split([',', ';'])
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .collect();
            // Several whole names; a comma inside one name
            // (`Doe, John`) is no list.
            if parts.len() > 1 && parts.iter().all(|p| p.contains('/')) {
                parts.into_iter().map(str::to_string).collect()
            } else {
                vec![value.to_string()]
            }
        }
    };
    let new: Vec<Draft<'s>> = values
        .iter()
        .map(|v| {
            let mut n = text_node("NAME", v);
            n.substructures.push(kind.type_node(cx.v7));
            n.substructures.extend(node.substructures.iter().cloned());
            n
        })
        .collect();
    if path.last().is_some_and(|t| t.as_str() == "NAME") {
        lift(parent, i, new, applied, cx)
    } else {
        place(parent, i, new, applied, cx)
    }
}

/// HTML entities undone (`&lt;` as `<`), once.
fn unescape_html(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", "\u{a0}")
        .replace("&amp;", "&")
}

/// The 7.x role a word names, and the word as its phrase when it says more.
fn role_of(word: &str) -> (&'static str, Option<String>) {
    let w = word.trim();
    let lower = w.to_ascii_lowercase();
    let lower = lower.trim_end_matches(|c: char| c == '_' || c.is_ascii_digit());
    let role = match lower {
        "witness"
        | "witn"
        | "witness_of_marriage"
        | "witness of marriage"
        | "temoin"
        | "témoin"
        | "zeuge"
        | "getuige" => "WITN",
        "godparent" | "godp" | "godfather" | "godmother" | "god father" | "god mother" | "godf"
        | "godm" | "parrain" | "marraine" | "pate" | "patin" | "pater" => "GODP",
        "officiator" | "official" | "officiant" | "civil officer" | "registry officer"
        | "officier" => "OFFICIATOR",
        "clergy" | "priest" | "pastor" | "minister" | "religious officer" | "curé" | "cure" => {
            "CLERGY"
        }
        "friend" => "FRIEND",
        "neighbor" | "neighbour" | "nghbr" => "NGHBR",
        "father" | "fath" => "FATH",
        "mother" | "moth" => "MOTH",
        "parent" => "PARENT",
        "child" | "chil" => "CHIL",
        "husband" | "husb" => "HUSB",
        "wife" => "WIFE",
        "spouse" | "spou" => "SPOU",
        _ => "OTHER",
    };
    let phrase = (!w.is_empty() && !w.eq_ignore_ascii_case(role)).then(|| w.to_string());
    (role, phrase)
}

/// Rewrites the role of an association by a producer's words.
fn role_words(assoc: &mut Draft<'_>, words: &[(&str, &str, &str)], v7: bool) -> bool {
    if !v7 {
        return false;
    }
    let Some(role) = child_mut(assoc, "ROLE") else {
        return false;
    };
    // Only a role a standard value does not say yet: `OTHER` + the
    // producer's word (as conversion writes a 5.5.1 `RELA`).
    let word = match (payload_str(role), child_text(role, "PHRASE")) {
        ("OTHER", Some(p)) => p.trim().to_string(),
        _ => return false,
    };
    let Some(&(_, to, phrase_to)) = words.iter().find(|(w, _, _)| w.eq_ignore_ascii_case(&word))
    else {
        return false;
    };
    if to == "OTHER" {
        return false;
    }
    set_text(role, to);
    role.substructures.retain(|s| !is(s, "PHRASE"));
    if !phrase_to.is_empty() {
        role.substructures.push(text_node("PHRASE", phrase_to));
    }
    true
}

/// `PEDI` for a pedigree word, as the version spells it.
fn pedigree_word(word: &str, v7: bool) -> Option<(&'static str, bool)> {
    let w = word.trim().to_ascii_lowercase();
    let (v7_word, v551) = if w.starts_with("natural")
        || w.starts_with("birth")
        || w.starts_with("biological")
        || w.starts_with("legitimate")
        || w == "natur"
    {
        ("BIRTH", "birth")
    } else if w.starts_with("adopt") {
        ("ADOPTED", "adopted")
    } else if w.starts_with("foster") {
        ("FOSTER", "foster")
    } else if w.starts_with("sealing") || w.starts_with("sealed") {
        ("SEALING", "sealing")
    } else if v7 {
        ("OTHER", "")
    } else {
        return None;
    };
    // Whether the word says more than the value.
    let exact = w == v551 || w == "natural" || w == "adopted" || w == "foster";
    Some((if v7 { v7_word } else { v551 }, exact))
}

fn pedigree_of<'s>(word: &str, v7: bool) -> Option<Draft<'s>> {
    // A text of several lines is no pedigree word (the writer's repair of
    // a `PEDI` that was not one, kept as `_PEDI`).
    if word.contains('\n') {
        return None;
    }
    let first = word
        .trim()
        .split(|c: char| c == '_' || c.is_whitespace())
        .next()
        .unwrap_or_default();
    let (value, exact) = pedigree_word(first, v7)?;
    let mut pedi = text_node("PEDI", value);
    if v7 && !(exact && word.trim().eq_ignore_ascii_case(first)) {
        pedi.substructures.push(text_node("PHRASE", word.trim()));
    }
    Some(pedi)
}

/// `_FREL`/`_MREL`: `PEDI` when both say the same.
fn pedigree<'s>(
    parent: &mut Draft<'s>,
    i: usize,
    applied: Applied,
    path: &[DraftTag<'s>],
    cx: &mut Cx<'_, 's>,
) -> Outcome<'s> {
    let Some(node) = parent.substructures.get(i) else {
        return Outcome::skip();
    };
    // Only the `_FREL` decides, once for the pair.
    if node.tag.as_str() != "_FREL" {
        return Outcome::skip();
    }
    let (Some(f), Some(m)) = (child_text(parent, "_FREL"), child_text(parent, "_MREL")) else {
        return Outcome::skip();
    };
    if !f.trim().eq_ignore_ascii_case(m.trim()) {
        return Outcome::skip();
    }
    let Some(pedi) = pedigree_of(f, cx.v7) else {
        return Outcome::skip();
    };
    match path.last().map(DraftTag::as_str) {
        Some("FAMC") => {
            if has(parent, "PEDI") {
                return Outcome::skip();
            }
            parent.substructures.push(pedi);
            noted(parent, i, applied, cx, Action::Added);
        }
        Some("CHIL") => {
            let (Some(family), Value::Pointer(child)) = (cx.record.clone(), parent.payload.clone())
            else {
                return Outcome::skip();
            };
            cx.deferred.push((
                applied,
                Deferred::Pedigree {
                    child: Xref::new(&*child),
                    family,
                    pedigree: pedi.into_owned(),
                },
            ));
        }
        _ => {}
    }
    Outcome::skip()
}

/// `_PHOTO @M1@`, `_WT_OBJE_SORT @M1@`: the multimedia links in that order.
fn prefer_pointer<'s>(
    parent: &mut Draft<'s>,
    i: usize,
    applied: Applied,
    cx: &mut Cx<'_, 's>,
) -> Outcome<'s> {
    let Some(node) = parent.substructures.get(i).cloned() else {
        return Outcome::skip();
    };
    let Value::Pointer(target) = &node.payload else {
        return Outcome::skip();
    };
    // The order of the pointers of this tag before this one.
    let rank = parent
        .substructures
        .iter()
        .take(i)
        .filter(|c| c.tag == node.tag)
        .count();
    let objes: Vec<usize> = parent
        .substructures
        .iter()
        .enumerate()
        .filter(|(_, c)| is(c, "OBJE"))
        .map(|(k, _)| k)
        .collect();
    let link = objes.iter().copied().find(|&k| {
        parent
            .substructures
            .get(k)
            .is_some_and(|c| c.pointer() == Some(target))
    });
    let dest = objes.get(rank).copied().or(objes.last().map(|l| l + 1));
    match (link, dest) {
        (Some(from), Some(to)) if from > to => {
            let moved = parent.substructures.remove(from);
            parent.substructures.insert(to, moved);
        }
        (Some(_), _) => {}
        // A link the writer's repair kept as `_OBJE`: nothing to add.
        (None, _)
            if parent
                .substructures
                .iter()
                .any(|c| is(c, "_OBJE") && c.pointer() == Some(target)) =>
        {
            cx.settled = true;
            return Outcome::skip();
        }
        (None, dest) => {
            // No link: one is added where it ranks.
            let at = dest.unwrap_or(i);
            parent.substructures.insert(
                at.min(parent.substructures.len()),
                pointer_node("OBJE", target),
            );
            if at <= i {
                // The pointer moved one place down.
                return finish_prefer(parent, i + 1, applied, cx);
            }
        }
    }
    finish_prefer(parent, i, applied, cx)
}

fn finish_prefer<'s>(
    parent: &mut Draft<'s>,
    i: usize,
    applied: Applied,
    cx: &mut Cx<'_, 's>,
) -> Outcome<'s> {
    noted(parent, i, applied, cx, Action::Reordered);
    if cx.v7 && applied.rule.fidelity == Fidelity::Lossless {
        parent.substructures.remove(i);
        return Outcome {
            slot: 0,
            ..Outcome::skip()
        };
    }
    Outcome::skip()
}

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

fn days_in(month: usize, year: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A date-and-time stamp (`12 JAN 2015 10:20:30 GMT -0500`,
/// `2018-07-19 14:19:00`): the date and the time, in UTC in 7.x when the
/// zone is given.
fn stamp(text: &str, v7: bool) -> Option<(String, Option<String>)> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let (mut day, mut month, mut year, rest): (i64, usize, i64, &[&str]) = if let Some(iso) = tokens
        .first()
        .filter(|t| t.len() == 10 && t.as_bytes().get(4) == Some(&b'-'))
    {
        let mut p = iso.split('-').map(|n| n.parse::<i64>().ok());
        let (y, m, d) = (p.next()??, p.next()??, p.next()??);
        (
            d,
            usize::try_from(m).ok()?,
            y,
            tokens.get(1..).unwrap_or_default(),
        )
    } else {
        let d = tokens.first()?.parse::<i64>().ok()?;
        let m = tokens.get(1)?.to_ascii_uppercase();
        let m = MONTHS.iter().position(|x| m.starts_with(x))? + 1;
        let y = tokens.get(2)?.parse::<i64>().ok()?;
        (d, m, y, tokens.get(3..).unwrap_or_default())
    };
    if !(1..=12).contains(&month) || day < 1 || day > days_in(month, year) {
        return None;
    }
    let date = |d: i64, m: usize, y: i64| {
        format!("{d} {} {y}", MONTHS.get(m - 1).copied().unwrap_or("JAN"))
    };
    let Some(time) = rest.first() else {
        return Some((date(day, month, year), None));
    };
    let mut hms = time.split(':').map(|n| n.parse::<i64>().ok());
    let (h, mi) = (hms.next()??, hms.next()??);
    let s = hms.next().flatten().unwrap_or(0);
    if h > 23 || mi > 59 || s > 59 {
        return None;
    }
    // The zone: `GMT -0500`, `GMT-5`, `GMT+11`, `-0500`, `UTC`, `Z`.
    let zone: String = rest.get(1..).unwrap_or_default().concat();
    let zone = zone
        .trim_start_matches(|c: char| c.is_ascii_alphabetic())
        .to_string();
    let offset = if zone.is_empty() {
        rest.get(1).map(|_| 0)
    } else {
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let digits: String = zone.chars().filter(char::is_ascii_digit).collect();
        let minutes = match digits.len() {
            1 | 2 => digits.parse::<i64>().ok()? * 60,
            3 | 4 => {
                let n = digits.parse::<i64>().ok()?;
                (n / 100) * 60 + n % 100
            }
            _ => return None,
        };
        Some(sign * minutes)
    };
    match (v7, offset) {
        (true, Some(offset)) => {
            let mut minutes = h * 60 + mi - offset;
            if minutes < 0 {
                minutes += 24 * 60;
                day -= 1;
                if day < 1 {
                    month = if month == 1 {
                        year -= 1;
                        12
                    } else {
                        month - 1
                    };
                    day = days_in(month, year);
                }
            } else if minutes >= 24 * 60 {
                minutes -= 24 * 60;
                day += 1;
                if day > days_in(month, year) {
                    day = 1;
                    month = if month == 12 {
                        year += 1;
                        1
                    } else {
                        month + 1
                    };
                }
            }
            let t = format!("{:02}:{:02}:{s:02}Z", minutes / 60, minutes % 60);
            Some((date(day, month, year), Some(t)))
        }
        _ => Some((
            date(day, month, year),
            Some(format!("{h:02}:{mi:02}:{s:02}")),
        )),
    }
}

/// A 7.x `CROP` from a producer's rectangle.
fn crop_of<'s>(node: &Draft<'s>, form: CropForm) -> Option<Draft<'s>> {
    let (left, top, width, height) = match form {
        CropForm::Ancestry => {
            let number =
                |tag: &str| child_text(node, tag).and_then(|v| v.trim().parse::<u32>().ok());
            (
                number("_LEFT")?,
                number("_TOP")?,
                number("_WDTH")?,
                number("_HGHT")?,
            )
        }
        CropForm::Area => {
            let area = payload_str(node).trim();
            let area = area.strip_prefix('{')?.strip_suffix('}')?;
            let mut sides = area.split(',').map(|side| side.trim().parse::<u32>().ok());
            let (left, top, right, bottom) = (
                sides.next()??,
                sides.next()??,
                sides.next()??,
                sides.next()??,
            );
            (
                left,
                top,
                right.checked_sub(left)?,
                bottom.checked_sub(top)?,
            )
        }
    };
    let mut crop = Draft::new("CROP");
    for (tag, n) in [
        ("TOP", top),
        ("LEFT", left),
        ("HEIGHT", height),
        ("WIDTH", width),
    ] {
        crop.substructures.push(text_node(tag, &n.to_string()));
    }
    Some(crop)
}

/// A date with its negative years written as the version writes years
/// before the epoch: `None` when it has none.
fn negative_years(value: &str, v7: bool) -> Option<String> {
    let mut changed = false;
    let words: Vec<String> = value
        .split_whitespace()
        .map(|w| {
            let digits = w
                .strip_prefix('-')
                .filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
            match digits {
                Some(d) => {
                    changed = true;
                    format!("{d} {}", if v7 { "BCE" } else { "B.C." })
                }
                None => w.to_string(),
            }
        })
        .collect();
    changed.then(|| words.join(" "))
}

/// Date words of the dialects the catalogue lists, as the grammar spells
/// them.
const DATE_WORDS: &[(&str, &str)] = &[
    ("ABT.", "ABT"),
    ("ABOUT", "ABT"),
    ("CIR", "ABT"),
    ("CIR.", "ABT"),
    ("CIRCA", "ABT"),
    ("CA", "ABT"),
    ("CA.", "ABT"),
    ("APX", "ABT"),
    ("APPROX", "ABT"),
    ("ENV", "ABT"),
    ("ENV.", "ABT"),
    ("VERS", "ABT"),
    ("BEF.", "BEF"),
    ("BEFORE", "BEF"),
    ("AVANT", "BEF"),
    ("AFT.", "AFT"),
    ("AFTER", "AFT"),
    ("APRES", "AFT"),
    ("APRÈS", "AFT"),
    ("BET.", "BET"),
    ("BETWEEN", "BET"),
    ("ENTRE", "BET"),
    ("ET", "AND"),
    ("EST.", "EST"),
    ("CAL.", "CAL"),
    ("INT.", "INT"),
    ("FROM.", "FROM"),
    ("SEPT", "SEP"),
    ("BC", "B.C."),
    ("VENDEMIAIRE", "VEND"),
    ("VENDÉMIAIRE", "VEND"),
    ("BRUMAIRE", "BRUM"),
    ("FRIMAIRE", "FRIM"),
    ("NIVOSE", "NIVO"),
    ("NIVÔSE", "NIVO"),
    ("PLUVIOSE", "PLUV"),
    ("PLUVIÔSE", "PLUV"),
    ("VENTOSE", "VENT"),
    ("VENTÔSE", "VENT"),
    ("GERMINAL", "GERM"),
    ("FLOREAL", "FLOR"),
    ("FLORÉAL", "FLOR"),
    ("PRAIRIAL", "PRAI"),
    ("MESSIDOR", "MESS"),
    ("THERMIDOR", "THER"),
    ("FRUCTIDOR", "FRUC"),
];

/// The English month a word names, in full or abbreviated (any case
/// given upper-cased), as the grammar spells it.
fn month_of(word: &str) -> Option<&'static str> {
    const FULL: [&str; 12] = [
        "JANUARY",
        "FEBRUARY",
        "MARCH",
        "APRIL",
        "MAY",
        "JUNE",
        "JULY",
        "AUGUST",
        "SEPTEMBER",
        "OCTOBER",
        "NOVEMBER",
        "DECEMBER",
    ];
    MONTHS
        .iter()
        .zip(FULL)
        .find(|(short, full)| word == **short || word == *full)
        .map(|(short, _)| *short)
}

/// A Roman numeral's value (the years of the French Republic).
fn roman(word: &str) -> Option<u32> {
    let mut total = 0_u32;
    let mut prev = 0_u32;
    for c in word.chars().rev() {
        let v = match c.to_ascii_uppercase() {
            'I' => 1,
            'V' => 5,
            'X' => 10,
            'L' => 50,
            _ => return None,
        };
        if v < prev {
            total = total.checked_sub(v)?;
        } else {
            total += v;
            prev = v;
        }
    }
    (total > 0).then_some(total)
}

/// Words of the date grammar that no dialect rewrites ([`plain_date`]
/// matches them).
#[cfg(test)]
const PLAIN_DATE_WORDS: &[&str] = &[
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC", "ABT",
    "AFT", "BEF", "BET", "AND", "FROM", "TO", "CAL", "EST",
];

/// Whether `date` (trimmed) is plain: numbers and [`PLAIN_DATE_WORDS`]
/// separated by single spaces, which [`date_dialect`] leaves as they are
/// (most dates: the quick answer).
fn plain_date(date: &str) -> bool {
    date.split(' ').all(|w| {
        !w.is_empty()
            && (w.bytes().all(|b| b.is_ascii_digit())
                // (The words of `PLAIN_DATE_WORDS`, without a search.)
                || matches!(
                    w,
                    "JAN" | "FEB" | "MAR" | "APR" | "MAY" | "JUN" | "JUL" | "AUG" | "SEP"
                        | "OCT" | "NOV" | "DEC" | "ABT" | "AFT" | "BEF" | "BET" | "AND"
                        | "FROM" | "TO" | "CAL" | "EST"
                ))
    })
}

/// A date in a dialect, in the grammar's words: `None` when unchanged.
fn date_dialect(value: &str, v7: bool) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.starts_with('(') || plain_date(trimmed) {
        return None;
    }
    // TMG's `EITHER 1850 OR 1851`.
    let upper = trimmed.to_uppercase();
    if let Some(rest) = upper.strip_prefix("EITHER ") {
        let (a, b) = rest.split_once(" OR ")?;
        // 5.5.1 has no interpreted range: it stays a phrase there.
        return v7.then(|| format!("BET {} AND {}", a.trim(), b.trim()));
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut k = 0;
    while let Some(w) = words.get(k) {
        let up = w.to_uppercase();
        // French Republican `AN II`.
        if up == "AN"
            && let Some(n) = words
                .get(k + 1)
                .and_then(|n| roman(n).or_else(|| n.parse().ok()))
        {
            out.push(n.to_string());
            k += 2;
            continue;
        }
        if let Some(&(_, to)) = DATE_WORDS.iter().find(|(from, _)| *from == up) {
            out.push(to.to_string());
        } else if let Some(m) = month_of(&up) {
            // Months in any case, in full (`July`) or not.
            out.push(m.to_string());
        } else {
            out.push((*w).to_string());
        }
        k += 1;
    }
    let fixed = out.join(" ");
    (fixed != trimmed).then_some(fixed)
}

fn date_words<'s>(
    parent: &mut Draft<'s>,
    i: usize,
    applied: Applied,
    path: &[DraftTag<'s>],
    cx: &mut Cx<'_, 's>,
) -> Outcome<'s> {
    let v7 = cx.v7;
    let Some(node) = parent.substructures.get(i).cloned() else {
        return Outcome::skip();
    };
    let value = payload_str(&node).to_string();
    // PAF's `DEAT.DATE Stillborn`: an age.
    if value.trim().eq_ignore_ascii_case("stillborn")
        && path.last().is_some_and(|t| t.as_str() == "DEAT")
        && !has(parent, "AGE")
    {
        let age = if v7 {
            let mut a = text_node("AGE", "0y");
            a.substructures.push(text_node("PHRASE", "Stillborn"));
            a
        } else {
            text_node("AGE", "STILLBORN")
        };
        return place(parent, i, vec![age], applied, cx);
    }
    let Some(fixed) = date_dialect(&value, v7) else {
        return Outcome::skip();
    };
    // Only a date the grammar reads is worth the rewrite.
    let version = if v7 {
        GedcomVersion::V7_0
    } else {
        GedcomVersion::V5_5_1
    };
    if DateValue::parse_strict(&fixed, version).is_err() {
        return Outcome::skip();
    }
    edit(parent, i, applied, cx, |s| {
        set_text(s, &fixed);
        if v7 && !has(s, "PHRASE") && !fixed.eq_ignore_ascii_case(value.trim()) {
            s.substructures.push(text_node("PHRASE", value.trim()));
        }
        true
    })
}

/// An age in a dialect as the grammar writes it: `None` when unchanged or
/// unreadable.
fn age_words(value: &str) -> Option<String> {
    let v = value.trim();
    let mut out = Vec::new();
    // `65-10-25`: years, months, days.
    if v.contains('-') && v.split('-').all(|p| p.trim().parse::<u32>().is_ok()) {
        for (n, unit) in v.split('-').zip(["y", "m", "d"]) {
            let n: u32 = n.trim().parse().ok()?;
            if n > 0 || unit == "y" {
                out.push(format!("{n}{unit}"));
            }
        }
        let fixed = out.join(" ");
        return (fixed != v).then_some(fixed);
    }
    let lower = v.to_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    let mut k = 0;
    let mut prefix = String::new();
    if let Some(p) = tokens.first().filter(|t| matches!(**t, "<" | ">")) {
        prefix = format!("{p} ");
        k = 1;
    }
    let unit_of =
        |u: &str| -> Option<char> {
            Some(match u {
                "y" | "yr" | "yrs" | "year" | "years" | "j" | "jahr" | "jahre" | "an" | "ans"
                | "a" => 'y',
                "m" | "mo" | "month" | "months" | "mois" | "monat" | "monate" => 'm',
                "w" | "wk" | "week" | "weeks" | "sem" | "semaine" | "semaines" | "woche"
                | "wochen" => 'w',
                "d" | "day" | "days" | "t" | "tag" | "tage" | "j." | "jour" | "jours" => 'd',
                _ => return None,
            })
        };
    while let Some(t) = tokens.get(k) {
        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        let rest = t.get(digits.len()..).unwrap_or_default();
        let unit = if rest.is_empty() {
            match tokens.get(k + 1).and_then(|u| unit_of(u)) {
                Some(u) => {
                    k += 1;
                    u
                }
                // A bare number is years.
                None if tokens.len() == k + 1 => 'y',
                None => return None,
            }
        } else {
            unit_of(rest)?
        };
        out.push(format!("{digits}{unit}"));
        k += 1;
    }
    if out.is_empty() {
        return None;
    }
    let fixed = format!("{prefix}{}", out.join(" "));
    (fixed != v).then_some(fixed)
}

/// Family Tree Maker for Windows' `HEAD._SCHEMA` labels: (record, tag,
/// label).
fn schema_labels(records: &[Draft<'_>]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for head in records.iter().filter(|r| is(r, "HEAD")) {
        for schema in head.substructures.iter().filter(|s| is(s, "_SCHEMA")) {
            for record in &schema.substructures {
                for tag in &record.substructures {
                    if let Some(label) = child_text(tag, "LABL") {
                        out.push((
                            record.tag.as_str().to_string(),
                            tag.tag.as_str().to_string(),
                            label.trim().to_string(),
                        ));
                    }
                }
            }
        }
    }
    out
}

/// The changes that span records.
///
/// `taken`, when the records are some of a file's only (the records the
/// changes concern, read one at a time: [`Interpreter::finish`]), tells
/// the identifiers the file's other records have.
fn apply_deferred(
    records: &mut Vec<Draft<'_>>,
    deferred: Vec<(Applied, Deferred)>,
    report: &mut InterpretReport,
    v7: bool,
    taken: Option<&dyn Fn(&str) -> bool>,
) {
    let mut next = 0_u32;
    // Where each record is (the first of an identifier), and where the
    // next placeholder a new record answers can be (they wait in order).
    let mut index: HashMap<Xref, usize, BuildHasherDefault<TagHasher>> = HashMap::default();
    for (k, r) in records.iter().enumerate() {
        if let Some(x) = &r.xref {
            index.entry(Xref::new(&**x)).or_insert(k);
        }
    }
    let mut scan_from = 0;
    let find = |index: &HashMap<Xref, usize, BuildHasherDefault<TagHasher>>, x: &Xref| {
        index.get(x).copied()
    };
    for (applied, change) in deferred {
        let done = match change {
            Deferred::Pedigree {
                child,
                family,
                pedigree,
            } => find(&index, &child)
                .and_then(|at| records.get_mut(at))
                .and_then(|r| {
                    r.substructures
                        .iter_mut()
                        .find(|c| is(c, "FAMC") && c.pointer() == Some(&family))
                })
                .is_some_and(|famc| {
                    !has(famc, "PEDI") && {
                        famc.substructures.push(pedigree);
                        true
                    }
                }),
            Deferred::FamilyFirst {
                person,
                family,
                as_child,
            } => find(&index, &person)
                .and_then(|at| records.get_mut(at))
                .is_some_and(|r| {
                    let tag = if as_child { "FAMC" } else { "FAMS" };
                    match r
                        .substructures
                        .iter()
                        .position(|c| is(c, tag) && c.pointer() == Some(&family))
                    {
                        Some(at) => {
                            let before = r.substructures.clone();
                            promote_child(r, at);
                            before != r.substructures
                        }
                        None => false,
                    }
                }),
            Deferred::Witness { family, assoc } => find(&index, &family)
                .and_then(|at| records.get_mut(at))
                .and_then(|r| child_mut(r, "MARR"))
                .is_some_and(|marr| push_new(marr, assoc)),
            Deferred::Adopted { child, family } => {
                let child_at = find(&index, &child);
                let family_at = find(&index, &family);
                adopt(records, child_at, family_at, &child, &family, v7)
            }
            Deferred::Record(mut record) => {
                let xref = fresh_xref(
                    |x| index.contains_key(&Xref::new(x)) || taken.is_some_and(|t| t(x)),
                    "R",
                    &mut next,
                );
                record.xref = Some(Cow::Owned(xref.as_str().to_string()));
                // The link that waits for this identifier.
                let placeholder = Xref::new("@\u{0}@");
                let mut replaced = false;
                while let Some(r) = records.get_mut(scan_from) {
                    replace_pointer(r, &placeholder, &xref, &mut replaced);
                    if replaced {
                        break;
                    }
                    scan_from += 1;
                }
                let at = records
                    .iter()
                    .rposition(|r| !is(r, "TRLR"))
                    .map_or(records.len(), |k| k + 1);
                records.insert(at, record);
                if records.iter().skip(at + 1).any(|r| r.xref.is_some()) {
                    for x in index.values_mut() {
                        if *x >= at {
                            *x += 1;
                        }
                    }
                }
                index.insert(xref, at);
                true
            }
        };
        if done {
            let tag = applied.rule.tag;
            report.note(&applied, tag, Action::Added, 0, None);
        }
    }
}

/// The adopted child `child` (the record at `child_at`) linked to its
/// adoptive family `family` (at `family_at`) both ways: `FAMC` + `PEDI
/// adopted` and `CHIL`. Whether anything was added.
fn adopt(
    records: &mut [Draft<'_>],
    child_at: Option<usize>,
    family_at: Option<usize>,
    child: &Xref,
    family: &Xref,
    v7: bool,
) -> bool {
    let linked = child_at
        .and_then(|at| records.get_mut(at))
        .is_some_and(|r| {
            let pedi = text_node("PEDI", if v7 { "ADOPTED" } else { "adopted" });
            match r
                .substructures
                .iter_mut()
                .find(|c| is(c, "FAMC") && c.pointer() == Some(family))
            {
                Some(famc) if !has(famc, "PEDI") => {
                    famc.substructures.push(pedi);
                    true
                }
                Some(_) => false,
                None => {
                    let mut famc = pointer_node("FAMC", family);
                    famc.substructures.push(pedi);
                    let at = r
                        .substructures
                        .iter()
                        .rposition(|c| is(c, "FAMC"))
                        .map_or(r.substructures.len(), |k| k + 1);
                    r.substructures.insert(at, famc);
                    true
                }
            }
        });
    let back = family_at
        .and_then(|at| records.get_mut(at))
        .is_some_and(|f| {
            !f.substructures
                .iter()
                .any(|c| is(c, "CHIL") && c.pointer() == Some(child))
                && push_new(f, pointer_node("CHIL", child))
        });
    linked || back
}

fn replace_pointer(s: &mut Draft<'_>, from: &Xref, to: &Xref, done: &mut bool) {
    if *done {
        return;
    }
    if s.pointer() == Some(from.as_str()) {
        s.payload = Value::Pointer(Cow::Owned(to.as_str().to_string()));
        *done = true;
        return;
    }
    for c in &mut s.substructures {
        replace_pointer(c, from, to, done);
    }
}

/// Declares the extension tags kept in `HEAD.SCHMA` (7.x), unless the
/// header declares them already.
fn declare(records: &mut [Draft<'_>], tags: &BTreeSet<(Box<str>, String)>) -> bool {
    if tags.is_empty() {
        return false;
    }
    let Some(head) = records.iter_mut().find(|r| is(r, "HEAD")) else {
        return false;
    };
    let mut changed = false;
    let schma_at = if let Some(at) = head.substructures.iter().position(|s| is(s, "SCHMA")) {
        at
    } else {
        head.substructures.push(Draft::new("SCHMA"));
        head.substructures.len() - 1
    };
    let Some(schma) = head.substructures.get_mut(schma_at) else {
        return false;
    };
    for (tag, uri) in tags {
        let declared = schma.substructures.iter().any(|t| {
            t.tag.as_str().trim_matches('_') == "TAG"
                && payload_str(t)
                    .split_whitespace()
                    .next()
                    .is_some_and(|x| x == &**tag)
        });
        if !declared {
            schma
                .substructures
                .push(text_node("TAG", &format!("{tag} {uri}")));
            changed = true;
        }
    }
    if schma.substructures.is_empty() {
        head.substructures.remove(schma_at);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags<'s>(path: &[&'s str]) -> Vec<DraftTag<'s>> {
        path.iter().map(|t| DraftTag::new(t)).collect()
    }

    fn ctx_matches(ctx: &'static str, path: &[DraftTag<'_>]) -> bool {
        Context::parse(ctx).matches(path)
    }

    #[test]
    fn contexts() {
        assert!(ctx_matches("", &[]));
        assert!(!ctx_matches("", &tags(&["INDI"])));
        assert!(ctx_matches("INDI.NAME", &tags(&["INDI", "NAME"])));
        assert!(ctx_matches("INDI.*", &tags(&["INDI", "BIRT"])));
        assert!(!ctx_matches("INDI.*", &tags(&["INDI"])));
        assert!(ctx_matches("INDI|FAM.*", &tags(&["FAM", "MARR"])));
        assert!(ctx_matches("**.SOUR", &tags(&["INDI", "BIRT", "SOUR"])));
        assert!(ctx_matches("**", &tags(&[])));
        assert!(ctx_matches("**", &tags(&["A", "B"])));
    }

    /// What [`RuleSet::any_standard`] leaves out never applies to a
    /// standard tag.
    #[test]
    fn standard_tags_are_neither_long_forms_nor_folded() {
        for tag in STANDARD_TAGS {
            assert!(long_form(tag).is_none(), "{tag}");
            assert!(!Op::CaseFold.may_apply_to_parts(tag, PayloadRef::None, false, false, true));
        }
    }

    #[test]
    fn pedigree_words_are_one_line() {
        assert!(pedigree_of("adopted", false).is_some());
        assert!(pedigree_of("adopted\n1 more", false).is_none());
    }

    #[test]
    fn plain_dates_are_no_dialect() {
        for w in PLAIN_DATE_WORDS {
            assert!(plain_date(w), "{w}");
        }
        for w in PLAIN_DATE_WORDS {
            assert!(
                DATE_WORDS.iter().all(|(from, to)| from != w || to == w),
                "{w}"
            );
            assert!(month_of(w).is_none_or(|m| m == *w), "{w}");
        }
        assert!(plain_date("BET 1 JAN 1800 AND 2 FEB 1801"));
        assert!(!plain_date("1  JAN 1800"));
        assert!(!plain_date("1 Jan 1800"));
        assert_eq!(
            date_dialect("1  JAN 1800", false).as_deref(),
            Some("1 JAN 1800")
        );
        assert_eq!(date_dialect("ABT 1800", false), None);
    }

    /// The rules on standard tags read the tag where a line has it.
    #[test]
    fn lines_of_a_tag() {
        let text = "0 HEAD\r\n1 DATE 1 JAN 1800\r\n0 @N1@ NOTE has DATE inside\n2 DATE\n1 EDATE x\r3\tDATE\tabt 1800";
        let found: Vec<&str> = lines_of(text, "DATE").collect();
        assert_eq!(found, ["1 JAN 1800", "", "abt 1800"]);
    }

    #[test]
    fn stamps() {
        assert_eq!(
            stamp("12 JAN 2015 10:20:30 GMT -0500", true),
            Some(("12 JAN 2015".into(), Some("15:20:30Z".into())))
        );
        assert_eq!(
            stamp("31 DEC 2015 23:30:00 GMT-1", true),
            Some(("1 JAN 2016".into(), Some("00:30:00Z".into())))
        );
        assert_eq!(
            stamp("1 MAR 2016 00:10:00 GMT+1", true),
            Some(("29 FEB 2016".into(), Some("23:10:00Z".into())))
        );
        assert_eq!(
            stamp("12 JAN 2015 10:20:30 GMT -0500", false),
            Some(("12 JAN 2015".into(), Some("10:20:30".into())))
        );
        assert_eq!(
            stamp("2018-07-19 14:19:00", true),
            Some(("19 JUL 2018".into(), Some("14:19:00".into())))
        );
        assert_eq!(stamp("1 JAN 2015", true), Some(("1 JAN 2015".into(), None)));
        assert_eq!(stamp("31 FEB 2015", true), None);
        assert_eq!(stamp("soon", true), None);
    }

    #[test]
    fn dialects() {
        assert_eq!(date_dialect("ENV 1850", true).as_deref(), Some("ABT 1850"));
        assert_eq!(
            date_dialect("12 Sept 1850", false).as_deref(),
            Some("12 SEP 1850")
        );
        assert_eq!(
            date_dialect("4 Jul 2014", true).as_deref(),
            Some("4 JUL 2014")
        );
        assert_eq!(
            date_dialect("@#DFRENCH R@ 24 NIVOSE AN II", false).as_deref(),
            Some("@#DFRENCH R@ 24 NIVO 2")
        );
        assert_eq!(
            date_dialect("EITHER 1850 OR 1851", true).as_deref(),
            Some("BET 1850 AND 1851")
        );
        assert_eq!(date_dialect("ABT 1850", true), None);
        assert_eq!(date_dialect("JULIAN 1700", true), None);
        assert_eq!(
            date_dialect("31 March 1727", false).as_deref(),
            Some("31 MAR 1727")
        );
        assert_eq!(
            negative_years("BET -7 AND -5", true).as_deref(),
            Some("BET 7 BCE AND 5 BCE")
        );
        assert_eq!(
            negative_years("ABT -50", false).as_deref(),
            Some("ABT 50 B.C.")
        );
        assert_eq!(negative_years("1850", false), None);
        assert_eq!(age_words("65 y 10 m").as_deref(), Some("65y 10m"));
        assert_eq!(age_words("65-10-25").as_deref(), Some("65y 10m 25d"));
        assert_eq!(age_words("65J 10M 25T").as_deref(), Some("65y 10m 25d"));
        assert_eq!(age_words("11 mois").as_deref(), Some("11m"));
        assert_eq!(age_words("65").as_deref(), Some("65y"));
        assert_eq!(age_words("65y"), None);
        assert_eq!(age_words("CHILD"), None);
        assert_eq!(roman("XII"), Some(12));
        assert_eq!(roman("IV"), Some(4));
    }

    #[test]
    fn crops() {
        let mut area = text_node("_AREA", "{154,46,243,127}");
        let crop = crop_of(&area, CropForm::Area).unwrap();
        assert_eq!(
            crop.substructures
                .iter()
                .map(|s| format!("{} {}", s.tag, payload_str(s)))
                .collect::<Vec<_>>(),
            ["TOP 46", "LEFT 154", "HEIGHT 81", "WIDTH 89"]
        );
        set_text(&mut area, "{1,2,0,0}");
        assert!(crop_of(&area, CropForm::Area).is_none());
    }
}
