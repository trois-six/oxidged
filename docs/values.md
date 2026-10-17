---
type: "Feature Reference"
title: "Dates, Ages and Times"
description: "The date, age and time grammars of GEDCOM 5.5.1 and 7.0, conversions between versions, and calendar arithmetic with the calendar feature."
tags: [oxidged, values, dates, calendars]
generated: { by: human:maintainer, at: 2026-10-17T11:42:19Z }
---

# Dates, Ages and Times

> Part of the [oxidged documentation](index.md).
> See also: [Conversion](conversion.md)

Dates, ages and times are kept exactly as written, and read on demand by
the grammars of both versions (`oxidged::value`), which never fail: wording
they do not understand is kept verbatim. `parse_strict` checks a payload
against one version's grammar; `Date::to_version` and `Age::to_version`
rewrite a value in the other version's syntax, as the writer does.

```rust
use oxidged::model::Date;
use oxidged::value::{Approximation, Calendar, DateValue};
use oxidged::{Dataset, GedcomVersion};

let store = Dataset::default();
let date = Date::new("ABT @#DJULIAN@ 1700");
let DateValue::Approximated(Approximation::About, d) = date.parse(&store) else { panic!() };
assert_eq!((d.calendar, d.year), (Calendar::Julian, Some(1700)));
assert_eq!(date.to_version(&store, GedcomVersion::V7_0).value.to_str(&store), "ABT JULIAN 1700");
```

- **Exact**: `15 MAR 1950`; **range**: `BET 1900 AND 1910`, `BEF 1900`,
  `AFT 1900`; **period**: `FROM 1900 TO 1910`; **approximate**: `ABT`,
  `CAL`, `EST`
- **Interpreted and phrases** (5.5.1): `INT 1900 (about 1900)`, `(unknown)`
- **Dual years** (5.5.1): `15 APR 1699/00`; **before the common era**:
  `44 B.C.`, `44 BCE`
- **Ages**: `25y 3m`, `< 8y`, `> 1y 400d`, `CHILD`; **times**:
  `12:34:56.789`, `02:50Z`

| Calendar | 5.5.1 | 7.0 | Arithmetic (`calendar` feature) |
|----------|-------|-----|:-------------------------------:|
| Gregorian | `@#DGREGORIAN@` | `GREGORIAN` | ✅ |
| Julian | `@#DJULIAN@` | `JULIAN` | ✅ |
| Hebrew | `@#DHEBREW@` | `HEBREW` | ✅ |
| French Republican | `@#DFRENCH R@` | `FRENCH_R` | ✅ |
| Roman, unknown | `@#DROMAN@`, `@#DUNKNOWN@` | `_ROMAN`, `_UNKNOWN` | - |
| Extension calendars | - | `_MYCAL` | - |

---

## Calendar arithmetic

With the `calendar` feature, a `CalendarDate` converts to and from day
numbers (`to_rata_die`, `from_rata_die`, `to_julian_day_number`,
`from_julian_day_number`), to another calendar (`convert_to`, also on
`DateValue` and `model::Date`), counts days (`days_until`, `add_days`),
gives its `weekday`, and orders dates across calendars (`ordering_key`,
`chronological_cmp`). `Date::normalize` writes a date in its version's
canonical form (upper case, single spaces, well-formed calendar escapes).
