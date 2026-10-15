//! The date payloads of GEDCOM 5.5.1 and 7.0.
//!
//! A `DATE` payload is more than one date. GEDCOM 7.0 calls it a
//! `DateValue`: a single date, an approximated one (`ABT`, `CAL`, `EST`), a
//! range (`BEF`, `AFT`, `BET … AND …`) or a period (`FROM … TO …`), each date
//! in its own calendar. GEDCOM 5.5.1 adds the interpreted date
//! (`INT date (phrase)`) and the date phrase (`(phrase)`), which 7.0 moved to
//! the `PHRASE` substructure. Two narrower payloads exist: `DateExact` (a
//! Gregorian day, month and year, for timestamps) and `DatePeriod` (`FROM`
//! and `TO` only).
//!
//! Each payload type parses two ways. `parse` never fails: it reads both
//! versions' syntaxes and common deviations from them (lower case, calendar
//! escapes before the qualifier, `BC`, English month names, ...), and keeps
//! any wording it does not understand in [`CalendarDate::text`], so that no
//! word is lost. `parse_strict` accepts exactly one version's grammar.
//! `to_gedcom` writes a value in one version's grammar.

use std::fmt::Write as _;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use super::calendar::{Calendar, Epoch, Month, Spelling, astronomical_year, max_day};
use super::time::Time;
use super::{
    Checker, Grammar, ValueError, Word, ascii_upper, is_ext_tag, is_integer, single_spaced, span,
    word_iter, words,
};
use crate::GedcomVersion;

/// One date: the `date` production of GEDCOM 7.0 (`[calendar] [[day] month]
/// year [epoch]`) and the `DATE` of GEDCOM 5.5.1.
///
/// The fields hold what was recognised. When the date holds words the
/// fields cannot represent (`vers 1850`, `1850 environ`), [`text`] keeps the
/// date's whole wording after its calendar marker, and the date is written
/// back with that wording: nothing is dropped.
///
/// [`text`]: CalendarDate::text
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CalendarDate {
    /// The calendar; Gregorian when the date names none.
    pub calendar: Calendar,
    /// The day of the month.
    pub day: Option<u8>,
    /// The month.
    pub month: Option<Month>,
    /// The year, as written: a positive count from the calendar's epoch,
    /// before the common era when [`epoch`](Self::epoch) says so.
    pub year: Option<u32>,
    /// The other year of a GEDCOM 5.5.1 dual year (`1699/00` has year 1699
    /// and dual year 1700): the first later year that ends with the digits
    /// written after the slash. GEDCOM 7.0 has no dual years.
    pub dual_year: Option<u32>,
    /// The epoch, such as before the common era.
    pub epoch: Option<Epoch>,
    /// The date's wording after its calendar marker, when it holds words the
    /// fields above do not model. `None` when every word was read.
    pub text: Option<String>,
}

/// How an approximated date is approximated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Approximation {
    /// `ABT`: near the date.
    About,
    /// `CAL`: calculated from other data.
    Calculated,
    /// `EST`: near the date, and calculated from other data.
    Estimated,
}

impl Approximation {
    /// The keyword (`ABT`, `CAL` or `EST`).
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Approximation::About => "ABT",
            Approximation::Calculated => "CAL",
            Approximation::Estimated => "EST",
        }
    }
}

/// A `DATE` payload: GEDCOM 7.0's `DateValue`, and the `DATE_VALUE` of
/// GEDCOM 5.5.1.
///
/// # Example
///
/// ```
/// use oxidged::value::{Calendar, DateValue};
/// use oxidged::GedcomVersion;
///
/// let value = DateValue::parse("BET @#DJULIAN@ 1700 AND @#DJULIAN@ 1710");
/// let DateValue::Between(start, end) = &value else { panic!() };
/// assert_eq!(start.calendar, Calendar::Julian);
/// assert_eq!(end.year, Some(1710));
/// assert_eq!(value.to_gedcom(GedcomVersion::V7_0), "BET JULIAN 1700 AND JULIAN 1710");
/// ```
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum DateValue {
    /// An empty payload, which GEDCOM 7.0 allows when only a `PHRASE` or a
    /// `TIME` is known.
    #[default]
    Empty,
    /// A single date.
    Date(CalendarDate),
    /// An approximated date: `ABT`, `CAL` or `EST`.
    Approximated(Approximation, CalendarDate),
    /// `BEF date`: no later than the date.
    Before(CalendarDate),
    /// `AFT date`: no earlier than the date.
    After(CalendarDate),
    /// `BET date AND date`: some day between the two dates.
    Between(CalendarDate, CalendarDate),
    /// A period, `FROM date`, `TO date` or `FROM date TO date`.
    Period(DatePeriod),
    /// GEDCOM 5.5.1's `INT date (phrase)`: a date interpreted from a phrase.
    Interpreted {
        /// The date read from the phrase.
        date: CalendarDate,
        /// The phrase, without its parentheses.
        phrase: Option<String>,
    },
    /// GEDCOM 5.5.1's `(phrase)`: text that is not a date.
    Phrase(String),
}

/// A period: GEDCOM 7.0's `DatePeriod`, and the `DATE_PERIOD` of 5.5.1.
/// Both bounds absent is the empty period, which 7.0 allows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DatePeriod {
    /// `FROM date`: the first day of the period.
    pub from: Option<CalendarDate>,
    /// `TO date`: the last day of the period.
    pub to: Option<CalendarDate>,
}

/// An exact Gregorian date: GEDCOM 7.0's `DateExact` (`day month year`), and
/// the `DATE_EXACT` of 5.5.1, used by timestamps such as `CHAN.DATE`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DateExact {
    /// The day of the month.
    pub day: u8,
    /// The month, `JAN` to `DEC`.
    pub month: Month,
    /// The year.
    pub year: u32,
    /// A GEDCOM 5.5.1 dual year (`YEAR_GREG` allows one); see
    /// [`CalendarDate::dual_year`].
    pub dual_year: Option<u32>,
}

/// The keywords that can start a date value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keyword {
    Approximated(Approximation),
    Before,
    After,
    Between,
    From,
    To,
    Interpreted,
}

impl Keyword {
    /// Reads a keyword of any case.
    fn from_word(word: &str) -> Option<Keyword> {
        let (upper, len) = ascii_upper::<4>(word)?;
        Some(match upper.get(..len)? {
            b"ABT" => Keyword::Approximated(Approximation::About),
            b"CAL" => Keyword::Approximated(Approximation::Calculated),
            b"EST" => Keyword::Approximated(Approximation::Estimated),
            b"BEF" => Keyword::Before,
            b"AFT" => Keyword::After,
            b"BET" => Keyword::Between,
            b"FROM" => Keyword::From,
            b"TO" => Keyword::To,
            b"INT" => Keyword::Interpreted,
            _ => return None,
        })
    }
}

/// Whether `word` is `keyword` in any case, and, in GEDCOM 7.0, exactly.
fn is_keyword(word: &Word<'_>, keyword: &str, ck: &mut Checker) -> bool {
    if !word.text.eq_ignore_ascii_case(keyword) {
        return false;
    }
    if ck.is_v7() && word.text != keyword {
        ck.fail(|| format!("{:?} must be upper case", word.text));
    }
    true
}

impl CalendarDate {
    /// Reads one date, never failing: the fields get what is recognised, and
    /// [`text`](Self::text) the whole wording when some of it is not.
    ///
    /// ```
    /// use oxidged::value::{Calendar, CalendarDate};
    ///
    /// let date = CalendarDate::parse("@#DJULIAN@ 1850 environ");
    /// assert_eq!(date.calendar, Calendar::Julian);
    /// assert_eq!(date.year, Some(1850));
    /// assert_eq!(date.text.as_deref(), Some("1850 environ"));
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> CalendarDate {
        let words = words(text);
        if words.is_empty() {
            return CalendarDate::default();
        }
        parse_date(text, &words, None, &mut Checker::lenient())
    }

