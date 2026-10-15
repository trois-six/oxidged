//! The schema tests of ArmidaleSoftware/gedcom7 (MIT, commit `dabc9a9`,
//! `Tests/SchemaTests.cs`) against `oxidged::spec`: every `ValidateGedcomText`
//! and `ValidateGedcomFile` call of the C# suite, with its value lists, as a
//! dataset and its expected outcome — valid, or a deviation of a given kind
//! on a given line.
//!
//! The documents are rebuilt here from the C# helpers; they hold no personal
//! data. Where Armidale and the specification disagree, the specification
//! wins and the case says why ([`Expect::Departure`]).

use oxidged::spec::{Deviation, DeviationKind as K, validate_bytes, validate_text};
use std::path::Path;

/// What a case expects.
#[derive(Clone, Debug)]
enum Expect {
    /// No deviation.
    Valid,
    /// A deviation of one of these kinds, on this line (`None`: any line).
    Invalid(Option<u32>, &'static [K]),
    /// Armidale expects the opposite of the specification: the expected
    /// outcome here, and the reason.
    Departure(Box<Expect>, &'static str),
}

use Expect::{Invalid, Valid};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum V {
    V551,
    V70,
    V71,
}

fn vers(v: V) -> &'static str {
    match v {
        V::V551 => "5.5.1",
        V::V70 => "7.0",
        V::V71 => "7.1",
    }
}

/// `GetGedcomVersionHeaderAdditions`.
fn additions(v: V) -> &'static str {
    match v {
        V::V551 => {
            "\n2 FORM LINEAGE-LINKED\n1 SOUR Test\n1 CHAR ASCII\n1 SUBM @S1@\n0 @S1@ SUBM\n1 NAME Test"
        }
        _ => "",
    }
}

fn extra(v: V) -> u32 {
    if v == V::V551 { 6 } else { 0 }
}

/// `VERS` and the additions, as the helpers concatenate them.
fn head(v: V) -> String {
    format!("0 HEAD\n1 GEDC\n2 VERS {}{}\n", vers(v), additions(v))
}

struct Cases(Vec<(String, String, Expect)>);

impl Cases {
    fn add(&mut self, name: impl Into<String>, text: impl Into<String>, expect: Expect) {
        self.0.push((name.into(), text.into(), expect));
    }
}

fn xref_cases(c: &mut Cases, v: V) {
    let n = extra(v);
    let h = head(v);
    let vs = vers(v);
    let add = additions(v);
    c.add(
        format!("{vs} xref on HEAD"),
        format!("0 @H1@ HEAD\n1 GEDC\n2 VERS {vs}{add}\n0 TRLR\n"),
        Invalid(Some(1), &[K::Xref]),
    );
    c.add(
        format!("{vs} record without xref"),
        format!("{h}0 INDI\n0 TRLR\n"),
        Valid,
    );
    c.add(
        format!("{vs} xref on TRLR"),
        format!("{h}0 @T1@ TRLR\n"),
        Invalid(Some(4 + n), &[K::Xref]),
    );
    c.add(
        format!("{vs} xref without leading @"),
        format!("{h}0 I1@ INDI\n0 TRLR\n"),
        Invalid(Some(4 + n), &[K::UnknownTag, K::Misplaced]),
    );
    c.add(
        format!("{vs} xref without trailing @"),
        format!("{h}0 @I1 INDI\n0 TRLR\n"),
        Invalid(Some(4 + n), &[K::Xref]),
    );
    c.add(
        format!("{vs} empty xref"),
        format!("{h}0 @ INDI\n0 TRLR\n"),
        Invalid(Some(4 + n), &[K::Xref]),
    );
    c.add(
        format!("{vs} valid xref"),
        format!("{h}0 @I1@ INDI\n0 TRLR\n"),
        Valid,
    );
}

fn header_and_trailer_cases(c: &mut Cases, v: V) {
    let n = extra(v);
    let h = head(v);
    let vs = vers(v);
    c.add(
        format!("{vs} missing TRLR"),
        h.clone(),
        Invalid(None, &[K::Header]),
    );
    c.add(format!("{vs} minimal"), format!("{h}0 TRLR\n"), Valid);
    c.add(
        format!("{vs} TRLR first"),
        format!("0 TRLR\n{h}"),
        Invalid(Some(1), &[K::Header]),
    );
    c.add(
        format!("{vs} TRLR with a substructure"),
        format!("{h}0 TRLR\n1 _EXT bad\n"),
        Invalid(Some(4 + n), &[K::Header]),
    );
    c.add(
        format!("{vs} two HEAD"),
        format!("{h}0 HEAD\n1 GEDC\n2 VERS {vs}\n0 TRLR\n"),
        Invalid(Some(4 + n), &[K::Header]),
    );
    c.add(
        format!("{vs} two TRLR"),
        format!("{h}0 TRLR\n0 TRLR\n"),
        Invalid(Some(5 + n), &[K::Header]),
    );
}

