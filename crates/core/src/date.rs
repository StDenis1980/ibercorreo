//! Proleptic Gregorian dates and wall-clock times, enough for a calendar view and iCalendar.
//!
//! Day arithmetic goes through a day number (days since 1970-01-01), using Howard Hinnant's
//! public-domain `days_from_civil` algorithm. Years are limited to 1..=9999, like iCalendar, so
//! no arithmetic can overflow.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const MIN_YEAR: i32 = 1;
pub const MAX_YEAR: i32 = 9999;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Date {
    year: i32,
    month: u8,
    day: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    /// 0 for Monday … 6 for Sunday (ISO 8601 order).
    pub fn index(self) -> u8 {
        self as u8
    }

    fn from_index(i: i64) -> Weekday {
        match i.rem_euclid(7) {
            0 => Weekday::Monday,
            1 => Weekday::Tuesday,
            2 => Weekday::Wednesday,
            3 => Weekday::Thursday,
            4 => Weekday::Friday,
            5 => Weekday::Saturday,
            _ => Weekday::Sunday,
        }
    }
}

pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

impl Date {
    /// 1970-01-01, day number 0.
    pub const EPOCH: Date = Date { year: 1970, month: 1, day: 1 };

    pub fn new(year: i32, month: u8, day: u8) -> Result<Date> {
        if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
            return Err(Error::invalid("date", format!("year {year} outside {MIN_YEAR}..={MAX_YEAR}")));
        }
        if !(1..=12).contains(&month) {
            return Err(Error::invalid("date", format!("month {month}")));
        }
        if day == 0 || day > days_in_month(year, month) {
            return Err(Error::invalid("date", format!("day {day} in {year}-{month:02}")));
        }
        Ok(Date { year, month, day })
    }

    pub fn year(self) -> i32 {
        self.year
    }
    pub fn month(self) -> u8 {
        self.month
    }
    pub fn day(self) -> u8 {
        self.day
    }

    /// Days since 1970-01-01 (negative before it).
    pub fn day_number(self) -> i64 {
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = i64::from(self.month);
        let mp = if m > 2 { m - 3 } else { m + 9 };
        let doy = (153 * mp + 2) / 5 + i64::from(self.day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    pub fn from_day_number(n: i64) -> Result<Date> {
        // Bound first so the arithmetic below stays far from overflow.
        let lo = Date { year: MIN_YEAR, month: 1, day: 1 }.day_number();
        let hi = Date { year: MAX_YEAR, month: 12, day: 31 }.day_number();
        if !(lo..=hi).contains(&n) {
            return Err(Error::invalid("date", format!("day number {n} out of range")));
        }
        let z = n + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        let year = i32::try_from(year).map_err(|_| Error::invalid("date", "year overflow"))?;
        let month = u8::try_from(month).map_err(|_| Error::invalid("date", "month overflow"))?;
        let day = u8::try_from(day).map_err(|_| Error::invalid("date", "day overflow"))?;
        Date::new(year, month, day)
    }

    pub fn add_days(self, days: i64) -> Result<Date> {
        let n = self.day_number().checked_add(days).ok_or_else(|| Error::invalid("date", "overflow"))?;
        Date::from_day_number(n)
    }

    pub fn weekday(self) -> Weekday {
        // 1970-01-01 was a Thursday (index 3).
        Weekday::from_index(self.day_number() + 3)
    }

    pub fn first_of_month(self) -> Date {
        Date { day: 1, ..self }
    }

    /// The first day of the next (`delta` = 1) or previous (`delta` = -1) month, and so on.
    pub fn add_months(self, delta: i32) -> Result<Date> {
        let idx = i64::from(self.year) * 12 + i64::from(self.month) - 1 + i64::from(delta);
        let year = i32::try_from(idx.div_euclid(12)).map_err(|_| Error::invalid("date", "year overflow"))?;
        let month = u8::try_from(idx.rem_euclid(12) + 1).map_err(|_| Error::invalid("date", "month"))?;
        let day = self.day.min(days_in_month(year, month).max(1));
        Date::new(year, month, day)
    }

    /// `YYYY-MM-DD` or the iCalendar basic form `YYYYMMDD`.
    pub fn parse(s: &str) -> Result<Date> {
        let s = s.trim();
        let digits: String = s.chars().filter(|c| *c != '-').collect();
        if digits.len() != 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::invalid("date", format!("{s:?}")));
        }
        let num = |a: usize, b: usize| -> Result<u32> {
            digits.get(a..b).and_then(|p| p.parse().ok()).ok_or_else(|| Error::invalid("date", format!("{s:?}")))
        };
        let year = i32::try_from(num(0, 4)?).map_err(|_| Error::invalid("date", "year"))?;
        let month = u8::try_from(num(4, 6)?).map_err(|_| Error::invalid("date", "month"))?;
        let day = u8::try_from(num(6, 8)?).map_err(|_| Error::invalid("date", "day"))?;
        Date::new(year, month, day)
    }
}

