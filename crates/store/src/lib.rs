//! The local store, L2: every folder, message, calendar, event and contact the app holds, and the
//! JSON file they are saved to. Servers (IMAP, CalDAV, EWS) sync into this store from M1 on; the
//! interface only ever reads it.
//!
//! Saving writes a temporary file and renames it over the old one, so a crash mid-save never
//! leaves a half-written store. Messages go to shard files beside it ([`shards`]), and a save
//! rewrites only the shards that changed.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod rules;
pub mod search;
pub mod shards;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ibercorreo_core::calendar::When;
use ibercorreo_core::mail::Attachment;
use ibercorreo_core::{
    Account, AccountId, AddressBook, Calendar, CalendarRemote, Contact, Content, Date, DateTime, Error, Event, EventId, EventRemote, Folder,
    FolderId, FolderRole, Message, MessageId, RemoteRef, Result, Zone, clamp_text,
};
use serde::{Deserialize, Serialize};

/// Largest store file read back: 1 GiB.
pub const MAX_STORE_BYTES: u64 = 1024 * 1024 * 1024;
/// Format version written into the file; a newer file is refused rather than misread.
/// 2: folders and messages remember their server copy (`remote`), and deleted synced messages
/// leave a tombstone until the server has deleted them too.
/// 3: contacts hold several phones (`phones`), a UID, title, note, birthday and unread vCard lines,
/// and live in address books (`books`), which may mirror CardDAV servers.
/// 4: several accounts: accounts have an `id`, folders the `account` they belong to (files from
/// before give everything account 0).
/// 5: messages are in shard files beside the store file (`message_shards`, [`shards`]); files
/// from before hold them inline and are written back sharded.
/// 6: a message's content (body, formatted body, invitation) is in its shard's content file, not
/// with its headers, and read when it is shown ([`Store::content`]).
pub const FORMAT_VERSION: u32 = 6;
/// The most accounts a store holds.
pub const MAX_ACCOUNTS: usize = 50;

/// The folders every account starts with.
const STANDARD_FOLDERS: [(&str, FolderRole); 7] = [
    ("Inbox", FolderRole::Inbox),
    ("Drafts", FolderRole::Drafts),
    ("Sent", FolderRole::Sent),
    ("Outbox", FolderRole::Outbox),
    ("Archive", FolderRole::Archive),
    ("Junk", FolderRole::Junk),
    ("Trash", FolderRole::Trash),
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    pub accounts: Vec<Account>,
    folders: Vec<Folder>,
    /// Shared, so an undo snapshot copies pointers, not mail: only a message that changes is
    /// copied ([`Store::message_mut`]). Kept in id order, so a lookup is a binary search.
    messages: Vec<Arc<Message>>,
    calendars: Vec<Calendar>,
    events: Vec<Event>,
    contacts: Vec<Contact>,
    next_folder: u32,
    #[serde(default)]
    next_account: u32,
    next_message: u64,
    next_event: u64,
    next_contact: u64,
    /// Synced messages deleted here for good, still to be deleted on the server.
    #[serde(default)]
    tombstones: Vec<RemoteRef>,
    /// The zone the calendar is shown in, chosen by the person; `None` follows the computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// The interface language (a lowercase language code, `uk`), chosen by the person; `None`
    /// follows the computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Reminders dismissed or snoozed, so they do not come back after a restart.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reminder_marks: Vec<ReminderMark>,
    /// Synced events deleted here, still to be deleted on the server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    remote_deletions: Vec<RemoteDeletion>,
    /// Address books; book 0 is the local one every store starts with.
    #[serde(default)]
    books: Vec<AddressBook>,
    /// Synced contacts deleted here, still to be deleted on the server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    card_deletions: Vec<RemoteDeletion>,
    /// The folders shown under Favorites, in the order they were added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    favorites: Vec<FolderId>,
    /// Mail rules, in the order they run ([`rules`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rules: Vec<rules::Rule>,
    /// The full-text index ([`search`]), kept in memory only, and which message versions it must
    /// match. Neither counts in comparing two stores.
    #[serde(skip)]
    search_cache: search::SearchCache,
    #[serde(skip)]
    stamps: search::Stamps,
    /// Set in a written store file whose messages are in shard files; never set in memory.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    message_shards: bool,
    /// The shards the last save wrote, so the next one writes only what changed.
    #[serde(skip)]
    saved: shards::Saved,
    /// Messages the last load left out ([`Store::repaired_on_load`]).
    #[serde(skip)]
    repaired: Repaired,
}

/// How many messages a load left out, and whether it read a format from before content files;
/// not part of the store's value.
#[derive(Debug, Clone, Copy, Default)]
struct Repaired {
    left_out: usize,
    old_format: bool,
}

impl PartialEq for Repaired {
    fn eq(&self, _: &Repaired) -> bool {
        true
    }
}

impl Eq for Repaired {}

/// A CalDAV or CardDAV resource deleted here: its collection (`calendar`, also for an address
/// book), path and last known version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDeletion {
    pub calendar: String,
    pub href: String,
    pub etag: String,
}

/// A reminder the person dealt with: the occurrence (event UID and start in UTC), and until when
/// it is snoozed (`None`: dismissed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReminderMark {
    uid: String,
    start: DateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snoozed_until: Option<DateTime>,
}

/// A reminder that is due: the occurrence as seen in the zone asked for, when it starts and when
/// the reminder fired (both UTC).
#[derive(Debug, Clone, Copy)]
pub struct DueReminder<'a> {
    pub event: &'a Event,
    pub when: When,
    pub start_utc: DateTime,
    pub fire_utc: DateTime,
}

/// How long a missed reminder stays up after its appointment started.
pub const OVERDUE_DAYS: i64 = 2;
/// Most reminders shown at once, and most marks kept.
pub const MAX_DUE: usize = 100;
const MAX_MARKS: usize = 5_000;

/// One occurrence of an event: the event, and when this occurrence happens (for a repeating
/// event, not the series' first date) as seen in the zone it was asked for.
#[derive(Debug, Clone, Copy)]
pub struct Occurrence<'a> {
    pub event: &'a Event,
    pub when: When,
    /// The same occurrence in the event's own times; its start date is the one `EXDATE` and
    /// `RECURRENCE-ID` name.
    pub own: When,
}

/// What a `.vcf` import did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VcfReport {
    pub added: usize,
    pub updated: usize,
    pub skipped_properties: usize,
    pub rejected: Vec<String>,
}

/// What an `.ics` import did, for the report shown to the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub added: usize,
    pub updated: usize,
    pub skipped_properties: usize,
    pub rejected: Vec<String>,
}

impl Store {
    /// An empty store with the standard folders and one calendar.
    pub fn new() -> Store {
        let mut s = Store { version: FORMAT_VERSION, ..Store::default() };
        s.add_standard_folders(AccountId(0));
        s.calendars.push(Calendar { id: 0, name: "Calendar".into(), color: "#1F7A8C".into(), remote: None });
        s.books.push(AddressBook { id: 0, name: "Contacts".into(), remote: None });
        s
    }

    // ---- accounts ----

    pub fn account(&self, id: AccountId) -> Result<&Account> {
        self.accounts.iter().find(|a| a.id == id).ok_or_else(|| Error::not_found("account", id.0.to_string()))
    }

    pub fn account_mut(&mut self, id: AccountId) -> Result<&mut Account> {
        self.accounts.iter_mut().find(|a| a.id == id).ok_or_else(|| Error::not_found("account", id.0.to_string()))
    }

    /// The account whose mail `folder` holds.
    pub fn account_of(&self, folder: FolderId) -> Result<AccountId> {
        Ok(self.folder(folder)?.account)
    }

    /// Add an account (its `id` is replaced) with the standard folders, and return its id. The
    /// first account takes over the folders a store starts with, and the mail already in them.
    pub fn add_account(&mut self, mut account: Account) -> Result<AccountId> {
        if self.accounts.len() >= MAX_ACCOUNTS {
            return Err(Error::invalid("account", format!("at most {MAX_ACCOUNTS} accounts")));
        }
        let id = if self.accounts.is_empty() && self.folders.iter().all(|f| f.account == AccountId(0)) {
            AccountId(0)
        } else {
            let next = self.accounts.iter().map(|a| a.id.0.saturating_add(1)).chain(self.folders.iter().map(|f| f.account.0.saturating_add(1))).max();
            AccountId(next.unwrap_or(0).max(self.next_account))
        };
        self.next_account = id.0.saturating_add(1).max(self.next_account);
        account.id = id;
        self.accounts.push(account);
        self.add_standard_folders(id);
        Ok(id)
    }

    /// Remove an account with its folders and the mail in them, returning it. The last one gone,
    /// the store keeps empty standard folders, as a new store has.
    pub fn remove_account(&mut self, id: AccountId) -> Result<Account> {
        let pos = self.accounts.iter().position(|a| a.id == id).ok_or_else(|| Error::not_found("account", id.0.to_string()))?;
        let account = self.accounts.remove(pos);
        let gone: std::collections::HashSet<FolderId> = self.folders.iter().filter(|f| f.account == id).map(|f| f.id).collect();
        self.messages.retain(|m| !gone.contains(&m.folder));
        self.tombstones.retain(|t| !gone.contains(&t.folder));
        self.folders.retain(|f| f.account != id);
        self.favorites.retain(|f| !gone.contains(f));
        self.rules.retain(|r| r.account != Some(id));
        if self.folders.is_empty() {
            self.add_standard_folders(AccountId(0));
        }
        Ok(account)
    }

    /// The account moved to another server (or another person's mailbox): what was synced from
    /// the old one goes, with the folders only the server had. Mail that only exists here stays.
    /// Returns how many messages went.
    pub fn forget_server_mail(&mut self, id: AccountId) -> usize {
        let synced: std::collections::HashSet<FolderId> =
            self.folders.iter().filter(|f| f.account == id && f.remote.is_some()).map(|f| f.id).collect();
        let before = self.messages.len();
        self.messages.retain(|m| !(m.remote.is_some_and(|r| synced.contains(&r.folder)) || (m.remote.is_some() && synced.contains(&m.folder))));
        let dropped = before.saturating_sub(self.messages.len());
        self.tombstones.retain(|t| !synced.contains(&t.folder));
        let held: std::collections::HashSet<FolderId> = self.messages.iter().map(|m| m.folder).collect();
        self.folders.retain(|f| !(synced.contains(&f.id) && f.role == FolderRole::Other && !held.contains(&f.id)));
        let folders = &self.folders;
        self.favorites.retain(|id| folders.iter().any(|f| f.id == *id));
        for f in self.folders.iter_mut().filter(|f| synced.contains(&f.id)) {
            f.remote = None;
        }
        self.add_standard_folders(id);
        dropped
    }

    /// Give `account` each standard folder it lacks.
    fn add_standard_folders(&mut self, account: AccountId) {
        for (name, role) in STANDARD_FOLDERS {
            if !self.folders.iter().any(|f| f.account == account && f.role == role) {
                self.add_folder(account, name, role);
            }
        }
    }

    // ---- folders ----

    pub fn add_folder(&mut self, account: AccountId, name: &str, role: FolderRole) -> FolderId {
        let id = FolderId(self.next_folder);
        self.next_folder = self.next_folder.saturating_add(1);
        self.folders.push(Folder { id, account, name: name.to_string(), role, remote: None });
        id
    }

    /// The favorite folders, in the order they were added.
    pub fn favorites(&self) -> &[FolderId] {
        &self.favorites
    }

    pub fn is_favorite(&self, id: FolderId) -> bool {
        self.favorites.contains(&id)
    }

    /// Show a folder under Favorites (at the end) or stop showing it there. Returns whether that
    /// changed anything.
    pub fn set_favorite(&mut self, id: FolderId, favorite: bool) -> Result<bool> {
        self.folder(id)?;
        let was = self.is_favorite(id);
        if favorite && !was {
            self.favorites.push(id);
        } else if !favorite && was {
            self.favorites.retain(|f| *f != id);
        }
        Ok(was != favorite)
    }

