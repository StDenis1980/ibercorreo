//! Importing Outlook data files (`.pst`).
//!
//! An archive can hold gigabytes, so the import has two steps, like a sync: [`read`] goes through
//! the file (off the interface thread, reporting progress) and returns a [`Batch`] of folders and
//! messages; [`crate::Session::apply_pst`] puts them in the store. Each mail folder of the archive
//! becomes a local folder named by its path under the archive's name (`Archive/Inbox/Projects`).
//! Calendar, contact, task and note folders are counted, not imported yet.

use std::collections::HashSet;

use ibercorreo_core::{AccountId, FolderId, FolderRole, Message, MessageId, Result};
pub use ibercorreo_outlook::pst::{FileSource, Pst, Source};

/// What [`read`] found.
#[derive(Debug, Clone, Default)]
pub struct Batch {
    /// The archive's name (`Outlook Data File`, `Archive`…), else empty.
    pub archive: String,
    /// Each mail folder's path and its messages.
    pub folders: Vec<(Vec<String>, Vec<Message>)>,
    /// Items that are not mail (appointments, contacts, tasks, notes), left for later.
    pub skipped: usize,
    /// Items that could not be read.
    pub failed: usize,
}

/// What [`crate::Session::apply_pst`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub folders: usize,
    pub messages: usize,
    /// Messages already in the folder (the archive imported before).
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
        if !f.holds_mail() {
            batch.skipped = batch.skipped.saturating_add(ids.len());
            done = done.saturating_add(ids.len());
            progress(done, total.max(done));
            continue;
        }
        let mut messages = Vec::new();
        for nid in ids {
            match pst.message(nid, MessageId(0), FolderId(0)) {
                Ok(Some(m)) => messages.push(m),
                Ok(None) => batch.skipped += 1,
                Err(_) => batch.failed += 1,
            }
            done = done.saturating_add(1);
            if done.is_multiple_of(64) {
                progress(done, total.max(done));
            }
        }
        batch.folders.push((f.path.clone(), messages));
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
        if report.duplicates > 0 {
            self.status.push_str(&format!(", {} already here", report.duplicates));
        }
        if report.skipped > 0 {
            self.status.push_str(&format!(", {} calendar, contact or other items left out", report.skipped));
        }
        if report.failed > 0 {
            self.status.push_str(&format!(", {} could not be read", report.failed));
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests;
