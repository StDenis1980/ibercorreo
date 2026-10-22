//! The Iber Correo desktop app.
//!
//! ```text
//! ibercorreo [--sample] [--store FILE] [--view mail|calendar|contacts] [FILE.eml | FILE.ics]...
//! ```
//!
//! The mailbox is one store file in the platform's data folder (`--store` picks another).
//! `--sample` opens the sample mailbox instead, without touching the store. `.eml` and `.ics`
//! files on the command line (or from the file manager's "Open with") are imported.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ibercorreo_core::{Date, DateTime};
use ibercorreo_engine::Session;
use ibercorreo_store::Store;
use ibercorreo_ui_egui::{App, NAME};

/// Largest file imported from the command line.
const MAX_IMPORT_BYTES: u64 = 64 * 1024 * 1024;
/// How often unsaved changes are written while the app runs.
const AUTOSAVE: Duration = Duration::from_secs(20);

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
}

impl Shell {
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
        self.app.ui(ui);
        if self.last_save.elapsed() >= AUTOSAVE {
            self.save_if_dirty();
        }
        ui.ctx().request_repaint_after(AUTOSAVE);
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
                println!("{NAME}\n\nibercorreo [--sample] [--store FILE] [--view mail|calendar|contacts] [FILE.eml | FILE.ics]...");
                return Ok(());
            }
            _ => imports.push(PathBuf::from(a)),
        }
    }
    let today = now().date;
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
            (Ok(bytes), _) => session.run("message.importEml", &serde_json::json!({ "raw": String::from_utf8_lossy(&bytes) })).map(|_| ()),
            (Err(e), _) => Err(ibercorreo_core::Error::Storage(e)),
        };
        if let Err(e) = outcome {
            status = format!("{}: {e}", path.display());
        }
    }
    if let Some(v) = view {
        let _ = session.run(&format!("view.{v}"), &serde_json::Value::Null);
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
    eframe::run_native(NAME, options, Box::new(move |_cc| Ok(Box::new(Shell { app: App::new(session), last_save: Instant::now(), persist }))))
}
