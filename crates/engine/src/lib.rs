//! The engine, L4: a [`Session`] holds the store, what is selected and the undo history, and every
//! user-visible action is a command in [`commands::REGISTRY`] (id, label, ribbon group, shortcut,
//! parameter doc, `run`). The interface, the CLI and (later) MCP all dispatch by id, so anything a
//! person can click a script can do too. Programmatic calls never open dialogs.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod commands;
pub mod sample;

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use ibercorreo_core::{AccountId, Date, DateTime, Error, EventId, FolderId, FolderRole, MessageId, Result, Zone};
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

/// How much time the calendar shows at once, as Outlook's Day, Work Week, Week and Month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CalendarMode {
    Day,
    WorkWeek,
    Week,
    Month,
}

impl CalendarMode {
    pub fn id(self) -> &'static str {
        match self {
            CalendarMode::Day => "day",
            CalendarMode::WorkWeek => "workWeek",
            CalendarMode::Week => "week",
            CalendarMode::Month => "month",
        }
    }
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
    /// Day, work week, week or month.
    pub calendar_mode: CalendarMode,
    pub search: String,
    /// Set by commands that change the store; cleared by `file.save`.
    pub dirty: bool,
    /// The last status line (what the last command did), for the status bar.
    pub status: String,
    /// Set by `account.sync`: the front end runs the sync (it owns the network) and clears it.
    pub sync_requested: bool,
    /// Show HTML mail as its plain text (`view.plainText`).
    pub plain_text: bool,
    /// The current time in UTC, set by the front end (the engine never reads the clock, so it runs
    /// the same in tests, scripts and wasm).
    now: DateTime,
    /// The computer's zone, set by the front end (UTC until it does).
    system_zone: Zone,
    /// The zone times are shown in: the store's choice, else the computer's. Kept in step with
    /// the store after every command.
    zone: Zone,
    undo: Vec<Store>,
    redo: Vec<Store>,
}

impl Session {
    pub fn new(store: Store, today: Date) -> Session {
        let folder = store.folder_by_role(store.first_account(), FolderRole::Inbox).unwrap_or(FolderId(0));
        let mut s = Session {
            store,
            path: None,
            view: View::Mail,
            folder,
            message: None,
            event: None,
            day: today,
            calendar_mode: CalendarMode::Month,
            search: String::new(),
            dirty: false,
            status: String::new(),
            sync_requested: false,
            plain_text: false,
            now: DateTime::midnight(today),
            system_zone: Zone::utc(),
            zone: Zone::utc(),
            undo: Vec::new(),
            redo: Vec::new(),
        };
        s.refresh_zone();
        s
    }

    /// Run command `id` with `params` (a JSON object; `null` counts as `{}`).
    ///
    /// An unknown id or bad parameter is an `Err`. A panic that escapes a command is turned into an
    /// error and the store is put back as it was: a safety net, not a licence.
    pub fn run(&mut self, id: &str, params: &Value) -> Result<Value> {
        let out = self.run_inner(id, params);
        self.refresh_zone();
        out
    }

