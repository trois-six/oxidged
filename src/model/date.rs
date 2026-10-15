//! Dates, ages and times, and the change and creation dates of records.
//!
//! Values are kept as written ([`Text`]), so that nothing a grammar does not
//! know is lost; the grammars of [`crate::value`] read them on demand.

use crate::spec::payload::Family;
use crate::spec::schema::Kind;
use crate::value::{AgeTexts, AgeValue, DateExact, DatePeriod, DateTexts, DateValue, Time};
use crate::version::GedcomVersion;

use super::driver::{Converted, WriteCx, gedcom_struct};
use super::list::ThinVec;
use super::node::{Extra, Node, Value};
use super::note::Note;
use super::text::{Store, Text};

/// The characters of an optional text as they will be written — without
/// the characters the target version bans, which the writer leaves out (a
/// 5.5.1 tab becomes a space) — `None` when empty.
fn owned(text: Option<&Text>, cx: &WriteCx<'_>) -> Option<String> {
    let rules = cx.version.rules();
    text.map(|t| t.to_str(cx.store))
        .map(|s| {
            s.chars()
                .map(|c| {
                    if c == '\t' && rules.is_banned(c) {
                        ' '
                    } else {
                        c
                    }
                })
                .filter(|&c| !rules.is_banned(c))
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
}

/// What [`date_to_version`] makes of a date.
enum DateConversion {
    /// It is valid in the target's grammar, as the conformance check finds
    /// it ([`crate::spec::payload::is_valid`]): written as it is.
    Valid,
    /// Written as it is: in the target's grammar already, or not one the
    /// grammar can read.
    AsIs,
    /// Written so.
    Into(DateTexts),
}

/// A date, its time and its phrase in the target version's grammar
/// ([`Date::to_version`]), the date of `kind` (a date value, an
/// exact date or a period).
///
/// A date valid in the target's grammar (most are: dates of the commonest
/// shapes without a parse) is written as it is, and said valid, so that
/// the conformance check need not read it again.
fn date_to_version(
    value: &Text,
    time: Option<&Text>,
    phrase: Option<&Text>,
    kind: Kind,
    cx: &WriteCx<'_>,
) -> DateConversion {
    let family = Family::of(cx.version.rules());
    let valid = |kind, text: &str| crate::spec::payload::is_valid(kind, None, text, family);
    // A date read valid in every version, as most are.
    let known = match kind {
        Kind::Date => super::text::SIMPLE_DATE,
        Kind::DateExact => super::text::EXACT_DATE,
        _ => 0,
    };
    if phrase.is_none()
        && value.valid_as() & known != 0
        && time.is_none_or(|t| valid(Kind::Time, &t.to_str(cx.store)))
    {
        debug_assert!(valid(kind, &value.to_str(cx.store)), "{value:?}");
        debug_assert!(
            convert(value, time, None, cx).is_none(),
            "{value:?} {time:?}"
        );
        return DateConversion::Valid;
    }
    if phrase.is_none() {
        let text = value.to_str(cx.store);
        if valid(kind, &text) && time.is_none_or(|t| valid(Kind::Time, &t.to_str(cx.store))) {
            debug_assert!(
                convert(value, time, None, cx).is_none(),
                "{value:?} {time:?}"
            );
            return DateConversion::Valid;
        }
    }
    // A date of the commonest shapes is written alike in every version
    // (5.5.1 reads its month in any case, 7.x in upper case).
    if time.is_none() && phrase.is_none() {
        let text = value.to_str(cx.store);
        let text = &*text;
        let v7 = cx.version.is_v7();
        if crate::spec::is_simple_date(text, false, !v7) {
            debug_assert!(convert(value, None, None, cx).is_none(), "{value:?}");
            return DateConversion::AsIs;
        }
        if crate::spec::is_simple_date(text, false, true) {
            let upper = DateTexts {
                value: Some(text.to_ascii_uppercase()),
                time: None,
                phrase: None,
            };
            debug_assert_eq!(
                convert(value, None, None, cx),
                Some(upper.clone()),
                "{value:?}"
            );
            return DateConversion::Into(upper);
        }
    }
    match convert(value, time, phrase, cx) {
        Some(c) => DateConversion::Into(c),
        None => DateConversion::AsIs,
    }
}

/// [`date_to_version`], by the grammars.
fn convert(
    value: &Text,
    time: Option<&Text>,
    phrase: Option<&Text>,
    cx: &WriteCx<'_>,
) -> Option<DateTexts> {
    let date = DateTexts {
        value: owned(Some(value), cx),
        time: owned(time, cx),
        phrase: owned(phrase, cx),
    };
    let converted = date.to_version(cx.version);
    (converted != date).then_some(converted)
}

/// The parts of an untyped value structure (a date, an age): its text
/// payload, the first text of each leaf `tags` names, and the other
/// substructures, as reading would give them; `None` when it would not fit
/// (an identifier, a pointer).
fn parts<const N: usize>(
    node: &Node,
    store: &Store,
    tags: [&str; N],
) -> Option<(Text, [Option<Text>; N], Extra)> {
    let value = match (&node.payload, node.xref) {
        (Value::None, None) => Text::default(),
        (Value::Text(t), None) => t.clone(),
        _ => return None,
    };
    let mut found: [Option<Text>; N] = std::array::from_fn(|_| None);
    let mut sent = [false; N];
    let mut extra = Extra::new();
    for c in &node.children {
        let at = tags.iter().position(|t| *t == store.tag(c.tag));
        let leaf = match &c.payload {
            Value::None => Some(Text::default()),
            Value::Text(t) => Some(t.clone()),
            Value::Pointer(_) => None,
        };
        match (at, leaf) {
            (Some(i), Some(t))
                if c.children.is_empty() && c.xref.is_none() && !sent[i] && found[i].is_none() =>
            {
                found[i] = Some(t);
            }
            (Some(i), _) => {
                sent[i] = true;
                extra.push(c.clone());
            }
            (None, _) => extra.push(c.clone()),
        }
    }
    Some((value, found, extra))
}

fn untyped_date(node: &Node, store: &Store) -> Option<Date> {
    let (value, [time, phrase], extra) = parts(node, store, ["TIME", "PHRASE"])?;
    Some(Date {
        value,
        detail: (time.is_some() || phrase.is_some()).then(|| Box::new(DateDetail { time, phrase })),
        extra,
    })
}

fn untyped_exact(node: &Node, store: &Store) -> Option<ExactDate> {
    let (value, [time], extra) = parts(node, store, ["TIME"])?;
    Some(ExactDate { value, time, extra })
}

fn untyped_period(node: &Node, store: &Store) -> Option<Period> {
    let (value, [phrase], extra) = parts(node, store, ["PHRASE"])?;
    Some(Period {
        value,
        phrase,
        extra,
    })
}

fn untyped_age(node: &Node, store: &Store) -> Option<Age> {
    let (value, [phrase], extra) = parts(node, store, ["PHRASE"])?;
    Some(Age {
        value,
        phrase,
        extra,
    })
}

/// An optional text of an optional string; `None` when empty.
fn text(s: Option<String>) -> Option<Text> {
    s.filter(|s| !s.is_empty()).map(Text::new)
}

fn convert_date(date: &Date, cx: &WriteCx<'_>) -> Converted<Date> {
    let detail = date.detail();
    let conversion = date_to_version(
        &date.value,
        detail.time.as_ref(),
        detail.phrase.as_ref(),
        Kind::Date,
        cx,
    );
    date_of(conversion, || date.extra.clone())
}

/// What [`convert_date`] makes of a date's conversion, the date's untyped
/// substructures given by `extra` (asked for only when it is converted).
fn date_of(conversion: DateConversion, extra: impl FnOnce() -> Extra) -> Converted<Date> {
    let c = match conversion {
        DateConversion::Valid => return Converted::AsIs(Some(Kind::Date)),
        DateConversion::AsIs => return Converted::AsIs(None),
        DateConversion::Into(c) => c,
    };
    let (time, phrase) = (text(c.time), text(c.phrase));
    Converted::Into(Date {
        value: Text::new(c.value.unwrap_or_default()),
        detail: (time.is_some() || phrase.is_some()).then(|| Box::new(DateDetail { time, phrase })),
        extra: extra(),
    })
}

fn convert_exact(date: &ExactDate, cx: &WriteCx<'_>) -> Converted<ExactDate> {
    let conversion = date_to_version(&date.value, date.time.as_ref(), None, Kind::DateExact, cx);
    exact_of(conversion, || date.extra.clone())
}

/// What [`convert_exact`] makes of a date's conversion.
fn exact_of(conversion: DateConversion, extra: impl FnOnce() -> Extra) -> Converted<ExactDate> {
    let c = match conversion {
        DateConversion::Valid => return Converted::AsIs(Some(Kind::DateExact)),
        DateConversion::AsIs => return Converted::AsIs(None),
        DateConversion::Into(c) => c,
    };
    // An exact date has no phrase to carry what the payload cannot say.
    match c.phrase {
        None => Converted::Into(ExactDate {
            value: Text::new(c.value.unwrap_or_default()),
            time: text(c.time),
            extra: extra(),
        }),
        Some(_) => Converted::AsIs(None),
    }
}

fn convert_period(period: &Period, cx: &WriteCx<'_>) -> Converted<Period> {
    let conversion = date_to_version(
        &period.value,
        None,
        period.phrase.as_ref(),
        Kind::DatePeriod,
        cx,
    );
    period_of(conversion, || period.extra.clone())
}

/// What [`convert_period`] makes of a period's conversion.
fn period_of(conversion: DateConversion, extra: impl FnOnce() -> Extra) -> Converted<Period> {
    let c = match conversion {
        DateConversion::Valid => return Converted::AsIs(Some(Kind::DatePeriod)),
        DateConversion::AsIs => return Converted::AsIs(None),
        DateConversion::Into(c) => c,
    };
    match c.time {
        None => Converted::Into(Period {
            value: Text::new(c.value.unwrap_or_default()),
            phrase: text(c.phrase),
            extra: extra(),
        }),
        Some(_) => Converted::AsIs(None),
    }
}

/// An age and its phrase in the target version's grammar
/// ([`Age::to_version`]).
fn convert_age(a: &Age, cx: &WriteCx<'_>) -> Converted<Age> {
    let age = AgeTexts {
        value: owned(Some(&a.value), cx),
        phrase: owned(a.phrase.as_ref(), cx),
    };
    // An age the target permits as it is (5.5.1 also spaces its bound,
    // `> 25y`, and ignores case) is kept so, as the conformance repair
    // keeps it: what a file says validly is not rewritten.
    let valid = age.value.as_deref().is_some_and(|v| {
        let family = Family::of(cx.version.rules());
        crate::spec::payload::check(Kind::Age, None, v, family).is_none()
    });
    if valid && (cx.version.is_v7() || age.phrase.is_none()) {
        // Valid as written when no character was left out of it.
        let written = age
            .value
            .as_deref()
            .is_some_and(|v| a.value.eq_str(cx.store, v));
        return Converted::AsIs(written.then_some(Kind::Age));
    }
    let c = age.to_version(cx.version);
    if c == age {
        return Converted::AsIs(None);
    }
    Converted::Into(Age {
        value: Text::new(c.value.unwrap_or_default()),
        phrase: text(c.phrase),
        extra: a.extra.clone(),
    })
}

/// [`convert_date`] of a row: the date is copied out only when it is
/// written otherwise.
fn convert_date_row(row: &DateRow, at: u32, cx: &WriteCx<'_>) -> Converted<Date> {
    use super::table::HasTable;
    let detail = row
        .detail
        .and_then(|d| DateDetail::table(cx.tables).rows.get(d.get() as usize));
    let value = Text::from_span(row.value);
    let time = detail.and_then(|d| d.time).map(Text::from_span);
    let phrase = detail.and_then(|d| d.phrase).map(Text::from_span);
    let conversion = date_to_version(&value, time.as_ref(), phrase.as_ref(), Kind::Date, cx);
    date_of(conversion, || {
        super::node::load_extra(row.extra_in(at, cx.tables), cx.tables)
    })
}

/// [`convert_exact`] of a row.
fn convert_exact_row(row: &ExactDateRow, at: u32, cx: &WriteCx<'_>) -> Converted<ExactDate> {
    let value = Text::from_span(row.value);
    let time = row.time.map(Text::from_span);
    let conversion = date_to_version(&value, time.as_ref(), None, Kind::DateExact, cx);
    exact_of(conversion, || {
        super::node::load_extra(row.extra_in(at, cx.tables), cx.tables)
    })
}

/// [`convert_period`] of a row.
fn convert_period_row(row: &PeriodRow, at: u32, cx: &WriteCx<'_>) -> Converted<Period> {
    let value = Text::from_span(row.value);
    let phrase = row.phrase.map(Text::from_span);
    let conversion = date_to_version(&value, None, phrase.as_ref(), Kind::DatePeriod, cx);
    period_of(conversion, || {
        super::node::load_extra(row.extra_in(at, cx.tables), cx.tables)
    })
}

/// [`convert_age`] of a row.
fn convert_age_row(row: &AgeRow, at: u32, cx: &WriteCx<'_>) -> Converted<Age> {
    use super::table::Item;
    convert_age(&Age::load_elem(row, at, cx.tables), cx)
}

gedcom_struct! {
    /// A date (`DATE`, 7.x also `SDATE`): a date value, the time of day
    /// (7.x) and the date in words (7.x `PHRASE`).
    ///
    /// Written in another version's grammar, it is converted
    /// ([`Date::to_version`]): `@#DJULIAN@` and `JULIAN`, `B.C.`
    /// and `BCE`, dual years, phrases.
    pub struct Date => DateRow [convert = convert_date, untyped_date, convert_date_row] {
        @payload
        /// The date value as written (`ABT 1900`, `@#DJULIAN@ 1 JAN 1700`).
        value: Text;
        @detail
        /// The time and phrase of a [`Date`], which few dates have.
        DateDetail => DateDetailRow {
            /// The time (`TIME`).
            "TIME" => time: Option<Text>,
            /// The date in words (`PHRASE`).
            "PHRASE" => phrase: Option<Text>,
        }
    }
    spec {
        v551: [
            "EVENT_DETAIL.DATE",
            "LDS_INDIVIDUAL_ORDINANCE.BAPL.DATE",
            "LDS_INDIVIDUAL_ORDINANCE.ENDL.DATE",
            "LDS_INDIVIDUAL_ORDINANCE.SLGC.DATE",
            "LDS_SPOUSE_SEALING.SLGS.DATE",
            "SOURCE_CITATION.SOUR.DATA.DATE",
        ],
        v70: ["DATE", "SDATE"],
        v71: ["DATE", "SDATE"],
    }
}

/// A text as an optional string: `None` when empty.
fn some(text: Option<&Text>, store: &Store) -> Option<String> {
    text.map(|t| t.to_str(store).into_owned())
        .filter(|t| !t.is_empty())
}

impl Date {
    /// A date with this value.
    #[must_use]
    pub fn new(value: impl Into<Text>) -> Self {
        Self {
            value: value.into(),
            ..Self::default()
        }
    }

    /// The date value read by the date grammar of either version: never
    /// fails, keeps the words it does not understand
    /// ([`DateValue::parse`]).
    ///
    /// ```rust
    /// use oxidged::model::Dataset;
    /// use oxidged::value::{Approximation, Calendar, DateValue};
    ///
    /// let data = Dataset::parse("0 HEAD\n0 @I1@ INDI\n1 BIRT\n2 DATE ABT @#DJULIAN@ 1700\n0 TRLR\n");
    /// let date = data.individuals().first().unwrap().birth().unwrap().date().unwrap();
    /// let date = date.into_owned();
    /// let DateValue::Approximated(Approximation::About, year) = date.parse(&data) else {
    ///     panic!()
    /// };
    /// assert_eq!((year.calendar, year.year), (Calendar::Julian, Some(1700)));
    /// ```
    #[must_use]
    pub fn parse<S: AsRef<Store> + ?Sized>(&self, store: &S) -> DateValue {
        DateValue::parse(&self.value.to_str(store))
    }

    /// The value, time and phrase as strings.
    fn texts(&self, store: &Store) -> DateTexts {
        let detail = self.detail();
        DateTexts {
            value: some(Some(&self.value), store),
            time: some(detail.time.as_ref(), store),
            phrase: some(detail.phrase.as_ref(), store),
        }
    }

    /// A date of these strings (owned texts), keeping `extra`.
    fn from_texts(texts: DateTexts, extra: &Extra) -> Self {
        let (time, phrase) = (text(texts.time), text(texts.phrase));
        Date {
            value: Text::new(texts.value.unwrap_or_default()),
            detail: (time.is_some() || phrase.is_some())
                .then(|| Box::new(DateDetail { time, phrase })),
            extra: extra.clone(),
        }
    }

    /// The value and the time in one string (`2 OCT 2019 12:00`), or the
    /// time alone without a value; `None` without a time.
    #[must_use]
    pub fn datetime<S: AsRef<Store> + ?Sized>(&self, store: &S) -> Option<String> {
        let store = store.as_ref();
        let time = some(self.detail().time.as_ref(), store)?;
        Some(match some(Some(&self.value), store) {
            Some(value) => format!("{value} {time}"),
            None => time,
        })
    }

    /// The date rewritten in the grammar of `version`, keeping everything
    /// it says (its texts owned); the writer converts dates so.
    ///
    /// A value that already follows the version's grammar is kept as
    /// written. Otherwise it is read and written back in the version's
    /// grammar:
    ///
    /// - to GEDCOM 7.x, calendar escapes become keywords (`@#DROMAN@` the
    ///   extension calendar `_ROMAN`), `B.C.` becomes `BCE`, `INT date
    ///   (phrase)` the date with the phrase as `PHRASE`, and `(phrase)` an
    ///   empty value with the `PHRASE`. A dual year becomes one year (the
    ///   later one when it is the year that follows, as in `1648/49`, the
    ///   year written otherwise), or a `BET … AND …` range for a year
    ///   alone, and the original wording becomes the `PHRASE`;
    /// - to GEDCOM 5.5.1, keywords become escapes, `BCE` becomes `B.C.`,
    ///   and the `PHRASE` moves into the value, as `INT date (phrase)`
    ///   after a single date or `(phrase)` alone. Next to a range or a
    ///   period it stays a `PHRASE`, which 5.5.1 cannot write (the writer
    ///   then keeps it as an extension).
    ///
    /// A value the grammar cannot fully read, or that the version cannot
    /// express, is left as written, for the writer to repair. The time is
    /// rewritten likewise; GEDCOM 5.5.1 has no `Z` for UTC.
    ///
    /// ```rust
    /// use oxidged::model::{Dataset, Date};
    /// use oxidged::GedcomVersion;
    ///
    /// let data = Dataset::default();
    /// let date = Date::new("INT 1850 (about 1850)").to_version(&data, GedcomVersion::V7_0);
    /// assert_eq!(date.value.to_str(&data), "1850");
    /// assert_eq!(date.detail().phrase.as_ref().unwrap().to_str(&data), "about 1850");
    /// ```
    #[must_use]
    pub fn to_version<S: AsRef<Store> + ?Sized>(&self, store: &S, version: GedcomVersion) -> Date {
        Self::from_texts(self.texts(store.as_ref()).to_version(version), &self.extra)
    }

    /// The date in the canonical spelling of `version`: converted as by
    /// [`to_version`](Self::to_version), then written back from its
    /// interpretation, upper case and single-spaced. Wording the grammar
    /// does not understand is kept as written.
    #[must_use]
    pub fn normalize<S: AsRef<Store> + ?Sized>(&self, store: &S, version: GedcomVersion) -> Date {
        Self::from_texts(self.texts(store.as_ref()).normalize(version), &self.extra)
    }

    /// The date with every date of its value converted to `calendar`,
    /// written in the grammar of `version`; qualifiers, ranges, periods,
    /// the time and the phrase are kept.
    ///
    /// ```rust
    /// use oxidged::model::{Dataset, Date};
    /// use oxidged::value::Calendar;
    /// use oxidged::GedcomVersion;
    ///
    /// let data = Dataset::default();
    /// let date = Date::new("@#DJULIAN@ 15 MAR 1582");
    /// let gregorian = date.convert_to(&data, &Calendar::Gregorian, GedcomVersion::V5_5_1).unwrap();
    /// assert_eq!(gregorian.value.to_str(&data), "25 MAR 1582");
    /// ```
    ///
    /// # Errors
    ///
    /// A [`CalendarError`](crate::value::CalendarError) when a date of the
    /// value cannot be converted: it is incomplete, holds unrecognised
    /// words, is invalid or out of range, or its calendar has no
    /// arithmetic.
    #[cfg(feature = "calendar")]
    pub fn convert_to<S: AsRef<Store> + ?Sized>(
        &self,
        store: &S,
        calendar: &crate::value::Calendar,
        version: GedcomVersion,
    ) -> Result<Date, crate::value::CalendarError> {
        let texts = self.texts(store.as_ref()).convert_to(calendar, version)?;
        Ok(Self::from_texts(texts, &self.extra))
    }

    /// The time read by the time grammar.
    #[must_use]
    pub fn parse_time<S: AsRef<Store> + ?Sized>(&self, store: &S) -> Option<Time> {
        Time::parse(&self.detail().time.as_ref()?.to_str(store))
    }
}

gedcom_struct! {
    /// An exact date (`DATE` of `CHAN`, `CREA`, an ordinance status or the
    /// header) and its time.
    pub struct ExactDate => ExactDateRow [convert = convert_exact, untyped_exact, convert_exact_row] {
        @payload
        /// The date as written (`1 JAN 2000`).
        value: Text;
        /// The time (`TIME`).
        "TIME" => time: Option<Text>,
    }
    spec {
        v551: [
            "CHANGE_DATE.CHAN.DATE",
            "HEADER.HEAD.DATE",
            "HEADER.HEAD.SOUR.DATA.DATE",
            "LDS_INDIVIDUAL_ORDINANCE.BAPL.STAT.DATE",
            "LDS_INDIVIDUAL_ORDINANCE.ENDL.STAT.DATE",
            "LDS_INDIVIDUAL_ORDINANCE.SLGC.STAT.DATE",
            "LDS_SPOUSE_SEALING.SLGS.STAT.DATE",
        ],
        v70: ["DATE-exact", "HEAD-DATE"],
        v71: ["DATE-exact", "HEAD-DATE"],
    }
}

impl ExactDate {
    /// The date read by the exact-date grammar; `None` when it does not
    /// follow it.
    #[must_use]
    pub fn parse<S: AsRef<Store> + ?Sized>(&self, store: &S) -> Option<DateExact> {
        DateExact::parse(&self.value.to_str(store))
    }

    /// The time read by the time grammar.
    #[must_use]
    pub fn parse_time<S: AsRef<Store> + ?Sized>(&self, store: &S) -> Option<Time> {
        Time::parse(&self.time.as_ref()?.to_str(store))
    }
}

gedcom_struct! {
    /// A date period (`DATE` of a source's recorded events, 7.x `NO`) and
    /// its phrase.
    pub struct Period => PeriodRow [convert = convert_period, untyped_period, convert_period_row] {
        @payload
        /// The period as written (`FROM 1900 TO 1910`).
        value: Text;
        /// The period in words (`PHRASE`).
        "PHRASE" => phrase: Option<Text>,
    }
    spec {
        v551: ["SOURCE_RECORD.SOUR.DATA.EVEN.DATE"],
        v70: ["DATA-EVEN-DATE", "NO-DATE"],
        v71: ["DATA-EVEN-DATE", "NO-DATE"],
    }
}

impl Period {
    /// The period read by the date-period grammar; `None` when it does not
    /// follow it.
    #[must_use]
    pub fn parse<S: AsRef<Store> + ?Sized>(&self, store: &S) -> Option<DatePeriod> {
        DatePeriod::parse(&self.value.to_str(store))
    }
}

gedcom_struct! {
    /// An age at an event (`AGE`) and its phrase. Written in another
    /// version's grammar, it is converted ([`Age::to_version`]).
    pub struct Age => AgeRow [convert = convert_age, untyped_age, convert_age_row] {
        @payload
        /// The age as written (`> 25y 3m`, `CHILD`).
        value: Text;
        /// The age in words (`PHRASE`).
        "PHRASE" => phrase: Option<Text>,
    }
    spec {
        v551: [
            "FAMILY_EVENT_DETAIL.HUSB.AGE",
            "FAMILY_EVENT_DETAIL.WIFE.AGE",
            "INDIVIDUAL_EVENT_DETAIL.AGE",
        ],
        v70: ["AGE"],
        v71: ["AGE"],
    }
}

impl Age {
    /// An age with this value.
    #[must_use]
    pub fn new(value: impl Into<Text>) -> Self {
        Self {
            value: value.into(),
            ..Self::default()
        }
    }

    /// The age read by the age grammar of either version: never fails
    /// ([`AgeValue::parse`]).
    ///
    /// ```rust
    /// use oxidged::model::{Age, Dataset};
    /// use oxidged::value::{AgeBound, AgeValue};
    ///
    /// let data = Dataset::default();
    /// let AgeValue::Duration { bound, duration } = Age::new("> 1y 400d").parse(&data) else {
    ///     panic!()
    /// };
    /// assert_eq!(bound, Some(AgeBound::GreaterThan));
    /// assert_eq!((duration.years, duration.days), (Some(1), Some(400)));
    /// ```
    #[must_use]
    pub fn parse<S: AsRef<Store> + ?Sized>(&self, store: &S) -> AgeValue {
        AgeValue::parse(&self.value.to_str(store))
    }

    /// The value and phrase as strings.
    pub(crate) fn texts<S: AsRef<Store> + ?Sized>(&self, store: &S) -> AgeTexts {
        let store = store.as_ref();
        AgeTexts {
            value: some(Some(&self.value), store),
            phrase: some(self.phrase.as_ref(), store),
        }
    }

    /// The age rewritten in the grammar of `version`, keeping everything it
    /// says (its texts owned); the writer converts ages so.
    ///
    /// A value that already follows the version's grammar is kept as
    /// written. Otherwise:
    ///
    /// - to GEDCOM 7.x, a keyword becomes its duration with the keyword as
    ///   it was written as the `PHRASE` (`CHILD` becomes `< 8y` and `PHRASE
    ///   CHILD`), a duration is written in 7.x's form (`<8Y` becomes
    ///   `< 8y`; a bare number is a number of years), and text that is not
    ///   an age moves to the `PHRASE`, the value left empty, unless the age
    ///   already has another `PHRASE`;
    /// - to GEDCOM 5.5.1, a duration is written in 5.5.1's form (`<8y`),
    ///   weeks counted as days; a duration with a `PHRASE` that names the
    ///   keyword it stands for becomes that keyword. Any other `PHRASE`
    ///   stays, which 5.5.1 cannot write.
    ///
    /// ```rust
    /// use oxidged::model::{Age, Dataset};
    /// use oxidged::GedcomVersion;
    ///
    /// let data = Dataset::default();
    /// let age = Age::new("CHILD").to_version(&data, GedcomVersion::V7_0);
    /// assert_eq!(age.value.to_str(&data), "< 8y");
    /// assert_eq!(age.phrase.as_ref().unwrap().to_str(&data), "CHILD");
    /// ```
    #[must_use]
    pub fn to_version<S: AsRef<Store> + ?Sized>(&self, store: &S, version: GedcomVersion) -> Age {
        let texts = self.texts(store).to_version(version);
        Age {
            value: Text::new(texts.value.unwrap_or_default()),
            phrase: text(texts.phrase),
            extra: self.extra.clone(),
        }
    }
}

gedcom_struct! {
    /// When a record last changed (`CHAN`), with notes about the change.
    pub struct ChangeDate => ChangeDateRow {
        /// The date and time of the change (`DATE`).
        "DATE" => date: Option<ExactDate>,
        /// Notes about the change (`NOTE`, 7.x `SNOTE`).
        "NOTE" | "SNOTE" => notes: ThinVec<Note>,
    }
    spec {
        v551: ["CHANGE_DATE.CHAN"],
        v70: ["CHAN"],
        v71: ["CHAN"],
    }
}

gedcom_struct! {
    /// When a record was created (7.x `CREA`).
    pub struct CreationDate => CreationDateRow {
        /// The date and time of the creation (`DATE`).
        "DATE" => date: Option<ExactDate>,
    }
    spec {
        v551: [],
        v70: ["CREA"],
        v71: ["CREA"],
    }
}

impl super::View<'_, Date> {
    /// The date value read by the date grammar of either version
    /// ([`Date::parse`]).
    #[must_use]
    pub fn parse(self) -> DateValue {
        DateValue::parse(&self.value().to_str())
    }

    /// The time read by the time grammar.
    #[must_use]
    pub fn parse_time(self) -> Option<Time> {
        Time::parse(&self.time()?.to_str())
    }

    /// The value and the time in one string ([`Date::datetime`]).
    #[must_use]
    pub fn datetime(self) -> Option<String> {
        let time = self
            .time()
            .map(|t| t.to_string())
            .filter(|t| !t.is_empty())?;
        let value = self.value().to_string();
        Some(if value.is_empty() {
            time
        } else {
            format!("{value} {time}")
        })
    }
}

impl super::View<'_, ExactDate> {
    /// The date read by the exact-date grammar ([`ExactDate::parse`]).
    #[must_use]
    pub fn parse(self) -> Option<DateExact> {
        DateExact::parse(&self.value().to_str())
    }

    /// The time read by the time grammar.
    #[must_use]
    pub fn parse_time(self) -> Option<Time> {
        Time::parse(&self.time()?.to_str())
    }
}

impl super::View<'_, Period> {
    /// The period read by the date-period grammar ([`Period::parse`]).
    #[must_use]
    pub fn parse(self) -> Option<DatePeriod> {
        DatePeriod::parse(&self.value().to_str())
    }
}

impl super::View<'_, Age> {
    /// The age read by the age grammar of either version ([`Age::parse`]).
    #[must_use]
    pub fn parse(self) -> AgeValue {
        AgeValue::parse(&self.value().to_str())
    }
}
