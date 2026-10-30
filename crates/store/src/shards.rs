//! Messages on disk: in shard files beside the store file (`store.json` → `store.mail/`), a
//! thousand message ids to a shard, so a save rewrites only the shards whose messages changed,
//! and no one file grows with the mailbox. Each shard is two files: the headers
//! (`m000042.json`, every message without its content) and the content (`c000042.json`, an
//! object from id to body, formatted body and invitation). A store reads only the headers when it
//! opens and reads a message's content when it is shown, so a mailbox of tens of thousands of
//! messages does not sit in memory.
//!
//! Which shards changed is found without a flag on every message: the store shares its messages
//! (`Arc`) with the copy a save writes, so the shards written last time are kept as the `Arc`s they
//! held, and a shard is written again only when one of its messages is no longer the same
//! allocation (it changed, as [`crate::Store::message_mut`] copies on write) or the set changed.
//! Its content file is written again only when a message brought content of its own (new mail, an
//! edited draft).
//!
//! A content file keeps the content of messages removed since the store was opened, so undo can
//! bring one back; the content of messages removed before that is dropped the next time the file
//! is written. Reading the content files for the search index finds such files.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use ibercorreo_core::{Content, Error, Message, MessageId, Result};
use serde_json::value::RawValue;

/// Message ids per shard.
pub const IDS_PER_SHARD: u64 = 1000;
/// Largest shard file read back: 256 MiB (a thousand messages of 256 KiB each).
pub const MAX_SHARD_BYTES: u64 = 256 * 1024 * 1024;
/// Content read back lately and kept, so going back to a message reads no file.
const RECENT: usize = 16;

/// The folder holding the shards of the store saved at `path`.
pub fn dir(path: &Path) -> PathBuf {
    path.with_extension("mail")
}

fn file_name(shard: u64) -> String {
    format!("m{shard:06}.json")
}

fn content_name(shard: u64) -> String {
    format!("c{shard:06}.json")
}

/// The shard a file name is, if it is one (`m000042.json`; `c000042.json` with `kind` `'c'`).
fn parse_kind(name: &str, kind: char) -> Option<u64> {
    let digits = name.strip_prefix(kind)?.strip_suffix(".json")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn parse_name(name: &str) -> Option<u64> {
    parse_kind(name, 'm')
}

fn shard_of(id: MessageId) -> u64 {
    id.0 / IDS_PER_SHARD
}

/// What the last save wrote and where content left on disk is. Shared by a store and its copies
/// (the one a save writes runs on another thread, the search index reads content on another);
/// not part of the store's value.
#[derive(Clone, Default)]
pub struct Saved(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    /// The store file whose shard folder holds the content of messages that have none in memory.
    home: Option<PathBuf>,
    /// Messages whose content the content files keep: every one read or saved since the store
    /// was opened, so one removed and put back by undo still has its content.
    known: HashSet<MessageId>,
    /// Shards whose content file holds messages that are not known: written again (or removed)
    /// by the next save.
    stale: BTreeSet<u64>,
    written: Option<Written>,
    recent: VecDeque<(MessageId, Arc<Content>)>,
}

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
    /// writes every shard. Where content is stays known.
    fn lock(&self) -> MutexGuard<'_, State> {
        let mut guard = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.0.is_poisoned() {
            guard.written = None;
            self.0.clear_poison();
        }
        guard
    }

    /// The shards just read from `path`'s folder are what is on disk; with `headers_only`, the
    /// messages' content is in its content files (a store from before keeps it in the headers,
    /// and every shard is written again).
    pub(crate) fn remember(&self, path: &Path, messages: &[Arc<Message>], headers_only: bool) {
        let mut state = self.lock();
        state.home = Some(path.to_path_buf());
        state.known = messages.iter().map(|m| m.id).collect();
        state.written = headers_only.then(|| Written { path: path.to_path_buf(), shards: group(messages) });
    }

    /// The content of message `id`, read from its content file: `None` when it has none there.
    pub(crate) fn content(&self, id: MessageId) -> Result<Option<Arc<Content>>> {
        let home = {
            let state = self.lock();
            if let Some((_, c)) = state.recent.iter().find(|(at, _)| *at == id) {
                return Ok(Some(Arc::clone(c)));
            }
            state.home.clone()
        };
        let Some(home) = home else { return Ok(None) };
        let file = dir(&home).join(content_name(shard_of(id)));
        let Some(raw) = read_raw(&file)?.remove(&id.0) else { return Ok(None) };
        let content: Content = serde_json::from_str(raw.get()).map_err(|e| Error::invalid("message content", format!("{}: {e}", file.display())))?;
        let content = Arc::new(content);
        let mut state = self.lock();
        if state.recent.len() >= RECENT {
            state.recent.pop_front();
        }
        state.recent.push_back((id, Arc::clone(&content)));
        Ok(Some(content))
    }

    /// A reader of content files one shard at a time, for going through every message in id
    /// order (the search index).
    pub(crate) fn reader(&self) -> Reader {
        Reader { saved: self.clone(), shard: None }
    }
}

