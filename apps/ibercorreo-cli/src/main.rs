//! `ibercorreo-cli`: Iber Correo without a window.
//!
//! ```text
//! ibercorreo-cli commands                         every command id, label, shortcut and parameters
//! ibercorreo-cli run [--store FILE | --sample] [--save] --cmd 'id={json}'...
//! ibercorreo-cli eml FILE.eml                     a message as JSON
//! ibercorreo-cli ics FILE.ics                     the events in a calendar file as JSON
//! ibercorreo-cli sync --store FILE --host H --user U [--port N] [--security tls|starttls] [--ca CERT.der]
//!                                                 two-way IMAP sync of a store file; the password
//!                                                 is read from IBERCORREO_PASSWORD, never an argument
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

fn print(v: &Value) {
    println!("{}", serde_json::to_string(v).unwrap_or_else(|_| "null".into()));
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: ibercorreo-cli commands | run [--store FILE | --sample] [--save] --cmd 'id={{json}}'... | eml FILE | ics FILE | sync --store FILE --host H --user U [--port N] [--security tls|starttls] [--ca CERT.der]"
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
    let today = now().date;
    let store = match (&store_path, sample) {
        (_, true) => ibercorreo_engine::sample::store(today).map_err(|e| e.to_string())?,
        (Some(p), false) if p.exists() => Store::load(p).map_err(|e| e.to_string())?,
        _ => Store::new(),
    };
    let mut s = Session::new(store, today);
    s.set_now(now());
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
    use ibercorreo_net::{Endpoint, Security, Tls};
    let mut store_path: Option<PathBuf> = None;
    let mut host = None;
    let mut user = None;
    let mut port = None;
    let mut security = Security::Tls;
    let mut ca = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--store" => store_path = Some(PathBuf::from(value()?)),
            "--host" => host = Some(value()?.clone()),
            "--user" => user = Some(value()?.clone()),
            "--port" => port = Some(value()?.parse::<u16>().map_err(|e| format!("--port: {e}"))?),
            "--ca" => ca = Some(value()?.clone()),
            "--security" => {
                security = match value()?.as_str() {
                    "tls" => Security::Tls,
                    "starttls" => Security::StartTls,
                    other => return Err(format!("--security: {other:?} is not tls or starttls")),
                }
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let store_path = store_path.ok_or("sync needs --store FILE")?;
    let host = host.ok_or("sync needs --host")?;
    let user = user.ok_or("sync needs --user")?;
    let password = std::env::var("IBERCORREO_PASSWORD").map_err(|_| "set IBERCORREO_PASSWORD to the account password")?;
    let port = port.unwrap_or(if security == Security::Tls { 993 } else { 143 });
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
    let mut client = ibercorreo_net::imap(&Endpoint { host, port, security }, &tls).map_err(|e| e.to_string())?;
    client.login(&user, &password).map_err(|e| e.to_string())?;
    let batch = ibercorreo_sync::run(&mut client, &s.sync_state(), &ibercorreo_sync::Limits::default()).map_err(|e| e.to_string())?;
    let _ = client.logout();
    let report = s.apply_sync(batch);
    s.run("file.save", &Value::Null).map_err(|e| e.to_string())?;
    print(&json!(report));
    Ok(())
}
