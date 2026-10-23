//! `ibercorreo-cli`: Iber Correo without a window.
//!
//! ```text
//! ibercorreo-cli commands                         every command id, label, shortcut and parameters
//! ibercorreo-cli run [--store FILE | --sample] [--save] --cmd 'id={json}'...
//! ibercorreo-cli eml FILE.eml                     a message as JSON
//! ibercorreo-cli ics FILE.ics                     the events in a calendar file as JSON
//! ibercorreo-cli sync --store FILE [--ca CERT.der] Send/Receive for the store's account (set it
//!                                                 with `run --cmd account.set=...`): send the
//!                                                 Outbox, then two-way IMAP sync; the password is
//!                                                 read from IBERCORREO_PASSWORD, never an argument
//! ```
//!
//! `run` prints each command's result as one JSON line and stops at the first error (exit code 1).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use ibercorreo_core::{Date, DateTime, FolderRole};
use ibercorreo_engine::{Session, commands};
use ibercorreo_store::Store;
use serde_json::{Value, json};

const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;

fn now() -> DateTime {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    DateTime::from_unix(i64::try_from(secs).unwrap_or(0)).unwrap_or(DateTime::midnight(Date::EPOCH))
}

fn read(path: &str) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if meta.len() > MAX_INPUT_BYTES {
        return Err(format!("{path}: larger than {MAX_INPUT_BYTES} bytes"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// One JSON line on stdout. A closed pipe (`… | head`) is not an error worth a crash.
fn print(v: &Value) {
    use std::io::Write;
    let _ = writeln!(std::io::stdout().lock(), "{}", serde_json::to_string(v).unwrap_or_else(|_| "null".into()));
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: ibercorreo-cli commands | run [--store FILE | --sample] [--save] --cmd 'id={{json}}'... | eml FILE | ics FILE | sync --store FILE [--ca CERT.der]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("commands") => {
            for c in commands::REGISTRY {
                print(&json!({ "id": c.id, "label": c.label, "group": c.group, "shortcut": c.shortcut, "params": c.params }));
            }
            Ok(())
        }
        Some("eml") => match args.get(1) {
            Some(path) => read(path).and_then(|raw| {
                let mut s = Session::new(Store::new(), now().date);
                s.run("message.importEml", &json!({ "raw": raw })).map(|v| print(&v)).map_err(|e| e.to_string())
            }),
            None => return usage(),
        },
        Some("ics") => match args.get(1) {
            Some(path) => read(path).and_then(|text| {
                let p = ibercorreo_ical_parse(&text)?;
                print(&p);
                Ok(())
            }),
            None => return usage(),
        },
        Some("run") => run(args.get(1..).unwrap_or(&[])),
        Some("sync") => sync(args.get(1..).unwrap_or(&[])),
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn ibercorreo_ical_parse(text: &str) -> Result<Value, String> {
    let mut s = Session::new(Store::new(), now().date);
    let report = s.run("calendar.importIcs", &json!({ "text": text })).map_err(|e| e.to_string())?;
    let events: Vec<Value> = s
        .store
        .events()
        .iter()
        .map(|e| json!({ "uid": e.uid, "summary": e.summary, "location": e.location, "when": e.when, "rrule": e.rrule }))
        .collect();
    Ok(json!({ "report": report, "events": events }))
}

fn run(args: &[String]) -> Result<(), String> {
    let mut store_path: Option<PathBuf> = None;
    let mut sample = false;
    let mut save = false;
    let mut cmds = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--store" => store_path = Some(PathBuf::from(it.next().ok_or("--store needs a file")?)),
            "--sample" => sample = true,
            "--save" => save = true,
            "--cmd" => cmds.push(it.next().ok_or("--cmd needs id or id={json}")?.clone()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    // The computer's zone (the TZ variable overrides it, as everywhere).
    let zone = ibercorreo_core::Zone::system();
    let today = zone.from_utc(now()).date;
    let store = match (&store_path, sample) {
        (_, true) => ibercorreo_engine::sample::store(today).map_err(|e| e.to_string())?,
        (Some(p), false) if p.exists() => Store::load(p).map_err(|e| e.to_string())?,
        _ => Store::new(),
    };
    let mut s = Session::new(store, today);
    s.set_now(now());
    s.set_system_zone(zone);
    s.day = s.today();
    s.path = store_path.clone();
    if let Ok(inbox) = s.store.folder_by_role(FolderRole::Inbox) {
        s.folder = inbox;
    }
    for c in cmds {
        let (id, params) = match c.split_once('=') {
            Some((id, json)) => (id.to_string(), serde_json::from_str::<Value>(json).map_err(|e| format!("{id}: bad JSON: {e}"))?),
            None => (c.clone(), Value::Null),
        };
        let v = s.run(&id, &params).map_err(|e| format!("{id}: {e}"))?;
        print(&v);
    }
    if save {
        if sample || store_path.is_none() {
            return Err("--save needs --store FILE".into());
        }
        s.run("file.save", &Value::Null).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn sync(args: &[String]) -> Result<(), String> {
    use ibercorreo_net::Tls;
    let mut store_path: Option<PathBuf> = None;
    let mut ca = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--store" => store_path = Some(PathBuf::from(value()?)),
            "--ca" => ca = Some(value()?.clone()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let store_path = store_path.ok_or("sync needs --store FILE")?;
    let password = std::env::var("IBERCORREO_PASSWORD").map_err(|_| "set IBERCORREO_PASSWORD to the account password")?;
    let tls = match ca {
        Some(path) => {
            let der = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
            Tls::with_roots(&[der])
        }
        None => Tls::platform(),
    }
    .map_err(|e| e.to_string())?;
    let store = if store_path.exists() { Store::load(&store_path).map_err(|e| e.to_string())? } else { Store::new() };
    let mut s = Session::new(store, now().date);
    s.set_now(now());
    s.path = Some(store_path);
    let account = s.store.accounts.first().cloned().ok_or("no account: run --cmd 'account.set={...}' first")?;
    let job = ibercorreo_net::job::Job {
        account,
        password,
        tls,
        state: s.sync_state(),
        outbox: s.outbox(),
        sent_path: s.sent_path(),
        limits: ibercorreo_sync::Limits::default(),
        calendar: s.calendar_sync_state(),
        contacts: s.contacts_sync_state(),
    };
    let finished = ibercorreo_net::job::run(&job);
    if !finished.sent.is_empty() {
        s.apply_sent(&finished.sent);
    }
    let report = finished.batch.map(|b| s.apply_sync(b));
    let calendar = finished.calendar.map(|r| r.map(|b| s.apply_calendar_sync(b)));
    let contacts = finished.contacts.map(|r| r.map(|b| s.apply_contacts_sync(b)));
    s.run("file.save", &Value::Null).map_err(|e| e.to_string())?;
    let report = report?;
    let calendar = match calendar {
        Some(Ok(r)) => json!(r),
        Some(Err(e)) => json!({ "error": e }),
        None => Value::Null,
    };
    let contacts = match contacts {
        Some(Ok(r)) => json!(r),
        Some(Err(e)) => json!({ "error": e }),
        None => Value::Null,
    };
    print(&json!({ "sent": finished.sent, "sync": report, "calendar": calendar, "contacts": contacts }));
    Ok(())
}
