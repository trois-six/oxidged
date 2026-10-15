//! The conformance repair's property: for any tree `t` and version `v`,
//! `validate(conform(t, v), v)` is empty, and a second `conform` changes
//! nothing. The one exception is a 5.5.1 record over 32K that holds no
//! inline note left to move out: the limit is a recommendation (p. 10), and
//! no lossless rewriting makes such a record smaller.
//!
//! The writer, which repairs what it writes, is idempotent: what it writes
//! needs no repair, and read back and written again, is the same bytes —
//! write(read(write(x))) == write(x) — for a tree and for the typed model
//! read from it.
//!
//! Checked on every fixture of the repository, on generated trees (fixed
//! seeds, no dependency), and, opt-in, on the fetched corpora and on a
//! fuzz corpus.

use oxidged::spec::{DeviationKind, conform, validate};
use oxidged::tree::{Payload, Structure, Tag, Tree, Xref};
use oxidged::{Bom, Dataset, GedcomVersion, GedcomWriter};
use std::path::{Path, PathBuf};

/// The versions a dataset is conformed to.
fn versions() -> [GedcomVersion; 3] {
    [
        GedcomVersion::V5_5_1,
        GedcomVersion::V7_0,
        GedcomVersion::V7_1,
    ]
}

/// Whether `s`, at `level`, holds an inline note (a `NOTE` with a text,
/// under standard structures only) longer than the pointer line that would
/// replace it (32 bytes at most).
fn holds_movable_note(s: &Structure, level: u8, v: GedcomVersion) -> bool {
    s.substructures.iter().any(|c| {
        !c.tag.as_str().starts_with('_')
            && (c.tag == "NOTE"
                && matches!(c.payload, Payload::Text(_))
                && c.to_gedcom(level + 1, v).len() > 32
                || holds_movable_note(c, level + 1, v))
    })
}

/// Whether the record of `records` read at `line` is one a 5.5.1 record
/// over 32K may be: one that holds no note to move out (repair j), a
/// header (whose note takes no pointer) or an extension record.
fn is_left(records: &[Structure], line: u32, v: GedcomVersion) -> bool {
    v == GedcomVersion::V5_5_1
        && records.iter().filter(|r| r.line == line).any(|r| {
            r.tag == "HEAD" || r.tag.as_str().starts_with('_') || !holds_movable_note(r, 0, v)
        })
}

/// `None` when the property holds for `records`, else what breaks it.
fn property(name: &str, records: &[Structure]) -> Option<String> {
    for v in versions() {
        let mut t = records.to_vec();
        let repairs = conform(&mut t, v);
        let mut left = validate(&t, v);
        left.retain(|d| !(d.kind == DeviationKind::RecordSize && is_left(&t, d.line, v)));
        if !left.is_empty() {
            let left: Vec<String> = left.iter().take(5).map(ToString::to_string).collect();
            let repairs: Vec<String> = repairs.iter().take(5).map(ToString::to_string).collect();
            return Some(format!("{name} as {v}: {left:?} after {repairs:?}"));
        }
        let again = conform(&mut t, v);
        if !again.is_empty() {
            let again: Vec<String> = again.iter().take(5).map(ToString::to_string).collect();
            return Some(format!("{name} as {v}: a second pass repairs {again:?}"));
        }
        if let Some(why) = idempotent(records, v) {
            return Some(format!("{name} as {v}: {why}"));
        }
    }
    None
}