    fn run_inner(&mut self, id: &str, params: &Value) -> Result<Value> {
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

    /// What message `id`'s meeting invitation (or reply, or cancellation) says, if it has one
    /// that can be read.
    pub fn invite_info(&self, id: MessageId) -> Option<commands::InviteInfo> {
        commands::invite_info(self, self.store.message(id).ok()?)
    }

    /// The first and last day the calendar shows: the day; Monday to Friday or Sunday of its week;
    /// or the first and last of its month. Weeks start on Monday.
    pub fn calendar_range(&self) -> (Date, Date) {
        let monday = self.day.add_days(-i64::from(self.day.weekday().index())).unwrap_or(self.day);
        let span = |from: Date, days: i64| (from, from.add_days(days).unwrap_or(from));
        match self.calendar_mode {
            CalendarMode::Day => (self.day, self.day),
            CalendarMode::WorkWeek => span(monday, 4),
            CalendarMode::Week => span(monday, 6),
            CalendarMode::Month => {
                let first = self.day.first_of_month();
                span(first, i64::from(ibercorreo_core::date::days_in_month(first.year(), first.month())) - 1)
            }
        }
    }

    /// The zone times are shown in.
    pub fn zone(&self) -> &Zone {
        &self.zone
    }

    /// Tell the session the computer's zone. Front ends call this at start.
    pub fn set_system_zone(&mut self, zone: Zone) {
        self.system_zone = zone;
        self.refresh_zone();
    }

    pub fn system_zone(&self) -> &Zone {
        &self.system_zone
    }

    fn refresh_zone(&mut self) {
        let chosen = self.store.time_zone.as_deref().and_then(Zone::named);
        let zone = chosen.unwrap_or_else(|| self.system_zone.clone());
        if zone != self.zone {
            self.zone = zone;
        }
    }

    /// Now, as wall-clock time in the zone times are shown in.
    pub fn local_now(&self) -> DateTime {
        self.zone.from_utc(self.now)
    }

    /// Today's date where the person is.
    pub fn today(&self) -> Date {
        self.local_now().date
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

    /// The account the open folder belongs to: where new mail is written from, and what
    /// Account settings shows.
    pub fn current_account(&self) -> AccountId {
        self.store.account_of(self.folder).unwrap_or_else(|_| self.store.first_account())
    }

    /// `account`'s folder with `role`, else the current account's.
    pub fn role_folder(&self, account: Option<AccountId>, role: FolderRole) -> Result<FolderId> {
        self.store.folder_by_role(account.unwrap_or_else(|| self.current_account()), role)
    }

    /// What a mail sync of `account` needs from the store: run [`ibercorreo_sync::run`] with it on
    /// a connection (off the interface thread), then give the result to [`Session::apply_sync`].
    pub fn sync_state(&self, account: AccountId) -> ibercorreo_sync::State {
        ibercorreo_sync::state(&self.store, account)
    }

    /// Fold `account`'s sync result into the store. Undo history is cleared: stepping back past a
    /// sync would bring back mail the server has since changed. (Operation-level undo will lift
    /// this; `docs/gaps.md`.)
    pub fn apply_sync(&mut self, account: AccountId, batch: ibercorreo_sync::Batch) -> ibercorreo_sync::Report {
        let report = ibercorreo_sync::apply(&mut self.store, account, batch);
        self.undo.clear();
        self.redo.clear();
        self.dirty = true;
        self.fix_selection();
        let who = if self.store.accounts.len() > 1 {
            self.store.account(account).map(|a| format!(" {}", a.address.email)).unwrap_or_default()
        } else {
            String::new()
        };
        self.status = format!("Synced{who}: {} new, {} updated, {} removed", report.added, report.updated, report.removed);
        if report.remaining > 0 {
            self.status.push_str(&format!(", {} more on the server still to download", report.remaining));
        }
        if !report.errors.is_empty() {
            self.status.push_str(&format!(" ({} problems)", report.errors.len()));
        }
        report
    }

    /// What a calendar sync needs from the store ([`ibercorreo_sync::calendar::run`]).
    pub fn calendar_sync_state(&self) -> ibercorreo_sync::calendar::State {
        ibercorreo_sync::calendar::state(&self.store, self.now)
    }

    /// Fold a calendar sync into the store. Undo history is cleared, as for mail.
    pub fn apply_calendar_sync(&mut self, batch: ibercorreo_sync::calendar::Batch) -> ibercorreo_sync::calendar::Report {
        let report = ibercorreo_sync::calendar::apply(&mut self.store, batch);
        self.undo.clear();
        self.redo.clear();
        self.dirty = true;
        self.fix_selection();
        let changes = report.added.saturating_add(report.updated).saturating_add(report.removed).saturating_add(report.sent);
        let mut line = format!("calendars: {} new, {} updated, {} removed", report.added, report.updated, report.removed);
        if report.sent > 0 {
            line.push_str(&format!(", {} sent", report.sent));
        }
        if report.conflicts > 0 {
            line.push_str(&format!(", {} changed on the server too (the server's copy was kept)", report.conflicts));
        }
        if changes > 0 || !report.errors.is_empty() {
            self.status = if self.status.starts_with("Synced") { format!("{}; {line}", self.status) } else { format!("Synced {line}") };
        }
        if !report.errors.is_empty() {
            self.status.push_str(&format!(" ({} calendar problems)", report.errors.len()));
        }
        report
    }

    /// What a contact sync needs from the store ([`ibercorreo_sync::contacts::run`]).
    pub fn contacts_sync_state(&self) -> ibercorreo_sync::contacts::State {
        ibercorreo_sync::contacts::state(&self.store)
    }

    /// Fold a contact sync into the store. Undo history is cleared, as for mail.
    pub fn apply_contacts_sync(&mut self, batch: ibercorreo_sync::contacts::Batch) -> ibercorreo_sync::contacts::Report {
        let report = ibercorreo_sync::contacts::apply(&mut self.store, batch);
        self.undo.clear();
        self.redo.clear();
        self.dirty = true;
        self.fix_selection();
        let changes = report.added.saturating_add(report.updated).saturating_add(report.removed).saturating_add(report.sent);
        let mut line = format!("contacts: {} new, {} updated, {} removed", report.added, report.updated, report.removed);
        if report.sent > 0 {
            line.push_str(&format!(", {} sent", report.sent));
        }
        if report.conflicts > 0 {
            line.push_str(&format!(", {} changed on the server too (the server's copy was kept)", report.conflicts));
        }
        if changes > 0 || !report.errors.is_empty() {
            self.status = if self.status.starts_with("Synced") { format!("{}; {line}", self.status) } else { format!("Synced {line}") };
        }
        if !report.errors.is_empty() {
            self.status.push_str(&format!(" ({} contact problems)", report.errors.len()));
        }
        report
    }

    /// The server mailbox that holds `account`'s sent copies, once its Sent folder is linked to one.
    pub fn sent_path(&self, account: AccountId) -> Option<String> {
        let sent = self.store.folder_by_role(account, FolderRole::Sent).ok()?;
        self.store.folder(sent).ok()?.remote.as_ref().map(|r| r.path.clone())
    }

    /// `account`'s Outbox, rendered for sending ([`ibercorreo_sync::outbox::send`]). Messages that
    /// cannot be written (no recipients) are left out; `message.send` already refuses those.
    pub fn outbox(&self, account: AccountId) -> Vec<ibercorreo_sync::outbox::Outgoing> {
        let Ok(outbox) = self.store.folder_by_role(account, FolderRole::Outbox) else { return Vec::new() };
        let mut out = Vec::new();
        for m in self.store.messages_in(outbox) {
            let Ok(raw) = commands::render(self, m.id) else { continue };
            let from = m.from.clone().unwrap_or_else(|| commands::the_user(self, account)).email;
            let to = m.to.iter().chain(&m.cc).chain(&m.bcc).map(|a| a.email.clone()).collect();
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
            // Into the Sent or Drafts of the account it was sent from.
            let account = self.store.message(r.id).and_then(|m| self.store.account_of(m.folder));
            let (sent_to, drafts) = match account {
                Ok(a) => (self.store.folder_by_role(a, FolderRole::Sent), self.store.folder_by_role(a, FolderRole::Drafts)),
                Err(e) => (Err(e.clone()), Err(e)),
            };
            let outcome = match &r.outcome {
                Outcome::Sent { saved: true } => self.store.drop_message(r.id).map(drop),
                Outcome::Sent { saved: false } => sent_to.and_then(|to| self.store.move_message(r.id, to)).map(drop),
                Outcome::Later(_) => Ok(()),
                Outcome::Failed(why) => {
                    failed.push(why.clone());
                    drafts.and_then(|to| self.store.move_message(r.id, to)).map(drop)
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
            self.folder = self.store.folder_by_role(self.store.first_account(), FolderRole::Inbox).unwrap_or(FolderId(0));
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
