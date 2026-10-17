//! A dataset with an index of the families of each individual, found in
//! constant time.
//!
//! [`Dataset`] finds records by identifier in constant time itself, and
//! finds the families of an individual by a linear search, which suits a
//! few lookups; [`IndexedDataset`] indexes those families once (a few
//! bytes per identifier and per link) for many. Everything else is the
//! dataset's, through `Deref`.
//!
//! ```rust
//! use oxidged::model::Dataset;
//! use oxidged::IndexedDataset;
//!
//! let data = Dataset::parse(
//!     "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 @I2@ INDI\n\
//!      0 @F1@ FAM\n1 WIFE @I1@\n1 CHIL @I2@\n0 TRLR\n",
//! );
//! let indexed = IndexedDataset::new(data);
//! let ann = indexed.find_individual("@I1@").unwrap();
//! let family = indexed.families_as_spouse(ann.xref()).next().unwrap();
//! assert_eq!(indexed.children(family).count(), 1);
//! assert_eq!(indexed.individuals().len(), 2); // a dataset still
//! ```

use std::ops::{Deref, DerefMut};

use crate::model::{Dataset, Family, View, XrefId, XrefKey};

/// How an individual belongs to a family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// A partner (`HUSB` or `WIFE`).
    Partner,
    /// A child (`CHIL`).
    Child,
}

/// Lists of positions per item, in one buffer: the families of each
/// individual.
#[derive(Clone, Debug, Default)]
pub(crate) struct Links {
    /// For item `i`, its positions are `items[starts[i]..starts[i + 1]]`.
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl Links {
    /// The families each identifier is a partner or a child in (by
    /// [`XrefId`]), whether or not a record has it, as the dataset's own
    /// navigation finds them: one walk of the families.
    pub(crate) fn of(data: &Dataset, role: Role) -> Self {
        let mut pairs = Vec::new();
        let index = |id: XrefId| u32::try_from(id.index()).ok();
        for (at, family) in data.families().into_iter().enumerate() {
            let Ok(at) = u32::try_from(at) else { break };
            match role {
                Role::Partner => {
                    for partner in [family.husband_id(), family.wife_id()]
                        .into_iter()
                        .flatten()
                    {
                        if let Some(i) = index(partner) {
                            pairs.push((i, at));
                        }
                    }
                }
                Role::Child => {
                    for child in family
                        .children()
                        .into_iter()
                        .filter_map(View::<crate::model::IndividualRef>::individual)
                    {
                        if let Some(i) = index(child) {
                            pairs.push((i, at));
                        }
                    }
                }
            }
        }
        // One list entry per family, even when an individual is both
        // partners of it.
        pairs.dedup();
        Self::build(data.store().xref_count(), &pairs)
    }

    /// Builds the lists from `(item, position)` pairs over `count` items.
    fn build(count: usize, pairs: &[(u32, u32)]) -> Self {
        let mut starts = vec![0_u32; count + 1];
        for &(item, _) in pairs {
            if let Some(s) = starts.get_mut(item as usize + 1) {
                *s += 1;
            }
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        let mut items = vec![0_u32; pairs.len()];
        let mut fill = starts.clone();
        for &(item, at) in pairs {
            if let Some(f) = fill.get_mut(item as usize) {
                if let Some(slot) = items.get_mut(*f as usize) {
                    *slot = at;
                }
                *f += 1;
            }
        }
        Self { starts, items }
    }

    /// The positions of `item`.
    pub(crate) fn get(&self, item: usize) -> &[u32] {
        let start = self.starts.get(item).copied().unwrap_or(0) as usize;
        let end = self.starts.get(item + 1).copied().unwrap_or(0) as usize;
        self.items.get(start..end).unwrap_or_default()
    }
}

/// A [`Dataset`] and its index of families: see the [module
/// documentation](self).
///
/// The index is built by [`new`](Self::new) and rebuilt after every edit
/// through [`data_mut`](Self::data_mut); the dataset is readable through
/// `Deref`, its lookups by identifier included.
#[derive(Clone, Debug)]
pub struct IndexedDataset {
    data: Dataset,
    /// The families each identifier is a partner in, by [`XrefId`].
    as_spouse: Links,
    /// The families each identifier is a child in, by [`XrefId`].
    as_child: Links,
}

impl IndexedDataset {
    /// Indexes a dataset.
    #[must_use]
    pub fn new(data: Dataset) -> Self {
        let mut indexed = Self {
            data,
            as_spouse: Links::default(),
            as_child: Links::default(),
        };
        indexed.reindex();
        indexed
    }

    fn reindex(&mut self) {
        self.as_spouse = Links::of(&self.data, Role::Partner);
        self.as_child = Links::of(&self.data, Role::Child);
    }

    /// The index of the families each identifier has `role` in.
    pub(crate) fn links(&self, role: Role) -> &Links {
        match role {
            Role::Partner => &self.as_spouse,
            Role::Child => &self.as_child,
        }
    }

    /// The dataset.
    #[must_use]
    pub fn data(&self) -> &Dataset {
        &self.data
    }

    /// The dataset, to edit: the index is rebuilt when the returned guard
    /// is dropped.
    pub fn data_mut(&mut self) -> DatasetMut<'_> {
        DatasetMut { indexed: self }
    }

    /// The dataset, without its index.
    #[must_use]
    pub fn into_inner(self) -> Dataset {
        self.data
    }

    fn families_in<'s>(
        &'s self,
        links: &'s Links,
        individual: impl XrefKey,
    ) -> impl Iterator<Item = View<'s, Family>> {
        let positions = individual
            .id_in(self.data.store())
            .map_or(&[][..], |id| links.get(id.index()));
        let families = self.data.families();
        positions
            .iter()
            .filter_map(move |&at| families.get(at as usize))
    }

    /// The families in which this individual is a partner (`HUSB` or
    /// `WIFE`), in order.
    pub fn families_as_spouse(
        &self,
        individual: impl XrefKey,
    ) -> impl Iterator<Item = View<'_, Family>> {
        self.families_in(&self.as_spouse, individual)
    }

    /// The families in which this individual is a child (`CHIL`), in
    /// order.
    pub fn families_as_child(
        &self,
        individual: impl XrefKey,
    ) -> impl Iterator<Item = View<'_, Family>> {
        self.families_in(&self.as_child, individual)
    }
}

