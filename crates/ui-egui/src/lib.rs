//! Tinta Correo's front end, L6: an Outlook-style window (navigation rail, folder pane, message
//! list, reading pane; day, week and month calendar; people) over [`ibercorreo_engine::Session`].
//!
//! Thin by rule: every change goes through `Session::run` with a command id, so the interface
//! never edits the store itself. Swappable: nothing below this crate depends on egui.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod chrome;
pub mod i18n;
pub mod icons;
pub mod theme;
pub mod tinta_feedback;
mod tinta_reports_ui;

use crate::i18n::{Language, tf, tl};
use chrome::avatar_color;
use egui::{Align, Color32, CornerRadius, Frame, Key, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use ibercorreo_core::calendar::{MINUTES_PER_DAY, When};
use ibercorreo_core::mail::{OAUTH_PROVIDERS, OAuthProvider};
use ibercorreo_core::rich::{BlockKind, Rich, Span};
use ibercorreo_core::{AccountId, Content, Date, FolderId, FolderRole, MessageId};
use ibercorreo_engine::{CalendarMode, SearchScope, Session, View};
use serde_json::{Value, json};

use icons::Icon;
use theme::Tokens;

/// The product name shown in the window.
pub const NAME: &str = "Tinta Correo";

#[derive(Default)]
struct Draft {
    id: u64,
    to: String,
    cc: String,
    bcc: String,
    /// The Bcc line is shown: asked for, or the draft has blind copies.
    show_bcc: bool,
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

/// Rules (Home ▸ Rules…): the list, and the rule being made or edited.
#[derive(Default)]
struct RuleForm {
    open: bool,
    /// The rule in the form: `Some(None)` a new one, `Some(Some(id))` an existing one.
    editing: Option<Option<u32>>,
    name: String,
    query: String,
    mark_read: bool,
    flag: bool,
    move_on: bool,
    move_to: Option<u32>,
    delete: bool,
    stop: bool,
    enabled: bool,
    /// What went wrong with the last save or run.
    note: String,
}

impl RuleForm {
    /// The form for a new rule with `query` as its condition.
    fn start(&mut self, query: &str) {
        *self = RuleForm { open: true, editing: Some(None), query: query.into(), enabled: true, ..RuleForm::default() };
    }

    /// The form for rule `r`, as `rule.list` gives it.
    fn edit(&mut self, r: &Value) {
        let flag = |k: &str| r.get(k).and_then(Value::as_bool).unwrap_or(false);
        let move_to = r.get("moveTo").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
        *self = RuleForm {
            open: true,
            editing: Some(r.get("id").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok())),
            name: r.get("name").and_then(Value::as_str).unwrap_or("").into(),
            query: r.get("query").and_then(Value::as_str).unwrap_or("").into(),
            mark_read: flag("markRead"),
            flag: flag("flag"),
            move_on: move_to.is_some(),
            move_to,
            delete: flag("delete"),
            stop: flag("stop"),
            enabled: flag("enabled"),
            note: String::new(),
        };
    }

    /// The form as `rule.add` / `rule.set` parameters.
    fn params(&self) -> Value {
        let move_to = match (self.move_on, self.move_to) {
            (true, Some(f)) => json!(f),
            _ => json!(false),
        };
        let id = match self.editing {
            Some(Some(id)) => json!(id),
            _ => Value::Null,
        };
        json!({
            "id": id,
            "name": self.name,
            "query": self.query,
            "markRead": self.mark_read,
            "flag": self.flag,
            "moveTo": move_to,
            "delete": self.delete,
            "stop": self.stop,
            "enabled": self.enabled,
        })
    }
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

/// A folder's name as shown: folders with a role (Inbox, Sent…) in the interface language,
/// others as the server names them.
/// The search box's tooltip: what a query can say.
const SEARCH_HELP: &str = "Words match from their start, accents ignored. \"exact phrase\", -word to leave out, OR for either. from: to: cc: subject: body: attachment:, has:attachment, is:unread, is:flagged, after:2026-10-01, before:, on:";

/// A rule's condition and actions in one line, for the Rules list.
fn rule_summary(r: &Value) -> String {
    let query = r["query"].as_str().unwrap_or("");
    let mut actions: Vec<String> = Vec::new();
    if r["markRead"].as_bool() == Some(true) {
        actions.push(tl("mark as read").into());
    }
    if r["flag"].as_bool() == Some(true) {
        actions.push(tl("flag").into());
    }
    if let Some(name) = r["moveToName"].as_str() {
        actions.push(tf("move to {folder}", &[("folder", tl(name))]));
    }
    if r["delete"].as_bool() == Some(true) {
        actions.push(tl("delete").into());
    }
    if r["stop"].as_bool() == Some(true) {
        actions.push(tl("stop").into());
    }
    let when = if query.is_empty() { tl("every message").to_string() } else { query.to_string() };
    format!("{when} — {}", actions.join(", "))
}

/// The search scope's name in the list, as Outlook's.
/// The periods offered for the automatic Send/Receive, in minutes (0: none).
const SYNC_INTERVALS: [u32; 8] = [0, 1, 2, 5, 10, 15, 30, 60];

fn sync_interval_label(minutes: u32) -> String {
    match minutes {
        0 => tl("Only by hand").to_string(),
        1 => tl("Every minute").to_string(),
        m => tf("Every {n} min", &[("n", &m.to_string())]),
    }
}

fn scope_label(scope: SearchScope) -> &'static str {
    match scope {
        SearchScope::All => "All mailboxes",
        SearchScope::Mailbox => "Current mailbox",
        SearchScope::Folder => "Current folder",
    }
}

fn folder_label(folder: &ibercorreo_core::Folder) -> String {
    if folder.role == FolderRole::Other { folder.name.clone() } else { tl(&folder.name).to_string() }
}

fn phone_kind_label(kind: &str) -> &'static str {
    tl(match kind {
        "mobile" => "Mobile",
        "work" => "Work",
        "home" => "Home",
        "fax" => "Fax",
        _ => "Other",
    })
}

/// The Account settings dialog. The password is held only while the dialog is open and then
/// handed to the app shell ([`App::take_password`]), which keeps it in the system's credential store.
#[derive(Default)]
struct AccountForm {
    open: bool,
    /// The account being changed, or `None` for a new one.
    id: Option<AccountId>,
    /// Ask before removing the account (the second click removes it).
    confirm_remove: bool,
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
    /// Added to new messages, replies and forwards.
    signature: String,
    /// The OAuth provider signed in with (its id), or empty for a password.
    oauth: String,
    /// Mail over Microsoft Graph instead of IMAP and SMTP (with the Microsoft sign-in only).
    graph_mail: bool,
    /// The Exchange server (its EWS address or name) mail goes over instead of IMAP and SMTP
    /// (with a password only); empty for none.
    ews: String,
    /// Why the dialog opened by itself (a missing or refused password), or what Find settings found.
    note: String,
    /// The GNOME Online Accounts account the mail signs in through (its id), or empty.
    goa: String,
    /// The GNOME Online Accounts account the calendars and contacts sign in through, or empty.
    dav_goa: String,
}

/// What to do when `p`'s sign-in page warns that it has not reviewed the app.
fn unverified_note(p: &OAuthProvider) -> String {
    tf(
        "If {provider} says it hasn't verified the app, choose Advanced, then Go to Tinta Correo: {provider}'s review comes later, and the sign-in is as safe.",
        &[("provider", p.name)],
    )
}

impl AccountForm {
    /// Fill the form from an account in GNOME Settings: its mail servers, or (an account with no
    /// mail, such as Nextcloud) its calendars and contacts. `dav_free`: no other account syncs
    /// calendars, so this one may.
    fn take_gnome(&mut self, g: &GnomeAccount, dav_free: bool) {
        let provider = if g.provider.is_empty() { tl("GNOME Settings").to_string() } else { g.provider.clone() };
        let dav = g.dav.as_ref().filter(|_| dav_free);
        if let Some(m) = &g.mail {
            if !m.name.is_empty() {
                self.name = m.name.clone();
            }
            self.email = m.email.clone();
            self.user = m.user.clone();
            self.imap_host = m.imap_host.clone();
            self.imap_port = m.imap_port.to_string();
            self.imap_starttls = m.imap_starttls;
            self.smtp_host = m.smtp_host.clone();
            self.smtp_port = m.smtp_port.to_string();
            self.smtp_starttls = m.smtp_starttls;
            self.oauth.clear();
            self.ews.clear();
            self.graph_mail = false;
            self.password.clear();
            self.goa = g.id.clone();
            if let Some(dav) = dav {
                self.caldav = dav.clone();
                self.dav_goa.clear();
            }
            self.note = tf(
                "{provider} from GNOME Settings: its servers are filled in, and GNOME Settings signs in, so no password is kept here. Save.",
                &[("provider", &provider)],
            );
        } else if let Some(dav) = dav {
            self.caldav = dav.clone();
            self.dav_goa = g.id.clone();
            self.note = tf(
                "{provider}'s calendars and contacts sync with this account, and GNOME Settings signs in to them. Save.",
                &[("provider", &provider)],
            );
        } else {
            self.note = tl("Calendars and contacts already sync with another account; one account at a time can sync them.").to_string();
        }
        if g.attention {
            self.note.push(' ');
            self.note.push_str(tl("GNOME Settings asks you to sign in to this account again first, under Online Accounts."));
        }
    }

    /// Fill in a provider's own servers: choosing Google or Microsoft needs nothing looked up.
    fn fill_provider(&mut self, p: &OAuthProvider) {
        self.imap_host = p.imap_host.into();
        self.imap_port = "993".into();
        self.imap_starttls = false;
        self.smtp_host = p.smtp_host.into();
        self.smtp_port = if p.smtp_starttls { "587" } else { "465" }.into();
        self.smtp_starttls = p.smtp_starttls;
        self.user.clear();
    }

    /// After the person chose a way to sign in or typed an address: fill in the servers of the
    /// provider it names. `ready` are the providers this build can sign in with.
    fn follow_provider(&mut self, oauth_was: &str, email_was: &str, ready: &[String]) {
        if self.oauth != oauth_was {
            if let Some(p) = OAuthProvider::get(&self.oauth) {
                self.fill_provider(p);
                self.note = tf(
                    "{provider}'s servers are filled in. Save: you then sign in on {provider}'s own page in your browser.",
                    &[("provider", p.name)],
                );
                if p.unverified {
                    self.note.push(' ');
                    self.note.push_str(&unverified_note(p));
                }
            }
            return;
        }
        if self.email == email_was {
            return;
        }
        let Some(p) = OAuthProvider::for_address(&self.email) else { return };
        // Servers typed by hand stay; another provider's (or none) give way.
        let host = self.imap_host.trim();
        let theirs = OAuthProvider::for_host(host);
        if !host.is_empty() && (theirs.is_none() || theirs.is_some_and(|t| t.id == p.id)) {
            return;
        }
        self.fill_provider(p);
        if p.app_password_url.is_some() {
            self.oauth.clear();
            self.note = tf(
                "A {provider} address: its servers are filled in. Enter an app password (the link below the password makes one) and save.",
                &[("provider", p.name)],
            );
        } else if ready.iter().any(|r| r == p.id) {
            self.oauth = p.id.to_string();
            self.note = tf(
                "A {provider} address: its servers are filled in. Save: you then sign in on {provider}'s own page in your browser.",
                &[("provider", p.name)],
            );
        } else {
            self.note = tf("A {provider} address: its servers are filled in. Enter an app password and save.", &[("provider", p.name)]);
        }
    }
}

/// What Autodiscover asks with: the address and the sign-in (the password is never kept).
#[derive(Clone)]
pub struct ExchangeQuery {
    pub email: String,
    /// The sign-in name: the address when the form leaves it empty.
    pub login: String,
    pub password: String,
}

/// An account from GNOME Settings (GNOME Online Accounts), as the shell lists it for Account
/// settings: what Tinta Correo can take from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GnomeAccount {
    /// GNOME's id for it (`account_…`).
    pub id: String,
    /// The provider's name: Google, Nextcloud, IMAP and SMTP…
    pub provider: String,
    /// The account as GNOME Settings shows it (`ana@gmail.com`).
    pub identity: String,
    /// Its mail servers, when it has mail.
    pub mail: Option<GnomeMail>,
    /// Its CalDAV and CardDAV address, when it has calendars or contacts.
    pub dav: Option<String>,
    /// GNOME Settings asks the person to sign in to it again.
    pub attention: bool,
}

/// The mail part of a [`GnomeAccount`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GnomeMail {
    pub email: String,
    /// The person's name; may be empty.
    pub name: String,
    /// The sign-in name; empty means the address.
    pub user: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_starttls: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_starttls: bool,
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
    /// Unread messages per folder, counted once a frame for the folder pane and status bar.
    unread: std::collections::HashMap<FolderId, usize>,
    /// The calendar the last appointment went into, the default for the next.
    last_calendar: Option<u32>,
    /// Set by the shell while a Send/Receive runs in the background.
    pub syncing: bool,
    /// Set by the shell as a Send/Receive goes: what it is doing, and the share done (0 to 1).
    pub sync_progress: Option<(String, f32)>,
    /// An address whose server settings the person asked to find; the shell takes it.
    pub autoconfig_request: Option<String>,
    /// Set by the shell while it looks for server settings.
    pub autoconfig_running: bool,
    /// An address whose Exchange server the person asked to find with Autodiscover, with the
    /// sign-in to ask with; the shell takes it.
    pub exchange_request: Option<ExchangeQuery>,
    /// Set by the shell while Autodiscover runs.
    pub exchange_running: bool,
    /// The OAuth providers this build can sign in with (ids), set by the shell.
    pub oauth_ready: Vec<String>,
    /// Account settings asks the shell to sign in in the browser to this account (its provider).
    pub oauth_request: Option<AccountId>,
    /// Account settings asks the shell to stop waiting for the browser.
    pub oauth_cancel: bool,
    /// Set by the shell while the browser sign-in runs: the address it opened.
    pub oauth_running: Option<String>,
    /// The accounts in GNOME Settings, set by the shell where GNOME Online Accounts answers;
    /// `None` elsewhere (Account settings then shows nothing of it).
    pub gnome_accounts: Option<Vec<GnomeAccount>>,
    /// Tinta Office: where the shell keeps error reports, set only by builds that send them
    /// (`tinta_reports_ui`).
    pub error_reports_dir: Option<std::path::PathBuf>,
    /// Tinta Office: the Send feedback dialog ([`tinta_feedback`]), opened from the status bar's
    /// smiling face; the desktop app sends.
    pub tinta_feedback: tinta_feedback::Feedback,
    account_form: AccountForm,
    rule_form: RuleForm,
    password: Option<(AccountId, String)>,
    tokens: Tokens,
    /// The toolbar is too narrow for labels this frame: its buttons show only their icons.
    compact_tools: bool,
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
    /// The open message's content, read once per message (from disk, for most mail) and again
    /// after anything runs; the error, if it could not be read.
    content: Option<(ibercorreo_core::MessageId, std::result::Result<std::sync::Arc<Content>, String>)>,
    /// How an answer to an invitation goes: 0 sent now, 1 edited first, 2 not at all.
    invite_reply: usize,
    /// Folder pane headings folded shut: an account's id, or [`FOLD_FAVORITES`].
    folded: std::collections::HashSet<u32>,
    /// The theme egui's own widgets were last styled for ([`theme::apply`]); `None`: not yet.
    styled: Option<Option<String>>,
    /// Folders folded shut in the folder pane, by account and path ([`TreeRow::key`]).
    folded_paths: std::collections::HashSet<(u32, String)>,
    /// The size the status bar's slider is being dragged to, saved when it is let go.
    zoom_drag: Option<f32>,
    /// The interface size's default, smallest and largest, in percent (from `view.zoom`).
    zoom_limits: Option<(u16, u16, u16)>,
    /// This frame the folder pane (or My Day) has no room and gives way to the main pane; not
    /// kept. `peek_…`: shown anyway, because the person asked to see it.
    folders_squeezed: bool,
    my_day_squeezed: bool,
    peek_folders: bool,
    peek_my_day: bool,
    /// How much wider the window would have to be for the folder pane (My Day) to fit.
    room_wanted: (f32, f32),
    /// The main pane's card width last frame, inside its border.
    main_width: f32,
}

/// The narrowest the main pane of each view gets before side panes give way: the reading pane,
/// the calendar grid, the contact table.
const MAIL_MAIN_MIN: f32 = 440.0;
const CALENDAR_MAIN_MIN: f32 = 480.0;
const PEOPLE_MAIN_MIN: f32 = 420.0;

/// A side pane's width last frame, and how narrow and wide it may be.
#[derive(Debug, Clone, Copy)]
struct Side {
    wanted: f32,
    min: f32,
    max: f32,
}

/// Which side panes are shown this frame and the widest each may be.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fit {
    left: bool,
    left_max: f32,
    middle_max: f32,
    right: bool,
    right_max: f32,
    /// The width the view would need to show the left (right) pane too, at its narrowest
    /// beside the others, with the main pane its room.
    room_for_left: f32,
    room_for_right: f32,
}

/// Lays out a view `full` wide whose main pane needs `main`, as Outlook gives way: first the
/// right pane (My Day) goes, then the middle (the message list) and left panes shrink to their
/// narrowest, then the left pane (folders) folds away. A pane in `peek` (left, right) is shown
/// whatever the room. No pane may grow past what leaves the main pane its room.
fn fit(full: f32, main: f32, left: Option<Side>, middle: Option<Side>, right: Option<Side>, peek: (bool, bool)) -> Fit {
    let width = |s: Option<Side>| s.map_or(0.0, |s| s.wanted.clamp(s.min, s.max.max(s.min)));
    let narrowest = |s: Option<Side>| s.map_or(0.0, |s| s.min);
    let show_right = right.is_some() && (peek.1 || width(left) + narrowest(middle) + width(right) + main <= full);
    let rw = if show_right { width(right) } else { 0.0 };
    let show_left = left.is_some() && (peek.0 || narrowest(left) + narrowest(middle) + rw + main <= full);
    let lw = if show_left { width(left) } else { 0.0 };
    let cap = |s: Option<Side>, room: f32| s.map_or(0.0, |s| room.clamp(s.min, s.max.max(s.min)));
    let left_max = cap(left, full - narrowest(middle) - rw - main);
    let lw = lw.min(left_max);
    Fit {
        left: show_left,
        left_max,
        middle_max: cap(middle, full - lw - rw - main),
        right: show_right,
        right_max: cap(right, full - lw - narrowest(middle) - main),
        room_for_left: narrowest(left) + narrowest(middle) + rw + main,
        room_for_right: lw + narrowest(middle) + width(right) + main,
    }
}

/// How far in each level of subfolders sits in the folder pane.
const FOLDER_INDENT: f32 = 16.0;

/// How a row of the folder pane is drawn.
#[derive(Debug, Clone, Default)]
struct FolderLine {
    /// The name shown; `None`: the folder's own label.
    label: Option<String>,
    /// How many levels of subfolders in.
    depth: usize,
    /// `Some(open)` when it has subfolders to fold.
    fold: Option<bool>,
    /// Unread mail in its folded-away subfolders, counted on it.
    folded_unread: usize,
    /// Show the account's address under the name (Favorites, with several accounts).
    with_account: bool,
}

/// A row of an account's folder tree.
#[derive(Debug, Clone, PartialEq)]
struct TreeRow {
    /// `None`: a path subfolders share that is no folder here (a server's `[Gmail]`, say).
    folder: Option<FolderId>,
    /// The path in lower case, segments joined by "/": what folding remembers.
    key: String,
    /// The last segment, as the server spells it; a standard folder's own label.
    label: String,
    depth: usize,
    children: bool,
}

/// An account's folders as a tree, in the store's order, subfolders right under their parent.
/// A folder's name is its server path with "/" between levels ("Clients/Acme"); Inbox's
/// subfolders ("INBOX/Work") go under the Inbox. Standard folders are never nested.
fn folder_tree(folders: &[&ibercorreo_core::Folder]) -> Vec<TreeRow> {
    struct Node {
        row: TreeRow,
        kids: Vec<usize>,
    }
    let mut nodes: Vec<Node> = Vec::new();
    let mut roots: Vec<usize> = Vec::new();
    let mut by_key: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for f in folders {
        let segments: Vec<&str> = if f.role == FolderRole::Other { f.name.split('/').filter(|s| !s.is_empty()).collect() } else { Vec::new() };
        let segments = if segments.is_empty() { vec![f.name.as_str()] } else { segments };
        let mut parent: Option<usize> = None;
        let mut key = String::new();
        for (i, segment) in segments.iter().enumerate() {
            if i > 0 {
                key.push('/');
            }
            key.push_str(&segment.to_lowercase());
            let last = i + 1 == segments.len();
            let label = if last && f.role != FolderRole::Other { folder_label(f) } else { (*segment).to_string() };
            let found = by_key.get(&key).copied();
            let reuse = match found.and_then(|n| nodes.get(n).map(|node| (n, node.row.folder.is_none()))) {
                Some((n, _)) if !last => Some(n),
                Some((n, true)) => Some(n),
                _ => None,
            };
            let n = if let Some(n) = reuse {
                if last && let Some(node) = nodes.get_mut(n) {
                    node.row.folder = Some(f.id);
                    node.row.label = label;
                }
                n
            } else {
                let n = nodes.len();
                nodes.push(Node {
                    row: TreeRow { folder: last.then_some(f.id), key: key.clone(), label, depth: i, children: false },
                    kids: Vec::new(),
                });
                by_key.entry(key.clone()).or_insert(n);
                match parent.and_then(|p| nodes.get_mut(p)) {
                    Some(p) => p.kids.push(n),
                    None => roots.push(n),
                }
                n
            };
            parent = Some(n);
        }
    }
    let mut out = Vec::with_capacity(nodes.len());
    let mut stack: Vec<(usize, usize)> = roots.iter().rev().map(|&n| (n, 0)).collect();
    while let Some((n, depth)) = stack.pop() {
        let Some(node) = nodes.get(n) else { continue };
        out.push(TreeRow { depth, children: !node.kids.is_empty(), ..node.row.clone() });
        stack.extend(node.kids.iter().rev().map(|&k| (k, depth + 1)));
    }
    out
}

/// The Favorites heading's key among [`App::folded`] (account ids are far below it).
const FOLD_FAVORITES: u32 = u32::MAX;
/// The height of a row in the message list.
const ROW_HEIGHT: f32 = 84.0;

impl App {
    pub fn new(session: Session) -> App {
        // With several accounts, each but the one open starts folded, so every account's
        // heading is in sight, however many folders the others have.
        let folded = if session.store.accounts.len() > 1 {
            let open = session.current_account();
            session.store.accounts.iter().map(|a| a.id).filter(|id| *id != open).map(|id| id.0).collect()
        } else {
            std::collections::HashSet::new()
        };
        App {
            session,
            unread: std::collections::HashMap::new(),
            syncing: false,
            sync_progress: None,
            autoconfig_request: None,
            autoconfig_running: false,
            exchange_request: None,
            exchange_running: false,
            oauth_ready: Vec::new(),
            oauth_request: None,
            oauth_cancel: false,
            oauth_running: None,
            gnome_accounts: None,
            error_reports_dir: None,
            tinta_feedback: tinta_feedback::Feedback::default(),
            account_form: AccountForm::default(),
            rule_form: RuleForm::default(),
            password: None,
            tokens: Tokens::light(),
            compact_tools: false,
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
            styled: None,
            folded_paths: std::collections::HashSet::new(),
            zoom_drag: None,
            zoom_limits: None,
            folders_squeezed: false,
            my_day_squeezed: false,
            peek_folders: false,
            peek_my_day: false,
            room_wanted: (0.0, 0.0),
            main_width: 0.0,
            last_calendar: None,
            invite: None,
            content: None,
            invite_reply: 0,
            folded,
        }
    }