impl std::fmt::Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// A date and a wall-clock time. Whether it is UTC, floating or in a named zone is said by the
/// value that holds it (an [`crate::Event`] keeps the zone id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DateTime {
    pub date: Date,
    hour: u8,
    minute: u8,
    second: u8,
}

impl DateTime {
    pub fn new(date: Date, hour: u8, minute: u8, second: u8) -> Result<DateTime> {
        // 60 allows a leap second, as RFC 5545 does.
        if hour > 23 || minute > 59 || second > 60 {
            return Err(Error::invalid("time", format!("{hour:02}:{minute:02}:{second:02}")));
        }
        Ok(DateTime { date, hour, minute, second })
    }

    /// 00:00 on `date` (infallible, unlike [`DateTime::new`]).
    pub fn midnight(date: Date) -> DateTime {
        DateTime { date, hour: 0, minute: 0, second: 0 }
    }

    /// Seconds since 1970-01-01T00:00 to a date-time, for front ends that read the system clock.
    pub fn from_unix(secs: i64) -> Result<DateTime> {
        let date = Date::from_day_number(secs.div_euclid(86_400))?;
        let s = secs.rem_euclid(86_400);
        let hour = u8::try_from(s / 3600).map_err(|_| Error::invalid("time", "hour"))?;
        let minute = u8::try_from(s % 3600 / 60).map_err(|_| Error::invalid("time", "minute"))?;
        let second = u8::try_from(s % 60).map_err(|_| Error::invalid("time", "second"))?;
        DateTime::new(date, hour, minute, second)
    }

    /// Seconds since 1970-01-01T00:00, reading the date-time as UTC.
    pub fn to_unix(self) -> i64 {
        self.date.day_number() * 86_400 + i64::from(self.hour) * 3600 + i64::from(self.minute) * 60 + i64::from(self.second)
    }

    pub fn hour(self) -> u8 {
        self.hour
    }
    pub fn minute(self) -> u8 {
        self.minute
    }
    pub fn second(self) -> u8 {
        self.second
    }

    /// Minutes since midnight.
    pub fn minute_of_day(self) -> u16 {
        u16::from(self.hour) * 60 + u16::from(self.minute)
    }

    /// `YYYY-MM-DDTHH:MM[:SS]` or the iCalendar basic form `YYYYMMDDTHHMMSS`; a trailing `Z` is
    /// accepted and returned as `true`.
    pub fn parse(s: &str) -> Result<(DateTime, bool)> {
        let s = s.trim();
        let (s, utc) = match s.strip_suffix('Z') {
            Some(rest) => (rest, true),
            None => (s, false),
        };
        let (d, t) = s.split_once(['T', ' ']).ok_or_else(|| Error::invalid("date-time", format!("{s:?}")))?;
        let date = Date::parse(d)?;
        let digits: String = t.chars().filter(|c| *c != ':').collect();
        if !(digits.len() == 4 || digits.len() == 6) || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::invalid("time", format!("{t:?}")));
        }
        let num = |a: usize, b: usize| -> u8 { digits.get(a..b).and_then(|p| p.parse().ok()).unwrap_or(0) };
        Ok((DateTime::new(date, num(0, 2), num(2, 4), num(4, 6))?, utc))
    }
}

