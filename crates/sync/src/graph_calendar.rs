//! Calendar sync over Microsoft Graph, for Outlook.com and Microsoft 365 accounts: the same
//! [`calendar::State`] in and [`calendar::Batch`] out as CalDAV, so the store side
//! ([`calendar::apply`]) is shared.
//!
//! - Calendars are collections at `/v1.0/users/{user}/calendars/{id}/` and events resources at
//!   `…/events/{id}`, both real Graph paths; the user's id keeps two accounts' calendars apart
//!   ([`calendar::Batch::scope`]).
//! - Graph has no change marker for a calendar, so every sync lists the events with their
//!   `@odata.etag` (Exchange's change key) and converts only the new and changed ones. A
//!   converted event is stored as iCalendar text, as a CalDAV one is.
//! - A repeating series comes as its master with an Outlook pattern ([`rrule`]). Its changed and
//!   deleted occurrences are read from `instances` over a window ([`WINDOW_BACK_DAYS`] back,
//!   [`WINDOW_AHEAD_DAYS`] ahead) and become `RECURRENCE-ID` overrides and `EXDATE`s.
//! - Changes made here go back as `POST` (new), `PATCH` (changed, `If-Match` the change key) and
//!   `DELETE`; a deleted or changed occurrence of a series is deleted or patched as that instance.
//!   When the same appointment changed on both sides the server's copy wins, as with CalDAV.

use std::collections::{BTreeMap, BTreeSet};

use ibercorreo_core::calendar::{When, clean_reminders};
use ibercorreo_core::date::Weekday;
use ibercorreo_core::recur::{Freq, Rule};
use ibercorreo_core::{Date, DateTime, Event, EventId, Zone};
use ibercorreo_dav::caldav::{FetchedItem, Http, RemoteCalendar};
use ibercorreo_dav::graph::Graph;
use ibercorreo_dav::{Error, Result};
use serde_json::{Value, json};

use crate::calendar::{Batch, CalendarSync, Change, GRAPH_PREFIX, Pushed, State};

/// Most calendars read from one account.
pub const MAX_CALENDARS: usize = 200;
/// Most events read from one calendar.
pub const MAX_EVENTS: usize = 50_000;
/// Most occurrences of one series read.
const MAX_INSTANCES: usize = 5_000;
/// How far back and ahead a series' changed and deleted occurrences are read.
pub const WINDOW_BACK_DAYS: i64 = 365;
pub const WINDOW_AHEAD_DAYS: i64 = 2 * 365;

/// Times in UTC, bodies as plain text.
const PREFER: (&str, &str) = ("Prefer", "outlook.timezone=\"UTC\", outlook.body-content-type=\"text\"");
const EVENT_FIELDS: &str = "id,iCalUId,subject,body,location,start,end,isAllDay,isReminderOn,reminderMinutesBeforeStart,\
recurrence,type,originalStart,originalStartTimeZone";

/// A Graph id as a path segment: anything but letters, digits and `-_=.` percent-encoded.
fn segment(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || b"-_=.".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// A Graph `dateTime` (`2026-11-02T09:00:00.0000000`), without its fraction.
fn date_time(s: &str) -> Option<DateTime> {
    let s = s.trim().trim_end_matches('Z');
    let s = s.split('.').next().unwrap_or(s);
    DateTime::parse(s).ok().map(|(t, _)| t)
}

/// A `dateTimeTimeZone` as UTC: its wall time in its zone (UTC when it names none we know).
fn utc_of(v: &Value) -> Option<DateTime> {
    let t = date_time(text(v, "dateTime")?)?;
    Some(match text(v, "timeZone").and_then(Zone::named) {
        Some(z) => z.to_utc(t),
        None => t,
    })
}

fn weekday(name: &str) -> Option<Weekday> {
    use Weekday::*;
    Some(match name.to_ascii_lowercase().as_str() {
        "monday" => Monday,
        "tuesday" => Tuesday,
        "wednesday" => Wednesday,
        "thursday" => Thursday,
        "friday" => Friday,
        "saturday" => Saturday,
        "sunday" => Sunday,
        _ => return None,
    })
}

fn weekday_name(w: Weekday) -> &'static str {
    use Weekday::*;
    match w {
        Monday => "monday",
        Tuesday => "tuesday",
        Wednesday => "wednesday",
        Thursday => "thursday",
        Friday => "friday",
        Saturday => "saturday",
        Sunday => "sunday",
    }
}

