//! Mail over Microsoft Graph, for Microsoft 365 work accounts whose organisation turned IMAP
//! off: a [`MailServer`] for the shared sync ([`crate::server_mail`]), and the Outbox sent with
//! `sendMail`.
//!
//! - The standard folders are found by their well-known names (`inbox`, `sentitems`…);
//!   Exchange's own Outbox is left out, the app has one.
//! - Ids are asked for as immutable ones, so they stay the same when Outlook files a message.
//! - Messages are listed by `receivedDateTime`, newest first, and read as MIME (`…/$value`).
//! - Changes made here go back as `PATCH` (read, flagged), `move` and `permanentDelete`.

use std::collections::{BTreeMap, BTreeSet};

use ibercorreo_dav::caldav::Http;
use ibercorreo_dav::graph::Graph;
use ibercorreo_dav::{Error, Result};
use ibercorreo_imap::Mailbox;
use serde_json::{Value, json};

use crate::graph_calendar::{segment, text};
use crate::outbox::{Outgoing, Sent};
use crate::server_mail::{MailServer, fatal};
use crate::{Batch, Limits, Progress, State};

pub use crate::server_mail::{MAX_MESSAGES, UID_VALIDITY, with_bcc};
/// Asks Graph for ids that stay the same when a message moves.
pub const IMMUTABLE_IDS: (&str, &str) = ("Prefer", "IdType=\"ImmutableId\"");
/// How deep folders inside folders are read.
pub const MAX_DEPTH: usize = 10;
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
fn listed(v: &Value) -> Option<(String, Vec<String>)> {
    let id = text(v, "id")?;
    let mut flags = Vec::new();
    if v.get("isRead").and_then(Value::as_bool) == Some(true) {
        flags.push("\\Seen".to_string());
    }
    if v.get("flag").and_then(|f| f.get("flagStatus")).and_then(Value::as_str) == Some("flagged") {
        flags.push("\\Flagged".to_string());
    }
    Some((id.to_string(), flags))
}

/// A Graph connection as a [`MailServer`].
pub struct GraphMail<'a, H>(pub &'a mut Graph<H>);

impl<H: Http> MailServer for GraphMail<'_, H> {
    fn name(&self) -> &'static str {
        "Microsoft Graph"
    }

    fn folders(&mut self, max: usize) -> Result<Vec<Mailbox>> {
        folders(self.0, max)
    }

    fn list(&mut self, folder: &str) -> Result<Vec<(String, Vec<String>)>> {
        let list = self.0.list(&format!("me/mailFolders/{}/messages?{MESSAGE_FIELDS}", segment(folder)), &[IMMUTABLE_IDS], MAX_MESSAGES)?;
        Ok(list.items.iter().filter_map(listed).collect())
    }

    fn fetch(&mut self, id: &str) -> Result<Vec<u8>> {
        self.0.raw("GET", &format!("me/messages/{}/$value", segment(id)), &[IMMUTABLE_IDS], b"", "")
    }

    fn set_flags(&mut self, id: &str, seen: bool, flagged: bool) -> Result<()> {
        let body = json!({ "isRead": seen, "flag": { "flagStatus": if flagged { "flagged" } else { "notFlagged" } } });
        self.0.call("PATCH", &format!("me/messages/{}", segment(id)), &[IMMUTABLE_IDS], Some(&body)).map(|_| ())
    }

    fn move_to(&mut self, id: &str, folder: &str) -> Result<()> {
        self.0.call("POST", &format!("me/messages/{}/move", segment(id)), &[IMMUTABLE_IDS], Some(&json!({ "destinationId": folder }))).map(|_| ())
    }

    fn delete(&mut self, id: &str) -> Result<()> {
        self.0.call("POST", &format!("me/messages/{}/permanentDelete", segment(id)), &[], None).map(|_| ())
    }

    fn send(&mut self, raw: &[u8]) -> Result<()> {
        let body = ibercorreo_mime::base64(raw);
        if body.len() > MAX_SEND_BYTES {
            return Err(Error::Invalid(format!(
                "larger than the {} MB Microsoft Graph sends in one message; make the attachments smaller",
                MAX_SEND_BYTES / (1024 * 1024)
            )));
        }
        self.0.raw("POST", "me/sendMail", &[], body.as_bytes(), "text/plain").map(|_| ())
    }
}

/// Send local changes, then fetch the account's mail ([`crate::server_mail::run_with`]).
pub fn run<H: Http>(g: &mut Graph<H>, state: &State, limits: &Limits) -> Result<Batch> {
    run_with(g, state, limits, &mut |_| {})
}

/// [`run`], telling `progress` as each folder starts and as its new mail comes in.
pub fn run_with<H: Http>(g: &mut Graph<H>, state: &State, limits: &Limits, progress: &mut dyn FnMut(&Progress)) -> Result<Batch> {
    crate::server_mail::run_with(&mut GraphMail(g), state, limits, progress)
}

/// Send each message with `sendMail`; Exchange files it in Sent Items itself.
pub fn send<H: Http>(g: &mut Graph<H>, mail: &[Outgoing]) -> Vec<Sent> {
    crate::server_mail::send(&mut GraphMail(g), mail)
}

#[cfg(test)]
mod tests;
