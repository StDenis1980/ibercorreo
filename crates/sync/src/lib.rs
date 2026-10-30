//! Mail sync, L3: keeping the local store and an IMAP account in step.
//!
//! A sync has three parts, so the slow one can run off the interface thread:
//! 1. [`state`] (fast, on the store): what the store knows per mailbox, and the local changes not
//!    yet on the server (read, flagged, moved, deleted for good).
//! 2. [`run`] (slow, on the connection): send those changes, then list mailboxes and fetch what is
//!    new, what changed and what vanished. It never touches the store.
//! 3. [`apply`] (fast, on the store): fold the result in.
//!
//! Local changes are found by comparing each message with what the server last reported
//! ([`RemoteRef`]), so nothing is lost if a sync fails halfway: the next one sees the same difference.
//! UIDs are only trusted while `UIDVALIDITY` stays the same (RFC 9051 §2.3.1.1).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};

use ibercorreo_core::{AccountId, FolderId, FolderRole, MessageId, RemoteFolder, RemoteRef};
use ibercorreo_imap::{Client, Items, Mailbox, uid_set};
use ibercorreo_store::Store;
use serde::Serialize;

pub use ibercorreo_imap::{Error, Result};

pub mod calendar;
pub mod contacts;
pub mod graph_calendar;
pub mod graph_contacts;
pub mod outbox;

#[cfg(test)]
mod tests;

/// How much one sync may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most new messages downloaded per mailbox per sync, newest first. Older mail follows on
    /// later syncs.
    pub new_per_folder: usize,
    /// Messages per `UID FETCH` of whole messages.
    pub fetch_batch: usize,
    /// UIDs per `UID FETCH` of flags.
    pub flag_batch: usize,
    /// The most mailboxes synced.
    pub folders: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { new_per_folder: 500, fetch_batch: 25, flag_batch: 1000, folders: 500 }
    }
}

/// A local change to send to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Push {
    /// Set `\Seen` / `\Flagged` to the local values.
    Flags { path: String, uid: u32, seen: bool, flagged: bool },
    /// The message was moved to another synced folder here.
    Move { path: String, uid: u32, to: String },
    /// The message was deleted for good here.
    Expunge { path: String, uid: u32 },
}

impl Push {
    fn path(&self) -> &str {
        match self {
            Push::Flags { path, .. } | Push::Move { path, .. } | Push::Expunge { path, .. } => path,
        }
    }
}

/// What the store knows about one server mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub path: String,
    pub uid_validity: u32,
    /// UIDs held here, including deleted ones still waiting for [`Push::Expunge`].
    pub uids: BTreeSet<u32>,
}

/// The input to [`run`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub known: Vec<Known>,
    pub pushes: Vec<Push>,
}

/// Read what the store knows of `account`'s server and which local changes it has not seen.
pub fn state(store: &Store, account: AccountId) -> State {
    let mut known: BTreeMap<FolderId, Known> = BTreeMap::new();
    for f in store.folders_of(account) {
        if let Some(r) = &f.remote {
            known.insert(f.id, Known { path: r.path.clone(), uid_validity: r.uid_validity, uids: BTreeSet::new() });
        }
    }
    let path = |id: FolderId| known.get(&id).map(|k| k.path.clone());
    let mut pushes = Vec::new();
    let mut held: Vec<(FolderId, u32)> = Vec::new();
    for m in store.all_messages() {
        let Some(r) = m.remote else { continue };
        let Some(from) = path(r.folder) else { continue };
        held.push((r.folder, r.uid));
        if m.folder != r.folder {
            // Moved here. To a folder that is not on the server: nothing to send.
            if let Some(to) = path(m.folder) {
                pushes.push(Push::Move { path: from, uid: r.uid, to });
            }
        } else if m.seen != r.seen || m.flagged != r.flagged {
            pushes.push(Push::Flags { path: from, uid: r.uid, seen: m.seen, flagged: m.flagged });
        }
    }
    for t in store.tombstones() {
        if let Some(from) = path(t.folder) {
            held.push((t.folder, t.uid));
            pushes.push(Push::Expunge { path: from, uid: t.uid });
        }
    }
    for (folder, uid) in held {
        if let Some(k) = known.get_mut(&folder) {
            k.uids.insert(uid);
        }
    }
    State { known: known.into_values().collect(), pushes }
}

/// A message seen for the first time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub uid: u32,
    pub flags: Vec<String>,
    pub raw: Vec<u8>,
}

