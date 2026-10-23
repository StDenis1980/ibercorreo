//! Iber Correo's front end, L6: an Outlook-style window (navigation rail, folder pane, message
//! list, reading pane; day, week and month calendar; people) over [`ibercorreo_engine::Session`].
//!
//! Thin by rule: every change goes through `Session::run` with a command id, so the interface
//! never edits the store itself. Swappable: nothing below this crate depends on egui.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod icons;
pub mod theme;

use egui::{Align, Color32, CornerRadius, Frame, Key, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use ibercorreo_core::calendar::{MINUTES_PER_DAY, When};
use ibercorreo_core::mail::{OAUTH_PROVIDERS, OAuthProvider};
use ibercorreo_core::rich::{BlockKind, Rich, Span};
use ibercorreo_core::{Date, FolderRole, MessageId};
use ibercorreo_engine::{CalendarMode, Session, View};
use serde_json::{Value, json};

use icons::Icon;
use theme::Tokens;

/// The product name shown in the window.
pub const NAME: &str = "Iber Correo";

#[derive(Default)]
struct Draft {
    id: u64,
    to: String,
    cc: String,
    subject: String,
    body: String,
}

#[derive(Default)]
struct EventForm {
    open: bool,
    summary: String,
    start: String,
    end: String,
    location: String,
    /// `repeat` for `event.add`: none, daily, weekdays, weekly, monthly, yearly.
    repeat: String,
    /// `reminder` for `event.add`, minutes or `none`; empty: the default (15 minutes).
    reminder: String,
    /// The calendar chosen; `None`: the one used last.
    calendar: Option<u32>,
}

/// The contact card on the right of People: a new contact (`id` none) or the one selected.
#[derive(Default)]
struct ContactForm {
    id: Option<u64>,
    name: String,
    email: String,
    /// (kind, number) rows.
    phones: Vec<(String, String)>,
    organization: String,
    title: String,
    birthday: String,
    note: String,
    /// The address book it is in, or goes to.
    book: u32,
}

impl ContactForm {
    /// An empty card for a contact in `book`.
    fn new_in(book: u32) -> ContactForm {
        ContactForm { book, ..ContactForm::default() }
    }

    fn of(c: &ibercorreo_core::Contact) -> ContactForm {
        ContactForm {
            id: Some(c.id),
            name: c.name.clone(),
            email: c.emails.iter().map(|a| a.email.clone()).collect::<Vec<_>>().join(", "),
            phones: c.phones.iter().map(|p| (p.kind.clone(), p.number.clone())).collect(),
            organization: c.organization.clone().unwrap_or_default(),
            title: c.title.clone().unwrap_or_default(),
            birthday: c.birthday.map(|b| b.to_string()).unwrap_or_default(),
            note: c.note.clone(),
            book: c.book,
        }
    }

    fn params(&self) -> Value {
        let phones: Vec<Value> = self.phones.iter().filter(|(_, n)| !n.trim().is_empty()).map(|(k, n)| json!({ "kind": k, "number": n })).collect();
        let mut p = json!({
            "name": self.name, "email": self.email, "phones": phones, "organization": self.organization,
            "title": self.title, "birthday": self.birthday, "note": self.note, "book": self.book,
        });
        if let (Some(id), Some(o)) = (self.id, p.as_object_mut()) {
            o.insert("id".into(), json!(id));
        }
        p
    }
}

fn phone_kind_label(kind: &str) -> &'static str {
    match kind {
        "mobile" => "Mobile",
        "work" => "Work",
        "home" => "Home",
        "fax" => "Fax",
        _ => "Other",
    }
}

/// The Account settings dialog. The password is held only while the dialog is open and then
/// handed to the app shell ([`App::take_password`]), which keeps it in the system's credential store.
#[derive(Default)]
struct AccountForm {
    open: bool,
    name: String,
    email: String,
    user: String,
    password: String,
    imap_host: String,
    imap_port: String,
    imap_starttls: bool,
    smtp_host: String,
    smtp_port: String,
    smtp_starttls: bool,
    caldav: String,
    /// The OAuth provider signed in with (its id), or empty for a password.
    oauth: String,
    /// Why the dialog opened by itself (a missing or refused password), or what Find settings found.
    note: String,
}

/// Server settings found for an address (by the shell, which can reach the network), to fill
/// Account settings with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The provider's name.
    pub provider: String,
    /// Where they came from: a configuration file's address, or `None` for the usual server names.
    pub source: Option<String>,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_starttls: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_starttls: bool,
    /// The sign-in name.
    pub user: String,
    /// The provider takes OAuth2 sign-in only: a plain password will be refused.
    pub oauth_only: bool,
}

pub struct App {
    pub session: Session,
    /// The calendar the last appointment went into, the default for the next.
    last_calendar: Option<u32>,
    /// Set by the shell while a Send/Receive runs in the background.
    pub syncing: bool,
    /// An address whose server settings the person asked to find; the shell takes it.
    pub autoconfig_request: Option<String>,
    /// Set by the shell while it looks for server settings.
    pub autoconfig_running: bool,
    /// The OAuth providers this build can sign in with (ids), set by the shell.
    pub oauth_ready: Vec<String>,
    /// Account settings asks the shell to sign in in the browser (the saved account's provider).
    pub oauth_request: bool,
    /// Account settings asks the shell to stop waiting for the browser.
    pub oauth_cancel: bool,
    /// Set by the shell while the browser sign-in runs: the address it opened.
    pub oauth_running: Option<String>,
    account_form: AccountForm,
    password: Option<String>,
    tokens: Tokens,
    draft: Option<Draft>,
    event_form: EventForm,
    contact_form: ContactForm,
    contact_query: String,
    /// The address book People shows; `None`: all of them.
    contact_book: Option<u32>,
    search: String,
    /// Scroll the day and week grid to the working morning on its next frame.
    scroll_to_morning: bool,
    /// The time zone picker's search text, and every zone id (read once, when first opened).
    zone_filter: String,
    zone_names: Vec<String>,
    /// Reminders due, as `reminder.list` gave them, and when (egui time) that was asked.
    due: Vec<Value>,
    due_checked: Option<f64>,
    /// The open message's invitation, read once per message (and again after anything runs).
    invite: Option<(ibercorreo_core::MessageId, Option<ibercorreo_engine::commands::InviteInfo>)>,
    /// How an answer to an invitation goes: 0 sent now, 1 edited first, 2 not at all.
    invite_reply: usize,
}

impl App {
    pub fn new(session: Session) -> App {
        App {
            session,
            syncing: false,
            autoconfig_request: None,
            autoconfig_running: false,
            oauth_ready: Vec::new(),
            oauth_request: false,
            oauth_cancel: false,
            oauth_running: None,
            account_form: AccountForm::default(),
            password: None,
            tokens: Tokens::light(),
            draft: None,
            event_form: EventForm::default(),
            contact_form: ContactForm::default(),
            contact_query: String::new(),
            contact_book: None,
            search: String::new(),
            scroll_to_morning: true,
            zone_filter: String::new(),
            zone_names: Vec::new(),
            due: Vec::new(),
            due_checked: None,
            last_calendar: None,
            invite: None,
            invite_reply: 0,
        }
    }

    /// A password typed in Account settings, for the shell to store. Taken once.
    pub fn take_password(&mut self) -> Option<String> {
        self.password.take()
    }

    /// Open Account settings, with a note saying why (`""` when the person asked for it).
    pub fn open_account_settings(&mut self, note: &str) {
        let v = self.session.run("account.get", &Value::Null).unwrap_or(Value::Null);
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let port = |k: &str| v.get(k).and_then(Value::as_u64).map(|n| n.to_string()).unwrap_or_default();
        self.account_form = AccountForm {
            open: true,
            name: text("displayName"),
            email: text("email"),
            user: text("user"),
            password: String::new(),
            imap_host: text("imapHost"),
            imap_port: port("imapPort"),
            imap_starttls: text("imapSecurity") == "starttls",
            smtp_host: text("smtpHost"),
            smtp_port: port("smtpPort"),
            smtp_starttls: text("smtpSecurity") == "starttls",
            caldav: text("caldav"),
            oauth: text("oauth"),
            note: note.to_string(),
        };
        if self.account_form.user == self.account_form.email {
            self.account_form.user.clear();
        }
    }

    /// Fill Account settings with what the shell found for the address (or say what failed).
    pub fn apply_autoconfig(&mut self, found: Result<Discovered, String>) {
        let f = &mut self.account_form;
        match found {
            Ok(d) => {
                f.imap_host = d.imap_host;
                f.imap_port = d.imap_port.to_string();
                f.imap_starttls = d.imap_starttls;
                f.smtp_host = d.smtp_host;
                f.smtp_port = d.smtp_port.to_string();
                f.smtp_starttls = d.smtp_starttls;
                f.user = if d.user.eq_ignore_ascii_case(f.email.trim()) { String::new() } else { d.user };
                let provider = OAuthProvider::for_host(&f.imap_host);
                let ready = provider.filter(|p| self.oauth_ready.iter().any(|r| r == p.id));
                f.oauth = ready.map(|p| p.id.to_string()).unwrap_or_default();
                f.note = match (ready, provider, d.source) {
                    (Some(p), _, _) => {
                        format!("Settings for {} found. Check them and save: you then sign in on {}'s own page in your browser.", d.provider, p.name)
                    }
                    (None, Some(p), _) if d.oauth_only => format!(
                        "Settings for {} found, but {} only takes sign-in on its own page (OAuth), and this build has no {} client ID for it.",
                        d.provider, p.name, p.name
                    ),
                    (None, _, Some(_)) => format!("Settings for {} found. Check them, enter the password and save.", d.provider),
                    (None, _, None) => "No published settings: these servers answered at the usual names. Check them and save.".to_string(),
                };
            }
            Err(e) => f.note = format!("No settings found: {e}. Enter the servers by hand."),
        }
    }

