//! The Tinta Correo desktop app.
//!
//! ```text
//! ibercorreo [--sample] [--store FILE] [--view mail|calendar|contacts|day|workweek|week|month] [FILE.eml | FILE.ics | FILE.vcf]...
//! ```
//!
//! The mailbox is one store file in the platform's data folder (`--store` picks another).
//! `--sample` opens the sample mailbox instead, without touching the store. `.eml`, `.ics` and
//! `.vcf` files on the command line (or from the file manager's "Open with") are imported.
//!
//! Send/Receive runs on a background thread (`ibercorreo_net::job`, one job per account, one
//! after another), at start, every five minutes and on request; each account's result is applied
//! to the session on the interface thread as it comes. While a folder still has mail on the
//! server to download (a sync takes the newest few hundred), another Send/Receive follows at
//! once. Passwords live in the system's credential store, never in the store file; so do OAuth
//! accounts' refresh tokens (Gmail, Microsoft 365: signed in through the browser on a thread of
//! its own, `ibercorreo_net::oauth`), whose access tokens are renewed as they run out. Once a sync has
//! signed in to an account, another thread keeps an IMAP `IDLE` session on its Inbox, so new mail
//! starts a Send/Receive at once. `IBERCORREO_CA`
//! names a DER certificate to trust instead of the system's (a company CA, or a test server).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tinta_report;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ibercorreo_core::mail::{KEYRING_SERVICE, OAUTH_KEYRING_SERVICE};
use ibercorreo_core::{AccountId, Date, DateTime};
use ibercorreo_engine::Session;
use ibercorreo_net::oauth::{self, Credential};
use ibercorreo_store::Store;
use ibercorreo_ui_egui::{App, NAME};

/// Largest file imported from the command line.
const MAX_IMPORT_BYTES: u64 = 64 * 1024 * 1024;
/// How often unsaved changes are written while the app runs.
const AUTOSAVE: Duration = Duration::from_secs(20);
/// How often mail is fetched while the app runs.
const AUTO_SYNC: Duration = Duration::from_secs(5 * 60);
/// The store's folder name on Windows and macOS. Like [`KEYRING_SERVICE`] it keeps the app's
/// first name, so the mail saved before the rename stays where the app looks for it.
const DATA_DIR_NAME: &str = "Iber Correo";

fn now() -> DateTime {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    DateTime::from_unix(i64::try_from(secs).unwrap_or(0)).unwrap_or(DateTime::midnight(Date::EPOCH))
}

/// Where the store lives: `%APPDATA%\Iber Correo`, `~/Library/Application Support/Iber Correo`,
/// or `$XDG_DATA_HOME/ibercorreo` (default `~/.local/share/ibercorreo`).
fn data_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "windows") {
        env("APPDATA").map(|p| p.join(DATA_DIR_NAME))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|p| p.join("Library/Application Support").join(DATA_DIR_NAME))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share"))).map(|p| p.join("ibercorreo"))
    }
}

struct Shell {
    app: App,
    last_save: Instant,
    /// The running save, if any: the store is written on another thread, as a big mailbox takes
    /// seconds to write and the window would stop for that long.
    saving: Option<Receiver<Result<(), String>>>,
    /// The sample mailbox is never written to disk.
    persist: bool,
    /// The running Send/Receive, if any.
    worker: Option<Receiver<Progress>>,
    next_sync: Instant,
    /// What the running Send/Receive found so far: each account's status, and whether a folder
    /// still has mail on the server to download.
    outcome: Vec<String>,
    more: bool,
    /// Passwords for this run (by sign-in name), when the credential store could not keep them.
    passwords: HashMap<String, String>,
    /// The running search for an address's server settings, if any.
    finder: Option<Receiver<Result<ibercorreo_ui_egui::Discovered, String>>>,
    /// The threads that keep an `IDLE` session open on each account's Inbox, once a sync has
    /// signed in to it.
    watchers: HashMap<AccountId, Watcher>,
    /// To wake the window from other threads.
    ctx: Option<egui::Context>,
    /// The OAuth accounts' grants (by `provider:login`), shared with the threads.
    credentials: HashMap<String, Credential>,
    /// The running browser sign-in, if any.
    sign_in: Option<SignIn>,
}