/// Content files read one shard at a time: see [`Saved::reader`].
pub(crate) struct Reader {
    saved: Saved,
    shard: Option<(u64, HashMap<u64, Arc<Content>>)>,
}

impl Reader {
    /// `m`'s content: its own, or the one in its content file. `None` when it has none anywhere
    /// (a file that could not be read counts as none: the message is still listed).
    pub(crate) fn get(&mut self, m: &Message) -> Option<Arc<Content>> {
        if let Some(c) = m.shared_content() {
            return Some(Arc::clone(c));
        }
        let shard = shard_of(m.id);
        if self.shard.as_ref().is_none_or(|(at, _)| *at != shard) {
            self.shard = Some((shard, self.read(shard)));
        }
        self.shard.as_ref().and_then(|(_, list)| list.get(&m.id.0)).cloned()
    }

    fn read(&self, shard: u64) -> HashMap<u64, Arc<Content>> {
        let Some(home) = self.saved.lock().home.clone() else { return HashMap::new() };
        let Ok(raw) = read_raw(&dir(&home).join(content_name(shard))) else { return HashMap::new() };
        let mut stale = false;
        let mut out = HashMap::new();
        {
            let state = self.saved.lock();
            for (id, value) in raw {
                stale |= !state.known.contains(&MessageId(id));
                if let Ok(c) = serde_json::from_str::<Content>(value.get()) {
                    out.insert(id, Arc::new(c));
                }
            }
        }
        if stale {
            self.saved.lock().stale.insert(shard);
        }
        out
    }
}

/// A content file as id → the JSON of its content, read without reading the content itself. A
/// missing file is empty.
fn read_raw(file: &Path) -> Result<BTreeMap<u64, Box<RawValue>>> {
    let meta = match std::fs::metadata(file) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(storage(file, e)),
    };
    if meta.len() > MAX_SHARD_BYTES {
        return Err(Error::TooLarge {
            what: "message content shard",
            size: usize::try_from(meta.len()).unwrap_or(usize::MAX),
            limit: usize::try_from(MAX_SHARD_BYTES).unwrap_or(usize::MAX),
        });
    }
    let bytes = std::fs::read(file).map_err(|e| storage(file, e))?;
    let raw: BTreeMap<String, Box<RawValue>> =
        serde_json::from_slice(&bytes).map_err(|e| Error::invalid("message content shard", format!("{}: {e}", file.display())))?;
    // Keys that are not ids (a hand-edited file) are passed over.
    Ok(raw.into_iter().filter_map(|(k, v)| Some((k.parse().ok()?, v))).collect())
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

fn to_json<T: serde::Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| Error::Storage(e.to_string()))
}

