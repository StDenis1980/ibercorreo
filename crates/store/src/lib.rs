//! The local store, L2: every folder, message, calendar, event and contact the app holds, and the
//! JSON file they are saved to. Servers (IMAP, CalDAV, EWS) sync into this store from M1 on; the
//! interface only ever reads it.
//!
//! Saving writes a temporary file and renames it over the old one, so a crash mid-save never
//! leaves a half-written store.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::Path;

use ibercorreo_core::calendar::When;
use ibercorreo_core::mail::Attachment;
use ibercorreo_core::{Account, Calendar, Contact, Date, Error, Event, EventId, Folder, FolderId, FolderRole, Message, MessageId, RemoteRef, Result};
use serde::{Deserialize, Serialize};

/// Largest store file read back: 1 GiB.
pub const MAX_STORE_BYTES: u64 = 1024 * 1024 * 1024;
/// Format version written into the file; a newer file is refused rather than misread.
/// 2: folders and messages remember their server copy (`remote`), and deleted synced messages
/// leave a tombstone until the server has deleted them too.
pub const FORMAT_VERSION: u32 = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    pub accounts: Vec<Account>,
    folders: Vec<Folder>,
    messages: Vec<Message>,
    calendars: Vec<Calendar>,
    events: Vec<Event>,
    contacts: Vec<Contact>,
    next_folder: u32,
    next_message: u64,
    next_event: u64,
    next_contact: u64,
    /// Synced messages deleted here for good, still to be deleted on the server.
    #[serde(default)]
    tombstones: Vec<RemoteRef>,
}

