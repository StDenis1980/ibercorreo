//! Taking everything out (M5.3): the mail, calendars and contacts of the store written to a new
//! folder in formats other programs read, so the person can move to another mail program or keep
//! a copy.
//!
//! ```text
//! Tinta Correo 2026-11-12/
//!   Mail/<account>/<folder>.mbox     one mbox per folder (mboxrd), oldest message first
//!   Calendars/<calendar>.ics         one iCalendar file per calendar
//!   Contacts/<address book>.vcf      one vCard file per address book
//! ```
//!
//! Thunderbird (ImportExportTools NG), Apple Mail and Evolution import the mbox files; Outlook
//! imports the `.ics` and `.vcf` files, and a message dragged out of Correo as `.eml`. Attachment
//! files are not kept by the store, so they are not written. Writing files is not a command: only
//! the window and the command-line tool, which the person drives, call [`write_all`]. The
//! commands return the text instead (`folder.exportMbox`, `calendar.exportIcs`,
//! `contact.exportVcf`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ibercorreo_core::{Error, FolderId, Result};
use ibercorreo_store::Store;

/// What [`write_all`] wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The folder made for the export.
    pub dir: PathBuf,
    pub mailboxes: usize,
    pub messages: usize,
    pub calendars: usize,
    pub events: usize,
    pub books: usize,
    pub contacts: usize,
    /// Messages whose text could not be read (left out).
    pub failed: usize,
}

/// Longest file or folder name written, in characters; Windows allows 255, and a path has room
/// for the folders above it.
const MAX_NAME_CHARS: usize = 80;
/// How many `Name (2)`, `Name (3)`… are tried before giving up on a free name.
const MAX_TRIES: u32 = 10_000;

/// The mbox text of `folder`, oldest message first, and how many messages it holds; messages
/// whose text cannot be read are left out and counted as the third value.
pub fn mbox(store: &Store, folder: FolderId) -> Result<(String, usize, usize)> {
    mbox_within(store, folder, MAX_MBOX_TEXT)
}

/// Largest mbox returned as text: a bigger folder goes to a file through [`write_all`].
const MAX_MBOX_TEXT: usize = 256 * 1024 * 1024;

fn mbox_within(store: &Store, folder: FolderId, limit: usize) -> Result<(String, usize, usize)> {
    store.folder(folder)?;
    let mut out = String::new();
    let (mut written, mut failed) = (0usize, 0usize);
    for m in oldest_first(store, folder) {
        match store.content(m.id) {
            Ok(c) => {
                ibercorreo_mime::archive::push_mbox(&mut out, m, &c);
                if out.len() > limit {
                    return Err(Error::TooLarge { what: "folder as mbox text (export to a folder instead)", size: out.len(), limit });
                }
                written = written.saturating_add(1);
            }
            Err(_) => failed = failed.saturating_add(1),
        }
    }
    Ok((out, written, failed))
}

