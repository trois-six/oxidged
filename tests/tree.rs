//! The lossless tree: line grammar, continuations, extensions and payload
//! typing, case by case (ids from the conformance suite and the reviews).
//! All data is fictitious.

use oxidged::GedcomVersion;
use oxidged::tree::{PayloadRef, Structure, StructureRef, Tree, TreeReader, parse_tree};

fn h7(body: &str) -> String {
    format!("0 HEAD\n1 GEDC\n2 VERS 7.0\n{body}0 TRLR\n")
}

fn h5(body: &str) -> String {
    format!("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n{body}0 TRLR\n")
}

/// The record with this xref.
fn record<'t>(tree: &'t Tree, xref: &str) -> StructureRef<'t> {
    tree.records()
        .find(|r| r.xref() == Some(xref))
        .unwrap_or_else(|| panic!("no record {xref}"))
}

/// The structure at a path of tags.
fn at<'t>(s: StructureRef<'t>, path: &[&str]) -> StructureRef<'t> {
    path.iter().fold(s, |s, tag| {
        s.first(tag)
            .unwrap_or_else(|| panic!("no {tag} in {path:?}"))
    })
}

fn tags(s: StructureRef<'_>) -> Vec<&str> {
    s.substructures().map(StructureRef::tag).collect()
}

/// Reading the dump gives the same tree, and streaming gives the same records.
fn assert_stable(tree: &Tree, source: &[u8]) {
    let owned = tree.to_structures();
    assert_eq!(parse_tree(&tree.to_gedcom()).to_structures(), owned);
    let streamed: Vec<Structure> = TreeReader::new(source)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(streamed, owned);
}

fn read(text: &str) -> Tree {
    let tree = parse_tree(text);
    assert_stable(&tree, text.as_bytes());
    tree
}

// b1: lines and terminators.

#[test]
fn g7_noeol_bom_tab_astral() {
    let tree = read(
        "\u{feff}0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NOTE ok\there 😀\n1 SEX F\n0 TRLR",
    );
    let i = record(&tree, "@I1@");
    assert_eq!(at(i, &["NOTE"]).text(), Some("ok\there 😀"));
    assert_eq!(at(i, &["SEX"]).text(), Some("F"));
    assert_eq!(tree.records().last().unwrap().tag(), "TRLR");
}

#[test]
fn g7_banned_c0_is_kept() {
    let tree = read(&h7("0 @I1@ INDI\n1 NOTE bad\u{7}char\n"));
    assert_eq!(
        at(record(&tree, "@I1@"), &["NOTE"]).text(),
        Some("bad\u{7}char")
    );
}

#[test]
fn g7_level_leading_zero_leading_space_double_space() {
    let tree = read(&h7("0 @I1@ INDI\n01 NAME A /B/\n  1 SEX M\n1  NOTE x\n"));
    assert_eq!(tags(record(&tree, "@I1@")), ["NAME", "SEX", "NOTE"]);
}

#[test]
fn g7_lowercase_tag_and_two_spaces_and_trailing_space() {
    let tree = read(&h7(
        "0 @I1@ INDI\n1 _x lower\n1 NOTE  leading space kept\n1 NOTE trailing \n",
    ));
    let i = record(&tree, "@I1@");
    assert_eq!(at(i, &["_x"]).text(), Some("lower"));
    let notes: Vec<_> = i.substructures().filter_map(StructureRef::text).collect();
    assert_eq!(notes, ["lower", " leading space kept", "trailing "]);
}

#[test]
fn g7_at_leading_only_first_and_interior() {
    let tree = read(&h7(concat!(
        "0 @I1@ INDI\n1 NOTE me@example.com is my email\n2 CONT @@me and @I are handles\n",
        "1 NOTE a@@b\n0 @N1@ SNOTE @@@@@ has four\n"
    )));
    let i = record(&tree, "@I1@");
    let notes: Vec<_> = i.substructures().filter_map(StructureRef::text).collect();
    assert_eq!(
        notes,
        ["me@example.com is my email\n@me and @I are handles", "a@@b"]
    );
    assert_eq!(record(&tree, "@N1@").text(), Some("@@@@ has four"));
}

#[test]
fn g7_level_skip_and_x_level_jump_no_reparent() {
    let tree = read(&h7(
        "0 @I1@ INDI\n1 BIRT\n0 @I2@ INDI\n2 _FOO bar\n1 SEX M\n",
    ));
    assert_eq!(tags(record(&tree, "@I2@")), ["_FOO", "SEX"]);
    assert!(tags(at(record(&tree, "@I1@"), &["BIRT"])).is_empty());
}