    /// Whether the date has a day, a month and a year, and no unread
    /// wording: the dates calendar arithmetic needs.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.day.is_some() && self.month.is_some() && self.year.is_some() && self.text.is_none()
    }

    /// The year in astronomical numbering, where 1 BCE is year 0 and 2 BCE
    /// year -1. `None` without a year, or with an extension epoch.
    #[must_use]
    pub fn astronomical_year(&self) -> Option<i64> {
        let year = self.year?;
        match &self.epoch {
            None => Some(i64::from(year)),
            Some(Epoch::Bce) => Some(astronomical_year(year, true)),
            Some(Epoch::Extension(_)) => None,
        }
    }

    /// The date in the grammar of `version`.
    ///
    /// GEDCOM 7.0 has no dual years: the year is written without its other
    /// year, which [`Date::to_version`](crate::model::Date::to_version) keeps in a
    /// `PHRASE`.
    #[must_use]
    pub fn to_gedcom(&self, version: GedcomVersion) -> String {
        let mut out = String::new();
        self.write(&mut out, Grammar::from(version), false);
        out
    }

    /// Writes the date in `grammar`. With `force_calendar`, a Gregorian date
    /// names its calendar too, as GEDCOM 7.0 recommends when another date of
    /// the same value is not Gregorian.
    pub(crate) fn write(&self, out: &mut String, grammar: Grammar, force_calendar: bool) {
        let start = out.len();
        let part = |out: &mut String, text: &str| {
            if out.len() > start {
                out.push(' ');
            }
            out.push_str(text);
        };
        // A date starting with an extension month would read as a date of
        // that extension calendar: it names its calendar, even Gregorian.
        let ambiguous = self.text.is_none()
            && self.day.is_none()
            && matches!(self.month, Some(Month::Extension(_)));
        match grammar {
            Grammar::V551 if self.calendar != Calendar::Gregorian || ambiguous => {
                part(out, &self.calendar.gedcom551_escape());
            }
            Grammar::V7 if self.calendar != Calendar::Gregorian || force_calendar || ambiguous => {
                part(out, &self.calendar.gedcom7_tag());
            }
            _ => {}
        }
        if let Some(text) = &self.text {
            part(out, text);
            return;
        }
        if let Some(day) = self.day {
            part(out, "");
            let _ = write!(out, "{day}");
        }
        if let Some(month) = &self.month {
            part(out, month.tag());
        }
        if let Some(year) = self.year {
            part(out, "");
            let _ = write!(out, "{year}");
            if let (Some(dual), Grammar::V551) = (self.dual_year, grammar) {
                out.push('/');
                out.push_str(&dual_suffix(year, dual));
            }
        }
        if let Some(epoch) = &self.epoch {
            part(out, epoch.tag(grammar));
        }
    }
}

impl DateValue {
    /// Reads a date payload, never failing.
    ///
    /// Both versions' syntaxes are read, and their common deviations:
    /// keywords, months and calendars of any case; calendar escapes and
    /// keywords; an escape in front of the whole value, which then applies to
    /// every date that names no calendar (`@#DJULIAN@ BET 1700 AND 1710`);
    /// `BC`, `B.C.` and `BCE`; English month names; any whitespace. Wording
    /// that is still not understood is kept in [`CalendarDate::text`]: a
    /// whole value that cannot be read is a [`DateValue::Date`] whose date
    /// holds it all.
    #[must_use]
    pub fn parse(text: &str) -> DateValue {
        parse_value(text, &mut Checker::lenient())
    }

    /// Reads a date payload that follows the grammar of `version` exactly.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] saying why the payload is not a valid
    /// `DateValue` (7.0) or `DATE_VALUE` (5.5.1).
    pub fn parse_strict(text: &str, version: GedcomVersion) -> Result<DateValue, ValueError> {
        DateValue::strict(text, Grammar::from(version))
    }

    /// [`parse_strict`](Self::parse_strict) in `grammar`.
    pub(crate) fn strict(text: &str, grammar: Grammar) -> Result<DateValue, ValueError> {
        let mut ck = Checker::strict(grammar);
        let value = parse_value(text, &mut ck);
        ck.finish("DateValue", text, value)
    }

    /// Whether [`strict`](Self::strict) accepts `text`, found without the
    /// value or the reason.
    pub(crate) fn is_strict(text: &str, grammar: Grammar) -> bool {
        let mut ck = Checker::verdict(grammar);
        let _ = parse_value(text, &mut ck);
        ck.passed()
    }

    /// The value in the grammar of `version`.
    ///
    /// GEDCOM 7.0 has neither interpreted dates nor date phrases in the
    /// payload: their phrase is left out here, and
    /// [`Date::to_version`](crate::model::Date::to_version) moves it to a `PHRASE`.
    #[must_use]
    pub fn to_gedcom(&self, version: GedcomVersion) -> String {
        self.format(Grammar::from(version))
    }

    /// [`to_gedcom`](Self::to_gedcom) in `grammar`.
    pub(crate) fn format(&self, grammar: Grammar) -> String {
        let mut out = String::new();
        self.write(&mut out, grammar);
        out
    }

    /// Writes the value in `grammar`.
    pub(crate) fn write(&self, out: &mut String, grammar: Grammar) {
        let force = grammar == Grammar::V7
            && self
                .dates()
                .any(|date| date.calendar != Calendar::Gregorian);
        let date = |out: &mut String, keyword: &str, date: &CalendarDate| {
            out.push_str(keyword);
            date.write(out, grammar, force);
        };
        match self {
            DateValue::Empty => {}
            DateValue::Date(d) => d.write(out, grammar, force),
            DateValue::Approximated(how, d) => {
                out.push_str(how.tag());
                date(out, " ", d);
            }
            DateValue::Before(d) => date(out, "BEF ", d),
            DateValue::After(d) => date(out, "AFT ", d),
            DateValue::Between(start, end) => {
                date(out, "BET ", start);
                date(out, " AND ", end);
            }
            DateValue::Period(period) => period.write(out, grammar, force),
            DateValue::Interpreted { date: d, phrase } => {
                if grammar == Grammar::V551 {
                    date(out, "INT ", d);
                    if let Some(phrase) = phrase {
                        let _ = write!(out, " ({phrase})");
                    }
                } else {
                    d.write(out, grammar, force);
                }
            }
            DateValue::Phrase(phrase) => {
                if grammar == Grammar::V551 {
                    let _ = write!(out, "({phrase})");
                }
            }
        }
    }

    /// The dates of the value: none, one or two.
    pub fn dates(&self) -> impl Iterator<Item = &CalendarDate> {
        let (first, second) = match self {
            DateValue::Empty | DateValue::Phrase(_) => (None, None),
            DateValue::Date(d)
            | DateValue::Approximated(_, d)
            | DateValue::Before(d)
            | DateValue::After(d)
            | DateValue::Interpreted { date: d, .. } => (Some(d), None),
            DateValue::Between(a, b) => (Some(a), Some(b)),
            DateValue::Period(period) => (period.from.as_ref(), period.to.as_ref()),
        };
        first.into_iter().chain(second)
    }

    /// The dates of the value, to change them in place.
    pub fn dates_mut(&mut self) -> impl Iterator<Item = &mut CalendarDate> {
        let (first, second) = match self {
            DateValue::Empty | DateValue::Phrase(_) => (None, None),
            DateValue::Date(d)
            | DateValue::Approximated(_, d)
            | DateValue::Before(d)
            | DateValue::After(d)
            | DateValue::Interpreted { date: d, .. } => (Some(d), None),
            DateValue::Between(a, b) => (Some(a), Some(b)),
            DateValue::Period(period) => (period.from.as_mut(), period.to.as_mut()),
        };
        first.into_iter().chain(second)
    }

    /// The calendar of the value's first date; Gregorian without a date.
    #[must_use]
    pub fn calendar(&self) -> Calendar {
        self.dates()
            .next()
            .map_or(Calendar::Gregorian, |date| date.calendar.clone())
    }

    /// The phrase of an interpreted date or a date phrase.
    #[must_use]
    pub fn phrase(&self) -> Option<&str> {
        match self {
            DateValue::Interpreted { phrase, .. } => phrase.as_deref(),
            DateValue::Phrase(phrase) => Some(phrase),
            _ => None,
        }
    }

    /// Whether some wording of the value was not understood (see
    /// [`CalendarDate::text`]).
    #[must_use]
    pub fn has_unrecognised_text(&self) -> bool {
        self.dates().any(|date| date.text.is_some())
    }
}

impl DatePeriod {
    /// Reads a period leniently: `None` when the payload is not a period.
    #[must_use]
    pub fn parse(text: &str) -> Option<DatePeriod> {
        match DateValue::parse(text) {
            DateValue::Empty => Some(DatePeriod::default()),
            DateValue::Period(period) if !period_has_text(&period) => Some(period),
            _ => None,
        }
    }

    /// Reads a period that follows the grammar of `version` exactly: an
    /// empty payload (7.0 only), `FROM date`, `TO date` or `FROM date TO
    /// date`.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] saying why the payload is not a valid
    /// `DatePeriod` (7.0) or `DATE_PERIOD` (5.5.1).
    pub fn parse_strict(text: &str, version: GedcomVersion) -> Result<DatePeriod, ValueError> {
        let mut ck = Checker::strict(Grammar::from(version));
        let period = DatePeriod::checked(text, &mut ck);
        ck.finish("DatePeriod", text, period)
    }

    /// Whether [`parse_strict`](Self::parse_strict) accepts `text` in
    /// `grammar`.
    pub(crate) fn is_strict(text: &str, grammar: Grammar) -> bool {
        let mut ck = Checker::verdict(grammar);
        let _ = DatePeriod::checked(text, &mut ck);
        ck.passed()
    }

    /// Reads a period, checking it against the checker's grammar.
    fn checked(text: &str, ck: &mut Checker) -> DatePeriod {
        match parse_value(text, ck) {
            DateValue::Empty => DatePeriod::default(),
            DateValue::Period(period) => period,
            _ => {
                ck.fail(|| "not a period: it must start with FROM or TO".to_string());
                DatePeriod::default()
            }
        }
    }

    /// The period in the grammar of `version`.
    #[must_use]
    pub fn to_gedcom(&self, version: GedcomVersion) -> String {
        let grammar = Grammar::from(version);
        let force = grammar == Grammar::V7
            && [&self.from, &self.to]
                .into_iter()
                .flatten()
                .any(|date| date.calendar != Calendar::Gregorian);
        let mut out = String::new();
        self.write(&mut out, grammar, force);
        out
    }

