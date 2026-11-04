//! One window per mailbox. Opening a `.ics`, a `.vcf`, an `.eml`, a `.msg`, a `.pst` or a
//! `mailto:` link while Tinta Correo is open (a double click in the file manager, a link in the
//! browser) starts the program again; without this, the second start would load the same store
//! and the two windows would overwrite each other's saves.
//!
//! The first start listens on a loopback port and writes `<store>.instance` (the port and a
//! random token, readable only by the person) beside the store. A later start for the same store
//! reads that file, connects, sends the token and its files and links, and exits once the first
//! window answers `ok`; the first window imports them and comes to the front. A file left behind
//! by a crash, a port nobody answers on or an answer that isn't `ok` counts as no window: the
//! later start takes over. Anything that goes wrong in setting this up leaves the program working
//! as before, one window per start.
//!
//! The same port takes commands from `ibercorreo-cli mcp` (`ibercorreo_engine::instance`): the
//! window runs each like a click and answers with its result, so an assistant works on the mailbox
//! the person sees instead of loading it a second time.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ibercorreo_engine::instance::{answer_line, instance_file, parse_command, read_instance};
use serde_json::{Value, json};

/// What a start was asked to open.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Request {
    pub imports: Vec<PathBuf>,
    pub mailto: Vec<String>,
}

/// How long a start waits for the open window's answer, and the open window for a request.
const WAIT: Duration = Duration::from_secs(3);
/// A request longer than this isn't one a start sends (a few paths and links).
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;
/// At most this many files and links are taken from one request.
const MAX_ITEMS: usize = 256;
/// How long the window has to run a command handed over (it runs between frames).
const COMMAND_WAIT: Duration = Duration::from_secs(20);

/// A command handed over by `ibercorreo-cli mcp`, and where its outcome goes.
pub struct Command {
    pub id: String,
    pub params: Value,
    pub reply: SyncSender<Result<Value, String>>,
}

/// What arrives from outside: files and links from a later start, or a command.
pub enum Incoming {
    Open(Request),
    Command(Command),
}

/// The outcome of a start.
pub enum Start {
    /// Another window has the request: this start exits.
    Forwarded,
    /// This is the first window: requests from later starts arrive here.
    First(Inbox),
    /// The check could not be made (no data folder, no port): run as before.
    Alone,
}

/// Requests from later starts, for the window. Removes the instance file when dropped.
pub struct Inbox {
    pub rx: Receiver<Incoming>,
    /// The window sets this on its first frame, so a request wakes it.
    pub ctx: Arc<OnceLock<egui::Context>>,
    file: PathBuf,
    token: String,
}

impl Drop for Inbox {
    fn drop(&mut self) {
        // Only our own file: a later window may have taken over after a hang.
        if read_instance(&self.file).is_some_and(|(_, t)| t == self.token) {
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

/// Hands `request` to the window already open on `store`, or becomes that window.
pub fn start(store: &Path, request: &Request) -> Start {
    let file = instance_file(store);
    for attempt in 0..3 {
        match read_instance(&file) {
            Some((port, token)) => {
                if forward(port, &token, request) {
                    return Start::Forwarded;
                }
                // No window answers for it.
                let _ = std::fs::remove_file(&file);
            }
            // Unreadable even after waiting: not a start still writing it, so left by a crash.
            None if attempt > 0 => {
                let _ = std::fs::remove_file(&file);
            }
            None => {}
        }
        match listen(&file) {
            Ok(Some(inbox)) => return Start::First(inbox),
            // Another start has the file (or is writing it): look again in a moment.
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(_) => return Start::Alone,
        }
    }
    Start::Alone
}

/// Sends the request; `true` only when the window answered `ok`.
fn forward(port: u16, token: &str, request: &Request) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, WAIT) else { return false };
    let _ = stream.set_read_timeout(Some(WAIT));
    let _ = stream.set_write_timeout(Some(WAIT));
    let line = json!({
        "token": token,
        "files": request.imports.iter().map(|p| path_to_wire(&std::path::absolute(p).unwrap_or_else(|_| p.clone()))).collect::<Vec<_>>(),
        "mailto": request.mailto,
    });
    if writeln!(stream, "{line}").and_then(|()| stream.flush()).is_err() {
        return false;
    }
    let mut answer = String::new();
    let _ = BufReader::new(stream.take(16)).read_line(&mut answer);
    answer.trim() == "ok"
}

/// Becomes the window for this store: `Ok(None)` when another start wrote the file first.
fn listen(file: &Path) -> std::io::Result<Option<Inbox>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let token = new_token().ok_or_else(|| std::io::Error::other("no randomness"))?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut out = match options.open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
        Err(e) => return Err(e),
    };
    out.write_all(format!("{port} {token}\n").as_bytes())?;
    let (tx, rx) = channel();
    let ctx: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
    let (wake, expected) = (ctx.clone(), token.clone());
    std::thread::Builder::new().name("single-instance".into()).spawn(move || serve(&listener, &expected, &tx, &wake))?;
    Ok(Some(Inbox { rx, ctx, file: file.to_path_buf(), token }))
}