/// What the Send/Receive thread reports: each account's result, then that it is done.
enum Progress {
    /// How far it has come: what to show, and the share done of every account's sync.
    Step(String, f32),
    Account(AccountId, Box<ibercorreo_net::job::Finished>),
    Done,
}

/// A browser sign-in on its own thread.
struct SignIn {
    /// The account signing in, and its `provider:login`.
    account: AccountId,
    key: String,
    app: oauth::App,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    rx: Receiver<SignInEvent>,
}

enum SignInEvent {
    /// The address opened in the browser.
    Opened(String),
    Done(Result<oauth::Tokens, String>),
}

impl Drop for SignIn {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Push mail: a background thread idling on the Inbox ([`ibercorreo_net::idle`]).
struct Watcher {
    /// The account and server it watches; another one means starting again.
    key: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// A message each time the Inbox changed.
    changed: Receiver<()>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Watch the Inbox until `stop`: each change is sent on `tx`. Lost connections are retried, less
/// often each time (one minute up to fifteen); a server without IDLE ends the watch (the
/// five-minute Send/Receive still runs).
fn watch_inbox(
    endpoint: ibercorreo_net::Endpoint,
    tls: ibercorreo_net::Tls,
    login: String,
    credential: Credential,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: mpsc::Sender<()>,
    ctx: Option<egui::Context>,
) {
    use std::sync::atomic::Ordering;
    let stopped = || stop.load(Ordering::Relaxed);
    let mut backoff = Duration::from_secs(60);
    while !stopped() {
        // An access token is renewed for each connection; a grant that is gone ends the watch.
        let secret = match credential.secret(&tls) {
            Ok(s) => Ok(s),
            Err(e @ oauth::Error::Network { .. }) => Err(e),
            Err(_) => return,
        };
        let result = secret
            .map_err(ibercorreo_net::Error::from)
            .and_then(|secret| ibercorreo_net::idle::watch(&endpoint, &tls, &login, &secret, "INBOX", &stopped));
        match result {
            Ok(ibercorreo_net::idle::Wake::Changed) => {
                backoff = Duration::from_secs(60);
                if tx.send(()).is_err() {
                    return;
                }
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
                }
            }
            Ok(ibercorreo_net::idle::Wake::Stopped) => return,
            Err(ibercorreo_net::Error::Imap(ibercorreo_net::imap::Error::Unsupported(_) | ibercorreo_net::imap::Error::No(_))) => {
                credential.forget_access();
                return;
            }
            Err(_) => {
                let until = Instant::now() + backoff;
                while Instant::now() < until && !stopped() {
                    std::thread::sleep(Duration::from_millis(500));
                }
                backoff = (backoff * 2).min(Duration::from_secs(15 * 60));
            }
        }
    }
}

/// Why nothing was found, in one sentence: the places tried are many and their errors long.
fn short_reason(tried: &[String]) -> String {
    match tried.last() {
        Some(last) if tried.len() > 1 => format!("the provider publishes none, and {last}"),
        Some(last) => last.clone(),
        None => "nothing answered".into(),
    }
}

/// The servers found for an address, as Account settings shows them.
fn discovered(found: ibercorreo_net::autoconfig::Found) -> Result<ibercorreo_ui_egui::Discovered, String> {
    use ibercorreo_net::dav::autoconfig::Socket;
    let s = found.settings;
    let (Some(imap), Some(smtp)) = (s.imap.first(), s.smtp.first()) else { return Err("no servers".into()) };
    Ok(ibercorreo_ui_egui::Discovered {
        provider: s.provider.clone(),
        source: match found.source {
            ibercorreo_net::autoconfig::Source::Published(url) => Some(url),
            ibercorreo_net::autoconfig::Source::Guessed => None,
        },
        imap_host: imap.host.clone(),
        imap_port: imap.port,
        imap_starttls: imap.socket == Socket::StartTls,
        smtp_host: smtp.host.clone(),
        smtp_port: smtp.port,
        smtp_starttls: smtp.socket == Socket::StartTls,
        user: imap.username.clone(),
        oauth_only: imap.oauth_only || smtp.oauth_only,
    })
}

fn keyring_entry(login: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, login).ok()
}