    /// A password typed in Account settings and the account it is for, for the shell to store.
    /// Taken once.
    pub fn take_password(&mut self) -> Option<(AccountId, String)> {
        self.password.take()
    }

    /// Open Account settings for `account` (`None`: a new one), with a note saying why (`""`
    /// when the person asked for it).
    pub fn open_account_settings(&mut self, account: Option<AccountId>, note: &str) {
        let v = match account {
            Some(id) => self.session.run("account.get", &json!({ "id": id.0 })).unwrap_or(Value::Null),
            None => Value::Null,
        };
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let port = |k: &str| v.get(k).and_then(Value::as_u64).map(|n| n.to_string()).unwrap_or_default();
        let id = v.get("id").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).map(AccountId);
        self.account_form = AccountForm {
            open: true,
            id,
            confirm_remove: false,
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
            signature: text("signature"),
            oauth: text("oauth"),
            graph_mail: v.get("graphMail").and_then(Value::as_bool).unwrap_or(false),
            ews: text("ews"),
            note: note.to_string(),
            goa: text("goa"),
            dav_goa: text("davGoa"),
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
                // A provider with app passwords offers them first.
                let ready = provider.filter(|p| p.app_password_url.is_none() && self.oauth_ready.iter().any(|r| r == p.id));
                f.oauth = ready.map(|p| p.id.to_string()).unwrap_or_default();
                f.note = match (ready, provider, d.source) {
                    (Some(p), _, _) => tf(
                        "Settings for {found} found. Check them and save: you then sign in on {provider}'s own page in your browser.",
                        &[("found", &d.provider), ("provider", p.name)],
                    ),
                    // Even where only OAuth is published: the provider takes app passwords too.
                    (None, Some(p), _) if p.app_password_url.is_some() => tf(
                        "Settings for {found} found. Enter an app password (the link below the password makes one) and save.",
                        &[("found", &d.provider)],
                    ),
                    (None, Some(p), _) if d.oauth_only => tf(
                        "Settings for {found} found, but {provider} only takes sign-in on its own page (OAuth), and this build has no {provider} client ID for it.",
                        &[("found", &d.provider), ("provider", p.name)],
                    ),
                    (None, _, Some(_)) => tf("Settings for {found} found. Check them, enter the password and save.", &[("found", &d.provider)]),
                    (None, _, None) => tl("No published settings: these servers answered at the usual names. Check them and save.").to_string(),
                };
            }
            Err(e) => f.note = tf("No settings found: {error}. Enter the servers by hand.", &[("error", &e.to_string())]),
        }
    }

    /// Fill Account settings with the EWS address Autodiscover found (or say what failed).
    pub fn apply_exchange(&mut self, found: Result<String, String>) {
        let f = &mut self.account_form;
        match found {
            Ok(ews) => {
                f.ews = ews;
                f.oauth.clear();
                f.note = tl("Exchange server found. Check the address and save.").to_string();
            }
            Err(e) => f.note = tf("No Exchange server found: {error}. Enter its address by hand.", &[("error", &e)]),
        }
    }

    /// Whether Account settings is open (the shell does not open it over itself).
    pub fn account_settings_open(&self) -> bool {
        self.account_form.open
    }

    /// What the shell's browser sign-in came to: the provider's name, or why it failed.
    pub fn apply_oauth(&mut self, result: Result<String, String>) {
        self.oauth_running = None;
        match result {
            Ok(name) => {
                self.account_form.open = false;
                self.account_form.note.clear();
                self.session.status = tf("Signed in with {name}", &[("name", &name)]);
                self.run("account.sync", json!({}));
            }
            Err(e) => self.account_form.note = tf("The sign-in did not finish: {error}.", &[("error", &e)]),
        }
    }

    fn account_window(&mut self, ui: &mut Ui) {
        if !self.account_form.open {
            return;
        }
        let mut open = true;
        let mut save = false;
        let mut remove = false;
        let title = tl(if self.account_form.id.is_some() { "Account settings" } else { "Add account" });
        // Another account already syncs calendars and contacts: this one can't take GNOME's.
        let editing = self.account_form.id;
        let dav_free = !self.session.store.accounts.iter().any(|a| !a.caldav.is_empty() && Some(a.id) != editing);
        egui::Window::new(title)
            .id(egui::Id::new("account-settings"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                let f = &mut self.account_form;
                let (oauth_was, email_was) = (f.oauth.clone(), f.email.clone());
                if !f.note.is_empty() {
                    ui.label(RichText::new(&f.note).color(self.tokens.unread));
                    ui.add_space(4.0);
                }
                egui::Grid::new("account-form").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    for (label, field, hint) in [("Your name", &mut f.name, "shown to recipients"), ("Email address", &mut f.email, "me@example.org")]
                    {
                        ui.label(tl(label));
                        ui.add(egui::TextEdit::singleline(field).hint_text(tl(hint)).desired_width(280.0));
                        ui.end_row();
                    }
                    ui.label("");
                    let label = tl(if self.autoconfig_running { "🔍 Looking…" } else { "🔍 Find settings" });
                    let can = !self.autoconfig_running && f.email.contains('@');
                    if ui
                        .add_enabled(can, egui::Button::new(label))
                        .on_hover_text(tl("Look up the servers for this address (the provider's published settings, then the usual names)"))
                        .clicked()
                    {
                        self.autoconfig_request = Some(f.email.trim().to_string());
                    }
                    ui.end_row();
                    let gnome = self.gnome_accounts.as_deref().unwrap_or_default();
                    if !gnome.is_empty() {
                        ui.label(tl("From GNOME Settings")).on_hover_text(tl(
                            "An account added under Online Accounts in GNOME Settings: its servers are filled in, and GNOME Settings signs in, so no password is kept here",
                        ));
                        let mut chosen = None;
                        egui::ComboBox::from_id_salt("gnome-account").width(280.0).selected_text(tl("Choose an account…")).show_ui(ui, |ui| {
                            for g in gnome {
                                let mut label = format!("{} — {}", g.provider, g.identity);
                                if g.attention {
                                    label.push_str(" ⚠");
                                }
                                if ui.selectable_label(g.id == f.goa || g.id == f.dav_goa, label).clicked() {
                                    chosen = Some(g);
                                }
                            }
                        });
                        if let Some(g) = chosen {
                            f.take_gnome(g, dav_free);
                        }
                        ui.end_row();
                    }
                    let gnome_name = |id: &str| {
                        gnome.iter().find(|g| g.id == id).map(|g| format!("{} — {}", g.provider, g.identity)).unwrap_or_else(|| id.to_string())
                    };
                    if !f.goa.is_empty() {
                        ui.label(tl("Sign in with"));
                        ui.horizontal(|ui| {
                            ui.label(tf("GNOME Settings ({account})", &[("account", &gnome_name(&f.goa))]));
                            if ui.small_button(tl("Unlink")).on_hover_text(tl("Sign in with a password or in the browser instead")).clicked() {
                                f.goa.clear();
                            }
                        });
                        ui.end_row();
                    }
                    if f.goa.is_empty() {
                    ui.label(tl("Sign in with"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut f.oauth, String::new(), tl("Password"));
                        for p in OAUTH_PROVIDERS {
                            let ready = self.oauth_ready.iter().any(|r| r == p.id);
                            let hover = if ready {
                                let mut h = tf(
                                    "Sign in on {provider}'s own page in your browser (OAuth 2.0): no password is kept here",
                                    &[("provider", p.name)],
                                );
                                if p.unverified {
                                    h.push_str(".\n");
                                    h.push_str(&unverified_note(p));
                                }
                                h
                            } else {
                                tf("This build has no {provider} client ID for OAuth sign-in", &[("provider", p.name)])
                            };
                            ui.add_enabled_ui(ready, |ui| ui.radio_value(&mut f.oauth, p.id.to_string(), p.name))
                                .inner
                                .on_hover_text(&hover)
                                .on_disabled_hover_text(hover);
                        }
                    });
                    ui.end_row();
                    }
                    if f.oauth == "microsoft" {
                        ui.label("");
                        ui.checkbox(&mut f.graph_mail, tl("Mail over Microsoft Graph")).on_hover_text(tl(
                            "For a Microsoft 365 work account whose organisation turned IMAP off: mail, calendar and contacts all go over Microsoft Graph, and the servers below are not used",
                        ));
                        ui.end_row();
                    }
                    if f.oauth.is_empty() && f.goa.is_empty() {
                        ui.label(tl("Exchange server (EWS)")).on_hover_text(tl(
                            "For an Exchange Server your organisation runs itself: its name or EWS address. Mail goes over Exchange Web Services with your password, and the servers below are not used",
                        ));
                        ui.add(egui::TextEdit::singleline(&mut f.ews).hint_text(tl("mail.example.org (optional)")).desired_width(280.0));
                        ui.end_row();
                        ui.label("");
                        let label = tl(if self.exchange_running { "🔍 Looking…" } else { "🔍 Find the Exchange server" });
                        let can = !self.exchange_running && f.email.contains('@') && !f.password.is_empty();
                        if ui
                            .add_enabled(can, egui::Button::new(label))
                            .on_hover_text(tl("Ask the organisation's Autodiscover for it, signed in with the name and password below"))
                            .on_disabled_hover_text(tl("Enter the email address and the password first"))
                            .clicked()
                        {
                            let login = if f.user.trim().is_empty() { f.email.trim() } else { f.user.trim() };
                            self.exchange_request = Some(ExchangeQuery { email: f.email.trim().to_string(), login: login.to_string(), password: f.password.clone() });
                        }
                        ui.end_row();
                    }
                    let other_server = (f.oauth == "microsoft" && f.graph_mail) || (f.oauth.is_empty() && f.goa.is_empty() && !f.ews.trim().is_empty());
                    if f.oauth.is_empty() && f.goa.is_empty() {
                        ui.label(tl("Password"));
                        ui.add(
                            egui::TextEdit::singleline(&mut f.password)
                                .password(true)
                                .hint_text(tl("kept in the system's credential store"))
                                .desired_width(280.0),
                        );
                        ui.end_row();
                        let provider = OAuthProvider::for_host(&f.imap_host).or_else(|| OAuthProvider::for_address(&f.email));
                        if let Some((p, url)) = provider.and_then(|p| p.app_password_url.map(|u| (p, u))) {
                            ui.label("");
                            ui.hyperlink_to(tf("Make a {provider} app password…", &[("provider", p.name)]), url).on_hover_text(tf(
                                "{provider} account → Security → App passwords (needs 2-Step Verification). Paste the 16 letters here.",
                                &[("provider", p.name)],
                            ));
                            ui.end_row();
                        }
                    }
                    ui.label(tl("Sign-in name"));
                    ui.add(egui::TextEdit::singleline(&mut f.user).hint_text(tl("if not the email address")).desired_width(280.0));
                    ui.end_row();
                    let servers = [
                        ("Incoming (IMAP)", &mut f.imap_host, &mut f.imap_port, &mut f.imap_starttls, "993", "143"),
                        ("Outgoing (SMTP)", &mut f.smtp_host, &mut f.smtp_port, &mut f.smtp_starttls, "465", "587"),
                    ];
                    for (title, host, port, starttls, tls_port, starttls_port) in servers.into_iter().filter(|_| !other_server) {
                        ui.label(RichText::new(tl(title)).strong());
                        ui.end_row();
                        ui.label(tl("Server"));
                        ui.add(egui::TextEdit::singleline(host).hint_text(tl("mail.example.org")).desired_width(280.0));
                        ui.end_row();
                        ui.label(tl("Port"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(port).hint_text(if *starttls { starttls_port } else { tls_port }).desired_width(60.0));
                            ui.radio_value(starttls, false, tl("TLS"));
                            ui.radio_value(starttls, true, tl("STARTTLS"));
                        });
                        ui.end_row();
                    }
                    ui.label(RichText::new(tl("Calendars and contacts")).strong())
                        .on_hover_text(tl("A CalDAV and CardDAV server, such as Nextcloud, Radicale or Fastmail"));
                    ui.end_row();
                    ui.label(tl("Address"));
                    ui.add(egui::TextEdit::singleline(&mut f.caldav).hint_text(tl("https://dav.example.org (optional)")).desired_width(280.0));
                    ui.end_row();
                    if !f.dav_goa.is_empty() {
                        ui.label("");
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tf("Signed in through GNOME Settings ({account})", &[("account", &gnome_name(&f.dav_goa))])).small());
                            if ui.small_button(tl("Unlink")).on_hover_text(tl("Sign in to the calendars as the mail does")).clicked() {
                                f.dav_goa.clear();
                            }
                        });
                        ui.end_row();
                    }
                    ui.label(RichText::new(tl("Signature")).strong())
                        .on_hover_text(tl("Added under new messages, replies and forwards from this account"));
                    ui.end_row();
                    ui.label("");
                    ui.add(
                        egui::TextEdit::multiline(&mut f.signature).hint_text(tl("Your name\nCompany · phone")).desired_width(280.0).desired_rows(3),
                    );
                    ui.end_row();
                });
                f.follow_provider(&oauth_was, &email_was, &self.oauth_ready);
                ui.add_space(6.0);
                match &self.oauth_running {
                    Some(url) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(tl("Waiting for you to sign in in your browser…"));
                            if ui.button(tl("Cancel")).clicked() {
                                self.oauth_cancel = true;
                            }
                        });
                        if !url.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(tl("No browser opened?")).small());
                                if ui.small_button(tl("Copy the sign-in address")).clicked() {
                                    ui.ctx().copy_text(url.clone());
                                }
                            });
                        }
                    }
                    None => {
                        let label = tl(if f.oauth.is_empty() { "Save and Send/Receive" } else { "Save and sign in" });
                        ui.horizontal(|ui| {
                            save = ui.add(egui::Button::new(RichText::new(label).strong())).clicked();
                            if f.id.is_some() && !f.confirm_remove && ui.button(tl("Remove account…")).clicked() {
                                f.confirm_remove = true;
                            }
                        });
                        if f.confirm_remove {
                            ui.add_space(4.0);
                            ui.label(tf(
                                "Remove {address} and the mail kept here? Nothing is deleted on the server.",
                                &[("address", f.email.trim())],
                            ));
                            ui.horizontal(|ui| {
                                remove = ui.button(tl("Remove")).clicked();
                                if ui.button(tl("Keep")).clicked() {
                                    f.confirm_remove = false;
                                }
                            });
                        }
                    }
                }
            });
        if remove
            && let Some(id) = self.account_form.id
            && self.run("account.remove", json!({ "id": id.0 })).is_some()
        {
            open = false;
        }
        if save && self.save_account() {
            open = false;
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

    /// Save Account settings (a new account is added); `true` when the dialog may close (a
    /// browser sign-in keeps it open).
    fn save_account(&mut self) -> bool {
        let f = &self.account_form;
        let mut p = json!({
            "name": f.name, "email": f.email, "user": f.user,
            "imapHost": f.imap_host, "imapSecurity": if f.imap_starttls { "starttls" } else { "tls" },
            "smtpHost": f.smtp_host, "smtpSecurity": if f.smtp_starttls { "starttls" } else { "tls" },
            "caldav": f.caldav, "oauth": f.oauth, "signature": f.signature,
            "graphMail": f.oauth == "microsoft" && f.graph_mail,
            "ews": if f.oauth.is_empty() && f.goa.is_empty() { f.ews.trim() } else { "" },
            "goa": f.goa, "davGoa": f.dav_goa,
        });
        if let Some(o) = p.as_object_mut() {
            for (k, v) in [("imapPort", &f.imap_port), ("smtpPort", &f.smtp_port)] {
                if !v.trim().is_empty() {
                    o.insert(k.into(), Value::String(v.trim().to_string()));
                }
            }
        }
        let saved = match (f.id, p.as_object_mut()) {
            (Some(id), Some(o)) => {
                o.insert("id".into(), json!(id.0));
                self.run("account.set", p)
            }
            _ => self.run("account.add", p),
        };
        let saved = saved.and_then(|v| v.get("id").and_then(Value::as_u64)).and_then(|n| u32::try_from(n).ok()).map(AccountId);
        if let Some(id) = saved {
            // A new account is this one from now on: saving again changes it.
            self.account_form.id = Some(id);
            if !self.account_form.oauth.is_empty() {
                // Stays open while the browser sign-in runs; Send/Receive follows it.
                self.account_form.password.clear();
                self.account_form.note.clear();
                self.oauth_request = Some(id);
            } else {
                if !self.account_form.password.is_empty() {
                    self.password = Some((id, std::mem::take(&mut self.account_form.password)));
                }
                self.run("account.sync", json!({}));
                return true;
            }
        } else if self.account_form.id.is_none()
            && let Some(id) = self.existing_account(&self.account_form.email)
        {
            // Already set up: show it rather than leave the person wondering where it went.
            self.show_account(id);
            self.session.status = tf("{email} is already set up: here it is", &[("email", self.account_form.email.trim())]);
            return true;
        }
        false
    }

    /// The account set up for `email`, compared without case.
    fn existing_account(&self, email: &str) -> Option<AccountId> {
        let email = email.trim();
        let accounts = &self.session.store.accounts;
        accounts.iter().find(|a| !email.is_empty() && a.address.email.trim().eq_ignore_ascii_case(email)).map(|a| a.id)
    }

    /// Unfold `account` in the folder pane and open its Inbox.
    fn show_account(&mut self, account: AccountId) {
        self.folded.remove(&account.0);
        if let Ok(inbox) = self.session.store.folder_by_role(account, FolderRole::Inbox) {
            self.run("folder.select", json!({ "folder": inbox.0 }));
        }
    }

    /// Run a command; a failure is shown in the status bar (the session records it).
    fn run(&mut self, id: &str, params: Value) -> Option<Value> {
        let out = self.session.run(id, &params).ok();
        // Anything may have changed what is due, or whether a meeting is in the calendar.
        self.due_checked = None;
        self.invite = None;
        self.content = None;
        out
    }

    /// A command from outside the window (a file or link a later start handed over): runs like a
    /// click, and what the window keeps from the last frame is refreshed.
    pub fn run_external(&mut self, id: &str, params: &Value) -> ibercorreo_core::Result<Value> {
        let out = self.session.run(id, params);
        self.due_checked = None;
        self.invite = None;
        self.content = None;
        out
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        i18n::set_current(i18n::resolve(self.session.store.language.as_deref()));
        let preference = match self.session.store.appearance.as_deref() {
            Some("light") => egui::ThemePreference::Light,
            Some("dark") => egui::ThemePreference::Dark,
            _ => egui::ThemePreference::System,
        };
        if ui.ctx().options(|o| o.theme_preference) != preference {
            ui.ctx().set_theme(preference);
        }
        if self.styled.as_ref() != Some(&self.session.store.theme) {
            theme::apply(ui.ctx(), self.session.store.theme.as_deref());
            self.styled = Some(self.session.store.theme.clone());
        }
        self.tokens = Tokens::themed(self.session.store.theme.as_deref(), ui.visuals().dark_mode);
        self.zoom(ui);
        self.shortcuts(ui);
        self.sync_draft();
        self.unread = self.session.store.unread_counts();
        let t = self.tokens;
        chrome::paint_window(ui.ctx(), &t);
        egui::Panel::top("title").exact_size(chrome::BAR_H).resizable(false).frame(Frame::NONE).show(ui, |ui| self.title_bar(ui));
        egui::Panel::bottom("status").frame(Frame::NONE.inner_margin(Margin::symmetric(14, 4))).show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("rail")
            .exact_size(chrome::RAIL_W)
            .resizable(false)
            .frame(Frame::NONE.inner_margin(Margin { left: 0, right: 0, top: 4, bottom: 10 }))
            .show(ui, |ui| self.rail(ui));
        match self.session.view {
            View::Mail => self.mail(ui),
            View::Calendar => self.calendar(ui),
            View::Contacts => self.people(ui),
        }
        self.account_window(ui);
        self.rules_window(ui);
        self.reminder_window(ui);
        tinta_reports_ui::first_start(self, ui.ctx());
        self.tinta_feedback.show(ui.ctx(), i18n::current().code());
    }

    /// Rules: the list in the order they run (on/off, up/down, edit, delete), New rule and Run
    /// rules now; and the form for one rule.
    fn rules_window(&mut self, ui: &mut Ui) {
        if !self.rule_form.open {
            return;
        }
        let t = self.tokens;
        let rules = self.session.run("rule.list", &Value::Null).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let mut open = true;
        let mut action: Option<(&'static str, Value)> = None;
        let mut edit: Option<Value> = None;
        let mut save = false;
        let mut cancel = false;
        let account = self.session.current_account();
        let folders: Vec<(u32, String)> = self.session.store.folders_of(account).iter().map(|f| (f.id.0, folder_label(f))).collect();
        let archive = self.session.store.folder_by_role(account, FolderRole::Archive).ok().map(|f| f.0);
        egui::Window::new(tl("Rules"))
            .id(egui::Id::new("rules"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.set_width(600.0);
                let f = &mut self.rule_form;
                if !f.note.is_empty() {
                    ui.label(RichText::new(&f.note).color(t.unread));
                }
                if f.editing.is_none() {
                    ui.label(RichText::new(tl("Rules run on new mail in the Inbox after Send/Receive, in this order.")).color(t.text_weak));
                    ui.add_space(4.0);
                    if rules.is_empty() {
                        ui.label(RichText::new(tl("No rules yet.")).color(t.text_weak));
                    }
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for (i, r) in rules.iter().enumerate() {
                            let id = r["id"].clone();
                            ui.horizontal(|ui| {
                                let mut on = r["enabled"].as_bool().unwrap_or(false);
                                if ui.checkbox(&mut on, "").on_hover_text(tl("On")).changed() {
                                    action = Some(("rule.set", json!({ "id": id, "enabled": on })));
                                }
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(r["name"].as_str().unwrap_or("")).strong().color(t.text));
                                    ui.label(RichText::new(rule_summary(r)).small().color(t.text_weak));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.small_button(tl("Delete")).clicked() {
                                        action = Some(("rule.delete", json!({ "id": id })));
                                    }
                                    if ui.small_button(tl("Edit…")).clicked() {
                                        edit = Some(r.clone());
                                    }
                                    if ui
                                        .add_enabled(i + 1 < rules.len(), egui::Button::new(tl("Later")).small())
                                        .on_hover_text(tl("Run this rule later"))
                                        .clicked()
                                    {
                                        action = Some(("rule.move", json!({ "id": id, "up": false })));
                                    }
                                    if ui
                                        .add_enabled(i > 0, egui::Button::new(tl("Earlier")).small())
                                        .on_hover_text(tl("Run this rule earlier"))
                                        .clicked()
                                    {
                                        action = Some(("rule.move", json!({ "id": id, "up": true })));
                                    }
                                });
                            });
                            ui.separator();
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button(tl("+ New rule")).clicked() {
                            f.start("");
                        }
                        if ui
                            .add_enabled(!rules.is_empty(), egui::Button::new(tl("Run rules now")))
                            .on_hover_text(tl("Run the rules that are on over the open folder"))
                            .clicked()
                        {
                            action = Some(("rule.run", json!({})));
                        }
                    });
                    return;
                }
                egui::Grid::new("rule-form").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label(tl("Name"));
                    ui.add(egui::TextEdit::singleline(&mut f.name).hint_text(tl("default: the condition")).desired_width(340.0));
                    ui.end_row();
                    ui.label(tl("Condition"));
                    ui.vertical(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.query).hint_text("from:ana@example.org subject:factura").desired_width(340.0))
                            .on_hover_text(tl(SEARCH_HELP));
                        ui.horizontal(|ui| {
                            for (label, key) in [("From", "from:"), ("Subject", "subject:"), ("Sent to", "to:"), ("Has attachment", "has:attachment")]
                            {
                                if ui.small_button(tl(label)).clicked() {
                                    if !f.query.is_empty() && !f.query.ends_with(' ') {
                                        f.query.push(' ');
                                    }
                                    f.query.push_str(key);
                                }
                            }
                        });
                        ui.label(RichText::new(tl("Search syntax; empty: every message.")).small().color(t.text_weak));
                    });
                    ui.end_row();
                    ui.label(tl("Do this"));
                    ui.vertical(|ui| {
                        ui.checkbox(&mut f.mark_read, tl("Mark as read"));
                        ui.checkbox(&mut f.flag, tl("Flag"));
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut f.move_on, tl("Move to")).changed() && f.move_on {
                                f.delete = false;
                                if f.move_to.is_none() {
                                    f.move_to = archive;
                                }
                            }
                            let current =
                                f.move_to.and_then(|id| folders.iter().find(|(fid, _)| *fid == id)).map(|(_, n)| n.clone()).unwrap_or_default();
                            ui.add_enabled_ui(f.move_on, |ui| {
                                egui::ComboBox::from_id_salt("rule-folder").selected_text(current).show_ui(ui, |ui| {
                                    for (id, name) in &folders {
                                        ui.selectable_value(&mut f.move_to, Some(*id), name);
                                    }
                                });
                            });
                        });
                        if ui.checkbox(&mut f.delete, tl("Delete (move to Deleted Items)")).changed() && f.delete {
                            f.move_on = false;
                        }
                        ui.checkbox(&mut f.stop, tl("Stop processing more rules"));
                    });
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut f.enabled, tl("On"));
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(tl("Save")).strong()).clicked() {
                        save = true;
                    }
                    if ui.button(tl("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(r) = edit {
            self.rule_form.edit(&r);
        }
        if cancel {
            self.rule_form.editing = None;
            self.rule_form.note.clear();
        }
        if save {
            let id = if matches!(self.rule_form.editing, Some(Some(_))) { "rule.set" } else { "rule.add" };
            match self.session.run(id, &self.rule_form.params()) {
                Ok(_) => {
                    self.rule_form.editing = None;
                    self.rule_form.note.clear();
                }
                Err(e) => self.rule_form.note = e.to_string(),
            }
        }
        if let Some((id, params)) = action {
            match self.session.run(id, &params) {
                Ok(_) => self.rule_form.note.clear(),
                Err(e) => self.rule_form.note = e.to_string(),
            }
        }
        if !open {
            self.rule_form = RuleForm::default();
        }
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
        let title = if self.due.len() == 1 {
            tl("🔔 1 Reminder").trim_start_matches('🔔').trim().to_string()
        } else {
            tf("{n} Reminders", &[("n", &self.due.len().to_string())])
        };
        // A compact header of our own, as a mail app's reminder: a bell and the count, on the left.
        let frame = Frame::window(&ui.ctx().global_style()).inner_margin(Margin { left: 14, right: 14, top: 10, bottom: 12 });
        egui::Window::new(&title)
            .id(egui::Id::new("reminders"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .frame(frame)
            // Above the status bar, never over it.
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -40.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(380.0);
                ui.horizontal(|ui| {
                    let (bell, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    icons::paint(ui.painter(), bell, Icon::Bell, t.accent);
                    ui.label(RichText::new(&title).size(15.0).strong().color(t.text));
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                    for r in &self.due {
                        let id = r["id"].clone();
                        let start = r["start"].clone();
                        Frame::NONE
                            .fill(theme::mix(t.card, t.window, 0.55))
                            .corner_radius(CornerRadius::same(8))
                            .stroke(Stroke::new(1.0, t.border))
                            .inner_margin(Margin { left: 12, right: 10, top: 8, bottom: 8 })
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 4.0;
                                ui.label(RichText::new(r["summary"].as_str().unwrap_or("")).strong().color(t.text));
                                let when = r["start"].as_str().unwrap_or("").replace('T', " ");
                                let place = r["location"].as_str().filter(|l| !l.is_empty()).map(|l| format!(" · 📍 {l}")).unwrap_or_default();
                                // When and where, how soon, and the buttons, on one line.
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                        ui.label(RichText::new(format!("{when}{place}")).small().color(t.text_weak));
                                        ui.label(RichText::new(due_in(r["minutesUntil"].as_i64().unwrap_or(0))).small().color(t.accent));
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.button(tl("Dismiss")).clicked() {
                                            action = Some(("reminder.dismiss", json!({ "id": id, "start": start })));
                                        }
                                        ui.menu_button(tl("Snooze…"), |ui| {
                                            for (minutes, label) in SNOOZES {
                                                if ui.button(tl(label)).clicked() {
                                                    action = Some(("reminder.snooze", json!({ "id": id, "start": start, "minutes": minutes })));
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                });
                            });
                        ui.add_space(6.0);
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(tl("Dismiss All")).clicked() {
                        action = Some(("reminder.dismiss", json!({})));
                    }
                    if ui.button(tl("Snooze all 5 minutes")).clicked() {
                        action = Some(("reminder.snooze", json!({ "minutes": 5 })));
                    }
                });
            });
        if let Some((id, params)) = action {
            self.run(id, params);
        }
    }

    /// The interface's size: Ctrl+= (or Ctrl++) and Ctrl+- step it, Ctrl+0 resets it, also while
    /// typing; the size kept in the mailbox is the one shown.
    fn zoom(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if ctx.options(|o| o.zoom_with_keyboard) {
            ctx.options_mut(|o| o.zoom_with_keyboard = false);
        }
        let step = ctx.input_mut(|i| {
            let cmd = egui::Modifiers::COMMAND;
            if i.consume_key(cmd, Key::Equals) || i.consume_key(cmd, Key::Plus) || i.consume_key(cmd | egui::Modifiers::SHIFT, Key::Equals) {
                Some(json!({ "step": 1 }))
            } else if i.consume_key(cmd, Key::Minus) {
                Some(json!({ "step": -1 }))
            } else if i.consume_key(cmd, Key::Num0) {
                Some(json!({ "reset": true }))
            } else {
                None
            }
        });
        if let Some(p) = step {
            self.run("view.zoom", p);
        }
        let factor = f32::from(self.session.store.zoom()) / 100.0;
        if (ctx.zoom_factor() - factor).abs() > f32::EPSILON {
            ctx.set_zoom_factor(factor);
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
        let store = &self.session.store;
        let open =
            self.session.message.and_then(|id| store.message(id).ok()).filter(|m| store.folder(m.folder).is_ok_and(|f| f.role == FolderRole::Drafts));
        match (open, &self.draft) {
            (Some(m), Some(d)) if d.id == m.id.0 => {}
            (Some(m), _) => {
                let list = |v: &[ibercorreo_core::Address]| v.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                self.draft = Some(Draft {
                    id: m.id.0,
                    to: list(&m.to),
                    cc: list(&m.cc),
                    bcc: list(&m.bcc),
                    show_bcc: !m.bcc.is_empty(),
                    subject: m.subject.clone(),
                    // A draft whose text cannot be read starts empty rather than not opening.
                    body: store.content(m.id).map(|c| c.body.clone()).unwrap_or_default(),
                });
            }
            (None, _) => self.draft = None,
        }
    }

    /// A thin upright line between groups of toolbar buttons.
    fn tool_gap(&self, ui: &mut Ui) {
        let (r, _) = ui.allocate_exact_size(vec2(13.0, 24.0), Sense::hover());
        ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, self.tokens.border));
    }

    /// Mail's toolbar: delete, archive, move, flag, read or unread; reply, reply all, forward;
    /// Send/Receive and how often; rules.
    fn mail_tools(&mut self, ui: &mut Ui) {
        let has_message = self.session.message.is_some();
        let seen = self.session.message.and_then(|id| self.session.store.message(id).ok()).is_some_and(|m| m.seen);
        let read_label = if seen { "Mark as unread" } else { "Mark as read" };
        let sync_label = if self.syncing { "Sending and receiving…" } else { "Send/Receive" };
        // When the labels do not fit, the message's own buttons give theirs up first, then
        // replying, Send/Receive and Rules too.
        let own = ["Delete", "Archive", "Move", "Flag", read_label];
        let rest = ["Reply", "Reply All", "Forward", sync_label, "Rules…"];
        let head = if self.folder_pane_collapsed() { 96.0 } else { 0.0 };
        let fixed = head + 2.0 * 15.0 + 115.0 + 4.0 * (own.len() + rest.len()) as f32;
        let (own_w, rest_w) = (self.tools_width(ui, &own), self.tools_width(ui, &rest));
        let icons_w = chrome::TOOL_ICON_W * own.len() as f32;
        let avail = ui.available_width();
        let (own_compact, rest_compact) = if fixed + own_w + rest_w <= avail { (false, false) } else { (true, fixed + icons_w + rest_w > avail) };
        self.compact_tools = own_compact;
        ui.set_min_height(chrome::BAR_H);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if self.hidden_pane_head(ui, Icon::Pencil, "New Email") {
                self.run("message.compose", json!({}));
            }
            if self.tool(ui, Icon::Trash, "Delete", has_message).clicked() {
                self.run("message.delete", json!({}));
            }
            if self.tool(ui, Icon::Archive, "Archive", has_message).clicked() {
                self.run("message.archive", json!({}));
            }
            let mv = self.tool(ui, Icon::Move, "Move", has_message);
            if has_message {
                let folders: Vec<(FolderId, String)> =
                    self.session.store.folders_of(self.session.current_account()).iter().map(|f| (f.id, folder_label(f))).collect();
                let current = self.session.folder;
                egui::Popup::menu(&mv).show(|ui| {
                    for (id, name) in folders {
                        if id != current && ui.button(name).clicked() {
                            self.run("message.move", json!({ "folder": id.0 }));
                        }
                    }
                });
            }
            if self.tool(ui, Icon::Flag, "Flag", has_message).clicked() {
                self.run("message.flag", json!({}));
            }
            if self.tool(ui, Icon::Unread, read_label, has_message).clicked() {
                self.run("message.markRead", json!({ "read": !seen }));
            }
            self.compact_tools = rest_compact;
            self.tool_gap(ui);
            if self.tool(ui, Icon::Reply, "Reply", has_message).clicked() {
                self.run("message.reply", json!({}));
            }
            if self.tool(ui, Icon::ReplyAll, "Reply All", has_message).clicked() {
                self.run("message.replyAll", json!({}));
            }
            if self.tool(ui, Icon::Forward, "Forward", has_message).clicked() {
                self.run("message.forward", json!({}));
            }
            self.tool_gap(ui);
            if self.tool(ui, Icon::Sync, sync_label, !self.syncing).on_hover_text(tl("Send the Outbox and get new mail (F9)")).clicked() {
                self.sync_or_set_up("Set up your mail server to send and receive.");
            }
            self.sync_interval(ui);
            if self.tool(ui, Icon::Rules, "Rules…", true).on_hover_text(tl("Sort, mark or delete mail as it arrives")).clicked() {
                self.rule_form = RuleForm { open: true, ..RuleForm::default() };
            }
            self.compact_tools = false;
        });
    }

    /// The calendar's toolbar: Today, back and forward, what is shown, the view, the time zone,
    /// Send/Receive.
    fn calendar_tools(&mut self, ui: &mut Ui) {
        self.compact_tools = false;
        ui.set_min_height(chrome::BAR_H);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if self.hidden_pane_head(ui, Icon::Calendar, "New Appointment") {
                self.new_appointment();
            }
            if ui.add(egui::Button::new(tl("Today")).corner_radius(6.0).fill(self.tokens.card)).clicked() {
                self.run("calendar.goto", json!({ "date": "today" }));
            }
            if self.icon_button(ui, Icon::ChevronLeft, "Back", false).clicked() {
                self.run("calendar.prev", json!({}));
            }
            if self.icon_button(ui, Icon::ChevronRight, "Forward", false).clicked() {
                self.run("calendar.next", json!({}));
            }
            ui.label(RichText::new(range_title(&self.session)).size(17.0).strong().color(self.tokens.text));
            self.tool_gap(ui);
            let modes = [
                (CalendarMode::Day, "Day", "calendar.viewDay", "Ctrl+Alt+1"),
                (CalendarMode::WorkWeek, "Work Week", "calendar.viewWorkWeek", "Ctrl+Alt+2"),
                (CalendarMode::Week, "Week", "calendar.viewWeek", "Ctrl+Alt+3"),
                (CalendarMode::Month, "Month", "calendar.viewMonth", "Ctrl+Alt+4"),
            ];
            let current = modes.iter().find(|m| m.0 == self.session.calendar_mode).map_or("Week", |m| m.1);
            let mut pick: Option<&str> = None;
            ui.menu_button(format!("{} ⏷", tl(current)), |ui| {
                for (mode, label, id, keys) in modes {
                    if ui.selectable_label(self.session.calendar_mode == mode, tl(label)).on_hover_text(keys).clicked() {
                        pick = Some(id);
                        ui.close();
                    }
                }
            });
            if let Some(id) = pick {
                self.run(id, json!({}));
                self.scroll_to_morning = true;
            }
            self.zone_picker(ui);
            let label = if self.syncing { "Syncing…" } else { "Send/Receive" };
            if self.tool(ui, Icon::Sync, label, !self.syncing).on_hover_text(tl("Sync mail, calendars and contacts (F9)")).clicked() {
                self.sync_or_set_up("Set up your account to sync calendars.");
            }
        });
    }

    /// People's toolbar: write to the chosen contact, edit, delete; Send/Receive.
    fn people_tools(&mut self, ui: &mut Ui) {
        let chosen = self.contact_form.id;
        let email = chosen.and_then(|id| self.session.store.contact(id).ok()).and_then(|c| c.emails.first().map(|a| a.to_string()));
        self.compact_tools = false;
        ui.set_min_height(chrome::BAR_H);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if self.hidden_pane_head(ui, Icon::PersonAdd, "New Contact") {
                self.contact_form = ContactForm::new_in(self.default_book());
            }
            if self.tool(ui, Icon::Mail, "Email", email.is_some()).clicked()
                && let Some(to) = &email
            {
                self.run("message.compose", json!({ "to": to }));
                self.run("view.mail", json!({}));
            }
            if self.tool(ui, Icon::Trash, "Delete", chosen.is_some()).clicked()
                && let Some(id) = chosen
            {
                self.run("contact.delete", json!({ "id": id }));
                self.contact_form = ContactForm::default();
            }
            self.tool_gap(ui);
            let label = if self.syncing { "Syncing…" } else { "Send/Receive" };
            if self.tool(ui, Icon::Sync, label, !self.syncing).on_hover_text(tl("Sync mail, calendars and contacts (F9)")).clicked() {
                self.sync_or_set_up("Set up your account to sync contacts.");
            }
        });
    }

    /// Opens the form for a new appointment on the day shown, at nine.
    fn new_appointment(&mut self) {
        self.event_form = EventForm { open: true, start: format!("{}T09:00", self.session.day), ..EventForm::default() };
    }

    /// How often Send/Receive runs on its own, as Outlook's "Schedule an automatic send/receive
    /// every N minutes".
    fn sync_interval(&mut self, ui: &mut Ui) {
        let current = self.session.store.sync_minutes();
        let mut chosen = current;
        egui::ComboBox::from_id_salt("sync-interval")
            .selected_text(sync_interval_label(current))
            .show_ui(ui, |ui| {
                let mut options = SYNC_INTERVALS.to_vec();
                if !options.contains(&current) {
                    options.push(current);
                    options.sort_unstable();
                }
                for m in options {
                    ui.selectable_value(&mut chosen, m, sync_interval_label(m));
                }
            })
            .response
            .on_hover_text(tl("How often Tinta Correo sends and receives on its own"));
        if chosen != current {
            self.run("account.syncInterval", json!({ "minutes": chosen }));
        }
    }

    /// Send/Receive when an account has a mail server; else Account settings, saying `setup`.
    fn sync_or_set_up(&mut self, setup: &str) {
        if self.session.store.accounts.iter().any(|a| a.login().is_some()) {
            self.run("account.sync", json!({}));
        } else {
            let first = self.session.store.accounts.first().map(|a| a.id);
            self.open_account_settings(first, tl(setup));
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
            if ui.selectable_label(follows, tf("Same as this computer ({zone})", &[("zone", &system.replace('_', " "))])).clicked() {
                chosen = Some("system".into());
            }
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut self.zone_filter).hint_text(tl("Search: Lisbon, Kyiv, New York…")).desired_width(240.0));
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
            let inbox = self.session.store.folder_by_role(self.session.current_account(), FolderRole::Inbox).ok();
            let unread = inbox.and_then(|f| self.unread.get(&f).copied()).unwrap_or(0);
            ui.label(RichText::new(tf("Unread in Inbox: {n}", &[("n", &unread.to_string())])).small().color(self.tokens.text_weak));
            ui.separator();
            if self.syncing
                && let Some((what, done)) = &self.sync_progress
            {
                let bar = egui::ProgressBar::new(done.clamp(0.0, 1.0)).desired_width(160.0).desired_height(10.0).fill(self.tokens.accent);
                ui.add(bar);
                ui.label(RichText::new(what).small().color(self.tokens.text_weak));
                ui.separator();
            }
            ui.label(RichText::new(tl(&self.session.status)).small().color(self.tokens.text_weak));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.zoom_slider(ui));
        });
    }

    /// The interface's size at the right of the status bar, as in an office suite: −, a slider in
    /// steps of 10, +, and the percent (a click on it goes back to the default). A drag is saved
    /// once, when let go.
    fn zoom_slider(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        // The default and the range, as view.zoom says them (asked once; asking changes nothing).
        let (default, lo, hi) = *self.zoom_limits.get_or_insert_with(|| {
            let v = self.session.run("view.zoom", &Value::Null).unwrap_or_default();
            let n = |k: &str, d: u16| v.get(k).and_then(Value::as_u64).and_then(|n| u16::try_from(n).ok()).unwrap_or(d);
            (n("default", 110), n("min", 80), n("max", 200))
        });
        let (lo, hi) = (f32::from(lo), f32::from(hi.max(lo.saturating_add(10))));
        let kept = f32::from(self.session.store.zoom());
        let mut value = self.zoom_drag.unwrap_or(kept);
        let mut set: Option<Value> = None;
        ui.spacing_mut().item_spacing.x = 2.0;
        // Right to left: the percent, +, the track, −.
        let (pct, resp) = ui.allocate_exact_size(vec2(44.0, 20.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(pct, 4.0, t.accent_soft);
        }
        ui.painter().text(
            pct.center(),
            egui::Align2::CENTER_CENTER,
            format!("{} %", value.round()),
            egui::FontId::proportional(theme::CAPTION),
            t.text_weak,
        );
        if resp.on_hover_text(tf("Back to {percent} % (Ctrl+0)", &[("percent", &default.to_string())])).clicked() {
            set = Some(json!({ "reset": true }));
        }
        if self.tiny_button(ui, true, "Larger (Ctrl+=)") {
            set = Some(json!({ "step": 1 }));
        }
        let (track, resp) = ui.allocate_exact_size(vec2(120.0, 20.0), Sense::click_and_drag());
        let span = (track.width() - 14.0).max(1.0);
        let x_of = |v: f32| track.left() + 7.0 + span * ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
        if let Some(p) = resp.interact_pointer_pos()
            && (resp.dragged() || resp.clicked())
        {
            let share = ((p.x - track.left() - 7.0) / span).clamp(0.0, 1.0);
            value = ((lo + share * (hi - lo)) / 10.0).round() * 10.0;
        }
        let y = track.center().y;
        let p = ui.painter();
        let line = |a: f32, b: f32| egui::Rect::from_min_max(egui::pos2(a, y - 2.0), egui::pos2(b, y + 2.0));
        p.rect_filled(line(track.left() + 7.0, track.right() - 7.0), 2.0, t.border);
        p.rect_filled(line(track.left() + 7.0, x_of(value)), 2.0, t.accent);
        let tick = x_of(f32::from(default));
        p.line_segment([egui::pos2(tick, y - 6.0), egui::pos2(tick, y - 3.5)], Stroke::new(1.0, t.text_weak));
        let big = resp.hovered() || resp.dragged();
        p.circle(egui::pos2(x_of(value), y), if big { 7.5 } else { 6.5 }, t.card, Stroke::new(1.5, t.accent));
        let resp = resp.on_hover_text(tl("Size"));
        if resp.dragged() {
            self.zoom_drag = Some(value);
        } else if resp.drag_stopped() || resp.clicked() {
            set = Some(json!({ "percent": value }));
        }
        if self.tiny_button(ui, false, "Smaller (Ctrl+-)") {
            set = Some(json!({ "step": -1 }));
        }
        if let Some(p) = set {
            self.zoom_drag = None;
            self.run("view.zoom", p);
        }
    }

    /// A frameless 20 × 20 point + (`plus`) or − button, drawn in thin strokes; true when clicked.
    fn tiny_button(&self, ui: &mut Ui, plus: bool, tip: &str) -> bool {
        let t = self.tokens;
        let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
        let color = if resp.hovered() { t.accent } else { t.text_weak };
        if resp.hovered() {
            ui.painter().rect_filled(r, 4.0, t.accent_soft);
        }
        let (c, stroke) = (r.center(), Stroke::new(1.4, color));
        ui.painter().line_segment([c - vec2(4.5, 0.0), c + vec2(4.5, 0.0)], stroke);
        if plus {
            ui.painter().line_segment([c - vec2(0.0, 4.5), c + vec2(0.0, 4.5)], stroke);
        }
        resp.on_hover_text(tl(tip)).clicked()
    }

    /// The title bar's interface language menu: Automatic (the computer's) or one by hand.
    fn language_menu(&mut self, ui: &mut Ui) {
        let chosen = self.session.store.language.clone();
        let current = i18n::current();
        let mut pick: Option<&str> = None;
        let tip = format!("{}: {}", tl("Interface language"), current.name());
        self.icon_menu(ui, Icon::Globe, &tip, |ui| {
            if ui.selectable_label(chosen.is_none(), tl("Automatic (system language)")).clicked() {
                pick = Some(i18n::AUTO);
            }
            for l in i18n::Language::ALL {
                if ui.selectable_label(chosen.as_deref() == Some(l.code()), l.name()).clicked() {
                    pick = Some(l.code());
                }
            }
        });
        if let Some(code) = pick {
            self.run("app.language", json!({ "language": code }));
        }
    }

    // ---- mail ----

    /// Which side panes fit beside a view's main pane this frame, from their widths last frame;
    /// panes the person hid stay hidden, and a pane they asked to see with no room is shown anyway.
    fn fit_panes(
        &mut self,
        ui: &Ui,
        main: f32,
        left: (&str, f32, f32, f32),
        middle: Option<(&str, f32, f32, f32)>,
        right: Option<(&str, f32, f32, f32)>,
        force_right: bool,
    ) -> Fit {
        let ctx = ui.ctx();
        let side = |(id, default, min, max): (&str, f32, f32, f32)| Side {
            wanted: egui::PanelState::load(ctx, egui::Id::new(id)).map_or(default, |s| s.size().x),
            min,
            max,
        };
        let store = &self.session.store;
        let fit = fit(
            ui.available_width(),
            main,
            (!store.hide_folder_pane).then(|| side(left)),
            middle.map(side),
            right.filter(|_| force_right || !store.hide_my_day).map(side),
            (self.peek_folders, self.peek_my_day || force_right),
        );
        let missing = |need: f32| (need - ui.available_width()).max(0.0);
        self.room_wanted = (missing(fit.room_for_left), missing(fit.room_for_right));
        self.folders_squeezed = !store.hide_folder_pane && !fit.left;
        self.my_day_squeezed = !force_right && right.is_some() && !store.hide_my_day && !fit.right;
        fit
    }

    fn mail(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let fit = self.fit_panes(
            ui,
            MAIL_MAIN_MIN,
            ("folders", 260.0, 190.0, 400.0),
            Some(("list", 360.0, 280.0, 680.0)),
            Some(("my-day", 280.0, 240.0, 440.0)),
            false,
        );
        if fit.left {
            egui::Panel::left("folders")
                .default_size(260.0)
                .size_range(190.0..=fit.left_max)
                .frame(Frame::NONE.inner_margin(Margin { left: 4, right: 10, top: 0, bottom: 10 }))
                .show(ui, |ui| {
                    if self.pane_head(ui, Icon::Pencil, "New Email") {
                        self.run("message.compose", json!({}));
                    }
                    self.folders(ui);
                });
        }
        egui::Panel::top("mail-tools").frame(Frame::NONE).show(ui, |ui| self.mail_tools(ui));
        if fit.right {
            egui::Panel::right("my-day")
                .default_size(280.0)
                .size_range(240.0..=fit.right_max)
                .frame(chrome::card(&t, 14))
                .show(ui, |ui| self.my_day(ui));
        }
        egui::Panel::left("list")
            .default_size(360.0)
            .size_range(280.0..=fit.middle_max)
            .frame(chrome::card(&t, 0))
            .show(ui, |ui| self.message_list(ui));
        egui::CentralPanel::no_frame().frame(chrome::card(&t, 24)).show(ui, |ui| {
            self.main_width = ui.available_width() + 48.0;
            if self.draft.is_some() {
                self.compose(ui);
            } else {
                self.reading(ui);
            }
        });
    }

    /// The folder pane, as Outlook lays it out: Favorites first, then each account's folders
    /// under its address. Each heading folds; a right click puts a folder in Favorites or takes
    /// it out.
    fn folders(&mut self, ui: &mut Ui) {
        let store = &self.session.store;
        let groups: Vec<(Option<AccountId>, String)> = if store.accounts.is_empty() {
            vec![(None, tl("Local").into())]
        } else {
            store.accounts.iter().map(|a| (Some(a.id), a.address.email.clone())).collect()
        };
        let several = store.accounts.len() > 1;
        let favorites: Vec<FolderId> = store.favorites().to_vec();
        let mut settings: Option<Option<AccountId>> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if self.folder_heading(ui, FOLD_FAVORITES, tl("Favorites"), None).0 {
                let line = FolderLine { with_account: several, ..FolderLine::default() };
                for id in &favorites {
                    self.folder_row(ui, Some(*id), &line);
                }
                if favorites.is_empty() {
                    ui.add_space(4.0);
                    // Wrapped, so a long translation never widens the pane.
                    Frame::NONE.inner_margin(Margin { left: 36, right: 6, top: 0, bottom: 0 }).show(ui, |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(tl("Right-click a folder: Add to Favorites")).small().color(self.tokens.text_weak)).wrap(),
                        );
                    });
                    ui.add_space(4.0);
                }
            }
            for (account, email) in groups {
                ui.add_space(10.0);
                let key = account.map_or(u32::MAX - 1, |a| a.0);
                let (open, gear) = self.folder_heading(ui, key, &email, account);
                if gear {
                    settings = Some(account);
                }
                if open {
                    let account = account.unwrap_or_else(|| self.session.store.first_account());
                    let rows = folder_tree(&self.session.store.folders_of(account));
                    self.folder_tree_rows(ui, account, &rows);
                }
            }
            ui.add_space(10.0);
            if ui.small_button(tl("+ Add account")).on_hover_text(tl("Another mailbox, with folders of its own")).clicked() {
                settings = Some(None);
            }
        });
        if let Some(account) = settings {
            self.open_account_settings(account, "");
        }
    }

    /// A heading of the folder pane that folds what is under it: `(open, settings clicked)`.
    /// Accounts have a ⚙ for their settings.
    fn folder_heading(&mut self, ui: &mut Ui, key: u32, title: &str, account: Option<AccountId>) -> (bool, bool) {
        let t = self.tokens;
        let open = !self.folded.contains(&key);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
        let painter = ui.painter_at(rect);
        if resp.hovered() {
            painter.rect_filled(rect, 6.0, t.accent_soft.gamma_multiply(0.5));
        }
        let chevron = if open { "⏷" } else { "⏵" };
        painter.text(
            egui::pos2(rect.left() + 4.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            chevron,
            egui::FontId::proportional(11.0),
            t.text_weak,
        );
        let mut x = rect.left() + 20.0;
        if key == FOLD_FAVORITES {
            icons::paint(&painter, egui::Rect::from_center_size(egui::pos2(x + 8.0, rect.center().y), vec2(17.0, 17.0)), Icon::Star, t.text);
            x += 24.0;
        }
        let font = egui::FontId::proportional(theme::BODY);
        painter.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, title, font, t.text);
        let mut gear = false;
        if account.is_some() {
            let g = egui::Rect::from_center_size(egui::pos2(rect.right() - 12.0, rect.center().y), vec2(20.0, 20.0));
            let r = ui.interact(g, ui.id().with(("account-settings", key)), Sense::click());
            if resp.hovered() || r.hovered() {
                painter.text(
                    g.center(),
                    egui::Align2::CENTER_CENTER,
                    "⚙",
                    egui::FontId::proportional(15.0),
                    if r.hovered() { t.accent } else { t.text_weak },
                );
            }
            gear = r.on_hover_text(tf("Settings of {account}", &[("account", title)])).clicked();
        }
        if resp.clicked() && !gear {
            if open {
                self.folded.insert(key);
            } else {
                self.folded.remove(&key);
            }
        }
        (open, gear)
    }

    /// One row of the folder pane: a folder's icon, name and unread count, or a path its
    /// subfolders share that is no folder here (`id` `None`). `line` says how far in it sits,
    /// whether it folds and what it shows; true when its chevron was clicked.
    fn folder_row(&mut self, ui: &mut Ui, id: Option<FolderId>, line: &FolderLine) -> bool {
        let t = self.tokens;
        let store = &self.session.store;
        let folder = id.and_then(|id| store.folder(id).ok());
        if id.is_some() && folder.is_none() {
            return false;
        }
        let name = match (&line.label, folder) {
            (Some(l), _) => l.clone(),
            (None, Some(f)) => folder_label(f),
            (None, None) => String::new(),
        };
        let role = folder.map_or(FolderRole::Other, |f| f.role);
        let email = if line.with_account { folder.and_then(|f| store.account(f.account).ok()).map(|a| a.address.email.clone()) } else { None };
        let unread = id.and_then(|id| self.unread.get(&id).copied()).unwrap_or(0).saturating_add(line.folded_unread);
        let favorite = id.is_some_and(|id| store.is_favorite(id));
        let selected = id.is_some_and(|id| self.session.folder == id) && self.search.is_empty();
        let height = if email.is_some() { 46.0 } else { 34.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
        let painter = ui.painter_at(rect);
        if selected {
            painter.rect_filled(rect, 6.0, t.accent_soft);
            painter.rect_filled(egui::Rect::from_min_size(rect.min + vec2(0.0, 8.0), vec2(3.0, height - 16.0)), 1.5, t.accent);
        } else if resp.hovered() {
            painter.rect_filled(rect, 6.0, t.accent_soft.gamma_multiply(0.5));
        }
        let icon = match role {
            FolderRole::Inbox => Icon::Inbox,
            FolderRole::Drafts => Icon::Drafts,
            FolderRole::Sent => Icon::Sent,
            FolderRole::Outbox => Icon::Outbox,
            FolderRole::Archive => Icon::Archive,
            FolderRole::Junk => Icon::Junk,
            FolderRole::Trash => Icon::Trash,
            FolderRole::Other => Icon::Folder,
        };
        let strong = unread > 0;
        let color = if strong {
            t.unread
        } else if id.is_none() {
            t.text_weak
        } else {
            t.text
        };
        let line_y = if email.is_some() { rect.top() + 16.0 } else { rect.center().y };
        let indent = FOLDER_INDENT * line.depth as f32;
        let mut toggled = false;
        if let Some(open) = line.fold {
            let chevron = egui::Rect::from_center_size(egui::pos2(rect.left() + indent + 11.0, line_y), vec2(20.0, 24.0));
            let r = ui.interact(chevron, resp.id.with("fold"), Sense::click());
            if r.hovered() {
                painter.rect_filled(chevron, 4.0, t.accent_soft);
            }
            let glyph = if open { "⏷" } else { "⏵" };
            painter.text(chevron.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(11.0), t.text_weak);
            toggled = r.on_hover_text(tl(if open { "Hide subfolders" } else { "Show subfolders" })).clicked();
        }
        icons::paint(&painter, egui::Rect::from_center_size(egui::pos2(rect.left() + indent + 32.0, line_y), vec2(18.0, 18.0)), icon, color);
        let count = if unread > 0 { unread.to_string() } else { String::new() };
        let count_w = if count.is_empty() {
            0.0
        } else {
            let g = painter.layout_no_wrap(count.clone(), egui::FontId::proportional(13.0), t.unread);
            let w = g.size().x;
            painter.galley(egui::pos2(rect.right() - 10.0 - w, line_y - g.size().y / 2.0), g, t.unread);
            w + 12.0
        };
        let text_left = rect.left() + indent + 52.0;
        let room = (rect.right() - count_w - 10.0 - text_left).max(0.0);
        let font = egui::FontId::proportional(15.0);
        let mut job = egui::text::LayoutJob::simple_singleline(name.clone(), font, color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        let g = painter.layout_job(job);
        painter.galley(egui::pos2(text_left, line_y - g.size().y / 2.0), g, color);
        if let Some(email) = &email {
            let mut job = egui::text::LayoutJob::simple_singleline(email.clone(), egui::FontId::proportional(theme::CAPTION), t.text_weak);
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            let g = painter.layout_job(job);
            painter.galley(egui::pos2(text_left, rect.top() + 32.0 - g.size().y / 2.0), g, t.text_weak);
        }
        let resp = match (&email, folder) {
            (Some(e), _) => resp.on_hover_text(format!("{name} — {e}")),
            (None, Some(f)) if line.label.is_some() => resp.on_hover_text(folder_label(f)),
            _ => resp,
        };
        let Some(id) = id else {
            // A shared path is no folder to open: a click folds it.
            return toggled || resp.clicked();
        };
        if resp.clicked() && !toggled {
            self.search.clear();
            self.run("folder.select", json!({ "folder": id.0 }));
        }
        resp.context_menu(|ui| {
            let label = tl(if favorite { "Remove from Favorites" } else { "Add to Favorites" });
            if ui.button(label).clicked() {
                self.run("folder.favorite", json!({ "folder": id.0, "favorite": !favorite }));
                ui.close();
            }
            if self.session.store.can_delete_folder(id) && ui.button(tl("Delete folder")).clicked() {
                self.run("folder.delete", json!({ "folder": id.0 }));
                ui.close();
            }
        });
        toggled
    }

    /// An account's folders as a tree: subfolders under their parent, a folded parent's hidden
    /// and its unread mail counted on it.
    fn folder_tree_rows(&mut self, ui: &mut Ui, account: AccountId, rows: &[TreeRow]) {
        let mut hidden_below: Option<usize> = None;
        for (i, row) in rows.iter().enumerate() {
            if let Some(depth) = hidden_below {
                if row.depth > depth {
                    continue;
                }
                hidden_below = None;
            }
            let key = (account.0, row.key.clone());
            let open = !self.folded_paths.contains(&key);
            let folded_unread = if row.children && !open {
                hidden_below = Some(row.depth);
                rows.iter()
                    .skip(i + 1)
                    .take_while(|r| r.depth > row.depth)
                    .filter_map(|r| r.folder.and_then(|id| self.unread.get(&id).copied()))
                    .fold(0usize, usize::saturating_add)
            } else {
                0
            };
            let line = FolderLine {
                label: Some(row.label.clone()),
                depth: row.depth,
                fold: row.children.then_some(open),
                folded_unread,
                with_account: false,
            };
            if self.folder_row(ui, row.folder, &line) && row.children {
                if open {
                    self.folded_paths.insert(key);
                } else {
                    self.folded_paths.remove(&key);
                }
            }
        }
    }

    fn message_list(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let ids: Vec<MessageId> = if self.search.is_empty() {
            self.session.store.messages_in(self.session.folder).iter().map(|m| m.id).collect()
        } else {
            self.session.search_results(&self.search).iter().map(|m| m.id).collect()
        };
        let title = if self.search.is_empty() {
            self.session.store.folder(self.session.folder).map(folder_label).unwrap_or_default()
        } else {
            tf("Results for “{query}”", &[("query", &self.search)])
        };
        Frame::NONE.inner_margin(Margin { left: 18, right: 16, top: 14, bottom: 10 }).show(ui, |ui| {
            ui.label(RichText::new(title).heading().color(t.text));
            if !self.search.is_empty() {
                // Where to look, as Outlook's scope list; shown while searching.
                let scope = self.session.search_scope;
                let mut chosen = scope;
                egui::ComboBox::from_id_salt("search-scope").selected_text(tl(scope_label(scope))).show_ui(ui, |ui| {
                    for option in SearchScope::ALL {
                        ui.selectable_value(&mut chosen, option, tl(scope_label(option)));
                    }
                });
                if chosen != scope {
                    let q = self.search.clone();
                    self.run("search.messages", json!({ "query": q, "scope": chosen.id() }));
                }
            }
        });
        if ids.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(tl("Nothing here")).color(t.text_weak)));
            return;
        }
        // Only the rows in view are laid out: a folder can hold tens of thousands of messages.
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW_HEIGHT, ids.len(), |ui, rows| {
            for &id in ids.get(rows).unwrap_or_default() {
                let Ok(m) = self.session.store.message(id) else { continue };
                let selected = self.session.message == Some(id);
                let role = self.session.store.folder(m.folder).map(|f| f.role).ok();
                let who = if matches!(role, Some(FolderRole::Sent | FolderRole::Drafts)) {
                    m.to.first().map(|a| tf("To: {names}", &[("names", a.display())])).unwrap_or_else(|| tl("(no recipients)").into())
                } else {
                    m.from.as_ref().map(|a| a.display().to_string()).unwrap_or_else(|| tl("(unknown sender)").into())
                };
                let subject = if m.subject.is_empty() { tl("(no subject)").to_string() } else { m.subject.clone() };
                let when = m.date.map(|d| short_when(self.session.zone().from_utc(d), self.session.today())).unwrap_or_default();
                let (seen, flagged, attachment, preview) = (m.seen, m.flagged, !m.attachments.is_empty(), m.preview(80));
                let sender = m.from.as_ref().map(|a| a.email.clone());
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                let painter = ui.painter_at(rect);
                if selected {
                    painter.rect_filled(rect, 0.0, t.accent_soft);
                } else if resp.hovered() {
                    painter.rect_filled(rect, 0.0, t.accent_soft.gamma_multiply(0.5));
                }
                if !seen {
                    painter.rect_filled(egui::Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, t.unread);
                }
                let x = rect.left() + 18.0;
                let font = |size: f32| egui::FontId::proportional(size);
                let strong = if seen { t.text } else { t.unread };
                painter.text(egui::pos2(x, rect.top() + 12.0), egui::Align2::LEFT_TOP, &who, font(15.0), strong);
                painter.text(egui::pos2(rect.right() - 14.0, rect.top() + 13.0), egui::Align2::RIGHT_TOP, &when, font(theme::CAPTION), t.text_weak);
                let mut subj = subject;
                if attachment {
                    subj = format!("📎 {subj}");
                }
                painter.text(egui::pos2(x, rect.top() + 35.0), egui::Align2::LEFT_TOP, &subj, font(theme::BODY), if seen { t.text } else { strong });
                painter.text(egui::pos2(x, rect.top() + 57.0), egui::Align2::LEFT_TOP, &preview, font(13.0), t.text_weak);
                if flagged {
                    painter.text(egui::pos2(rect.right() - 14.0, rect.top() + 35.0), egui::Align2::RIGHT_TOP, "⚑", font(14.0), t.flag);
                }
                painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.border.gamma_multiply(0.6)));
                if resp.clicked() {
                    self.run("message.select", json!({ "id": id.0 }));
                }
                resp.context_menu(|ui| {
                    if ui.button(tl("Create rule…")).clicked() {
                        let query = sender.as_deref().map(|e| format!("from:{e}")).unwrap_or_default();
                        self.rule_form.start(&query);
                        ui.close();
                    }
                });
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
        if let Some(id) = self.session.message
            && self.content.as_ref().is_none_or(|(at, _)| *at != id)
        {
            self.content = Some((id, self.session.store.content(id).map_err(|e| e.to_string())));
        }
        let content = self.content.as_ref().map(|(_, c)| c.clone());
        // Borrowed, not cloned: an HTML body can be megabytes, and this runs every frame.
        let Some(m) = self.session.message.and_then(|id| self.session.store.message(id).ok()) else {
            // Centred, not justified: a justified label spreads its letters apart.
            let room = ui.available_height();
            ui.vertical_centered(|ui| {
                ui.add_space((room * 0.45).max(0.0));
                ui.label(RichText::new(tl("Select a message to read it")).color(t.text_weak));
            });
            return;
        };
        ui.label(RichText::new(if m.subject.is_empty() { tl("(no subject)") } else { &m.subject }).size(24.0).strong().color(t.text));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let who = m.from.as_ref().map(ToString::to_string).unwrap_or_default();
            ui.label(RichText::new(who).size(theme::SUBTITLE).strong().color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(m.date.map(|d| self.session.zone().from_utc(d).to_string().replace('T', " ")).unwrap_or_default())
                        .color(t.text_weak),
                );
            });
        });
        if !m.to.is_empty() {
            ui.label(
                RichText::new(tf("To: {names}", &[("names", &m.to.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))]))
                    .small()
                    .color(t.text_weak),
            );
        }
        for (label, list) in [("Cc", &m.cc), ("Bcc", &m.bcc)] {
            if !list.is_empty() {
                let names = list.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                ui.label(RichText::new(format!("{}: {names}", tl(label))).small().color(t.text_weak));
            }
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
        let rich = match &content {
            Some(Ok(c)) => c.rich.as_ref(),
            _ => None,
        };
        if let Some(rich) = rich {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if rich.remote_images > 0 {
                    let n = rich.remote_images;
                    ui.label(
                        RichText::new(format!(
                            "🛡 {}",
                            tf(if n == 1 { "{n} remote image not loaded" } else { "{n} remote images not loaded" }, &[("n", &n.to_string())])
                        ))
                        .small()
                        .color(t.text_weak),
                    )
                    .on_hover_text(tl("Images on other servers can tell the sender that you opened the message, so Tinta Correo never loads them."));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    toggle_plain = ui.small_button(tl(if self.session.plain_text { "Show formatted" } else { "Show as plain text" })).clicked();
                });
            });
        }
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match (&content, rich) {
            (_, Some(rich)) if !self.session.plain_text => rich_body(ui, &t, rich),
            (Some(Ok(c)), _) => {
                ui.add(egui::Label::new(RichText::new(&c.body).size(15.0).color(t.text)).wrap());
            }
            (Some(Err(e)), _) => {
                ui.label(RichText::new(tl("The text of this message could not be read.")).color(t.text_weak)).on_hover_text(e);
            }
            (None, _) => {}
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
            send = ui.add(egui::Button::new(RichText::new(tl("Send")).strong().color(Color32::WHITE)).fill(t.accent)).clicked();
            discard = ui.button(tl("Discard")).clicked();
            if !d.show_bcc && ui.button(tl("Bcc")).on_hover_text(tl("Blind copies: they get the message, the others don't see them")).clicked() {
                d.show_bcc = true;
            }
        });
        ui.add_space(8.0);
        let mut changed = false;
        egui::Grid::new("compose").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            let mut rows = vec![("To", &mut d.to), ("Cc", &mut d.cc)];
            if d.show_bcc {
                rows.push(("Bcc", &mut d.bcc));
            }
            rows.push(("Subject", &mut d.subject));
            for (label, field) in rows {
                ui.label(RichText::new(tl(label)).color(t.text_weak));
                changed |= ui.add(egui::TextEdit::singleline(field).desired_width(f32::INFINITY)).lost_focus();
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        changed |= ui.add_sized(ui.available_size(), egui::TextEdit::multiline(&mut d.body)).lost_focus();
        let params = json!({ "id": d.id, "to": d.to, "cc": d.cc, "bcc": d.bcc, "subject": d.subject, "body": d.body });
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
        let fit = self.fit_panes(ui, CALENDAR_MAIN_MIN, ("calendar-pane", 260.0, 210.0, 400.0), None, Some(("agenda", 300.0, 240.0, 440.0)), false);
        if fit.left {
            egui::Panel::left("calendar-pane")
                .default_size(260.0)
                .size_range(210.0..=fit.left_max)
                .frame(Frame::NONE.inner_margin(Margin { left: 4, right: 10, top: 0, bottom: 10 }))
                .show(ui, |ui| {
                    if self.pane_head(ui, Icon::Calendar, "New Appointment") {
                        self.new_appointment();
                    }
                    ui.add_space(4.0);
                    self.mini_month(ui, false);
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.calendar_list(ui));
                });
        }
        egui::Panel::top("calendar-tools").frame(Frame::NONE).show(ui, |ui| self.calendar_tools(ui));
        if fit.right {
            egui::Panel::right("agenda").default_size(300.0).size_range(240.0..=fit.right_max).frame(chrome::card(&t, 14)).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.agenda(ui));
            });
        }
        egui::CentralPanel::no_frame().frame(chrome::card(&t, 12)).show(ui, |ui| {
            self.main_width = ui.available_width() + 24.0;
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
        let names = WEEKDAYS.map(tl);
        let avail = ui.available_size();
        let cell_w = (avail.x / 7.0).max(40.0);
        let cell_h = ((avail.y - 28.0) / 6.0).max(48.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for n in names {
                ui.add_sized([cell_w, 24.0], egui::Label::new(RichText::new(n).small().color(t.text_weak)));
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
                            t.card
                        } else {
                            theme::mix(t.card, t.window, 0.6)
                        },
                    );
                    p.rect_stroke(rect, 0.0, Stroke::new(0.5, t.border), StrokeKind::Inside);
                    let num_pos = rect.left_top() + vec2(8.0, 6.0);
                    if day == today {
                        p.circle_filled(num_pos + vec2(9.0, 10.0), 13.0, t.today);
                        p.text(
                            num_pos + vec2(9.0, 10.0),
                            egui::Align2::CENTER_CENTER,
                            day.day().to_string(),
                            egui::FontId::proportional(13.0),
                            Color32::WHITE,
                        );
                    } else {
                        p.text(
                            num_pos,
                            egui::Align2::LEFT_TOP,
                            day.day().to_string(),
                            egui::FontId::proportional(13.0),
                            if in_month { t.text } else { t.text_weak },
                        );
                    }
                    let mut y = rect.top() + 32.0;
                    for o in self.session.store.events_on(day, self.session.zone()) {
                        let e = o.event;
                        if y + 20.0 > rect.bottom() {
                            p.text(egui::pos2(rect.left() + 8.0, y), egui::Align2::LEFT_TOP, "…", egui::FontId::proportional(12.0), t.text_weak);
                            break;
                        }
                        let chip = egui::Rect::from_min_size(egui::pos2(rect.left() + 4.0, y), vec2(rect.width() - 8.0, 20.0));
                        let repeat = if e.rrule.is_some() { "⟳ " } else { "" };
                        let label = match o.when {
                            When::Timed { start, .. } if start.date == day => {
                                format!("{:02}:{:02} {repeat}{}", start.hour(), start.minute(), e.summary)
                            }
                            _ => format!("{repeat}{}", e.summary),
                        };
                        p.rect_filled(chip, 3.0, self.calendar_color(e.calendar).gamma_multiply(0.85));
                        p.with_clip_rect(chip).text(
                            chip.left_center() + vec2(6.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(12.0),
                            Color32::WHITE,
                        );
                        y += 22.0;
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
        const GUTTER: f32 = 56.0;
        const HOUR: f32 = 52.0;
        const ALL_DAY_ROW: f32 = 24.0;
        let t = self.tokens;
        let (from, to) = self.session.calendar_range();
        let count = usize::try_from(to.day_number().saturating_sub(from.day_number()).saturating_add(1)).unwrap_or(1).clamp(1, 7);
        let days: Vec<Date> = (0..count).filter_map(|n| from.add_days(i64::try_from(n).unwrap_or(0)).ok()).collect();
        let now = self.session.local_now();
        let col_w = ((ui.available_width() - GUTTER) / days.len().max(1) as f32).max(60.0);
        let x_of = |left: f32, n: usize| left + GUTTER + col_w * n as f32;

        // Day headings.
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::hover());
        let p = ui.painter_at(head);
        for (n, day) in days.iter().enumerate() {
            let x = x_of(head.left(), n);
            let is_today = *day == now.date;
            let colour = if is_today { t.today } else { t.text };
            p.text(
                egui::pos2(x + 8.0, head.top() + 4.0),
                egui::Align2::LEFT_TOP,
                weekday_abbr(*day),
                egui::FontId::proportional(theme::CAPTION),
                t.text_weak,
            );
            p.text(egui::pos2(x + 8.0, head.top() + 20.0), egui::Align2::LEFT_TOP, day.day().to_string(), egui::FontId::proportional(22.0), colour);
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
            p.rect_filled(strip, 0.0, theme::mix(t.card, t.window, 0.6));
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
                    p.rect_filled(chip, 3.0, soft(self.calendar_color(*calendar), t.card));
                    p.with_clip_rect(chip).text(
                        chip.left_center() + vec2(4.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        summary,
                        egui::FontId::proportional(12.5),
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
                        theme::mix(t.card, t.window, 0.6)
                    } else {
                        t.card
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
                        egui::FontId::proportional(theme::CAPTION),
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
                    let bottom = y_of(*end).max(top + 24.0);
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
                    p.rect_filled(rect, 3.0, soft(color, t.card));
                    p.rect_filled(egui::Rect::from_min_size(rect.left_top(), vec2(3.0, rect.height())), 1.0, color);
                    let clip = p.with_clip_rect(rect.shrink(1.0));
                    let repeat = if e.rrule.is_some() { "⟳ " } else { "" };
                    let title = format!("{repeat}{}", e.summary);
                    if rect.height() < 36.0 {
                        clip.text(rect.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(13.0), t.text);
                    } else {
                        clip.text(rect.left_top() + vec2(8.0, 4.0), egui::Align2::LEFT_TOP, title, egui::FontId::proportional(13.0), t.text);
                    }
                    if rect.height() > 40.0 {
                        let detail = if e.location.is_empty() { times } else { e.location.clone() };
                        clip.text(
                            rect.left_top() + vec2(8.0, 22.0),
                            egui::Align2::LEFT_TOP,
                            detail,
                            egui::FontId::proportional(theme::CAPTION),
                            t.text_weak,
                        );
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
            ui.label(RichText::new(tl("No appointments")).color(t.text_weak));
        }
        for (e, occurrence) in events {
            let when = match occurrence {
                When::AllDay { .. } => tl("All day").to_string(),
                When::Timed { start, end, .. } => format!("{:02}:{:02} – {:02}:{:02}", start.hour(), start.minute(), end.hour(), end.minute()),
            };
            Frame::NONE.fill(t.card).corner_radius(CornerRadius::same(6)).stroke(Stroke::new(1.0, t.border)).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(when).small().color(t.accent));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if e.rrule.is_some() {
                            if ui.small_button(tl("🗑 Series")).on_hover_text(tl("Delete every occurrence")).clicked() {
                                self.run("event.delete", json!({ "id": e.id.0 }));
                            }
                            if ui.small_button("🗑").on_hover_text(tl("Delete this occurrence only")).clicked() {
                                self.run("event.delete", json!({ "id": e.id.0, "occurrence": occurrence.start_date().to_string() }));
                            }
                        } else if ui.small_button("🗑").on_hover_text(tl("Delete appointment")).clicked() {
                            self.run("event.delete", json!({ "id": e.id.0 }));
                        }
                    });
                });
                ui.label(RichText::new(&e.summary).strong().color(t.text));
                if let Some(r) = &e.rrule {
                    let text = e.rule().map_or_else(
                        || tf("Repeats ({rule}), shown once", &[("rule", &r.to_string())]),
                        |rule| format!("⟳ {}", describe_rule(&rule)),
                    );
                    ui.label(RichText::new(text).small().color(t.text_weak));
                }
                if !e.location.is_empty() {
                    ui.label(RichText::new(format!("📍 {}", e.location)).small().color(t.text_weak));
                }
                if let Some(m) = e.reminders.first() {
                    let text = match *m {
                        0 => format!("🔔 {}", tl("At start time")),
                        m if m > 0 => format!("🔔 {}", tf("{span} before", &[("span", &span(i64::from(m)))])),
                        m => format!("🔔 {}", tf("{span} after the start", &[("span", &span(i64::from(m)))])),
                    };
                    ui.label(RichText::new(text).small().color(t.text_weak));
                }
            });
            ui.add_space(6.0);
        }
    }

    /// The calendars, with their colours; the ones synced from a server are marked.
    fn calendar_list(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        ui.add_space(12.0);
        ui.label(RichText::new(tl("Calendars")).strong().color(t.text));
        let calendars: Vec<(u32, String, bool)> = self.session.store.calendars().iter().map(|c| (c.id, c.name.clone(), c.remote.is_some())).collect();
        for (id, name, synced) in calendars {
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                ui.painter().rect_filled(dot, 3.0, self.calendar_color(id));
                ui.label(RichText::new(name).color(t.text));
                if synced {
                    ui.label(RichText::new("⟳").small().color(t.text_weak)).on_hover_text(tl("Synced with the CalDAV server at Send/Receive"));
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
        egui::Window::new(tl("New Appointment")).collapsible(false).resizable(false).open(&mut open).show(ui.ctx(), |ui| {
            let f = &mut self.event_form;
            egui::Grid::new("event-form").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                for (label, field, hint) in [
                    ("Title", &mut f.summary, ""),
                    ("Start", &mut f.start, "2026-10-22T09:00 or 2026-10-22"),
                    ("End", &mut f.end, "optional"),
                    ("Location", &mut f.location, ""),
                ] {
                    ui.label(tl(label));
                    ui.add(egui::TextEdit::singleline(field).hint_text(tl(hint)).desired_width(260.0));
                    ui.end_row();
                }
                ui.label(tl("Reminder"));
                let shown = tl(REMINDERS.iter().find(|(id, _)| *id == f.reminder.as_str()).map_or("15 minutes", |(_, label)| *label));
                egui::ComboBox::from_id_salt("event-reminder").selected_text(shown).show_ui(ui, |ui| {
                    for (id, label) in REMINDERS {
                        ui.selectable_value(&mut f.reminder, id.to_string(), tl(label));
                    }
                });
                ui.end_row();
                if calendars.len() > 1 {
                    ui.label(tl("Calendar"));
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
                ui.label(tl("Repeat"));
                let shown = tl(REPEATS.iter().find(|(id, _)| *id == f.repeat.as_str()).map_or("Never", |(_, label)| *label));
                egui::ComboBox::from_id_salt("event-repeat").selected_text(shown).show_ui(ui, |ui| {
                    for (id, label) in REPEATS {
                        ui.selectable_value(&mut f.repeat, id.to_string(), tl(label));
                    }
                });
                ui.end_row();
            });
            save = ui.button(tl("Save")).clicked();
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
        let books: Vec<(u32, String, bool)> = self.session.store.books().iter().map(|b| (b.id, b.name.clone(), b.remote.is_some())).collect();
        if self.contact_book.is_some_and(|id| !books.iter().any(|b| b.0 == id)) {
            self.contact_book = None;
        }
        let fit = self.fit_panes(ui, PEOPLE_MAIN_MIN, ("books", 260.0, 190.0, 400.0), None, Some(("contact-card", 300.0, 240.0, 420.0)), true);
        if fit.left {
            egui::Panel::left("books")
                .default_size(260.0)
                .size_range(190.0..=fit.left_max)
                .frame(Frame::NONE.inner_margin(Margin { left: 4, right: 10, top: 0, bottom: 10 }))
                .show(ui, |ui| {
                    if self.pane_head(ui, Icon::PersonAdd, "New Contact") {
                        self.contact_form = ContactForm::new_in(self.default_book());
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new(tl("Address books")).small().strong().color(t.text_weak));
                    let total = self.session.store.contacts("").len();
                    if self.pane_row(ui, Icon::People, tl("All contacts"), total, self.contact_book.is_none()) {
                        self.contact_book = None;
                    }
                    for (id, name, synced) in &books {
                        let n = self.session.store.contacts("").iter().filter(|c| c.book == *id).count();
                        let label = if *synced { format!("{name} ⟳") } else { name.clone() };
                        if self.pane_row(ui, Icon::Folder, &label, n, self.contact_book == Some(*id)) {
                            self.contact_book = Some(*id);
                        }
                    }
                });
        }
        egui::Panel::top("people-tools").frame(Frame::NONE).show(ui, |ui| self.people_tools(ui));
        // The contact card is where a contact is read and edited: always there, at most as wide
        // as leaves the table its room.
        egui::Panel::right("contact-card").default_size(300.0).size_range(240.0..=fit.right_max).frame(chrome::card(&t, 14)).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.contact_card(ui));
        });
        egui::CentralPanel::no_frame().frame(chrome::card(&t, 0)).show(ui, |ui| {
            self.main_width = ui.available_width();
            self.contact_table(ui);
        });
    }

    /// A row of a left pane: an icon, a name, a count; shaded when chosen. True when clicked.
    fn pane_row(&self, ui: &mut Ui, icon: Icon, label: &str, count: usize, chosen: bool) -> bool {
        let t = self.tokens;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
        if chosen || resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, if chosen { t.accent_soft } else { t.accent_soft.gamma_multiply(0.5) });
        }
        let color = if chosen { t.accent } else { t.text };
        icons::paint(
            ui.painter(),
            Rect::from_center_size(egui::pos2(rect.left() + 18.0, rect.center().y), vec2(18.0, 18.0)),
            icon,
            if chosen { t.accent } else { t.text_weak },
        );
        let font = egui::FontId::proportional(theme::BODY);
        ui.painter().text(egui::pos2(rect.left() + 38.0, rect.center().y), egui::Align2::LEFT_CENTER, label, font.clone(), color);
        if count > 0 {
            ui.painter().text(
                egui::pos2(rect.right() - 10.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                count.to_string(),
                egui::FontId::proportional(13.0),
                t.text_weak,
            );
        }
        resp.clicked()
    }

    /// The contacts as a table: name, e-mail, phone, address book, title and company. A click
    /// shows the contact's card.
    fn contact_table(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        Frame::NONE.inner_margin(Margin { left: 18, right: 16, top: 14, bottom: 10 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tl("Contacts")).size(theme::TITLE).strong().color(t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.contact_query).hint_text(tl("Search people")).desired_width(260.0));
                });
            });
        });
        let q = self.contact_query.to_lowercase();
        let book = self.contact_book;
        let contacts: Vec<_> = self.session.store.contacts(&q).into_iter().filter(|c| book.is_none_or(|b| c.book == b)).cloned().collect();
        let books: std::collections::HashMap<u32, String> = self.session.store.books().iter().map(|b| (b.id, b.name.clone())).collect();
        if contacts.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(tl(if q.is_empty() {
                        "No contacts yet. Add one, or open a .vcf file with Tinta Correo."
                    } else {
                        "Nobody matches."
                    }))
                    .color(t.text_weak),
                );
            });
            return;
        }
        const SHARES: [f32; 5] = [0.26, 0.27, 0.16, 0.13, 0.18];
        let headings = ["Name", "Email", "Phone", "Address book", "Title and company"];
        let font = egui::FontId::proportional(theme::BODY);
        let small = egui::FontId::proportional(theme::CAPTION);
        let columns = |rect: Rect| -> Vec<Rect> {
            let mut x = rect.left() + 18.0;
            let w = (rect.width() - 34.0).max(200.0);
            SHARES
                .iter()
                .map(|s| {
                    let r = Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + w * s - 8.0, rect.bottom()));
                    x += w * s;
                    r
                })
                .collect()
        };
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        for (r, h) in columns(head).into_iter().zip(headings) {
            ui.painter().with_clip_rect(r).text(egui::pos2(r.left(), r.center().y), egui::Align2::LEFT_CENTER, tl(h), small.clone(), t.text_weak);
        }
        ui.painter().line_segment([head.left_bottom(), head.right_bottom()], Stroke::new(1.0, t.border));
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, 48.0, contacts.len(), |ui, rows| {
            for c in contacts.get(rows).unwrap_or_default() {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
                let chosen = self.contact_form.id == Some(c.id);
                if chosen || resp.hovered() {
                    ui.painter().rect_filled(rect, 0.0, if chosen { t.accent_soft } else { t.accent_soft.gamma_multiply(0.5) });
                }
                let cols = columns(rect);
                let initials: String = c.name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
                let company = match (&c.title, &c.organization) {
                    (Some(a), Some(b)) => format!("{a}, {b}"),
                    (Some(a), None) | (None, Some(a)) => a.clone(),
                    (None, None) => String::new(),
                };
                let cells = [
                    c.name.clone(),
                    c.emails.first().map(|a| a.email.clone()).unwrap_or_default(),
                    c.phones.first().map(|p| p.number.clone()).unwrap_or_default(),
                    books.get(&c.book).cloned().unwrap_or_default(),
                    company,
                ];
                for (i, (r, text)) in cols.iter().zip(cells).enumerate() {
                    let p = ui.painter().with_clip_rect(*r);
                    if i == 0 {
                        let dot = egui::pos2(r.left() + 15.0, r.center().y);
                        p.circle_filled(dot, 15.0, avatar_color(&c.name));
                        p.text(dot, egui::Align2::CENTER_CENTER, &initials, small.clone(), Color32::WHITE);
                        p.text(egui::pos2(r.left() + 40.0, r.center().y), egui::Align2::LEFT_CENTER, text, font.clone(), t.text);
                    } else {
                        p.text(
                            egui::pos2(r.left(), r.center().y),
                            egui::Align2::LEFT_CENTER,
                            text,
                            font.clone(),
                            if i == 1 { t.accent } else { t.text },
                        );
                    }
                }
                ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.border.gamma_multiply(0.6)));
                if resp.clicked() {
                    self.contact_form = ContactForm::of(c);
                }
            }
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
        ui.label(RichText::new(tl(if f.id.is_some() { "Contact" } else { "New contact" })).strong());
        ui.add_space(6.0);
        let field = |ui: &mut Ui, label: &str, value: &mut String, hint: &str| {
            ui.label(RichText::new(label).small().color(t.text_weak));
            ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(f32::INFINITY));
        };
        field(ui, "Name", &mut f.name, "");
        field(ui, "Email", &mut f.email, "one or more, comma-separated");
        ui.label(RichText::new(tl("Phone")).small().color(t.text_weak));
        let mut remove = None;
        for (i, (kind, number)) in f.phones.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("phone-kind", i)).width(70.0).selected_text(phone_kind_label(kind)).show_ui(ui, |ui| {
                    for k in ibercorreo_core::contact::PHONE_KINDS {
                        ui.selectable_value(kind, (*k).to_string(), phone_kind_label(k));
                    }
                });
                ui.add(egui::TextEdit::singleline(number).desired_width((ui.available_width() - 40.0).max(60.0)));
                if ui.small_button("✖").on_hover_text(tl("Remove this phone")).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            f.phones.remove(i);
        }
        if f.phones.len() < 50 && ui.small_button(tl("➕ Add phone")).clicked() {
            f.phones.push((if f.phones.is_empty() { "mobile" } else { "work" }.into(), String::new()));
        }
        field(ui, "Organization", &mut f.organization, "");
        field(ui, "Job title", &mut f.title, "");
        field(ui, "Birthday", &mut f.birthday, "YYYY-MM-DD");
        ui.label(RichText::new(tl("Notes")).small().color(t.text_weak));
        ui.add(egui::TextEdit::multiline(&mut f.note).desired_rows(3).desired_width(f32::INFINITY));
        if books.len() > 1 {
            ui.label(RichText::new(tl("Address book")).small().color(t.text_weak));
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
            save = ui.button(tl(if f.id.is_some() { "Save" } else { "Add" })).clicked();
            if f.id.is_some() {
                delete = ui.button(tl("🗑 Delete")).clicked();
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
    let fill = if cancelled { soft(t.flag, t.card) } else { t.accent_soft };
    Frame::NONE.fill(fill).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let answered = |s: &str| {
            tl(match s {
                "ACCEPTED" => "accepted",
                "TENTATIVE" => "tentatively accepted",
                "DECLINED" => "declined",
                "DELEGATED" => "delegated",
                _ => "answered",
            })
        };
        let headline = match info.method.as_str() {
            "CANCEL" => tl("This meeting has been cancelled").to_string(),
            "REPLY" => tf(
                "{who} has {status} this meeting",
                &[("who", info.replier.as_deref().unwrap_or(tl("An attendee"))), ("status", info.status.as_deref().map_or(tl("answered"), answered))],
            ),
            "PUBLISH" => tl("Event").to_string(),
            "COUNTER" => tl("An attendee proposes a new time").to_string(),
            _ => match &info.organizer {
                Some(o) => tf("Meeting invitation from {organizer}", &[("organizer", o)]),
                None => tl("Meeting invitation").to_string(),
            },
        };
        ui.label(RichText::new(format!("📅 {headline}")).small().color(t.text_weak));
        ui.label(RichText::new(if info.summary.is_empty() { tl("(no title)") } else { &info.summary }).size(16.0).strong().color(t.text));
        let mut when = match info.when {
            Some(When::AllDay { start, end }) => {
                let last = end.add_days(-1).unwrap_or(start);
                if last <= start {
                    tf("{date}, all day", &[("date", &long_date(start))])
                } else {
                    format!("{} – {}", long_date(start), long_date(last))
                }
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
            when.push_str(&format!(" · {}", tl("repeats")));
        }
        if !info.location.is_empty() {
            when.push_str(&format!(" · {}", info.location));
        }
        ui.label(RichText::new(when).color(t.text));
        if info.in_calendar {
            ui.label(RichText::new(tl("✔ In your calendar")).small().color(t.accent));
        } else if info.method == "REQUEST"
            && let Some(status) = info.status.as_deref().filter(|s| *s != "NEEDS-ACTION")
        {
            ui.label(RichText::new(tf("You have {status} it", &[("status", answered(status))])).small().color(t.text_weak));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| match info.method.as_str() {
            "REQUEST" | "PUBLISH" => {
                let mode = ["send", "edit", "none"].get(*reply).copied().unwrap_or("send");
                for (label, response) in [("✔ Accept", "accept"), ("？ Tentative", "tentative"), ("✖ Decline", "decline")] {
                    if ui.button(tl(label)).clicked() {
                        out = Some(json!({ "response": response, "reply": mode }));
                    }
                }
                if info.method == "REQUEST" && info.organizer.is_some() {
                    let names = ["Send the response now", "Edit the response first", "Do not send a response"].map(tl);
                    egui::ComboBox::from_id_salt("invite-reply").selected_text(names.get(*reply).copied().unwrap_or(names[0])).show_ui(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            ui.selectable_value(reply, i, *n);
                        }
                    });
                }
            }
            "CANCEL" if ui.add_enabled(info.in_calendar, egui::Button::new(tl("Remove from Calendar"))).clicked() => out = Some(json!({})),
            _ => {}
        });
    });
    out
}