/// `ValidateSpacing(versionString)`: no header additions.
fn spacing_cases(c: &mut Cases, vs: &str) {
    c.add(
        format!("{vs} two spaces before the tag"),
        format!("0 HEAD\n1 GEDC\n2  VERS {vs}\n0 TRLR\n"),
        Invalid(Some(3), &[K::LineSyntax]),
    );
    c.add(
        format!("{vs} trailing space"),
        format!("0 HEAD\n1 GEDC \n2 VERS {vs}\n0 TRLR\n"),
        Invalid(Some(2), &[K::LineSyntax]),
    );
}

fn name_cases(c: &mut Cases, v: V) {
    let h = head(v);
    let n = extra(v);
    let vs = vers(v);
    for value in ["John Smith", "John /Smith/", "John /Smith/ Jr."] {
        c.add(
            format!("{vs} name {value:?}"),
            format!("{h}0 @I1@ INDI\n1 NAME {value}\n0 TRLR\n"),
            Valid,
        );
    }
    for value in ["/", "a/b/c/d", "a\tb"] {
        // The invalid-name helper writes no trailer.
        c.add(
            format!("{vs} name {value:?}"),
            format!("{h}0 @I1@ INDI\n1 NAME {value}\n"),
            Invalid(Some(5 + n), &[K::Payload, K::Character]),
        );
    }
}

fn date_value(c: &mut Cases, v: V, value: &str, valid: bool) {
    let h = head(v);
    let n = extra(v);
    c.add(
        format!("{} date value {value:?}", vers(v)),
        format!("{h}0 @I1@ INDI\n1 DEAT\n2 DATE {value}\n0 TRLR\n"),
        if valid {
            Valid
        } else {
            Invalid(Some(6 + n), &[K::Payload])
        },
    );
}

fn date_period(c: &mut Cases, v: V, value: &str, valid: bool) {
    let h = head(v);
    let n = extra(v);
    c.add(
        format!("{} date period {value:?}", vers(v)),
        format!("{h}0 @I1@ SOUR\n1 DATA\n2 EVEN MARR\n3 DATE {value}\n0 TRLR\n"),
        if valid {
            Valid
        } else {
            Invalid(Some(7 + n), &[K::Payload])
        },
    );
}

fn common_date_values(c: &mut Cases, v: V) {
    for value in [
        "3 DEC 2023",
        "DEC 2023",
        "2023",
        "TO 3 DEC 2023",
        "TO DEC 2023",
        "TO 2023",
        "FROM 03 DEC 2023",
        "FROM 2000 TO 2020",
        "FROM MAR 2000 TO JUN 2000",
        "FROM 30 NOV 2000 TO 1 DEC 2000",
        "BEF 3 DEC 2023",
        "BEF DEC 2023",
        "BEF 2023",
        "AFT 03 DEC 2023",
        "BET 2000 AND 2020",
        "BET MAR 2000 AND JUN 2000",
        "BET 30 NOV 2000 AND 1 DEC 2000",
        "ABT 3 DEC 2023",
        "CAL DEC 2023",
    ] {
        date_value(c, v, value, true);
    }
    for value in [
        "TO 40 DEC 2023",
        "TO 3 JUNE 2023",
        "TO ABC 2023",
        "BEF 40 DEC 2023",
        "BEF 3 JUNE 2023",
        "BEF ABC 2023",
        "BET 2000",
    ] {
        date_value(c, v, value, false);
    }
    for value in ["TO 3 dec 2023", "BEF 3 dec 2023"] {
        if v == V::V551 {
            c.add(
                format!("5.5.1 date value {value:?}"),
                format!("{}0 @I1@ INDI\n1 DEAT\n2 DATE {value}\n0 TRLR\n", head(v)),
                Expect::Departure(
                    Box::new(Valid),
                    "5.5.1 controlled values, months included, are case-insensitive (p. 21)",
                ),
            );
        } else {
            date_value(c, v, value, false);
        }
    }
}

fn common_date_periods(c: &mut Cases, v: V) {
    for value in [
        "TO 3 DEC 2023",
        "TO DEC 2023",
        "TO 2023",
        "FROM 03 DEC 2023",
        "FROM 2000 TO 2020",
        "FROM MAR 2000 TO JUN 2000",
        "FROM 30 NOV 2000 TO 1 DEC 2000",
    ] {
        date_period(c, v, value, true);
    }
    for value in ["2023", "TO 40 DEC 2023", "TO 3 JUNE 2023", "TO ABC 2023"] {
        date_period(c, v, value, false);
    }
    if v == V::V551 {
        c.add(
            "5.5.1 date period \"TO 3 dec 2023\"",
            format!(
                "{}0 @I1@ SOUR\n1 DATA\n2 EVEN MARR\n3 DATE TO 3 dec 2023\n0 TRLR\n",
                head(v)
            ),
            Expect::Departure(
                Box::new(Valid),
                "5.5.1 controlled values, months included, are case-insensitive (p. 21)",
            ),
        );
    } else {
        date_period(c, v, "TO 3 dec 2023", false);
    }
}

