//! Mail sync over a server that names messages by id rather than IMAP UID: Microsoft Graph
//! ([`crate::graph_mail`]) and Exchange Web Services ([`crate::ews_mail`]). The same [`State`]
//! in and [`Batch`] out as the IMAP sync, so the store side ([`crate::apply`]) is shared.
//!
//! - Each folder is a mailbox named by its server id, its name the folder path joined with `/`;
//!   the standard folders carry their special-use attribute (`\Inbox`, `\Sent`…).
//! - A message gets the next number of its folder the first time it is seen, and keeps the
//!   server's id beside it ([`ibercorreo_core::RemoteRef::id`]).
//! - Every sync lists each folder's message ids with their read and flag state, newest first,
//!   and downloads the new ones as MIME text, the newest [`Limits::new_per_folder`] first, as
//!   over IMAP.
//! - Changes made here go back as the server's own calls ([`MailServer`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ibercorreo_dav::{Error, Result};
use ibercorreo_imap::Mailbox;

use crate::outbox::{Outcome, Outgoing, Sent};
use crate::{Batch, FolderSync, Limits, NewMessage, Progress, Push, State};

/// The `UIDVALIDITY` kept for such a folder: the numbers are this app's, they never change.
pub const UID_VALIDITY: u32 = 1;
/// Most messages listed in one folder.
pub const MAX_MESSAGES: usize = 500_000;

/// What a server does for the mail sync. Ids are the server's own; "gone" is `Status` 404.
pub trait MailServer {
    /// How the server is named in messages (`Microsoft Graph`).
    fn name(&self) -> &'static str;
    /// The mail folders, the standard ones marked, at most `max`.
    fn folders(&mut self, max: usize) -> Result<Vec<Mailbox>>;
    /// The messages of `folder`, newest first: each id and its flags as IMAP spells them
    /// (`\Seen`, `\Flagged`).
    fn list(&mut self, folder: &str) -> Result<Vec<(String, Vec<String>)>>;
    /// A message as MIME text.
    fn fetch(&mut self, id: &str) -> Result<Vec<u8>>;
    fn set_flags(&mut self, id: &str, seen: bool, flagged: bool) -> Result<()>;
    fn move_to(&mut self, id: &str, folder: &str) -> Result<()>;
    /// Delete for good.
    fn delete(&mut self, id: &str) -> Result<()>;
    /// Send `raw` (its recipients in its headers, [`with_bcc`]) and file the copy in the
    /// server's Sent. One it can never send is [`Error::Invalid`].
    fn send(&mut self, raw: &[u8]) -> Result<()>;
}

/// An error that ends the whole sync rather than one folder or message.
pub(crate) fn fatal(e: &Error) -> bool {
    matches!(e, Error::Auth(_) | Error::Io(_) | Error::Busy { .. })
}

fn gone(e: &Error) -> bool {
    matches!(e, Error::Status { status: 404, .. })
}

/// Sync one folder: what vanished, the flags of what is held, and the newest new messages.
fn folder(s: &mut dyn MailServer, mailbox: &Mailbox, state: &State, limits: &Limits, step: &mut dyn FnMut(usize, usize)) -> Result<FolderSync> {
    let known = state.known.iter().find(|k| k.path == mailbox.raw);
    let reset = known.is_some_and(|k| k.uid_validity != UID_VALIDITY);
    let held: BTreeMap<&str, u32> = match known {
        Some(k) if !reset => k.ids.iter().map(|(uid, id)| (&**id, *uid)).collect(),
        _ => BTreeMap::new(),
    };
    let mut next_uid = match known {
        Some(k) if !reset => k.uids.last().map_or(Some(1), |u| u.checked_add(1)),
        _ => Some(1),
    };
    let list = s.list(&mailbox.raw)?;
    let mut f = FolderSync { path: mailbox.raw.clone(), uid_validity: UID_VALIDITY, reset, ..FolderSync::default() };
    let mut on_server = BTreeSet::new();
    // Not held yet, newest first, and whether each is newer than everything held.
    let mut unseen: Vec<(&str, &[String], bool)> = Vec::new();
    let mut past_held = false;
    for (id, flags) in &list {
        if !on_server.insert(id.as_str()) {
            continue;
        }
        match held.get(id.as_str()) {
            Some(uid) => {
                past_held = true;
                f.flags.push((*uid, flags.clone()));
            }
            None => unseen.push((id, flags, !past_held)),
        }
    }
    f.vanished = held.iter().filter(|(id, _)| !on_server.contains(*id)).map(|(_, uid)| *uid).collect();
    let take = unseen.len().min(limits.new_per_folder);
    f.remaining = unseen.len().saturating_sub(take);
    let newest = unseen.get(..take).unwrap_or(&[]);
    // Oldest first, so the numbers grow with the mail as IMAP UIDs do.
    let mut fetched = 0usize;
    for (id, flags, arrived) in newest.iter().rev() {
        step(fetched, newest.len());
        fetched = fetched.saturating_add(1);
        let raw = match s.fetch(id) {
            Ok(raw) => raw,
            // Deleted or moved since the list: it shows up where it went at the next sync.
            Err(e) if gone(&e) => continue,
            Err(e) => return Err(e),
        };
        let Some(uid) = next_uid else { return Err(Error::TooLarge { what: "the message numbers of the folder", limit: u32::MAX as usize }) };
        next_uid = uid.checked_add(1);
        f.new.push(NewMessage { uid, flags: flags.to_vec(), raw, id: Some(Arc::from(*id)), arrived: *arrived });
    }
    if !newest.is_empty() {
        step(fetched, newest.len());
    }
    Ok(f)
}

