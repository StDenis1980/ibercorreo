//! Iber Correo's model, L0: what a mailbox, a calendar and an address book hold, independent of
//! any protocol (IMAP, SMTP, CalDAV, EWS) or user interface.
//!
//! Every value here can come from a hostile server or file, so constructors validate and nothing
//! panics: see `AGENTS.md` §Never crash.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod calendar;
pub mod contact;
pub mod date;
pub mod error;
pub mod mail;

pub use calendar::{Calendar, Event, EventId};
pub use contact::Contact;
pub use date::{Date, DateTime, Weekday};
pub use error::{Error, Result};
pub use mail::{Account, Address, Folder, FolderId, FolderRole, Message, MessageId, RemoteFolder, RemoteRef};

/// Upper bound on any single text field taken from input (subject, name, location): 64 KiB.
/// Longer values are cut at a char boundary rather than rejected, and the cut is recorded by the caller.
pub const MAX_FIELD_LEN: usize = 64 * 1024;

/// `s` cut to at most `max` bytes on a char boundary.
pub fn clamp_text(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_text_respects_char_boundaries() {
        assert_eq!(clamp_text("hello", 10), "hello");
        assert_eq!(clamp_text("hello", 3), "hel");
        // "ї" is two bytes: cutting inside it backs off to the boundary before it.
        assert_eq!(clamp_text("aї", 2), "a");
        assert_eq!(clamp_text("ї", 0), "");
    }
}
