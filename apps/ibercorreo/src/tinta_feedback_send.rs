//! Tinta Office: sends what the person wrote in the feedback dialog
//! ([`ibercorreo_ui_egui::tinta_feedback`]) to `<TINTA_REPORT_URL>/v1/feedback`.
//!
//! Feedback is sent only when the person presses Send, so it needs no other consent and goes
//! whatever they answered about automatic error reports. Without a connection it waits in
//! `<settings>/error-reports/feedback` (at most [`MAX_QUEUED`]) and is tried again every
//! [`RETRY_EVERY`]; builds without `TINTA_REPORT_URL` never show the dialog.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use ibercorreo_ui_egui::tinta_feedback::{Draft, Feedback, Outcome, base64};
use serde_json::{Value, json};

use crate::tinta_report as report;

/// Feedback kept waiting for a connection; older ones are deleted.
const MAX_QUEUED: usize = 10;
const RETRY_EVERY: Duration = Duration::from_secs(120);

struct State {
    tx: Sender<Option<Outcome>>,
    rx: Receiver<Option<Outcome>>,
    working: bool,
    last_retry: Option<Instant>,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

/// Each frame, from the desktop app's `logic`.
#[allow(clippy::disallowed_methods)] // `Instant::now`: the desktop app only, never wasm.
pub fn tick(feedback: &mut Feedback, locale: &str) {
    feedback.available = report::ENDPOINT.is_some();
    let Some(endpoint) = report::ENDPOINT else { return };
    let state = STATE.get_or_init(|| {
        let (tx, rx) = channel();
        Mutex::new(State { tx, rx, working: false, last_retry: None })
    });
    let mut st = state.lock().unwrap_or_else(PoisonError::into_inner);
    while let Ok(outcome) = st.rx.try_recv() {
        st.working = false;
        if let Some(outcome) = outcome {
            feedback.finished(outcome);
        }
    }
    if st.working {
        return;
    }
    let dir = report::feedback_dir();
    let tx = st.tx.clone();
    if let Some(draft) = feedback.take_draft() {
        let body = body(&draft, locale);
        st.working = std::thread::Builder::new()
            .name("tinta-feedback-send".into())
            .spawn(move || {
                let outcome = match send(endpoint, &body) {
                    Sent::Taken(id) => Outcome::Sent(id),
                    Sent::Refused => Outcome::Failed,
                    Sent::NotNow => match dir.as_deref().map(|d| queue(d, &body)) {
                        Some(true) => Outcome::Queued,
                        _ => Outcome::Failed,
                    },
                };
                let _ = tx.send(Some(outcome));
            })
            .is_ok();
        if !st.working {
            feedback.finished(Outcome::Failed);
        }
        return;
    }
    let Some(dir) = dir else { return };
    if st.last_retry.is_some_and(|t| t.elapsed() < RETRY_EVERY) {
        return;
    }
    st.last_retry = Some(Instant::now());
    if queued(&dir).is_empty() {
        return;
    }
    st.working = std::thread::Builder::new()
        .name("tinta-feedback-retry".into())
        .spawn(move || {
            retry(endpoint, &dir);
            let _ = tx.send(None);
        })
        .is_ok();
}

/// The request body: what the dialog shows, and the technical details only when ticked.
pub fn body(d: &Draft, locale: &str) -> Value {
    let mut b = json!({
        "app": report::APP,
        "kind": d.kind.code(),
        "text": d.text,
        "version": env!("CARGO_PKG_VERSION"),
        "locale": locale,
    });
    if let Some(o) = b.as_object_mut() {
        if let Some(email) = &d.email {
            o.insert("email".into(), json!(email));
        }
        if let Some(png) = &d.screenshot_png {
            o.insert("screenshot".into(), json!(base64(png)));
        }
        if d.diagnostics {
            o.insert("diagnostics".into(), report::diagnostics());
        }
    }
    b
}

enum Sent {
    /// The server has it (and maybe gave its reference).
    Taken(Option<String>),
    /// Refused for good (malformed, too large): retrying would not help.
    Refused,
    /// No connection, a server error or too busy: try again later.
    NotNow,
}

fn send(endpoint: &str, body: &Value) -> Sent {
    match report::post_json(endpoint, "/v1/feedback", body) {
        Some((s, reply)) if (200..300).contains(&s) => Sent::Taken(reply.get("id").and_then(Value::as_str).map(|id| id.chars().take(36).collect())),
        Some((s, _)) if (400..500).contains(&s) && s != 429 => Sent::Refused,
        _ => Sent::NotNow,
    }
}

fn queued(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    files.sort();
    files
}

/// Keep the body for later; `false` when it could not be written.
#[allow(clippy::disallowed_methods)] // `SystemTime::now`: the desktop app only, never wasm.
fn queue(dir: &Path, body: &Value) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let ok = std::fs::write(dir.join(format!("{ms}.json")), body.to_string()).is_ok();
    let files = queued(dir);
    for f in files.iter().take(files.len().saturating_sub(MAX_QUEUED)) {
        let _ = std::fs::remove_file(f);
    }
    ok
}

/// Send queued feedback oldest first; stop at the first one the server can't take now.
fn retry(endpoint: &str, dir: &Path) {
    for f in queued(dir) {
        let Some(body) = std::fs::read(&f).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(Value::is_object) else {
            let _ = std::fs::remove_file(&f);
            continue;
        };
        match send(endpoint, &body) {
            Sent::Taken(_) | Sent::Refused => {
                let _ = std::fs::remove_file(&f);
            }
            Sent::NotNow => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibercorreo_ui_egui::tinta_feedback::Kind;

    fn draft() -> Draft {
        Draft { kind: Kind::Dislike, text: "El botón no hace nada".into(), email: None, screenshot_png: None, diagnostics: false }
    }

    #[test]
    fn the_body_holds_only_what_was_ticked() {
        let b = body(&draft(), "es");
        assert_eq!(b["app"], report::APP);
        assert_eq!(b["kind"], "dislike");
        assert_eq!(b["text"], "El botón no hace nada");
        assert_eq!(b["locale"], "es");
        for key in ["email", "screenshot", "diagnostics", "install_id"] {
            assert!(b.get(key).is_none(), "{key}");
        }
        let full = body(
            &Draft { email: Some("ana@example.es".into()), screenshot_png: Some(vec![0x89, b'P', b'N', b'G']), diagnostics: true, ..draft() },
            "uk",
        );
        assert_eq!(full["email"], "ana@example.es");
        assert_eq!(full["screenshot"], "iVBORw==");
        assert!(full["diagnostics"]["os"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[test]
    fn unreachable_server_keeps_the_queue_and_drops_junk() {
        let dir = std::env::temp_dir().join(format!("tinta-feedback-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(queue(&dir, &body(&draft(), "es")));
        std::fs::write(dir.join("0.json"), b"not json").unwrap();
        retry("http://127.0.0.1:9", &dir);
        let left = queued(&dir);
        assert_eq!(left.len(), 1);
        assert!(!left[0].ends_with("0.json"));
        for _ in 0..MAX_QUEUED + 3 {
            queue(&dir, &json!({"app": "x"}));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(queued(&dir).len(), MAX_QUEUED);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_server_the_dialog_stays_hidden_or_appears_by_build() {
        let mut fb = Feedback::default();
        tick(&mut fb, "es");
        assert_eq!(fb.available, report::ENDPOINT.is_some());
    }
}