/// What [`run`] found in one mailbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderSync {
    pub path: String,
    pub uid_validity: u32,
    /// `UIDVALIDITY` changed: everything held for this mailbox is void.
    pub reset: bool,
    pub new: Vec<NewMessage>,
    /// Current flags of messages already held.
    pub flags: Vec<(u32, Vec<String>)>,
    /// Held UIDs the server no longer has.
    pub vanished: Vec<u32>,
    /// UIDs on the server not downloaded yet (past [`Limits::new_per_folder`]).
    pub remaining: usize,
}

/// The output of [`run`], the input to [`apply`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    pub mailboxes: Vec<Mailbox>,
    pub folders: Vec<FolderSync>,
    /// Each local change and whether the server took it.
    pub pushed: Vec<(Push, std::result::Result<(), String>)>,
    /// Mailboxes that could not be synced, and why; the others still were.
    pub errors: Vec<String>,
}

fn has(flags: &[String], flag: &str) -> bool {
    flags.iter().any(|f| f.eq_ignore_ascii_case(flag))
}

/// How far a sync has come, for a progress bar: the folder being synced (`index` of `folders`,
/// counting from 0) and how many of its new messages are downloaded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub folder: String,
    pub index: usize,
    pub folders: usize,
    pub fetched: usize,
    pub to_fetch: usize,
}

impl Progress {
    /// The share done, 0 to 1.
    pub fn fraction(&self) -> f32 {
        if self.folders == 0 {
            return 1.0;
        }
        let within = if self.to_fetch == 0 { 0.0 } else { self.fetched.min(self.to_fetch) as f32 / self.to_fetch as f32 };
        ((self.index as f32 + within) / self.folders as f32).clamp(0.0, 1.0)
    }
}

/// Send local changes, then fetch the account's state. A failure in one mailbox is recorded in
/// [`Batch::errors`] and the others continue; a lost connection ends the sync with an error.
pub fn run<S: Read + Write>(client: &mut Client<S>, state: &State, limits: &Limits) -> Result<Batch> {
    run_with(client, state, limits, &mut |_| {})
}

/// [`run`], telling `progress` as each folder starts and as its new mail comes in.
pub fn run_with<S: Read + Write>(client: &mut Client<S>, state: &State, limits: &Limits, progress: &mut dyn FnMut(&Progress)) -> Result<Batch> {
    let mut batch = Batch::default();
    push(client, state, &mut batch)?;

    let mut mailboxes = client.list()?;
    mailboxes.truncate(limits.folders);
    let selectable: Vec<&Mailbox> = mailboxes.iter().filter(|m| m.selectable()).collect();
    let folders = selectable.len();
    for (index, mailbox) in selectable.into_iter().enumerate() {
        let mut at = Progress { folder: mailbox.name.clone(), index, folders, ..Progress::default() };
        progress(&at);
        let mut step = |fetched: usize, to_fetch: usize| {
            at.fetched = fetched;
            at.to_fetch = to_fetch;
            progress(&at);
        };
        match folder(client, mailbox, state, limits, &mut step) {
            Ok(f) => batch.folders.push(f),
            Err(e @ (Error::Io(_) | Error::Timeout | Error::Closed | Error::Bye(_))) => return Err(e),
            Err(e) => batch.errors.push(format!("{}: {e}", mailbox.name)),
        }
    }
    batch.mailboxes = mailboxes;
    Ok(batch)
}

fn push<S: Read + Write>(client: &mut Client<S>, state: &State, batch: &mut Batch) -> Result<()> {
    let mut by_path: BTreeMap<&str, Vec<&Push>> = BTreeMap::new();
    for p in &state.pushes {
        by_path.entry(p.path()).or_default().push(p);
    }
    for (path, pushes) in by_path {
        if let Err(e) = client.select(path, false) {
            if matches!(e, Error::Io(_) | Error::Timeout | Error::Closed | Error::Bye(_)) {
                return Err(e);
            }
            for p in pushes {
                batch.pushed.push((p.clone(), Err(e.to_string())));
            }
            continue;
        }
        for p in pushes {
            let r = match p {
                Push::Flags { uid, seen, flagged, .. } => {
                    let set = uid.to_string();
                    let mut add = Vec::new();
                    let mut remove = Vec::new();
                    for (on, flag) in [(*seen, "\\Seen"), (*flagged, "\\Flagged")] {
                        if on { add.push(flag) } else { remove.push(flag) }
                    }
                    let r = if add.is_empty() { Ok(()) } else { client.uid_store(&set, true, &add) };
                    r.and_then(|()| if remove.is_empty() { Ok(()) } else { client.uid_store(&set, false, &remove) })
                }
                Push::Move { uid, to, .. } => client.uid_move(&uid.to_string(), to),
                Push::Expunge { uid, .. } => {
                    let set = uid.to_string();
                    client.uid_store(&set, true, &["\\Deleted"]).and_then(|()| client.uid_expunge(&set))
                }
            };
            if let Err(e @ (Error::Io(_) | Error::Timeout | Error::Closed | Error::Bye(_))) = &r {
                return Err(e.clone());
            }
            batch.pushed.push((p.clone(), r.map_err(|e| e.to_string())));
        }
    }
    Ok(())
}