    fn write(&self, out: &mut String, grammar: Grammar, force: bool) {
        if let Some(from) = &self.from {
            out.push_str("FROM ");
            from.write(out, grammar, force);
        }
        if let Some(to) = &self.to {
            if self.from.is_some() {
                out.push(' ');
            }
            out.push_str("TO ");
            to.write(out, grammar, force);
        }
    }
}

fn period_has_text(period: &DatePeriod) -> bool {
    [&period.from, &period.to]
        .into_iter()
        .flatten()
        .any(|date| date.text.is_some())
}

impl DateExact {
    /// Reads an exact date leniently: `None` unless the payload is a
    /// Gregorian day, month and year, with no qualifier or epoch.
    #[must_use]
    pub fn parse(text: &str) -> Option<DateExact> {
        match DateValue::parse(text) {
            DateValue::Date(date) => DateExact::from_date(&date),
            _ => None,
        }
    }

    /// Reads an exact date that follows the grammar of `version` exactly:
    /// `day month year`, in the Gregorian calendar, with no calendar marker
    /// or epoch.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] saying why the payload is not a valid
    /// `DateExact` (7.0) or `DATE_EXACT` (5.5.1).
    pub fn parse_strict(text: &str, version: GedcomVersion) -> Result<DateExact, ValueError> {
        let mut ck = Checker::strict(Grammar::from(version));
        let exact = DateExact::checked(text, &mut ck);
        ck.finish("DateExact", text, exact)
    }

    /// Whether [`parse_strict`](Self::parse_strict) accepts `text` in
    /// `grammar`.
    pub(crate) fn is_strict(text: &str, grammar: Grammar) -> bool {
        let mut ck = Checker::verdict(grammar);
        let _ = DateExact::checked(text, &mut ck);
        ck.passed()
    }

    /// Reads an exact date, checking it against the checker's grammar.
    fn checked(text: &str, ck: &mut Checker) -> DateExact {
        let value = parse_value(text, ck);
        let exact = match &value {
            DateValue::Date(date) if word_iter(text).count() == 3 => DateExact::from_date(date),
            _ => None,
        };
        exact.unwrap_or_else(|| {
            ck.fail(|| "an exact date is a day, a month and a year".to_string());
            DateExact {
                day: 0,
                month: Month::Jan,
                year: 0,
                dual_year: None,
            }
        })
    }

    fn from_date(date: &CalendarDate) -> Option<DateExact> {
        (date.calendar == Calendar::Gregorian
            && date.epoch.is_none()
            && date.text.is_none()
            && date
                .month
                .as_ref()
                .is_some_and(|m| m.belongs_to(&Calendar::Gregorian)))
        .then(|| {
            Some(DateExact {
                day: date.day?,
                month: date.month.clone()?,
                year: date.year?,
                dual_year: date.dual_year,
            })
        })
        .flatten()
    }

    /// The date in the grammar of `version` (`3 DEC 2023`).
    #[must_use]
    pub fn to_gedcom(&self, version: GedcomVersion) -> String {
        CalendarDate::from(self.clone()).to_gedcom(version)
    }
}

impl From<DateExact> for CalendarDate {
    fn from(exact: DateExact) -> CalendarDate {
        CalendarDate {
            day: Some(exact.day),
            month: Some(exact.month),
            year: Some(exact.year),
            dual_year: exact.dual_year,
            ..CalendarDate::default()
        }
    }
}

/// A whole value that cannot be read: one date holding all its wording.
fn unrecognised(text: &str, ck: &mut Checker) -> DateValue {
    ck.fail(|| "not a date value".to_string());
    DateValue::Date(CalendarDate {
        text: Some(ck.kept(text.trim())),
        ..CalendarDate::default()
    })
}

/// Reads a date value, checking it against the checker's grammar.
fn parse_value(text: &str, ck: &mut Checker) -> DateValue {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        ck.require(text.is_empty(), || "only whitespace".to_string());
        ck.require(!ck.is_v551(), || {
            "GEDCOM 5.5.1 has no empty date".to_string()
        });
        return DateValue::Empty;
    }
    if let Some(inside) = trimmed.strip_prefix('(') {
        ck.require(trimmed.len() == text.len(), || {
            "leading or trailing whitespace".to_string()
        });
        ck.require(!ck.is_v7(), || {
            "GEDCOM 7.0 has no date phrase in the payload; it uses PHRASE".to_string()
        });
        let phrase = inside.strip_suffix(')').unwrap_or_else(|| {
            ck.fail(|| "the date phrase is not closed".to_string());
            inside
        });
        ck.require(!phrase.is_empty(), || "empty date phrase".to_string());
        return DateValue::Phrase(ck.kept(phrase));
    }

    let all = words(text);
    let mut list: &[Word<'_>] = &all;

    // A calendar in front of the whole value, before its keyword: not the
    // grammar of either version, but written by some programs.
    let mut default = None;
    if let [first, second, ..] = list
        && let (Some((calendar, _)), Some(_)) = (
            Calendar::from_marker(first.text),
            Keyword::from_word(second.text),
        )
    {
        ck.fail(|| "the calendar must follow the keyword".to_string());
        default = Some(calendar);
        list = &list[1..];
    }
    let default = default.as_ref();

    let keyword = Keyword::from_word(list[0].text);
    if keyword.is_some() && ck.is_v7() && list[0].text != list[0].text.to_ascii_uppercase() {
        ck.fail(|| format!("{:?} must be upper case", list[0].text));
    }
    if keyword == Some(Keyword::Interpreted) {
        return parse_interpreted(text, list, default, ck);
    }
    ck.require(single_spaced(text, &all), || {
        "words must be separated by single spaces".to_string()
    });

    let date = |words: &[Word<'_>], ck: &mut Checker| parse_date(text, words, default, ck);
    let rest = &list[1..];
    match keyword {
        None => DateValue::Date(date(list, ck)),
        Some(_) if rest.is_empty() => unrecognised(text, ck),
        Some(Keyword::Approximated(how)) => DateValue::Approximated(how, date(rest, ck)),
        Some(Keyword::Before) => DateValue::Before(date(rest, ck)),
        Some(Keyword::After) => DateValue::After(date(rest, ck)),
        Some(Keyword::Between) => match split_at_keyword(rest, "AND", ck) {
            Some((start, end)) => DateValue::Between(date(start, ck), date(end, ck)),
            None => unrecognised(text, ck),
        },
        Some(Keyword::From) => match split_at_keyword(rest, "TO", ck) {
            Some((from, to)) => DateValue::Period(DatePeriod {
                from: Some(date(from, ck)),
                to: Some(date(to, ck)),
            }),
            None if rest.iter().any(|w| w.text.eq_ignore_ascii_case("TO")) => {
                unrecognised(text, ck)
            }
            None => DateValue::Period(DatePeriod {
                from: Some(date(rest, ck)),
                to: None,
            }),
        },
        Some(Keyword::To) => DateValue::Period(DatePeriod {
            from: None,
            to: Some(date(rest, ck)),
        }),
        Some(Keyword::Interpreted) => unreachable!("handled above"),
    }
}

/// Splits `words` on the first `keyword` with words on both sides.
fn split_at_keyword<'w, 'a>(
    words: &'w [Word<'a>],
    keyword: &str,
    ck: &mut Checker,
) -> Option<(&'w [Word<'a>], &'w [Word<'a>])> {
    let at = words
        .iter()
        .position(|w| w.text.eq_ignore_ascii_case(keyword))?;
    if at == 0 || at + 1 == words.len() {
        return None;
    }
    is_keyword(&words[at], keyword, ck);
    Some((&words[..at], &words[at + 1..]))
}

