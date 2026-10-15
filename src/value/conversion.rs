//! Calendar arithmetic: day numbers, conversions between calendars, days of
//! the week and chronological comparison (the `calendar` feature).
//!
//! Every date is turned into a Rata Die, a count of days from 1 January 1 of
//! the proleptic Gregorian calendar, and back. Years are bounded to
//! [`MAX_YEAR`] on either side of the epoch before any arithmetic, so that no
//! input can make a conversion overflow or run for long.

use std::cmp::Ordering;
use std::fmt;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use calendrical_calculations::hebrew::BookHebrew;
use calendrical_calculations::rata_die::RataDie;

use super::calendar::{Calendar, Epoch, Month, max_day};
use super::date::{CalendarDate, DatePeriod, DateValue};

/// The largest year, before or after the epoch, calendar arithmetic accepts.
pub const MAX_YEAR: u32 = 1_000_000;

/// The largest Rata Die, in absolute value, calendar arithmetic accepts or
/// returns: about 990,000 years on either side of the common era, the
/// range in which every calendar's years stay within [`MAX_YEAR`] (Hebrew
/// years run 3,760 years ahead).
pub const MAX_RATA_DIE: i64 = 363_000_000;

/// Offset from a Rata Die to a Julian Day Number.
const RATA_DIE_TO_JDN_OFFSET: i64 = 1_721_425;

/// Rata Die of 1 Vendémiaire An I (22 September 1792 Gregorian), from which
/// `calendrier` counts Republican days.
///
/// Going through `calendrier`'s `chrono` conversions instead would apply the
/// Paris-versus-Greenwich clock offset it uses, which pushes the start of a
/// Republican day 18 minutes into the previous Gregorian day.
const FRENCH_REPUBLICAN_EPOCH_RD: i64 = 654_415;

/// The last French Republican year with arithmetic. Beyond about 100,000
/// years `calendrier` stops searching for the year of a day and estimates
/// it, so its dates no longer match its day counts.
const FRENCH_REPUBLICAN_MAX_YEAR: u32 = 90_000;

/// The last Rata Die converted to the French Republican calendar, within
/// [`FRENCH_REPUBLICAN_MAX_YEAR`].
const FRENCH_REPUBLICAN_MAX_RD: i64 = FRENCH_REPUBLICAN_EPOCH_RD + 365 * 90_000;

/// Seconds in a French Republican day (10 hours of 100 minutes of 100
/// seconds), the unit of `calendrier` timestamps.
const REPUBLICAN_SECONDS_PER_DAY: i64 = 100_000;

/// Why a date cannot be converted.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum CalendarError {
    /// The date lacks a day, a month or a year.
    Incomplete,
    /// The date holds words it does not model (see [`CalendarDate::text`]),
    /// which a converted date could not keep.
    UnrecognisedText(String),
    /// The date is not a day of its calendar (`31 VEND`, `29 FEB 1900`).
    InvalidDate,
    /// The year or day number is beyond [`MAX_YEAR`] or [`MAX_RATA_DIE`], or
    /// before the first day of the target calendar.
    OutOfRange,
    /// The calendar has no arithmetic: the Roman and unknown calendars of
    /// GEDCOM 5.5.1, and extension calendars.
    UnsupportedCalendar(Calendar),
}

impl fmt::Display for CalendarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalendarError::Incomplete => write!(f, "the date lacks a day, a month or a year"),
            CalendarError::UnrecognisedText(text) => {
                write!(f, "the date holds unrecognised words: {text}")
            }
            CalendarError::InvalidDate => write!(f, "the date is not a day of its calendar"),
            CalendarError::OutOfRange => write!(f, "the date is out of range"),
            CalendarError::UnsupportedCalendar(calendar) => {
                write!(f, "the {calendar} calendar has no arithmetic")
            }
        }
    }
}

impl std::error::Error for CalendarError {}

/// A day of the week.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Weekday {
    /// Monday.
    Monday,
    /// Tuesday.
    Tuesday,
    /// Wednesday.
    Wednesday,
    /// Thursday.
    Thursday,
    /// Friday.
    Friday,
    /// Saturday.
    Saturday,
    /// Sunday.
    Sunday,
}

