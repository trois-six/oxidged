//! The date, age and time grammars of GEDCOM 5.5.1 and 7.0 (`oxidged::value`)
//! and the conversions of the model's `Date` and `Age` between versions.
//!
//! - The test-suite plan's cases (`G7-AGE-*`, `G7-DATE-*`, `G7-TIME`,
//!   `G5-DATES`, `G5-AGES`, `G5-AMBIG-NUMERIC-DATE`), read and written.
//! - Calendars: one date per calendar and month, per epoch, and per date
//!   form, valid in both versions, converted from one to the other, and
//!   re-emitted identically by the writer.
//! - Payload types: the valid samples of each payload type are accepted by
//!   the strict grammar and re-emitted identically; the invalid ones are
//!   rejected by it, read leniently and kept.
//! - Free-text ages and date phrases, written in each version's syntax;
//!   words a date grammar does not recognise are kept.
//! - Bounded-input properties: no parser or conversion panics or runs away,
//!   the strict parses agree with the lenient ones, formatting reads back,
//!   and a conversion conforms to its target or leaves the value as written.
//!
//! The samples come from the ArmidaleSoftware/gedcom7 validator's tests (MIT)
//! and the gedcom7code/test-files `age-*` and `date-*` files (Unlicense).

use oxidged::model::{Age, Date, Text};
use oxidged::value::{
    AgeValue, Approximation, Calendar, CalendarDate, DateExact, DatePeriod, DateValue, Month, Time,
};
use oxidged::{Dataset, GedcomBuilder, GedcomVersion, GedcomWriter};

const V551: GedcomVersion = GedcomVersion::V5_5_1;
const V7: GedcomVersion = GedcomVersion::V7_0;

fn read(text: &str) -> Dataset {
    GedcomBuilder::new()
        .build_from_str(text)
        .unwrap_or_else(|e| panic!("{e}\n{text}"))
}

/// The data written in `version`, read back.
fn rewrite(data: &Dataset, version: GedcomVersion) -> Dataset {
    let written = GedcomWriter::new()
        .gedcom_version(version)
        .write_to_string(data)
        .unwrap();
    read(&written)
}

fn write(data: &Dataset, version: GedcomVersion) -> String {
    GedcomWriter::new()
        .gedcom_version(version)
        .write_to_string(data)
        .unwrap()
}

/// An optional text as a string, `None` when absent or empty.
fn opt(text: Option<&Text>, data: &Dataset) -> Option<String> {
    text.map(|t| t.to_str(data).into_owned())
        .filter(|t| !t.is_empty())
}

/// A date of owned texts.
fn date(value: &str, time: Option<&str>, phrase: Option<&str>) -> Date {
    let mut date = Date::new(value);
    if time.is_some() || phrase.is_some() {
        date.detail_mut().time = time.map(Text::new);
        date.detail_mut().phrase = phrase.map(Text::new);
    }
    date
}

/// A date's value, time and phrase.
fn date_texts(date: &Date, data: &Dataset) -> (String, Option<String>, Option<String>) {
    let detail = date.detail();
    (
        date.value.to_str(data).into_owned(),
        opt(detail.time.as_ref(), data),
        opt(detail.phrase.as_ref(), data),
    )
}

/// An age's value and phrase.
fn age_texts(age: &Age, data: &Dataset) -> (String, Option<String>) {
    (
        age.value.to_str(data).into_owned(),
        opt(age.phrase.as_ref(), data),
    )
}

// The test-suite plan's cases

struct Case {
    id: &'static str,
    file: &'static str,
    want: &'static [&'static str],
    has: &'static [&'static str],
}

/// Each file is read, then written in its own version, whose output must
/// hold the `want` lines; the `has` texts must be among the dates and ages
/// read. `G7-AGE-OVERFLOW` and `G7-AGE-INVALID` are regressions (TS14).
const CASES: &[Case] = &[
    Case {
        id: "G7-AGE-ALL",
        file: "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 DEAT\n2 AGE > 79y 1m 1w 1d\n1 BURI\n2 AGE < 1m\n1 CHR\n2 AGE 0y\n3 PHRASE Stillborn\n0 TRLR\n",
        want: &["2 AGE > 79y 1m 1w 1d", "2 AGE < 1m", "3 PHRASE Stillborn"],
        has: &[],
    },
    Case {
        id: "G7-AGE-OVERFLOW",
        file: "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 CHR\n2 AGE 1y 30m 100w 400d\n0 TRLR\n",
        want: &["2 AGE 1y 30m 100w 400d"],
        has: &[],
    },
    Case {
        id: "G7-AGE-INVALID",
        file: "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 DEAT\n2 AGE 79\n0 TRLR\n",
        want: &[],
        has: &["79"],
    },
    Case {
        id: "G7-DATE-FORMS",
        file: "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 BIRT\n2 DATE BET GREGORIAN 20 BCE AND GREGORIAN 12 BCE\n1 DEAT\n2 DATE FROM HEBREW 1 TSH 1\n1 BURI\n2 DATE EST JULIAN 16 AUG 918\n1 CHR\n2 DATE ABT 3 DEC 2023\n1 CREM\n2 DATE FRENCH_R 1 VEND 1\n0 TRLR\n",
        want: &[
            "2 DATE BET GREGORIAN 20 BCE AND GREGORIAN 12 BCE",
            "2 DATE FROM HEBREW 1 TSH 1",
            "2 DATE EST JULIAN 16 AUG 918",
            "2 DATE FRENCH_R 1 VEND 1",
        ],
        has: &[],
    },
    Case {
        id: "G7-DATE-INVALID",
        file: "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 BIRT\n2 DATE 32 JAN 2000\n1 DEAT\n2 DATE BET 2000\n0 TRLR\n",
        want: &[],
        has: &["32 JAN 2000", "BET 2000"],
    },
    Case {
        id: "G7-DATE-EXT-CAL",
        file: "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 BIRT\n2 DATE _MAYAN 1 POP 1\n0 TRLR\n",
        want: &["2 DATE _MAYAN 1 POP 1"],
        has: &[],
    },
    Case {
        id: "G7-DATE-PHRASE-ONLY",
        file: "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 BIRT\n2 DATE\n3 PHRASE Easter\n0 TRLR\n",
        want: &["3 PHRASE Easter"],
        has: &["Easter"],
    },
    Case {
        id: "G7-TIME",
        file: "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 CHAN\n2 DATE 1 DEC 2023\n3 TIME 2:50:00.00Z\n0 TRLR\n",
        want: &["3 TIME 2:50:00.00Z"],
        has: &[],
    },
    Case {
        id: "G5-AMBIG-NUMERIC-DATE",
        file: "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n0 @I1@ INDI\n1 BIRT\n2 DATE 7/11/1959\n1 DEAT\n2 DATE 25/3/1934\n0 TRLR\n",
        // Not dates of either grammar: written as 5.5.1 date phrases.
        want: &["2 DATE (7/11/1959)", "2 DATE (25/3/1934)"],
        has: &["7/11/1959", "25/3/1934"],
    },
    Case {
        id: "G5-DATES",
        file: "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n0 @I1@ INDI\n1 BIRT\n2 DATE 1740/41\n1 CHR\n2 DATE @#DGREGORIAN@ 20 B.C.\n1 DEAT\n2 DATE @#DHEBREW@ 1 TSH 5760\n1 BURI\n2 DATE @#DFRENCH R@ 1 VEND 1\n1 CREM\n2 DATE INT 1900 (about nineteen hundred)\n1 PROB\n2 DATE (Easter)\n1 WILL\n2 DATE @#DJULIAN@ 1 JAN 1700\n0 TRLR\n",
        want: &[
            "2 DATE 1740/41",
            "2 DATE @#DGREGORIAN@ 20 B.C.",
            "2 DATE @#DHEBREW@ 1 TSH 5760",
            "2 DATE @#DFRENCH R@ 1 VEND 1",
            "2 DATE INT 1900 (about nineteen hundred)",
            "2 DATE (Easter)",
            "2 DATE @#DJULIAN@ 1 JAN 1700",
        ],
        has: &[],
    },
    Case {
        id: "G5-AGES",
        file: "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR UTF-8\n0 @I1@ INDI\n1 DEAT\n2 AGE STILLBORN\n1 BURI\n2 AGE <1y\n1 CREM\n2 AGE >79y\n0 TRLR\n",
        want: &["2 AGE STILLBORN"],
        has: &["<1y", ">79y"],
    },
];