#[test]
fn g7_deep_nesting() {
    let mut body = String::from("0 @I1@ INDI\n");
    for level in 1..=120 {
        body.push_str(&format!("{level} _L{level}\n"));
    }
    body.push_str("121 _L121 x\n");
    let tree = read(&h7(&body));
    let mut s = record(&tree, "@I1@");
    while let Some(c) = s.substructures().next() {
        s = c;
    }
    assert_eq!((s.level(), s.tag(), s.text()), (121, "_L121", Some("x")));
}

#[test]
fn g7_pointers_void_dangling_forward_and_dup_xref() {
    let tree = read(&h7(concat!(
        "0 @I1@ INDI\n1 ALIA @I2@\n1 FAMC @VOID@\n1 FAMS @F9@\n0 @I2@ INDI\n",
        "0 @I1@ INDI\n1 SEX F\n0 @VOID@ INDI\n0 @i1@ INDI\n0 INDI\n1 NOTE no xref\n",
    )));
    let i = record(&tree, "@I1@");
    let ptrs: Vec<_> = i
        .substructures()
        .filter_map(StructureRef::pointer)
        .collect();
    assert_eq!(ptrs, ["@I2@", "@VOID@", "@F9@"]);
    let xrefs: Vec<_> = tree.records().filter_map(StructureRef::xref).collect();
    assert_eq!(xrefs, ["@I1@", "@I2@", "@I1@", "@VOID@", "@i1@"]);
}

#[test]
fn g7_substruct_xref_is_kept() {
    let tree = read(&h7("0 @I1@ INDI\n1 @X1@ NOTE hi\n"));
    let note = at(record(&tree, "@I1@"), &["NOTE"]);
    assert_eq!((note.xref(), note.text()), (Some("@X1@"), Some("hi")));
}

#[test]
fn g7_cont_blank_and_conc_in_7() {
    let tree = read(&h7(concat!(
        "0 @I1@ INDI\n1 NOTE This is a note field that\n2 CONT   spans four lines.\n2 CONT\n",
        "2 CONT (the third line was blank)\n1 NOTE abc\n2 CONC def\n"
    )));
    let notes: Vec<_> = record(&tree, "@I1@")
        .substructures()
        .filter_map(StructureRef::text)
        .collect();
    assert_eq!(
        notes,
        [
            "This is a note field that\n  spans four lines.\n\n(the third line was blank)",
            "abcdef"
        ]
    );
}

#[test]
fn g7_cont_after_sub_and_wrong_level() {
    let tree = read(&h7(
        "0 @I1@ INDI\n1 NOTE a\n2 LANG en\n2 CONT b\n1 NOTE c\n1 CONT d\n",
    ));
    let i = record(&tree, "@I1@");
    let notes: Vec<_> = i.substructures().filter_map(StructureRef::text).collect();
    assert_eq!(notes, ["a\nb", "c\nd"]);
    assert_eq!(at(i, &["NOTE", "LANG"]).text(), Some("en"));
}

#[test]
fn g7_no_head_no_trlr_trlr_sub_and_empty() {
    let tree = read("0 @I1@ INDI\n1 SEX M\n");
    assert_eq!(tree.version(), GedcomVersion::V5_5_1);
    assert_eq!(tree.records().count(), 1);
    let tree = read("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n1 NOTE nope\n");
    assert_eq!(
        at(tree.records().last().unwrap(), &["NOTE"]).text(),
        Some("nope")
    );
    assert!(read("").is_empty());
}

#[test]
fn g5_line_break_no_cont() {
    let tree = read(&h5(
        "0 @I1@ INDI\n1 NOTE First line.\nThis line lacks a CONT tag.\n1 SEX M\n",
    ));
    let i = record(&tree, "@I1@");
    assert_eq!(
        at(i, &["NOTE"]).text(),
        Some("First line.\nThis line lacks a CONT tag.")
    );
    assert_eq!(at(i, &["SEX"]).text(), Some("M"));
}

#[test]
fn eol_mixtures_number_lines_like_the_source() {
    let text = "0 HEAD\r\n1 GEDC\r2 VERS 5.5.1\n\r0 @I1@ INDI\n\n1 SEX M\r\n0 TRLR";
    let tree = read(text);
    let i = record(&tree, "@I1@");
    assert_eq!((i.line(), at(i, &["SEX"]).line()), (4, 6));
}

// b3: continuations, escapes and extensions (D2, D3, D11, F3, F4, H8–H11, C1).

