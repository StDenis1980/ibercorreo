//! Tinta Office: automatic error reports, sent only with the person's consent
//! ([`ibercorreo_store::tinta_reports`] keeps the choice in the store, `ibercorreo_ui_egui`
//! asks for it).
//!
//! Builds carry the address in `TINTA_REPORT_URL` (`.cargo/config.toml`); without it nothing here
//! runs, so upstream's and other builds never ask and never send. A panic (caught by
//! `Session::run`'s guard or not) and a window that stops responding for [`FREEZE_SECS`] write a
//! report into `<data>/error-reports/pending` (never when the person said no). While reports
//! are allowed, a worker thread sends pending ones to `<URL>/v1/report` and moves them to `sent`,
//! where the person can read them. Reports are scrubbed here before they are written ([`scrub`]):
//! no document text, file names, paths, e-mail addresses or long numbers. The server scrubs again.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ibercorreo_ui_egui::App;
use serde_json::{Value, json};

/// The report server, set at build time (empty counts as unset).
pub const ENDPOINT: Option<&str> = match option_env!("TINTA_REPORT_URL") {
    Some(v) if !v.is_empty() => Some(v),
    _ => None,
};
/// Sent as `x-tinta-report`, so stray traffic is turned away before it costs anything.
const KEY: &str = match option_env!("TINTA_REPORT_KEY") {
    Some(v) => v,
    None => "",
};
/// The app's name on the server.
const APP: &str = "correo";
/// A window busy this long in one frame counts as not responding.
pub const FREEZE_SECS: u64 = 15;
/// Reports kept waiting to be sent, and sent ones kept to read; older ones are deleted.
const MAX_PENDING: usize = 50;
const MAX_SENT: usize = 50;
/// The server refuses more than 64 KiB.
const MAX_REPORT_BYTES: usize = 60 * 1024;
/// How often pending reports are tried while allowed.
const SEND_EVERY: Duration = Duration::from_secs(60);
/// Log lines a report carries, read from the end of at most this many bytes of the log.
const LOG_LINES: usize = 40;
const LOG_TAIL_BYTES: u64 = 16 * 1024;

static DIR: OnceLock<PathBuf> = OnceLock::new();
static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
static COMMIT: OnceLock<&'static str> = OnceLock::new();
/// 0: not asked yet, 1: allowed, 2: refused. Read by the panic hook and the watchdog.
static CONSENT: AtomicU8 = AtomicU8::new(0);
/// When the current frame phase started (ms since the epoch), 0 between phases.
static BUSY_SINCE_MS: AtomicU64 = AtomicU64::new(0);

/// `<data>/error-reports` when this build sends reports and the mailbox is saved (not the
/// sample), else `None`.
pub fn dir(data: Option<PathBuf>, persist: bool) -> Option<PathBuf> {
    ENDPOINT?;
    persist.then_some(data?.join("error-reports"))
}

/// Start: remember where reports go, add the report to the panic hook (after the logger's) and
/// start the watchdog. Call once.
pub fn install(dir: PathBuf, log_file: Option<PathBuf>, commit: &'static str) {
    if DIR.set(dir).is_err() {
        return;
    }
    if let Some(f) = log_file {
        let _ = LOG_FILE.set(f);
    }
    let _ = COMMIT.set(commit);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        let trace = std::backtrace::Backtrace::force_capture().to_string();
        let location = info.location().map(ToString::to_string).unwrap_or_default();
        let message = info.payload_as_str().unwrap_or("(no message)");
        write_report("panic", message, &location, frames(&trace), None);
    }));
    let _ = std::thread::Builder::new().name("tinta-report-watchdog".into()).spawn(watchdog);
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The UI thread started a frame.
pub fn busy() {
    BUSY_SINCE_MS.store(now_ms().max(1), Ordering::Relaxed);
}

/// The UI thread finished the frame.
pub fn idle() {
    BUSY_SINCE_MS.store(0, Ordering::Relaxed);
}

/// One report per phase that runs past [`FREEZE_SECS`]; its length is what it had reached then.
fn watchdog() {
    let mut reported = 0;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let since = BUSY_SINCE_MS.load(Ordering::Relaxed);
        let secs = now_ms().saturating_sub(since) / 1000;
        if since != 0 && since != reported && secs >= FREEZE_SECS {
            reported = since;
            write_report("freeze", "the window stopped responding", "", Vec::new(), Some(secs));
        }
    }
}