#[test]
fn date_age_and_time_cases() {
    for case in CASES {
        let data = read(case.file);
        let written = GedcomWriter::new().write_to_string(&data).unwrap();
        for want in case.want {
            assert!(
                written.lines().any(|line| line == *want),
                "{}: missing {want:?} in:\n{written}",
                case.id
            );
        }
        // The texts of the dates and ages read.
        let mut texts = Vec::new();
        for event in &data.individuals().get(0).unwrap().into_owned().events {
            if let Some(date) = &event.date {
                let (value, time, phrase) = date_texts(date, &data);
                texts.extend([Some(value), time, phrase].into_iter().flatten());
            }
            if let Some(age) = &event.detail().age {
                let (value, phrase) = age_texts(age, &data);
                texts.extend([Some(value), phrase].into_iter().flatten());
            }
        }
        for has in case.has {
            assert!(
                texts.iter().any(|t| t == has),
                "{}: lacks {has:?} in {texts:?}",
                case.id
            );
        }
    }
}

#[test]
fn g5_dates_as_gedcom_7() {
    let data = read(CASES[9].file);
    let written = write(&data, V7);
    for want in [
        "2 DATE BET 1740 AND 1741\n3 PHRASE 1740/41",
        "2 DATE 20 BCE",
        "2 DATE HEBREW 1 TSH 5760",
        "2 DATE FRENCH_R 1 VEND 1",
        "2 DATE 1900\n3 PHRASE about nineteen hundred",
        "2 DATE\n3 PHRASE Easter",
        "2 DATE JULIAN 1 JAN 1700",
    ] {
        assert!(written.contains(want), "missing {want:?} in:\n{written}");
    }
    assert!(!written.contains("@#D"), "{written}");
}

// Calendars, months, epochs and date forms

/// A calendar: its 7.0 keyword, its 5.5.1 escape and its months.
struct CalendarCase {
    keyword: &'static str,
    escape: &'static str,
    months: &'static [&'static str],
    bce: bool,
}

const CALENDARS: [CalendarCase; 4] = [
    CalendarCase {
        keyword: "GREGORIAN",
        escape: "@#DGREGORIAN@",
        months: &[
            "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
        ],
        bce: true,
    },
    CalendarCase {
        keyword: "JULIAN",
        escape: "@#DJULIAN@",
        months: &[
            "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
        ],
        bce: true,
    },
    CalendarCase {
        keyword: "FRENCH_R",
        escape: "@#DFRENCH R@",
        months: &[
            "VEND", "BRUM", "FRIM", "NIVO", "PLUV", "VENT", "GERM", "FLOR", "PRAI", "MESS", "THER",
            "FRUC", "COMP",
        ],
        bce: false,
    },
    CalendarCase {
        keyword: "HEBREW",
        escape: "@#DHEBREW@",
        months: &[
            "TSH", "CSH", "KSL", "TVT", "SHV", "ADR", "ADS", "NSN", "IYR", "SVN", "TMZ", "AAV",
            "ELL",
        ],
        bce: false,
    },
];

/// A date in both versions' spellings.
struct Pair {
    v7: String,
    v551: String,
}

/// One date per calendar and month, per epoch, and per date form.
fn calendar_cases() -> Vec<Pair> {
    let mut cases = Vec::new();
    for calendar in &CALENDARS {
        // The Gregorian calendar is the default: no marker in either version.
        let (kw, esc) = if calendar.keyword == "GREGORIAN" {
            (String::new(), String::new())
        } else {
            (
                format!("{} ", calendar.keyword),
                format!("{} ", calendar.escape),
            )
        };
        for (index, month) in calendar.months.iter().enumerate() {
            // Within the five complementary days, the shortest month.
            let day = index % 5 + 1;
            cases.push(Pair {
                v7: format!("{kw}{day} {month} 1801"),
                v551: format!("{esc}{day} {month} 1801"),
            });
        }
        if calendar.bce {
            cases.push(Pair {
                v7: format!("{kw}20 BCE"),
                v551: format!("{esc}20 B.C."),
            });
        }
        for form in [
            "{d}",
            "ABT {d}",
            "CAL {d}",
            "EST {d}",
            "BEF {d}",
            "AFT {d}",
            "BET {d} AND {e}",
            "FROM {d}",
            "TO {e}",
            "FROM {d} TO {e}",
        ] {
            let month = calendar.months[1];
            let fill = |marker: &str| {
                form.replace("{d}", &format!("{marker}{month} 1801"))
                    .replace("{e}", &format!("{marker}1802"))
            };
            cases.push(Pair {
                v7: fill(&kw),
                v551: fill(&esc),
            });
        }
    }
    cases
}

/// A file of the given version holding one birth per date.
fn file_with_dates(version: &str, dates: &[&str]) -> String {
    let mut file = format!("0 HEAD\n1 GEDC\n2 VERS {version}\n0 @I1@ INDI\n");
    for date in dates {
        file.push_str(&format!("1 BIRT\n2 DATE {date}\n"));
    }
    file.push_str("0 TRLR\n");
    file
}

/// The date values of the events of the first individual.
fn dates_of(data: &Dataset) -> Vec<String> {
    data.individuals()
        .get(0)
        .unwrap()
        .into_owned()
        .events
        .iter()
        .map(|event| event.date.as_ref().unwrap().value.to_str(data).into_owned())
        .collect()
}

#[test]
fn calendars_months_epochs_and_forms() {
    let none = Dataset::default();
    let cases = calendar_cases();
    assert!(cases.len() >= 60, "{}", cases.len());
    for case in &cases {
        let v7 = DateValue::parse_strict(&case.v7, V7)
            .unwrap_or_else(|error| panic!("{}: {error}", case.v7));
        let v551 = DateValue::parse_strict(&case.v551, V551)
            .unwrap_or_else(|error| panic!("{}: {error}", case.v551));
        // One meaning, two spellings.
        assert_eq!(v7, v551, "{} / {}", case.v7, case.v551);
        assert_eq!(v7.to_gedcom(V7), case.v7);
        assert_eq!(v551.to_gedcom(V551), case.v551);
        // Each version's spelling is invalid in the other, but for the
        // Gregorian dates without an epoch, which both spell alike.
        if case.v7 != case.v551 {
            assert!(
                DateValue::parse_strict(&case.v7, V551).is_err(),
                "{}",
                case.v7
            );
            assert!(
                DateValue::parse_strict(&case.v551, V7).is_err(),
                "{}",
                case.v551
            );
        }
        assert_eq!(
            Date::new(case.v7.as_str())
                .to_version(&none, V551)
                .value
                .to_str(&none),
            case.v551
        );
        assert_eq!(
            Date::new(case.v551.as_str())
                .to_version(&none, V7)
                .value
                .to_str(&none),
            case.v7
        );
    }
}