/// Write the shards of `messages` that changed since the last save to `path` (all of them when
/// it went elsewhere). The caller writes the store file after, then calls [`finish`].
pub(crate) fn write(saved: &Saved, path: &Path, messages: &[Arc<Message>]) -> Result<Pending> {
    let dir = dir(path);
    std::fs::create_dir_all(&dir).map_err(|e| storage(&dir, e))?;
    let shards = group(messages);
    let state = saved.lock();
    // Content not in memory is read from where it is; written elsewhere, it goes along.
    let from = state.home.as_deref().map(self::dir);
    let moved = state.home.as_deref() != Some(path);
    let before = state.written.as_ref().filter(|w| w.path == path).map(|w| &w.shards);
    let mut contents_written = Vec::new();
    for (shard, list) in &shards {
        let old = before.and_then(|b| b.get(shard));
        let stale = state.stale.contains(shard);
        if old.is_some_and(|old| same(old, list)) && !stale && !moved {
            continue;
        }
        if old.is_none_or(|old| !same(old, list)) {
            let headers: Vec<Message> = list.iter().map(|m| m.headers_only()).collect();
            write_file(&dir.join(file_name(*shard)), &to_json(&headers)?)?;
        }
        // Content the content file does not have yet: what was not there when it was written.
        let had: HashMap<MessageId, &Arc<Content>> = old.into_iter().flatten().filter_map(|m| Some((m.id, m.shared_content()?))).collect();
        let fresh: Vec<&Arc<Message>> =
            list.iter().filter(|m| m.shared_content().is_some_and(|c| had.get(&m.id).is_none_or(|h| !Arc::ptr_eq(h, c)))).collect();
        if fresh.is_empty() && !stale && !moved {
            continue;
        }
        let source = from.as_ref().map(|d| d.join(content_name(*shard)));
        let mut entries = match source.as_deref().map(read_raw).transpose() {
            Ok(entries) => entries.unwrap_or_default(),
            // Unreadable: kept aside for whoever wants to look, never written over.
            Err(_) => {
                if let Some(src) = &source {
                    let _ = std::fs::rename(src, src.with_extension("unreadable.json"));
                }
                BTreeMap::new()
            }
        };
        let here: HashSet<MessageId> = list.iter().map(|m| m.id).collect();
        entries.retain(|id, _| here.contains(&MessageId(*id)) || state.known.contains(&MessageId(*id)));
        for m in list {
            if let Some(c) = m.shared_content() {
                let json = serde_json::value::to_raw_value(&**c).map_err(|e| Error::Storage(e.to_string()))?;
                entries.insert(m.id.0, json);
            }
        }
        write_file(&dir.join(content_name(*shard)), &to_json(&entries)?)?;
        contents_written.push(*shard);
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
    // Content files of shards with no message left: kept while they may hold a removed message
    // undo can bring back.
    let known_shards: HashSet<u64> = state.known.iter().map(|id| shard_of(*id)).collect();
    let mut gone_contents: Vec<u64> = state.stale.iter().copied().filter(|k| !shards.contains_key(k) && !known_shards.contains(k)).collect();
    if moved {
        // A folder written for the first time: content files nothing here owns.
        if let Ok(entries) = std::fs::read_dir(&dir) {
            gone_contents
                .extend(entries.filter_map(|e| e.ok()?.file_name().to_str().and_then(|n| parse_kind(n, 'c'))).filter(|k| !shards.contains_key(k)));
        }
    }
    drop(state);
    Ok(Pending { dir, shards, gone, gone_contents, contents_written })
}

/// Shards written, the store file still to write.
pub(crate) struct Pending {
    dir: PathBuf,
    shards: HashMap<u64, Vec<Arc<Message>>>,
    gone: Vec<u64>,
    gone_contents: Vec<u64>,
    contents_written: Vec<u64>,
}

/// The store file is written: drop the shards it no longer has and remember what is on disk.
pub(crate) fn finish(saved: &Saved, path: &Path, pending: Pending) {
    for shard in &pending.gone {
        // Left behind, a stale shard is only read for ids the store no longer has; harmless.
        let _ = std::fs::remove_file(pending.dir.join(file_name(*shard)));
    }
    for shard in &pending.gone_contents {
        let _ = std::fs::remove_file(pending.dir.join(content_name(*shard)));
    }
    let mut state = saved.lock();
    for shard in pending.contents_written.iter().chain(&pending.gone_contents) {
        state.stale.remove(shard);
    }
    let ids: Vec<MessageId> = pending.shards.values().flatten().map(|m| m.id).collect();
    state.known.extend(ids);
    state.home = Some(path.to_path_buf());
    state.written = Some(Written { path: path.to_path_buf(), shards: pending.shards });
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
        for other in ["m.json", "x000042.json", "m00004a.json", "m000042.tmp", "m-1.json", "store.json", "c000042.json"] {
            assert_eq!(parse_name(other), None, "{other}");
        }
        assert_eq!(parse_kind(&content_name(42), 'c'), Some(42));
        assert_eq!(shard_of(MessageId(999)), 0);
        assert_eq!(shard_of(MessageId(1000)), 1);
        assert_eq!(dir(Path::new("/x/store.json")), Path::new("/x/store.mail"));
    }
}