fn short_when(dt: ibercorreo_core::DateTime, today: Date) -> String {
    if dt.date == today {
        format!("{:02}:{:02}", dt.hour(), dt.minute())
    } else if dt.date.year() == today.year() {
        // Lithuanian puts the month first: `spal. 26 d.`.
        if i18n::current() == Language::Lt {
            return format!("{} {} d.", month_abbr(dt.date.month()), dt.date.day());
        }
        format!("{} {}", day_num(dt.date), month_abbr(dt.date.month()))
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
    let names = match i18n::current() {
        Language::Uk => MONTHS_ABBR_UK,
        Language::Es => MONTHS_ABBR_ES,
        Language::Ca => MONTHS_ABBR_CA,
        Language::Gl => MONTHS_ABBR_GL,
        Language::Eu => MONTHS_ABBR_EU,
        Language::It => ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"],
        Language::Fr => ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
        Language::De => ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.", "Dez."],
        Language::Pl => ["sty", "lut", "mar", "kwi", "maj", "cze", "lip", "sie", "wrz", "paź", "lis", "gru"],
        Language::Ro => ["ian.", "feb.", "mar.", "apr.", "mai", "iun.", "iul.", "aug.", "sept.", "oct.", "nov.", "dec."],
        Language::Cs => ["led.", "úno.", "bře.", "dub.", "kvě.", "čvn.", "čvc.", "srp.", "zář.", "říj.", "lis.", "pro."],
        Language::Lt => MONTHS_ABBR_LT,
        Language::En => ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    };
    names.get(usize::from(m).saturating_sub(1)).copied().unwrap_or("")
}

/// A day number before a month: `26`, but `26.` in German and Czech.
fn day_num(d: Date) -> String {
    if matches!(i18n::current(), Language::De | Language::Cs) { format!("{}.", d.day()) } else { d.day().to_string() }
}

fn weekday_abbr(d: Date) -> &'static str {
    WEEKDAYS.get(usize::from(d.weekday().index())).copied().map_or("", tl)
}

