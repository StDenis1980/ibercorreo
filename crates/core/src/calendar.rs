//! Calendars and events.

use serde::{Deserialize, Serialize};

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
    /// The raw `RRULE`, preserved so a round trip loses nothing. Expansion is not built yet.
    pub rrule: Option<String>,
}

impl Event {
    pub fn occurs_on(&self, day: Date) -> bool {
        self.when.covers(day)
    }

    /// Minutes since midnight the event starts on `day` (0 for all-day or for later days of a long event).
    pub fn start_minute_on(&self, day: Date) -> u16 {
        match self.when {
            When::Timed { start, .. } if start.date == day => start.minute_of_day(),
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