/// `None` when writing `records` in `v` is idempotent, for the tree and for
/// the typed model read from it; else the first output that is not.
fn idempotent(records: &[Structure], v: GedcomVersion) -> Option<String> {
    let writer = GedcomWriter::new().gedcom_version(v).bom(Bom::Never);
    let write = |records: &[Structure]| {
        let mut out = Vec::new();
        writer.write_structures(&mut out, records).expect("repairs");
        String::from_utf8(out).expect("UTF-8")
    };
    let out = write(records);
    let mut back = Tree::parse(out.as_str()).to_structures();
    let repairs = conform(&mut back, v);
    if !repairs.is_empty() {
        let repairs: Vec<String> = repairs.iter().take(5).map(ToString::to_string).collect();
        return Some(format!("the output needs repairs {repairs:?}\n{out}"));
    }
    let again = write(&back);
    if again != out {
        return Some(format!(
            "the tree is written anew: {}",
            difference(&out, &again)
        ));
    }
    let text: String = records.iter().map(|r| r.to_gedcom(0, v)).collect();
    let out = writer
        .write_to_string(&Dataset::parse(text))
        .expect("repairs");
    let again = writer
        .write_to_string(&Dataset::parse(out.as_str()))
        .expect("repairs");
    (again != out).then(|| format!("the model is written anew: {}", difference(&out, &again)))
}

