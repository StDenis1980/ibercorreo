use std::io::{Read, Write};

use super::*;

struct Script {
    replies: std::io::Cursor<Vec<u8>>,
    sent: Vec<u8>,
}

impl Read for Script {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(5);
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

fn script(replies: &str) -> Script {
    Script { replies: std::io::Cursor::new(replies.replace('\n', "\r\n").into_bytes()), sent: Vec::new() }
}

fn sent(c: Client<Script>) -> String {
    String::from_utf8(c.into_stream().sent).unwrap().replace("\r\n", "\n")
}

const GREETING: &str = "220 mail.example.org ESMTP ready
250-mail.example.org hello
250-SIZE 1000
250-8BITMIME
250-AUTH LOGIN PLAIN
250 STARTTLS
";

#[test]
fn a_submission() {
    let replies = format!("{GREETING}235 ok\n250 sender ok\n250 rcpt ok\n251 will forward\n354 go\n250 queued as 1\n221 bye\n");
    let mut c = Client::new(script(&replies), true, "laptop.local").unwrap();
    assert!(c.has("starttls") && c.has("SIZE") && !c.has("SMTPUTF8"));
    assert_eq!(c.size_limit(), Some(1000));
    c.login("denys", "pass word").unwrap();
    c.send("denys@example.org", &["ana@example.org", "bob@example.net"], b"Subject: hi\n\n.leading dot\r\nend").unwrap();
    let _ = c.command("QUIT").unwrap();
    let token = ibercorreo_mime::base64(b"\0denys\0pass word");
    assert_eq!(
        sent(c),
        format!(
            "EHLO laptop.local
AUTH PLAIN {token}
MAIL FROM:<denys@example.org> SIZE=30
RCPT TO:<ana@example.org>
RCPT TO:<bob@example.net>
DATA
Subject: hi

..leading dot
end
.
QUIT
"
        )
    );
}

#[test]
fn dot_stuffing_and_line_ends() {
    assert_eq!(data(b".\n..\r\na.b\rc"), b"..\r\n...\r\na.b\rc\r\n.\r\n");
    assert_eq!(data(b""), b"\r\n.\r\n");
    assert_eq!(data(b"x\r\n"), b"x\r\n.\r\n");
}

#[test]
fn auth_login_when_only_login_is_offered() {
    let replies = "220 hi\n250-hi\n250 AUTH LOGIN\n334 VXNlcm5hbWU6\n334 UGFzc3dvcmQ6\n235 ok\n";
    let mut c = Client::new(script(replies), true, "h").unwrap();
    c.login("u", "p").unwrap();
    assert_eq!(sent(c), format!("EHLO h\nAUTH LOGIN\n{}\n{}\n", ibercorreo_mime::base64(b"u"), ibercorreo_mime::base64(b"p")));
}

#[test]
fn refusals_and_limits() {
    // No password over plain TCP, and nothing is sent.
    let mut c = Client::new(script(GREETING), false, "h").unwrap();
    assert_eq!(c.login("u", "p"), Err(Error::Insecure));
    // Over the SIZE limit: refused before MAIL FROM.
    assert_eq!(c.send("a@b.c", &["d@e.f"], &[b'x'; 1001]), Err(Error::TooLarge("message")));
    // Addresses cannot carry commands.
    for bad in ["a@b.c>\r\nRCPT TO:<evil@x.y", "no-at", "a b@c.d", "<a@b.c>", ""] {
        assert!(matches!(c.send(bad, &["d@e.f"], b"x"), Err(Error::Invalid(_))), "{bad}");
        assert!(matches!(c.send("a@b.c", &[bad], b"x"), Err(Error::Invalid(_))), "{bad}");
    }
    assert!(matches!(c.send("a@b.c", &[], b"x"), Err(Error::Invalid(_))));
    assert!(matches!(c.send("a@b.c", &["ünï@b.c"], b"x"), Err(Error::Unsupported(_))));
    assert_eq!(sent(c), "EHLO h\n");

    // A rejected recipient fails the message, with a reset.
    let replies = format!("{GREETING}250 ok\n550 5.1.1 no such user\n250 reset\n");
    let mut c = Client::new(script(&replies), true, "h").unwrap();
    let e = c.send("a@b.c", &["nobody@b.c"], b"x").unwrap_err();
    assert_eq!(e, Error::Rejected { code: 550, text: "nobody@b.c: 5.1.1 no such user".into() });
    assert!(!e.is_temporary());
    assert!(sent(c).ends_with("RSET\n"));

    // A temporary failure is reported as one.
    let replies = format!("{GREETING}451 try later\n");
    let mut c = Client::new(script(&replies), true, "h").unwrap();
    assert!(c.send("a@b.c", &["d@e.f"], b"x").unwrap_err().is_temporary());

    // A greeting that is not 220, and HELO fallback.
    assert!(matches!(Client::new(script("554 go away\n"), true, "h"), Err(Error::Rejected { code: 554, .. })));
    let c = Client::new(script("220 old\n500 what\n250 ok\n"), true, "bad name\r\n").unwrap();
    assert!(!c.has("SIZE"));
    assert_eq!(sent(c), "EHLO localhost\nHELO localhost\n");
}

#[test]
fn starttls_refuses_injected_plaintext() {
    let c = Client::new(script(&format!("{GREETING}220 go ahead\n250 injected\n")), false, "h").unwrap();
    assert!(matches!(c.starttls(), Err(Error::Protocol(_))));
    let c = Client::new(script(&format!("{GREETING}220 go ahead\n")), false, "h").unwrap();
    let mut stream = c.starttls().unwrap();
    stream.replies = std::io::Cursor::new(b"250-hi again\r\n250 AUTH PLAIN\r\n".to_vec());
    let c = Client::resume(stream, "h").unwrap();
    assert!(c.is_secure() && c.has("AUTH") && !c.has("STARTTLS"));
}

#[test]
fn hostile_servers_never_panic() {
    let mut seed: u64 = 0x5317_0000_0001;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let good = format!("{GREETING}235 ok\n250 ok\n250 ok\n354 go\n250 ok\n").replace('\n', "\r\n").into_bytes();
    for round in 0..3000 {
        let mut bytes = good.clone();
        if round % 3 == 0 {
            bytes = (0..(next() % 300)).map(|_| (next() % 256) as u8).collect();
        } else {
            for _ in 0..(next() % 6 + 1) {
                let len = bytes.len() as u64;
                if let Some(b) = bytes.get_mut((next() % len) as usize) {
                    *b = b"0123456789- \r\n\xff"[(next() % 15) as usize];
                }
            }
        }
        let s = Script { replies: std::io::Cursor::new(bytes), sent: Vec::new() };
        if let Ok(mut c) = Client::new(s, true, "h") {
            let _ = c.login("u", "p");
            let _ = c.send("a@b.c", &["d@e.f"], b"hello");
        }
    }
}