fn oauth_entry(key: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(OAUTH_KEYRING_SERVICE, key).ok()
}

fn tls() -> Result<ibercorreo_net::Tls, String> {
    match std::env::var_os("IBERCORREO_CA") {
        Some(path) => {
            let der = std::fs::read(&path).map_err(|e| format!("{}: {e}", PathBuf::from(&path).display()))?;
            ibercorreo_net::Tls::with_roots(&[der]).map_err(|e| e.to_string())
        }
        None => ibercorreo_net::Tls::platform().map_err(|e| e.to_string()),
    }
}

impl Shell {
    /// A password typed in Account settings: into the credential store, or kept for this run only.
    fn keep_password(&mut self) {
        let Some((account, password)) = self.app.take_password() else { return };
        let Some(login) = self.app.session.store.account(account).ok().and_then(|a| a.login()).map(str::to_owned) else { return };
        let stored = keyring_entry(&login).is_some_and(|e| e.set_password(&password).is_ok());
        if !stored {
            self.app.session.status = "The system credential store is unavailable: the password is kept until Tinta Correo closes".into();
        }
        // A new password is watched with afresh.
        self.watchers.remove(&account);
        self.passwords.insert(login, password);
    }

    /// How the account signs in: its password, or its OAuth grant. `Err` is what Account settings
    /// should ask for.
    fn credential(&mut self, account: &ibercorreo_core::mail::Account) -> Result<Credential, String> {
        let Some(login) = account.login().map(str::to_owned) else { return Err("This account has no sign-in name.".into()) };
        let Some(provider) = account.oauth() else {
            let password = self.passwords.get(&login).cloned().or_else(|| keyring_entry(&login).and_then(|e| e.get_password().ok()));
            return password.map(Credential::Password).ok_or_else(|| format!("Enter the password for {}.", account.address.email));
        };
        let key = format!("{}:{login}", provider.id);
        if let Some(c) = self.credentials.get(&key) {
            return Ok(c.clone());
        }
        let app = oauth::app(provider.id).map_err(|e| e.to_string())?;
        let refresh = oauth_entry(&key)
            .and_then(|e| e.get_password().ok())
            .ok_or_else(|| format!("Sign in with {} to send and receive {}'s mail: press Save and sign in.", provider.name, account.address.email))?;
        let credential = Credential::oauth(app, refresh);
        self.credentials.insert(key, credential.clone());
        Ok(credential)
    }

    /// Refresh tokens the providers replaced go into the credential store.
    fn keep_rotated(&mut self) {
        for (key, c) in &self.credentials {
            if let Some(token) = c.take_rotated() {
                // Unstored, it still serves until Tinta Correo closes.
                let _ = oauth_entry(key).map(|e| e.set_password(&token));
            }
        }
    }

    /// Start the browser sign-in Account settings asked for, stop it, or take its outcome.
    fn sign_in(&mut self) {
        if std::mem::take(&mut self.app.oauth_cancel) && self.sign_in.take().is_some() {
            self.app.apply_oauth(Err("cancelled".into()));
        }
        if let Some(account) = self.app.oauth_request.take()
            && self.sign_in.is_none()
        {
            match self.start_sign_in(account) {
                Ok(s) => {
                    self.sign_in = Some(s);
                    self.app.oauth_running = Some(String::new());
                }
                Err(e) => self.app.apply_oauth(Err(e)),
            }
        }
        let Some(s) = &self.sign_in else { return };
        let event = match s.rx.try_recv() {
            Ok(e) => e,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => SignInEvent::Done(Err("the sign-in stopped unexpectedly".into())),
        };
        match event {
            SignInEvent::Opened(url) => self.app.oauth_running = Some(url),
            SignInEvent::Done(result) => {
                let Some(s) = self.sign_in.take() else { return };
                match result {
                    Ok(tokens) => {
                        let name = s.app.provider.name.to_string();
                        let stored = tokens.refresh.as_deref().is_some_and(|t| oauth_entry(&s.key).is_some_and(|e| e.set_password(t).is_ok()));
                        self.credentials.insert(s.key.clone(), Credential::signed_in(s.app.clone(), tokens));
                        // Watch the Inbox again with the new sign-in.
                        self.watchers.remove(&s.account);
                        self.app.apply_oauth(Ok(name));
                        if !stored {
                            self.app.session.status =
                                "Signed in; the system credential store is unavailable, so you sign in again after Tinta Correo closes".into();
                        }
                    }
                    Err(e) => self.app.apply_oauth(Err(e)),
                }
            }
        }
    }