fn serve(listener: &TcpListener, token: &str, tx: &Sender<Incoming>, ctx: &OnceLock<egui::Context>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let _ = stream.set_read_timeout(Some(WAIT));
        let _ = stream.set_write_timeout(Some(WAIT));
        let mut line = String::new();
        if BufReader::new((&stream).take(MAX_REQUEST_BYTES)).read_line(&mut line).is_err() {
            continue;
        }
        let Some(incoming) = parse_line(&line, token) else { continue };
        match incoming {
            Parsed::Open(request) => {
                if tx.send(Incoming::Open(request)).is_err() {
                    // The window is gone.
                    return;
                }
                let _ = (&stream).write_all(b"ok\n");
                wake(ctx);
            }
            Parsed::Command(id, params) => {
                let (reply, outcome) = sync_channel(1);
                if tx.send(Incoming::Command(Command { id, params, reply })).is_err() {
                    return;
                }
                wake(ctx);
                // Waited for on its own thread, so a slow command never holds up the next start.
                let _ = std::thread::Builder::new().name("instance command".into()).spawn(move || {
                    let line = match outcome.recv_timeout(COMMAND_WAIT) {
                        Ok(outcome) => answer_line(&outcome),
                        Err(_) => String::new(),
                    };
                    let _ = (&stream).write_all(format!("{line}\n").as_bytes());
                });
            }
        }
    }
}

fn wake(ctx: &OnceLock<egui::Context>) {
    if let Some(ctx) = ctx.get() {
        ctx.request_repaint();
    }
}

/// What a request line asks for.
#[derive(Debug, PartialEq)]
enum Parsed {
    Open(Request),
    Command(String, Value),
}

/// A request line, when it carries the token: a command, or at most [`MAX_ITEMS`] files and links.
fn parse_line(line: &str, token: &str) -> Option<Parsed> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    (v.get("token")?.as_str()? == token).then_some(())?;
    if v.get("command").is_some() {
        return parse_command(&v).map(|(id, params)| Parsed::Command(id, params));
    }
    parse_request(&v).map(Parsed::Open)
}

/// The files and links of a request line.
fn parse_request(v: &Value) -> Option<Request> {
    let strings = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).take(MAX_ITEMS).map(str::to_string).collect())
            .unwrap_or_default()
    };
    let imports = strings("files").iter().filter_map(|s| path_from_wire(s)).collect();
    let mailto = strings("mailto").into_iter().filter(|l| l.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:"))).collect();
    Some(Request { imports, mailto })
}

/// 32 random hex digits.
fn new_token() -> Option<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Paths travel as hex of their bytes on Unix (they need not be UTF-8), as text elsewhere.
fn path_to_wire(p: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        p.as_os_str().as_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }
    #[cfg(not(unix))]
    {
        p.to_string_lossy().into_owned()
    }
}