/// Reads `INT date (phrase)`, the words starting at `INT`.
fn parse_interpreted(
    text: &str,
    list: &[Word<'_>],
    default: Option<&Calendar>,
    ck: &mut Checker,
) -> DateValue {
    ck.require(!ck.is_v7(), || {
        "GEDCOM 7.0 has no INT; the phrase goes to PHRASE".to_string()
    });
    let after_keyword = list[0].end;
    let open = text[after_keyword..].find('(').map(|at| after_keyword + at);
    let date_end = open.unwrap_or(text.len());
    // The words before the phrase: words come in order.
    let count = list[1..].iter().take_while(|w| w.end <= date_end).count();
    let head = &list[..=count];
    let date_words = &head[1..];
    if date_words.is_empty() {
        return unrecognised(text, ck);
    }

    let phrase = match open {
        None => {
            ck.fail(|| "INT needs a phrase in parentheses".to_string());
            None
        }
        Some(open) => {
            let inside = &text[open + 1..];
            let trimmed = inside.trim_end();
            ck.require(trimmed.len() == inside.len(), || {
                "trailing whitespace".to_string()
            });
            let phrase = trimmed.strip_suffix(')').unwrap_or_else(|| {
                ck.fail(|| "the date phrase is not closed".to_string());
                trimmed
            });
            ck.require(!phrase.is_empty(), || "empty date phrase".to_string());
            Some(ck.kept(phrase))
        }
    };
    if ck.grammar().is_some() {
        // `INT date (` with single spaces; the phrase itself is free text.
        let head_end = date_words.last().map_or(after_keyword, |w| w.end);
        let spaced = list[0].start == 0
            && single_spaced(&text[..head_end], head)
            && open.is_some_and(|open| open == head_end + 1 && &text[head_end..open] == " ");
        ck.require(spaced, || {
            "words must be separated by single spaces".to_string()
        });
    }
    DateValue::Interpreted {
        date: parse_date(text, date_words, default, ck),
        phrase,
    }
}

/// What a year's word holds: `1699`, `1699/00`, `20B.C.`.
struct YearWord {
    year: u32,
    dual: Option<(u32, usize)>,
    attached_epoch: Option<(Epoch, Spelling)>,
}

/// Reads a year's word: digits, optionally a slash and the last digits of a
/// dual year, optionally an attached epoch.
fn parse_year(word: &str) -> Option<YearWord> {
    let core_len = word
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b'/')
        .count();
    let (core, attached) = word.split_at(core_len);
    // Only `BCE` and its spellings attach: an extension epoch is a word.
    let attached_epoch = match Epoch::from_word(attached) {
        _ if attached.is_empty() => None,
        Some((Epoch::Bce, spelling)) => Some((Epoch::Bce, spelling)),
        _ => return None,
    };
    let (year, suffix) = match core.split_once('/') {
        Some((year, suffix)) => (year, Some(suffix)),
        None => (core, None),
    };
    if !is_integer(year) {
        return None;
    }
    let year: u32 = year.parse().ok()?;
    let dual = match suffix {
        None => None,
        Some(suffix) if is_integer(suffix) && suffix.len() <= 4 => {
            Some((dual_alternative(year, suffix)?, suffix.len()))
        }
        Some(_) => return None,
    };
    Some(YearWord {
        year,
        dual,
        attached_epoch,
    })
}

/// The first year after `year` that ends with the digits `suffix`:
/// `1699/00` gives 1700, `1401/8` gives 1408, `1701/99` gives 1799.
fn dual_alternative(year: u32, suffix: &str) -> Option<u32> {
    let modulus = 10u32.checked_pow(u32::try_from(suffix.len()).ok()?)?;
    let ending: u32 = suffix.parse().ok()?;
    let candidate = (year - year % modulus).checked_add(ending)?;
    if candidate > year {
        Some(candidate)
    } else {
        candidate.checked_add(modulus)
    }
}

/// The digits that name `dual` after the slash of `year`'s dual year: its
/// last two digits, or more when `dual` is further than a century away.
fn dual_suffix(year: u32, dual: u32) -> String {
    let full = dual.to_string();
    (2..full.len())
        .map(|digits| {
            format!(
                "{:0digits$}",
                u64::from(dual) % 10u64.pow(u32::try_from(digits).unwrap_or(0))
            )
        })
        .find(|suffix| dual_alternative(year, suffix) == Some(dual))
        .unwrap_or(full)
}

/// Reads one date from `list`, a non-empty run of the words of `text`.
/// Whether `word` is a tag: a standard tag (`POP`: an upper-case letter,
/// then upper-case letters, digits or underscores) or an extension tag.
fn is_tag(word: &str) -> bool {
    let mut bytes = word.bytes();
    let first = bytes.next();
    (first.is_some_and(|b| b.is_ascii_uppercase()) || is_ext_tag(word))
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn parse_date(
    text: &str,
    list: &[Word<'_>],
    default: Option<&Calendar>,
    ck: &mut Checker,
) -> CalendarDate {
    let mut date = CalendarDate {
        calendar: default.cloned().unwrap_or_default(),
        ..CalendarDate::default()
    };
    let mut rest = list;
    if let Some(first) = rest.first()
        && let Some((calendar, form)) = Calendar::from_marker(first.text)
    {
        // A marker names a calendar only when a date follows it.
        if rest.len() > 1 {
            if let Some(grammar) = ck.grammar() {
                ck.require(calendar.marker_is_valid(first.text, form, grammar), || {
                    format!("{:?} is not a calendar of this version", first.text)
                });
            }
            date.calendar = calendar;
            rest = &rest[1..];
        }
    }

    // Recognise `[[day] month] year [epoch]` from the left.
    let mut read = 0;
    let mut day_word = None;
    let mut month_word = None;
    let mut dual_digits = 0;
    let mut epoch_spelling = None;
    let mut epoch_attached = false;
    // The month of `rest[i]`, followed by its year: an extension calendar
    // defines its own months, so any tag names one there.
    let month_at = |i: usize| -> Option<Month> {
        let word = rest.get(i)?.text;
        if matches!(date.calendar, Calendar::Extension(_))
            && is_tag(word)
            && rest
                .get(i + 1)
                .is_some_and(|w| parse_year(w.text).is_some())
        {
            return Some(Month::Extension(word.to_string()));
        }
        Month::from_word(word)
    };
    if let [day, ..] = rest
        && is_integer(day.text)
        && month_at(1).is_some()
        && let Ok(value) = day.text.parse::<u8>()
    {
        date.day = Some(value);
        day_word = Some(day.text);
        read = 1;
    }
    if let Some(month) = month_at(read) {
        date.month = Some(month);
        month_word = rest.get(read).map(|w| w.text);
        read += 1;
    }
    if let Some(year) = rest.get(read).and_then(|w| parse_year(w.text)) {
        date.year = Some(year.year);
        if let Some((dual, digits)) = year.dual {
            date.dual_year = Some(dual);
            dual_digits = digits;
        }
        if let Some((epoch, spelling)) = year.attached_epoch {
            date.epoch = Some(epoch);
            epoch_spelling = Some(spelling);
            epoch_attached = true;
        }
        read += 1;
        if date.epoch.is_none()
            && let Some((epoch, spelling)) = rest.get(read).and_then(|w| Epoch::from_word(w.text))
        {
            date.epoch = Some(epoch);
            epoch_spelling = Some(spelling);
            read += 1;
        }
    }
    let nothing_read = read == 0 && !rest.is_empty();
    if read < rest.len() || nothing_read {
        date.text = Some(ck.kept(span(text, rest)));
    }

    if let Some(grammar) = ck.grammar() {
        check_date(
            &date,
            grammar,
            ck,
            DateWords {
                day: day_word,
                month: month_word,
                dual_digits,
                epoch_spelling,
                epoch_attached,
            },
        );
    }
    date
}

/// How the parts of a date were written, for the strict checks.
#[derive(Clone, Copy)]
struct DateWords<'a> {
    day: Option<&'a str>,
    month: Option<&'a str>,
    dual_digits: usize,
    epoch_spelling: Option<Spelling>,
    epoch_attached: bool,
}

/// Checks a date read from `words` against `grammar`.
fn check_date(date: &CalendarDate, grammar: Grammar, ck: &mut Checker, words: DateWords<'_>) {
    if let Some(text) = &date.text {
        ck.fail(|| format!("{text:?} is not a date"));
        return;
    }
    let Some(year) = date.year else {
        ck.fail(|| "a date needs a year".to_string());
        return;
    };
    let calendar = &date.calendar;
    if calendar.is_defined() {
        ck.require(year >= 1, || "there is no year 0".to_string());
    }

    // The month: one of the calendar's, spelled as the version does.
    if let (Some(month), Some(word)) = (&date.month, words.month) {
        let open_months =
            grammar == Grammar::V551 && matches!(calendar, Calendar::Roman | Calendar::Unknown);
        ck.require(open_months || month.belongs_to(calendar), || {
            format!("{word:?} is not a month of the {calendar} calendar")
        });
        let spelled = match grammar {
            Grammar::V7 => word == month.tag(),
            Grammar::V551 => word.eq_ignore_ascii_case(month.tag()),
        };
        ck.require(spelled, || format!("{word:?} is not a month tag"));
    }

    // The day: within the month.
    if let (Some(day), Some(word)) = (date.day, words.day) {
        ck.require(grammar == Grammar::V7 || word.len() <= 2, || {
            format!("the day {word:?} has more than two digits")
        });
        ck.require(day >= 1, || "there is no day 0".to_string());
        // 7.0 holds a day to its month's length (§2.4: "a month-specific
        // maximum"). A 5.5.1 day is any `dd` up to 31: its readers keep
        // `31 NOV` as written, so a 5.5.1 file is not told otherwise.
        let max = match grammar {
            Grammar::V7 => date
                .month
                .as_ref()
                .and_then(|month| max_day(calendar, month, date.astronomical_year())),
            Grammar::V551 => Some(31),
        };
        if let Some(max) = max {
            ck.require(day <= max, || format!("the month has no day {day}"));
        }
    }

    // The dual year: 5.5.1 Gregorian only, the next year's last two digits.
    if let Some(dual) = date.dual_year {
        match grammar {
            Grammar::V7 => ck.fail(|| "GEDCOM 7.0 has no dual years".to_string()),
            Grammar::V551 => {
                ck.require(*calendar == Calendar::Gregorian, || {
                    "only Gregorian years can be dual".to_string()
                });
                ck.require(
                    words.dual_digits == 2 && Some(dual) == year.checked_add(1),
                    || {
                        format!(
                            "a dual year is the next year's last two digits, {:02}",
                            (u64::from(year) + 1) % 100
                        )
                    },
                );
            }
        }
    }

    // The epoch: BCE in the Gregorian and Julian calendars, extension
    // epochs in extension calendars.
    if let (Some(epoch), Some(spelling)) = (&date.epoch, words.epoch_spelling) {
        ck.require(spelling.is(grammar), || {
            format!("the epoch must be written {:?}", epoch.tag(grammar))
        });
        ck.require(grammar == Grammar::V551 || !words.epoch_attached, || {
            "the epoch must be separated from the year".to_string()
        });
        let allowed = match (epoch, grammar) {
            (Epoch::Bce, Grammar::V7) => calendar.has_bce(),
            (Epoch::Bce, Grammar::V551) => {
                calendar.has_bce()
                    || matches!(calendar, Calendar::Roman | Calendar::Unknown)
                    || (date.month.is_none() && calendar.is_defined())
            }
            (Epoch::Extension(_), Grammar::V7) => !calendar.is_defined(),
            (Epoch::Extension(_), Grammar::V551) => false,
        };
        ck.require(allowed, || {
            format!("the {calendar} calendar has no such epoch here")
        });
    }
}

/// A date payload, its `TIME` and its `PHRASE` as texts, which the model's
/// [`Date`](crate::model::Date) converts between versions and calendars
/// with.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DateTexts {
    /// The payload: a `DateValue`, `DateExact` or `DatePeriod` depending on
    /// where the date stands, as written.
    pub value: Option<String>,
    /// The `TIME` substructure, as written.
    pub time: Option<String>,
    /// The `PHRASE` substructure (GEDCOM 7.0): the date in the words of the
    /// source, when the payload cannot say it.
    pub phrase: Option<String>,
}

