//! iCalendar (RFC 5545), L1: `.ics` files, CalDAV bodies and meeting invitations.
//!
//! Written from the RFCs (5545, and 5546 for invitations: [`invite`] reads one, [`reply`] answers
//! it). Reads `VEVENT`s inside a `VCALENDAR`: `UID`, `SUMMARY`, `LOCATION`,
//! `DESCRIPTION`, `DTSTART`, `DTEND` / `DURATION` (days, hours, minutes), `TZID`, the raw `RRULE`,
//! `EXDATE` and `RECURRENCE-ID` (by day).
//! Anything else is skipped and counted in [`Parsed::skipped`], never silently dropped.
//! Writes the same subset back with escaping and 75-octet folding.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use ibercorreo_core::calendar::When;
use ibercorreo_core::{Date, DateTime, Error, Event, EventId, MAX_FIELD_LEN, Result, clamp_text};

pub const MAX_ICS_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_EVENTS: usize = 100_000;
const MAX_DEPTH: usize = 32;
/// Most cancelled occurrences kept per event.
const MAX_EXDATES: usize = 1_000;

/// An event as read from a file, before the store gives it an id and a calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcsEvent {
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    pub when: When,
    pub tzid: Option<String>,
    pub rrule: Option<String>,
    pub exdates: Vec<Date>,
    pub recurrence_id: Option<Date>,
    pub reminders: Vec<i32>,
}

impl IcsEvent {
    pub fn into_event(self, id: EventId, calendar: u32) -> Event {
        Event {
            id,
            calendar,
            uid: self.uid,
            summary: self.summary,
            location: self.location,
            description: self.description,
            when: self.when,
            tzid: self.tzid,
            rrule: self.rrule,
            exdates: self.exdates,
            recurrence_id: self.recurrence_id,
            reminders: self.reminders,
            remote: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub events: Vec<IcsEvent>,
    /// Properties and components that were not understood (kept count of, for the import report).
    pub skipped: usize,
    /// Events dropped because they had no usable start, with the reason.
    pub rejected: Vec<String>,
}

/// One content line: `NAME;PARAM=V;…:VALUE`.
struct Line<'a> {
    name: String,
    params: Vec<(String, &'a str)>,
    value: &'a str,
}

fn unfold(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        match line.strip_prefix([' ', '\t']) {
            Some(rest) if i > 0 => out.push_str(rest),
            _ => {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(line);
            }
        }
    }
    out
}

fn split_line(line: &str) -> Option<Line<'_>> {
    // The value starts after the first ':' outside a quoted parameter value.
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let head = line.get(..colon)?;
    let value = line.get(colon + 1..)?;
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let params = parts.filter_map(|p| p.split_once('=').map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim_matches('"')))).collect();
    Some(Line { name, params, value })
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    clamp_text(&out, MAX_FIELD_LEN).to_string()
}

#[derive(Default)]
struct Draft {
    uid: Option<String>,
    summary: String,
    location: String,
    description: String,
    start: Option<(Stamp, Option<String>)>,
    end: Option<Stamp>,
    duration_minutes: Option<i64>,
    rrule: Option<String>,
    exdates: Vec<Date>,
    recurrence_id: Option<Date>,
    alarms: Vec<Trigger>,
}

/// A `VALARM` `TRIGGER`: minutes from the start or the end, or a moment.
#[derive(Clone, Copy)]
enum Trigger {
    FromStart(i64),
    FromEnd(i64),
    At(DateTime),
}

#[derive(Clone, Copy)]
enum Stamp {
    Day(Date),
    Time(DateTime, bool),
}

fn stamp(line: &Line<'_>) -> Result<Stamp> {
    let is_date = line.params.iter().any(|(k, v)| k == "VALUE" && v.eq_ignore_ascii_case("DATE"));
    if is_date || !line.value.contains('T') {
        Ok(Stamp::Day(Date::parse(line.value)?))
    } else {
        let (dt, utc) = DateTime::parse(line.value)?;
        Ok(Stamp::Time(dt, utc))
    }
}

/// `P1D`, `PT1H30M`, `-PT15M`, `P2W` in minutes.
fn duration_minutes(v: &str) -> Option<i64> {
    let (neg, v) = match v.trim().strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, v.trim().strip_prefix('+').unwrap_or(v.trim())),
    };
    let v = v.strip_prefix('P')?;
    let mut total: i64 = 0;
    let mut num: i64 = 0;
    let mut seen_digit = false;
    for c in v.chars() {
        if let Some(d) = c.to_digit(10) {
            num = num.checked_mul(10)?.checked_add(i64::from(d))?;
            seen_digit = true;
            continue;
        }
        let unit = match c {
            'W' => 7 * 24 * 60,
            'D' => 24 * 60,
            'H' => 60,
            'M' => 1,
            'S' => 0,
            'T' => continue,
            _ => return None,
        };
        if !seen_digit {
            return None;
        }
        total = total.checked_add(num.checked_mul(unit)?)?;
        num = 0;
        seen_digit = false;
    }
    // Ten years is plenty for one occurrence.
    if seen_digit || total > 10 * 366 * 24 * 60 {
        return None;
    }
    Some(if neg { -total } else { total })
}

fn add_minutes(dt: DateTime, minutes: i64) -> Result<DateTime> {
    let total = i64::from(dt.minute_of_day()).checked_add(minutes).ok_or_else(|| Error::invalid("duration", "overflow"))?;
    let date = dt.date.add_days(total.div_euclid(24 * 60))?;
    let m = total.rem_euclid(24 * 60);
    let hour = u8::try_from(m / 60).map_err(|_| Error::invalid("duration", "hour"))?;
    let minute = u8::try_from(m % 60).map_err(|_| Error::invalid("duration", "minute"))?;
    DateTime::new(date, hour, minute, dt.second())
}

