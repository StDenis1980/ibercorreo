//! Attachment files on disk (Tinta, for `save_attachment` in `ibercorreo-cli mcp`): the contents
//! of the attachments of mail that arrives, kept beside the store file (`store.json` →
//! `store.files/`), one file per content, named by its SHA-256 (64 lowercase hex digits). The same
//! file attached to several messages is kept once, and a file never changes once written, so a
//! reader (the MCP server while the window has the mailbox) can read one at any time.
//!
//! A file kept by [`crate::Store::keep_file`] stays in memory until the store is saved (a store
//! that is never saved, like the sample mailbox, keeps it there). A save writes the new ones first,
//! each through a temporary file, so a store file never names a file that is not on disk.
//!
//! Files no message names any more (the message was deleted, or the server dropped it) are removed
//! when the store is opened, once they are a day old: a file just written by another program for a
//! store file it has not written yet is left alone.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use ibercorreo_core::{Error, Result};
use sha2::Digest;

/// Largest attachment file kept, and read back: as large as a whole message may be.
pub const MAX_FILE_BYTES: usize = ibercorreo_mime::MAX_MESSAGE_BYTES;
/// How old a file no message names must be before it is removed.
const PRUNE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// The folder holding the attachment files of the store saved at `path`.
pub fn dir(path: &Path) -> PathBuf {
    path.with_extension("files")
}

/// The SHA-256 of `bytes`, as 64 lowercase hex digits: the file's name.
pub fn hash(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `name` is a file name this module writes (64 lowercase hex digits): anything else is
/// never read or removed.
pub fn is_hash(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn storage(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::Storage(format!("{}: {e}", path.display()))
}

/// Read the attachment file `hash` of the store saved at `store_path`. A file that is not there is
/// [`Error::NotFound`]; one whose contents do not match its name is refused.
pub fn read(store_path: &Path, hash: &str) -> Result<Vec<u8>> {
    if !is_hash(hash) {
        return Err(Error::invalid("attachment file", "not a SHA-256 name"));
    }
    let file = dir(store_path).join(hash);
    let meta = match std::fs::symlink_metadata(&file) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::not_found("attachment file", hash.to_string())),
        Err(e) => return Err(storage(&file, e)),
    };
    if !meta.is_file() {
        return Err(Error::invalid("attachment file", format!("{}: not a regular file", file.display())));
    }
    if meta.len() > MAX_FILE_BYTES as u64 {
        return Err(Error::TooLarge { what: "attachment file", size: usize::try_from(meta.len()).unwrap_or(usize::MAX), limit: MAX_FILE_BYTES });
    }
    let bytes = std::fs::read(&file).map_err(|e| storage(&file, e))?;
    if self::hash(&bytes) != hash {
        return Err(Error::invalid("attachment file", format!("{}: its contents changed on disk", file.display())));
    }
    Ok(bytes)
}

/// Attachment files kept and not saved yet. Shared by a store and its copies (an undo snapshot,
/// the copy a save writes on another thread); not part of the store's value.
#[derive(Clone, Default)]
pub struct Files(Arc<Mutex<HashMap<String, Arc<[u8]>>>>);

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Files")
    }
}

impl PartialEq for Files {
    fn eq(&self, _: &Files) -> bool {
        true
    }
}

impl Eq for Files {}

impl Files {
    /// A poisoned lock only means a save panicked while it held it: the map is still whole.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Arc<[u8]>>> {
        let guard = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.0.clear_poison();
        guard
    }

    /// Keep `bytes` until the next save; returns its name.
    pub(crate) fn keep(&self, bytes: Vec<u8>) -> String {
        let name = hash(&bytes);
        self.lock().entry(name.clone()).or_insert_with(|| Arc::from(bytes));
        name
    }

    /// The file `hash` if it is kept here, not saved yet.
    pub(crate) fn pending(&self, hash: &str) -> Option<Arc<[u8]>> {
        self.lock().get(hash).cloned()
    }

    /// Write the files not saved yet into `path`'s folder (each through a temporary file; one
    /// already there is left as it is), and the ones `wanted` that are only in `from`'s folder (a
    /// store saved somewhere new). Then they are no longer kept in memory.
    pub(crate) fn write(&self, path: &Path, from: Option<&Path>, wanted: &HashSet<&str>) -> Result<()> {
        let pending: Vec<(String, Arc<[u8]>)> = self.lock().iter().map(|(k, v)| (k.clone(), Arc::clone(v))).collect();
        let moved = from.is_some_and(|f| f != path);
        if pending.is_empty() && !moved {
            return Ok(());
        }
        let dir = dir(path);
        std::fs::create_dir_all(&dir).map_err(|e| storage(&dir, e))?;
        for (name, bytes) in &pending {
            let file = dir.join(name);
            if !file.exists() {
                crate::shards::write_file(&file, bytes)?;
            }
        }
        if let (true, Some(from)) = (moved, from) {
            for name in wanted {
                let file = dir.join(name);
                if file.exists() || pending.iter().any(|(n, _)| n == name) {
                    continue;
                }
                // One missing where the store was is left missing: its message says so when asked.
                if let Ok(bytes) = read(from, name) {
                    crate::shards::write_file(&file, &bytes)?;
                }
            }
        }
        let mut kept = self.lock();
        for (name, _) in pending {
            kept.remove(&name);
        }
        Ok(())
    }
}