    pub fn folder_mut(&mut self, id: FolderId) -> Result<&mut Folder> {
        self.folders.iter_mut().find(|f| f.id == id).ok_or_else(|| Error::not_found("folder", id.0.to_string()))
    }

    /// The folder of `account` that mirrors server mailbox `path`.
    pub fn folder_by_path(&self, account: AccountId, path: &str) -> Option<FolderId> {
        self.folders.iter().find(|f| f.account == account && f.remote.as_ref().is_some_and(|r| r.path == path)).map(|f| f.id)
    }

    /// Folders in the order the folder pane shows them: account by account (in the order they
    /// were added), Inbox first.
    pub fn folders(&self) -> Vec<&Folder> {
        let rank = |a: AccountId| self.accounts.iter().position(|x| x.id == a).unwrap_or(0);
        let mut v: Vec<&Folder> = self.folders.iter().collect();
        v.sort_by_key(|f| (rank(f.account), f.role.order(), f.name.to_lowercase()));
        v
    }

    /// The folders of one account, in folder pane order.
    pub fn folders_of(&self, account: AccountId) -> Vec<&Folder> {
        self.folders().into_iter().filter(|f| f.account == account).collect()
    }

    pub fn folder(&self, id: FolderId) -> Result<&Folder> {
        self.folders.iter().find(|f| f.id == id).ok_or_else(|| Error::not_found("folder", id.0.to_string()))
    }

    /// `account`'s folder with `role` (its Inbox, its Sent…).
    pub fn folder_by_role(&self, account: AccountId, role: FolderRole) -> Result<FolderId> {
        self.folders
            .iter()
            .find(|f| f.account == account && f.role == role)
            .map(|f| f.id)
            .ok_or_else(|| Error::not_found("folder", format!("{role:?}")))
    }

    /// The account shown first: the first one added, or the folders of a store with none.
    pub fn first_account(&self) -> AccountId {
        self.accounts.first().map(|a| a.id).or_else(|| self.folders.first().map(|f| f.account)).unwrap_or_default()
    }

    pub fn unread_count(&self, folder: FolderId) -> usize {
        self.messages.iter().filter(|m| m.folder == folder && !m.seen).count()
    }

    /// Unread messages per folder, in one pass over the mail (folders with none are left out).
    pub fn unread_counts(&self) -> HashMap<FolderId, usize> {
        let mut counts = HashMap::new();
        for m in self.messages.iter().filter(|m| !m.seen) {
            let n: &mut usize = counts.entry(m.folder).or_default();
            *n = n.saturating_add(1);
        }
        counts
    }

    // ---- messages ----

    pub fn next_message_id(&mut self) -> MessageId {
        let id = MessageId(self.next_message);
        self.next_message = self.next_message.saturating_add(1);
        id
    }

    /// Add a message; its id is replaced with a fresh one and returned.
    pub fn add_message(&mut self, mut m: Message) -> Result<MessageId> {
        self.folder(m.folder)?;
        // Past every id held, so pushing keeps the id order even after a hand-edited file.
        if let Some(last) = self.messages.last() {
            self.next_message = self.next_message.max(last.id.0.saturating_add(1));
        }
        m.id = self.next_message_id();
        let id = m.id;
        self.stamps.touch(id);
        self.messages.push(Arc::new(m));
        Ok(id)
    }

    /// Parse a raw `.eml` into `folder`.
    pub fn import_eml(&mut self, raw: &[u8], folder: FolderId) -> Result<MessageId> {
        self.folder(folder)?;
        let m = ibercorreo_mime::parse(raw, MessageId(0), folder)?;
        self.add_message(m)
    }

    /// Every message, in no particular order (for sync).
    pub fn all_messages(&self) -> impl ExactSizeIterator<Item = &Message> + Clone {
        self.messages.iter().map(|m| &**m)
    }

    /// Where message `id` is in `messages`: a binary search, as they are kept in id order, with
    /// a scan as the fallback should that order ever be broken.
    fn position(&self, id: MessageId) -> Option<usize> {
        match self.messages.binary_search_by_key(&id, |m| m.id) {
            Ok(pos) => Some(pos),
            Err(_) => self.messages.iter().position(|m| m.id == id),
        }
    }

    /// The message whose server copy is `uid` in `folder`'s mailbox.
    pub fn find_remote(&self, folder: FolderId, uid: u32) -> Option<MessageId> {
        self.messages.iter().find(|m| m.remote.is_some_and(|r| r.folder == folder && r.uid == uid)).map(|m| m.id)
    }

    /// Synced messages deleted here and not yet on the server.
    pub fn tombstones(&self) -> &[RemoteRef] {
        &self.tombstones
    }

    /// The server deleted the message too (or never had it): stop remembering it.
    pub fn forget_tombstone(&mut self, folder: FolderId, uid: u32) {
        self.tombstones.retain(|t| !(t.folder == folder && t.uid == uid));
    }

    /// Remove a message the server already removed: no tombstone is left.
    pub fn drop_message(&mut self, id: MessageId) -> Result<Message> {
        let pos = self.position(id).ok_or_else(|| Error::not_found("message", id.0.to_string()))?;
        self.stamps.forget(id);
        Ok(Arc::unwrap_or_clone(self.messages.remove(pos)))
    }

    /// Messages in `folder`, newest first (undated last).
    pub fn messages_in(&self, folder: FolderId) -> Vec<&Message> {
        let mut v: Vec<&Message> = self.messages.iter().filter(|m| m.folder == folder).map(|m| &**m).collect();
        v.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
        v
    }

