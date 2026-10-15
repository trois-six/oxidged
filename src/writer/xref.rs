//! Record identifiers of a written file: valid, unique and consistent with
//! the pointers.
//!
//! Before anything is written, [`XrefMap`] looks at every record's
//! identifier once, in write order:
//!
//! 1. the first record holding a valid identifier keeps it;
//! 2. a record whose identifier is invalid for the target version (`@i1@` in
//!    7.x, 25 characters in 5.5.1, `@VOID@` in 7.x) gets the nearest valid
//!    one ([`candidate_xref`]), made unique with a `_2`, `_3`… suffix, and
//!    every pointer to the old identifier follows it;
//! 3. a record holding an identifier already taken by an earlier record gets
//!    a new one; pointers keep resolving to the first record, as lookups do;
//! 4. a standard record without an identifier gets the first free `@I1@`,
//!    `@F1@`, … of its kind.
//!
//! A pointer to no record is kept when valid and otherwise rewritten the
//! same way, without taking an identifier of a record. A new identifier is
//! never one the data uses, a record's or a pointer's (`known`, from the
//! conformance repair): a record given the identifier a pointer to no
//! record holds would become what it names.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};

use super::emit::{candidate_xref, is_valid_pointer, new_xref};
use super::{Repair, RepairKind};
use crate::version::{VersionRules, XrefGrammar};

/// `candidate` (an identifier) with the suffix `_n`, shortened in 5.5.1 to
/// fit 22 characters: the `n`-th try at making it unique.
pub(crate) fn numbered_xref(rules: &VersionRules, candidate: &str, n: usize) -> String {
    let base = candidate.trim_end_matches('@');
    let suffix = format!("_{n}@");
    let keep = match rules.xref {
        XrefGrammar::V551 { max_len } => max_len.saturating_sub(suffix.len()),
        XrefGrammar::V7 => usize::MAX,
    };
    let base: String = base.chars().take(keep).collect();
    format!("{base}{suffix}")
}

/// Identifiers, each with a value. An identifier of at most 15 bytes (all
/// but the rarest) is a key packed in an integer, so that a lookup reads
/// the table and not the text the identifier was read from; longer ones
/// are kept as text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct XrefIndex<'a, V> {
    short: HashMap<u128, V, KeyHasher>,
    long: HashMap<Cow<'a, str>, V>,
}

/// The hash of a packed identifier: one multiplication of its halves, each
/// mixed with a random key first (the folded multiply of `foldhash`), so
/// that the identifiers of a file cannot be chosen to collide.
#[derive(Clone, Debug)]
pub(crate) struct KeyHasher {
    keys: [u64; 2],
}

impl Default for KeyHasher {
    /// The keys of the thread, drawn once: an index is made for each write
    /// (and each conformance pass), most of which hold few identifiers.
    fn default() -> Self {
        thread_local! {
            static KEYS: [u64; 2] = {
                let random = std::collections::hash_map::RandomState::new();
                [random.hash_one(0_u8), random.hash_one(1_u8)]
            };
        }
        Self {
            keys: KEYS.with(|k| *k),
        }
    }
}

impl BuildHasher for KeyHasher {
    type Hasher = KeyHash;
    fn build_hasher(&self) -> KeyHash {
        KeyHash {
            keys: self.keys,
            hash: 0,
        }
    }
}

/// See [`KeyHasher`].
pub(crate) struct KeyHash {
    keys: [u64; 2],
    hash: u64,
}

impl Hasher for KeyHash {
    fn write(&mut self, bytes: &[u8]) {
        // Not used by the keys (`u128`), but correct for any input.
        for chunk in bytes.chunks(16) {
            let mut word = [0_u8; 16];
            word.get_mut(..chunk.len())
                .into_iter()
                .for_each(|w| w.copy_from_slice(chunk));
            self.write_u128(u128::from_le_bytes(word) ^ u128::from(self.hash));
        }
    }

    // Truncations are the point: the key's halves, the product's halves.
    #[allow(clippy::cast_possible_truncation)]
    fn write_u128(&mut self, n: u128) {
        let low = (n as u64) ^ self.keys[0];
        let high = ((n >> 64) as u64) ^ self.keys[1];
        let product = u128::from(low) * u128::from(high);
        self.hash = (product as u64) ^ ((product >> 64) as u64);
    }

