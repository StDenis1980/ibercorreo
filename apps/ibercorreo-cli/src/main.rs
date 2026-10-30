//! `ibercorreo-cli`: Tinta Correo without a window.
//!
//! ```text
//! ibercorreo-cli commands                         every command id, label, shortcut and parameters
//! ibercorreo-cli run [--store FILE | --sample] [--save] --cmd 'id={json}'...
//! ibercorreo-cli eml FILE.eml                     a message as JSON
//! ibercorreo-cli ics FILE.ics                     the events in a calendar file as JSON
//! ibercorreo-cli sync --store FILE [--account ID] [--ca CERT.der]
//!                                                 Send/Receive for one of the store's accounts
//!                                                 (the first, or ID from account.list; set it
//!                                                 with `run --cmd account.set=...`): send the
//!                                                 Outbox, then two-way IMAP sync; the password
//!                                                 (an OAuth account's refresh token) is read from
//!                                                 IBERCORREO_PASSWORD (IBERCORREO_REFRESH_TOKEN),
//!                                                 else from the system credential store, where the
//!                                                 app keeps it; never an argument
//! ibercorreo-cli autoconfig EMAIL [--ca CERT.der]  find the address's IMAP and SMTP servers; prints
//!                                                 them with the `account.set` parameters to use
//! ibercorreo-cli oauth google|microsoft [EMAIL] [--ca CERT.der]
//!                                                 sign in in the browser; prints the refresh token
//!                                                 for IBERCORREO_REFRESH_TOKEN (keep it like a
//!                                                 password)
//! ibercorreo-cli graph --store FILE [--account ID] [--ca CERT.der] [PATH]
//!                                                 a Microsoft account's Microsoft Graph: who is
//!                                                 signed in, or the JSON a GET of PATH (such as
//!                                                 `me/calendars`) returns; read-only, and signs in
//!                                                 like `sync`
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
        "usage: ibercorreo-cli --version | commands | run [--store FILE | --sample] [--save] --cmd 'id={{json}}'... | eml FILE | ics FILE | sync --store FILE [--account ID] [--ca CERT.der] | autoconfig EMAIL [--ca CERT.der] | oauth google|microsoft [EMAIL] [--ca CERT.der] | graph --store FILE [--account ID] [--ca CERT.der] [PATH]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--version" | "version") => {
            use std::io::Write;
            let _ = writeln!(std::io::stdout().lock(), "Tinta Correo {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
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
        Some("autoconfig") => autoconfig(args.get(1..).unwrap_or(&[])),
        Some("oauth") => oauth(args.get(1..).unwrap_or(&[])),
        Some("graph") => graph(args.get(1..).unwrap_or(&[])),
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
    if let Ok(inbox) = s.store.folder_by_role(s.store.first_account(), FolderRole::Inbox) {
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

fn tls_for(ca: Option<String>) -> Result<ibercorreo_net::Tls, String> {
    match ca {
        Some(path) => {
            let der = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
            ibercorreo_net::Tls::with_roots(&[der])
        }
        None => ibercorreo_net::Tls::platform(),
    }
    .map_err(|e| e.to_string())
}

fn autoconfig(args: &[String]) -> Result<(), String> {
    use ibercorreo_net::autoconfig::Source;
    use ibercorreo_net::dav::autoconfig::{Server, Socket};
    let mut email = None;
    let mut ca = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ca" => ca = Some(it.next().ok_or("--ca needs a value")?.clone()),
            other if email.is_none() && !other.starts_with("--") => email = Some(other.to_string()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let email = email.ok_or("autoconfig needs an email address")?;
    let found = ibercorreo_net::autoconfig::find(&email, &tls_for(ca)?).map_err(|tried| format!("no settings found: {}", tried.join("; ")))?;
    let server = |s: &Server| json!({ "host": s.host, "port": s.port, "security": if s.socket == Socket::StartTls { "starttls" } else { "tls" }, "user": s.username, "oauthOnly": s.oauth_only });
    let s = &found.settings;
    let (Some(imap), Some(smtp)) = (s.imap.first(), s.smtp.first()) else { return Err("no servers".into()) };
    let source = match &found.source {
        Source::Published(url) => json!(url),
        Source::Guessed => json!("guessed"),
    };
    let mut account_set = json!({
        "email": email.trim(), "user": imap.username,
        "imapHost": imap.host, "imapPort": imap.port, "imapSecurity": if imap.socket == Socket::StartTls { "starttls" } else { "tls" },
        "smtpHost": smtp.host, "smtpPort": smtp.port, "smtpSecurity": if smtp.socket == Socket::StartTls { "starttls" } else { "tls" },
    });
    if let (Some(p), Some(obj)) = (ibercorreo_core::mail::OAuthProvider::for_host(&imap.host), account_set.as_object_mut()) {
        obj.insert("oauth".into(), json!(p.id));
    }
    print(&json!({
        "provider": s.provider, "source": source,
        "imap": s.imap.iter().map(server).collect::<Vec<_>>(), "smtp": s.smtp.iter().map(server).collect::<Vec<_>>(),
        "accountSet": account_set,
    }));
    Ok(())
}

fn sync(args: &[String]) -> Result<(), String> {
    use ibercorreo_net::Tls;
    let mut store_path: Option<PathBuf> = None;
    let mut ca = None;
    let mut account_id: Option<u32> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--store" => store_path = Some(PathBuf::from(value()?)),
            "--ca" => ca = Some(value()?.clone()),
            "--account" => account_id = Some(value()?.parse::<u32>().map_err(|e| format!("--account: {e}"))?),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let store_path = store_path.ok_or("sync needs --store FILE")?;
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
    let account = match account_id {
        Some(id) => s.store.account(ibercorreo_core::AccountId(id)).cloned().map_err(|e| e.to_string())?,
        None => s.store.accounts.first().cloned().ok_or("no account: run --cmd 'account.set={...}' first")?,
    };
    let id = account.id;
    let (credential, from_store) =
        credential(&account, &|k| std::env::var(k).ok(), &|service, key| keyring::Entry::new(service, key).ok().and_then(|e| e.get_password().ok()))?;
    let job = ibercorreo_net::job::Job {
        account,
        credential: credential.clone(),
        tls,
        state: s.sync_state(id),
        outbox: s.outbox(id),
        sent_path: s.sent_path(id),
        limits: ibercorreo_sync::Limits::default(),
        calendar: s.calendar_sync_state(),
        contacts: s.contacts_sync_state(),
    };
    let finished = ibercorreo_net::job::run(&job);
    if !finished.sent.is_empty() {
        s.apply_sent(&finished.sent);
    }
    let report = finished.batch.map(|b| s.apply_sync(id, b));
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
    let mut out = json!({ "sent": finished.sent, "sync": report, "calendar": calendar, "contacts": contacts });
    // Microsoft replaces the refresh token as it renews; the old one stops working in time. One
    // from the credential store goes back there, as the app does; one from the environment is
    // printed for the caller to keep.
    if let Some(token) = credential.take_rotated() {
        let kept = from_store.is_some_and(|(service, key)| keyring::Entry::new(service, &key).is_ok_and(|e| e.set_password(&token).is_ok()));
        if let (false, Some(obj)) = (kept, out.as_object_mut()) {
            obj.insert("newRefreshToken".into(), json!(token));
        }
    }
    print(&out);
    Ok(())
}

fn graph(args: &[String]) -> Result<(), String> {
    let (mut store_path, mut ca, mut account_id, mut path) = (None, None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--store" => store_path = Some(PathBuf::from(value()?)),
            "--ca" => ca = Some(value()?.clone()),
            "--account" => account_id = Some(value()?.parse::<u32>().map_err(|e| format!("--account: {e}"))?),
            other if path.is_none() && !other.starts_with("--") => path = Some(other.to_string()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let store_path = store_path.ok_or("graph needs --store FILE")?;
    let tls = tls_for(ca)?;
    let store = Store::load(&store_path).map_err(|e| e.to_string())?;
    let account = match account_id {
        Some(id) => store.account(ibercorreo_core::AccountId(id)).cloned().map_err(|e| e.to_string())?,
        None => store.accounts.iter().find(|a| a.oauth().is_some_and(|p| p.id == "microsoft")).cloned().ok_or("no Microsoft account in the store")?,
    };
    let (credential, from_store) =
        credential(&account, &|k| std::env::var(k).ok(), &|service, key| keyring::Entry::new(service, key).ok().and_then(|e| e.get_password().ok()))?;
    let token = credential.graph_token(&tls);
    // Keep a replaced refresh token whatever Graph answers, as `sync` does.
    if let Some(new) = credential.take_rotated() {
        let kept = from_store.is_some_and(|(service, key)| keyring::Entry::new(service, &key).is_ok_and(|e| e.set_password(&new).is_ok()));
        if !kept {
            print(&json!({ "newRefreshToken": new }));
        }
    }
    let mut graph = ibercorreo_net::graph(&tls, &token.map_err(|e| e.to_string())?);
    let out = match path {
        Some(p) => graph.get(&p).map_err(|e| e.to_string())?,
        None => {
            let me = graph.me().map_err(|e| e.to_string())?;
            json!({ "name": me.name, "email": me.email })
        }
    };
    print(&out);
    Ok(())
}

/// A system credential store entry: service name and key.
type StoreEntry = (&'static str, String);

/// How `account` signs in: from the environment (`IBERCORREO_PASSWORD`, or
/// `IBERCORREO_REFRESH_TOKEN` for OAuth), else from the system credential store entry the app
/// keeps ([`ibercorreo_core::Account::credential_entry`]), which is then returned too so a
/// rotated refresh token can go back there.
fn credential(
    account: &ibercorreo_core::Account,
    env: &dyn Fn(&str) -> Option<String>,
    store: &dyn Fn(&str, &str) -> Option<String>,
) -> Result<(ibercorreo_net::oauth::Credential, Option<StoreEntry>), String> {
    let entry = account.credential_entry().ok_or("this account has no sign-in name")?;
    let var = if account.oauth().is_some() { "IBERCORREO_REFRESH_TOKEN" } else { "IBERCORREO_PASSWORD" };
    let (secret, from_store) = match env(var).filter(|v| !v.is_empty()) {
        Some(v) => (v, None),
        None => match store(entry.0, &entry.1).filter(|v| !v.is_empty()) {
            Some(v) => (v, Some(entry)),
            None => {
                return Err(match account.oauth() {
                    Some(p) => format!(
                        "no sign-in for {}: sign in once in Tinta Correo (Account settings), or set IBERCORREO_REFRESH_TOKEN (from `ibercorreo-cli oauth {}`)",
                        account.address.email, p.id
                    ),
                    None => format!(
                        "no password for {}: save it once in Tinta Correo (Account settings), or set IBERCORREO_PASSWORD",
                        account.address.email
                    ),
                });
            }
        },
    };
    let credential = match account.oauth() {
        Some(p) => ibercorreo_net::oauth::Credential::oauth(ibercorreo_net::oauth::app(p.id).map_err(|e| e.to_string())?, secret),
        None => ibercorreo_net::oauth::Credential::Password(secret),
    };
    Ok((credential, from_store))
}

fn oauth(args: &[String]) -> Result<(), String> {
    let (mut provider, mut email, mut ca) = (None, String::new(), None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ca" => ca = Some(it.next().ok_or("--ca needs a value")?.clone()),
            other if provider.is_none() && !other.starts_with("--") => provider = Some(other.to_string()),
            other if email.is_empty() && !other.starts_with("--") => email = other.to_string(),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let provider = provider.ok_or("oauth needs a provider: google or microsoft")?;
    let app = ibercorreo_net::oauth::app(&provider).map_err(|e| e.to_string())?;
    let tls = tls_for(ca)?;
    let mut open = |url: &str| {
        eprintln!("Sign in in the browser. If none opens, open this address:\n{url}");
        // No browser is fine: the address is printed.
        let _ = ibercorreo_net::oauth::open_browser(url);
        Ok(())
    };
    let tokens = ibercorreo_net::oauth::sign_in_with(&mut ibercorreo_net::oauth::Net { tls: &tls }, &app, &email, &mut open, &|| false)
        .map_err(|e| e.to_string())?;
    print(&json!({ "provider": app.provider.id, "refreshToken": tokens.refresh, "expiresIn": tokens.expires_in }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use ibercorreo_core::mail::{AccountKind, KEYRING_SERVICE, OAUTH_KEYRING_SERVICE};
    use ibercorreo_core::{Account, AccountId, Address};
    use ibercorreo_net::oauth::Credential;

    use super::credential;

    fn account(oauth: &str) -> Account {
        Account {
            id: AccountId(0),
            name: "Ana".into(),
            address: Address::parse("ana@gmail.com").unwrap(),
            kind: AccountKind::Imap {
                imap_host: "imap.gmail.com".into(),
                imap_port: 993,
                smtp_host: "smtp.gmail.com".into(),
                smtp_port: 465,
                user: String::new(),
                imap_security: Default::default(),
                smtp_security: Default::default(),
                oauth: oauth.into(),
            },
            caldav: String::new(),
            signature: String::new(),
        }
    }

    #[test]
    fn passwords_come_from_the_environment_else_the_apps_credential_store() {
        let a = account("");
        let store = |service: &str, key: &str| (service == KEYRING_SERVICE && key == "ana@gmail.com").then(|| "stored".to_string());
        let (c, from) = credential(&a, &|_| None, &store).unwrap();
        assert!(matches!(c, Credential::Password(p) if p == "stored"));
        assert_eq!(from, Some((KEYRING_SERVICE, "ana@gmail.com".to_string())));
        let env = |k: &str| (k == "IBERCORREO_PASSWORD").then(|| "env".to_string());
        let (c, from) = credential(&a, &env, &store).unwrap();
        assert!(matches!(c, Credential::Password(p) if p == "env"));
        assert_eq!(from, None);
        let err = credential(&a, &|_| None, &|_, _| None).err().unwrap();
        assert!(err.contains("Account settings"), "{err}");
        // An empty variable counts as unset.
        assert!(credential(&a, &|_| Some(String::new()), &store).unwrap().1.is_some());
    }

    #[test]
    fn oauth_refresh_tokens_are_read_where_the_app_keeps_them() {
        // The client ID comes from the build or the environment; without one there is nothing
        // to sign in with, which is an error and not a panic.
        let a = account("google");
        let store = |service: &str, key: &str| (service == OAUTH_KEYRING_SERVICE && key == "google:ana@gmail.com").then(|| "refresh".to_string());
        match credential(&a, &|_| None, &store) {
            Ok((_, from)) => assert_eq!(from, Some((OAUTH_KEYRING_SERVICE, "google:ana@gmail.com".to_string()))),
            Err(e) => assert!(e.contains("client"), "{e}"),
        }
        let err = credential(&a, &|_| None, &|_, _| None).err().unwrap();
        assert!(err.contains("IBERCORREO_REFRESH_TOKEN"), "{err}");
    }
}