impl CalendarDate {
    /// The date's Rata Die: days from 1 January 1 of the proleptic Gregorian
    /// calendar, which is day 1. Calendar-neutral, it compares and subtracts
    /// dates of any calendar.
    ///
    /// # Errors
    ///
    /// Returns a [`CalendarError`] when the date is incomplete, holds
    /// unrecognised words, is not a day of its calendar, is out of range
    /// (beyond [`MAX_YEAR`] or [`MAX_RATA_DIE`]) or is in a calendar without
    /// arithmetic.
    pub fn to_rata_die(&self) -> Result<i64, CalendarError> {
        let rata_die = self.unbounded_rata_die()?;
        if (-MAX_RATA_DIE..=MAX_RATA_DIE).contains(&rata_die) {
            Ok(rata_die)
        } else {
            Err(CalendarError::OutOfRange)
        }
    }

    /// The Rata Die of a date whose year is within [`MAX_YEAR`].
    fn unbounded_rata_die(&self) -> Result<i64, CalendarError> {
        if let Some(text) = &self.text {
            return Err(CalendarError::UnrecognisedText(text.clone()));
        }
        let (Some(day), Some(month), Some(year)) = (self.day, &self.month, self.year) else {
            return Err(CalendarError::Incomplete);
        };
        if !self.calendar.is_defined() {
            return Err(CalendarError::UnsupportedCalendar(self.calendar.clone()));
        }
        if year > MAX_YEAR {
            return Err(CalendarError::OutOfRange);
        }
        // No calendar has a year 0: 1 BCE precedes 1.
        if year == 0 {
            return Err(CalendarError::InvalidDate);
        }
        let number = month.number().ok_or(CalendarError::InvalidDate)?;
        let bce = match &self.epoch {
            None => false,
            Some(Epoch::Bce) if self.calendar.has_bce() => true,
            Some(_) => return Err(CalendarError::InvalidDate),
        };
        // `year <= MAX_YEAR`, so the astronomical year fits an `i32`.
        let astronomical = i32::try_from(super::calendar::astronomical_year(year, bce))
            .map_err(|_| CalendarError::OutOfRange)?;
        let valid_day = max_day(&self.calendar, month, Some(i64::from(astronomical)))
            .is_some_and(|max| (1..=max).contains(&day));

        match &self.calendar {
            Calendar::Gregorian | Calendar::Julian if valid_day => Ok(if self.calendar
                == Calendar::Gregorian
            {
                calendrical_calculations::gregorian::fixed_from_gregorian(astronomical, number, day)
            } else {
                calendrical_calculations::julian::fixed_from_julian(astronomical, number, day)
            }
            .to_i64_date()),
            Calendar::Hebrew if month.belongs_to(&Calendar::Hebrew) => {
                let book_month = hebrew_book_month(month, astronomical);
                if !(1..=BookHebrew::last_day_of_book_hebrew_month(astronomical, book_month))
                    .contains(&day)
                {
                    return Err(CalendarError::InvalidDate);
                }
                Ok(BookHebrew::fixed_from_book_hebrew(BookHebrew {
                    year: astronomical,
                    month: book_month,
                    day,
                })
                .to_i64_date())
            }
            Calendar::FrenchRepublican if valid_day => {
                french_republican_rata_die(year, number, day)
            }
            _ => Err(CalendarError::InvalidDate),
        }
    }

    /// The date's Julian Day Number, counted from 1 January 4713 BCE
    /// (Julian), as astronomy and much genealogy software do.
    ///
    /// # Errors
    ///
    /// As [`to_rata_die`](Self::to_rata_die).
    pub fn to_julian_day_number(&self) -> Result<i64, CalendarError> {
        self.to_rata_die().map(|rd| rd + RATA_DIE_TO_JDN_OFFSET)
    }

