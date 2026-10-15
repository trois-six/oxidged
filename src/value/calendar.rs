//! Calendars, months and epochs of GEDCOM dates.
//!
//! GEDCOM 5.5.1 names a date's calendar with an escape (`@#DJULIAN@`), GEDCOM
//! 7.0 with a keyword (`JULIAN`) or an extension tag (`_MYCAL`). Both name the
//! months with the tags of the calendar (`JAN`, `VEND`, `TSH`, ...) and mark
//! years before the common era with an epoch (`B.C.` in 5.5.1, `BCE` in
//! 7.0). The types here are shared by both versions; each knows its spelling
//! in either.

use std::borrow::Cow;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use super::{Grammar, ascii_upper, is_ext_tag, is_lenient_ext_tag};

/// The calendar of a date.
///
/// The four calendars both versions define, the two 5.5.1 escapes whose
/// calendars were never defined (`@#DROMAN@`, `@#DUNKNOWN@`), and extension
/// calendars (`_MYCAL` in 7.0, or an escape this crate does not know).
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Calendar {
    /// The Gregorian calendar, the default (`@#DGREGORIAN@`, `GREGORIAN`).
    #[default]
    Gregorian,
    /// The Julian calendar (`@#DJULIAN@`, `JULIAN`).
    Julian,
    /// The Hebrew calendar (`@#DHEBREW@`, `HEBREW`).
    Hebrew,
    /// The French Republican calendar (`@#DFRENCH R@`, `FRENCH_R`).
    FrenchRepublican,
    /// The 5.5.1 Roman calendar (`@#DROMAN@`), "for future definition".
    /// GEDCOM 7.0 has no such calendar; it is written as the extension
    /// calendar `_ROMAN`.
    Roman,
    /// The 5.5.1 unknown calendar (`@#DUNKNOWN@`). GEDCOM 7.0 has no such
    /// calendar; it is written as the extension calendar `_UNKNOWN`.
    Unknown,
    /// An extension calendar, named as written: a 7.0 extension tag such as
    /// `_MYCAL`, or the name inside an unknown 5.5.1 escape (`@#DMYCAL@`
    /// gives `MYCAL`).
    Extension(String),
}

/// How a calendar marker was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkerForm {
    /// A 5.5.1 escape, `@#D...@`.
    Escape,
    /// A 7.0 keyword, `JULIAN`.
    Keyword,
    /// An extension tag, `_MYCAL`.
    ExtTag,
}

