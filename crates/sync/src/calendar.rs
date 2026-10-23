//! Calendar sync over CalDAV (RFC 4791): the server's calendars and events mirrored into the
//! store. Pull only for now: changes made here to a server calendar are not sent back yet
//! (`docs/gaps.md`).
//!
//! The same three parts as mail sync:
//! 1. [`state`] (on the store): the calendars this store mirrors, their change markers, and the
//!    version (ETag) of every event resource.
//! 2. [`run`] (on the connection): find the calendars; for each whose marker changed, list the
//!    ETags and fetch only the resources that are new or changed.
//! 3. [`apply`] (on the store): add, update and remove calendars and events.

use std::collections::BTreeMap;

use ibercorreo_core::{CalendarRemote, EventId, EventRemote};
use ibercorreo_dav::Url;
use ibercorreo_dav::caldav::{self, FetchedItem, Http, RemoteCalendar};
use ibercorreo_store::Store;
use serde::Serialize;

/// What the store holds of one server calendar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    pub ctag: Option<String>,
    /// Event resources by href, with their ETags.
    pub items: BTreeMap<String, String>,
}

/// What the store holds of the server's calendars, by collection href.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub calendars: BTreeMap<String, Known>,
}

pub fn state(store: &Store) -> State {
    let mut out = State::default();
    for c in store.calendars() {
        let Some(remote) = &c.remote else { continue };
        let mut known = Known { ctag: remote.ctag.clone(), items: BTreeMap::new() };
        for e in store.events().iter().filter(|e| e.calendar == c.id) {
            if let Some(r) = &e.remote {
                known.items.insert(r.href.clone(), r.etag.clone());
            }
        }
        out.calendars.insert(remote.href.clone(), known);
    }
    out
}

/// One server calendar after [`run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarSync {
    pub calendar: RemoteCalendar,
    /// Nothing changed since the last sync (or listing it failed: see [`Batch::errors`]).
    pub unchanged: bool,
    /// Resources new or changed, with their text.
    pub fetched: Vec<FetchedItem>,
    /// Resources the server no longer has.
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    /// Every calendar the server has (that can hold events), changed or not.
    pub calendars: Vec<CalendarSync>,
    pub errors: Vec<String>,
}

/// Find the calendars at `url` and fetch what changed since `state`. Failing to reach or sign in
/// to the server is an error; a calendar that fails on its own is reported in
/// [`Batch::errors`] and left as it is.
pub fn run(http: &mut dyn Http, url: &Url, state: &State) -> ibercorreo_dav::Result<Batch> {
    let mut batch = Batch::default();
    for cal in caldav::discover(http, url)? {
        let known = state.calendars.get(&cal.href);
        if let Some(k) = known
            && k.ctag.is_some()
            && k.ctag == cal.ctag
        {
            batch.calendars.push(CalendarSync { calendar: cal, unchanged: true, fetched: Vec::new(), removed: Vec::new() });
            continue;
        }
        let empty = BTreeMap::new();
        let items = known.map_or(&empty, |k| &k.items);
        let listed = match caldav::list(http, url, &cal.href) {
            Ok(listed) => listed,
            Err(e) => {
                batch.errors.push(format!("{}: {e}", cal.name));
                let calendar = RemoteCalendar { ctag: known.and_then(|k| k.ctag.clone()), ..cal };
                batch.calendars.push(CalendarSync { calendar, unchanged: true, fetched: Vec::new(), removed: Vec::new() });
                continue;
            }
        };
        let changed: Vec<String> = listed.iter().filter(|i| items.get(&i.href) != Some(&i.etag)).map(|i| i.href.clone()).collect();
        let removed: Vec<String> = items.keys().filter(|h| !listed.iter().any(|i| &i.href == *h)).cloned().collect();
        let mut calendar = cal;
        let fetched = match caldav::fetch(http, url, &calendar.href, &changed) {
            Ok(f) => f,
            Err(e) => {
                batch.errors.push(format!("{}: {e}", calendar.name));
                Vec::new()
            }
        };
        // Something listed but not fetched: forget the marker, so the next sync lists again.
        if fetched.len() < changed.len() {
            calendar.ctag = None;
        }
        batch.calendars.push(CalendarSync { calendar, unchanged: false, fetched, removed });
    }
    Ok(batch)
}

