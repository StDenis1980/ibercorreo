//! Calendars and events.

use serde::{Deserialize, Serialize};

use crate::recur::Rule;
use crate::zone::Zone;
use crate::{Date, DateTime, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    pub id: u32,
    pub name: String,
    /// `#RRGGBB`, shown as the event colour.
    pub color: String,
    /// The calendar on a CalDAV server this one mirrors; `None` for a calendar kept only here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<CalendarRemote>,
}

/// Where a calendar lives on its CalDAV server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarRemote {
    /// The collection's path on the server.
    pub href: String,
    /// The server's change marker when last synced; while it is the same, nothing changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctag: Option<String>,
}

/// The server resource an event came from. A repeating event and its changed occurrences share
/// one resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRemote {
    pub href: String,
    /// The resource's version when last synced; empty when it must be fetched again.
    pub etag: String,
    /// Changed here since the last sync, so the next one sends it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dirty: bool,
    /// The server's text of the resource when last synced, so writing it back keeps what Iber
    /// Correo does not read (attendees, categories, time zone definitions). Empty when larger
    /// than [`MAX_REMOTE_RAW`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw: String,
}

/// Largest server copy kept per event resource.
pub const MAX_REMOTE_RAW: usize = 256 * 1024;

impl EventRemote {
    pub fn new(href: &str, etag: &str, raw: &str) -> EventRemote {
        let raw = if raw.len() <= MAX_REMOTE_RAW { raw.to_string() } else { String::new() };
        EventRemote { href: href.to_string(), etag: etag.to_string(), dirty: false, raw }
    }
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

    /// For a timed span, the minutes of `day` it fills, `start..end` with `end` up to 1440 (it
    /// runs past midnight). `None` for all-day spans and days it does not touch. A zero-length
    /// span still fills its start minute, so it can be drawn.
    pub fn minutes_on(&self, day: Date) -> Option<(u16, u16)> {
        let When::Timed { start, end, .. } = *self else { return None };
        if !self.covers(day) && !(start.date == day && end == start) {
            return None;
        }
        let from = if start.date == day { start.minute_of_day() } else { 0 };
        let to = if end.date == day { end.minute_of_day() } else { MINUTES_PER_DAY };
        Some((from.min(MINUTES_PER_DAY - 1), to.clamp(from.saturating_add(1), MINUTES_PER_DAY)))
    }

    /// The span as wall-clock times in `view`. UTC times and times in the event's own zone `own`
    /// are converted; floating times and all-day dates stay as they are.
    pub fn in_zone(self, own: Option<&Zone>, view: &Zone) -> When {
        let When::Timed { start, end, utc } = self else { return self };
        let convert = |t: DateTime| match (utc, own) {
            (true, _) => view.from_utc(t),
            (false, Some(own)) if own != view => view.from_utc(own.to_utc(t)),
            _ => t,
        };
        let (start, end) = (convert(start), convert(end));
        // A clock change can not put the end before the start, but keep the invariant anyway.
        When::Timed { start, end: end.max(start), utc: false }
    }

