//! Recurrence rules (`RRULE`, RFC 5545 §3.3.10): which days a repeating event falls on.
//!
//! Supported: `FREQ` `DAILY`, `WEEKLY`, `MONTHLY` and `YEARLY` with `INTERVAL`, `COUNT`, `UNTIL`,
//! `BYDAY` (with ordinals such as `2TU` or `-1FR` in monthly and yearly rules), `BYMONTHDAY`
//! (negative counts from the month's end), `BYMONTH`, `BYSETPOS` and `WKST`. That covers what
//! calendar programs write. Rules with parts it does not know (`BYHOUR`, `BYWEEKNO`, `BYYEARDAY`,
//! sub-daily frequencies) fail to parse, and the event then shows only its first occurrence;
//! the rule text itself is always kept, so nothing is lost on a round trip.
//!
//! Every loop is bounded: a rule is followed for at most [`MAX_PERIODS`] periods, and a query
//! starts near the dates it asks about rather than at the first occurrence, unless `COUNT`
//! makes the count from the start matter.

use crate::date::Weekday;
use crate::{Date, Error, Result, date::days_in_month};

/// Most periods (days, weeks, months or years) one query walks.
pub const MAX_PERIODS: u32 = 20_000;
/// Largest `COUNT` honoured; higher counts are treated as this.
pub const MAX_COUNT: u32 = 10_000;
const MAX_LIST: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// A parsed rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub freq: Freq,
    pub interval: u32,
    pub count: Option<u32>,
    /// The last day an occurrence may start, and on that day the latest start minute (when
    /// `UNTIL` has a time).
    pub until: Option<(Date, Option<u16>)>,
    /// Weekdays, each with an optional ordinal (`1` first, `-1` last).
    pub by_day: Vec<(Option<i8>, Weekday)>,
    pub by_month_day: Vec<i8>,
    pub by_month: Vec<u8>,
    pub by_set_pos: Vec<i16>,
    pub week_start: Weekday,
}

fn bad(detail: impl Into<String>) -> Error {
    Error::invalid("RRULE", detail)
}

fn weekday(code: &str) -> Option<Weekday> {
    Some(match code {
        "MO" => Weekday::Monday,
        "TU" => Weekday::Tuesday,
        "WE" => Weekday::Wednesday,
        "TH" => Weekday::Thursday,
        "FR" => Weekday::Friday,
        "SA" => Weekday::Saturday,
        "SU" => Weekday::Sunday,
        _ => return None,
    })
}

fn code(w: Weekday) -> &'static str {
    match w {
        Weekday::Monday => "MO",
        Weekday::Tuesday => "TU",
        Weekday::Wednesday => "WE",
        Weekday::Thursday => "TH",
        Weekday::Friday => "FR",
        Weekday::Saturday => "SA",
        Weekday::Sunday => "SU",
    }
}

fn list<T>(v: &str, item: impl Fn(&str) -> Option<T>) -> Result<Vec<T>> {
    let parts: Vec<&str> = v.split(',').collect();
    if parts.len() > MAX_LIST {
        return Err(bad("list too long"));
    }
    parts.into_iter().map(|p| item(p.trim()).ok_or_else(|| bad(format!("bad value {p:?}")))).collect()
}