impl Calendar {
    /// The calendar's name in a GEDCOM 7.0 date: its keyword, or an
    /// extension tag (`_ROMAN`, `_UNKNOWN`, `_MYCAL`).
    #[must_use]
    pub fn gedcom7_tag(&self) -> Cow<'_, str> {
        match self {
            Calendar::Gregorian => Cow::Borrowed("GREGORIAN"),
            Calendar::Julian => Cow::Borrowed("JULIAN"),
            Calendar::Hebrew => Cow::Borrowed("HEBREW"),
            Calendar::FrenchRepublican => Cow::Borrowed("FRENCH_R"),
            Calendar::Roman => Cow::Borrowed("_ROMAN"),
            Calendar::Unknown => Cow::Borrowed("_UNKNOWN"),
            Calendar::Extension(name) if name.starts_with('_') => Cow::Borrowed(name),
            Calendar::Extension(name) => Cow::Owned(format!("_{name}")),
        }
    }

    /// The calendar's escape in a GEDCOM 5.5.1 date (`@#DJULIAN@`).
    ///
    /// GEDCOM 5.5.1 has no extension calendars: an extension calendar gets
    /// an escape of its own name (`@#D_MYCAL@`), which a 5.5.1 reader does
    /// not know.
    #[must_use]
    pub fn gedcom551_escape(&self) -> Cow<'_, str> {
        match self {
            Calendar::Gregorian => Cow::Borrowed("@#DGREGORIAN@"),
            Calendar::Julian => Cow::Borrowed("@#DJULIAN@"),
            Calendar::Hebrew => Cow::Borrowed("@#DHEBREW@"),
            Calendar::FrenchRepublican => Cow::Borrowed("@#DFRENCH R@"),
            Calendar::Roman => Cow::Borrowed("@#DROMAN@"),
            Calendar::Unknown => Cow::Borrowed("@#DUNKNOWN@"),
            Calendar::Extension(name) => Cow::Owned(format!("@#D{name}@")),
        }
    }

    /// The months this calendar defines, in order; empty for a calendar
    /// whose months are not standard (Roman, unknown and extension
    /// calendars, whose months are extension tags).
    #[must_use]
    pub fn months(&self) -> &'static [Month] {
        match self {
            Calendar::Gregorian | Calendar::Julian => &GREGORIAN_MONTHS,
            Calendar::Hebrew => &HEBREW_MONTHS,
            Calendar::FrenchRepublican => &FRENCH_REPUBLICAN_MONTHS,
            _ => &[],
        }
    }

    /// Whether this is one of the four calendars the specifications define.
    #[must_use]
    pub fn is_defined(&self) -> bool {
        matches!(
            self,
            Calendar::Gregorian | Calendar::Julian | Calendar::Hebrew | Calendar::FrenchRepublican
        )
    }

    /// Whether this calendar counts years before an epoch (`BCE`): only the
    /// Gregorian and Julian calendars do.
    #[must_use]
    pub fn has_bce(&self) -> bool {
        matches!(self, Calendar::Gregorian | Calendar::Julian)
    }

    /// Reads a calendar marker: an escape of any case, a 7.0 keyword of any
    /// case, `ROMAN`/`UNKNOWN` as some 5.5.1 files write them, or an
    /// extension tag. Returns the calendar and how it was written.
    pub(crate) fn from_marker(word: &str) -> Option<(Calendar, MarkerForm)> {
        // Markers start with `@`, a letter or `_`: most words are numbers.
        if word.as_bytes().first().is_none_or(u8::is_ascii_digit) {
            return None;
        }
        // The longest name compared: `GREGORIAN`.
        let upper = |word: &str| ascii_upper::<9>(word);
        if let Some(name) = escape_name(word) {
            let calendar = match upper(name).as_ref().and_then(|(u, n)| u.get(..*n)) {
                Some(b"GREGORIAN") => Calendar::Gregorian,
                Some(b"JULIAN") => Calendar::Julian,
                Some(b"HEBREW") => Calendar::Hebrew,
                Some(b"FRENCH R" | b"FRENCH_R") => Calendar::FrenchRepublican,
                Some(b"ROMAN") => Calendar::Roman,
                Some(b"UNKNOWN") => Calendar::Unknown,
                _ => Calendar::Extension(name.to_string()),
            };
            return Some((calendar, MarkerForm::Escape));
        }
        let calendar = match upper(word).as_ref().and_then(|(u, n)| u.get(..*n)) {
            Some(b"GREGORIAN") => Calendar::Gregorian,
            Some(b"JULIAN") => Calendar::Julian,
            Some(b"HEBREW") => Calendar::Hebrew,
            Some(b"FRENCH_R") => Calendar::FrenchRepublican,
            Some(b"ROMAN" | b"_ROMAN") => Calendar::Roman,
            Some(b"UNKNOWN" | b"_UNKNOWN") => Calendar::Unknown,
            _ if is_lenient_ext_tag(word) => {
                return Some((Calendar::Extension(word.to_string()), MarkerForm::ExtTag));
            }
            _ => return None,
        };
        let form = if word.starts_with('_') {
            MarkerForm::ExtTag
        } else {
            MarkerForm::Keyword
        };
        Some((calendar, form))
    }

    /// Whether a marker, written as `word` in `form`, is valid in `grammar`.
    pub(crate) fn marker_is_valid(&self, word: &str, form: MarkerForm, grammar: Grammar) -> bool {
        match grammar {
            // 5.5.1: one of its six escapes, upper case as tags are.
            Grammar::V551 => {
                form == MarkerForm::Escape
                    && !matches!(self, Calendar::Extension(_))
                    && word == self.gedcom551_escape()
            }
            // 7.0: a keyword exactly, or an extension tag.
            Grammar::V7 => match form {
                MarkerForm::Escape => false,
                MarkerForm::Keyword => self.is_defined() && word == self.gedcom7_tag(),
                MarkerForm::ExtTag => is_ext_tag(word),
            },
        }
    }
}