    /// What the shell's browser sign-in came to: the provider's name, or why it failed.
    pub fn apply_oauth(&mut self, result: Result<String, String>) {
        self.oauth_running = None;
        match result {
            Ok(name) => {
                self.account_form.open = false;
                self.account_form.note.clear();
                self.session.status = format!("Signed in with {name}");
                self.run("account.sync", json!({}));
            }
            Err(e) => self.account_form.note = format!("The sign-in did not finish: {e}."),
        }
    }

    fn account_window(&mut self, ui: &mut Ui) {
        if !self.account_form.open {
            return;
        }
        let mut open = true;
        let mut save = false;
        egui::Window::new("Account settings")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                let f = &mut self.account_form;
                if !f.note.is_empty() {
                    ui.label(RichText::new(&f.note).color(self.tokens.unread));
                    ui.add_space(4.0);
                }
                egui::Grid::new("account-form").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    for (label, field, hint) in [("Your name", &mut f.name, "shown to recipients"), ("Email address", &mut f.email, "me@example.org")]
                    {
                        ui.label(label);
                        ui.add(egui::TextEdit::singleline(field).hint_text(hint).desired_width(280.0));
                        ui.end_row();
                    }
                    ui.label("");
                    let label = if self.autoconfig_running { "🔍 Looking…" } else { "🔍 Find settings" };
                    let can = !self.autoconfig_running && f.email.contains('@');
                    if ui
                        .add_enabled(can, egui::Button::new(label))
                        .on_hover_text("Look up the servers for this address (the provider's published settings, then the usual names)")
                        .clicked()
                    {
                        self.autoconfig_request = Some(f.email.trim().to_string());
                    }
                    ui.end_row();
                    ui.label("Sign in with");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut f.oauth, String::new(), "Password");
                        for p in OAUTH_PROVIDERS {
                            let ready = self.oauth_ready.iter().any(|r| r == p.id);
                            let hover = if ready {
                                format!("Sign in on {}'s own page in your browser (OAuth 2.0): no password is kept here", p.name)
                            } else {
                                format!("This build has no {} client ID for OAuth sign-in", p.name)
                            };
                            ui.add_enabled_ui(ready, |ui| ui.radio_value(&mut f.oauth, p.id.to_string(), p.name))
                                .inner
                                .on_hover_text(&hover)
                                .on_disabled_hover_text(hover);
                        }
                    });
                    ui.end_row();
                    if f.oauth.is_empty() {
                        ui.label("Password");
                        ui.add(
                            egui::TextEdit::singleline(&mut f.password)
                                .password(true)
                                .hint_text("kept in the system's credential store")
                                .desired_width(280.0),
                        );
                        ui.end_row();
                    }
                    ui.label("Sign-in name");
                    ui.add(egui::TextEdit::singleline(&mut f.user).hint_text("if not the email address").desired_width(280.0));
                    ui.end_row();
                    for (title, host, port, starttls, tls_port, starttls_port) in [
                        ("Incoming (IMAP)", &mut f.imap_host, &mut f.imap_port, &mut f.imap_starttls, "993", "143"),
                        ("Outgoing (SMTP)", &mut f.smtp_host, &mut f.smtp_port, &mut f.smtp_starttls, "465", "587"),
                    ] {
                        ui.label(RichText::new(title).strong());
                        ui.end_row();
                        ui.label("Server");
                        ui.add(egui::TextEdit::singleline(host).hint_text("mail.example.org").desired_width(280.0));
                        ui.end_row();
                        ui.label("Port");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(port).hint_text(if *starttls { starttls_port } else { tls_port }).desired_width(60.0));
                            ui.radio_value(starttls, false, "TLS");
                            ui.radio_value(starttls, true, "STARTTLS");
                        });
                        ui.end_row();
                    }
                    ui.label(RichText::new("Calendars and contacts").strong())
                        .on_hover_text("A CalDAV and CardDAV server, such as Nextcloud, Radicale or Fastmail");
                    ui.end_row();
                    ui.label("Address");
                    ui.add(egui::TextEdit::singleline(&mut f.caldav).hint_text("https://dav.example.org (optional)").desired_width(280.0));
                    ui.end_row();
                });
                ui.add_space(6.0);
                match &self.oauth_running {
                    Some(url) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Waiting for you to sign in in your browser…");
                            if ui.button("Cancel").clicked() {
                                self.oauth_cancel = true;
                            }
                        });
                        if !url.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("No browser opened?").small());
                                if ui.small_button("Copy the sign-in address").clicked() {
                                    ui.ctx().copy_text(url.clone());
                                }
                            });
                        }
                    }
                    None => {
                        let label = if f.oauth.is_empty() { "Save and Send/Receive" } else { "Save and sign in" };
                        save = ui.add(egui::Button::new(RichText::new(label).strong())).clicked();
                    }
                }
            });
        if save {
            let f = &self.account_form;
            let mut p = json!({
                "name": f.name, "email": f.email, "user": f.user,
                "imapHost": f.imap_host, "imapSecurity": if f.imap_starttls { "starttls" } else { "tls" },
                "smtpHost": f.smtp_host, "smtpSecurity": if f.smtp_starttls { "starttls" } else { "tls" },
                "caldav": f.caldav, "oauth": f.oauth,
            });
            if let Some(o) = p.as_object_mut() {
                for (k, v) in [("imapPort", &f.imap_port), ("smtpPort", &f.smtp_port)] {
                    if !v.trim().is_empty() {
                        o.insert(k.into(), Value::String(v.trim().to_string()));
                    }
                }
            }
            if self.run("account.set", p).is_some() {
                if !self.account_form.oauth.is_empty() {
                    // Stays open while the browser sign-in runs; Send/Receive follows it.
                    self.account_form.password.clear();
                    self.account_form.note.clear();
                    self.oauth_request = true;
                } else {
                    if !self.account_form.password.is_empty() {
                        self.password = Some(std::mem::take(&mut self.account_form.password));
                    }
                    self.run("account.sync", json!({}));
                    open = false;
                }
            }
        }
        if !open {
            self.account_form.password.clear();
            // Closing the dialog gives up a sign-in still waiting for the browser.
            if self.oauth_running.is_some() {
                self.oauth_cancel = true;
            }
        }
        self.account_form.open = open;
    }

    /// Run a command; a failure is shown in the status bar (the session records it).
    fn run(&mut self, id: &str, params: Value) -> Option<Value> {
        let out = self.session.run(id, &params).ok();
        // Anything may have changed what is due, or whether a meeting is in the calendar.
        self.due_checked = None;
        self.invite = None;
        out
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        self.tokens = if ui.visuals().dark_mode { Tokens::dark() } else { Tokens::light() };
        self.shortcuts(ui);
        self.sync_draft();
        let t = self.tokens;
        egui::Panel::left("rail").exact_size(56.0).resizable(false).frame(Frame::NONE.fill(t.rail)).show(ui, |ui| self.rail(ui));
        egui::Panel::top("ribbon").frame(Frame::NONE.fill(t.pane).inner_margin(Margin::symmetric(10, 6))).show(ui, |ui| self.ribbon(ui));
        egui::Panel::bottom("status").frame(Frame::NONE.fill(t.pane).inner_margin(Margin::symmetric(10, 3))).show(ui, |ui| self.status_bar(ui));
        match self.session.view {
            View::Mail => self.mail(ui),
            View::Calendar => self.calendar(ui),
            View::Contacts => self.people(ui),
        }
        self.account_window(ui);
        self.reminder_window(ui);
    }

    /// How often due reminders are looked for while the window is idle.
    const REMINDER_CHECK_SECS: f64 = 15.0;

    /// The reminder window, Outlook-style: what is due, each with Dismiss and Snooze, and
    /// Dismiss All. It opens by itself when a reminder fires.
    fn reminder_window(&mut self, ui: &mut Ui) {
        let now = ui.input(|i| i.time);
        if self.due_checked.is_none_or(|t| now - t >= Self::REMINDER_CHECK_SECS || now < t) {
            self.due = self.session.run("reminder.list", &Value::Null).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            self.due_checked = Some(now);
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(Self::REMINDER_CHECK_SECS));
        if self.due.is_empty() {
            return;
        }
        let t = self.tokens;
        let mut action: Option<(&'static str, Value)> = None;
        let title = if self.due.len() == 1 { "🔔 1 Reminder".to_string() } else { format!("🔔 {} Reminders", self.due.len()) };
        egui::Window::new(title)
            .id(egui::Id::new("reminders"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -40.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(340.0);
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for r in &self.due {
                        let id = r["id"].clone();
                        let start = r["start"].clone();
                        Frame::NONE.fill(t.reading).corner_radius(CornerRadius::same(6)).stroke(Stroke::new(1.0, t.border)).inner_margin(8).show(
                            ui,
                            |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new(r["summary"].as_str().unwrap_or("")).strong().color(t.text));
                                let when = r["start"].as_str().unwrap_or("").replace('T', " ");
                                let place = r["location"].as_str().filter(|l| !l.is_empty()).map(|l| format!(" · 📍 {l}")).unwrap_or_default();
                                ui.label(RichText::new(format!("{when}{place}")).small().color(t.text_weak));
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(due_in(r["minutesUntil"].as_i64().unwrap_or(0))).small().color(t.accent));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.small_button("Dismiss").clicked() {
                                            action = Some(("reminder.dismiss", json!({ "id": id, "start": start })));
                                        }
                                        ui.menu_button("Snooze…", |ui| {
                                            for (minutes, label) in SNOOZES {
                                                if ui.button(label).clicked() {
                                                    action = Some(("reminder.snooze", json!({ "id": id, "start": start, "minutes": minutes })));
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                });
                            },
                        );
                        ui.add_space(4.0);
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Dismiss All").clicked() {
                        action = Some(("reminder.dismiss", json!({})));
                    }
                    if ui.button("Snooze all 5 minutes").clicked() {
                        action = Some(("reminder.snooze", json!({ "minutes": 5 })));
                    }
                });
            });
        if let Some((id, params)) = action {
            self.run(id, params);
        }
    }

    fn shortcuts(&mut self, ui: &mut Ui) {
        // Typing in a field must never trigger a mail action.
        if ui.ctx().memory(|m| m.focused().is_some()) {
            return;
        }
        let pressed_alt = |ui: &Ui, cmd: bool, shift: bool, alt: bool, key: Key| {
            ui.input(|i| i.key_pressed(key) && i.modifiers.command == cmd && i.modifiers.shift == shift && i.modifiers.alt == alt)
        };
        let pressed = |ui: &Ui, cmd: bool, shift: bool, key: Key| pressed_alt(ui, cmd, shift, false, key);
        if pressed(ui, false, false, Key::F9) && !self.syncing {
            self.run("account.sync", json!({}));
        }
        if self.session.view == View::Calendar {
            let calendar: [(bool, Key, &str); 6] = [
                (true, Key::Num1, "calendar.viewDay"),
                (true, Key::Num2, "calendar.viewWorkWeek"),
                (true, Key::Num3, "calendar.viewWeek"),
                (true, Key::Num4, "calendar.viewMonth"),
                (false, Key::ArrowLeft, "calendar.prev"),
                (false, Key::ArrowRight, "calendar.next"),
            ];
            for (cmd, key, id) in calendar {
                if pressed_alt(ui, cmd, false, true, key) {
                    self.run(id, json!({}));
                    self.scroll_to_morning = true;
                }
            }
        }
        let actions: [(bool, bool, Key, &str); 10] = [
            (true, false, Key::N, "message.compose"),
            (true, false, Key::R, "message.reply"),
            (true, true, Key::R, "message.replyAll"),
            (false, false, Key::Delete, "message.delete"),
            (true, false, Key::Num1, "view.mail"),
            (true, false, Key::Num2, "view.calendar"),
            (true, false, Key::Num3, "view.contacts"),
            (true, false, Key::Z, "edit.undo"),
            (true, false, Key::Y, "edit.redo"),
            (true, false, Key::Q, "message.markRead"),
        ];
        for (cmd, shift, key, id) in actions {
            if pressed(ui, cmd, shift, key) {
                self.run(id, json!({}));
            }
        }
    }

    /// Keep the editable draft buffer in step with the open message.
    fn sync_draft(&mut self) {
        let drafts = self.session.store.folder_by_role(FolderRole::Drafts).ok();
        let open = self.session.message.and_then(|id| self.session.store.message(id).ok()).filter(|m| Some(m.folder) == drafts);
        match (open, &self.draft) {
            (Some(m), Some(d)) if d.id == m.id.0 => {}
            (Some(m), _) => {
                let list = |v: &[ibercorreo_core::Address]| v.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                self.draft = Some(Draft { id: m.id.0, to: list(&m.to), cc: list(&m.cc), subject: m.subject.clone(), body: m.body.clone() });
            }
            (None, _) => self.draft = None,
        }
    }

    fn rail(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            for (icon, view, id, tip) in [
                (Icon::Mail, View::Mail, "view.mail", "Mail (Ctrl+1)"),
                (Icon::Calendar, View::Calendar, "view.calendar", "Calendar (Ctrl+2)"),
                (Icon::People, View::Contacts, "view.contacts", "People (Ctrl+3)"),
            ] {
                let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
                let active = self.session.view == view;
                if active || resp.hovered() {
                    ui.painter().rect_filled(rect, 8.0, if active { t.accent } else { t.accent.gamma_multiply(0.35) });
                }
                icons::paint(ui.painter(), rect.shrink(9.0), icon, t.rail_icon);
                if resp.on_hover_text(tip).clicked() {
                    self.run(id, json!({}));
                }
                ui.add_space(6.0);
            }
        });
    }

    fn button(&mut self, ui: &mut Ui, label: &str, id: &str, enabled: bool) {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            self.run(id, json!({}));
        }
    }

    fn ribbon(&mut self, ui: &mut Ui) {
        let has_message = self.session.message.is_some();
        ui.horizontal(|ui| {
            match self.session.view {
                View::Mail => {
                    if ui.add(egui::Button::new(RichText::new("✉ New Email").strong()).fill(self.tokens.accent_soft)).clicked() {
                        self.run("message.compose", json!({}));
                    }
                    ui.separator();
                    self.button(ui, "Reply", "message.reply", has_message);
                    self.button(ui, "Reply All", "message.replyAll", has_message);
                    self.button(ui, "Forward", "message.forward", has_message);
                    ui.separator();
                    self.button(ui, "Delete", "message.delete", has_message);
                    self.button(ui, "Archive", "message.archive", has_message);
                    self.button(ui, "Flag", "message.flag", has_message);
                    self.button(ui, "Unread", "message.markRead", has_message);
                    if has_message && ui.small_button("↺").on_hover_text("Mark as unread").clicked() {
                        self.run("message.markRead", json!({ "read": false }));
                    }
                    ui.separator();
                    let label = if self.syncing { "⟳ Sending and receiving…" } else { "⟳ Send/Receive" };
                    if ui.add_enabled(!self.syncing, egui::Button::new(label)).on_hover_text("Send the Outbox and get new mail (F9)").clicked() {
                        if self.session.store.accounts.first().and_then(|a| a.login()).is_some() {
                            self.run("account.sync", json!({}));
                        } else {
                            self.open_account_settings("Set up your mail server to send and receive.");
                        }
                    }
                    if ui.button("Account…").clicked() {
                        self.open_account_settings("");
                    }
                }
                View::Calendar => {
                    if ui.add(egui::Button::new(RichText::new("+ New Appointment").strong()).fill(self.tokens.accent_soft)).clicked() {
                        self.event_form = EventForm { open: true, start: format!("{}T09:00", self.session.day), ..EventForm::default() };
                    }
                    ui.separator();
                    if ui.button("Today").clicked() {
                        self.run("calendar.goto", json!({ "date": "today" }));
                    }
                    self.button(ui, "◀", "calendar.prev", true);
                    self.button(ui, "▶", "calendar.next", true);
                    ui.label(RichText::new(range_title(&self.session)).heading());
                    ui.separator();
                    for (mode, label, id, tip) in [
                        (CalendarMode::Day, "Day", "calendar.viewDay", "Ctrl+Alt+1"),
                        (CalendarMode::WorkWeek, "Work Week", "calendar.viewWorkWeek", "Ctrl+Alt+2"),
                        (CalendarMode::Week, "Week", "calendar.viewWeek", "Ctrl+Alt+3"),
                        (CalendarMode::Month, "Month", "calendar.viewMonth", "Ctrl+Alt+4"),
                    ] {
                        if ui.selectable_label(self.session.calendar_mode == mode, label).on_hover_text(tip).clicked() {
                            self.run(id, json!({}));
                            self.scroll_to_morning = true;
                        }
                    }
                    ui.separator();
                    self.zone_picker(ui);
                    ui.separator();
                    self.sync_button(ui, "Set up your account to sync calendars.");
                }
                View::Contacts => {
                    ui.label(RichText::new("People").heading());
                    ui.separator();
                    self.sync_button(ui, "Set up your account to sync contacts.");
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.button(ui, "Redo", "edit.redo", self.session.can_redo());
                self.button(ui, "Undo", "edit.undo", self.session.can_undo());
                if self.session.view == View::Mail {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search mail").desired_width(220.0));
                    if r.changed() {
                        let q = self.search.clone();
                        self.run("search.messages", json!({ "query": q }));
                    }
                }
            });
        });
    }

    /// Send/Receive, which also syncs the calendars and address books; without an account it
    /// opens the account settings with `setup` as the reason.
    fn sync_button(&mut self, ui: &mut Ui, setup: &str) {
        let label = if self.syncing { "⟳ Syncing…" } else { "⟳ Send/Receive" };
        if ui.add_enabled(!self.syncing, egui::Button::new(label)).on_hover_text("Sync mail, calendars and contacts (F9)").clicked() {
            if self.session.store.accounts.first().and_then(|a| a.login()).is_some() {
                self.run("account.sync", json!({}));
            } else {
                self.open_account_settings(setup);
            }
        }
    }

    /// The zone times are shown in: a button with its name and offset, opening a searchable list.
    fn zone_picker(&mut self, ui: &mut Ui) {
        let zone = self.session.zone().clone();
        let label = format!("🌐 {} ({})", zone.name().replace('_', " "), zone.offset_label(self.session.now()));
        let mut chosen: Option<String> = None;
        // Clicks inside (the search field) keep it open.
        let config = egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        egui::containers::menu::MenuButton::new(label).config(config).ui(ui, |ui| {
            if self.zone_names.is_empty() {
                self.zone_names = ibercorreo_core::zone::all_names();
            }
            let system = self.session.system_zone().name().to_string();
            let follows = self.session.store.time_zone.is_none();
            if ui.selectable_label(follows, format!("Same as this computer ({})", system.replace('_', " "))).clicked() {
                chosen = Some("system".into());
            }
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut self.zone_filter).hint_text("Search: Lisbon, Kyiv, New York…").desired_width(240.0));
            let needle = self.zone_filter.to_lowercase().replace(' ', "_");
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for name in self.zone_names.iter().filter(|n| needle.is_empty() || n.to_lowercase().contains(&needle)).take(400) {
                    if ui.selectable_label(!follows && name == zone.name(), name.replace('_', " ")).clicked() {
                        chosen = Some(name.clone());
                    }
                }
            });
            if chosen.is_some() {
                ui.close();
            }
        });
        if let Some(z) = chosen {
            self.run("calendar.timeZone", json!({ "zone": z }));
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let inbox = self.session.store.folder_by_role(FolderRole::Inbox).ok();
            let unread = inbox.map(|f| self.session.store.unread_count(f)).unwrap_or(0);
            ui.label(RichText::new(format!("Unread in Inbox: {unread}")).small().color(self.tokens.text_weak));
            ui.separator();
            ui.label(RichText::new(&self.session.status).small().color(self.tokens.text_weak));
        });
    }

    // ---- mail ----

    fn mail(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        egui::Panel::left("folders")
            .default_size(190.0)
            .size_range(140.0..=320.0)
            .frame(Frame::NONE.fill(t.pane).inner_margin(8))
            .show(ui, |ui| self.folders(ui));
        egui::Panel::left("list").default_size(360.0).size_range(240.0..=640.0).frame(Frame::NONE.fill(t.list)).show(ui, |ui| self.message_list(ui));
        egui::CentralPanel::no_frame().frame(Frame::NONE.fill(t.reading).inner_margin(16)).show(ui, |ui| {
            if self.draft.is_some() {
                self.compose(ui);
            } else {
                self.reading(ui);
            }
        });
    }

    fn folders(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let account = self.session.store.accounts.first().map(|a| a.address.email.clone()).unwrap_or_else(|| "Local".into());
        ui.label(RichText::new(account).small().color(t.text_weak));
        ui.add_space(4.0);
        let folders: Vec<_> = self.session.store.folders().into_iter().map(|f| (f.id, f.name.clone(), f.role)).collect();
        ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
            for (id, name, role) in folders {
                let unread = self.session.store.unread_count(id);
                let selected = self.session.folder == id && self.search.is_empty();
                let text = if unread > 0 && matches!(role, FolderRole::Inbox | FolderRole::Junk) {
                    RichText::new(format!("{name}   {unread}")).strong().color(t.unread)
                } else {
                    RichText::new(name).color(t.text)
                };
                if ui.add(egui::Button::selectable(selected, text).min_size(vec2(0.0, 26.0))).clicked() {
                    self.search.clear();
                    self.run("folder.select", json!({ "folder": id.0 }));
                }
            }
        });
    }

    fn message_list(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let ids: Vec<MessageId> = if self.search.is_empty() {
            self.session.store.messages_in(self.session.folder).iter().map(|m| m.id).collect()
        } else {
            self.session.store.search(&self.search).iter().map(|m| m.id).collect()
        };
        let title = if self.search.is_empty() {
            self.session.store.folder(self.session.folder).map(|f| f.name.clone()).unwrap_or_default()
        } else {
            format!("Results for “{}”", self.search)
        };
        Frame::NONE.inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.label(RichText::new(title).heading().color(t.text));
        });
        if ids.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("Nothing here").color(t.text_weak)));
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for id in ids {
                let Ok(m) = self.session.store.message(id) else { continue };
                let selected = self.session.message == Some(id);
                let who = if m.folder == self.session.store.folder_by_role(FolderRole::Sent).unwrap_or(m.folder)
                    || m.folder == self.session.store.folder_by_role(FolderRole::Drafts).unwrap_or(m.folder)
                {
                    m.to.first().map(|a| format!("To: {}", a.display())).unwrap_or_else(|| "(no recipients)".into())
                } else {
                    m.from.as_ref().map(|a| a.display().to_string()).unwrap_or_else(|| "(unknown sender)".into())
                };
                let subject = if m.subject.is_empty() { "(no subject)".to_string() } else { m.subject.clone() };
                let when = m.date.map(|d| short_when(self.session.zone().from_utc(d), self.session.today())).unwrap_or_default();
                let (seen, flagged, attachment, preview) = (m.seen, m.flagged, !m.attachments.is_empty(), m.preview(80));
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click());
                let painter = ui.painter_at(rect);
                if selected {
                    painter.rect_filled(rect, 0.0, t.accent_soft);
                } else if resp.hovered() {
                    painter.rect_filled(rect, 0.0, t.accent_soft.gamma_multiply(0.5));
                }
                if !seen {
                    painter.rect_filled(egui::Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, t.unread);
                }
                let x = rect.left() + 14.0;
                let font = |size: f32| egui::FontId::proportional(size);
                let strong = if seen { t.text } else { t.unread };
                painter.text(egui::pos2(x, rect.top() + 8.0), egui::Align2::LEFT_TOP, &who, font(14.0), strong);
                painter.text(egui::pos2(rect.right() - 10.0, rect.top() + 8.0), egui::Align2::RIGHT_TOP, &when, font(11.5), t.text_weak);
                let mut subj = subject;
                if attachment {
                    subj = format!("📎 {subj}");
                }
                painter.text(egui::pos2(x, rect.top() + 27.0), egui::Align2::LEFT_TOP, &subj, font(12.5), if seen { t.text } else { strong });
                painter.text(egui::pos2(x, rect.top() + 44.0), egui::Align2::LEFT_TOP, &preview, font(11.5), t.text_weak);
                if flagged {
                    painter.text(egui::pos2(rect.right() - 10.0, rect.top() + 27.0), egui::Align2::RIGHT_TOP, "⚑", font(13.0), t.flag);
                }
                painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.border.gamma_multiply(0.6)));
                if resp.clicked() {
                    self.run("message.select", json!({ "id": id.0 }));
                }
            }
        });
    }

    fn reading(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        if let Some(id) = self.session.message
            && self.invite.as_ref().is_none_or(|(at, _)| *at != id)
        {
            self.invite = Some((id, self.session.invite_info(id)));
        }
        // Borrowed, not cloned: an HTML body can be megabytes, and this runs every frame.
        let Some(m) = self.session.message.and_then(|id| self.session.store.message(id).ok()) else {
            ui.centered_and_justified(|ui| ui.label(RichText::new("Select a message to read it").color(t.text_weak)));
            return;
        };
        ui.label(RichText::new(if m.subject.is_empty() { "(no subject)" } else { &m.subject }).size(20.0).strong().color(t.text));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let who = m.from.as_ref().map(ToString::to_string).unwrap_or_default();
            ui.label(RichText::new(who).strong().color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(m.date.map(|d| self.session.zone().from_utc(d).to_string().replace('T', " ")).unwrap_or_default())
                        .color(t.text_weak),
                );
            });
        });
        if !m.to.is_empty() {
            ui.label(
                RichText::new(format!("To: {}", m.to.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))).small().color(t.text_weak),
            );
        }
        if !m.cc.is_empty() {
            ui.label(
                RichText::new(format!("Cc: {}", m.cc.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))).small().color(t.text_weak),
            );
        }
        if !m.attachments.is_empty() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for a in &m.attachments {
                    Frame::NONE.stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 4)).show(
                        ui,
                        |ui| {
                            ui.label(RichText::new(format!("📎 {}  ·  {}", a.name, human_size(a.size))).small());
                        },
                    );
                }
            });
        }
        let mut answer: Option<Value> = None;
        if let Some((_, Some(info))) = &self.invite {
            ui.add_space(8.0);
            answer = invite_banner(ui, &t, info, &mut self.invite_reply);
        }
        let mut toggle_plain = false;
        if let Some(rich) = &m.rich {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if rich.remote_images > 0 {
                    let n = rich.remote_images;
                    ui.label(RichText::new(format!("🛡 {n} remote image{} not loaded", if n == 1 { "" } else { "s" })).small().color(t.text_weak))
                        .on_hover_text("Images on other servers can tell the sender that you opened the message, so Iber Correo never loads them.");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    toggle_plain = ui.small_button(if self.session.plain_text { "Show formatted" } else { "Show as plain text" }).clicked();
                });
            });
        }
        ui.add_space(10.0);
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match &m.rich {
            Some(rich) if !self.session.plain_text => rich_body(ui, &t, rich),
            _ => {
                ui.add(egui::Label::new(RichText::new(&m.body).size(14.0).color(t.text)).wrap());
            }
        });
        if toggle_plain {
            self.run("view.plainText", json!({}));
        }
        if let Some(p) = answer {
            let id = if p.get("response").is_some() { "invite.respond" } else { "invite.remove" };
            self.run(id, p);
        }
    }

    fn compose(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let Some(d) = self.draft.as_mut() else { return };
        let mut send = false;
        let mut discard = false;
        ui.horizontal(|ui| {
            send = ui.add(egui::Button::new(RichText::new("Send").strong().color(Color32::WHITE)).fill(t.accent)).clicked();
            discard = ui.button("Discard").clicked();
        });
        ui.add_space(8.0);
        let mut changed = false;
        egui::Grid::new("compose").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            for (label, field) in [("To", &mut d.to), ("Cc", &mut d.cc), ("Subject", &mut d.subject)] {
                ui.label(RichText::new(label).color(t.text_weak));
                changed |= ui.add(egui::TextEdit::singleline(field).desired_width(f32::INFINITY)).lost_focus();
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        changed |= ui.add_sized(ui.available_size(), egui::TextEdit::multiline(&mut d.body)).lost_focus();
        let params = json!({ "id": d.id, "to": d.to, "cc": d.cc, "subject": d.subject, "body": d.body });
        let id = d.id;
        if changed || send {
            self.run("message.update", params);
        }
        if send {
            self.run("message.send", json!({ "id": id }));
        } else if discard {
            self.run("message.delete", json!({ "id": id }));
        }
    }

    // ---- calendar ----

    fn calendar(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        egui::Panel::right("agenda").default_size(300.0).frame(Frame::NONE.fill(t.pane).inner_margin(12)).show(ui, |ui| self.agenda(ui));
        egui::CentralPanel::no_frame().frame(Frame::NONE.fill(t.reading).inner_margin(10)).show(ui, |ui| {
            if self.session.calendar_mode == CalendarMode::Month {
                self.month(ui);
            } else {
                self.time_grid(ui);
            }
        });
        self.event_window(ui);
    }

    fn month(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let first = self.session.day.first_of_month();
        let Ok(grid_start) = first.add_days(-i64::from(first.weekday().index())) else { return };
        let today = self.session.today();
        let names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let avail = ui.available_size();
        let cell_w = (avail.x / 7.0).max(40.0);
        let cell_h = ((avail.y - 24.0) / 6.0).max(40.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for n in names {
                ui.add_sized([cell_w, 20.0], egui::Label::new(RichText::new(n).small().color(t.text_weak)));
            }
        });
        for week in 0..6i64 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for wd in 0..7i64 {
                    let Ok(day) = grid_start.add_days(week * 7 + wd) else { continue };
                    let (rect, resp) = ui.allocate_exact_size(vec2(cell_w, cell_h), Sense::click());
                    let p = ui.painter_at(rect);
                    let in_month = day.month() == first.month();
                    let selected = day == self.session.day;
                    p.rect_filled(
                        rect,
                        0.0,
                        if selected {
                            t.accent_soft
                        } else if in_month {
                            t.reading
                        } else {
                            t.list
                        },
                    );
                    p.rect_stroke(rect, 0.0, Stroke::new(0.5, t.border), StrokeKind::Inside);
                    let num_pos = rect.left_top() + vec2(6.0, 4.0);
                    if day == today {
                        p.circle_filled(num_pos + vec2(8.0, 9.0), 11.0, t.today);
                        p.text(
                            num_pos + vec2(8.0, 9.0),
                            egui::Align2::CENTER_CENTER,
                            day.day().to_string(),
                            egui::FontId::proportional(12.0),
                            Color32::WHITE,
                        );
                    } else {
                        p.text(
                            num_pos,
                            egui::Align2::LEFT_TOP,
                            day.day().to_string(),
                            egui::FontId::proportional(12.0),
                            if in_month { t.text } else { t.text_weak },
                        );
                    }
                    let mut y = rect.top() + 26.0;
                    for o in self.session.store.events_on(day, self.session.zone()) {
                        let e = o.event;
                        if y + 16.0 > rect.bottom() {
                            p.text(egui::pos2(rect.left() + 6.0, y), egui::Align2::LEFT_TOP, "…", egui::FontId::proportional(11.0), t.text_weak);
                            break;
                        }
                        let chip = egui::Rect::from_min_size(egui::pos2(rect.left() + 3.0, y), vec2(rect.width() - 6.0, 16.0));
                        let repeat = if e.rrule.is_some() { "⟳ " } else { "" };
                        let label = match o.when {
                            When::Timed { start, .. } if start.date == day => {
                                format!("{:02}:{:02} {repeat}{}", start.hour(), start.minute(), e.summary)
                            }
                            _ => format!("{repeat}{}", e.summary),
                        };
                        p.rect_filled(chip, 3.0, self.calendar_color(e.calendar).gamma_multiply(0.85));
                        p.with_clip_rect(chip).text(
                            chip.left_center() + vec2(4.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(11.0),
                            Color32::WHITE,
                        );
                        y += 18.0;
                    }
                    if resp.clicked() {
                        self.run("calendar.goto", json!({ "date": day.to_string() }));
                    }
                    if resp.double_clicked() {
                        self.event_form = EventForm { open: true, start: format!("{day}T09:00"), ..EventForm::default() };
                    }
                }
            });
        }
    }

    /// Day, work week and week: a column per day, an all-day strip on top and the hours below,
    /// with overlapping appointments side by side.
    fn time_grid(&mut self, ui: &mut Ui) {
        const GUTTER: f32 = 48.0;
        const HOUR: f32 = 44.0;
        const ALL_DAY_ROW: f32 = 20.0;
        let t = self.tokens;
        let (from, to) = self.session.calendar_range();
        let count = usize::try_from(to.day_number().saturating_sub(from.day_number()).saturating_add(1)).unwrap_or(1).clamp(1, 7);
        let days: Vec<Date> = (0..count).filter_map(|n| from.add_days(i64::try_from(n).unwrap_or(0)).ok()).collect();
        let now = self.session.local_now();
        let col_w = ((ui.available_width() - GUTTER) / days.len().max(1) as f32).max(60.0);
        let x_of = |left: f32, n: usize| left + GUTTER + col_w * n as f32;

        // Day headings.
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        let p = ui.painter_at(head);
        for (n, day) in days.iter().enumerate() {
            let x = x_of(head.left(), n);
            let is_today = *day == now.date;
            let colour = if is_today { t.today } else { t.text };
            p.text(egui::pos2(x + 6.0, head.top() + 2.0), egui::Align2::LEFT_TOP, weekday_abbr(*day), egui::FontId::proportional(11.0), t.text_weak);
            p.text(egui::pos2(x + 6.0, head.top() + 16.0), egui::Align2::LEFT_TOP, day.day().to_string(), egui::FontId::proportional(18.0), colour);
            if *day == self.session.day {
                p.rect_filled(egui::Rect::from_min_size(egui::pos2(x + 2.0, head.bottom() - 3.0), vec2(col_w - 4.0, 3.0)), 1.0, t.accent);
            }
        }

        // The all-day strip: one row per all-day appointment of the busiest day.
        let all_day: Vec<Vec<(String, Date, u32)>> = days
            .iter()
            .map(|d| {
                self.session
                    .store
                    .events_on(*d, self.session.zone())
                    .into_iter()
                    .filter(|o| matches!(o.when, When::AllDay { .. }))
                    .map(|o| (o.event.summary.clone(), *d, o.event.calendar))
                    .collect()
            })
            .collect();
        let rows = all_day.iter().map(Vec::len).max().unwrap_or(0).min(4);
        if rows > 0 {
            let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), ALL_DAY_ROW * rows as f32 + 4.0), Sense::hover());
            let p = ui.painter_at(strip);
            p.rect_filled(strip, 0.0, t.list);
            for (n, list) in all_day.iter().enumerate() {
                for (row, (summary, day, calendar)) in list.iter().take(rows).enumerate() {
                    let chip = egui::Rect::from_min_size(
                        egui::pos2(x_of(strip.left(), n) + 2.0, strip.top() + 2.0 + ALL_DAY_ROW * row as f32),
                        vec2(col_w - 4.0, ALL_DAY_ROW - 2.0),
                    );
                    let r = ui.interact(chip, ui.id().with(("all-day", n, row)), Sense::click()).on_hover_text(summary.as_str());
                    if r.clicked() {
                        self.run("calendar.goto", json!({ "date": day.to_string() }));
                    }
                    p.rect_filled(chip, 3.0, soft(self.calendar_color(*calendar), t.reading));
                    p.with_clip_rect(chip).text(
                        chip.left_center() + vec2(4.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        summary,
                        egui::FontId::proportional(11.0),
                        t.text,
                    );
                }
            }
        }

        let mut scroll = egui::ScrollArea::vertical().id_salt("time-grid").auto_shrink([false, false]);
        if std::mem::take(&mut self.scroll_to_morning) {
            scroll = scroll.vertical_scroll_offset(7.0 * HOUR);
        }
        scroll.show(ui, |ui| {
            let (grid, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0 * HOUR), Sense::click());
            let p = ui.painter_at(grid);
            for (n, day) in days.iter().enumerate() {
                let x = x_of(grid.left(), n);
                let weekend = day.weekday().index() >= 5;
                let column = egui::Rect::from_min_size(egui::pos2(x, grid.top()), vec2(col_w, grid.height()));
                p.rect_filled(
                    column,
                    0.0,
                    if *day == self.session.day {
                        t.accent_soft.gamma_multiply(0.4)
                    } else if weekend {
                        t.list
                    } else {
                        t.reading
                    },
                );
                p.line_segment([egui::pos2(x, grid.top()), egui::pos2(x, grid.bottom())], Stroke::new(0.5, t.border));
            }
            for hour in 0..24u8 {
                let y = grid.top() + HOUR * f32::from(hour);
                p.line_segment([egui::pos2(grid.left() + GUTTER, y), egui::pos2(grid.right(), y)], Stroke::new(0.5, t.border));
                p.line_segment(
                    [egui::pos2(grid.left() + GUTTER, y + HOUR / 2.0), egui::pos2(grid.right(), y + HOUR / 2.0)],
                    Stroke::new(0.5, t.border.gamma_multiply(0.4)),
                );
                if hour > 0 {
                    p.text(
                        egui::pos2(grid.left() + GUTTER - 6.0, y),
                        egui::Align2::RIGHT_CENTER,
                        format!("{hour:02}:00"),
                        egui::FontId::proportional(11.0),
                        t.text_weak,
                    );
                }
            }
            let y_of = |minute: u16| grid.top() + HOUR * f32::from(minute) / 60.0;

            // Clicking a free slot picks its day; double-clicking starts an appointment there.
            if let Some(pos) = resp.interact_pointer_pos()
                && pos.x >= grid.left() + GUTTER
            {
                let n = ((pos.x - grid.left() - GUTTER) / col_w).floor();
                let half_hours = ((pos.y - grid.top()) / (HOUR / 2.0)).floor().clamp(0.0, 47.0);
                if let Some(day) = days.get(n.max(0.0) as usize) {
                    if resp.double_clicked() {
                        let minute = half_hours as u16 * 30;
                        self.event_form =
                            EventForm { open: true, start: format!("{day}T{:02}:{:02}", minute / 60, minute % 60), ..EventForm::default() };
                    } else if resp.clicked() {
                        self.run("calendar.goto", json!({ "date": day.to_string() }));
                    }
                }
            }

            for (n, day) in days.iter().enumerate() {
                let occurrences: Vec<_> = self
                    .session
                    .store
                    .events_on(*day, self.session.zone())
                    .into_iter()
                    .filter_map(|o| o.when.minutes_on(*day).map(|m| (o.event.clone(), o.when, m)))
                    .collect();
                let spans: Vec<(u16, u16)> = occurrences.iter().map(|(_, _, m)| *m).collect();
                let x = x_of(grid.left(), n);
                for ((e, when, (start, end)), (column, columns)) in occurrences.iter().zip(ibercorreo_core::calendar::columns(&spans)) {
                    let w = (col_w - 6.0) / columns.max(1) as f32;
                    let top = y_of(*start);
                    let bottom = y_of(*end).max(top + 20.0);
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(x + 2.0 + w * column as f32, top + 1.0),
                        egui::pos2(x + 2.0 + w * (column + 1) as f32 - 2.0, bottom - 1.0),
                    );
                    let times = match when {
                        When::Timed { start, end, .. } => {
                            format!("{:02}:{:02} – {:02}:{:02}", start.hour(), start.minute(), end.hour(), end.minute())
                        }
                        When::AllDay { .. } => String::new(),
                    };
                    let tip = if e.location.is_empty() {
                        format!("{}\n{times}", e.summary)
                    } else {
                        format!("{}\n{times}\n📍 {}", e.summary, e.location)
                    };
                    let r = ui.interact(rect, ui.id().with(("event", e.id.0, n)), Sense::click()).on_hover_text(tip);
                    if r.clicked() {
                        self.run("calendar.goto", json!({ "date": day.to_string() }));
                    }
                    let color = self.calendar_color(e.calendar);
                    p.rect_filled(rect, 3.0, soft(color, t.reading));
                    p.rect_filled(egui::Rect::from_min_size(rect.left_top(), vec2(3.0, rect.height())), 1.0, color);
                    let clip = p.with_clip_rect(rect.shrink(1.0));
                    let repeat = if e.rrule.is_some() { "⟳ " } else { "" };
                    let title = format!("{repeat}{}", e.summary);
                    if rect.height() < 30.0 {
                        clip.text(rect.left_center() + vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(11.5), t.text);
                    } else {
                        clip.text(rect.left_top() + vec2(6.0, 2.0), egui::Align2::LEFT_TOP, title, egui::FontId::proportional(11.5), t.text);
                    }
                    if rect.height() > 32.0 {
                        let detail = if e.location.is_empty() { times } else { e.location.clone() };
                        clip.text(rect.left_top() + vec2(6.0, 17.0), egui::Align2::LEFT_TOP, detail, egui::FontId::proportional(10.5), t.text_weak);
                    }
                }
                // The current time, on today's column.
                if *day == now.date {
                    let y = y_of(now.minute_of_day().min(MINUTES_PER_DAY));
                    p.circle_filled(egui::pos2(x, y), 4.0, t.flag);
                    p.line_segment([egui::pos2(x, y), egui::pos2(x + col_w, y)], Stroke::new(1.5, t.flag));
                }
            }
        });
    }

    fn agenda(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let day = self.session.day;
        ui.label(RichText::new(long_date(day)).heading().color(t.text));
        ui.add_space(8.0);
        let events: Vec<_> = self.session.store.events_on(day, self.session.zone()).into_iter().map(|o| (o.event.clone(), o.when)).collect();
        if events.is_empty() {
            ui.label(RichText::new("No appointments").color(t.text_weak));
        }
        for (e, occurrence) in events {
            let when = match occurrence {
                When::AllDay { .. } => "All day".to_string(),
                When::Timed { start, end, .. } => format!("{:02}:{:02} – {:02}:{:02}", start.hour(), start.minute(), end.hour(), end.minute()),
            };
            Frame::NONE.fill(t.reading).corner_radius(CornerRadius::same(6)).stroke(Stroke::new(1.0, t.border)).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(when).small().color(t.accent));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if e.rrule.is_some() {
                            if ui.small_button("🗑 Series").on_hover_text("Delete every occurrence").clicked() {
                                self.run("event.delete", json!({ "id": e.id.0 }));
                            }
                            if ui.small_button("🗑").on_hover_text("Delete this occurrence only").clicked() {
                                self.run("event.delete", json!({ "id": e.id.0, "occurrence": occurrence.start_date().to_string() }));
                            }
                        } else if ui.small_button("🗑").on_hover_text("Delete appointment").clicked() {
                            self.run("event.delete", json!({ "id": e.id.0 }));
                        }
                    });
                });
                ui.label(RichText::new(&e.summary).strong().color(t.text));
                if let Some(r) = &e.rrule {
                    let text = e.rule().map_or_else(|| format!("Repeats ({r}), shown once"), |rule| format!("⟳ {}", describe_rule(&rule)));
                    ui.label(RichText::new(text).small().color(t.text_weak));
                }
                if !e.location.is_empty() {
                    ui.label(RichText::new(format!("📍 {}", e.location)).small().color(t.text_weak));
                }
                if let Some(m) = e.reminders.first() {
                    let text = match *m {
                        0 => "🔔 At start time".to_string(),
                        m if m > 0 => format!("🔔 {} before", due_in(i64::from(m)).trim_start_matches("in ")),
                        m => format!("🔔 {} after the start", due_in(i64::from(m)).trim_end_matches(" overdue")),
                    };
                    ui.label(RichText::new(text).small().color(t.text_weak));
                }
            });
            ui.add_space(6.0);
        }
        self.calendar_list(ui);
    }

    /// The calendars, with their colours; the ones synced from a server are marked.
    fn calendar_list(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        ui.add_space(12.0);
        ui.label(RichText::new("Calendars").strong().color(t.text));
        let calendars: Vec<(u32, String, bool)> = self.session.store.calendars().iter().map(|c| (c.id, c.name.clone(), c.remote.is_some())).collect();
        for (id, name, synced) in calendars {
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                ui.painter().rect_filled(dot, 3.0, self.calendar_color(id));
                ui.label(RichText::new(name).color(t.text));
                if synced {
                    ui.label(RichText::new("⟳").small().color(t.text_weak)).on_hover_text("Synced with the CalDAV server at Send/Receive");
                }
            });
        }
    }

    /// The colour of calendar `id` (`#RRGGBB`), or the accent when it has none.
    fn calendar_color(&self, id: u32) -> Color32 {
        self.session.store.calendar(id).ok().and_then(|c| parse_hex_color(&c.color)).unwrap_or(self.tokens.accent)
    }

    fn event_window(&mut self, ui: &mut Ui) {
        if !self.event_form.open {
            return;
        }
        let mut open = true;
        let mut save = false;
        let calendars: Vec<(u32, String, bool)> = self.session.store.calendars().iter().map(|c| (c.id, c.name.clone(), c.remote.is_some())).collect();
        let default_calendar = self.last_calendar.filter(|id| calendars.iter().any(|c| c.0 == *id)).or_else(|| calendars.first().map(|c| c.0));
        egui::Window::new("New Appointment").collapsible(false).resizable(false).open(&mut open).show(ui.ctx(), |ui| {
            let f = &mut self.event_form;
            egui::Grid::new("event-form").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                for (label, field, hint) in [
                    ("Title", &mut f.summary, ""),
                    ("Start", &mut f.start, "2026-10-22T09:00 or 2026-10-22"),
                    ("End", &mut f.end, "optional"),
                    ("Location", &mut f.location, ""),
                ] {
                    ui.label(label);
                    ui.add(egui::TextEdit::singleline(field).hint_text(hint).desired_width(260.0));
                    ui.end_row();
                }
                ui.label("Reminder");
                let shown = REMINDERS.iter().find(|(id, _)| *id == f.reminder.as_str()).map_or("15 minutes", |(_, label)| *label);
                egui::ComboBox::from_id_salt("event-reminder").selected_text(shown).show_ui(ui, |ui| {
                    for (id, label) in REMINDERS {
                        ui.selectable_value(&mut f.reminder, id.to_string(), label);
                    }
                });
                ui.end_row();
                if calendars.len() > 1 {
                    ui.label("Calendar");
                    let chosen = f.calendar.or(default_calendar);
                    let shown = calendars.iter().find(|c| Some(c.0) == chosen).map_or(String::new(), |c| c.1.clone());
                    egui::ComboBox::from_id_salt("event-calendar").selected_text(shown).show_ui(ui, |ui| {
                        for (id, name, synced) in &calendars {
                            let label = if *synced { format!("{name}  ⟳") } else { name.clone() };
                            if ui.selectable_label(chosen == Some(*id), label).clicked() {
                                f.calendar = Some(*id);
                            }
                        }
                    });
                    ui.end_row();
                }
                ui.label("Repeat");
                let shown = REPEATS.iter().find(|(id, _)| *id == f.repeat.as_str()).map_or("Never", |(_, label)| *label);
                egui::ComboBox::from_id_salt("event-repeat").selected_text(shown).show_ui(ui, |ui| {
                    for (id, label) in REPEATS {
                        ui.selectable_value(&mut f.repeat, id.to_string(), label);
                    }
                });
                ui.end_row();
            });
            save = ui.button("Save").clicked();
        });
        if save {
            let f = &self.event_form;
            let mut p = json!({ "summary": f.summary, "start": f.start, "location": f.location, "repeat": f.repeat });
            if !f.reminder.is_empty()
                && let Some(o) = p.as_object_mut()
            {
                o.insert("reminder".into(), Value::String(f.reminder.clone()));
            }
            if !f.end.trim().is_empty()
                && let Some(o) = p.as_object_mut()
            {
                o.insert("end".into(), Value::String(f.end.clone()));
            }
            let calendar = f.calendar.or(default_calendar);
            if let (Some(c), Some(o)) = (calendar, p.as_object_mut()) {
                o.insert("calendar".into(), json!(c));
            }
            if self.run("event.add", p).is_some() {
                self.last_calendar = calendar;
                open = false;
            }
        }
        self.event_form.open = open;
    }

    // ---- people ----

    fn people(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        egui::Panel::right("contact-card").default_size(300.0).frame(Frame::NONE.fill(t.pane).inner_margin(12)).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.contact_card(ui));
        });
        egui::CentralPanel::no_frame().frame(Frame::NONE.fill(t.reading).inner_margin(16)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.contact_query).hint_text("Search people").desired_width(320.0));
                if ui.button("➕ New Contact").clicked() {
                    self.contact_form = ContactForm::new_in(self.default_book());
                }
            });
            // The address books, when there is more than one: all, or one of them.
            let books: Vec<(u32, String, bool)> = self.session.store.books().iter().map(|b| (b.id, b.name.clone(), b.remote.is_some())).collect();
            if self.contact_book.is_some_and(|id| !books.iter().any(|b| b.0 == id)) {
                self.contact_book = None;
            }
            if books.len() > 1 {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(self.contact_book.is_none(), "All").clicked() {
                        self.contact_book = None;
                    }
                    for (id, name, synced) in &books {
                        let label = if *synced { format!("⟳ {name}") } else { name.clone() };
                        let r = ui.selectable_label(self.contact_book == Some(*id), label);
                        let r = if *synced { r.on_hover_text("Synced with the CardDAV server at Send/Receive") } else { r };
                        if r.clicked() {
                            self.contact_book = Some(*id);
                        }
                    }
                });
            }
            ui.add_space(8.0);
            let q = self.contact_query.to_lowercase();
            let book = self.contact_book;
            let contacts: Vec<_> = self.session.store.contacts(&q).into_iter().filter(|c| book.is_none_or(|b| c.book == b)).cloned().collect();
            if contacts.is_empty() {
                ui.label(
                    RichText::new(if q.is_empty() { "No contacts yet. Add one, or open a .vcf file with Iber Correo." } else { "Nobody matches." })
                        .color(t.text_weak),
                );
            }
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                for c in contacts {
                    let selected = self.contact_form.id == Some(c.id);
                    let row = Frame::NONE
                        .fill(if selected { t.accent_soft } else { Color32::TRANSPARENT })
                        .inner_margin(Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                                ui.painter().circle_filled(rect.center(), 18.0, t.accent_soft);
                                let initials: String = c.name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
                                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initials, egui::FontId::proportional(13.0), t.accent);
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&c.name).strong().color(t.text));
                                    let mut line: Vec<String> = c.emails.first().map(|a| a.email.clone()).into_iter().collect();
                                    line.extend(c.phones.first().map(|p| p.number.clone()));
                                    line.extend(c.organization.clone());
                                    ui.label(RichText::new(line.join("  ·  ")).small().color(t.text_weak));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if let Some(a) = c.emails.first()
                                        && ui.button("Email").clicked()
                                    {
                                        self.run("message.compose", json!({ "to": a.to_string() }));
                                        self.run("view.mail", json!({}));
                                    }
                                });
                            });
                        });
                    if row.response.interact(Sense::click()).clicked() {
                        self.contact_form = ContactForm::of(&c);
                    }
                    ui.separator();
                }
            });
        });
    }

    /// Where a new contact goes: the address book shown, else the first.
    fn default_book(&self) -> u32 {
        self.contact_book.or_else(|| self.session.store.books().first().map(|b| b.id)).unwrap_or(0)
    }

    fn contact_card(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let books: Vec<(u32, String)> = self.session.store.books().iter().map(|b| (b.id, b.name.clone())).collect();
        let f = &mut self.contact_form;
        ui.label(RichText::new(if f.id.is_some() { "Contact" } else { "New contact" }).strong());
        ui.add_space(6.0);
        let field = |ui: &mut Ui, label: &str, value: &mut String, hint: &str| {
            ui.label(RichText::new(label).small().color(t.text_weak));
            ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(f32::INFINITY));
        };
        field(ui, "Name", &mut f.name, "");
        field(ui, "Email", &mut f.email, "one or more, comma-separated");
        ui.label(RichText::new("Phone").small().color(t.text_weak));
        let mut remove = None;
        for (i, (kind, number)) in f.phones.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("phone-kind", i)).width(70.0).selected_text(phone_kind_label(kind)).show_ui(ui, |ui| {
                    for k in ibercorreo_core::contact::PHONE_KINDS {
                        ui.selectable_value(kind, (*k).to_string(), phone_kind_label(k));
                    }
                });
                ui.add(egui::TextEdit::singleline(number).desired_width((ui.available_width() - 40.0).max(60.0)));
                if ui.small_button("✖").on_hover_text("Remove this phone").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            f.phones.remove(i);
        }
        if f.phones.len() < 50 && ui.small_button("➕ Add phone").clicked() {
            f.phones.push((if f.phones.is_empty() { "mobile" } else { "work" }.into(), String::new()));
        }
        field(ui, "Organization", &mut f.organization, "");
        field(ui, "Job title", &mut f.title, "");
        field(ui, "Birthday", &mut f.birthday, "YYYY-MM-DD");
        ui.label(RichText::new("Notes").small().color(t.text_weak));
        ui.add(egui::TextEdit::multiline(&mut f.note).desired_rows(3).desired_width(f32::INFINITY));
        if books.len() > 1 {
            ui.label(RichText::new("Address book").small().color(t.text_weak));
            let current = books.iter().find(|b| b.0 == f.book).map_or("?", |b| b.1.as_str());
            egui::ComboBox::from_id_salt("contact-book").width(ui.available_width()).selected_text(current).show_ui(ui, |ui| {
                for (id, name) in &books {
                    ui.selectable_value(&mut f.book, *id, name);
                }
            });
        }
        ui.add_space(8.0);
        let (mut save, mut delete) = (false, false);
        ui.horizontal(|ui| {
            save = ui.button(if f.id.is_some() { "Save" } else { "Add" }).clicked();
            if f.id.is_some() {
                delete = ui.button("🗑 Delete").clicked();
            }
        });
        if save {
            let p = self.contact_form.params();
            let id = if self.contact_form.id.is_some() { "contact.update" } else { "contact.add" };
            if let Some(v) = self.run(id, p)
                && let Some(c) = v.get("id").and_then(Value::as_u64).and_then(|id| self.session.store.contact(id).ok())
            {
                self.contact_form = ContactForm::of(c);
            }
        }
        if delete
            && let Some(id) = self.contact_form.id
            && self.run("contact.delete", json!({ "id": id })).is_some()
        {
            self.contact_form = ContactForm::new_in(self.default_book());
        }
    }
}