impl From<Dataset> for IndexedDataset {
    fn from(data: Dataset) -> Self {
        Self::new(data)
    }
}

impl Deref for IndexedDataset {
    type Target = Dataset;

    fn deref(&self) -> &Dataset {
        &self.data
    }
}

/// The dataset of an [`IndexedDataset`], to edit: dropping it rebuilds the
/// index.
#[derive(Debug)]
pub struct DatasetMut<'a> {
    indexed: &'a mut IndexedDataset,
}

impl Deref for DatasetMut<'_> {
    type Target = Dataset;

    fn deref(&self) -> &Dataset {
        &self.indexed.data
    }
}

impl DerefMut for DatasetMut<'_> {
    fn deref_mut(&mut self) -> &mut Dataset {
        &mut self.indexed.data
    }
}

impl Drop for DatasetMut<'_> {
    fn drop(&mut self) {
        self.indexed.reindex();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Individual, IndividualRef, Name, RecordRef};

    const FILE: &str = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n\
        0 @I1@ INDI\n1 NAME Ann /Example/\n\
        0 @I2@ INDI\n1 NAME Bob /Example/\n\
        0 @I3@ INDI\n1 NAME Cid /Example/\n\
        0 @I1@ INDI\n1 NAME Duplicate /Example/\n\
        0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I1@\n1 CHIL @I3@\n1 CHIL @I9@\n\
        0 @F2@ FAM\n1 WIFE @I1@\n\
        0 @S1@ SOUR\n0 @R1@ REPO\n0 @M1@ OBJE\n0 @U1@ SUBM\n0 @N1@ NOTE Shared\n0 @X1@ _LOC\n\
        0 TRLR\n";

    #[test]
    fn finds_every_record_type_first_wins() {
        let indexed = IndexedDataset::new(Dataset::parse(FILE));
        let ann = indexed.find_individual("@I1@").unwrap();
        assert_eq!(ann.full_name().as_deref(), Some("Ann Example"));
        assert!(indexed.find_family("@F1@").is_some());
        assert!(indexed.find_source("@S1@").is_some());
        assert!(indexed.find_repository("@R1@").is_some());
        assert!(indexed.find_multimedia("@M1@").is_some());
        assert!(indexed.find_submitter("@U1@").is_some());
        assert!(indexed.find_note("@N1@").is_some());
        assert!(matches!(indexed.find("@X1@"), Some(RecordRef::Other(_))));
        assert!(indexed.find_family("@I1@").is_none());
        assert!(indexed.find("@I9@").is_none());
        assert!(indexed.find("@nothing@").is_none());
        // Every lookup agrees with the dataset's linear search.
        for xref in ["@I1@", "@I2@", "@F2@", "@S1@", "@N1@", "@X1@", "@I9@"] {
            assert_eq!(indexed.find(xref), indexed.data().find(xref), "{xref}");
        }
    }

    #[test]
    fn navigates_families() {
        let indexed = IndexedDataset::from(Dataset::parse(FILE));
        let ann = indexed.find_individual("@I1@").unwrap();
        let families: Vec<_> = indexed.families_as_spouse(ann.xref()).collect();
        assert_eq!(families.len(), 2);
        assert_eq!(indexed.parents(families[0]).count(), 2);
        assert_eq!(indexed.children(families[0]).count(), 1); // @I9@ has no record
        assert_eq!(
            indexed
                .spouse("@I1@", families[0])
                .and_then(|s| s.full_name()),
            Some("Bob Example".to_string())
        );
        assert_eq!(indexed.families_as_child("@I3@").count(), 1);
        assert_eq!(indexed.families_as_child("@I1@").count(), 0);
        // The same answers as the dataset's own navigation.
        let data = indexed.data();
        assert_eq!(
            data.families_as_spouse("@I1@").count(),
            indexed.families_as_spouse("@I1@").count()
        );
    }

    #[test]
    fn edits_reindex() {
        let mut indexed = IndexedDataset::new(Dataset::parse(FILE));
        {
            let mut data = indexed.data_mut();
            let id = data.store_mut().intern_xref("@I7@").unwrap();
            let mut person = Individual {
                xref: Some(id),
                ..Individual::default()
            };
            person.names.push(Name::new("Eve /Example/"));
            data.add(person).unwrap();
            let wife = IndividualRef::new(id);
            data.get_mut_at::<crate::model::Family>(1).unwrap().wife = Some(wife);
        }
        assert!(indexed.find_individual("@I7@").is_some());
        assert_eq!(indexed.families_as_spouse("@I7@").count(), 1);
        assert_eq!(indexed.families_as_spouse("@I1@").count(), 1);
        assert_eq!(indexed.into_inner().individuals().len(), 5);
    }
}
