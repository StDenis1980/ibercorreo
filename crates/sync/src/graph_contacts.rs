//! Contact sync over Microsoft Graph, for Outlook.com and Microsoft 365 accounts: the same
//! [`contacts::State`] in and [`contacts::Batch`] out as CardDAV, so the store side
//! ([`contacts::apply`]) is shared.
//!
//! - The account's main Contacts folder is the address book `/v1.0/users/{user}/contacts/`, and
//!   each of its other folders `…/contactFolders/{id}/contacts/`; a contact is `…/{id}` in its
//!   book. These are real Graph paths, and the user's id keeps two accounts apart.
//! - Every sync lists each folder's contacts with their `@odata.etag` and converts only the new
//!   and changed ones, stored as vCard text as a CardDAV card is.
//! - Changes made here go back as `POST`, `PATCH` (`If-Match` the change key) and `DELETE`; when
//!   the same contact changed on both sides the server's copy wins.
//!
//! Outlook keeps at most three mail addresses and one mobile number a contact; more are said in
//! the errors when sent, not dropped silently.

use std::collections::{BTreeMap, BTreeSet};

use ibercorreo_core::contact::Phone;
use ibercorreo_core::{Address, Contact, Date};
use ibercorreo_dav::caldav::{FetchedItem, Http};
use ibercorreo_dav::carddav::RemoteBook;
use ibercorreo_dav::graph::Graph;
use ibercorreo_dav::{Error, Result};
use serde_json::{Value, json};

use crate::calendar::GRAPH_PREFIX;
use crate::contacts::{Batch, BookSync, Change, Pushed, State};
use crate::graph_calendar::{Failed, failed, segment, text};

/// Most contact folders read from one account.
pub const MAX_FOLDERS: usize = 200;
/// Most contacts read from one folder.
pub const MAX_CONTACTS: usize = 50_000;
/// Mail addresses Outlook keeps on a contact.
pub const MAX_EMAILS: usize = 3;

const CONTACT_FIELDS: &str = "id,displayName,emailAddresses,mobilePhone,businessPhones,homePhones,companyName,jobTitle,personalNotes,birthday";

/// Numbers from a Graph string array.
fn numbers(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// A Graph contact as a [`Contact`]. `None` for one with neither a name nor an address.
pub fn contact(v: &Value) -> Option<Contact> {
    let emails: Vec<Address> = v
        .get("emailAddresses")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let email = text(e, "address")?.to_string();
                    Some(Address { name: text(e, "name").map(str::to_string).filter(|n| *n != email), email })
                })
                .collect()
        })
        .unwrap_or_default();
    let name = text(v, "displayName").map(str::to_string).or_else(|| emails.first().map(|e| e.email.clone()))?;
    let mut c = Contact::new(&name);
    c.uid = text(v, "id")?.to_string();
    c.emails = emails;
    let phone = |kind: &str, number: String| Phone { kind: kind.to_string(), number };
    c.phones.extend(text(v, "mobilePhone").map(|n| phone("mobile", n.to_string())));
    c.phones.extend(numbers(v, "businessPhones").into_iter().map(|n| phone("work", n)));
    c.phones.extend(numbers(v, "homePhones").into_iter().map(|n| phone("home", n)));
    c.organization = text(v, "companyName").map(str::to_string);
    c.title = text(v, "jobTitle").map(str::to_string);
    c.note = text(v, "personalNotes").unwrap_or("").to_string();
    // `1980-05-01T11:59:00Z`: Outlook keeps the day at noon-ish UTC.
    c.birthday = text(v, "birthday").and_then(|b| b.get(..10)).and_then(|d| Date::parse(d).ok());
    Some(c)
}

/// `c` as a Graph contact body, and what Outlook cannot keep of it. `new` adds a given name and
/// surname from the name (Outlook files contacts by them); a change leaves Outlook's own.
pub fn body(c: &Contact, new: bool) -> (Value, Vec<String>) {
    let mut lost = Vec::new();
    let emails: Vec<Value> =
        c.emails.iter().take(MAX_EMAILS).map(|a| json!({ "name": a.name.as_deref().unwrap_or(&c.name), "address": a.email })).collect();
    if c.emails.len() > MAX_EMAILS {
        lost.push(format!("{}: Outlook keeps only the first {MAX_EMAILS} mail addresses", c.name));
    }
    let (mut mobile, mut business, mut home) = (None, Vec::new(), Vec::new());
    for p in &c.phones {
        match p.kind.as_str() {
            "mobile" if mobile.is_none() => mobile = Some(p.number.clone()),
            "work" => business.push(p.number.clone()),
            // A second mobile, a fax or a number of no kind: Outlook's home numbers hold it.
            _ => home.push(p.number.clone()),
        }
    }
    let mut v = json!({
        "displayName": c.name,
        "emailAddresses": emails,
        "mobilePhone": mobile,
        "businessPhones": business,
        "homePhones": home,
        "companyName": c.organization,
        "jobTitle": c.title,
        "personalNotes": c.note,
        "birthday": c.birthday.map(|d| format!("{d}T11:59:00Z")),
    });
    if new && let Some(o) = v.as_object_mut() {
        let mut words = c.name.split_whitespace();
        o.insert("givenName".into(), json!(words.next().unwrap_or("")));
        o.insert("surname".into(), json!(words.collect::<Vec<_>>().join(" ")));
    }
    (v, lost)
}