/// Each trigger as minutes before the start.
fn reminder_minutes(alarms: &[Trigger], when: When, tzid: Option<&str>) -> Vec<i32> {
    let (start, end, utc) = match when {
        When::Timed { start, end, utc } => (start, end, utc),
        When::AllDay { start, end } => (DateTime::midnight(start), DateTime::midnight(end), false),
    };
    let length = (end.to_unix() - start.to_unix()) / 60;
    // A moment is compared with the start in UTC; floating times are read as UTC.
    let start_utc = match tzid.and_then(ibercorreo_core::Zone::named) {
        Some(z) if !utc => z.to_utc(start),
        _ => start,
    };
    let minutes = alarms
        .iter()
        .map(|t| match *t {
            Trigger::FromStart(d) => d.saturating_neg(),
            Trigger::FromEnd(d) => length.saturating_add(d).saturating_neg(),
            Trigger::At(at) => (start_utc.to_unix() - at.to_unix()) / 60,
        })
        .collect();
    ibercorreo_core::calendar::clean_reminders(minutes)
}

impl Draft {
    fn finish(self, seq: usize) -> Result<IcsEvent> {
        let (start, tzid) = self.start.ok_or_else(|| Error::invalid("event", "no DTSTART"))?;
        let when = match (start, self.end) {
            (Stamp::Day(s), Some(Stamp::Day(e))) => When::all_day(s, e)?,
            (Stamp::Day(s), _) => {
                let days = self.duration_minutes.map(|m| (m / (24 * 60)).max(1)).unwrap_or(1);
                When::all_day(s, s.add_days(days)?)?
            }
            (Stamp::Time(s, utc), Some(Stamp::Time(e, _))) => When::timed(s, e, utc)?,
            (Stamp::Time(s, utc), Some(Stamp::Day(_))) => When::timed(s, s, utc)?,
            (Stamp::Time(s, utc), None) => When::timed(s, add_minutes(s, self.duration_minutes.unwrap_or(0).max(0))?, utc)?,
        };
        let reminders = reminder_minutes(&self.alarms, when, tzid.as_deref());
        Ok(IcsEvent {
            uid: self.uid.unwrap_or_else(|| format!("ibercorreo-import-{seq}")),
            summary: self.summary,
            location: self.location,
            description: self.description,
            when,
            tzid,
            rrule: self.rrule,
            exdates: self.exdates,
            recurrence_id: self.recurrence_id,
            reminders,
        })
    }
}

/// Read every `VEVENT` in `text`.
pub fn parse(text: &str) -> Result<Parsed> {
    if text.len() > MAX_ICS_BYTES {
        return Err(Error::TooLarge { what: "calendar file", size: text.len(), limit: MAX_ICS_BYTES });
    }
    let unfolded = unfold(text.trim_start_matches('\u{feff}'));
    let mut out = Parsed::default();
    let mut stack: Vec<String> = Vec::new();
    let mut draft: Option<Draft> = None;
    let mut saw_calendar = false;
    for raw in unfolded.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        let Some(line) = split_line(raw) else {
            out.skipped = out.skipped.saturating_add(1);
            continue;
        };
        match line.name.as_str() {
            "BEGIN" => {
                if stack.len() >= MAX_DEPTH {
                    return Err(Error::invalid("calendar file", "components nested too deep"));
                }
                let comp = line.value.trim().to_ascii_uppercase();
                if comp == "VCALENDAR" {
                    saw_calendar = true;
                }
                if comp == "VEVENT" && stack.last().map(String::as_str) == Some("VCALENDAR") {
                    draft = Some(Draft::default());
                } else if comp != "VCALENDAR" {
                    out.skipped = out.skipped.saturating_add(1);
                }
                stack.push(comp);
            }
            "END" => {
                let comp = line.value.trim().to_ascii_uppercase();
                if stack.last() != Some(&comp) {
                    return Err(Error::invalid("calendar file", format!("END:{comp} does not close {:?}", stack.last())));
                }
                stack.pop();
                if comp == "VEVENT"
                    && stack.last().map(String::as_str) == Some("VCALENDAR")
                    && let Some(d) = draft.take()
                {
                    if out.events.len() >= MAX_EVENTS {
                        return Err(Error::TooLarge { what: "events", size: out.events.len() + 1, limit: MAX_EVENTS });
                    }
                    let seq = out.events.len() + out.rejected.len();
                    match d.finish(seq) {
                        Ok(e) => out.events.push(e),
                        Err(e) => out.rejected.push(e.to_string()),
                    }
                }
            }
            // A reminder: only its trigger matters (every alarm is shown, whatever its ACTION).
            _ if stack.len() == 3
                && line.name == "TRIGGER"
                && stack.last().map(String::as_str) == Some("VALARM")
                && stack.get(1).map(String::as_str) == Some("VEVENT") =>
            {
                let Some(d) = draft.as_mut() else { continue };
                let absolute = line.params.iter().any(|(k, v)| k == "VALUE" && v.eq_ignore_ascii_case("DATE-TIME"));
                let from_end = line.params.iter().any(|(k, v)| k == "RELATED" && v.eq_ignore_ascii_case("END"));
                let trigger = if absolute {
                    DateTime::parse(line.value).ok().map(|(t, _)| Trigger::At(t))
                } else {
                    duration_minutes(line.value).map(|m| if from_end { Trigger::FromEnd(m) } else { Trigger::FromStart(m) })
                };
                match trigger {
                    Some(t) if d.alarms.len() < 64 => d.alarms.push(t),
                    _ => out.skipped = out.skipped.saturating_add(1),
                }
            }
            _ if stack.len() == 2 && stack.last().map(String::as_str) == Some("VEVENT") => {
                let Some(d) = draft.as_mut() else { continue };
                match line.name.as_str() {
                    "UID" => d.uid = Some(clamp_text(line.value.trim(), 1024).to_string()),
                    "SUMMARY" => d.summary = unescape(line.value),
                    "LOCATION" => d.location = unescape(line.value),
                    "DESCRIPTION" => d.description = unescape(line.value),
                    "DTSTART" => match stamp(&line) {
                        Ok(s) => {
                            let tzid = line.params.iter().find(|(k, _)| k == "TZID").map(|(_, v)| clamp_text(v, 256).to_string());
                            d.start = Some((s, tzid));
                        }
                        Err(e) => out.rejected.push(e.to_string()),
                    },
                    "DTEND" => d.end = stamp(&line).ok(),
                    "DURATION" => d.duration_minutes = duration_minutes(line.value),
                    "RRULE" => d.rrule = Some(clamp_text(line.value.trim(), 4096).to_string()),
                    // A list of dates or date-times; only the day matters (one occurrence a day at most).
                    "EXDATE" => {
                        for v in line.value.split(',').take(MAX_EXDATES.saturating_sub(d.exdates.len())) {
                            match Date::parse(v.trim().get(..8).unwrap_or("")) {
                                Ok(day) => d.exdates.push(day),
                                Err(_) => out.skipped = out.skipped.saturating_add(1),
                            }
                        }
                    }
                    "RECURRENCE-ID" => match Date::parse(line.value.trim().get(..8).unwrap_or("")) {
                        Ok(day) => d.recurrence_id = Some(day),
                        Err(e) => out.rejected.push(e.to_string()),
                    },
                    _ => out.skipped = out.skipped.saturating_add(1),
                }
            }
            _ => out.skipped = out.skipped.saturating_add(1),
        }
    }
    if !saw_calendar {
        return Err(Error::invalid("calendar file", "no BEGIN:VCALENDAR"));
    }
    if !stack.is_empty() {
        return Err(Error::invalid("calendar file", format!("{} not closed", stack.join(", "))));
    }
    Ok(out)
}

