//! Messages on disk: in shard files beside the store file (`store.json` → `store.mail/m000042.json`),
//! a thousand message ids to a shard, so a save rewrites only the shards whose messages changed,
//! and no one file grows with the mailbox.
//!
//! Which shards changed is found without a flag on every message: the store shares its messages
//! (`Arc`) with the copy a save writes, so the shards written last time are kept as the `Arc`s they
//! held, and a shard is written again only when one of its messages is no longer the same
//! allocation (it changed, as [`crate::Store::message_mut`] copies on write) or the set changed.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use ibercorreo_core::{Error, Message, MessageId, Result};

/// Message ids per shard.
pub const IDS_PER_SHARD: u64 = 1000;
/// Largest shard file read back: 256 MiB (a thousand messages of 256 KiB each).
pub const MAX_SHARD_BYTES: u64 = 256 * 1024 * 1024;

/// The folder holding the shards of the store saved at `path`.
pub fn dir(path: &Path) -> PathBuf {
    path.with_extension("mail")
}

fn file_name(shard: u64) -> String {
    format!("m{shard:06}.json")
}

/// The shard a file name is, if it is one (`m000042.json`).
fn parse_name(name: &str) -> Option<u64> {
    let digits = name.strip_prefix('m')?.strip_suffix(".json")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn shard_of(id: MessageId) -> u64 {
    id.0 / IDS_PER_SHARD
}

/// What the last save wrote: where, and each shard's messages. Shared by a store and its copies
/// (the one a save writes runs on another thread); not part of the store's value.
#[derive(Clone, Default)]
pub struct Saved(Arc<Mutex<Option<Written>>>);

struct Written {
    path: PathBuf,
    shards: HashMap<u64, Vec<Arc<Message>>>,
}

impl std::fmt::Debug for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Saved")
    }
}

impl PartialEq for Saved {
    fn eq(&self, _: &Saved) -> bool {
        true
    }
}

impl Eq for Saved {}

impl Saved {
    /// A poisoned lock means a save panicked half way: forget what it wrote, so the next one
    /// writes every shard.
    fn lock(&self) -> MutexGuard<'_, Option<Written>> {
        let mut guard = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.0.is_poisoned() {
            *guard = None;
            self.0.clear_poison();
        }
        guard
    }

    /// The shards just read from `path`'s folder are what is on disk.
    pub(crate) fn remember(&self, path: &Path, messages: &[Arc<Message>]) {
        *self.lock() = Some(Written { path: path.to_path_buf(), shards: group(messages) });
    }
}

fn group(messages: &[Arc<Message>]) -> HashMap<u64, Vec<Arc<Message>>> {
    let mut shards: HashMap<u64, Vec<Arc<Message>>> = HashMap::new();
    for m in messages {
        shards.entry(shard_of(m.id)).or_default().push(Arc::clone(m));
    }
    shards
}

fn same(a: &[Arc<Message>], b: &[Arc<Message>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| Arc::ptr_eq(x, y))
}

fn storage(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::Storage(format!("{}: {e}", path.display()))
}

/// Write `bytes` to `path` through a temporary file, so a crash never leaves half a file.
pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| storage(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| storage(path, e))
}

/// Write the shards of `messages` that changed since the last save to `path` (all of them when
/// it went elsewhere). The caller writes the store file after, then calls [`finish`].
pub(crate) fn write(saved: &Saved, path: &Path, messages: &[Arc<Message>]) -> Result<Pending> {
    let dir = dir(path);
    std::fs::create_dir_all(&dir).map_err(|e| storage(&dir, e))?;
    let shards = group(messages);
    let last = saved.lock();
    let before = last.as_ref().filter(|w| w.path == path).map(|w| &w.shards);
    for (shard, list) in &shards {
        if before.and_then(|b| b.get(shard)).is_some_and(|old| same(old, list)) {
            continue;
        }
        let refs: Vec<&Message> = list.iter().map(|m| &**m).collect();
        let json = serde_json::to_vec(&refs).map_err(|e| Error::Storage(e.to_string()))?;
        write_file(&dir.join(file_name(*shard)), &json)?;
    }
    // Shards on disk the store no longer has: removed once the store file no longer counts on
    // them, or a later load would read them back. Saved here before: the ones it wrote then;
    // saved elsewhere (or never): whatever shard files the folder holds.
    let gone: Vec<u64> = match before {
        Some(b) => b.keys().filter(|k| !shards.contains_key(k)).copied().collect(),
        None => std::fs::read_dir(&dir)
            .map_err(|e| storage(&dir, e))?
            .filter_map(|e| e.ok()?.file_name().to_str().and_then(parse_name))
            .filter(|k| !shards.contains_key(k))
            .collect(),
    };
    Ok(Pending { dir, shards, gone })
}

/// Shards written, the store file still to write.
pub(crate) struct Pending {
    dir: PathBuf,
    shards: HashMap<u64, Vec<Arc<Message>>>,
    gone: Vec<u64>,
}

/// The store file is written: drop the shards it no longer has and remember what is on disk.
pub(crate) fn finish(saved: &Saved, path: &Path, pending: Pending) {
    for shard in &pending.gone {
        // Left behind, a stale shard is only read for ids the store no longer has; harmless.
        let _ = std::fs::remove_file(pending.dir.join(file_name(*shard)));
    }
    *saved.lock() = Some(Written { path: path.to_path_buf(), shards: pending.shards });
}

/// Every message in the shards beside `path`, in id order. A shard that cannot be read fails the
/// load (the caller keeps the files aside rather than lose them).
pub(crate) fn read(path: &Path) -> Result<Vec<Message>> {
    let dir = dir(path);
    let entries = std::fs::read_dir(&dir).map_err(|e| storage(&dir, e))?;
    let mut files = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|e| storage(&dir, e))?;
        if let Some(shard) = entry.file_name().to_str().and_then(parse_name) {
            files.insert(shard, entry.path());
        }
    }
    let mut out = Vec::new();
    for (shard, file) in files {
        let meta = std::fs::metadata(&file).map_err(|e| storage(&file, e))?;
        if meta.len() > MAX_SHARD_BYTES {
            return Err(Error::TooLarge {
                what: "message shard",
                size: usize::try_from(meta.len()).unwrap_or(usize::MAX),
                limit: usize::try_from(MAX_SHARD_BYTES).unwrap_or(usize::MAX),
            });
        }
        let bytes = std::fs::read(&file).map_err(|e| storage(&file, e))?;
        let list: Vec<Message> = serde_json::from_slice(&bytes).map_err(|e| Error::invalid("message shard", format!("{}: {e}", file.display())))?;
        // A message filed under the wrong shard (a hand-edited file) would be written to another
        // one and leave this copy behind: it is refused.
        if let Some(m) = list.iter().find(|m| shard_of(m.id) != shard) {
            return Err(Error::invalid("message shard", format!("{}: message {} belongs in another shard", file.display(), m.id.0)));
        }
        out.extend(list);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_names_round_trip_and_reject_others() {
        assert_eq!(file_name(42), "m000042.json");
        assert_eq!(parse_name("m000042.json"), Some(42));
        assert_eq!(parse_name(&file_name(1_234_567)), Some(1_234_567));
        for other in ["m.json", "x000042.json", "m00004a.json", "m000042.tmp", "m-1.json", "store.json"] {
            assert_eq!(parse_name(other), None, "{other}");
        }
        assert_eq!(shard_of(MessageId(999)), 0);
        assert_eq!(shard_of(MessageId(1000)), 1);
        assert_eq!(dir(Path::new("/x/store.json")), Path::new("/x/store.mail"));
    }
}
