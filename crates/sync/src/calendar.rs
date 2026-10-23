//! Calendar sync over CalDAV (RFC 4791): the server's calendars mirrored into the store, both ways.
//!
//! The same three parts as mail sync:
//! 1. [`state`] (on the store): the calendars this store mirrors, their change markers, the
//!    version (ETag) of every event resource, and the changes made here: appointments added to a
//!    server calendar, changed, or deleted.
//! 2. [`run`] (on the connection): send those changes, each only if the server's copy is still
//!    the version last seen; then find the calendars, and for each whose marker changed list
//!    the ETags and fetch only the resources that are new or changed.
//! 3. [`apply`] (on the store): link what was sent, then add, update and remove calendars and
//!    events.
//!
//! When the same appointment changed on both sides, the server's copy wins: the write is refused
//! (`412`), and the server's version comes down in the same sync.

use std::collections::BTreeMap;

use ibercorreo_core::{CalendarRemote, DateTime, Event, EventId, EventRemote};
use ibercorreo_dav::Url;
use ibercorreo_dav::caldav::{self, FetchedItem, Http, RemoteCalendar, Written};
use ibercorreo_store::Store;
use serde::Serialize;

/// What the store holds of one server calendar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    pub ctag: Option<String>,
    /// Event resources by href, with their ETags.
    pub items: BTreeMap<String, String>,
}

/// A change made here, to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Change {
    /// Write resource `href` in `calendar`: new when `etag` is `None`. `uid` names the events it
    /// holds.
    Put { calendar: String, href: String, etag: Option<String>, uid: String, body: String },
    /// Delete resource `href`, last seen as `etag`.
    Delete { calendar: String, href: String, etag: String },
}

/// What the store holds of the server's calendars, by collection href, and the changes to send.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub calendars: BTreeMap<String, Known>,
    pub changes: Vec<Change>,
}

/// Most changes sent in one sync; the rest go in the next.
pub const MAX_CHANGES: usize = 500;

/// The resource name for a new appointment: its UID with anything but letters, digits and `-_.`
/// replaced, so it is a safe path segment that servers list back exactly as sent (no `%40`).
pub(crate) fn resource_name(uid: &str) -> String {
    let name: String = uid.chars().take(120).map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    let name = name.trim_matches('.');
    if name.is_empty() { "event".to_string() } else { name.to_string() }
}

/// `now` (UTC) is written as the changes' `DTSTAMP`.
pub fn state(store: &Store, now: DateTime) -> State {
    let mut out = State::default();
    for c in store.calendars() {
        let Some(remote) = &c.remote else { continue };
        let mut known = Known { ctag: remote.ctag.clone(), items: BTreeMap::new() };
        let events: Vec<&Event> = store.events().iter().filter(|e| e.calendar == c.id).collect();
        for e in &events {
            if let Some(r) = &e.remote {
                known.items.insert(r.href.clone(), r.etag.clone());
            }
        }
        // Changed resources: everything that shares the href goes back together.
        let mut dirty: Vec<&str> = events.iter().filter_map(|e| e.remote.as_ref().filter(|r| r.dirty).map(|r| r.href.as_str())).collect();
        dirty.sort_unstable();
        dirty.dedup();
        for href in dirty {
            let group: Vec<&Event> = events.iter().copied().filter(|e| e.remote.as_ref().is_some_and(|r| r.href == href)).collect();
            let Some(first) = group.first().and_then(|e| e.remote.as_ref()) else { continue };
            let etag = Some(first.etag.clone()).filter(|e| !e.is_empty());
            let body = ibercorreo_ical::write_resource(group.iter().copied(), &first.raw, now);
            let uid = group.first().map(|e| e.uid.clone()).unwrap_or_default();
            out.changes.push(Change::Put { calendar: remote.href.clone(), href: href.to_string(), etag, uid, body });
        }
        // New appointments: one resource per UID.
        let mut uids: Vec<&str> = events.iter().filter(|e| e.remote.is_none()).map(|e| e.uid.as_str()).collect();
        uids.sort_unstable();
        uids.dedup();
        for uid in uids {
            let group: Vec<&Event> = events.iter().copied().filter(|e| e.remote.is_none() && e.uid == uid).collect();
            let href = format!("{}{}.ics", remote.href, resource_name(uid));
            let body = ibercorreo_ical::write_resource(group.iter().copied(), "", now);
            out.changes.push(Change::Put { calendar: remote.href.clone(), href, etag: None, uid: uid.to_string(), body });
        }
        out.calendars.insert(remote.href.clone(), known);
    }
    for d in store.remote_deletions() {
        out.changes.push(Change::Delete { calendar: d.calendar.clone(), href: d.href.clone(), etag: d.etag.clone() });
    }
    out.changes.truncate(MAX_CHANGES);
    out
}