/// The lines around the first line where `a` and `b` differ.
fn difference(a: &str, b: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let at = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let around = |l: &[&str]| l[at.saturating_sub(4)..(at + 6).min(l.len())].join("\n");
    format!("line {}\n{}\n---\n{}", at + 1, around(&a), around(&b))
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            files_under(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn check_files(files: &[PathBuf]) -> (usize, Vec<String>) {
    let mut wrong = Vec::new();
    let mut n = 0;
    for path in files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        n += 1;
        let records = Tree::from_bytes(bytes).to_structures();
        if let Some(why) = property(&path.display().to_string(), &records) {
            wrong.push(why);
        }
    }
    (n, wrong)
}

#[test]
fn every_fixture_conforms() {
    let mut files = Vec::new();
    files_under(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        &mut files,
    );
    files.retain(|p| p.extension().is_some_and(|e| e == "ged"));
    files.sort();
    let (n, wrong) = check_files(&files);
    assert!(n > 50, "{n} fixtures");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The fixtures of the conformance suite written as text, conformed.
#[test]
fn every_case_conforms() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance/cases");
    let mut wrong = Vec::new();
    let mut n = 0;
    for entry in std::fs::read_dir(dir).expect("cases").flatten() {
        let text = std::fs::read_to_string(entry.path()).expect("case file");
        // Cases are separated by `### <id>` headers; directives start with
        // `#`.
        for case in text.split("\n### ").skip(1) {
            let (id, body) = case.split_once('\n').unwrap_or((case, ""));
            n += 1;
            let body: String = body
                .lines()
                .filter(|l| !l.starts_with('#'))
                .map(|l| format!("{}\n", tokens(l)))
                .collect();
            let records = Tree::parse(body).to_structures();
            if let Some(why) = property(id.trim(), &records) {
                wrong.push(why);
            }
        }
    }
    assert!(n > 100, "{n} cases");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A case line with its `<TAB>`, `<BOM>`, `<CR>`, `<CRLF>`, `<NOEOL>` and
/// `<U+XXXX>` tokens replaced.
fn tokens(line: &str) -> String {
    let mut out = line
        .replace("<TAB>", "\t")
        .replace("<BOM>", "\u{feff}")
        .replace("<CRLF>", "\r")
        .replace("<CR>", "\r")
        .replace("<NOEOL>", "");
    while let Some(at) = out.find("<U+") {
        let Some(end) = out[at..].find('>') else {
            break;
        };
        let c = u32::from_str_radix(&out[at + 3..at + end], 16)
            .ok()
            .and_then(char::from_u32)
            .unwrap_or('?');
        out.replace_range(at..at + end + 1, &c.to_string());
    }
    out
}

/// A xorshift generator: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const TAGS: &[&str] = &[
    "HEAD", "TRLR", "INDI", "FAM", "SOUR", "REPO", "OBJE", "NOTE", "SNOTE", "SUBM", "SUBN", "GEDC",
    "VERS", "FORM", "CHAR", "NAME", "SEX", "BIRT", "DEAT", "DATE", "PLAC", "AGE", "TIME", "FAMC",
    "FAMS", "HUSB", "WIFE", "CHIL", "PEDI", "STAT", "RESN", "LANG", "MIME", "TRAN", "FILE", "MAP",
    "LATI", "LONG", "EVEN", "TYPE", "ASSO", "ROLE", "RELA", "PHRASE", "SCHMA", "TAG", "SLGC",
    "BAPL", "CHAN", "CREA", "EXID", "UID", "REFN", "QUAY", "PAGE", "TEXT", "DATA", "CONT", "CONC",
    "NCHI", "CROP", "TOP", "MEDI", "CALN", "ADDR", "PHON", "_X", "_ext", "name", "", "FOO",
];

const PAYLOADS: &[&str] = &[
    "",
    "Y",
    "N",
    "M",
    "male",
    "7.0",
    "5.5.1",
    "1 JAN 1900",
    "30 FEB 1900",
    "BET 1900",
    "ABT @#DJULIAN@ 1700",
    "79y",
    "79",
    "> 1y",
    "12:00",
    "25:00",
    "en",
    "English",
    "en US",
    "text/plain",
    "image/png",
    "media/a.jpg",
    "a b\\c",
    "N1.5",
    "X1",
    "CONFIDENTIAL, LOCKED",
    "confidential",
    "BIRTH",
    "birth",
    "OTHER",
    "_EXTVAL",
    "@ at",
    "a @ b",
    "tab\there",
    "bell\u{7}",
    "line\nbreak",
    "John /Doe/",
    "a/b/c/d",
    "_TAG https://example.com/t",
    "_ADDR x",
    "LINEAGE-LINKED",
    "UTF-8",
    "ANSI",
    "long text ",
];

const XREFS: &[&str] = &[
    "@I1@", "@F1@", "@S1@", "@N1@", "@U1@", "@i1@", "@VOID@", "@X 1@",
];

fn node(rng: &mut Rng, depth: usize) -> Structure {
    let tag = rng.pick(TAGS);
    let payload = match rng.below(5) {
        0 => Payload::None,
        1 => Payload::Pointer(Xref::new(rng.pick(XREFS))),
        _ => {
            let p = rng.pick(PAYLOADS);
            if p.is_empty() {
                Payload::None
            } else if p == "long text " {
                Payload::Text(p.repeat(40).into())
            } else {
                Payload::Text(p.into())
            }
        }
    };
    let mut s = Structure {
        tag: Tag::new(tag),
        payload,
        ..Structure::default()
    };
    if rng.below(12) == 0 {
        s.xref = Some(Xref::new(rng.pick(XREFS)));
    }
    if depth < 5 {
        for _ in 0..rng.below(5) {
            s.substructures.push(node(rng, depth + 1));
        }
    }
    s
}

fn dataset(rng: &mut Rng) -> Vec<Structure> {
    let mut out = Vec::new();
    if rng.below(8) != 0 {
        let mut head = Structure::new("HEAD");
        let mut gedc = Structure::new("GEDC");
        gedc.substructures.push(Structure {
            payload: Payload::Text(rng.pick(&["5.5.1", "7.0", "7.1", "5.5"]).into()),
            ..Structure::new("VERS")
        });
        head.substructures.push(gedc);
        for _ in 0..rng.below(3) {
            head.substructures.push(node(rng, 1));
        }
        out.push(head);
    }
    for _ in 0..rng.below(8) {
        let mut r = node(rng, 0);
        if rng.below(3) != 0 {
            r.tag = Tag::new(rng.pick(&[
                "INDI", "FAM", "SOUR", "OBJE", "SUBM", "SNOTE", "NOTE", "REPO", "_R",
            ]));
            r.xref = Some(Xref::new(rng.pick(XREFS)));
        }
        out.push(r);
    }
    if rng.below(8) != 0 {
        out.push(Structure::new("TRLR"));
    }
    out
}

#[test]
fn generated_trees_conform() {
    let mut wrong = Vec::new();
    for seed in 1..=800_u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let records = dataset(&mut rng);
        if let Some(why) = property(&format!("seed {seed}"), &records) {
            wrong.push(why);
            if wrong.len() > 10 {
                break;
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

fn corpora_dir() -> PathBuf {
    std::env::var_os("CORPORA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/corpora"))
}

/// Every file of the fetched corpora (FamilySearch, gedcom4j, Gramps).
#[test]
#[ignore = "opt-in: run tools/fetch-corpora.sh, then cargo test --test spec_conform -- --ignored"]
fn every_corpus_file_conforms() {
    let mut files = Vec::new();
    for corpus in ["familysearch", "gedcom4j", "gramps"] {
        files_under(&corpora_dir().join(corpus), &mut files);
    }
    files.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ged")));
    files.sort();
    let (n, wrong) = check_files(&files);
    assert!(n > 100, "{n} corpus files: fetch them first");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every input of a fuzz corpus: `$FUZZ_CORPUS`, else the cargo-fuzz
/// corpora under `fuzz/corpus/`.
#[test]
#[ignore = "opt-in: run a fuzz target first, or set FUZZ_CORPUS"]
fn every_fuzz_input_conforms() {
    let dir = std::env::var_os("FUZZ_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/corpus"));
    let mut files = Vec::new();
    files_under(&dir, &mut files);
    files.sort();
    let (n, wrong) = check_files(&files);
    assert!(n > 0, "no fuzz input under {}", dir.display());
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The tree writer repairs structures before it writes lines: its output
/// validates, here on the suite's G7-DANGLING and G7-PTR-WRONG-TYPE inputs
/// (whose typed-model rows wait for the model writer's switch to the same
/// path).
#[test]
fn the_tree_writer_conforms() {
    use oxidged::spec::{RepairKind, validate_text};
    use oxidged::{GedcomWriter, RepairPolicy};
    for (input, kept) in [
        (
            "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 FAMS @F9@\n0 TRLR\n",
            "1 _FAMS @@F9@\n",
        ),
        (
            "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SUBM @I2@\n0 @I2@ INDI\n1 SEX M\n0 TRLR\n",
            "1 _SUBM @I2@\n",
        ),
    ] {
        let tree = Tree::parse(input);
        let mut out = Vec::new();
        let report = GedcomWriter::new().write_tree(&mut out, &tree).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(report.repairs.len(), 1, "{:?}", report.repairs);
        assert_eq!(report.repairs[0].kind, RepairKind::Pointer);
        assert!(text.contains(kept), "{text}");
        assert_eq!(
            validate_text(text.trim_start_matches('\u{feff}')),
            [],
            "{text}"
        );
        // The same through owned structures; and an error under
        // RepairPolicy::Error.
        let mut out = Vec::new();
        let report = GedcomWriter::new()
            .write_structures(&mut out, &tree.to_structures())
            .unwrap();
        assert_eq!(report.repairs.len(), 1);
        let err = GedcomWriter::new()
            .on_nonconformant(RepairPolicy::Error)
            .write_tree(Vec::new(), &tree);
        assert!(err.is_err());
    }
}

/// Valid 5.5.1 identifiers are written as they are on the tree path: spaces,
/// `!`, `:` and a leading `_`.
#[test]
fn the_tree_writer_keeps_valid_551_identifiers() {
    use oxidged::GedcomWriter;
    let input = "0 HEAD\n1 SOUR EXAMPLE\n1 SUBM @U1@\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n0 @U1@ SUBM\n1 NAME Example\n0 @_X 1@ INDI\n1 FAMS @A:B@\n0 @A:B@ FAM\n1 HUSB @_X 1@\n0 @I1!2@ _NOTE x\n0 TRLR\n";
    let mut out = Vec::new();
    let report = GedcomWriter::new()
        .write_tree(&mut out, &Tree::parse(input))
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(report.repairs, [], "{text}");
    for line in [
        "0 @_X 1@ INDI",
        "1 FAMS @A:B@",
        "0 @A:B@ FAM",
        "1 HUSB @_X 1@",
        "0 @I1!2@ _NOTE x",
    ] {
        assert!(text.contains(line), "{line}: {text}");
    }
}