    /// The date of `calendar` whose Rata Die is `rata_die`.
    ///
    /// # Errors
    ///
    /// Returns [`CalendarError::OutOfRange`] beyond [`MAX_RATA_DIE`] or
    /// before the first day of a calendar that has no earlier years (Hebrew,
    /// French Republican), and [`CalendarError::UnsupportedCalendar`] for a
    /// calendar without arithmetic.
    pub fn from_rata_die(
        rata_die: i64,
        calendar: &Calendar,
    ) -> Result<CalendarDate, CalendarError> {
        if !(-MAX_RATA_DIE..=MAX_RATA_DIE).contains(&rata_die) {
            return Err(CalendarError::OutOfRange);
        }
        let fixed = RataDie::new(rata_die);
        let (astronomical, month, day) = match calendar {
            Calendar::Gregorian => calendrical_calculations::gregorian::gregorian_from_fixed(fixed)
                .map_err(|_| CalendarError::OutOfRange)?,
            Calendar::Julian => calendrical_calculations::julian::julian_from_fixed(fixed)
                .map_err(|_| CalendarError::OutOfRange)?,
            Calendar::Hebrew => {
                let date = BookHebrew::book_hebrew_from_fixed(fixed);
                let month = hebrew_gedcom_month(date.month, date.year);
                return year_from_astronomical(date.year, false).map(|(year, _)| CalendarDate {
                    calendar: Calendar::Hebrew,
                    day: Some(date.day),
                    month: Some(month),
                    year: Some(year),
                    ..CalendarDate::default()
                });
            }
            Calendar::FrenchRepublican => return french_republican_from_rata_die(rata_die),
            other => return Err(CalendarError::UnsupportedCalendar(other.clone())),
        };
        let (year, bce) = year_from_astronomical(astronomical, true)?;
        Ok(CalendarDate {
            calendar: calendar.clone(),
            day: Some(day),
            month: Month::of(calendar, month),
            year: Some(year),
            epoch: bce.then_some(Epoch::Bce),
            ..CalendarDate::default()
        })
    }

    /// The date of `calendar` whose Julian Day Number is `jdn`.
    ///
    /// # Errors
    ///
    /// As [`from_rata_die`](Self::from_rata_die).
    pub fn from_julian_day_number(
        jdn: i64,
        calendar: &Calendar,
    ) -> Result<CalendarDate, CalendarError> {
        let rata_die = jdn
            .checked_sub(RATA_DIE_TO_JDN_OFFSET)
            .ok_or(CalendarError::OutOfRange)?;
        CalendarDate::from_rata_die(rata_die, calendar)
    }

    /// The same day in `calendar`.
    ///
    /// ```
    /// use oxidged::value::{Calendar, CalendarDate};
    /// use oxidged::GedcomVersion;
    ///
    /// let julian = CalendarDate::parse("@#DJULIAN@ 5 OCT 1582");
    /// let gregorian = julian.convert_to(&Calendar::Gregorian).unwrap();
    /// assert_eq!(gregorian.to_gedcom(GedcomVersion::V7_0), "15 OCT 1582");
    /// ```
    ///
    /// # Errors
    ///
    /// As [`to_rata_die`](Self::to_rata_die) and
    /// [`from_rata_die`](Self::from_rata_die).
    pub fn convert_to(&self, calendar: &Calendar) -> Result<CalendarDate, CalendarError> {
        let rata_die = self.to_rata_die()?;
        if &self.calendar == calendar {
            return Ok(self.clone());
        }
        CalendarDate::from_rata_die(rata_die, calendar)
    }

    /// The number of days from this date to `other`: positive when `other`
    /// is later.
    ///
    /// # Errors
    ///
    /// As [`to_rata_die`](Self::to_rata_die), for either date.
    pub fn days_until(&self, other: &CalendarDate) -> Result<i64, CalendarError> {
        Ok(other.to_rata_die()? - self.to_rata_die()?)
    }

    /// The date `days` days later (earlier when negative), in the same
    /// calendar.
    ///
    /// # Errors
    ///
    /// As [`to_rata_die`](Self::to_rata_die) and
    /// [`from_rata_die`](Self::from_rata_die).
    pub fn add_days(&self, days: i64) -> Result<CalendarDate, CalendarError> {
        let rata_die = self
            .to_rata_die()?
            .checked_add(days)
            .ok_or(CalendarError::OutOfRange)?;
        CalendarDate::from_rata_die(rata_die, &self.calendar)
    }

