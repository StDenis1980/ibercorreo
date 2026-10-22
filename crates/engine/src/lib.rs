//! The engine, L4: a [`Session`] holds the store, what is selected and the undo history, and every
//! user-visible action is a command in [`commands::REGISTRY`] (id, label, ribbon group, shortcut,
//! parameter doc, `run`). The interface, the CLI and (later) MCP all dispatch by id, so anything a
//! person can click a script can do too. Programmatic calls never open dialogs.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod commands;
pub mod sample;

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use ibercorreo_core::{Date, DateTime, Error, EventId, FolderId, FolderRole, MessageId, Result};
use ibercorreo_store::Store;
use serde_json::Value;

/// How many steps undo goes back.
pub const UNDO_DEPTH: usize = 50;

/// Which of the three Outlook-style views is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum View {
    Mail,
    Calendar,
    Contacts,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub store: Store,
    /// Where `file.save` writes; `None` until saved once or opened from a file.
    pub path: Option<PathBuf>,
    pub view: View,
    pub folder: FolderId,
    pub message: Option<MessageId>,
    pub event: Option<EventId>,
    /// The day the calendar view is centred on.
    pub day: Date,
    pub search: String,
    /// Set by commands that change the store; cleared by `file.save`.
    pub dirty: bool,
    /// The last status line (what the last command did), for the status bar.
    pub status: String,
    /// The current time in UTC, set by the front end (the engine never reads the clock, so it runs
    /// the same in tests, scripts and wasm).
    now: DateTime,
    undo: Vec<Store>,
    redo: Vec<Store>,
}