    fn finish(&self) -> u64 {
        self.hash
    }
}

impl<'a, V> XrefIndex<'a, V> {
    /// `xref` packed with its length, when it fits.
    pub(crate) fn packed(xref: &str) -> Option<u128> {
        Self::key(xref)
    }

    /// The value of a packed identifier ([`XrefIndex::packed`]).
    #[inline]
    pub(crate) fn get_packed(&self, key: u128) -> Option<&V> {
        self.short.get(&key)
    }

    #[inline]
    fn key(xref: &str) -> Option<u128> {
        let bytes = xref.as_bytes();
        let mut key = [0_u8; 16];
        key.get_mut(..bytes.len())?.copy_from_slice(bytes);
        *key.get_mut(15)? = u8::try_from(bytes.len()).ok().filter(|&n| n < 16)?;
        Some(u128::from_le_bytes(key))
    }

    #[inline]
    pub(crate) fn get(&self, xref: &str) -> Option<&V> {
        match Self::key(xref) {
            Some(k) => self.short.get(&k),
            None => self.long.get(xref),
        }
    }

    #[inline]
    pub(crate) fn contains_key(&self, xref: &str) -> bool {
        self.get(xref).is_some()
    }

    /// Adds `xref` with `value`; the previous value, if it was there.
    pub(crate) fn insert(&mut self, xref: Cow<'a, str>, value: V) -> Option<V> {
        match Self::key(&xref) {
            Some(k) => self.short.insert(k, value),
            None => self.long.insert(xref, value),
        }
    }

    /// Adds `xref` with `value` unless it is there; whether it was added.
    pub(crate) fn insert_new(&mut self, xref: Cow<'a, str>, value: V) -> bool {
        use std::collections::hash_map::Entry;
        match Self::key(&xref) {
            Some(k) => match self.short.entry(k) {
                Entry::Vacant(slot) => {
                    slot.insert(value);
                    true
                }
                Entry::Occupied(_) => false,
            },
            None => match self.long.entry(xref) {
                Entry::Vacant(slot) => {
                    slot.insert(value);
                    true
                }
                Entry::Occupied(_) => false,
            },
        }
    }

    pub(crate) fn clear(&mut self) {
        self.short.clear();
        self.long.clear();
    }

    pub(crate) fn reserve(&mut self, n: usize) {
        self.short.reserve(n);
    }
}

/// The identifiers of the records of one written file.
pub(crate) struct XrefMap<'a> {
    rules: &'static VersionRules,
    /// The identifiers of the data that records keep.
    taken: XrefIndex<'a, ()>,
    /// Whether an identifier is one of the data's, when the conformance
    /// repair made every one valid and unique (`taken` then stays empty).
    known: Option<&'a dyn Fn(&str) -> bool>,
    /// The identifiers the map made.
    made: HashSet<Box<str>>,
    /// The new identifier of each record that does not keep its own, by its
    /// position in write order.
    records: HashMap<usize, Box<str>>,
    /// The new identifier of each invalid identifier, for pointers.
    renamed: HashMap<Cow<'a, str>, Box<str>>,
    /// The renames, to report.
    pub(crate) repairs: Vec<Repair>,
}

/// A record as the map sees it: the prefix of the identifiers generated for
/// its kind (`None` for records that need none, such as extension records),
/// and its identifier.
pub(crate) type RecordXref<'a> = (Option<&'static str>, Option<&'a str>);

