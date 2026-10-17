//! Ancestors and descendants: the individuals a dataset relates to one,
//! generation by generation.
//!
//! Families relate them: an individual's parents are the partners
//! (`HUSB`, `WIFE`) of the families they are a child (`CHIL`) in, their
//! children the children of the families they are a partner in, whatever
//! the pedigree (birth, adoption, foster) of the link. The walk is breadth
//! first: generation 1 holds the parents (or the children), generation 2
//! the grandparents (or grandchildren), and so on; an individual reached
//! twice (pedigree collapse, a cycle in the data) is listed once, at the
//! first generation it is reached in.

use crate::indexed::{Links, Role};

use super::dataset::{Dataset, XrefKey};
use super::record::{Family, Individual};
use super::table::View;
use super::text::XrefId;

/// An individual related to another, and how many generations apart: see
/// [`Dataset::ancestors`] and [`Dataset::descendants`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct Relative<'a> {
    /// The individual.
    pub individual: View<'a, Individual>,
    /// The generations between them: 1 for a parent or a child, 2 for a
    /// grandparent or a grandchild, …
    pub generation: u32,
}

/// Walks from `start`, generation by generation up to `generations`
/// (`None`: all), `next` giving the families each individual leads to
/// (as positions in [`Dataset::families`]) and `members` the members of a
/// family it leads to.
fn walk<'a>(
    data: &'a Dataset,
    start: Option<XrefId>,
    generations: Option<u32>,
    links: &Links,
    members: fn(View<'a, Family>, &mut dyn FnMut(XrefId)),
) -> Vec<Relative<'a>> {
    let mut out = Vec::new();
    let Some(start) = start else { return out };
    if data.find_individual(start).is_none() {
        return out;
    }
    let families = data.families();
    let mut seen = vec![false; data.store().xref_count()];
    let mut mark = |id: XrefId| match seen.get_mut(id.index()) {
        Some(s) if !*s => {
            *s = true;
            true
        }
        _ => false,
    };
    mark(start);
    let mut current = vec![start];
    let mut generation = 0_u32;
    while !current.is_empty() && generations.is_none_or(|g| generation < g) {
        generation += 1;
        let mut next = Vec::new();
        for id in current {
            for &at in links.get(id.index()) {
                let Some(family) = families.get(at as usize) else {
                    continue;
                };
                members(family, &mut |member| {
                    if let Some(individual) = data.find_individual(member)
                        && mark(member)
                    {
                        out.push(Relative {
                            individual,
                            generation,
                        });
                        next.push(member);
                    }
                });
            }
        }
        current = next;
    }
    out
}

fn partners(family: View<'_, Family>, f: &mut dyn FnMut(XrefId)) {
    for id in [family.husband_id(), family.wife_id()]
        .into_iter()
        .flatten()
    {
        f(id);
    }
}

fn children(family: View<'_, Family>, f: &mut dyn FnMut(XrefId)) {
    for id in family
        .children()
        .into_iter()
        .filter_map(View::<super::IndividualRef>::individual)
    {
        f(id);
    }
}

impl Dataset {
    /// The ancestors of an individual with a record, generation by
    /// generation up to `generations` (`None`: every generation): the
    /// partners of the families they are a child in, then theirs, and so
    /// on, in the order of the families and of their partners (`HUSB`, then
    /// `WIFE`), whatever the pedigree of the link. Each ancestor is listed
    /// once, at the nearest generation, even when the data reaches it twice
    /// (pedigree collapse, a cycle), and the individual itself never.
    /// Linear in the size of the dataset.
    ///
    /// ```rust
    /// use oxidged::model::Dataset;
    ///
    /// let data = Dataset::parse(
    ///     "0 HEAD\n1 GEDC\n2 VERS 7.0\n\
    ///      0 @I1@ INDI\n0 @I2@ INDI\n0 @I3@ INDI\n0 @I4@ INDI\n\
    ///      0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I3@\n1 CHIL @I1@\n\
    ///      0 @F2@ FAM\n1 HUSB @I4@\n1 CHIL @I2@\n0 TRLR\n",
    /// );
    /// let ancestors: Vec<_> = data
    ///     .ancestors("@I1@", None)
    ///     .iter()
    ///     .map(|r| (data.store().xref(r.individual.xref().unwrap()), r.generation))
    ///     .collect();
    /// assert_eq!(ancestors, [("@I2@", 1), ("@I3@", 1), ("@I4@", 2)]);
    /// assert_eq!(data.ancestors("@I1@", Some(1)).len(), 2);
    /// ```
    #[must_use]
    pub fn ancestors(
        &self,
        individual: impl XrefKey,
        generations: Option<u32>,
    ) -> Vec<Relative<'_>> {
        let links = Links::of(self, Role::Child);
        walk(
            self,
            individual.id_in(self.store()),
            generations,
            &links,
            partners,
        )
    }

    /// The descendants of an individual with a record, generation by
    /// generation up to `generations` (`None`: every generation): the
    /// children of the families they are a partner in, then theirs, and so
    /// on, in the order of the families and of their children. Each
    /// descendant is listed once, at the nearest generation. Linear in the
    /// size of the dataset.
    ///
    /// ```rust
    /// use oxidged::model::Dataset;
    ///
    /// let data = Dataset::parse(
    ///     "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n0 @I2@ INDI\n0 @I3@ INDI\n\
    ///      0 @F1@ FAM\n1 WIFE @I1@\n1 CHIL @I2@\n0 @F2@ FAM\n1 HUSB @I2@\n1 CHIL @I3@\n0 TRLR\n",
    /// );
    /// let generations: Vec<u32> = data.descendants("@I1@", None).iter().map(|r| r.generation).collect();
    /// assert_eq!(generations, [1, 2]);
    /// ```
    #[must_use]
    pub fn descendants(
        &self,
        individual: impl XrefKey,
        generations: Option<u32>,
    ) -> Vec<Relative<'_>> {
        let links = Links::of(self, Role::Partner);
        walk(
            self,
            individual.id_in(self.store()),
            generations,
            &links,
            children,
        )
    }
}