impl std::fmt::Display for Calendar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Calendar::Gregorian => write!(f, "Gregorian"),
            Calendar::Julian => write!(f, "Julian"),
            Calendar::Hebrew => write!(f, "Hebrew"),
            Calendar::FrenchRepublican => write!(f, "French Republican"),
            Calendar::Roman => write!(f, "Roman"),
            Calendar::Unknown => write!(f, "unknown"),
            Calendar::Extension(name) => write!(f, "{name}"),
        }
    }
}

/// The name inside a 5.5.1 calendar escape: `JULIAN` for `@#DJULIAN@`.
fn escape_name(word: &str) -> Option<&str> {
    let inner = word.strip_suffix('@')?;
    let prefix = inner.get(..3)?;
    prefix
        .eq_ignore_ascii_case("@#D")
        .then(|| &inner[3..])
        .filter(|name| !name.is_empty())
}

/// A month of a date.
///
/// GEDCOM names months with tags. The tags of the four defined calendars
/// are all distinct, so a month is known from its tag alone; which calendar
/// it belongs to constrains its validity, not its meaning. Months of other
/// calendars are extension tags.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Month {
    /// January (`JAN`), Gregorian and Julian.
    Jan,
    /// February (`FEB`).
    Feb,
    /// March (`MAR`).
    Mar,
    /// April (`APR`).
    Apr,
    /// May (`MAY`).
    May,
    /// June (`JUN`).
    Jun,
    /// July (`JUL`).
    Jul,
    /// August (`AUG`).
    Aug,
    /// September (`SEP`).
    Sep,
    /// October (`OCT`).
    Oct,
    /// November (`NOV`).
    Nov,
    /// December (`DEC`).
    Dec,
    /// Vendémiaire (`VEND`), French Republican.
    Vend,
    /// Brumaire (`BRUM`).
    Brum,
    /// Frimaire (`FRIM`).
    Frim,
    /// Nivôse (`NIVO`).
    Nivo,
    /// Pluviôse (`PLUV`).
    Pluv,
    /// Ventôse (`VENT`).
    Vent,
    /// Germinal (`GERM`).
    Germ,
    /// Floréal (`FLOR`).
    Flor,
    /// Prairial (`PRAI`).
    Prai,
    /// Messidor (`MESS`).
    Mess,
    /// Thermidor (`THER`).
    Ther,
    /// Fructidor (`FRUC`).
    Fruc,
    /// The complementary days (`COMP`).
    Comp,
    /// Tishrei (`TSH`), Hebrew.
    Tsh,
    /// Cheshvan (`CSH`).
    Csh,
    /// Kislev (`KSL`).
    Ksl,
    /// Tevet (`TVT`).
    Tvt,
    /// Shevat (`SHV`).
    Shv,
    /// Adar I (`ADR`); the only Adar of a common year is `ADS`.
    Adr,
    /// Adar, or Adar II in a leap year (`ADS`).
    Ads,
    /// Nisan (`NSN`).
    Nsn,
    /// Iyar (`IYR`).
    Iyr,
    /// Sivan (`SVN`).
    Svn,
    /// Tammuz (`TMZ`).
    Tmz,
    /// Av (`AAV`).
    Aav,
    /// Elul (`ELL`).
    Ell,
    /// A month named by an extension tag (`_MONTH`), or a month of an
    /// extension calendar, whose months that calendar defines (`POP` in
    /// `_MAYAN 1 POP 1`), as written.
    Extension(String),
}

/// The months of the Gregorian and Julian calendars.
const GREGORIAN_MONTHS: [Month; 12] = [
    Month::Jan,
    Month::Feb,
    Month::Mar,
    Month::Apr,
    Month::May,
    Month::Jun,
    Month::Jul,
    Month::Aug,
    Month::Sep,
    Month::Oct,
    Month::Nov,
    Month::Dec,
];

