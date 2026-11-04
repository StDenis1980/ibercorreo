use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;

fn sample_server(limits: Limits) -> Server {
    let today = ibercorreo_core::Zone::utc().from_utc(now()).date;
    Server::new(Mailbox::Sample(Box::new(session(ibercorreo_engine::sample::store(today).unwrap(), None))), limits)
}

fn sending() -> Limits {
    Limits { send: true, ..Limits::default() }
}

fn rpc(s: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    serde_json::from_str(&s.handle_line(&line).unwrap()).unwrap()
}

/// A tool call's result: `Ok(json)` or `Err(message)` for `isError`.
fn call(s: &mut Server, name: &str, args: Value) -> Result<Value, String> {
    let r = rpc(s, "tools/call", json!({ "name": name, "arguments": args }));
    let result = &r["result"];
    assert!(result.is_object(), "{name}: {r}");
    let text = result["content"][0]["text"].as_str().unwrap().to_string();
    if result["isError"] == true { Err(text) } else { Ok(serde_json::from_str(&text).unwrap()) }
}

fn tool_names(s: &mut Server) -> Vec<String> {
    rpc(s, "tools/list", json!({}))["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn initialize_answers_in_the_clients_version_and_lists_the_tools_with_hints() {
    let mut s = sample_server(Limits::default());
    let r = rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-03-26" }));
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert!(r["result"]["instructions"].as_str().unwrap().contains("Sending is off"));
    assert_eq!(rpc(&mut s, "initialize", json!({ "protocolVersion": "1999" }))["result"]["protocolVersion"], PROTOCOL_VERSION);
    let tools = rpc(&mut s, "tools/list", json!({}))["result"]["tools"].clone();
    let find = |n: &str| tools.as_array().unwrap().iter().find(|t| t["name"] == n).cloned();
    assert_eq!(find("list_messages").unwrap()["annotations"]["readOnlyHint"], true);
    assert_eq!(find("delete_message").unwrap()["annotations"]["destructiveHint"], true);
    assert_eq!(find("create_draft").unwrap()["annotations"]["readOnlyHint"], false);
    assert!(find("send_draft").is_none(), "sending is off unless asked for");
    // A notification has no answer; an unknown method is -32601.
    assert_eq!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), None);
    assert_eq!(rpc(&mut s, "nope", json!({}))["error"]["code"], METHOD_NOT_FOUND);
    // Every tool's commands exist.
    for t in TOOLS {
        for c in t.commands {
            assert!(commands::find(c).is_some(), "{}: {c}", t.name);
        }
    }
}

#[test]
fn read_reply_and_send_only_what_was_shown() {
    let mut s = sample_server(sending());
    let inbox = call(&mut s, "list_messages", json!({ "folder": "inbox", "limit": 2 })).unwrap();
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 2);
    assert_eq!(inbox["more"], true);
    let first = inbox["messages"][0]["id"].as_u64().unwrap();
    let m = call(&mut s, "read_message", json!({ "id": first })).unwrap();
    assert!(m["body"].as_str().unwrap().contains("presupuesto"), "{m}");
    assert!(m.get("rich").is_none());
    assert_eq!(m["attachmentList"][0]["name"], "presupuesto-t2.xlsx");
    let cut = call(&mut s, "read_message", json!({ "id": first, "maxChars": 4 })).unwrap();
    assert_eq!(cut["body"], "Hola");
    assert_eq!(cut["bodyTruncated"], true);

    let draft =
        call(&mut s, "reply_to_message", json!({ "id": first, "body": "Gracias, Marta. Lo vemos el lunes." })).unwrap()["id"].as_u64().unwrap();
    let shown = call(&mut s, "read_message", json!({ "id": draft })).unwrap();
    assert!(shown["body"].as_str().unwrap().starts_with("Gracias, Marta."), "{shown}");
    assert_eq!(shown["to"][0], "Marta Iglesias <marta@example.es>");
    let hash = shown["contentHash"].as_str().unwrap().to_string();
    assert!(hash.starts_with("sha256:") && hash.len() == 7 + 64, "{hash}");

    // Changed after it was shown: refused, and it stays a draft.
    call(&mut s, "update_draft", json!({ "id": draft, "to": "otra@example.org" })).unwrap();
    let e = call(&mut s, "send_draft", json!({ "id": draft, "contentHash": hash })).unwrap_err();
    assert!(e.contains("changed"), "{e}");
    let e = call(&mut s, "send_draft", json!({ "id": draft, "contentHash": "sha256:00" })).unwrap_err();
    assert!(e.contains("changed"), "{e}");
    let drafts = call(&mut s, "list_messages", json!({ "folder": "drafts" })).unwrap();
    assert!(drafts["messages"].as_array().unwrap().iter().any(|m| m["id"] == draft));

    // Shown again and sent: it is queued in the Outbox.
    let hash = call(&mut s, "read_message", json!({ "id": draft })).unwrap()["contentHash"].as_str().unwrap().to_string();
    let sent = call(&mut s, "send_draft", json!({ "id": draft, "contentHash": hash })).unwrap();
    assert_eq!(sent["queued"], true);
    let outbox = call(&mut s, "list_messages", json!({ "folder": "outbox" })).unwrap();
    assert!(outbox["messages"].as_array().unwrap().iter().any(|m| m["id"] == draft));
}