#[test]
fn calendar_dates_are_re_emitted_identically() {
    let cases = calendar_cases();
    let v7: Vec<&str> = cases.iter().map(|case| case.v7.as_str()).collect();
    let v551: Vec<&str> = cases.iter().map(|case| case.v551.as_str()).collect();

    let data7 = read(&file_with_dates("7.0", &v7));
    assert_eq!(dates_of(&data7), v7);
    assert_eq!(dates_of(&rewrite(&data7, V7)), v7);
    assert_eq!(dates_of(&rewrite(&data7, V551)), v551);

    let data551 = read(&file_with_dates("5.5.1", &v551));
    assert_eq!(dates_of(&rewrite(&data551, V551)), v551);
    assert_eq!(dates_of(&rewrite(&data551, V7)), v7);
}

#[cfg(feature = "calendar")]
#[test]
fn every_calendar_month_converts_and_back() {
    use oxidged::value::CalendarError;
    for case in calendar_cases() {
        let DateValue::Date(date) = DateValue::parse(&case.v7) else {
            continue;
        };
        if date.day.is_none() {
            continue;
        }
        let rata_die = date
            .to_rata_die()
            .unwrap_or_else(|error| panic!("{}: {error}", case.v7));
        for calendar in [
            Calendar::Gregorian,
            Calendar::Julian,
            Calendar::Hebrew,
            Calendar::FrenchRepublican,
        ] {
            match date.convert_to(&calendar) {
                Ok(converted) => assert_eq!(converted.to_rata_die(), Ok(rata_die), "{}", case.v7),
                // 1801 is before the French Republican calendar for the
                // other calendars' early days: only that may fail.
                Err(error) => assert_eq!(
                    (calendar, error),
                    (Calendar::FrenchRepublican, CalendarError::OutOfRange),
                    "{}",
                    case.v7
                ),
            }
        }
        assert_eq!(
            CalendarDate::from_rata_die(rata_die, &date.calendar)
                .unwrap()
                .to_rata_die(),
            Ok(rata_die)
        );
    }
}

#[cfg(feature = "calendar")]
#[test]
fn model_dates_convert_between_calendars() {
    let none = Dataset::default();
    let julian = date("ABT @#DJULIAN@ 15 MAR 1582", None, Some("old style"));
    let gregorian = julian
        .convert_to(&none, &Calendar::Gregorian, V551)
        .unwrap();
    assert_eq!(
        date_texts(&gregorian, &none),
        (
            "ABT 25 MAR 1582".to_string(),
            None,
            Some("old style".into())
        )
    );
    let gregorian = julian.convert_to(&none, &Calendar::Gregorian, V7).unwrap();
    assert_eq!(gregorian.value.to_str(&none), "ABT 25 MAR 1582");
    let back = gregorian.convert_to(&none, &Calendar::Julian, V7).unwrap();
    assert_eq!(back.value.to_str(&none), "ABT JULIAN 15 MAR 1582");
    // An incomplete date cannot be converted.
    assert!(
        Date::new("1582")
            .convert_to(&none, &Calendar::Julian, V7)
            .is_err()
    );
}

#[test]
fn months_are_tags() {
    for calendar in [
        Calendar::Gregorian,
        Calendar::Julian,
        Calendar::FrenchRepublican,
        Calendar::Hebrew,
    ] {
        for (index, month) in calendar.months().iter().enumerate() {
            assert_eq!(Month::from_tag(month.tag()).as_ref(), Some(month));
            assert_eq!(usize::from(month.number().unwrap()), index + 1);
            assert_eq!(
                Month::of(&calendar, month.number().unwrap()).as_ref(),
                Some(month)
            );
        }
    }
}

// Payload samples

/// Valid `DateValue` samples in every version (Armidale).
const DATE_VALUES: &[&str] = &[
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
];

/// Invalid `DateValue` samples in every version (Armidale). The validator
/// also rejects lower-case months in 5.5.1; the 5.5.1 specification makes
/// controlled values case-insensitive (p. 21), so they are valid here.
const INVALID_DATE_VALUES: &[&str] = &[
    "TO 40 DEC 2023",
    "TO 3 JUNE 2023",
    "TO ABC 2023",
    "BEF 40 DEC 2023",
    "BEF 3 JUNE 2023",
    "BEF ABC 2023",
    "BET 2000",
];

/// Valid 5.5.1 dates (Armidale; test-files `5/date-dual-valid.ged`).
const DATE_VALUES_551: &[&str] = &[
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
    "1699/00",
    "JAN 1699/00",
    "8 JAN 1699/00",
    "ABT 8 JAN 1699/00",
    "FROM JAN 1699/00 TO FEB 1699/00",
    "BET JAN 1699/00 AND FEB 1699/00",
    "INT 1 JAN 1900 (New Year's day)",
    "(the year of the flood)",
    "@#DROMAN@ 1860",
    "@#DUNKNOWN@ 1871",
    "TO 3 dec 2023",
];

/// Invalid 5.5.1 dates (Armidale; test-files `5/date-dual-invalid.ged`).
const INVALID_DATE_VALUES_551: &[&str] = &[
    "FROM @#DHEBREW@ 1 TSH 1 B.C.",
    "AFT @#DHEBREW@ 1 TSH 1 B.C.",
    "1701/99",
    "JAN 1701/03",
    "1751/9",
    "GREGORIAN 20 BCE",
    "INT 1900",
    "",
];

/// Valid 7.0 dates (Armidale).
const DATE_VALUES_7: &[&str] = &[
    "GREGORIAN 20 BCE",
    "HEBREW 1 TSH 1",
    "TO GREGORIAN 20 BCE",
    "FROM HEBREW 1 TSH 1",
    "FROM GREGORIAN 20 BCE TO GREGORIAN 12 BCE",
    "BEF GREGORIAN 20 BCE",
    "AFT HEBREW 1 TSH 1",
    "BET GREGORIAN 20 BCE AND GREGORIAN 12 BCE",
    "EST GREGORIAN 20 BCE",
    "_MYCAL 12 _MON 1500",
    "_ROMAN 28",
    "",
];

/// Invalid 7.0 dates (Armidale and the 7.0 grammar).
const INVALID_DATE_VALUES_7: &[&str] = &[
    "FROM HEBREW 1 TSH 1 BCE",
    "AFT HEBREW 1 TSH 1 BCE",
    "TO 3 dec 2023",
    "BEF 3 dec 2023",
    "1740/41",
    "20 B.C.",
    "@#DJULIAN@ 1700",
    "INT 1 JAN 1900 (New Year's day)",
    "(the year of the flood)",
    "ROMAN 28",
    "1 JAN",
];

