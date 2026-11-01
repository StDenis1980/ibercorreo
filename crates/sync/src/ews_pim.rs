//! Calendars and contacts over Exchange Web Services, both ways, for an account whose mail goes
//! over EWS ([`crate::ews_mail`]). The same [`calendar::State`] / [`contacts::State`] in and
//! batches out as CalDAV and CardDAV, so the store side ([`calendar::apply`],
//! [`contacts::apply`]) is shared.
//!
//! - Each calendar folder (class `IPF.Appointment`) and contacts folder (`IPF.Contact`) is a
//!   collection named [`EWS_PREFIX`], the server, then the folder's id; an item is the collection
//!   and its id. An item's version is its EWS `ChangeKey`.
//! - Items are read as Exchange exports them, iCalendar and vCard (`MimeContent`), so the
//!   CalDAV and CardDAV readers are reused, and written back the same way (`CreateItem` with
//!   `MimeContent`, as Exchange imports them). EWS cannot change an item's MIME content in place,
//!   so a change made here replaces the item: the new one is created, then the old one deleted.
//! - A change goes only if the server's `ChangeKey` is still the one last seen; otherwise the
//!   server's copy wins and comes down in the same sync, as over CalDAV.
//! - No invitations or cancellations are sent for meetings changed here (`SendToNone`).

use std::collections::{BTreeMap, BTreeSet};

use ibercorreo_dav::caldav::{FetchedItem, Http, RemoteCalendar};
use ibercorreo_dav::carddav::RemoteBook;
use ibercorreo_dav::ews::{Ews, MESSAGES_NS, TYPES_NS};
use ibercorreo_dav::xml::escape;
use ibercorreo_dav::{Error, Result};

use ews::delete_item::{DeleteItem, SendMeetingCancellations};
use ews::find_item::{FindItem, Traversal};
use ews::get_item::GetItem;
use ews::{BaseFolderId, BaseItemId, BasePoint, BaseShape, DeleteType, Folder, ItemShape, View};

use crate::calendar::{self, CalendarSync, Known};
use crate::contacts::{self, BookSync};
use crate::ews_mail::{PAGE, hierarchy};
use crate::graph_calendar::segment;
use crate::server_mail::fatal;

/// Collection hrefs of calendars and address books mirrored over EWS start with this (then the
/// server); see [`calendar::is_caldav`].
pub const EWS_PREFIX: &str = "/ews/";
/// Most items listed in one folder.
pub const MAX_ITEMS: usize = 100_000;
/// Most folders of one kind.
const MAX_FOLDERS: usize = 200;
/// Largest item read.
const MAX_ITEM_BYTES: usize = 4 * 1024 * 1024;

/// Calendars or contacts.
#[derive(Clone, Copy)]
struct Kind {
    /// The folder class (`IPF.Appointment`): only folders of exactly this class, so Exchange's
    /// hidden ones (`IPF.Contact.RecipientCache`, the birthday calendar…) are left out.
    class: &'static str,
    /// The item's element in a `CreateItem` (`CalendarItem`).
    item: &'static str,
    /// The text an item holds, from its first line to its last.
    begin: &'static str,
    end: &'static str,
    /// The attribute `CreateItem` needs for this kind.
    create_attr: &'static str,
}

const CALENDAR: Kind = Kind {
    class: "IPF.Appointment",
    item: "CalendarItem",
    begin: "BEGIN:VCALENDAR",
    end: "END:VCALENDAR",
    create_attr: r#" SendMeetingInvitations="SendToNone""#,
};
const CONTACTS: Kind = Kind { class: "IPF.Contact", item: "Contact", begin: "BEGIN:VCARD", end: "END:VCARD", create_attr: "" };

/// The collection prefix for the server at `ews` (its EWS address).
pub fn prefix(ews: &str) -> String {
    let server = ews.trim().trim_start_matches("https://").trim_start_matches("HTTPS://").to_ascii_lowercase();
    format!("{EWS_PREFIX}{}/", segment(&server))
}

