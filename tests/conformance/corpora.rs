//! Opt-in tier: external corpora that cannot be vendored (no licence, GPL,
//! non-commercial, CC-BY-SA or third-party content). They are pinned by
//! commit and SHA-256 in `tests/fixtures/corpora.lock.tsv` and fetched by
//! `tools/fetch-corpora.sh` into `target/corpora/` (or `$CORPORA_DIR`).
//!
//! ```sh
//! tools/fetch-corpora.sh --registries
//! cargo test --all-features --test conformance -- --ignored
//! ```
//!
//! When run, a missing file is a failure, not a skip.
//!
//! * FamilySearch GEDCOM.io test files (7.0 and 7.1, `.ged` and `.gdz`):
//!   conformance cases (`corpus/familysearch/…`) through the ratchet; the 7.1
//!   files are written back as 7.1.
//! * gedcom4j and Gramps samples (real-world dialects): accepted without a
//!   fatal error, written conformant, nothing lost, measured against the
//!   per-file baseline `corpora_baseline.tsv`, which may only go down.
//! * The specification tables are regenerated from the fetched inputs and
//!   compared with the committed ones; the 5.5.1 table is cross-checked
//!   against GEDCOM-registries.

use crate::support::adapter::{self, Target as WriteTarget};
use crate::support::cases::{self, Case, Kind};
use crate::support::checker::{self, Target};
use crate::support::ratchet::{self, Failure};
use crate::support::{semantic, tree};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const LOCK: &str = include_str!("../fixtures/corpora.lock.tsv");
const BASELINE: &str = include_str!("../fixtures/conformance/corpora_baseline.tsv");

/// The baseline, and its file name.
fn baseline() -> (&'static str, &'static str) {
    (BASELINE, "corpora_baseline.tsv")
}

fn root() -> PathBuf {
    std::env::var_os("CORPORA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/corpora"))
}

/// (path in the lock, local file) of a corpus.
fn files(corpus: &str) -> Vec<(String, PathBuf)> {
    LOCK.lines()
        .skip(1)
        .map(|l| l.split('\t').collect::<Vec<_>>())
        .filter(|f| f[0] == corpus)
        .map(|f| (f[1].to_string(), root().join(f[0]).join(f[1])))
        .collect()
}

fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p)
        .unwrap_or_else(|e| panic!("{}: {e}; run tools/fetch-corpora.sh first", p.display()))
}

#[test]
#[ignore = "opt-in: run tools/fetch-corpora.sh, then cargo test --all-features --test conformance -- --ignored"]
fn familysearch_test_files() {
    let mut failures = Vec::new();
    let mut n = 0;
    for (path, file) in files("familysearch") {
        let bytes = read(&file);
        let name = path.trim_start_matches("testfiles/");
        n += 1;
        if path.ends_with(".gdz") {
            if cfg!(feature = "gedzip") {
                failures.extend(gedzip(name, &bytes));
            }
            continue;
        }
        let mut c = Case::new(name, Kind::C, bytes);
        c.purpose = "FamilySearch GEDCOM.io test file".into();
        failures.extend(cases::run("corpus/familysearch", &c).failures);
    }
    let skipped: &[&str] = if cfg!(feature = "gedzip") {
        &[]
    } else {
        &[
            "corpus/familysearch/gedcom70/maximal70.gdz",
            "corpus/familysearch/gedcom71/maximal71.gdz",
        ]
    };
    ratchet::verify_skipping("corpus", n, failures, skipped);
}

#[cfg(feature = "gedzip")]
fn gedzip(name: &str, bytes: &[u8]) -> Vec<Failure> {
    let id = format!("corpus/familysearch/{name}");
    let mut out = Vec::new();
    match adapter::read_gedzip(bytes) {
        Ok(m) => match adapter::write(&m, WriteTarget::Same) {
            Ok(o) => {
                let t = Target::from_vers(&tree::declared_vers(&o).unwrap_or_default());
                for i in checker::check(&o, t, None) {
                    out.push(Failure::new(
                        &id,
                        format!("output:{}", i.rule),
                        "NONCONFORMANT",
                        i.to_string(),
                    ));
                }
            }
            Err(e) => out.push(Failure::new(&id, "write", "FATAL", e)),
        },
        Err(e) => out.push(Failure::new(
            &id,
            "parse",
            if e.starts_with("PANIC") {
                "PANIC"
            } else {
                "FATAL"
            },
            e,
        )),
    }
    out
}

#[cfg(not(feature = "gedzip"))]
fn gedzip(_: &str, _: &[u8]) -> Vec<Failure> {
    Vec::new()
}

/// One measured file: outcome, payload lines lost, output rules violated.
#[derive(Debug, PartialEq, Eq)]
struct Measure {
    outcome: String,
    lost: usize,
    rules: Vec<&'static str>,
}