/// The months of the French Republican calendar.
const FRENCH_REPUBLICAN_MONTHS: [Month; 13] = [
    Month::Vend,
    Month::Brum,
    Month::Frim,
    Month::Nivo,
    Month::Pluv,
    Month::Vent,
    Month::Germ,
    Month::Flor,
    Month::Prai,
    Month::Mess,
    Month::Ther,
    Month::Fruc,
    Month::Comp,
];

/// The months of the Hebrew calendar, in GEDCOM's order, from Tishrei.
const HEBREW_MONTHS: [Month; 13] = [
    Month::Tsh,
    Month::Csh,
    Month::Ksl,
    Month::Tvt,
    Month::Shv,
    Month::Adr,
    Month::Ads,
    Month::Nsn,
    Month::Iyr,
    Month::Svn,
    Month::Tmz,
    Month::Aav,
    Month::Ell,
];

/// English month names read as Gregorian and Julian months.
const ENGLISH_MONTH_NAMES: [&str; 12] = [
    "JANUARY",
    "FEBRUARY",
    "MARCH",
    "APRIL",
    "MAY",
    "JUNE",
    "JULY",
    "AUGUST",
    "SEPTEMBER",
    "OCTOBER",
    "NOVEMBER",
    "DECEMBER",
];

impl Month {
    /// The month's tag, as both versions write it (`JAN`, `VEND`, `TSH`,
    /// or the extension tag).
    #[must_use]
    pub fn tag(&self) -> &str {
        match self {
            Month::Jan => "JAN",
            Month::Feb => "FEB",
            Month::Mar => "MAR",
            Month::Apr => "APR",
            Month::May => "MAY",
            Month::Jun => "JUN",
            Month::Jul => "JUL",
            Month::Aug => "AUG",
            Month::Sep => "SEP",
            Month::Oct => "OCT",
            Month::Nov => "NOV",
            Month::Dec => "DEC",
            Month::Vend => "VEND",
            Month::Brum => "BRUM",
            Month::Frim => "FRIM",
            Month::Nivo => "NIVO",
            Month::Pluv => "PLUV",
            Month::Vent => "VENT",
            Month::Germ => "GERM",
            Month::Flor => "FLOR",
            Month::Prai => "PRAI",
            Month::Mess => "MESS",
            Month::Ther => "THER",
            Month::Fruc => "FRUC",
            Month::Comp => "COMP",
            Month::Tsh => "TSH",
            Month::Csh => "CSH",
            Month::Ksl => "KSL",
            Month::Tvt => "TVT",
            Month::Shv => "SHV",
            Month::Adr => "ADR",
            Month::Ads => "ADS",
            Month::Nsn => "NSN",
            Month::Iyr => "IYR",
            Month::Svn => "SVN",
            Month::Tmz => "TMZ",
            Month::Aav => "AAV",
            Month::Ell => "ELL",
            Month::Extension(tag) => tag,
        }
    }