    /// The day of the week.
    #[must_use]
    pub fn weekday(&self) -> Option<Weekday> {
        // Rata Die 1, 1 January 1, was a Monday.
        Some(match self.to_rata_die().ok()?.rem_euclid(7) {
            1 => Weekday::Monday,
            2 => Weekday::Tuesday,
            3 => Weekday::Wednesday,
            4 => Weekday::Thursday,
            5 => Weekday::Friday,
            6 => Weekday::Saturday,
            _ => Weekday::Sunday,
        })
    }

    /// A key that sorts dates chronologically across calendars: the Rata Die
    /// of the date, or of its first day when the day or the month is
    /// missing. `None` without a year, or when the date cannot be converted.
    #[must_use]
    pub fn ordering_key(&self) -> Option<i64> {
        self.year?;
        let first_day = CalendarDate {
            day: Some(self.day.unwrap_or(1)),
            month: self
                .month
                .clone()
                .or_else(|| self.calendar.months().first().cloned()),
            ..self.clone()
        };
        first_day.to_rata_die().ok()
    }

    /// Compares two dates chronologically, across calendars.
    ///
    /// Complete dates compare by their day; incomplete dates of one calendar
    /// compare by year, then month, then day, as far as both are known.
    /// `None` when the order is unknown: an incomplete date of another
    /// calendar, or one date more precise than the other within the same
    /// year or month.
    #[must_use]
    pub fn chronological_cmp(&self, other: &CalendarDate) -> Option<Ordering> {
        if let (Ok(a), Ok(b)) = (self.to_rata_die(), other.to_rata_die()) {
            return Some(a.cmp(&b));
        }
        if self.calendar != other.calendar || self.text.is_some() || other.text.is_some() {
            return None;
        }
        match self.astronomical_year()?.cmp(&other.astronomical_year()?) {
            Ordering::Equal => {}
            order => return Some(order),
        }
        match (self.month.as_ref(), other.month.as_ref()) {
            (None, None) => return Some(Ordering::Equal),
            (Some(a), Some(b)) => match a.number()?.cmp(&b.number()?) {
                Ordering::Equal => {}
                order => return Some(order),
            },
            _ => return None,
        }
        match (self.day, other.day) {
            (Some(a), Some(b)) => Some(a.cmp(&b)),
            (None, None) => Some(Ordering::Equal),
            _ => None,
        }
    }
}

impl DateValue {
    /// The value with every date converted to `calendar`; qualifiers,
    /// ranges, periods and phrases are kept.
    ///
    /// # Errors
    ///
    /// The first [`CalendarError`] of a date that cannot be converted.
    pub fn convert_to(&self, calendar: &Calendar) -> Result<DateValue, CalendarError> {
        let convert = |date: &CalendarDate| date.convert_to(calendar);
        Ok(match self {
            DateValue::Empty | DateValue::Phrase(_) => self.clone(),
            DateValue::Date(date) => DateValue::Date(convert(date)?),
            DateValue::Approximated(how, date) => DateValue::Approximated(*how, convert(date)?),
            DateValue::Before(date) => DateValue::Before(convert(date)?),
            DateValue::After(date) => DateValue::After(convert(date)?),
            DateValue::Between(start, end) => DateValue::Between(convert(start)?, convert(end)?),
            DateValue::Period(period) => DateValue::Period(DatePeriod {
                from: period.from.as_ref().map(convert).transpose()?,
                to: period.to.as_ref().map(convert).transpose()?,
            }),
            DateValue::Interpreted { date, phrase } => DateValue::Interpreted {
                date: convert(date)?,
                phrase: phrase.clone(),
            },
        })
    }
}

