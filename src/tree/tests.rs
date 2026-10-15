use super::*;

/// The records as `(level, xref, tag, payload)` lines, depth first.
fn flat(tree: &Tree) -> Vec<String> {
    fn walk(s: StructureRef<'_>, out: &mut Vec<String>) {
        out.push(format!(
            "{} {} {} {:?}",
            s.level(),
            s.xref().unwrap_or("-"),
            s.tag(),
            s.payload()
        ));
        for c in s.substructures() {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    for r in tree.records() {
        walk(r, &mut out);
    }
    out
}

fn h7(body: &str) -> String {
    format!("0 HEAD\n1 GEDC\n2 VERS 7.0\n{body}0 TRLR\n")
}

fn h5(body: &str) -> String {
    format!("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n{body}0 TRLR\n")
}

/// The structure at a path of tags below the record with this xref.
fn find<'t>(tree: &'t Tree, xref: &str, path: &[&str]) -> StructureRef<'t> {
    let mut s = tree.records().find(|r| r.xref() == Some(xref)).unwrap();
    for tag in path {
        s = s
            .first(tag)
            .unwrap_or_else(|| panic!("no {tag} in {path:?}"));
    }
    s
}

#[test]
fn terminators_and_blank_lines() {
    for eol in ["\n", "\r\n", "\r", "\n\r"] {
        let text = h7("0 @I1@ INDI\n\n  \n1 SEX M\n").replace('\n', eol);
        let tree = parse_tree(&text);
        assert_eq!(find(&tree, "@I1@", &["SEX"]).text(), Some("M"), "{eol:?}");
        assert_eq!(tree.version(), GedcomVersion::V7_0);
    }
}

#[test]
fn empty_cont_with_trailing_delimiter_in_crlf() {
    // `2 CONT ` + CRLF must not swallow the next line.
    let text = h5("0 @N1@ NOTE first\n1 CONT \n1 CONT third\n").replace('\n', "\r\n");
    let tree = parse_tree(&text);
    assert_eq!(find(&tree, "@N1@", &[]).text(), Some("first\n\nthird"));
}

#[test]
fn delimiters_and_spaces() {
    let tree = parse_tree(&h7(
        "0 @I1@ INDI\n  1 NAME John /Doe/\n1  SEX M\n01 NOTE  two\n1 _X trailing \n",
    ));
    let i = find(&tree, "@I1@", &[]);
    assert_eq!(i.first("NAME").unwrap().text(), Some("John /Doe/"));
    assert_eq!(i.first("SEX").unwrap().text(), Some("M"));
    assert_eq!(i.first("NOTE").unwrap().text(), Some(" two"));
    assert_eq!(i.first("_X").unwrap().text(), Some("trailing "));
}

#[test]
fn continuations_join_under_any_tag() {
    let tree = parse_tree(&h5(concat!(
        "0 @I1@ INDI\n",
        "1 OCCU Long\n2 CONT second line\n2 CONC  joined\n2 DATE 1900\n",
        "1 DSCR Hair brown, eyes\n2 CONC  green\n",
        "1 NOTE a\n2 LANG en\n2 CONT b\n",
        "1 NOTE c\n1 CONT d\n",
        "1 _EXT value\n2 CONT more\n",
    )));
    let i = find(&tree, "@I1@", &[]);
    let occu = i.first("OCCU").unwrap();
    assert_eq!(occu.text(), Some("Long\nsecond line joined"));
    assert_eq!(
        occu.substructures().map(|s| s.tag()).collect::<Vec<_>>(),
        ["DATE"]
    );
    assert_eq!(
        i.first("DSCR").unwrap().text(),
        Some("Hair brown, eyes green")
    );
    let notes: Vec<_> = i.substructures().filter(|s| s.tag() == "NOTE").collect();
    // After a substructure: still the parent.
    assert_eq!(notes[0].text(), Some("a\nb"));
    // One level too high: the previous sibling.
    assert_eq!(notes[1].text(), Some("c\nd"));
    assert_eq!(i.first("_EXT").unwrap().text(), Some("value\nmore"));
    assert!(
        flat(&tree)
            .iter()
            .all(|l| !l.contains("CONT") && !l.contains("CONC"))
    );
}

#[test]
fn continuation_with_an_identifier_joins() {
    let tree = parse_tree(&h5("0 @N1@ NOTE a\n1 @N2@ CONT b\n1 @C@ CONC c\n"));
    let note = find(&tree, "@N1@", &[]);
    assert_eq!(note.text(), Some("a\nbc"));
    assert_eq!(note.substructures().count(), 0);
}