/// The ribbon's title for what the calendar shows: `22 October 2026`, `19–25 October 2026`,
/// `26 October – 1 November 2026` or `October 2026`.
fn range_title(s: &Session) -> String {
    let (from, to) = s.calendar_range();
    if i18n::current() == Language::Eu {
        return range_title_eu(s.calendar_mode, s.day, from, to);
    }
    if i18n::current() == Language::Lt {
        return range_title_lt(s.calendar_mode, s.day, from, to);
    }
    match s.calendar_mode {
        CalendarMode::Month => month_title(s.day),
        CalendarMode::Day => format!("{} {}", day_num(from), month_year(from)),
        _ if from.month() == to.month() => format!("{}–{} {}", day_num(from), day_num(to), month_year(to)),
        _ if from.year() == to.year() => format!("{} {} – {} {}", day_num(from), month_name(from), day_num(to), month_year(to)),
        _ => format!("{} {} – {} {}", day_num(from), month_year(from), day_num(to), month_year(to)),
    }
}

/// [`range_title`] in Basque, which puts the year first and the month in the genitive:
/// `2026ko urriaren 22a`, `2026ko urriaren 19–25`, `2026ko urriaren 26 – azaroaren 1`, `2026ko urria`.
fn range_title_eu(mode: CalendarMode, day: Date, from: Date, to: Date) -> String {
    let genitive = |d: Date| MONTHS_GENITIVE_EU.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or("");
    match mode {
        CalendarMode::Month => month_title(day),
        CalendarMode::Day => format!("{} {} {}a", year_eu(from.year()), genitive(from), from.day()),
        _ if from.month() == to.month() => format!("{} {} {}–{}", year_eu(to.year()), genitive(to), from.day(), to.day()),
        _ if from.year() == to.year() => format!("{} {} {} – {} {}", year_eu(to.year()), genitive(from), from.day(), genitive(to), to.day()),
        _ => format!("{} {} {} – {} {} {}", year_eu(from.year()), genitive(from), from.day(), year_eu(to.year()), genitive(to), to.day()),
    }
}

