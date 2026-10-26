//! Real TCP and TLS on 127.0.0.1 against a tiny scripted IMAP server with a generated certificate.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::*;

struct Cert {
    der: Vec<u8>,
    server: Arc<ServerConfig>,
}

fn cert(names: &[&str]) -> Cert {
    let key = rcgen::generate_simple_self_signed(names.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
    let der = key.cert.der().to_vec();
    let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()));
    let server = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(der.clone())], private)
        .unwrap();
    Cert { der, server: Arc::new(server) }
}

/// Answer tagged commands from a script: each command name maps to the lines sent back.
fn serve<S: std::io::Read + Write>(stream: S, capabilities: &str) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
        let mut words = line.split_whitespace();
        let tag = words.next().unwrap_or("*").to_owned();
        let name = words.next().unwrap_or("").to_ascii_uppercase();
        let reply = match name.as_str() {
            "CAPABILITY" => format!("* CAPABILITY {capabilities}\r\n{tag} OK done\r\n"),
            "LOGIN" => format!("{tag} OK [CAPABILITY IMAP4rev1 MOVE] logged in\r\n"),
            "LIST" => format!("* LIST (\\HasNoChildren) \"/\" INBOX\r\n* LIST (\\Sent) \"/\" Sent\r\n{tag} OK done\r\n"),
            "LOGOUT" => format!("* BYE bye\r\n{tag} OK done\r\n"),
            _ => format!("{tag} BAD unknown\r\n"),
        };
        if reader.get_mut().write_all(reply.as_bytes()).is_err() || name == "LOGOUT" {
            return;
        }
        line.clear();
    }
}

/// A server on a free port: implicit TLS, or plain with STARTTLS.
fn server(cert: &Cert, starttls: bool) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::clone(&cert.server);
    thread::spawn(move || {
        let Ok((mut tcp, _)) = listener.accept() else { return };
        if starttls {
            tcp.write_all(b"* OK [CAPABILITY IMAP4rev1 STARTTLS LOGINDISABLED] plain\r\n").unwrap();
            let mut reader = BufReader::new(tcp.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with("a1 STARTTLS"), "{line}");
            tcp.write_all(b"a1 OK begin TLS now\r\n").unwrap();
        }
        let conn = ServerConnection::new(config).unwrap();
        let mut tls = StreamOwned::new(conn, tcp);
        if !starttls && tls.write_all(b"* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] secure\r\n").is_err() {
            return;
        }
        serve(tls, "IMAP4rev1 AUTH=PLAIN");
    });
    port
}

fn endpoint(host: &str, port: u16, security: Security) -> Endpoint {
    Endpoint { host: host.into(), port, security }
}

#[test]
fn implicit_tls_session() {
    let c = cert(&["localhost"]);
    let port = server(&c, false);
    let tls = Tls::with_roots(std::slice::from_ref(&c.der)).unwrap();
    let mut client = imap(&endpoint("localhost", port, Security::Tls), &tls).unwrap();
    assert!(client.is_secure());
    client.login("denys", "secret").unwrap();
    assert!(client.has("MOVE"));
    let names: Vec<_> = client.list().unwrap().into_iter().map(|m| m.name).collect();
    assert_eq!(names, ["INBOX", "Sent"]);
    client.logout().unwrap();
}

#[test]
fn starttls_session() {
    let c = cert(&["localhost"]);
    let port = server(&c, true);
    let tls = Tls::with_roots(std::slice::from_ref(&c.der)).unwrap();
    let mut client = imap(&endpoint("localhost", port, Security::StartTls), &tls).unwrap();
    assert!(client.is_secure());
    assert!(!client.has("LOGINDISABLED"), "capabilities from before TLS are dropped");
    client.login("denys", "secret").unwrap();
}