/// The meaningful lines of a backtrace: this app's own frames and where they are, without the
/// panic machinery around them.
pub fn frames(trace: &str) -> Vec<String> {
    trace
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| {
            !["std::panic", "std::panicking", "core::panicking", "rust_begin_unwind", "std::backtrace", "__rust", "std::rt::", "std::sys::"]
                .iter()
                .any(|skip| l.contains(skip))
        })
        .take(40)
        .map(scrub)
        .collect()
}

/// Remove what could identify the person or their work: text a message quotes between backticks
/// (a slice of the document), the home folder and user name, paths, e-mail addresses and runs of
/// six or more digits. A Rust source location (`crates/x/src/y.rs:12:5`) keeps its in-repo part.
pub fn scrub(text: &str) -> String {
    let mut s = String::with_capacity(text.len().min(8192));
    for (i, part) in text.split('`').enumerate() {
        s.push_str(if i % 2 == 1 { "`…`" } else { part });
    }
    for var in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(var).and_then(|h| h.into_string().ok()).filter(|h| h.len() > 3) {
            s = s.replace(&home, "~");
        }
    }
    for var in ["USER", "USERNAME", "LOGNAME"] {
        if let Some(user) = std::env::var_os(var).and_then(|h| h.into_string().ok()).filter(|u| u.chars().count() >= 3) {
            s = s.replace(&user, "<user>");
        }
    }
    let mut out = String::with_capacity(s.len());
    for (i, token) in s.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&scrub_token(token));
    }
    digits(&out)
}

fn scrub_token(t: &str) -> String {
    if let Some(at) = t.find('@')
        && t.get(at + 1..).is_some_and(|d| d.contains('.'))
    {
        return "<email>".into();
    }
    let seps = t.chars().filter(|c| matches!(c, '/' | '\\')).count();
    let is_path = seps >= 2 || t.starts_with('/') || t.starts_with("~/") || t.get(1..3) == Some(":\\");
    if !is_path {
        return t.to_string();
    }
    if t.contains(".rs:") {
        for root in ["crates/", "apps/", "crates\\", "apps\\", "src/", "src\\"] {
            if let Some(i) = t.find(root) {
                return t.get(i..).unwrap_or(t).to_string();
            }
        }
    }
    "<path>".into()
}

fn digits(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            run.push(c);
            continue;
        }
        out.push_str(if run.len() >= 6 { "<n>" } else { &run });
        run.clear();
        out.push(c);
    }
    out.push_str(if run.len() >= 6 { "<n>" } else { &run });
    out
}

/// The last [`LOG_LINES`] lines of the log, scrubbed.
fn log_tail() -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Some(path) = LOG_FILE.get() else { return Vec::new() };
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    let len = f.metadata().map_or(0, |m| m.len());
    if f.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL_BYTES))).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if f.take(LOG_TAIL_BYTES).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines.iter().skip(lines.len().saturating_sub(LOG_LINES)).map(|l| scrub(l)).collect()
}

/// The report as written (the installation id and language are added when it is sent).
pub fn report(kind: &str, message: &str, location: &str, frames: Vec<String>, seconds: Option<u64>, log: Vec<String>) -> Value {
    let mut r = json!({
        "app": APP,
        "kind": kind,
        "version": env!("CARGO_PKG_VERSION"),
        "commit": COMMIT.get().copied().unwrap_or("dev"),
        "os": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        "message": scrub(message).chars().take(2000).collect::<String>(),
        "location": scrub(location),
        "frames": frames,
        "log": log,
    });
    if let (Some(s), Some(o)) = (seconds, r.as_object_mut()) {
        o.insert("seconds".into(), json!(s));
    }
    r
}

/// Write a report into `pending`, unless the person said no. Never panics (it runs inside the
/// panic hook).
fn write_report(kind: &str, message: &str, location: &str, frames: Vec<String>, seconds: Option<u64>) {
    if CONSENT.load(Ordering::Relaxed) == 2 {
        return;
    }
    let Some(dir) = DIR.get() else { return };
    let pending = dir.join("pending");
    if std::fs::create_dir_all(&pending).is_err() {
        return;
    }
    let mut body = report(kind, message, location, frames, seconds, log_tail());
    let mut text = body.to_string();
    if text.len() > MAX_REPORT_BYTES
        && let Some(o) = body.as_object_mut()
    {
        let frames: Vec<Value> = o.get("frames").and_then(Value::as_array).map(|f| f.iter().take(10).cloned().collect()).unwrap_or_default();
        o.insert("log".into(), json!([]));
        o.insert("frames".into(), Value::Array(frames));
        text = body.to_string();
    }
    let name = format!("{}-{kind}.json", now_ms());
    let _ = std::fs::write(pending.join(name), text);
    prune(&pending, MAX_PENDING);
}

