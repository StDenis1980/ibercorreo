//! One window per mailbox: where the mailbox lives, and how another program finds the window
//! open on it and hands it a command.
//!
//! The window open on a store listens on a loopback port and writes `<store>.instance` beside the
//! store: the port and a random token, readable only by the person. Whoever can read that file
//! can already read the store, so the token adds no new access; it keeps other people's programs
//! on the same computer out. A later start of the app hands its files and links over this way
//! (`apps/ibercorreo`, `single_instance`), and `ibercorreo-cli mcp` runs its commands in the open
//! window instead of loading the store a second time, so two programs never save over each other.
//!
//! The wire is one JSON line each way. A command: `{"token", "command", "params"}`, answered with
//! `{"ok": true, "result"}` or `{"ok": false, "error"}`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// The store's folder name on Windows and macOS. Like the credential store's service name it
/// keeps the app's first name, so the mail saved before the rename stays where the app looks.
pub const DATA_DIR_NAME: &str = "Iber Correo";

/// Longest answer read from a window (a long message list, a long message).
pub const MAX_ANSWER_BYTES: u64 = 64 * 1024 * 1024;

/// Where the app keeps its mailbox and settings: `%APPDATA%\Iber Correo`, `~/Library/Application
/// Support/Iber Correo`, `$XDG_DATA_HOME/ibercorreo` (`~/.local/share/ibercorreo`).
pub fn data_dir() -> Option<PathBuf> {
    data_dir_from(&|k| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from))
}

fn data_dir_from(env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        env("APPDATA").map(|p| p.join(DATA_DIR_NAME))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|p| p.join("Library/Application Support").join(DATA_DIR_NAME))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share"))).map(|p| p.join("ibercorreo"))
    }
}

/// The mailbox the app opens when it is given none.
pub fn default_store() -> Option<PathBuf> {
    data_dir().map(|d| d.join("store.json"))
}

/// The instance file of a store.
pub fn instance_file(store: &Path) -> PathBuf {
    let mut name = store.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".instance");
    store.with_file_name(name)
}

/// `port token` from an instance file; `None` for a missing, unreadable or malformed one.
pub fn read_instance(file: &Path) -> Option<(u16, String)> {
    let mut text = String::new();
    std::fs::File::open(file).ok()?.take(256).read_to_string(&mut text).ok()?;
    let (port, token) = text.trim().split_once(' ')?;
    let token = token.trim();
    (token.len() >= 32 && token.bytes().all(|b| b.is_ascii_hexdigit())).then_some(())?;
    Some((port.parse().ok()?, token.to_string()))
}

/// What a window answered to a command.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The window ran it: its result, or the command's error.
    Ran(Result<Value, String>),
    /// No window is open on the store (no instance file, or nobody answers on its port).
    NoWindow,
    /// A window took the line but did not answer like one that runs commands (an older version,
    /// or it did not get to it in time).
    Failed(String),
}

/// Runs command `id` in the window open on `store`, waiting at most `wait` for its answer.
pub fn call(store: &Path, id: &str, params: &Value, wait: Duration) -> Answer {
    let Some((port, token)) = read_instance(&instance_file(store)) else { return Answer::NoWindow };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(3)) else { return Answer::NoWindow };
    let _ = stream.set_read_timeout(Some(wait));
    let _ = stream.set_write_timeout(Some(wait));
    let line = json!({ "token": token, "command": id, "params": params });
    if writeln!(stream, "{line}").and_then(|()| stream.flush()).is_err() {
        return Answer::Failed("the open Tinta Correo window did not take the command".into());
    }
    let mut answer = Vec::new();
    if let Err(e) = BufReader::new(stream.take(MAX_ANSWER_BYTES)).read_until(b'\n', &mut answer) {
        return Answer::Failed(format!("the open Tinta Correo window did not answer: {e}"));
    }
    parse_answer(&answer)
}