fn escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Fold to lines of at most 75 octets, never inside a UTF-8 sequence.
fn push_folded(out: &mut String, line: &str) {
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
}

fn basic_date(d: Date) -> String {
    format!("{:04}{:02}{:02}", d.year(), d.month(), d.day())
}

fn basic_time(t: DateTime, utc: bool) -> String {
    format!("{}T{:02}{:02}{:02}{}", basic_date(t.date), t.hour(), t.minute(), t.second(), if utc { "Z" } else { "" })
}

/// A `VCALENDAR` holding `events`.
pub fn write<'a>(events: impl IntoIterator<Item = &'a Event>) -> String {
    write_with(events, &Kept::default(), None)
}

/// What a server's copy of a resource holds that Iber Correo does not read, kept when the
/// resource is written back: other components and calendar properties (`VTIMEZONE`, `X-…`), and
/// per event (by `UID` and day of `RECURRENCE-ID`) its other properties (`ATTENDEE`,
/// `ORGANIZER`, `CATEGORIES`, `X-…`), its other sub-components, and its own `RECURRENCE-ID`
/// line (exact to the second, where the model keeps the day).
#[derive(Debug, Default)]
struct Kept {
    calendar: Vec<String>,
    events: Vec<KeptEvent>,
}

#[derive(Debug)]
struct KeptEvent {
    uid: String,
    recurrence_id: Option<Date>,
    lines: Vec<String>,
    recurrence_line: Option<String>,
    sequence: u32,
}

/// Properties of a `VEVENT` that the model holds, so the written copy replaces them.
const OWN: &[&str] =
    &["UID", "DTSTART", "DTEND", "DURATION", "SUMMARY", "LOCATION", "DESCRIPTION", "RRULE", "EXDATE", "RECURRENCE-ID", "DTSTAMP", "SEQUENCE"];
/// Most kept lines per resource.
const MAX_KEPT: usize = 10_000;

fn kept_from(raw: &str) -> Kept {
    let mut kept = Kept::default();
    let unfolded = unfold(raw.trim_start_matches('\u{feff}'));
    let mut stack: Vec<String> = Vec::new();
    let mut event: Option<KeptEvent> = None;
    for line in unfolded.lines().filter(|l| !l.trim().is_empty()).take(MAX_KEPT) {
        let Some(l) = split_line(line) else { continue };
        let name = l.name.as_str();
        let comp = l.value.trim().to_ascii_uppercase();
        if name == "BEGIN" {
            if stack.len() >= MAX_DEPTH {
                break;
            }
            stack.push(comp);
            if stack.len() == 2 && stack.get(1).is_some_and(|c| c == "VEVENT") {
                event = Some(KeptEvent { uid: String::new(), recurrence_id: None, lines: Vec::new(), recurrence_line: None, sequence: 0 });
                continue;
            }
        }
        let in_event = stack.get(1).is_some_and(|c| c == "VEVENT");
        // Alarms are written from the model, so none of their lines are kept.
        let in_alarm = stack.iter().skip(2).any(|c| c == "VALARM");
        match (stack.len(), in_event) {
            (0, _) => {}
            (1, _) => {
                // The calendar's own BEGIN and END, and the properties written anew.
                if !matches!(name, "BEGIN" | "END" | "VERSION" | "PRODID" | "CALSCALE" | "METHOD") {
                    kept.calendar.push(line.to_string());
                }
            }
            (2, true) if name == "END" => {
                if let Some(ev) = event.take() {
                    kept.events.push(ev);
                }
            }
            (2, true) => {
                if let Some(ev) = &mut event {
                    match name {
                        "UID" => ev.uid = l.value.trim().to_string(),
                        "RECURRENCE-ID" => {
                            ev.recurrence_id = Date::parse(l.value.trim().get(..8).unwrap_or("")).ok();
                            ev.recurrence_line = Some(line.to_string());
                        }
                        "SEQUENCE" => ev.sequence = l.value.trim().parse().unwrap_or(0),
                        n if OWN.contains(&n) => {}
                        _ => ev.lines.push(line.to_string()),
                    }
                }
            }
            (_, true) => {
                if let (Some(ev), false) = (&mut event, in_alarm) {
                    ev.lines.push(line.to_string());
                }
            }
            (_, false) => kept.calendar.push(line.to_string()),
        }
        if name == "END" {
            stack.pop();
        }
    }
    kept
}