impl DateTexts {
    /// The payload, interpreted by the date grammar of both versions; an
    /// absent payload is [`DateValue::Empty`].
    #[cfg(any(test, feature = "calendar"))]
    pub(crate) fn date_value(&self) -> DateValue {
        self.value
            .as_deref()
            .map_or(DateValue::Empty, DateValue::parse)
    }

    /// The `TIME`, interpreted; `None` without one or when it is not a time.
    #[cfg(test)]
    pub(crate) fn time_value(&self) -> Option<Time> {
        self.time.as_deref().and_then(Time::parse)
    }

    /// The payload and the time in one string (`2 OCT 2019 12:00`), or the
    /// time alone without a payload; `None` without a time.
    #[cfg(test)]
    pub(crate) fn datetime(&self) -> Option<String> {
        let time = self.time.as_deref()?;
        Some(match self.value.as_deref().filter(|v| !v.is_empty()) {
            Some(value) => format!("{value} {time}"),
            None => time.to_string(),
        })
    }

    /// The date rewritten in the grammar of `version`, keeping everything it
    /// says.
    ///
    /// A payload that already follows the version's grammar is kept as
    /// written. Otherwise the payload is read and written back in the
    /// version's grammar:
    ///
    /// - to GEDCOM 7.0, calendar escapes become keywords (`@#DROMAN@` the
    ///   extension calendar `_ROMAN`), `B.C.` becomes `BCE`, `INT date
    ///   (phrase)` the date with the phrase as `PHRASE`, and `(phrase)` an
    ///   empty payload with the `PHRASE`. A dual year becomes one year (the
    ///   later one when it is the year that follows, as in `30 JAN 1648/49`,
    ///   and the date falls from January to March; the year written
    ///   otherwise, before the common era too), or a
    ///   `BET … AND …` range for a year alone,
    ///   and the original wording becomes the `PHRASE`;
    /// - to GEDCOM 5.5.1, a `PHRASE` that is the 5.5.1 date the payload was
    ///   converted from (a dual year) becomes the payload again; otherwise
    ///   keywords become escapes, `BCE` becomes `B.C.`, and
    ///   the `PHRASE` moves into the payload, as `INT date (phrase)` after a
    ///   single date or `(phrase)` alone. Next to a range or a period it
    ///   stays in [`phrase`](Self::phrase), which 5.5.1 cannot write.
    ///
    /// A payload the grammar cannot fully read, or that the version cannot
    /// express, is left as written, to be repaired by the writer. The time
    /// is rewritten likewise; GEDCOM 5.5.1 has no `Z` for UTC.
    pub(crate) fn to_version(&self, version: GedcomVersion) -> DateTexts {
        self.convert(Grammar::from(version))
    }

    /// [`to_version`](Self::to_version) in `grammar`.
    fn convert(&self, grammar: Grammar) -> DateTexts {
        let time = self.time.as_ref().map(|raw| convert_time(raw, grammar));
        let unchanged = || DateTexts {
            value: self.value.clone(),
            time: time.clone(),
            phrase: self.phrase.clone(),
        };
        let raw = self
            .value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let phrase = self.phrase.as_deref().filter(|p| !p.is_empty());
        match grammar {
            Grammar::V7 => {
                let Some(raw) = raw else {
                    return unchanged();
                };
                if DateValue::is_strict(raw, grammar) {
                    return unchanged();
                }
                let value = DateValue::parse(raw);
                if value.has_unrecognised_text() {
                    return unchanged();
                }
                let has_dual_year = value.dates().any(|d| d.dual_year.is_some());
                let single = matches!(value, DateValue::Date(_));
                let (mut value, mut carried) = match value {
                    DateValue::Phrase(text) => (DateValue::Empty, Some(text)),
                    DateValue::Interpreted { date, phrase } => (DateValue::Date(date), phrase),
                    value => (value, None),
                };
                if has_dual_year {
                    carried = Some(raw.to_string());
                    value = resolve_dual_years(value, single);
                }
                let payload = value.format(grammar);
                if !DateValue::is_strict(&payload, grammar) {
                    return unchanged();
                }
                let phrase = match (phrase, carried) {
                    (Some(own), Some(carried)) if own != carried => return unchanged(),
                    (Some(own), _) => Some(own.to_string()),
                    (None, carried) => carried,
                };
                DateTexts {
                    value: (!payload.is_empty()).then_some(payload),
                    time,
                    phrase,
                }
            }
            Grammar::V551 => {
                if let Some(original) = converted_from(raw, phrase) {
                    return DateTexts {
                        value: Some(original.to_string()),
                        time,
                        phrase: None,
                    };
                }
                let value = match raw {
                    None => DateValue::Empty,
                    Some(raw) => {
                        if phrase.is_none() && DateValue::is_strict(raw, grammar) {
                            return unchanged();
                        }
                        let value = DateValue::parse(raw);
                        if value.has_unrecognised_text() {
                            return unchanged();
                        }
                        value
                    }
                };
                let (value, leftover) = match (value, phrase) {
                    (DateValue::Empty, None) => return unchanged(),
                    (DateValue::Empty, Some(text)) => (DateValue::Phrase(text.to_string()), None),
                    (
                        DateValue::Date(date) | DateValue::Interpreted { date, phrase: None },
                        Some(text),
                    ) => (
                        DateValue::Interpreted {
                            date,
                            phrase: Some(text.to_string()),
                        },
                        None,
                    ),
                    (value, phrase) => (value, phrase.map(str::to_string)),
                };
                let payload = value.format(grammar);
                if !DateValue::is_strict(&payload, grammar) {
                    return unchanged();
                }
                DateTexts {
                    value: Some(payload),
                    time,
                    phrase: leftover,
                }
            }
        }
    }

    /// The date in the canonical spelling of `version`: converted as by
    /// [`to_version`](Self::to_version), then written back from its
    /// interpretation, upper case and single-spaced. Wording the grammar
    /// does not understand is kept as written.
    pub(crate) fn normalize(&self, version: GedcomVersion) -> DateTexts {
        let grammar = Grammar::from(version);
        let converted = self.convert(grammar);
        let v7 = grammar == Grammar::V7;
        let value = converted.value.as_deref().map(|raw| {
            let value = DateValue::parse(raw);
            // A 7.0 payload has no room for a phrase that could not move.
            if v7 && value.phrase().is_some() {
                raw.to_string()
            } else {
                value.format(grammar)
            }
        });
        let time = converted
            .time
            .map(|raw| Time::parse(&raw).map_or(raw, |time| time.format(grammar)));
        DateTexts {
            value,
            time,
            phrase: converted.phrase,
        }
    }

    /// The date with every date of its payload converted to `calendar`,
    /// written in the grammar of `version`; qualifiers, ranges, periods,
    /// the time and the phrase are kept.
    ///
    /// # Errors
    ///
    /// Returns a [`CalendarError`] when a date of the payload cannot be
    /// converted: it is incomplete, holds unrecognised words, is invalid or
    /// out of range, or its calendar has no arithmetic.
    #[cfg(feature = "calendar")]
    pub(crate) fn convert_to(
        &self,
        calendar: &Calendar,
        version: GedcomVersion,
    ) -> Result<DateTexts, super::CalendarError> {
        let value = self.date_value().convert_to(calendar)?;
        let payload = value.format(Grammar::V551);
        let converted = DateTexts {
            value: (!payload.is_empty()).then_some(payload),
            time: self.time.clone(),
            phrase: self.phrase.clone(),
        };
        Ok(converted.to_version(version))
    }
}

/// The time in `grammar`: kept when it follows it, rewritten when it is a
/// time, left as written otherwise.
fn convert_time(raw: &str, grammar: Grammar) -> String {
    if Time::is_strict(raw, grammar) {
        return raw.to_string();
    }
    Time::parse(raw).map_or_else(|| raw.to_string(), |time| time.format(grammar))
}