/// [`range_title`] in Lithuanian, which puts the year first and the month in the genitive:
/// `2026 m. spalio 22 d.`, `2026 m. spalio 19–25 d.`, `2026 m. spalio 26 d. – lapkričio 1 d.`,
/// `2026 m. spalis`.
fn range_title_lt(mode: CalendarMode, day: Date, from: Date, to: Date) -> String {
    let genitive = |d: Date| MONTHS_GENITIVE_LT.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or("");
    match mode {
        CalendarMode::Month => month_title(day),
        CalendarMode::Day => format!("{} m. {} {} d.", from.year(), genitive(from), from.day()),
        _ if from.month() == to.month() => format!("{} m. {} {}–{} d.", to.year(), genitive(to), from.day(), to.day()),
        _ if from.year() == to.year() => format!("{} m. {} {} d. – {} {} d.", to.year(), genitive(from), from.day(), genitive(to), to.day()),
        _ => format!("{} m. {} {} d. – {} m. {} {} d.", from.year(), genitive(from), from.day(), to.year(), genitive(to), to.day()),
    }
}

/// A Basque year with its "of" suffix: `2026ko`, but `2025eko`, `2030eko` (after a consonant sound).
fn year_eu(year: i32) -> String {
    let n = year.rem_euclid(100);
    let consonant = match n % 10 {
        1 | 5 => true,
        0 if n != 0 => (n / 10) % 2 == 1,
        _ => false,
    };
    format!("{year}{}", if consonant { "eko" } else { "ko" })
}