#[test]
fn untrusted_or_mismatched_certificates_are_refused() {
    let c = cert(&["localhost"]);
    let stranger = cert(&["localhost"]);
    let port = server(&c, false);
    let tls = Tls::with_roots(&[stranger.der]).unwrap();
    assert!(matches!(imap(&endpoint("localhost", port, Security::Tls), &tls), Err(Error::Tls(_))));

    let port = server(&c, false);
    let tls = Tls::with_roots(std::slice::from_ref(&c.der)).unwrap();
    // The certificate names "localhost", not this address.
    assert!(matches!(imap(&endpoint("127.0.0.1", port, Security::Tls), &tls), Err(Error::Tls(_))));
}

#[test]
fn plain_connections_never_carry_a_password() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut tcp, _) = listener.accept().unwrap();
        tcp.write_all(b"* OK [CAPABILITY IMAP4rev1] plain\r\n").unwrap();
        let mut got = Vec::new();
        let _ = std::io::Read::read_to_end(&mut tcp, &mut got);
        got
    });
    let tls = Tls::with_roots(&[]).unwrap();
    let mut client = imap(&endpoint("127.0.0.1", port, Security::Plain), &tls).unwrap();
    assert_eq!(client.login("denys", "secret"), Err(imap::Error::Insecure));
    drop(client);
    assert_eq!(handle.join().unwrap(), b"");
}

#[test]
fn connection_failures_are_errors() {
    let tls = Tls::with_roots(&[]).unwrap();
    // A port nothing listens on.
    let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert!(matches!(imap(&endpoint("127.0.0.1", free, Security::Tls), &tls), Err(Error::Connect { .. })));
    assert!(matches!(imap(&endpoint("bad host name", 993, Security::Tls), &tls), Err(Error::Connect { .. } | Error::Host(_))));
    assert!(Tls::platform().is_ok());
}

/// Against a real server when one is configured: `IBERCORREO_TEST_IMAP="host tls-port starttls-port
/// user password cert.der"` (a throwaway local Dovecot; see `docs/testing.md`). Skips otherwise.
#[test]
fn real_server_when_configured() {
    let Ok(spec) = std::env::var("IBERCORREO_TEST_IMAP") else { return };
    let parts: Vec<&str> = spec.split_whitespace().collect();
    let [host, tls_port, starttls_port, user, password, cert] = parts.as_slice() else { panic!("bad IBERCORREO_TEST_IMAP: {spec}") };
    let tls = Tls::with_roots(&[std::fs::read(cert).unwrap()]).unwrap();

    let mut c = imap(&endpoint(host, starttls_port.parse().unwrap(), Security::StartTls), &tls).unwrap();
    c.login(user, password).unwrap();
    c.logout().unwrap();

    let mut c = imap(&endpoint(host, tls_port.parse().unwrap(), Security::Tls), &tls).unwrap();
    assert!(matches!(c.login(user, "wrong"), Err(imap::Error::No(_))));
    c.login(user, password).unwrap();
    let folder = "Iber тест & Co";
    let raw = imap::utf7::encode(folder);
    let _ = c.create(&raw);
    let boxes = c.list().unwrap();
    assert!(boxes.iter().any(|m| m.name == folder), "{boxes:?}");
    assert!(boxes.iter().any(|m| m.role() == ibercorreo_core::FolderRole::Sent));

    let before = c.select(&raw, false).unwrap();
    let message = "From: a@example.org\r\nTo: b@example.org\r\nSubject: Привіт\r\n\r\nТіло листа\r\n".as_bytes();
    c.append(&raw, &["\\Seen"], message).unwrap();
    let s = c.select(&raw, false).unwrap();
    assert_eq!(s.exists, before.exists + 1);
    let uids = c.uid_search("1:*").unwrap();
    let last = *uids.last().unwrap();
    let fetched = c.uid_fetch(&last.to_string(), imap::Items::Full).unwrap();
    let f = fetched.first().unwrap();
    assert_eq!(f.body.as_deref(), Some(message));
    assert_eq!(f.flags.as_deref(), Some(&["\\Seen".to_string()][..]));
    c.uid_store(&last.to_string(), true, &["\\Flagged"]).unwrap();
    let f = c.uid_fetch(&last.to_string(), imap::Items::Summary).unwrap();
    assert!(f.first().unwrap().flags.as_ref().unwrap().iter().any(|x| x == "\\Flagged"));
    c.uid_move(&last.to_string(), "Trash").unwrap();
    assert!(!c.uid_search("1:*").unwrap().contains(&last));
    c.logout().unwrap();
}