/// Keep the newest `keep` `.json` files of a folder.
fn prune(dir: &Path, keep: usize) {
    let mut files = json_files(dir);
    if files.len() > keep {
        files.sort();
        for f in files.iter().take(files.len().saturating_sub(keep)) {
            let _ = std::fs::remove_file(f);
        }
    }
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default()
}

/// A random version 4 UUID for the installation, or `None` without randomness from the system.
pub fn new_install_id() -> Option<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).ok()?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Some(format!("{}-{}-{}-{}-{}", hex.get(..8)?, hex.get(8..12)?, hex.get(12..16)?, hex.get(16..20)?, hex.get(20..)?))
}

/// What a worker thread found.
enum Outcome {
    Forgotten(String),
    Done,
}

/// The UI thread's side: follows the choice in the settings and starts the workers.
struct Reporter {
    last_send: Option<Instant>,
    working: bool,
    tx: Sender<Outcome>,
    rx: Receiver<Outcome>,
}

static REPORTER: OnceLock<Mutex<Reporter>> = OnceLock::new();

/// Each frame, from the desktop app's `logic`.
pub fn tick(app: &mut App) {
    let Some(dir) = DIR.get().cloned() else { return };
    let Some(endpoint) = ENDPOINT else { return };
    let locale = ibercorreo_ui_egui::i18n::current().code().to_string();
    let before = app.session.store.error_reports.clone();
    let prefs = &mut app.session.store.error_reports;
    CONSENT.store(
        match prefs.consent {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        },
        Ordering::Relaxed,
    );
    if prefs.consent == Some(true)
        && prefs.install_id.is_empty()
        && let Some(id) = new_install_id()
    {
        prefs.install_id = id;
    }
    let reporter = REPORTER.get_or_init(|| {
        let (tx, rx) = channel();
        Mutex::new(Reporter { last_send: None, working: false, tx, rx })
    });
    let mut r = reporter.lock().unwrap_or_else(PoisonError::into_inner);
    while let Ok(outcome) = r.rx.try_recv() {
        r.working = false;
        if let Outcome::Forgotten(id) = outcome
            && prefs.forget_pending.as_deref() == Some(id.as_str())
        {
            prefs.forget_pending = None;
        }
    }
    if *prefs != before {
        // The new id, or a finished deletion, is saved with the mailbox.
        app.session.dirty = true;
    }
    let prefs = &mut app.session.store.error_reports;
    if r.working {
        return;
    }
    let due = r.last_send.is_none_or(|t| t.elapsed() >= SEND_EVERY);
    if !due {
        return;
    }
    r.last_send = Some(Instant::now());
    let forget = prefs.forget_pending.clone();
    let send_as = (prefs.consent == Some(true) && !prefs.install_id.is_empty()).then(|| prefs.install_id.clone());
    if forget.is_none() && send_as.is_none() {
        return;
    }
    let tx = r.tx.clone();
    r.working = std::thread::Builder::new()
        .name("tinta-report-send".into())
        .spawn(move || {
            let Ok(tls) = crate::tls() else {
                let _ = tx.send(Outcome::Done);
                return;
            };
            let agent = Agent { tls };
            if let Some(id) = forget
                && post(&agent, endpoint, "/v1/forget", &json!({"install_id": id})).is_some_and(|s| (200..300).contains(&s))
            {
                let _ = tx.send(Outcome::Forgotten(id));
            }
            if let Some(id) = send_as {
                send_pending(&agent, endpoint, &dir, &id, &locale);
            }
            let _ = tx.send(Outcome::Done);
        })
        .is_ok();
}

/// The app's own HTTPS client (the one OAuth sign-in uses), trusting what the mail servers do.
struct Agent {
    tls: ibercorreo_net::Tls,
}

/// POST JSON; the status, or `None` when the server could not be reached.
fn post(agent: &Agent, endpoint: &str, path: &str, body: &Value) -> Option<u16> {
    let url = ibercorreo_net::dav::Url::parse(&format!("{}{path}", endpoint.trim_end_matches('/'))).ok()?;
    let mut client = ibercorreo_net::dav_anonymous(&url, &agent.tls);
    let headers = [("Content-Type", "application/json"), ("x-tinta-report", KEY)];
    client.send("POST", &url.path, &headers, body.to_string().as_bytes()).ok().map(|r| r.status)
}