/// The invitation strip above a message: what the meeting is, and Accept / Tentative / Decline
/// (or Remove from Calendar, for a cancellation). Returns the parameters of the command to run.
fn invite_banner(ui: &mut Ui, t: &Tokens, info: &ibercorreo_engine::commands::InviteInfo, reply: &mut usize) -> Option<Value> {
    use ibercorreo_core::calendar::When;
    let cancelled = info.method == "CANCEL";
    let mut out = None;
    let fill = if cancelled { soft(t.flag, t.reading) } else { t.accent_soft };
    Frame::NONE.fill(fill).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let answered = |s: &str| match s {
            "ACCEPTED" => "accepted",
            "TENTATIVE" => "tentatively accepted",
            "DECLINED" => "declined",
            "DELEGATED" => "delegated",
            _ => "answered",
        };
        let headline = match info.method.as_str() {
            "CANCEL" => "This meeting has been cancelled".to_string(),
            "REPLY" => format!(
                "{} has {} this meeting",
                info.replier.as_deref().unwrap_or("An attendee"),
                info.status.as_deref().map_or("answered", answered)
            ),
            "PUBLISH" => "Event".to_string(),
            "COUNTER" => "An attendee proposes a new time".to_string(),
            _ => match &info.organizer {
                Some(o) => format!("Meeting invitation from {o}"),
                None => "Meeting invitation".to_string(),
            },
        };
        ui.label(RichText::new(format!("📅 {headline}")).small().color(t.text_weak));
        ui.label(RichText::new(if info.summary.is_empty() { "(no title)" } else { &info.summary }).size(16.0).strong().color(t.text));
        let mut when = match info.when {
            Some(When::AllDay { start, end }) => {
                let last = end.add_days(-1).unwrap_or(start);
                if last <= start { format!("{}, all day", long_date(start)) } else { format!("{} – {}", long_date(start), long_date(last)) }
            }
            Some(When::Timed { start, end, .. }) if start.date == end.date => {
                format!("{}, {:02}:{:02}–{:02}:{:02}", long_date(start.date), start.hour(), start.minute(), end.hour(), end.minute())
            }
            Some(When::Timed { start, end, .. }) => format!(
                "{}, {:02}:{:02} – {}, {:02}:{:02}",
                long_date(start.date),
                start.hour(),
                start.minute(),
                long_date(end.date),
                end.hour(),
                end.minute()
            ),
            None => String::new(),
        };
        if info.repeats {
            when.push_str(" · repeats");
        }
        if !info.location.is_empty() {
            when.push_str(&format!(" · {}", info.location));
        }
        ui.label(RichText::new(when).color(t.text));
        if info.in_calendar {
            ui.label(RichText::new("✔ In your calendar").small().color(t.accent));
        } else if info.method == "REQUEST"
            && let Some(status) = info.status.as_deref().filter(|s| *s != "NEEDS-ACTION")
        {
            ui.label(RichText::new(format!("You have {} it", answered(status))).small().color(t.text_weak));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| match info.method.as_str() {
            "REQUEST" | "PUBLISH" => {
                let mode = ["send", "edit", "none"].get(*reply).copied().unwrap_or("send");
                for (label, response) in [("✔ Accept", "accept"), ("？ Tentative", "tentative"), ("✖ Decline", "decline")] {
                    if ui.button(label).clicked() {
                        out = Some(json!({ "response": response, "reply": mode }));
                    }
                }
                if info.method == "REQUEST" && info.organizer.is_some() {
                    let names = ["Send the response now", "Edit the response first", "Do not send a response"];
                    egui::ComboBox::from_id_salt("invite-reply").selected_text(names.get(*reply).copied().unwrap_or(names[0])).show_ui(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            ui.selectable_value(reply, i, *n);
                        }
                    });
                }
            }
            "CANCEL" if ui.add_enabled(info.in_calendar, egui::Button::new("Remove from Calendar")).clicked() => out = Some(json!({})),
            _ => {}
        });
    });
    out
}