fn path_from_wire(s: &str) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let bytes: Option<Vec<u8>> = (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect();
        let bytes = bytes.filter(|b| !b.is_empty())?;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        (!s.is_empty()).then(|| PathBuf::from(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tinta-single-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("store.json")
    }

    #[test]
    fn a_second_start_hands_its_files_to_the_first_window() {
        let store = temp_store("handover");
        let Start::First(inbox) = start(&store, &Request::default()) else { panic!("first start") };
        let request = Request { imports: vec![PathBuf::from("/tmp/Cita médica.ics")], mailto: vec!["mailto:ana@example.org".into()] };
        assert!(matches!(start(&store, &request), Start::Forwarded));
        assert!(matches!(inbox.rx.recv_timeout(WAIT).unwrap(), Incoming::Open(r) if r == request));
        // A plain second start (no files) only brings the window forward.
        assert!(matches!(start(&store, &Request::default()), Start::Forwarded));
        assert!(matches!(inbox.rx.recv_timeout(WAIT).unwrap(), Incoming::Open(r) if r == Request::default()));
        let file = instance_file(&store);
        drop(inbox);
        assert!(!file.exists(), "the window's file goes with it");
    }

    #[test]
    fn a_file_left_by_a_crash_is_taken_over() {
        let store = temp_store("stale");
        // A port nobody listens on any more.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        std::fs::write(instance_file(&store), format!("{port} {}\n", "ab".repeat(16))).unwrap();
        let Start::First(inbox) = start(&store, &Request::default()) else { panic!("takes over") };
        assert_ne!(read_instance(&instance_file(&store)).unwrap().1, "ab".repeat(16));
        drop(inbox);
        // Garbage in the file is no window either.
        std::fs::write(instance_file(&store), "not an instance").unwrap();
        assert!(matches!(start(&store, &Request::default()), Start::First(_)));
    }

    fn open(line: &str, token: &str) -> Option<Request> {
        match parse_line(line, token)? {
            Parsed::Open(r) => Some(r),
            Parsed::Command(..) => None,
        }
    }

    #[test]
    fn requests_need_the_token_and_are_capped() {
        let token = "0123456789abcdef0123456789abcdef";
        assert_eq!(open(&json!({"token": "wrong", "files": []}).to_string(), token), None);
        assert_eq!(open("not json", token), None);
        assert_eq!(open("", token), None);
        let many: Vec<String> = (0..1000).map(|i| format!("mailto:a{i}@example.org")).collect();
        let r = open(&json!({"token": token, "mailto": many, "files": ["zz", "", 7]}).to_string(), token).unwrap();
        assert_eq!(r.mailto.len(), MAX_ITEMS);
        assert!(r.imports.is_empty());
        let r = open(&json!({"token": token, "mailto": ["javascript:alert(1)", "MAILTO:x@example.org"]}).to_string(), token).unwrap();
        assert_eq!(r.mailto, ["MAILTO:x@example.org"]);
        let p = PathBuf::from("/home/ana/carta.eml");
        assert_eq!(path_from_wire(&path_to_wire(&p)), Some(p));
        assert_eq!(path_from_wire("abc"), None);
    }

    #[test]
    fn a_command_runs_in_the_window_and_its_result_goes_back() {
        use ibercorreo_engine::instance::{Answer, call};
        let token = "0123456789abcdef0123456789abcdef";
        assert_eq!(parse_line(&json!({"token": "wrong", "command": "folder.list"}).to_string(), token), None);
        assert_eq!(parse_line(&json!({"token": token, "command": ""}).to_string(), token), None, "a command line is never taken as files");
        let store = temp_store("command");
        let Start::First(inbox) = start(&store, &Request::default()) else { panic!("first start") };
        // The window's side: run what arrives, as the frame loop does.
        let window = std::thread::spawn(move || {
            for _ in 0..2 {
                match inbox.rx.recv_timeout(WAIT).unwrap() {
                    Incoming::Command(c) => {
                        let outcome = if c.id == "folder.list" { Ok(json!({"seen": c.params})) } else { Err(format!("{}: not found", c.id)) };
                        c.reply.send(outcome).unwrap();
                    }
                    Incoming::Open(_) => panic!("a command, not files"),
                }
            }
            inbox
        });
        let wait = Duration::from_secs(10);
        assert_eq!(call(&store, "folder.list", &json!({"x": 1}), wait), Answer::Ran(Ok(json!({"seen": {"x": 1}}))));
        assert_eq!(call(&store, "nope", &Value::Null, wait), Answer::Ran(Err("nope: not found".into())));
        drop(window.join().unwrap());
        assert_eq!(call(&store, "folder.list", &Value::Null, wait), Answer::NoWindow, "a closed window takes no commands");
    }
}