    /// A job that reads every message into the full-text index, to run on another thread when
    /// the store opens or mail arrives: the first search of a big mailbox then has nothing to
    /// read. The index is shared with this store and its copies.
    pub fn search_warmup(&self) -> impl FnOnce() + Send + 'static {
        let (cache, stamps, messages, saved) = (self.search_cache.clone(), self.stamps.clone(), self.messages.clone(), self.saved.clone());
        move || search::warm(&cache, &stamps, &messages, &mut saved.reader())
    }

    /// Messages in every folder that match `query` ([`search`] has the syntax), newest first.
    pub fn search(&self, query: &str) -> Vec<&Message> {
        self.search_in(query, search::Scope::All)
    }

    /// Messages in `scope` that match `query` ([`search`] has the syntax), newest first. A blank
    /// query matches every message in scope; one with nothing to look for (only punctuation)
    /// matches none.
    pub fn search_in(&self, query: &str, scope: search::Scope) -> Vec<&Message> {
        let query = search::Query::parse(query);
        let allowed = |folder: FolderId| match scope {
            search::Scope::All => true,
            search::Scope::Folder(f) => folder == f,
            search::Scope::Account(a) => self.folder(folder).is_ok_and(|f| f.account == a),
        };
        let mut v = search::run(&self.search_cache, &self.stamps, &self.messages, &mut self.saved.reader(), &allowed, &query);
        v.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
        v
    }

    pub fn message(&self, id: MessageId) -> Result<&Message> {
        self.position(id).and_then(|pos| self.messages.get(pos)).map(|m| &**m).ok_or_else(|| Error::not_found("message", id.0.to_string()))
    }

    /// What message `id` says: its own content, or the one kept on disk for it, read now.
    pub fn content(&self, id: MessageId) -> Result<Arc<Content>> {
        let m = self.message(id)?;
        if let Some(c) = m.shared_content() {
            return Ok(Arc::clone(c));
        }
        self.saved.content(id)?.ok_or_else(|| Error::not_found("message text", id.0.to_string()))
    }

    /// The content of message `id` to change (a draft being written), read from disk first if
    /// it is kept there.
    pub fn content_mut(&mut self, id: MessageId) -> Result<&mut Content> {
        let content = match self.message(id)?.shared_content() {
            Some(_) => None,
            None => Some(self.content(id)?),
        };
        let m = self.message_mut(id)?;
        if let Some(c) = content {
            m.set_content(c);
        }
        m.content_mut().ok_or_else(|| Error::not_found("message text", id.0.to_string()))
    }

    /// A message to change; the search index reads it again.
    pub fn message_mut(&mut self, id: MessageId) -> Result<&mut Message> {
        let pos = self.position(id).ok_or_else(|| Error::not_found("message", id.0.to_string()))?;
        self.stamps.touch(id);
        self.messages.get_mut(pos).map(Arc::make_mut).ok_or_else(|| Error::not_found("message", id.0.to_string()))
    }

    pub fn move_message(&mut self, id: MessageId, to: FolderId) -> Result<FolderId> {
        self.folder(to)?;
        let m = self.message_mut(id)?;
        let from = m.folder;
        m.folder = to;
        Ok(from)
    }

    /// Remove a message for good, returning it (for undo). A synced message leaves a tombstone so
    /// the next sync deletes it on the server too.
    pub fn remove_message(&mut self, id: MessageId) -> Result<Message> {
        let m = self.drop_message(id)?;
        if let Some(r) = m.remote {
            self.tombstones.push(r);
        }
        Ok(m)
    }

    /// Put back a message removed by [`Store::remove_message`], keeping its id.
    pub fn restore_message(&mut self, m: Message) -> Result<()> {
        self.folder(m.folder)?;
        if self.position(m.id).is_some() {
            return Err(Error::invalid("message", format!("id {} already present", m.id.0)));
        }
        self.next_message = self.next_message.max(m.id.0.saturating_add(1));
        if let Some(r) = m.remote {
            self.forget_tombstone(r.folder, r.uid);
        }
        self.stamps.touch(m.id);
        let at = self.messages.partition_point(|x| x.id < m.id);
        self.messages.insert(at, Arc::new(m));
        Ok(())
    }

    // ---- calendar ----

    pub fn calendars(&self) -> &[Calendar] {
        &self.calendars
    }

    pub fn calendar(&self, id: u32) -> Result<&Calendar> {
        self.calendars.iter().find(|c| c.id == id).ok_or_else(|| Error::not_found("calendar", id.to_string()))
    }

    /// Most calendars kept.
    pub const MAX_CALENDARS: usize = 500;

    /// Add a calendar; its id is one past the highest in use.
    pub fn add_calendar(&mut self, name: &str, color: &str, remote: Option<CalendarRemote>) -> Result<u32> {
        if self.calendars.len() >= Self::MAX_CALENDARS {
            return Err(Error::invalid("calendar", format!("at most {} calendars", Self::MAX_CALENDARS)));
        }
        let id = self.calendars.iter().map(|c| c.id).max().map_or(0, |m| m.saturating_add(1));
        if self.calendars.iter().any(|c| c.id == id) {
            return Err(Error::invalid("calendar", "no calendar id left"));
        }
        self.calendars.push(Calendar { id, name: clamp_text(name, 200).to_string(), color: color.to_string(), remote });
        Ok(id)
    }

    pub fn calendar_mut(&mut self, id: u32) -> Result<&mut Calendar> {
        self.calendars.iter_mut().find(|c| c.id == id).ok_or_else(|| Error::not_found("calendar", id.to_string()))
    }

    /// The calendar mirroring server collection `href`.
    pub fn calendar_by_href(&self, href: &str) -> Option<u32> {
        self.calendars.iter().find(|c| c.remote.as_ref().is_some_and(|r| r.href == href)).map(|c| c.id)
    }

    /// Remove a calendar and its events. The last calendar stays: events always need one.
    pub fn remove_calendar(&mut self, id: u32) -> Result<Calendar> {
        let pos = self.calendars.iter().position(|c| c.id == id).ok_or_else(|| Error::not_found("calendar", id.to_string()))?;
        if self.calendars.len() <= 1 {
            return Err(Error::invalid("calendar", "the last calendar cannot be removed"));
        }
        self.events.retain(|e| e.calendar != id);
        let removed = self.calendars.remove(pos);
        if let Some(r) = &removed.remote {
            self.remote_deletions.retain(|d| d.calendar != r.href);
        }
        Ok(removed)
    }

    /// Replace the events that came from server resource `href` in `calendar` with `events`
    /// (a series and its changed occurrences). An event that was already here keeps its id, so a
    /// selection survives a sync. Returns whether the resource was new.
    pub fn replace_remote_events(&mut self, calendar: u32, href: &str, events: Vec<Event>) -> Result<bool> {
        self.calendar(calendar)?;
        let from_href = |e: &Event| e.calendar == calendar && e.remote.as_ref().is_some_and(|r| r.href == href);
        let old: Vec<(String, Option<Date>, EventId)> =
            self.events.iter().filter(|e| from_href(e)).map(|e| (e.uid.clone(), e.recurrence_id, e.id)).collect();
        self.events.retain(|e| !from_href(e));
        for mut e in events {
            e.calendar = calendar;
            match old.iter().find(|(uid, rid, _)| *uid == e.uid && *rid == e.recurrence_id) {
                Some((_, _, id)) => {
                    e.id = *id;
                    self.events.push(e);
                }
                None => {
                    self.add_event(e)?;
                }
            }
        }
        Ok(old.is_empty())
    }

    /// Record that the events of `uid` in `calendar` were written to server resource `href`: new
    /// ones are linked to it, changed ones are no longer marked. The ETag is left empty, so the
    /// server's copy is fetched again.
    pub fn mark_remote_sent(&mut self, calendar: u32, uid: &str, href: &str) {
        for e in self.events.iter_mut().filter(|e| e.calendar == calendar) {
            match &mut e.remote {
                Some(r) if r.href == href => {
                    r.dirty = false;
                    r.etag.clear();
                }
                None if e.uid == uid => e.remote = Some(EventRemote::new(href, "", "")),
                _ => {}
            }
        }
    }

    /// Remove the events that came from server resource `href` in `calendar`; how many.
    pub fn remove_remote_events(&mut self, calendar: u32, href: &str) -> usize {
        let before = self.events.len();
        self.events.retain(|e| !(e.calendar == calendar && e.remote.as_ref().is_some_and(|r| r.href == href)));
        before.saturating_sub(self.events.len())
    }

    pub fn add_event(&mut self, mut e: Event) -> Result<EventId> {
        self.calendar(e.calendar)?;
        e.id = EventId(self.next_event);
        self.next_event = self.next_event.saturating_add(1);
        let id = e.id;
        self.events.push(e);
        Ok(id)
    }

    pub fn event(&self, id: EventId) -> Result<&Event> {
        self.events.iter().find(|e| e.id == id).ok_or_else(|| Error::not_found("event", id.0.to_string()))
    }

    /// The event, to change it. An event synced from a server is marked to be sent back.
    pub fn event_mut(&mut self, id: EventId) -> Result<&mut Event> {
        let e = self.events.iter_mut().find(|e| e.id == id).ok_or_else(|| Error::invalid("event", format!("no event {}", id.0)))?;
        if let Some(r) = &mut e.remote {
            r.dirty = true;
        }
        Ok(e)
    }

    /// Remove an event. One synced from a server is deleted there at the next sync; when other
    /// events share its resource (a series and its changed occurrences), the resource is sent
    /// again without it instead.
    pub fn remove_event(&mut self, id: EventId) -> Result<Event> {
        let pos = self.events.iter().position(|e| e.id == id).ok_or_else(|| Error::not_found("event", id.0.to_string()))?;
        let e = self.events.remove(pos);
        if let Some(r) = &e.remote {
            let mut shared = false;
            for other in self.events.iter_mut().filter(|o| o.calendar == e.calendar) {
                if let Some(or) = other.remote.as_mut().filter(|or| or.href == r.href) {
                    or.dirty = true;
                    shared = true;
                }
            }
            if !shared && let Some(href) = self.calendar(e.calendar).ok().and_then(|c| c.remote.as_ref()).map(|c| c.href.clone()) {
                if self.remote_deletions.len() >= MAX_MARKS {
                    self.remote_deletions.remove(0);
                }
                self.remote_deletions.push(RemoteDeletion { calendar: href, href: r.href.clone(), etag: r.etag.clone() });
            }
        }
        Ok(e)
    }

    /// Server resources deleted here, still to be deleted on the server.
    pub fn remote_deletions(&self) -> &[RemoteDeletion] {
        &self.remote_deletions
    }

    /// Forget a pending server deletion (done, or the server's copy won).
    pub fn forget_remote_deletion(&mut self, href: &str) {
        self.remote_deletions.retain(|d| d.href != href);
    }

    pub fn restore_event(&mut self, e: Event) -> Result<()> {
        self.calendar(e.calendar)?;
        if let Some(r) = &e.remote {
            let href = r.href.clone();
            self.forget_remote_deletion(&href);
        }
        if self.events.iter().any(|x| x.id == e.id) {
            return Err(Error::invalid("event", format!("id {} already present", e.id.0)));
        }
        self.next_event = self.next_event.max(e.id.0.saturating_add(1));
        self.events.push(e);
        Ok(())
    }

    /// Occurrences on `day` as seen in zone `view`, all-day first, then by start time. A
    /// repeating event's occurrence that was changed on its own (`RECURRENCE-ID`) shows as the
    /// changed copy instead.
    pub fn events_on(&self, day: Date, view: &Zone) -> Vec<Occurrence<'_>> {
        let mut v: Vec<Occurrence<'_>> = self
            .events
            .iter()
            .flat_map(|e| e.occurrences_in(day, day, view).into_iter().map(move |(own, when)| Occurrence { event: e, when, own }))
            .filter(|o| o.event.rrule.is_none() || o.event.recurrence_id.is_some() || !self.overridden(o.event, o.own.start_date()))
            .collect();
        v.sort_by_key(|o| {
            let minute = match o.when {
                When::Timed { start, .. } if start.date == day => start.minute_of_day(),
                _ => 0,
            };
            (!matches!(o.when, When::AllDay { .. }), minute, o.event.id)
        });
        v
    }

    /// Reminders due at `now` (UTC), as seen in `view`, soonest appointment first: fired, not
    /// dismissed, not snoozed, and for an appointment that started at most [`OVERDUE_DAYS`] ago.
    pub fn reminders_due(&self, now: DateTime, view: &Zone) -> Vec<DueReminder<'_>> {
        let today = view.from_utc(now).date;
        let from = today.add_days(-OVERDUE_DAYS - 1).unwrap_or(today);
        // A reminder fires at most four weeks ahead.
        let to = today.add_days(29).unwrap_or(today);
        let oldest = now.to_unix().saturating_sub(OVERDUE_DAYS * 86_400);
        let mut due: Vec<DueReminder<'_>> = Vec::new();
        for e in self.events.iter().filter(|e| !e.reminders.is_empty()) {
            let own_zone = e.zone();
            let lead = e.reminders.iter().copied().max().unwrap_or(0);
            for (own, when) in e.occurrences_in(from, to, view) {
                if e.rrule.is_some() && e.recurrence_id.is_none() && self.overridden(e, own.start_date()) {
                    continue;
                }
                let start_utc = own.start_utc(own_zone.as_ref(), view);
                let fire = start_utc.to_unix().saturating_sub(i64::from(lead) * 60);
                if fire > now.to_unix() || start_utc.to_unix() < oldest {
                    continue;
                }
                let mark = self.reminder_marks.iter().find(|m| m.uid == e.uid && m.start == start_utc);
                if mark.is_some_and(|m| m.snoozed_until.is_none_or(|until| until > now)) {
                    continue;
                }
                let Ok(fire_utc) = DateTime::from_unix(fire) else { continue };
                due.push(DueReminder { event: e, when, start_utc, fire_utc });
            }
        }
        due.sort_by_key(|d| (d.start_utc, d.event.id));
        due.truncate(MAX_DUE);
        due
    }

    /// Dismiss (`snooze_until` `None`) or snooze the reminder for the occurrence of `uid`
    /// starting at `start` (UTC). Marks for appointments long past are dropped on the way.
    pub fn mark_reminder(&mut self, uid: &str, start: DateTime, snooze_until: Option<DateTime>, now: DateTime) {
        let keep_from = now.to_unix().saturating_sub((OVERDUE_DAYS + 1) * 86_400);
        self.reminder_marks.retain(|m| m.start.to_unix() >= keep_from && !(m.uid == uid && m.start == start));
        if self.reminder_marks.len() >= MAX_MARKS {
            self.reminder_marks.remove(0);
        }
        self.reminder_marks.push(ReminderMark { uid: uid.to_string(), start, snoozed_until: snooze_until });
    }

    /// Whether the series `e` has a changed copy of its occurrence on `day`.
    fn overridden(&self, e: &Event, day: Date) -> bool {
        self.events.iter().any(|x| x.uid == e.uid && x.calendar == e.calendar && x.recurrence_id == Some(day))
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Import an `.ics` file into `calendar`. An event whose UID is already there is updated in
    /// place; new ones are added.
    pub fn import_ics(&mut self, text: &str, calendar: u32) -> Result<ImportReport> {
        self.calendar(calendar)?;
        let parsed = ibercorreo_ical::parse(text)?;
        let mut report = ImportReport { added: 0, updated: 0, skipped_properties: parsed.skipped, rejected: parsed.rejected };
        for ev in parsed.events {
            // The series and each changed occurrence share a UID; RECURRENCE-ID tells them apart.
            if let Some(existing) = self.events.iter_mut().find(|e| e.uid == ev.uid && e.calendar == calendar && e.recurrence_id == ev.recurrence_id)
            {
                let id = existing.id;
                // Still the same server resource, now changed here.
                let remote = existing.remote.take().map(|r| EventRemote { dirty: true, ..r });
                *existing = ev.into_event(id, calendar);
                existing.remote = remote;
                report.updated += 1;
            } else {
                self.add_event(ev.into_event(EventId(0), calendar))?;
                report.added += 1;
            }
        }
        Ok(report)
    }

    pub fn export_ics(&self, calendar: Option<u32>) -> String {
        ibercorreo_ical::write(self.events.iter().filter(|e| calendar.is_none_or(|c| e.calendar == c)))
    }

    // ---- contacts ----

    pub fn books(&self) -> &[AddressBook] {
        &self.books
    }

    pub fn book(&self, id: u32) -> Result<&AddressBook> {
        self.books.iter().find(|b| b.id == id).ok_or_else(|| Error::not_found("address book", id.to_string()))
    }

    pub fn book_mut(&mut self, id: u32) -> Result<&mut AddressBook> {
        self.books.iter_mut().find(|b| b.id == id).ok_or_else(|| Error::not_found("address book", id.to_string()))
    }

    /// Most address books kept.
    pub const MAX_BOOKS: usize = 100;

    /// Add an address book; its id is one past the highest in use.
    pub fn add_book(&mut self, name: &str, remote: Option<CalendarRemote>) -> Result<u32> {
        if self.books.len() >= Self::MAX_BOOKS {
            return Err(Error::invalid("address book", format!("at most {} address books", Self::MAX_BOOKS)));
        }
        let id = self.books.iter().map(|b| b.id).max().map_or(0, |m| m.saturating_add(1));
        if self.books.iter().any(|b| b.id == id) {
            return Err(Error::invalid("address book", "no address book id left"));
        }
        self.books.push(AddressBook { id, name: clamp_text(name, 200).to_string(), remote });
        Ok(id)
    }

    /// The address book mirroring server collection `href`.
    pub fn book_by_href(&self, href: &str) -> Option<u32> {
        self.books.iter().find(|b| b.remote.as_ref().is_some_and(|r| r.href == href)).map(|b| b.id)
    }

    /// Remove an address book and its contacts. The last one stays: contacts always need one.
    pub fn remove_book(&mut self, id: u32) -> Result<AddressBook> {
        let pos = self.books.iter().position(|b| b.id == id).ok_or_else(|| Error::not_found("address book", id.to_string()))?;
        if self.books.len() <= 1 {
            return Err(Error::invalid("address book", "the last address book cannot be removed"));
        }
        self.contacts.retain(|c| c.book != id);
        let removed = self.books.remove(pos);
        if let Some(r) = &removed.remote {
            self.card_deletions.retain(|d| d.calendar != r.href);
        }
        Ok(removed)
    }

    /// Add a contact. It goes to book 0 if its book does not exist; a contact without a UID gets
    /// one, so it can be written to a server.
    pub fn add_contact(&mut self, mut c: Contact) -> u64 {
        c.id = self.next_contact;
        self.next_contact = self.next_contact.saturating_add(1);
        if self.book(c.book).is_err() {
            c.book = self.books.first().map_or(0, |b| b.id);
        }
        if c.uid.is_empty() {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (&c.name, c.emails.first().map(|a| a.email.as_str()), c.id).hash(&mut h);
            c.uid = format!("ibercorreo-{}-{:016x}", c.id, h.finish());
        }
        let id = c.id;
        self.contacts.push(c);
        id
    }

    /// Most contacts kept.
    pub const MAX_CONTACTS: usize = 100_000;

    pub fn contact_count(&self) -> usize {
        self.contacts.len()
    }

    pub fn all_contacts(&self) -> &[Contact] {
        &self.contacts
    }

    pub fn contact(&self, id: u64) -> Result<&Contact> {
        self.contacts.iter().find(|c| c.id == id).ok_or_else(|| Error::not_found("contact", id.to_string()))
    }

    /// The contact, to change it. One synced from a server is marked to be sent back.
    pub fn contact_mut(&mut self, id: u64) -> Result<&mut Contact> {
        let c = self.contacts.iter_mut().find(|c| c.id == id).ok_or_else(|| Error::not_found("contact", id.to_string()))?;
        if let Some(r) = &mut c.remote {
            r.dirty = true;
        }
        Ok(c)
    }

    /// Remove a contact; one synced from a server is deleted there at the next sync.
    pub fn remove_contact(&mut self, id: u64) -> Result<Contact> {
        let pos = self.contacts.iter().position(|c| c.id == id).ok_or_else(|| Error::not_found("contact", id.to_string()))?;
        let c = self.contacts.remove(pos);
        if let (Some(r), Some(book)) = (&c.remote, self.book(c.book).ok().and_then(|b| b.remote.as_ref()).map(|b| b.href.clone())) {
            if self.card_deletions.len() >= MAX_MARKS {
                self.card_deletions.remove(0);
            }
            self.card_deletions.push(RemoteDeletion { calendar: book, href: r.href.clone(), etag: r.etag.clone() });
        }
        Ok(c)
    }

    /// Move a contact to another address book. A card synced from a server is deleted there and
    /// written to the new book's server (if it has one) as a new card.
    pub fn move_contact(&mut self, id: u64, book: u32) -> Result<()> {
        self.book(book)?;
        let c = self.contact(id)?;
        if c.book == book {
            return Ok(());
        }
        let old_book = self.book(c.book).ok().and_then(|b| b.remote.as_ref()).map(|r| r.href.clone());
        let deletion = c.remote.as_ref().zip(old_book).map(|(r, book)| RemoteDeletion { calendar: book, href: r.href.clone(), etag: r.etag.clone() });
        if let Some(d) = deletion {
            if self.card_deletions.len() >= MAX_MARKS {
                self.card_deletions.remove(0);
            }
            self.card_deletions.push(d);
        }
        if let Some(c) = self.contacts.iter_mut().find(|c| c.id == id) {
            c.book = book;
            c.remote = None;
        }
        Ok(())
    }

    /// Server cards deleted here, still to be deleted on the server.
    pub fn card_deletions(&self) -> &[RemoteDeletion] {
        &self.card_deletions
    }

    pub fn forget_card_deletion(&mut self, href: &str) {
        self.card_deletions.retain(|d| d.href != href);
    }

    /// Put `c` in `book` as the server's card `href`: the contact already there keeps its id
    /// (so a selection survives a sync). Returns whether it was new.
    pub fn replace_remote_card(&mut self, book: u32, href: &str, mut c: Contact) -> Result<bool> {
        self.book(book)?;
        c.book = book;
        let existing = self.contacts.iter_mut().find(|x| x.book == book && x.remote.as_ref().is_some_and(|r| r.href == href));
        match existing {
            Some(x) => {
                c.id = x.id;
                *x = c;
                Ok(false)
            }
            None => {
                if self.contacts.len() >= Self::MAX_CONTACTS {
                    return Err(Error::TooLarge { what: "contacts", size: self.contacts.len() + 1, limit: Self::MAX_CONTACTS });
                }
                self.add_contact(c);
                Ok(true)
            }
        }
    }

    /// Record that contact `id` was written to server card `href`. The ETag is left empty, so
    /// the server's copy is fetched again.
    pub fn mark_card_sent(&mut self, id: u64, href: &str) {
        if let Some(c) = self.contacts.iter_mut().find(|c| c.id == id) {
            c.remote = Some(EventRemote::new(href, "", ""));
        }
    }

    /// Remove the contact that came from server card `href` in `book`; how many went.
    pub fn remove_remote_card(&mut self, book: u32, href: &str) -> usize {
        let before = self.contacts.len();
        self.contacts.retain(|c| !(c.book == book && c.remote.as_ref().is_some_and(|r| r.href == href)));
        before.saturating_sub(self.contacts.len())
    }

    /// Import a `.vcf` file into `book`. A card whose UID is already here, or (for cards without
    /// one) that shares an address with a contact here, updates that contact where it is; the
    /// rest are added.
    pub fn import_vcf(&mut self, text: &str, book: u32) -> Result<VcfReport> {
        self.book(book)?;
        let parsed = ibercorreo_vcard::parse(text)?;
        let mut report = VcfReport { added: 0, updated: 0, skipped_properties: parsed.skipped, rejected: parsed.rejected };
        for card in parsed.contacts {
            let same = |c: &Contact| {
                if !card.uid.is_empty() {
                    c.uid == card.uid
                } else {
                    c.emails.iter().any(|a| card.emails.iter().any(|b| a.email.eq_ignore_ascii_case(&b.email)))
                }
            };
            if let Some(existing) = self.contacts.iter_mut().find(|c| same(c)) {
                let uid = if card.uid.is_empty() { existing.uid.clone() } else { card.uid.clone() };
                // Still the same server card, now changed here.
                let remote = existing.remote.take().map(|r| EventRemote { dirty: true, ..r });
                *existing = Contact { id: existing.id, book: existing.book, uid, remote, ..card };
                report.updated += 1;
            } else {
                if self.contacts.len() >= Self::MAX_CONTACTS {
                    return Err(Error::TooLarge { what: "contacts", size: self.contacts.len() + 1, limit: Self::MAX_CONTACTS });
                }
                self.add_contact(Contact { book, ..card });
                report.added += 1;
            }
        }
        Ok(report)
    }

    /// The contacts `ids` (every contact when empty) as a `.vcf` file.
    pub fn export_vcf(&self, ids: &[u64]) -> String {
        ibercorreo_vcard::write(self.contacts.iter().filter(|c| ids.is_empty() || ids.contains(&c.id)))
    }

    pub fn contacts(&self, query: &str) -> Vec<&Contact> {
        let q = query.to_lowercase();
        let mut v: Vec<&Contact> = self.contacts.iter().filter(|c| c.matches(&q)).collect();
        v.sort_by_key(|c| c.name.to_lowercase());
        v
    }

    // ---- disk ----

    /// Write the store to `path`: the messages that changed to their shards, then the rest.
    pub fn save(&self, path: &Path) -> Result<()> {
        let pending = shards::write(&self.saved, path, &self.messages)?;
        let mut rest = self.clone();
        rest.messages = Vec::new();
        rest.message_shards = true;
        let json = serde_json::to_vec_pretty(&rest).map_err(|e| Error::Storage(e.to_string()))?;
        shards::write_file(path, &json)?;
        shards::finish(&self.saved, path, pending);
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Store> {
        let meta = std::fs::metadata(path).map_err(|e| Error::Storage(format!("{}: {e}", path.display())))?;
        if meta.len() > MAX_STORE_BYTES {
            return Err(Error::TooLarge {
                what: "store file",
                size: usize::try_from(meta.len()).unwrap_or(usize::MAX),
                limit: MAX_STORE_BYTES as usize,
            });
        }
        let bytes = std::fs::read(path).map_err(|e| Error::Storage(format!("{}: {e}", path.display())))?;
        let mut s = Store::parse(&bytes)?;
        let headers_only = s.version >= 6;
        s.repaired.old_format = !headers_only;
        if !s.message_shards {
            return s.finish_load();
        }
        s.message_shards = false;
        s.messages.extend(shards::read(path)?.into_iter().map(Arc::new));
        // Past the ids of messages left out too, so none is taken again while its copy is on disk.
        if let Some(top) = s.messages.iter().map(|m| m.id.0.saturating_add(1)).max() {
            s.next_message = s.next_message.max(top);
        }
        s.repaired.left_out = s.drop_strays();
        let s = s.finish_load()?;
        s.saved.remember(path, &s.messages, headers_only);
        Ok(s)
    }

    /// Read a store file whose messages are inline (one written before shards, or by a test).
    pub fn from_json(bytes: &[u8]) -> Result<Store> {
        let s = Store::parse(bytes)?;
        if s.message_shards {
            return Err(Error::invalid("store file", "its messages are in shard files beside it: open it with Store::load"));
        }
        s.finish_load()
    }

    /// Move a store file that could not be read out of the way, with its shards, so a fresh store
    /// can start without touching them. Returns where the file went.
    pub fn set_aside(path: &Path) -> Result<PathBuf> {
        let aside = path.with_extension("unreadable.json");
        std::fs::rename(path, &aside).map_err(|e| Error::Storage(format!("{}: {e}", path.display())))?;
        let dir = shards::dir(path);
        if dir.exists() {
            std::fs::rename(&dir, shards::dir(&aside)).map_err(|e| Error::Storage(format!("{}: {e}", dir.display())))?;
        }
        Ok(aside)
    }

    /// Before the first save of a store file from before shards: copy it aside once
    /// (`store.before-shards.json`), so the move to shards can be undone by hand. Returns the copy
    /// made, if one was.
    pub fn keep_unsharded_copy(path: &Path) -> Result<Option<PathBuf>> {
        let copy = path.with_extension("before-shards.json");
        if !path.exists() || shards::dir(path).exists() || copy.exists() {
            return Ok(None);
        }
        std::fs::copy(path, &copy).map_err(|e| Error::Storage(format!("{}: {e}", copy.display())))?;
        Ok(Some(copy))
    }

    /// How many messages the last load left out: ones in a folder the store file no longer has,
    /// or a second copy of one id. Only a save cut short (shards written, the store file not)
    /// leaves them; a synced one comes back with the folder at the next Send/Receive.
    pub fn repaired_on_load(&self) -> usize {
        self.repaired.left_out
    }

    /// Whether the load read a store from before content files, which holds every message's
    /// content in memory until it is saved (split) and opened again: save it soon.
    pub fn needs_rewrite(&self) -> bool {
        self.repaired.old_format
    }

    /// Drop messages whose folder (or server folder) is gone, and repeated ids; returns how many.
    fn drop_strays(&mut self) -> usize {
        let folders: std::collections::HashSet<FolderId> = self.folders.iter().map(|f| f.id).collect();
        let mut ids = std::collections::HashSet::new();
        let before = self.messages.len();
        self.messages.retain(|m| folders.contains(&m.folder) && m.remote.is_none_or(|r| folders.contains(&r.folder)) && ids.insert(m.id));
        before.saturating_sub(self.messages.len())
    }

    fn parse(bytes: &[u8]) -> Result<Store> {
        let s: Store = serde_json::from_slice(bytes).map_err(|e| Error::invalid("store file", e.to_string()))?;
        if s.version > FORMAT_VERSION {
            return Err(Error::invalid("store file", format!("format {} is newer than this build ({FORMAT_VERSION})", s.version)));
        }
        Ok(s)
    }

    /// Check a store just read and bring it up to this format.
    fn finish_load(mut self) -> Result<Store> {
        let s = &mut self;
        // Stores from before address books: one local book holds every contact.
        if s.books.is_empty() {
            s.books.push(AddressBook { id: 0, name: "Contacts".into(), remote: None });
        }
        s.check()?;
        // Ids in order, for the binary search in `position` (`check` made them unique).
        if !s.messages.is_sorted_by_key(|m| m.id) {
            s.messages.sort_by_key(|m| m.id);
        }
        let accounts: Vec<AccountId> = s.accounts.iter().map(|a| a.id).collect();
        for a in accounts {
            s.add_standard_folders(a);
        }
        if s.folders.is_empty() {
            s.add_standard_folders(AccountId(0));
        }
        // A hand-edited list keeps only folders that exist, each once.
        let mut seen = std::collections::HashSet::new();
        let folders = &s.folders;
        s.favorites.retain(|id| folders.iter().any(|f| f.id == *id) && seen.insert(*id));
        if let Some(top) = s.accounts.iter().map(|a| a.id.0.saturating_add(1)).max() {
            s.next_account = s.next_account.max(top);
        }
        // Past every id held, so a new message never takes one a shard already has.
        if let Some(top) = s.messages.iter().map(|m| m.id.0.saturating_add(1)).max() {
            s.next_message = s.next_message.max(top);
        }
        for c in &mut s.contacts {
            c.upgrade();
        }
        // A hand-edited file: rules past the limit or with a repeated id are dropped.
        let mut rule_ids = std::collections::HashSet::new();
        s.rules.retain(|r| rule_ids.insert(r.id));
        s.rules.truncate(rules::MAX_RULES);
        // Older files read as this format (new fields default) and are written back as it.
        s.version = FORMAT_VERSION;
        Ok(self)
    }

    /// Ids unique and every reference resolvable, so a hand-edited file can't confuse the app.
    fn check(&self) -> Result<()> {
        if self.accounts.len() > MAX_ACCOUNTS {
            return Err(Error::invalid("store file", format!("more than {MAX_ACCOUNTS} accounts")));
        }
        let mut accounts = std::collections::HashSet::new();
        for a in &self.accounts {
            if !accounts.insert(a.id) {
                return Err(Error::invalid("store file", format!("duplicate account id {}", a.id.0)));
            }
        }
        for f in &self.folders {
            if !self.accounts.is_empty() && !accounts.contains(&f.account) {
                return Err(Error::invalid("store file", format!("folder {} belongs to no account", f.id.0)));
            }
        }
        let mut ids = std::collections::HashSet::new();
        for m in &self.messages {
            if !ids.insert(m.id) {
                return Err(Error::invalid("store file", format!("duplicate message id {}", m.id.0)));
            }
            self.folder(m.folder)?;
            if let Some(r) = m.remote {
                self.folder(r.folder)?;
            }
        }
        for t in &self.tombstones {
            self.folder(t.folder)?;
        }
        for e in &self.events {
            self.calendar(e.calendar)?;
        }
        for c in &self.contacts {
            self.book(c.book)?;
        }
        Ok(())
    }

    /// Attachment names of a message, for the reading pane.
    pub fn attachments(&self, id: MessageId) -> Result<&[Attachment]> {
        Ok(&self.message(id)?.attachments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibercorreo_core::{Address, DateTime, RemoteFolder};

    #[test]
    fn favorites_keep_their_order_and_go_with_their_folder() {
        let mut s = Store::new();
        let ana = s.add_account(imap("ana@example.org")).unwrap();
        let bob = s.add_account(imap("bob@example.org")).unwrap();
        let ana_sent = s.folder_by_role(ana, FolderRole::Sent).unwrap();
        let bob_inbox = s.folder_by_role(bob, FolderRole::Inbox).unwrap();
        assert!(s.set_favorite(bob_inbox, true).unwrap());
        assert!(s.set_favorite(ana_sent, true).unwrap());
        assert!(!s.set_favorite(ana_sent, true).unwrap(), "once only");
        assert_eq!(s.favorites(), [bob_inbox, ana_sent]);
        assert!(s.set_favorite(FolderId(9999), true).is_err());

        let back = Store::from_json(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back.favorites(), [bob_inbox, ana_sent]);

        s.remove_account(bob).unwrap();
        assert_eq!(s.favorites(), [ana_sent]);
        assert!(s.set_favorite(ana_sent, false).unwrap());
        assert!(s.favorites().is_empty());

        // A hand-edited file: unknown and repeated folders are dropped.
        let mut v: serde_json::Value = serde_json::to_value(&back).unwrap();
        v["favorites"] = serde_json::json!([ana_sent.0, 9999, ana_sent.0]);
        let edited = Store::from_json(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(edited.favorites(), [ana_sent]);
    }

    fn msg(store: &Store, subject: &str, day: u8) -> Message {
        let mut m = Message::new(MessageId(0), store.folder_by_role(AccountId(0), FolderRole::Inbox).unwrap());
        m.set_subject(subject);
        m.from = Some(Address::parse("Ana <ana@example.org>").unwrap());
        m.date = Some(DateTime::new(Date::new(2026, 10, day).unwrap(), 9, 0, 0).unwrap());
        m
    }

    #[test]
    fn search_finds_words_by_prefix_without_accents_in_every_field() {
        let mut s = Store::new();
        let mut m = msg(&s, "Factura de octubre", 3);
        m.content_mut().unwrap().body = "El camión llega el lunes. Prou col·lecció!".into();
        m.to = vec![Address::parse("Олена Коваль <olena@example.com>").unwrap()];
        m.attachments.push(Attachment { name: "presupuesto-2026.pdf".into(), mime_type: "application/pdf".into(), size: 10 });
        let a = s.add_message(m).unwrap();
        let mut other = msg(&s, "Weekly report", 4);
        other.from = Some(Address::parse("Luis <luis@example.net>").unwrap());
        other.flagged = true;
        other.seen = true;
        let b = s.add_message(other).unwrap();
        let ids = |q: &str, s: &Store| s.search(q).iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids("camion", &s), [a]);
        assert_eq!(ids("CAMIÓN lun", &s), [a]);
        assert_eq!(ids("colleccio", &s), [a]);
        assert_eq!(ids("коваль", &s), [a]);
        assert_eq!(ids("to:olena", &s), [a]);
        assert!(ids("from:olena", &s).is_empty());
        assert_eq!(ids("attachment:presupuesto", &s), [a]);
        assert_eq!(ids("has:attachment", &s), [a]);
        assert_eq!(ids("fact", &s), [a]);
        assert!(ids("actura", &s).is_empty(), "words match from their start, as in Outlook");
        assert_eq!(ids("from:luis is:flagged", &s), [b]);
        assert_eq!(ids("is:unread", &s), [a]);
        assert_eq!(ids("is:read", &s), [b]);
        assert_eq!(ids("\"weekly report\"", &s), [b]);
        assert!(ids("\"report weekly\"", &s).is_empty());
        assert_eq!(ids("example -factura", &s), [b]);
        assert_eq!(ids("camion OR weekly", &s), [b, a]);
        assert_eq!(ids("after:2026-10-04", &s), [b]);
        assert_eq!(ids("before:2026-10-04", &s), [a]);
        assert_eq!(ids("on:2026-10-03", &s), [a]);
        assert_eq!(ids("", &s), [b, a]);
        assert!(ids("!!!", &s).is_empty());
        assert!(ids("nothing-like-this", &s).is_empty());
    }

    #[test]
    fn search_index_follows_every_change() {
        let mut s = Store::new();
        let a = s.add_message(msg(&s, "alpha", 1)).unwrap();
        assert_eq!(s.search("alpha").len(), 1);
        // Changed in place: the old words go, the new ones are found.
        s.message_mut(a).unwrap().set_subject("beta");
        assert!(s.search("alpha").is_empty());
        assert_eq!(s.search("beta").len(), 1);
        // A copy (an undo snapshot) shares the index but keeps its own words.
        let snapshot = s.clone();
        s.message_mut(a).unwrap().set_subject("gamma");
        assert_eq!(s.search("gamma").len(), 1);
        assert_eq!(snapshot.search("beta").len(), 1);
        assert!(snapshot.search("gamma").is_empty());
        assert_eq!(s.search("gamma").len(), 1);
        // Removed and put back.
        let removed = s.remove_message(a).unwrap();
        assert!(s.search("gamma").is_empty());
        s.restore_message(removed).unwrap();
        assert_eq!(s.search("gamma").len(), 1);
        // Read back from disk.
        let back = Store::from_json(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.search("gamma").len(), 1);
        assert!(back.search("beta").is_empty());
    }

    /// A copy of the store (an undo step, the copy a save writes) shares the messages: only a
    /// message changed after is copied, so a big mailbox costs no more to copy than a small one.
    #[test]
    fn copies_share_messages_until_one_changes() {
        let mut s = Store::new();
        let a = s.add_message(msg(&s, "alpha", 1)).unwrap();
        let b = s.add_message(msg(&s, "beta", 2)).unwrap();
        let copy = s.clone();
        assert!(std::ptr::eq(s.message(a).unwrap(), copy.message(a).unwrap()));
        s.message_mut(a).unwrap().seen = true;
        assert!(!copy.message(a).unwrap().seen, "the copy keeps the message as it was");
        assert!(!std::ptr::eq(s.message(a).unwrap(), copy.message(a).unwrap()));
        assert!(std::ptr::eq(s.message(b).unwrap(), copy.message(b).unwrap()), "an unchanged message stays shared");
        assert_ne!(s, copy);
    }

    /// Messages are found by id in any order the file has them, and after one is put back.
    #[test]
    fn messages_are_found_by_id_in_any_order() {
        let mut s = Store::new();
        let ids: Vec<MessageId> = (1..=5).map(|d| s.add_message(msg(&s, "m", d)).unwrap()).collect();
        let mut v = serde_json::to_value(&s).unwrap();
        v["messages"].as_array_mut().unwrap().reverse();
        let mut back = Store::from_json(&serde_json::to_vec(&v).unwrap()).unwrap();
        for &id in &ids {
            assert_eq!(back.message(id).unwrap().id, id);
        }
        let removed = back.remove_message(ids[2]).unwrap();
        let fresh = back.add_message(msg(&back, "new", 9)).unwrap();
        assert!(fresh.0 > ids[4].0);
        back.restore_message(removed).unwrap();
        for id in ids.iter().copied().chain([fresh]) {
            assert_eq!(back.message(id).unwrap().id, id);
        }
        assert!(back.message(MessageId(999)).is_err());
    }

    /// The index can be read on another thread ahead of a search, which then reads only what
    /// changed after.
    #[test]
    fn the_search_index_warms_up_ahead_of_a_search() {
        let mut s = Store::new();
        let a = s.add_message(msg(&s, "quarterly budget", 1)).unwrap();
        let job = s.search_warmup();
        std::thread::spawn(job).join().unwrap();
        assert_eq!(s.search_cache.indexed(), 1);
        let b = s.add_message(msg(&s, "budget review", 2)).unwrap();
        assert_eq!(s.search("budget").iter().map(|m| m.id).collect::<Vec<_>>(), [b, a]);
        assert_eq!(s.search_cache.indexed(), 2);
    }

    #[test]
    fn search_scopes_to_a_folder_or_an_account() {
        let mut s = Store::new();
        let first = s.add_account(imap("ana@example.org")).unwrap();
        let second = s.add_account(imap("bob@example.org")).unwrap();
        let inbox1 = s.folder_by_role(first, FolderRole::Inbox).unwrap();
        let sent1 = s.folder_by_role(first, FolderRole::Sent).unwrap();
        let inbox2 = s.folder_by_role(second, FolderRole::Inbox).unwrap();
        let mut put = |folder: FolderId| {
            let mut m = Message::new(MessageId(0), folder);
            m.set_subject("budget");
            s.add_message(m).unwrap()
        };
        let (a, b, c) = (put(inbox1), put(sent1), put(inbox2));
        let ids = |scope: search::Scope| s.search_in("budget", scope).iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(search::Scope::All), [c, b, a]);
        assert_eq!(ids(search::Scope::Account(first)), [b, a]);
        assert_eq!(ids(search::Scope::Account(second)), [c]);
        assert_eq!(ids(search::Scope::Folder(sent1)), [b]);
        assert!(ids(search::Scope::Folder(FolderId(9999))).is_empty());
    }

    #[test]
    fn messages_sort_newest_first_and_move() {
        let mut s = Store::new();
        let inbox = s.folder_by_role(AccountId(0), FolderRole::Inbox).unwrap();
        let trash = s.folder_by_role(AccountId(0), FolderRole::Trash).unwrap();
        let a = s.add_message(msg(&s, "older", 1)).unwrap();
        let b = s.add_message(msg(&s, "newer", 20)).unwrap();
        assert_eq!(s.messages_in(inbox).iter().map(|m| m.id).collect::<Vec<_>>(), vec![b, a]);
        assert_eq!(s.unread_count(inbox), 2);
        s.move_message(a, trash).unwrap();
        assert_eq!(s.messages_in(trash).len(), 1);
        assert!(s.move_message(a, FolderId(999)).is_err());
        assert_eq!(s.search("NEWER").len(), 1);
        assert_eq!(s.search("ana@").len(), 2);
        let removed = s.remove_message(b).unwrap();
        assert!(s.message(b).is_err());
        s.restore_message(removed.clone()).unwrap();
        assert!(s.restore_message(removed).is_err());
    }

    #[test]
    fn ics_import_updates_by_uid() {
        let mut s = Store::new();
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\nDTSTART:20261022T100000\nDTEND:20261022T110000\nSUMMARY:A\nEND:VEVENT\nEND:VCALENDAR\n";
        assert_eq!(s.import_ics(ics, 0).unwrap().added, 1);
        let r = s.import_ics(&ics.replace("SUMMARY:A", "SUMMARY:B"), 0).unwrap();
        assert_eq!((r.added, r.updated), (0, 1));
        let day = Date::new(2026, 10, 22).unwrap();
        assert_eq!(s.events_on(day, &Zone::utc()).first().map(|o| o.event.summary.as_str()), Some("B"));
        assert!(s.import_ics(ics, 42).is_err());
        assert!(s.export_ics(None).contains("SUMMARY:B"));
    }

    #[test]
    fn repeating_events_expand_skip_exdates_and_yield_to_changed_occurrences() {
        let mut s = Store::new();
        let ics = "BEGIN:VCALENDAR\r\n\
BEGIN:VEVENT\r\nUID:standup\r\nDTSTART:20261019T091500\r\nDTEND:20261019T093000\r\nSUMMARY:Stand-up\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR\r\nEXDATE:20261021T091500,20261023T091500\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:standup\r\nRECURRENCE-ID:20261022T091500\r\nDTSTART:20261022T110000\r\nDTEND:20261022T111500\r\n\
SUMMARY:Stand-up (moved)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let r = s.import_ics(ics, 0).unwrap();
        assert_eq!((r.added, r.updated), (2, 0), "the changed occurrence is its own event, not an update of the series");
        let on = |s: &Store, d: u8| -> Vec<(String, String)> {
            s.events_on(Date::new(2026, 10, d).unwrap(), &Zone::utc())
                .iter()
                .map(|o| (o.event.summary.clone(), o.when.start_date().to_string()))
                .collect()
        };
        assert_eq!(on(&s, 19), [("Stand-up".to_string(), "2026-10-19".to_string())]);
        assert!(on(&s, 21).is_empty(), "EXDATE removes the Wednesday");
        assert_eq!(on(&s, 22), [("Stand-up (moved)".to_string(), "2026-10-22".to_string())], "the changed copy replaces the occurrence");
        assert!(on(&s, 24).is_empty(), "no weekend occurrences");
        assert_eq!(on(&s, 26).len(), 1, "the next week");
        assert_eq!(on(&s, 30).len(), 1);
        // A second import updates both, each by its own key.
        let r = s.import_ics(ics, 0).unwrap();
        assert_eq!((r.added, r.updated), (0, 2));
        let out = s.export_ics(None);
        // EXDATE takes DTSTART's form: the start time on each cancelled day.
        assert!(out.contains("EXDATE:20261021T091500,20261023T091500") && out.contains("RECURRENCE-ID;VALUE=DATE:20261022"), "{out}");
    }

    #[test]
    fn events_show_in_the_viewers_zone_and_repeat_in_their_own() {
        let mut s = Store::new();
        // Weekly at 09:00 Lisbon time, Mondays; a one-off at 23:30 Lisbon; a UTC call; a floating
        // reminder; an Outlook-style Windows zone name.
        let ics = "BEGIN:VCALENDAR\r\n\
BEGIN:VEVENT\r\nUID:weekly\r\nDTSTART;TZID=Europe/Lisbon:20261019T090000\r\nDTEND;TZID=Europe/Lisbon:20261019T100000\r\n\
SUMMARY:Weekly\r\nRRULE:FREQ=WEEKLY;COUNT=4\r\nEXDATE;TZID=Europe/Lisbon:20261109T090000\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:late\r\nDTSTART;TZID=Europe/Lisbon:20261022T233000\r\nDTEND;TZID=Europe/Lisbon:20261023T000000\r\nSUMMARY:Late\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:utc\r\nDTSTART:20261022T120000Z\r\nDTEND:20261022T130000Z\r\nSUMMARY:UTC call\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:float\r\nDTSTART:20261022T080000\r\nDTEND:20261022T081500\r\nSUMMARY:Pills\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:win\r\nDTSTART;TZID=\"Eastern Standard Time\":20261022T090000\r\nDTEND;TZID=\"Eastern Standard Time\":20261022T093000\r\nSUMMARY:NY\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
        s.import_ics(ics, 0).unwrap();
        let on = |s: &Store, y: i32, m: u8, d: u8, zone: &str| -> Vec<(String, String)> {
            let z = Zone::named(zone).unwrap();
            s.events_on(Date::new(y, m, d).unwrap(), &z)
                .iter()
                .map(|o| match o.when {
                    When::Timed { start, .. } => (o.event.summary.clone(), format!("{:02}:{:02}", start.hour(), start.minute())),
                    When::AllDay { .. } => (o.event.summary.clone(), "all day".into()),
                })
                .collect()
        };
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        // In Kyiv (UTC+3 in October): Lisbon 23:30 is 01:30 the next day; UTC 12:00 is 15:00;
        // floating stays 08:00; New York 09:00 (EDT) is 16:00.
        assert_eq!(on(&s, 2026, 10, 22, "Europe/Kyiv"), [pair("Pills", "08:00"), pair("UTC call", "15:00"), pair("NY", "16:00")]);
        assert_eq!(on(&s, 2026, 10, 23, "Europe/Kyiv"), [pair("Late", "01:30")]);
        assert_eq!(
            on(&s, 2026, 10, 22, "Europe/Lisbon"),
            [pair("Pills", "08:00"), pair("UTC call", "13:00"), pair("NY", "14:00"), pair("Late", "23:30")]
        );
        // The weekly event stays 09:00 in Lisbon; seen from New York it moves when Europe changes
        // its clocks (25 Oct) but America has not yet (1 Nov).
        assert_eq!(on(&s, 2026, 10, 19, "America/New_York"), [pair("Weekly", "04:00")]);
        assert_eq!(on(&s, 2026, 10, 26, "America/New_York"), [pair("Weekly", "05:00")], "the week between the two changes");
        assert_eq!(on(&s, 2026, 11, 2, "America/New_York"), [pair("Weekly", "04:00")]);
        assert!(on(&s, 2026, 11, 9, "America/New_York").is_empty(), "EXDATE in Lisbon time");
        assert_eq!(on(&s, 2026, 11, 2, "Europe/Lisbon"), [pair("Weekly", "09:00")]);
        // A one-off in a zone is exported in UTC; the repeating one keeps its zone.
        let out = s.export_ics(None);
        assert!(out.contains("DTSTART:20261022T223000Z"), "{out}");
        assert!(out.contains("DTSTART;TZID=Europe/Lisbon:20261019T090000"), "{out}");
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ibercorreo-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("store.json")
    }

    fn shard_files(path: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            std::fs::read_dir(shards::dir(path)).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    /// `s` as a load gives it back: messages with their headers only (content stays on disk).
    fn as_loaded(s: &Store) -> Store {
        let mut c = s.clone();
        c.messages = c.messages.iter().map(|m| Arc::new(m.headers_only())).collect();
        c
    }

    fn modified(path: &Path, shard: &str) -> std::time::SystemTime {
        std::fs::metadata(shards::dir(path).join(shard)).unwrap().modified().unwrap()
    }

    /// Messages go to shard files beside the store file, a save rewrites only the shards that
    /// changed, and the store file stays small however big the mailbox grows.
    #[test]
    fn messages_are_saved_in_shards_and_only_changed_ones_are_rewritten() {
        let path = temp("shards");
        let mut s = Store::new();
        for d in 1..=3 {
            s.add_message(msg(&s, "early", d)).unwrap();
        }
        s.next_message = 2500;
        let late = s.add_message(msg(&s, "late", 4)).unwrap();
        s.save(&path).unwrap();
        assert_eq!(shard_files(&path), ["c000000.json", "c000002.json", "m000000.json", "m000002.json"]);
        let inline = std::fs::read_to_string(&path).unwrap();
        assert!(!inline.contains("early") && inline.contains("\"message_shards\": true"), "{inline}");
        let back = Store::load(&path).unwrap();
        assert_eq!(back, as_loaded(&s));
        assert_eq!(back.repaired_on_load(), 0);
        // Change the late message only: its shard is written again, the other one is not.
        let (first, second) = (modified(&path, "m000000.json"), modified(&path, "m000002.json"));
        let (first_content, second_content) = (modified(&path, "c000000.json"), modified(&path, "c000002.json"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut back = back;
        back.message_mut(late).unwrap().seen = true;
        back.save(&path).unwrap();
        assert_eq!(modified(&path, "m000000.json"), first, "an unchanged shard is not rewritten");
        assert_ne!(modified(&path, "m000002.json"), second);
        assert_eq!(modified(&path, "c000000.json"), first_content);
        assert_eq!(modified(&path, "c000002.json"), second_content, "a flag change leaves the content file alone");
        // The late message gone, its shard goes too; its content stays while undo may want it.
        back.remove_message(late).unwrap();
        back.save(&path).unwrap();
        assert_eq!(shard_files(&path), ["c000000.json", "c000002.json", "m000000.json"]);
        let again = Store::load(&path).unwrap();
        assert_eq!(again, back);
        assert!(again.message(late).is_err());
        // A file with shards is opened with its path, never as bare JSON missing its mail.
        assert!(Store::from_json(&std::fs::read(&path).unwrap()).is_err());
    }

    /// A message's content stays in its content file until it is read: a load holds headers
    /// only, and showing, searching, editing a draft, a rule and undo read it from there.
    #[test]
    fn message_content_stays_on_disk_until_it_is_read() {
        let path = temp("content");
        let mut s = Store::new();
        let mut m = msg(&s, "Factura", 1);
        let c = m.content_mut().unwrap();
        c.body = "El camión llega el lunes.\nSegunda línea".into();
        c.invite = Some("BEGIN:VCALENDAR".into());
        c.rich = Some(ibercorreo_core::rich::Rich::default());
        let a = s.add_message(m).unwrap();
        let mut draft = msg(&s, "Borrador", 2);
        draft.content_mut().unwrap().body = "primera versión".into();
        let d = s.add_message(draft).unwrap();
        let original = s.content(a).unwrap();
        s.save(&path).unwrap();
        let headers = std::fs::read_to_string(shards::dir(&path).join("m000000.json")).unwrap();
        assert!(!headers.contains("Segunda") && !headers.contains("VCALENDAR"), "{headers}");

        let mut back = Store::load(&path).unwrap();
        assert_eq!(back.message(a).unwrap().content(), None, "nothing read yet");
        assert_eq!(back.message(a).unwrap().preview(80), "El camión llega el lunes.", "the list needs no content");
        assert_eq!(back.content(a).unwrap(), original);
        assert_eq!(back.message(a).unwrap().content(), None, "reading does not keep it with the message");
        // Search reads content files for the index, and the text itself for a phrase.
        let ids = |q: &str, s: &Store| s.search(q).iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids("camion", &back), [a]);
        assert_eq!(ids("\"llega el lunes\"", &back), [a]);
        assert!(ids("\"lunes llega\"", &back).is_empty());
        // A rule that looks at the body reads it too.
        back.add_rule(rules::Rule {
            id: 0,
            name: "camión".into(),
            enabled: true,
            account: None,
            query: "body:camion".into(),
            actions: vec![rules::RuleAction::Flag],
            stop: false,
        })
        .unwrap();
        back.run_rules(&[a, d], None);
        assert!(back.message(a).unwrap().flagged && !back.message(d).unwrap().flagged);
        // A draft edited: only its content is rewritten into the file, the other one kept.
        back.content_mut(d).unwrap().body = "segunda versión".into();
        back.save(&path).unwrap();
        let back = Store::load(&path).unwrap();
        assert_eq!(back.content(d).unwrap().body, "segunda versión");
        assert_eq!(back.content(a).unwrap(), original);

        // Removed and saved, then put back by undo (the copy from before): its content is still
        // on disk until the store is opened again.
        let snapshot = back.clone();
        let mut back = back;
        back.remove_message(a).unwrap();
        // The draft edited in the same save, so the content file is written again.
        back.content_mut(d).unwrap().body = "tercera versión".into();
        back.save(&path).unwrap();
        assert_eq!(snapshot.content(a).unwrap(), original);
        snapshot.save(&path).unwrap();
        assert_eq!(Store::load(&path).unwrap().content(a).unwrap(), original);
        back.save(&path).unwrap();
        // Opened again without it: reading the content files finds it, and the next save drops
        // it, so deleted mail does not stay on disk.
        let content_file = shards::dir(&path).join("c000000.json");
        assert!(std::fs::read_to_string(&content_file).unwrap().contains("camión"));
        let reopened = Store::load(&path).unwrap();
        (reopened.search_warmup())();
        reopened.save(&path).unwrap();
        let kept = std::fs::read_to_string(&content_file).unwrap();
        assert!(!kept.contains("camión") && kept.contains("tercera versión"), "{kept}");
    }

    /// Shards written before content files (format 5) hold each message whole: they open as
    /// they are, and the next save splits them.
    #[test]
    fn shards_from_before_content_files_are_split_on_save() {
        let path = temp("split");
        let mut s = Store::new();
        let mut m = msg(&s, "old", 1);
        m.content_mut().unwrap().body = "texto antiguo".into();
        let id = s.add_message(m).unwrap();
        let whole: Vec<&Message> = s.all_messages().collect();
        let mut rest = s.clone();
        rest.messages.clear();
        rest.message_shards = true;
        rest.version = 5;
        std::fs::create_dir_all(shards::dir(&path)).unwrap();
        std::fs::write(shards::dir(&path).join("m000000.json"), serde_json::to_vec(&whole).unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&rest).unwrap()).unwrap();

        let back = Store::load(&path).unwrap();
        assert!(back.needs_rewrite());
        assert_eq!(back.message(id).unwrap().content().map(|c| c.body.as_str()), Some("texto antiguo"));
        back.save(&path).unwrap();
        assert_eq!(shard_files(&path), ["c000000.json", "m000000.json"]);
        let headers = std::fs::read_to_string(shards::dir(&path).join("m000000.json")).unwrap();
        assert!(!headers.contains("\"body\""), "only the preview stays with the headers: {headers}");
        let again = Store::load(&path).unwrap();
        assert!(!again.needs_rewrite());
        assert_eq!(again.message(id).unwrap().content(), None);
        assert_eq!(again.content(id).unwrap().body, "texto antiguo");
    }

    /// A content file that cannot be read loses only the text: the messages still list, opening
    /// one says its text is missing, and a save keeps the broken file aside instead of over it.
    #[test]
    fn a_broken_content_file_loses_only_the_text() {
        let path = temp("broken");
        let mut s = Store::new();
        let a = s.add_message(msg(&s, "listed", 1)).unwrap();
        s.save(&path).unwrap();
        let content_file = shards::dir(&path).join("c000000.json");
        std::fs::write(&content_file, b"not json").unwrap();
        let mut back = Store::load(&path).unwrap();
        assert_eq!(back.message(a).unwrap().subject, "listed");
        assert!(back.content(a).is_err());
        (back.search_warmup())();
        assert_eq!(back.search("listed").len(), 1);
        back.add_message(msg(&back, "new", 2)).unwrap();
        back.save(&path).unwrap();
        assert!(content_file.with_extension("unreadable.json").exists());
        assert_eq!(Store::load(&path).unwrap().all_messages().len(), 2);
    }

    /// A store from before shards (messages inline) opens as it is and is written back sharded.
    #[test]
    fn an_inline_store_is_written_back_sharded() {
        let path = temp("inline");
        let mut s = Store::new();
        let id = s.add_message(msg(&s, "old", 1)).unwrap();
        let mut v = serde_json::to_value(&s).unwrap();
        v["version"] = serde_json::json!(4);
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        let back = Store::load(&path).unwrap();
        assert_eq!(back.message(id).unwrap().subject, "old");
        let copy = Store::keep_unsharded_copy(&path).unwrap().unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&path).unwrap());
        back.save(&path).unwrap();
        assert_eq!(Store::keep_unsharded_copy(&path).unwrap(), None, "once only");
        assert_eq!(shard_files(&path), ["c000000.json", "m000000.json"]);
        assert_eq!(Store::load(&path).unwrap().message(id).unwrap().subject, "old");
    }

    /// A save cut short (shards written, the store file not) leaves messages in a folder the
    /// store file does not know yet: the load leaves them out and says how many, instead of
    /// refusing the whole mailbox.
    #[test]
    fn a_save_cut_short_still_opens() {
        let path = temp("cut");
        let mut s = Store::new();
        let kept = s.add_message(msg(&s, "kept", 1)).unwrap();
        s.save(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let folder = s.add_folder(AccountId(0), "New on the server", FolderRole::Other);
        let mut m = msg(&s, "stray", 2);
        m.folder = folder;
        s.add_message(m).unwrap();
        s.save(&path).unwrap();
        std::fs::write(&path, before).unwrap();
        let back = Store::load(&path).unwrap();
        assert_eq!(back.repaired_on_load(), 1);
        assert_eq!(back.all_messages().map(|m| m.id).collect::<Vec<_>>(), [kept]);
        // A new message never takes an id a shard already used.
        let mut back = back;
        assert!(back.add_message(msg(&back, "next", 3)).unwrap().0 > kept.0 + 1);
    }

    /// A store that cannot be read is set aside with its shards, so a fresh one starts clean.
    #[test]
    fn an_unreadable_store_is_set_aside_with_its_shards() {
        let path = temp("aside");
        let mut s = Store::new();
        s.add_message(msg(&s, "kept", 1)).unwrap();
        s.save(&path).unwrap();
        std::fs::write(shards::dir(&path).join("m000000.json"), b"not json").unwrap();
        assert!(Store::load(&path).is_err());
        let aside = Store::set_aside(&path).unwrap();
        assert!(!path.exists() && !shards::dir(&path).exists());
        assert!(aside.exists() && shards::dir(&aside).join("m000000.json").exists());
        // A fresh store saved in its place does not see the old shards.
        let fresh = Store::new();
        fresh.save(&path).unwrap();
        assert!(Store::load(&path).unwrap().all_messages().len() == 0);
    }

    /// Saved somewhere new, a store clears shard files it does not have, so a later load does
    /// not read someone else's messages back.
    #[test]
    fn saving_over_another_store_clears_its_shards() {
        let path = temp("over");
        let mut big = Store::new();
        big.next_message = 5000;
        big.add_message(msg(&big, "other", 1)).unwrap();
        big.save(&path).unwrap();
        let mut small = Store::new();
        small.add_message(msg(&small, "mine", 1)).unwrap();
        small.save(&path).unwrap();
        assert_eq!(shard_files(&path), ["c000000.json", "m000000.json"]);
        let back = Store::load(&path).unwrap();
        assert_eq!(back.all_messages().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["mine"]);
    }

    #[test]
    fn saves_and_loads_and_rejects_bad_files() {
        let dir = std::env::temp_dir().join(format!("ibercorreo-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        let mut s = Store::new();
        s.add_message(msg(&s, "kept", 3)).unwrap();
        s.save(&path).unwrap();
        assert_eq!(Store::load(&path).unwrap(), as_loaded(&s));
        assert!(Store::from_json(b"{").is_err());
        assert!(Store::from_json(b"[]").is_err());
        let mut newer = serde_json::to_value(&s).unwrap();
        newer["version"] = serde_json::json!(99);
        assert!(Store::from_json(newer.to_string().as_bytes()).is_err());
        let mut dangling = serde_json::to_value(&s).unwrap();
        dangling["messages"][0]["folder"] = serde_json::json!(12345);
        assert!(Store::from_json(dangling.to_string().as_bytes()).is_err());
        assert!(Store::load(&dir.join("missing.json")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn deleting_a_synced_message_leaves_a_tombstone_until_undone() {
        let mut s = Store::new();
        let inbox = s.folder_by_role(AccountId(0), FolderRole::Inbox).unwrap();
        let mut m = msg(&s, "synced", 1);
        m.remote = Some(RemoteRef { folder: inbox, uid: 42, seen: false, flagged: false });
        let id = s.add_message(m).unwrap();
        assert_eq!(s.find_remote(inbox, 42), Some(id));
        let removed = s.remove_message(id).unwrap();
        assert_eq!(s.tombstones().len(), 1);
        s.restore_message(removed).unwrap();
        assert!(s.tombstones().is_empty());
        s.drop_message(id).unwrap();
        assert!(s.tombstones().is_empty(), "a removal the server made needs no tombstone");
        let local = s.add_message(msg(&s, "local", 2)).unwrap();
        s.remove_message(local).unwrap();
        assert!(s.tombstones().is_empty());
    }

    #[test]
    fn format_1_files_load_and_are_written_as_format_2() {
        let mut v1 = serde_json::to_value(Store::new()).unwrap();
        let obj = v1.as_object_mut().unwrap();
        obj.insert("version".into(), serde_json::json!(1));
        obj.remove("tombstones");
        let s = Store::from_json(v1.to_string().as_bytes()).unwrap();
        assert_eq!(s.version, FORMAT_VERSION);
        // A tombstone pointing at a folder that does not exist is refused.
        let mut bad = serde_json::to_value(Store::new()).unwrap();
        bad.as_object_mut().unwrap().insert("tombstones".into(), serde_json::json!([{ "folder": 999, "uid": 1, "seen": false, "flagged": false }]));
        assert!(Store::from_json(bad.to_string().as_bytes()).is_err());
    }

    #[test]
    fn format_2_contacts_keep_their_phone() {
        let mut v2 = serde_json::to_value(Store::new()).unwrap();
        let obj = v2.as_object_mut().unwrap();
        obj.insert("version".into(), serde_json::json!(2));
        obj.insert("contacts".into(), serde_json::json!([{ "id": 0, "name": "Ana", "emails": [], "phone": "+351 1", "organization": null }]));
        let s = Store::from_json(v2.to_string().as_bytes()).unwrap();
        assert_eq!(s.contact(0).unwrap().phones[0].number, "+351 1");
    }

    #[test]
    fn vcf_import_updates_by_uid_or_address_and_exports() {
        let mut s = Store::new();
        let one = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:u1\r\nFN:Ana\r\nEMAIL:ana@example.org\r\nEND:VCARD\r\n\
BEGIN:VCARD\r\nVERSION:2.1\r\nFN:Rui\r\nEMAIL:rui@example.pt\r\nEND:VCARD\r\n";
        let r = s.import_vcf(one, 0).unwrap();
        assert_eq!((r.added, r.updated), (2, 0));
        let again = "BEGIN:VCARD\r\nUID:u1\r\nFN:Ana Souza\r\nEND:VCARD\r\nBEGIN:VCARD\r\nFN:Rui Carvalho\r\nEMAIL:RUI@example.pt\r\nEND:VCARD\r\n";
        let r = s.import_vcf(again, 0).unwrap();
        assert_eq!((r.added, r.updated), (0, 2), "the same people, not copies");
        let names: Vec<&str> = s.contacts("").iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Ana Souza", "Rui Carvalho"]);
        let out = s.export_vcf(&[]);
        assert_eq!(out.matches("BEGIN:VCARD").count(), 2);
        assert_eq!(ibercorreo_vcard::parse(&out).unwrap().contacts.len(), 2);
        assert_eq!(s.export_vcf(&[0]).matches("BEGIN:VCARD").count(), 1);
        assert!(s.import_vcf("not a card", 0).is_err());
    }

    #[test]
    fn address_books_hold_contacts_and_server_cards_leave_tombstones() {
        let mut s = Store::new();
        assert_eq!(s.books().len(), 1, "a new store has one local address book");
        let remote = CalendarRemote { href: "/dav/books/phone/".into(), ctag: None };
        let phone = s.add_book("Phone", Some(remote)).unwrap();
        assert_eq!(s.book_by_href("/dav/books/phone/"), Some(phone));

        let mut lost = Contact::new("Lost");
        lost.book = 77;
        let lost = s.add_contact(lost);
        assert_eq!(s.contact(lost).unwrap().book, 0, "a missing book falls back to the first");
        assert!(!s.contact(lost).unwrap().uid.is_empty(), "every contact gets a UID, so it can go to a server");

        let mut ana = Contact::new("Ana");
        ana.remote = Some(EventRemote::new("/dav/books/phone/ana.vcf", "\"1\"", ""));
        assert!(s.replace_remote_card(phone, "/dav/books/phone/ana.vcf", ana.clone()).unwrap());
        let id = s.all_contacts().iter().find(|c| c.name == "Ana").unwrap().id;
        assert!(!s.replace_remote_card(phone, "/dav/books/phone/ana.vcf", Contact { name: "Ana S.".into(), ..ana }).unwrap());
        assert_eq!(s.contact(id).unwrap().name, "Ana S.", "updated in place, keeping its id");
        s.contact_mut(id).unwrap();
        assert!(s.contact(id).unwrap().remote.as_ref().unwrap().dirty, "changed here: to send");

        s.move_contact(id, 0).unwrap();
        assert_eq!(s.card_deletions().len(), 1, "moved off the server book: deleted there");
        assert!(s.contact(id).unwrap().remote.is_none());
        s.move_contact(id, phone).unwrap();
        assert_eq!(s.card_deletions().len(), 1, "a contact that was never sent leaves no tombstone");
        assert!(s.move_contact(id, 9).is_err());

        s.remove_book(phone).unwrap();
        assert!(s.contact(id).is_err(), "its contacts go with it");
        assert!(s.card_deletions().is_empty(), "and so do its tombstones");
        assert!(s.remove_book(0).is_err(), "the last book stays");
        let back = Store::from_json(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back.books().len(), 1);
    }

    fn imap(email: &str) -> Account {
        Account {
            id: AccountId(99),
            name: email.into(),
            address: Address::parse(email).unwrap(),
            kind: ibercorreo_core::mail::AccountKind::Imap {
                imap_host: "imap.example.org".into(),
                imap_port: 993,
                smtp_host: "smtp.example.org".into(),
                smtp_port: 465,
                user: String::new(),
                imap_security: Default::default(),
                smtp_security: Default::default(),
                oauth: String::new(),
            },
            caldav: String::new(),
            signature: String::new(),
        }
    }

    fn synced(s: &mut Store, folder: FolderId, uid: u32, subject: &str) -> MessageId {
        let mut m = Message::new(MessageId(0), folder);
        m.set_subject(subject);
        m.remote = Some(RemoteRef { folder, uid, seen: false, flagged: false });
        s.add_message(m).unwrap()
    }

    #[test]
    fn each_account_has_its_own_folders() {
        let mut s = Store::new();
        let local = s.add_message(msg(&s, "written before any account", 1)).unwrap();
        let a = s.add_account(imap("ana@example.org")).unwrap();
        assert_eq!(a, AccountId(0), "the first account takes over the folders the store started with");
        assert_eq!(s.message(local).unwrap().folder, s.folder_by_role(a, FolderRole::Inbox).unwrap());
        let b = s.add_account(imap("bo@example.net")).unwrap();
        assert_ne!(a, b);
        assert_eq!(s.folders_of(b).len(), 7, "a new account gets the standard folders");
        assert_ne!(s.folder_by_role(a, FolderRole::Inbox).unwrap(), s.folder_by_role(b, FolderRole::Inbox).unwrap());
        let order: Vec<AccountId> = s.folders().iter().map(|f| f.account).collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "the pane lists account by account: {order:?}");
        // Mailboxes with the same name on two servers stay apart.
        let (ia, ib) = (s.folder_by_role(a, FolderRole::Inbox).unwrap(), s.folder_by_role(b, FolderRole::Inbox).unwrap());
        for id in [ia, ib] {
            s.folder_mut(id).unwrap().remote = Some(RemoteFolder { path: "INBOX".into(), uid_validity: 1 });
        }
        assert_eq!(s.folder_by_path(a, "INBOX"), Some(ia));
        assert_eq!(s.folder_by_path(b, "INBOX"), Some(ib));
        let back = Store::from_json(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        // A third account after removing the second does not reuse its id.
        s.remove_account(b).unwrap();
        let c = s.add_account(imap("cy@example.com")).unwrap();
        assert!(c > b, "{c:?} after {b:?}");
    }

    #[test]
    fn removing_an_account_takes_its_mail_and_leaves_the_rest() {
        let mut s = Store::new();
        let a = s.add_account(imap("ana@example.org")).unwrap();
        let b = s.add_account(imap("bo@example.net")).unwrap();
        let ia = s.folder_by_role(a, FolderRole::Inbox).unwrap();
        let ib = s.folder_by_role(b, FolderRole::Inbox).unwrap();
        let keep = synced(&mut s, ia, 1, "ana's");
        let gone = synced(&mut s, ib, 1, "bo's");
        s.remove_message(gone).unwrap();
        let gone2 = synced(&mut s, ib, 2, "bo's too");
        assert_eq!(s.tombstones().len(), 1);
        s.remove_account(b).unwrap();
        assert!(s.message(gone2).is_err() && s.folder(ib).is_err() && s.tombstones().is_empty());
        assert!(s.message(keep).is_ok());
        assert!(s.remove_account(b).is_err(), "removed already");
        s.remove_account(a).unwrap();
        assert!(s.accounts.is_empty());
        assert_eq!(s.folders().len(), 7, "a store without accounts keeps empty standard folders");
        assert!(s.all_messages().len() == 0);
        assert_eq!(s.add_account(imap("di@example.org")).unwrap(), AccountId(0));
    }

    #[test]
    fn another_server_forgets_the_mail_synced_from_the_old_one() {
        let mut s = Store::new();
        let a = s.add_account(imap("ana@example.org")).unwrap();
        let inbox = s.folder_by_role(a, FolderRole::Inbox).unwrap();
        s.folder_mut(inbox).unwrap().remote = Some(RemoteFolder { path: "INBOX".into(), uid_validity: 7 });
        let lists = s.add_folder(a, "Lists", FolderRole::Other);
        s.folder_mut(lists).unwrap().remote = Some(RemoteFolder { path: "Lists".into(), uid_validity: 7 });
        let kept = s.add_folder(a, "Projects", FolderRole::Other);
        s.folder_mut(kept).unwrap().remote = Some(RemoteFolder { path: "Projects".into(), uid_validity: 7 });
        synced(&mut s, inbox, 1, "old server");
        synced(&mut s, lists, 2, "old server list");
        let tomb = synced(&mut s, inbox, 3, "deleted here");
        s.remove_message(tomb).unwrap();
        let mut mine = msg(&s, "only here", 2);
        mine.folder = kept;
        let mine = s.add_message(mine).unwrap();
        let draft = s.add_message(msg(&s, "local draft", 3)).unwrap();
        let other = s.add_account(imap("bo@example.net")).unwrap();
        let their_inbox = s.folder_by_role(other, FolderRole::Inbox).unwrap();
        let theirs = synced(&mut s, their_inbox, 1, "other account");

        assert_eq!(s.forget_server_mail(a), 2);
        assert!(s.tombstones().is_empty(), "nothing is deleted on the old server");
        assert!(s.folder(lists).is_err(), "an empty server-only folder goes");
        assert!(s.folder(kept).is_ok_and(|f| f.remote.is_none()), "a folder holding mail of its own stays, unlinked");
        assert!(s.message(mine).is_ok() && s.message(draft).is_ok() && s.message(theirs).is_ok());
        assert!(s.folder(inbox).is_ok_and(|f| f.remote.is_none()));
    }

    #[test]
    fn hand_edited_accounts_are_refused() {
        let mut s = Store::new();
        s.add_account(imap("ana@example.org")).unwrap();
        let mut v = serde_json::to_value(&s).unwrap();
        let first = v["accounts"][0].clone();
        v["accounts"].as_array_mut().unwrap().push(first);
        assert!(Store::from_json(&serde_json::to_vec(&v).unwrap()).is_err(), "two accounts with one id");
        let mut v = serde_json::to_value(&s).unwrap();
        v["folders"][0]["account"] = serde_json::json!(5);
        assert!(Store::from_json(&serde_json::to_vec(&v).unwrap()).is_err(), "a folder of no account");
    }

    #[test]
    fn a_store_from_before_several_accounts_reads_as_account_zero() {
        let mut s = Store::new();
        s.add_account(imap("ana@example.org")).unwrap();
        let mut v = serde_json::to_value(&s).unwrap();
        v["version"] = serde_json::json!(3);
        v.as_object_mut().unwrap().remove("next_account");
        v["accounts"][0].as_object_mut().unwrap().remove("id");
        for f in v["folders"].as_array_mut().unwrap() {
            f.as_object_mut().unwrap().remove("account");
        }
        let back = Store::from_json(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(back.version, FORMAT_VERSION);
        assert_eq!(back.accounts[0].id, AccountId(0));
        assert_eq!(back.folders().len(), 7);
        let mut back = back;
        assert_eq!(back.add_account(imap("bo@example.net")).unwrap(), AccountId(1));
    }
}