const INDEX: [(&str, i16); 5] = [("first", 1), ("second", 2), ("third", 3), ("fourth", 4), ("last", -1)];

/// The `RRULE` for an Outlook `patternedRecurrence`, or `None` for a pattern it cannot say.
pub fn rrule(recurrence: &Value) -> Option<String> {
    let pattern = recurrence.get("pattern")?;
    let range = recurrence.get("range");
    let kind = text(pattern, "type")?;
    let interval = pattern.get("interval").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0).unwrap_or(1);
    let days: Vec<_> = pattern
        .get("daysOfWeek")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|d| d.as_str().and_then(weekday)).collect())
        .unwrap_or_default();
    let month_day = pattern.get("dayOfMonth").and_then(Value::as_i64).and_then(|d| i8::try_from(d).ok()).filter(|d| (1..=31).contains(d));
    let month = pattern.get("month").and_then(Value::as_u64).and_then(|m| u8::try_from(m).ok()).filter(|m| (1..=12).contains(m));
    let index = text(pattern, "index").and_then(|i| INDEX.iter().find(|(n, _)| i.eq_ignore_ascii_case(n))).map_or(1, |(_, n)| *n);
    let mut rule = Rule {
        freq: Freq::Daily,
        interval,
        count: None,
        until: None,
        by_day: Vec::new(),
        by_month_day: Vec::new(),
        by_month: Vec::new(),
        by_set_pos: Vec::new(),
        week_start: text(pattern, "firstDayOfWeek").and_then(weekday).unwrap_or(Weekday::Sunday),
    };
    match kind {
        "daily" => {}
        "weekly" => {
            if days.is_empty() {
                return None;
            }
            rule.freq = Freq::Weekly;
            rule.by_day = days.into_iter().map(|d| (None, d)).collect();
        }
        "absoluteMonthly" => {
            rule.freq = Freq::Monthly;
            rule.by_month_day = vec![month_day?];
        }
        "relativeMonthly" | "relativeYearly" => {
            if days.is_empty() {
                return None;
            }
            rule.freq = if kind == "relativeMonthly" { Freq::Monthly } else { Freq::Yearly };
            rule.by_day = days.into_iter().map(|d| (None, d)).collect();
            rule.by_set_pos = vec![index];
            if kind == "relativeYearly" {
                rule.by_month = vec![month?];
            }
        }
        "absoluteYearly" => {
            rule.freq = Freq::Yearly;
            rule.by_month = vec![month?];
            rule.by_month_day = vec![month_day?];
        }
        _ => return None,
    }
    match range.and_then(|r| text(r, "type")) {
        Some("endDate") => rule.until = Some((Date::parse(text(range?, "endDate")?).ok()?, None)),
        Some("numbered") => {
            rule.count = range?.get("numberOfOccurrences").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0);
        }
        _ => {}
    }
    Some(rule.to_text())
}