fn exact_dates(c: &mut Cases, v: V) {
    let vs = vers(v);
    let add = additions(v);
    for value in ["3 DEC 2023", "03 DEC 2023"] {
        c.add(
            format!("{vs} exact date {value:?}"),
            format!("0 HEAD\n1 DATE {value}\n1 GEDC\n2 VERS {vs}{add}\n0 TRLR\n"),
            Valid,
        );
    }
    for value in ["invalid", "3 dec 2023", "3 JUNE 2023", "DEC 2023", "2023"] {
        let expect = if v == V::V551 && value == "3 dec 2023" {
            Expect::Departure(
                // The invalid-exact-date helper writes no trailer.
                Box::new(Invalid(None, &[K::Header])),
                "5.5.1 controlled values, months included, are case-insensitive (p. 21)",
            )
        } else {
            Invalid(Some(2), &[K::Payload])
        };
        c.add(
            format!("{vs} exact date {value:?}"),
            format!("0 HEAD\n1 DATE {value}\n1 GEDC\n2 VERS {vs}{add}\n"),
            expect,
        );
    }
}

fn times(c: &mut Cases, v: V) {
    let vs = vers(v);
    let add = additions(v);
    let doc = |value: &str| {
        format!("0 HEAD\n1 DATE 1 DEC 2023\n2 TIME {value}\n1 GEDC\n2 VERS {vs}{add}\n0 TRLR\n")
    };
    for value in ["02:50", "2:50"] {
        c.add(format!("{vs} time {value:?}"), doc(value), Valid);
    }
    c.add(
        format!("{vs} time \"2:50:00.00Z\""),
        doc("2:50:00.00Z"),
        if v == V::V551 {
            Expect::Departure(
                Box::new(Invalid(Some(3), &[K::Payload])),
                "5.5.1 TIME_VALUE is hh:mm:ss.fs, without the 7.0 Z (p. 63)",
            )
        } else {
            Valid
        },
    );
    for value in [
        " ", "invalid", "000:00", "24:00:00", "2:5", "2:60", "2:00:60",
    ] {
        c.add(
            format!("{vs} time {value:?}"),
            doc(value),
            Invalid(Some(3), &[K::Payload, K::LineSyntax]),
        );
    }
}

fn xref_payloads(c: &mut Cases, vs: &str) {
    let h = format!("0 HEAD\n1 GEDC\n2 VERS {vs}\n");
    c.add(
        "multimedia link without its closing @",
        format!("{h}1 SUBM @S1@\n0 @S1@ SUBM\n1 NAME Test\n1 OBJE @O1\n0 TRLR\n"),
        Invalid(Some(7), &[K::Payload]),
    );
    c.add(
        "SUBM without payload",
        format!("{h}1 SUBM\n0 TRLR\n"),
        Invalid(Some(4), &[K::Payload]),
    );
    c.add(
        "SUBM without leading @",
        format!("{h}1 SUBM S1@\n0 TRLR\n"),
        Invalid(Some(4), &[K::Payload]),
    );
    c.add(
        "SUBM to no record",
        format!("{h}1 SUBM @S1@\n0 TRLR\n"),
        Invalid(Some(4), &[K::DanglingPointer]),
    );
    c.add(
        "SUBM to an INDI",
        format!("{h}1 SUBM @I1@\n0 @I1@ INDI\n0 TRLR\n"),
        Invalid(Some(4), &[K::PointerTarget]),
    );
    c.add(
        "SUBM to an extension record",
        format!("{h}1 SUBM @I1@\n0 @I1@ _SUBM\n0 TRLR\n"),
        Invalid(Some(4), &[K::PointerTarget]),
    );
    c.add(
        "extension pointing anywhere",
        format!("{h}1 _SUBM @I1@\n0 @I1@ INDI\n0 TRLR\n"),
        Valid,
    );
}