#[test]
fn date_value_samples() {
    for (version, valid, invalid) in [
        (V551, DATE_VALUES_551, INVALID_DATE_VALUES_551),
        (V7, DATE_VALUES_7, INVALID_DATE_VALUES_7),
    ] {
        for value in DATE_VALUES.iter().chain(valid) {
            let parsed = DateValue::parse_strict(value, version)
                .unwrap_or_else(|error| panic!("{version:?}: {error}"));
            // The strict reading is the lenient one.
            assert_eq!(parsed, DateValue::parse(value), "{value}");
        }
        for value in INVALID_DATE_VALUES.iter().chain(invalid) {
            assert!(
                DateValue::parse_strict(value, version).is_err(),
                "{version:?}: {value}"
            );
        }
    }
}

#[test]
fn date_value_samples_through_the_reader_and_writer() {
    for (version, valid, invalid) in [
        (V551, DATE_VALUES_551, INVALID_DATE_VALUES_551),
        (V7, DATE_VALUES_7, INVALID_DATE_VALUES_7),
    ] {
        let samples: Vec<&str> = DATE_VALUES
            .iter()
            .chain(valid)
            .chain(invalid)
            .copied()
            .filter(|value| !value.is_empty())
            .collect();
        let data = read(&file_with_dates(version.as_str(), &samples));
        // Read verbatim, valid or not.
        assert_eq!(dates_of(&data), samples);
        let written = rewrite(&data, version);
        for (index, sample) in samples.iter().enumerate() {
            let event_record = written.individuals().get(0).unwrap().into_owned();
            let event = &event_record.events[index];
            let date = event.date.as_ref().unwrap();
            let (value, _, phrase) = date_texts(date, &written);
            if DateValue::parse_strict(sample, version).is_ok() {
                // Valid: re-emitted identically.
                assert_eq!(value, *sample);
            } else {
                // Invalid: what it says is kept, rewritten in the version's
                // grammar when it can be, verbatim otherwise.
                let kept = value == *sample
                    || value == format!("({sample})")
                    || phrase.as_deref() == Some(*sample)
                    || DateValue::parse(&value) == DateValue::parse(sample)
                    || date.to_version(&written, V551).value.to_str(&written) == *sample;
                assert!(kept, "{version}: {sample} became {value:?} {phrase:?}");
            }
        }
    }
}

#[test]
fn period_and_exact_samples() {
    for version in [V551, V7] {
        for valid in [
            "TO 3 DEC 2023",
            "TO DEC 2023",
            "TO 2023",
            "FROM 03 DEC 2023",
            "FROM 2000 TO 2020",
            "FROM MAR 2000 TO JUN 2000",
            "FROM 30 NOV 2000 TO 1 DEC 2000",
        ] {
            assert!(DatePeriod::parse_strict(valid, version).is_ok(), "{valid}");
        }
        for invalid in [
            "2023",
            "TO 40 DEC 2023",
            "TO 3 JUNE 2023",
            "TO ABC 2023",
            "BEF 2023",
        ] {
            assert!(
                DatePeriod::parse_strict(invalid, version).is_err(),
                "{invalid}"
            );
        }
        for valid in ["3 DEC 2023", "03 DEC 2023"] {
            assert!(DateExact::parse_strict(valid, version).is_ok(), "{valid}");
        }
        for invalid in [
            "invalid",
            "3 JUNE 2023",
            "DEC 2023",
            "2023",
            "ABT 3 DEC 2023",
        ] {
            assert!(
                DateExact::parse_strict(invalid, version).is_err(),
                "{invalid}"
            );
        }
    }
    assert!(DateExact::parse_strict("3 dec 2023", V7).is_err());
    assert!(DatePeriod::parse_strict("FROM HEBREW 1 TSH 1 BCE", V7).is_err());
    assert!(DatePeriod::parse_strict("FROM @#DHEBREW@ 1 TSH 1 B.C.", V551).is_err());
    assert!(DatePeriod::parse_strict("", V7).is_ok());
    assert!(DatePeriod::parse_strict("", V551).is_err());
}

#[test]
fn time_samples() {
    let none = Dataset::default();
    for version in [V551, V7] {
        for valid in ["02:50", "2:50", "12:34:56.789"] {
            assert!(Time::parse_strict(valid, version).is_ok(), "{valid}");
        }
        for invalid in [
            " ", "invalid", "000:00", "24:00:00", "2:5", "2:60", "2:00:60",
        ] {
            assert!(Time::parse_strict(invalid, version).is_err(), "{invalid}");
            // Kept by the date that holds it.
            let converted = date("1 DEC 2023", Some(invalid), None).to_version(&none, version);
            let (_, time, _) = date_texts(&converted, &none);
            let kept = time.as_deref() == Some(invalid)
                || time.as_deref().and_then(Time::parse) == Time::parse(invalid);
            assert!(kept, "{invalid}: {time:?}");
        }
    }
    assert!(Time::parse_strict("2:50:00.00Z", V7).is_ok());
    assert!(Time::parse_strict("2:50:00.00Z", V551).is_err());
}

#[test]
fn date_and_time_together() {
    let none = Dataset::default();
    let with_time = date("2 OCT 2019", Some("12:00"), None);
    assert_eq!(
        with_time.datetime(&none).as_deref(),
        Some("2 OCT 2019 12:00")
    );
    assert_eq!(
        date("", Some("12:00"), None).datetime(&none).as_deref(),
        Some("12:00")
    );
    assert_eq!(Date::new("2 OCT 2019").datetime(&none), None);
    // GEDCOM 5.5.1 has no `Z` for UTC.
    let converted = date("2 OCT 2019", Some("2:50:00Z"), None).to_version(&none, V551);
    let (_, time, _) = date_texts(&converted, &none);
    assert!(
        Time::parse_strict(time.as_deref().unwrap(), V551).is_ok(),
        "{time:?}"
    );
}

/// Valid 7.0 ages (Armidale; test-files `7/age-valid.ged`).
const AGES_7: &[&str] = &[
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
    "< 8y",
    "0y",
    "< 0m",
    "> 0d",
    "99y 11m 30d",
    "1y 400d",
    "8w 30d",
];

/// Invalid 7.0 ages (Armidale; test-files `5/age-*.ged` forms).
const INVALID_AGES_7: &[&str] = &[
    " ",
    "invalid",
    "d",
    "79",
    "1d 1m",
    "<>1y",
    ">79y",
    "<79y 1m 1w 1d",
    "CHILD",
    "0Y",
    "99y11m",
];

/// Valid 5.5.1 ages (test-files `5/age-valid.ged`).
const AGES_551: &[&str] = &[
    "child",
    "CHILD",
    "Child",
    "infant",
    "INFANT",
    "stillborn",
    "STILLBORN",
    "0y",
    "0Y",
    "<0y",
    "<0Y",
    ">0y",
    "0m",
    "0M",
    "<0m",
    ">0M",
    "0d",
    "<0D",
    ">0d",
    "99y",
    ">99y",
    "11m",
    ">11m",
    "99y 11m",
    "30d",
    ">30d",
    "99y 30d",
    "11m 30d",
    "99y 11m 30d",
];

