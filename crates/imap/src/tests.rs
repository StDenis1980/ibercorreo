//! The client against a scripted server: what it sends, byte for byte, and what it makes of the replies.

use std::io::{Read, Write};

use super::*;

/// A server whose replies are written in advance. Everything the client sends is recorded.
struct Script {
    replies: std::io::Cursor<Vec<u8>>,
    sent: Vec<u8>,
}

impl Script {
    fn new(replies: &str) -> Self {
        Script { replies: std::io::Cursor::new(replies.replace('\n', "\r\n").into_bytes()), sent: Vec::new() }
    }

    fn bytes(replies: Vec<u8>) -> Self {
        Script { replies: std::io::Cursor::new(replies), sent: Vec::new() }
    }
}

impl Read for Script {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Hand out small pieces so responses arrive split across reads, as on a network.
        let n = buf.len().min(7);
        self.replies.read(buf.get_mut(..n).unwrap_or(&mut []))
    }
}

impl Write for Script {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sent.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn sent(c: Client<Script>) -> String {
    let s = c.into_stream();
    String::from_utf8(s.sent).unwrap().replace("\r\n", "\n")
}

#[test]
fn a_session() {
    let server = Script::new(
        "* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] ready
a1 OK [CAPABILITY IMAP4rev1 UIDPLUS SPECIAL-USE] logged in
* LIST (\\HasNoChildren) \"/\" INBOX
* LIST (\\HasNoChildren \\Sent) \"/\" \"Sent Items\"
* LIST (\\Noselect \\HasChildren) \"/\" Work
a2 OK done
* 2 EXISTS
* 0 RECENT
* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)
* OK [UIDVALIDITY 3857529045] UIDs valid
* OK [UIDNEXT 4392] Predicted next UID
a3 OK [READ-WRITE] SELECT completed
* SEARCH 4390 4391
a4 OK done
* 1 FETCH (UID 4390 FLAGS (\\Seen) RFC822.SIZE 20 INTERNALDATE \"22-Oct-2026 09:00:00 +0200\" BODY[] {20}
Subject: hi

Hello)
* 2 FETCH (FLAGS (\\Seen))
a5 OK done
a6 OK done
a7 OK copied
a8 OK stored
a9 OK expunged
+ go ahead
a10 OK [APPENDUID 3857529045 7] appended
* BYE logging out
a11 OK bye
",
    );
    let mut c = Client::new(server, true).unwrap();
    assert!(c.has("auth=plain"));
    c.login("denys@example.org", "s3cret \"pw\"").unwrap();
    assert!(c.has("UIDPLUS") && !c.has("AUTH=PLAIN"), "capabilities after login replace the ones before");

    let boxes = c.list().unwrap();
    let roles: Vec<_> = boxes.iter().map(|m| (m.name.as_str(), m.role(), m.selectable())).collect();
    assert_eq!(roles, [("INBOX", FolderRole::Inbox, true), ("Sent Items", FolderRole::Sent, true), ("Work", FolderRole::Other, false)]);

    let s = c.select("INBOX", false).unwrap();
    assert_eq!((s.exists, s.uid_validity, s.uid_next, s.flags.len(), s.read_only), (2, Some(3857529045), Some(4392), 5, false));

    assert_eq!(c.uid_search("4390:*").unwrap(), [4390, 4391]);
    let fetched = c.uid_fetch("4390", Items::Full).unwrap();
    assert_eq!(fetched.len(), 1, "a FETCH without a UID is an unsolicited flag update, not a result");
    let f = fetched.first().unwrap();
    assert_eq!(f.body.as_deref(), Some(&b"Subject: hi\r\n\r\nHello"[..]));
    assert_eq!(f.internal_date.as_deref(), Some("22-Oct-2026 09:00:00 +0200"));

    c.uid_store("4390", true, &["\\Flagged"]).unwrap();
    c.uid_move("4390:4391", &utf7::encode("Архів")).unwrap();
    c.append("Sent Items", &["\\Seen"], b"Subject: x\r\n\r\nbody").unwrap();
    let _ = c.run(&Command::new("LOGOUT")).unwrap();

    let arch = utf7::encode("Архів");
    assert_eq!(
        sent(c),
        format!(
            "a1 LOGIN denys@example.org \"s3cret \\\"pw\\\"\"
a2 LIST \"\" \"*\"
a3 SELECT INBOX
a4 UID SEARCH UID 4390:*
a5 UID FETCH 4390 (UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[])
a6 UID STORE 4390 +FLAGS.SILENT (\\Flagged)
a7 UID COPY 4390:4391 {arch}
a8 UID STORE 4390:4391 +FLAGS.SILENT (\\Deleted)
a9 UID EXPUNGE 4390:4391
a10 APPEND \"Sent Items\" (\\Seen) {{18}}
Subject: x

body
a11 LOGOUT
"
        )
    );
}

