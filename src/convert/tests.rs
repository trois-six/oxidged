use super::*;
use crate::Bom;
use crate::tree::Tree;

const V551: GedcomVersion = GedcomVersion::V5_5_1;
const V70: GedcomVersion = GedcomVersion::V7_0;
const V71: GedcomVersion = GedcomVersion::V7_1;

fn head(vers: &str) -> String {
    format!("0 HEAD\n1 SOUR SAMPLER\n1 GEDC\n2 VERS {vers}\n")
}

/// `body` (records after a header of version `from`) written in `to`, as a
/// typed dataset, with `options`.
fn write_with(from: &str, body: &str, to: GedcomVersion, options: ConversionOptions) -> String {
    let data = Dataset::parse(format!("{}{body}0 TRLR\n", head(from)));
    let out = GedcomWriter::new()
        .gedcom_version(to)
        .conversion(options)
        .write_to_string(&data)
        .unwrap();
    let deviations = crate::spec::validate_text(&out);
    assert!(deviations.is_empty(), "{deviations:?}\n{out}");
    out
}

fn write(from: &str, body: &str, to: GedcomVersion) -> String {
    write_with(from, body, to, ConversionOptions::default())
}

/// `body` written in `to` as a lossless tree: it converts as the dataset
/// does, but for the order of the substructures.
fn write_tree(from: &str, body: &str, to: GedcomVersion) -> String {
    let tree = Tree::parse(format!("{}{body}0 TRLR\n", head(from)));
    let mut out = Vec::new();
    GedcomWriter::new()
        .gedcom_version(to)
        .bom(Bom::Never)
        .write_tree(&mut out, &tree)
        .unwrap();
    String::from_utf8(out).unwrap()
}

fn has(out: &str, lines: &str) {
    assert!(out.contains(lines), "missing\n{lines}\nin\n{out}");
}

/// The gaps the 7.x port found: a relation, an inline multimedia link, a
/// medium outside the set, a 5.5.1 file `TYPE`, a record named `@VOID@`.
#[test]
fn the_known_gaps_are_converted() {
    let body = "0 @I1@ INDI\n1 ASSO @I2@\n2 RELA neighbor\n1 ASSO @I2@\n2 RELA friend\n\
                1 ASSO @I2@\n2 RELA land-lord\n1 OBJE\n2 FILE photos\\ann.png\n3 FORM png\n4 MEDI photo\n\
                2 TITL Portrait\n0 @I2@ INDI\n0 @VOID@ INDI\n1 NAME Void /Example/\n1 FAMS @F1@\n\
                0 @F1@ FAM\n1 HUSB @VOID@\n0 @S1@ SOUR\n1 REPO @R1@\n2 CALN 1\n3 MEDI microfilm\n\
                0 @R1@ REPO\n1 NAME Archive\n0 @M1@ OBJE\n1 FILE a.jpg\n2 FORM jpg\n3 TYPE photo\n";
    for to in [V70, V71] {
        for out in [write("5.5.1", body, to), write_tree("5.5.1", body, to)] {
            has(&out, "1 ASSO @I2@\n2 ROLE NGHBR\n3 PHRASE neighbor\n");
            has(&out, "1 ASSO @I2@\n2 ROLE FRIEND\n");
            has(&out, "1 ASSO @I2@\n2 ROLE OTHER\n3 PHRASE land-lord\n");
            assert!(!out.contains("RELA") && !out.contains("_FILE"), "{out}");
            // The link points to a new record, which holds the file.
            has(&out, "1 OBJE @M2@\n2 TITL Portrait\n");
            has(
                &out,
                "0 @M2@ OBJE\n1 FILE photos/ann.png\n2 FORM image/png\n3 MEDI PHOTO\n",
            );
            // The file of a record: its medium is MEDI.
            has(
                &out,
                "0 @M1@ OBJE\n1 FILE a.jpg\n2 FORM image/jpeg\n3 MEDI PHOTO\n",
            );
            has(&out, "3 MEDI OTHER\n4 PHRASE microfilm\n");
            // The record named @VOID@ keeps its links.
            has(
                &out,
                "0 @VOID_1@ INDI\n1 NAME Void /Example/\n1 FAMS @F1@\n",
            );
            has(&out, "0 @F1@ FAM\n1 HUSB @VOID_1@\n");
        }
    }
}

