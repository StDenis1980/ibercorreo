//! One occurrence of a repeating appointment, or the whole series (Tinta), as Outlook asks when
//! one is dragged or edited: "this occurrence" becomes a changed copy of it (`RECURRENCE-ID`,
//! sharing the series' UID and its server resource), "the series" moves every occurrence by the
//! same amount. A series moved to another day keeps its rule's weekdays in step (`BYDAY=MO`
//! becomes `BYDAY=TU`), and its deleted and changed occurrences move with it; a rule that names
//! days in a way that cannot simply shift (`BYDAY=1MO`, `BYMONTHDAY`, `BYSETPOS`…) moves only
//! within its day.

use ibercorreo_core::calendar::When;
use ibercorreo_core::{Date, DateTime, Error, Event, EventId, Result, Zone};

use crate::Session;

const WEEKDAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

fn minutes(t: DateTime) -> i64 {
    t.to_unix().div_euclid(60)
}

fn shift_time(t: DateTime, by: i64) -> Result<DateTime> {
    DateTime::from_unix(minutes(t).saturating_add(by).saturating_mul(60))
}

/// A time as the calendar (`view`) shows it, back in an event's own terms: UTC, its own zone, or
/// floating.
pub fn to_own(t: DateTime, utc: bool, own: Option<&Zone>, view: &Zone) -> DateTime {
    match (utc, own) {
        (true, _) => view.to_utc(t),
        (false, Some(own)) if own != view => own.from_utc(view.to_utc(t)),
        _ => t,
    }
}

/// The occurrence of series `e` on `day` as the calendar shows days: in its own terms and as shown.
pub fn occurrence(e: &Event, day: Date, view: &Zone) -> Result<(When, When)> {
    if e.rrule.is_none() || e.recurrence_id.is_some() {
        return Err(Error::invalid("occurrence", "the appointment does not repeat; leave occurrence out"));
    }
    e.occurrences_in(day, day, view)
        .into_iter()
        .find(|(_, shown)| shown.start_date() == day)
        .or_else(|| e.occurrences_in(day, day, view).into_iter().next())
        .ok_or_else(|| Error::invalid("occurrence", format!("the appointment has no occurrence on {day}")))
}

/// The changed copy of the occurrence of series `id` on `day` (as the calendar shows days): the
/// one there already, or a new one made from the series, which then hides that day's occurrence.
pub fn detach(s: &mut Session, id: EventId, day: Date) -> Result<EventId> {
    let view = s.zone().clone();
    let series = s.store.event(id)?.clone();
    let (own, _) = occurrence(&series, day, &view)?;
    let original = own.start_date();
    if let Some(existing) =
        s.store.events().iter().find(|x| x.uid == series.uid && x.calendar == series.calendar && x.recurrence_id == Some(original)).map(|x| x.id)
    {
        return Ok(existing);
    }
    let mut copy = series.clone();
    copy.rrule = None;
    copy.exdates.clear();
    copy.recurrence_id = Some(original);
    copy.when = own;
    // It travels in the series' resource, so both are sent again.
    if let Some(r) = &mut copy.remote {
        r.dirty = true;
    }
    s.store.event_mut(id)?;
    s.store.add_event(copy)
}

/// `rule` for a series moved by `days`: its plain weekdays shifted with it. `None` when it names
/// days in a way that cannot simply shift.
fn shifted_rule(rule: &str, days: i64) -> Option<String> {
    if days == 0 {
        return Some(rule.to_string());
    }
    let mut parts = Vec::new();
    for part in rule.split(';') {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        match key.to_ascii_uppercase().as_str() {
            "BYMONTHDAY" | "BYYEARDAY" | "BYWEEKNO" | "BYSETPOS" | "BYHOUR" | "BYMINUTE" => return None,
            "BYDAY" => {
                let mut out = Vec::new();
                for d in value.split(',') {
                    let at = WEEKDAYS.iter().position(|w| w.eq_ignore_ascii_case(d.trim()))?;
                    let to = (i64::try_from(at).ok()? + days).rem_euclid(7);
                    out.push(*WEEKDAYS.get(usize::try_from(to).ok()?)?);
                }
                parts.push(format!("{key}={}", out.join(",")));
            }
            _ => parts.push(part.to_string()),
        }
    }
    Some(parts.join(";"))
}