/// `events` (one resource: a series and its changed occurrences) written to send back to the
/// server, keeping what `raw`, the server's copy, holds that Iber Correo does not read. `DTSTAMP`
/// is `now` and `SEQUENCE` goes up by one.
pub fn write_resource<'a>(events: impl IntoIterator<Item = &'a Event>, raw: &str, now: DateTime) -> String {
    write_with(events, &kept_from(raw), Some(now))
}

fn write_with<'a>(events: impl IntoIterator<Item = &'a Event>, kept: &Kept, now: Option<DateTime>) -> String {
    let mut out = String::new();
    for l in ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//IberOffice//Iber Correo//EN", "CALSCALE:GREGORIAN"] {
        push_folded(&mut out, l);
    }
    for l in &kept.calendar {
        push_folded(&mut out, l);
    }
    for e in events {
        let old = kept.events.iter().find(|k| k.uid == e.uid && k.recurrence_id == e.recurrence_id);
        push_folded(&mut out, "BEGIN:VEVENT");
        push_folded(&mut out, &format!("UID:{}", e.uid.replace(['\r', '\n'], "")));
        if let Some(now) = now {
            push_folded(&mut out, &format!("DTSTAMP:{}", basic_time(now, true)));
            push_folded(&mut out, &format!("SEQUENCE:{}", old.map_or(0, |k| k.sequence.saturating_add(1))));
        }
        let tz = e.tzid.as_deref().map(|t| format!(";TZID={}", t.replace(['"', ':', ';', '\r', '\n'], ""))).unwrap_or_default();
        // EXDATE takes DTSTART's form (RFC 5545 §3.8.5.1): dates for an all-day event, the
        // start time on each day for a timed one.
        let mut exdate = String::new();
        match e.when {
            When::AllDay { start, end } => {
                push_folded(&mut out, &format!("DTSTART;VALUE=DATE:{}", basic_date(start)));
                push_folded(&mut out, &format!("DTEND;VALUE=DATE:{}", basic_date(end)));
                if !e.exdates.is_empty() {
                    let days: Vec<String> = e.exdates.iter().map(|d| basic_date(*d)).collect();
                    exdate = format!("EXDATE;VALUE=DATE:{}", days.join(","));
                }
            }
            When::Timed { start, end, utc } => {
                // A one-off event in a known zone is written in UTC: exact anywhere, and no
                // VTIMEZONE needed. A repeating one keeps its zone, so it follows summer time.
                let (start, end, utc) = match e.zone() {
                    Some(z) if !utc && e.rrule.is_none() && e.recurrence_id.is_none() => (z.to_utc(start), z.to_utc(end), true),
                    _ => (start, end, utc),
                };
                let tz = if utc { "" } else { tz.as_str() };
                push_folded(&mut out, &format!("DTSTART{tz}:{}", basic_time(start, utc)));
                push_folded(&mut out, &format!("DTEND{tz}:{}", basic_time(end, utc)));
                if !e.exdates.is_empty() {
                    let times: Vec<String> = e
                        .exdates
                        .iter()
                        .filter_map(|d| DateTime::new(*d, start.hour(), start.minute(), start.second()).ok())
                        .map(|t| basic_time(t, utc))
                        .collect();
                    exdate = format!("EXDATE{tz}:{}", times.join(","));
                }
            }
        }
        for (name, value) in [("SUMMARY", &e.summary), ("LOCATION", &e.location), ("DESCRIPTION", &e.description)] {
            if !value.is_empty() {
                push_folded(&mut out, &format!("{name}:{}", escape(value)));
            }
        }
        if let Some(r) = &e.rrule {
            push_folded(&mut out, &format!("RRULE:{}", r.replace(['\r', '\n'], "")));
        }
        if !exdate.is_empty() {
            push_folded(&mut out, &exdate);
        }
        match (old.and_then(|k| k.recurrence_line.as_deref()), e.recurrence_id) {
            (Some(line), Some(_)) => push_folded(&mut out, line),
            (None, Some(r)) => push_folded(&mut out, &format!("RECURRENCE-ID;VALUE=DATE:{}", basic_date(r))),
            _ => {}
        }
        for m in &e.reminders {
            push_folded(&mut out, "BEGIN:VALARM");
            push_folded(&mut out, "ACTION:DISPLAY");
            push_folded(&mut out, &format!("DESCRIPTION:{}", escape(&e.summary)));
            let trigger = if *m >= 0 { format!("TRIGGER:-PT{m}M") } else { format!("TRIGGER:PT{}M", m.unsigned_abs()) };
            push_folded(&mut out, &trigger);
            push_folded(&mut out, "END:VALARM");
        }
        for l in old.map_or(&[][..], |k| k.lines.as_slice()) {
            push_folded(&mut out, l);
        }
        push_folded(&mut out, "END:VEVENT");
    }
    push_folded(&mut out, "END:VCALENDAR");
    out
}