#[test]
fn continuation_of_an_empty_payload() {
    let tree = parse_tree(&h5(
        "0 @N1@ NOTE\n1 CONC abc\n1 CONT def\n0 @N2@ NOTE\n1 CONC\n",
    ));
    assert_eq!(find(&tree, "@N1@", &[]).text(), Some("abc\ndef"));
    assert_eq!(find(&tree, "@N2@", &[]).payload(), PayloadRef::None);
}

#[test]
fn continuation_under_a_pointer_is_kept() {
    let tree = parse_tree(&h7("0 @I1@ INDI\n1 FAMC @F1@\n2 CONT odd\n"));
    let famc = find(&tree, "@I1@", &["FAMC"]);
    assert_eq!(famc.pointer(), Some("@F1@"));
    assert_eq!(famc.first("CONT").unwrap().text(), Some("odd"));
}

#[test]
fn extensions_stay_under_their_real_parent() {
    let tree = parse_tree(&h5(concat!(
        "0 @F1@ FAM\n1 CHIL @I2@\n2 _FREL Natural\n2 _MREL Adopted\n",
        "0 @N1@ NOTE text\n1 _X ext\n",
        "0 @I1@ INDI\n1 EXID 123\n2 TYPE https://example.org/ids\n3 _SOURCE x\n",
    )));
    let chil = find(&tree, "@F1@", &["CHIL"]);
    assert_eq!(chil.pointer(), Some("@I2@"));
    assert_eq!(chil.substructures().count(), 2);
    assert_eq!(find(&tree, "@N1@", &["_X"]).text(), Some("ext"));
    let ty = find(&tree, "@I1@", &["EXID", "TYPE"]);
    assert_eq!(ty.first("_SOURCE").unwrap().level(), 3);
}

#[test]
fn level_jumps_stay_in_their_record() {
    let tree = parse_tree(&h7(
        "0 @I1@ INDI\n1 BIRT\n0 @I2@ INDI\n2 _FOO bar\n1 SEX F\n",
    ));
    let i2 = find(&tree, "@I2@", &[]);
    assert_eq!(i2.first("_FOO").unwrap().level(), 1);
    assert_eq!(i2.first("SEX").unwrap().text(), Some("F"));
    assert_eq!(find(&tree, "@I1@", &["BIRT"]).substructures().count(), 0);
}

#[test]
fn deep_nesting_is_capped() {
    let mut body = String::from("0 @I1@ INDI\n");
    for level in 1..=300 {
        body.push_str(&format!("{level} _L{level} x\n"));
    }
    let tree = parse_tree(&h7(&body));
    let max = tree.records().nth(1).unwrap();
    let mut depth = 0;
    let mut s = max;
    while let Some(c) = s.substructures().last() {
        s = c;
        depth = depth.max(s.level());
    }
    assert_eq!(depth, 255);
    assert_eq!(tree.len(), 3 + 300 + 2);
    let reparsed = parse_tree(&tree.to_gedcom());
    assert_eq!(reparsed.to_structures(), tree.to_structures());
}

#[test]
fn lines_without_a_level() {
    let tree = parse_tree(&h5(
        "0 @I1@ INDI\n1 NOTE First line.\nThis line lacks a CONT tag.\n1 FAMC @F1@\nstray text\n1 SEX M\n",
    ));
    let i = find(&tree, "@I1@", &[]);
    assert_eq!(
        i.first("NOTE").unwrap().text(),
        Some("First line.\nThis line lacks a CONT tag.")
    );
    let stray = i.first("FAMC").unwrap().first("").unwrap();
    assert_eq!(stray.text(), Some("stray text"));
    assert_eq!(i.first("SEX").unwrap().text(), Some("M"));
    let tree = parse_tree("junk before\n0 HEAD\n0 TRLR\n");
    assert_eq!(tree.records().next().unwrap().text(), Some("junk before"));
}

#[test]
fn pointers_and_text_are_typed_by_shape() {
    let tree = parse_tree(&h7(concat!(
        "0 @I1@ INDI\n1 NOTE @@N1@\n1 SNOTE @N1@\n1 FAMC @VOID@\n1 NOTE @not pointer\n",
        "1 @X1@ NOTE xref kept\n",
        "0 @N1@ SNOTE @@@@@ has four\n",
    )));
    let i = find(&tree, "@I1@", &[]);
    let subs: Vec<_> = i.substructures().map(StructureRef::payload).collect();
    assert_eq!(
        subs,
        [
            PayloadRef::Text("@N1@"),
            PayloadRef::Pointer("@N1@"),
            PayloadRef::Pointer("@VOID@"),
            PayloadRef::Text("@not pointer"),
            PayloadRef::Text("xref kept"),
        ]
    );
    assert_eq!(i.substructures().nth(4).unwrap().xref(), Some("@X1@"));
    assert_eq!(find(&tree, "@N1@", &[]).text(), Some("@@@@ has four"));
}