/// The 5.5.1 date a 7.x date (`raw`, `phrase`) was converted from, when
/// its phrase is that date: a dual year (`1699/00` became
/// `BET 1699 AND 1700` with the phrase `1699/00`).
fn converted_from<'p>(raw: Option<&str>, phrase: Option<&'p str>) -> Option<&'p str> {
    let original = phrase.filter(|p| DateValue::is_strict(p, Grammar::V551))?;
    let back = DateTexts {
        value: Some(original.to_string()),
        time: None,
        phrase: None,
    }
    .convert(Grammar::V7);
    (back.value.as_deref() == raw && back.phrase.as_deref() == Some(original)).then_some(original)
}

/// Replaces dual years, which GEDCOM 7.0 does not have: a `single` date
/// that is a year alone becomes the range of its two years; otherwise the
/// later year is kept where the migration guide takes it
/// ([`takes_later_year`]), and the year written elsewhere.
fn resolve_dual_years(value: DateValue, single: bool) -> DateValue {
    if let (DateValue::Date(date), true) = (&value, single)
        && let (None, None, None, Some(dual)) = (date.day, &date.month, &date.epoch, date.dual_year)
    {
        let first = CalendarDate {
            dual_year: None,
            ..date.clone()
        };
        let second = CalendarDate {
            year: Some(dual),
            ..first.clone()
        };
        return DateValue::Between(first, second);
    }
    let mut value = value;
    for date in value.dates_mut() {
        if let Some(dual) = date.dual_year.take()
            && takes_later_year(date, dual)
        {
            date.year = Some(dual);
        }
    }
    value
}

/// Whether a date with a dual year is the later of its years in 7.x (the
/// migration guide: `30 JAN 1648/49` is `30 JAN 1649`): when the dual year
/// is the year that follows (the old and the new style of counting from
/// 25 March) and the date falls where the two differ, from January to
/// March. A year alone, a year counted before the common era, or a dual
/// year of any other shape keeps the year written: what the slash meant is
/// the `PHRASE`'s to say.
fn takes_later_year(date: &CalendarDate, dual: u32) -> bool {
    date.year.and_then(|year| year.checked_add(1)) == Some(dual)
        && date.epoch.is_none()
        && date
            .month
            .as_ref()
            .is_some_and(|m| matches!(m, Month::Jan | Month::Feb | Month::Mar))
}

#[cfg(test)]
mod tests {
    use super::*;

    const V551: GedcomVersion = GedcomVersion::V5_5_1;
    const V7: GedcomVersion = GedcomVersion::V7_0;

    fn julian(year: u32) -> CalendarDate {
        CalendarDate {
            calendar: Calendar::Julian,
            year: Some(year),
            ..CalendarDate::default()
        }
    }

    #[test]
    fn test_between_with_a_calendar_per_date() {
        let value = DateValue::parse("BET @#DJULIAN@ 1700 AND @#DJULIAN@ 1710");
        assert_eq!(value, DateValue::Between(julian(1700), julian(1710)));
        assert_eq!(value.calendar(), Calendar::Julian);
        assert_eq!(
            value.to_gedcom(V551),
            "BET @#DJULIAN@ 1700 AND @#DJULIAN@ 1710"
        );
        assert_eq!(value.to_gedcom(V7), "BET JULIAN 1700 AND JULIAN 1710");
    }

    #[test]
    fn test_mixed_calendars_name_the_gregorian_one_in_gedcom_7() {
        let value = DateValue::parse("FROM @#DJULIAN@ 1700 TO 1752");
        let DateValue::Period(DatePeriod {
            from: Some(from),
            to: Some(to),
        }) = &value
        else {
            panic!("not a period: {value:?}");
        };
        assert_eq!(from.calendar, Calendar::Julian);
        assert_eq!(to.calendar, Calendar::Gregorian);
        assert_eq!(value.to_gedcom(V7), "FROM JULIAN 1700 TO GREGORIAN 1752");
        assert_eq!(value.to_gedcom(V551), "FROM @#DJULIAN@ 1700 TO 1752");
    }

    #[test]
    fn test_leading_escape_applies_to_every_date() {
        let value = DateValue::parse("@#DFRENCH R@ BET 1 VEND 2 AND 3 BRUM 2");
        let DateValue::Between(start, end) = &value else {
            panic!("not a range: {value:?}");
        };
        assert_eq!(start.calendar, Calendar::FrenchRepublican);
        assert_eq!(end.calendar, Calendar::FrenchRepublican);
        assert_eq!(end.month, Some(Month::Brum));
        assert!(DateValue::parse_strict("@#DFRENCH R@ BET 1 VEND 2 AND 3 BRUM 2", V551).is_err());
    }

    #[test]
    fn test_qualifiers_ranges_and_periods() {
        for (text, expected) in [
            ("ABT 1700", "ABT 1700"),
            ("cal 1 jan 1700", "CAL 1 JAN 1700"),
            ("est   1700", "EST 1700"),
            ("BEF 1700", "BEF 1700"),
            ("aft 1700", "AFT 1700"),
            ("FROM 1700", "FROM 1700"),
            ("TO 1700", "TO 1700"),
            ("from 1904 to 1915", "FROM 1904 TO 1915"),
        ] {
            assert_eq!(DateValue::parse(text).to_gedcom(V7), expected, "{text}");
        }
    }

    #[test]
    fn test_interpreted_and_phrase() {
        let value = DateValue::parse("INT 1 JAN 1900 (New Year's day 1900)");
        let DateValue::Interpreted { date, phrase } = &value else {
            panic!("not interpreted: {value:?}");
        };
        assert_eq!(date.day, Some(1));
        assert_eq!(phrase.as_deref(), Some("New Year's day 1900"));
        assert_eq!(
            value.to_gedcom(V551),
            "INT 1 JAN 1900 (New Year's day 1900)"
        );
        assert_eq!(value.to_gedcom(V7), "1 JAN 1900");
        assert_eq!(value.phrase(), Some("New Year's day 1900"));

        let value = DateValue::parse("(the year of the flood)");
        assert_eq!(value, DateValue::Phrase("the year of the flood".into()));
        assert_eq!(value.to_gedcom(V551), "(the year of the flood)");
        assert_eq!(value.to_gedcom(V7), "");

        // Without its phrase, as some programs write it.
        let value = DateValue::parse("INT 1800");
        assert_eq!(value.to_gedcom(V551), "INT 1800");
        assert!(DateValue::parse_strict("INT 1800", V551).is_err());
    }

    #[test]
    fn test_unreadable_values_keep_their_wording() {
        for text in [
            "BET 1700",
            "FROM",
            "garbage",
            "vers 1850",
            "BET AND 1700",
            "FROM 1 TO",
        ] {
            let value = DateValue::parse(text);
            assert!(value.has_unrecognised_text(), "{text}: {value:?}");
            assert_eq!(value.to_gedcom(V551), text);
        }
        assert_eq!(DateValue::parse(""), DateValue::Empty);
        assert_eq!(DateValue::parse("   "), DateValue::Empty);
    }

    #[test]
    fn test_roman_and_unknown_keep_their_year() {
        let date = CalendarDate::parse("@#DROMAN@ 1860");
        assert_eq!(date.calendar, Calendar::Roman);
        assert_eq!(date.year, Some(1860));
        assert_eq!(date.to_gedcom(V7), "_ROMAN 1860");
        let date = CalendarDate::parse("@#DUNKNOWN@ 1871");
        assert_eq!(date.calendar, Calendar::Unknown);
        assert_eq!(date.year, Some(1871));
        assert_eq!(date.to_gedcom(V551), "@#DUNKNOWN@ 1871");
    }

    #[test]
    fn test_month_of_another_calendar_is_kept() {
        let date = CalendarDate::parse("@#DHEBREW@ 1 JAN 5600");
        assert_eq!(date.calendar, Calendar::Hebrew);
        assert_eq!(date.month, Some(Month::Jan));
        assert_eq!(date.year, Some(5600));
        assert_eq!(date.text, None);
        assert!(DateValue::parse_strict("@#DHEBREW@ 1 JAN 5600", V551).is_err());
    }

    #[test]
    fn test_extension_calendars() {
        let value = DateValue::parse("_MYCAL 12 _MON 1500");
        let DateValue::Date(date) = &value else {
            panic!("{value:?}")
        };
        assert_eq!(date.calendar, Calendar::Extension("_MYCAL".into()));
        assert_eq!(date.day, Some(12));
        assert_eq!(date.month, Some(Month::Extension("_MON".into())));
        assert_eq!(date.year, Some(1500));
        assert!(DateValue::parse_strict("_MYCAL 12 _MON 1500", V7).is_ok());
        assert!(DateValue::parse_strict("_MYCAL 1500 _AUC", V7).is_ok());
        // An extension calendar defines its months: any tag names one
        // (7.0 §2.4: `month = stdTag / extTag`, constrained by the calendar).
        let value = DateValue::parse("_MAYAN 1 POP 1");
        let DateValue::Date(date) = &value else {
            panic!("{value:?}")
        };
        assert_eq!(date.day, Some(1));
        assert_eq!(date.month, Some(Month::Extension("POP".into())));
        assert_eq!(date.year, Some(1));
        assert_eq!(date.text, None);
        assert_eq!(value.to_gedcom(V7), "_MAYAN 1 POP 1");
        assert!(DateValue::parse_strict("_MAYAN 1 POP 1", V7).is_ok());
        assert!(DateValue::parse_strict("_MYCAL 12 JAN 1500", V7).is_ok());
        assert!(DateValue::parse_strict("_MAYAN POP 1", V7).is_ok());
        // Not a month without a year after it, nor in lower case.
        assert!(DateValue::parse("_MAYAN 1 POP").has_unrecognised_text());
        assert!(DateValue::parse_strict("_MAYAN 1 pop 1", V7).is_err());
        assert!(DateValue::parse_strict("1 POP 1", V7).is_err());
        assert!(DateValue::parse_strict("_MYCAL 12 _MON 1500", V551).is_err());
        assert!(DateValue::parse_strict("12 _MON 1500", V7).is_err());
    }