/// Someone named in an invitation: its `ORGANIZER` or an `ATTENDEE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Party {
    pub email: String,
    /// The `CN` parameter.
    pub name: Option<String>,
    /// The `PARTSTAT` parameter, upper case (`NEEDS-ACTION`, `ACCEPTED`, `TENTATIVE`, `DECLINED`…).
    pub status: Option<String>,
}

/// A meeting invitation, reply or cancellation (iTIP, RFC 5546) as read from a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// `REQUEST`, `REPLY`, `CANCEL`, `PUBLISH`… upper case; `REQUEST` when the object names none.
    pub method: String,
    /// The meetings: one, or a series and its changed occurrences.
    pub events: Vec<IcsEvent>,
    /// From the first event, as every event of one invitation shares them.
    pub organizer: Option<Party>,
    pub attendees: Vec<Party>,
    /// The first event's `SEQUENCE`: an update to a meeting has a higher one.
    pub sequence: u32,
}

/// Most attendees read from one invitation.
const MAX_ATTENDEES: usize = 1_000;

/// The address in a `CAL-ADDRESS` value: `mailto:ana@example.org` without its scheme.
fn cal_address(v: &str) -> Option<String> {
    let v = v.trim();
    let email = match v.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("mailto:") => v.get(7..)?,
        _ => v,
    };
    let email = email.trim();
    (email.contains('@') && email.len() <= 254 && !email.chars().any(|c| c.is_whitespace() || c.is_control())).then(|| email.to_string())
}

fn party(line: &Line<'_>) -> Option<Party> {
    let param = |name: &str| line.params.iter().find(|(k, _)| k == name).map(|(_, v)| *v);
    Some(Party {
        email: cal_address(line.value)?,
        name: param("CN").map(|n| clamp_text(n.trim(), 256).to_string()).filter(|n| !n.is_empty()),
        status: param("PARTSTAT").map(|p| clamp_text(p.trim(), 32).to_ascii_uppercase()),
    })
}

/// Read an invitation: its events (through [`parse`]), its `METHOD`, and who organises and attends.
pub fn invite(text: &str) -> Result<Invite> {
    let parsed = parse(text)?;
    let mut out = Invite { method: "REQUEST".into(), events: parsed.events, organizer: None, attendees: Vec::new(), sequence: 0 };
    let unfolded = unfold(text.trim_start_matches('\u{feff}'));
    let mut stack: Vec<String> = Vec::new();
    let mut events_seen = 0usize;
    for raw in unfolded.lines() {
        let Some(line) = split_line(raw) else { continue };
        match line.name.as_str() {
            "BEGIN" => {
                if stack.len() >= MAX_DEPTH {
                    break;
                }
                stack.push(line.value.trim().to_ascii_uppercase());
                if stack.len() == 2 && stack.last().is_some_and(|c| c == "VEVENT") {
                    events_seen = events_seen.saturating_add(1);
                }
            }
            "END" => {
                stack.pop();
            }
            "METHOD" if stack.len() == 1 => {
                let m = line.value.trim().to_ascii_uppercase();
                if !m.is_empty() && m.len() <= 32 && m.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    out.method = m;
                }
            }
            // Only the first event's: the parties are the same for the whole meeting.
            name if stack.len() == 2 && events_seen == 1 && stack.last().is_some_and(|c| c == "VEVENT") => match name {
                "ORGANIZER" => out.organizer = party(&line),
                "ATTENDEE" if out.attendees.len() < MAX_ATTENDEES => out.attendees.extend(party(&line)),
                "SEQUENCE" => out.sequence = line.value.trim().parse().unwrap_or(0),
                _ => {}
            },
            _ => {}
        }
    }
    Ok(out)
}

/// A parameter value made safe to write: no quotes, no line breaks, quoted when it needs to be.
fn param_value(v: &str) -> String {
    let v: String = clamp_text(v, 256).chars().filter(|c| !c.is_control() && *c != '"').collect();
    if v.contains([':', ';', ',']) { format!("\"{v}\"") } else { v }
}