/// The Outlook `patternedRecurrence` for `rule`, repeating from `start` in `zone`; an error
/// names a rule Outlook has no pattern for.
pub fn recurrence(rule: &Rule, start: Date, zone: &str) -> std::result::Result<Value, String> {
    let unsupported = || format!("Outlook cannot repeat an appointment as {}", rule.to_text());
    let days = |rule: &Rule| -> Vec<&'static str> { rule.by_day.iter().map(|(_, d)| weekday_name(*d)).collect() };
    // `BYDAY=2MO` is the same as `BYDAY=MO;BYSETPOS=2`.
    let ordinal: Option<i16> = match (rule.by_set_pos.as_slice(), rule.by_day.as_slice()) {
        ([], [(Some(n), _)]) => Some(i16::from(*n)),
        ([n], days) if days.iter().all(|(o, _)| o.is_none()) => Some(*n),
        ([], days) if days.iter().all(|(o, _)| o.is_none()) => None,
        _ => return Err(unsupported()),
    };
    let index = |n: i16| INDEX.iter().find(|(_, i)| *i == n).map(|(name, _)| *name).ok_or_else(unsupported);
    let mut pattern = json!({ "interval": rule.interval.max(1), "firstDayOfWeek": weekday_name(rule.week_start) });
    let set = |p: &mut Value, k: &str, v: Value| {
        if let Some(o) = p.as_object_mut() {
            o.insert(k.to_string(), v);
        }
    };
    let single = |v: &[u8]| if v.len() == 1 { v.first().copied() } else { None };
    let month_day = match rule.by_month_day.as_slice() {
        [] => None,
        [d] if *d > 0 => Some(*d),
        _ => return Err(unsupported()),
    };
    match rule.freq {
        Freq::Daily if rule.by_day.is_empty() && month_day.is_none() && rule.by_month.is_empty() => set(&mut pattern, "type", json!("daily")),
        Freq::Weekly if ordinal.is_none() && month_day.is_none() => {
            let d = if rule.by_day.is_empty() { vec![weekday_name(start.weekday())] } else { days(rule) };
            set(&mut pattern, "type", json!("weekly"));
            set(&mut pattern, "daysOfWeek", json!(d));
        }
        Freq::Monthly if rule.by_day.is_empty() => {
            set(&mut pattern, "type", json!("absoluteMonthly"));
            set(&mut pattern, "dayOfMonth", json!(month_day.unwrap_or(i8::try_from(start.day()).unwrap_or(1))));
        }
        Freq::Monthly if month_day.is_none() => {
            set(&mut pattern, "type", json!("relativeMonthly"));
            set(&mut pattern, "daysOfWeek", json!(days(rule)));
            set(&mut pattern, "index", json!(index(ordinal.unwrap_or(1))?));
        }
        Freq::Yearly if rule.by_day.is_empty() => {
            set(&mut pattern, "type", json!("absoluteYearly"));
            set(&mut pattern, "month", json!(single(&rule.by_month).unwrap_or(start.month())));
            set(&mut pattern, "dayOfMonth", json!(month_day.unwrap_or(i8::try_from(start.day()).unwrap_or(1))));
        }
        Freq::Yearly if month_day.is_none() => {
            set(&mut pattern, "type", json!("relativeYearly"));
            set(&mut pattern, "month", json!(single(&rule.by_month).unwrap_or(start.month())));
            set(&mut pattern, "daysOfWeek", json!(days(rule)));
            set(&mut pattern, "index", json!(index(ordinal.unwrap_or(1))?));
        }
        _ => return Err(unsupported()),
    }
    if rule.by_month.len() > 1 {
        return Err(unsupported());
    }
    let range = match (rule.count, rule.until) {
        (Some(n), _) => json!({ "type": "numbered", "startDate": start.to_string(), "numberOfOccurrences": n, "recurrenceTimeZone": zone }),
        (None, Some((d, _))) => json!({ "type": "endDate", "startDate": start.to_string(), "endDate": d.to_string(), "recurrenceTimeZone": zone }),
        (None, None) => json!({ "type": "noEnd", "startDate": start.to_string(), "recurrenceTimeZone": zone }),
    };
    Ok(json!({ "pattern": pattern, "range": range }))
}

/// A Graph event (asked for in UTC with a text body) as an [`Event`] in calendar `calendar`.
/// `None` for one without a usable start and end.
pub fn event(v: &Value, calendar: u32) -> Option<Event> {
    let start = v.get("start").and_then(utc_of)?;
    let end = v.get("end").and_then(utc_of)?;
    let all_day = v.get("isAllDay").and_then(Value::as_bool).unwrap_or(false);
    let zone_name = text(v, "originalStartTimeZone").filter(|z| Zone::named(z).is_some_and(|z| !z.is_utc()));
    let (when, tzid) = if all_day {
        let end = if end.date > start.date { end.date } else { start.date.add_days(1).ok()? };
        (When::all_day(start.date, end).ok()?, None)
    } else {
        match zone_name.and_then(|n| Zone::named(n).map(|z| (n, z))) {
            Some((name, z)) => (When::timed(z.from_utc(start), z.from_utc(end), false).ok()?, Some(name.to_string())),
            None => (When::timed(start, end, true).ok()?, None),
        }
    };
    let uid = text(v, "iCalUId").or_else(|| text(v, "id"))?.to_string();
    let mut e = Event {
        id: EventId(0),
        calendar,
        uid,
        summary: text(v, "subject").unwrap_or("").to_string(),
        location: v.get("location").and_then(|l| text(l, "displayName")).unwrap_or("").to_string(),
        description: v.get("body").and_then(|b| b.get("content")).and_then(Value::as_str).unwrap_or("").trim().to_string(),
        when,
        tzid,
        rrule: None,
        exdates: Vec::new(),
        recurrence_id: None,
        reminders: Vec::new(),
        remote: None,
    };
    if v.get("isReminderOn").and_then(Value::as_bool).unwrap_or(false) {
        let minutes = v.get("reminderMinutesBeforeStart").and_then(Value::as_i64).unwrap_or(15);
        e.reminders = clean_reminders(vec![minutes]);
    }
    if text(v, "type") == Some("seriesMaster") {
        e.rrule = v.get("recurrence").and_then(rrule);
    }
    Some(e)
}