#[test]
fn literal_plus_and_move() {
    let server = Script::new(
        "* PREAUTH [CAPABILITY IMAP4rev2 LITERAL+ MOVE] welcome back
a1 OK appended
a2 OK moved
",
    );
    let mut c = Client::new(server, false).unwrap();
    assert!(c.is_authenticated());
    c.append("Drafts", &[], "Тема".as_bytes()).unwrap();
    c.uid_move("5", "Trash").unwrap();
    assert_eq!(sent(c), "a1 APPEND Drafts {8+}\nТема\na2 UID MOVE 5 Trash\n");
}

#[test]
fn passwords_never_go_out_in_the_clear() {
    let mut c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1] hi\n"), false).unwrap();
    assert_eq!(c.login("u", "p"), Err(Error::Insecure));
    let mut c2 = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1 LOGINDISABLED] hi\n"), true).unwrap();
    assert!(matches!(c2.login("u", "p"), Err(Error::Unsupported(_))));
    assert_eq!(sent(c), "");
    assert_eq!(sent(c2), "");
}

#[test]
fn refusals_become_errors() {
    let mut c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1] hi\na1 NO [AUTHENTICATIONFAILED] wrong password\n"), true).unwrap();
    assert_eq!(c.login("u", "bad"), Err(Error::No("wrong password".into())));
    assert!(!c.is_authenticated());
    assert!(matches!(Client::new(Script::new("* BYE too busy\n"), true), Err(Error::Bye(_))));
    let mut c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1] hi\na1 BAD what?\n"), true).unwrap();
    assert_eq!(c.select("INBOX", true), Err(Error::Bad("what?".into())));
    // The server hangs up mid-command: an error, and the client knows it is closed.
    let mut c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1] hi\n* 3 EXISTS\n"), true).unwrap();
    assert_eq!(c.noop(), Err(Error::Closed));
    assert_eq!(c.noop(), Err(Error::Closed));
    // A synchronising literal the server refuses.
    let mut c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1] hi\na1 NO [TRYCREATE] no such mailbox\n"), true).unwrap();
    assert_eq!(c.append("Nope", &[], b"x"), Err(Error::No("no such mailbox".into())));
}

#[test]
fn starttls_refuses_injected_plaintext() {
    // RFC 9051 §6.2.1: bytes after the STARTTLS OK but before the handshake are not trusted.
    let c = Client::new(
        Script::new("* OK [CAPABILITY IMAP4rev1 STARTTLS LOGINDISABLED] hi\na1 OK begin TLS\n* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] injected\n"),
        false,
    )
    .unwrap();
    assert!(matches!(c.starttls(), Err(Error::Protocol(_))));
    let c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1 STARTTLS] hi\na1 OK begin TLS\n"), false).unwrap();
    let mut stream = c.starttls().unwrap();
    stream.replies = std::io::Cursor::new(b"* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\na1 OK\r\n".to_vec());
    let c = Client::resume(stream).unwrap();
    assert!(c.is_secure() && c.has("AUTH=PLAIN"));
    let c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1] hi\n"), false).unwrap();
    assert!(matches!(c.starttls(), Err(Error::Unsupported(_))));
}

#[test]
fn bad_arguments_are_refused_before_sending() {
    let mut c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1] hi\n"), true).unwrap();
    assert!(matches!(c.uid_fetch("1\r\na2 DELETE INBOX", Items::Summary), Err(Error::Invalid(_))));
    assert!(matches!(c.uid_store("1", true, &["\\Seen) (x"]), Err(Error::Invalid(_))));
    assert!(matches!(c.uid_search(""), Err(Error::Invalid(_))));
    assert_eq!(sent(c), "");
}

#[test]
fn role_guesses() {
    let m = |name: &str, attrs: &[&str]| Mailbox {
        raw: name.into(),
        name: name.into(),
        delimiter: Some('.'),
        attributes: attrs.iter().map(|s| s.to_string()).collect(),
    };
    assert_eq!(m("INBOX.Papelera", &[]).role(), FolderRole::Trash);
    assert_eq!(m("Кошик", &[]).role(), FolderRole::Trash);
    assert_eq!(m("Whatever", &["\\Junk"]).role(), FolderRole::Junk);
    assert_eq!(m("[Gmail].All Mail", &["\\All"]).role(), FolderRole::Other);
    assert_eq!(m("Projects", &[]).role(), FolderRole::Other);
}