/// What [`apply`] changed, for the status bar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub calendars_added: usize,
    pub calendars_removed: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub errors: Vec<String>,
}

/// The colour used for a server calendar that has none.
const DEFAULT_COLOR: &str = "#1F7A8C";

/// Fold a [`run`] result into the store. Calendars the server no longer has are removed here too.
pub fn apply(store: &mut Store, batch: Batch) -> Report {
    let mut report = Report { errors: batch.errors, ..Report::default() };
    let hrefs: Vec<String> = batch.calendars.iter().map(|cs| cs.calendar.href.clone()).collect();
    for cs in batch.calendars {
        let cal = &cs.calendar;
        // `#RRGGBBAA` from Apple's servers: the alpha is dropped.
        let color = cal.color.as_deref().and_then(|c| c.get(..7)).unwrap_or(DEFAULT_COLOR).to_string();
        let id = match store.calendar_by_href(&cal.href) {
            Some(id) => id,
            None => match store.add_calendar(&cal.name, &color, Some(CalendarRemote { href: cal.href.clone(), ctag: None })) {
                Ok(id) => {
                    report.calendars_added = report.calendars_added.saturating_add(1);
                    id
                }
                Err(e) => {
                    report.errors.push(format!("{}: {e}", cal.name));
                    continue;
                }
            },
        };
        if let Ok(c) = store.calendar_mut(id) {
            c.name = ibercorreo_core::clamp_text(&cal.name, 200).to_string();
            if cal.color.is_some() {
                c.color = color;
            }
        }
        if cs.unchanged {
            continue;
        }
        for item in cs.fetched {
            let parsed = match ibercorreo_ical::parse(&item.data) {
                Ok(p) => p,
                Err(e) => {
                    report.errors.push(format!("{}: {e}", item.href));
                    continue;
                }
            };
            let remote = EventRemote { href: item.href.clone(), etag: item.etag.clone() };
            let events = parsed
                .events
                .into_iter()
                .map(|ev| {
                    let mut e = ev.into_event(EventId(0), id);
                    e.remote = Some(remote.clone());
                    e
                })
                .collect();
            match store.replace_remote_events(id, &item.href, events) {
                Ok(true) => report.added = report.added.saturating_add(1),
                Ok(false) => report.updated = report.updated.saturating_add(1),
                Err(e) => report.errors.push(format!("{}: {e}", item.href)),
            }
        }
        for href in &cs.removed {
            if store.remove_remote_events(id, href) > 0 {
                report.removed = report.removed.saturating_add(1);
            }
        }
        if let Ok(c) = store.calendar_mut(id) {
            c.remote = Some(CalendarRemote { href: cal.href.clone(), ctag: cal.ctag.clone() });
        }
    }
    // A batch exists only after a run that listed every calendar, so a network failure never
    // empties the store.
    remove_missing(store, &hrefs, &mut report);
    report
}

/// Remove the calendars mirrored from a server that are not in `hrefs`.
fn remove_missing(store: &mut Store, hrefs: &[String], report: &mut Report) {
    let gone: Vec<u32> = store.calendars().iter().filter(|c| c.remote.as_ref().is_some_and(|r| !hrefs.contains(&r.href))).map(|c| c.id).collect();
    for id in gone {
        match store.remove_calendar(id) {
            Ok(_) => report.calendars_removed = report.calendars_removed.saturating_add(1),
            // The last calendar stays, emptied of what came from the server.
            Err(_) => {
                if let Ok(c) = store.calendar_mut(id) {
                    c.remote = None;
                }
                let hrefs: Vec<String> =
                    store.events().iter().filter(|e| e.calendar == id).filter_map(|e| e.remote.as_ref().map(|r| r.href.clone())).collect();
                for h in hrefs {
                    store.remove_remote_events(id, &h);
                }
            }
        }
    }
}

/// Stop mirroring: forget every server calendar (when the CalDAV address is removed).
pub fn forget(store: &mut Store) -> Report {
    let mut report = Report::default();
    remove_missing(store, &[], &mut report);
    report
}

#[cfg(test)]
mod tests;