/// Send the pending reports oldest first; stop at the first one the server can't take now.
fn send_pending(agent: &Agent, endpoint: &str, dir: &Path, id: &str, locale: &str) {
    let pending = dir.join("pending");
    let sent = dir.join("sent");
    let mut files = json_files(&pending);
    files.sort();
    for f in files.into_iter().take(10) {
        // Only reports this app wrote are sent; anything else in the folder is dropped.
        let Some(Value::Object(mut fields)) = std::fs::read(&f).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
            let _ = std::fs::remove_file(&f);
            continue;
        };
        fields.insert("install_id".into(), json!(id));
        fields.insert("locale".into(), json!(locale));
        let body = Value::Object(fields);
        match post(agent, endpoint, "/v1/report", &body) {
            // Taken, or refused for good (a malformed or oversized report): either way it is done.
            Some(s) if (200..300).contains(&s) || (400..500).contains(&s) && s != 429 => {
                if std::fs::create_dir_all(&sent).is_ok() {
                    let name = f.file_name().map(|n| n.to_owned()).unwrap_or_default();
                    let _ = std::fs::write(sent.join(name), serde_json::to_vec_pretty(&body).unwrap_or_default());
                }
                let _ = std::fs::remove_file(&f);
            }
            _ => break,
        }
    }
    prune(&sent, MAX_SENT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_keeps_source_locations_and_removes_personal_data() {
        let s = scrub(
            "cannot read `Salario Ana 2400` from C:\\Users\\juan\\Factura.eml for juan@example.es DNI 12345678Z at crates/docx/src/read.rs:44:5",
        );
        assert!(!s.contains("Salario") && !s.contains("Factura") && !s.contains("juan@") && !s.contains("12345678"), "{s}");
        assert!(s.contains("crates/docx/src/read.rs:44:5"), "{s}");
        assert_eq!(scrub("/home/ana/Documentos/carta.docx"), "<path>");
        assert_eq!(scrub("index 3 out of range for slice of length 2"), "index 3 out of range for slice of length 2");
    }

    #[test]
    fn frames_drop_the_panic_machinery() {
        let trace = "   0: std::backtrace::Backtrace::force_capture\n   1: std::panicking::begin_panic_handler\n   2: ibercorreo_mime::parse::message\n             at /home/ana/src/ibercorreo/crates/mime/src/parse.rs:120:9";
        assert_eq!(frames(trace), ["2: ibercorreo_mime::parse::message", "at crates/mime/src/parse.rs:120:9"]);
    }

    #[test]
    fn install_ids_are_v4_uuids() {
        let id = new_install_id().unwrap();
        assert!(ibercorreo_store::tinta_reports::id_ok(&id), "{id}");
        assert_eq!(id.get(14..15), Some("4"));
        assert_ne!(new_install_id(), Some(id));
    }

    #[test]
    fn reports_carry_no_install_id_until_sent() {
        let r = report("panic", "boom at /home/ana/x.eml", "crates/a/src/b.rs:1:2", vec![], None, vec![]);
        assert_eq!(r["app"], "correo");
        assert!(r.get("install_id").is_none());
        assert_eq!(r["message"], "boom at <path>");
    }

    /// A pending file that is not a report (damaged, or put there by hand) is dropped, and the
    /// sender carries on instead of panicking on it.
    #[test]
    fn malformed_pending_files_are_dropped() {
        let dir = std::env::temp_dir().join(format!("tinta-report-bad-{}", now_ms()));
        std::fs::create_dir_all(dir.join("pending")).unwrap();
        std::fs::write(dir.join("pending").join("1-panic.json"), "[1, 2]").unwrap();
        send_pending(&Agent { tls: crate::tls().unwrap() }, "http://127.0.0.1:9", &dir, "0f8fad5b-d9cb-469f-a165-70867728950e", "es");
        assert!(json_files(&dir.join("pending")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_the_newest() {
        let dir = std::env::temp_dir().join(format!("tinta-report-prune-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            std::fs::write(dir.join(format!("{i}-panic.json")), "{}").unwrap();
        }
        prune(&dir, 2);
        let mut left: Vec<String> = json_files(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["3-panic.json", "4-panic.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
