//! Mail over Microsoft Graph, for Microsoft 365 work accounts whose organisation turned IMAP
//! off: the same [`State`] in and [`Batch`] out as the IMAP sync, so the store side
//! ([`crate::apply`]) is shared, and the Outbox sent with `sendMail`.
//!
//! - Each mail folder is a mailbox named by its Graph id, its name the folder path joined with
//!   `/`. The standard folders are found by their well-known names (`inbox`, `sentitems`…);
//!   Exchange's own Outbox is left out, the app has one.
//! - Graph has no UIDs: a message gets the next number of its folder the first time it is seen,
//!   and keeps its Graph id beside it ([`ibercorreo_core::RemoteRef::id`]). Ids are asked for as
//!   immutable ones, so they stay the same when Outlook files a message.
//! - Every sync lists each folder's message ids with their read and flag state, newest first,
//!   and downloads the new ones as MIME text (`…/$value`), the newest [`Limits::new_per_folder`]
//!   first, as over IMAP.
//! - Changes made here go back as `PATCH` (read, flagged), `move` and `permanentDelete`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ibercorreo_dav::caldav::Http;
use ibercorreo_dav::graph::Graph;
use ibercorreo_dav::{Error, Result};
use ibercorreo_imap::Mailbox;
use serde_json::{Value, json};

use crate::graph_calendar::{segment, text};
use crate::outbox::{Outcome, Outgoing, Sent};
use crate::{Batch, FolderSync, Limits, NewMessage, Progress, Push, State};

/// The `UIDVALIDITY` kept for a Graph folder: the numbers are this app's, they never change.
pub const UID_VALIDITY: u32 = 1;
/// Asks Graph for ids that stay the same when a message moves.
pub const IMMUTABLE_IDS: (&str, &str) = ("Prefer", "IdType=\"ImmutableId\"");
/// How deep folders inside folders are read.
pub const MAX_DEPTH: usize = 10;
/// Most messages listed in one folder.
pub const MAX_MESSAGES: usize = 500_000;
/// Largest `sendMail` request Graph takes (the message as base64).
pub const MAX_SEND_BYTES: usize = 4 * 1024 * 1024;

/// Exchange's Outbox: not synced, the app has its own.
const OUTBOX: &str = "\\Outbox";
/// Well-known folder names and the special-use attribute each stands for.
const WELL_KNOWN: [(&str, &str); 7] = [
    ("inbox", "\\Inbox"),
    ("sentitems", "\\Sent"),
    ("drafts", "\\Drafts"),
    ("deleteditems", "\\Trash"),
    ("junkemail", "\\Junk"),
    ("archive", "\\Archive"),
    ("outbox", OUTBOX),
];
const FOLDER_FIELDS: &str = "$select=id,displayName,childFolderCount&$top=100";
const MESSAGE_FIELDS: &str = "$select=id,isRead,flag&$orderby=receivedDateTime%20desc&$top=500";

/// An error that ends the whole sync rather than one folder or message.
fn fatal(e: &Error) -> bool {
    matches!(e, Error::Auth(_) | Error::Io(_) | Error::Busy { .. })
}

