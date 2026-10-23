//! Tinta Correo's IMAP client, L2, written from RFC 9051 (IMAP4rev2) and RFC 3501 (IMAP4rev1)
//! plus the extensions it uses: MOVE (RFC 6851), UIDPLUS (RFC 4315), LITERAL+ (RFC 7888) and
//! SPECIAL-USE (RFC 6154).
//!
//! It works over any `Read + Write` stream and never opens a socket, so it builds for wasm and
//! is tested against a scripted server. The server is untrusted: every response is size-capped
//! and parsed without panicking (`AGENTS.md` §Never crash).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod client;
pub mod command;
pub mod response;
pub mod utf7;
pub mod wire;

#[cfg(test)]
mod tests;

pub use client::{Client, Done, Items, Selected, is_news, xoauth2};
pub use command::{Command, uid_set};
pub use response::{Code, Fetch, Mailbox, Response, Status, Untagged};

use ibercorreo_core::FolderRole;
use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    /// Reading from or writing to the connection failed.
    #[error("connection: {0}")]
    Io(String),
    /// The server closed the connection.
    #[error("the server closed the connection")]
    Closed,
    /// The server sent something that is not IMAP.
    #[error("protocol: {0}")]
    Protocol(String),
    /// A response is larger than this client accepts.
    #[error("{0} too large")]
    TooLarge(&'static str),
    /// The server refused the command (`NO`), e.g. a wrong password.
    #[error("the server refused: {0}")]
    No(String),
    /// The server did not understand the command (`BAD`).
    #[error("the server rejected the command: {0}")]
    Bad(String),
    /// The server is closing the connection (`BYE`).
    #[error("the server is closing the connection: {0}")]
    Bye(String),
    /// A password would have been sent without encryption.
    #[error("refusing to send a password over an unencrypted connection")]
    Insecure,
    /// An argument (set, flag) is malformed.
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// The server lacks something this needs.
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    /// Nothing arrived before the connection's read timeout. Only [`Client::idle_wait`] reports
    /// it; the session is still usable.
    #[error("the server sent nothing in time")]
    Timeout,
}

impl From<Error> for ibercorreo_core::Error {
    fn from(e: Error) -> Self {
        ibercorreo_core::Error::Network(e.to_string())
    }
}

impl Mailbox {
    /// What the folder is for: the RFC 6154 special-use attribute if the server sends one, else a
    /// guess from common names (English, Spanish, Ukrainian), else [`FolderRole::Other`].
    pub fn role(&self) -> FolderRole {
        if self.raw == "INBOX" {
            return FolderRole::Inbox;
        }
        for (attribute, role) in [
            ("\\Sent", FolderRole::Sent),
            ("\\Drafts", FolderRole::Drafts),
            ("\\Trash", FolderRole::Trash),
            ("\\Junk", FolderRole::Junk),
            ("\\Archive", FolderRole::Archive),
        ] {
            if self.has(attribute) {
                return role;
            }
        }
        let leaf = self.delimiter.and_then(|d| self.name.rsplit(d).next()).unwrap_or(&self.name).to_lowercase();
        match leaf.as_str() {
            "sent" | "sent items" | "sent messages" | "sent mail" | "enviados" | "elementos enviados" | "надіслані" => FolderRole::Sent,
            "drafts" | "borradores" | "чернетки" => FolderRole::Drafts,
            "trash" | "deleted items" | "deleted messages" | "papelera" | "elementos eliminados" | "кошик" | "видалені" => {
                FolderRole::Trash
            }
            "junk" | "spam" | "junk e-mail" | "junk email" | "correo no deseado" | "спам" => FolderRole::Junk,
            "archive" | "archivo" | "архів" => FolderRole::Archive,
            _ => FolderRole::Other,
        }
    }
}