fn short_when(dt: ibercorreo_core::DateTime, today: Date) -> String {
    if dt.date == today {
        format!("{:02}:{:02}", dt.hour(), dt.minute())
    } else if dt.date.year() == today.year() {
        format!("{} {}", dt.date.day(), month_abbr(dt.date.month()))
    } else {
        dt.date.to_string()
    }
}

/// `#RRGGBB` as a colour.
fn parse_hex_color(s: &str) -> Option<Color32> {
    let hex = s.strip_prefix('#')?;
    let channel = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    (hex.len() == 6).then_some(())?;
    Some(Color32::from_rgb(channel(0)?, channel(2)?, channel(4)?))
}

/// `c` a quarter of the way from `bg`: an appointment's background, readable under the theme's
/// text colour in light and dark themes.
fn soft(c: Color32, bg: Color32) -> Color32 {
    let mix = |v: u8, b: u8| u8::try_from((u16::from(v) + 3 * u16::from(b)) / 4).unwrap_or(255);
    Color32::from_rgb(mix(c.r(), bg.r()), mix(c.g(), bg.g()), mix(c.b(), bg.b()))
}

fn month_abbr(m: u8) -> &'static str {
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].get(usize::from(m).saturating_sub(1)).copied().unwrap_or("")
}

fn weekday_abbr(d: Date) -> &'static str {
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].get(usize::from(d.weekday().index())).copied().unwrap_or("")
}