impl<'a> XrefMap<'a> {
    /// Assigns the identifiers of `records`, given in write order.
    pub(crate) fn new<I>(rules: &'static VersionRules, records: I) -> Self
    where
        I: Iterator<Item = RecordXref<'a>>,
    {
        Self::assign(rules, records, None)
    }

    /// Assigns the identifiers of records the conformance repair made
    /// valid and unique, `known` telling its identifiers: only records
    /// without one may need one.
    pub(crate) fn conformed<I>(
        rules: &'static VersionRules,
        records: I,
        known: &'a dyn Fn(&str) -> bool,
    ) -> Self
    where
        I: Iterator<Item = RecordXref<'a>> + Clone,
    {
        #[cfg(debug_assertions)]
        {
            let mut seen = HashSet::new();
            for (_, xref) in records.clone() {
                if let Some(x) = xref {
                    assert!(rules.is_valid_xref(x) && seen.insert(x) && known(x), "{x}");
                }
            }
        }
        Self::assign(rules, records, Some(known))
    }

    /// Assigns identifiers to the records without one of a dataset whose
    /// record identifiers are valid and unique, `known` telling them:
    /// `missing` lists, in write order, the position and the identifier
    /// prefix of each standard record without one.
    pub(crate) fn missing(
        rules: &'static VersionRules,
        missing: impl Iterator<Item = (usize, &'static str)>,
        known: &'a dyn Fn(&str) -> bool,
    ) -> Self {
        let mut map = Self::empty(rules, Some(known));
        map.number(missing.map(|(index, prefix)| (index, Some(prefix), None)));
        map
    }

    fn empty(rules: &'static VersionRules, known: Option<&'a dyn Fn(&str) -> bool>) -> Self {
        Self {
            rules,
            taken: XrefIndex::default(),
            known,
            made: HashSet::new(),
            records: HashMap::new(),
            renamed: HashMap::new(),
            repairs: Vec::new(),
        }
    }

    fn assign<I>(
        rules: &'static VersionRules,
        records: I,
        known: Option<&'a dyn Fn(&str) -> bool>,
    ) -> Self
    where
        I: Iterator<Item = RecordXref<'a>>,
    {
        let mut map = Self::empty(rules, known);
        // One pass: the valid identifiers first, so that a rewritten one
        // never takes them; the other records are handled after it.
        let mut irregular = Vec::new();
        if known.is_none() {
            map.taken.reserve(records.size_hint().0);
        }
        for (index, (prefix, xref)) in records.enumerate() {
            let keep = match (xref, known) {
                (Some(_), Some(_)) => true,
                (Some(x), None) => {
                    rules.is_valid_xref(x) && map.taken.insert(Cow::Borrowed(x), ()).is_none()
                }
                (None, _) => false,
            };
            if !keep && (xref.is_some() || prefix.is_some()) {
                irregular.push((index, prefix, xref));
            }
        }
        map.number(irregular.into_iter());
        map
    }

    /// Renames the records `irregular` lists — their position, the prefix
    /// of the identifiers generated for their kind and their identifier —
    /// in order: an invalid or repeated identifier, or none.
    fn number(
        &mut self,
        irregular: impl Iterator<Item = (usize, Option<&'static str>, Option<&'a str>)>,
    ) {
        let rules = self.rules;
        let map = self;
        let mut seen_invalid: HashSet<&'a str> = HashSet::new();
        let mut counters: HashMap<&'static str, usize> = HashMap::new();
        for (index, prefix, xref) in irregular {
            let new = match xref {
                Some(x) => {
                    let new = map.unique(new_xref(rules, x));
                    if !rules.is_valid_xref(x) && seen_invalid.insert(x) {
                        map.renamed.insert(x.into(), new.clone().into());
                    }
                    map.repairs.push(Repair::new(
                        0,
                        RepairKind::Xref,
                        format!("identifier {x} written as {new}"),
                    ));
                    new
                }
                None => match prefix {
                    Some(prefix) => {
                        let n = counters.entry(prefix).or_insert(0);
                        loop {
                            *n += 1;
                            let candidate = format!("@{prefix}{n}@");
                            if !map.is_taken(&candidate) {
                                map.made.insert(candidate.as_str().into());
                                break candidate;
                            }
                        }
                    }
                    None => continue,
                },
            };
            map.records.insert(index, new.into());
        }
    }

    /// Whether a record already has `xref`.
    fn is_taken(&self, xref: &str) -> bool {
        self.taken.contains_key(xref)
            || self.made.contains(xref)
            || self.known.is_some_and(|known| known(xref))
    }

    /// `candidate`, or `candidate` with the first free `_n` suffix; taken.
    fn unique(&mut self, candidate: String) -> String {
        let mut n = 1_usize;
        let mut new = candidate;
        let base = new.clone();
        while self.is_taken(&new) {
            n += 1;
            new = numbered_xref(self.rules, &base, n);
        }
        self.made.insert(new.as_str().into());
        new
    }

    /// A new identifier for a record that is not in the map (the stub
    /// submitter of a 5.5.1 header).
    pub(crate) fn fresh(&mut self, prefix: &str) -> String {
        let mut n = 0_usize;
        loop {
            n += 1;
            let candidate = format!("@{prefix}{n}@");
            if !self.is_taken(&candidate) {
                self.made.insert(candidate.as_str().into());
                return candidate;
            }
        }
    }

    /// The identifier written for the record at `index` in write order,
    /// whose own identifier is `xref`.
    pub(crate) fn record<'x>(&self, index: usize, xref: Option<&'x str>) -> Option<Cow<'x, str>> {
        if self.records.is_empty() {
            return xref.map(Cow::Borrowed);
        }
        match self.records.get(&index) {
            Some(new) => Some(Cow::Owned(new.to_string())),
            None => xref.map(Cow::Borrowed),
        }
    }