/// Send `state`'s changes for this account (`user`: its address books' href prefix).
fn push<H: Http>(g: &mut Graph<H>, user: &str, state: &State, batch: &mut Batch) -> Result<()> {
    for change in &state.changes {
        let (book, href) = match change {
            Change::Put { book, href, .. } | Change::Delete { book, href, .. } => (book, href),
        };
        if !(href.starts_with(book.as_str()) && book.starts_with(user)) {
            batch.errors.push(format!("{href}: not inside its address book; not sent"));
            continue;
        }
        let result: std::result::Result<Pushed, Failed> = match change {
            Change::Put { body: card, etag, contact: id, .. } => {
                let parsed = match ibercorreo_vcard::parse(card) {
                    Ok(p) => p.contacts.into_iter().next(),
                    Err(e) => {
                        batch.errors.push(format!("{href}: {e}"));
                        continue;
                    }
                };
                let Some(c) = parsed else { continue };
                let (json, lost) = body(&c, etag.is_none());
                batch.errors.extend(lost);
                match etag {
                    // A new contact's href is made up from its UID: it is created in its book.
                    None => match g.call("POST", book, &[], Some(&json)) {
                        Ok(Some(created)) => match text(&created, "id") {
                            Some(new) => Ok(Pushed::Put { href: format!("{book}{}", segment(new)), contact: *id }),
                            None => Err(Failed::Other(Error::Protocol("the new contact came back without an id".into()))),
                        },
                        Ok(None) => Err(Failed::Other(Error::Protocol("the new contact came back empty".into()))),
                        Err(e) => Err(failed(e)),
                    },
                    Some(etag) => match g.call("PATCH", href, &[("If-Match", etag)], Some(&json)) {
                        Ok(_) => Ok(Pushed::Put { href: href.clone(), contact: *id }),
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
            Err(Failed::Other(e @ (Error::Auth(_) | Error::Io(_)))) => return Err(e),
            Err(Failed::Other(e)) => batch.errors.push(format!("{href}: {e}")),
        }
    }
    Ok(())
}

/// Sync this account's Outlook contacts: send `state`'s changes (those of this account's address
/// books), then read every folder and convert the contacts that are new or changed since `state`.
/// Failing to reach or sign in to Graph is an error; a folder that fails on its own is reported
/// in [`Batch::errors`] and left as it is.
pub fn run<H: Http>(g: &mut Graph<H>, state: &State) -> Result<Batch> {
    let me = g.get("me?$select=id")?;
    let id = text(&me, "id").ok_or_else(|| Error::Protocol("Microsoft Graph did not say who is signed in".into()))?;
    let user = format!("{GRAPH_PREFIX}{}/", segment(id));
    let state = state.only(&|h| h.starts_with(&user));
    let mut batch = Batch { scope: Some(user.clone()), ..Batch::default() };
    push(g, &user, &state, &mut batch)?;
    let pushed: BTreeSet<String> = batch
        .pushed
        .iter()
        .map(|p| match p {
            Pushed::Put { href, .. } | Pushed::Deleted { href } | Pushed::Conflict { href } => href.clone(),
        })
        .collect();
    // The main Contacts folder, then the others.
    let mut books = vec![(format!("{user}contacts/"), "Contacts".to_string())];
    let folders = g.list(&format!("{user}contactFolders?$select=id,displayName&$top=100"), &[], MAX_FOLDERS)?;
    for f in &folders.items {
        let Some(fid) = text(f, "id") else { continue };
        books.push((format!("{user}contactFolders/{}/contacts/", segment(fid)), text(f, "displayName").unwrap_or("Contacts").to_string()));
    }
    for (href, name) in books {
        let book = RemoteBook { href: href.clone(), name: name.clone(), ctag: None };
        let empty = BTreeMap::new();
        let known = state.books.get(&href).map_or(&empty, |k| &k.items);
        let listed = match g.list(&format!("{href}?$select={CONTACT_FIELDS}&$top=500"), &[], MAX_CONTACTS) {
            Ok(l) => l.items,
            Err(e @ (Error::Auth(_) | Error::Io(_))) => return Err(e),
            Err(e) => {
                batch.errors.push(format!("{name}: {e}"));
                batch.books.push(BookSync { book, unchanged: true, fetched: Vec::new(), removed: Vec::new() });
                continue;
            }
        };
        let mut seen = BTreeSet::new();
        let mut fetched = Vec::new();
        for v in &listed {
            let (Some(cid), Some(etag)) = (text(v, "id"), text(v, "@odata.etag")) else { continue };
            let card_href = format!("{href}{}", segment(cid));
            seen.insert(card_href.clone());
            if known.get(&card_href).is_some_and(|k| k == etag) && !pushed.contains(&card_href) {
                continue;
            }
            // A contact with no name and no address has nothing to show.
            let Some(c) = contact(v) else { continue };
            fetched.push(FetchedItem { href: card_href, etag: etag.to_string(), data: ibercorreo_vcard::write_card(&c) });
        }
        let removed: Vec<String> = known.keys().filter(|h| !seen.contains(*h)).cloned().collect();
        batch.books.push(BookSync { book, unchanged: false, fetched, removed });
    }
    Ok(batch)
}

#[cfg(test)]
mod tests;
