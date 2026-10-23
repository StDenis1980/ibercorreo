//! Contact sync over CardDAV (RFC 6352): the server's address books mirrored into the store, both
//! ways, so the contacts here are the ones on the phone.
//!
//! The same three parts as calendar sync ([`crate::calendar`]):
//! 1. [`state`] (on the store): the address books this store mirrors, their change markers, the
//!    version (ETag) of every card, and the contacts added to a server address book, changed or
//!    deleted here.
//! 2. [`run`] (on the connection): send those changes, each only if the server's card is still
//!    the version last seen; then find the address books, and for each whose marker changed
//!    list the ETags and fetch only the cards that are new or changed.
//! 3. [`apply`] (on the store): link what was sent, then add, update and remove address books
//!    and contacts.
//!
//! When the same contact changed on both sides, the server's card wins: the write is refused
//! (`412`), and the server's version comes down in the same sync.

use std::collections::BTreeMap;

use ibercorreo_core::{CalendarRemote, Contact, EventRemote};
use ibercorreo_dav::Url;
use ibercorreo_dav::caldav::{FetchedItem, Http, Written};
use ibercorreo_dav::carddav::{self, RemoteBook};
use ibercorreo_store::Store;
use serde::Serialize;

use crate::calendar::{Known, resource_name};

/// A change made here, to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Change {
    /// Write card `href` in `book` from contact `contact`: new when `etag` is `None`.
    Put { book: String, href: String, etag: Option<String>, contact: u64, body: String },
    /// Delete card `href`, last seen as `etag`.
    Delete { book: String, href: String, etag: String },
}

/// What the store holds of the server's address books, by collection href, and the changes to
/// send.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub books: BTreeMap<String, Known>,
    pub changes: Vec<Change>,
}

/// Most changes sent in one sync; the rest go in the next.
pub const MAX_CHANGES: usize = 500;

pub fn state(store: &Store) -> State {
    let mut out = State::default();
    for b in store.books() {
        let Some(remote) = &b.remote else { continue };
        let mut known = Known { ctag: remote.ctag.clone(), items: BTreeMap::new() };
        for c in store.all_contacts().iter().filter(|c| c.book == b.id) {
            match &c.remote {
                Some(r) => {
                    known.items.insert(r.href.clone(), r.etag.clone());
                    if r.dirty {
                        let etag = Some(r.etag.clone()).filter(|e| !e.is_empty());
                        let body = ibercorreo_vcard::write_card(c);
                        out.changes.push(Change::Put { book: remote.href.clone(), href: r.href.clone(), etag, contact: c.id, body });
                    }
                }
                None => {
                    let href = format!("{}{}.vcf", remote.href, resource_name(&c.uid));
                    let body = ibercorreo_vcard::write_card(c);
                    out.changes.push(Change::Put { book: remote.href.clone(), href, etag: None, contact: c.id, body });
                }
            }
        }
        out.books.insert(remote.href.clone(), known);
    }
    for d in store.card_deletions() {
        out.changes.push(Change::Delete { book: d.calendar.clone(), href: d.href.clone(), etag: d.etag.clone() });
    }
    out.changes.truncate(MAX_CHANGES);
    out
}

/// What became of one [`Change`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Pushed {
    /// Written; contact `contact` now lives at `href`.
    Put { href: String, contact: u64 },
    /// Deleted, or already gone.
    Deleted { href: String },
    /// The server's card changed meanwhile, so it wins: it comes down in this sync.
    Conflict { href: String },
}

/// One server address book after [`run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookSync {
    pub book: RemoteBook,
    /// Nothing changed since the last sync (or listing it failed: see [`Batch::errors`]).
    pub unchanged: bool,
    /// Cards new or changed, with their text.
    pub fetched: Vec<FetchedItem>,
    /// Cards the server no longer has.
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    /// The changes sent, and what became of them. A change that failed is not here, and is sent
    /// again next time.
    pub pushed: Vec<Pushed>,
    /// Every address book the server has, changed or not.
    pub books: Vec<BookSync>,
    pub errors: Vec<String>,
}