fn measure(bytes: &[u8]) -> Measure {
    let fail = |outcome: String| Measure {
        outcome,
        lost: 0,
        rules: Vec::new(),
    };
    let m = match adapter::read(bytes) {
        Ok(m) => m,
        Err(e) if e.starts_with("PANIC") => return fail("PANIC".into()),
        Err(_) => return fail("FATAL".into()),
    };
    let out = match adapter::write(&m, WriteTarget::Same) {
        Ok(o) => o,
        Err(_) => return fail("WRITE".into()),
    };
    let input = tree::decode_bytes(bytes);
    let lost = semantic::lost(&input, &out)
        .iter()
        .map(|l| {
            l.rsplit("(x")
                .next()
                .and_then(|n| n.trim_end_matches(')').parse::<usize>().ok())
                .unwrap_or(1)
        })
        .sum();
    let t = Target::from_vers(&tree::declared_vers(&out).unwrap_or_default());
    let mut rules: Vec<&'static str> = checker::check(&out, t, None)
        .into_iter()
        .map(|i| i.rule)
        .collect();
    rules.sort_unstable();
    rules.dedup();
    let outcome = if adapter::read(out.as_bytes()).is_ok() {
        "OK"
    } else {
        "REPARSE"
    };
    Measure {
        outcome: outcome.into(),
        lost,
        rules,
    }
}

/// gedcom4j and Gramps samples against `corpora_baseline.tsv`. A file that does better than its
/// baseline fails too, so the baseline only goes down;
/// `CORPORA_BASELINE_WRITE=1` rewrites it.
#[test]
#[ignore = "opt-in: run tools/fetch-corpora.sh, then cargo test --all-features --test conformance -- --ignored"]
fn real_world_dialects() {
    let (baseline, baseline_file) = baseline();
    let mut base: BTreeMap<String, Measure> = BTreeMap::new();
    for l in baseline
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("file\t") && !l.is_empty())
    {
        let f: Vec<&str> = l.split('\t').collect();
        let rules = if f[3].is_empty() {
            Vec::new()
        } else {
            f[3].split(',')
                .map(|r| &*Box::leak(r.to_string().into_boxed_str()))
                .collect()
        };
        base.insert(
            f[0].into(),
            Measure {
                outcome: f[1].into(),
                lost: f[2].parse().unwrap(),
                rules,
            },
        );
    }
    let mut now = BTreeMap::new();
    for corpus in ["gedcom4j", "gramps"] {
        for (path, file) in files(corpus) {
            now.insert(format!("{corpus}/{path}"), measure(&read(&file)));
        }
    }
    if std::env::var_os("CORPORA_BASELINE_WRITE").is_some() {
        let mut s = String::from(
            "file\toutcome\tlost\trules\n\
             # Per-file baseline of the opt-in real-world corpora (gedcom4j, Gramps): read outcome (OK, FATAL,\n\
             # PANIC, REPARSE: the written output does not read back), number of input payloads and structures\n\
             # missing from the written output, and the output-checker rules the output violates. Regenerate\n\
             # with CORPORA_BASELINE_WRITE=1; every change must be an improvement.\n",
        );
        for (k, m) in &now {
            s += &format!("{k}\t{}\t{}\t{}\n", m.outcome, m.lost, m.rules.join(","));
        }
        std::fs::write(crate::support::fixture(baseline_file), s).unwrap();
        return;
    }
    let mut diffs = Vec::new();
    for (k, m) in &now {
        match base.get(k) {
            None => diffs.push(format!("{k}: no baseline")),
            Some(b) if b != m => diffs.push(format!("{k}: baseline {b:?}, now {m:?}")),
            _ => {}
        }
    }
    assert!(
        diffs.is_empty(),
        "{} files differ from {baseline_file} (regenerate it if every change is an improvement):\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

/// The committed tables are what the generator makes of the pinned inputs.
#[test]
#[ignore = "opt-in: needs python3 and tools/fetch-corpora.sh spec"]
fn spec_tables_are_current() {
    let st = std::process::Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/spec-tables/gen.py"))
        .arg("--check")
        .arg("--inputs")
        .arg(root().join("spec"))
        .status()
        .expect("python3");
    assert!(
        st.success(),
        "spec tables are out of date: run tools/spec-tables/gen.py"
    );
}

/// The 5.5.1 table agrees with GEDCOM-registries except where the PDF
/// settles a disagreement (`tools/spec-tables/551-registries-diff.txt`).
#[test]
#[ignore = "opt-in: needs python3 and tools/fetch-corpora.sh --registries"]
fn spec_551_matches_the_registries() {
    let out = std::process::Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/spec-tables/gen.py"))
        .arg("--inputs")
        .arg(root().join("spec"))
        .arg("--crosscheck-551")
        .arg(root().join("registries"))
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = String::from_utf8(out.stdout).unwrap();
    let want: String = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/spec-tables/551-registries-diff.txt"),
    )
    .unwrap()
    .lines()
    .filter(|l| !l.starts_with('#') && !l.is_empty())
    .map(|l| format!("{l}\n"))
    .collect();
    assert_eq!(got, want);
}