#[test]
fn values_and_identifiers_convert_to_7() {
    let body = "0 @U1@ SUBM\n1 NAME Ann\n1 LANG English\n1 LANG Hmong\n1 RIN 7\n\
                0 @I1@ INDI\n1 NAME Ann /Example/\n2 FONE an\n3 TYPE kana\n2 ROMN Ann\n3 TYPE Some Method\n\
                1 AFN 1A2\n1 RFN Archive:42\n1 _UID 0123\n1 ALIA Nan /Example/\n\
                1 EVEN\n2 TYPE test\n2 AGE STILLBORN\n1 NOTE a @#DJULIAN@ date, @#XOLD@ gone\n";
    let out = write("5.5.1", body, V70);
    has(&out, "1 LANG en\n");
    // A language with a phrase, which 7.x has not: an extension that
    // `HEAD.SCHMA` declares as one.
    has(&out, "1 LANG und\n2 _PHRASE Hmong\n");
    has(&out, "2 TAG _PHRASE https://gedcom.io/terms/v7/PHRASE\n");
    has(
        &out,
        "1 EXID 7\n2 TYPE https://gedcom.io/terms/v7/RIN#SAMPLER\n",
    );
    has(&out, "2 TRAN an\n3 LANG ja-Hrkt\n");
    has(
        &out,
        "2 TRAN Ann\n3 LANG und-Latn-x-some-method\n3 _TYPE Some Method\n",
    );
    has(&out, "1 EXID 1A2\n2 TYPE https://gedcom.io/terms/v7/AFN\n");
    has(
        &out,
        "1 EXID 42\n2 TYPE https://gedcom.io/terms/v7/RFN#Archive\n",
    );
    has(&out, "1 UID 0123\n");
    has(&out, "1 NAME Nan /Example/\n2 TYPE AKA\n");
    has(&out, "2 AGE 0y\n3 PHRASE STILLBORN\n");
    has(&out, "1 DEAT\n2 AGE 0y\n3 PHRASE Stillborn\n");
    has(&out, "1 NOTE a JULIAN date, gone\n");
}

#[test]
fn obsolete_structures_go_or_are_kept() {
    let body = "0 @U1@ SUBM\n1 NAME Ann\n0 @B1@ SUBN\n1 FAMF Example\n";
    let header = "1 FILE family.ged\n1 CHAR ANSEL\n1 SUBN @B1@\n";
    let data = Dataset::parse(format!(
        "{}{header}{body}0 TRLR\n",
        head("5.5.1").replace("2 VERS 5.5.1\n", "2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n")
    ));
    let writer = GedcomWriter::new().gedcom_version(V70);
    let dropped = writer.write_to_string(&data).unwrap();
    for gone in ["SUBN", "FILE", "CHAR", "FORM", "FAMF"] {
        assert!(!dropped.contains(gone), "{gone}: {dropped}");
    }
    let keep = ConversionOptions::default().keep_obsolete(true);
    let kept = writer
        .clone()
        .conversion(keep)
        .write_to_string(&data)
        .unwrap();
    has(&kept, "1 _FILE family.ged\n");
    has(&kept, "1 _SUBN @B1@\n");
    has(&kept, "0 @B1@ _SUBMISSION\n1 FAMF Example\n");
    has(
        &kept,
        "2 TAG _SUBMISSION https://gedcom.io/terms/v5.5.1/record-SUBN\n",
    );
    has(&kept, "2 TAG _SUBN https://gedcom.io/terms/v5.5.1/SUBN\n");
    assert!(crate::spec::validate_text(&kept).is_empty(), "{kept}");
    // Back in 5.5.1, they are what they were.
    let back = GedcomWriter::new()
        .gedcom_version(V551)
        .write_to_string(&Dataset::parse(kept))
        .unwrap();
    has(&back, "1 FILE family.ged\n");
    has(&back, "1 SUBN @B1@\n");
    has(&back, "0 @B1@ SUBN\n1 FAMF Example\n");
    assert!(!back.contains("SCHMA") && !back.contains("_SUB"), "{back}");
}