/// The `METHOD:REPLY` that answers invitation `text` for attendee `me` (RFC 5546 §3.2.3): per
/// event its `UID`, `RECURRENCE-ID`, `SEQUENCE`, `ORGANIZER`, times and title as the organizer
/// wrote them, a fresh `DTSTAMP`, and one `ATTENDEE` (`me`) with `status` (`ACCEPTED`,
/// `TENTATIVE` or `DECLINED`) and an optional `COMMENT`. Time zones the times refer to go along.
pub fn reply(text: &str, me: &ibercorreo_core::Address, status: &str, comment: &str, now: DateTime) -> Result<String> {
    if text.len() > MAX_ICS_BYTES {
        return Err(Error::TooLarge { what: "calendar file", size: text.len(), limit: MAX_ICS_BYTES });
    }
    let status = status.to_ascii_uppercase();
    if !matches!(status.as_str(), "ACCEPTED" | "TENTATIVE" | "DECLINED") {
        return Err(Error::invalid("status", "ACCEPTED, TENTATIVE or DECLINED"));
    }
    let mut out = String::new();
    for l in ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//IberOffice//Iber Correo//EN", "CALSCALE:GREGORIAN", "METHOD:REPLY"] {
        push_folded(&mut out, l);
    }
    let unfolded = unfold(text.trim_start_matches('\u{feff}'));
    let mut stack: Vec<String> = Vec::new();
    let mut events = 0usize;
    let mut has_uid = false;
    let mut lines = 0usize;
    let attendee = format!(
        "ATTENDEE;PARTSTAT={status}{}:mailto:{}",
        me.name.as_deref().map(|n| format!(";CN={}", param_value(n))).unwrap_or_default(),
        me.email.replace(|c: char| c.is_control() || c.is_whitespace(), "")
    );
    for raw in unfolded.lines() {
        let Some(line) = split_line(raw) else { continue };
        let name = line.name.as_str();
        let raw = raw.trim_end_matches('\r');
        if name == "BEGIN" {
            if stack.len() >= MAX_DEPTH {
                return Err(Error::invalid("calendar file", "components nested too deep"));
            }
            stack.push(line.value.trim().to_ascii_uppercase());
        }
        let top = stack.get(1).map(String::as_str);
        match (stack.len(), top) {
            // Time zones go along whole.
            (n, Some("VTIMEZONE")) if n >= 2 => {
                lines = lines.saturating_add(1);
                if lines > MAX_KEPT {
                    return Err(Error::TooLarge { what: "invitation", size: lines, limit: MAX_KEPT });
                }
                push_folded(&mut out, raw);
            }
            (2, Some("VEVENT")) => match name {
                "BEGIN" => {
                    if events >= MAX_EVENTS {
                        return Err(Error::TooLarge { what: "events", size: events, limit: MAX_EVENTS });
                    }
                    events = events.saturating_add(1);
                    has_uid = false;
                    push_folded(&mut out, "BEGIN:VEVENT");
                    push_folded(&mut out, &format!("DTSTAMP:{}", basic_time(now, true)));
                }
                "END" => {
                    if !has_uid {
                        return Err(Error::invalid("invitation", "an event has no UID"));
                    }
                    push_folded(&mut out, &attendee);
                    if !comment.trim().is_empty() {
                        push_folded(&mut out, &format!("COMMENT:{}", escape(clamp_text(comment.trim(), MAX_FIELD_LEN))));
                    }
                    push_folded(&mut out, "END:VEVENT");
                }
                "UID" | "RECURRENCE-ID" | "SEQUENCE" | "ORGANIZER" | "DTSTART" | "DTEND" | "DURATION" | "SUMMARY" => {
                    has_uid |= name == "UID";
                    push_folded(&mut out, raw);
                }
                _ => {}
            },
            _ => {}
        }
        if name == "END" {
            stack.pop();
        }
    }
    if events == 0 {
        return Err(Error::invalid("invitation", "no event to answer"));
    }
    push_folded(&mut out, "END:VCALENDAR");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//EN\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Lisbon\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:one@example.org\r\nDTSTART;TZID=Europe/Lisbon:20261026T100000\r\n\