    #[test]
    fn test_epochs_per_version() {
        let date = CalendarDate::parse("44 B.C.");
        assert_eq!(date.year, Some(44));
        assert_eq!(date.epoch, Some(Epoch::Bce));
        assert_eq!(date.astronomical_year(), Some(-43));
        assert_eq!(date.to_gedcom(V551), "44 B.C.");
        assert_eq!(date.to_gedcom(V7), "44 BCE");
        assert_eq!(CalendarDate::parse("20B.C.").epoch, Some(Epoch::Bce));
        assert_eq!(CalendarDate::parse("1 BCE").astronomical_year(), Some(0));

        assert!(DateValue::parse_strict("@#DGREGORIAN@ 20 B.C.", V551).is_ok());
        assert!(DateValue::parse_strict("20B.C.", V551).is_ok());
        assert!(DateValue::parse_strict("20 BCE", V551).is_err());
        assert!(DateValue::parse_strict("GREGORIAN 20 BCE", V7).is_ok());
        assert!(DateValue::parse_strict("20 B.C.", V7).is_err());
        assert!(DateValue::parse_strict("HEBREW 1 TSH 1 BCE", V7).is_err());
        assert!(DateValue::parse_strict("AFT @#DHEBREW@ 1 TSH 1 B.C.", V551).is_err());
        assert!(DateValue::parse_strict("@#DHEBREW@ 1 B.C.", V551).is_ok());
    }

    #[test]
    fn test_dual_years() {
        let date = CalendarDate::parse("15 APR 1699/00");
        assert_eq!(date.year, Some(1699));
        assert_eq!(date.dual_year, Some(1700));
        assert_eq!(date.to_gedcom(V551), "15 APR 1699/00");
        assert_eq!(date.to_gedcom(V7), "15 APR 1699");
        assert_eq!(CalendarDate::parse("1401/8").dual_year, Some(1408));
        assert_eq!(CalendarDate::parse("1701/99").dual_year, Some(1799));
        assert_eq!(CalendarDate::parse("1799/97").dual_year, Some(1897));
        let far = CalendarDate::parse("5/0000");
        assert_eq!(far.dual_year, Some(10_000));
        assert_eq!(far.to_gedcom(V551), "5/0000");
        assert_eq!(CalendarDate::parse("1699/1700").to_gedcom(V551), "1699/00");

        assert!(DateValue::parse_strict("1740/41", V551).is_ok());
        assert!(DateValue::parse_strict("1701/99", V551).is_err());
        assert!(DateValue::parse_strict("1751/9", V551).is_err());
        assert!(DateValue::parse_strict("@#DJULIAN@ 1740/41", V551).is_err());
        assert!(DateValue::parse_strict("1740/41", V7).is_err());

        // Not a dual year: nothing is read.
        let date = CalendarDate::parse("7/11/1959");
        assert_eq!(date.year, None);
        assert_eq!(date.text.as_deref(), Some("7/11/1959"));
    }

    #[test]
    fn test_strict_days_and_case() {
        assert!(DateValue::parse_strict("29 FEB 2000", V7).is_ok());
        assert!(DateValue::parse_strict("29 FEB 1900", V7).is_err());
        assert!(DateValue::parse_strict("@#DJULIAN@ 29 FEB 1900", V551).is_ok());
        assert!(DateValue::parse_strict("40 DEC 2023", V7).is_err());
        assert!(DateValue::parse_strict("0 DEC 2023", V7).is_err());
        assert!(DateValue::parse_strict("03 DEC 2023", V7).is_ok());
        assert!(DateValue::parse_strict("003 DEC 2023", V551).is_err());
        assert!(DateValue::parse_strict("3 dec 2023", V7).is_err());
        assert!(DateValue::parse_strict("3 dec 2023", V551).is_ok());
        assert!(DateValue::parse_strict("bef 2023", V7).is_err());
        assert!(DateValue::parse_strict("bef 2023", V551).is_ok());
        assert!(DateValue::parse_strict("3 JUNE 2023", V551).is_err());
        assert!(DateValue::parse_strict("1 JAN", V551).is_err());
        assert!(DateValue::parse_strict("0", V7).is_err());
        assert!(DateValue::parse_strict("1900  ", V7).is_err());
        assert!(DateValue::parse_strict("ABT  1900", V7).is_err());
        assert!(DateValue::parse_strict("", V7).is_ok());
        assert!(DateValue::parse_strict("", V551).is_err());
        assert!(DateValue::parse_strict("JULIAN 1700", V551).is_err());
        assert!(DateValue::parse_strict("@#DJULIAN@ 1700", V7).is_err());
        assert!(DateValue::parse_strict("FRENCH_R 13 COMP 3", V7).is_err());
        assert!(DateValue::parse_strict("FRENCH_R 6 COMP 3", V7).is_ok());
        // A 5.5.1 day goes up to 31 in any month; 7.0 holds it to the month.
        assert!(DateValue::parse_strict("@#DFRENCH R@ 13 COMP 3", V551).is_ok());
        assert!(DateValue::parse_strict("31 NOV 1900", V551).is_ok());
        assert!(DateValue::parse_strict("31 NOV 1900", V7).is_err());
        assert!(DateValue::parse_strict("30 NOV 1900", V7).is_ok());
        assert!(DateValue::parse_strict("32 JAN 1900", V551).is_err());
    }

    #[test]
    fn test_strict_interpreted_spacing() {
        assert!(DateValue::parse_strict("INT 1800 (about  1800)", V551).is_ok());
        assert!(DateValue::parse_strict("INT 1800  (x)", V551).is_err());
        assert!(DateValue::parse_strict("INT 1800(x)", V551).is_err());
        assert!(DateValue::parse_strict("INT 1800 ()", V551).is_err());
        assert!(DateValue::parse_strict("INT 1800 (x", V551).is_err());
        assert!(DateValue::parse_strict("(x)", V551).is_ok());
        assert!(DateValue::parse_strict("()", V551).is_err());
        assert!(DateValue::parse_strict("(x)", V7).is_err());
    }

    #[test]
    fn test_periods_and_exact_dates() {
        assert_eq!(
            DatePeriod::parse_strict("FROM 1900 TO 1910", V7)
                .unwrap()
                .to_gedcom(V7),
            "FROM 1900 TO 1910"
        );
        assert!(DatePeriod::parse_strict("", V7).is_ok());
        assert!(DatePeriod::parse_strict("2023", V7).is_err());
        assert!(DatePeriod::parse_strict("BET 1900 AND 1910", V551).is_err());
        assert_eq!(
            DatePeriod::parse("from 1900"),
            Some(DatePeriod {
                from: Some(CalendarDate {
                    year: Some(1900),
                    ..CalendarDate::default()
                }),
                to: None,
            })
        );
        assert_eq!(DatePeriod::parse("ABT 1900"), None);

        let exact = DateExact::parse_strict("03 DEC 2023", V7).unwrap();
        assert_eq!(
            (exact.day, exact.month.clone(), exact.year),
            (3, Month::Dec, 2023)
        );
        assert_eq!(exact.to_gedcom(V7), "3 DEC 2023");
        for invalid in [
            "invalid",
            "3 dec 2023",
            "3 JUNE 2023",
            "DEC 2023",
            "2023",
            "GREGORIAN 3 DEC 2023",
            "3 DEC 20 BCE",
        ] {
            assert!(DateExact::parse_strict(invalid, V7).is_err(), "{invalid}");
        }
        assert!(DateExact::parse_strict("2 oct 2019", V551).is_ok());
        assert_eq!(DateExact::parse("2 Oct 2019").map(|e| e.year), Some(2019));
        assert_eq!(DateExact::parse("ABT 2 OCT 2019"), None);
    }

    #[test]
    fn test_errors_say_why() {
        let error = DateValue::parse_strict("40 DEC 2023", V7).unwrap_err();
        assert_eq!(error.expected, "DateValue");
        assert_eq!(error.text, "40 DEC 2023");
        assert!(error.to_string().contains("no day 40"), "{error}");
    }
}

#[cfg(test)]
mod texts_tests {
    use super::{DateTexts as Date, DateValue};
    use crate::GedcomVersion;

    const V551: GedcomVersion = GedcomVersion::V5_5_1;
    const V7: GedcomVersion = GedcomVersion::V7_0;