    fn start_sign_in(&mut self, account: AccountId) -> Result<SignIn, String> {
        let account = self.app.session.store.account(account).map_err(|e| e.to_string())?.clone();
        let provider = account.oauth().ok_or("this account signs in with a password")?;
        let login = account.login().ok_or("no sign-in name")?;
        let app = oauth::app(provider.id).map_err(|e| e.to_string())?;
        let tls = tls()?;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (thread_app, thread_stop, ctx, hint) = (app.clone(), stop.clone(), self.ctx.clone(), account.address.email.clone());
        std::thread::Builder::new()
            .name("oauth".into())
            .spawn(move || {
                let wake = || {
                    if let Some(ctx) = &ctx {
                        ctx.request_repaint();
                    }
                };
                let mut open = |url: &str| {
                    let _ = tx.send(SignInEvent::Opened(url.to_string()));
                    wake();
                    // Without a browser the person copies the address from Account settings.
                    let _ = oauth::open_browser(url);
                    Ok(())
                };
                let stopped = || thread_stop.load(std::sync::atomic::Ordering::Relaxed);
                let result = oauth::sign_in_with(&mut oauth::Net { tls: &tls }, &thread_app, &hint, &mut open, &stopped);
                let _ = tx.send(SignInEvent::Done(result.map_err(|e| e.to_string())));
                wake();
            })
            .map_err(|e| e.to_string())?;
        Ok(SignIn { account: account.id, key: format!("{}:{login}", provider.id), app, stop, rx })
    }