/// Two-way sync against the same real server: what is read, flagged, moved and deleted here
/// reaches the server, and what changes there reaches the store.
#[test]
fn real_server_sync_when_configured() {
    use ibercorreo_core::FolderRole;
    let Ok(spec) = std::env::var("IBERCORREO_TEST_IMAP") else { return };
    let parts: Vec<&str> = spec.split_whitespace().collect();
    let [host, tls_port, _, _, password, cert] = parts.as_slice() else { panic!("bad IBERCORREO_TEST_IMAP: {spec}") };
    let tls = Tls::with_roots(&[std::fs::read(cert).unwrap()]).unwrap();
    // A fresh mailbox per run (the test Dovecot accepts any user name).
    let user = format!("sync{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let connect = || {
        let mut c = imap(&endpoint(host, tls_port.parse().unwrap(), Security::Tls), &tls).unwrap();
        c.login(&user, password).unwrap();
        c
    };
    let mut c = connect();
    for n in 1..=3 {
        let raw = format!("From: a@example.org\r\nSubject: m{n}\r\nDate: Thu, 22 Oct 2026 0{n}:00:00 +0000\r\n\r\nbody {n}\r\n");
        c.append("INBOX", &[], raw.as_bytes()).unwrap();
    }
    let sync = |store: &mut ibercorreo_store::Store| {
        let mut c = connect();
        let batch =
            ibercorreo_sync::run(&mut c, &ibercorreo_sync::state(store, ibercorreo_core::AccountId(0)), &ibercorreo_sync::Limits::default()).unwrap();
        c.logout().unwrap();
        ibercorreo_sync::apply(store, ibercorreo_core::AccountId(0), batch)
    };
    let mut store = ibercorreo_store::Store::new();
    let r = sync(&mut store);
    assert_eq!((r.added, r.errors.len()), (3, 0), "{r:?}");
    let inbox = store.folder_by_role(ibercorreo_core::AccountId(0), FolderRole::Inbox).unwrap();
    let trash = store.folder_by_role(ibercorreo_core::AccountId(0), FolderRole::Trash).unwrap();
    let id = |store: &ibercorreo_store::Store, subject: &str| store.all_messages().iter().find(|m| m.subject == subject).map(|m| m.id);
    let m1 = id(&store, "m1").unwrap();
    let m2 = id(&store, "m2").unwrap();
    store.message_mut(m1).unwrap().seen = true;
    store.message_mut(m2).unwrap().flagged = true;
    store.move_message(id(&store, "m3").unwrap(), trash).unwrap();
    let r = sync(&mut store);
    assert_eq!((r.pushed, r.errors.len()), (3, 0), "{r:?}");

    c.select("INBOX", false).unwrap();
    let flags: Vec<_> = c.uid_fetch("1:*", imap::Items::Summary).unwrap().into_iter().map(|f| (f.uid.unwrap(), f.flags.unwrap())).collect();
    assert_eq!(flags, [(1, vec!["\\Seen".to_string()]), (2, vec!["\\Flagged".to_string()])]);
    // Changed on the server: the store follows.
    c.uid_store("2", false, &["\\Flagged"]).unwrap();
    let r = sync(&mut store);
    assert_eq!(r.updated, 1);
    assert!(!store.message(m2).unwrap().flagged);
    // Deleted for good here: gone from the server's Trash too.
    let in_trash = store.messages_in(trash).first().map(|m| m.id).unwrap();
    store.remove_message(in_trash).unwrap();
    sync(&mut store);
    c.select("Trash", false).unwrap();
    assert_eq!(c.uid_search("1:*").unwrap(), Vec::<u32>::new());
    assert_eq!(store.messages_in(inbox).len(), 2);
    assert!(ibercorreo_sync::state(&store, ibercorreo_core::AccountId(0)).pushes.is_empty());
    c.logout().unwrap();
}