/// One occurrence of an event: the event, and when this occurrence happens (for a repeating
/// event, not the series' first date).
#[derive(Debug, Clone, Copy)]
pub struct Occurrence<'a> {
    pub event: &'a Event,
    pub when: When,
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
        for (name, role) in [
            ("Inbox", FolderRole::Inbox),
            ("Drafts", FolderRole::Drafts),
            ("Sent", FolderRole::Sent),
            ("Outbox", FolderRole::Outbox),
            ("Archive", FolderRole::Archive),
            ("Junk", FolderRole::Junk),
            ("Trash", FolderRole::Trash),
        ] {
            s.add_folder(name, role);
        }
        s.calendars.push(Calendar { id: 0, name: "Calendar".into(), color: "#1F7A8C".into() });
        s
    }

    // ---- folders ----

    pub fn add_folder(&mut self, name: &str, role: FolderRole) -> FolderId {
        let id = FolderId(self.next_folder);
        self.next_folder = self.next_folder.saturating_add(1);
        self.folders.push(Folder { id, name: name.to_string(), role, remote: None });
        id
    }

    pub fn folder_mut(&mut self, id: FolderId) -> Result<&mut Folder> {
        self.folders.iter_mut().find(|f| f.id == id).ok_or_else(|| Error::not_found("folder", id.0.to_string()))
    }

    /// The folder that mirrors server mailbox `path`.
    pub fn folder_by_path(&self, path: &str) -> Option<FolderId> {
        self.folders.iter().find(|f| f.remote.as_ref().is_some_and(|r| r.path == path)).map(|f| f.id)
    }

    /// Folders in the order the folder pane shows them.
    pub fn folders(&self) -> Vec<&Folder> {
        let mut v: Vec<&Folder> = self.folders.iter().collect();
        v.sort_by_key(|f| (f.role.order(), f.name.to_lowercase()));
        v
    }

    pub fn folder(&self, id: FolderId) -> Result<&Folder> {
        self.folders.iter().find(|f| f.id == id).ok_or_else(|| Error::not_found("folder", id.0.to_string()))
    }

    pub fn folder_by_role(&self, role: FolderRole) -> Result<FolderId> {
        self.folders.iter().find(|f| f.role == role).map(|f| f.id).ok_or_else(|| Error::not_found("folder", format!("{role:?}")))
    }

    pub fn unread_count(&self, folder: FolderId) -> usize {
        self.messages.iter().filter(|m| m.folder == folder && !m.seen).count()
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
        m.id = self.next_message_id();
        let id = m.id;
        self.messages.push(m);
        Ok(id)
    }

    /// Parse a raw `.eml` into `folder`.
    pub fn import_eml(&mut self, raw: &[u8], folder: FolderId) -> Result<MessageId> {
        self.folder(folder)?;
        let m = ibercorreo_mime::parse(raw, MessageId(0), folder)?;
        self.add_message(m)
    }

    /// Every message, in no particular order (for sync).
    pub fn all_messages(&self) -> &[Message] {
        &self.messages
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
        let pos = self.messages.iter().position(|m| m.id == id).ok_or_else(|| Error::not_found("message", id.0.to_string()))?;
        Ok(self.messages.remove(pos))
    }

    /// Messages in `folder`, newest first (undated last).
    pub fn messages_in(&self, folder: FolderId) -> Vec<&Message> {
        let mut v: Vec<&Message> = self.messages.iter().filter(|m| m.folder == folder).collect();
        v.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
        v
    }

    /// Messages whose subject, sender or body contains `query` (case-insensitive), newest first.
    pub fn search(&self, query: &str) -> Vec<&Message> {
        let q = query.to_lowercase();
        let mut v: Vec<&Message> = self
            .messages
            .iter()
            .filter(|m| {
                q.is_empty()
                    || m.subject.to_lowercase().contains(&q)
                    || m.body.to_lowercase().contains(&q)
                    || m.from.as_ref().is_some_and(|a| a.email.to_lowercase().contains(&q) || a.display().to_lowercase().contains(&q))
            })
            .collect();
        v.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
        v
    }

    pub fn message(&self, id: MessageId) -> Result<&Message> {
        self.messages.iter().find(|m| m.id == id).ok_or_else(|| Error::not_found("message", id.0.to_string()))
    }

    pub fn message_mut(&mut self, id: MessageId) -> Result<&mut Message> {
        self.messages.iter_mut().find(|m| m.id == id).ok_or_else(|| Error::not_found("message", id.0.to_string()))
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
        if self.messages.iter().any(|x| x.id == m.id) {
            return Err(Error::invalid("message", format!("id {} already present", m.id.0)));
        }
        self.next_message = self.next_message.max(m.id.0.saturating_add(1));
        if let Some(r) = m.remote {
            self.forget_tombstone(r.folder, r.uid);
        }
        self.messages.push(m);
        Ok(())
    }

    // ---- calendar ----

    pub fn calendars(&self) -> &[Calendar] {
        &self.calendars
    }

    pub fn calendar(&self, id: u32) -> Result<&Calendar> {
        self.calendars.iter().find(|c| c.id == id).ok_or_else(|| Error::not_found("calendar", id.to_string()))
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

    pub fn event_mut(&mut self, id: EventId) -> Result<&mut Event> {
        self.events.iter_mut().find(|e| e.id == id).ok_or_else(|| Error::invalid("event", format!("no event {}", id.0)))
    }

    pub fn remove_event(&mut self, id: EventId) -> Result<Event> {
        let pos = self.events.iter().position(|e| e.id == id).ok_or_else(|| Error::not_found("event", id.0.to_string()))?;
        Ok(self.events.remove(pos))
    }

    pub fn restore_event(&mut self, e: Event) -> Result<()> {
        self.calendar(e.calendar)?;
        if self.events.iter().any(|x| x.id == e.id) {
            return Err(Error::invalid("event", format!("id {} already present", e.id.0)));
        }
        self.next_event = self.next_event.max(e.id.0.saturating_add(1));
        self.events.push(e);
        Ok(())
    }

    /// Occurrences on `day`, all-day first, then by start time. A repeating event's occurrence
    /// that was changed on its own (`RECURRENCE-ID`) shows as the changed copy instead.
    pub fn events_on(&self, day: Date) -> Vec<Occurrence<'_>> {
        let mut v: Vec<Occurrence<'_>> = self
            .events
            .iter()
            .filter_map(|e| e.occurrence_on(day).map(|when| Occurrence { event: e, when }))
            .filter(|o| o.event.rrule.is_none() || o.event.recurrence_id.is_some() || !self.overridden(o.event, o.when.start_date()))
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
                *existing = ev.into_event(id, calendar);
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

    pub fn add_contact(&mut self, mut c: Contact) -> u64 {
        c.id = self.next_contact;
        self.next_contact = self.next_contact.saturating_add(1);
        let id = c.id;
        self.contacts.push(c);
        id
    }

    pub fn contacts(&self, query: &str) -> Vec<&Contact> {
        let q = query.to_lowercase();
        let mut v: Vec<&Contact> = self.contacts.iter().filter(|c| c.matches(&q)).collect();
        v.sort_by_key(|c| c.name.to_lowercase());
        v
    }

    // ---- disk ----

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| Error::Storage(e.to_string()))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json).map_err(|e| Error::Storage(format!("{}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path).map_err(|e| Error::Storage(format!("{}: {e}", path.display())))
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
        Store::from_json(&bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Store> {
        let mut s: Store = serde_json::from_slice(bytes).map_err(|e| Error::invalid("store file", e.to_string()))?;
        if s.version > FORMAT_VERSION {
            return Err(Error::invalid("store file", format!("format {} is newer than this build ({FORMAT_VERSION})", s.version)));
        }
        s.check()?;
        // Older files read as this format (new fields default) and are written back as it.
        s.version = FORMAT_VERSION;
        Ok(s)
    }

    /// Ids unique and every reference resolvable, so a hand-edited file can't confuse the app.
    fn check(&self) -> Result<()> {
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
    use ibercorreo_core::{Address, DateTime};

    fn msg(store: &Store, subject: &str, day: u8) -> Message {
        let mut m = Message::new(MessageId(0), store.folder_by_role(FolderRole::Inbox).unwrap());
        m.set_subject(subject);
        m.from = Some(Address::parse("Ana <ana@example.org>").unwrap());
        m.date = Some(DateTime::new(Date::new(2026, 10, day).unwrap(), 9, 0, 0).unwrap());
        m
    }

    #[test]
    fn messages_sort_newest_first_and_move() {
        let mut s = Store::new();
        let inbox = s.folder_by_role(FolderRole::Inbox).unwrap();
        let trash = s.folder_by_role(FolderRole::Trash).unwrap();
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
        assert_eq!(s.events_on(day).first().map(|o| o.event.summary.as_str()), Some("B"));
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
            s.events_on(Date::new(2026, 10, d).unwrap()).iter().map(|o| (o.event.summary.clone(), o.when.start_date().to_string())).collect()
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
        assert!(out.contains("EXDATE;VALUE=DATE:20261021,20261023") && out.contains("RECURRENCE-ID;VALUE=DATE:20261022"), "{out}");
    }

    #[test]
    fn saves_and_loads_and_rejects_bad_files() {
        let dir = std::env::temp_dir().join(format!("ibercorreo-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        let mut s = Store::new();
        s.add_message(msg(&s, "kept", 3)).unwrap();
        s.save(&path).unwrap();
        assert_eq!(Store::load(&path).unwrap(), s);
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
        let inbox = s.folder_by_role(FolderRole::Inbox).unwrap();
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
}