/// Send `state`'s changes; returns the state as it stands after them (what was written is
/// fetched again, since the server may store its own version of it).
fn push(http: &mut dyn Http, url: &Url, state: &State, batch: &mut Batch) -> ibercorreo_dav::Result<State> {
    let mut after = state.clone();
    after.changes.clear();
    for change in &state.changes {
        let (book, href) = match change {
            Change::Put { book, href, .. } | Change::Delete { book, href, .. } => (book, href),
        };
        // Only cards inside their address book, on this server.
        let inside = url.join(href).is_some_and(|p| p.starts_with(book.as_str()) && p.len() > book.len());
        if !inside {
            batch.errors.push(format!("{href}: not inside its address book; not sent"));
            continue;
        }
        let result = match change {
            Change::Put { body, etag, .. } => carddav::put(http, href, body, etag.as_deref()),
            Change::Delete { etag, .. } => ibercorreo_dav::caldav::delete(http, href, etag),
        };
        let known = after.books.entry(book.clone()).or_default();
        // Whatever happened, the book changed or may have: list it again.
        known.ctag = None;
        match (change, result) {
            // Signed out or offline: stop, and send everything next time.
            (_, Err(e @ (ibercorreo_dav::Error::Auth(_) | ibercorreo_dav::Error::Io(_)))) => return Err(e),
            (_, Err(e)) => batch.errors.push(format!("{href}: {e}")),
            (Change::Put { contact, .. }, Ok(Written::Done { .. })) => {
                known.items.insert(href.clone(), String::new());
                batch.pushed.push(Pushed::Put { href: href.clone(), contact: *contact });
            }
            (Change::Put { etag: None, .. }, Ok(Written::Conflict)) => {
                batch.errors.push(format!("{href}: the server already has a card there; not sent"));
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

/// Find the address books at `url` and fetch what changed since `state`. Failing to reach or
/// sign in to the server is an error; a book that fails on its own is reported in
/// [`Batch::errors`] and left as it is.
pub fn run(http: &mut dyn Http, url: &Url, state: &State) -> ibercorreo_dav::Result<Batch> {
    let mut batch = Batch::default();
    let state = &push(http, url, state, &mut batch)?;
    for book in carddav::discover(http, url)? {
        let known = state.books.get(&book.href);
        if let Some(k) = known
            && k.ctag.is_some()
            && k.ctag == book.ctag
        {
            batch.books.push(BookSync { book, unchanged: true, fetched: Vec::new(), removed: Vec::new() });
            continue;
        }
        let empty = BTreeMap::new();
        let items = known.map_or(&empty, |k| &k.items);
        let listed = match carddav::list(http, url, &book.href) {
            Ok(listed) => listed,
            Err(e) => {
                batch.errors.push(format!("{}: {e}", book.name));
                let book = RemoteBook { ctag: known.and_then(|k| k.ctag.clone()), ..book };
                batch.books.push(BookSync { book, unchanged: true, fetched: Vec::new(), removed: Vec::new() });
                continue;
            }
        };
        let changed: Vec<String> = listed.iter().filter(|i| items.get(&i.href) != Some(&i.etag)).map(|i| i.href.clone()).collect();
        let removed: Vec<String> = items.keys().filter(|h| !listed.iter().any(|i| &i.href == *h)).cloned().collect();
        let mut book = book;
        let fetched = match carddav::fetch(http, url, &book.href, &changed) {
            Ok(f) => f,
            Err(e) => {
                batch.errors.push(format!("{}: {e}", book.name));
                Vec::new()
            }
        };
        // Something listed but not fetched: forget the marker, so the next sync lists again.
        if fetched.len() < changed.len() {
            book.ctag = None;
        }
        batch.books.push(BookSync { book, unchanged: false, fetched, removed });
    }
    Ok(batch)
}

/// What [`apply`] changed, for the status bar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Changes made here and written to the server.
    pub sent: usize,
    /// Changes made here that the server's newer card replaced.
    pub conflicts: usize,
    pub books_added: usize,
    pub books_removed: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub errors: Vec<String>,
}

/// Fold a [`run`] result into the store. Address books the server no longer has are removed here
/// too.
pub fn apply(store: &mut Store, batch: Batch) -> Report {
    let mut report = Report { errors: batch.errors, ..Report::default() };
    for p in &batch.pushed {
        match p {
            Pushed::Put { href, contact } => {
                report.sent = report.sent.saturating_add(1);
                store.mark_card_sent(*contact, href);
            }
            Pushed::Deleted { href } => {
                report.sent = report.sent.saturating_add(1);
                store.forget_card_deletion(href);
            }
            Pushed::Conflict { href } => {
                report.conflicts = report.conflicts.saturating_add(1);
                store.forget_card_deletion(href);
            }
        }
    }
    let hrefs: Vec<String> = batch.books.iter().map(|bs| bs.book.href.clone()).collect();
    for bs in batch.books {
        let book = &bs.book;
        let id = match store.book_by_href(&book.href) {
            Some(id) => id,
            None => match store.add_book(&book.name, Some(CalendarRemote { href: book.href.clone(), ctag: None })) {
                Ok(id) => {
                    report.books_added = report.books_added.saturating_add(1);
                    id
                }
                Err(e) => {
                    report.errors.push(format!("{}: {e}", book.name));
                    continue;
                }
            },
        };
        if let Ok(b) = store.book_mut(id) {
            b.name = ibercorreo_core::clamp_text(&book.name, 200).to_string();
        }
        if bs.unchanged {
            continue;
        }
        for item in bs.fetched {
            let card = match ibercorreo_vcard::parse(&item.data) {
                Ok(p) => p.contacts.into_iter().next(),
                Err(e) => {
                    report.errors.push(format!("{}: {e}", item.href));
                    continue;
                }
            };
            let Some(card) = card else {
                report.errors.push(format!("{}: no contact in the card", item.href));
                continue;
            };
            let c = Contact { remote: Some(EventRemote::new(&item.href, &item.etag, "")), ..card };
            match store.replace_remote_card(id, &item.href, c) {
                Ok(true) => report.added = report.added.saturating_add(1),
                Ok(false) => report.updated = report.updated.saturating_add(1),
                Err(e) => report.errors.push(format!("{}: {e}", item.href)),
            }
        }
        for href in &bs.removed {
            if store.remove_remote_card(id, href) > 0 {
                report.removed = report.removed.saturating_add(1);
            }
        }
        if let Ok(b) = store.book_mut(id) {
            b.remote = Some(CalendarRemote { href: book.href.clone(), ctag: book.ctag.clone() });
        }
    }
    // A batch exists only after a run that listed every address book, so a network failure never
    // empties the store.
    remove_missing(store, &hrefs, &mut report);
    report
}

/// Remove the address books mirrored from a server that are not in `hrefs`.
fn remove_missing(store: &mut Store, hrefs: &[String], report: &mut Report) {
    let gone: Vec<u32> = store.books().iter().filter(|b| b.remote.as_ref().is_some_and(|r| !hrefs.contains(&r.href))).map(|b| b.id).collect();
    for id in gone {
        match store.remove_book(id) {
            Ok(_) => report.books_removed = report.books_removed.saturating_add(1),
            // The last book stays, emptied of what came from the server.
            Err(_) => {
                if let Ok(b) = store.book_mut(id) {
                    b.remote = None;
                }
                let hrefs: Vec<String> =
                    store.all_contacts().iter().filter(|c| c.book == id).filter_map(|c| c.remote.as_ref().map(|r| r.href.clone())).collect();
                for h in hrefs {
                    store.remove_remote_card(id, &h);
                }
            }
        }
    }
}

/// Stop mirroring: forget every server address book (when the server address is removed).
pub fn forget(store: &mut Store) -> Report {
    let mut report = Report::default();
    remove_missing(store, &[], &mut report);
    report
}

#[cfg(test)]
mod tests;