fn folder<S: Read + Write>(
    client: &mut Client<S>,
    mailbox: &Mailbox,
    state: &State,
    limits: &Limits,
    step: &mut dyn FnMut(usize, usize),
) -> Result<FolderSync> {
    let selected = client.select(&mailbox.raw, true)?;
    // A server without UIDVALIDITY (not allowed by the RFC) gets 0: its UIDs are then never trusted
    // across a change to a real value.
    let uid_validity = selected.uid_validity.unwrap_or(0);
    let known = state.known.iter().find(|k| k.path == mailbox.raw);
    let reset = known.is_some_and(|k| k.uid_validity != uid_validity);
    let held: BTreeSet<u32> = match known {
        Some(k) if !reset => k.uids.clone(),
        _ => BTreeSet::new(),
    };
    let on_server: BTreeSet<u32> = if selected.exists == 0 { BTreeSet::new() } else { client.uid_search("1:*")?.into_iter().collect() };

    let mut f = FolderSync { path: mailbox.raw.clone(), uid_validity, reset, ..FolderSync::default() };
    f.vanished = held.difference(&on_server).copied().collect();
    let still: Vec<u32> = held.intersection(&on_server).copied().collect();
    for chunk in still.chunks(limits.flag_batch.max(1)) {
        for item in client.uid_fetch(&uid_set(chunk)?, Items::Summary)? {
            if let (Some(uid), Some(flags)) = (item.uid, item.flags) {
                f.flags.push((uid, flags));
            }
        }
    }
    let unseen: Vec<u32> = on_server.difference(&held).copied().collect();
    let take = unseen.len().min(limits.new_per_folder);
    f.remaining = unseen.len().saturating_sub(take);
    let newest = unseen.get(unseen.len().saturating_sub(take)..).unwrap_or(&[]);
    let mut fetched = 0usize;
    for chunk in newest.chunks(limits.fetch_batch.max(1)) {
        step(fetched, newest.len());
        for item in client.uid_fetch(&uid_set(chunk)?, Items::Full)? {
            if let (Some(uid), Some(raw)) = (item.uid, item.body) {
                f.new.push(NewMessage { uid, flags: item.flags.unwrap_or_default(), raw });
            }
        }
        fetched = fetched.saturating_add(chunk.len());
    }
    if !newest.is_empty() {
        step(fetched, newest.len());
    }
    Ok(f)
}

/// What [`apply`] changed, for the status bar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub folders_added: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub pushed: usize,
    /// Messages on the server still to download.
    pub remaining: usize,
    pub errors: Vec<String>,
    /// Mail new in an Inbox that was synced before (newer than everything it held), for the
    /// rules to run on; not the first download of a folder, nor older mail fetched later.
    #[serde(skip)]
    pub arrived: Vec<MessageId>,
}

/// The local folder for a server mailbox: the one that already mirrors it, else a standard folder
/// of the same role not yet linked, else a new folder.
fn local_folder(store: &mut Store, account: AccountId, mailbox: &Mailbox, report: &mut Report) -> FolderId {
    if let Some(id) = store.folder_by_path(account, &mailbox.raw) {
        return id;
    }
    let role = mailbox.role();
    if role != FolderRole::Other
        && let Some(id) = store.folders_of(account).iter().find(|f| f.role == role && f.remote.is_none()).map(|f| f.id)
    {
        return id;
    }
    report.folders_added = report.folders_added.saturating_add(1);
    let name = match mailbox.delimiter {
        Some(d) => mailbox.name.replace(d, "/"),
        None => mailbox.name.clone(),
    };
    // A role already taken (two `\Sent` mailboxes, say) is kept as an ordinary folder.
    store.add_folder(account, &name, FolderRole::Other)
}