/// A path segment back to the id it was made from ([`segment`]).
fn unsegment(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The folder id of collection `href` under `prefix`.
fn folder_id(prefix: &str, href: &str) -> Option<String> {
    let seg = href.strip_prefix(prefix)?.strip_suffix('/')?;
    if seg.is_empty() || seg.contains('/') {
        return None;
    }
    unsegment(seg)
}

/// The item id of `href` inside collection `collection`.
fn item_id(collection: &str, href: &str) -> Option<String> {
    let seg = href.strip_prefix(collection)?;
    if seg.is_empty() || seg.contains('/') {
        return None;
    }
    unsegment(seg)
}

/// The folders of `kind`: (id, name), the standard one first.
fn folders<H: Http>(e: &mut Ews<H>, kind: Kind) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    for f in hierarchy(e, MAX_FOLDERS.saturating_mul(20))? {
        let (id, class, name) = match &f {
            Folder::CalendarFolder { folder_id, folder_class, display_name, .. }
            | Folder::ContactsFolder { folder_id, folder_class, display_name, .. }
            | Folder::Folder { folder_id, folder_class, display_name, .. } => (folder_id, folder_class, display_name),
            _ => continue,
        };
        let (Some(id), Some(class)) = (id, class) else { continue };
        if class != kind.class || out.iter().any(|(i, _)| *i == id.id) {
            continue;
        }
        out.push((id.id.clone(), name.clone().unwrap_or_else(|| kind.item.to_string())));
    }
    out.truncate(MAX_FOLDERS);
    Ok(out)
}

/// The items of a folder: (id, change key).
fn list<H: Http>(e: &mut Ews<H>, folder: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    loop {
        let op = FindItem {
            traversal: Traversal::Shallow,
            item_shape: ItemShape { base_shape: BaseShape::IdOnly, include_mime_content: None, additional_properties: None },
            view: Some(View::IndexedPageItemView { max_entries_returned: Some(PAGE), base_point: BasePoint::Beginning, offset }),
            parent_folder_ids: vec![BaseFolderId::new_folder(folder)],
        };
        let Some(page) = e.call(op)?.into_iter().next() else { break };
        let got = page.root_folder.items.inner.len();
        for item in page.root_folder.items.inner {
            if let Some(id) = crate::ews_mail::message(item).and_then(|m| m.item_id) {
                out.push((id.id, id.change_key.unwrap_or_default()));
            }
        }
        if out.len() > MAX_ITEMS {
            return Err(Error::TooLarge { what: "the folder", limit: MAX_ITEMS });
        }
        if got == 0 || page.root_folder.includes_last_item_in_range != Some(false) {
            break;
        }
        let next = page.root_folder.indexed_paging_offset.unwrap_or(offset.saturating_add(got));
        if next <= offset {
            break;
        }
        offset = next;
    }
    Ok(out)
}

/// An item as the iCalendar or vCard text Exchange exports.
fn fetch<H: Http>(e: &mut Ews<H>, kind: Kind, id: &str) -> Result<String> {
    let op = GetItem {
        item_shape: ItemShape { base_shape: BaseShape::IdOnly, include_mime_content: Some(true), additional_properties: None },
        item_ids: vec![BaseItemId::ItemId { id: id.to_string(), change_key: None }],
    };
    let item = e.call(op)?.into_iter().next().and_then(|m| m.items.inner.into_iter().next()).and_then(crate::ews_mail::message);
    let mime = item.and_then(|m| m.mime_content).ok_or_else(|| Error::Protocol("Exchange sent the item without its content".into()))?;
    if mime.content.len() > MAX_ITEM_BYTES.saturating_mul(2) {
        return Err(Error::TooLarge { what: "the item", limit: MAX_ITEM_BYTES });
    }
    let raw = ibercorreo_mime::unbase64(&mime.content).ok_or_else(|| Error::Protocol("Exchange sent content that is not base64".into()))?;
    text_of(kind, &raw).ok_or_else(|| Error::Protocol(format!("Exchange sent the item without {}", kind.begin)))
}