fn parse_answer(line: &[u8]) -> Answer {
    let text = String::from_utf8_lossy(line);
    let text = text.trim();
    if text.is_empty() {
        return Answer::Failed("the open Tinta Correo window did not answer in time; the command may still run there".into());
    }
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        // `ok` alone: a window from before commands could be handed over.
        return Answer::Failed("the open Tinta Correo window is an older version that cannot run commands: update it, or close it".into());
    };
    match v.get("ok").and_then(Value::as_bool) {
        Some(true) => Answer::Ran(Ok(v.get("result").cloned().unwrap_or(Value::Null))),
        Some(false) => Answer::Ran(Err(v.get("error").and_then(Value::as_str).unwrap_or("the command failed").to_string())),
        None => Answer::Failed("the open Tinta Correo window answered something unexpected".into()),
    }
}

/// A command a request line carries, when it has the token: its id and parameters.
pub fn parse_command(v: &Value) -> Option<(String, Value)> {
    let id = v.get("command")?.as_str()?;
    (!id.is_empty() && id.len() <= 128).then_some(())?;
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    Some((id.to_string(), params))
}

/// The answer line for a command's outcome.
pub fn answer_line(outcome: &Result<Value, String>) -> String {
    match outcome {
        Ok(v) => json!({ "ok": true, "result": v }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn a_command_goes_to_the_window_and_its_answer_comes_back() {
        let dir = std::env::temp_dir().join(format!("tinta-instance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("store.json");
        assert_eq!(call(&store, "folder.list", &Value::Null, Duration::from_secs(1)), Answer::NoWindow);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let token = "0123456789abcdef0123456789abcdef";
        std::fs::write(instance_file(&store), format!("{} {token}\n", listener.local_addr().unwrap().port())).unwrap();
        let server = std::thread::spawn(move || {
            for answer in [Ok(json!([1, 2])), Err("no such message".to_string())] {
                let (stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let v: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(v["token"], token);
                assert_eq!(parse_command(&v), Some(("message.list".to_string(), json!({"limit": 2}))));
                writeln!(&stream, "{}", answer_line(&answer)).unwrap();
            }
        });
        let wait = Duration::from_secs(5);
        assert_eq!(call(&store, "message.list", &json!({"limit": 2}), wait), Answer::Ran(Ok(json!([1, 2]))));
        assert_eq!(call(&store, "message.list", &json!({"limit": 2}), wait), Answer::Ran(Err("no such message".into())));
        server.join().unwrap();
        // Nobody listens any more.
        assert_eq!(call(&store, "message.list", &Value::Null, Duration::from_secs(1)), Answer::NoWindow);
    }

    #[test]
    fn answers_that_are_not_a_commands_are_failures() {
        assert!(matches!(parse_answer(b"ok\n"), Answer::Failed(e) if e.contains("older")));
        assert!(matches!(parse_answer(b""), Answer::Failed(e) if e.contains("in time")));
        assert!(matches!(parse_answer(b"{\"x\":1}"), Answer::Failed(_)));
        assert_eq!(parse_answer(b"{\"ok\":false}"), Answer::Ran(Err("the command failed".into())));
        assert_eq!(parse_command(&json!({"command": ""})), None);
        assert_eq!(parse_command(&json!({"command": 7})), None);
        assert_eq!(parse_command(&json!({"files": []})), None);
    }

    #[test]
    fn the_mailbox_is_where_the_app_keeps_it() {
        let env = |k: &str| match k {
            "HOME" => Some(PathBuf::from("/home/ana")),
            "APPDATA" => Some(PathBuf::from("C:/Users/ana/AppData/Roaming")),
            _ => None,
        };
        let dir = data_dir_from(&env).unwrap();
        assert!(dir.ends_with("ibercorreo") || dir.ends_with(DATA_DIR_NAME), "{}", dir.display());
        assert_eq!(data_dir_from(&|_| None), None);
        assert_eq!(instance_file(Path::new("/x/store.json")), PathBuf::from("/x/store.json.instance"));
        assert_eq!(read_instance(Path::new("/nonexistent/store.json.instance")), None);
    }
}