#[test]
fn at_signs_in_551() {
    let tree = parse_tree(&h5(concat!(
        "0 @I1@ INDI\n1 NOTE a@@b.example and @@c\n2 CONT @@d\n",
        "1 BIRT\n2 DATE @#DJULIAN@ 1 JAN 1700\n",
        "1 NOTE @NoTe ref@\n",
        "1 ASSO @I132!1@\n",
        "0 @NoTe ref@ NOTE mixed case and space\n",
    )));
    let i = find(&tree, "@I1@", &[]);
    assert_eq!(
        i.first("NOTE").unwrap().text(),
        Some("a@b.example and @c\n@d")
    );
    assert_eq!(
        find(&tree, "@I1@", &["BIRT", "DATE"]).text(),
        Some("@#DJULIAN@ 1 JAN 1700")
    );
    assert_eq!(
        i.substructures().nth(2).unwrap().pointer(),
        Some("@NoTe ref@")
    );
    assert_eq!(i.first("ASSO").unwrap().pointer(), Some("@I132!1@"));
    assert_eq!(find(&tree, "@NoTe ref@", &[]).tag(), "NOTE");
}

#[test]
fn records_after_trlr_and_without_head() {
    let tree = parse_tree("0 @I1@ INDI\n1 SEX M\n0 TRLR\n1 NOTE nope\n0 @I2@ INDI\n");
    let tags: Vec<_> = tree.records().map(StructureRef::tag).collect();
    assert_eq!(tags, ["INDI", "TRLR", "INDI"]);
    assert_eq!(
        tree.records().nth(1).unwrap().first("NOTE").unwrap().text(),
        Some("nope")
    );
    assert_eq!(tree.version(), GedcomVersion::V5_5_1);
    assert!(parse_tree("").is_empty());
    assert!(parse_tree("\n \r\n\t").is_empty());
}

#[test]
fn line_numbers_follow_the_source() {
    let tree = parse_tree("0 HEAD\r\n\r\n0 @I1@ INDI\r1 NAME x\n2 CONT y\n1 SEX M\n");
    let i = find(&tree, "@I1@", &[]);
    assert_eq!(i.line(), 3);
    assert_eq!(i.first("SEX").unwrap().line(), 6);
}

#[test]
fn owned_structures_and_dump() {
    let text = h5("0 @I1@ INDI\n1 NAME A@@B /C/\n2 _X\n1 NOTE l1\n2 CONT  l2\n2 CONT\n");
    let tree = parse_tree(&text);
    let owned = tree.to_structures();
    assert_eq!(owned.len(), 3);
    let indi = &owned[1];
    assert_eq!(indi.xref.as_deref(), Some("@I1@"));
    assert_eq!(indi.first("NAME").unwrap().text(), Some("A@B /C/"));
    assert_eq!(indi.first("NOTE").unwrap().text(), Some("l1\n l2\n"));
    assert_eq!(
        indi.to_gedcom(0, GedcomVersion::V5_5_1),
        "0 @I1@ INDI\n1 NAME A@@B /C/\n2 _X\n1 NOTE l1\n2 CONT  l2\n2 CONT\n"
    );
    assert_eq!(parse_tree(&tree.to_gedcom()).to_structures(), owned);
}

#[test]
fn segments_split_large_inputs_at_records() {
    let mut body = String::new();
    for i in 0..50 {
        body.push_str(&format!(
            "0 @I{i}@ INDI\n1 NAME N{i} /S/\n2 CONT x@@y\n1 _E{} z\n",
            i % 3
        ));
    }
    let text = h5(&body);
    let whole = Tree::parse(text.clone());
    for limit in [64, 100, 200, 1000] {
        let split = Tree::parse_segmented(text.clone(), limit);
        assert!(split.segments.len() > 1, "limit {limit}");
        let lines = |t: &Tree| t.records().map(|r| r.line()).collect::<Vec<_>>();
        assert_eq!(
            split.to_structures(),
            whole.to_structures(),
            "limit {limit}"
        );
        assert_eq!(lines(&split), lines(&whole));
        assert_eq!(split.to_gedcom(), whole.to_gedcom());
    }
    // A record larger than a segment is cut at a line: its data stays.
    let split = Tree::parse_segmented(text.clone(), 20);
    let texts = |t: &Tree| {
        let mut v: Vec<String> = t
            .to_gedcom()
            .lines()
            .map(|l| l.split_once(' ').unwrap().1.to_string())
            .collect();
        v.sort();
        v
    };
    assert_eq!(texts(&split), texts(&whole));
}

#[test]
fn structure_size_budget() {
    assert_eq!(std::mem::size_of::<RawNode>(), 24);
    assert!(std::mem::size_of::<Structure>() <= 96);
    assert_eq!(std::mem::size_of::<Tag>(), 16);
    assert_eq!(std::mem::size_of::<Payload>(), 24);
}