/// The iCalendar or vCard text in `raw`: the text itself, or (a calendar item exported as a
/// message) its `text/calendar` part.
fn text_of(kind: Kind, raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    if let Some(start) = text.find(kind.begin)
        && let Some(end) = text.rfind(kind.end).filter(|e| *e >= start)
    {
        return text.get(start..end.saturating_add(kind.end.len())).map(str::to_string);
    }
    // Nothing for a card: a message holds no vCard part this reads.
    let m = ibercorreo_mime::parse(raw, ibercorreo_core::MessageId(0), ibercorreo_core::FolderId(0)).ok()?;
    m.content().and_then(|c| c.invite.clone())
}

/// An item's change key now; `None` when it is gone.
fn change_key<H: Http>(e: &mut Ews<H>, id: &str) -> Result<Option<String>> {
    let op = GetItem {
        item_shape: ItemShape { base_shape: BaseShape::IdOnly, include_mime_content: None, additional_properties: None },
        item_ids: vec![BaseItemId::ItemId { id: id.to_string(), change_key: None }],
    };
    match e.call(op) {
        Ok(m) => Ok(m
            .into_iter()
            .next()
            .and_then(|m| m.items.inner.into_iter().next())
            .and_then(crate::ews_mail::message)
            .and_then(|m| m.item_id)
            .and_then(|i| i.change_key)),
        Err(Error::Status { status: 404, .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Create an item of `kind` in `folder` from its text; its id.
fn create<H: Http>(e: &mut Ews<H>, kind: Kind, folder: &str, text: &str) -> Result<String> {
    if text.len() > MAX_ITEM_BYTES {
        return Err(Error::Invalid(format!("larger than {} MB", MAX_ITEM_BYTES / 1024 / 1024)));
    }
    let op = format!(
        r#"<CreateItem xmlns="{MESSAGES_NS}"{attr}><SavedItemFolderId><t:FolderId Id="{folder}"/></SavedItemFolderId><Items><t:{item}><t:MimeContent CharacterSet="UTF-8">{mime}</t:MimeContent></t:{item}></Items></CreateItem>"#,
        attr = kind.create_attr,
        folder = escape(folder),
        item = kind.item,
        mime = ibercorreo_mime::base64(text.as_bytes()),
    );
    let answer = e.call_raw(&op)?.into_iter().next().ok_or_else(|| Error::Protocol("Exchange did not answer the CreateItem".into()))??;
    let id = answer
        .child(MESSAGES_NS, "Items")
        .and_then(|items| items.children.first())
        .and_then(|item| item.child(TYPES_NS, "ItemId"))
        .and_then(|i| i.attr("Id"))
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::Protocol("Exchange created the item without saying its id".into()))?;
    Ok(id.to_string())
}

/// Delete an item (to Deleted Items, as Outlook does); one already gone counts as deleted.
fn delete<H: Http>(e: &mut Ews<H>, id: &str) -> Result<()> {
    let op = DeleteItem {
        delete_type: DeleteType::MoveToDeletedItems,
        send_meeting_cancellations: Some(SendMeetingCancellations::SendToNone),
        affected_task_occurrences: None,
        suppress_read_receipts: None,
        item_ids: vec![BaseItemId::ItemId { id: id.to_string(), change_key: None }],
    };
    match e.call(op) {
        Ok(_) | Err(Error::Status { status: 404, .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

/// What became of one change, in the terms both stores share.
enum Done {
    Put(String),
    Deleted,
    Conflict,
}

/// Write `text` as an item of `collection`, replacing the item at `href` last seen as `etag`.
fn put<H: Http>(e: &mut Ews<H>, kind: Kind, prefix: &str, collection: &str, href: &str, etag: Option<&str>, text: &str) -> Result<Done> {
    let folder = folder_id(prefix, collection).ok_or_else(|| Error::Invalid(format!("{collection}: not an Exchange folder")))?;
    let old = match etag {
        None => None,
        Some(etag) => {
            let id = item_id(collection, href).ok_or_else(|| Error::Invalid(format!("{href}: not inside its folder")))?;
            if change_key(e, &id)?.as_deref() != Some(etag) {
                return Ok(Done::Conflict);
            }
            Some(id)
        }
    };
    let new = create(e, kind, &folder, text)?;
    if let Some(old) = old {
        delete(e, &old)?;
    }
    Ok(Done::Put(format!("{collection}{}", segment(&new))))
}

/// Delete the item at `href` of `collection`, last seen as `etag`.
fn remove<H: Http>(e: &mut Ews<H>, collection: &str, href: &str, etag: &str) -> Result<Done> {
    let id = item_id(collection, href).ok_or_else(|| Error::Invalid(format!("{href}: not inside its folder")))?;
    match change_key(e, &id)? {
        None => Ok(Done::Deleted),
        Some(now) if now != etag => Ok(Done::Conflict),
        Some(_) => delete(e, &id).map(|()| Done::Deleted),
    }
}

/// One folder's items new or changed since `known` (or just written), and those gone.
fn folder_sync<H: Http>(
    e: &mut Ews<H>,
    kind: Kind,
    collection: &str,
    folder: &str,
    known: &BTreeMap<String, String>,
    errors: &mut Vec<String>,
) -> Result<(Vec<FetchedItem>, Vec<String>)> {
    let listed = list(e, folder)?;
    let mut seen = BTreeSet::new();
    let mut fetched = Vec::new();
    for (id, key) in listed {
        let href = format!("{collection}{}", segment(&id));
        seen.insert(href.clone());
        // Unchanged. What this sync wrote has a new id, and a conflict a new key: both come down.
        if known.get(&href).is_some_and(|k| *k == key) {
            continue;
        }
        match fetch(e, kind, &id) {
            Ok(data) => fetched.push(FetchedItem { href, etag: key, data }),
            Err(e) if fatal(&e) => return Err(e),
            // Gone since the list: it is missing from the next one.
            Err(Error::Status { status: 404, .. }) => {
                seen.remove(&href);
            }
            Err(err) => {
                // Kept as it is here; listed again next time.
                errors.push(format!("{href}: {err}"));
            }
        }
    }
    let removed = known.keys().filter(|h| !seen.contains(*h)).cloned().collect();
    Ok((fetched, removed))
}

/// Sync the account's Exchange calendars: send `state`'s changes (those of this server's
/// calendars), then read every calendar folder. Failing to reach or sign in to the server is an
/// error; a calendar that fails on its own is reported in [`calendar::Batch::errors`].
pub fn calendar<H: Http>(e: &mut Ews<H>, ews: &str, state: &calendar::State) -> Result<calendar::Batch> {
    let prefix = prefix(ews);
    let mut state = state.only(&|h| h.starts_with(&prefix));
    let mut batch = calendar::Batch { scope: Some(prefix.clone()), ..calendar::Batch::default() };
    for change in std::mem::take(&mut state.changes) {
        let (cal, href) = match &change {
            calendar::Change::Put { calendar, href, .. } | calendar::Change::Delete { calendar, href, .. } => (calendar.clone(), href.clone()),
        };
        let done = match &change {
            calendar::Change::Put { etag, body, .. } => put(e, CALENDAR, &prefix, &cal, &href, etag.as_deref(), body),
            calendar::Change::Delete { etag, .. } => remove(e, &cal, &href, etag),
        };
        let known: &mut Known = state.calendars.entry(cal.clone()).or_default();
        match (done, &change) {
            (Err(err), _) if fatal(&err) => return Err(err),
            (Err(err), _) => batch.errors.push(format!("{href}: {err}")),
            (Ok(Done::Put(new)), calendar::Change::Put { uid, .. }) => {
                known.items.remove(&href);
                batch.pushed.push(calendar::Pushed::Put { calendar: cal, href: new, uid: uid.clone() });
            }
            (Ok(Done::Deleted | Done::Put(_)), _) => {
                known.items.remove(&href);
                batch.pushed.push(calendar::Pushed::Deleted { href });
            }
            (Ok(Done::Conflict), _) => {
                batch.pushed.push(calendar::Pushed::Conflict { href });
            }
        }
    }
    for (id, name) in folders(e, CALENDAR)? {
        let href = format!("{prefix}{}/", segment(&id));
        let calendar = RemoteCalendar { href: href.clone(), name: name.clone(), color: None, ctag: None };
        let empty = BTreeMap::new();
        let known = state.calendars.get(&href).map_or(&empty, |k| &k.items);
        match folder_sync(e, CALENDAR, &href, &id, known, &mut batch.errors) {
            Ok((fetched, removed)) => batch.calendars.push(CalendarSync { calendar, unchanged: false, fetched, removed }),
            Err(err) if fatal(&err) => return Err(err),
            Err(err) => {
                batch.errors.push(format!("{name}: {err}"));
                batch.calendars.push(CalendarSync { calendar, unchanged: true, fetched: Vec::new(), removed: Vec::new() });
            }
        }
    }
    Ok(batch)
}

/// Sync the account's Exchange contacts, as [`calendar`] does the calendars.
pub fn contacts<H: Http>(e: &mut Ews<H>, ews: &str, state: &contacts::State) -> Result<contacts::Batch> {
    let prefix = prefix(ews);
    let mut state = state.only(&|h| h.starts_with(&prefix));
    let mut batch = contacts::Batch { scope: Some(prefix.clone()), ..contacts::Batch::default() };
    for change in std::mem::take(&mut state.changes) {
        let (book, href) = match &change {
            contacts::Change::Put { book, href, .. } | contacts::Change::Delete { book, href, .. } => (book.clone(), href.clone()),
        };
        let done = match &change {
            contacts::Change::Put { etag, body, .. } => put(e, CONTACTS, &prefix, &book, &href, etag.as_deref(), body),
            contacts::Change::Delete { etag, .. } => remove(e, &book, &href, etag),
        };
        let known: &mut Known = state.books.entry(book.clone()).or_default();
        match (done, &change) {
            (Err(err), _) if fatal(&err) => return Err(err),
            (Err(err), _) => batch.errors.push(format!("{href}: {err}")),
            (Ok(Done::Put(new)), contacts::Change::Put { contact, .. }) => {
                known.items.remove(&href);
                batch.pushed.push(contacts::Pushed::Put { href: new, contact: *contact });
            }
            (Ok(Done::Deleted | Done::Put(_)), _) => {
                known.items.remove(&href);
                batch.pushed.push(contacts::Pushed::Deleted { href });
            }
            (Ok(Done::Conflict), _) => {
                batch.pushed.push(contacts::Pushed::Conflict { href });
            }
        }
    }
    for (id, name) in folders(e, CONTACTS)? {
        let href = format!("{prefix}{}/", segment(&id));
        let book = RemoteBook { href: href.clone(), name: name.clone(), ctag: None };
        let empty = BTreeMap::new();
        let known = state.books.get(&href).map_or(&empty, |k| &k.items);
        match folder_sync(e, CONTACTS, &href, &id, known, &mut batch.errors) {
            Ok((fetched, removed)) => batch.books.push(BookSync { book, unchanged: false, fetched, removed }),
            Err(err) if fatal(&err) => return Err(err),
            Err(err) => {
                batch.errors.push(format!("{name}: {err}"));
                batch.books.push(BookSync { book, unchanged: true, fetched: Vec::new(), removed: Vec::new() });
            }
        }
    }
    Ok(batch)
}

#[cfg(test)]
mod tests;