/// Remove the files in `path`'s folder that are not `wanted` and are older than a day; returns how
/// many went. Errors are passed over: a file left behind costs room, nothing else.
pub(crate) fn prune(path: &Path, wanted: &HashSet<&str>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir(path)) else { return 0 };
    let now = SystemTime::now();
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_hash(name) || wanted.contains(name) {
            continue;
        }
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age >= PRUNE_AFTER));
        if old && std::fs::remove_file(entry.path()).is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sha256_hex_and_nothing_else() {
        let h = hash(b"hola");
        assert_eq!(h.len(), 64);
        assert!(is_hash(&h));
        for other in ["", "../x", &h.to_uppercase(), &h[..63], &format!("{h}.tmp"), &format!("{}g", &h[..63])] {
            assert!(!is_hash(other), "{other}");
        }
        assert_eq!(dir(Path::new("/x/store.json")), Path::new("/x/store.files"));
    }

    const MAIL: &str = "From: Ana <ana@example.org>\r\nTo: b@example.org\r\nSubject: Factura\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nAdjunto.\r\n--b\r\n\
Content-Type: application/pdf; name=factura.pdf\r\nContent-Disposition: attachment; filename=factura.pdf\r\n\
Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQgaG9sYQ==\r\n--b--\r\n";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tinta-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("store.json")
    }

    #[test]
    fn mail_that_arrives_keeps_its_attachments_until_saved_then_on_disk() {
        let path = temp("keep");
        let mut s = crate::Store::new();
        let inbox = s.folder_by_role(s.first_account(), ibercorreo_core::FolderRole::Inbox).unwrap();
        let id = s.import_eml(MAIL.as_bytes(), inbox).unwrap();
        let a = s.message(id).unwrap().attachments.first().cloned().unwrap();
        let h = a.sha256.clone().unwrap();
        assert_eq!(h, hash(b"%PDF-1.4 hola"));
        assert_eq!(s.file(&h).unwrap(), b"%PDF-1.4 hola", "kept in memory before any save");
        assert!(s.file(&hash(b"other")).is_err());
        s.save(&path).unwrap();
        assert_eq!(std::fs::read(dir(&path).join(&h)).unwrap(), b"%PDF-1.4 hola");
        assert!(s.files.pending(&h).is_none(), "written, so no longer held in memory");
        let back = crate::Store::load(&path).unwrap();
        assert_eq!(back.message(id).unwrap().attachments.first(), Some(&a));
        assert_eq!(back.file(&h).unwrap(), b"%PDF-1.4 hola");
        assert_eq!(read(&path, &h).unwrap(), b"%PDF-1.4 hola");
        assert!(read(&path, "../store.json").is_err());
        // Saved somewhere new, the files go along.
        let elsewhere = temp("keep-moved");
        back.save(&elsewhere).unwrap();
        assert_eq!(read(&elsewhere, &h).unwrap(), b"%PDF-1.4 hola");
    }

    #[test]
    fn files_no_message_names_go_once_a_day_old() {
        let path = temp("prune");
        let mut s = crate::Store::new();
        let inbox = s.folder_by_role(s.first_account(), ibercorreo_core::FolderRole::Inbox).unwrap();
        let id = s.import_eml(MAIL.as_bytes(), inbox).unwrap();
        let kept = hash(b"%PDF-1.4 hola");
        s.save(&path).unwrap();
        let (fresh, old) = (hash(b"fresh"), hash(b"old"));
        std::fs::write(dir(&path).join(&fresh), b"fresh").unwrap();
        let f = std::fs::File::options().write(true).create(true).truncate(true).open(dir(&path).join(&old)).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60)).unwrap();
        drop(f);
        std::fs::write(dir(&path).join("notes.txt"), b"mine").unwrap();
        let s = crate::Store::load(&path).unwrap();
        assert!(dir(&path).join(&kept).exists());
        assert!(dir(&path).join(&fresh).exists(), "too new to remove");
        assert!(!dir(&path).join(&old).exists(), "named by no message and old");
        assert!(dir(&path).join("notes.txt").exists(), "not ours");
        // Its message removed, the file stays until it is a day old.
        let mut s = s;
        s.remove_message(id).unwrap();
        s.save(&path).unwrap();
        crate::Store::load(&path).unwrap();
        assert!(dir(&path).join(&kept).exists());
    }
}