/// Invalid 5.5.1 ages (test-files `5/age-invalid.ged`).
const INVALID_AGES_551: &[&str] = &[
    "0",
    "<8",
    "> 99",
    "< 0y",
    "> 0Y",
    "0 y",
    "<0 y",
    "< 0 Y",
    "< 0m",
    "0 m",
    "< 0d",
    "0 D",
    "< 99y",
    "99y11m",
    ">99y11m",
    "< 99y 11m",
    "11m99y",
    "11m 99y",
    "< 11m 99y",
    "99y30d",
    "30d99y",
    "11m30d",
    "30d 11m",
    "99y11m30d",
    "99y 30d 11m",
    "11m 30d 99y",
    "1w",
    "",
];

#[test]
fn age_samples() {
    for (version, valid, invalid) in [
        (V7, AGES_7, INVALID_AGES_7),
        (V551, AGES_551, INVALID_AGES_551),
    ] {
        for value in valid {
            let parsed = AgeValue::parse_strict(value, version)
                .unwrap_or_else(|error| panic!("{version:?}: {error}"));
            assert_eq!(parsed, AgeValue::parse(value), "{value}");
        }
        for value in invalid {
            assert!(
                AgeValue::parse_strict(value, version).is_err(),
                "{version:?}: {value}"
            );
        }
    }
}

#[test]
fn age_conversion_matches_the_test_files() {
    let none = Dataset::default();
    // test-files 5/age-*.ged converted to 7/age-*.ged.
    for (v551, v7, phrase) in [
        ("child", "< 8y", Some("child")),
        ("Infant", "< 1y", Some("Infant")),
        ("STILLBORN", "0y", Some("STILLBORN")),
        ("0Y", "0y", None),
        ("<0y", "< 0y", None),
        (">0D", "> 0d", None),
        ("0", "0y", None),
        ("<8", "< 8y", None),
        ("> 99", "> 99y", None),
        ("< 0 Y", "< 0y", None),
        ("99y11m", "99y 11m", None),
        (">11m99y", "> 99y 11m", None),
        ("< 30d 11m 99y", "< 99y 11m 30d", None),
        ("30d11m99y", "99y 11m 30d", None),
    ] {
        let converted = Age::new(v551).to_version(&none, V7);
        assert_eq!(
            age_texts(&converted, &none),
            (v7.to_string(), phrase.map(str::to_string)),
            "{v551}"
        );
        // And back: the duration in 5.5.1's form, or the keyword.
        let (back, _) = age_texts(&converted.to_version(&none, V551), &none);
        assert!(AgeValue::parse_strict(&back, V551).is_ok(), "{back}");
        if let Some(phrase) = phrase {
            assert_eq!(back, phrase);
        }
    }
}

/// The age values of the events of the first individual.
fn ages_of(data: &Dataset) -> Vec<Option<String>> {
    data.individuals()
        .get(0)
        .unwrap()
        .into_owned()
        .events
        .iter()
        .map(|event| opt(event.detail().age.as_ref().map(|age| &age.value), data))
        .collect()
}

#[test]
fn ages_are_kept_by_the_model() {
    // TS14: an invalid age and ages whose components overflow a byte are
    // kept as written, and written back in the version's form.
    let file = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 DEAT\n2 AGE 79\n1 BURI\n2 AGE 1y 400d\n\
                1 CREM\n2 AGE 1y 30m 100w 400d\n1 EVEN\n2 AGE about thirty\n0 TRLR\n";
    let data = read(file);
    let some = |ages: &[&str]| -> Vec<Option<String>> {
        ages.iter()
            .map(|a| Some((*a).to_string()).filter(|a| !a.is_empty()))
            .collect()
    };
    assert_eq!(
        ages_of(&data),
        some(&["79", "1y 400d", "1y 30m 100w 400d", "about thirty"])
    );
    let v7 = rewrite(&data, V7);
    assert_eq!(
        ages_of(&v7),
        some(&["79y", "1y 400d", "1y 30m 100w 400d", ""])
    );
    // GEDCOM 7.0 keeps the text that is not an age in the PHRASE.
    let text_record = v7.individuals().get(0).unwrap().into_owned();
    let text = text_record.events[3].detail().age.as_ref().unwrap();
    assert_eq!(
        opt(text.phrase.as_ref(), &v7).as_deref(),
        Some("about thirty")
    );
    // GEDCOM 5.5.1 keeps the text that is not an age as an extension.
    let v551 = rewrite(&data, V551);
    assert_eq!(
        ages_of(&v551),
        some(&["79y", "1y 400d", "1y 30m 1100d", ""])
    );
    assert_eq!(texts_of(&v551, "_AGE"), ["about thirty"]);
}

// Free-text ages and phrases

#[test]
fn free_text_ages_round_trip() {
    // AGE values outside the grammar are common in real files. They must
    // neither fail the whole file nor be read as a number.
    let original = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
                    1 DEAT Y\n2 AGE majeur\n1 BURI\n2 AGE\n2 PLAC Sampleton\n\
                    0 @F1@ FAM\n1 MARR\n2 HUSB\n3 AGE environ 30 ans\n0 TRLR";
    let data = read(original);
    let husband_age = |data: &Dataset| {
        let detail_record = data.families().get(0).unwrap().into_owned();
        let detail = detail_record.events[0].detail();
        let age = detail.husband.as_ref().unwrap().age.as_ref().unwrap();
        age_texts(age, data)
    };
    assert_eq!(ages_of(&data)[0].as_deref(), Some("majeur"));
    assert_eq!(husband_age(&data), ("environ 30 ans".to_string(), None));

    // GEDCOM 5.5.1 cannot write them as ages: they are kept as extensions.
    let written = write(&data, V551);
    assert!(written.contains("1 DEAT Y\n2 _AGE majeur\n"), "{written}");
    assert!(
        written.contains("2 _HUSB\n3 _AGE environ 30 ans\n"),
        "{written}"
    );
    assert!(!written.contains("PHRASE"), "{written}");
    let back = read(&written);
    assert_eq!(texts_of(&back, "_AGE"), ["majeur", "environ 30 ans"]);

    // GEDCOM 7.0 keeps them as the phrases of empty ages.
    let v7 = write(&data, V7);
    assert!(v7.contains("2 AGE\n3 PHRASE majeur\n"), "{v7}");
    assert!(v7.contains("3 AGE\n4 PHRASE environ 30 ans\n"), "{v7}");
    let back = read(&v7);
    let death = back.individuals().get(0).unwrap().into_owned().events[0]
        .detail()
        .age
        .clone()
        .unwrap();
    assert_eq!(
        age_texts(&death, &back),
        (String::new(), Some("majeur".to_string()))
    );
    assert_eq!(
        husband_age(&back),
        (String::new(), Some("environ 30 ans".to_string()))
    );
}

/// The texts of every structure tagged `tag`, in order.
fn texts_of(data: &Dataset, tag: &str) -> Vec<String> {
    fn walk(structure: &oxidged::tree::Structure, tag: &str, out: &mut Vec<String>) {
        if structure.tag.as_str() == tag {
            out.extend(structure.text().map(str::to_string));
        }
        for child in &structure.substructures {
            walk(child, tag, out);
        }
    }
    let mut out = Vec::new();
    for record in data.to_structures() {
        walk(&record, tag, &mut out);
    }
    out
}