impl crate::IndexedDataset {
    /// [`Dataset::ancestors`], with the index: linear in the number of
    /// ancestors and their families.
    #[must_use]
    pub fn ancestors(
        &self,
        individual: impl XrefKey,
        generations: Option<u32>,
    ) -> Vec<Relative<'_>> {
        let data = self.data();
        walk(
            data,
            individual.id_in(data.store()),
            generations,
            self.links(Role::Child),
            partners,
        )
    }

    /// [`Dataset::descendants`], with the index: linear in the number of
    /// descendants and their families.
    #[must_use]
    pub fn descendants(
        &self,
        individual: impl XrefKey,
        generations: Option<u32>,
    ) -> Vec<Relative<'_>> {
        let data = self.data();
        walk(
            data,
            individual.id_in(data.store()),
            generations,
            self.links(Role::Partner),
            children,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::IndexedDataset;
    use crate::model::Dataset;

    /// Ann (@I1@) and Bob (@I2@) are the parents of Cid (@I3@) and Dan
    /// (@I4@); Cid and Eve (@I5@, of no family as a child) of Fay (@I6@);
    /// Ann is also the adoptive child of Gus (@I7@) — and, by an error of
    /// the data, of her grandchild Fay, a cycle. @I9@ has no record.
    const FILE: &str = "0 HEAD\n1 GEDC\n2 VERS 7.0\n\
        0 @I1@ INDI\n0 @I2@ INDI\n0 @I3@ INDI\n0 @I4@ INDI\n0 @I5@ INDI\n\
        0 @I6@ INDI\n0 @I7@ INDI\n\
        0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I1@\n1 CHIL @I3@\n1 CHIL @I4@\n1 CHIL @I9@\n\
        0 @F2@ FAM\n1 HUSB @I3@\n1 WIFE @I5@\n1 CHIL @I6@\n\
        0 @F3@ FAM\n1 HUSB @I7@\n1 WIFE @I6@\n1 CHIL @I1@\n\
        0 TRLR\n";

    fn names(data: &Dataset, relatives: &[super::Relative<'_>]) -> Vec<(String, u32)> {
        relatives
            .iter()
            .map(|r| {
                let xref = r.individual.xref().unwrap();
                (data.store().xref(xref).to_string(), r.generation)
            })
            .collect()
    }

    #[test]
    fn ancestors_by_generation_once_each() {
        let data = Dataset::parse(FILE);
        let all = data.ancestors("@I6@", None);
        // Fay's parents, Cid's parents, then Ann's: Gus and Fay herself,
        // who is the start and not listed.
        assert_eq!(
            names(&data, &all),
            [
                ("@I3@".into(), 1),
                ("@I5@".into(), 1),
                ("@I2@".into(), 2),
                ("@I1@".into(), 2),
                ("@I7@".into(), 3),
            ]
        );
        assert_eq!(names(&data, &data.ancestors("@I6@", Some(2))).len(), 4);
        assert!(data.ancestors("@I6@", Some(0)).is_empty());
        assert!(data.ancestors("@I5@", None).is_empty());
        assert!(data.ancestors("@I9@", None).is_empty(), "no record");
        assert!(data.ancestors("@F1@", None).is_empty(), "not an individual");
        assert!(data.ancestors("@nothing@", None).is_empty());
    }

    #[test]
    fn descendants_by_generation_once_each() {
        let data = Dataset::parse(FILE);
        // Ann's children (not @I9@, which has no record), Cid's child Fay,
        // Fay's adoptive child Ann: the start, not listed.
        assert_eq!(
            names(&data, &data.descendants("@I1@", None)),
            [("@I3@".into(), 1), ("@I4@".into(), 1), ("@I6@".into(), 2)]
        );
        assert_eq!(names(&data, &data.descendants("@I1@", Some(1))).len(), 2);
        assert!(data.descendants("@I4@", None).is_empty());
    }

    #[test]
    fn the_index_walks_the_same() {
        let data = Dataset::parse(FILE);
        let indexed = IndexedDataset::new(data.clone());
        for xref in ["@I1@", "@I3@", "@I6@", "@I7@", "@I9@"] {
            for depth in [None, Some(1), Some(2)] {
                assert_eq!(
                    names(&data, &data.ancestors(xref, depth)),
                    names(&data, &indexed.ancestors(xref, depth)),
                    "{xref} {depth:?}"
                );
                assert_eq!(
                    names(&data, &data.descendants(xref, depth)),
                    names(&data, &indexed.descendants(xref, depth)),
                    "{xref} {depth:?}"
                );
            }
        }
    }
}