/// A year as written, and whether it is before the common era, from its
/// astronomical number; `allow_bce` for the calendars that count years
/// before their epoch.
fn year_from_astronomical(
    astronomical: i32,
    allow_bce: bool,
) -> Result<(u32, bool), CalendarError> {
    if astronomical >= 1 {
        return u32::try_from(astronomical)
            .map(|year| (year, false))
            .map_err(|_| CalendarError::OutOfRange);
    }
    if !allow_bce {
        return Err(CalendarError::OutOfRange);
    }
    u32::try_from(1 - i64::from(astronomical))
        .map(|year| (year, true))
        .map_err(|_| CalendarError::OutOfRange)
}

/// The month of `calendrical_calculations`' Hebrew calendar ("book" months,
/// from Nisan) for a GEDCOM month in `year`.
///
/// GEDCOM orders months from Tishrei. A leap year has Adar I (`ADR`) and
/// Adar II (`ADS`); a common year has one Adar, which GEDCOM writes `ADS`
/// and many files `ADR`: both name it.
fn hebrew_book_month(month: &Month, year: i32) -> u8 {
    match month {
        Month::Tsh => 7,
        Month::Csh => 8,
        Month::Ksl => 9,
        Month::Tvt => 10,
        Month::Shv => 11,
        Month::Ads if BookHebrew::is_hebrew_leap_year(year) => 13,
        Month::Adr | Month::Ads => 12,
        Month::Nsn => 1,
        Month::Iyr => 2,
        Month::Svn => 3,
        Month::Tmz => 4,
        Month::Aav => 5,
        _ => 6,
    }
}

/// The GEDCOM month of a "book" Hebrew month in `year`; the only Adar of a
/// common year is `ADS`, as GEDCOM 7.0 recommends.
fn hebrew_gedcom_month(book_month: u8, year: i32) -> Month {
    match book_month {
        1 => Month::Nsn,
        2 => Month::Iyr,
        3 => Month::Svn,
        4 => Month::Tmz,
        5 => Month::Aav,
        6 => Month::Ell,
        7 => Month::Tsh,
        8 => Month::Csh,
        9 => Month::Ksl,
        10 => Month::Tvt,
        11 => Month::Shv,
        12 if BookHebrew::is_hebrew_leap_year(year) => Month::Adr,
        _ => Month::Ads,
    }
}

/// The Rata Die of a French Republican date whose day is valid for its
/// month, the year given.
fn french_republican_rata_die(year: u32, month: u8, day: u8) -> Result<i64, CalendarError> {
    if year > FRENCH_REPUBLICAN_MAX_YEAR {
        return Err(CalendarError::OutOfRange);
    }
    let year = i64::from(year);
    // The complementary days are what the year has beyond twelve months of
    // thirty days: five, or six in a sextile year.
    if month == 13 && i64::from(day) > calendrier::get_day_count(year) - 360 {
        return Err(CalendarError::InvalidDate);
    }
    let date = calendrier::Date::from_ymd(year, i64::from(month), i64::from(day));
    let days = date
        .timestamp()
        .seconds
        .div_euclid(REPUBLICAN_SECONDS_PER_DAY);
    FRENCH_REPUBLICAN_EPOCH_RD
        .checked_add(days)
        .ok_or(CalendarError::OutOfRange)
}