    fn start_sync(&mut self) {
        // A request made while a Send/Receive runs waits for it to finish.
        if self.worker.is_some() {
            return;
        }
        self.app.session.sync_requested = false;
        self.next_sync = Instant::now() + AUTO_SYNC;
        // Removed accounts are no longer watched.
        let store = &self.app.session.store;
        self.watchers.retain(|id, _| store.account(*id).is_ok());
        let accounts: Vec<_> = store.accounts.iter().filter(|a| a.login().is_some()).cloned().collect();
        // A sign-in in the browser comes first.
        if accounts.is_empty() || self.sign_in.is_some() {
            return;
        }
        let tls = match tls() {
            Ok(t) => t,
            Err(e) => {
                self.app.session.status = format!("Secure connections are unavailable: {e}");
                return;
            }
        };
        let mut jobs = Vec::new();
        let mut asked = false;
        for account in accounts {
            let credential = match self.credential(&account) {
                Ok(c) => c,
                Err(note) => {
                    // The others still sync; Account settings asks for the first one missing.
                    self.app.session.status = note.clone();
                    if !asked {
                        asked = true;
                        self.app.open_account_settings(Some(account.id), &note);
                    }
                    continue;
                }
            };
            let s = &self.app.session;
            let id = account.id;
            jobs.push(ibercorreo_net::job::Job {
                state: s.sync_state(id),
                outbox: s.outbox(id),
                sent_path: s.sent_path(id),
                account,
                credential,
                tls: tls.clone(),
                limits: ibercorreo_sync::Limits::default(),
                calendar: s.calendar_sync_state(),
                contacts: s.contacts_sync_state(),
            });
        }
        if jobs.is_empty() {
            return;
        }
        self.outcome.clear();
        self.more = false;
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new().name("send-receive".into()).spawn(move || {
            let count = jobs.len().max(1) as f32;
            for (n, job) in jobs.iter().enumerate() {
                let email = job.account.address.email.clone();
                let mut step = |p: &ibercorreo_sync::Progress| {
                    let what = if p.to_fetch > 0 {
                        format!("{email} · {} {} of {}", p.folder, p.fetched, p.to_fetch)
                    } else {
                        format!("{email} · {}", p.folder)
                    };
                    let _ = tx.send(Progress::Step(what, (n as f32 + p.fraction()) / count));
                    if let Some(ctx) = &ctx {
                        ctx.request_repaint();
                    }
                };
                let finished = ibercorreo_net::job::run_with(job, &mut step);
                if tx.send(Progress::Account(job.account.id, Box::new(finished))).is_err() {
                    return;
                }
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
                }
            }
            let _ = tx.send(Progress::Done);
        });
        match spawned {
            Ok(_) => self.worker = Some(rx),
            Err(e) => self.app.session.status = format!("Could not start Send/Receive: {e}"),
        }
    }

    /// Take what the running Send/Receive has finished: each account's result as it comes.
    fn finish_sync(&mut self) {
        loop {
            let Some(rx) = &self.worker else { return };
            match rx.try_recv() {
                Ok(Progress::Step(what, done)) => self.app.sync_progress = Some((what, done)),
                Ok(Progress::Account(id, finished)) => self.finish_account(id, *finished),
                Ok(Progress::Done) => {
                    self.worker = None;
                    self.app.sync_progress = None;
                    if !self.outcome.is_empty() {
                        self.app.session.status = self.outcome.join("; ");
                    }
                    warm_search(&self.app.session.store);
                    // Mail still on the server: the next part comes at once.
                    if self.more {
                        self.app.session.sync_requested = true;
                    }
                    return;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.app.sync_progress = None;
                    self.app.session.status = "Send/Receive stopped unexpectedly; what had not finished was not changed".into();
                    return;
                }
            }
        }
    }

    /// Fold one account's Send/Receive into the session.
    fn finish_account(&mut self, id: AccountId, finished: ibercorreo_net::job::Finished) {
        // The account may have gone while it synced: nothing of it is kept.
        if self.app.session.store.account(id).is_err() {
            return;
        }
        if !finished.sent.is_empty() {
            self.app.session.apply_sent(&finished.sent);
        }
        match finished.batch {
            Ok(batch) => {
                let report = self.app.session.apply_sync(id, batch);
                self.more |= report.remaining > 0 && report.added > 0;
                self.keep_watching(id);
            }
            Err(e) => {
                if e.starts_with("sign-in") {
                    self.signed_out(id, &e);
                }
                let email = self.app.session.store.account(id).map(|a| a.address.email.clone()).unwrap_or_default();
                self.app.session.status = format!("Send/Receive failed for {email}: {e}");
            }
        }
        match finished.calendar {
            Some(Ok(batch)) => {
                self.app.session.apply_calendar_sync(batch);
            }
            Some(Err(e)) => {
                let status = &mut self.app.session.status;
                status.push_str(&format!("; calendars: {e}"));
            }
            None => {}
        }
        match finished.contacts {
            Some(Ok(batch)) => {
                self.app.session.apply_contacts_sync(batch);
            }
            Some(Err(e)) => {
                let status = &mut self.app.session.status;
                status.push_str(&format!("; contacts: {e}"));
            }
            None => {}
        }
        self.outcome.push(self.app.session.status.clone());
    }

    /// The server refused an account's sign-in: a refused password is not kept for the next try,
    /// nor watched with; nor is an OAuth grant the provider says is gone. Account settings opens
    /// for it.
    fn signed_out(&mut self, id: AccountId, e: &str) {
        self.watchers.remove(&id);
        let Ok(account) = self.app.session.store.account(id) else { return };
        let Some(login) = account.login().map(str::to_owned) else { return };
        let note = match account.oauth() {
            Some(provider) => {
                let key = format!("{}:{login}", provider.id);
                match self.credentials.get(&key) {
                    Some(c) if c.revoked() => {
                        self.credentials.remove(&key);
                        let _ = oauth_entry(&key).map(|e| e.delete_credential());
                        format!("{}: press Save and sign in.", e.trim_start_matches("sign-in: "))
                    }
                    _ => format!("The server refused the sign-in: {e}"),
                }
            }
            None => {
                self.passwords.remove(&login);
                format!("The server refused the sign-in: {e}")
            }
        };
        if !self.app.account_settings_open() {
            self.app.open_account_settings(Some(id), &note);
        }
    }

    /// After a sync that signed in: make sure the account's Inbox is watched with its settings.
    fn keep_watching(&mut self, id: AccountId) {
        let Ok(account) = self.app.session.store.account(id).cloned() else {
            self.watchers.remove(&id);
            return;
        };
        let ibercorreo_core::mail::AccountKind::Imap { imap_host, imap_port, imap_security, .. } = &account.kind else { return };
        let Some(login) = account.login().map(str::to_owned) else { return };
        let key = format!("{login}@{imap_host}:{imap_port}");
        if self.watchers.get(&id).is_some_and(|w| w.key == key) {
            return;
        }
        let Ok(credential) = self.credential(&account) else { return };
        let Ok(tls) = tls() else { return };
        let security = match imap_security {
            ibercorreo_core::Security::Tls => ibercorreo_net::Security::Tls,
            ibercorreo_core::Security::StartTls => ibercorreo_net::Security::StartTls,
        };
        let endpoint = ibercorreo_net::Endpoint { host: imap_host.clone(), port: *imap_port, security };
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (thread_stop, ctx) = (stop.clone(), self.ctx.clone());
        let spawned =
            std::thread::Builder::new().name("idle".into()).spawn(move || watch_inbox(endpoint, tls, login, credential, thread_stop, tx, ctx));
        match spawned {
            Ok(_) => {
                self.watchers.insert(id, Watcher { key, stop, changed: rx });
            }
            Err(_) => {
                self.watchers.remove(&id);
            }
        }
    }

    /// A change a watcher saw asks for a Send/Receive.
    fn watched(&mut self) {
        let mut gone = Vec::new();
        for (id, w) in &self.watchers {
            match w.changed.try_recv() {
                Ok(()) => {
                    while w.changed.try_recv().is_ok() {}
                    self.app.session.sync_requested = true;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => gone.push(*id),
            }
        }
        for id in gone {
            self.watchers.remove(&id);
        }
    }

    /// Start looking for the servers of the address Account settings asked about, or take the
    /// answer when it has come.
    fn autoconfig(&mut self) {
        if let Some(email) = self.app.autoconfig_request.take()
            && self.finder.is_none()
        {
            match tls() {
                Ok(tls) => {
                    let (tx, rx) = mpsc::channel();
                    let spawned = std::thread::Builder::new().name("autoconfig".into()).spawn(move || {
                        let found = ibercorreo_net::autoconfig::find(&email, &tls).map_err(|tried| short_reason(&tried)).and_then(discovered);
                        let _ = tx.send(found);
                    });
                    match spawned {
                        Ok(_) => self.finder = Some(rx),
                        Err(e) => self.app.apply_autoconfig(Err(e.to_string())),
                    }
                }
                Err(e) => self.app.apply_autoconfig(Err(e)),
            }
        }
        if let Some(rx) = &self.finder {
            match rx.try_recv() {
                Ok(found) => {
                    self.finder = None;
                    self.app.apply_autoconfig(found);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.finder = None;
                    self.app.apply_autoconfig(Err("the search stopped unexpectedly".into()));
                }
            }
        }
        self.app.autoconfig_running = self.finder.is_some();
    }

    /// Write the store if it changed, on another thread; one save runs at a time.
    fn save_if_dirty(&mut self) {
        self.last_save = Instant::now();
        let session = &mut self.app.session;
        if !self.persist || !session.dirty || self.saving.is_some() {
            return;
        }
        let Some(path) = session.path.clone() else { return };
        // A copy shares the messages with the session, so it is quick to make.
        let store = session.store.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("save".into()).spawn(move || {
            let _ = tx.send(store.save(&path).map_err(|e| e.to_string()));
        });
        match spawned {
            Ok(_) => {
                session.dirty = false;
                self.saving = Some(rx);
            }
            // No thread: save here, as before.
            Err(_) => {
                let _ = session.run("file.save", &serde_json::Value::Null);
            }
        }
    }

    /// Take the result of the running save: a failed one leaves the store to be saved again.
    fn finish_save(&mut self, wait: bool) {
        let Some(rx) = &self.saving else { return };
        let result = if wait { rx.recv().map_err(|_| TryRecvError::Disconnected) } else { rx.try_recv() };
        let failed = match result {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Some("the save stopped unexpectedly".to_string()),
        };
        self.saving = None;
        if let Some(e) = failed {
            self.app.session.dirty = true;
            self.app.session.status = format!("Could not save the mailbox: {e}");
        }
    }
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        tinta_report::busy();
        tinta_report::tick(&mut self.app);
        self.app.session.set_now(now());
        if self.ctx.is_none() {
            self.ctx = Some(ui.ctx().clone());
        }
        self.finish_save(false);
        self.finish_sync();
        self.watched();
        self.app.syncing = self.worker.is_some();
        self.autoconfig();
        self.app.ui(ui);
        self.keep_password();
        self.sign_in();
        self.keep_rotated();
        if self.persist && (self.app.session.sync_requested || Instant::now() >= self.next_sync) {
            self.start_sync();
        }
        if self.last_save.elapsed() >= AUTOSAVE {
            self.save_if_dirty();
        }
        // Poll a running Send/Receive often; otherwise wake for autosave and the next sync.
        let busy = self.worker.is_some() || self.finder.is_some() || self.sign_in.is_some() || self.saving.is_some();
        ui.ctx().request_repaint_after(if busy { Duration::from_millis(200) } else { AUTOSAVE });
        tinta_report::idle();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.watchers.clear();
        self.sign_in = None;
        self.keep_rotated();
        // The last save waits for the one running, then writes what changed since.
        self.finish_save(true);
        self.save_if_dirty();
        self.finish_save(true);
    }
}