/// SMTP over implicit TLS and STARTTLS against a scripted local server.
#[test]
fn smtp_sessions() {
    for starttls in [false, true] {
        let c = cert(&["localhost"]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = Arc::clone(&c.server);
        let server = thread::spawn(move || {
            let (mut tcp, _) = listener.accept().unwrap();
            let reply = |w: &mut dyn Write, s: &str| w.write_all(s.as_bytes()).unwrap();
            if starttls {
                reply(&mut tcp, "220 hi\r\n");
                let mut r = BufReader::new(tcp.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                reply(&mut tcp, "250-hi\r\n250 STARTTLS\r\n");
                line.clear();
                r.read_line(&mut line).unwrap();
                assert_eq!(line, "STARTTLS\r\n");
                reply(&mut tcp, "220 go\r\n");
            }
            let mut tls = StreamOwned::new(ServerConnection::new(config).unwrap(), tcp);
            if !starttls {
                reply(&mut tls, "220 hi\r\n");
            }
            let mut r = BufReader::new(tls);
            let mut got = Vec::new();
            let mut line = String::new();
            while r.read_line(&mut line).is_ok_and(|n| n > 0) {
                got.push(line.trim_end().to_owned());
                let answer = match line.split([' ', ':', '\r']).next().unwrap_or("") {
                    "EHLO" => "250-hi\r\n250 AUTH PLAIN\r\n",
                    "AUTH" => "235 ok\r\n",
                    "MAIL" | "RCPT" => "250 ok\r\n",
                    "DATA" => "354 go\r\n",
                    "." => "250 queued\r\n",
                    "QUIT" => "221 bye\r\n",
                    _ => "",
                };
                reply(r.get_mut(), answer);
                if answer.starts_with("221") {
                    break;
                }
                line.clear();
            }
            got
        });
        let tls = Tls::with_roots(std::slice::from_ref(&c.der)).unwrap();
        let security = if starttls { Security::StartTls } else { Security::Tls };
        let mut client = smtp(&endpoint("localhost", port, security), &tls, "laptop").unwrap();
        assert!(client.is_secure());
        client.login("denys", "secret").unwrap();
        client.send("denys@example.org", &["ana@example.org"], b"Subject: hi\r\n\r\nbody\r\n").unwrap();
        client.quit().unwrap();
        let got = server.join().unwrap();
        assert_eq!(got.first().map(String::as_str), Some("EHLO laptop"));
        assert!(got.contains(&"RCPT TO:<ana@example.org>".to_string()));
        assert!(got.contains(&"body".to_string()));
    }
}

/// Send through a real submission server and find the message in the recipient's real INBOX:
/// `IBERCORREO_TEST_SMTP="host port tls|starttls"` together with `IBERCORREO_TEST_IMAP` (same
/// certificate and password; see `docs/testing.md`). Skips otherwise.
#[test]
fn real_submission_when_configured() {
    let (Ok(smtp_spec), Ok(imap_spec)) = (std::env::var("IBERCORREO_TEST_SMTP"), std::env::var("IBERCORREO_TEST_IMAP")) else { return };
    let s: Vec<&str> = smtp_spec.split_whitespace().collect();
    let i: Vec<&str> = imap_spec.split_whitespace().collect();
    let ([smtp_host, smtp_port, mode], [host, tls_port, _, _, password, cert]) = (s.as_slice(), i.as_slice()) else { panic!("bad test settings") };
    let tls = Tls::with_roots(&[std::fs::read(cert).unwrap()]).unwrap();
    let security = if *mode == "tls" { Security::Tls } else { Security::StartTls };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let to = format!("rcpt{stamp}@example.org");
    let mut c = smtp(&endpoint(smtp_host, smtp_port.parse().unwrap(), security), &tls, "localhost").unwrap();
    assert!(matches!(c.login("sender@example.org", "wrong"), Err(smtp::Error::Rejected { code: 535, .. })));
    c.login("sender@example.org", password).unwrap();
    let body = format!("From: sender@example.org\r\nTo: {to}\r\nSubject: s{stamp}\r\n\r\n.dot line\r\nend\r\n");
    c.send("sender@example.org", &[&to], body.as_bytes()).unwrap();
    c.quit().unwrap();

    let mut found = None;
    for _ in 0..50 {
        let mut imap_client = imap(&endpoint(host, tls_port.parse().unwrap(), Security::Tls), &tls).unwrap();
        imap_client.login(&to, password).unwrap();
        imap_client.select("INBOX", true).unwrap();
        found = imap_client.uid_fetch("1:*", imap::Items::Full).unwrap().into_iter().find_map(|f| f.body);
        imap_client.logout().unwrap();
        if found.is_some() {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(100));
    }
    let raw = String::from_utf8(found.expect("delivered")).unwrap();
    assert!(raw.contains(&format!("Subject: s{stamp}")) && raw.contains("\r\n.dot line\r\nend"), "{raw}");
}

/// Against a real CalDAV server when one is configured:
/// `IBERCORREO_TEST_CALDAV="https://host:port/calendar/ user password cert.der"`, the address being
/// a calendar the test may fill and empty (docs/testing.md has a throwaway Radicale).
#[test]
fn real_caldav_sync_when_configured() {
    use ibercorreo_store::Store;
    use ibercorreo_sync::calendar;
    let Ok(spec) = std::env::var("IBERCORREO_TEST_CALDAV") else { return };
    let parts: Vec<&str> = spec.split_whitespace().collect();
    let [url, user, password, cert] = parts.as_slice() else { panic!("bad IBERCORREO_TEST_CALDAV: {spec}") };
    let tls = Tls::with_roots(&[std::fs::read(cert).unwrap()]).unwrap();
    let url = dav::Url::parse(url).unwrap();
    let mut client = dav(&url, &tls, user, password);
    let ics = |uid: &str, summary: &str| {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261022T000000Z\r\nDTSTART:20261102T100000Z\r\nDTEND:20261102T110000Z\r\nSUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    };
    let put = |c: &mut dyn dav::caldav::Http, name: &str, body: &str| {
        let r = c.send("PUT", &format!("{}{name}", url.path), &[("Content-Type", "text/calendar; charset=utf-8")], body.as_bytes()).unwrap();
        assert!(r.is_success(), "PUT {name}: {}", r.status);
    };
    // Start from an empty calendar.
    for name in ["iber-a.ics", "iber-b.ics", "iber-c.ics", "iber-d.ics"] {
        let _ = client.send("DELETE", &format!("{}{name}", url.path), &[], b"");
    }
    put(&mut client, "iber-a.ics", &ics("iber-a", "Planning"));
    put(&mut client, "iber-b.ics", &ics("iber-b", "Review"));

    let mut store = Store::new();
    let sync = |client: &mut dyn dav::caldav::Http, store: &mut Store| {
        let batch = calendar::run(client, &url, &calendar::state(store, ibercorreo_core::DateTime::from_unix(1_792_000_000).unwrap())).unwrap();
        calendar::apply(store, batch)
    };
    let r = sync(&mut client, &mut store);
    assert_eq!((r.calendars_added, r.added), (1, 2), "{r:?}");
    let mut names: Vec<&str> = store.events().iter().map(|e| e.summary.as_str()).collect();
    names.sort();
    assert_eq!(names, ["Planning", "Review"]);

    let r = sync(&mut client, &mut store);
    assert_eq!((r.added, r.updated, r.removed), (0, 0, 0), "{r:?}");

    put(&mut client, "iber-a.ics", &ics("iber-a", "Planning (moved)"));
    assert!(client.send("DELETE", &format!("{}iber-b.ics", url.path), &[], b"").unwrap().is_success());
    put(&mut client, "iber-c.ics", &ics("iber-c", "Retro"));
    let r = sync(&mut client, &mut store);
    assert_eq!((r.added, r.updated, r.removed), (1, 1, 1), "{r:?}");
    let mut names: Vec<&str> = store.events().iter().map(|e| e.summary.as_str()).collect();
    names.sort();
    assert_eq!(names, ["Planning (moved)", "Retro"]);

    // Changes made here go to the server: an edit, a new appointment, a deletion.
    let id_of = |store: &Store, summary: &str| store.events().iter().find(|e| e.summary == summary).unwrap().id;
    let moved = id_of(&store, "Planning (moved)");
    store.event_mut(moved).unwrap().summary = "Planning (room 2)".into();
    let mut lunch = store.event(moved).unwrap().clone();
    lunch.uid = "iber-d".into();
    lunch.summary = "Lunch".into();
    lunch.remote = None;
    store.add_event(lunch).unwrap();
    store.remove_event(id_of(&store, "Retro")).unwrap();
    let r = sync(&mut client, &mut store);
    assert_eq!((r.sent, r.conflicts), (3, 0), "{r:?}");
    assert!(r.errors.is_empty(), "{r:?}");
    let mut names: Vec<&str> = store.events().iter().map(|e| e.summary.as_str()).collect();
    names.sort();
    assert_eq!(names, ["Lunch", "Planning (room 2)"]);
    // A fresh store sees the same on the server.
    let mut fresh = Store::new();
    sync(&mut client, &mut fresh);
    let mut names: Vec<&str> = fresh.events().iter().map(|e| e.summary.as_str()).collect();
    names.sort();
    assert_eq!(names, ["Lunch", "Planning (room 2)"]);
    let _ = client.send("DELETE", &format!("{}iber-d.ics", url.path), &[], b"");

    let mut wrong = dav(&url, &tls, user, "wrong");
    assert!(matches!(calendar::run(&mut wrong, &url, &calendar::State::default()), Err(dav::Error::Auth(401))));
}

/// Against a real CardDAV server when one is configured:
/// `IBERCORREO_TEST_CARDDAV="https://host:port/addressbook/ user password cert.der"`, the address
/// being an address book the test may fill and empty (docs/testing.md has a throwaway Radicale).
#[test]
fn real_carddav_sync_when_configured() {
    use ibercorreo_core::Contact;
    use ibercorreo_store::Store;
    use ibercorreo_sync::contacts;
    let Ok(spec) = std::env::var("IBERCORREO_TEST_CARDDAV") else { return };
    let parts: Vec<&str> = spec.split_whitespace().collect();
    let [url, user, password, cert] = parts.as_slice() else { panic!("bad IBERCORREO_TEST_CARDDAV: {spec}") };
    let tls = Tls::with_roots(&[std::fs::read(cert).unwrap()]).unwrap();
    let url = dav::Url::parse(url).unwrap();
    let mut client = dav(&url, &tls, user, password);
    let vcf = |uid: &str, name: &str| format!("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{name}\r\nN:;{name};;;\r\nEND:VCARD\r\n");
    let put = |c: &mut dyn dav::caldav::Http, name: &str, body: &str| {
        let r = c.send("PUT", &format!("{}{name}", url.path), &[("Content-Type", "text/vcard; charset=utf-8")], body.as_bytes()).unwrap();
        assert!(r.is_success(), "PUT {name}: {}", r.status);
    };
    for name in ["iber-a.vcf", "iber-b.vcf", "iber-c.vcf"] {
        let _ = client.send("DELETE", &format!("{}{name}", url.path), &[], b"");
    }
    put(&mut client, "iber-a.vcf", &vcf("iber-a", "Ana"));
    put(&mut client, "iber-b.vcf", &vcf("iber-b", "Rui"));

    let names = |store: &Store| {
        let mut v: Vec<String> = store.all_contacts().iter().filter(|c| c.remote.is_some()).map(|c| c.name.clone()).collect();
        v.sort();
        v
    };
    let mut store = Store::new();
    let sync = |client: &mut dyn dav::caldav::Http, store: &mut Store| {
        let batch = contacts::run(client, &url, &contacts::state(store)).unwrap();
        contacts::apply(store, batch)
    };
    let r = sync(&mut client, &mut store);
    assert_eq!((r.books_added, r.added), (1, 2), "{r:?}");
    assert_eq!(names(&store), ["Ana", "Rui"]);

    // Changes made here go to the server: an edit, a new contact, a deletion.
    let id_of = |store: &Store, name: &str| store.all_contacts().iter().find(|c| c.name == name).unwrap().id;
    store.contact_mut(id_of(&store, "Ana")).unwrap().title = Some("Diretora".into());
    let book = store.book_by_href(&url.path).unwrap();
    let mut eva = Contact::new("Eva");
    eva.uid = "iber-c".into();
    eva.book = book;
    store.add_contact(eva);
    store.remove_contact(id_of(&store, "Rui")).unwrap();
    let r = sync(&mut client, &mut store);
    assert_eq!((r.sent, r.conflicts), (3, 0), "{r:?}");
    assert!(r.errors.is_empty(), "{r:?}");
    assert_eq!(names(&store), ["Ana", "Eva"]);
    let mut fresh = Store::new();
    sync(&mut client, &mut fresh);
    assert_eq!(names(&fresh), ["Ana", "Eva"]);
    assert!(fresh.all_contacts().iter().any(|c| c.title.as_deref() == Some("Diretora")));
    for name in ["iber-a.vcf", "iber-c.vcf"] {
        let _ = client.send("DELETE", &format!("{}{name}", url.path), &[], b"");
    }
}

#[test]
fn autoconfig_probes_and_fetches_over_real_tls() {
    use crate::autoconfig::{Net, Probe};
    use crate::dav::autoconfig::{Server, Socket};
    let c = cert(&["localhost"]);
    let tls = Tls::with_roots(std::slice::from_ref(&c.der)).unwrap();
    let at = |port| Server { host: "localhost".into(), port, socket: Socket::Tls, username: String::new(), oauth_only: false };
    let mut net = Net { tls: &tls };
    assert!(net.answers(&at(server(&c, false)), false), "an IMAP server with a trusted certificate answers");
    let other = cert(&["localhost"]);
    assert!(!net.answers(&at(server(&other, false)), false), "an untrusted certificate does not count");
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert!(!net.answers(&at(closed), true));

    // A configuration file over HTTPS, fetched without credentials.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::clone(&c.server);
    let seen = thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let mut tls = StreamOwned::new(ServerConnection::new(config).unwrap(), tcp);
        let mut head = String::new();
        let mut reader = BufReader::new(&mut tls);
        while reader.read_line(&mut head).unwrap() > 2 && !head.ends_with("\r\n\r\n") {}
        let body = "<clientConfig/>";
        let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        tls.write_all(reply.as_bytes()).unwrap();
        tls.flush().unwrap();
        head
    });
    let (status, _, body) = net.get(&format!("https://localhost:{port}/mail/config-v1.1.xml")).unwrap();
    assert_eq!((status, body.as_slice()), (200, &b"<clientConfig/>"[..]));
    let head = seen.join().unwrap();
    assert!(head.starts_with("GET /mail/config-v1.1.xml HTTP/1.1\r\n") && !head.contains("Authorization"), "{head}");
    assert!(net.get("http://localhost/x").is_err(), "plain HTTP is refused");
}