#[test]
fn sources_in_words_become_records() {
    let body = "0 @I1@ INDI\n1 SOUR Parish register\n2 PAGE p. 4\n2 TEXT Born at home\n2 NOTE Seen\n\
                2 TEXT second text\n";
    let out = write("5.5.1", body, V70);
    has(
        &out,
        "1 SOUR @S1@\n2 PAGE p. 4\n2 DATA\n3 TEXT Born at home\n3 TEXT second text\n2 NOTE Seen\n",
    );
    has(&out, "0 @S1@ SOUR\n1 NOTE Parish register\n");
}

/// A 5.5 link holds its format beside its file.
#[test]
fn a_5_5_link_moves_its_format_into_its_file() {
    let body = "0 @I1@ INDI\n1 OBJE\n2 FORM bmp\n2 FILE photo.bmp\n2 TITL Photo\n";
    let out = write("5.5.1", body, V70);
    has(&out, "1 OBJE @M1@\n2 TITL Photo\n");
    has(&out, "0 @M1@ OBJE\n1 FILE photo.bmp\n2 FORM image/bmp\n");
}

#[test]
fn known_extensions_are_declared() {
    let body = "0 @L1@ _LOC\n1 NAME Sampleton\n0 @I1@ INDI\n1 BIRT\n2 PLAC Sampleton\n3 _GOV ABC\n";
    let out = write("5.5.1", body, V70);
    has(&out, "2 TAG _GOV http://genealogy.net/GEDCOM#_GOV\n");
    has(&out, "2 TAG _LOC http://genealogy.net/GEDCOM#_LOC\n");
}

#[test]
fn values_and_identifiers_convert_to_5_5_1() {
    let body = "0 @I1@ INDI\n1 NAME Ann /Example/\n2 TRAN Ann\n3 LANG und-Latn-x-some-method\n3 _TYPE Some Method\n\
                1 EXID 9\n2 TYPE https://gedcom.io/exid-type/FamilySearch-PersonId\n1 EXID 1\n2 TYPE https://example.com/\n\
                1 BAPM Y\n1 BIRT Y\n0 @U1@ SUBM\n1 NAME Ann\n1 LANG und\n2 _PHRASE Hmong\n";
    let schma = "1 SCHMA\n2 TAG _PHRASE https://gedcom.io/terms/v7/PHRASE\n";
    let data = Dataset::parse(format!("{}{schma}{body}0 TRLR\n", head("7.0")));
    let out = GedcomWriter::new()
        .gedcom_version(V551)
        .write_to_string(&data)
        .unwrap();
    has(&out, "2 ROMN Ann\n3 TYPE Some Method\n");
    has(&out, "1 _FSFTID 9\n");
    has(&out, "1 _EXID 1\n");
    has(&out, "1 BAPM\n");
    has(&out, "1 BIRT Y\n");
    // A language 5.5.1 does not list stays an extension.
    has(&out, "1 _LANG Hmong\n");
    assert!(!out.contains("SCHMA"), "{out}");
}

#[test]
fn within_a_family_nothing_converts() {
    let body = "0 @I1@ INDI\n1 ASSO @I2@\n2 RELA friend\n0 @I2@ INDI\n";
    assert!(Converter::new(V70, V71, ConversionOptions::default(), true).is_none());
    assert!(Converter::new(V551, V551, ConversionOptions::default(), true).is_none());
    let mut records = Tree::parse(format!("{}{body}0 TRLR\n", head("7.0"))).to_structures();
    let before = records.clone();
    convert_structures(&mut records, V70, V71, &ConversionOptions::default());
    assert_eq!(records, before);
}

#[test]
fn a_dataset_converts() {
    let data = Dataset::parse(format!(
        "{}0 @I1@ INDI\n1 OBJE\n2 FILE a.jpg\n3 FORM jpg\n0 TRLR\n",
        head("5.5.1")
    ));
    let v7 = convert_dataset(&data, V70, &ConversionOptions::default());
    assert_eq!(v7.version(), V70);
    assert_eq!(v7.multimedia().len(), 1);
    let back = convert_dataset(&v7, V551, &ConversionOptions::default());
    assert_eq!(back.version(), V551);
    assert_eq!(back.multimedia().len(), 1);
}