/// Writes everything in `store` to a new folder `Tinta Correo <today>` inside `parent` (made if
/// missing; `Tinta Correo <today> (2)` when the name is taken), never into or over files that
/// were there. `progress` is told how many messages were written out of how many there are.
pub fn write_all(store: &Store, parent: &Path, today: &str, mut progress: impl FnMut(usize, usize)) -> Result<Report> {
    fs::create_dir_all(parent).map_err(|e| io("folder", parent, e))?;
    let dir = fresh(parent, &format!("Tinta Correo {}", safe_name(today)), "", |p| fs::create_dir(p))?;
    let mut report = Report { dir: dir.clone(), ..Report::default() };
    let total = store.all_messages().len();
    let mut done = 0usize;

    let mail = dir.join("Mail");
    for folder in store.folders() {
        let messages = oldest_first(store, folder.id);
        if messages.is_empty() {
            continue;
        }
        let account = store.account(folder.account).map(|a| a.name.clone()).unwrap_or_default();
        let mut path = mail.join(safe_name(&account));
        let mut parts: Vec<String> = folder.name.split('/').map(safe_name).collect();
        let leaf = parts.pop().unwrap_or_default();
        for p in parts {
            path.push(p);
        }
        fs::create_dir_all(&path).map_err(|e| io("folder", &path, e))?;
        let (file, path) = new_file(&path, &leaf, ".mbox")?;
        let mut file = std::io::BufWriter::new(file);
        let mut one = String::new();
        for m in messages {
            done = done.saturating_add(1);
            progress(done, total);
            match store.content(m.id) {
                Ok(c) => {
                    one.clear();
                    ibercorreo_mime::archive::push_mbox(&mut one, m, &c);
                    file.write_all(one.as_bytes()).map_err(|e| io("file", &path, e))?;
                    report.messages = report.messages.saturating_add(1);
                }
                Err(_) => report.failed = report.failed.saturating_add(1),
            }
        }
        file.flush().map_err(|e| io("file", &path, e))?;
        report.mailboxes = report.mailboxes.saturating_add(1);
    }

    let calendars = dir.join("Calendars");
    for c in store.calendars() {
        let events = store.events().iter().filter(|e| e.calendar == c.id).count();
        if events == 0 {
            continue;
        }
        fs::create_dir_all(&calendars).map_err(|e| io("folder", &calendars, e))?;
        let (mut file, path) = new_file(&calendars, &safe_name(&c.name), ".ics")?;
        file.write_all(store.export_ics(Some(c.id)).as_bytes()).map_err(|e| io("file", &path, e))?;
        report.calendars = report.calendars.saturating_add(1);
        report.events = report.events.saturating_add(events);
    }

    let contacts = dir.join("Contacts");
    for b in store.books() {
        let ids: Vec<u64> = store.all_contacts().iter().filter(|c| c.book == b.id).map(|c| c.id).collect();
        if ids.is_empty() {
            continue;
        }
        fs::create_dir_all(&contacts).map_err(|e| io("folder", &contacts, e))?;
        let (mut file, path) = new_file(&contacts, &safe_name(&b.name), ".vcf")?;
        file.write_all(store.export_vcf(&ids).as_bytes()).map_err(|e| io("file", &path, e))?;
        report.books = report.books.saturating_add(1);
        report.contacts = report.contacts.saturating_add(ids.len());
    }
    Ok(report)
}

/// The messages of `folder` in the order an mbox keeps them: oldest first.
fn oldest_first(store: &Store, folder: FolderId) -> Vec<&ibercorreo_core::Message> {
    let mut v = store.messages_in(folder);
    v.reverse();
    v
}

/// A file `<name><ext>` made in `dir`, or `<name> (2)<ext>`… when two folders have the same name.
fn new_file(dir: &Path, name: &str, ext: &str) -> Result<(fs::File, PathBuf)> {
    let mut made = None;
    let path = fresh(dir, name, ext, |p| {
        made = Some(fs::OpenOptions::new().write(true).create_new(true).open(p)?);
        Ok(())
    })?;
    made.map(|f| (f, path.clone())).ok_or_else(|| Error::Storage(format!("{}: not made", path.display())))
}

/// The first of `dir/<name><ext>`, `dir/<name> (2)<ext>`… that `make` creates; `make` must fail
/// with `AlreadyExists` for a taken name, so nothing there is ever overwritten.
fn fresh(dir: &Path, name: &str, ext: &str, mut make: impl FnMut(&Path) -> std::io::Result<()>) -> Result<PathBuf> {
    for n in 1..=MAX_TRIES {
        let file = if n == 1 { format!("{name}{ext}") } else { format!("{name} ({n}){ext}") };
        let path = dir.join(file);
        match make(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io("file", &path, e)),
        }
    }
    Err(Error::Storage(format!("{}: no free name for {name}{ext}", dir.display())))
}

fn io(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::Storage(format!("{what} {}: {e}", path.display()))
}

/// `name` as a file name every system accepts: the characters Windows refuses and control
/// characters become `_`, leading and trailing dots and spaces go, a name Windows reserves
/// (`CON`, `NUL`, `COM1`…) gets a `_` in front, and an empty one becomes `_`. At most
/// [`MAX_NAME_CHARS`] characters.
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .take(MAX_NAME_CHARS)
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        return "_".into();
    }
    let stem = trimmed.split('.').next().unwrap_or(trimmed).trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.ends_with(|c: char| c.is_ascii_digit()));
    if reserved { format!("_{trimmed}") } else { trimmed.to_string() }
}

#[cfg(test)]
mod tests;
