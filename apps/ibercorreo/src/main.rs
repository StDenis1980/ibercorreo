//! The Iber Correo desktop app.
//!
//! ```text
//! ibercorreo [--sample] [--store FILE] [--view mail|calendar|contacts|day|workweek|week|month] [FILE.eml | FILE.ics | FILE.vcf]...
//! ```
//!
//! The mailbox is one store file in the platform's data folder (`--store` picks another).
//! `--sample` opens the sample mailbox instead, without touching the store. `.eml`, `.ics` and
//! `.vcf` files on the command line (or from the file manager's "Open with") are imported.
//!
//! Send/Receive runs on a background thread (`ibercorreo_net::job`), at start, every five
//! minutes and on request; its result is applied to the session on the interface thread. The
//! password lives in the system's credential store, never in the store file. `IBERCORREO_CA`
//! names a DER certificate to trust instead of the system's (a company CA, or a test server).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ibercorreo_core::{Date, DateTime};
use ibercorreo_engine::Session;
use ibercorreo_store::Store;
use ibercorreo_ui_egui::{App, NAME};

/// Largest file imported from the command line.
const MAX_IMPORT_BYTES: u64 = 64 * 1024 * 1024;
/// How often unsaved changes are written while the app runs.
const AUTOSAVE: Duration = Duration::from_secs(20);
/// How often mail is fetched while the app runs.
const AUTO_SYNC: Duration = Duration::from_secs(5 * 60);
/// The credential store's service name for account passwords.
const KEYRING_SERVICE: &str = "Iber Correo";

fn now() -> DateTime {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    DateTime::from_unix(i64::try_from(secs).unwrap_or(0)).unwrap_or(DateTime::midnight(Date::EPOCH))
}

/// Where the store lives: `%APPDATA%\Iber Correo`, `~/Library/Application Support/Iber Correo`,
/// or `$XDG_DATA_HOME/ibercorreo` (default `~/.local/share/ibercorreo`).
fn data_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "windows") {
        env("APPDATA").map(|p| p.join(NAME))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|p| p.join("Library/Application Support").join(NAME))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share"))).map(|p| p.join("ibercorreo"))
    }
}

struct Shell {
    app: App,
    last_save: Instant,
    /// The sample mailbox is never written to disk.
    persist: bool,
    /// The running Send/Receive, if any.
    worker: Option<Receiver<ibercorreo_net::job::Finished>>,
    next_sync: Instant,
    /// The password for this run, when the credential store could not keep it.
    password: Option<String>,
}

fn keyring_entry(login: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, login).ok()
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
        let Some(password) = self.app.take_password() else { return };
        let login = self.app.session.store.accounts.first().and_then(|a| a.login()).map(str::to_owned);
        let stored = login.as_deref().and_then(keyring_entry).is_some_and(|e| e.set_password(&password).is_ok());
        if !stored {
            self.app.session.status = "The system credential store is unavailable: the password is kept until Iber Correo closes".into();
        }
        self.password = Some(password);
    }

    fn start_sync(&mut self) {
        self.app.session.sync_requested = false;
        self.next_sync = Instant::now() + AUTO_SYNC;
        if self.worker.is_some() {
            return;
        }
        let Some(account) = self.app.session.store.accounts.first().cloned() else { return };
        let Some(login) = account.login().map(str::to_owned) else { return };
        let password = self.password.clone().or_else(|| keyring_entry(&login).and_then(|e| e.get_password().ok()));
        let Some(password) = password else {
            self.app.session.status = "Send/Receive needs the account's password".into();
            self.app.open_account_settings("Enter the password for this account.");
            return;
        };
        let tls = match tls() {
            Ok(t) => t,
            Err(e) => {
                self.app.session.status = format!("Secure connections are unavailable: {e}");
                return;
            }
        };
        let s = &self.app.session;
        let job = ibercorreo_net::job::Job {
            account,
            password,
            tls,
            state: s.sync_state(),
            outbox: s.outbox(),
            sent_path: s.sent_path(),
            limits: ibercorreo_sync::Limits::default(),
            calendar: s.calendar_sync_state(),
        };
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("send-receive".into()).spawn(move || {
            let _ = tx.send(ibercorreo_net::job::run(&job));
        });
        match spawned {
            Ok(_) => self.worker = Some(rx),
            Err(e) => self.app.session.status = format!("Could not start Send/Receive: {e}"),
        }
    }

    fn finish_sync(&mut self) {
        let Some(rx) = &self.worker else { return };
        let finished = match rx.try_recv() {
            Ok(f) => f,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.worker = None;
                self.app.session.status = "Send/Receive stopped unexpectedly; nothing was changed".into();
                return;
            }
        };
        self.worker = None;
        if !finished.sent.is_empty() {
            self.app.session.apply_sent(&finished.sent);
        }
        match finished.batch {
            Ok(batch) => {
                self.app.session.apply_sync(batch);
            }
            Err(e) => {
                if e.starts_with("sign-in") {
                    // A refused password is not kept for the next try.
                    self.password = None;
                    self.app.open_account_settings(&format!("The server refused the sign-in: {e}"));
                }
                self.app.session.status = format!("Send/Receive failed: {e}");
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
    }
    fn save_if_dirty(&mut self) {
        if self.persist && self.app.session.dirty {
            // A failed save stays dirty and is shown in the status bar; the next attempt retries.
            let _ = self.app.session.run("file.save", &serde_json::Value::Null);
        }
        self.last_save = Instant::now();
    }
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.session.set_now(now());
        self.finish_sync();
        self.app.syncing = self.worker.is_some();
        self.app.ui(ui);
        self.keep_password();
        if self.persist && (self.app.session.sync_requested || Instant::now() >= self.next_sync) {
            self.start_sync();
        }
        if self.last_save.elapsed() >= AUTOSAVE {
            self.save_if_dirty();
        }
        // Poll a running Send/Receive often; otherwise wake for autosave and the next sync.
        ui.ctx().request_repaint_after(if self.worker.is_some() { Duration::from_millis(200) } else { AUTOSAVE });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_if_dirty();
    }
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
    let store = if sample {
        ibercorreo_engine::sample::store(today).unwrap_or_default()
    } else {
        match store_path.as_ref().filter(|p| p.exists()) {
            Some(p) => match Store::load(p) {
                Ok(s) => s,
                Err(e) => {
                    // Never overwrite a store we could not read: keep it aside and start fresh.
                    let aside = p.with_extension("unreadable.json");
                    let _ = std::fs::rename(p, &aside);
                    status = format!("Could not read the mailbox ({e}); it was kept as {}", aside.display());
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
    if !sample && let Some(p) = &store_path {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        session.path = Some(p.clone());
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
    eframe::run_native(
        NAME,
        options,
        Box::new(move |_cc| {
            Ok(Box::new(Shell {
                app: App::new(session),
                last_save: Instant::now(),
                persist,
                worker: None,
                // A first Send/Receive right after start.
                next_sync: Instant::now(),
                password: None,
            }))
        }),
    )
}