#[test]
fn g5_conc_trailing_space_and_custom_after_cont() {
    let tree = read(&h5(concat!(
        "0 @I1@ INDI\n1 NOTE The FTM way --> \n2 CONC <--\n",
        "1 NOTE First line\n2 CONT second line\n2 _H custom after cont\n"
    )));
    let i = record(&tree, "@I1@");
    let notes: Vec<_> = i.substructures().collect();
    assert_eq!(notes[0].text(), Some("The FTM way --> <--"));
    assert_eq!(notes[1].text(), Some("First line\nsecond line"));
    assert_eq!(at(notes[1], &["_H"]).text(), Some("custom after cont"));
}

#[test]
fn attribute_and_event_values_are_continued() {
    // D11, H10, attribute-CONC repro: CONC/CONT under OCCU, DSCR, EVEN, TITL.
    let tree = read(&h5(concat!(
        "0 @I1@ INDI\n1 DSCR Hair brown, eyes\n2 CONC  green\n2 DATE 1900\n",
        "1 EVEN First line\n2 CONT Second line\n2 TYPE Custom\n",
        "1 TITL Sir\n2 CONC  Example\n"
    )));
    let i = record(&tree, "@I1@");
    assert_eq!(at(i, &["DSCR"]).text(), Some("Hair brown, eyes green"));
    assert_eq!(tags(at(i, &["DSCR"])), ["DATE"]);
    assert_eq!(at(i, &["EVEN"]).text(), Some("First line\nSecond line"));
    assert_eq!(at(i, &["TITL"]).text(), Some("Sir Example"));
}

#[test]
fn extensions_keep_their_place() {
    // D2, D3, D21, H8, F3, C1, G5-CUSTOM-*.
    let tree = read(&h5(concat!(
        "0 @F1@ FAM\n1 CHIL @I2@\n2 _FREL Natural\n2 _MREL Adopted\n1 _SEPR\n2 DATE 1901\n",
        "0 @I1@ INDI\n1 NAME A /B/\n2 SOUR @S1@\n3 _LINK https://example.org/record/1\n3 _JUST why\n",
        "1 EXID 123\n2 TYPE https://example.org/ids\n2 _SOURCE x\n",
        "1 BIRT\n2 DATE 1900\n3 _DX extra\n1 NOTE text\n2 _NX ext\n",
        "0 @O1@ OBJE\n1 NOTE This is a note\n2 _AREA {1,2,3,4}\n",
        "0 @X1@ _CUSTOM rec\n1 NAME standard under extension\n",
    )));
    assert_eq!(
        tags(at(record(&tree, "@F1@"), &["CHIL"])),
        ["_FREL", "_MREL"]
    );
    assert_eq!(tags(at(record(&tree, "@F1@"), &["_SEPR"])), ["DATE"]);
    let i = record(&tree, "@I1@");
    assert_eq!(tags(at(i, &["NAME", "SOUR"])), ["_LINK", "_JUST"]);
    assert_eq!(tags(at(i, &["EXID"])), ["TYPE", "_SOURCE"]);
    assert_eq!(at(i, &["BIRT", "DATE", "_DX"]).text(), Some("extra"));
    assert_eq!(at(i, &["NOTE", "_NX"]).text(), Some("ext"));
    assert_eq!(
        at(record(&tree, "@O1@"), &["NOTE", "_AREA"]).text(),
        Some("{1,2,3,4}")
    );
    assert_eq!(
        at(record(&tree, "@X1@"), &["NAME"]).text(),
        Some("standard under extension")
    );
}

#[test]
fn text_substructures_stay_with_their_text() {
    // H9: SOUR.TEXT.MIME/LANG and SNOTE.TRAN.LANG stay where they are.
    let tree = read(&h7(concat!(
        "0 @S1@ SOUR\n1 TEXT line one\n2 CONT line two\n2 MIME text/plain\n2 LANG en\n",
        "0 @N1@ SNOTE note\n1 TRAN traduction\n2 CONT suite\n2 LANG fr\n1 LANG en\n",
    )));
    let text = at(record(&tree, "@S1@"), &["TEXT"]);
    assert_eq!(text.text(), Some("line one\nline two"));
    assert_eq!(tags(text), ["MIME", "LANG"]);
    let n = record(&tree, "@N1@");
    assert_eq!(tags(n), ["TRAN", "LANG"]);
    assert_eq!(at(n, &["TRAN"]).text(), Some("traduction\nsuite"));
    assert_eq!(at(n, &["TRAN", "LANG"]).text(), Some("fr"));
}

#[test]
fn leading_spaces_of_payloads_are_kept() {
    // D16, H11.
    let tree = read(&h5(
        "0 @I1@ INDI\n1 BIRT\n2 PLAC  Example Hospital\n1 NOTE \tindented\n",
    ));
    let i = record(&tree, "@I1@");
    assert_eq!(at(i, &["BIRT", "PLAC"]).text(), Some(" Example Hospital"));
    assert_eq!(at(i, &["NOTE"]).text(), Some("\tindented"));
}

