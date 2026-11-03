//! Importing Outlook data files (`.pst`).
//!
//! An archive can hold gigabytes, so the import has two steps, like a sync: [`read`] goes through
//! the file (off the interface thread, reporting progress) and returns a [`Batch`] of folders and
//! messages; [`crate::Session::apply_pst`] puts them in the store. Each mail folder of the archive
//! becomes a local folder named by its path under the archive's name (`Archive/Inbox/Projects`),
//! each calendar a local calendar and each contacts folder a local address book named the same
//! way. Task, note and journal folders are counted, not imported.

use std::collections::HashSet;

use ibercorreo_core::{AccountId, Contact, Event, FolderId, FolderRole, Message, MessageId, Result};
pub use ibercorreo_outlook::pst::{FileSource, Pst, Source};

/// What [`read`] found.
#[derive(Debug, Clone, Default)]
pub struct Batch {
    /// The archive's name (`Outlook Data File`, `Archive`…), else empty.
    pub archive: String,
    /// Each mail folder's path and its messages.
    pub folders: Vec<(Vec<String>, Vec<Message>)>,
    /// Each calendar's path and its events (a series is followed by its changed occurrences).
    pub calendars: Vec<(Vec<String>, Vec<Event>)>,
    /// Each contacts folder's path and its contacts.
    pub books: Vec<(Vec<String>, Vec<Contact>)>,
    /// Items left out: tasks, notes, journal entries, contact groups, and items in a folder of
    /// another kind.
    pub skipped: usize,
    /// Items that could not be read.
    pub failed: usize,
}

/// What [`crate::Session::apply_pst`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub folders: usize,
    pub messages: usize,
    pub calendars: usize,
    pub events: usize,
    pub books: usize,
    pub contacts: usize,
    /// Messages, events and contacts already there (the archive imported before).
    pub duplicates: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// Reads every mail folder of `pst`. `progress` is told how many items were read out of how many
/// the folders say they hold.
pub fn read<S: Source>(pst: &Pst<S>, mut progress: impl FnMut(usize, usize)) -> Result<Batch> {
    let folders = pst.folders()?;
    let total: usize = folders.iter().map(|f| f.count as usize).sum();
    let mut batch = Batch { archive: pst.name(), ..Batch::default() };
    let mut done = 0usize;
    for f in &folders {
        let ids = match pst.messages(f) {
            Ok(ids) => ids,
            Err(_) => {
                batch.failed = batch.failed.saturating_add(f.count as usize);
                continue;
            }
        };
        if !(f.holds_mail() || f.holds_calendar() || f.holds_contacts()) {
            batch.skipped = batch.skipped.saturating_add(ids.len());
            done = done.saturating_add(ids.len());
            progress(done, total.max(done));
            continue;
        }
        let (mut messages, mut events, mut contacts) = (Vec::new(), Vec::new(), Vec::new());
        for nid in ids {
            let found = if f.holds_calendar() {
                pst.appointment(nid).map(|e| e.map(|e| events.extend(e)))
            } else if f.holds_contacts() {
                pst.contact(nid).map(|c| c.map(|c| contacts.push(c)))
            } else {
                pst.message(nid, MessageId(0), FolderId(0)).map(|m| m.map(|m| messages.push(m)))
            };
            match found {
                Ok(Some(())) => {}
                Ok(None) => batch.skipped += 1,
                Err(_) => batch.failed += 1,
            }
            done = done.saturating_add(1);
            if done.is_multiple_of(64) {
                progress(done, total.max(done));
            }
        }
        if f.holds_calendar() {
            batch.calendars.push((f.path.clone(), events));
        } else if f.holds_contacts() {
            batch.books.push((f.path.clone(), contacts));
        } else {
            batch.folders.push((f.path.clone(), messages));
        }
    }
    progress(done, total.max(done));
    Ok(batch)
}

/// The local folder name for an archive folder.
pub(crate) fn folder_name(archive: &str, path: &[String]) -> String {
    let archive = if archive.trim().is_empty() { "Outlook" } else { archive.trim() };
    let mut parts = vec![archive.to_string()];
    parts.extend(path.iter().map(|p| if p.trim().is_empty() { "?".to_string() } else { p.trim().replace('/', "∕") }));
    ibercorreo_core::clamp_text(&parts.join("/"), 1024).to_string()
}

/// What tells two copies of one message apart from two messages: who sent it, when, and its
/// subject. Importing the same archive again adds nothing.
fn key(m: &Message) -> (Option<String>, Option<i64>, String) {
    (m.from.as_ref().map(|a| a.email.to_lowercase()), m.date.map(|d| d.to_unix()), m.subject.clone())
}