/// The ribbon's title for what the calendar shows: `22 October 2026`, `19–25 October 2026`,
/// `26 October – 1 November 2026` or `October 2026`.
fn range_title(s: &Session) -> String {
    let (from, to) = s.calendar_range();
    match s.calendar_mode {
        CalendarMode::Month => month_title(s.day),
        CalendarMode::Day => format!("{} {}", from.day(), month_title(from)),
        _ if from.month() == to.month() => format!("{}–{} {}", from.day(), to.day(), month_title(to)),
        _ if from.year() == to.year() => format!("{} {} – {} {}", from.day(), month_name(from), to.day(), month_title(to)),
        _ => format!("{} {} – {} {}", from.day(), month_title(from), to.day(), month_title(to)),
    }
}

fn month_name(d: Date) -> &'static str {
    MONTHS.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or("")
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

fn month_title(d: Date) -> String {
    format!("{} {}", month_name(d), d.year())
}

fn long_date(d: Date) -> String {
    let days = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    format!("{}, {} {}", days.get(usize::from(d.weekday().index())).copied().unwrap_or(""), d.day(), month_title(d))
}

/// Draw a formatted message ([`Rich`]): our fonts and colours, nothing from the sender's styles.
/// Links show their real address on hover; a link whose text names another site is marked ⚠.
fn rich_body(ui: &mut Ui, t: &Tokens, rich: &Rich) {
    ui.spacing_mut().item_spacing.y = 6.0;
    for block in &rich.blocks {
        let quote = f32::from(block.quote) * 14.0;
        if block.kind == BlockKind::Rule {
            ui.horizontal(|ui| {
                ui.add_space(quote);
                ui.add(egui::Separator::default().horizontal());
            });
            continue;
        }
        let (size, marker, list) = match block.kind {
            BlockKind::Heading(1) => (22.0, None, 0.0),
            BlockKind::Heading(2) => (19.0, None, 0.0),
            BlockKind::Heading(_) => (16.5, None, 0.0),
            BlockKind::Bullet { depth } => (14.0, Some("•".to_string()), f32::from(depth) * 18.0),
            BlockKind::Numbered { depth, number } => (14.0, Some(format!("{number}.")), f32::from(depth) * 18.0),
            _ => (14.0, None, 0.0),
        };
        let pre = block.kind == BlockKind::Pre;
        if matches!(block.kind, BlockKind::Heading(_)) {
            ui.add_space(4.0);
        }
        let rect = ui
            .horizontal_top(|ui| {
                ui.add_space(quote + (list - 14.0).max(0.0));
                if let Some(marker) = &marker {
                    ui.add_sized(vec2(14.0, size), egui::Label::new(RichText::new(marker).size(size).color(t.text_weak)));
                }
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for span in &block.spans {
                            rich_span(ui, t, span, size, pre);
                        }
                    });
                });
            })
            .response
            .rect;
        // A bar per quotation level, as mail readers draw quoted text.
        for level in 0..block.quote {
            let x = rect.left() + f32::from(level) * 14.0 + 3.0;
            ui.painter().vline(x, rect.y_range(), Stroke::new(2.0, t.border));
        }
    }
}