/// The month's name after a day number: `22 October`, `22 жовтня` (Ukrainian takes the genitive),
/// `22 de octubre`, `22 d'octubre`.
fn month_name(d: Date) -> &'static str {
    let names = match i18n::current() {
        Language::Uk => MONTHS_GENITIVE_UK,
        Language::Es => MONTHS_AFTER_DAY_ES,
        Language::Ca => MONTHS_AFTER_DAY_CA,
        Language::Gl => MONTHS_AFTER_DAY_GL,
        Language::Eu => MONTHS_GENITIVE_EU,
        Language::It => MONTHS_AFTER_DAY_IT,
        Language::Fr => MONTHS_AFTER_DAY_FR,
        Language::De => MONTHS_DE,
        Language::Pl => MONTHS_GENITIVE_PL,
        Language::Ro => MONTHS_AFTER_DAY_RO,
        Language::Cs => MONTHS_GENITIVE_CS,
        Language::Lt => MONTHS_GENITIVE_LT,
        Language::En => MONTHS,
    };
    names.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or("")
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const MONTHS_UK: [&str; 12] =
    ["Січень", "Лютий", "Березень", "Квітень", "Травень", "Червень", "Липень", "Серпень", "Вересень", "Жовтень", "Листопад", "Грудень"];
const MONTHS_GENITIVE_UK: [&str; 12] =
    ["січня", "лютого", "березня", "квітня", "травня", "червня", "липня", "серпня", "вересня", "жовтня", "листопада", "грудня"];
const MONTHS_ABBR_UK: [&str; 12] = ["січ.", "лют.", "бер.", "квіт.", "трав.", "черв.", "лип.", "серп.", "вер.", "жовт.", "лист.", "груд."];
const MONTHS_ES: [&str; 12] =
    ["Enero", "Febrero", "Marzo", "Abril", "Mayo", "Junio", "Julio", "Agosto", "Septiembre", "Octubre", "Noviembre", "Diciembre"];
const MONTHS_AFTER_DAY_ES: [&str; 12] = [
    "de enero",
    "de febrero",
    "de marzo",
    "de abril",
    "de mayo",
    "de junio",
    "de julio",
    "de agosto",
    "de septiembre",
    "de octubre",
    "de noviembre",
    "de diciembre",
];
const MONTHS_CA: [&str; 12] = ["Gener", "Febrer", "Març", "Abril", "Maig", "Juny", "Juliol", "Agost", "Setembre", "Octubre", "Novembre", "Desembre"];
const MONTHS_AFTER_DAY_CA: [&str; 12] = [
    "de gener",
    "de febrer",
    "de març",
    "d'abril",
    "de maig",
    "de juny",
    "de juliol",
    "d'agost",
    "de setembre",
    "d'octubre",
    "de novembre",
    "de desembre",
];
const MONTHS_ABBR_CA: [&str; 12] = ["gen.", "febr.", "març", "abr.", "maig", "juny", "jul.", "ag.", "set.", "oct.", "nov.", "des."];
const MONTHS_GL: [&str; 12] =
    ["Xaneiro", "Febreiro", "Marzo", "Abril", "Maio", "Xuño", "Xullo", "Agosto", "Setembro", "Outubro", "Novembro", "Decembro"];
const MONTHS_AFTER_DAY_GL: [&str; 12] = [
    "de xaneiro",
    "de febreiro",
    "de marzo",
    "de abril",
    "de maio",
    "de xuño",
    "de xullo",
    "de agosto",
    "de setembro",
    "de outubro",
    "de novembro",
    "de decembro",
];
const MONTHS_ABBR_GL: [&str; 12] = ["xan.", "feb.", "mar.", "abr.", "maio", "xuño", "xul.", "ago.", "set.", "out.", "nov.", "dec."];
const MONTHS_EU: [&str; 12] =
    ["urtarrila", "otsaila", "martxoa", "apirila", "maiatza", "ekaina", "uztaila", "abuztua", "iraila", "urria", "azaroa", "abendua"];
const MONTHS_GENITIVE_EU: [&str; 12] = [
    "urtarrilaren",
    "otsailaren",
    "martxoaren",
    "apirilaren",
    "maiatzaren",
    "ekainaren",
    "uztailaren",
    "abuztuaren",
    "irailaren",
    "urriaren",
    "azaroaren",
    "abenduaren",
];
const MONTHS_ABBR_EU: [&str; 12] = ["urt.", "ots.", "mar.", "api.", "mai.", "eka.", "uzt.", "abu.", "ira.", "urr.", "aza.", "abe."];
const MONTHS_IT: [&str; 12] =
    ["Gennaio", "Febbraio", "Marzo", "Aprile", "Maggio", "Giugno", "Luglio", "Agosto", "Settembre", "Ottobre", "Novembre", "Dicembre"];
const MONTHS_AFTER_DAY_IT: [&str; 12] =
    ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"];
const MONTHS_FR: [&str; 12] =
    ["Janvier", "Février", "Mars", "Avril", "Mai", "Juin", "Juillet", "Août", "Septembre", "Octobre", "Novembre", "Décembre"];
const MONTHS_AFTER_DAY_FR: [&str; 12] =
    ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"];
const MONTHS_DE: [&str; 12] = ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"];
const MONTHS_PL: [&str; 12] =
    ["Styczeń", "Luty", "Marzec", "Kwiecień", "Maj", "Czerwiec", "Lipiec", "Sierpień", "Wrzesień", "Październik", "Listopad", "Grudzień"];
const MONTHS_GENITIVE_PL: [&str; 12] =
    ["stycznia", "lutego", "marca", "kwietnia", "maja", "czerwca", "lipca", "sierpnia", "września", "października", "listopada", "grudnia"];
const MONTHS_RO: [&str; 12] =
    ["Ianuarie", "Februarie", "Martie", "Aprilie", "Mai", "Iunie", "Iulie", "August", "Septembrie", "Octombrie", "Noiembrie", "Decembrie"];
const MONTHS_AFTER_DAY_RO: [&str; 12] =
    ["ianuarie", "februarie", "martie", "aprilie", "mai", "iunie", "iulie", "august", "septembrie", "octombrie", "noiembrie", "decembrie"];
const MONTHS_CS: [&str; 12] = ["Leden", "Únor", "Březen", "Duben", "Květen", "Červen", "Červenec", "Srpen", "Září", "Říjen", "Listopad", "Prosinec"];
const MONTHS_GENITIVE_CS: [&str; 12] =
    ["ledna", "února", "března", "dubna", "května", "června", "července", "srpna", "září", "října", "listopadu", "prosince"];
const MONTHS_LT: [&str; 12] =
    ["sausis", "vasaris", "kovas", "balandis", "gegužė", "birželis", "liepa", "rugpjūtis", "rugsėjis", "spalis", "lapkritis", "gruodis"];
const MONTHS_GENITIVE_LT: [&str; 12] =
    ["sausio", "vasario", "kovo", "balandžio", "gegužės", "birželio", "liepos", "rugpjūčio", "rugsėjo", "spalio", "lapkričio", "gruodžio"];
const MONTHS_ABBR_LT: [&str; 12] = ["saus.", "vas.", "kov.", "bal.", "geg.", "birž.", "liep.", "rugp.", "rugs.", "spal.", "lapkr.", "gruod."];
const MONTHS_ABBR_ES: [&str; 12] = ["ene.", "feb.", "mar.", "abr.", "may.", "jun.", "jul.", "ago.", "sept.", "oct.", "nov.", "dic."];

/// A month on its own: `October 2026`, `Жовтень 2026`, `Octubre de 2026`, `2026ko urria`.
fn month_title(d: Date) -> String {
    if i18n::current() == Language::Eu {
        return format!("{} {}", year_eu(d.year()), MONTHS_EU.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or(""));
    }
    if i18n::current() == Language::Lt {
        return format!("{} m. {}", d.year(), MONTHS_LT.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or(""));
    }
    let (names, of) = match i18n::current() {
        Language::Uk => (MONTHS_UK, ""),
        Language::Es => (MONTHS_ES, "de "),
        Language::Ca => (MONTHS_CA, "de "),
        Language::Gl => (MONTHS_GL, "de "),
        Language::It => (MONTHS_IT, ""),
        Language::Fr => (MONTHS_FR, ""),
        Language::De => (MONTHS_DE, ""),
        Language::Pl => (MONTHS_PL, ""),
        Language::Ro => (MONTHS_RO, ""),
        Language::Cs => (MONTHS_CS, ""),
        Language::Eu | Language::Lt | Language::En => (MONTHS, ""),
    };
    format!("{} {of}{}", names.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or(""), d.year())
}

/// The month and year after a day number: `22 October 2026`, `22 жовтня 2026`, `22 de octubre de 2026`.
fn month_year(d: Date) -> String {
    let of = if matches!(i18n::current(), Language::Es | Language::Ca | Language::Gl) { "de " } else { "" };
    format!("{} {of}{}", month_name(d), d.year())
}

/// A day in full: `Monday, 26 October 2026`, `lunes, 26 de octubre de 2026`,
/// `2026ko urriaren 26a, astelehena`.
fn long_date(d: Date) -> String {
    if i18n::current() == Language::Eu {
        let days = ["astelehena", "asteartea", "asteazkena", "osteguna", "ostirala", "larunbata", "igandea"];
        let month = MONTHS_GENITIVE_EU.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or("");
        return format!("{} {month} {}a, {}", year_eu(d.year()), d.day(), days.get(usize::from(d.weekday().index())).copied().unwrap_or(""));
    }
    if i18n::current() == Language::Lt {
        let days = ["pirmadienis", "antradienis", "trečiadienis", "ketvirtadienis", "penktadienis", "šeštadienis", "sekmadienis"];
        let month = MONTHS_GENITIVE_LT.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or("");
        return format!("{} m. {month} {} d., {}", d.year(), d.day(), days.get(usize::from(d.weekday().index())).copied().unwrap_or(""));
    }
    let days = match i18n::current() {
        Language::Uk => ["понеділок", "вівторок", "середа", "четвер", "пʼятниця", "субота", "неділя"],
        Language::Es => ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"],
        Language::Ca => ["dilluns", "dimarts", "dimecres", "dijous", "divendres", "dissabte", "diumenge"],
        Language::Gl => ["luns", "martes", "mércores", "xoves", "venres", "sábado", "domingo"],
        Language::It => ["lunedì", "martedì", "mercoledì", "giovedì", "venerdì", "sabato", "domenica"],
        Language::Fr => ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"],
        Language::De => ["Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag", "Sonntag"],
        Language::Pl => ["poniedziałek", "wtorek", "środa", "czwartek", "piątek", "sobota", "niedziela"],
        Language::Ro => ["luni", "marți", "miercuri", "joi", "vineri", "sâmbătă", "duminică"],
        Language::Cs => ["pondělí", "úterý", "středa", "čtvrtek", "pátek", "sobota", "neděle"],
        Language::Eu | Language::Lt | Language::En => ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"],
    };
    // French and Czech write the weekday without a comma: `lundi 26 octobre 2026`, `pondělí 26. října 2026`.
    let comma = if matches!(i18n::current(), Language::Fr | Language::Cs) { "" } else { "," };
    format!("{}{comma} {} {}", days.get(usize::from(d.weekday().index())).copied().unwrap_or(""), day_num(d), month_year(d))
}

/// Draw a formatted message ([`Rich`]): our fonts and colours, nothing from the sender's styles.
/// Links show their real address on hover; a link whose text names another site is marked ⚠.
fn rich_body(ui: &mut Ui, t: &Tokens, rich: &Rich) {
    ui.spacing_mut().item_spacing.y = 8.0;
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
            BlockKind::Bullet { depth } => (15.0, Some("•".to_string()), f32::from(depth) * 18.0),
            BlockKind::Numbered { depth, number } => (15.0, Some(format!("{number}.")), f32::from(depth) * 18.0),
            _ => (15.0, None, 0.0),
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
        ui.label(RichText::new("⚠ ").size(size).color(t.flag)).on_hover_text(tl("This link does not go where its text says"));
    }
    let shown = egui::Link::new(text.color(t.accent).underline());
    let hover = if span.deceptive { format!("Careful: the text names one site, but the link goes to\n{url}") } else { url.clone() };
    if ui.add(shown).on_hover_text(hover).clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// A rule in words: "Every week on Mon, Thu until 2026-12-31".