impl std::fmt::Display for DateTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}T{:02}:{:02}", self.date, self.hour, self.minute)?;
        if self.second != 0 {
            write!(f, ":{:02}", self.second)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_numbers_round_trip_across_the_whole_range() {
        assert_eq!(Date::new(1970, 1, 1).unwrap().day_number(), 0);
        assert_eq!(Date::new(2000, 3, 1).unwrap().day_number(), 11_017);
        for n in (-719_162..=2_932_896).step_by(997) {
            let d = Date::from_day_number(n).unwrap();
            assert_eq!(d.day_number(), n, "{d}");
        }
        assert!(Date::from_day_number(i64::MAX).is_err());
        assert!(Date::from_day_number(i64::MIN).is_err());
    }

    #[test]
    fn validates_calendar_dates() {
        assert!(Date::new(2024, 2, 29).is_ok());
        assert!(Date::new(2023, 2, 29).is_err());
        assert!(Date::new(1900, 2, 29).is_err());
        assert!(Date::new(2026, 13, 1).is_err());
        assert!(Date::new(2026, 4, 31).is_err());
        assert!(Date::new(0, 1, 1).is_err());
        assert!(Date::new(10_000, 1, 1).is_err());
    }

    #[test]
    fn weekdays_and_months() {
        assert_eq!(Date::new(2026, 10, 22).unwrap().weekday(), Weekday::Thursday);
        assert_eq!(Date::new(1, 1, 1).unwrap().weekday(), Weekday::Monday);
        let jan31 = Date::new(2026, 1, 31).unwrap();
        assert_eq!(jan31.add_months(1).unwrap(), Date::new(2026, 2, 28).unwrap());
        assert_eq!(jan31.add_months(-1).unwrap(), Date::new(2025, 12, 31).unwrap());
        assert!(Date::new(9999, 12, 1).unwrap().add_months(1).is_err());
        assert!(Date::new(1, 1, 1).unwrap().add_days(-1).is_err());
        assert!(Date::new(2026, 1, 1).unwrap().add_days(i64::MAX).is_err());
    }

    #[test]
    fn parses_both_forms() {
        assert_eq!(Date::parse("2026-10-22").unwrap(), Date::new(2026, 10, 22).unwrap());
        assert_eq!(Date::parse("20261022").unwrap(), Date::new(2026, 10, 22).unwrap());
        for bad in ["", "2026-1-2", "abcd-ef-gh", "2026-02-30", "２０２６１０２２", "20261022999"] {
            assert!(Date::parse(bad).is_err(), "{bad:?}");
        }
        let (dt, utc) = DateTime::parse("20261022T093000Z").unwrap();
        assert!(utc);
        assert_eq!((dt.hour(), dt.minute()), (9, 30));
        let (dt, utc) = DateTime::parse("2026-10-22T18:05").unwrap();
        assert!(!utc);
        assert_eq!(dt.to_string(), "2026-10-22T18:05");
        let t = DateTime::from_unix(1_792_670_400).unwrap();
        assert_eq!(t.to_string(), "2026-10-22T12:00");
        assert!(DateTime::from_unix(i64::MAX).is_err());
        assert!(DateTime::from_unix(i64::MIN).is_err());
        for bad in ["2026-10-22", "2026-10-22T25:00", "2026-10-22T9", "T", "2026-10-22Tïï:00"] {
            assert!(DateTime::parse(bad).is_err(), "{bad:?}");
        }
    }
}