/// The day an occurrence of `master` was due (its `originalStart`, UTC), in the master's times.
fn original_day(instance: &Value, master: &Event) -> Option<Date> {
    let t = date_time(text(instance, "originalStart")?)?;
    Some(match (master.when, master.zone()) {
        (When::Timed { utc: false, .. }, Some(z)) => z.from_utc(t).date,
        // An all-day series' occurrences fall at midnight in the mailbox's zone; the date of the
        // start in UTC is a day early east of Greenwich, so take the instance's own start.
        (When::AllDay { .. }, _) => instance.get("start").and_then(|s| text(s, "dateTime")).and_then(date_time).map_or(t.date, |s| s.date),
        _ => t.date,
    })
}

/// `master` with what the series' `instances` say: the occurrences missing from them become
/// `EXDATE`s, and the changed ones (`exception`) overrides of their day.
pub fn with_instances(mut master: Event, instances: &[Value], from: Date, to: Date) -> Vec<Event> {
    let mut present = BTreeSet::new();
    let mut out = Vec::new();
    for i in instances {
        let Some(day) = original_day(i, &master) else { continue };
        present.insert(day);
        if text(i, "type") == Some("exception")
            && let Some(mut e) = event(i, master.calendar)
        {
            e.uid = master.uid.clone();
            e.rrule = None;
            e.recurrence_id = Some(day);
            if master.tzid.is_some() && e.tzid.is_none() && matches!(e.when, When::Timed { utc: true, .. }) {
                // Keep the series' zone, so the override reads like the rest.
                if let Some(z) = master.zone() {
                    let When::Timed { start, end, .. } = e.when else { continue };
                    if let Ok(w) = When::timed(z.from_utc(start), z.from_utc(end), false) {
                        e.when = w;
                        e.tzid = master.tzid.clone();
                    }
                }
            }
            out.push(e);
        }
    }
    let due: Vec<Date> = master.occurrences(from, to).iter().map(When::start_date).filter(|d| *d >= from && *d <= to).collect();
    master.exdates = due.into_iter().filter(|d| !present.contains(d)).collect();
    out.insert(0, master);
    out
}

/// A `dateTimeTimeZone` for wall time `t` in the zone named `zone`.
fn graph_time(t: DateTime, zone: &str) -> Value {
    json!({ "dateTime": format!("{}T{:02}:{:02}:{:02}", t.date, t.hour(), t.minute(), t.second()), "timeZone": zone })
}

/// The zone name Graph is sent for `e`'s times, and the times in it.
fn times(e: &Event) -> (String, DateTime, DateTime) {
    let system = || {
        let z = Zone::system();
        z.windows_name().map_or_else(|| z.name().to_string(), str::to_string)
    };
    match e.when {
        When::AllDay { start, end } => (system(), DateTime::midnight(start), DateTime::midnight(end)),
        When::Timed { start, end, utc: true } => ("UTC".to_string(), start, end),
        When::Timed { start, end, utc: false } => match e.zone() {
            Some(z) => (z.windows_name().map_or_else(|| z.name().to_string(), str::to_string), start, end),
            // Floating times: the computer's zone, as they were shown.
            None => (system(), start, end),
        },
    }
}

