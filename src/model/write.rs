//! Writing the typed model: each record writes its structures for the
//! target version into the writer's flat arena, when the writer needs them,
//! so that a dataset is never copied as owned structures.

use std::borrow::Cow;

use crate::spec::conform::Build;
use crate::tree::{Flat, FlatPayload, Structure};
use crate::version::{GedcomVersion, VersionRules};
use crate::writer::GedcomWriter;

use super::dataset::{Dataset, RecordRef};
use super::driver::{RowRewrite, WriteCx};
use crate::convert::Adoptions;

/// A record of a dataset that writes its structures for the target version
/// when the writer needs them.
pub(crate) struct Built<'d> {
    /// The record; `None` for the trailer, which the dataset does not keep.
    record: Option<RecordRef<'d>>,
    /// The version to write.
    version: GedcomVersion,
    /// The extension structures written as standard ones
    /// ([`WriteCx::adopt`]).
    adopt: Option<&'d Adoptions>,
    /// What an export writes instead of some structures
    /// ([`WriteCx::rewrite`]).
    rewrite: Option<&'d dyn RowRewrite>,
}

impl<'d> Built<'d> {
    /// The header as an owned structure, as the writer writes it
    /// ([`Build::owned`]).
    pub(crate) fn owned_record(&self) -> Option<Structure> {
        let header = self.record.filter(|r| matches!(r, RecordRef::Header(_)))?;
        Some(header.to_structure(&self.cx(header)))
    }

    /// A record to write in `version`.
    pub(crate) fn new(record: RecordRef<'d>, version: GedcomVersion) -> Self {
        Self {
            record: Some(record),
            version,
            adopt: None,
            rewrite: None,
        }
    }

    /// How `record`, this one, writes.
    fn cx(&self, record: RecordRef<'d>) -> WriteCx<'d> {
        let mut cx = WriteCx::of(record.dataset(), self.version, true);
        cx.adopt = self.adopt;
        if let Some(r) = self.rewrite {
            cx.rewrite = Some(r);
            cx.rewrite_tags = r.tags();
        }
        cx
    }
}

impl<'d> Build<'d> for Built<'d> {
    fn tag(&self) -> &'d str {
        self.record.map_or("TRLR", |r| r.tag(self.version))
    }

    fn xref(&self) -> Option<&'d str> {
        let record = self.record?;
        record.xref().map(|x| record.store().xref(x))
    }

    fn is_empty(&self) -> bool {
        self.record.is_none_or(RecordRef::is_empty)
    }

    fn xref_id(&self) -> Option<super::XrefId> {
        self.record?.xref()
    }

    fn standard_tag(&self) -> Option<u16> {
        match self.record {
            Some(r) => r.standard_tag(self.version),
            None => const { crate::tree::standard_index_const("TRLR") },
        }
    }

    fn build(&self, out: &mut Flat<'d>) {
        if let Some(r) = self.record {
            if let Some(rewrite) = self.rewrite {
                rewrite.record(r.tag(self.version));
            }
            r.to_flat(&self.cx(r), out);
        } else {
            let at = out.open(Cow::Borrowed("TRLR"), None, FlatPayload::None);
            out.close(at);
        }
    }

    fn declared_version(&self) -> Option<Cow<'d, str>> {
        let h = self.typed_head()?;
        let gedcom_aside = h.gedcom.as_ref().is_some_and(|g| !g.extra.is_empty());
        if !h.extra.is_empty() || gedcom_aside {
            return None;
        }
        let store = self.record?.store();
        Some(h.declared_version(store).unwrap_or_default())
    }

    fn typed_head(&self) -> Option<&'d super::Header> {
        match self.record {
            Some(RecordRef::Header(h)) => Some(h.header()),
            _ => None,
        }
    }

    fn build_completed_head(
        &self,
        out: &mut Flat<'d>,
        rules: &VersionRules,
        submitter: FlatPayload<'d>,
    ) -> bool {
        let (Some(record), Some(h)) = (self.record, self.typed_head()) else {
            return false;
        };
        h.write_completed_flat(&self.cx(record), out, rules, submitter)
    }

    fn owned(&self) -> Option<Structure> {
        self.owned_record()
    }
}