    /// The month a tag names: a standard month tag exactly as the
    /// specifications spell it, or an extension tag.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Month> {
        if is_ext_tag(tag) {
            return Some(Month::Extension(tag.to_string()));
        }
        Month::standard(tag.as_bytes())
    }

    /// The month of a standard month tag, exactly as the specifications
    /// spell it.
    fn standard(tag: &[u8]) -> Option<Month> {
        Some(match tag {
            b"JAN" => Month::Jan,
            b"FEB" => Month::Feb,
            b"MAR" => Month::Mar,
            b"APR" => Month::Apr,
            b"MAY" => Month::May,
            b"JUN" => Month::Jun,
            b"JUL" => Month::Jul,
            b"AUG" => Month::Aug,
            b"SEP" => Month::Sep,
            b"OCT" => Month::Oct,
            b"NOV" => Month::Nov,
            b"DEC" => Month::Dec,
            b"VEND" => Month::Vend,
            b"BRUM" => Month::Brum,
            b"FRIM" => Month::Frim,
            b"NIVO" => Month::Nivo,
            b"PLUV" => Month::Pluv,
            b"VENT" => Month::Vent,
            b"GERM" => Month::Germ,
            b"FLOR" => Month::Flor,
            b"PRAI" => Month::Prai,
            b"MESS" => Month::Mess,
            b"THER" => Month::Ther,
            b"FRUC" => Month::Fruc,
            b"COMP" => Month::Comp,
            b"TSH" => Month::Tsh,
            b"CSH" => Month::Csh,
            b"KSL" => Month::Ksl,
            b"TVT" => Month::Tvt,
            b"SHV" => Month::Shv,
            b"ADR" => Month::Adr,
            b"ADS" => Month::Ads,
            b"NSN" => Month::Nsn,
            b"IYR" => Month::Iyr,
            b"SVN" => Month::Svn,
            b"TMZ" => Month::Tmz,
            b"AAV" => Month::Aav,
            b"ELL" => Month::Ell,
            _ => return None,
        })
    }

    /// Reads a month leniently: a standard tag of any case, an English month
    /// name (`March`), or an extension tag of any case.
    pub(crate) fn from_word(word: &str) -> Option<Month> {
        // Months start with a letter or `_`: most words are numbers.
        if word.as_bytes().first().is_none_or(u8::is_ascii_digit) {
            return None;
        }
        if is_lenient_ext_tag(word) {
            return Some(Month::Extension(word.to_string()));
        }
        // The longest name: `SEPTEMBER`. Upper case, the word is no
        // extension tag either (`_` and letters or digits would be one).
        let (upper, len) = ascii_upper::<9>(word)?;
        let upper = upper.get(..len)?;
        Month::standard(upper).or_else(|| {
            ENGLISH_MONTH_NAMES
                .iter()
                .position(|name| name.as_bytes() == upper)
                .and_then(|index| GREGORIAN_MONTHS.get(index).cloned())
        })
    }

    /// The month's position in its calendar, from 1: `JAN` and `VEND` are 1,
    /// and so is `TSH`, as GEDCOM orders Hebrew months from Tishrei. `None`
    /// for an extension month.
    #[must_use]
    pub fn number(&self) -> Option<u8> {
        [
            &GREGORIAN_MONTHS[..],
            &FRENCH_REPUBLICAN_MONTHS[..],
            &HEBREW_MONTHS[..],
        ]
        .into_iter()
        .find_map(|months| months.iter().position(|month| month == self))
        .and_then(|index| u8::try_from(index + 1).ok())
    }

    /// The month at position `number` (from 1) of `calendar`.
    #[must_use]
    pub fn of(calendar: &Calendar, number: u8) -> Option<Month> {
        calendar
            .months()
            .get(usize::from(number).checked_sub(1)?)
            .cloned()
    }

    /// Whether this month belongs to `calendar`. Extension months belong to
    /// the calendars whose months are not standard.
    pub(crate) fn belongs_to(&self, calendar: &Calendar) -> bool {
        match self {
            Month::Extension(_) => calendar.months().is_empty(),
            _ => calendar.months().contains(self),
        }
    }
}

/// The epoch of a year.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Epoch {
    /// Before the common era (`BCE` in 7.0, `B.C.` in 5.5.1): year `y BCE`
    /// is `y` years before year 1, so there is no year 0.
    Bce,
    /// An epoch of an extension calendar, as its extension tag.
    Extension(String),
}

impl Epoch {
    /// Reads an epoch marker leniently: `BCE`, `B.C.`, `BC` or `B.C.E.` in
    /// any case, or an extension tag. Returns the epoch and whether it was
    /// written exactly as `grammar` spells it.
    pub(crate) fn from_word(word: &str) -> Option<(Epoch, Spelling)> {
        if is_lenient_ext_tag(word) {
            let spelling = if is_ext_tag(word) {
                Spelling::V7
            } else {
                Spelling::Neither
            };
            return Some((Epoch::Extension(word.to_string()), spelling));
        }
        let spelling = match word {
            "BCE" => Spelling::V7,
            _ if word.eq_ignore_ascii_case("B.C.") => Spelling::V551,
            _ if ["BCE", "BC", "B.C.E.", "B.C"]
                .iter()
                .any(|alias| word.eq_ignore_ascii_case(alias)) =>
            {
                Spelling::Neither
            }
            _ => return None,
        };
        Some((Epoch::Bce, spelling))
    }

