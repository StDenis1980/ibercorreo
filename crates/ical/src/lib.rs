//! iCalendar (RFC 5545), L1: `.ics` files, CalDAV bodies and meeting invitations.
//!
//! Written from the RFC. Reads `VEVENT`s inside a `VCALENDAR`: `UID`, `SUMMARY`, `LOCATION`,
//! `DESCRIPTION`, `DTSTART`, `DTEND` / `DURATION` (days, hours, minutes), `TZID` and the raw `RRULE`.
//! Anything else is skipped and counted in [`Parsed::skipped`], never silently dropped.
//! Writes the same subset back with escaping and 75-octet folding.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use ibercorreo_core::calendar::When;
use ibercorreo_core::{Date, DateTime, Error, Event, EventId, MAX_FIELD_LEN, Result, clamp_text};

pub const MAX_ICS_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_EVENTS: usize = 100_000;
const MAX_DEPTH: usize = 32;

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
        Ok(IcsEvent {
            uid: self.uid.unwrap_or_else(|| format!("ibercorreo-import-{seq}")),
            summary: self.summary,
            location: self.location,
            description: self.description,
            when,
            tzid,
            rrule: self.rrule,
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
    let mut out = String::new();
    for l in ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//IberOffice//Iber Correo//EN", "CALSCALE:GREGORIAN"] {
        push_folded(&mut out, l);
    }
    for e in events {
        push_folded(&mut out, "BEGIN:VEVENT");
        push_folded(&mut out, &format!("UID:{}", e.uid.replace(['\r', '\n'], "")));
        let tz = e.tzid.as_deref().map(|t| format!(";TZID={}", t.replace(['"', ':', ';', '\r', '\n'], ""))).unwrap_or_default();
        match e.when {
            When::AllDay { start, end } => {
                push_folded(&mut out, &format!("DTSTART;VALUE=DATE:{}", basic_date(start)));
                push_folded(&mut out, &format!("DTEND;VALUE=DATE:{}", basic_date(end)));
            }
            When::Timed { start, end, utc } => {
                let tz = if utc { "" } else { tz.as_str() };
                push_folded(&mut out, &format!("DTSTART{tz}:{}", basic_time(start, utc)));
                push_folded(&mut out, &format!("DTEND{tz}:{}", basic_time(end, utc)));
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
        push_folded(&mut out, "END:VEVENT");
    }
    push_folded(&mut out, "END:VCALENDAR");
    out
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
}