/// `e` as a Graph event body. `series` adds the repeat pattern (`null` when there is none), for
/// a series master.
pub fn body(e: &Event, series: bool) -> std::result::Result<Value, String> {
    let (zone, start, end) = times(e);
    let mut v = json!({
        "subject": e.summary,
        "body": { "contentType": "text", "content": e.description },
        "location": { "displayName": e.location },
        "isAllDay": matches!(e.when, When::AllDay { .. }),
        "start": graph_time(start, &zone),
        "end": graph_time(end, &zone),
        "isReminderOn": !e.reminders.is_empty(),
    });
    let obj = v.as_object_mut().ok_or("not an object")?;
    if let Some(m) = e.reminders.iter().copied().find(|m| *m >= 0) {
        obj.insert("reminderMinutesBeforeStart".into(), json!(m));
    }
    if series {
        let r = match e.rrule.as_deref() {
            Some(text) => {
                let rule = Rule::parse(text).map_err(|err| err.to_string())?;
                recurrence(&rule, e.when.start_date(), &zone)?
            }
            None => Value::Null,
        };
        obj.insert("recurrence".into(), r);
    }
    Ok(v)
}

/// The id at the end of an event href (`…/events/{id}`), still percent-encoded.
fn event_id(href: &str) -> Option<&str> {
    href.rsplit_once("/events/").map(|(_, id)| id).filter(|id| !id.is_empty() && !id.contains('/'))
}

/// The occurrence window for a series starting on `start`, around `today`.
fn window(start: Date, today: Date) -> Option<(Date, Date)> {
    let from = today.add_days(-WINDOW_BACK_DAYS).ok()?.max(start);
    let to = today.add_days(WINDOW_AHEAD_DAYS).ok()?;
    (from <= to).then_some((from, to))
}

/// The occurrences of series `id` (percent-encoded) over `from..=to`.
fn instances<H: Http>(g: &mut Graph<H>, user: &str, id: &str, from: Date, to: Date) -> Result<Vec<Value>> {
    let (Ok(a), Ok(b)) = (from.add_days(-1), to.add_days(2)) else { return Ok(Vec::new()) };
    let path = format!("{user}events/{id}/instances?startDateTime={a}T00:00:00Z&endDateTime={b}T00:00:00Z&$select={EVENT_FIELDS}&$top=500");
    Ok(g.list(&path, &[PREFER], MAX_INSTANCES)?.items)
}

/// The id of `series`' occurrence due on `day` (in the series' times), when it still has one.
fn instance_on(found: &[Value], master: &Event, day: Date) -> Option<String> {
    found.iter().find(|i| original_day(i, master) == Some(day)).and_then(|i| text(i, "id")).map(segment)
}

/// What a write's failure means: the server's copy changed (it wins), the item is gone, or a
/// real error.
enum Failed {
    Conflict,
    Gone,
    Other(Error),
}

fn failed(e: Error) -> Failed {
    match e {
        Error::Status { status: 409 | 412, .. } => Failed::Conflict,
        Error::Status { status: 404 | 410, .. } => Failed::Gone,
        other => Failed::Other(other),
    }
}

/// Write a changed series' deleted and changed occurrences as its instances.
fn push_occurrences<H: Http>(g: &mut Graph<H>, user: &str, id: &str, events: &[Event], today: Date, errors: &mut Vec<String>) -> Result<()> {
    let Some(master) = events.iter().find(|e| e.recurrence_id.is_none() && e.rrule.is_some()) else { return Ok(()) };
    let overrides: Vec<&Event> = events.iter().filter(|e| e.recurrence_id.is_some()).collect();
    if master.exdates.is_empty() && overrides.is_empty() {
        return Ok(());
    }
    let Some((from, to)) = window(master.when.start_date(), today) else { return Ok(()) };
    let found = instances(g, user, id, from, to)?;
    for day in &master.exdates {
        if let Some(occ) = instance_on(&found, master, *day) {
            match g.call("DELETE", &format!("{user}events/{occ}"), &[], None) {
                Ok(_) => {}
                Err(e) => match failed(e) {
                    Failed::Gone | Failed::Conflict => {}
                    Failed::Other(e @ (Error::Auth(_) | Error::Io(_))) => return Err(e),
                    Failed::Other(e) => errors.push(format!("{}, {day}: {e}", master.summary)),
                },
            }
        }
    }
    for o in overrides {
        let Some(day) = o.recurrence_id else { continue };
        let Some(occ) = instance_on(&found, master, day) else { continue };
        let patch = match body(o, false) {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("{}, {day}: {e}", o.summary));
                continue;
            }
        };
        if let Err(e) = g.call("PATCH", &format!("{user}events/{occ}"), &[], Some(&patch)) {
            match failed(e) {
                Failed::Gone | Failed::Conflict => {}
                Failed::Other(e @ (Error::Auth(_) | Error::Io(_))) => return Err(e),
                Failed::Other(e) => errors.push(format!("{}, {day}: {e}", o.summary)),
            }
        }
    }
    Ok(())
}