    fn date(value: &str) -> Date {
        Date {
            value: Some(value.to_string()),
            ..Date::default()
        }
    }

    fn with_phrase(value: Option<&str>, phrase: &str) -> Date {
        Date {
            value: value.map(str::to_string),
            time: None,
            phrase: Some(phrase.to_string()),
        }
    }

    #[test]
    fn test_calendar_after_qualifier_and_gedcom_7_keyword() {
        use crate::value::Calendar;
        for (value, calendar) in [
            ("ABT @#DJULIAN@ 1700", Calendar::Julian),
            ("BET @#DJULIAN@ 1700 AND @#DJULIAN@ 1710", Calendar::Julian),
            ("JULIAN 10 MAY 1700", Calendar::Julian),
            ("FRENCH_R 1 VEND 2", Calendar::FrenchRepublican),
            ("@#DJULIAN@ ABT 1700", Calendar::Julian),
            ("ABT 1700", Calendar::Gregorian),
            ("@#DROMAN@ 1700", Calendar::Roman),
        ] {
            assert_eq!(date(value).date_value().calendar(), calendar, "{value}");
        }
        // The 5.5.1 escape is written after the qualifier.
        assert_eq!(
            date("@#DJULIAN@ ABT 1700").normalize(V551).value.as_deref(),
            Some("ABT @#DJULIAN@ 1700")
        );
    }

    #[test]
    fn test_datetime_never_panics() {
        assert_eq!(date("2 OCT 2019").datetime(), None);
        let with_time = Date {
            value: Some("2 OCT 2019".into()),
            time: Some("12:00".into()),
            phrase: None,
        };
        assert_eq!(with_time.datetime().as_deref(), Some("2 OCT 2019 12:00"));
        let time_only = Date {
            time: Some("12:00".into()),
            ..Date::default()
        };
        assert_eq!(time_only.datetime().as_deref(), Some("12:00"));
    }

    #[test]
    fn test_to_gedcom_7() {
        for (value, expected, phrase) in [
            ("2 OCT 2019", Some("2 OCT 2019"), None),
            ("@#DJULIAN@ 15 MAR 1582", Some("JULIAN 15 MAR 1582"), None),
            (
                "ABT @#DHEBREW@ 1 TSH 5600",
                Some("ABT HEBREW 1 TSH 5600"),
                None,
            ),
            ("44 B.C.", Some("44 BCE"), None),
            ("@#DROMAN@ 1860", Some("_ROMAN 1860"), None),
            ("from 1900 to 1905", Some("FROM 1900 TO 1905"), None),
            (
                "(the year of the flood)",
                None,
                Some("the year of the flood"),
            ),
            ("INT 1850 (about 1850)", Some("1850"), Some("about 1850")),
            (
                "30 JAN 1648/49",
                Some("30 JAN 1649"),
                Some("30 JAN 1648/49"),
            ),
            ("8 MAR 1401/8", Some("8 MAR 1401"), Some("8 MAR 1401/8")),
            // Outside January to March, both styles count the same year;
            // before the common era, or for a year alone, the year written.
            ("NOV 1042/43", Some("NOV 1042"), Some("NOV 1042/43")),
            (
                "21 FEB 366/7 B.C.",
                Some("21 FEB 366 BCE"),
                Some("21 FEB 366/7 B.C."),
            ),
            ("ABT 1648/49", Some("ABT 1648"), Some("ABT 1648/49")),
            (
                "FROM 2 FEB 1700/01 TO 1710",
                Some("FROM 2 FEB 1701 TO 1710"),
                Some("FROM 2 FEB 1700/01 TO 1710"),
            ),
            ("1699/00", Some("BET 1699 AND 1700"), Some("1699/00")),
            ("1401/8 B.C.", Some("1401 BCE"), Some("1401/8 B.C.")),
            (
                "INT 799/81 (some text)",
                Some("799"),
                Some("INT 799/81 (some text)"),
            ),
            // Not convertible: left as written.
            ("vers 1850", Some("vers 1850"), None),
        ] {
            let converted = date(value).to_version(V7);
            assert_eq!(converted.value.as_deref(), expected, "{value}");
            assert_eq!(converted.phrase.as_deref(), phrase, "{value}");
        }
        // An own PHRASE wins over an equal one; two different ones cannot
        // both be kept.
        let both = with_phrase(Some("(flood)"), "flood").to_version(V7);
        assert_eq!((both.value, both.phrase.as_deref()), (None, Some("flood")));
        let clash = with_phrase(Some("(flood)"), "storm").to_version(V7);
        assert_eq!(clash.value.as_deref(), Some("(flood)"));
    }

    #[test]
    fn test_to_gedcom_551() {
        for (value, expected) in [
            ("2 OCT 2019", "2 OCT 2019"),
            ("JULIAN 15 MAR 1582", "@#DJULIAN@ 15 MAR 1582"),
            (
                "BET GREGORIAN 20 BCE AND JULIAN 12 BCE",
                "BET 20 B.C. AND @#DJULIAN@ 12 B.C.",
            ),
            ("_ROMAN 1860", "@#DROMAN@ 1860"),
            ("2 oct 2019", "2 oct 2019"),
            ("< 1900", "< 1900"),
            ("_MYCAL 12 _MON 1500", "_MYCAL 12 _MON 1500"),
        ] {
            assert_eq!(
                date(value).to_version(V551).value.as_deref(),
                Some(expected),
                "{value}"
            );
        }
        let interpreted = with_phrase(Some("15 MAR 1820"), "The Ides of March").to_version(V551);
        assert_eq!(
            interpreted.value.as_deref(),
            Some("INT 15 MAR 1820 (The Ides of March)")
        );
        assert_eq!(interpreted.phrase, None);
        let phrase_only = with_phrase(None, "the year of the flood").to_version(V551);
        assert_eq!(
            phrase_only.value.as_deref(),
            Some("(the year of the flood)")
        );
        let range = with_phrase(Some("BET 1820 AND 1825"), "in his youth").to_version(V551);
        assert_eq!(range.value.as_deref(), Some("BET 1820 AND 1825"));
        assert_eq!(range.phrase.as_deref(), Some("in his youth"));
        // A dual year converted to 7.x converts back.
        for dual in ["1699/00", "30 JAN 1648/49", "FROM 2 FEB 1700/01 TO 1710"] {
            let v7 = date(dual).to_version(V7);
            let back = with_phrase(v7.value.as_deref(), v7.phrase.as_deref().unwrap_or(""))
                .to_version(V551);
            assert_eq!(
                (back.value.as_deref(), back.phrase),
                (Some(dual), None),
                "{dual}"
            );
        }
        // A phrase that is a date, but not the one converted, stays a phrase.
        let other = with_phrase(Some("BET 1699 AND 1700"), "1650/51").to_version(V551);
        assert_eq!(other.phrase.as_deref(), Some("1650/51"));
    }

    #[test]
    fn test_time_per_version() {
        let date = Date {
            value: Some("1 DEC 2023".into()),
            time: Some("2:50:00.00Z".into()),
            phrase: None,
        };
        assert_eq!(date.to_version(V7).time.as_deref(), Some("2:50:00.00Z"));
        assert_eq!(date.to_version(V551).time.as_deref(), Some("02:50:00.00"));
        let noon = Date {
            time: Some("noon".into()),
            ..Date::default()
        };
        assert_eq!(noon.to_version(V7).time.as_deref(), Some("noon"));
        assert_eq!(noon.time_value(), None);
    }

    #[test]
    fn test_normalize() {
        for (input, expected) in [
            ("15 mar 1820", "15 MAR 1820"),
            ("@#djulian@ 15 mar 1582", "@#DJULIAN@ 15 MAR 1582"),
            ("15  MAR   1820", "15 MAR 1820"),
            ("abt vers 1850", "ABT vers 1850"),
        ] {
            assert_eq!(
                date(input).normalize(V551).value.as_deref(),
                Some(expected),
                "{input}"
            );
        }
        let date = Date {
            value: Some("15 mar 1820".to_string()),
            time: Some("12:34:56".to_string()),
            phrase: Some("The Ides of March".to_string()),
        };
        let normalized = date.normalize(V7);
        assert_eq!(normalized.value.as_deref(), Some("15 MAR 1820"));
        assert_eq!(normalized.time.as_deref(), Some("12:34:56"));
        assert_eq!(normalized.phrase.as_deref(), Some("The Ides of March"));
        assert_eq!(Date::default().normalize(V7), Date::default());
        assert_eq!(Date::default().date_value(), DateValue::Empty);
    }

    #[cfg(feature = "calendar")]
    #[test]
    fn test_convert_to() {
        use crate::value::{Calendar, CalendarError};
        let julian = date("ABT @#DJULIAN@ 15 MAR 1582");
        let converted = julian.convert_to(&Calendar::Gregorian, V7).unwrap();
        assert_eq!(converted.value.as_deref(), Some("ABT 25 MAR 1582"));
        let back = converted.convert_to(&Calendar::Julian, V551).unwrap();
        assert_eq!(back.value.as_deref(), Some("ABT @#DJULIAN@ 15 MAR 1582"));
        assert_eq!(
            date("1582").convert_to(&Calendar::Julian, V7),
            Err(CalendarError::Incomplete)
        );
    }
}