impl crate::Session {
    /// Put an archive [`read`] in the store, in local folders of `account` (the current one when
    /// `None`) named after the archive's folders. A folder imported before is reused, and messages
    /// already in it are left out. Undo history is cleared, as for a sync: undoing thousands of
    /// messages one step at a time helps nobody.
    pub fn apply_pst(&mut self, account: Option<AccountId>, batch: Batch) -> Result<Report> {
        let account = account.unwrap_or_else(|| self.current_account());
        if !self.store.accounts.is_empty() {
            self.store.account(account)?;
        }
        let mut report = Report { skipped: batch.skipped, failed: batch.failed, ..Report::default() };
        self.apply_pst_calendars(&batch.archive, batch.calendars, &mut report);
        self.apply_pst_books(&batch.archive, batch.books, &mut report);
        let mut first = None;
        for (path, messages) in batch.folders {
            if messages.is_empty() {
                continue;
            }
            let name = folder_name(&batch.archive, &path);
            let existing = self.store.folders_of(account).into_iter().find(|f| f.remote.is_none() && f.name == name).map(|f| f.id);
            let folder = match existing {
                Some(f) => f,
                None => {
                    report.folders = report.folders.saturating_add(1);
                    self.store.add_folder(account, &name, FolderRole::Other)
                }
            };
            first.get_or_insert(folder);
            let mut held: HashSet<_> = self.store.messages_in(folder).into_iter().map(key).collect();
            for mut m in messages {
                if !held.insert(key(&m)) {
                    report.duplicates = report.duplicates.saturating_add(1);
                    continue;
                }
                m.folder = folder;
                self.store.add_message(m)?;
                report.messages = report.messages.saturating_add(1);
            }
        }
        self.undo.clear();
        self.redo.clear();
        self.dirty = true;
        if let Some(f) = first {
            self.folder = f;
            self.message = None;
        }
        self.fix_selection();
        self.status = format!("Imported {} messages into {} new folders", report.messages, report.folders);
        if report.events > 0 || report.calendars > 0 {
            self.status.push_str(&format!(", {} events into {} new calendars", report.events, report.calendars));
        }
        if report.contacts > 0 || report.books > 0 {
            self.status.push_str(&format!(", {} contacts into {} new address books", report.contacts, report.books));
        }
        if report.duplicates > 0 {
            self.status.push_str(&format!(", {} already here", report.duplicates));
        }
        if report.skipped > 0 {
            self.status.push_str(&format!(", {} tasks, notes or other items left out", report.skipped));
        }
        if report.failed > 0 {
            self.status.push_str(&format!(", {} could not be read", report.failed));
        }
        Ok(report)
    }

    /// Each calendar of the archive into a local calendar of the same name (made, or reused when
    /// imported before); events already there (same UID and occurrence) are left out.
    fn apply_pst_calendars(&mut self, archive: &str, calendars: Vec<(Vec<String>, Vec<Event>)>, report: &mut Report) {
        for (path, events) in calendars {
            if events.is_empty() {
                continue;
            }
            let name = folder_name(archive, &path);
            let existing = self.store.calendars().iter().find(|c| c.remote.is_none() && c.name == name).map(|c| c.id);
            let calendar = match existing {
                Some(c) => c,
                None => match self.store.add_calendar(&name, PST_COLOR, None) {
                    Ok(c) => {
                        report.calendars = report.calendars.saturating_add(1);
                        c
                    }
                    Err(_) => {
                        report.failed = report.failed.saturating_add(events.len());
                        continue;
                    }
                },
            };
            let mut held: HashSet<(String, Option<ibercorreo_core::Date>)> =
                self.store.events().iter().filter(|e| e.calendar == calendar).map(|e| (e.uid.clone(), e.recurrence_id)).collect();
            for mut e in events {
                if e.uid.is_empty() {
                    e.uid = made_uid(&e);
                }
                if !held.insert((e.uid.clone(), e.recurrence_id)) {
                    report.duplicates = report.duplicates.saturating_add(1);
                    continue;
                }
                e.calendar = calendar;
                match self.store.add_event(e) {
                    Ok(_) => report.events = report.events.saturating_add(1),
                    Err(_) => report.failed = report.failed.saturating_add(1),
                }
            }
        }
    }

    /// Each contacts folder of the archive into a local address book of the same name; contacts
    /// already there (same name and first address) are left out.
    fn apply_pst_books(&mut self, archive: &str, books: Vec<(Vec<String>, Vec<Contact>)>, report: &mut Report) {
        for (path, contacts) in books {
            if contacts.is_empty() {
                continue;
            }
            let name = folder_name(archive, &path);
            let existing = self.store.books().iter().find(|b| b.remote.is_none() && b.name == name).map(|b| b.id);
            let book = match existing {
                Some(b) => b,
                None => match self.store.add_book(&name, None) {
                    Ok(b) => {
                        report.books = report.books.saturating_add(1);
                        b
                    }
                    Err(_) => {
                        report.failed = report.failed.saturating_add(contacts.len());
                        continue;
                    }
                },
            };
            let mut held: HashSet<(String, Option<String>)> = self.store.all_contacts().iter().filter(|c| c.book == book).map(contact_key).collect();
            for mut c in contacts {
                if !held.insert(contact_key(&c)) {
                    report.duplicates = report.duplicates.saturating_add(1);
                    continue;
                }
                c.book = book;
                self.store.add_contact(c);
                report.contacts = report.contacts.saturating_add(1);
            }
        }
    }
}

/// The colour of calendars imported from Outlook.
const PST_COLOR: &str = "#5b7fa6";

fn contact_key(c: &Contact) -> (String, Option<String>) {
    (c.name.to_lowercase(), c.emails.first().map(|a| a.email.to_lowercase()))
}

/// A UID for an appointment that had no meeting id: the same one each time the archive is
/// imported, so a second import finds it.
fn made_uid(e: &Event) -> String {
    // FNV-1a: unlike the standard hasher, the same in every version of the app.
    let text = format!("{}\u{0}{}\u{0}{:?}\u{0}{}", e.summary, e.location, e.when, e.rrule.as_deref().unwrap_or(""));
    let h = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("outlook-{h:016x}")
}

#[cfg(test)]
mod tests;