/// Move series `id`: its occurrence on `day` (as the calendar shows days) to `start`–`end` (as the
/// calendar shows times, or dates for an all-day series), every other occurrence by as much.
pub fn move_series(s: &mut Session, id: EventId, day: Date, start: &str, end: Option<&str>) -> Result<()> {
    let view = s.zone().clone();
    let series = s.store.event(id)?.clone();
    let (own, shown) = occurrence(&series, day, &view)?;
    // How far it moves, in the series' own days and wall-clock minutes, and its new length.
    let (days, new_when) = match (series.when, own, shown) {
        (When::AllDay { start: first, .. }, When::AllDay { start: occ, end: occ_end }, _) => {
            let to = Date::parse(start).map_err(|_| Error::invalid("start", "an all-day series moves to a date: YYYY-MM-DD"))?;
            let length = match end {
                Some(e) => Date::parse(e).map_err(|_| Error::invalid("end", "YYYY-MM-DD"))?.day_number() - to.day_number(),
                None => occ_end.day_number() - occ.day_number(),
            };
            if !(1..=366).contains(&length) {
                return Err(Error::invalid("end", "an appointment lasts from one day to a year"));
            }
            let days = to.day_number() - occ.day_number();
            let new_start = first.add_days(days)?;
            (days, When::all_day(new_start, new_start.add_days(length)?)?)
        }
        (When::Timed { start: first, utc, .. }, When::Timed { start: occ, .. }, When::Timed { start: occ_shown, end: occ_shown_end, .. }) => {
            let (to, _) = DateTime::parse(start)?;
            let to_end = match end {
                Some(e) => DateTime::parse(e)?.0,
                None => shift_time(to, minutes(occ_shown_end) - minutes(occ_shown))?,
            };
            let length = minutes(to_end) - minutes(to);
            if !(1..=366 * 24 * 60).contains(&length) {
                return Err(Error::invalid("end", format!("{to_end} is not after the start {to}, or it lasts over a year")));
            }
            let own_zone = series.zone();
            let to_own_start = to_own(to, utc, own_zone.as_ref(), &view);
            let by = minutes(to_own_start) - minutes(occ);
            let days = to_own_start.date.day_number() - occ.date.day_number();
            let new_start = shift_time(first, by)?;
            (days, When::timed(new_start, shift_time(new_start, length)?, utc)?)
        }
        _ => return Err(Error::invalid("event", "the occurrence is not like its series")),
    };
    let rule = series.rrule.clone().unwrap_or_default();
    let Some(rule) = shifted_rule(&rule, days) else {
        return Err(Error::invalid(
            "start",
            "this series repeats on days its rule names (a day of the month, \"the first Monday\"…), so it moves only within its day; move this occurrence alone instead",
        ));
    };
    let shift = |d: Date| d.add_days(days).unwrap_or(d);
    let e = s.store.event_mut(id)?;
    e.when = new_when;
    e.rrule = Some(rule);
    e.exdates = e.exdates.iter().map(|d| shift(*d)).collect();
    if days != 0 {
        let changed: Vec<EventId> = s
            .store
            .events()
            .iter()
            .filter(|x| x.uid == series.uid && x.calendar == series.calendar && x.recurrence_id.is_some())
            .map(|x| x.id)
            .collect();
        for x in changed {
            let x = s.store.event_mut(x)?;
            x.recurrence_id = x.recurrence_id.map(shift);
        }
    }
    s.event = Some(id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_weekdays_shift_and_others_refuse() {
        assert_eq!(shifted_rule("FREQ=WEEKLY;BYDAY=MO,WE", 1).as_deref(), Some("FREQ=WEEKLY;BYDAY=TU,TH"));
        assert_eq!(shifted_rule("FREQ=WEEKLY;BYDAY=SU", 1).as_deref(), Some("FREQ=WEEKLY;BYDAY=MO"), "round the week");
        assert_eq!(shifted_rule("FREQ=WEEKLY;BYDAY=MO", -1).as_deref(), Some("FREQ=WEEKLY;BYDAY=SU"));
        assert_eq!(shifted_rule("FREQ=DAILY;COUNT=5", 3).as_deref(), Some("FREQ=DAILY;COUNT=5"));
        assert_eq!(shifted_rule("FREQ=MONTHLY;BYDAY=1MO", 1), None);
        assert_eq!(shifted_rule("FREQ=MONTHLY;BYMONTHDAY=15", 2), None);
        assert_eq!(shifted_rule("FREQ=MONTHLY;BYMONTHDAY=15", 0).as_deref(), Some("FREQ=MONTHLY;BYMONTHDAY=15"), "a time change only");
    }
}
