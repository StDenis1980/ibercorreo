//! Sending the Outbox: each message goes out over SMTP, then a copy is stored in the server's
//! Sent mailbox (IMAP `APPEND`). Like the rest of sync this runs on connections only; the store
//! is changed afterwards from the [`Sent`] results.

use std::io::{Read, Write};

use ibercorreo_core::MessageId;
use serde::Serialize;

/// One message ready to go: the envelope and the exact bytes sent (and saved to Sent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub id: MessageId,
    pub from: String,
    pub to: Vec<String>,
    pub raw: Vec<u8>,
}

/// What happened to one message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Outcome {
    /// Delivered to the server; `saved` says whether the copy reached the server's Sent mailbox.
    Sent { saved: bool },
    /// Refused for now (4xx, connection lost): it stays in the Outbox for the next try.
    Later(String),
    /// Refused for good (5xx, bad address, too large): it goes back to Drafts to be fixed.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sent {
    pub id: MessageId,
    pub outcome: Outcome,
}

/// Send each message. After a lost connection the rest are left for later rather than tried.
pub fn send<S: Read + Write>(client: &mut ibercorreo_smtp::Client<S>, mail: &[Outgoing]) -> Vec<Sent> {
    let mut out = Vec::with_capacity(mail.len());
    let mut lost: Option<String> = None;
    for m in mail {
        if let Some(why) = &lost {
            out.push(Sent { id: m.id, outcome: Outcome::Later(why.clone()) });
            continue;
        }
        let to: Vec<&str> = m.to.iter().map(String::as_str).collect();
        let outcome = match client.send(&m.from, &to, &m.raw) {
            Ok(()) => Outcome::Sent { saved: false },
            Err(e @ (ibercorreo_smtp::Error::Io(_) | ibercorreo_smtp::Error::Closed)) => {
                lost = Some(e.to_string());
                Outcome::Later(e.to_string())
            }
            Err(e) if e.is_temporary() => Outcome::Later(e.to_string()),
            Err(e) => Outcome::Failed(e.to_string()),
        };
        out.push(Sent { id: m.id, outcome });
    }
    out
}

/// Store a copy of each sent message in the server mailbox `sent_path`, marked read. Servers that
/// save sent mail themselves (Gmail, Microsoft 365) would then hold two copies: an account setting
/// will turn this off (`docs/gaps.md`).
pub fn save_sent<S: Read + Write>(client: &mut ibercorreo_imap::Client<S>, sent_path: &str, mail: &[Outgoing], results: &mut [Sent]) {
    for r in results.iter_mut() {
        let Outcome::Sent { saved } = &mut r.outcome else { continue };
        if let Some(m) = mail.iter().find(|m| m.id == r.id) {
            *saved = client.append(sent_path, &["\\Seen"], &m.raw).is_ok();
        }
    }
}