/// The French Republican date of a Rata Die, from the calendar's first day
/// to [`FRENCH_REPUBLICAN_MAX_RD`].
fn french_republican_from_rata_die(rata_die: i64) -> Result<CalendarDate, CalendarError> {
    if !(FRENCH_REPUBLICAN_EPOCH_RD..=FRENCH_REPUBLICAN_MAX_RD).contains(&rata_die) {
        return Err(CalendarError::OutOfRange);
    }
    let seconds = (rata_die - FRENCH_REPUBLICAN_EPOCH_RD) * REPUBLICAN_SECONDS_PER_DAY;
    let date = calendrier::Date::from_timestamp(calendrier::Timestamp { seconds });
    let year = u32::try_from(date.year())
        .ok()
        .filter(|year| *year >= 1)
        .ok_or(CalendarError::OutOfRange)?;
    let month = u8::try_from(date.month().num()).map_err(|_| CalendarError::OutOfRange)?;
    let day = u8::try_from(date.day()).map_err(|_| CalendarError::OutOfRange)?;
    Ok(CalendarDate {
        calendar: Calendar::FrenchRepublican,
        day: Some(day),
        month: Month::of(&Calendar::FrenchRepublican, month),
        year: Some(year),
        ..CalendarDate::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A year, month number and day, in the calendar the context names.
    type Ymd = (u32, u8, u8);

    fn ymd(calendar: Calendar, (year, month, day): Ymd) -> CalendarDate {
        CalendarDate {
            month: Month::of(&calendar, month),
            calendar,
            day: Some(day),
            year: Some(year),
            ..CalendarDate::default()
        }
    }

    fn triple(date: &CalendarDate) -> Ymd {
        (
            date.year.unwrap(),
            date.month.as_ref().and_then(Month::number).unwrap(),
            date.day.unwrap(),
        )
    }

    #[test]
    fn test_gregorian_julian_conversion() {
        // The day the Gregorian calendar was adopted.
        let gregorian = ymd(Calendar::Gregorian, (1582, 10, 15));
        let julian = gregorian.convert_to(&Calendar::Julian).unwrap();
        assert_eq!(julian.calendar, Calendar::Julian);
        assert_eq!(triple(&julian), (1582, 10, 5));
        assert_eq!(julian.convert_to(&Calendar::Gregorian).unwrap(), gregorian);
    }

    #[test]
    fn test_known_day_numbers() {
        assert_eq!(
            ymd(Calendar::Gregorian, (2000, 1, 1)).to_rata_die(),
            Ok(730_120)
        );
        assert_eq!(ymd(Calendar::Gregorian, (1, 1, 1)).to_rata_die(), Ok(1));
        assert_eq!(
            ymd(Calendar::Gregorian, (2026, 3, 23))
                .to_julian_day_number()
                .unwrap()
                - ymd(Calendar::Gregorian, (2026, 3, 23))
                    .to_rata_die()
                    .unwrap(),
            1_721_425
        );
    }

    #[test]
    fn test_bce_years_have_no_year_zero() {
        // 1 January 1 BCE is the day 365 days (a leap year) before 1 January 1.
        let one_bce = CalendarDate::parse("1 JAN 1 BCE");
        assert_eq!(one_bce.to_rata_die(), Ok(1 - 366));
        assert_eq!(one_bce.to_julian_day_number(), Ok(1_721_060));
        let julian = CalendarDate::parse("JULIAN 1 JAN 1 BCE");
        assert_eq!(julian.to_julian_day_number(), Ok(1_721_058));
        let back = CalendarDate::from_rata_die(1 - 366, &Calendar::Gregorian).unwrap();
        assert_eq!(back, one_bce);
        assert_eq!(back.epoch, Some(Epoch::Bce));
        // 31 December 1 BCE is the day before 1 January 1.
        let eve = CalendarDate::from_rata_die(0, &Calendar::Gregorian).unwrap();
        assert_eq!((triple(&eve), eve.epoch), ((1, 12, 31), Some(Epoch::Bce)));
    }

    #[test]
    fn test_hebrew_conversion() {
        // 15 Tishrei 5784 is 30 September 2023.
        let hebrew = ymd(Calendar::Hebrew, (5784, 1, 15));
        let gregorian = hebrew.convert_to(&Calendar::Gregorian).unwrap();
        assert_eq!(triple(&gregorian), (2023, 9, 30));
        assert_eq!(gregorian.convert_to(&Calendar::Hebrew).unwrap(), hebrew);
    }

    #[test]
    fn test_hebrew_adar_in_common_and_leap_years() {
        // 5783 is a common year: its one Adar is written ADS, or ADR.
        let ads = CalendarDate::parse("HEBREW 1 ADS 5783");
        let adr = CalendarDate::parse("HEBREW 1 ADR 5783");
        assert_eq!(ads.to_rata_die(), adr.to_rata_die());
        let back =
            CalendarDate::from_rata_die(ads.to_rata_die().unwrap(), &Calendar::Hebrew).unwrap();
        assert_eq!(back.month, Some(Month::Ads));
        // 5784 is a leap year: Adar I and Adar II are 30 days apart.
        let adar_1 = CalendarDate::parse("HEBREW 1 ADR 5784")
            .to_rata_die()
            .unwrap();
        let adar_2 = CalendarDate::parse("HEBREW 1 ADS 5784")
            .to_rata_die()
            .unwrap();
        assert_eq!(adar_2 - adar_1, 30);
        let back = CalendarDate::from_rata_die(adar_1, &Calendar::Hebrew).unwrap();
        assert_eq!(back.month, Some(Month::Adr));
        assert_eq!(
            CalendarDate::parse("HEBREW 30 ELL 5784").to_rata_die(),
            Err(CalendarError::InvalidDate)
        );
    }

    /// Dates the French Republican calendar is pinned to in the historical
    /// record, as `(republican, gregorian)` pairs.
    const REPUBLICAN_LANDMARKS: &[(Ymd, Ymd)] = &[
        // 1 Vendémiaire An I: the day the Republic was proclaimed.
        ((1, 1, 1), (1792, 9, 22)),
        ((1, 1, 2), (1792, 9, 23)),
        ((2, 1, 1), (1793, 9, 22)),
        // 9 Thermidor An II.
        ((2, 11, 9), (1794, 7, 27)),
        // 18 Brumaire An VIII.
        ((8, 2, 18), (1799, 11, 9)),
        // 11 Nivôse An XIV: the last day before the calendar was abolished.
        ((14, 4, 11), (1806, 1, 1)),
    ];

    #[test]
    fn test_french_republican_conversion() {
        for &(republican, gregorian) in REPUBLICAN_LANDMARKS {
            let converted = ymd(Calendar::FrenchRepublican, republican)
                .convert_to(&Calendar::Gregorian)
                .unwrap();
            assert_eq!(triple(&converted), gregorian, "{republican:?}");
            let back = ymd(Calendar::Gregorian, gregorian)
                .convert_to(&Calendar::FrenchRepublican)
                .unwrap();
            assert_eq!(triple(&back), republican, "{gregorian:?}");
        }
    }

    #[test]
    fn test_french_republican_complementary_days() {
        // An I was not sextile; An III was.
        let last = ymd(Calendar::FrenchRepublican, (1, 13, 5))
            .convert_to(&Calendar::Gregorian)
            .unwrap();
        assert_eq!(triple(&last), (1793, 9, 21));
        let sextile = ymd(Calendar::FrenchRepublican, (3, 13, 6))
            .convert_to(&Calendar::Gregorian)
            .unwrap();
        assert_eq!(triple(&sextile), (1795, 9, 22));
        for invalid in [(1, 1, 31), (1, 13, 6), (1, 1, 0)] {
            assert_eq!(
                ymd(Calendar::FrenchRepublican, invalid).to_rata_die(),
                Err(CalendarError::InvalidDate),
                "{invalid:?}"
            );
        }
        // Far years are out of `calendrier`'s exact range.
        assert_eq!(
            ymd(Calendar::FrenchRepublican, (499_336, 13, 5)).to_rata_die(),
            Err(CalendarError::OutOfRange)
        );
        assert_eq!(
            CalendarDate::from_rata_die(183_036_888, &Calendar::FrenchRepublican),
            Err(CalendarError::OutOfRange)
        );
        // Before An I there is no Republican date.
        assert_eq!(
            CalendarDate::from_rata_die(1, &Calendar::FrenchRepublican),
            Err(CalendarError::OutOfRange)
        );
    }

    #[test]
    fn test_french_republican_rata_die_round_trip() {
        let first = ymd(Calendar::FrenchRepublican, (1, 1, 1))
            .to_rata_die()
            .unwrap();
        for rata_die in first..first + 6_000 {
            let date = CalendarDate::from_rata_die(rata_die, &Calendar::FrenchRepublican).unwrap();
            assert_eq!(date.to_rata_die(), Ok(rata_die));
        }
    }

    #[test]
    fn test_errors() {
        assert_eq!(
            CalendarDate::parse("MAR 1900").to_rata_die(),
            Err(CalendarError::Incomplete)
        );
        assert_eq!(
            CalendarDate::parse("1 JAN 1700 old style").convert_to(&Calendar::Gregorian),
            Err(CalendarError::UnrecognisedText(
                "1 JAN 1700 old style".into()
            ))
        );
        assert_eq!(
            CalendarDate::parse("29 FEB 1900").to_rata_die(),
            Err(CalendarError::InvalidDate)
        );
        assert_eq!(
            CalendarDate::parse("@#DROMAN@ 1 _M 1900").to_rata_die(),
            Err(CalendarError::UnsupportedCalendar(Calendar::Roman))
        );
        assert_eq!(
            CalendarDate::parse("1 JAN 0").to_rata_die(),
            Err(CalendarError::InvalidDate)
        );
        assert_eq!(
            CalendarDate::parse("2 OCT 6000000").convert_to(&Calendar::Hebrew),
            Err(CalendarError::OutOfRange)
        );
        assert_eq!(
            CalendarDate::from_rata_die(i64::MIN, &Calendar::Hebrew),
            Err(CalendarError::OutOfRange)
        );
        assert_eq!(
            CalendarDate::parse("1 JAN 2000").add_days(i64::MAX),
            Err(CalendarError::OutOfRange)
        );
    }

    #[test]
    fn test_arithmetic() {
        let a = CalendarDate::parse("8 MAY 1980");
        assert_eq!(a.days_until(&CalendarDate::parse("9 MAY 1980")), Ok(1));
        assert_eq!(
            a.days_until(&CalendarDate::parse("@#DJULIAN@ 25 APR 1980")),
            Ok(0)
        );
        let b = CalendarDate::parse("11 SEP 2001").add_days(20).unwrap();
        assert_eq!(triple(&b), (2001, 10, 1));
        assert_eq!(
            CalendarDate::parse("1 JAN 2000").weekday(),
            Some(Weekday::Saturday)
        );
        assert_eq!(
            CalendarDate::parse("4 JUL 1776").weekday(),
            Some(Weekday::Thursday)
        );
        assert_eq!(CalendarDate::parse("1980").weekday(), None);
    }

    #[test]
    fn test_ordering() {
        let key = |text: &str| CalendarDate::parse(text).ordering_key();
        assert!(key("27 JAN 1986") < key("28 JAN 1986"));
        assert!(key("1986") < key("FEB 1986"));
        assert_eq!(key("1986"), key("1 JAN 1986"));
        assert_eq!(key("HEBREW 5784"), key("HEBREW 1 TSH 5784"));
        assert_eq!(key("vers 1986"), None);

        let cmp =
            |a: &str, b: &str| CalendarDate::parse(a).chronological_cmp(&CalendarDate::parse(b));
        assert_eq!(
            cmp("15 OCT 1582", "@#DJULIAN@ 5 OCT 1582"),
            Some(Ordering::Equal)
        );
        assert_eq!(cmp("1984", "1985"), Some(Ordering::Less));
        assert_eq!(cmp("1984", "@#DHEBREW@ 5740"), None);
        assert_eq!(cmp("1984", "15 MAR 1984"), None);
        assert_eq!(cmp("44 BCE", "1 BCE"), Some(Ordering::Less));
    }

    #[test]
    fn test_value_conversion_keeps_its_shape() {
        let value = DateValue::parse("ABT @#DJULIAN@ 1 JAN 1700");
        let converted = value.convert_to(&Calendar::Gregorian).unwrap();
        assert_eq!(
            converted.to_gedcom(crate::GedcomVersion::V5_5_1),
            "ABT 11 JAN 1700"
        );
        let value = DateValue::parse("BET 1 JAN 1700 AND 1710");
        assert_eq!(
            value.convert_to(&Calendar::Julian),
            Err(CalendarError::Incomplete)
        );
        assert_eq!(
            DateValue::parse("(unknown)").convert_to(&Calendar::Julian),
            Ok(DateValue::Phrase("unknown".into()))
        );
    }
}