fn schema_551(c: &mut Cases) {
    let v = V::V551;
    header_and_trailer_cases(c, v);
    xref_cases(c, v);
    let h = head(v);
    c.add(
        "5.5.1 @VOID@ is an identifier",
        format!("{h}0 @VOID@ INDI\n0 TRLR\n"),
        Valid,
    );
    c.add(
        "5.5.1 # inside an identifier",
        format!("{h}0 @I#1@ INDI\n0 TRLR\n"),
        Valid,
    );
    c.add(
        "5.5.1 # first in an identifier",
        format!("{h}0 @#I1@ INDI\n0 TRLR\n"),
        Invalid(Some(10), &[K::Xref]),
    );
    c.add(
        "5.5.1 _ in an identifier",
        format!("{h}0 @I_1@ INDI\n0 TRLR\n"),
        Expect::Departure(
            Box::new(Valid),
            "alpha includes the underscore, and an identifier is alphanum then pointer_char (pp. 11, 13)",
        ),
    );
    c.add(
        "5.5.1 lower case identifier",
        format!("{h}0 @i1@ INDI\n0 TRLR\n"),
        Valid,
    );

    common_date_values(c, v);
    for value in [
        "1740/41",
        "@#DGREGORIAN@ 1740/41",
        "@#DGREGORIAN@ 20 B.C.",
        "@#DHEBREW@ 1 TSH 1",
        "TO @#DGREGORIAN@ 20 B.C.",
        "FROM @#DHEBREW@ 1 TSH 1",
        "FROM @#DGREGORIAN@ 20 B.C. TO @#DGREGORIAN@ 12 B.C.",
        "BEF @#DGREGORIAN@ 20 B.C.",
        "AFT @#DHEBREW@ 1 TSH 1",
        "BET @#DGREGORIAN@ 20 B.C. AND @#DGREGORIAN@ 12 B.C.",
        "EST @#DGREGORIAN@ 20 B.C.",
    ] {
        date_value(c, v, value, true);
    }
    for value in [
        "FROM @#DHEBREW@ 1 TSH 1 B.C.",
        "AFT @#DHEBREW@ 1 TSH 1 B.C.",
    ] {
        date_value(c, v, value, false);
    }
    common_date_periods(c, v);
    for value in [
        "TO @#DGREGORIAN@ 20 B.C.",
        "FROM @#DHEBREW@ 1 TSH 1",
        "FROM @#DGREGORIAN@ 20 B.C. TO @#DGREGORIAN@ 12 B.C.",
    ] {
        date_period(c, v, value, true);
    }
    date_period(c, v, "FROM @#DHEBREW@ 1 TSH 1 B.C.", false);

    let add = additions(v);
    c.add(
        "5.5.1 GEDC with a payload",
        format!("0 HEAD\n1 GEDC 1\n2 VERS 5.5.1{add}\n0 TRLR\n"),
        Invalid(Some(2), &[K::Payload]),
    );
    for (value, expect) in [
        ("0", Valid),
        ("-1", Invalid(Some(11), &[K::Payload])),
        ("", Invalid(Some(11), &[K::Payload])),
    ] {
        let line = if value.is_empty() {
            "1 NCHI".to_string()
        } else {
            format!("1 NCHI {value}")
        };
        c.add(
            format!("5.5.1 NCHI {value:?}"),
            format!("{h}0 @I1@ INDI\n{line}\n0 TRLR\n"),
            expect,
        );
    }
    c.add(
        "5.5.1 BIRT N",
        format!("{h}0 @I1@ INDI\n1 BIRT N\n0 TRLR\n"),
        Invalid(Some(11), &[K::Payload]),
    );

    c.add(
        "5.5.1 leading white space",
        "0 HEAD\n1 GEDC\n 2 VERS 5.5.1\n 2 FORM LINEAGE-LINKED\n1 SOUR Test\n1 CHAR ASCII\n1 SUBM @S1@\n0 @S1@ SUBM\n1 NAME Test\n0 TRLR\n",
        Valid,
    );
    c.add(
        "5.5.1 two spaces before the tag",
        "0 HEAD\n1 GEDC\n2  VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 SOUR Test\n1 CHAR ASCII\n1 SUBM @S1@\n0 @S1@ SUBM\n1 NAME Test\n0 TRLR\n",
        Invalid(Some(3), &[K::LineSyntax]),
    );
    spacing_cases(c, "5.5.1");
    name_cases(c, v);
    exact_dates(c, v);
    times(c, v);

    // CHILD_LINKAGE_STATUS (case-insensitive).
    for (value, expect) in [
        ("challenged", Valid),
        ("disproven", Valid),
        ("proven", Valid),
        ("CHALLENGED", Valid),
        ("Proven", Valid),
        ("invalid", Invalid(Some(12), &[K::EnumValue])),
    ] {
        c.add(
            format!("5.5.1 FAMC.STAT {value}"),
            format!("{h}0 @I1@ INDI\n1 FAMC @F1@\n2 STAT {value}\n0 @F1@ FAM\n0 TRLR\n"),
            expect,
        );
    }
    let ordinance = |c: &mut Cases, rec: &str, ord: &str, value: &str, expect: Expect| {
        let x = if rec == "INDI" { "@I1@" } else { "@F1@" };
        c.add(
            format!("5.5.1 {ord}.STAT {value}"),
            format!("{h}0 {x} {rec}\n1 {ord}\n2 STAT {value}\n3 DATE 1 JAN 2000\n0 TRLR\n"),
            expect,
        );
    };
    for value in [
        "CHILD",
        "COMPLETED",
        "EXCLUDED",
        "PRE-1970",
        "STILLBORN",
        "SUBMITTED",
        "UNCLEARED",
        "completed",
        "Excluded",
    ] {
        ordinance(c, "INDI", "BAPL", value, Valid);
    }
    for value in ["INVALID", "INFANT"] {
        ordinance(c, "INDI", "BAPL", value, Invalid(Some(12), &[K::EnumValue]));
    }
    for value in [
        "CHILD",
        "COMPLETED",
        "EXCLUDED",
        "INFANT",
        "PRE-1970",
        "STILLBORN",
        "SUBMITTED",
        "UNCLEARED",
    ] {
        ordinance(c, "INDI", "ENDL", value, Valid);
    }
    ordinance(
        c,
        "INDI",
        "ENDL",
        "NOTVALID",
        Invalid(Some(12), &[K::EnumValue]),
    );
    // The SLGC cases have no FAMC, which LDS_INDIVIDUAL_ORDINANCE requires.
    let no_famc = || {
        Expect::Departure(
            Box::new(Invalid(Some(11), &[K::MissingRequired])),
            "SLGC requires +1 FAMC @<XREF:FAM>@ {1:1} (p. 36)",
        )
    };
    for value in [
        "BIC",
        "COMPLETED",
        "EXCLUDED",
        "DNS",
        "PRE-1970",
        "STILLBORN",
        "SUBMITTED",
        "UNCLEARED",
        "bic",
        "Completed",
    ] {
        ordinance(c, "INDI", "SLGC", value, no_famc());
    }
    ordinance(
        c,
        "INDI",
        "SLGC",
        "WRONG",
        Invalid(Some(12), &[K::EnumValue]),
    );
    for value in [
        "CANCELED",
        "COMPLETED",
        "DNS",
        "EXCLUDED",
        "DNS/CAN",
        "PRE-1970",
        "SUBMITTED",
        "UNCLEARED",
        "canceled",
        "Dns/Can",
    ] {
        ordinance(c, "FAM", "SLGS", value, Valid);
    }
    ordinance(
        c,
        "FAM",
        "SLGS",
        "BADVALUE",
        Invalid(Some(12), &[K::EnumValue]),
    );
}