// Payload typing (F6, R3, H12 read side, X11, D13 read side).

#[test]
fn pointer_and_text_are_typed_on_read() {
    let tree = read(&h7(
        "0 @I1@ INDI\n1 NOTE @@home@\n1 NOTE @@N1@\n1 SNOTE @N1@\n0 @N1@ SNOTE x\n",
    ));
    let payloads: Vec<_> = record(&tree, "@I1@")
        .substructures()
        .map(StructureRef::payload)
        .collect();
    assert_eq!(
        payloads,
        [
            PayloadRef::Text("@home@"),
            PayloadRef::Text("@N1@"),
            PayloadRef::Pointer("@N1@")
        ]
    );
}

#[test]
fn g5_at_escape_and_escapes_kept() {
    let tree = read(&h5(concat!(
        "0 @I1@ INDI\n1 NOTE a@@b.example and @@c\n1 BIRT\n2 DATE @#DJULIAN@ 1 JAN 1700\n",
        "1 DEAT\n2 DATE FROM @#DJULIAN@ 1700 TO @#DGREGORIAN@ 1710\n",
        "0 @N0@ NOTE @@N0@\n"
    )));
    let i = record(&tree, "@I1@");
    assert_eq!(at(i, &["NOTE"]).text(), Some("a@b.example and @c"));
    assert_eq!(
        at(i, &["BIRT", "DATE"]).text(),
        Some("@#DJULIAN@ 1 JAN 1700")
    );
    assert_eq!(
        at(i, &["DEAT", "DATE"]).text(),
        Some("FROM @#DJULIAN@ 1700 TO @#DGREGORIAN@ 1710")
    );
    // G5-NOTE-LOOKS-LIKE-PTR: the text of a NOTE record that reads `@N0@`.
    assert_eq!(record(&tree, "@N0@").payload(), PayloadRef::Text("@N0@"));
}

#[test]
fn g5_xref_with_space_and_network_forms() {
    let tree = read(&h5(concat!(
        "0 @TEST@ SUBM\n1 NOTE @NoTe@\n1 NOTE @NoTe ref@\n",
        "0 @NoTe@ NOTE mixed case\n0 @NoTe ref@ NOTE mixed case and space\n",
        "0 @I1@ INDI\n1 ASSO @I2!1@\n1 ASSO @!3@\n1 ALIA @NET:I9@\n"
    )));
    let ptrs: Vec<_> = record(&tree, "@TEST@")
        .substructures()
        .filter_map(StructureRef::pointer)
        .collect();
    assert_eq!(ptrs, ["@NoTe@", "@NoTe ref@"]);
    assert_eq!(
        record(&tree, "@NoTe ref@").text(),
        Some("mixed case and space")
    );
    let ptrs: Vec<_> = record(&tree, "@I1@")
        .substructures()
        .filter_map(StructureRef::pointer)
        .collect();
    assert_eq!(ptrs, ["@I2!1@", "@!3@", "@NET:I9@"]);
}

// Version detection on the lexed header (M4, L9).

#[test]
fn version_detection_reads_the_header_only() {
    let tree = read("0 HEAD\r1 SOUR app\r2 VERS 9.9\r1 GEDC\r2 VERS 7.0.14\r0 TRLR\r");
    assert_eq!(tree.version(), GedcomVersion::V7_0);
    assert_eq!(tree.declared_version(), Some("7.0.14"));
    let late = format!(
        "0 HEAD\n1 NOTE {}\n1 GEDC\n2 VERS 7.0\n0 TRLR\n",
        "é".repeat(600)
    );
    assert_eq!(oxidged::version::detect_version(&late), GedcomVersion::V7_0);
    // An SNOTE record or a SCHMA elsewhere does not make a 5.5.1 file 7.0.
    let tree = read(&h5("0 @N1@ SNOTE x\n"));
    assert_eq!(tree.version(), GedcomVersion::V5_5_1);
}

/// Decoded bytes give the same tree as the text (ANSEL across CONC, TS10).
#[test]
fn bytes_of_any_encoding() {
    let bytes = b"0 HEAD\r\n1 GEDC\r\n2 VERS 5.5.1\r\n1 CHAR ANSEL\r\n0 @I1@ INDI\r\n1 NOTE Jos\xE2\r\n2 CONC e Mar\xE2ia\r\n0 TRLR\r\n";
    let tree = Tree::from_bytes(bytes);
    assert_eq!(
        at(record(&tree, "@I1@"), &["NOTE"]).text(),
        Some("José María")
    );
    assert_stable(&tree, bytes);
}
