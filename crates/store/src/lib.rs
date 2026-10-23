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
use ibercorreo_core::{
    Account, AddressBook, Calendar, CalendarRemote, Contact, Date, DateTime, Error, Event, EventId, EventRemote, Folder, FolderId, FolderRole,
    Message, MessageId, RemoteRef, Result, Zone, clamp_text,
};
use serde::{Deserialize, Serialize};

/// Largest store file read back: 1 GiB.
pub const MAX_STORE_BYTES: u64 = 1024 * 1024 * 1024;
/// Format version written into the file; a newer file is refused rather than misread.
/// 2: folders and messages remember their server copy (`remote`), and deleted synced messages
/// leave a tombstone until the server has deleted them too.
/// 3: contacts hold several phones (`phones`), a UID, title, note, birthday and unread vCard lines,
/// and live in address books (`books`), which may mirror CardDAV servers.
pub const FORMAT_VERSION: u32 = 3;

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
    /// The zone the calendar is shown in, chosen by the person; `None` follows the computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
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
}

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
        s.calendars.push(Calendar { id: 0, name: "Calendar".into(), color: "#1F7A8C".into(), remote: None });
        s.books.push(AddressBook { id: 0, name: "Contacts".into(), remote: None });
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
        // Stores from before address books: one local book holds every contact.
        if s.books.is_empty() {
            s.books.push(AddressBook { id: 0, name: "Contacts".into(), remote: None });
        }
        s.check()?;
        for c in &mut s.contacts {
            c.upgrade();
        }
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
}
