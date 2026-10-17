//! Producer profiles: coverage of the compatibility research, the reader's
//! own handling of what profiles need not do, one fictitious fixture per
//! import profile and the export profiles on one fictitious dataset.
//!
//! `tests/profiles/catalogue.tsv` and `tests/profiles/export.tsv` are the
//! research's tables (import quirks and importer limitations), trimmed to
//! their facts. Every name and place in the fixtures is fictitious. Set
//! `BLESS=1` to rewrite the expected outputs after a deliberate change.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use oxidged::model::HeaderSource;
use oxidged::profile::{
    self, Action, Detection, ExportOptions, Exporter, HeadSource, Interpretation, export_profiles,
    import_profiles,
};
use oxidged::{Dataset, GedcomBuilder, GedcomEncoding, GedcomVersion, GedcomWriter};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/profiles")
}

/// The first line where two texts differ, with the two lines.
fn first_difference(a: &str, b: &str) -> String {
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let at = la
        .iter()
        .zip(&lb)
        .position(|(x, y)| x != y)
        .unwrap_or(la.len().min(lb.len()));
    format!(
        "line {}: {:?} != {:?}",
        at + 1,
        la.get(at.saturating_sub(1)..(at + 2).min(la.len())),
        lb.get(at.saturating_sub(1)..(at + 2).min(lb.len()))
    )
}

/// A research table: rows by id, columns by name.
fn table(name: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let text = std::fs::read_to_string(dir().join(name)).unwrap();
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().unwrap().split('\t').collect();
    lines
        .map(|l| {
            let cells: BTreeMap<String, String> = header
                .iter()
                .zip(l.split('\t'))
                .map(|(h, c)| ((*h).to_string(), c.to_string()))
                .collect();
            (cells["id"].clone(), cells)
        })
        .collect()
}

/// Whether a catalogue row is lossless and has a 7.0 target: what a rule,
/// the reader or the `NOT_YET` list must account for.
fn wants_rule(row: &BTreeMap<String, String>) -> bool {
    let target = row["std70"].trim();
    row["lossless"].starts_with("yes")
        && ![
            "none",
            "same",
            "n/a",
            "—",
            "as-is",
            "as shown",
            "extension",
            "undocumented",
            "keep",
            "order irrelevant",
            "documented",
        ]
        .iter()
        .any(|p| target.starts_with(p))
}

/// A catalogue row the reader handles without a profile rule: the decoder,
/// the lexer, the typed model, the converter, the writer's repair or the
/// detection. Each is checked: the input, read with the default builder and
/// written in `version`, gives output that contains every expected line
/// (`detect:<id>` checks the detected profile instead).
struct Builtin {
    row: &'static str,
    input: &'static [u8],
    version: GedcomVersion,
    expect: &'static [&'static str],
}

const V7: GedcomVersion = GedcomVersion::V7_0;
const V5: GedcomVersion = GedcomVersion::V5_5_1;

macro_rules! builtin {
    ($row:literal, $version:expr, $input:expr, [$($e:expr),* $(,)?]) => {
        Builtin { row: $row, input: $input, version: $version, expect: &[$($e),*] }
    };
}