impl Dataset {
    /// The records, to write with `writer` (in its version, unless one is
    /// configured), then the trailer: what the writer's whole pass, which
    /// the direct write must equal, is tested with.
    #[cfg(test)]
    pub(crate) fn built(&self, writer: &GedcomWriter) -> Vec<Built<'_>> {
        self.built_adopting(writer, None)
    }

    /// [`Dataset::built`], the extension structures `adopt` names written
    /// as the standard structures they stand for ([`WriteCx::adopt`]).
    pub(crate) fn built_adopting<'d>(
        &'d self,
        writer: &GedcomWriter,
        adopt: Option<&'d Adoptions>,
    ) -> Vec<Built<'d>> {
        self.built_with(writer, adopt, None)
    }

    /// [`Dataset::built_adopting`], the structures `rewrite` rewrites
    /// written as it says ([`WriteCx::rewrite`]).
    pub(crate) fn built_with<'d>(
        &'d self,
        writer: &GedcomWriter,
        adopt: Option<&'d Adoptions>,
        rewrite: Option<&'d dyn RowRewrite>,
    ) -> Vec<Built<'d>> {
        let version = writer.config().version.unwrap_or(self.version);
        // The records in the order of [`Dataset::records`], one table after
        // the other.
        let mut built = Vec::with_capacity(self.record_count() + 1);
        let mut add = |record| {
            built.push(Built {
                record: Some(record),
                version,
                adopt,
                rewrite,
            });
        };
        if let Some(h) = self.header_ref() {
            add(RecordRef::Header(h));
        }
        self.submitters()
            .into_iter()
            .for_each(|r| add(RecordRef::Submitter(r)));
        self.submissions()
            .into_iter()
            .for_each(|r| add(RecordRef::Submission(r)));
        self.individuals()
            .into_iter()
            .for_each(|r| add(RecordRef::Individual(r)));
        self.families()
            .into_iter()
            .for_each(|r| add(RecordRef::Family(r)));
        self.notes()
            .into_iter()
            .for_each(|r| add(RecordRef::Note(r)));
        self.sources()
            .into_iter()
            .for_each(|r| add(RecordRef::Source(r)));
        self.repositories()
            .into_iter()
            .for_each(|r| add(RecordRef::Repository(r)));
        self.multimedia()
            .into_iter()
            .for_each(|r| add(RecordRef::Multimedia(r)));
        self.extra()
            .into_iter()
            .for_each(|r| add(RecordRef::Other(r)));
        // The trailer, which the dataset does not keep.
        built.push(Built {
            record: None,
            version,
            adopt: None,
            rewrite: None,
        });
        built
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::conform::Typing;
    use crate::tree::Node as _;
    use crate::version::GedcomVersion;

    /// Each record of `text` but the header, written typed for `version`:
    /// whether it typed itself, the tags of the structures whose payloads
    /// the check looks at, and of its extension structures.
    fn typed(text: &str, version: GedcomVersion) -> Vec<(bool, Vec<String>, Vec<String>)> {
        let data = Dataset::parse(text);
        let cx = WriteCx::of(&data, version, true);
        let mut flat = Flat::default();
        flat.set_typing(Some(Typing::of(version)));
        data.records()
            .filter(|r| !matches!(r, RecordRef::Header(_)))
            .map(|r| {
                flat.clear();
                r.to_flat(&cx, &mut flat);
                let checks = flat.check_tags().map(str::to_string).collect();
                let ext = flat.extensions().map(|e| e.tag().to_string()).collect();
                (flat.typed(), checks, ext)
            })
            .collect()
    }

    /// A typed record places each structure by its type and trusts the
    /// values its types made valid (an enumeration value, a date of the
    /// commonest shapes); its texts and pointers are left to the check.
    #[test]
    fn typed_records_trust_their_typed_values() {
        let text = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n2 GIVN Ann\n1 SEX F\n1 BIRT\n2 DATE 1 JAN 1900\n2 PLAC Sampleton\n1 FAMS @F1@\n1 _UID 0123\n0 @F1@ FAM\n1 WIFE @I1@\n0 TRLR\n";
        let records = typed(text, GedcomVersion::V5_5_1);
        let (typed, checks, ext) = &records[0];
        assert!(typed);
        assert_eq!(checks, &["NAME", "GIVN", "PLAC", "FAMS"]);
        assert_eq!(ext, &["_UID"]);
        assert_eq!(records[1], (true, vec!["WIFE".to_string()], vec![]));
    }

    /// What a type cannot vouch for leaves the record to the check's walk:
    /// a repeated singleton kept in `extra`, a structure the target version
    /// does not have; an enumeration value of another set is looked at.
    #[test]
    fn untyped_structures_leave_the_record_to_the_walk() {
        let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX F\n1 SEX M\n0 @I2@ INDI\n1 NO MARR\n0 @I3@ INDI\n1 CONL\n2 STAT DNS_CAN\n3 DATE 1 JAN 2000\n0 TRLR\n";
        let records = typed(text, GedcomVersion::V5_5_1);
        assert!(!records[0].0, "{records:?}");
        assert!(!records[1].0, "{records:?}");
        assert_eq!(records[2], (true, vec!["STAT".to_string()], vec![]));
        let records = typed(text, GedcomVersion::V7_0);
        assert!(records[1].0, "{records:?}");
    }

    /// Writing a dataset one record at a time writes what the whole
    /// conformance pass writes, with the same repairs, on every fixture in
    /// every version, whatever the output; most fixtures need no restart.
    #[test]
    fn the_direct_write_is_the_whole_pass() {
        let mut files = Vec::new();
        ged_files(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
            &mut files,
        );
        files.sort();
        let restarts = || crate::writer::RESTARTS.with(std::cell::Cell::get);
        let (mut writes, before) = (0, restarts());
        let mut restarted = Vec::new();
        for path in files {
            let data = Dataset::from_bytes(std::fs::read(&path).unwrap());
            for version in [
                GedcomVersion::V5_5_1,
                GedcomVersion::V7_0,
                GedcomVersion::V7_1,
            ] {
                let name = path.display();
                let writer = GedcomWriter::new().gedcom_version(version);
                let at = restarts();
                let direct = writer.write_to_string_with_report(&data).unwrap();
                if restarts() > at {
                    restarted.push(format!("{name} {version}"));
                }
                let whole = writer.write_to_string_whole(&data).unwrap();
                assert_eq!(direct.0, whole.0, "{name} as {version}");
                assert_eq!(direct.1, whole.1, "{name} as {version}");
                // The first repair as the error, under the strict policy.
                let strict = writer.clone().on_nonconformant(crate::RepairPolicy::Error);
                let error = |r: Result<(String, crate::WriteReport), crate::WriteError>| match r {
                    Ok((text, _)) => Ok(text),
                    Err(e) => Err(e.to_string()),
                };
                assert_eq!(
                    error(strict.write_to_string_with_report(&data)),
                    error(strict.write_to_string_whole(&data)),
                    "{name} as {version}"
                );
                writes += 1;
            }
        }
        let restarts = restarts() - before;
        eprintln!("{restarts} of {writes} direct writes restarted: {restarted:#?}");
        assert!(restarts * 4 < writes, "{restarts} of {writes}");
    }

    /// A 5.5.1 extension that converting to 7.x renames to a structure of
    /// texts (`_UID`, `_EMAIL`) is written as that structure where its
    /// record writes itself, its record not converted on its own: what the
    /// whole pass writes, converting every record. What no field takes, has
    /// substructures or escapes, is converted with its record.
    #[test]
    fn renamed_extensions_are_written_where_they_stand() {
        let alone = || crate::writer::ALONE.with(std::cell::Cell::get);
        let writer = GedcomWriter::new().gedcom_version(GedcomVersion::V7_0);
        let adopted = Dataset::parse(
            "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 _UID 0A1B\n\
             1 BIRT\n2 DATE 1 JAN 1900\n2 _UID 0C1D\n1 _UID 0a1b\n0 @R1@ REPO\n\
             1 NAME Sample archive\n1 _EMAIL desk(at)example.invalid\n1 PHON 555\n0 TRLR\n",
        );
        let before = alone();
        let direct = writer.write_to_string_with_report(&adopted).unwrap();
        assert_eq!(alone(), before, "converted on its own");
        assert_eq!(direct, writer.write_to_string_whole(&adopted).unwrap());
        assert!(
            direct
                .0
                .contains("1 BIRT\n2 DATE 1 JAN 1900\n2 UID 0C1D\n1 UID 0A1B\n1 UID 0a1b\n0 @R1@"),
            "{}",
            direct.0
        );
        assert!(
            direct
                .0
                .contains("1 PHON 555\n1 EMAIL desk(at)example.invalid\n")
        );
        for record in [
            "0 @I1@ INDI\n1 _UID 0A1B\n2 _X x\n",
            "0 @I1@ INDI\n1 NAME Ann\n2 _UID 0A1B\n",
            "0 @I1@ INDI\n1 _UID @#DJULIAN@ 0A1B\n",
        ] {
            let data = Dataset::parse(format!("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n{record}0 TRLR\n"));
            let before = alone();
            let direct = writer.write_to_string_with_report(&data).unwrap();
            assert_eq!(alone(), before + 1, "{record}");
            assert_eq!(direct, writer.write_to_string_whole(&data).unwrap());
        }
    }

    /// A dataset whose identifiers are valid and unique is written without
    /// a look at each record's identifier: its index says so, and tells
    /// the record of each. Duplicates, or identifiers the target's grammar
    /// refuses, take the pass.
    #[test]
    fn identifiers_the_index_vouches_for_are_not_checked_again() {
        use crate::spec::conform::ID_PASSES;
        let passes = || ID_PASSES.with(std::cell::Cell::get);
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/washington.ged");
        let data = Dataset::from_bytes(std::fs::read(path).unwrap());
        let before = passes();
        // In its own version family: the records written are its records.
        GedcomWriter::new().write_to_string(&data).unwrap();
        let v7 = Dataset::parse(
            "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 FAMS @F1@\n0 @F1@ FAM\n\
             1 HUSB @I1@\n0 @L1@ _LOC\n0 TRLR\n",
        );
        for version in [GedcomVersion::V7_0, GedcomVersion::V7_1] {
            GedcomWriter::new()
                .gedcom_version(version)
                .write_to_string(&v7)
                .unwrap();
        }
        assert_eq!(passes(), before);

        let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n0 @I1@ INDI\n0 TRLR\n";
        let duplicate = Dataset::parse(text);
        let out = GedcomWriter::new().write_to_string(&duplicate).unwrap();
        assert_eq!(passes(), before + 1);
        assert!(out.contains("0 @I1_2@ INDI\n"), "{out}");

        let mut renamed = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n0 TRLR\n");
        renamed.rename("@I1@", "@i1@").unwrap();
        GedcomWriter::new().write_to_string(&renamed).unwrap();
        assert_eq!(passes(), before + 2);
        renamed.rename("@i1@", "@I1@").unwrap();
        GedcomWriter::new().write_to_string(&renamed).unwrap();
        assert_eq!(passes(), before + 2);
    }

    /// Every `.ged` file under `dir`.
    fn ged_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                ged_files(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ged")) {
                out.push(p);
            }
        }
    }

    /// A dataset's records write their structures where they are, one at a
    /// time: what the writer writes of them is what it writes of the owned
    /// structures they give, in the order of the model, repairs included.
    #[test]
    fn the_model_writes_as_its_owned_structures() {
        let mut files = Vec::new();
        ged_files(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
            &mut files,
        );
        files.sort();
        assert!(files.len() > 20, "{}", files.len());
        for path in files {
            let data = Dataset::from_bytes(std::fs::read(&path).unwrap());
            for version in [
                GedcomVersion::V5_5_1,
                GedcomVersion::V7_0,
                GedcomVersion::V7_1,
            ] {
                let writer = GedcomWriter::new()
                    .gedcom_version(version)
                    .bom(crate::Bom::Never);
                let mut direct = Vec::new();
                let report = writer.write(&mut direct, &data).unwrap();
                let mut owned = Vec::new();
                let expected = writer
                    .write_model_structures(
                        &mut owned,
                        &data.to_structures_for(version),
                        data.version(),
                        super::super::field_order,
                    )
                    .unwrap();
                let name = path.display();
                assert_eq!(
                    String::from_utf8_lossy(&direct),
                    String::from_utf8_lossy(&owned),
                    "{name} as {version}"
                );
                assert_eq!(report.repairs, expected.repairs, "{name} as {version}");
                assert_eq!(
                    writer.write_to_string(&data).unwrap().as_bytes(),
                    direct.as_slice(),
                    "{name} as {version}"
                );
            }
        }
    }
}