/// `ValidateValidDateValuePayload` / `Invalid…` with `GedcomVersion.V70`
/// specific values (the 7.1 header differs only by its version).
fn schema_7(c: &mut Cases, v: V) {
    let vs = vers(v);
    header_and_trailer_cases(c, v);
    xref_cases(c, v);
    let h = head(v);
    c.add(
        format!("{vs} missing GEDC"),
        "0 HEAD\n0 TRLR\n",
        Invalid(Some(1), &[K::MissingRequired]),
    );
    c.add(
        format!("{vs} two VERS"),
        format!("0 HEAD\n1 GEDC\n2 VERS {vs}\n2 VERS {vs}\n0 TRLR\n"),
        Invalid(Some(2), &[K::Cardinality]),
    );
    c.add(
        format!("{vs} two SCHMA"),
        format!("{h}1 SCHMA\n1 SCHMA\n0 TRLR\n"),
        Invalid(Some(1), &[K::Cardinality]),
    );
    c.add(
        format!("{vs} OBJE without FILE"),
        format!("{h}0 @O1@ OBJE\n0 TRLR\n"),
        Invalid(Some(4), &[K::MissingRequired]),
    );
    c.add(
        format!("{vs} COPR record"),
        format!("{h}0 @C0@ COPR\n0 TRLR\n"),
        Invalid(Some(4), &[K::Misplaced, K::UnknownTag]),
    );
    c.add(
        format!("{vs} HEAD.PHON"),
        format!("{h}1 PHON\n0 TRLR\n"),
        Invalid(Some(4), &[K::Misplaced]),
    );
    c.add(
        format!("{vs} CONT under HEAD"),
        format!("{h}1 CONT bad\n0 TRLR\n"),
        Invalid(Some(4), &[K::Continuation, K::Misplaced]),
    );
    c.add(
        format!("{vs} leading white space"),
        format!("0 HEAD\n1 GEDC\n 2 VERS {vs}\n0 TRLR\n"),
        Invalid(Some(3), &[K::LineSyntax]),
    );
    c.add(
        format!("{vs} leading white space twice"),
        format!("0 HEAD\n 1 GEDC\n 2 VERS {vs}\n0 TRLR\n"),
        Invalid(Some(2), &[K::LineSyntax]),
    );
    spacing_cases(c, vs);
    c.add(
        format!("{vs} @VOID@ as an identifier"),
        format!("{h}0 @VOID@ INDI\n0 TRLR\n"),
        Invalid(Some(4), &[K::Xref]),
    );
    c.add(
        format!("{vs} # in an identifier"),
        format!("{h}0 @I#1@ INDI\n0 TRLR\n"),
        Invalid(Some(4), &[K::Xref]),
    );
    c.add(
        format!("{vs} _ in an identifier"),
        format!("{h}0 @I_1@ INDI\n0 TRLR\n"),
        Valid,
    );
    c.add(
        format!("{vs} lower case identifier"),
        format!("{h}0 @i1@ INDI\n0 TRLR\n"),
        Invalid(Some(4), &[K::Xref]),
    );
    c.add(
        format!("{vs} duplicate identifier"),
        format!("{h}0 @I1@ INDI\n0 @I1@ INDI\n0 TRLR\n"),
        Invalid(Some(5), &[K::Xref]),
    );
    c.add(
        format!("{vs} standard tag under an extension"),
        format!("{h}1 _UNKNOWN\n2 UNKNOWN\n0 TRLR\n"),
        Valid,
    );
    c.add(
        format!("{vs} extension record"),
        format!("{h}0 @U1@ _UNKNOWN\n1 SOUR @S1@\n0 @S1@ SOUR\n1 TITL Title\n0 TRLR\n"),
        Valid,
    );
    let file = |value: &str| {
        format!("{h}0 @O1@ OBJE\n1 FILE {value}\n2 FORM application/x-other\n0 TRLR\n")
    };
    for value in [
        "media/filename",
        "http://www.contoso.com/path/filename",
        "file://host.example.com/path/to/file",
        "file:///path/to/file",
    ] {
        c.add(format!("{vs} FILE {value:?}"), file(value), Valid);
    }
    for value in [
        "http://www.contoso.com/path???/file name",
        "c:\\\\directory\\filename",
        "file://c:/directory/filename",
        "http:\\\\\\host/path/file",
        "2013.05.29_14:33:41",
    ] {
        c.add(
            format!("{vs} FILE {value:?}"),
            file(value),
            Invalid(Some(5), &[K::Payload]),
        );
    }
    let indi = |line: &str| format!("{h}0 @I1@ INDI\n{line}\n0 TRLR\n");
    c.add(format!("{vs} SEX U"), indi("1 SEX U"), Valid);
    c.add(
        format!("{vs} SEX UNKNOWN"),
        indi("1 SEX UNKNOWN"),
        Invalid(Some(5), &[K::EnumValue]),
    );
    c.add(format!("{vs} NO CENS"), indi("1 NO CENS"), Valid);
    c.add(format!("{vs} NO ADOP"), indi("1 NO ADOP"), Valid);
    c.add(
        format!("{vs} NO FAM"),
        indi("1 NO FAM"),
        Invalid(Some(5), &[K::EnumValue]),
    );
    c.add(format!("{vs} RESN"), indi("1 RESN CONFIDENTIAL"), Valid);
    c.add(
        format!("{vs} RESN list"),
        indi("1 RESN CONFIDENTIAL, LOCKED"),
        Valid,
    );
    c.add(
        format!("{vs} RESN UNKNOWN"),
        indi("1 RESN UNKNOWN"),
        Invalid(Some(5), &[K::EnumValue]),
    );
    c.add(
        format!("{vs} RESN trailing comma"),
        indi("1 RESN CONFIDENTIAL,"),
        Invalid(Some(5), &[K::EnumValue]),
    );
    name_cases(c, v);
    exact_dates(c, v);
    common_date_periods(c, v);
    for value in [
        "TO GREGORIAN 20 BCE",
        "FROM HEBREW 1 TSH 1",
        "FROM GREGORIAN 20 BCE TO GREGORIAN 12 BCE",
    ] {
        date_period(c, v, value, true);
    }
    date_period(c, v, "FROM HEBREW 1 TSH 1 BCE", false);
    common_date_values(c, v);
    for value in [
        "GREGORIAN 20 BCE",
        "HEBREW 1 TSH 1",
        "TO GREGORIAN 20 BCE",
        "FROM HEBREW 1 TSH 1",
        "FROM GREGORIAN 20 BCE TO GREGORIAN 12 BCE",
        "BEF GREGORIAN 20 BCE",
        "AFT HEBREW 1 TSH 1",
        "BET GREGORIAN 20 BCE AND GREGORIAN 12 BCE",
        "EST GREGORIAN 20 BCE",
    ] {
        date_value(c, v, value, true);
    }
    for value in ["FROM HEBREW 1 TSH 1 BCE", "AFT HEBREW 1 TSH 1 BCE"] {
        date_value(c, v, value, false);
    }
    times(c, v);
    let age = |value: &str| format!("{h}0 @I1@ INDI\n1 DEAT\n2 AGE {value}\n0 TRLR\n");
    for value in [
        "79y",
        "79y 1d",
        "79y 1w",
        "79y 1w 1d",
        "79y 1m",
        "79y 1m 1d",
        "79y 1m 1w",
        "79y 1m 1w 1d",
        "79m",
        "1m 1d",
        "1m 1w",
        "1m 1w 1d",
        "79w",
        "79w 1d",
        "79d",
        "> 79y",
        "< 79y 1m 1w 1d",
    ] {
        c.add(format!("{vs} AGE {value:?}"), age(value), Valid);
    }
    for value in [
        " ",
        "invalid",
        "d",
        "79",
        "1d 1m",
        "<>1y",
        ">79y",
        "<79y 1m 1w 1d",
    ] {
        c.add(
            format!("{vs} AGE {value:?}"),
            age(value),
            Invalid(Some(6), &[K::Payload]),
        );
    }
    let lang = |value: &str| format!("{h}1 LANG {value}\n0 TRLR\n");
    for value in ["und", "mul", "en", "en-US", "und-Latn-pinyin"] {
        c.add(format!("{vs} LANG {value:?}"), lang(value), Valid);
    }
    for value in [" ", "-", "und-", "-und", "en US"] {
        c.add(
            format!("{vs} LANG {value:?}"),
            lang(value),
            Invalid(Some(4), &[K::Payload]),
        );
    }
    c.add(
        format!("{vs} empty FORM"),
        format!("{h}0 @O1@ OBJE\n1 FILE foo\n2 FORM\n0 TRLR\n"),
        Invalid(Some(6), &[K::Payload]),
    );
    c.add(
        format!("{vs} FORM"),
        format!("{h}0 @O1@ OBJE\n1 FILE foo\n2 FORM application/x-other\n0 TRLR\n"),
        Valid,
    );
    for value in ["invalid media type", "text/", "/text", "text/a/b", "text"] {
        c.add(
            format!("{vs} FORM {value:?}"),
            format!("{h}0 @O1@ OBJE\n1 FILE foo\n2 FORM {value}\n0 TRLR\n"),
            Invalid(Some(6), &[K::Payload]),
        );
    }
    c.add(
        format!("{vs} SNOTE MIME text"),
        format!("{h}0 @N1@ SNOTE Test\n1 MIME text/unknown\n0 TRLR\n"),
        Valid,
    );
    c.add(
        format!("{vs} SNOTE MIME image"),
        format!("{h}0 @N1@ SNOTE Test\n1 MIME image/unknown\n0 TRLR\n"),
        Invalid(Some(5), &[K::Payload]),
    );
    xref_payloads(c, vs);
}