#[test]
fn age_phrases_per_version() {
    let original = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n\
                    1 DEAT Y\n2 AGE > 80y\n3 PHRASE over eighty\n\
                    1 BURI\n2 AGE\n3 PHRASE of full age\n\
                    1 CHR\n2 AGE CHILD\n\
                    1 OCCU Miller\n2 AGE 30y\n3 PHRASE about thirty\n0 TRLR";
    let data = read(original);

    // GEDCOM 5.5.1 has no PHRASE: the phrases are kept as extensions. A
    // bound is written against the number; a duration whose phrase names
    // its keyword is the keyword.
    let v551 = write(&data, V551);
    for expected in [
        "2 AGE >80y\n3 _PHRASE over eighty\n",
        "2 AGE\n3 _PHRASE of full age\n",
        "2 AGE CHILD\n",
        "2 AGE 30y\n3 _PHRASE about thirty\n",
    ] {
        assert!(v551.contains(expected), "missing {expected:?} in:\n{v551}");
    }
    assert!(!has_tag(&v551, "PHRASE"), "{v551}");

    // GEDCOM 7.0 has no CHILD keyword: it becomes its duration, with the
    // keyword as written as the phrase. Every phrase is kept, the
    // attribute's included.
    let v7 = write(&data, V7);
    for expected in [
        "2 AGE > 80y\n3 PHRASE over eighty\n",
        "2 AGE\n3 PHRASE of full age\n",
        "2 AGE < 8y\n3 PHRASE CHILD\n",
        "2 AGE 30y\n3 PHRASE about thirty\n",
    ] {
        assert!(v7.contains(expected), "missing {expected:?} in:\n{v7}");
    }

    let back = read(&v7);
    for index in [1, 3] {
        let age = |data: &Dataset| {
            let age_record = data.individuals().get(0).unwrap().into_owned();
            let age = age_record.events[index].detail().age.clone();
            age_texts(&age.unwrap(), data)
        };
        assert_eq!(age(&back), age(&data), "event {index}");
    }
}

/// Whether a line of `written` has `tag`.
fn has_tag(written: &str, tag: &str) -> bool {
    written
        .lines()
        .any(|line| line.split(' ').nth(1) == Some(tag))
}

/// The first individual record, as an owned structure.
fn individual_structure(data: &Dataset) -> oxidged::tree::Structure {
    data.to_structures()
        .into_iter()
        .find(|s| s.tag.as_str() == "INDI")
        .unwrap()
}

#[test]
fn date_phrases_per_version() {
    let v7_source = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n\
                     1 BIRT\n2 DATE 15 MAR 1820\n3 PHRASE The Ides of March\n\
                     1 BAPM\n2 DATE\n3 PHRASE the year of the flood\n\
                     1 RESI\n2 DATE BET 1820 AND 1825\n3 PHRASE in his youth\n\
                     1 DEAT\n2 DATE 1900\n2 SDATE 1900\n3 PHRASE sorted as 1900\n0 TRLR";
    let data = read(v7_source);

    // GEDCOM 5.5.1 has no PHRASE: the phrase moves into the 5.5.1 date
    // phrase forms, or is kept as an extension next to a range. 5.5.1 has
    // no SDATE either.
    let v551 = write(&data, V551);
    for expected in [
        "2 VERS 5.5.1\n",
        "2 DATE INT 15 MAR 1820 (The Ides of March)\n",
        "2 DATE (the year of the flood)\n",
        "2 DATE BET 1820 AND 1825\n3 _PHRASE in his youth\n",
        "2 DATE 1900\n2 _SDATE INT 1900 (sorted as 1900)\n",
    ] {
        assert!(v551.contains(expected), "missing {expected:?} in:\n{v551}");
    }
    assert!(!has_tag(&v551, "PHRASE"), "{v551}");

    // Written without forcing a version, 7.0 data stays 7.0.
    let v7 = GedcomWriter::new().write_to_string(&data).unwrap();
    for expected in [
        "2 VERS 7.0\n",
        "2 DATE 15 MAR 1820\n3 PHRASE The Ides of March\n",
        "2 DATE\n3 PHRASE the year of the flood\n",
        "2 DATE BET 1820 AND 1825\n3 PHRASE in his youth\n",
        "2 SDATE 1900\n3 PHRASE sorted as 1900\n",
    ] {
        assert!(v7.contains(expected), "missing {expected:?} in:\n{v7}");
    }
    assert_eq!(
        individual_structure(&read(&v7)),
        individual_structure(&data)
    );
}

#[test]
fn gedcom_5_date_phrases_as_gedcom_7() {
    let v551_source = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n\
                       1 BIRT\n2 DATE (the year of the flood)\n\
                       1 DEAT\n2 DATE INT 15 MAR 1820 (The Ides of March)\n0 TRLR";
    let data = read(v551_source);

    // Unchanged in 5.5.1.
    let v551 = GedcomWriter::new().write_to_string(&data).unwrap();
    assert!(v551.contains("2 DATE (the year of the flood)\n"), "{v551}");
    assert!(
        v551.contains("2 DATE INT 15 MAR 1820 (The Ides of March)\n"),
        "{v551}"
    );

    // GEDCOM 7.0 has neither form: the text becomes a PHRASE.
    let v7 = write(&data, V7);
    for expected in [
        "2 VERS 7.0\n",
        "2 DATE\n3 PHRASE the year of the flood\n",
        "2 DATE 15 MAR 1820\n3 PHRASE The Ides of March\n",
    ] {
        assert!(v7.contains(expected), "missing {expected:?} in:\n{v7}");
    }

    // The model's conversion is the writer's.
    let none = Dataset::default();
    let converted = Date::new("INT 15 MAR 1820 (The Ides of March)").to_version(&none, V7);
    assert_eq!(
        date_texts(&converted, &none),
        (
            "15 MAR 1820".to_string(),
            None,
            Some("The Ides of March".to_string())
        )
    );
    let back = converted.to_version(&none, V551);
    assert_eq!(
        back.value.to_str(&none),
        "INT 15 MAR 1820 (The Ides of March)"
    );
}

// Unrecognised words (carried over from upstream PR #109): a date holding
// words the date model does not recognise keeps its wording, so that
// parsing and formatting a date never drops part of it. A date's qualifier
// belongs to the date value, and each date of a range or period keeps its
// own wording.

#[test]
fn unrecognised_words_of_a_bound_are_kept() {
    // Free text where a date is expected, as some programs write it.
    let date = CalendarDate::parse("vers 1850");
    assert_eq!(date.text.as_deref(), Some("vers 1850"));
    assert_eq!(date.year, None);
    assert_eq!(date.to_gedcom(V551), "vers 1850");

    // What is recognised is still read; the wording stays whole.
    let value = DateValue::parse("ABT @#DJULIAN@ 1850 environ");
    let DateValue::Approximated(Approximation::About, date) = &value else {
        panic!("not an approximated date: {value:?}");
    };
    assert_eq!(date.calendar, Calendar::Julian);
    assert_eq!(date.year, Some(1850));
    assert_eq!(date.text.as_deref(), Some("1850 environ"));
    assert_eq!(value.to_gedcom(V551), "ABT @#DJULIAN@ 1850 environ");
    assert_eq!(value.to_gedcom(V7), "ABT JULIAN 1850 environ");

    // A date read whole has no text of its own.
    let value = DateValue::parse("ABT 2 jan 1850");
    assert!(!value.has_unrecognised_text());
    assert_eq!(value.to_gedcom(V551), "ABT 2 JAN 1850");
}