/// IDLE against the same real server: a message put in the Inbox from another connection wakes
/// the watch within seconds, and stopping ends it.
#[test]
fn real_idle_when_configured() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let Ok(spec) = std::env::var("IBERCORREO_TEST_IMAP") else { return };
    let parts: Vec<&str> = spec.split_whitespace().collect();
    let [host, tls_port, _, user, password, cert] = parts.as_slice() else { panic!("bad IBERCORREO_TEST_IMAP: {spec}") };
    let tls = Tls::with_roots(&[std::fs::read(cert).unwrap()]).unwrap();
    let at = endpoint(host, tls_port.parse().unwrap(), Security::Tls);
    let (w_at, w_tls, w_user, w_password) = (at.clone(), tls.clone(), user.to_string(), password.to_string());
    let started = std::time::Instant::now();
    let watcher = thread::spawn(move || idle::watch(&w_at, &w_tls, &w_user, &oauth::Secret::Password(w_password), "INBOX", &|| false));
    thread::sleep(std::time::Duration::from_secs(2));
    let mut c = imap(&at, &tls).unwrap();
    c.login(user, password).unwrap();
    c.append("INBOX", &[], b"From: a@example.org\r\nSubject: idle\r\n\r\nnew\r\n").unwrap();
    c.logout().unwrap();
    assert_eq!(watcher.join().unwrap().unwrap(), idle::Wake::Changed);
    assert!(started.elapsed() < std::time::Duration::from_secs(15), "woke in {:?}", started.elapsed());

    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let (w_stop, w_user, w_password) = (stop.clone(), user.to_string(), password.to_string());
    let watcher =
        thread::spawn(move || idle::watch(&at, &tls, &w_user, &oauth::Secret::Password(w_password), "INBOX", &|| w_stop.load(Ordering::Relaxed)));
    thread::sleep(std::time::Duration::from_secs(1));
    stop.store(true, Ordering::Relaxed);
    let stopping = std::time::Instant::now();
    assert_eq!(watcher.join().unwrap().unwrap(), idle::Wake::Stopped);
    assert!(stopping.elapsed() <= idle::SLICE + std::time::Duration::from_secs(2));
}