impl Session {
    pub fn new(store: Store, today: Date) -> Session {
        let folder = store.folder_by_role(FolderRole::Inbox).unwrap_or(FolderId(0));
        Session {
            store,
            path: None,
            view: View::Mail,
            folder,
            message: None,
            event: None,
            day: today,
            search: String::new(),
            dirty: false,
            status: String::new(),
            now: DateTime::midnight(today),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Run command `id` with `params` (a JSON object; `null` counts as `{}`).
    ///
    /// An unknown id or bad parameter is an `Err`. A panic that escapes a command is turned into an
    /// error and the store is put back as it was: a safety net, not a licence.
    pub fn run(&mut self, id: &str, params: &Value) -> Result<Value> {
        let spec = commands::find(id).ok_or_else(|| Error::not_found("command", id))?;
        let empty = Value::Object(serde_json::Map::new());
        let params = if params.is_null() { &empty } else { params };
        if !params.is_object() {
            return Err(Error::invalid("params", "expected a JSON object"));
        }
        let before = self.clone();
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| (spec.run)(self, params)));
        match outcome {
            Ok(Ok(v)) => {
                if spec.edits && self.store != before.store {
                    self.undo.push(before.store);
                    if self.undo.len() > UNDO_DEPTH {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                    self.dirty = true;
                }
                Ok(v)
            }
            Ok(Err(e)) => {
                *self = before;
                self.status = e.to_string();
                Err(e)
            }
            Err(_) => {
                *self = before;
                let e = Error::invalid("command", format!("{id} failed unexpectedly; nothing was changed"));
                self.status = e.to_string();
                Err(e)
            }
        }
    }

    pub fn now(&self) -> DateTime {
        self.now
    }

    /// Set the clock (UTC). Front ends call this before running commands.
    pub fn set_now(&mut self, now: DateTime) {
        self.now = now;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn undo(&mut self) -> Result<()> {
        let prev = self.undo.pop().ok_or_else(|| Error::invalid("undo", "nothing to undo"))?;
        let cur = std::mem::replace(&mut self.store, prev);
        self.redo.push(cur);
        self.fix_selection();
        self.dirty = true;
        Ok(())
    }

    fn redo(&mut self) -> Result<()> {
        let next = self.redo.pop().ok_or_else(|| Error::invalid("redo", "nothing to redo"))?;
        let cur = std::mem::replace(&mut self.store, next);
        self.undo.push(cur);
        self.fix_selection();
        self.dirty = true;
        Ok(())
    }

    /// What a mail sync needs from the store: run [`ibercorreo_sync::run`] with it on a connection
    /// (off the interface thread), then give the result to [`Session::apply_sync`].
    pub fn sync_state(&self) -> ibercorreo_sync::State {
        ibercorreo_sync::state(&self.store)
    }

    /// Fold a sync result into the store. Undo history is cleared: stepping back past a sync would
    /// bring back mail the server has since changed. (Operation-level undo will lift this; `docs/gaps.md`.)
    pub fn apply_sync(&mut self, batch: ibercorreo_sync::Batch) -> ibercorreo_sync::Report {
        let report = ibercorreo_sync::apply(&mut self.store, batch);
        self.undo.clear();
        self.redo.clear();
        self.dirty = true;
        self.fix_selection();
        self.status = format!("Synced: {} new, {} updated, {} removed", report.added, report.updated, report.removed);
        if !report.errors.is_empty() {
            self.status.push_str(&format!(" ({} problems)", report.errors.len()));
        }
        report
    }

    /// The Outbox, rendered for sending ([`ibercorreo_sync::outbox::send`]). Messages that cannot be
    /// written (no recipients) are left out; `message.send` already refuses those.
    pub fn outbox(&self) -> Vec<ibercorreo_sync::outbox::Outgoing> {
        let Ok(outbox) = self.store.folder_by_role(FolderRole::Outbox) else { return Vec::new() };
        let mut out = Vec::new();
        for m in self.store.messages_in(outbox) {
            let Ok(raw) = commands::render(self, m.id) else { continue };
            let from = m.from.clone().unwrap_or_else(|| commands::the_user(self)).email;
            let to = m.to.iter().chain(&m.cc).map(|a| a.email.clone()).collect();
            out.push(ibercorreo_sync::outbox::Outgoing { id: m.id, from, to, raw: raw.into_bytes() });
        }
        out
    }

    /// Record what sending did: sent mail leaves the Outbox (into Sent, or dropped when the
    /// server's Sent has the copy, which the next sync brings back), mail refused for good goes
    /// back to Drafts, the rest stays queued. Undo history is cleared, so undo can never put a sent
    /// message back in the Outbox to be sent twice.
    pub fn apply_sent(&mut self, results: &[ibercorreo_sync::outbox::Sent]) {
        use ibercorreo_sync::outbox::Outcome;
        let (mut sent, mut later, mut failed) = (0usize, 0usize, Vec::new());
        for r in results {
            let outcome = match &r.outcome {
                Outcome::Sent { saved: true } => self.store.drop_message(r.id).map(drop),
                Outcome::Sent { saved: false } => {
                    self.store.folder_by_role(FolderRole::Sent).and_then(|to| self.store.move_message(r.id, to)).map(drop)
                }
                Outcome::Later(_) => Ok(()),
                Outcome::Failed(why) => {
                    failed.push(why.clone());
                    self.store.folder_by_role(FolderRole::Drafts).and_then(|to| self.store.move_message(r.id, to)).map(drop)
                }
            };
            match (&r.outcome, outcome) {
                (Outcome::Sent { .. }, Ok(())) => {
                    sent = sent.saturating_add(1);
                    if let Ok(m) = self.store.message_mut(r.id) {
                        m.seen = true;
                    }
                }
                (Outcome::Later(_), _) => later = later.saturating_add(1),
                _ => {}
            }
        }
        self.undo.clear();
        self.redo.clear();
        self.dirty = true;
        self.fix_selection();
        self.status = format!("Sent {sent}");
        if later > 0 {
            self.status.push_str(&format!(", {later} waiting in Outbox"));
        }
        if let Some(why) = failed.first() {
            self.status.push_str(&format!(", {} back in Drafts: {why}", failed.len()));
        }
    }

    /// Drop a selection that no longer exists (after undo, delete or a move).
    fn fix_selection(&mut self) {
        if self.store.folder(self.folder).is_err() {
            self.folder = self.store.folder_by_role(FolderRole::Inbox).unwrap_or(FolderId(0));
        }
        if let Some(m) = self.message
            && self.store.message(m).map(|m| m.folder != self.folder).unwrap_or(true)
        {
            self.message = None;
        }
        if let Some(e) = self.event
            && self.store.event(e).is_err()
        {
            self.event = None;
        }
    }
}

#[cfg(test)]
mod tests;
