//! Differential run between the published ged_io 0.17.0 and this checkout.
//!
//! For each file (duplicates by content skipped), both versions read it and
//! write it back in the version it declares. What 0.17 kept of the input
//! is the baseline: every payload and tag of its output that the input
//! holds must be in this checkout's output too, compared as the
//! conformance suite's "nothing lost" check compares
//! (`tests/conformance/support/semantic.rs`: an extension tag counts as its
//! standard one, values compare by meaning). What 0.17 wrote that the input
//! does not hold (a surname piece derived from the name) is not data it
//! kept; its text is compared in Unicode NFC (0.17 left ANSEL marks
//! uncomposed), through the crate's ANSEL tables, and a tab as a space
//! (5.5.1 permits no control character: its writer writes a tab as a
//! space). A file 0.17 read and this checkout does not read is a
//! regression too.
//!
//! What this checkout writes differently on purpose is listed in
//! `tools/differential/allow.tsv` — the file (a `*` matches any run of
//! characters), the lost item, and the reviewed reason — and does not
//! count; an allowed row that matches nothing is reported, so the list
//! cannot go stale.
//!
//! ```sh
//! cargo run --release --manifest-path tools/differential/Cargo.toml -- target/corpora/**/*.ged
//! ```
//!
//! `DIFFERENTIAL_DUMP=<dir>` also writes both outputs of each file there
//! (`<file>.old`, `<file>.new`), for review.

use std::collections::{BTreeSet, HashSet};
use std::process::ExitCode;

#[allow(dead_code)]
#[path = "../../../tests/conformance/support/semantic.rs"]
mod semantic;
#[allow(dead_code)]
#[path = "../../../tests/conformance/support/tree.rs"]
mod tree;

/// The reviewed intended changes.
const ALLOW: &str = include_str!("../allow.tsv");

fn published(bytes: &[u8]) -> Result<String, String> {
    let d = ged_io_published::GedcomBuilder::new()
        .build_from_bytes(bytes)
        .map_err(|e| e.to_string())?;
    ged_io_published::GedcomWriter::new()
        .write_to_string(&d)
        .map_err(|e| e.to_string())
}

/// `text` with its characters composed as the crate's ANSEL decoding
/// composes them (Unicode NFC over the ANSEL repertoire), line by line (a
/// line ANSEL cannot hold stays as it is), and its tabs as spaces.
fn composed(text: &str) -> String {
    use oxidged::encoding::{GedcomEncoding, decode_as, encode};
    text.replace('\t', " ")
        .lines()
        .map(|line| match encode(line, GedcomEncoding::Ansel) {
            Ok(bytes) => decode_as(&bytes, GedcomEncoding::Ansel),
            Err(_) => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The items of `lost` and their counts (`text Ann (x2)`).
fn counted(lost: Vec<String>) -> Vec<(String, usize)> {
    lost.into_iter()
        .map(|item| match item.rsplit_once(" (x") {
            Some((name, n)) => (
                name.to_string(),
                n.trim_end_matches(')').parse().unwrap_or(1),
            ),
            None => (item, 1),
        })
        .collect()
}

/// What `old` kept of `input` and `new` lost.
fn kept_and_lost(input: &str, old: &str, new: &str) -> Vec<String> {
    let invented: std::collections::HashMap<String, usize> =
        counted(semantic::lost(old, input)).into_iter().collect();
    counted(semantic::lost(old, new))
        .into_iter()
        .filter_map(|(item, n)| {
            let n = n.saturating_sub(invented.get(&item).copied().unwrap_or(0));
            (n > 0).then(|| format!("{item} (x{n})"))
        })
        .collect()
}

fn current(bytes: &[u8]) -> Result<String, String> {
    let d = oxidged::GedcomBuilder::new()
        .build_from_bytes(bytes)
        .map_err(|e| e.to_string())?;
    oxidged::GedcomWriter::new()
        .write_to_string(&d)
        .map_err(|e| e.to_string())
}

/// `*` matches any run of characters.
fn glob(pattern: &str, s: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, middle @ .., last] = parts.as_slice() else {
        return pattern == s;
    };
    let Some(mut rest) = s.strip_prefix(first) else {
        return false;
    };
    for p in middle {
        match rest.find(p) {
            Some(at) => rest = &rest[at + p.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// The allowed rows: file pattern, item pattern, reason.
fn allowed() -> Vec<(&'static str, &'static str)> {
    ALLOW
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some((f.next()?, f.next()?))
        })
        .collect()
}

fn main() -> ExitCode {
    let allow = allowed();
    let mut used = vec![false; allow.len()];
    let mut regressions = 0;
    let mut allowed_losses = 0;
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    let (mut files, mut now_reads) = (0, 0);
    let mut paths: BTreeSet<String> = std::env::args().skip(1).collect();
    while let Some(path) = paths.pop_first() {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{path}: {e}");
                return ExitCode::FAILURE;
            }
        };
        if !seen.insert(bytes.clone()) {
            continue;
        }
        files += 1;
        let name = path.rsplit('/').next().unwrap_or(&path);
        match (published(&bytes), current(&bytes)) {
            (Ok(old), Ok(new)) => {
                if let Ok(dir) = std::env::var("DIFFERENTIAL_DUMP") {
                    let _ = std::fs::write(format!("{dir}/{name}.old"), &old);
                    let _ = std::fs::write(format!("{dir}/{name}.new"), &new);
                }
                let input = oxidged::encoding::decode(&bytes).text;
                let new = new.replace('\t', " ");
                for item in kept_and_lost(&input, &composed(&old), &new) {
                    match allow
                        .iter()
                        .position(|(f, i)| glob(f, name) && glob(i, &item))
                    {
                        Some(at) => {
                            used[at] = true;
                            allowed_losses += 1;
                        }
                        None => {
                            regressions += 1;
                            println!("{name}\tlost\t{item}");
                        }
                    }
                }
            }
            (Ok(_), Err(e)) => {
                regressions += 1;
                println!("{name}\tnow fails\t{e}");
            }
            (Err(_), Ok(_)) => now_reads += 1,
            (Err(_), Err(_)) => {}
        }
    }
    for ((file, item), used) in allow.iter().zip(&used) {
        if !used {
            println!("allow.tsv\tunused\t{file}\t{item}");
            regressions += 1;
        }
    }
    eprintln!(
        "{files} distinct files: {now_reads} that 0.17 could not read now read; \
         {allowed_losses} reviewed changes; {regressions} regressions"
    );
    if regressions == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
