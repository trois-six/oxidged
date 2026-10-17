//! The `oxidged` binary, command by command: what each prints or writes,
//! its JSON, its exit codes and its usage errors (exit code 3). Every name
//! and place is fictitious.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use oxidged::{GedcomBuilder, GedcomVersion};

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// A file in the temporary directory, removed when dropped.
struct TempFile(PathBuf);

impl TempFile {
    fn new(contents: impl AsRef<[u8]>) -> Self {
        Self::with_extension(contents, "ged")
    }

    fn with_extension(contents: impl AsRef<[u8]>, extension: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "oxidged_cli_test_{}_{}.{extension}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, contents).expect("write a temporary file");
        TempFile(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxidged"))
        .args(args)
        .output()
        .expect("run the oxidged binary")
}

/// Runs the binary with `input` on its standard input.
fn run_with_input(args: &[&str], input: &[u8]) -> Output {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxidged"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run the oxidged binary");
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

/// The JSON a command printed.
fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(output)))
}

/// A path in the temporary directory, removed when dropped, not created.
fn temp_path(extension: &str) -> TempFile {
    let file = TempFile::with_extension("", extension);
    let _ = std::fs::remove_file(&file.0);
    file
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A conformant 5.5.1 file.
const VALID_551: &str = "0 HEAD\n1 SOUR EXAMPLE\n1 SUBM @U1@\n1 GEDC\n2 VERS 5.5.1\n\
                         2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n0 @U1@ SUBM\n\
                         1 NAME Example Submitter\n0 TRLR\n";

/// A small 5.5.1 family: two parents and a child.
const FAMILY: &str = "0 HEAD\n1 SOUR EXAMPLE\n1 SUBM @U1@\n1 GEDC\n2 VERS 5.5.1\n\
                      2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n0 @U1@ SUBM\n1 NAME Example Submitter\n\
                      0 @I1@ INDI\n1 NAME Ann /Example/\n1 SEX F\n1 BIRT\n2 DATE 1 JAN 1900\n\
                      2 PLAC Sampleton\n1 FAMS @F1@\n\
                      0 @I2@ INDI\n1 NAME Bob /Sample/\n1 SEX M\n1 DEAT\n2 DATE 1950\n1 FAMS @F1@\n\
                      0 @I3@ INDI\n1 NAME Cleo /Sample/\n1 SEX F\n1 FAMC @F1@\n\
                      0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I1@\n1 CHIL @I3@\n\
                      0 @N1@ NOTE A shared note\n0 TRLR\n";

// Inspecting a file

#[test]
fn info_counts_the_records() {
    let file = TempFile::new(FAMILY);
    let output = run(&["info", file.path()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.starts_with("GEDCOM 5.5.1 (declared 5.5.1)\nformat: GEDCOM\n"),
        "{text}"
    );
    for line in [
        "encoding: UTF-8 (declared UTF-8)\n",
        "producer: generic",
        "individuals: 3\n",
        "families: 1\n",
        "shared notes: 1\n",
        "submitters: 1\n",
        "sources: 0\n",
        "pointers to no record: 0\n",
        "deviations: 0\n",
    ] {
        assert!(text.contains(line), "{line:?} in\n{text}");
    }
    let dangling = TempFile::new(FAMILY.replace("1 CHIL @I3@", "1 CHIL @I9@"));
    let output = run(&["--json", "info", dangling.path()]);
    let info = json(&output);
    assert_eq!(info["records"]["individuals"], 3);
    assert_eq!(info["dangling_pointers"][0]["pointer"], "@I9@");
    assert_eq!(info["dangling_pointers"][0]["tag"], "CHIL");
    assert_eq!(info["encoding"], "UTF-8");
}

#[test]
fn show_describes_individuals_and_families_and_prints_any_record() {
    let file = TempFile::new(FAMILY);
    let output = run(&["show", file.path(), "@I1@"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "@I1@ Ann Example\n  sex: female\n  born: 1 JAN 1900, Sampleton\n\
         \x20 partner: @I2@ Bob Sample ( – 1950)\n  child: @I3@ Cleo Sample\n"
    );
    let text = stdout(&run(&["show", file.path(), "@I3@", "@F1@"]));
    assert!(
        text.contains(
            "  child of: @I2@ Bob Sample ( – 1950) and @I1@ Ann Example (1 JAN 1900 – )\n"
        ),
        "{text}"
    );
    assert!(text.contains("@F1@ family\n  partner: @I2@"), "{text}");
    let text = stdout(&run(&["show", "--gedcom", file.path(), "@N1@", "@I3@"]));
    assert_eq!(
        text,
        "0 @N1@ NOTE A shared note\n0 @I3@ INDI\n1 NAME Cleo /Sample/\n1 SEX F\n1 FAMC @F1@\n"
    );
    let shown = json(&run(&["--json", "show", file.path(), "@I3@"]));
    assert_eq!(shown[0]["child_of"][0]["parents"][1]["name"], "Ann Example");
    // No such record: an input error.
    let output = run(&["show", file.path(), "@I9@"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no record @I9@"));
}

#[test]
fn search_by_name_surname_and_given_name() {
    let file = TempFile::new(FAMILY);
    let names = |args: &[&str]| -> Vec<String> {
        let mut all = vec!["search", file.path()];
        all.extend(args);
        let output = run(&all);
        assert!(output.status.success(), "{}", stderr(&output));
        stdout(&output).lines().map(str::to_string).collect()
    };
    // Case-insensitive, on the surname or on the given names.
    assert_eq!(
        names(&["--surname", "sample"]),
        ["@I2@ Bob Sample ( – 1950)", "@I3@ Cleo Sample"]
    );
    assert_eq!(
        names(&["--given", "ANN"]),
        ["@I1@ Ann Example (1 JAN 1900 – )"]
    );
    assert_eq!(
        names(&["--surname", "Sample", "--given", "cleo"]),
        ["@I3@ Cleo Sample"]
    );
    assert_eq!(names(&["bob sam"]), ["@I2@ Bob Sample ( – 1950)"]);
    assert!(names(&["--surname", "Nobody"]).is_empty());
    let output = run(&["search", file.path()]);
    assert_eq!(output.status.code(), Some(3));
}

/// Three generations: Gus (@I7@) is the father of Bob (@I2@), Bob and Ann
/// (@I1@) the parents of Cleo (@I3@), Cleo the mother of Dan (@I4@).
fn generations() -> String {
    FAMILY
        .replace("1 SEX M\n1 DEAT", "1 SEX M\n1 FAMC @F0@\n1 DEAT")
        .replace(
            "0 @N1@",
            "0 @I7@ INDI\n1 NAME Gus /Sample/\n1 FAMS @F0@\n\
             0 @I4@ INDI\n1 NAME Dan /Sample/\n1 FAMC @F2@\n\
             0 @F0@ FAM\n1 HUSB @I7@\n1 CHIL @I2@\n\
             0 @F2@ FAM\n1 WIFE @I3@\n1 CHIL @I4@\n0 @N1@",
        )
        .replace("1 SEX F\n1 FAMC @F1@", "1 SEX F\n1 FAMC @F1@\n1 FAMS @F2@")
}

#[test]
fn ancestors_and_descendants() {
    let file = TempFile::new(generations());
    let output = run(&["ancestors", file.path(), "@I4@"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "1 @I3@ Cleo Sample\n2 @I2@ Bob Sample ( – 1950)\n2 @I1@ Ann Example (1 JAN 1900 – )\n\
         3 @I7@ Gus Sample\n"
    );
    let output = run(&["descendants", file.path(), "@I7@", "--generations", "2"]);
    assert_eq!(
        stdout(&output),
        "1 @I2@ Bob Sample ( – 1950)\n2 @I3@ Cleo Sample\n"
    );
    let relatives = json(&run(&["--json", "descendants", file.path(), "@I1@"]));
    assert_eq!(relatives[1]["xref"], "@I4@");
    assert_eq!(relatives[1]["generation"], 2);
    let output = run(&["ancestors", file.path(), "@F1@"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no individual @F1@"));
}

#[test]
fn subset_writes_a_branch() {
    let file = TempFile::new(generations());
    let output = run(&["subset", file.path(), "--descendants-of", "@I2@"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    let records: Vec<&str> = out.lines().filter(|l| l.starts_with("0 @")).collect();
    assert_eq!(
        records,
        [
            "0 @U1@ SUBM",
            "0 @I2@ INDI",
            "0 @I3@ INDI",
            "0 @I4@ INDI",
            "0 @F1@ FAM",
            "0 @F2@ FAM"
        ]
    );
    // Bob's parents' family and Ann are left out, and the pointers to them.
    assert!(!out.contains("@F0@") && !out.contains("@I1@"), "{out}");
    assert!(oxidged::spec::validate_text(&out).is_empty(), "{out}");
    let output = run(&[
        "subset",
        file.path(),
        "--ancestors-of",
        "@I4@",
        "--generations",
        "1",
        "-i",
        "@I7@",
    ]);
    let out = stdout(&output);
    assert!(
        out.contains("0 @I3@ INDI") && out.contains("0 @I7@ INDI"),
        "{out}"
    );
    assert!(!out.contains("0 @I2@ INDI"), "{out}");
    let output = run(&["subset", file.path()]);
    assert_eq!(output.status.code(), Some(3));
}

// Writing

#[test]
fn write_7_0_reads_back_and_validates() {
    let file = TempFile::new(FAMILY.replace("2 DATE 1950", "2 DATE @#DJULIAN@ 1950"));
    let output = run(&["write", file.path(), "--gedcom-version", "7.0"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let written = stdout(&output);
    assert!(written.starts_with("\u{feff}0 HEAD\n1 GEDC\n2 VERS 7.0\n"));
    assert!(written.contains("2 DATE JULIAN 1950\n"), "{written}");
    assert!(
        written.contains("0 @N1@ SNOTE A shared note\n"),
        "{written}"
    );
    let data = GedcomBuilder::new()
        .strict(true)
        .build_from_str(written.as_str())
        .unwrap_or_else(|e| panic!("{e}\n{written}"));
    assert_eq!(data.version(), GedcomVersion::V7_0);
    assert_eq!(data.individuals().len(), 3);
    let rewritten = TempFile::new(written.as_bytes());
    let output = run(&["validate", rewritten.path()]);
    assert!(output.status.success(), "{}", stdout(&output));
    assert_eq!(stdout(&output), "conformant\n");
}

/// A 5.5.1 file written as 7.0 is converted; its submission goes, unless
/// kept as a declared extension.
#[test]
fn write_converts_and_keeps_obsolete_structures_on_request() {
    let file = TempFile::new(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 SUBN @B1@\n0 @I1@ INDI\n1 ASSO @I2@\n2 RELA witness\n\
         0 @I2@ INDI\n0 @B1@ SUBN\n1 FAMF Example\n0 TRLR\n",
    );
    let output = run(&["write", file.path(), "-g", "7.0"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let written = stdout(&output);
    assert!(written.contains("1 ASSO @I2@\n2 ROLE WITN\n"), "{written}");
    assert!(
        !written.contains("SUBN") && !written.contains("FAMF"),
        "{written}"
    );
    let output = run(&["write", file.path(), "-g", "7.0", "--keep-obsolete"]);
    let written = stdout(&output);
    assert!(
        written.contains("0 @B1@ _SUBMISSION\n1 FAMF Example\n"),
        "{written}"
    );
}

#[test]
fn write_lists_the_repairs_or_refuses_them() {
    let file = TempFile::new(FAMILY.replace("1 SEX F\n1 FAMC", "1 SEX female\n1 FAMC"));
    let output = run(&["write", file.path(), "-g", "7.0"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let repairs = stderr(&output);
    assert!(
        repairs.lines().all(|l| l.starts_with("repair: ")),
        "{repairs}"
    );
    assert!(repairs.contains("SEX"), "{repairs}");
    // The value 7.0 does not permit is kept as an extension.
    assert!(stdout(&output).contains("1 _SEX female\n"));
    let output = run(&["--json", "write", file.path(), "-g", "7.0"]);
    let report: serde_json::Value = serde_json::from_str(stderr(&output).trim()).unwrap();
    assert!(
        report["repairs"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("SEX")
    );
    // Refused: exit code 2, nothing written.
    let output = run(&["write", file.path(), "-g", "7.0", "--no-repair"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

/// The file's own version is written conformant, in another encoding on
/// request.
#[test]
fn write_in_another_encoding_and_line_ending() {
    let file = TempFile::new(
        b"0 HEAD\r\n1 CHAR ANSI\r\n1 GEDC\r\n2 VERS 5.5.1\r\n0 @I1@ INDI\r\n1 NAME Ren\xE9e /Example/\r\n\
          1 SEX female\r\n0 TRLR\r\n"
            .as_slice(),
    );
    let output = run(&["write", file.path()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("1 CHAR UTF-8\n") && out.contains("1 NAME Renée /Example/\n"),
        "{out}"
    );
    assert!(oxidged::spec::validate_text(&out).is_empty(), "{out}");

    let output = run(&[
        "write",
        file.path(),
        "--output-encoding",
        "ANSEL",
        "--line-ending",
        "crlf",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let bytes = output.stdout;
    assert!(bytes.windows(12).any(|w| w == b"CHAR ANSEL\r\n"));
    let back = GedcomBuilder::new().build_from_bytes(bytes).unwrap();
    assert_eq!(
        back.individuals().first().unwrap().full_name().as_deref(),
        Some("Renée Example")
    );
    let seven = TempFile::new("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n");
    let output = run(&["write", seven.path(), "--output-encoding", "ANSEL"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(stderr(&output).contains("always UTF-8"));
}

/// GEDCOM, GEDZIP and JSON, chosen by the extension or by option, read
/// back the same; a GEDZIP archive keeps its media files.
#[test]
fn gedzip_and_json_round_trips() {
    let file = TempFile::new(FAMILY.replace(
        "0 @N1@",
        "0 @M1@ OBJE\n1 FILE photo.jpg\n2 FORM jpg\n0 @N1@",
    ));
    let dir = std::env::temp_dir().join(format!("oxidged_cli_media_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("photo.jpg"), b"not really a JPEG").unwrap();
    let archive = temp_path("gdz");
    let output = run(&[
        "write",
        file.path(),
        "-o",
        archive.path(),
        "--media-dir",
        dir.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let info = json(&run(&["--json", "info", archive.path()]));
    assert_eq!(info["format"], "GEDZIP");
    assert_eq!(info["version"], "7.0");
    assert_eq!(info["media_files"], 1);
    // GEDZIP to JSON, JSON to GEDCOM: the GEDCOM written from the archive.
    let as_json = temp_path("json");
    let output = run(&["write", archive.path(), "-o", as_json.path()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let from_json = stdout(&run(&["write", as_json.path()]));
    let from_archive = stdout(&run(&["write", archive.path()]));
    assert_eq!(from_json, from_archive);
    // An archive rewritten keeps its media file.
    let again = temp_path("gdz");
    run(&["write", archive.path(), "--to", "gdz", "-o", again.path()]);
    assert_eq!(
        json(&run(&["--json", "info", again.path()]))["media_files"],
        1
    );
    // Standard input, its format found from its bytes.
    let bytes = std::fs::read(archive.path()).unwrap();
    let output = run_with_input(&["info", "-"], &bytes);
    assert!(
        stdout(&output).contains("format: GEDZIP"),
        "{}",
        stderr(&output)
    );
    let output = run(&["write", file.path(), "--to", "gdz", "-g", "5.5.1"]);
    assert_eq!(output.status.code(), Some(3));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Streaming writes the records in the file's order, the same records as
/// writing the whole dataset, which groups them by type.
#[test]
fn stream_writes_what_write_writes() {
    let file = TempFile::new(generations());
    for version in ["5.5.1", "7.0"] {
        let streamed = run(&["write", "--stream", file.path(), "-g", version]);
        assert!(streamed.status.success(), "{}", stderr(&streamed));
        let streamed = stdout(&streamed);
        assert!(
            oxidged::spec::validate_text(&streamed).is_empty(),
            "{streamed}"
        );
        let whole = stdout(&run(&["write", file.path(), "-g", version]));
        // Not knowing the records to come, a 7.x stream declares every
        // extension a conversion may write.
        let records = |text: &str| -> Vec<String> {
            text.lines()
                .filter(|l| !l.starts_with("1 SCHMA") && !l.starts_with("2 TAG "))
                .map(str::to_string)
                .collect()
        };
        let mut a = records(&streamed);
        let mut b = records(&whole);
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "{version}");
    }
    let output = run(&["write", "--stream", file.path(), "--to", "json"]);
    assert_eq!(output.status.code(), Some(3));
}

// Validation

#[test]
fn validate_lists_every_deviation() {
    let file = TempFile::new(VALID_551);
    let output = run(&["validate", file.path()]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "conformant\n");

    let file =
        TempFile::new(VALID_551.replace("0 TRLR", "0 @I1@ INDI\n1 SEX male\n1 FAMC @F9@\n0 TRLR"));
    let output = run(&["validate", file.path()]);
    assert_eq!(output.status.code(), Some(2));
    let text = stdout(&output);
    assert!(text.contains("line 11: SEX \"male\""), "{text}");
    assert!(text.contains("line 12: FAMC @F9@"), "{text}");
    assert!(text.ends_with("2 deviations\n"), "{text}");
    let report = json(&run(&["--json", "validate", file.path()]));
    assert_eq!(report["conformant"], false);
    assert_eq!(report["deviations"][1]["line"], 12);
}

#[test]
fn validate_reads_the_encoding_and_the_line_grammar() {
    // CR line endings and a Windows-1252 byte under CHAR UTF-8: the bytes
    // are read, and the label is reported.
    let file = TempFile::new(
        b"0 HEAD\r1 SOUR EXAMPLE\r1 SUBM @U1@\r1 GEDC\r2 VERS 5.5.1\r2 FORM LINEAGE-LINKED\r\
          1 CHAR UTF-8\r0 @U1@ SUBM\r1 NAME Caf\xe9 Example\r0 TRLR\r",
    );
    let output = run(&["validate", file.path()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("HEAD.CHAR says UTF-8"));
    let output = run(&["encoding", file.path()]);
    assert_eq!(
        stdout(&output),
        "encoding: WINDOWS-1252\ndeclared: UTF-8\nbyte order mark: no\n\
         the bytes are not in the declared encoding: evidence wins\n"
    );
    let output = run(&["decode", file.path()]);
    assert!(stdout(&output).contains("1 NAME Café Example\r"));
    assert_eq!(stderr(&output), "decoded as WINDOWS-1252\n");
}

#[test]
fn strict_reading_refuses_a_deviation() {
    let file = TempFile::new("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @F1@ FAM\n1 HUSB @I999@\n0 TRLR\n");
    let output = run(&["info", "--strict", file.path()]);
    assert_eq!(output.status.code(), Some(2));
    let error = stderr(&output);
    assert!(
        error.contains("line 5: HUSB @I999@: no record has this identifier"),
        "{error}"
    );
    assert!(run(&["info", file.path()]).status.success());
}

// Producers and importers

/// `detect` names the producer from the header and the tags.
#[test]
fn detect_prints_the_producer() {
    let file = TempFile::new(
        "0 HEAD\n1 SOUR FTM\n2 VERS Family Tree Maker (22.2.5)\n2 CORP Ancestry.com\n0 @I1@ INDI\n\
         1 NAME Ann /Example/\n0 TRLR\n",
    );
    let output = run(&["detect", file.path()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.starts_with("producer: ftm (Family Tree Maker)\nversion: 22.2.5\n"),
        "{out}"
    );
    assert!(out.contains("evidence: Sour"), "{out}");
    assert_eq!(json(&run(&["--json", "detect", file.path()]))["id"], "ftm");
}

/// `export` writes for an importer and lists what was lost.
#[test]
fn export_for_an_importer() {
    let file = TempFile::new(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 OCCU Weaver\n\
         1 OCCU Miller\n1 CHAN\n2 DATE 1 JAN 2020\n0 TRLR\n",
    );
    let output = run(&["export", file.path(), "--profile", "geneanet"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("1 OCCU Weaver, Miller\n"), "{out}");
    assert!(out.contains("2 VERS 5.5.1\n"), "{out}");
    assert!(stderr(&output).contains("lossy: @I1@: 2 occupations made one"));
    let out = stdout(&run(&[
        "export",
        file.path(),
        "-p",
        "geneanet",
        "--omit-change-dates",
    ]));
    assert!(!out.contains("CHAN"), "{out}");
    let output = run(&["export", file.path(), "-p", "nowhere"]);
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn profiles_list_and_describe() {
    let output = run(&["profiles"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(
        text.contains("geneanet") && text.contains("Importers"),
        "{text}"
    );
    let text = stdout(&run(&["profiles", "heredis"]));
    assert!(text.starts_with("producer heredis (Heredis)\n"), "{text}");
    assert!(text.contains("importer heredis"), "{text}");
    let described = json(&run(&["--json", "profiles", "heredis"]));
    assert_eq!(described["producer"]["parent"], "generic");
    assert!(described["importer"]["version"].is_string());
    assert_eq!(run(&["profiles", "nowhere"]).status.code(), Some(3));
    // A producer forced when reading.
    let file = TempFile::new(FAMILY);
    let info = json(&run(&[
        "--json",
        "info",
        "--producer",
        "heredis",
        file.path(),
    ]));
    assert_eq!(info["producer"]["id"], "heredis");
}

// Editing

#[test]
fn edits_keep_references_whole() {
    let file = TempFile::new(FAMILY);
    let out = stdout(&run(&["edit", "rename", file.path(), "@I1@", "@ANN@"]));
    assert!(
        out.contains("0 @ANN@ INDI\n") && out.contains("1 WIFE @ANN@\n"),
        "{out}"
    );
    // Refused while pointed to, unless detached.
    let output = run(&["edit", "remove", file.path(), "@I3@"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("edit refused"),
        "{}",
        stderr(&output)
    );
    let out = stdout(&run(&[
        "edit",
        "remove",
        file.path(),
        "@I3@",
        "--pointers",
        "detach",
    ]));
    assert!(!out.contains("@I3@"), "{out}");
    // Edits chain through standard input.
    let renamed = run(&["edit", "rename", file.path(), "@N1@", "@NOTE@"]);
    let output = run_with_input(
        &["edit", "link", "-", "@I3@", "note", "@NOTE@"],
        &renamed.stdout,
    );
    assert!(
        stdout(&output).contains("1 FAMC @F1@\n1 NOTE @NOTE@\n"),
        "{}",
        stderr(&output)
    );
    let out = stdout(&run(&[
        "edit",
        "unlink",
        file.path(),
        "@F1@",
        "child",
        "@I3@",
    ]));
    assert!(!out.contains("CHIL") && !out.contains("FAMC"), "{out}");
    // A 5.5.1 header holds a note as text: the pointer is a repair there,
    // a shared note in 7.0.
    let output = run(&["edit", "link", file.path(), "HEAD", "note", "@N1@"]);
    assert!(
        stderr(&output).contains("repair: HEAD.NOTE @N1@"),
        "{}",
        stderr(&output)
    );
    let out = stdout(&run(&[
        "edit",
        "link",
        file.path(),
        "HEAD",
        "note",
        "@N1@",
        "-g",
        "7.0",
    ]));
    assert!(out.contains("1 SUBM @U1@\n1 SNOTE @N1@\n"), "{out}");
    let out = stdout(&run(&["edit", "detach", file.path(), "@I2@"]));
    assert!(
        !out.contains("HUSB") && out.contains("0 @I2@ INDI"),
        "{out}"
    );
}

#[test]
fn redirect_merges_two_records() {
    let file = TempFile::new(
        FAMILY
            .replace(
                "0 @N1@ NOTE A shared note",
                "0 @N1@ NOTE A shared note\n0 @N2@ NOTE The same note",
            )
            .replace("1 SEX M\n", "1 SEX M\n1 NOTE @N2@\n"),
    );
    let output = run(&["edit", "redirect", file.path(), "@N2@", "@N1@", "--remove"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("1 NOTE @N1@\n") && !out.contains("@N2@"),
        "{out}"
    );
    assert_eq!(stderr(&output), "redirected: 1\n");
}

// The lossless tree and values

#[test]
fn tree_prints_the_structures_as_read() {
    let file =
        TempFile::new("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 _UNKNOWN x\n2 SEX Q\n0 TRLR\n");
    let output = run(&["tree", file.path()]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("1 _UNKNOWN x\n2 SEX Q\n"));
    let tree = json(&run(&["--json", "tree", file.path()]));
    assert_eq!(tree["version"], "7.0");
    assert_eq!(tree["records"][1]["tag"], "INDI");
}

#[test]
fn date_and_age_in_every_version() {
    let output = run(&["date", "ABT @#DJULIAN@ 1 MAR 1700"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("GEDCOM 5.5.1: valid; written ABT @#DJULIAN@ 1 MAR 1700\n"),
        "{text}"
    );
    assert!(text.contains("GEDCOM 7.0: not valid ("), "{text}");
    assert!(text.contains("written ABT JULIAN 1 MAR 1700\n"), "{text}");
    assert!(text.contains("GREGORIAN: 12 MAR 1700\n"), "{text}");
    let date = json(&run(&[
        "--json",
        "date",
        "1 JAN 2000",
        "--calendar",
        "julian",
    ]));
    assert_eq!(
        date["dates"][0]["calendars"]["JULIAN"],
        "JULIAN 19 DEC 1999"
    );
    assert_eq!(date["dates"][0]["weekday"], "Saturday");
    assert_eq!(date["versions"]["7.0"]["valid"], true);
    assert_eq!(
        run(&["date", "1900", "--calendar", "mayan"]).status.code(),
        Some(3)
    );
    let age = json(&run(&["--json", "age", "> 2y 3m"]));
    assert_eq!(age["versions"]["5.5.1"]["valid"], false);
    assert_eq!(age["versions"]["5.5.1"]["written"], ">2y 3m");
    assert_eq!(age["versions"]["7.0"]["valid"], true);
}

// The command line itself

#[test]
fn usage_errors_exit_with_code_3() {
    let file = TempFile::new(VALID_551);
    let path = file.path();
    for args in [
        vec!["write", path, "-g", "6.0"],
        vec!["write"],
        vec!["frobnicate", path],
        vec!["info", "--frobnicate", path],
        vec![],
        vec!["info", path, path],
        vec!["write", path, "--max-line-length", "300"],
        vec!["info", "--producer", "nowhere", path],
        vec!["info", "--encoding", "EBCDIC", path],
        vec!["write", path, "--media-dir", "."],
    ] {
        let output = run(&args);
        assert_eq!(
            output.status.code(),
            Some(3),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn help_version_completions_man_and_io_errors() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("subset"));
    let output = run(&["write", "--help"]);
    assert!(stdout(&output).contains("--gedcom-version <VERSION>"));
    let output = run(&["--version"]);
    assert!(stdout(&output).starts_with("oxidged "));
    let output = run(&["completions", "bash"]);
    assert!(stdout(&output).contains("_oxidged()"));
    let output = run(&["man"]);
    assert!(stdout(&output).contains(".TH oxidged 1"));
    let dir = std::env::temp_dir().join(format!("oxidged_cli_man_{}", std::process::id()));
    let output = run(&["man", "--out-dir", dir.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(dir.join("oxidged-subset.1").is_file());
    std::fs::remove_dir_all(&dir).unwrap();

    let missing = Path::new(&std::env::temp_dir()).join("oxidged_cli_test_no_such_file.ged");
    let output = run(&["info", missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("oxidged_cli_test_no_such_file.ged"));
}
