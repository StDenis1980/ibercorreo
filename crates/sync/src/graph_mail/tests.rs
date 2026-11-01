use super::*;
use crate::{apply, state};
use ibercorreo_core::{AccountId, FolderRole, MessageId};
use ibercorreo_dav::Response;
use ibercorreo_store::Store;

const A: AccountId = AccountId(0);

/// Method, path, headers and body of a request.
type Asked = (String, String, Vec<(String, String)>, String);

/// Answers from routes (method, path under `/v1.0/` without its query, status, body).
#[derive(Default)]
struct Fake {
    routes: Vec<(&'static str, String, u16, String)>,
    asked: Vec<Asked>,
}

impl Fake {
    fn on(mut self, method: &'static str, path: &str, status: u16, body: &str) -> Self {
        self.routes.push((method, format!("/v1.0/{path}"), status, body.to_string()));
        self
    }
}

impl Http for Fake {
    fn send(&mut self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
        let headers = headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
        self.asked.push((method.to_string(), path.to_string(), headers, String::from_utf8_lossy(body).into_owned()));
        let bare = path.split('?').next().unwrap_or(path);
        let found = self.routes.iter().find(|(m, p, _, _)| *m == method && p == bare);
        let (status, body) = found.map_or((404, String::new()), |(_, _, s, b)| (*s, b.clone()));
        Ok(Response { status, headers: Vec::new(), body: body.into_bytes(), close: false })
    }
}

fn mime(subject: &str) -> String {
    format!("From: Ana <ana@contoso.es>\r\nSubject: {subject}\r\nDate: Thu, 22 Oct 2026 09:00:00 +0000\r\n\r\nbody of {subject}\r\n")
}

fn message(id: &str, read: bool, flagged: bool) -> Value {
    json!({ "id": id, "isRead": read, "flag": { "flagStatus": if flagged { "flagged" } else { "notFlagged" } } })
}

/// An account with an Inbox (holding `inbox`, newest first), a folder inside it, Sent Items and
/// Exchange's Outbox.
fn mailbox(inbox: &[Value]) -> Fake {
    let mut f = Fake::default()
        .on("GET", "me/mailFolders/inbox", 200, r#"{"id":"IN"}"#)
        .on("GET", "me/mailFolders/sentitems", 200, r#"{"id":"SE"}"#)
        .on("GET", "me/mailFolders/outbox", 200, r#"{"id":"OB"}"#)
        .on(
            "GET",
            "me/mailFolders",
            200,
            r#"{"value":[
                {"id":"IN","displayName":"Bandeja de entrada","childFolderCount":1},
                {"id":"SE","displayName":"Elementos enviados","childFolderCount":0},
                {"id":"OB","displayName":"Bandeja de salida","childFolderCount":0}]}"#,
        )
        .on("GET", "me/mailFolders/IN/childFolders", 200, r#"{"value":[{"id":"P1","displayName":"Proyectos","childFolderCount":0}]}"#)
        .on("GET", "me/mailFolders/IN/messages", 200, &json!({ "value": inbox }).to_string())
        .on("GET", "me/mailFolders/SE/messages", 200, r#"{"value":[]}"#)
        .on("GET", "me/mailFolders/P1/messages", 200, r#"{"value":[]}"#);
    for m in inbox {
        let id = m["id"].as_str().unwrap();
        f = f.on("GET", &format!("me/messages/{id}/$value"), 200, &mime(&format!("subject {id}")));
    }
    f
}

/// `over`'s routes first, then `base`'s.
fn with(mut over: Fake, base: Fake) -> Fake {
    over.routes.extend(base.routes);
    over
}

fn graph(f: Fake) -> Graph<Fake> {
    Graph::new(f).without_sleeping()
}

fn find(store: &Store, subject: &str) -> MessageId {
    store.all_messages().find(|m| m.subject == subject).unwrap().id
}

fn asked(g: &Graph<Fake>, method: &str, path: &str) -> Vec<String> {
    g.http().asked.iter().filter(|(m, p, _, _)| m == method && p.starts_with(&format!("/v1.0/{path}"))).map(|a| a.3.clone()).collect()
}

#[test]
fn mail_comes_over_graph_newest_first_and_changes_go_back() {
    let mut store = Store::new();
    let limits = Limits { new_per_folder: 2, ..Limits::default() };
    let mut g = graph(mailbox(&[message("m3", false, true), message("m2", true, false), message("m1", false, false)]));
    let batch = run(&mut g, &state(&store, A), &limits).unwrap();
    assert!(batch.errors.is_empty(), "{:?}", batch.errors);
    assert!(!batch.mailboxes.iter().any(|m| m.raw == "OB"), "Exchange's Outbox is left out");
    let inbox = batch.folders.iter().find(|f| f.path == "IN").unwrap();
    assert_eq!(inbox.remaining, 1, "m1 waits for the next sync; the newest come first");
    let fetched: Vec<(u32, Option<&str>)> = inbox.new.iter().map(|n| (n.uid, n.id.as_deref())).collect();
    assert_eq!(fetched, [(1, Some("m2")), (2, Some("m3"))], "numbered oldest first, as UIDs grow");
    // Every request asks for ids that survive a move.
    assert!(g.http().asked.iter().all(|(_, _, h, _)| h.iter().any(|(n, v)| n == "Prefer" && v == "IdType=\"ImmutableId\"")));

    let report = apply(&mut store, A, batch);
    assert_eq!((report.added, report.remaining, report.folders_added), (2, 1, 1));
    let ib = store.folder_by_role(A, FolderRole::Inbox).unwrap();
    assert_eq!(store.folder(ib).unwrap().remote.as_ref().map(|r| r.path.as_str()), Some("IN"), "the standard Inbox mirrors Graph's");
    let sent = store.folder_by_role(A, FolderRole::Sent).unwrap();
    assert_eq!(store.folder(sent).unwrap().remote.as_ref().map(|r| r.path.as_str()), Some("SE"));
    let projects = store.folder_by_path(A, "P1").unwrap();
    assert_eq!(store.folder(projects).unwrap().name, "Bandeja de entrada/Proyectos");
    let m3 = store.message(find(&store, "subject m3")).unwrap();
    assert_eq!((m3.seen, m3.flagged, m3.remote.as_ref().and_then(|r| r.id.as_deref())), (false, true, Some("m3")));
    assert!(store.message(find(&store, "subject m2")).unwrap().seen);

    // Next sync: m4 came in, m1 (older) follows, m2 went, m3 was read in Outlook.
    let mut g = graph(mailbox(&[message("m4", false, false), message("m3", true, true), message("m1", false, false)]));
    let batch = run(&mut g, &state(&store, A), &limits).unwrap();
    let report = apply(&mut store, A, batch);
    assert_eq!((report.added, report.removed, report.updated, report.remaining), (2, 1, 1, 0));
    let arrived: Vec<String> = report.arrived.iter().map(|id| store.message(*id).unwrap().subject.clone()).collect();
    assert_eq!(arrived, ["subject m4"], "older mail fetched later is not new mail");
    let uid = |s: &str| store.message(find(&store, s)).unwrap().remote.as_ref().map(|r| r.uid);
    assert_eq!((uid("subject m1"), uid("subject m4")), (Some(3), Some(4)), "after every number held");
    assert!(store.message(find(&store, "subject m3")).unwrap().seen);
    assert!(!store.all_messages().any(|m| m.subject == "subject m2"));

    // Changes made here: m4 flagged, m1 filed in Proyectos, m3 deleted for good.
    let (m1, m3, m4) = (find(&store, "subject m1"), find(&store, "subject m3"), find(&store, "subject m4"));
    store.message_mut(m4).unwrap().flagged = true;
    store.move_message(m1, projects).unwrap();
    store.remove_message(m3).unwrap();
    let s = state(&store, A);
    assert_eq!(s.pushes.len(), 3);
    let fake = Fake::default()
        .on("PATCH", "me/messages/m4", 200, "{}")
        .on("POST", "me/messages/m1/move", 201, r#"{"id":"m1"}"#)
        .on("POST", "me/messages/m3/permanentDelete", 404, "")
        .on("GET", "me/mailFolders/P1/messages", 200, &json!({ "value": [message("m1", false, false)] }).to_string())
        .on("GET", "me/messages/m1/$value", 200, &mime("subject m1"));
    let fake = with(fake, mailbox(&[message("m4", false, true)]));
    let mut g = graph(fake);
    let batch = run(&mut g, &s, &limits).unwrap();
    assert!(batch.pushed.iter().all(|(_, r)| r.is_ok()), "{:?}", batch.pushed);
    let patch: Value = serde_json::from_str(&asked(&g, "PATCH", "me/messages/m4")[0]).unwrap();
    assert_eq!(patch, json!({ "isRead": false, "flag": { "flagStatus": "flagged" } }));
    assert_eq!(asked(&g, "POST", "me/messages/m1/move"), [r#"{"destinationId":"P1"}"#]);
    assert_eq!(asked(&g, "POST", "me/messages/m3/permanentDelete").len(), 1, "already gone there counts as done");
    let report = apply(&mut store, A, batch);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(store.tombstones().is_empty());
    assert_eq!(store.messages_in(projects).len(), 1, "the moved message is in Proyectos once");
    assert_eq!(store.messages_in(ib).len(), 1);
    assert!(store.messages_in(ib).first().is_some_and(|m| m.flagged));
}

#[test]
fn a_folder_that_fails_alone_is_reported_and_a_refused_token_ends_the_sync() {
    let store = Store::new();
    let broken = Fake::default().on("GET", "me/mailFolders/P1/messages", 500, r#"{"error":{"code":"ErrorInternalServerError"}}"#);
    let f = with(broken, mailbox(&[message("m1", false, false)]));
    let batch = run(&mut graph(f), &state(&store, A), &Limits::default()).unwrap();
    assert_eq!(batch.errors.len(), 1, "{:?}", batch.errors);
    assert!(batch.errors[0].starts_with("Bandeja de entrada/Proyectos:"), "{:?}", batch.errors);
    assert_eq!(batch.folders.len(), 2, "the Inbox and Sent Items still synced");

    let f = with(Fake::default().on("GET", "me/mailFolders/IN/messages", 401, ""), mailbox(&[]));
    assert!(matches!(run(&mut graph(f), &state(&store, A), &Limits::default()), Err(Error::Auth(401))));
    // A message listed and gone before it was read is left for the next sync.
    let f = with(Fake::default().on("GET", "me/messages/m1/$value", 404, ""), mailbox(&[message("m1", false, false)]));
    let batch = run(&mut graph(f), &state(&store, A), &Limits::default()).unwrap();
    assert!(batch.errors.is_empty() && batch.folders.iter().all(|f| f.new.is_empty()), "{batch:?}");
}

#[test]
fn a_change_without_a_graph_id_is_reported_not_sent() {
    let s = State {
        known: vec![crate::Known { path: "IN".into(), uid_validity: UID_VALIDITY, uids: [7].into(), ids: BTreeMap::new() }],
        pushes: vec![Push::Flags { path: "IN".into(), uid: 7, seen: true, flagged: false }],
    };
    let mut g = graph(mailbox(&[]));
    let batch = run(&mut g, &s, &Limits::default()).unwrap();
    assert_eq!(batch.pushed.len(), 1);
    assert!(batch.pushed[0].1.as_ref().is_err_and(|e| e.contains("Graph id")));
    assert!(asked(&g, "PATCH", "").is_empty());
}

#[test]
fn blind_copies_go_in_a_bcc_header() {
    let raw = b"From: ana@contoso.es\r\nTo: Luis <luis@contoso.es>,\r\n Marta <MARTA@contoso.es>\r\nCc: pep@contoso.es\r\nSubject: Hola\r\n\r\nBcc: not@a.header\r\n";
    let envelope: Vec<String> = ["luis@contoso.es", "marta@contoso.es", "pep@contoso.es", "jefe@contoso.es", "bad\r\nX: y"].map(String::from).into();
    let out = String::from_utf8(with_bcc(raw, &envelope)).unwrap();
    assert!(out.starts_with("Bcc: jefe@contoso.es\r\nFrom:"), "{out}");
    assert!(out.ends_with(std::str::from_utf8(raw).unwrap()));
    // Nobody blind: the message as it was.
    assert_eq!(with_bcc(raw, &envelope[..3]), raw);
}

#[test]
fn the_outbox_goes_out_with_send_mail() {
    let out = |id: u64, to: &[&str], body: &str| Outgoing {
        id: MessageId(id),
        from: "ana@contoso.es".into(),
        to: to.iter().map(|s| s.to_string()).collect(),
        raw: format!("From: ana@contoso.es\r\nTo: luis@contoso.es\r\nSubject: {id}\r\n\r\n{body}\r\n").into_bytes(),
    };
    let mail = [out(1, &["luis@contoso.es", "jefe@contoso.es"], "hola"), out(2, &["luis@contoso.es"], &"x".repeat(MAX_SEND_BYTES))];
    let mut g = graph(Fake::default().on("POST", "me/sendMail", 202, ""));
    let sent = send(&mut g, &mail);
    assert_eq!(sent[0].outcome, Outcome::Sent { saved: true }, "Exchange files it in Sent Items");
    assert!(matches!(&sent[1].outcome, Outcome::Failed(why) if why.contains("4 MB")), "{:?}", sent[1]);
    let (_, _, headers, body) = &g.http().asked[0];
    assert!(headers.iter().any(|(n, v)| n == "Content-Type" && v == "text/plain"));
    assert_eq!(body, &ibercorreo_mime::base64(&with_bcc(&mail[0].raw, &mail[0].to)));
    assert_eq!(g.http().asked.len(), 1, "the one too large is not sent");

    // A bad message goes back to be fixed; throttling leaves it and the rest for later.
    let mail = [out(1, &["luis@contoso.es"], "a"), out(2, &["luis@contoso.es"], "b")];
    let mut g = graph(Fake::default().on("POST", "me/sendMail", 400, r#"{"error":{"code":"ErrorMimeContentInvalid"}}"#));
    assert!(send(&mut g, &mail).iter().all(|s| matches!(s.outcome, Outcome::Failed(_))));
    let mut g = graph(Fake::default().on("POST", "me/sendMail", 429, ""));
    let sent = send(&mut g, &mail);
    assert!(sent.iter().all(|s| matches!(s.outcome, Outcome::Later(_))), "{sent:?}");
    assert_eq!(g.http().asked.len(), 4, "the first tried with its retries, the second not at all");
}