/// The account's mail folders, the standard ones marked with their special-use attribute, at
/// most `max`.
pub fn folders<H: Http>(g: &mut Graph<H>, max: usize) -> Result<Vec<Mailbox>> {
    let mut roles: BTreeMap<String, &'static str> = BTreeMap::new();
    for (name, attribute) in WELL_KNOWN {
        match g.call("GET", &format!("me/mailFolders/{name}?$select=id"), &[IMMUTABLE_IDS], None) {
            Ok(Some(v)) => {
                if let Some(id) = text(&v, "id") {
                    roles.insert(id.to_string(), attribute);
                }
            }
            Ok(None) => {}
            Err(e) if fatal(&e) => return Err(e),
            // An account without an Archive, say.
            Err(_) => {}
        }
    }
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    // (list address, parent path, depth)
    let mut queue = vec![(format!("me/mailFolders?{FOLDER_FIELDS}"), String::new(), 0usize)];
    while let Some((list, parent, depth)) = queue.pop() {
        let listed = g.list(&list, &[IMMUTABLE_IDS], max.saturating_add(1))?;
        for f in &listed.items {
            let Some(id) = text(f, "id") else { continue };
            if !seen.insert(id.to_string()) {
                continue;
            }
            let attribute = roles.get(id).copied();
            if attribute == Some(OUTBOX) {
                continue;
            }
            let leaf = text(f, "displayName").unwrap_or(id);
            let name = if parent.is_empty() { leaf.to_string() } else { format!("{parent}/{leaf}") };
            if f.get("childFolderCount").and_then(Value::as_u64).is_some_and(|n| n > 0) && depth < MAX_DEPTH {
                queue.push((format!("me/mailFolders/{}/childFolders?{FOLDER_FIELDS}", segment(id)), name.clone(), depth.saturating_add(1)));
            }
            out.push(Mailbox { raw: id.to_string(), name, delimiter: Some('/'), attributes: attribute.map(str::to_string).into_iter().collect() });
            if out.len() >= max {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

/// A listed message's id and its flags as IMAP spells them.
fn listed(v: &Value) -> Option<(&str, Vec<String>)> {
    let id = text(v, "id")?;
    let mut flags = Vec::new();
    if v.get("isRead").and_then(Value::as_bool) == Some(true) {
        flags.push("\\Seen".to_string());
    }
    if v.get("flag").and_then(|f| f.get("flagStatus")).and_then(Value::as_str) == Some("flagged") {
        flags.push("\\Flagged".to_string());
    }
    Some((id, flags))
}

/// Sync one folder: what vanished, the flags of what is held, and the newest new messages.
fn folder<H: Http>(g: &mut Graph<H>, mailbox: &Mailbox, state: &State, limits: &Limits, step: &mut dyn FnMut(usize, usize)) -> Result<FolderSync> {
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
    let list = g.list(&format!("me/mailFolders/{}/messages?{MESSAGE_FIELDS}", segment(&mailbox.raw)), &[IMMUTABLE_IDS], MAX_MESSAGES)?;
    let mut f = FolderSync { path: mailbox.raw.clone(), uid_validity: UID_VALIDITY, reset, ..FolderSync::default() };
    let mut on_server = BTreeSet::new();
    // Not held yet, newest first, and whether each is newer than everything held.
    let mut unseen: Vec<(&str, Vec<String>, bool)> = Vec::new();
    let mut past_held = false;
    for v in &list.items {
        let Some((id, flags)) = listed(v) else { continue };
        if !on_server.insert(id) {
            continue;
        }
        match held.get(id) {
            Some(uid) => {
                past_held = true;
                f.flags.push((*uid, flags));
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
        let raw = match g.raw("GET", &format!("me/messages/{}/$value", segment(id)), &[IMMUTABLE_IDS], b"", "") {
            Ok(raw) => raw,
            // Deleted or moved since the list: it shows up where it went at the next sync.
            Err(Error::Status { status: 404, .. }) => continue,
            Err(e) => return Err(e),
        };
        let Some(uid) = next_uid else { return Err(Error::TooLarge { what: "the message numbers of the folder", limit: u32::MAX as usize }) };
        next_uid = uid.checked_add(1);
        f.new.push(NewMessage { uid, flags: flags.clone(), raw, id: Some(Arc::from(*id)), arrived: *arrived });
    }
    if !newest.is_empty() {
        step(fetched, newest.len());
    }
    Ok(f)
}

/// Send the local changes. A change to a message whose Graph id is not known is reported, not sent.
fn push<H: Http>(g: &mut Graph<H>, state: &State, batch: &mut Batch) -> Result<()> {
    for p in &state.pushes {
        let (path, uid) = match p {
            Push::Flags { path, uid, .. } | Push::Move { path, uid, .. } | Push::Expunge { path, uid } => (path, *uid),
        };
        let Some(id) = state.known.iter().find(|k| &k.path == path).and_then(|k| k.ids.get(&uid)) else {
            batch.pushed.push((p.clone(), Err("no Microsoft Graph id for this message".into())));
            continue;
        };
        let message = format!("me/messages/{}", segment(id));
        let r = match p {
            Push::Flags { seen, flagged, .. } => {
                let body = json!({ "isRead": seen, "flag": { "flagStatus": if *flagged { "flagged" } else { "notFlagged" } } });
                g.call("PATCH", &message, &[IMMUTABLE_IDS], Some(&body)).map(|_| ())
            }
            Push::Move { to, .. } => g.call("POST", &format!("{message}/move"), &[IMMUTABLE_IDS], Some(&json!({ "destinationId": to }))).map(|_| ()),
            Push::Expunge { .. } => match g.call("POST", &format!("{message}/permanentDelete"), &[], None) {
                // Already gone on the server: what was asked for.
                Err(Error::Status { status: 404, .. }) => Ok(()),
                other => other.map(|_| ()),
            },
        };
        match r {
            Err(e) if fatal(&e) => return Err(e),
            r => batch.pushed.push((p.clone(), r.map_err(|e| e.to_string()))),
        }
    }
    Ok(())
}

/// Send local changes, then fetch the account's mail. A folder that fails on its own is
/// reported in [`Batch::errors`] and the others continue; a refused token, a lost connection or
/// throttling that does not end soon ends the sync with an error.
pub fn run<H: Http>(g: &mut Graph<H>, state: &State, limits: &Limits) -> Result<Batch> {
    run_with(g, state, limits, &mut |_| {})
}

/// [`run`], telling `progress` as each folder starts and as its new mail comes in.
pub fn run_with<H: Http>(g: &mut Graph<H>, state: &State, limits: &Limits, progress: &mut dyn FnMut(&Progress)) -> Result<Batch> {
    let mut batch = Batch::default();
    push(g, state, &mut batch)?;
    let mailboxes = folders(g, limits.folders)?;
    let count = mailboxes.len();
    for (index, mailbox) in mailboxes.iter().enumerate() {
        let mut at = Progress { folder: mailbox.name.clone(), index, folders: count, ..Progress::default() };
        progress(&at);
        let mut step = |fetched: usize, to_fetch: usize| {
            at.fetched = fetched;
            at.to_fetch = to_fetch;
            progress(&at);
        };
        match folder(g, mailbox, state, limits, &mut step) {
            Ok(f) => batch.folders.push(f),
            Err(e) if fatal(&e) => return Err(e),
            Err(e) => batch.errors.push(format!("{}: {e}", mailbox.name)),
        }
    }
    batch.mailboxes = mailboxes;
    Ok(batch)
}

/// `raw` with a `Bcc` header naming the envelope's recipients that its `To` and `Cc` do not:
/// Graph takes a MIME message's recipients from its headers, and the app keeps blind copies out
/// of them. Exchange delivers them and keeps the header only in the sender's copy.
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

/// Send each message with `sendMail`; Exchange files it in Sent Items itself. After a lost
/// connection, a refused token or throttling, the rest are left for later rather than tried.
pub fn send<H: Http>(g: &mut Graph<H>, mail: &[Outgoing]) -> Vec<Sent> {
    let mut out = Vec::with_capacity(mail.len());
    let mut stop: Option<String> = None;
    for m in mail {
        if let Some(why) = &stop {
            out.push(Sent { id: m.id, outcome: Outcome::Later(why.clone()) });
            continue;
        }
        let body = ibercorreo_mime::base64(&with_bcc(&m.raw, &m.to));
        if body.len() > MAX_SEND_BYTES {
            out.push(Sent {
                id: m.id,
                outcome: Outcome::Failed(format!(
                    "larger than the {} MB Microsoft Graph sends in one message; make the attachments smaller",
                    MAX_SEND_BYTES / (1024 * 1024)
                )),
            });
            continue;
        }
        let outcome = match g.raw("POST", "me/sendMail", &[], body.as_bytes(), "text/plain") {
            Ok(_) => Outcome::Sent { saved: true },
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

#[cfg(test)]
mod tests;