    /// When the span starts, in UTC. Times in `own` zone are converted from it; floating times
    /// and all-day dates (from midnight) are read in `view`, where the person is.
    pub fn start_utc(&self, own: Option<&Zone>, view: &Zone) -> DateTime {
        match *self {
            When::Timed { start, utc: true, .. } => start,
            When::Timed { start, .. } => own.unwrap_or(view).to_utc(start),
            When::AllDay { start, .. } => view.to_utc(DateTime::midnight(start)),
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

pub const MINUTES_PER_DAY: u16 = 24 * 60;

/// Side-by-side columns for a day's timed events, as Outlook draws a busy day: events that
/// overlap share the width, each in the first column free at its start. `spans` are
/// `(start, end)` minutes in display order; the result gives each its `(column, columns)`, where
/// `columns` is how many the group of overlapping events it belongs to needs.
pub fn columns(spans: &[(u16, u16)]) -> Vec<(usize, usize)> {
    let mut out = vec![(0, 1); spans.len()];
    // Ends of the events in each column of the current group, and where that group began.
    let mut ends: Vec<u16> = Vec::new();
    let mut group_start = 0;
    let mut group_end = 0u16;
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans.get(i).map_or((0, std::cmp::Reverse(0)), |&(s, e)| (s, std::cmp::Reverse(e))));
    for (n, &i) in order.iter().enumerate() {
        let Some(&(start, end)) = spans.get(i) else { continue };
        let end = end.max(start.saturating_add(1));
        if start >= group_end && !ends.is_empty() {
            close_group(&mut out, order.get(group_start..n).unwrap_or(&[]), ends.len());
            ends.clear();
            group_start = n;
        }
        let column = match ends.iter().position(|&e| e <= start) {
            Some(c) => c,
            None => {
                ends.push(0);
                ends.len() - 1
            }
        };
        if let Some(e) = ends.get_mut(column) {
            *e = end;
        }
        if let Some(slot) = out.get_mut(i) {
            slot.0 = column;
        }
        group_end = group_end.max(end);
    }
    close_group(&mut out, order.get(group_start..).unwrap_or(&[]), ends.len());
    out
}

fn close_group(out: &mut [(usize, usize)], members: &[usize], width: usize) {
    for &i in members {
        if let Some(slot) = out.get_mut(i) {
            slot.1 = width.max(1);
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
    /// The `TZID` the times are in, if any: an IANA id, a Windows name or a vendor id (see
    /// [`Zone::named`]). Times with no zone and not UTC are floating: the same wall time
    /// wherever they are seen.
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
    /// Reminders, in minutes before the start (negative: after it), as `VALARM` triggers. At most
    /// [`MAX_REMINDERS`], each within [`MAX_REMINDER_MINUTES`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reminders: Vec<i32>,
    /// The CalDAV resource this event was synced from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<EventRemote>,
}

pub const MAX_REMINDERS: usize = 8;
/// Four weeks, the furthest ahead a reminder may be set.
pub const MAX_REMINDER_MINUTES: i32 = 4 * 7 * 24 * 60;

/// Reminder offsets made safe to keep: clamped to four weeks either way, duplicates dropped,
/// at most [`MAX_REMINDERS`], earliest-firing first.
pub fn clean_reminders(mut minutes: Vec<i64>) -> Vec<i32> {
    minutes.truncate(64);
    let mut v: Vec<i32> =
        minutes.into_iter().map(|m| i32::try_from(m.clamp(i64::from(-MAX_REMINDER_MINUTES), i64::from(MAX_REMINDER_MINUTES))).unwrap_or(0)).collect();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v.dedup();
    v.truncate(MAX_REMINDERS);
    v
}

/// Most occurrences one query returns.
pub const MAX_OCCURRENCES: usize = 5_000;

impl Event {
    /// The repeat rule, when there is one Tinta Correo can follow.
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

    /// The zone the event's times are in: `None` for floating or UTC times, or a zone id the
    /// database does not know (its times are then shown as written).
    pub fn zone(&self) -> Option<Zone> {
        self.tzid.as_deref().and_then(Zone::named)
    }

    /// Whether [`Event::occurrences_in`] has to convert this event's times for `view`.
    fn needs_converting(&self, own: Option<&Zone>, view: &Zone) -> bool {
        match self.when {
            When::AllDay { .. } => false,
            When::Timed { utc: true, .. } => !view.is_utc(),
            When::Timed { .. } => own.is_some_and(|z| z != view),
        }
    }

    /// Each occurrence touching `from..=to` as seen in zone `view`, paired with the same
    /// occurrence in the event's own times (whose start date is what `EXDATE` and
    /// `RECURRENCE-ID` name). The rule repeats in the event's own zone, so a weekly 09:00 in
    /// Lisbon stays 09:00 there across summer time, as RFC 5545 says.
    pub fn occurrences_in(&self, from: Date, to: Date, view: &Zone) -> Vec<(When, When)> {
        let own = self.zone();
        if !self.needs_converting(own.as_ref(), view) {
            return self.occurrences(from, to).into_iter().map(|w| (w, w)).collect();
        }
        // Offsets are under a day, so a day either side catches everything that moves in.
        let wide_from = from.add_days(-1).unwrap_or(from);
        let wide_to = to.add_days(1).unwrap_or(to);
        self.occurrences(wide_from, wide_to).into_iter().map(|w| (w, w.in_zone(own.as_ref(), view))).filter(|(_, v)| v.touches(from, to)).collect()
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

    #[test]
    fn minutes_on_clips_to_the_day() {
        let at = |day: Date, h, m| DateTime::new(day, h, m, 0).unwrap();
        let (a, b, c) = (d(2026, 10, 22), d(2026, 10, 23), d(2026, 10, 24));
        let w = When::timed(at(a, 9, 15), at(a, 9, 30), false).unwrap();
        assert_eq!(w.minutes_on(a), Some((555, 570)));
        assert_eq!(w.minutes_on(b), None);
        // Overnight: to midnight, then from midnight.
        let night = When::timed(at(a, 22, 0), at(c, 2, 0), false).unwrap();
        assert_eq!(night.minutes_on(a), Some((1320, 1440)));
        assert_eq!(night.minutes_on(b), Some((0, 1440)));
        assert_eq!(night.minutes_on(c), Some((0, 120)));
        // Ending at midnight does not spill; a zero-length reminder still gets a minute.
        assert_eq!(When::timed(at(a, 23, 0), at(b, 0, 0), false).unwrap().minutes_on(b), None);
        assert_eq!(When::timed(at(a, 23, 59), at(a, 23, 59), false).unwrap().minutes_on(a), Some((1439, 1440)));
        assert_eq!(When::all_day(a, b).unwrap().minutes_on(a), None);
    }

    #[test]
    fn overlapping_events_share_columns() {
        assert_eq!(columns(&[]), []);
        // 9–10 and 9:30–11 overlap; 11–12 starts a new group, alone.
        assert_eq!(columns(&[(540, 600), (570, 660), (660, 720)]), [(0, 2), (1, 2), (0, 1)]);
        // A long event with two short ones beside it that do not touch each other: two columns,
        // and the second short one reuses the freed column.
        assert_eq!(columns(&[(540, 720), (540, 600), (600, 660)]), [(0, 2), (1, 2), (1, 2)]);
        // Given out of order, the longer of two at the same start takes the first column.
        assert_eq!(columns(&[(600, 630), (540, 600), (540, 700)]), [(1, 2), (1, 2), (0, 2)]);
        // Three at once.
        assert_eq!(columns(&[(0, 60), (0, 60), (30, 90)]), [(0, 3), (1, 3), (2, 3)]);
        // Zero-length and reversed spans do not break it.
        assert_eq!(columns(&[(100, 100), (100, 50)]), [(0, 2), (1, 2)]);
    }
}