fn all_cases() -> Cases {
    let mut c = Cases(Vec::new());
    // SchemaTestsCommon.
    c.add("missing HEAD", "0 TRLR\n", Invalid(Some(1), &[K::Header]));
    c.add("no records", "", Invalid(None, &[K::Header]));
    schema_551(&mut c);
    schema_7(&mut c, V::V70);
    schema_7(&mut c, V::V71);
    c
}

/// Whether `found` meets `expect`.
fn meets(found: &[Deviation], expect: &Expect) -> bool {
    match expect {
        Valid => found.is_empty(),
        Invalid(line, kinds) => found
            .iter()
            .any(|d| kinds.contains(&d.kind) && line.is_none_or(|l| d.line == l)),
        Expect::Departure(e, _) => meets(found, e),
    }
}

#[test]
fn armidale_schema_cases() {
    let cases = all_cases();
    let mut wrong = Vec::new();
    for (name, text, expect) in &cases.0 {
        let found = validate_text(text);
        if !meets(&found, expect) {
            let found: Vec<String> = found
                .iter()
                .map(|d| format!("{d} ({:?})", d.kind))
                .collect();
            wrong.push(format!("{name}: expected {expect:?}, found {found:?}"));
        }
    }
    assert!(cases.0.len() > 300, "{} cases", cases.0.len());
    // Every departure from Armidale cites the specification.
    for (name, _, expect) in &cases.0 {
        if let Expect::Departure(_, reason) = expect {
            assert!(reason.contains('('), "{name}: {reason}");
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} cases:\n{}",
        wrong.len(),
        cases.0.len(),
        wrong.join("\n")
    );
}