/// Fold a [`run`] result for `account` into the store.
pub fn apply(store: &mut Store, account: AccountId, batch: Batch) -> Report {
    let mut report = Report { errors: batch.errors, ..Report::default() };

    for (p, outcome) in &batch.pushed {
        match outcome {
            Ok(()) => report.pushed = report.pushed.saturating_add(1),
            Err(e) => report.errors.push(format!("{p:?}: {e}")),
        }
        if let (Push::Expunge { path, uid }, Ok(())) = (p, outcome)
            && let Some(folder) = store.folder_by_path(account, path)
        {
            store.forget_tombstone(folder, *uid);
        }
        if let (Push::Flags { path, uid, seen, flagged }, Ok(())) = (p, outcome)
            && let Some(id) = store.folder_by_path(account, path).and_then(|f| store.find_remote(f, *uid))
            && let Ok(m) = store.message_mut(id)
            && let Some(r) = m.remote.as_mut()
        {
            r.seen = *seen;
            r.flagged = *flagged;
        }
    }

    let mut folder_ids: BTreeMap<String, FolderId> = BTreeMap::new();
    for mailbox in batch.mailboxes.iter().filter(|m| m.selectable()) {
        let id = local_folder(store, account, mailbox, &mut report);
        folder_ids.insert(mailbox.raw.clone(), id);
    }

    for f in batch.folders {
        let Some(&folder) = folder_ids.get(&f.path) else { continue };
        let inbox_seen_before = store.folder(folder).is_ok_and(|l| l.role == FolderRole::Inbox && l.remote.is_some()) && !f.reset;
        let newest_known = store.all_messages().filter_map(|m| m.remote.filter(|r| r.folder == folder).map(|r| r.uid)).max();
        if let Ok(local) = store.folder_mut(folder) {
            local.remote = Some(RemoteFolder { path: f.path.clone(), uid_validity: f.uid_validity });
        }
        report.remaining = report.remaining.saturating_add(f.remaining);
        if f.reset {
            let void: Vec<MessageId> = store.all_messages().filter(|m| m.remote.is_some_and(|r| r.folder == folder)).map(|m| m.id).collect();
            for id in void {
                if store.drop_message(id).is_ok() {
                    report.removed = report.removed.saturating_add(1);
                }
            }
            let stale: Vec<u32> = store.tombstones().iter().filter(|t| t.folder == folder).map(|t| t.uid).collect();
            for uid in stale {
                store.forget_tombstone(folder, uid);
            }
        }
        for uid in f.vanished {
            store.forget_tombstone(folder, uid);
            if let Some(id) = store.find_remote(folder, uid)
                && store.drop_message(id).is_ok()
            {
                report.removed = report.removed.saturating_add(1);
            }
        }
        for (uid, flags) in f.flags {
            let (seen, flagged) = (has(&flags, "\\Seen"), has(&flags, "\\Flagged"));
            let Some(id) = store.find_remote(folder, uid) else { continue };
            let Ok(m) = store.message_mut(id) else { continue };
            let Some(r) = m.remote.as_mut() else { continue };
            if (r.seen, r.flagged) == (seen, flagged) {
                continue;
            }
            // Take the server's value unless it was changed here and not sent yet.
            if m.seen == r.seen {
                m.seen = seen;
            }
            if m.flagged == r.flagged {
                m.flagged = flagged;
            }
            r.seen = seen;
            r.flagged = flagged;
            report.updated = report.updated.saturating_add(1);
        }
        for n in f.new {
            if store.find_remote(folder, n.uid).is_some() {
                continue;
            }
            let mut m = match ibercorreo_mime::parse(&n.raw, MessageId(0), folder) {
                Ok(m) => m,
                Err(e) => {
                    report.errors.push(format!("{} UID {}: {e}", f.path, n.uid));
                    continue;
                }
            };
            let (seen, flagged) = (has(&n.flags, "\\Seen"), has(&n.flags, "\\Flagged"));
            m.seen = seen;
            m.flagged = flagged;
            m.remote = Some(RemoteRef { folder, uid: n.uid, seen, flagged });
            if let Ok(id) = store.add_message(m) {
                report.added = report.added.saturating_add(1);
                if inbox_seen_before && newest_known.is_none_or(|top| n.uid > top) {
                    report.arrived.push(id);
                }
            }
        }
    }
    report
}