/// Random server bytes, and real responses with random corruption, never panic the client.
#[test]
fn hostile_servers_never_panic() {
    let mut seed: u64 = 0x1BE5_C022;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let good = b"* OK [CAPABILITY IMAP4rev1 LITERAL+] hi\r\na1 OK in\r\n* LIST () \"/\" {3}\r\nabc\r\na2 OK\r\n* 1 FETCH (UID 1 BODY[] {2}\r\nhi)\r\na3 OK\r\n".to_vec();
    for round in 0..3000 {
        let mut bytes = good.clone();
        if round % 3 == 0 {
            bytes = (0..(next() % 200)).map(|_| (next() % 256) as u8).collect();
        } else {
            for _ in 0..(next() % 6 + 1) {
                let len = bytes.len() as u64;
                if let Some(b) = bytes.get_mut((next() % len) as usize) {
                    *b = b"{}()[]\"\\ \r\n*+0123456789xa"[(next() % 25) as usize];
                }
            }
        }
        if let Ok(mut c) = Client::new(Script::bytes(bytes), true) {
            let _ = c.login("u", "p");
            let _ = c.list();
            let _ = c.uid_fetch("1:*", Items::Full);
            let _ = c.noop();
        }
    }
}

/// A server that goes quiet between bursts: each burst is read out, then one read times out.
struct Quiet {
    bursts: std::collections::VecDeque<Vec<u8>>,
    current: std::io::Cursor<Vec<u8>>,
    sent: Vec<u8>,
}

impl Quiet {
    fn new(bursts: &[&str]) -> Self {
        let mut bursts: std::collections::VecDeque<Vec<u8>> = bursts.iter().map(|b| b.replace('\n', "\r\n").into_bytes()).collect();
        let first = bursts.pop_front().unwrap_or_default();
        Quiet { bursts, current: std::io::Cursor::new(first), sent: Vec::new() }
    }
}

impl Read for Quiet {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.current.read(buf)?;
        if n > 0 {
            return Ok(n);
        }
        match self.bursts.pop_front() {
            Some(next) => {
                self.current = std::io::Cursor::new(next);
                Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "timed out"))
            }
            None => Ok(0),
        }
    }
}

impl Write for Quiet {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sent.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn idle_reports_news_survives_quiet_spells_and_ends_cleanly() {
    let server = Quiet::new(&[
        "* OK [CAPABILITY IMAP4rev1 IDLE] ready\n+ idling\n",
        // Quiet, then a keep-alive, then quiet, then a new message, half a line at first.
        "* OK Still here\n",
        "* 3 EXI",
        "STS\n",
        "a1 OK IDLE terminated\n",
    ]);
    let mut c = Client::new(server, true).unwrap();
    assert!(c.idle_start().unwrap().is_empty());
    assert!(c.is_idling());
    assert!(matches!(c.run(&Command::new("NOOP")), Err(Error::Invalid(_))), "no other command while idling");
    assert_eq!(c.idle_wait().unwrap(), [], "a quiet spell is not an error");
    assert_eq!(c.idle_wait().unwrap(), [], "a keep-alive is not news");
    assert_eq!(c.idle_wait().unwrap(), [], "quiet again");
    assert_eq!(c.idle_wait().unwrap(), [], "half a line waits for the rest");
    let news = c.idle_wait().unwrap();
    assert_eq!(news, [Untagged::Exists(3)]);
    assert!(news.iter().all(is_news));
    assert_eq!(c.idle_wait().unwrap(), [], "the rest of the quiet spell");
    c.idle_done().unwrap();
    assert!(!c.is_idling());
    let s = c.into_stream();
    assert_eq!(String::from_utf8(s.sent).unwrap(), "a1 IDLE\r\nDONE\r\n");
}

#[test]
fn idle_needs_the_capability_and_reports_refusals() {
    let mut c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1] ready\n"), true).unwrap();
    assert!(matches!(c.idle_start(), Err(Error::Unsupported(_))));
    let mut c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1 IDLE] ready\na1 NO no mailbox selected\n"), true).unwrap();
    assert!(matches!(c.idle_start(), Err(Error::No(_))));
    assert!(!c.is_idling());
    // The server hangs up while idling.
    let mut c = Client::new(Script::new("* OK [CAPABILITY IMAP4rev1 IDLE] ready\n+ idling\n* BYE shutting down\n"), true).unwrap();
    c.idle_start().unwrap();
    assert!(c.idle_wait().is_err());
    assert!(c.idle_wait().is_err(), "closed for good");
    // A timeout in the middle of a literal ends the session.
    let mut c =
        Client::new(Quiet::new(&["* OK [CAPABILITY IMAP4rev1 IDLE] ready\n+ idling\n* 1 FETCH (BODY[] {10}\nabc", "defghij)\n"]), true).unwrap();
    c.idle_start().unwrap();
    assert!(matches!(c.idle_wait(), Err(Error::Io(_))));
}