/// Whether `expect` wants no deviation.
fn wants_valid(expect: &Expect) -> bool {
    match expect {
        Valid => true,
        Invalid(..) => false,
        Expect::Departure(e, _) => wants_valid(e),
    }
}

/// Strict mode is the lenient read after the validator: every case reads
/// leniently, with its records; a valid case reads in strict mode too, an
/// invalid one is refused with its deviations.
#[test]
fn armidale_cases_in_strict_mode() {
    let lenient = oxidged::GedcomBuilder::new();
    let strict = oxidged::GedcomBuilder::new().strict(true);
    let mut wrong = Vec::new();
    for (name, text, expect) in &all_cases().0 {
        let read = lenient.build_from_str(text.as_str());
        if read.is_err() {
            wrong.push(format!("{name}: refused leniently: {read:?}"));
        }
        match (strict.build_from_str(text.as_str()), wants_valid(expect)) {
            (Ok(_), true) => {}
            (Err(oxidged::GedcomError::NonConformant(found)), false) if meets(&found, expect) => {}
            (result, _) => wrong.push(format!(
                "{name}: expected {expect:?}, strict mode gave {result:?}"
            )),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `ValidateTestFile*`: the gedcom7code test-files the suite vendors, as
/// SchemaTests551 and SchemaTests70 name them (the ones Armidale ignores as
/// known issues included).
#[test]
fn armidale_test_files() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance/test-files");
    let any_at = |reason| Expect::Departure(Box::new(Invalid(None, &[K::Escape])), reason);
    let cases: Vec<(&str, Expect)> = vec![
        (
            "5/atsign.ged",
            any_at(
                "5.5.1 text holds @ only doubled, or in an @#…@ escape (any_char, line_item, p. 12)",
            ),
        ),
        ("5/char_ascii_1.ged", Valid),
        ("5/char_ascii_2.ged", Invalid(Some(7), &[K::EnumValue])),
        ("5/char_utf16be-1.ged", Valid),
        ("5/char_utf16be-2.ged", Valid),
        ("5/char_utf16le-1.ged", Valid),
        ("5/char_utf16le-2.ged", Valid),
        ("5/char_utf8-1.ged", Valid),
        (
            "5/char_utf8-2.ged",
            Expect::Departure(
                Box::new(Invalid(None, &[K::Encoding])),
                "CHAR UNICODE names the 16-bit Unicode encoding (p. 78); these bytes are UTF-8",
            ),
        ),
        ("5/char_utf8-3.ged", Valid),
        (
            "5/date-all.ged",
            Expect::Departure(
                Box::new(Invalid(Some(1), &[K::MissingRequired])),
                "the header has no SOUR or SUBM (p. 23), and 7.0 calendar keywords are no 5.5.1 dates",
            ),
        ),
        ("5/date-dual-valid.ged", Valid),
        ("5/enum-ext.ged", Valid),
        ("5/filename-1.ged", Valid),
        ("5/lang-all.ged", Valid),
        ("5/notes-1.ged", Valid),
        // `FORM other` is a 5.5.1 format (accepted practice: the list is
        // open); the header lacks SOUR and SUBM.
        ("5/obje-1.ged", Invalid(Some(1), &[K::MissingRequired])),
        ("5/obsolete-1.ged", Valid),
        ("5/pedi-1.ged", Valid),
        ("5/rela_1.ged", Valid),
        ("5/sour-1.ged", Valid),
        ("5/tiny-1.ged", Invalid(Some(1), &[K::MissingRequired])),
        ("5/xref-case.ged", Invalid(Some(3), &[K::DanglingPointer])),
        ("7/atsign.ged", Valid),
        ("7/char_ascii_1.ged", Valid),
        ("7/char_ascii_2.ged", Valid),
        ("7/char_utf16be-1.ged", Valid),
        ("7/char_utf16be-2.ged", Valid),
        ("7/char_utf16le-1.ged", Valid),
        ("7/char_utf16le-2.ged", Valid),
        ("7/char_utf8-1.ged", Valid),
        ("7/char_utf8-2.ged", Valid),
        ("7/char_utf8-3.ged", Valid),
        (
            "7/date-all.ged",
            Expect::Departure(
                Box::new(Invalid(Some(266), &[K::Payload])),
                "calendars restrict a day to its month's maximum (2.4); 31 NOV is no date",
            ),
        ),
        ("7/enum-ext.ged", Valid),
        ("7/filename-1.ged", Valid),
        ("7/lang-all.ged", Valid),
        (
            "7/notes-1.ged",
            Expect::Departure(
                Box::new(Invalid(Some(22), &[K::Cycle])),
                "a shared note citing the source that points to it is a cycle (3.2, SOURCE_RECORD; 7.0.17)",
            ),
        ),
        ("7/obje-1.ged", Valid),
        ("7/obsolete-1.ged", Valid),
        ("7/pedi-1.ged", Valid),
        ("7/rela_1.ged", Valid),
        ("7/sour-1.ged", Valid),
        ("7/tiny-1.ged", Valid),
        ("7/xref-case.ged", Valid),
    ];
    let mut wrong = Vec::new();
    for (name, expect) in &cases {
        if let Expect::Departure(_, reason) = expect {
            assert!(reason.contains('('), "{name}: {reason}");
        }
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let found = validate_bytes(&bytes);
        if !meets(&found, expect) {
            let found: Vec<String> = found
                .iter()
                .take(8)
                .map(|d| format!("{d} ({:?})", d.kind))
                .collect();
            wrong.push(format!("{name}: expected {expect:?}, found {found:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `ValidateFile*` of SchemaTests70: FamilySearch's 7.0 test files, fetched
/// by `tools/fetch-corpora.sh` (no licence is stated, so they are not
/// vendored).
#[test]
#[ignore = "opt-in: run tools/fetch-corpora.sh, then cargo test --test spec_armidale -- --ignored"]
fn armidale_familysearch_files() {
    let root = std::env::var_os("CORPORA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/corpora"))
        .join("familysearch/testfiles/gedcom70");
    let mut wrong = Vec::new();
    for name in [
        "escapes.ged",
        "extension-record.ged",
        "long-url.ged",
        "maximal70-lds.ged",
        "maximal70-memories1.ged",
        "maximal70-memories2.ged",
        "maximal70-tree1.ged",
        "maximal70-tree2.ged",
        "maximal70.ged",
        "minimal70.ged",
        "remarriage1.ged",
        "remarriage2.ged",
        "same-sex-marriage.ged",
        "voidptr.ged",
    ] {
        let path = root.join(name);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut found = validate_bytes(&bytes);
        // FamilySearch's maximal file keeps a SOUR-SNOTE cycle, which 7.0
        // prohibits (3.2, SOURCE_RECORD; 7.0.17): @N1@ cites @S1@, whose
        // four pointers to @N1@ each close it (its SOUR-OBJE loops were
        // taken out, GEDCOM.io #368 and #395).
        if name == "maximal70.ged" {
            let cycles = found.iter().filter(|d| d.kind == K::Cycle).count();
            assert_eq!(cycles, 4, "{name}: {found:?}");
            found.retain(|d| d.kind != K::Cycle);
        }
        if !found.is_empty() {
            let found: Vec<String> = found.iter().map(ToString::to_string).collect();
            wrong.push(format!("{name}: {found:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