#[test]
fn date_value_keeps_the_wording_of_each_bound() {
    for (value, written) in [
        ("vers 1850", "vers 1850"),
        ("BET vers 1850 AND 1860", "BET vers 1850 AND 1860"),
        ("FROM 1850 TO later", "FROM 1850 TO later"),
        ("BEF @#DJULIAN@ 1700 or so", "BEF @#DJULIAN@ 1700 or so"),
        ("INT 1850? (about 1850)", "INT 1850? (about 1850)"),
    ] {
        let parsed = DateValue::parse(value);
        assert_eq!(parsed.to_gedcom(V551), written, "{parsed:?}");
    }

    let DateValue::Between(start, end) = DateValue::parse("BET vers 1850 AND 1860") else {
        panic!("not a range");
    };
    assert_eq!(start.text.as_deref(), Some("vers 1850"));
    assert_eq!(end.year, Some(1860));
    assert_eq!(end.text, None);
}

#[test]
fn normalizing_a_date_keeps_unrecognised_words() {
    let none = Dataset::default();
    let date = Date::new("abt vers 1850");
    assert_eq!(
        date.normalize(&none, V551).value.to_str(&none),
        "ABT vers 1850"
    );
    // Converting to another version cannot express the words: the date is
    // left as written.
    assert_eq!(date.to_version(&none, V7), date);
}

#[cfg(feature = "calendar")]
#[test]
fn a_bound_with_unrecognised_words_is_not_converted() {
    // Converting would write the recognised day, month and year in the
    // other calendar and drop the rest of the wording.
    let date = CalendarDate::parse("@#DJULIAN@ 1 JAN 1700 old style");
    assert!(date.convert_to(&Calendar::Gregorian).is_err());
    let value = DateValue::parse("BET @#DJULIAN@ 1 JAN 1700 old style AND 1710");
    assert!(value.convert_to(&Calendar::Gregorian).is_err());
    let none = Dataset::default();
    assert!(
        Date::new("@#DJULIAN@ 1 JAN 1700 old style")
            .convert_to(&none, &Calendar::Gregorian, V551)
            .is_err()
    );
}

// Bounded-input properties. The inputs are generated from a fixed seed by a
// small xorshift generator, so every run checks the same cases and a
// failure is reproducible.

/// Inputs generated per property.
const PROPERTY_CASES: usize = 20_000;

/// The longest input generated, in words.
const MAX_WORDS: usize = 8;

/// A xorshift64* generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % bound as u64).unwrap()
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// Words that date and age payloads are made of, and their usual
/// corruptions.
const WORDS: &[&str] = &[
    "ABT",
    "abt",
    "CAL",
    "EST",
    "BEF",
    "AFT",
    "BET",
    "AND",
    "and",
    "FROM",
    "TO",
    "to",
    "INT",
    "(",
    ")",
    "(phrase)",
    "(a (b) c)",
    "@#DGREGORIAN@",
    "@#DJULIAN@",
    "@#DHEBREW@",
    "@#DFRENCH R@",
    "@#DROMAN@",
    "@#DUNKNOWN@",
    "@#DMYCAL@",
    "@#D",
    "@#Djulian@",
    "@@",
    "@",
    "GREGORIAN",
    "JULIAN",
    "HEBREW",
    "FRENCH_R",
    "ROMAN",
    "_ROMAN",
    "_MYCAL",
    "_mon",
    "_MON",
    "JAN",
    "jan",
    "FEB",
    "DEC",
    "June",
    "VEND",
    "COMP",
    "TSH",
    "ADR",
    "ADS",
    "ELL",
    "0",
    "1",
    "01",
    "29",
    "30",
    "31",
    "36",
    "40",
    "255",
    "256",
    "1699",
    "1699/00",
    "1701/99",
    "1401/8",
    "5/0000",
    "1/",
    "/1",
    "7/11/1959",
    "20B.C.",
    "B.C.",
    "BCE",
    "bc",
    "B.C.E.",
    "1900",
    "2000",
    "4294967295",
    "4294967296",
    "99999999999999999999",
    "6000000",
    "1000000",
    "1000001",
    "<",
    ">",
    "<8",
    "> 99",
    "y",
    "m",
    "w",
    "d",
    "1y",
    "2M",
    "3w",
    "4D",
    "1y6m",
    "400d",
    "0Y",
    "CHILD",
    "infant",
    "Stillborn",
    "12:30",
    "2:5",
    "24:00",
    "23:59:59.999",
    "1:00Z",
    "z",
    ":",
    ".",
    "é",
    "\u{0}",
    "\t",
    "",
    " ",
];

/// A random payload of up to [`MAX_WORDS`] words with random separators.
fn payload(rng: &mut Rng) -> String {
    let mut out = String::new();
    for _ in 0..rng.below(MAX_WORDS + 1) {
        out.push_str(rng.pick(&["", " ", " ", " ", "  ", "\t"]));
        out.push_str(rng.pick(WORDS));
    }
    if rng.below(4) == 0 {
        out.push(' ');
    }
    out
}

#[test]
fn property_date_values() {
    let mut rng = Rng(0x0DA7_E5EE_D000_0001);
    for _ in 0..PROPERTY_CASES {
        let text = payload(&mut rng);
        let lenient = DateValue::parse(&text);
        for version in [V551, V7] {
            // Strict parsing is lenient parsing that may refuse.
            if let Ok(strict) = DateValue::parse_strict(&text, version) {
                assert_eq!(strict, lenient, "{text:?}");
            }
            let _ = DatePeriod::parse_strict(&text, version);
            let _ = DateExact::parse_strict(&text, version);
            let _ = Time::parse_strict(&text, version);
        }
        let _ = DatePeriod::parse(&text);
        let _ = DateExact::parse(&text);
        let _ = Time::parse(&text);
        let _ = CalendarDate::parse(&text);

        // What is read whole writes back to itself in 5.5.1, whose grammar
        // can say everything the model holds.
        if !lenient.has_unrecognised_text() && lenient != DateValue::Empty {
            let written = lenient.to_gedcom(V551);
            assert_eq!(
                DateValue::parse(&written),
                lenient,
                "{text:?} → {written:?}"
            );
        }
        let _ = lenient.to_gedcom(V7);
    }
}

#[test]
fn property_date_conversions_conform_or_keep_the_value() {
    let none = Dataset::default();
    let mut rng = Rng(0x0DA7_E5EE_D000_0002);
    for _ in 0..PROPERTY_CASES {
        let value = (rng.below(5) > 0).then(|| payload(&mut rng));
        let time = (rng.below(3) == 0).then(|| payload(&mut rng));
        let phrase = (rng.below(3) == 0).then(|| rng.pick(WORDS).to_string());
        let date = date(
            value.as_deref().unwrap_or_default(),
            time.as_deref(),
            phrase.as_deref(),
        );
        for version in [V551, V7] {
            let converted = date.to_version(&none, version);
            let value = converted.value.to_str(&none);
            let conforms = DateValue::parse_strict(&value, version).is_ok();
            assert!(
                conforms || converted.value.to_str(&none) == date.value.to_str(&none),
                "{date:?} → {converted:?}"
            );
            // Converting again changes nothing.
            assert_eq!(
                date_texts(&converted.to_version(&none, version), &none),
                date_texts(&converted, &none),
                "{date:?}"
            );
            let _ = date.normalize(&none, version);
            let _ = date.datetime(&none);
        }
    }
}