const BUILTIN: &[Builtin] = &[
    builtin!("X-004", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 RFN geni:6000000000000000001\n0 TRLR\n", ["1 EXID 6000000000000000001\n2 TYPE https://gedcom.io/terms/v7/RFN#geni\n"]),
    builtin!("VD-101", V7, b"0 HEAD\n1 SOUR Geni.com\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 RFN geni:6000000000000000002\n0 TRLR\n", ["1 EXID 6000000000000000002\n2 TYPE https://gedcom.io/terms/v7/RFN#geni\n"]),
    builtin!("X-006", V7, b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 EXID Example-123\n2 TYPE https://www.wikitree.com/wiki/\n0 TRLR\n", ["1 EXID Example-123\n2 TYPE https://www.wikitree.com/wiki/\n"]),
    builtin!("X-007", V7, b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 EXID 123456\n2 TYPE https://www.findagrave.com/memorial/\n0 TRLR\n", ["1 EXID 123456\n2 TYPE https://www.findagrave.com/memorial/\n"]),
    builtin!("VD-133", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 AFN 1234-567\n0 TRLR\n", ["1 EXID 1234-567\n2 TYPE https://gedcom.io/terms/v7/AFN\n"]),
    builtin!("VD-134", V7, b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 EXID ABCD-12Y\n2 TYPE https://gedcom.io/exid-type/FamilySearch-PersonId\n0 TRLR\n", ["2 TYPE https://gedcom.io/exid-type/FamilySearch-PersonId\n"]),
    builtin!("VD-135", V7, b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 BURI\n2 EXID 98765\n3 TYPE https://www.findagrave.com/memorial/\n0 TRLR\n", ["3 TYPE https://www.findagrave.com/memorial/\n"]),
    builtin!("VD-103", V7, b"0 HEAD\n1 SOUR WikiTree.com\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 REFN 1234567\n2 TYPE wikitree.user_id\n0 TRLR\n", ["1 REFN 1234567\n2 TYPE wikitree.user_id\n"]),
    builtin!("X-008", V7, b"0 HEAD\n1 SOUR PAF\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 RIN 1234\n0 TRLR\n", ["1 EXID 1234\n2 TYPE https://gedcom.io/terms/v7/RIN#PAF\n"]),
    builtin!("VD-071", V7, b"0 HEAD\n1 SOUR MYHERITAGE\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 RIN MH:I1\n0 TRLR\n", ["1 EXID MH:I1\n2 TYPE https://gedcom.io/terms/v7/RIN#MYHERITAGE\n"]),
    builtin!("X-019", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Charles /Sample/\n1 ALIA Charlie\n0 TRLR\n", ["1 NAME Charlie\n2 TYPE AKA\n"]),
    builtin!("X-058", V7, b"0 HEAD\n1 SOUR Gramps\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 FAMC @F1@\n2 PEDI stepchild\n0 @F1@ FAM\n1 CHIL @I1@\n0 TRLR\n", ["2 PEDI OTHER\n3 PHRASE stepchild\n"]),
    builtin!("X-077", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 BAPM\n2 ASSO @I2@\n3 RELA godfather\n0 @I2@ INDI\n0 TRLR\n", ["2 ASSO @I2@\n3 ROLE GODP\n4 PHRASE godfather\n"]),
    builtin!("FR-028", V7, b"0 HEAD\n1 SOUR HEREDIS 14 PC\n1 GEDC\n2 VERS 5.5.1\n0 @F1@ FAM\n1 MARR\n2 ASSO @I5@\n3 TYPE INDI\n3 RELA Witness\n0 @I5@ INDI\n0 TRLR\n", ["2 ASSO @I5@\n3 ROLE WITN\n"]),
    builtin!("FR-213", V7, b"0 HEAD\n1 SOUR Geneanet\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 BAPM\n2 ASSO @I9@\n3 TYPE INDI\n3 RELA Witness\n3 NOTE Witness note\n0 @I9@ INDI\n0 TRLR\n", ["2 ASSO @I9@\n3 ROLE WITN\n4 PHRASE Witness\n3 NOTE Witness note\n"]),
    builtin!("X-118", V7, b"0 HEAD\n1 SOUR GenoPro\n1 GEDC\n2 VERS 5.5.1\n0 @G1@ GENOMAP\n1 NAME Example map\n0 TRLR\n", ["0 @G1@ _GENOMAP\n"]),
    builtin!("X-146", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @F1@ FAM\n1 MARR\n2 TYPE CIVIL\n0 TRLR\n", ["1 MARR\n2 TYPE CIVIL\n"]),
    builtin!("X-157", V7, b"0 HEAD\n1 SOUR Gramps\n1 GEDC\n2 VERS 5.5.1\n0 @F1@ FAM\n1 MARR\n2 TYPE Marriage of A and B\n0 TRLR\n", ["2 TYPE Marriage of A and B\n"]),
    builtin!("X-154", V7, b"0 HEAD\n1 SOUR FTM\n1 GEDC\n2 VERS 5.5.1\n0 @R1@ REPO\n1 NAME Example archive\n1 PHON\n1 NOTE\n0 TRLR\n", ["0 @R1@ REPO\n1 NAME Example archive\n0 TRLR\n"]),
    builtin!("X-155", V7, b"0 HEAD\n1 SOUR FTW\n1 GEDC\n2 VERS 5.5.1\n0 @S1@ SOUR\n1 TITL Example\n1 REPO\n2 CALN 12\n3 MEDI Other\n0 TRLR\n", ["1 REPO @VOID@\n2 CALN 12\n3 MEDI OTHER\n"]),
    builtin!("X-162", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 BIRT\n2 DATE @#DJULIAN@ BET 1600 AND 1610\n0 TRLR\n", ["2 DATE BET JULIAN 1600 AND JULIAN 1610\n"]),
    builtin!("FR-104", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 BIRT\n2 DATE @#DJULIAN@ BEF 1580\n0 TRLR\n", ["2 DATE BEF JULIAN 1580\n"]),
    builtin!("X-169", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 BIRT y\n0 TRLR\n", ["1 BIRT Y\n"]),
    builtin!("FR-083", V7, b"0 HEAD\n1 SOUR GeneWeb\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 SEX M\n1 DEAT Y\n0 TRLR\n", ["1 DEAT Y\n"]),
    builtin!("ENC-001", V7, b"0 HEAD\n1 SOUR FTW\n1 CHAR ANSI\n0 @I1@ INDI\n1 NAME Ren\xE9e /Exemple/\n0 TRLR\n", ["1 NAME Ren\u{e9}e /Exemple/\n"]),
    builtin!("TJ-009", V7, b"0 HEAD\n1 CHAR ANSI\n0 @I1@ INDI\n1 NAME Fran\xE7oise /Exemple/\n1 NOTE \x93quoted\x94\n0 TRLR\n", ["1 NAME Fran\u{e7}oise /Exemple/\n", "1 NOTE \u{201c}quoted\u{201d}\n"]),
    builtin!("FR-008", V7, b"0 HEAD\n1 SOUR HEREDIS 98 PC\n1 CHAR ANSI\n0 @I1@ INDI\n1 NAME C\xE9cile /Exemple/\n0 TRLR\n", ["1 NAME C\u{e9}cile /Exemple/\n"]),
    builtin!("FR-066", V7, b"0 HEAD\n1 SOUR GENEATIQUE\n1 CHAR ANSI\n0 @I1@ INDI\n1 NAME No\xEBl /Exemple/\n0 TRLR\n", ["1 NAME No\u{eb}l /Exemple/\n"]),
    builtin!("ENC-002", V7, b"0 HEAD\n1 SOUR GEDitCOM\n1 CHAR ASCII\n2 VERS MacOS Roman\n0 @I1@ INDI\n1 NAME Ren\x8Ee /Exemple/\n0 TRLR\n", ["1 NAME Ren\u{e9}e /Exemple/\n"]),
    builtin!("TJ-004", V7, b"0 HEAD\n1 SOUR GEDitCOM\n1 CHAR ASCII\n2 VERS MacOS Roman\n0 @I1@ INDI\n1 NAME Andr\x8E /Exemple/\n0 TRLR\n", ["1 NAME Andr\u{e9} /Exemple/\n"]),
    builtin!("ENC-003", V7, b"0 HEAD\n1 SOUR SyniumFamilyTree\n1 CHAR ASCII\n2 VERS MACINTOSH\n0 @I1@ INDI\n1 NAME Ren\x8Ee /Exemple/\n0 TRLR\n", ["1 NAME Ren\u{e9}e /Exemple/\n"]),
    builtin!("TJ-005", V7, b"0 HEAD\n1 SOUR MacFamilyTree\n1 CHAR MACINTOSH\n0 @I1@ INDI\n1 NAME Andr\x8E /Exemple/\n0 TRLR\n", ["1 NAME Andr\u{e9} /Exemple/\n"]),
    builtin!("ENC-004", V7, b"0 HEAD\n1 SOUR Reunion\n1 CHAR IBM-PC\n0 @I1@ INDI\n1 NAME Ren\x82e /Exemple/\n0 TRLR\n", ["1 NAME Ren\u{e9}e /Exemple/\n"]),
    builtin!("FR-101", V7, b"0 HEAD\n1 CHAR IBMPC\n0 @I1@ INDI\n1 NAME Jos\x82 /Exemple/\n0 TRLR\n", ["1 NAME Jos\u{e9} /Exemple/\n"]),
    builtin!("TJ-001", V7, b"0 HEAD\n1 SOUR BROSKEEP\n1 CHAR IBMPC\n0 @I1@ INDI\n1 NAME Ren\xE9e /Exemple/\n0 TRLR\n", ["1 NAME Ren\u{e9}e /Exemple/\n"]),
    builtin!("ENC-006", V7, b"0 HEAD\n1 SOUR GenoPro\n1 CHAR windows-1250\n0 @I1@ INDI\n1 NAME \xA3ucja /Przyk\xB3ad/\n0 TRLR\n", ["1 NAME \u{141}ucja /Przyk\u{142}ad/\n"]),
    builtin!("TJ-010", V7, b"0 HEAD\n1 CHAR WINDOWS-1251\n0 @I1@ INDI\n1 NAME \xC8\xE2\xE0\xED /\xCF\xF0\xE8\xEC\xE5\xF0/\n0 TRLR\n", ["1 NAME \u{418}\u{432}\u{430}\u{43d} /\u{41f}\u{440}\u{438}\u{43c}\u{435}\u{440}/\n"]),
    builtin!("ENC-007", V7, b"0 HEAD\n1 CHAR ISO-8859-2\n0 @I1@ INDI\n1 NAME Ji\xF8\xED /P\xF8\xEDklad/\n0 TRLR\n", ["1 NAME Ji\u{159}\u{ed} /P\u{159}\u{ed}klad/\n"]),
    builtin!("TJ-003", V7, b"0 HEAD\n1 SOUR Geves\n1 CHAR ASCII\n0 @I1@ INDI\n1 NAME Ren\xE9e /Exemple/\n0 TRLR\n", ["1 NAME Ren\u{e9}e /Exemple/\n"]),
    builtin!("TJ-006", V7, b"0 HEAD\n1 SOUR Famberry.com\n1 CHAR MACINTOSH\n0 @I1@ INDI\n1 NAME Ren\xE9e /Exemple/\n0 TRLR\n", ["1 NAME Ren\u{e9}e /Exemple/\n"]),
    builtin!("FR-070", V7, b"0 HEAD\n1 SOUR GeneWeb\n1 CHAR ANSI\n0 @I1@ INDI\n1 NAME H\xE9l\xE8ne /Exemple/\n1 SEX F\n0 TRLR\n", ["1 NAME H\u{e9}l\u{e8}ne /Exemple/\n"]),
    builtin!("ENC-008", V7, b"0 HEAD\n1 CHAR UNICODE\n0 @I1@ INDI\n1 NAME Zo\xC3\xAB /Exemple/\n0 TRLR\n", ["1 NAME Zo\u{eb} /Exemple/\n"]),
    builtin!("ENC-009", V7, b"0 HEAD\n1 CHAR UTF8\n0 @I1@ INDI\n1 NAME Zo\xC3\xAB /Exemple/\n0 TRLR\n", ["1 NAME Zo\u{eb} /Exemple/\n"]),
    builtin!("ENC-010", V7, b"0\0 \0H\0E\0A\0D\0\n\x001\0 \0C\0H\0A\0R\0 \0U\0N\0I\0C\0O\0D\0E\0\n\x000\0 \0@\0I\x001\0@\0 \0I\0N\0D\0I\0\n\x000\0 \0T\0R\0L\0R\0\n\0", ["0 @I1@ INDI\n"]),
    builtin!("TJ-011", V7, b"\xFF\xFE0\0 \0H\0E\0A\0D\0\n\x001\0 \0C\0H\0A\0R\0 \0U\0N\0I\0C\0O\0D\0E\0\n\x000\0 \0@\0I\x001\0@\0 \0I\0N\0D\0I\0\n\x000\0 \0T\0R\0L\0R\0\n\0", ["0 @I1@ INDI\n"]),
    builtin!("ENC-011", V7, b"\xEF\xBB\xBF0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n0 TRLR\n", ["0 @I1@ INDI\n"]),
    builtin!("TJ-012", V7, b"\xEF\xBB\xBF0 HEAD\n1 SOUR PAF\n1 CHAR UTF-8\n0 @I1@ INDI\n1 NAME Zo\xC3\xAB /Exemple/\n0 TRLR\n", ["1 NAME Zo\u{eb} /Exemple/\n"]),
    builtin!("TJ-015", V7, b"0 HEAD\n1 SOUR Exemple d\xE9mo\n1 CHAR ANSI\n0 TRLR\n", ["0 HEAD\n"]),
    builtin!("ENC-012", V7, b"0 HEAD\n1 CHAR ANSEL\n0 @I1@ INDI\n1 NAME Ren\xE2ee /Exemple/\n0 TRLR\n", ["1 NAME Ren\u{e9}e /Exemple/\n"]),
    builtin!("TJ-020", V7, b"0 HEAD\n1 CHAR ANSEL\n0 @I1@ INDI\n1 NAME Andr\xE2e /Exemple/\n0 TRLR\n", ["1 NAME Andr\u{e9} /Exemple/\n"]),
    builtin!("ENC-014", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n  1 NAME Ann /Example/\n\t2 GIVN Ann\n0 TRLR\n", ["1 NAME Ann /Example/\n2 GIVN Ann\n"]),
    builtin!("VD-088", V7, b"0 HEAD\n\t1 CHAR ANSI\n\t\t2 VERS (2010.3)\n0 @I1@ INDI\n\t1 NAME Ann /Example/\n0 TRLR\n", ["1 NAME Ann /Example/\n"]),
    builtin!("VD-102", V7, b"0 HEAD\n 1 SOUR Geni.com\n0 @I1@ INDI\n 1 NAME Ann /Example/\n  2 GIVN Ann\n0 TRLR\n", ["1 NAME Ann /Example/\n2 GIVN Ann\n"]),
    builtin!("FR-063", V7, b"0 HEAD\n  1 SOUR GENEATIQUE\n0 @I1@ INDI\n  1 NAME Ann /Example/\n0 TRLR\n", ["1 NAME Ann /Example/\n"]),
    builtin!("LINE-001", V7, b"0 HEAD\r1 GEDC\n2 VERS 5.5.1\n\r0 @I1@ INDI\r\n1 NAME Ann /Example/\r0 TRLR\r", ["1 NAME Ann /Example/\n"]),
    builtin!("TJ-022", V7, b"0 HEAD\n1 GEDC\r\n2 VERS 5.5.1\r0 @I1@ INDI\n0 TRLR\n", ["0 @I1@ INDI\n"]),
    builtin!("VD-112", V7, b"0 HEAD\r1 SOUR Reunion\r0 @I1@ INDI\r1 NAME Ann /Example/\r0 TRLR\r", ["1 NAME Ann /Example/\n"]),
    builtin!("LINE-005", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NOTE First\n2 CONT\n2 CONT Third\n0 TRLR\n", ["1 NOTE First\n2 CONT\n2 CONT Third\n"]),
    builtin!("LINE-006", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @R1@ REPO\n1 NAME Example\n1 EMAIL name@@example.org\n0 TRLR\n", ["1 EMAIL name@example.org\n"]),
    builtin!("TJ-039", V7, b"0 HEAD\n1 SOUR MYHERITAGE\n1 GEDC\n2 VERS 5.5.1\n0 @R1@ REPO\n1 NAME Example\n1 EMAIL anna@@example.org\n0 TRLR\n", ["1 EMAIL anna@example.org\n"]),
    builtin!("LINE-007", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I12345678901234567890123@ INDI\n1 NOTE xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxEND\n0 TRLR\n", ["xxxxEND\n"]),
    builtin!("TJ-030", V7, b"0 HEAD\n1 SOUR TMG\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa /Example/\n0 TRLR\n", ["aaaa /Example/\n"]),
    builtin!("FR-223", V7, b"0 HEAD\n1 SOUR Geneanet\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 SEX M\n1 SOUR Etat civil de Villefictive, acte 12, registre des naissances de l'an 1850, vue 42, transcrit par un contributeur fictif, avec mention marginale d'un mariage en 1875 a Villefictive et d'un deces en 1910 a Bourg-Exemple, cote 9 E 999, archives fictives\n0 TRLR\n", ["archives fictives\n"]),
    builtin!("MED-001", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @M1@ OBJE\n1 FILE C:\\Users\\Example\\Pictures\\photo.jpg\n2 FORM jpg\n0 TRLR\n", ["1 FILE file:///C:/Users/Example/Pictures/photo.jpg\n"]),
    builtin!("MED-002", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @M1@ OBJE\n1 FILE ./media/photo.jpg\n2 FORM jpg\n0 TRLR\n", ["media/photo.jpg\n"]),
    builtin!("MED-003", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 OBJE\n2 FILE photo.jpg\n3 FORM jpg\n2 TITL Portrait\n0 TRLR\n", ["2 FORM image/jpeg\n"]),
    builtin!("MED-004", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @M1@ OBJE\n1 FILE scan.jpg\n2 FORM JPG\n0 TRLR\n", ["2 FORM image/jpeg\n"]),
    builtin!("FR-207", V7, b"0 HEAD\n1 SOUR Geneanet\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 SEX F\n1 OBJE\n2 FILE http://gw.geneanet.org/public/img/media/deposits/00/00/1/medium.jpg?t=1\n0 TRLR\n", ["2 FORM image/jpeg\n"]),
    builtin!("PLAC-003", V7, b"0 HEAD\n1 SOUR FTW\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 BIRT\n2 PLAC 456 Example St, Example City, Example County, Exampleland\n3 FORM Street, City, County, Country\n0 TRLR\n", ["3 FORM Street, City, County, Country\n"]),
    builtin!("FR-004", V7, b"0 HEAD\n1 SOUR HEREDIS 14 PC\n1 GEDC\n2 VERS 5.5.1\n1 PLAC\n2 FORM Town, Area code, County, Region, Country, Subdivision\n0 TRLR\n", ["2 FORM Town, Area code, County, Region, Country, Subdivision\n"]),
    builtin!("FR-005", V7, b"0 HEAD\n1 SOUR HEREDIS 14 PC\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 BIRT\n2 PLAC Villefictive,12345,Departement-A,Region-B,FRANCE,\n0 TRLR\n", ["2 PLAC Villefictive,12345,Departement-A,Region-B,FRANCE,\n"]),
    builtin!("FR-098", V7, b"0 HEAD\n1 SOUR GeneWeb\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 SEX M\n1 BIRT\n2 PLAC [Hameau-D] - Villefictive, Departement-A, FRANCE\n0 TRLR\n", ["2 PLAC [Hameau-D] - Villefictive, Departement-A, FRANCE\n"]),
    builtin!("LDS-001", V7, b"0 HEAD\n1 SOUR PAF\n1 GEDC\n2 VERS 5.5.1\n0 @F1@ FAM\n1 SLGS\n2 STAT DNS/CAN\n3 DATE 1 JAN 1950\n0 TRLR\n", ["2 STAT DNS_CAN\n3 DATE 1 JAN 1950\n"]),
    builtin!("TJ-024", V7, b"0 HEAD\n1 SOUR FTW\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Anna /Example/ \n0 TRLR\n", ["1 NAME Anna /Example/"]),
    builtin!("TJ-027", V7, b"0 HEAD\n1 SOUR FTM\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NOTE The text may appear \n2 CONC as the first part.\n0 TRLR\n", ["1 NOTE The text may appear as the first part.\n"]),
    builtin!("TJ-031", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NOTE First line\n2 @N9@ CONT Second line\n0 TRLR\n", ["1 NOTE First line\n2 CONT Second line\n"]),
    builtin!("TJ-032", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.\n3 CONC 5.1\n0 @I1@ INDI\n1 BIRT\n2 DATE @#DJULIAN@ 1700\n0 TRLR\n", ["2 DATE JULIAN 1700\n"]),
    builtin!("TJ-033", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n", ["1 NAME Ann /Example/\n0 TRLR\n"]),
    builtin!("TJ-040", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @0100000c@ FAM\n0 TRLR\n", [" FAM\n"]),
    builtin!("FR-044", V7, b"0 HEAD\n1 SOUR HEREDIS 15 PC\n1 GEDC\n2 VERS 5.5.1\n0 @12I@ INDI\n0 @50@ FAM\n1 HUSB @12I@\n0 TRLR\n", ["0 @12I@ INDI\n", "0 @50@ FAM\n1 HUSB @12I@\n"]),
    builtin!("TJ-041", V7, b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I7@ SUBM\n1 NAME Example Submitter\n0 @I7@ INDI\n0 TRLR\n", ["0 @I7@ SUBM\n"]),
    builtin!("TJ-042", V5, b"0 HEAD\n1 SOUR WikiTree.com\n1 SUBM Anna Example\n1 GEDC\n2 VERS 5.5.1\n0 TRLR\n", ["1 SUBM @"]),
    builtin!("TJ-051", V7, b"0 HEAD\n1 SOUR PAF\n1 GEDC\n2 VERS 5.5\n0 @I1@ INDI\n1 BIRT\n2 DATE @#DJULIAN@ 1700\n0 TRLR\n", ["2 DATE JULIAN 1700\n"]),
    builtin!("VD-108", V7, b"0 HEAD\n1 SOUR SyniumFamilyTree\n1 CHAR UTF-8\n1 GEDC\n2 VERS 7.0.3\n2 FORM LINEAGE-LINKED\n0 TRLR\n", ["0 HEAD\n1 GEDC\n2 VERS 7.0\n"]),
    builtin!("FR-009", V7, b"0 HEAD\n1 SOUR HEREDIS 14 PC\n1 GEDC\n2 VERS 5.5.1\n1 LANG FRENCH\n0 TRLR\n", ["1 LANG fr\n"]),
    builtin!("FR-029", V7, b"0 HEAD\n1 SOUR HEREDIS 14 PC\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 BIRT\n2 DATE 5 MAR 1825\n3 TIME 14:30\n0 TRLR\n", ["2 DATE 5 MAR 1825\n3 TIME 14:30\n"]),
    builtin!("FR-069", V7, b"0 HEAD\n1 SOUR Ancestrologie\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 DEAT\n2 DATE 1 JAN 1900\n3 TIME 10:00\n0 TRLR\n", ["2 DATE 1 JAN 1900\n3 TIME 10:00\n"]),
    builtin!("FR-045", V7, b"0 HEAD\n1 SOUR HEREDIS 14 PC\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 BIRT\n2 DATE @#DHEBREW@ 1 TSH 5600\n0 TRLR\n", ["2 DATE HEBREW 1 TSH 5600\n"]),
    builtin!("FR-065", V7, b"0 HEAD\n1 SOUR GENEATIQUE\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 SOUR Source generale fictive\n0 TRLR\n", ["1 SOUR @"]),
    builtin!("FR-072", V7, b"0 HEAD\n1 SOUR GeneWeb\n1 GEDC\n2 VERS 5.5.1\n1 NOTE Notes of an example base\n0 TRLR\n", ["1 NOTE Notes of an example base\n"]),
    builtin!("FR-080", V7, b"0 HEAD\n1 SOUR GeneWeb\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 SEX M\n1 BIRT\n2 DATE (vers la Saint-Martin 1850)\n0 TRLR\n", ["3 PHRASE vers la Saint-Martin 1850\n"]),
    builtin!("FR-081", V7, b"0 HEAD\n1 SOUR GeneWeb\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 SEX M\n1 BIRT\n2 DATE 125 B.C.\n0 TRLR\n", ["2 DATE 125 BCE\n"]),
    builtin!("FR-082", V7, b"0 HEAD\n1 SOUR GeneWeb\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 SEX M\n1 BIRT\n2 DATE @#DFRENCH R@ 14 PLUV 2\n0 TRLR\n", ["2 DATE FRENCH_R 14 PLUV 2\n"]),
    builtin!("FR-090", V7, b"0 HEAD\n1 SOUR GeneWeb\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 SEX M\n1 TITL comte, Villefictive, 3\n2 DATE FROM 1800 TO 1820\n0 TRLR\n", ["1 TITL comte, Villefictive, 3\n"]),
    builtin!("FR-201", V5, b"0 HEAD\n1 SOUR Geneanet\n2 NAME GeneWeb\n2 VERS 7.0.0\n1 GEDC\n2 VERS 5.5.1\n0 TRLR\n", ["detect:geneanet"]),
    builtin!("TJ-056", V5, b"0 HEAD\n1 SOUR FTM\n2 VERS Family Tree Maker (22.0.0.1404)\n0 TRLR\n", ["detect:ftm", "version:22.0.0"]),
    builtin!("VD-089", V5, b"0 HEAD\n1 SOUR FTM\n2 VERS Family Tree Maker (22.2.5)\n0 TRLR\n", ["detect:ftm", "version:22.2.5"]),
    builtin!("TJ-058", V5, b"0 HEAD\n1 SOUR EXAMPLE\n1 DEST GED55\n0 TRLR\n", ["detect:generic"]),
    builtin!("TJ-059", V5, b"0 HEAD\n1 SOUR\n2 NAME MacFamilyTree\n2 VERS 8.0\n0 TRLR\n", ["detect:macfamilytree"]),
    builtin!("TJ-060", V5, b"0 HEAD\n1 SOUR HEREDIS PC2018\n0 TRLR\n", ["detect:heredis", "version:2018.0.0"]),
    builtin!("TJ-061", V5, b"0 HEAD\n1 SOUR The Next Generation of Genealogy Sitebuilding\n0 TRLR\n", ["detect:tng"]),
];

/// Lossless catalogue rows with a 7.0 target that nothing implements yet,
/// and why.
const NOT_YET: &[(&str, &str)] = &[
    (
        "TJ-016",
        "HEAD.SOUR in the file's own code page (Généalogie): detection reads the header byte for byte",
    ),
    (
        "TJ-017",
        "PAF 5.2.18's own ANSEL bytes D9 52 for an ellipsis: needs a PAF ANSEL table",
    ),
    (
        "TJ-028",
        "CONC splitting a UTF-8 sequence (WikiTree): needs joining before decoding",
    ),
    (
        "TJ-080",
        "`_STAT NOT MARRIED` under MARR: meaning inferred by the research, not confirmed",
    ),
    (
        "FR-057",
        "Généatique `_CCB`: the research gives no label for the event",
    ),
    (
        "FR-215",
        "a Geneanet person-sheet URL, not GEDCOM: OxidGene's Geneanet join",
    ),
    ("FR-216", "Geneanet person-sheet text, not GEDCOM"),
    ("FR-217", "Geneanet person-sheet text, not GEDCOM"),
    ("FR-218", "Geneanet person-sheet markup, not GEDCOM"),
    ("FR-220", "Geneanet person-sheet markup, not GEDCOM"),
    ("FR-225", "Geneanet's internal API, not GEDCOM"),
];

/// Export rows the writer itself follows, whatever the profile.
const EXPORT_BUILTIN: &[(&str, &str)] = &[
    (
        "EXF-041",
        "5.5.1 escapes `@#…@` are written without doubling their `@`",
    ),
    (
        "EXF-069",
        "CONC splits inside words, never after a space (generic-551)",
    ),
    (
        "EXF-071",
        "5.5.1 multimedia records keep one FILE each as the data holds",
    ),
    ("EXF-074", "RESN privacy: ExportOptions::omit_private"),
    (
        "EXF-076",
        "PEDI is standard; _FREL/_MREL only where they came from",
    ),
    (
        "EXF-077",
        "one NAME per name with TYPE aka/married: the model",
    ),
    ("EXF-078", "no SURN packing outside the Geneanet profiles"),
    (
        "EXF-080",
        "extensions are kept where they are (level 1 under INDI/FAM as read)",
    ),
    ("EXF-085", "OBJE records as the data holds them"),
    ("EXF-091", "MAP/LATI/LONG written standard"),
    (
        "EXF-095",
        "event payloads other than Y become notes in 7.x conversion",
    ),
    (
        "EXF-063",
        "citation TEXT is not CONC-wrapped below 255 characters",
    ),
    (
        "EXF-057",
        "one NAME per alternate name with TYPE: the model",
    ),
    ("EXF-058", "5.5.1 calendar escapes"),
    ("EXF-004", "GIVN written only as the data holds it"),
    (
        "EXF-007",
        "regnal numbers stay where the data puts them (NSFX, TITL)",
    ),
    ("EXF-009", "UTF-8, CHAR UTF-8 (generic-551 target)"),
    (
        "EXF-010",
        "LF line endings; nesting reported by the target's max_depth",
    ),
    ("EXF-016", "covered by EventsSaidY"),
    ("EXF-022", "ExportOptions::omit_private"),
    (
        "EXF-023",
        "English month dates, calendar escapes: the 5.5.1 writer",
    ),
    ("EXF-024", "the writer never writes numeric dates"),
    ("EXF-025", "B.C. in 5.5.1, BCE in 7.0: the value converter"),
    (
        "EXF-027",
        "godparents at person level with RELA: Godparents",
    ),
    ("EXF-029", "TITL is free text, written as held"),
    ("EXF-033", "HUSB and WIFE written conformant"),
    ("EXF-034", "records written in data order"),
    ("EXF-043", "UTF-8 without BOM, CHAR UTF-8"),
    ("EXF-055", "quoting the usual name is an importer option"),
    ("EXF-065", "media paths as held (absolute)"),
    (
        "EXF-066",
        "UTF-8 is the target; Windows-1252 only on request",
    ),
    ("EXF-067", "UTF-8 + CHAR UTF-8"),
    ("EXF-068", "5.5.1, FORM LINEAGE-LINKED: the 5.5.1 header"),
    (
        "EXF-079",
        "English months; one calendar per range: the value converter",
    ),
    ("EXF-082", "the writer writes its own HEAD.SOUR"),
    ("EXF-083", "exactly CHAR UTF-8"),
    ("EXF-084", "CONC split inside words"),
    ("EXF-086", "relative paths with '/': MediaPaths::Relative"),
    ("EXF-087", "5.5.1 FORM jpg: the converter"),
    ("EXF-090", "spec dates: the writer"),
    ("EXF-092", "5.5.1 target"),
    (
        "EXF-098",
        "single-line values are not wrapped below 255 characters",
    ),
    ("EXF-100", "7.0 or 5.5.1 UTF-8"),
    ("EXF-201", "two profiles, geneanet and geneweb"),
    ("EXF-202", "5.5.1 target for Geneanet"),
    ("EXF-203", "UTF-8 without BOM"),
    ("EXF-204", "never UTF-16"),
    (
        "EXF-217",
        "the quoted usual name; the user ticks the option",
    ),
    ("EXF-218", "a .ged, never GEDZIP (the Geneanet target)"),
    ("EXF-222", "limits reported by the target"),
    (
        "EXF-050",
        "5.5.1 .ged by default; ExportOptions::version for 7.0 GEDZIP",
    ),
    ("EXF-051", "UTF-8"),
    ("EXF-053", "MAP/LATI/LONG standard"),
    ("EXF-054", "ExtensionsAsEvents"),
    ("EXF-056", "MediaPaths::Relative"),
    ("EXF-059", "_PRIM on the preferred image"),
    ("EXF-060", "standard OBJE/FILE/FORM"),
    ("EXF-061", "UTF-8 + CHAR UTF-8"),
    ("EXF-062", "5.5.1 target"),
    ("EXF-064", "PlainNotes"),
    ("EXF-088", "XreflessRecords"),
    ("EXF-093", "GedcomL associations"),
    ("EXF-094", "Places with HEAD.PLAC.FORM"),
    ("EXF-099", "CitationRefnToSource; DATA.DATE as held"),
    ("EXC-001", "size limit reported"),
    ("EXC-002", "ExportOptions::omit_media"),
    (
        "EXC-006",
        "CENS as RESI is optional and changes the event: not done",
    ),
    ("EXC-018", "spoofing HEAD.SOUR is never done"),
    ("EXC-019", "alternate names as NAME TYPE aka: the model"),
    ("EXC-020", "one NAME per name, primary first: the model"),
    ("EXC-032", "absolute media paths as held"),
    ("EXC-034", "VERS 5.5.1"),
    (
        "EXC-035",
        "UTF-8, CONC split inside words, no trailing whitespace",
    ),
    ("EXC-036", "names over 40 characters reported"),
    ("EXC-047", "ExportOptions::omit_media"),
    ("EXC-048", "cosmetic: no change needed"),
    (
        "EXC-050",
        "no CONC under NAME below 255 characters; long names reported",
    ),
    ("EXC-051", "HTML notes kept"),
    ("EXC-053", "size limit reported"),
    ("EXC-055", "ExportOptions::omit_media"),
    ("EXC-056", "living persons kept (they stay hidden)"),
    ("EXC-057", "spoofing HEAD.SOUR is never done"),
    ("EXC-066", "SUBN is kept: RootsMagic ignores it"),
    ("EXC-068", "ExportOptions::omit_change_dates"),
    ("EXC-070", "PEDI written standard"),
    ("EXC-072", "5.5.1 UTF-8, never GEDZIP"),
    ("EXC-073", "notes kept as held"),
    ("EXC-074", "no CONC under NAME below 255 characters"),
    ("EXC-075", "UTF-8"),
    (
        "EXC-083",
        "Legacy's note-xref prefixes are not written: identifiers are kept",
    ),
    ("EXC-084", "PLAC.MAP as held"),
    ("EXC-087", "Legacy's formatting codes are not written"),
    ("EXC-088", "families over 60 children reported"),
    ("EXC-095", "UTF-8 with BOM"),
    ("EXC-098", "UTF-8 with BOM"),
    ("EXC-099", "long names reported"),
    ("EXC-100", "one line terminator"),
    ("EXC-101", "person count reported"),
    (
        "EXC-102",
        "omitting technical tags would lose data: not done",
    ),
    ("EXC-103", "standard tags only: the writer"),
    ("EXC-104", "living names are never masked"),
    ("EXC-105", "5.5.1 UTF-8"),
    ("EXC-106", "person count reported"),
    (
        "EXC-107",
        "size and count reported; ExportOptions::omit_media",
    ),
    (
        "EXC-112",
        "a NOTE record's text stays on its line (conformant)",
    ),
    ("EXC-113", "HTML notes kept"),
    ("EXC-114", "multiple NAME and CENS written standard"),
    ("EXC-116", "FORM and TITL under FILE: the 5.5.1 layout"),
    ("EXC-119", "CROP in 7.0; 5.5.1 keeps it as an extension"),
    ("EXC-120", "7.0 GEDZIP: gedzip::write_gedzip"),
    ("EXC-121", "UTF-8"),
    ("EXC-123", "_UID kept as held"),
    ("EXC-124", "standard tags; flat citations as held"),
    ("EXC-125", "7.0 UTF-8, relative media paths"),
    ("EXC-126", "7.0"),
    ("EXC-127", "foreign extensions interpreted on import"),
    ("EXC-130", "no CONC under NAME below 255 characters"),
    ("EXC-052", "5.5.1 target"),
];

/// Export rows nothing follows yet, and why.
const EXPORT_NOT_YET: &[(&str, &str)] = &[
    (
        "EXF-044",
        "shrinking a file by leaving out notes and sources is OxidGene's choice",
    ),
    ("EXF-049", "duplicating TITL as an event is optional"),
    ("EXF-205", "an empty PACS is reported, not made non-empty"),
];

/// Rows of the import and export ExportOptions.
const OPTION_ROWS: &[&str] = &[
    "EXF-022", "EXC-002", "EXC-047", "EXC-055", "EXC-056", "EXC-068", "EXC-074", "EXC-107",
];

fn cited_import_rows() -> BTreeSet<&'static str> {
    import_profiles().iter().flat_map(|p| p.rows()).collect()
}

fn cited_export_rows() -> BTreeSet<&'static str> {
    export_profiles().iter().flat_map(|p| p.rows()).collect()
}

#[test]
fn every_catalogue_row_cited_exists_and_every_lossless_row_is_covered() {
    let catalogue = table("catalogue.tsv");
    let cited = cited_import_rows();
    for row in &cited {
        assert!(
            catalogue.contains_key(*row),
            "a rule cites {row}, which the catalogue lacks"
        );
    }
    let builtin: BTreeSet<&str> = BUILTIN.iter().map(|b| b.row).collect();
    let not_yet: BTreeSet<&str> = NOT_YET.iter().map(|(r, _)| *r).collect();
    let needless: Vec<&&str> = builtin
        .iter()
        .chain(&not_yet)
        .filter(|row| {
            let entry = catalogue
                .get(**row)
                .unwrap_or_else(|| panic!("{row} is not a catalogue row"));
            !wants_rule(entry)
        })
        .collect();
    assert!(needless.is_empty(), "listed but need no rule: {needless:?}");
    for row in &not_yet {
        assert!(
            !cited.contains(row) && !builtin.contains(row),
            "{row} is covered and not yet"
        );
    }
    let uncovered: Vec<&String> = catalogue
        .iter()
        .filter(|(_, r)| wants_rule(r))
        .map(|(id, _)| id)
        .filter(|id| {
            !cited.contains(id.as_str())
                && !builtin.contains(id.as_str())
                && !not_yet.contains(id.as_str())
        })
        .collect();
    assert!(
        uncovered.is_empty(),
        "lossless rows with a 7.0 target nothing covers: {uncovered:?}"
    );
}

#[test]
fn every_export_row_cited_exists_and_every_row_is_followed() {
    let export = table("export.tsv");
    let cited = cited_export_rows();
    for row in &cited {
        assert!(
            export.contains_key(*row),
            "a profile cites {row}, which export.tsv lacks"
        );
    }
    let builtin: BTreeSet<&str> = EXPORT_BUILTIN
        .iter()
        .map(|(r, _)| *r)
        .chain(OPTION_ROWS.iter().copied())
        .collect();
    let not_yet: BTreeSet<&str> = EXPORT_NOT_YET.iter().map(|(r, _)| *r).collect();
    for row in builtin.iter().chain(&not_yet) {
        assert!(export.contains_key(*row), "{row} is not an export.tsv row");
    }
    let unfollowed: Vec<&String> = export
        .keys()
        .filter(|id| {
            !cited.contains(id.as_str())
                && !builtin.contains(id.as_str())
                && !not_yet.contains(id.as_str())
        })
        .collect();
    assert!(
        unfollowed.is_empty(),
        "export rows nothing follows: {unfollowed:?}"
    );
}

#[test]
fn the_reader_handles_its_rows() {
    for b in BUILTIN {
        let (data, report) = GedcomBuilder::new()
            .build_from_bytes_with_report(b.input)
            .unwrap();
        let out = GedcomWriter::new()
            .gedcom_version(b.version)
            .write_to_string(&data)
            .unwrap();
        for e in b.expect {
            if let Some(id) = e.strip_prefix("detect:") {
                assert_eq!(report.detection.profile().id(), id, "{}", b.row);
            } else if let Some(v) = e.strip_prefix("version:") {
                assert_eq!(
                    report
                        .detection
                        .producer_version()
                        .map(|v| v.to_string())
                        .as_deref(),
                    Some(v),
                    "{}",
                    b.row
                );
            } else {
                assert!(out.contains(e), "{}: {e:?} not in\n{out}", b.row);
            }
        }
    }
}

/// The fixtures of the import profiles: `fixtures/<profile>.ged`, written
/// in 7.0 after interpretation as `fixtures/<profile>.v70.ged`.
#[test]
fn import_fixtures() {
    let fixtures = dir().join("fixtures");
    let bless = std::env::var_os("BLESS").is_some();
    let mut seen = BTreeSet::new();
    for p in import_profiles() {
        let path = fixtures.join(format!("{}.ged", p.id()));
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        seen.insert(p.id());
        let (data, report) = GedcomBuilder::new()
            .build_from_bytes_with_report(bytes.clone())
            .unwrap();
        assert_eq!(
            report.detection.profile().id(),
            p.id(),
            "{}: {}",
            path.display(),
            report.detection
        );
        assert!(
            report.interpretation.changed(),
            "{}: nothing interpreted",
            p.id()
        );
        for v in [GedcomVersion::V7_0, GedcomVersion::V5_5_1] {
            let out = GedcomWriter::new()
                .gedcom_version(v)
                .write_to_string(&data)
                .unwrap();
            let deviations = oxidged::spec::validate_text(&out);
            assert!(
                deviations.is_empty(),
                "{} in {v}: {deviations:?}\n{out}",
                p.id()
            );
            // Interpretation applied again changes nothing.
            let again = GedcomBuilder::new()
                .profile(Some(p))
                .build_from_str(out.clone())
                .unwrap();
            let out2 = GedcomWriter::new()
                .gedcom_version(v)
                .write_to_string(&again)
                .unwrap();
            assert!(
                out2 == out,
                "{} in {v}: not idempotent: {}",
                p.id(),
                first_difference(&out, &out2)
            );
            if v == GedcomVersion::V7_0 {
                let expected = fixtures.join(format!("{}.v70.ged", p.id()));
                if bless {
                    std::fs::write(&expected, &out).unwrap();
                }
                let want = std::fs::read_to_string(&expected).unwrap_or_default();
                assert_eq!(
                    out,
                    want,
                    "{}: run with BLESS=1 after a deliberate change",
                    p.id()
                );
            }
        }
    }
    // (`oxidged` interprets nothing: `own_output_is_read_as_written`.)
    for p in import_profiles()
        .iter()
        .filter(|p| p.tier() <= 2 && p.id() != "oxidged")
    {
        assert!(seen.contains(p.id()), "no fixture for {}", p.id());
    }
}

/// The export profiles on one fictitious dataset: conformant, idempotent,
/// and as recorded in `fixtures/export/<profile>.ged` (with the report in
/// `<profile>.report`).
#[test]
fn export_fixtures() {
    let fixtures = dir().join("fixtures/export");
    let bless = std::env::var_os("BLESS").is_some();
    let source = std::fs::read(fixtures.join("source.ged")).unwrap();
    let data = GedcomBuilder::new().build_from_bytes(source).unwrap();
    for p in export_profiles() {
        let exporter = Exporter::new(p);
        let (out, report) = exporter.write_to_string(&data).unwrap();
        let version = p.target().version();
        let deviations = oxidged::spec::validate_text(&out);
        assert!(deviations.is_empty(), "{}: {deviations:?}\n{out}", p.id());
        let again = GedcomBuilder::new().build_from_str(out.clone()).unwrap();
        let (out2, _) = exporter.write_to_string(&again).unwrap();
        assert!(
            out2 == out,
            "{}: not idempotent: {}",
            p.id(),
            first_difference(&out, &out2)
        );
        let vers = format!("2 VERS {}\n", if version.is_v7() { "7.0" } else { "5.5.1" });
        assert!(
            out.replace("\r\n", "\n").contains(&vers),
            "{}: {out}",
            p.id()
        );
        let mut lines: Vec<String> = report.folds.iter().map(ToString::to_string).collect();
        lines.extend(report.limits.iter().map(|l| format!("limit: {l}")));
        lines.extend(report.repairs.iter().map(|r| format!("repair: {r}")));
        let text = lines.join("\n") + "\n";
        if bless {
            std::fs::write(fixtures.join(format!("{}.ged", p.id())), &out).unwrap();
            std::fs::write(fixtures.join(format!("{}.report", p.id())), &text).unwrap();
        }
        let want =
            std::fs::read_to_string(fixtures.join(format!("{}.ged", p.id()))).unwrap_or_default();
        assert_eq!(
            out,
            want,
            "{}: run with BLESS=1 after a deliberate change",
            p.id()
        );
        let want = std::fs::read_to_string(fixtures.join(format!("{}.report", p.id())))
            .unwrap_or_default();
        assert_eq!(
            text,
            want,
            "{}: run with BLESS=1 after a deliberate change",
            p.id()
        );
    }
}

#[test]
fn geneanet_export_folds_what_ged2gwb_needs() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Jean /Fictif/\n1 NAME Jean /Fictive/\n\
         1 NAME Johnny /Fictif/\n2 TYPE AKA\n1 SEX M\n1 OCCU Meunier\n1 OCCU Laboureur\n\
         1 OCCU Maire\n2 DATE 1850\n1 BAPM\n2 DATE 2 JAN 1820\n2 ASSO @I2@\n3 ROLE GODP\n\
         2 ASSO @I3@\n3 ROLE GODP\n2 ASSO @I4@\n3 ROLE WITN\n1 BURI\n1 SOUR @S1@\n2 PAGE folio 3\n\
         1 FAMC @F1@\n2 PEDI ADOPTED\n1 FAMS @F2@\n0 @I2@ INDI\n1 NAME Marie /Exemple/\n1 SEX F\n\
         0 @I3@ INDI\n1 NAME Paul /Exemple/\n1 SEX M\n0 @I4@ INDI\n1 NAME Luc /Exemple/\n\
         0 @F1@ FAM\n1 CHIL @I1@\n0 @F2@ FAM\n1 HUSB @I1@\n1 WIFE @I2@\n1 NO MARR\n\
         0 @S1@ SOUR\n1 TITL Registre fictif\n0 TRLR\n",
    );
    let (out, report) = Exporter::named("geneanet")
        .unwrap()
        .write_to_string(&data)
        .unwrap();
    assert!(
        out.contains("1 NAME Jean /Fictif/\n2 SURN Fictif,Fictive\n"),
        "{out}"
    );
    assert!(out.contains("1 NAME Johnny /Fictif/\n"), "{out}");
    assert!(out.contains("1 OCCU Meunier, Laboureur\n"), "{out}");
    assert!(out.contains("1 OCCU Maire\n2 DATE 1850\n"), "{out}");
    // Godfather then godmother, in the godchild.
    let godfather = out.find("1 ASSO @I3@\n2 RELA Godfather\n").unwrap();
    let godmother = out.find("1 ASSO @I2@\n2 RELA Godmother\n").unwrap();
    assert!(godfather < godmother, "{out}");
    assert!(
        out.contains("2 NOTE Witness: Luc Exemple (witness)\n"),
        "{out}"
    );
    assert!(out.contains("1 DEAT Y\n"), "{out}");
    assert!(out.contains("1 SOUR Registre fictif, folio 3\n"), "{out}");
    assert!(out.contains("1 ADOP\n2 FAMC @F1@\n3 ADOP BOTH\n"), "{out}");
    assert!(out.contains("1 EVEN\n2 TYPE unmarried\n"), "{out}");
    assert!(out.contains("2 VERS 5.5.1\n"));
    assert!(!out.contains("\n1 _") && !out.contains("\n2 _"), "{out}");
    let rows: BTreeSet<&str> = report
        .lossy()
        .flat_map(|f| f.rows.iter().copied())
        .collect();
    assert!(
        rows.contains("EXF-013") && rows.contains("EXF-003") && rows.contains("EXF-026"),
        "{rows:?}"
    );
}

#[test]
fn detection_reads_the_header_and_the_tags() {
    let cases: &[(&[u8], &str)] = &[
        (b"0 HEAD\n1 SOUR FTW\n2 VERS 4.0\n0 TRLR\n", "ftm"),
        (
            b"0 HEAD\n1 SOUR Ancestry.com Family Trees\n2 _TREE Example\n0 TRLR\n",
            "ancestry",
        ),
        (
            b"0 HEAD\n1 SOUR MYHERITAGE\n1 _RINS I-2\n0 TRLR\n",
            "myheritage",
        ),
        (
            b"0 HEAD\n1 SOUR PAF\n2 NAME Personal Ancestral File\n0 TRLR\n",
            "paf",
        ),
        (b"0 HEAD\n1 SOUR AncestQuest\n0 TRLR\n", "ancestral-quest"),
        (
            b"0 HEAD\n1 SOUR GeneWeb\n2 NAME gwb2ged\n2 CORP INRIA\n0 TRLR\n",
            "geneweb",
        ),
        (
            b"0 HEAD\n1 SOUR GeneWeb\n2 NAME gwb2ged\n2 CORP INRIA\n1 FILE base.ged.tmp\n0 TRLR\n",
            "geneanet",
        ),
        (
            b"0 HEAD\n1 SOUR Geneanet\n2 CORP Geneanet\n3 ADDR https://www.geneanet.org/\n0 TRLR\n",
            "geneanet",
        ),
        (
            b"0 HEAD\n1 SOUR Heredis PC2022\n1 _GUID x\n0 TRLR\n",
            "heredis",
        ),
        (b"0 HEAD\n1 SOUR RootsMagic\n0 TRLR\n", "rootsmagic"),
        (b"0 HEADER\n1 SOURCE FTW\n0 TRLR\n", "ftm"),
        (b"0 HEAD\n1 SOUR APPROVED_SOURCE_NAME\n0 TRLR\n", "generic"),
    ];
    for (bytes, id) in cases {
        assert_eq!(
            Detection::from_head(bytes).profile().id(),
            *id,
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }
    // A Gramps file that passed through Ancestry: the producer stays
    // Gramps, Ancestry's unambiguous tags are a hint.
    let file = b"0 HEAD\n1 SOUR Gramps\n0 @I1@ INDI\n1 SOUR @S1@\n2 _APID 1,7602::1\n0 @S1@ SOUR\n0 TRLR\n";
    let d = profile::detect(file);
    assert_eq!(d.profile().id(), "gramps");
    assert!(d.hints().iter().any(|h| h.id() == "ancestry"), "{d}");
    // Tags alone can name the producer of a header that says nothing.
    let d = profile::detect(b"0 HEAD\n0 @I1@ INDI\n1 _FIL LEGITIMATE_CHILD\n1 BIRT\n2 _FNA NO\n0 @F1@ FAM\n1 _UST MARRIED\n0 TRLR\n");
    assert_eq!(d.profile().id(), "generic");
    assert!(d.hints().iter().any(|h| h.id() == "heredis"), "{d}");
}

#[test]
fn producers_read_ansi_their_way() {
    // GeneWeb reads IBMPC as Latin-1, not code page 437.
    let bytes = b"0 HEAD\n1 SOUR GeneWeb\n1 CHAR IBMPC\n0 @I1@ INDI\n1 NAME Ren\xE9e /Exemple/\n1 SEX F\n0 TRLR\n";
    let (data, report) = GedcomBuilder::new()
        .build_from_bytes_with_report(bytes.to_vec())
        .unwrap();
    assert_eq!(report.encoding, Some(GedcomEncoding::Iso8859_1));
    assert_eq!(
        data.individuals().first().unwrap().full_name().as_deref(),
        Some("Renée Exemple")
    );
    let raw = GedcomBuilder::new()
        .interpretation(Interpretation::None)
        .build_from_bytes(bytes.to_vec())
        .unwrap();
    assert_eq!(
        raw.individuals().first().unwrap().full_name().as_deref(),
        Some("Ren\u{398}e Exemple")
    );
    // Central European bytes under ANSI: byte statistics find Windows-1250.
    let bytes = b"0 HEAD\n1 SOUR Geneanet\n1 CHAR ANSI\n0 @I1@ INDI\n1 NAME \xA3ucja /Kowalska/\n1 SEX F\n1 NOTE Urodzona w Bia\xB3ymstoku, \xBFona\n0 TRLR\n";
    let (data, report) = GedcomBuilder::new()
        .build_from_bytes_with_report(bytes.to_vec())
        .unwrap();
    assert_eq!(report.encoding, Some(GedcomEncoding::Windows1250));
    assert_eq!(
        data.individuals().first().unwrap().full_name().as_deref(),
        Some("Łucja Kowalska")
    );
    // French text under ANSI stays Windows-1252.
    let bytes = b"0 HEAD\n1 SOUR Geneanet\n1 CHAR ANSI\n0 @I1@ INDI\n1 NAME C\xE9cile /Exemple/\n1 NOTE \xC9l\xE8ve \xE0 l'\xE9cole, n\xE9e \xE0 Bourg\n0 TRLR\n";
    let (_, report) = GedcomBuilder::new()
        .build_from_bytes_with_report(bytes.to_vec())
        .unwrap();
    assert_eq!(report.encoding, Some(GedcomEncoding::Iso8859_1));
}

#[test]
fn interpretation_levels() {
    let file = "0 HEAD\n1 SOUR GeneWeb\n2 NAME gwb2ged\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n\
                1 NAME Jean /Fictif/\n2 SURN Fictif,Fictive\n1 SEX M\n0 @F1@ FAM\n1 HUSB @I1@\n\
                1 EVEN\n2 TYPE unmarried\n0 TRLR\n";
    let lossless = GedcomBuilder::new().build_from_str(file).unwrap();
    assert_eq!(lossless.individuals().first().unwrap().names().len(), 1);
    let best = GedcomBuilder::new()
        .interpretation(Interpretation::BestEffort)
        .build_from_str(file)
        .unwrap();
    assert_eq!(best.individuals().first().unwrap().names().len(), 2);
    // `unmarried` as NO MARR only in 7.x, and only at best effort.
    let out = GedcomWriter::new()
        .gedcom_version(GedcomVersion::V7_0)
        .write_to_string(&best)
        .unwrap();
    assert!(out.contains("1 NO MARR\n"), "{out}");
    let out = GedcomWriter::new()
        .gedcom_version(GedcomVersion::V7_0)
        .write_to_string(&lossless)
        .unwrap();
    assert!(!out.contains("1 NO MARR\n"), "{out}");
    assert_eq!(best.interpretation(), Interpretation::BestEffort);
    assert_eq!(best.producer().unwrap().profile().id(), "geneweb");
}

#[test]
fn seven_only_meanings_wait_for_the_conversion() {
    let file = "0 HEAD\n1 SOUR RootsMagic\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
                1 _UID 0123456789ABCDEF0123456789ABCDEF1A2B\n1 BIRT\n2 DATE ABT 1850\n2 _SDATE 1 JAN 1850\n\
                2 _SHAR @I2@\n3 ROLE Witness\n0 @I2@ INDI\n0 TRLR\n";
    let data = GedcomBuilder::new().build_from_str(file).unwrap();
    // In 5.5.1 the extensions stay as they are.
    let v5 = GedcomWriter::new().write_to_string(&data).unwrap();
    assert!(
        v5.contains("2 _SDATE 1 JAN 1850\n") && v5.contains("2 _SHAR @I2@\n"),
        "{v5}"
    );
    // In 7.0 they say what they mean.
    let v7 = GedcomWriter::new()
        .gedcom_version(GedcomVersion::V7_0)
        .write_to_string(&data)
        .unwrap();
    assert!(
        v7.contains("1 UID 0123456789ABCDEF0123456789ABCDEF1A2B\n"),
        "{v7}"
    );
    assert!(v7.contains("2 SDATE 1 JAN 1850\n"), "{v7}");
    assert!(v7.contains("2 ASSO @I2@\n3 ROLE WITN\n"), "{v7}");
    assert!(oxidged::spec::validate_text(&v7).is_empty());
}

/// A rule that gives an extension its standard tag keeps the extension
/// where the version has no such value: 5.5.1 has no sex `X`, 7.0 has.
#[test]
fn renamed_values_fit_the_version() {
    let written = |vers: &str, sex: &str, to: GedcomVersion| {
        let file = format!(
            "0 HEAD\n1 SOUR FAMILY_HISTORIAN\n1 GEDC\n2 VERS {vers}\n0 @I1@ INDI\n\
             1 NAME Ann /Example/\n1 _SEX {sex}\n0 TRLR\n"
        );
        let (data, report) = GedcomBuilder::new()
            .profile(profile::profile("family-historian"))
            .build_from_str_with_report(&file)
            .unwrap();
        let renamed = report
            .interpretation
            .mappings()
            .iter()
            .any(|m| &*m.tag == "_SEX" && m.action == Action::Replaced);
        let out = GedcomWriter::new()
            .gedcom_version(to)
            .write_to_string(&data)
            .unwrap();
        (renamed, out)
    };
    let (renamed, out) = written("5.5.1", "U", GedcomVersion::V5_5_1);
    assert!(
        renamed && out.contains("1 SEX U\n") && !out.contains("_SEX"),
        "{out}"
    );
    let (renamed, out) = written("5.5.1", "X", GedcomVersion::V5_5_1);
    assert!(
        !renamed && out.contains("1 _SEX X\n") && !out.contains("1 SEX"),
        "{out}"
    );
    let (_, out) = written("5.5.1", "X", GedcomVersion::V7_0);
    assert!(out.contains("1 SEX X\n") && !out.contains("_SEX"), "{out}");
    let (renamed, out) = written("7.0", "X", GedcomVersion::V7_0);
    assert!(
        renamed && out.contains("1 SEX X\n") && !out.contains("_SEX"),
        "{out}"
    );
}

/// A name of type AKA holding its own `_AKA` (as exports for importers
/// that read `_AKA` write it) says it already: nothing is lifted, so that
/// reading such an export changes nothing.
#[test]
fn a_name_says_its_own_aka() {
    let file = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n2 TYPE aka\n\
                2 _AKA Ann /Example/\n0 TRLR\n";
    let (data, report) = GedcomBuilder::new()
        .build_from_str_with_report(file)
        .unwrap();
    assert!(
        report
            .interpretation
            .mappings()
            .iter()
            .all(|m| &*m.tag != "_AKA"),
        "{report:?}"
    );
    let out = GedcomWriter::new().write_to_string(&data).unwrap();
    assert!(out.contains("2 _AKA Ann /Example/\n"), "{out}");
}

/// What this crate exported is read as written: its producer is
/// `oxidged`, no other producer is hinted at, nothing is interpreted.
#[test]
fn own_output_is_read_as_written() {
    let source = std::fs::read(dir().join("fixtures/export/source.ged")).unwrap();
    let data = GedcomBuilder::new().build_from_bytes(source).unwrap();
    for p in export_profiles() {
        let (out, _) = Exporter::new(p).write_to_string(&data).unwrap();
        let (_, report) = GedcomBuilder::new()
            .build_from_str_with_report(out.as_str())
            .unwrap();
        assert_eq!(report.detection.profile().id(), "oxidged", "{}", p.id());
        assert!(report.detection.hints().is_empty(), "{}", p.id());
        assert!(report.interpretation.is_empty(), "{}", p.id());
    }
}

/// What the crate wrote under its former name (`HEAD.SOUR ged_io`, as the
/// builds before it was named oxidged wrote) is its own output too: read as
/// written, and exported under its present name without a relabelling.
#[test]
fn own_output_under_the_former_name_is_read_as_written() {
    for (head, label) in [
        (
            "1 SOUR ged_io\n2 VERS 0.17.0\n2 NAME ged_io\n",
            "former name",
        ),
        (
            "1 SOUR oxidged\n2 VERS 0.1.0\n2 NAME oxidged\n",
            "present name",
        ),
        ("1 SOUR FTM\n2 _OXIDGED 0.1.0\n", "marker"),
    ] {
        let file = format!(
            "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n{head}\
             0 @I1@ INDI\n1 NAME Ann /Example/\n1 _FUN\n2 DATE 1900\n0 TRLR\n"
        );
        let (data, report) = GedcomBuilder::new()
            .build_from_str_with_report(file.as_str())
            .unwrap();
        assert_eq!(report.detection.profile().id(), "oxidged", "{label}");
        assert!(report.detection.hints().is_empty(), "{label}");
        assert!(report.interpretation.is_empty(), "{label}");
        if label == "former name" {
            let (out, report) = Exporter::named("geneanet")
                .unwrap()
                .write_to_string(&data)
                .unwrap();
            assert!(out.contains("1 SOUR oxidged\n"), "{out}");
            assert!(
                report.folds.iter().all(|f| !f.detail.contains("HEAD.SOUR")),
                "{report:?}"
            );
        }
    }
}

/// A program's dataset: its header names the program.
fn program_dataset() -> Dataset {
    let mut data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
         1 OCCU Weaver\n1 OCCU Miller\n0 TRLR\n",
    );
    let mut header = data.header().cloned().unwrap_or_default();
    header.source = Some(HeaderSource {
        product: "EXAMPLE_APP".into(),
        version: Some("1.2".into()),
        name: Some("Example App".into()),
        ..HeaderSource::default()
    });
    data.set_header(Some(header));
    data
}

/// An export names the program that wrote it when the program set
/// `HEAD.SOUR`, the crate beside it; reading it back finds the crate's own
/// output, which is not interpreted again.
#[test]
fn exports_name_the_program() {
    let data = program_dataset();
    let version = env!("CARGO_PKG_VERSION");
    for p in export_profiles() {
        let (out, report) = Exporter::new(p).write_to_string(&data).unwrap();
        let lines = out.replace("\r\n", "\n").replace('\r', "\n");
        assert!(
            lines.contains("1 SOUR EXAMPLE_APP\n2 VERS 1.2\n2 NAME Example App\n")
                && lines.contains(&format!("2 _OXIDGED {version}\n")),
            "{}: {out}",
            p.id()
        );
        assert!(
            report.folds.iter().all(|f| !f.detail.contains("HEAD.SOUR")),
            "{}: {report:?}",
            p.id()
        );
        if p.target().version().is_v7() {
            assert!(
                lines.contains("2 TAG _OXIDGED https://github.com/trois-six/oxidged/blob/main/docs/extensions.md#oxidged-_OXIDGED\n"),
                "{}: {out}",
                p.id()
            );
        }
        let (again, read) = GedcomBuilder::new()
            .build_from_str_with_report(out.as_str())
            .unwrap();
        assert_eq!(read.detection.profile().id(), "oxidged", "{}", p.id());
        assert!(read.interpretation.is_empty(), "{}", p.id());
        // Exported again, the same.
        let (out2, _) = Exporter::new(p).write_to_string(&again).unwrap();
        assert_eq!(out2, out, "{}", p.id());
    }
}

/// A dataset read from a file names the crate, unless the program renames
/// it, or asks otherwise ([`HeadSource`]).
#[test]
fn exports_name_the_crate_for_what_they_read() {
    let file = "0 HEAD\n1 SOUR FTM\n2 VERS 24.0\n1 GEDC\n2 VERS 5.5.1\n\
                0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n";
    let read = GedcomBuilder::new().build_from_str(file).unwrap();
    let geneanet = Exporter::named("geneanet").unwrap();
    let (out, report) = geneanet.write_to_string(&read).unwrap();
    assert!(
        out.contains("1 SOUR oxidged\n") && !out.contains("_OXIDGED"),
        "{out}"
    );
    assert!(
        report
            .folds
            .iter()
            .any(|f| f.detail == "HEAD.SOUR FTM written as oxidged"),
        "{report:?}"
    );
    // Asked: the dataset's, the crate beside it.
    let kept = geneanet.clone().options(ExportOptions {
        source: HeadSource::Dataset,
        ..ExportOptions::default()
    });
    let (out, _) = kept.write_to_string(&read).unwrap();
    assert!(
        out.contains("1 SOUR FTM\n2 VERS 24.0\n2 _OXIDGED "),
        "{out}"
    );
    // Renamed by the program: its own.
    let mut renamed = read.clone();
    let mut header = renamed.header().cloned().unwrap_or_default();
    if let Some(source) = &mut header.source {
        source.product = "EXAMPLE_APP".into();
    }
    renamed.set_header(Some(header));
    let (out, _) = geneanet.write_to_string(&renamed).unwrap();
    assert!(out.contains("1 SOUR EXAMPLE_APP\n"), "{out}");
    // Asked: the crate's, whatever the program set.
    let library = geneanet.options(ExportOptions {
        source: HeadSource::Library,
        ..ExportOptions::default()
    });
    let (out, _) = library.write_to_string(&program_dataset()).unwrap();
    assert!(
        out.contains("1 SOUR oxidged\n") && !out.contains("EXAMPLE_APP"),
        "{out}"
    );
}

#[test]
fn reports_are_queries() {
    let file = b"0 HEAD\n1 SOUR FTM\n2 VERS Family Tree Maker (22.2.5)\n1 GEDC\n2 VERS 5.5.1\n\
                 0 @I1@ INDI\n1 NAME Ann /Example/\n2 _MARNM Sample\n1 _FUN\n2 DATE 1900\n1 SOUR @S1@\n\
                 2 _APID 1,7602::1\n1 FAMC @F1@\n0 @F1@ FAM\n1 CHIL @I1@\n2 _FREL Natural\n2 _MREL Natural\n\
                 0 @S1@ SOUR\n0 TRLR\n";
    let (data, report) = GedcomBuilder::new()
        .build_from_bytes_with_report(file.to_vec())
        .unwrap();
    let i = &report.interpretation;
    assert!(i.for_row("X-020").any(|m| m.action == Action::Replaced));
    assert!(i.for_row("X-056").any(|m| m.action == Action::Added));
    assert!(i.with_action(Action::Kept).any(|m| &*m.tag == "_APID"));
    assert!(i.rows().contains(&"VD-069"));
    let ann = data.individuals().first().unwrap();
    assert_eq!(ann.names().len(), 2);
    let out = GedcomWriter::new().write_to_string(&data).unwrap();
    assert!(out.contains("1 FAMC @F1@\n2 PEDI birth\n"), "{out}");
    assert!(out.contains("2 _FREL Natural\n2 _MREL Natural\n"), "{out}");
    // The default read says nothing.
    let silent = GedcomBuilder::new()
        .build_from_bytes(file.to_vec())
        .unwrap();
    assert_eq!(silent, data);
    assert!(
        report
            .deviations
            .iter()
            .any(|d| d.to_string().contains("_FREL") || d.line > 0)
    );
}

#[test]
fn export_options_leave_out_only_when_asked() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 OBJE @M1@\n\
         0 @I2@ INDI\n1 NAME Bob /Example/\n1 RESN PRIVACY\n0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I1@\n\
         0 @M1@ OBJE\n1 FILE photo.jpg\n2 FORM image/jpeg\n0 TRLR\n",
    );
    let exporter = Exporter::named("familysearch").unwrap();
    let (out, report) = exporter.write_to_string(&data).unwrap();
    assert!(out.contains("OBJE") && out.contains("Bob"));
    assert!(report.lossy().next().is_none(), "{report:?}");
    let (out, report) = exporter
        .options(ExportOptions {
            omit_media: true,
            omit_private: true,
            ..ExportOptions::default()
        })
        .write_to_string(&data)
        .unwrap();
    assert!(
        !out.contains("OBJE") && !out.contains("Bob") && !out.contains("@I2@"),
        "{out}"
    );
    assert_eq!(report.lossy().count(), 2);
    assert!(oxidged::spec::validate_text(&out).is_empty());
}

/// Exports leave the credentials a file holds out, and say so, unless
/// asked to keep them.
#[test]
fn exports_leave_credentials_out() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @S1@ SOUR\n1 TITL Example register\n\
         1 _DBID example-db\n2 _USER reader\n2 _PASS secret\n0 TRLR\n",
    );
    for p in export_profiles() {
        let (out, report) = Exporter::new(p).write_to_string(&data).unwrap();
        assert!(
            !out.contains("secret") && !out.contains("reader"),
            "{}: {out}",
            p.id()
        );
        let fold = report.for_record("@S1@").next();
        assert!(
            fold.is_some_and(|f| f.lossy && f.detail.contains("credentials")),
            "{}: {report:?}",
            p.id()
        );
    }
    let (out, report) = Exporter::named("generic-551")
        .unwrap()
        .options(ExportOptions {
            credentials: oxidged::Credentials::Written,
            ..ExportOptions::default()
        })
        .write_to_string(&data)
        .unwrap();
    assert!(out.contains("2 _PASS secret\n"), "{out}");
    assert!(report.lossy().next().is_none(), "{report:?}");
}

/// `docs/extensions.md` documents every extension tag a profile keeps and
/// declares in 7.x with oxidged's URI: its anchor is the URI's fragment.
#[test]
fn extension_docs_are_current() {
    let catalogue = table("catalogue.tsv");
    let mut doc = String::from(
        "# Extension Tags\n\n\
         > Part of the [oxidged documentation](index.md).\n\
         > See also: [Producer Profiles](profiles.md)\n\n\
         <!-- Generated by `BLESS=1 cargo test --test profiles extension_docs_are_current`: \
         do not edit by hand. -->\n\n\
         When oxidged reads a file, its producer profile keeps the extension tags it knows and \
         that have no standard form as they are; when it writes GEDCOM 7.x, it declares each \
         of them in `HEAD.SCHMA`. No producer publishes URIs for its own tags, so oxidged \
         documents them here, one anchor per producer and tag: the URI is \
         `https://github.com/trois-six/oxidged/blob/main/docs/extensions.md#<profile>-<TAG>`. \
         The anchors are stable: a tag documented once keeps its anchor. Tags with a URI \
         registered elsewhere (the GEDCOM-L addendum's) are declared with that URI and \
         listed here only for reference.\n\n\
         Facts come from the compatibility research (catalogue rows in brackets); every \
         example in it is fictitious.\n",
    );
    for p in import_profiles() {
        let tags: Vec<_> = p.documented_tags().collect();
        if tags.is_empty() {
            continue;
        }
        doc.push_str(&format!("\n## {} (`{}`)\n", p.label(), p.id()));
        let mut anchored = BTreeSet::new();
        for t in tags {
            // One anchor per producer and tag: a tag documented in several
            // places lists each under the first.
            if anchored.insert(t.tag) {
                doc.push_str(&format!(
                    "\n<a id=\"{}-{}\"></a>\n\n### `{}` ({})\n\n",
                    p.id(),
                    t.tag,
                    t.tag,
                    p.id()
                ));
            } else {
                doc.push('\n');
            }
            doc.push_str(&format!(
                "- Where: `{}`\n",
                if t.context.is_empty() {
                    "(record)"
                } else {
                    t.context
                }
            ));
            for row in t.rows {
                let meaning = catalogue
                    .get(*row)
                    .map(|r| r["meaning"].clone())
                    .unwrap_or_default();
                doc.push_str(&format!("- Meaning: {meaning} [{row}]\n"));
            }
            if t.registered {
                doc.push_str(&format!(
                    "- Declared with its registered URI: <{}>\n",
                    t.uri
                ));
            } else {
                doc.push_str(&format!("- Declared as `TAG {} {}`\n", t.tag, t.uri));
            }
        }
    }
    // The crate's own (`profile::HeadSource`).
    doc.push_str(&format!(
        "\n## oxidged (`oxidged`)\n\n<a id=\"oxidged-_OXIDGED\"></a>\n\n### `_OXIDGED` (oxidged)\n\n\
         - Where: `HEAD.SOUR`\n\
         - Meaning: the version of oxidged that wrote the file, when the program that \
         wrote its data names itself in `HEAD.SOUR`; reading the file back finds the \
         `oxidged` profile, which interprets nothing again\n\
         - Declared as `TAG _OXIDGED {}`\n",
        profile::docs_uri("oxidged", "_OXIDGED")
    ));
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/extensions.md");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    // The page is an OKF concept: its frontmatter dates the body's last
    // change, kept as long as the body is the same.
    let (generated, body) = split_frontmatter(&current);
    let at = match generated {
        Some(at) if body == doc => at.to_string(),
        _ => utc_now(),
    };
    let page = format!(
        "---\n\
         type: \"Reference\"\n\
         title: \"Extension Tags\"\n\
         description: \"The producers' extension tags oxidged keeps and declares in GEDCOM 7.x \
         headers, one anchor per producer and tag, which the URIs it declares point to.\"\n\
         tags: [oxidged, profiles, extensions]\n\
         generated: {{ by: human:maintainer, at: {at} }}\n\
         ---\n\n{doc}"
    );
    if std::env::var_os("BLESS").is_some() {
        std::fs::write(&path, &page).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        want == page,
        "docs/extensions.md is stale: run with BLESS=1"
    );
    // Every URI a profile declares with resolves to an anchor of the page.
    assert_eq!(doc.matches("<a id=\"oxidged-_OXIDGED\"></a>").count(), 1);
    for p in import_profiles() {
        for t in p.documented_tags().filter(|t| !t.registered) {
            let fragment = t.uri.split_once('#').map(|(_, f)| f).unwrap();
            assert_eq!(
                doc.matches(&format!("<a id=\"{fragment}\"></a>")).count(),
                1,
                "{}",
                t.uri
            );
        }
    }
}

/// The `generated.at` of a document's OKF frontmatter and the body after
/// it (the whole text when it has none).
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    let Some((front, body)) = rest.split_once("\n---\n\n") else {
        return (None, text);
    };
    let at = front
        .lines()
        .find_map(|l| l.strip_prefix("generated: { by: human:maintainer, at: "))
        .and_then(|l| l.strip_suffix(" }"));
    (at, body)
}

/// The current time as an ISO 8601 UTC datetime, to the second.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (H. Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