/// Send `state`'s changes for this account (`user`: its calendars' href prefix).
fn push<H: Http>(g: &mut Graph<H>, user: &str, state: &State, today: Date, batch: &mut Batch) -> Result<()> {
    for change in &state.changes {
        let (cal, href) = match change {
            Change::Put { calendar, href, .. } | Change::Delete { calendar, href, .. } => (calendar, href),
        };
        // A new event's href is made up from its UID, not a Graph id: it is created in its calendar.
        let inside = href.starts_with(cal.as_str()) && cal.starts_with(user);
        if !inside {
            batch.errors.push(format!("{href}: not inside its calendar; not sent"));
            continue;
        }
        let result: std::result::Result<Pushed, Failed> = match change {
            Change::Put { body: ics, etag, uid, .. } => {
                let events = match ibercorreo_ical::parse(ics) {
                    Ok(p) => p.events.into_iter().map(|e| e.into_event(EventId(0), 0)).collect::<Vec<_>>(),
                    Err(e) => {
                        batch.errors.push(format!("{href}: {e}"));
                        continue;
                    }
                };
                let Some(master) = events.iter().find(|e| e.recurrence_id.is_none()).or(events.first()) else { continue };
                let json = match body(master, true) {
                    Ok(j) => j,
                    Err(e) => {
                        batch.errors.push(format!("{}: {e}; not sent", master.summary));
                        continue;
                    }
                };
                match etag {
                    None => match g.call("POST", &format!("{cal}events"), &[], Some(&json)) {
                        Ok(Some(created)) => match text(&created, "id") {
                            Some(id) => {
                                let new_href = format!("{cal}events/{}", segment(id));
                                if master.rrule.is_some() {
                                    push_occurrences(g, user, &segment(id), &events, today, &mut batch.errors)?;
                                }
                                Ok(Pushed::Put { calendar: cal.clone(), href: new_href, uid: uid.clone() })
                            }
                            None => Err(Failed::Other(Error::Protocol("the new appointment came back without an id".into()))),
                        },
                        Ok(None) => Err(Failed::Other(Error::Protocol("the new appointment came back empty".into()))),
                        Err(e) => Err(failed(e)),
                    },
                    Some(etag) => match g.call("PATCH", href, &[("If-Match", etag)], Some(&json)) {
                        Ok(_) => {
                            if let Some(id) = event_id(href) {
                                push_occurrences(g, user, id, &events, today, &mut batch.errors)?;
                            }
                            Ok(Pushed::Put { calendar: cal.clone(), href: href.clone(), uid: uid.clone() })
                        }
                        Err(e) => Err(failed(e)),
                    },
                }
            }
            Change::Delete { etag, .. } => match g.call("DELETE", href, &[("If-Match", etag)], None) {
                Ok(_) => Ok(Pushed::Deleted { href: href.clone() }),
                Err(e) => match failed(e) {
                    Failed::Gone => Ok(Pushed::Deleted { href: href.clone() }),
                    other => Err(other),
                },
            },
        };
        match result {
            Ok(p) => batch.pushed.push(p),
            Err(Failed::Conflict) | Err(Failed::Gone) => batch.pushed.push(Pushed::Conflict { href: href.clone() }),
            // Signed out or offline: stop, and send everything next time.
            Err(Failed::Other(e @ (Error::Auth(_) | Error::Io(_)))) => return Err(e),
            Err(Failed::Other(e)) => batch.errors.push(format!("{href}: {e}")),
        }
    }
    Ok(())
}

/// The calendar colour Graph keeps: `hexColor`, else one for its named colour.
fn color(v: &Value) -> Option<String> {
    if let Some(hex) = text(v, "hexColor").filter(|h| h.len() == 7 && h.starts_with('#')) {
        return Some(hex.to_string());
    }
    // Tinta Correo's own shades for Outlook's colour names.
    let named = [
        ("lightBlue", "#4A90C2"),
        ("lightGreen", "#5BA55B"),
        ("lightOrange", "#E0883B"),
        ("lightGray", "#8C8C8C"),
        ("lightYellow", "#D6B43A"),
        ("lightTeal", "#3AA3A3"),
        ("lightPink", "#D46A9A"),
        ("lightBrown", "#A0704A"),
        ("lightRed", "#D05050"),
    ];
    let c = text(v, "color")?;
    named.iter().find(|(n, _)| *n == c).map(|(_, hex)| (*hex).to_string())
}