/// Read the mail into the search index on another thread, so a search does not wait for it.
fn warm_search(store: &Store) {
    let job = store.search_warmup();
    let _ = std::thread::Builder::new().name("search index".into()).spawn(job);
}

fn main() -> eframe::Result {
    let mut sample = false;
    let mut store_path = None;
    let mut imports = Vec::new();
    let mut view: Option<String> = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--sample") => sample = true,
            Some("--store") => store_path = args.next().map(PathBuf::from),
            // Opens on a view (mail, calendar, contacts): for screenshots and launchers.
            Some("--view") => view = args.next().and_then(|v| v.into_string().ok()),
            Some("--help" | "-h") => {
                println!(
                    "{NAME}\n\nibercorreo [--sample] [--store FILE] [--view mail|calendar|contacts|day|workweek|week|month] [FILE.eml | FILE.ics | FILE.vcf]..."
                );
                return Ok(());
            }
            _ => imports.push(PathBuf::from(a)),
        }
    }
    let zone = ibercorreo_core::Zone::system();
    let today = zone.from_utc(now()).date;
    let store_path = store_path.or_else(|| data_dir().map(|d| d.join("store.json")));
    let mut status = String::new();
    // Off when a mailbox that could not be read could not be moved aside either: never save over it.
    let mut writable = true;
    let store = if sample {
        ibercorreo_engine::sample::store(today).unwrap_or_default()
    } else {
        match store_path.as_ref().filter(|p| p.exists()) {
            Some(p) => match Store::load(p) {
                Ok(s) => {
                    let n = s.repaired_on_load();
                    if n > 0 {
                        status = format!(
                            "{n} messages in folders the mailbox did not list were left out (the last save was cut short); synced ones come back at the next Send/Receive"
                        );
                    }
                    s
                }
                Err(e) => {
                    // Never overwrite a store we could not read: keep it aside (with its message
                    // shards) and start fresh.
                    status = match Store::set_aside(p) {
                        Ok(aside) => format!("Could not read the mailbox ({e}); it was kept as {}", aside.display()),
                        Err(moved) => {
                            writable = false;
                            format!("Could not read the mailbox ({e}), nor keep it aside ({moved}); nothing is saved over it")
                        }
                    };
                    Store::new()
                }
            },
            None => Store::new(),
        }
    };
    let mut session = Session::new(store, today);
    session.set_now(now());
    session.set_system_zone(zone);
    // The store may name another zone than the computer's; open on today there.
    session.day = session.today();
    if !sample
        && writable
        && let Some(p) = &store_path
    {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // The first save of a store from before shards rewrites it: keep the old file once.
        if let Err(e) = Store::keep_unsharded_copy(p) {
            status = format!("Could not keep a copy of the mailbox before converting it: {e}");
        }
        session.path = Some(p.clone());
        // A mailbox from an older version is written in the current format right away, so the
        // next start keeps message text on disk instead of in memory.
        if session.store.needs_rewrite() {
            session.dirty = true;
        }
    }
    for path in imports {
        let result = std::fs::metadata(&path)
            .map_err(|e| e.to_string())
            .and_then(|m| if m.len() > MAX_IMPORT_BYTES { Err("file too large".to_string()) } else { Ok(()) })
            .and_then(|()| std::fs::read(&path).map_err(|e| e.to_string()));
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        let outcome = match (result, ext.as_deref()) {
            (Ok(bytes), Some("ics")) => {
                session.run("calendar.importIcs", &serde_json::json!({ "text": String::from_utf8_lossy(&bytes) })).map(|_| ())
            }
            (Ok(bytes), Some("vcf" | "vcard")) => {
                session.run("contact.importVcf", &serde_json::json!({ "text": String::from_utf8_lossy(&bytes) })).map(|_| ())
            }
            (Ok(bytes), _) => session.run("message.importEml", &serde_json::json!({ "raw": String::from_utf8_lossy(&bytes) })).map(|_| ()),
            (Err(e), _) => Err(ibercorreo_core::Error::Storage(e)),
        };
        if let Err(e) = outcome {
            status = format!("{}: {e}", path.display());
        }
    }
    if let Some(v) = view {
        // The calendar's own views open the calendar on that view.
        let id = match v.as_str() {
            "day" => "calendar.viewDay".to_string(),
            "workweek" => "calendar.viewWorkWeek".to_string(),
            "week" => "calendar.viewWeek".to_string(),
            "month" => "calendar.viewMonth".to_string(),
            other => format!("view.{other}"),
        };
        let _ = session.run(&id, &serde_json::Value::Null);
    }
    if !status.is_empty() {
        session.status = status;
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(NAME)
        .with_app_id("io.github.stdenis1980.ibercorreo")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([760.0, 480.0]);
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/io.github.stdenis1980.ibercorreo.png"))
    {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let persist = !sample;
    // Tinta Office: error reports, sent only with consent (asked at the first start). Not for
    // the sample mailbox, which is never saved.
    let reports = tinta_report::dir(data_dir(), persist && store_path.is_some());
    if let Some(dir) = &reports {
        tinta_report::install(dir.clone(), None, "release");
    }
    eframe::run_native(
        NAME,
        options,
        Box::new(move |_cc| {
            warm_search(&session.store);
            let mut app = App::new(session);
            app.oauth_ready = oauth::configured().iter().map(|p| p.id.to_string()).collect();
            app.error_reports_dir = reports;
            Ok(Box::new(Shell {
                app,
                last_save: Instant::now(),
                saving: None,
                persist,
                worker: None,
                outcome: Vec::new(),
                more: false,
                finder: None,
                watchers: HashMap::new(),
                ctx: None,
                credentials: HashMap::new(),
                sign_in: None,
                // A first Send/Receive right after start.
                next_sync: Instant::now(),
                passwords: HashMap::new(),
            }))
        }),
    )
}