/// Send the local changes. A change to a message whose server id is not known is reported, not sent.
fn push(s: &mut dyn MailServer, state: &State, batch: &mut Batch) -> Result<()> {
    for p in &state.pushes {
        let (path, uid) = match p {
            Push::Flags { path, uid, .. } | Push::Move { path, uid, .. } | Push::Expunge { path, uid } => (path, *uid),
        };
        let Some(id) = state.known.iter().find(|k| &k.path == path).and_then(|k| k.ids.get(&uid)) else {
            batch.pushed.push((p.clone(), Err(format!("no {} id for this message", s.name()))));
            continue;
        };
        let r = match p {
            Push::Flags { seen, flagged, .. } => s.set_flags(id, *seen, *flagged),
            Push::Move { to, .. } => s.move_to(id, to),
            Push::Expunge { .. } => match s.delete(id) {
                // Already gone on the server: what was asked for.
                Err(e) if gone(&e) => Ok(()),
                other => other,
            },
        };
        match r {
            Err(e) if fatal(&e) => return Err(e),
            r => batch.pushed.push((p.clone(), r.map_err(|e| e.to_string()))),
        }
    }
    Ok(())
}

/// Send local changes, then fetch the account's mail, telling `progress` as each folder starts
/// and as its new mail comes in. A folder that fails on its own is reported in
/// [`Batch::errors`] and the others continue; a refused sign-in, a lost connection or throttling
/// that does not end soon ends the sync with an error.
pub fn run_with(s: &mut dyn MailServer, state: &State, limits: &Limits, progress: &mut dyn FnMut(&Progress)) -> Result<Batch> {
    let mut batch = Batch::default();
    push(s, state, &mut batch)?;
    let mailboxes = s.folders(limits.folders)?;
    let count = mailboxes.len();
    // A list as long as the limit may have been cut short.
    batch.listed_all = count < limits.folders;
    for (index, mailbox) in mailboxes.iter().enumerate() {
        let mut at = Progress { folder: mailbox.name.clone(), index, folders: count, ..Progress::default() };
        progress(&at);
        let mut step = |fetched: usize, to_fetch: usize| {
            at.fetched = fetched;
            at.to_fetch = to_fetch;
            progress(&at);
        };
        match folder(s, mailbox, state, limits, &mut step) {
            Ok(f) => batch.folders.push(f),
            Err(e) if fatal(&e) => return Err(e),
            Err(e) => batch.errors.push(format!("{}: {e}", mailbox.name)),
        }
    }
    batch.mailboxes = mailboxes;
    Ok(batch)
}

/// `raw` with a `Bcc` header naming the envelope's recipients that its `To` and `Cc` do not:
/// Graph and EWS take a MIME message's recipients from its headers, and the app keeps blind
/// copies out of them. Exchange delivers them and keeps the header only in the sender's copy.
pub fn with_bcc(raw: &[u8], envelope: &[String]) -> Vec<u8> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").or_else(|| raw.windows(2).position(|w| w == b"\n\n")).unwrap_or(raw.len());
    let head = String::from_utf8_lossy(raw.get(..end).unwrap_or(raw)).replace("\r\n", "\n");
    // To and Cc, with their folded lines.
    let mut named = String::new();
    let mut inside = false;
    for line in head.split('\n') {
        if line.starts_with([' ', '\t']) {
            if inside {
                named.push_str(line);
            }
            continue;
        }
        let lower = line.to_ascii_lowercase();
        inside = lower.starts_with("to:") || lower.starts_with("cc:");
        if inside {
            named.push_str(&lower);
            named.push('\n');
        }
    }
    let named = named.to_lowercase();
    let blind: Vec<&str> = envelope
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty() && !a.chars().any(|c| c.is_control() || c == ',') && !named.contains(&a.to_lowercase()))
        .collect();
    if blind.is_empty() {
        return raw.to_vec();
    }
    let mut out = format!("Bcc: {}\r\n", blind.join(", ")).into_bytes();
    out.extend_from_slice(raw);
    out
}

/// Send each message; the server files it in Sent itself. After a lost connection, a refused
/// sign-in or throttling, the rest are left for later rather than tried.
pub fn send(s: &mut dyn MailServer, mail: &[Outgoing]) -> Vec<Sent> {
    let mut out = Vec::with_capacity(mail.len());
    let mut stop: Option<String> = None;
    for m in mail {
        if let Some(why) = &stop {
            out.push(Sent { id: m.id, outcome: Outcome::Later(why.clone()) });
            continue;
        }
        let outcome = match s.send(&with_bcc(&m.raw, &m.to)) {
            Ok(()) => Outcome::Sent { saved: true },
            Err(e) if fatal(&e) => {
                stop = Some(e.to_string());
                Outcome::Later(e.to_string())
            }
            // Missing consent or a server fault: it may go through later.
            Err(e @ Error::Status { status: 403 | 500..=599, .. }) => Outcome::Later(e.to_string()),
            Err(e) => Outcome::Failed(e.to_string()),
        };
        out.push(Sent { id: m.id, outcome });
    }
    out
}