/// What became of one [`Change`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Pushed {
    /// Written; the events of `uid` in `calendar` now live at `href`.
    Put { calendar: String, href: String, uid: String },
    /// Deleted, or already gone.
    Deleted { href: String },
    /// The server's copy changed meanwhile, so it wins: it comes down in this sync.
    Conflict { href: String },
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
    /// The changes sent, and what became of them. A change that failed is not here, and is sent
    /// again next time.
    pub pushed: Vec<Pushed>,
    /// Every calendar the server has (that can hold events), changed or not.
    pub calendars: Vec<CalendarSync>,
    pub errors: Vec<String>,
}

/// Send `state`'s changes; returns the state as it stands after them (what was written must be
/// fetched again, since the server may store its own version of it).
fn push(http: &mut dyn Http, url: &Url, state: &State, batch: &mut Batch) -> ibercorreo_dav::Result<State> {
    let mut after = state.clone();
    after.changes.clear();
    for change in &state.changes {
        let (calendar, href) = match change {
            Change::Put { calendar, href, .. } | Change::Delete { calendar, href, .. } => (calendar, href),
        };
        // Only resources inside their calendar, on this server.
        let inside = url.join(href).is_some_and(|p| p.starts_with(calendar.as_str()) && p.len() > calendar.len());
        if !inside {
            batch.errors.push(format!("{href}: not inside its calendar; not sent"));
            continue;
        }
        let result = match change {
            Change::Put { body, etag, .. } => caldav::put(http, href, body, etag.as_deref()),
            Change::Delete { etag, .. } => caldav::delete(http, href, etag),
        };
        let known = after.calendars.entry(calendar.clone()).or_default();
        // Whatever happened, the calendar changed or may have: list it again.
        known.ctag = None;
        match (change, result) {
            // Signed out or offline: stop, and send everything next time.
            (_, Err(e @ (ibercorreo_dav::Error::Auth(_) | ibercorreo_dav::Error::Io(_)))) => return Err(e),
            (_, Err(e)) => batch.errors.push(format!("{href}: {e}")),
            (Change::Put { uid, .. }, Ok(Written::Done { .. })) => {
                known.items.insert(href.clone(), String::new());
                batch.pushed.push(Pushed::Put { calendar: calendar.clone(), href: href.clone(), uid: uid.clone() });
            }
            (Change::Put { etag: None, .. }, Ok(Written::Conflict)) => {
                batch.errors.push(format!("{href}: the server already has an appointment there; not sent"));
            }
            (Change::Delete { .. }, Ok(Written::Done { .. } | Written::Gone)) => {
                known.items.remove(href);
                batch.pushed.push(Pushed::Deleted { href: href.clone() });
            }
            (_, Ok(Written::Conflict | Written::Gone)) => {
                known.items.insert(href.clone(), String::new());
                batch.pushed.push(Pushed::Conflict { href: href.clone() });
            }
        }
    }
    Ok(after)
}

/// Find the calendars at `url` and fetch what changed since `state`. Failing to reach or sign in
/// to the server is an error; a calendar that fails on its own is reported in
/// [`Batch::errors`] and left as it is.
pub fn run(http: &mut dyn Http, url: &Url, state: &State) -> ibercorreo_dav::Result<Batch> {
    let mut batch = Batch::default();
    let state = &push(http, url, state, &mut batch)?;
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
    /// Changes made here and written to the server.
    pub sent: usize,
    /// Changes made here that the server's newer copy replaced.
    pub conflicts: usize,
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
    for p in &batch.pushed {
        match p {
            Pushed::Put { calendar, href, uid } => {
                report.sent = report.sent.saturating_add(1);
                if let Some(id) = store.calendar_by_href(calendar) {
                    store.mark_remote_sent(id, uid, href);
                }
            }
            Pushed::Deleted { href } => {
                report.sent = report.sent.saturating_add(1);
                store.forget_remote_deletion(href);
            }
            Pushed::Conflict { href } => {
                report.conflicts = report.conflicts.saturating_add(1);
                store.forget_remote_deletion(href);
            }
        }
    }
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
            let remote = EventRemote::new(&item.href, &item.etag, &item.data);
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