impl Rule {
    pub fn parse(text: &str) -> Result<Rule> {
        let text = text.trim();
        let text = text.strip_prefix("RRULE:").unwrap_or(text);
        let mut freq = None;
        let mut rule = Rule {
            freq: Freq::Daily,
            interval: 1,
            count: None,
            until: None,
            by_day: Vec::new(),
            by_month_day: Vec::new(),
            by_month: Vec::new(),
            by_set_pos: Vec::new(),
            week_start: Weekday::Monday,
        };
        for part in text.split(';').filter(|p| !p.trim().is_empty()).take(32) {
            let (key, value) = part.split_once('=').ok_or_else(|| bad(format!("{part:?} has no '='")))?;
            let value = value.trim().to_ascii_uppercase();
            match key.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match value.as_str() {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        other => return Err(bad(format!("FREQ={other} is not supported"))),
                    })
                }
                "INTERVAL" => rule.interval = value.parse().ok().filter(|n| (1..=1000).contains(n)).ok_or_else(|| bad("INTERVAL"))?,
                "COUNT" => rule.count = Some(value.parse::<u32>().map_err(|_| bad("COUNT"))?.clamp(1, MAX_COUNT)),
                "UNTIL" => {
                    let date = Date::parse(value.get(..8).unwrap_or(""))?;
                    let minute = match value.get(8..) {
                        Some(t) if t.starts_with('T') => {
                            let h: u16 = t.get(1..3).and_then(|v| v.parse().ok()).ok_or_else(|| bad("UNTIL time"))?;
                            let m: u16 = t.get(3..5).and_then(|v| v.parse().ok()).ok_or_else(|| bad("UNTIL time"))?;
                            if h > 23 || m > 59 {
                                return Err(bad("UNTIL time"));
                            }
                            Some(h * 60 + m)
                        }
                        _ => None,
                    };
                    rule.until = Some((date, minute));
                }
                "BYDAY" => {
                    rule.by_day = list(&value, |d| {
                        let split = d.len().checked_sub(2)?;
                        let (n, w) = (d.get(..split)?, d.get(split..)?);
                        let ordinal = if n.is_empty() { None } else { Some(n.parse::<i8>().ok().filter(|n| *n != 0 && (-53..=53).contains(n))?) };
                        Some((ordinal, weekday(w)?))
                    })?
                }
                "BYMONTHDAY" => rule.by_month_day = list(&value, |d| d.parse::<i8>().ok().filter(|n| *n != 0 && (-31..=31).contains(n)))?,
                "BYMONTH" => rule.by_month = list(&value, |d| d.parse::<u8>().ok().filter(|n| (1..=12).contains(n)))?,
                "BYSETPOS" => rule.by_set_pos = list(&value, |d| d.parse::<i16>().ok().filter(|n| *n != 0 && (-366..=366).contains(n)))?,
                "WKST" => rule.week_start = weekday(&value).ok_or_else(|| bad("WKST"))?,
                other => return Err(bad(format!("{other} is not supported"))),
            }
        }
        rule.freq = freq.ok_or_else(|| bad("no FREQ"))?;
        if rule.by_day.iter().any(|(n, _)| n.is_some()) && !matches!(rule.freq, Freq::Monthly | Freq::Yearly) {
            return Err(bad("numbered BYDAY needs FREQ=MONTHLY or YEARLY"));
        }
        Ok(rule)
    }

    /// The rule as `RRULE` text (without the `RRULE:` name).
    pub fn to_text(&self) -> String {
        let freq = match self.freq {
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        };
        let mut out = format!("FREQ={freq}");
        if self.interval != 1 {
            out.push_str(&format!(";INTERVAL={}", self.interval));
        }
        if let Some(c) = self.count {
            out.push_str(&format!(";COUNT={c}"));
        }
        if let Some((d, m)) = self.until {
            out.push_str(&format!(";UNTIL={:04}{:02}{:02}", d.year(), d.month(), d.day()));
            if let Some(m) = m {
                out.push_str(&format!("T{:02}{:02}00", m / 60, m % 60));
            }
        }
        let join = |v: Vec<String>| v.join(",");
        if !self.by_day.is_empty() {
            out.push_str(";BYDAY=");
            out.push_str(&join(self.by_day.iter().map(|(n, w)| format!("{}{}", n.map(|n| n.to_string()).unwrap_or_default(), code(*w))).collect()));
        }
        if !self.by_month_day.is_empty() {
            out.push_str(";BYMONTHDAY=");
            out.push_str(&join(self.by_month_day.iter().map(ToString::to_string).collect()));
        }
        if !self.by_month.is_empty() {
            out.push_str(";BYMONTH=");
            out.push_str(&join(self.by_month.iter().map(ToString::to_string).collect()));
        }
        if !self.by_set_pos.is_empty() {
            out.push_str(";BYSETPOS=");
            out.push_str(&join(self.by_set_pos.iter().map(ToString::to_string).collect()));
        }
        if self.week_start != Weekday::Monday {
            out.push_str(&format!(";WKST={}", code(self.week_start)));
        }
        out
    }

    /// Start days of the occurrences of an event first starting on `start` (at minute
    /// `start_minute`) that fall within `from..=to`, without the `excluded` days. The first
    /// occurrence is always `start` itself, as RFC 5545 counts it even when the rule would not
    /// produce it.
    pub fn occurrences(&self, start: Date, start_minute: u16, from: Date, to: Date, excluded: &[Date]) -> Vec<Date> {
        let mut out = Vec::new();
        if to < from || to < start {
            return out;
        }
        let mut seen: u32 = 0;
        let limit = self.count;
        // Without COUNT the walk may begin just before `from`.
        let first_period = if limit.is_none() { self.periods_before(start, from) } else { 0 };
        let mut emit = |d: Date, out: &mut Vec<Date>| -> bool {
            if let Some((until, minute)) = self.until
                && (d > until || (d == until && minute.is_some_and(|m| start_minute > m)))
            {
                return false;
            }
            seen = seen.saturating_add(1);
            if limit.is_some_and(|c| seen > c) {
                return false;
            }
            if d > to {
                return false;
            }
            if d >= from && !excluded.contains(&d) {
                out.push(d);
            }
            true
        };
        if first_period == 0 && !emit(start, &mut out) {
            return out;
        }
        for k in first_period..first_period.saturating_add(MAX_PERIODS) {
            let Some(period) = self.period_start(start, k) else { break };
            if period > to {
                break;
            }
            let mut days = self.candidates(start, period);
            days.sort();
            days.dedup();
            let days = self.set_positions(days);
            for d in days {
                if d <= start {
                    continue;
                }
                if !emit(d, &mut out) {
                    return out;
                }
            }
        }
        out
    }

    /// How many whole periods lie between the first period and the one holding `from`, minus
    /// one for safety (multi-day spans and week starts).
    fn periods_before(&self, start: Date, from: Date) -> u32 {
        let interval = i64::from(self.interval.max(1));
        let units = match self.freq {
            Freq::Daily => from.day_number() - start.day_number(),
            Freq::Weekly => (from.day_number() - start.day_number()).div_euclid(7),
            Freq::Monthly => (i64::from(from.year()) * 12 + i64::from(from.month())) - (i64::from(start.year()) * 12 + i64::from(start.month())),
            Freq::Yearly => i64::from(from.year()) - i64::from(start.year()),
        };
        u32::try_from((units.div_euclid(interval) - 1).max(0)).unwrap_or(u32::MAX)
    }

    /// The first day of period `k` (0 holds `start`).
    fn period_start(&self, start: Date, k: u32) -> Option<Date> {
        let step = i64::from(k).checked_mul(i64::from(self.interval))?;
        match self.freq {
            Freq::Daily => start.add_days(step).ok(),
            Freq::Weekly => {
                let back = (i64::from(start.weekday().index()) - i64::from(self.week_start.index())).rem_euclid(7);
                start.add_days(step.checked_mul(7)?.checked_sub(back)?).ok()
            }
            Freq::Monthly => start.first_of_month().add_months(i32::try_from(step).ok()?).ok(),
            Freq::Yearly => Date::new(start.year().checked_add(i32::try_from(step).ok()?)?, 1, 1).ok(),
        }
    }

    /// The days a period offers before `BYSETPOS`, `COUNT` and `UNTIL`.
    fn candidates(&self, start: Date, period: Date) -> Vec<Date> {
        let month_ok = |d: &Date| self.by_month.is_empty() || self.by_month.contains(&d.month());
        let weekday_ok = |d: &Date| self.by_day.is_empty() || self.by_day.iter().any(|(_, w)| *w == d.weekday());
        let monthday_ok = |d: &Date| self.by_month_day.is_empty() || self.by_month_day.iter().any(|n| month_day(*d, *n));
        match self.freq {
            Freq::Daily => [period].into_iter().filter(|d| month_ok(d) && weekday_ok(d) && monthday_ok(d)).collect(),
            Freq::Weekly => {
                let days = (0..7).filter_map(|i| period.add_days(i).ok());
                if self.by_day.is_empty() {
                    days.filter(|d| d.weekday() == start.weekday()).filter(month_ok).collect()
                } else {
                    days.filter(|d| weekday_ok(d) && month_ok(d)).collect()
                }
            }
            Freq::Monthly => {
                if !month_ok(&period) {
                    return Vec::new();
                }
                self.in_month(start, period.year(), period.month(), false)
            }
            Freq::Yearly => {
                let months: Vec<u8> = if !self.by_month.is_empty() {
                    self.by_month.clone()
                } else if self.by_day.iter().any(|(n, _)| n.is_some()) && self.by_month_day.is_empty() {
                    // `FREQ=YEARLY;BYDAY=20MO`: ordinals count through the whole year.
                    return self.in_year(period.year());
                } else if !self.by_day.is_empty() || !self.by_month_day.is_empty() {
                    (1..=12).collect()
                } else {
                    vec![start.month()]
                };
                months.into_iter().flat_map(|m| self.in_month(start, period.year(), m, true)).collect()
            }
        }
    }

    /// The days of one month a monthly (or yearly, per month) rule offers.
    fn in_month(&self, start: Date, year: i32, month: u8, yearly: bool) -> Vec<Date> {
        let len = days_in_month(year, month);
        let all = (1..=len).filter_map(|d| Date::new(year, month, d).ok());
        if self.by_day.is_empty() && self.by_month_day.is_empty() {
            // Only the start's day of the month; months without it (the 31st) are skipped.
            if yearly && month != start.month() && self.by_month.is_empty() {
                return Vec::new();
            }
            return Date::new(year, month, start.day()).ok().into_iter().collect();
        }
        all.filter(|d| {
            let md = self.by_month_day.is_empty() || self.by_month_day.iter().any(|n| month_day(*d, *n));
            let wd = self.by_day.is_empty() || self.by_day.iter().any(|(n, w)| *w == d.weekday() && n.is_none_or(|n| nth_in_month(*d, n)));
            md && wd
        })
        .collect()
    }

    /// `BYDAY` with ordinals across a whole year.
    fn in_year(&self, year: i32) -> Vec<Date> {
        let (Ok(first), Ok(last)) = (Date::new(year, 1, 1), Date::new(year, 12, 31)) else { return Vec::new() };
        let len = last.day_number() - first.day_number() + 1;
        (0..len)
            .filter_map(|i| first.add_days(i).ok())
            .filter(|d| {
                self.by_day.iter().any(|(n, w)| {
                    *w == d.weekday()
                        && n.is_none_or(|n| {
                            let k = i8::try_from((d.day_number() - first.day_number()) / 7 + 1).unwrap_or(i8::MAX);
                            let from_end = i8::try_from((last.day_number() - d.day_number()) / 7 + 1).unwrap_or(i8::MAX);
                            if n > 0 { k == n } else { from_end == -n }
                        })
                })
            })
            .collect()
    }

    fn set_positions(&self, days: Vec<Date>) -> Vec<Date> {
        if self.by_set_pos.is_empty() {
            return days;
        }
        let len = i64::try_from(days.len()).unwrap_or(i64::MAX);
        let mut picked: Vec<Date> = self
            .by_set_pos
            .iter()
            .filter_map(|p| {
                let i = if *p > 0 { i64::from(*p) - 1 } else { len + i64::from(*p) };
                usize::try_from(i).ok().and_then(|i| days.get(i).copied())
            })
            .collect();
        picked.sort();
        picked.dedup();
        picked
    }
}

/// Whether `d` is day `n` of its month (`-1` the last).
fn month_day(d: Date, n: i8) -> bool {
    let len = i16::from(days_in_month(d.year(), d.month()));
    let n = i16::from(n);
    let want = if n > 0 { n } else { len + n + 1 };
    i16::from(d.day()) == want
}

/// Whether `d` is the `n`th of its weekday in its month (`-1` the last).
fn nth_in_month(d: Date, n: i8) -> bool {
    let day = i16::from(d.day());
    let len = i16::from(days_in_month(d.year(), d.month()));
    if n > 0 { (day - 1) / 7 + 1 == i16::from(n) } else { (len - day) / 7 + 1 == -i16::from(n) }
}

#[cfg(test)]
mod tests;