fn describe_rule(r: &ibercorreo_core::recur::Rule) -> String {
    match i18n::current() {
        Language::Uk => return describe_rule_uk(r),
        Language::Es => return describe_rule_es(r),
        Language::Ca => return describe_rule_ca(r),
        Language::Gl => return describe_rule_gl(r),
        Language::Eu => return describe_rule_eu(r),
        Language::It => return describe_rule_with(r, &RULE_IT),
        Language::Fr => return describe_rule_with(r, &RULE_FR),
        Language::De => return describe_rule_with(r, &RULE_DE),
        Language::Pl => return describe_rule_with(r, &RULE_PL),
        Language::Ro => return describe_rule_with(r, &RULE_RO),
        Language::Cs => return describe_rule_with(r, &RULE_CS),
        Language::Lt => return describe_rule_with(r, &RULE_LT),
        Language::En => {}
    }
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

/// The words a repeat rule is described with, for the languages whose description is
/// "frequency: days, count, until" with fixed words.
struct RuleWords {
    /// Daily, weekly, monthly, yearly.
    every: [&'static str; 4],
    /// The same with an interval; `{n}` is the interval.
    every_n: [&'static str; 4],
    /// `{name}` is a weekday.
    last_weekday: &'static str,
    /// `{n}` is the position, `{name}` the weekday.
    nth_weekday: &'static str,
    last_day: &'static str,
    /// `{d}` is the day of the month.
    month_day: &'static str,
    /// `{c}` is the count.
    times: &'static str,
    /// `{d}` is the last date.
    until: &'static str,
}

const RULE_IT: RuleWords = RuleWords {
    every: ["Ogni giorno", "Ogni settimana", "Ogni mese", "Ogni anno"],
    every_n: ["Ogni {n} giorni", "Ogni {n} settimane", "Ogni {n} mesi", "Ogni {n} anni"],
    last_weekday: "l'ultimo {name}",
    nth_weekday: "il {n}° {name}",
    last_day: "l'ultimo giorno",
    month_day: "il giorno {d}",
    times: ", {c} volte",
    until: " fino al {d}",
};

const RULE_FR: RuleWords = RuleWords {
    every: ["Tous les jours", "Toutes les semaines", "Tous les mois", "Tous les ans"],
    every_n: ["Tous les {n} jours", "Toutes les {n} semaines", "Tous les {n} mois", "Tous les {n} ans"],
    last_weekday: "le dernier {name}",
    nth_weekday: "le {n}e {name}",
    last_day: "le dernier jour",
    month_day: "le {d}",
    times: ", {c} fois",
    until: " jusqu'au {d}",
};

const RULE_DE: RuleWords = RuleWords {
    every: ["Täglich", "Wöchentlich", "Monatlich", "Jährlich"],
    every_n: ["Alle {n} Tage", "Alle {n} Wochen", "Alle {n} Monate", "Alle {n} Jahre"],
    last_weekday: "letzter {name}",
    nth_weekday: "{n}. {name}",
    last_day: "letzter Tag",
    month_day: "am {d}.",
    times: ", {c}-mal",
    until: " bis {d}",
};

// Polish counts after a number change form (2 tygodnie, 5 tygodni); the abbreviated units do not.
const RULE_PL: RuleWords = RuleWords {
    every: ["Codziennie", "Co tydzień", "Co miesiąc", "Co roku"],
    every_n: ["Co {n} dni", "Co {n} tyg.", "Co {n} mies.", "Co {n} l."],
    last_weekday: "ostatni {name}",
    nth_weekday: "{n}. {name}",
    last_day: "ostatni dzień",
    month_day: "{d}. dzień",
    times: ", powtórzeń: {c}",
    until: " do {d}",
};

const RULE_RO: RuleWords = RuleWords {
    every: ["Zilnic", "Săptămânal", "Lunar", "Anual"],
    every_n: ["La fiecare {n} zile", "La fiecare {n} săptămâni", "La fiecare {n} luni", "La fiecare {n} ani"],
    last_weekday: "ultima {name}",
    nth_weekday: "a {n}-a {name}",
    last_day: "ultima zi",
    month_day: "ziua {d}",
    times: ", de {c} ori",
    until: " până la {d}",
};

const RULE_CS: RuleWords = RuleWords {
    every: ["Denně", "Týdně", "Měsíčně", "Ročně"],
    every_n: ["Každý {n}. den", "Každý {n}. týden", "Každý {n}. měsíc", "Každý {n}. rok"],
    last_weekday: "poslední {name}",
    nth_weekday: "{n}. {name}",
    last_day: "poslední den",
    month_day: "{d}. den",
    times: ", počet opakování: {c}",
    until: " do {d}",
};

const RULE_LT: RuleWords = RuleWords {
    every: ["Kasdien", "Kas savaitę", "Kas mėnesį", "Kasmet"],
    every_n: ["Kas {n} d.", "Kas {n} sav.", "Kas {n} mėn.", "Kas {n} m."],
    last_weekday: "paskutinis {name}",
    nth_weekday: "{n}. {name}",
    last_day: "paskutinė diena",
    month_day: "{d} d.",
    times: ", kartų: {c}",
    until: " iki {d}",
};

/// [`describe_rule`] from a [`RuleWords`] table: "Ogni 2 settimane: lun, gio, 5 volte".
fn describe_rule_with(r: &ibercorreo_core::recur::Rule, w: &RuleWords) -> String {
    use ibercorreo_core::recur::Freq;
    let index = match r.freq {
        Freq::Daily => 0,
        Freq::Weekly => 1,
        Freq::Monthly => 2,
        Freq::Yearly => 3,
    };
    let pick = |words: &[&'static str; 4]| words.get(index).copied().unwrap_or("");
    let mut out = if r.interval == 1 { pick(&w.every).to_string() } else { pick(&w.every_n).replace("{n}", &r.interval.to_string()) };
    let mut on: Vec<String> = r
        .by_day
        .iter()
        .map(|(n, day)| {
            let name = WEEKDAYS.get(usize::from(day.index())).copied().map_or("?", tl);
            match n {
                Some(-1) => w.last_weekday.replace("{name}", name),
                Some(n) => w.nth_weekday.replace("{n}", &n.to_string()).replace("{name}", name),
                None => name.to_string(),
            }
        })
        .collect();
    on.extend(r.by_month_day.iter().map(|d| if *d == -1 { w.last_day.to_string() } else { w.month_day.replace("{d}", &d.to_string()) }));
    if !on.is_empty() {
        out.push_str(&format!(": {}", on.join(", ")));
    }
    if let Some(c) = r.count {
        out.push_str(&w.times.replace("{c}", &c.to_string()));
    }
    if let Some((d, _)) = r.until {
        out.push_str(&w.until.replace("{d}", &d.to_string()));
    }
    out
}

/// [`describe_rule`] in Ukrainian: "Щотижня: Пн, Чт до 2026-12-31", "Кожні 2 міс.: останній день".
fn describe_rule_uk(r: &ibercorreo_core::recur::Rule) -> String {
    use ibercorreo_core::recur::Freq;
    let mut out = match (r.freq, r.interval) {
        (Freq::Daily, 1) => "Щодня".to_string(),
        (Freq::Weekly, 1) => "Щотижня".to_string(),
        (Freq::Monthly, 1) => "Щомісяця".to_string(),
        (Freq::Yearly, 1) => "Щороку".to_string(),
        (freq, n) => {
            let unit = match freq {
                Freq::Daily => "дн.",
                Freq::Weekly => "тиж.",
                Freq::Monthly => "міс.",
                Freq::Yearly => "р.",
            };
            format!("Кожні {n} {unit}")
        }
    };
    let mut on: Vec<String> = r
        .by_day
        .iter()
        .map(|(n, w)| {
            let name = WEEKDAYS.get(usize::from(w.index())).copied().map_or("?", tl);
            match n {
                Some(-1) => format!("останній {name}"),
                Some(n) => format!("{n}-й {name}"),
                None => name.to_string(),
            }
        })
        .collect();
    on.extend(r.by_month_day.iter().map(|d| if *d == -1 { "останній день".to_string() } else { format!("{d}-го числа") }));
    if !on.is_empty() {
        out.push_str(&format!(": {}", on.join(", ")));
    }
    if let Some(c) = r.count {
        out.push_str(&format!(", повторень: {c}"));
    }
    if let Some((d, _)) = r.until {
        out.push_str(&format!(" до {d}"));
    }
    out
}

/// [`describe_rule`] in Spanish: "Cada semana: lun., jue. hasta 2026-12-31", "Cada 2 meses: el último día".
fn describe_rule_es(r: &ibercorreo_core::recur::Rule) -> String {
    use ibercorreo_core::recur::Freq;
    let mut out = match (r.freq, r.interval) {
        (Freq::Daily, 1) => "Cada día".to_string(),
        (Freq::Weekly, 1) => "Cada semana".to_string(),
        (Freq::Monthly, 1) => "Cada mes".to_string(),
        (Freq::Yearly, 1) => "Cada año".to_string(),
        (freq, n) => {
            let unit = match freq {
                Freq::Daily => "días",
                Freq::Weekly => "semanas",
                Freq::Monthly => "meses",
                Freq::Yearly => "años",
            };
            format!("Cada {n} {unit}")
        }
    };
    let mut on: Vec<String> = r
        .by_day
        .iter()
        .map(|(n, w)| {
            let name = WEEKDAYS.get(usize::from(w.index())).copied().map_or("?", tl);
            match n {
                Some(-1) => format!("el último {name}"),
                Some(n) => format!("el {n}.º {name}"),
                None => name.to_string(),
            }
        })
        .collect();
    on.extend(r.by_month_day.iter().map(|d| if *d == -1 { "el último día".to_string() } else { format!("el día {d}") }));
    if !on.is_empty() {
        out.push_str(&format!(": {}", on.join(", ")));
    }
    if let Some(c) = r.count {
        out.push_str(&format!(", {c} veces"));
    }
    if let Some((d, _)) = r.until {
        out.push_str(&format!(" hasta {d}"));
    }
    out
}

/// [`describe_rule`] in Catalan: "Cada setmana: dl., dj. fins a 2026-12-31", "Cada 2 mesos: l'últim dia".
fn describe_rule_ca(r: &ibercorreo_core::recur::Rule) -> String {
    use ibercorreo_core::recur::Freq;
    let mut out = match (r.freq, r.interval) {
        (Freq::Daily, 1) => "Cada dia".to_string(),
        (Freq::Weekly, 1) => "Cada setmana".to_string(),
        (Freq::Monthly, 1) => "Cada mes".to_string(),
        (Freq::Yearly, 1) => "Cada any".to_string(),
        (freq, n) => {
            let unit = match freq {
                Freq::Daily => "dies",
                Freq::Weekly => "setmanes",
                Freq::Monthly => "mesos",
                Freq::Yearly => "anys",
            };
            format!("Cada {n} {unit}")
        }
    };
    let mut on: Vec<String> = r
        .by_day
        .iter()
        .map(|(n, w)| {
            let name = WEEKDAYS.get(usize::from(w.index())).copied().map_or("?", tl);
            match n {
                Some(-1) => format!("l'últim {name}"),
                Some(n) => {
                    let suffix = match n {
                        1 | 3 => "r",
                        2 => "n",
                        4 => "t",
                        _ => "è",
                    };
                    format!("el {n}{suffix} {name}")
                }
                None => name.to_string(),
            }
        })
        .collect();
    on.extend(r.by_month_day.iter().map(|d| if *d == -1 { "l'últim dia".to_string() } else { format!("el dia {d}") }));
    if !on.is_empty() {
        out.push_str(&format!(": {}", on.join(", ")));
    }
    if let Some(c) = r.count {
        out.push_str(&format!(", {c} vegades"));
    }
    if let Some((d, _)) = r.until {
        out.push_str(&format!(" fins a {d}"));
    }
    out
}

/// [`describe_rule`] in Galician: "Cada semana: lun., xov. ata 2026-12-31", "Cada 2 meses: o último día".
fn describe_rule_gl(r: &ibercorreo_core::recur::Rule) -> String {
    use ibercorreo_core::recur::Freq;
    let mut out = match (r.freq, r.interval) {
        (Freq::Daily, 1) => "Cada día".to_string(),
        (Freq::Weekly, 1) => "Cada semana".to_string(),
        (Freq::Monthly, 1) => "Cada mes".to_string(),
        (Freq::Yearly, 1) => "Cada ano".to_string(),
        (freq, n) => {
            let unit = match freq {
                Freq::Daily => "días",
                Freq::Weekly => "semanas",
                Freq::Monthly => "meses",
                Freq::Yearly => "anos",
            };
            format!("Cada {n} {unit}")
        }
    };
    let mut on: Vec<String> = r
        .by_day
        .iter()
        .map(|(n, w)| {
            let name = WEEKDAYS.get(usize::from(w.index())).copied().map_or("?", tl);
            match n {
                Some(-1) => format!("o último {name}"),
                Some(n) => format!("o {n}.º {name}"),
                None => name.to_string(),
            }
        })
        .collect();
    on.extend(r.by_month_day.iter().map(|d| if *d == -1 { "o último día".to_string() } else { format!("o día {d}") }));
    if !on.is_empty() {
        out.push_str(&format!(": {}", on.join(", ")));
    }
    if let Some(c) = r.count {
        out.push_str(&format!(", {c} veces"));
    }
    if let Some((d, _)) = r.until {
        out.push_str(&format!(" ata {d}"));
    }
    out
}

/// [`describe_rule`] in Basque: "Astero: al., og., 2026-12-31 arte", "2 hiletik behin: azken eguna".
fn describe_rule_eu(r: &ibercorreo_core::recur::Rule) -> String {
    use ibercorreo_core::recur::Freq;
    let mut out = match (r.freq, r.interval) {
        (Freq::Daily, 1) => "Egunero".to_string(),
        (Freq::Weekly, 1) => "Astero".to_string(),
        (Freq::Monthly, 1) => "Hilero".to_string(),
        (Freq::Yearly, 1) => "Urtero".to_string(),
        (freq, n) => {
            let unit = match freq {
                Freq::Daily => "egunetik",
                Freq::Weekly => "astetik",
                Freq::Monthly => "hiletik",
                Freq::Yearly => "urtetik",
            };
            format!("{n} {unit} behin")
        }
    };
    let mut on: Vec<String> = r
        .by_day
        .iter()
        .map(|(n, w)| {
            let name = WEEKDAYS.get(usize::from(w.index())).copied().map_or("?", tl);
            match n {
                Some(-1) => format!("azken {name}"),
                Some(n) => format!("{n}. {name}"),
                None => name.to_string(),
            }
        })
        .collect();
    on.extend(r.by_month_day.iter().map(|d| if *d == -1 { "azken eguna".to_string() } else { format!("{d}. eguna") }));
    if !on.is_empty() {
        out.push_str(&format!(": {}", on.join(", ")));
    }
    if let Some(c) = r.count {
        out.push_str(&format!(", {c} aldiz"));
    }
    if let Some((d, _)) = r.until {
        out.push_str(&format!(" {d} arte"));
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

/// A length of time in its largest whole unit: `1 day`, `2 hours`, `25 minutes` (`25 хв`).
fn span(minutes: i64) -> String {
    let m = minutes.unsigned_abs();
    let (n, one, many, short) = if m >= 1440 {
        (m / 1440, "1 day", "{n} days", "{n} дн.")
    } else if m >= 60 {
        (m / 60, "1 hour", "{n} hours", "{n} год")
    } else {
        (m, "1 minute", "{n} minutes", "{n} хв")
    };
    // Ukrainian counts take three plural forms; the abbreviated units have none. Other languages
    // translate the singular and plural through the catalog.
    let template = if i18n::current() == Language::Uk {
        short
    } else if n == 1 {
        tl(one)
    } else {
        tl(many)
    };
    template.replace("{n}", &n.to_string())
}

/// `in 10 minutes`, `now`, `25 minutes overdue`, `in 2 hours`, `1 day overdue`.
fn due_in(minutes: i64) -> String {
    match minutes {
        0 => tl("now").into(),
        m if m > 0 => tf("in {span}", &[("span", &span(m))]),
        m => tf("{span} overdue", &[("span", &span(m))]),
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
        0..1024 => format!("{bytes} {}", tl("B")),
        1024..1_048_576 => format!("{} {}", bytes / 1024, tl("KB")),
        _ => format!("{:.1} {}", bytes as f64 / 1_048_576.0, tl("MB")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gnome(mail: bool) -> GnomeAccount {
        GnomeAccount {
            id: if mail { "account_1700000000_2" } else { "account_1700000000_0" }.into(),
            provider: if mail { "Google" } else { "Nextcloud" }.into(),
            identity: "ana@example.org".into(),
            mail: mail.then(|| GnomeMail {
                email: "ana@gmail.com".into(),
                name: "Ana".into(),
                user: String::new(),
                imap_host: "imap.gmail.com".into(),
                imap_port: 993,
                imap_starttls: false,
                smtp_host: "smtp.gmail.com".into(),
                smtp_port: 465,
                smtp_starttls: false,
            }),
            dav: Some(
                if mail { "https://apidata.googleusercontent.com/caldav/v2/ana@gmail.com/user" } else { "https://cloud.example.org/remote.php/dav" }
                    .into(),
            ),
            attention: false,
        }
    }

    #[test]
    fn an_account_from_gnome_settings_fills_the_form_and_saves_without_a_password() {
        i18n::set_current(Language::En);
        let mut app = App::new(Session::new(ibercorreo_store::Store::new(), Date::new(2026, 11, 3).unwrap()));
        app.open_account_settings(None, "");
        app.account_form.password = "typed earlier".into();
        app.account_form.oauth = "google".into();
        app.account_form.take_gnome(&gnome(true), true);
        let f = &app.account_form;
        assert_eq!(
            (f.email.as_str(), f.name.as_str(), f.imap_host.as_str(), f.smtp_port.as_str()),
            ("ana@gmail.com", "Ana", "imap.gmail.com", "465")
        );
        assert_eq!(
            (f.goa.as_str(), f.oauth.as_str(), f.password.as_str()),
            ("account_1700000000_2", "", ""),
            "GNOME signs in, nothing typed is kept"
        );
        assert!(f.caldav.starts_with("https://apidata.googleusercontent.com/"));
        assert!(app.save_account());
        let a = app.session.store.accounts.first().unwrap().clone();
        assert_eq!((a.goa.as_str(), a.caldav.as_str(), a.dav_goa.as_str()), ("account_1700000000_2", f_caldav(), ""));
        assert!(app.take_password().is_none(), "no password for the credential store");

        // Nextcloud has no mail: its calendars go with the account open, signed in to as Nextcloud.
        let id = a.id;
        app.open_account_settings(Some(id), "");
        app.account_form.caldav.clear();
        app.account_form.take_gnome(&gnome(false), true);
        assert_eq!(app.account_form.email, "ana@gmail.com", "the mail is untouched");
        assert_eq!(
            (app.account_form.caldav.as_str(), app.account_form.dav_goa.as_str()),
            ("https://cloud.example.org/remote.php/dav", "account_1700000000_0")
        );
        assert!(app.save_account());
        assert_eq!(app.session.store.account(id).unwrap().dav_goa, "account_1700000000_0");

        // Another account already syncs calendars: nothing of them is taken.
        app.open_account_settings(None, "");
        app.account_form.take_gnome(&gnome(false), false);
        assert!(app.account_form.caldav.is_empty() && app.account_form.dav_goa.is_empty());
        assert!(app.account_form.note.contains("another account"), "{}", app.account_form.note);
    }

    fn f_caldav() -> &'static str {
        "https://apidata.googleusercontent.com/caldav/v2/ana@gmail.com/user"
    }

    /// Dates, durations and repeat rules read naturally in each language: Spanish and Catalan put "de" before
    /// the month and the year, and counts take a plural.
    #[test]
    fn dates_spans_and_rules_follow_the_interface_language() {
        let d = Date::new(2026, 10, 26).unwrap();
        let rule = ibercorreo_core::recur::Rule::parse("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TH;COUNT=5").unwrap();
        i18n::set_current(Language::Es);
        assert_eq!(long_date(d), "lunes, 26 de octubre de 2026");
        assert_eq!(month_title(d), "Octubre de 2026");
        assert_eq!(month_abbr(10), "oct.");
        assert_eq!(due_in(120), "dentro de 2 horas");
        assert_eq!(due_in(-1), "vencido hace 1 minuto");
        assert_eq!(describe_rule(&rule), "Cada 2 semanas: lun., jue., 5 veces");
        i18n::set_current(Language::Ca);
        assert_eq!(long_date(d), "dilluns, 26 d'octubre de 2026");
        assert_eq!(due_in(-1440), "vençut fa 1 dia");
        assert_eq!(describe_rule(&rule), "Cada 2 setmanes: dl., dj., 5 vegades");
        i18n::set_current(Language::Gl);
        assert_eq!(long_date(d), "luns, 26 de outubro de 2026");
        assert_eq!(describe_rule(&rule), "Cada 2 semanas: lun., xov., 5 veces");
        i18n::set_current(Language::Eu);
        assert_eq!(long_date(d), "2026ko urriaren 26a, astelehena");
        assert_eq!(month_title(d), "2026ko urria");
        assert_eq!(range_title_eu(CalendarMode::Week, d, d, Date::new(2026, 11, 1).unwrap()), "2026ko urriaren 26 – azaroaren 1");
        assert_eq!(year_eu(2025), "2025eko");
        assert_eq!(year_eu(2030), "2030eko");
        assert_eq!(year_eu(2040), "2040ko");
        assert_eq!(due_in(120), "2 ordu barru");
        assert_eq!(describe_rule(&rule), "2 astetik behin: al., og., 5 aldiz");
        i18n::set_current(Language::It);
        assert_eq!(long_date(d), "lunedì, 26 ottobre 2026");
        assert_eq!(month_title(d), "Ottobre 2026");
        assert_eq!(due_in(120), "tra 2 ore");
        assert_eq!(describe_rule(&rule), "Ogni 2 settimane: lun, gio, 5 volte");
        i18n::set_current(Language::Fr);
        assert_eq!(long_date(d), "lundi 26 octobre 2026");
        assert_eq!(describe_rule(&rule), "Toutes les 2 semaines: lun., jeu., 5 fois");
        i18n::set_current(Language::De);
        assert_eq!(long_date(d), "Montag, 26. Oktober 2026");
        assert_eq!(short_when(ibercorreo_core::DateTime::new(d, 9, 0, 0).unwrap(), Date::new(2026, 1, 1).unwrap()), "26. Okt.");
        assert_eq!(describe_rule(&rule), "Alle 2 Wochen: Mo, Do, 5-mal");
        i18n::set_current(Language::Pl);
        assert_eq!(long_date(d), "poniedziałek, 26 października 2026");
        assert_eq!(month_title(d), "Październik 2026");
        assert_eq!(due_in(120), "za 2 godz.");
        assert_eq!(describe_rule(&rule), "Co 2 tyg.: pon., czw., powtórzeń: 5");
        i18n::set_current(Language::Ro);
        assert_eq!(long_date(d), "luni, 26 octombrie 2026");
        assert_eq!(month_title(d), "Octombrie 2026");
        assert_eq!(due_in(120), "peste 2 ore");
        assert_eq!(describe_rule(&rule), "La fiecare 2 săptămâni: Lun., Joi, de 5 ori");
        i18n::set_current(Language::Cs);
        assert_eq!(long_date(d), "pondělí 26. října 2026");
        assert_eq!(month_title(d), "Říjen 2026");
        assert_eq!(due_in(120), "za 2 h");
        assert_eq!(describe_rule(&rule), "Každý 2. týden: Po, Čt, počet opakování: 5");
        i18n::set_current(Language::Lt);
        assert_eq!(long_date(d), "2026 m. spalio 26 d., pirmadienis");
        assert_eq!(month_title(d), "2026 m. spalis");
        assert_eq!(range_title_lt(CalendarMode::Week, d, d, Date::new(2026, 11, 1).unwrap()), "2026 m. spalio 26 d. – lapkričio 1 d.");
        assert_eq!(short_when(ibercorreo_core::DateTime::new(d, 9, 0, 0).unwrap(), Date::new(2026, 1, 1).unwrap()), "spal. 26 d.");
        assert_eq!(due_in(120), "po 2 val.");
        assert_eq!(describe_rule(&rule), "Kas 2 sav.: Pr, Kt, kartų: 5");
        i18n::set_current(Language::Uk);
        assert_eq!(long_date(d), "понеділок, 26 жовтня 2026");
        assert_eq!(due_in(120), "через 2 год");
        i18n::set_current(Language::En);
        assert_eq!(long_date(d), "Monday, 26 October 2026");
        assert_eq!(month_title(d), "October 2026");
        assert_eq!(due_in(120), "in 2 hours");
        assert_eq!(describe_rule(&rule), "Every 2 weeks on Mon, Thu, 5 times");
    }

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
    fn found_settings_pick_oauth_or_an_app_password() {
        let mut app = App::new(Session::new(ibercorreo_store::Store::new(), Date::EPOCH));
        app.open_account_settings(None, "");
        app.account_form.email = "ana@gmail.com".into();
        app.apply_autoconfig(Ok(gmail()));
        assert_eq!(app.account_form.oauth, "", "no client ID: a password it stays");
        assert!(app.account_form.note.contains("app password"), "{}", app.account_form.note);

        app.oauth_ready = vec!["google".into()];
        app.apply_autoconfig(Ok(gmail()));
        assert_eq!(app.account_form.oauth, "", "Gmail offers an app password first, even with a client ID");
        assert!(app.account_form.note.contains("app password"), "{}", app.account_form.note);

        let mut other = gmail();
        other.imap_host = "imap.example.org".into();
        app.apply_autoconfig(Ok(other));
        assert_eq!(app.account_form.oauth, "", "another provider's servers take a password");
    }

    #[test]
    fn graph_mail_is_a_microsoft_choice_kept_through_the_dialog() {
        let mut app = App::new(Session::new(ibercorreo_store::Store::new(), Date::EPOCH));
        let work = json!({"email": "ana@contoso.es", "imapHost": "outlook.office365.com", "smtpHost": "smtp.office365.com", "oauth": "microsoft", "graphMail": true});
        let id = AccountId(app.session.run("account.set", &work).unwrap()["id"].as_u64().unwrap() as u32);
        app.open_account_settings(Some(id), "");
        assert!(app.account_form.graph_mail, "the dialog shows it");
        app.save_account();
        assert!(app.session.store.account(id).unwrap().mail_over_graph(), "saving the dialog keeps it");
        app.open_account_settings(Some(id), "");
        app.account_form.graph_mail = false;
        app.save_account();
        assert!(!app.session.store.account(id).unwrap().mail_over_graph());
        // Only with the Microsoft sign-in: changed to a password, the account stops asking Graph.
        app.session.run("account.set", &work).unwrap();
        app.open_account_settings(Some(id), "");
        app.account_form.graph_mail = true;
        app.account_form.oauth.clear();
        app.save_account();
        assert!(!app.session.store.account(id).unwrap().mail_over_graph());
    }

    #[test]
    fn an_exchange_server_stands_in_for_imap_and_smtp_through_the_dialog() {
        let mut app = App::new(Session::new(ibercorreo_store::Store::new(), Date::EPOCH));
        app.open_account_settings(None, "");
        app.account_form.email = "ana@contoso.es".into();
        app.account_form.ews = "mail.contoso.es".into();
        // The IMAP and SMTP rows are hidden and left empty.
        app.save_account();
        let id = app.session.store.accounts.first().map(|a| a.id).unwrap();
        assert_eq!(app.session.store.account(id).unwrap().mail_over_ews(), Some("https://mail.contoso.es/EWS/Exchange.asmx"));
        app.open_account_settings(Some(id), "");
        assert_eq!(app.account_form.ews, "https://mail.contoso.es/EWS/Exchange.asmx", "the dialog shows it");
        app.save_account();
        assert!(app.session.store.account(id).unwrap().mail_over_ews().is_some(), "saving the dialog keeps it");
        // A browser sign-in is not for EWS: choosing one goes back to IMAP and SMTP.
        app.open_account_settings(Some(id), "");
        app.account_form.oauth = "microsoft".into();
        app.save_account();
        assert_eq!(app.session.store.account(id).unwrap().mail_over_ews(), None);
    }

    #[test]
    fn autodiscover_fills_the_exchange_server() {
        let mut app = App::new(Session::new(ibercorreo_store::Store::new(), Date::EPOCH));
        app.open_account_settings(None, "");
        app.account_form.email = "ana@contoso.es".into();
        app.account_form.oauth = "microsoft".into();
        app.apply_exchange(Ok("https://mail.contoso.es/EWS/Exchange.asmx".into()));
        assert_eq!(app.account_form.ews, "https://mail.contoso.es/EWS/Exchange.asmx");
        assert_eq!(app.account_form.oauth, "", "EWS signs in with the password");
        assert!(app.account_form.note.contains("found"), "{}", app.account_form.note);
        app.apply_exchange(Err("HTTP 404".into()));
        assert_eq!(app.account_form.ews, "https://mail.contoso.es/EWS/Exchange.asmx", "a failure keeps what is there");
        assert!(app.account_form.note.contains("HTTP 404"), "{}", app.account_form.note);
    }

    #[test]
    fn a_finished_browser_sign_in_closes_settings_and_syncs() {
        let mut app = App::new(Session::new(ibercorreo_store::Store::new(), Date::EPOCH));
        let gmail = json!({"email": "ana@gmail.com", "imapHost": "imap.gmail.com", "smtpHost": "smtp.gmail.com", "oauth": "google"});
        let id = app.session.run("account.set", &gmail).unwrap()["id"].as_u64().unwrap();
        app.open_account_settings(Some(AccountId(id as u32)), "Sign in");
        assert_eq!(app.account_form.oauth, "google", "the dialog shows how the account signs in");
        app.oauth_running = Some(String::new());
        app.apply_oauth(Err("cancelled".into()));
        assert!(app.account_form.open && app.oauth_running.is_none() && !app.session.sync_requested);
        app.apply_oauth(Ok("Google".into()));
        assert!(!app.account_form.open && app.session.sync_requested);
    }

    #[test]
    fn choosing_google_or_microsoft_fills_in_their_servers() {
        let mut f = AccountForm { email: "ana@example.org".into(), ..AccountForm::default() };
        f.oauth = "google".into();
        f.follow_provider("", "ana@example.org", &[]);
        assert!(f.note.contains("Go to Tinta Correo"), "how to pass Google's unverified-app page: {}", f.note);
        assert_eq!((f.imap_host.as_str(), f.imap_port.as_str(), f.imap_starttls), ("imap.gmail.com", "993", false));
        assert_eq!((f.smtp_host.as_str(), f.smtp_port.as_str(), f.smtp_starttls), ("smtp.gmail.com", "465", false));
        f.oauth = "microsoft".into();
        f.follow_provider("google", "ana@example.org", &[]);
        assert!(!f.note.contains("Go to Tinta Correo"), "{}", f.note);
        assert_eq!((f.imap_host.as_str(), f.smtp_host.as_str()), ("outlook.office365.com", "smtp.office365.com"));
        assert_eq!((f.smtp_port.as_str(), f.smtp_starttls), ("587", true));
        // Back to a password: the servers stay (an app password signs in to the same ones).
        f.oauth.clear();
        f.follow_provider("microsoft", "ana@example.org", &[]);
        assert_eq!(f.imap_host, "outlook.office365.com");
    }

    #[test]
    fn typing_a_gmail_or_outlook_address_fills_in_the_servers() {
        let ready = ["google".to_string(), "microsoft".to_string()];
        let mut f = AccountForm { email: "ana@gmail.com".into(), ..AccountForm::default() };
        f.follow_provider("", "ana@gmail.co", &ready);
        assert_eq!((f.imap_host.as_str(), f.oauth.as_str()), ("imap.gmail.com", ""), "Gmail: an app password first");
        assert!(f.note.contains("app password"), "{}", f.note);
        f.email = "ana@hotmail.com".into();
        f.follow_provider("", "ana@gmail.com", &ready);
        assert_eq!((f.imap_host.as_str(), f.oauth.as_str()), ("outlook.office365.com", "microsoft"), "another provider's servers give way");

        // Without a client ID it is a password (an app password), with the same servers.
        let mut f = AccountForm { email: "ana@outlook.com".into(), ..AccountForm::default() };
        f.follow_provider("", "", &[]);
        assert_eq!((f.smtp_host.as_str(), f.oauth.as_str()), ("smtp.office365.com", ""));

        // Servers typed by hand stay.
        let mut f = AccountForm { email: "ana@gmail.com".into(), imap_host: "mail.example.org".into(), ..AccountForm::default() };
        f.follow_provider("", "", &ready);
        assert_eq!((f.imap_host.as_str(), f.oauth.as_str()), ("mail.example.org", ""));
    }

    #[test]
    fn saving_a_new_account_adds_it_beside_the_first() {
        let mut app = App::new(Session::new(ibercorreo_store::Store::new(), Date::EPOCH));
        let fill = |app: &mut App, email: &str, host: &str| {
            app.account_form.email = email.into();
            app.account_form.imap_host = format!("imap.{host}");
            app.account_form.smtp_host = format!("smtp.{host}");
            app.account_form.password = "secret".into();
        };
        app.open_account_settings(None, "");
        fill(&mut app, "ana@example.org", "example.org");
        assert!(app.save_account());
        let first = app.take_password().unwrap().0;

        app.open_account_settings(None, "");
        assert!(app.account_form.email.is_empty(), "a new account starts blank");
        fill(&mut app, "ana@example.net", "example.net");
        assert!(app.save_account());
        let (second, password) = app.take_password().unwrap();
        assert_ne!(first, second);
        assert_eq!(password, "secret");
        assert_eq!(app.session.store.accounts.len(), 2);
        assert_eq!(app.session.current_account(), second, "the new account's Inbox opens");

        // Its settings change it, not the first.
        app.open_account_settings(Some(second), "");
        app.account_form.name = "Ana".into();
        assert!(app.save_account());
        assert_eq!(app.session.store.account(second).unwrap().address.name.as_deref(), Some("Ana"));
        assert_eq!(app.session.store.account(first).unwrap().address.email, "ana@example.org");

        // An OAuth account asks the shell to sign in to that account.
        app.open_account_settings(None, "");
        app.account_form.email = "ana@gmail.com".into();
        app.oauth_ready = vec!["google".into()];
        app.account_form.oauth = "google".into();
        app.account_form.follow_provider("", "ana@gmail.com", &app.oauth_ready.clone());
        assert!(!app.save_account(), "stays open for the browser");
        let third = app.oauth_request.unwrap();
        assert!(third != first && third != second);
    }

    /// Every account's heading is in sight when the app opens, and adding one that is already
    /// set up shows it instead.
    #[test]
    fn several_accounts_open_folded_and_a_second_add_shows_the_first() {
        let mut session = Session::new(ibercorreo_store::Store::new(), Date::EPOCH);
        let mut add = |email: &str, host: &str| {
            let p = json!({ "email": email, "imapHost": format!("imap.{host}"), "smtpHost": format!("smtp.{host}") });
            let id = session.run("account.add", &p).unwrap()["id"].as_u64().unwrap();
            AccountId(u32::try_from(id).unwrap())
        };
        let outlook = add("ana@outlook.com", "outlook.com");
        let gmail = add("ana@gmail.com", "gmail.com");
        for i in 0..80 {
            session.store.add_folder(outlook, &format!("Folder {i}"), FolderRole::Other);
        }
        session.run("folder.select", &json!({ "folder": session.store.folder_by_role(outlook, FolderRole::Inbox).unwrap().0 })).unwrap();
        let mut app = App::new(session);
        assert!(!app.folded.contains(&outlook.0), "the account in use stays open");
        assert!(app.folded.contains(&gmail.0), "the other one is folded to its heading");

        app.open_account_settings(None, "");
        app.account_form.email = " Ana@Gmail.com ".into();
        app.account_form.imap_host = "imap.gmail.com".into();
        app.account_form.smtp_host = "smtp.gmail.com".into();
        app.account_form.password = "secret".into();
        assert!(app.save_account(), "the dialog closes");
        assert_eq!(app.session.store.accounts.len(), 2, "no second copy");
        assert_eq!(app.session.current_account(), gmail, "its Inbox opens");
        assert!(!app.folded.contains(&gmail.0));
        assert!(app.session.status.contains("already set up"), "{}", app.session.status);
        assert!(app.take_password().is_none(), "nothing to store");
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

    /// A big folder (tens of thousands of messages in a real Inbox) lays out only the rows in
    /// view: painting every row took seconds a frame and froze the window.
    #[test]
    fn a_big_folder_paints_only_the_rows_in_view() {
        let mut s = Session::new(ibercorreo_store::Store::new(), Date::EPOCH);
        s.run("account.add", &json!({ "email": "ana@example.org", "imapHost": "imap.example.org", "smtpHost": "smtp.example.org" })).unwrap();
        let inbox = s.store.folder_by_role(AccountId(0), FolderRole::Inbox).unwrap();
        for i in 0..3000u32 {
            let mut m = ibercorreo_core::Message::new(MessageId(0), inbox);
            m.set_subject(&format!("Row subject {i}"));
            m.seen = i % 3 == 0;
            s.store.add_message(m).unwrap();
        }
        s.run("folder.select", &json!({ "folder": inbox.0 })).unwrap();
        let mut app = App::new(s);
        let ctx = egui::Context::default();
        let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1360.0, 860.0))), ..Default::default() };
        // The second frame, once fonts and sizes are known.
        ctx.run_ui(input(), |ui| app.ui(ui)).textures_delta.clear();
        let mut out = ctx.run_ui(input(), |ui| app.ui(ui));
        out.textures_delta.clear();
        let rows = out.shapes.iter().filter(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.job.text.starts_with("Row subject"))).count();
        assert!((1..=30).contains(&rows), "{rows} rows painted for a window 860 px high");
        // The unread count of the folder pane and status bar is right, counted once a frame.
        assert_eq!(app.unread.get(&inbox), Some(&2000));
    }

    /// Mail reopened from disk keeps its text there: the list shows each preview without reading
    /// it, and the reading pane reads the open message's text (or says it could not).
    #[test]
    fn the_reading_pane_reads_message_text_kept_on_disk() {
        let dir = std::env::temp_dir().join(format!("ibercorreo-ui-content-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        let mut store = ibercorreo_store::Store::new();
        let inbox = store.folder_by_role(AccountId(0), FolderRole::Inbox).unwrap();
        let mut m = ibercorreo_core::Message::new(MessageId(0), inbox);
        m.set_subject("Kept on disk");
        m.content_mut().unwrap().body = "First line of the text\nSecond line, only in the pane".into();
        let id = store.add_message(m).unwrap();
        let other = store.add_message(ibercorreo_core::Message::new(MessageId(0), inbox)).unwrap();
        store.save(&path).unwrap();
        let mut s = Session::new(ibercorreo_store::Store::load(&path).unwrap(), Date::EPOCH);
        s.run("folder.select", &json!({ "folder": inbox.0 })).unwrap();
        let mut app = App::new(s);
        let ctx = egui::Context::default();
        let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1360.0, 860.0))), ..Default::default() };
        let texts = |app: &mut App| {
            ctx.run_ui(input(), |ui| app.ui(ui)).textures_delta.clear();
            let mut out = ctx.run_ui(input(), |ui| app.ui(ui));
            out.textures_delta.clear();
            out.shapes
                .iter()
                .filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.job.text.clone()) } else { None })
                .collect::<Vec<_>>()
        };
        let shown = texts(&mut app);
        assert!(shown.iter().any(|t| t.contains("First line of the text")), "the preview: {shown:?}");
        assert!(app.session.store.message(id).unwrap().content().is_none(), "the list read nothing");
        app.session.message = Some(id);
        let shown = texts(&mut app);
        assert!(shown.iter().any(|t| t.contains("Second line, only in the pane")), "{shown:?}");
        // Another message's text gone from disk: the pane says so instead of showing nothing.
        std::fs::write(ibercorreo_store::shards::dir(&path).join("c000000.json"), b"{}").unwrap();
        app.session.message = Some(other);
        let shown = texts(&mut app);
        assert!(shown.iter().any(|t| t.contains("could not be read")), "{shown:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Subfolders sit under their parent ("INBOX/Work" under the Inbox), a path no folder
    /// holds still gets a row, and standard folders are never nested.
    #[test]
    fn the_folder_pane_is_a_tree() {
        let mut store = ibercorreo_store::Store::new();
        let a = store.first_account();
        for name in ["Clients/Acme", "Clients", "INBOX/Work", "[Gmail]/Important", "Clients/Acme/2026", "Notes"] {
            store.add_folder(a, name, FolderRole::Other);
        }
        let rows = folder_tree(&store.folders_of(a));
        let shown: Vec<(usize, &str, bool, bool)> = rows.iter().map(|r| (r.depth, r.label.as_str(), r.folder.is_some(), r.children)).collect();
        let at = |label: &str| shown.iter().position(|(_, l, _, _)| *l == label).unwrap();
        assert_eq!(shown.iter().filter(|(_, l, _, _)| *l == "Clients").count(), 1, "the folder fills the path's row: {shown:?}");
        assert_eq!(shown[at("Clients")], (0, "Clients", true, true));
        assert_eq!(shown[at("Acme")], (1, "Acme", true, true));
        assert_eq!(shown[at("2026")], (2, "2026", true, false));
        assert_eq!(at("Acme"), at("Clients") + 1);
        assert_eq!(at("2026"), at("Acme") + 1);
        assert_eq!(shown[at("Work")].0, 1);
        assert_eq!(at("Work"), at("Inbox") + 1, "under the Inbox: {shown:?}");
        assert_eq!(shown[at("Inbox")], (0, "Inbox", true, true));
        assert_eq!(shown[at("[Gmail]")], (0, "[Gmail]", false, true), "a path no folder holds");
        assert_eq!(shown[at("Important")].0, 1);
        assert_eq!(shown[at("Notes")], (0, "Notes", true, false));
        assert!(shown.iter().filter(|r| r.0 == 0).count() >= 8, "standard folders stay at the top level");
    }

    /// Folding a parent hides its subfolders, and their unread mail is counted on it.
    #[test]
    fn a_folded_folder_hides_its_subfolders() {
        let mut store = ibercorreo_store::Store::new();
        let a = store.first_account();
        store.add_folder(a, "Clients", FolderRole::Other);
        let acme = store.add_folder(a, "Clients/Acme", FolderRole::Other);
        for _ in 0..3 {
            store.add_message(ibercorreo_core::Message::new(MessageId(0), acme)).unwrap();
        }
        let mut app = App::new(Session::new(store, Date::EPOCH));
        let ctx = egui::Context::default();
        let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1360.0, 860.0))), ..Default::default() };
        let texts = |app: &mut App| {
            ctx.run_ui(input(), |ui| app.ui(ui)).textures_delta.clear();
            let mut out = ctx.run_ui(input(), |ui| app.ui(ui));
            out.textures_delta.clear();
            out.shapes
                .iter()
                .filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.job.text.clone()) } else { None })
                .collect::<Vec<_>>()
        };
        let shown = texts(&mut app);
        assert!(shown.iter().any(|t| t == "Acme"), "{shown:?}");
        app.folded_paths.insert((a.0, "clients".into()));
        let shown = texts(&mut app);
        assert!(shown.iter().any(|t| t == "Clients"), "{shown:?}");
        assert!(!shown.iter().any(|t| t == "Acme"), "hidden under its folded parent: {shown:?}");
        assert!(shown.iter().any(|t| t == "3"), "Acme's unread counted on Clients: {shown:?}");
    }

    /// The side panes give way, in Outlook's order, before the main pane gets narrower than it needs.
    #[test]
    fn side_panes_give_way_to_the_main_pane() {
        let side = |wanted, min, max| Some(Side { wanted, min, max });
        // Room for everything.
        let f = fit(1800.0, 440.0, side(260.0, 190.0, 400.0), side(360.0, 280.0, 680.0), side(280.0, 240.0, 440.0), (false, false));
        assert!(f.left && f.right);
        assert!((f.middle_max - 680.0).abs() < 0.01, "as wide as the list may be: {f:?}");
        // A 1360 px window at 130 %: My Day goes, the list is capped.
        let f = fit(986.0, 440.0, side(260.0, 190.0, 400.0), side(450.0, 280.0, 680.0), side(280.0, 240.0, 440.0), (false, false));
        assert!(f.left && !f.right, "{f:?}");
        assert!(260.0 + f.middle_max + 440.0 <= 986.0 + 0.01, "{f:?}");
        assert!(f.room_for_right > 986.0);
        // Narrower still: the folder pane folds away too.
        let f = fit(800.0, 440.0, side(260.0, 190.0, 400.0), side(360.0, 280.0, 680.0), side(280.0, 240.0, 440.0), (false, false));
        assert!(!f.left && !f.right, "{f:?}");
        assert!((f.middle_max - 360.0).abs() < 0.01);
        // Asked for: My Day shows, and the folder pane makes room for it.
        let f = fit(986.0, 440.0, side(260.0, 190.0, 400.0), side(300.0, 280.0, 680.0), side(280.0, 240.0, 440.0), (false, true));
        assert!(f.right && !f.left, "{f:?}");
    }

    /// At 1100 points, and in a 1360 px window at 130 %, every view's main pane keeps its room
    /// and a long subject wraps by words, not by letters.
    #[test]
    fn the_main_pane_is_never_squeezed() {
        for (width, zoom) in [(1100.0, None), (1360.0 / 1.3, Some(130))] {
            let mut store = ibercorreo_store::Store::new();
            store.zoom = zoom;
            let inbox = store.folder_by_role(AccountId(0), FolderRole::Inbox).unwrap();
            let mut m = ibercorreo_core::Message::new(MessageId(0), inbox);
            m.set_subject("Ha Sid Meier's Civilization V and one more product in your library");
            let id = store.add_message(m).unwrap();
            let mut s = Session::new(store, Date::EPOCH);
            s.run("folder.select", &json!({ "folder": inbox.0 })).unwrap();
            let mut app = App::new(s);
            app.session.message = Some(id);
            let ctx = egui::Context::default();
            let input =
                || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 800.0))), ..Default::default() };
            for (view, min) in [("view.mail", MAIL_MAIN_MIN), ("view.calendar", CALENDAR_MAIN_MIN), ("view.contacts", PEOPLE_MAIN_MIN)] {
                app.session.run(view, &json!({})).unwrap();
                let mut out = ctx.run_ui(input(), |ui| app.ui(ui));
                for _ in 0..3 {
                    out.textures_delta.clear();
                    out = ctx.run_ui(input(), |ui| app.ui(ui));
                }
                out.textures_delta.clear();
                assert!(app.main_width >= min - 30.0, "{view} at {width}: the main pane is {} wide", app.main_width);
                if view == "view.mail" {
                    let subject = out
                        .shapes
                        .iter()
                        .find_map(|c| match &c.shape {
                            egui::Shape::Text(t)
                                if t.galley.job.text.starts_with("Ha Sid Meier")
                                    && t.galley.job.sections.iter().any(|s| s.format.font_id.size > 20.0) =>
                            {
                                Some(t.galley.clone())
                            }
                            _ => None,
                        })
                        .expect("the subject in the reading pane");
                    assert!(subject.rows.len() <= 3, "wrapped into {} rows at {width}", subject.rows.len());
                }
            }
        }
    }

    /// Asked for a pane with no room, the window grows by what is missing within the screen,
    /// moving left if it must; maximized, it does not.
    #[test]
    fn the_window_grows_for_a_pane_asked_for() {
        let app = App::new(Session::new(ibercorreo_store::Store::new(), Date::EPOCH));
        let ctx = egui::Context::default();
        let run = |maximized: bool, extra: f32| {
            let mut raw = egui::RawInput::default();
            let info = raw.viewports.entry(egui::ViewportId::ROOT).or_default();
            info.inner_rect = Some(egui::Rect::from_min_size(egui::pos2(308.0, 40.0), vec2(1000.0, 700.0)));
            info.outer_rect = Some(egui::Rect::from_min_size(egui::pos2(300.0, 10.0), vec2(1016.0, 738.0)));
            info.monitor_size = Some(vec2(1600.0, 900.0));
            info.maximized = Some(maximized);
            let mut grown = false;
            let mut out = ctx.run_ui(raw, |ui| grown = app.grow_window(ui.ctx(), extra));
            out.textures_delta.clear();
            let commands = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default();
            (grown, commands)
        };
        let (grown, commands) = run(false, 300.0);
        assert!(grown);
        assert!(commands.contains(&egui::ViewportCommand::InnerSize(vec2(1300.0, 700.0))), "{commands:?}");
        assert!(commands.contains(&egui::ViewportCommand::OuterPosition(egui::pos2(284.0, 10.0))), "moved left to stay on screen: {commands:?}");
        let (grown, commands) = run(false, 5000.0);
        assert!(grown && commands.contains(&egui::ViewportCommand::InnerSize(vec2(1584.0, 700.0))), "never wider than the screen: {commands:?}");
        let (grown, commands) = run(true, 250.0);
        assert!(!grown && commands.is_empty(), "maximized: the panes make room instead: {commands:?}");
    }

    /// The status bar's size slider shows the size kept, and its + saves a step.
    #[test]
    fn the_size_slider_follows_the_size_kept() {
        let mut store = ibercorreo_store::Store::new();
        store.zoom = Some(150);
        let mut app = App::new(Session::new(store, Date::EPOCH));
        let ctx = egui::Context::default();
        let mut shown = Vec::new();
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui));
            out.textures_delta.clear();
            shown =
                out.shapes.iter().filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.job.text.clone()) } else { None }).collect();
        }
        assert!(shown.iter().any(|t| t == "150 %"), "{shown:?}");
        app.session.store.zoom = Some(90);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui));
        out.textures_delta.clear();
        let shown: Vec<String> =
            out.shapes.iter().filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.job.text.clone()) } else { None }).collect();
        assert!(shown.iter().any(|t| t == "90 %"), "{shown:?}");
        assert_eq!(app.zoom_drag, None, "nothing dragged, nothing pending");
    }

    /// The interface takes the size kept in the mailbox, and Ctrl+= makes it larger even while
    /// typing.
    #[test]
    fn the_interface_takes_the_size_kept_and_ctrl_plus_steps_it() {
        let mut store = ibercorreo_store::Store::new();
        store.zoom = Some(150);
        let mut app = App::new(Session::new(store, Date::EPOCH));
        let ctx = egui::Context::default();
        // A new zoom takes effect at the next frame.
        for _ in 0..2 {
            ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui)).textures_delta.clear();
        }
        assert!((ctx.zoom_factor() - 1.5).abs() < 1e-6, "{}", ctx.zoom_factor());
        let press = egui::Event::Key { key: Key::Equals, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
        ctx.run_ui(egui::RawInput { events: vec![press], ..Default::default() }, |ui| app.ui(ui)).textures_delta.clear();
        ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui)).textures_delta.clear();
        assert_eq!(app.session.store.zoom, Some(160));
        assert!((ctx.zoom_factor() - 1.6).abs() < 1e-6, "{}", ctx.zoom_factor());
        assert!(!ctx.options(|o| o.zoom_with_keyboard), "egui's own zoom keys would fight the kept size");
    }

    /// The toolbar never draws one label over another, nor past the window's edge: Ukrainian
    /// labels are long, so on a narrow window the buttons give up their labels instead.
    #[test]
    fn the_toolbar_labels_never_overlap_nor_run_off_the_window() {
        for hide_folders in [false, true] {
            for width in [1920.0, 1600.0, 1360.0, 1100.0, 900.0] {
                let mut store = ibercorreo_store::Store::new();
                store.language = Some("uk".into());
                store.hide_folder_pane = hide_folders;
                let mut app = App::new(Session::new(store, Date::EPOCH));
                let ctx = egui::Context::default();
                let input =
                    || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 860.0))), ..Default::default() };
                ctx.run_ui(input(), |ui| app.ui(ui)).textures_delta.clear();
                let mut out = ctx.run_ui(input(), |ui| app.ui(ui));
                out.textures_delta.clear();
                i18n::set_current(Language::En);
                // Shapes are in points, and the interface is zoomed (110 % by default).
                let limit = width / ctx.zoom_factor();
                let band = chrome::BAR_H..chrome::BAR_H * 2.0;
                let labels: Vec<(String, egui::Rect)> = out
                    .shapes
                    .iter()
                    .filter_map(|c| match &c.shape {
                        egui::Shape::Text(t) => Some((t.galley.job.text.clone(), t.galley.rect.translate(t.pos.to_vec2()))),
                        _ => None,
                    })
                    .filter(|(_, r)| band.contains(&r.center().y))
                    .collect();
                if width >= 1920.0 {
                    assert!(labels.iter().any(|(l, _)| l == "Відповісти"), "Reply keeps its label at {width} px: {labels:?}");
                }
                for (i, (a, ra)) in labels.iter().enumerate() {
                    assert!(ra.max.x <= limit, "{a} runs off a {width} px window at {ra:?}");
                    for (b, rb) in labels.iter().skip(i + 1) {
                        assert!(!ra.intersects(*rb), "{a} {ra:?} overlaps {b} {rb:?} at {width} px");
                    }
                }
            }
        }
    }
}