/// The token request over real TLS: a form `POST`, no credentials header, the JSON answer read.
#[test]
fn oauth_token_requests_over_real_tls() {
    use crate::oauth::{Net, Post};
    let c = cert(&["localhost"]);
    let tls = Tls::with_roots(std::slice::from_ref(&c.der)).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::clone(&c.server);
    let seen = thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let mut tls = StreamOwned::new(ServerConnection::new(config).unwrap(), tcp);
        let mut reader = BufReader::new(&mut tls);
        let mut head = String::new();
        while reader.read_line(&mut head).unwrap() > 2 && !head.ends_with("\r\n\r\n") {}
        let length: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).unwrap().trim().parse().unwrap();
        let mut body = vec![0; length];
        std::io::Read::read_exact(&mut reader, &mut body).unwrap();
        let answer = r#"{"access_token":"A1","expires_in":3600,"token_type":"Bearer"}"#;
        let reply =
            format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
        tls.write_all(reply.as_bytes()).unwrap();
        tls.flush().unwrap();
        (head, String::from_utf8(body).unwrap())
    });
    let (status, body) = Net { tls: &tls }.post(&format!("https://localhost:{port}/token"), "grant_type=refresh_token&refresh_token=R").unwrap();
    assert_eq!(status, 200);
    assert!(String::from_utf8(body).unwrap().contains("A1"));
    let (head, form) = seen.join().unwrap();
    assert!(head.starts_with("POST /token HTTP/1.1\r\n") && head.contains("Content-Type: application/x-www-form-urlencoded"), "{head}");
    assert!(!head.contains("Authorization"), "{head}");
    assert_eq!(form, "grant_type=refresh_token&refresh_token=R");
}