/// Sync this account's Outlook calendars: send `state`'s changes (those of this account's
/// calendars), then read every calendar and convert the events that are new or changed since
/// `state`. `now` is UTC. Failing to reach or sign in to Graph is an error; a calendar that
/// fails on its own is reported in [`Batch::errors`] and left as it is.
pub fn run<H: Http>(g: &mut Graph<H>, state: &State, now: DateTime) -> Result<Batch> {
    let me = g.get("me?$select=id")?;
    let id = text(&me, "id").ok_or_else(|| Error::Protocol("Microsoft Graph did not say who is signed in".into()))?;
    let user = format!("{GRAPH_PREFIX}{}/", segment(id));
    let state = state.only(&|h| h.starts_with(&user));
    let mut batch = Batch { scope: Some(user.clone()), ..Batch::default() };
    push(g, &user, &state, now.date, &mut batch)?;
    let pushed: BTreeSet<String> = batch
        .pushed
        .iter()
        .map(|p| match p {
            Pushed::Put { href, .. } | Pushed::Deleted { href } | Pushed::Conflict { href } => href.clone(),
        })
        .collect();
    let calendars = g.list(&format!("{user}calendars?$select=id,name,hexColor,color&$top=100"), &[], MAX_CALENDARS)?;
    for c in &calendars.items {
        let (Some(cid), name) = (text(c, "id"), text(c, "name").unwrap_or("Calendar")) else { continue };
        let href = format!("{user}calendars/{}/", segment(cid));
        let calendar = RemoteCalendar { href: href.clone(), name: name.to_string(), color: color(c), ctag: None };
        let empty = BTreeMap::new();
        let known = state.calendars.get(&href).map_or(&empty, |k| &k.items);
        let listed = match g.list(&format!("{href}events?$select={EVENT_FIELDS}&$top=500"), &[PREFER], MAX_EVENTS) {
            Ok(l) => l.items,
            Err(e @ (Error::Auth(_) | Error::Io(_))) => return Err(e),
            Err(e) => {
                batch.errors.push(format!("{name}: {e}"));
                batch.calendars.push(CalendarSync { calendar, unchanged: true, fetched: Vec::new(), removed: Vec::new() });
                continue;
            }
        };
        let mut seen = BTreeSet::new();
        let mut fetched = Vec::new();
        for v in &listed {
            let (Some(eid), Some(etag)) = (text(v, "id"), text(v, "@odata.etag")) else { continue };
            let ev_href = format!("{href}events/{}", segment(eid));
            seen.insert(ev_href.clone());
            // Unchanged, unless this sync just wrote it (its new version must come down).
            if known.get(&ev_href).is_some_and(|k| k == etag) && !pushed.contains(&ev_href) {
                continue;
            }
            let Some(master) = event(v, 0) else {
                batch.errors.push(format!("{name}: {}: no usable start and end", text(v, "subject").unwrap_or("")));
                continue;
            };
            let events = match (master.rrule.is_some(), window(master.when.start_date(), now.date)) {
                (true, Some((from, to))) => match instances(g, &user, &segment(eid), from, to) {
                    Ok(found) => with_instances(master, &found, from, to),
                    Err(e @ (Error::Auth(_) | Error::Io(_))) => return Err(e),
                    Err(e) => {
                        batch.errors.push(format!("{}: {e}", master.summary));
                        vec![master]
                    }
                },
                _ => vec![master],
            };
            let data = ibercorreo_ical::write_resource(events.iter(), "", now);
            fetched.push(FetchedItem { href: ev_href, etag: etag.to_string(), data });
        }
        let removed: Vec<String> = known.keys().filter(|h| !seen.contains(*h)).cloned().collect();
        batch.calendars.push(CalendarSync { calendar, unchanged: false, fetched, removed });
    }
    Ok(batch)
}

#[cfg(test)]
mod tests;