DTEND;TZID=Europe/Lisbon:20261026T113000\r\nSUMMARY:Reunião de equipa\\, sala 2\r\n\
LOCATION:Lisboa\r\nDESCRIPTION:Linha um\\nLinha \r\n dois\r\nRRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:two@example.org\r\nDTSTART;VALUE=DATE:20261031\r\nSUMMARY:Feriado\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nDTSTART:20261027T090000Z\r\nDURATION:PT45M\r\nSUMMARY:Call\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nSUMMARY:No start\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn reads_events_with_folding_escapes_and_durations() {
        let p = parse(SAMPLE).unwrap();
        assert_eq!(p.events.len(), 3);
        assert_eq!(p.rejected.len(), 1);
        assert!(p.skipped > 0);
        let one = p.events.first().unwrap();
        assert_eq!(one.summary, "Reunião de equipa, sala 2");
        assert_eq!(one.description, "Linha um\nLinha dois");
        assert_eq!(one.tzid.as_deref(), Some("Europe/Lisbon"));
        assert_eq!(one.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=MO"));
        let two = p.events.get(1).unwrap();
        assert_eq!(two.when, When::all_day(Date::new(2026, 10, 31).unwrap(), Date::new(2026, 11, 1).unwrap()).unwrap());
        match p.events.get(2).unwrap().when {
            When::Timed { end, utc, .. } => assert_eq!((end.hour(), end.minute(), utc), (9, 45, true)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn written_back_resources_keep_what_is_not_read() {
        let raw = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//EN\r\nMETHOD:REQUEST\r\nX-WR-CALNAME:Work\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Lisbon\r\nBEGIN:STANDARD\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\nDTSTART:19701025T020000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:s\r\nSEQUENCE:4\r\nDTSTAMP:20260101T000000Z\r\nDTSTART;TZID=Europe/Lisbon:20261102T090000\r\nDTEND;TZID=Europe/Lisbon:20261102T093000\r\n\
RRULE:FREQ=DAILY;COUNT=5\r\nSUMMARY:Stand-up\r\nORGANIZER;CN=Ana:mailto:ana@example.org\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:rui@example.org\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT5M\r\nACTION:AUDIO\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:s\r\nRECURRENCE-ID;TZID=Europe/Lisbon:20261103T090000\r\nDTSTART;TZID=Europe/Lisbon:20261103T100000\r\nDTEND;TZID=Europe/Lisbon:20261103T103000\r\nSUMMARY:Late\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
        let mut events: Vec<Event> = parse(raw).unwrap().events.into_iter().enumerate().map(|(i, e)| e.into_event(EventId(i as u64), 0)).collect();
        events[0].summary = "Stand-up (new room)".into();
        events[0].exdates.push(Date::new(2026, 11, 5).unwrap());
        events[0].reminders = vec![10];
        let now = DateTime::new(Date::new(2026, 10, 23).unwrap(), 8, 0, 0).unwrap();
        let text = write_resource(&events, raw, now);
        let lines: Vec<&str> = text.lines().collect();
        let count = |l: &str| lines.iter().filter(|x| **x == l).count();
        assert_eq!((count("BEGIN:VCALENDAR"), count("END:VCALENDAR"), count("BEGIN:VEVENT")), (1, 1, 2), "{text}");
        for kept in [
            "X-WR-CALNAME:Work",
            "BEGIN:VTIMEZONE",
            "TZOFFSETTO:+0000",
            "END:VTIMEZONE",
            "ORGANIZER;CN=Ana:mailto:ana@example.org",
            "ATTENDEE;PARTSTAT=ACCEPTED:mailto:rui@example.org",
            "RECURRENCE-ID;TZID=Europe/Lisbon:20261103T090000",
        ] {
            assert_eq!(count(kept), 1, "{kept} in {text}");
        }
        for gone in ["METHOD:REQUEST", "SEQUENCE:4", "DTSTAMP:20260101T000000Z", "ACTION:AUDIO", "SUMMARY:Stand-up"] {
            assert_eq!(count(gone), 0, "{gone} in {text}");
        }
        assert_eq!((count("SEQUENCE:5"), count("DTSTAMP:20261023T080000Z")), (1, 2), "{text}");
        assert_eq!(count("EXDATE;TZID=Europe/Lisbon:20261105T090000"), 1, "EXDATE takes DTSTART's form: {text}");
        assert_eq!(count("TRIGGER:-PT10M"), 1);
        // And it reads back the same.
        let again: Vec<Event> = parse(&text).unwrap().events.into_iter().enumerate().map(|(i, e)| e.into_event(EventId(i as u64), 0)).collect();
        assert_eq!(again, events);
        // Hostile server copies are no problem.
        for raw in ["", "BEGIN:VCALENDAR", "END:VEVENT\r\nEND:VCALENDAR\r\nEND:X", &"BEGIN:X\r\n".repeat(100)] {
            let _ = write_resource(&events, raw, now);
        }
    }

    #[test]
    fn round_trips_through_write() {
        let p = parse(SAMPLE).unwrap();
        let events: Vec<Event> = p.events.into_iter().enumerate().map(|(i, e)| e.into_event(EventId(i as u64), 0)).collect();
        let text = write(&events);
        assert!(text.lines().all(|l| l.len() <= 76), "{text}");
        let again = parse(&text).unwrap();
        assert_eq!(again.events.len(), 3);
        for (a, b) in events.iter().zip(again.events) {
            assert_eq!(a.summary, b.summary);
            assert_eq!(a.description, b.description);
            assert_eq!(a.when, b.when);
            assert_eq!(a.rrule, b.rrule);
        }
        let long = Event { summary: "ї".repeat(200), ..events.first().unwrap().clone() };
        let text = write([&long]);
        assert!(text.lines().all(|l| l.len() <= 76));
        assert_eq!(parse(&text).unwrap().events.first().unwrap().summary, long.summary);
    }

    #[test]
    fn durations() {
        assert_eq!(duration_minutes("PT1H30M"), Some(90));
        assert_eq!(duration_minutes("P1D"), Some(1440));
        assert_eq!(duration_minutes("P2W"), Some(20_160));
        assert_eq!(duration_minutes("-PT15M"), Some(-15));
        for bad in ["", "1H", "PTH", "P99999999999999999999D", "P5000W", "PT5", "PX"] {
            assert!(duration_minutes(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn hostile_files_never_panic() {
        let deep = "BEGIN:VCALENDAR\n".to_string() + &"BEGIN:X\n".repeat(1000);
        for text in [
            "",
            "garbage",
            "BEGIN:VCALENDAR",
            "END:VCALENDAR",
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nEND:VCALENDAR\n",
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:99999999T999999\nEND:VEVENT\nEND:VCALENDAR",
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART;VALUE=DATE:20261022\nDURATION:P99999D\nEND:VEVENT\nEND:VCALENDAR",
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:20261022T100000\nDTEND:20261021T100000\nEND:VEVENT\nEND:VCALENDAR",
            "BEGIN:VCALENDAR\n:::\n;;;\n\"\nSUMMARY;X=\"a:b\":c\\\nEND:VCALENDAR",
            deep.as_str(),
        ] {
            let _ = parse(text);
        }
    }

    #[test]
    fn reminders_read_every_trigger_form_and_round_trip() {
        let text = "BEGIN:VCALENDAR\r\n\
BEGIN:VEVENT\r\nUID:a\r\nDTSTART;TZID=Europe/Lisbon:20261026T100000\r\nDTEND;TZID=Europe/Lisbon:20261026T110000\r\nSUMMARY:A\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n\
BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER;RELATED=END:-PT30M\r\nEND:VALARM\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;VALUE=DATE-TIME:20261025T110000Z\r\nEND:VALARM\r\n\
BEGIN:VALARM\r\nTRIGGER:-P1D\r\nEND:VALARM\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b\r\nDTSTART:20261026T100000Z\r\nSUMMARY:B\r\n\
BEGIN:VALARM\r\nTRIGGER:PT5M\r\nEND:VALARM\r\nBEGIN:VALARM\r\nTRIGGER:-P99W\r\nEND:VALARM\r\nBEGIN:VALARM\r\nTRIGGER:soon\r\nEND:VALARM\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
        let p = parse(text).unwrap();
        // 15 min before; 30 before the end of a one-hour event is 30 after the start; 11:00 UTC
        // the day before a 10:00 Lisbon start (10:00 UTC: winter time from 25 October) is 23
        // hours before; a day; the repeated 15 is dropped. Earliest first.
        assert_eq!(p.events[0].reminders, [1440, 1380, 15, -30]);
        // After the start; four weeks at most; nonsense skipped.
        assert_eq!(p.events[1].reminders, [ibercorreo_core::calendar::MAX_REMINDER_MINUTES, -5]);
        let events: Vec<Event> = p.events.into_iter().enumerate().map(|(i, e)| e.into_event(EventId(i as u64), 0)).collect();
        let out = write(&events);
        assert!(out.contains("TRIGGER:-PT1380M") && out.contains("TRIGGER:PT5M"), "{out}");
        let again = parse(&out).unwrap();
        assert_eq!(again.events[0].reminders, [1440, 1380, 15, -30]);
        assert_eq!(again.events[1].reminders, [ibercorreo_core::calendar::MAX_REMINDER_MINUTES, -5]);
    }

    const INVITE: &str = "BEGIN:VCALENDAR\r\nPRODID:-//Other//EN\r\nVERSION:2.0\r\nMETHOD:request\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Lisbon\r\nBEGIN:STANDARD\r\nTZOFFSETTO:+0000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:plan@example.org\r\nSEQUENCE:2\r\nDTSTAMP:20261020T120000Z\r\n\
DTSTART;TZID=Europe/Lisbon:20261102T100000\r\nDTEND;TZID=Europe/Lisbon:20261102T110000\r\nSUMMARY:Planeamento\r\n\
ORGANIZER;CN=\"Souza, Ana\":mailto:ana@example.org\r\n\
ATTENDEE;CN=Denys;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:MAILTO:denys@example.com\r\n\
ATTENDEE;PARTSTAT=accepted:mailto:rui@example.org\r\nATTENDEE:not-an-address\r\n\
DESCRIPTION:Agenda\r\nBEGIN:VALARM\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:plan@example.org\r\nRECURRENCE-ID;TZID=Europe/Lisbon:20261109T100000\r\n\
DTSTART;TZID=Europe/Lisbon:20261109T120000\r\nDTEND;TZID=Europe/Lisbon:20261109T130000\r\nSUMMARY:Planeamento (moved)\r\n\
ATTENDEE:mailto:other@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn invitations_name_their_method_and_parties() {
        let inv = invite(INVITE).unwrap();
        assert_eq!(inv.method, "REQUEST");
        assert_eq!(inv.sequence, 2);
        assert_eq!(inv.events.len(), 2);
        let org = inv.organizer.unwrap();
        assert_eq!((org.email.as_str(), org.name.as_deref()), ("ana@example.org", Some("Souza, Ana")));
        let found: Vec<(&str, Option<&str>)> = inv.attendees.iter().map(|a| (a.email.as_str(), a.status.as_deref())).collect();
        assert_eq!(
            found,
            [("denys@example.com", Some("NEEDS-ACTION")), ("rui@example.org", Some("ACCEPTED"))],
            "only the first event's, with real addresses"
        );
        assert_eq!(invite("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap().method, "REQUEST", "no METHOD reads as a request");
        assert!(invite("nonsense").is_err());
    }

    #[test]
    fn replies_answer_for_one_attendee() {
        let me = ibercorreo_core::Address::new(Some("Denys; \"M\""), "denys@example.com").unwrap();
        let now = DateTime::new(Date::new(2026, 10, 23).unwrap(), 9, 0, 0).unwrap();
        let text = reply(INVITE, &me, "accepted", "Até lá\nobrigado", now).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let count = |l: &str| lines.iter().filter(|x| **x == l).count();
        for one in
            ["METHOD:REPLY", "BEGIN:VTIMEZONE", "TZOFFSETTO:+0000", "END:VTIMEZONE", "SEQUENCE:2", "RECURRENCE-ID;TZID=Europe/Lisbon:20261109T100000"]
        {
            assert_eq!(count(one), 1, "{one} in {text}");
        }
        assert_eq!(count("ATTENDEE;PARTSTAT=ACCEPTED;CN=\"Denys; M\":mailto:denys@example.com"), 2, "{text}");
        assert_eq!(count("COMMENT:Até lá\\nobrigado"), 2);
        assert_eq!(count("DTSTAMP:20261023T090000Z"), 2);
        for gone in [
            "METHOD:request",
            "DESCRIPTION:Agenda",
            "TRIGGER:-PT10M",
            "ATTENDEE;PARTSTAT=accepted:mailto:rui@example.org",
            "DTSTAMP:20261020T120000Z",
        ] {
            assert_eq!(count(gone), 0, "{gone} in {text}");
        }
        // The organizer's calendar reads it back as the same meeting.
        let back = invite(&text).unwrap();
        assert_eq!(back.method, "REPLY");
        assert_eq!(back.attendees.len(), 1);
        assert_eq!(back.events.len(), 2);
        assert_eq!(back.events[0].when, invite(INVITE).unwrap().events[0].when);
        assert!(text.lines().all(|l| l.len() <= 76));
        // Nothing to answer, or a made-up answer: an error.
        assert!(reply(INVITE, &me, "maybe", "", now).is_err());
        assert!(reply("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n", &me, "DECLINED", "", now).is_err());
        assert!(
            reply("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n", &me, "DECLINED", "", now).is_err(),
            "no UID"
        );
        let deep = "BEGIN:VCALENDAR\n".to_string() + &"BEGIN:X\n".repeat(1000);
        for hostile in ["", "garbage", deep.as_str(), "BEGIN:VCALENDAR\nBEGIN:VTIMEZONE\nBEGIN:VTIMEZONE\nEND:VEVENT"] {
            let _ = reply(hostile, &me, "DECLINED", "", now);
            let _ = invite(hostile);
        }
    }
}