fn rich_span(ui: &mut Ui, t: &Tokens, span: &Span, size: f32, pre: bool) {
    let mut text = if span.image { RichText::new(format!("🖼 {}", span.text)).italics() } else { RichText::new(&span.text) };
    text = text.size(size);
    if pre || span.code {
        text = text.monospace();
    }
    if span.bold {
        text = text.strong();
    }
    if span.italic {
        text = text.italics();
    }
    if span.strike {
        text = text.strikethrough();
    }
    let Some(url) = &span.link else {
        if span.underline {
            text = text.underline();
        }
        // The interface font has one weight, so bold shows as the strong text colour.
        let color = if span.image {
            t.text_weak
        } else if span.bold {
            ui.visuals().strong_text_color()
        } else {
            t.text
        };
        ui.label(text.color(color));
        return;
    };
    if span.deceptive {
        ui.label(RichText::new("⚠ ").size(size).color(t.flag)).on_hover_text("This link does not go where its text says");
    }
    let shown = egui::Link::new(text.color(t.accent).underline());
    let hover = if span.deceptive { format!("Careful: the text names one site, but the link goes to\n{url}") } else { url.clone() };
    if ui.add(shown).on_hover_text(hover).clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// A rule in words: "Every week on Mon, Thu until 2026-12-31".
fn describe_rule(r: &ibercorreo_core::recur::Rule) -> String {
    use ibercorreo_core::recur::Freq;
    let unit = match r.freq {
        Freq::Daily => "day",
        Freq::Weekly => "week",
        Freq::Monthly => "month",
        Freq::Yearly => "year",
    };
    let mut out = if r.interval == 1 { format!("Every {unit}") } else { format!("Every {} {unit}s", r.interval) };
    if !r.by_day.is_empty() {
        let days: Vec<String> = r
            .by_day
            .iter()
            .map(|(n, w)| {
                let name = WEEKDAYS.get(usize::from(w.index())).copied().unwrap_or("?");
                match n {
                    Some(-1) => format!("last {name}"),
                    Some(n) => format!("{} {name}", ordinal(*n)),
                    None => name.to_string(),
                }
            })
            .collect();
        out.push_str(&format!(" on {}", days.join(", ")));
    }
    if !r.by_month_day.is_empty() {
        let days: Vec<String> = r.by_month_day.iter().map(|d| if *d == -1 { "the last day".into() } else { format!("day {d}") }).collect();
        out.push_str(&format!(" on {}", days.join(", ")));
    }
    if let Some(c) = r.count {
        out.push_str(&format!(", {c} times"));
    }
    if let Some((d, _)) = r.until {
        out.push_str(&format!(" until {d}"));
    }
    out
}

/// The Repeat choices in the appointment form: `event.add` values and their labels.
/// The appointment form's reminder choices: `reminder` values for `event.add` ("": its default).
const REMINDERS: [(&str, &str); 8] = [
    ("", "15 minutes"),
    ("none", "None"),
    ("0", "At start time"),
    ("5", "5 minutes"),
    ("30", "30 minutes"),
    ("60", "1 hour"),
    ("120", "2 hours"),
    ("1440", "1 day"),
];

const SNOOZES: [(u64, &str); 5] = [(5, "5 minutes"), (10, "10 minutes"), (15, "15 minutes"), (60, "1 hour"), (1440, "1 day")];

/// `in 10 minutes`, `now`, `25 minutes overdue`, `in 2 hours`, `1 day overdue`.
fn due_in(minutes: i64) -> String {
    let span = |m: i64| {
        let m = m.unsigned_abs();
        if m >= 1440 {
            let d = m / 1440;
            if d == 1 { "1 day".to_string() } else { format!("{d} days") }
        } else if m >= 60 {
            let h = m / 60;
            if h == 1 { "1 hour".to_string() } else { format!("{h} hours") }
        } else if m == 1 {
            "1 minute".to_string()
        } else {
            format!("{m} minutes")
        }
    };
    match minutes {
        0 => "now".into(),
        m if m > 0 => format!("in {}", span(m)),
        m => format!("{} overdue", span(m)),
    }
}

const REPEATS: [(&str, &str); 6] = [
    ("", "Never"),
    ("daily", "Every day"),
    ("weekdays", "Every weekday"),
    ("weekly", "Every week"),
    ("monthly", "Every month"),
    ("yearly", "Every year"),
];

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn ordinal(n: i8) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn human_size(bytes: usize) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gmail() -> Discovered {
        Discovered {
            provider: "Google Mail".into(),
            source: Some("https://autoconfig.thunderbird.net/v1.1/gmail.com".into()),
            imap_host: "imap.gmail.com".into(),
            imap_port: 993,
            imap_starttls: false,
            smtp_host: "smtp.gmail.com".into(),
            smtp_port: 465,
            smtp_starttls: false,
            user: "ana@gmail.com".into(),
            oauth_only: true,
        }
    }

    #[test]
    fn found_settings_pick_oauth_when_this_build_can_sign_in() {
        let mut app = App::new(Session::new(ibercorreo_store::Store::new(), Date::EPOCH));
        app.open_account_settings("");
        app.account_form.email = "ana@gmail.com".into();
        app.apply_autoconfig(Ok(gmail()));
        assert_eq!(app.account_form.oauth, "", "no client ID: a password it stays");
        assert!(app.account_form.note.contains("no Google client ID"), "{}", app.account_form.note);

        app.oauth_ready = vec!["google".into()];
        app.apply_autoconfig(Ok(gmail()));
        assert_eq!(app.account_form.oauth, "google");
        assert!(app.account_form.note.contains("sign in on Google's own page"), "{}", app.account_form.note);

        let mut other = gmail();
        other.imap_host = "imap.example.org".into();
        app.apply_autoconfig(Ok(other));
        assert_eq!(app.account_form.oauth, "", "another provider's servers take a password");
    }

    #[test]
    fn a_finished_browser_sign_in_closes_settings_and_syncs() {
        let mut app = App::new(Session::new(ibercorreo_store::Store::new(), Date::EPOCH));
        let gmail = json!({"email": "ana@gmail.com", "imapHost": "imap.gmail.com", "smtpHost": "smtp.gmail.com", "oauth": "google"});
        app.session.run("account.set", &gmail).unwrap();
        app.open_account_settings("Sign in");
        assert_eq!(app.account_form.oauth, "google", "the dialog shows how the account signs in");
        app.oauth_running = Some(String::new());
        app.apply_oauth(Err("cancelled".into()));
        assert!(app.account_form.open && app.oauth_running.is_none() && !app.session.sync_requested);
        app.apply_oauth(Ok("Google".into()));
        assert!(!app.account_form.open && app.session.sync_requested);
    }

    #[test]
    fn reminder_times_read_naturally() {
        assert_eq!(due_in(0), "now");
        assert_eq!(due_in(1), "in 1 minute");
        assert_eq!(due_in(10), "in 10 minutes");
        assert_eq!(due_in(-25), "25 minutes overdue");
        assert_eq!(due_in(119), "in 1 hour");
        assert_eq!(due_in(-1440), "1 day overdue");
        assert_eq!(due_in(i64::MIN), format!("{} days overdue", i64::MIN.unsigned_abs() / 1440));
    }
}
