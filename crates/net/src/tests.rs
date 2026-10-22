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
    let _ = c.create(folder);
    let boxes = c.list().unwrap();
    assert!(boxes.iter().any(|m| m.name == folder), "{boxes:?}");
    assert!(boxes.iter().any(|m| m.role() == ibercorreo_core::FolderRole::Sent));

    let before = c.select(folder, false).unwrap();
    let message = "From: a@example.org\r\nTo: b@example.org\r\nSubject: Привіт\r\n\r\nТіло листа\r\n".as_bytes();
    c.append(folder, &["\\Seen"], message).unwrap();
    let s = c.select(folder, false).unwrap();
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
