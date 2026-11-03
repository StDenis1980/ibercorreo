//! Outlook's recurrence pattern (`PidLidAppointmentRecur`, MS-OXOCAL 2.2.1.44.1) as an iCalendar
//! `RRULE`, with the days taken out of the series.

use ibercorreo_core::{Date, DateTime};

use crate::ndb::{u16_at, u32_at};

const DAILY: u16 = 0x200a;
const WEEKLY: u16 = 0x200b;
const MONTHLY: u16 = 0x200c;
const YEARLY: u16 = 0x200d;

const DAY: u16 = 0x0000;
const WEEK: u16 = 0x0001;
const MONTH: u16 = 0x0002;
const MONTH_NTH: u16 = 0x0003;
const MONTH_END: u16 = 0x0004;

const END_AFTER_DATE: u32 = 0x2021;
const END_AFTER_COUNT: u32 = 0x2022;

/// Deleted or changed days read, at most.
const MAX_DATES: usize = 10_000;

const DAYS: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];

/// A series read from a recurrence pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Series {
    pub(crate) rrule: String,
    /// Days whose occurrence was deleted, not counting those that were changed instead.
    pub(crate) deleted: Vec<Date>,
    /// Days whose occurrence was changed (each has an exception with its new details).
    pub(crate) changed: Vec<Date>,
}

/// The day `minutes` after 1601-01-01 00:00, the pattern's way of naming days.
fn day(minutes: u32) -> Option<Date> {
    const UNIX_1601: i64 = 11_644_473_600;
    DateTime::from_unix(i64::from(minutes) * 60 - UNIX_1601).ok().map(|t| t.date)
}

fn by_day(mask: u32) -> String {
    DAYS.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, d)| *d).collect::<Vec<_>>().join(",")
}

/// Reads a pattern; `None` for one that is damaged or not Gregorian (Hijri patterns).
pub(crate) fn read(b: &[u8]) -> Option<Series> {
    let mut at = 4usize;
    let u16_next = |at: &mut usize| {
        let v = u16_at(b, *at);
        *at += 2;
        v
    };
    let frequency = u16_next(&mut at)?;
    let pattern = u16_next(&mut at)?;
    let _calendar = u16_next(&mut at)?;
    let u32_next = |at: &mut usize| {
        let v = u32_at(b, *at);
        *at += 4;
        v
    };
    let _first = u32_next(&mut at)?;
    let period = u32_next(&mut at)?;
    let _sliding = u32_next(&mut at)?;
    let (a, n) = match pattern {
        DAY => (0, 0),
        WEEK | MONTH | MONTH_END => (u32_next(&mut at)?, 0),
        MONTH_NTH => (u32_next(&mut at)?, u32_next(&mut at)?),
        _ => return None,
    };
    let end_type = u32_next(&mut at)?;
    let count = u32_next(&mut at)?;
    let first_dow = u32_next(&mut at)?;
    let dates = |at: &mut usize| -> Option<Vec<Date>> {
        let n = usize::try_from(u32_next(at)?).ok()?;
        if n > MAX_DATES {
            return None;
        }
        (0..n).map(|_| u32_next(at).and_then(day)).collect()
    };
    let deleted = dates(&mut at)?;
    let changed = dates(&mut at)?;
    let start = u32_next(&mut at).and_then(day)?;
    let end = u32_next(&mut at).and_then(day)?;

    let mut rule = match (frequency, pattern) {
        (DAILY, DAY) => format!("FREQ=DAILY;INTERVAL={}", (period / 1440).max(1)),
        // "Every weekday" is saved as a weekly pattern with a daily frequency.
        (DAILY | WEEKLY, WEEK) => format!("FREQ=WEEKLY;INTERVAL={};BYDAY={}", period.max(1), by_day(a)),
        (MONTHLY, MONTH) => format!("FREQ=MONTHLY;INTERVAL={};BYMONTHDAY={}", period.max(1), a.clamp(1, 31)),
        (MONTHLY, MONTH_END) => format!("FREQ=MONTHLY;INTERVAL={};BYMONTHDAY=-1", period.max(1)),
        (MONTHLY, MONTH_NTH) => format!("FREQ=MONTHLY;INTERVAL={};BYDAY={};BYSETPOS={}", period.max(1), by_day(a), nth(n)),
        (YEARLY, MONTH) => format!("FREQ=YEARLY;INTERVAL={};BYMONTH={};BYMONTHDAY={}", (period / 12).max(1), start.month(), a.clamp(1, 31)),
        (YEARLY, MONTH_END) => format!("FREQ=YEARLY;INTERVAL={};BYMONTH={};BYMONTHDAY=-1", (period / 12).max(1), start.month()),
        (YEARLY, MONTH_NTH) => {
            format!("FREQ=YEARLY;INTERVAL={};BYMONTH={};BYDAY={};BYSETPOS={}", (period / 12).max(1), start.month(), by_day(a), nth(n))
        }
        _ => return None,
    };
    if rule.ends_with("BYDAY=") || rule.contains("BYDAY=;") {
        return None;
    }
    match end_type {
        END_AFTER_DATE => rule.push_str(&format!(";UNTIL={:04}{:02}{:02}", end.year(), end.month(), end.day())),
        END_AFTER_COUNT => rule.push_str(&format!(";COUNT={}", count.clamp(1, 9999))),
        _ => {}
    }
    if let Some(d) = DAYS.get(usize::try_from(first_dow).unwrap_or(0)).filter(|_| first_dow != 0) {
        rule.push_str(&format!(";WKST={d}"));
    }
    let deleted = deleted.into_iter().filter(|d| !changed.contains(d)).collect();
    Some(Series { rrule: rule, deleted, changed })
}

/// `BYSETPOS` for the pattern's N: first to fourth, or 5 for the last.
fn nth(n: u32) -> i32 {
    match n {
        1..=4 => n as i32,
        _ => -1,
    }
}

#[cfg(test)]
mod tests;