    /// The epoch's marker in `grammar`.
    pub(crate) fn tag(&self, grammar: Grammar) -> &str {
        match (self, grammar) {
            (Epoch::Bce, Grammar::V7) => "BCE",
            (Epoch::Bce, Grammar::V551) => "B.C.",
            (Epoch::Extension(tag), _) => tag,
        }
    }
}

/// Which version spells a marker the way it was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Spelling {
    /// As GEDCOM 5.5.1 spells it.
    V551,
    /// As GEDCOM 7.0 spells it.
    V7,
    /// As neither does.
    Neither,
}

impl Spelling {
    /// Whether the spelling is the one `grammar` uses.
    pub(crate) fn is(self, grammar: Grammar) -> bool {
        matches!(
            (self, grammar),
            (Spelling::V551, Grammar::V551) | (Spelling::V7, Grammar::V7)
        )
    }
}

/// The astronomical number of a year: year `y BCE` is `1 - y`, so 1 BCE is
/// 0 and 2 BCE is -1.
#[must_use]
pub(crate) fn astronomical_year(year: u32, bce: bool) -> i64 {
    if bce {
        1 - i64::from(year)
    } else {
        i64::from(year)
    }
}

/// The largest day a month can have in a defined calendar, in the year
/// given (astronomical numbering) when its length depends on it. `None` for
/// a month the calendar does not define, or a calendar without known months.
pub(crate) fn max_day(calendar: &Calendar, month: &Month, year: Option<i64>) -> Option<u8> {
    if !month.belongs_to(calendar) || !calendar.is_defined() {
        return None;
    }
    let leap = |gregorian: bool| {
        year.is_none_or(|y| {
            y.rem_euclid(4) == 0 && (!gregorian || y.rem_euclid(100) != 0 || y.rem_euclid(400) == 0)
        })
    };
    Some(match month {
        Month::Feb => {
            if leap(*calendar == Calendar::Gregorian) {
                29
            } else {
                28
            }
        }
        Month::Jan
        | Month::Mar
        | Month::May
        | Month::Jul
        | Month::Aug
        | Month::Oct
        | Month::Dec => 31,
        // Five complementary days, six in a sextile year.
        Month::Comp => 6,
        // Hebrew months of 29 days.
        Month::Tvt | Month::Ads | Month::Iyr | Month::Tmz | Month::Ell => 29,
        // April, June, September, November, the Republican months and the
        // other Hebrew months: 30 days, at most.
        _ => 30,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markers() {
        assert_eq!(
            Calendar::from_marker("@#DJULIAN@"),
            Some((Calendar::Julian, MarkerForm::Escape))
        );
        assert_eq!(
            Calendar::from_marker("@#dfrench r@"),
            Some((Calendar::FrenchRepublican, MarkerForm::Escape))
        );
        assert_eq!(
            Calendar::from_marker("@#DROMAN@"),
            Some((Calendar::Roman, MarkerForm::Escape))
        );
        assert_eq!(
            Calendar::from_marker("@#DMYCAL@"),
            Some((Calendar::Extension("MYCAL".into()), MarkerForm::Escape))
        );
        assert_eq!(
            Calendar::from_marker("julian"),
            Some((Calendar::Julian, MarkerForm::Keyword))
        );
        assert_eq!(
            Calendar::from_marker("_ROMAN"),
            Some((Calendar::Roman, MarkerForm::ExtTag))
        );
        assert_eq!(
            Calendar::from_marker("_MYCAL"),
            Some((Calendar::Extension("_MYCAL".into()), MarkerForm::ExtTag))
        );
        assert_eq!(Calendar::from_marker("JAN"), None);
        assert_eq!(Calendar::from_marker("@#D@"), None);
    }

    #[test]
    fn test_spellings_per_version() {
        assert_eq!(Calendar::FrenchRepublican.gedcom7_tag(), "FRENCH_R");
        assert_eq!(
            Calendar::FrenchRepublican.gedcom551_escape(),
            "@#DFRENCH R@"
        );
        assert_eq!(Calendar::Roman.gedcom7_tag(), "_ROMAN");
        assert_eq!(Calendar::Extension("MYCAL".into()).gedcom7_tag(), "_MYCAL");
        assert_eq!(Calendar::Extension("_MYCAL".into()).gedcom7_tag(), "_MYCAL");
        assert!(Calendar::Julian.marker_is_valid("JULIAN", MarkerForm::Keyword, Grammar::V7));
        assert!(!Calendar::Julian.marker_is_valid("julian", MarkerForm::Keyword, Grammar::V7));
        assert!(!Calendar::Roman.marker_is_valid("ROMAN", MarkerForm::Keyword, Grammar::V7));
        assert!(Calendar::Roman.marker_is_valid("_ROMAN", MarkerForm::ExtTag, Grammar::V7));
        assert!(Calendar::Roman.marker_is_valid("@#DROMAN@", MarkerForm::Escape, Grammar::V551));
        assert!(!Calendar::Julian.marker_is_valid("JULIAN", MarkerForm::Keyword, Grammar::V551));
    }

    #[test]
    fn test_months() {
        assert_eq!(Month::from_tag("VEND"), Some(Month::Vend));
        assert_eq!(Month::from_tag("vend"), None);
        assert_eq!(Month::from_word("vend"), Some(Month::Vend));
        assert_eq!(Month::from_word("September"), Some(Month::Sep));
        assert_eq!(
            Month::from_word("_mon"),
            Some(Month::Extension("_mon".into()))
        );
        assert_eq!(Month::Tsh.number(), Some(1));
        assert_eq!(Month::Ell.number(), Some(13));
        assert_eq!(Month::Comp.number(), Some(13));
        assert_eq!(Month::Extension("_X".into()).number(), None);
        assert_eq!(Month::of(&Calendar::Hebrew, 7), Some(Month::Ads));
        assert_eq!(Month::of(&Calendar::Julian, 13), None);
        assert_eq!(Month::of(&Calendar::Julian, 0), None);
        assert!(Month::Jan.belongs_to(&Calendar::Julian));
        assert!(!Month::Jan.belongs_to(&Calendar::Hebrew));
        assert!(Month::Extension("_M".into()).belongs_to(&Calendar::Roman));
    }

    #[test]
    fn test_epochs() {
        assert_eq!(Epoch::from_word("BCE"), Some((Epoch::Bce, Spelling::V7)));
        assert_eq!(Epoch::from_word("b.c."), Some((Epoch::Bce, Spelling::V551)));
        assert_eq!(
            Epoch::from_word("bc"),
            Some((Epoch::Bce, Spelling::Neither))
        );
        assert_eq!(Epoch::from_word("AD"), None);
    }

    #[test]
    fn test_max_day() {
        let g = Calendar::Gregorian;
        assert_eq!(max_day(&g, &Month::Feb, Some(1900)), Some(28));
        assert_eq!(max_day(&g, &Month::Feb, Some(2000)), Some(29));
        assert_eq!(
            max_day(&Calendar::Julian, &Month::Feb, Some(1900)),
            Some(29)
        );
        // 1 BCE is astronomical year 0, a leap year.
        assert_eq!(
            max_day(&g, &Month::Feb, Some(astronomical_year(1, true))),
            Some(29)
        );
        assert_eq!(max_day(&g, &Month::Feb, None), Some(29));
        assert_eq!(max_day(&g, &Month::Vend, Some(1)), None);
        assert_eq!(
            max_day(&Calendar::FrenchRepublican, &Month::Comp, Some(3)),
            Some(6)
        );
        assert_eq!(
            max_day(&Calendar::Hebrew, &Month::Ell, Some(5784)),
            Some(29)
        );
        assert_eq!(
            max_day(&Calendar::Roman, &Month::Extension("_M".into()), None),
            None
        );
    }
}