    /// The identifier a pointer is written with.
    #[inline]
    pub(crate) fn pointer<'p>(&mut self, pointer: &'p str) -> Cow<'p, str> {
        // `@VOID@` points to nothing in 7.x, even when a record had it.
        if self.rules.xref == XrefGrammar::V7 && pointer == "@VOID@" {
            return Cow::Borrowed(pointer);
        }
        if !self.renamed.is_empty()
            && let Some(new) = self.renamed.get(pointer)
        {
            return Cow::Owned(new.to_string());
        }
        if is_valid_pointer(self.rules, pointer) {
            return Cow::Borrowed(pointer);
        }
        // A pointer to no record, outside the grammar: rewritten once, the
        // same way everywhere, never onto a record's identifier.
        let new = self.unique(candidate_xref(self.rules, pointer));
        self.renamed
            .insert(Cow::Owned(pointer.to_string()), new.clone().into());
        Cow::Owned(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::{V70, V551};

    #[test]
    fn valid_unique_identifiers_are_kept() {
        let records = [(Some("I"), Some("@I1@")), (Some("F"), Some("@F1@"))];
        let mut map = XrefMap::new(&V70, records.iter().copied());
        assert_eq!(map.record(0, Some("@I1@")).as_deref(), Some("@I1@"));
        assert_eq!(map.pointer("@F1@"), "@F1@");
        assert!(map.repairs.is_empty());
    }

    #[test]
    fn invalid_duplicate_void_and_missing() {
        let records = [
            (Some("I"), Some("@i1@")),
            (Some("I"), Some("@I1@")),
            (Some("I"), Some("@I1@")),
            (Some("I"), Some("@VOID@")),
            (Some("I"), None),
            (None, None),
        ];
        let mut map = XrefMap::new(&V70, records.iter().copied());
        // The valid @I1@ keeps its name; @i1@ moves aside, and so does the
        // second @I1@.
        assert_eq!(map.record(1, Some("@I1@")).as_deref(), Some("@I1@"));
        assert_eq!(map.record(0, Some("@i1@")).as_deref(), Some("@I1_2@"));
        assert_eq!(map.record(2, Some("@I1@")).as_deref(), Some("@I1_3@"));
        assert_eq!(map.record(3, Some("@VOID@")).as_deref(), Some("@VOID_@"));
        assert_eq!(map.record(4, None).as_deref(), Some("@I2@"));
        assert_eq!(map.record(5, None).as_deref(), None);
        assert_eq!(map.pointer("@i1@"), "@I1_2@");
        assert_eq!(map.pointer("@I1@"), "@I1@");
        assert_eq!(map.pointer("@VOID@"), "@VOID@");
        // A dangling invalid pointer never lands on a record.
        assert_eq!(map.pointer("@i2@"), "@I2_2@");
        assert_eq!(map.repairs.len(), 3);
    }

    #[test]
    fn long_identifiers_in_551() {
        let long = "@IABCDEFGHIJKLMNOPQRSTUVWXYZ0123@";
        let records = [(Some("I"), Some(long)), (Some("I"), Some(long))];
        let mut map = XrefMap::new(&V551, records.iter().copied());
        let first = map.record(0, Some(long)).unwrap().to_string();
        let second = map.record(1, Some(long)).unwrap().to_string();
        assert!(V551.is_valid_xref(&first), "{first}");
        assert!(V551.is_valid_xref(&second), "{second}");
        assert_ne!(first, second);
        assert_eq!(map.pointer(long), first);
    }
}