#[test]
fn property_ages() {
    let none = Dataset::default();
    let mut rng = Rng(0x0A6E_5EED_0000_0003);
    for _ in 0..PROPERTY_CASES {
        let text = payload(&mut rng);
        let lenient = AgeValue::parse(&text);
        for version in [V551, V7] {
            if let Ok(strict) = AgeValue::parse_strict(&text, version) {
                assert_eq!(strict, lenient, "{text:?}");
            }
            let mut age = Age::new(text.as_str());
            if rng.below(3) == 0 {
                age.phrase = Some(Text::new(rng.pick(WORDS)));
            }
            let converted = age.to_version(&none, version);
            let (value, _) = age_texts(&converted, &none);
            assert!(
                AgeValue::parse_strict(&value, version).is_ok()
                    || age_texts(&converted, &none) == age_texts(&age, &none),
                "{age:?} → {converted:?}"
            );
            assert_eq!(converted.to_version(&none, version), converted, "{age:?}");
        }
        // A duration writes back to itself in 7.0, whose grammar has weeks.
        if let AgeValue::Duration { .. } = lenient {
            let written = lenient.to_gedcom(V7);
            assert_eq!(AgeValue::parse(&written), lenient, "{text:?} → {written:?}");
            assert!(AgeValue::parse_strict(&written, V7).is_ok(), "{written:?}");
        }
        let _ = lenient.to_gedcom(V551);
    }
}

#[test]
fn property_times() {
    let mut rng = Rng(0x7135_EED0_0000_0004);
    for _ in 0..PROPERTY_CASES {
        let text = format!(
            "{}:{}{}{}",
            rng.below(30),
            rng.pick(&["0", "5", "59", "60", "07", ""]),
            rng.pick(&["", ":00", ":59", ":60", ":5"]),
            rng.pick(&[
                "",
                ".0",
                ".123456789012345678901234567890",
                "Z",
                ".5Z",
                "z",
                "."
            ]),
        );
        let lenient = Time::parse(&text);
        for version in [V551, V7] {
            if let Ok(strict) = Time::parse_strict(&text, version) {
                assert_eq!(Some(strict), lenient, "{text:?}");
            }
        }
        if let Some(time) = lenient {
            let written = time.to_gedcom(V7);
            assert_eq!(Time::parse(&written), Some(time.clone()), "{text:?}");
            assert!(Time::parse_strict(&written, V7).is_ok(), "{written:?}");
            assert!(Time::parse_strict(&time.to_gedcom(V551), V551).is_ok());
        }
    }
}

#[cfg(feature = "calendar")]
mod calendar_properties {
    use super::{PROPERTY_CASES, Rng, V7, payload};
    use oxidged::Dataset;
    use oxidged::model::Date;
    use oxidged::value::{Calendar, CalendarDate, CalendarError, DateValue, Epoch, Month};

    const CALENDARS: [Calendar; 6] = [
        Calendar::Gregorian,
        Calendar::Julian,
        Calendar::Hebrew,
        Calendar::FrenchRepublican,
        Calendar::Roman,
        Calendar::Unknown,
    ];

    /// A year anywhere in `u32`, mostly near the calendars' range.
    fn year(rng: &mut Rng) -> u32 {
        match rng.below(4) {
            0 => u32::try_from(rng.next() >> 32).unwrap(),
            1 => u32::try_from(rng.below(7_000_000)).unwrap(),
            _ => u32::try_from(rng.below(6_000)).unwrap(),
        }
    }

    #[test]
    fn conversions_terminate_and_round_trip() {
        let mut rng = Rng(0xCA1E_5EED_0000_0005);
        for _ in 0..PROPERTY_CASES {
            let calendar = CALENDARS[rng.below(CALENDARS.len())].clone();
            let months = calendar.months();
            let date = CalendarDate {
                day: (rng.below(8) > 0).then(|| u8::try_from(rng.below(40)).unwrap()),
                month: match months.len() {
                    0 => Some(Month::Extension("_M".into())),
                    count => Month::of(&calendar, u8::try_from(rng.below(count + 2)).unwrap()),
                },
                year: Some(year(&mut rng)),
                epoch: (rng.below(4) == 0).then_some(Epoch::Bce),
                calendar,
                ..CalendarDate::default()
            };
            let Ok(rata_die) = date.to_rata_die() else {
                let _ = date.ordering_key();
                continue;
            };
            for target in &CALENDARS[..4] {
                match date.convert_to(target) {
                    Ok(converted) => assert_eq!(converted.to_rata_die(), Ok(rata_die), "{date:?}"),
                    Err(error) => assert_eq!(error, CalendarError::OutOfRange, "{date:?}"),
                }
            }
            let _ = date.weekday();
            let _ = date.add_days(i64::try_from(rng.next() >> 1).unwrap());
            let _ = date.add_days(-i64::try_from(rng.next() >> 1).unwrap());
        }
    }

    #[test]
    fn any_day_number_converts_or_is_out_of_range() {
        let mut rng = Rng(0xCA1E_5EED_0000_0006);
        for _ in 0..PROPERTY_CASES {
            let rata_die = match rng.below(3) {
                0 => i64::from_ne_bytes(rng.next().to_ne_bytes()),
                1 => i64::try_from(rng.below(900_000_000)).unwrap() - 450_000_000,
                _ => i64::try_from(rng.below(2_000_000)).unwrap() - 500_000,
            };
            for calendar in &CALENDARS {
                match CalendarDate::from_rata_die(rata_die, calendar) {
                    Ok(date) => assert_eq!(date.to_rata_die(), Ok(rata_die), "{rata_die}"),
                    Err(error) => assert!(
                        matches!(
                            error,
                            CalendarError::OutOfRange | CalendarError::UnsupportedCalendar(_)
                        ),
                        "{rata_die}: {error:?}"
                    ),
                }
                let _ = CalendarDate::from_julian_day_number(rata_die, calendar);
            }
        }
    }

    #[test]
    fn payload_conversions_terminate() {
        let none = Dataset::default();
        let mut rng = Rng(0xCA1E_5EED_0000_0007);
        for _ in 0..PROPERTY_CASES {
            let value = DateValue::parse(&payload(&mut rng));
            for calendar in &CALENDARS {
                let _ = value.convert_to(calendar);
            }
            let date = Date::new(payload(&mut rng));
            let _ = date.convert_to(&none, &Calendar::Hebrew, V7);
        }
    }

    #[test]
    fn hebrew_hang_is_gone() {
        // 2 OCT 6000000 must not loop forever (a release-build hang).
        let date = CalendarDate::parse("2 OCT 6000000");
        assert_eq!(
            date.convert_to(&Calendar::Hebrew),
            Err(CalendarError::OutOfRange)
        );
        let edge = CalendarDate::parse("31 DEC 990000");
        let hebrew = edge.convert_to(&Calendar::Hebrew).unwrap();
        assert_eq!(hebrew.to_rata_die(), edge.to_rata_die());
    }
}
