//! Calendars and events.

use serde::{Deserialize, Serialize};

use crate::recur::Rule;
use crate::{Date, DateTime, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    pub id: u32,
    pub name: String,
    /// `#RRGGBB`, shown as the event colour.
    pub color: String,
}

/// When an event happens. All-day events span whole dates, `end` exclusive (as in iCalendar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum When {
    AllDay { start: Date, end: Date },
    Timed { start: DateTime, end: DateTime, utc: bool },
}

impl When {
    pub fn all_day(start: Date, end: Date) -> Result<When> {
        if end <= start {
            return Err(Error::invalid("event", format!("all-day end {end} not after start {start}")));
        }
        Ok(When::AllDay { start, end })
    }

    pub fn timed(start: DateTime, end: DateTime, utc: bool) -> Result<When> {
        if end < start {
            return Err(Error::invalid("event", format!("end {end} before start {start}")));
        }
        Ok(When::Timed { start, end, utc })
    }

    pub fn start_date(&self) -> Date {
        match self {
            When::AllDay { start, .. } => *start,
            When::Timed { start, .. } => start.date,
        }
    }

    /// The same span moved by `days`.
    pub fn shifted(self, days: i64) -> Result<When> {
        Ok(match self {
            When::AllDay { start, end } => When::AllDay { start: start.add_days(days)?, end: end.add_days(days)? },
            When::Timed { mut start, mut end, utc } => {
                start.date = start.date.add_days(days)?;
                end.date = end.date.add_days(days)?;
                When::Timed { start, end, utc }
            }
        })
    }

    /// The last date any part of the span falls on.
    pub fn last_date(&self) -> Date {
        match self {
            When::AllDay { start, end } => end.add_days(-1).unwrap_or(*start).max(*start),
            When::Timed { start, end, .. } => {
                let ends_at_midnight = end.minute_of_day() == 0 && end.second() == 0 && end.date > start.date;
                if ends_at_midnight { end.date.add_days(-1).unwrap_or(start.date) } else { end.date }
            }
        }
    }

    /// Whether any part of the span falls within `from..=to`.
    pub fn touches(&self, from: Date, to: Date) -> bool {
        self.start_date() <= to && self.last_date() >= from
    }

    /// How many dates the span touches, at least one.
    fn days_touched(&self) -> i64 {
        match self {
            When::AllDay { start, end } => (end.day_number() - start.day_number()).max(1),
            When::Timed { start, end, .. } => (end.date.day_number() - start.date.day_number() + 1).max(1),
        }
    }

    /// Whether any part of the event falls on `day` (ignoring recurrence).
    pub fn covers(&self, day: Date) -> bool {
        match self {
            When::AllDay { start, end } => *start <= day && day < *end,
            When::Timed { start, end, .. } => {
                // An event ending exactly at midnight does not spill onto the next day.
                let ends_at_midnight = end.minute_of_day() == 0 && end.second() == 0 && end.date > start.date;
                start.date <= day && (day < end.date || (day == end.date && !ends_at_midnight))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    pub calendar: u32,
    /// The iCalendar UID, stable across servers and files.
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    pub when: When,
    /// The `TZID` the times are in, if any (time zones are applied from M3, see docs/gaps.md).
    pub tzid: Option<String>,
    /// The raw `RRULE`, preserved so a round trip loses nothing. Expanded by [`crate::recur`];
    /// a rule it cannot read leaves only the first occurrence.
    pub rrule: Option<String>,
    /// Days whose occurrence was cancelled (`EXDATE`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exdates: Vec<Date>,
    /// For a changed single occurrence of a repeating event: the day it originally fell on
    /// (`RECURRENCE-ID`). It shares the series' `uid` and replaces that day's occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<Date>,
}

/// Most occurrences one query returns.
pub const MAX_OCCURRENCES: usize = 5_000;

impl Event {
    /// The repeat rule, when there is one Iber Correo can follow.
    pub fn rule(&self) -> Option<Rule> {
        self.rrule.as_deref().and_then(|r| Rule::parse(r).ok())
    }

    /// Each occurrence touching `from..=to`, in order, as its own span.
    pub fn occurrences(&self, from: Date, to: Date) -> Vec<When> {
        let Some(rule) = self.rule() else {
            let w = self.when;
            return if w.touches(from, to) { vec![w] } else { Vec::new() };
        };
        let first = self.when.start_date();
        let minute = match self.when {
            When::Timed { start, .. } => start.minute_of_day(),
            When::AllDay { .. } => 0,
        };
        let earliest = from.add_days(1 - self.when.days_touched()).unwrap_or(from);
        rule.occurrences(first, minute, earliest, to, &self.exdates)
            .into_iter()
            .filter_map(|d| self.when.shifted(d.day_number() - first.day_number()).ok())
            .filter(|w| w.touches(from, to))
            .take(MAX_OCCURRENCES)
            .collect()
    }

    /// The occurrence that falls on `day`, if any.
    pub fn occurrence_on(&self, day: Date) -> Option<When> {
        self.occurrences(day, day).into_iter().next()
    }

    pub fn occurs_on(&self, day: Date) -> bool {
        self.occurrence_on(day).is_some()
    }

    /// Minutes since midnight the event starts on `day` (0 for all-day or for later days of a long event).
    pub fn start_minute_on(&self, day: Date) -> u16 {
        match self.occurrence_on(day) {
            Some(When::Timed { start, .. }) if start.date == day => start.minute_of_day(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    #[test]
    fn all_day_end_is_exclusive() {
        let w = When::all_day(d(2026, 10, 22), d(2026, 10, 24)).unwrap();
        assert!(w.covers(d(2026, 10, 22)));
        assert!(w.covers(d(2026, 10, 23)));
        assert!(!w.covers(d(2026, 10, 24)));
        assert!(When::all_day(d(2026, 10, 22), d(2026, 10, 22)).is_err());
    }

    #[test]
    fn timed_events_ending_at_midnight_stay_on_their_day() {
        let start = DateTime::new(d(2026, 10, 22), 22, 0, 0).unwrap();
        let end = DateTime::new(d(2026, 10, 23), 0, 0, 0).unwrap();
        let w = When::timed(start, end, false).unwrap();
        assert!(w.covers(d(2026, 10, 22)));
        assert!(!w.covers(d(2026, 10, 23)));
        assert!(When::timed(end, start, false).is_err());
    }
}