/// The records a conversion adds and those it makes extension records go
/// where the typed model writes them, so that writing what was written
/// changes nothing.
#[test]
fn conversion_output_is_written_again_as_it_is() {
    let body = "0 @I1@ INDI\n1 SOUR Register\n1 OBJE\n2 FILE a.jpg\n0 @S1@ SOUR\n1 TITL Book\n\
                0 @M1@ OBJE\n1 FILE b.jpg\n0 @B1@ SUBN\n0 @X1@ _LOC\n1 NAME Here\n";
    for options in [
        ConversionOptions::default(),
        ConversionOptions::default().keep_obsolete(true),
    ] {
        for to in [V70, V71] {
            let out = write_with("5.5.1", body, to, options);
            let again = GedcomWriter::new()
                .gedcom_version(to)
                .write_to_string(&Dataset::parse(out.as_str()))
                .unwrap();
            assert_eq!(again, out);
        }
    }
}

/// A date's phrase keeps the 5.5.1 wording the date was converted from,
/// escapes included: it is the date's words, not text to convert (fuzz
/// `convert`).
#[test]
fn a_date_phrase_keeps_its_escapes() {
    let body = "0 @I1@ INDI\n1 BIRT\n2 DATE @#DJULIAN@ 15 APR 1699/00\n1 NOTE seen @#DJULIAN@ x\n";
    for out in [write("5.5.1", body, V70), write_tree("5.5.1", body, V70)] {
        has(
            &out,
            "2 DATE JULIAN 15 APR 1699\n3 PHRASE @@#DJULIAN@ 15 APR 1699/00\n",
        );
        has(&out, "1 NOTE seen JULIAN x\n");
    }
}

/// The filter of [`Watch`] lets through every structure a rule applies to:
/// [`Converter::triggers_at`] is false where neither
/// [`Converter::may_apply`] (by tag and payload) nor
/// [`Converter::may_apply_to`] (by payload) say a rule may apply.
#[test]
fn the_watch_misses_no_structure_a_rule_applies_to() {
    use crate::tree::PayloadRef;
    let extensions = [
        "_UID",
        "_EMAIL",
        "_ASSO",
        "_FSFTID",
        "_FID",
        "_SDATE",
        "_LANG",
        "_LOC",
        "_GOV",
        "_X",
        "_SUBN",
        "_FILE",
        "_SUBMISSION",
        "_PHRASE",
    ];
    let tags: Vec<&str> = STANDARD_TAGS.iter().copied().chain(extensions).collect();
    let payloads = [
        PayloadRef::None,
        PayloadRef::Text("x"),
        PayloadRef::Text("OTHER"),
        PayloadRef::Text("Y"),
        PayloadRef::Text("a @#DJULIAN@ b"),
        PayloadRef::Text("STILLBORN"),
        PayloadRef::Text("0y"),
        PayloadRef::Text("English"),
        PayloadRef::Text("en"),
        PayloadRef::Text("(witness)"),
        PayloadRef::Text("C:\\photos\\a b.jpg"),
        PayloadRef::Text("jpg"),
        PayloadRef::Text("file:///a.jpg"),
        PayloadRef::Text("image/jpeg"),
        PayloadRef::Text("kana"),
        PayloadRef::Pointer("@VOID@"),
        PayloadRef::Pointer("@I1@"),
    ];
    let parents = [
        "", "INDI", "FAM", "NAME", "PLAC", "FORM", "FILE", "OBJE", "EVEN", "DATE",
    ];
    for (from, to) in [(V551, V70), (V70, V551)] {
        for keep in [false, true] {
            let options = ConversionOptions::default().keep_obsolete(keep);
            let mut conv = Converter::new(from, to, options, false).unwrap();
            for void in [false, true] {
                if void && from == V551 {
                    conv.rename_void("@VOID_1@".into());
                }
                for tag in &tags {
                    for payload in payloads {
                        let special = match payload {
                            PayloadRef::Text(t) => crate::writer::special_bytes(t),
                            _ => 0,
                        };
                        let may = conv.may_apply(tag) & payload_bit(payload) != 0
                            || conv.may_apply_to(payload, special);
                        for parent in parents {
                            for grand in parents {
                                for sub in [false, true] {
                                    assert!(
                                        may || !conv.triggers_at(tag, payload, parent, grand, sub),
                                        "{from}->{to} {tag} {payload:?} under {parent} {grand}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