#[test]
fn sending_needs_the_flag_and_a_hash() {
    let mut s = sample_server(Limits::default());
    let draft = call(&mut s, "create_draft", json!({ "to": "ana@example.org", "subject": "Hola", "body": "Texto" })).unwrap()["id"].as_u64().unwrap();
    let hash = call(&mut s, "read_message", json!({ "id": draft })).unwrap()["contentHash"].as_str().unwrap().to_string();
    let e = call(&mut s, "send_draft", json!({ "id": draft, "contentHash": hash })).unwrap_err();
    assert!(e.contains("--allow-send"), "{e}");
    let mut s = sample_server(sending());
    assert!(tool_names(&mut s).contains(&"send_draft".to_string()));
    // Without a hash the call is not even made.
    let r = rpc(&mut s, "tools/call", json!({ "name": "send_draft", "arguments": { "id": draft } }));
    assert_eq!(r["error"]["code"], INVALID_PARAMS);
}

#[test]
fn the_allow_list_narrows_the_tools_and_typos_are_errors() {
    assert!(parse_allow_list("message.lsit").unwrap_err().contains("message.lsit"));
    assert!(parse_allow_list("nothing.*").is_err());
    assert!(parse_allow_list(" , # only a comment").is_err());
    let list = parse_allow_list("message.list, message.read # reading\nevent.*").unwrap();
    assert!(list.contains(&"event.add".to_string()) && list.contains(&"message.read".to_string()));
    let mut s = sample_server(Limits { commands: Some(list), send: true, ..Limits::default() });
    let names = tool_names(&mut s);
    assert!(names.contains(&"list_messages".to_string()) && names.contains(&"add_event".to_string()));
    assert!(!names.contains(&"create_draft".to_string()) && !names.contains(&"send_draft".to_string()));
    let e = call(&mut s, "create_draft", json!({ "to": "ana@example.org" })).unwrap_err();
    assert!(e.contains("--allow-commands"), "{e}");
    // From a file.
    let dir = std::env::temp_dir().join(format!("tinta-mcp-allow-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("allow.txt");
    std::fs::write(&file, "contact.*\n").unwrap();
    assert!(parse_allow_list(&format!("@{}", file.display())).unwrap().contains(&"contact.add".to_string()));
    assert!(parse_allow_list(&format!("@{}", dir.display())).is_err(), "a folder is not a list");
}

#[test]
fn answers_are_capped_and_bad_arguments_are_refused() {
    let mut s = sample_server(Limits { max_messages: 1, ..Limits::default() });
    let r = call(&mut s, "list_messages", json!({ "folder": "inbox", "limit": 1_000_000_000_000u64 })).unwrap();
    assert_eq!(r["messages"].as_array().unwrap().len(), 1);
    let r = call(&mut s, "search_messages", json!({ "query": "a" })).unwrap();
    assert!(r["messages"].as_array().unwrap().len() <= 1);
    for (name, args) in [
        ("list_messages", json!({ "folder": "inbox", "password": "x" })),
        ("list_messages", json!({ "folder": [1] })),
        ("read_message", json!({})),
        ("read_message", json!({ "id": "7" })),
        ("nope", json!({})),
    ] {
        let r = rpc(&mut s, "tools/call", json!({ "name": name, "arguments": args }));
        assert_eq!(r["error"]["code"], INVALID_PARAMS, "{name} {args}: {r}");
    }
    // Hostile values reach the engine and come back as tool errors, never a panic.
    assert!(call(&mut s, "read_message", json!({ "id": u64::MAX })).is_err());
    assert!(call(&mut s, "list_messages", json!({ "folder": 4_000_000_000u64 })).is_err());
    assert!(call(&mut s, "list_events", json!({ "from": "2026-01-01", "to": "2099-01-01" })).is_err());
    assert!(call(&mut s, "move_message", json!({ "id": 0, "folder": "nowhere" })).is_err());
    assert!(s.handle_line("{not json").unwrap().contains(&PARSE_ERROR.to_string()));
    assert!(s.handle_line("[]").unwrap().contains(&INVALID_REQUEST.to_string()));
    let mut out = Vec::new();
    s.serve(&b"\xff\xfe\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n"[..], &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("UTF-8") && text.contains("\"id\":2"), "{text}");
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tinta-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn without_a_window_the_server_works_on_the_file_and_saves() {
    let dir = temp_dir("alone");
    let path = dir.join("store.json");
    let today = ibercorreo_core::Zone::utc().from_utc(now()).date;
    ibercorreo_engine::sample::store(today).unwrap().save(&path).unwrap();
    let mut s = Server::new(Mailbox::Store { path: path.clone(), loaded: None }, Limits::default());
    let id = call(&mut s, "create_draft", json!({ "to": "ana@example.org", "subject": "Guardado" })).unwrap()["id"].as_u64().unwrap();
    let saved = Store::load(&path).unwrap();
    assert_eq!(saved.message(ibercorreo_core::MessageId(id)).unwrap().subject, "Guardado");
    // Another program saves in between: the next call reads what it saved.
    let mut other = saved.clone();
    let inbox = other.folder_by_role(other.first_account(), ibercorreo_core::FolderRole::Inbox).unwrap();
    other.remove_message(ibercorreo_core::MessageId(id)).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    other.save(&path).unwrap();
    assert!(call(&mut s, "read_message", json!({ "id": id })).is_err(), "the draft removed elsewhere is gone here too");
    assert!(call(&mut s, "list_messages", json!({ "folder": inbox.0 })).is_ok());
    // No mailbox: an error that says what to do.
    let mut none = Server::new(Mailbox::Store { path: dir.join("missing.json"), loaded: None }, Limits::default());
    assert!(call(&mut none, "list_folders", json!({})).unwrap_err().contains("open Tinta Correo once"));
}

#[test]
fn with_the_window_open_commands_run_there_and_the_file_is_left_alone() {
    let dir = temp_dir("window");
    let path = dir.join("store.json");
    let today = ibercorreo_core::Zone::utc().from_utc(now()).date;
    ibercorreo_engine::sample::store(today).unwrap().save(&path).unwrap();
    let before = std::fs::read(&path).unwrap();
    // A stand-in for the window: a session of its own that answers on the instance port.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let token = "0123456789abcdef0123456789abcdef";
    std::fs::write(instance::instance_file(&path), format!("{} {token}\n", listener.local_addr().unwrap().port())).unwrap();
    let window = std::thread::spawn(move || {
        let mut s = session(ibercorreo_engine::sample::store(today).unwrap(), None);
        let mut ran = Vec::new();
        for _ in 0..2 {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(v["token"], token);
            let (id, params) = instance::parse_command(&v).unwrap();
            let outcome = s.run(&id, &params).map_err(|e| e.to_string());
            ran.push(id);
            writeln!(&stream, "{}", instance::answer_line(&outcome)).unwrap();
        }
        ran
    });
    let mut s = Server::new(Mailbox::Store { path: path.clone(), loaded: None }, Limits::default());
    let r = call(&mut s, "create_draft", json!({ "to": "ana@example.org", "subject": "En la ventana" })).unwrap();
    let id = r["id"].as_u64().unwrap();
    assert_eq!(call(&mut s, "read_message", json!({ "id": id })).unwrap()["subject"], "En la ventana");
    assert_eq!(window.join().unwrap(), ["message.compose", "message.read"]);
    assert_eq!(std::fs::read(&path).unwrap(), before, "the window saves its mailbox; the server never wrote it");
}
