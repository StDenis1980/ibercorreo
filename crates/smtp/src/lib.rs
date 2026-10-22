//! Iber Correo's SMTP submission client, L2, written from RFC 5321 (SMTP), RFC 6409 (message
//! submission), RFC 3207 (STARTTLS), RFC 4954 and RFC 4616 (AUTH PLAIN), RFC 1870 (SIZE) and
//! RFC 6152 (8BITMIME).
//!
//! Like the IMAP client it works over any `Read + Write` stream and opens no socket. The server is
//! untrusted: replies are size-capped and parsed without panicking. Addresses are checked before
//! they go into a command, so a recipient can never smuggle in a second command.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::{Read, Write};

use thiserror::Error;

/// The longest reply line accepted (RFC 5321 allows 512; some servers send more).
pub const MAX_LINE: usize = 64 * 1024;
/// The most lines in one reply.
pub const MAX_REPLY_LINES: usize = 1000;
/// The largest message sent: as large as `ibercorreo_mime` reads.
pub const MAX_MESSAGE: usize = ibercorreo_mime::MAX_MESSAGE_BYTES;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("connection: {0}")]
    Io(String),
    #[error("the server closed the connection")]
    Closed,
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("{0} too large")]
    TooLarge(&'static str),
    /// The server answered with an error reply (4xx temporary, 5xx permanent).
    #[error("the server refused ({code}): {text}")]
    Rejected { code: u16, text: String },
    #[error("refusing to send a password over an unencrypted connection")]
    Insecure,
    #[error("invalid {0}")]
    Invalid(String),
    #[error("not supported: {0}")]
    Unsupported(&'static str),
}

impl Error {
    /// Whether trying again later may work (a 4xx reply or a dropped connection).
    pub fn is_temporary(&self) -> bool {
        match self {
            Error::Rejected { code, .. } => (400..500).contains(code),
            Error::Io(_) | Error::Closed => true,
            _ => false,
        }
    }
}

/// One reply: its code and the text of each line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub code: u16,
    pub lines: Vec<String>,
}

impl Reply {
    pub fn text(&self) -> String {
        self.lines.join(" ")
    }
}

#[derive(Debug)]
pub struct Client<S> {
    stream: S,
    buf: Vec<u8>,
    secure: bool,
    helo: String,
    /// EHLO keywords and their parameters, upper-cased (`AUTH PLAIN LOGIN`, `SIZE 35882577`).
    extensions: Vec<String>,
    closed: bool,
}

/// A mailbox for `MAIL FROM` / `RCPT TO`: printable, no spaces, brackets or line breaks, one `@`.
fn check_address(a: &str) -> Result<&str> {
    let ok = a.len() <= 254
        && a.split_once('@').is_some_and(|(l, d)| !l.is_empty() && !d.is_empty() && !d.contains('@'))
        && !a.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '<' | '>'));
    if ok { Ok(a) } else { Err(Error::Invalid(format!("address {a:?}"))) }
}

/// The message with CRLF line ends, dot-stuffed (RFC 5321 §4.5.2), ending in CRLF `.` CRLF.
pub fn data(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len().saturating_add(message.len() / 64).saturating_add(5));
    let mut at_line_start = true;
    let mut prev = 0u8;
    for &b in message {
        if b == b'\n' && prev != b'\r' {
            out.push(b'\r');
        }
        if at_line_start && b == b'.' {
            out.push(b'.');
        }
        out.push(b);
        at_line_start = b == b'\n';
        prev = b;
    }
    if !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b".\r\n");
    out
}

impl<S: Read + Write> Client<S> {
    /// Read the `220` greeting and introduce ourselves (`EHLO helo`). `secure` says whether the
    /// stream is already TLS; `helo` is this machine's name (any domain or `[address literal]`).
    pub fn new(stream: S, secure: bool, helo: &str) -> Result<Self> {
        let mut c = Client { stream, buf: Vec::new(), secure, helo: clean_helo(helo), extensions: Vec::new(), closed: false };
        let greeting = c.reply()?;
        if greeting.code != 220 {
            return Err(Error::Rejected { code: greeting.code, text: greeting.text() });
        }
        c.ehlo()?;
        Ok(c)
    }

    /// The stream back, e.g. to close it.
    pub fn into_stream(self) -> S {
        self.stream
    }

    pub fn is_secure(&self) -> bool {
        self.secure
    }

    /// Whether the server announced an extension (`STARTTLS`, `8BITMIME`, ...).
    pub fn has(&self, extension: &str) -> bool {
        self.extensions.iter().any(|e| e.split(' ').next().is_some_and(|k| k.eq_ignore_ascii_case(extension)))
    }

    fn auth_mechanisms(&self) -> Vec<String> {
        self.extensions.iter().filter_map(|e| e.strip_prefix("AUTH ")).flat_map(|m| m.split(' ').map(str::to_owned)).collect()
    }

    /// The largest message the server takes (`SIZE`), if it says.
    pub fn size_limit(&self) -> Option<usize> {
        self.extensions.iter().find_map(|e| e.strip_prefix("SIZE ")).and_then(|n| n.trim().parse().ok()).filter(|&n| n > 0)
    }

    fn ehlo(&mut self) -> Result<()> {
        let r = self.command(&format!("EHLO {}", self.helo))?;
        if r.code == 250 {
            self.extensions = r.lines.iter().skip(1).map(|l| l.trim().to_ascii_uppercase()).collect();
            return Ok(());
        }
        // A server without ESMTP: plain HELO, no extensions.
        let r = self.command(&format!("HELO {}", self.helo))?;
        self.extensions.clear();
        expect(&r, 250)
    }

    fn fill(&mut self) -> Result<()> {
        let mut chunk = [0u8; 4096];
        match self.stream.read(&mut chunk) {
            Ok(0) => {
                self.closed = true;
                Err(Error::Closed)
            }
            Ok(n) => {
                self.buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                Ok(())
            }
            Err(e) => {
                self.closed = true;
                Err(Error::Io(e.to_string()))
            }
        }
    }

    fn line(&mut self) -> Result<String> {
        loop {
            if let Some(p) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=p).collect();
                let text = String::from_utf8_lossy(&line);
                return Ok(text.trim_end_matches(['\r', '\n']).to_owned());
            }
            if self.buf.len() > MAX_LINE {
                self.closed = true;
                return Err(Error::TooLarge("reply line"));
            }
            self.fill()?;
        }
    }

    /// One reply, possibly over several lines (`250-...` then `250 ...`).
    pub fn reply(&mut self) -> Result<Reply> {
        let mut lines = Vec::new();
        let mut code = None;
        loop {
            let line = self.line()?;
            let digits = line.get(..3).ok_or_else(|| Error::Protocol(format!("short reply {line:?}")))?;
            let n: u16 = digits.parse().map_err(|_| Error::Protocol(format!("reply without a code: {line:?}")))?;
            if !(200..600).contains(&n) && !(100..200).contains(&n) {
                return Err(Error::Protocol(format!("reply code {n}")));
            }
            if code.is_some_and(|c| c != n) {
                return Err(Error::Protocol("reply lines with different codes".into()));
            }
            code = Some(n);
            let more = line.get(3..4) == Some("-");
            lines.push(line.get(4..).unwrap_or("").to_owned());
            if lines.len() > MAX_REPLY_LINES {
                return Err(Error::TooLarge("reply"));
            }
            if !more {
                return Ok(Reply { code: n, lines });
            }
        }
    }

    fn send_raw(&mut self, bytes: &[u8]) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        let r = self.stream.write_all(bytes).and_then(|()| self.stream.flush());
        r.map_err(|e| {
            self.closed = true;
            Error::Io(e.to_string())
        })
    }

    /// Send one command line (this crate's own text; arguments are checked by the callers).
    fn command(&mut self, line: &str) -> Result<Reply> {
        if line.contains(['\r', '\n']) {
            return Err(Error::Invalid("line break in a command".into()));
        }
        self.send_raw(format!("{line}\r\n").as_bytes())?;
        self.reply()
    }

    /// `STARTTLS`, handing the stream back for the TLS handshake (then [`Client::resume`]).
    pub fn starttls(mut self) -> Result<S> {
        if !self.has("STARTTLS") {
            return Err(Error::Unsupported("the server does not offer STARTTLS"));
        }
        let r = self.command("STARTTLS")?;
        expect(&r, 220)?;
        // Bytes before the handshake could be an attacker's (RFC 3207 §6): refuse them.
        if !self.buf.is_empty() {
            return Err(Error::Protocol("data after STARTTLS before the TLS handshake".into()));
        }
        Ok(self.stream)
    }

    /// Continue on the encrypted stream: `EHLO` again, as RFC 3207 §4.2 requires.
    pub fn resume(stream: S, helo: &str) -> Result<Self> {
        let mut c = Client { stream, buf: Vec::new(), secure: true, helo: clean_helo(helo), extensions: Vec::new(), closed: false };
        c.ehlo()?;
        Ok(c)
    }

    /// Log in with `AUTH PLAIN` (or `AUTH LOGIN` when that is all the server offers). Refused on an
    /// unencrypted connection.
    pub fn login(&mut self, user: &str, password: &str) -> Result<()> {
        if !self.secure {
            return Err(Error::Insecure);
        }
        if user.contains('\0') || password.contains('\0') {
            return Err(Error::Invalid("NUL in a user name or password".into()));
        }
        let mechanisms = self.auth_mechanisms();
        let offers = |m: &str| mechanisms.iter().any(|x| x.eq_ignore_ascii_case(m));
        if offers("PLAIN") || !offers("LOGIN") {
            let token = ibercorreo_mime::base64(format!("\0{user}\0{password}").as_bytes());
            let r = self.command(&format!("AUTH PLAIN {token}"))?;
            return expect(&r, 235);
        }
        let r = self.command("AUTH LOGIN")?;
        expect(&r, 334)?;
        let r = self.command(&ibercorreo_mime::base64(user.as_bytes()))?;
        expect(&r, 334)?;
        let r = self.command(&ibercorreo_mime::base64(password.as_bytes()))?;
        expect(&r, 235)
    }

    /// Send one message. Any rejected recipient fails the whole message (nothing is delivered),
    /// so the person can fix the address and send again.
    pub fn send(&mut self, from: &str, to: &[&str], message: &[u8]) -> Result<()> {
        let from = check_address(from)?;
        if to.is_empty() {
            return Err(Error::Invalid("no recipients".into()));
        }
        for r in to {
            check_address(r)?;
        }
        if message.len() > MAX_MESSAGE || self.size_limit().is_some_and(|limit| message.len() > limit) {
            return Err(Error::TooLarge("message"));
        }
        let non_ascii_address = !from.is_ascii() || to.iter().any(|a| !a.is_ascii());
        if non_ascii_address && !self.has("SMTPUTF8") {
            return Err(Error::Unsupported("the server does not take non-ASCII addresses (SMTPUTF8)"));
        }
        let mut mail = format!("MAIL FROM:<{from}>");
        if self.has("SIZE") {
            mail.push_str(&format!(" SIZE={}", message.len()));
        }
        if !message.is_ascii() && self.has("8BITMIME") {
            mail.push_str(" BODY=8BITMIME");
        }
        if non_ascii_address {
            mail.push_str(" SMTPUTF8");
        }
        let r = self.command(&mail)?;
        expect(&r, 250)?;
        for rcpt in to {
            let r = self.command(&format!("RCPT TO:<{rcpt}>"))?;
            if r.code != 250 && r.code != 251 {
                let _ = self.command("RSET");
                return Err(Error::Rejected { code: r.code, text: format!("{rcpt}: {}", r.text()) });
            }
        }
        let r = self.command("DATA")?;
        expect(&r, 354)?;
        self.send_raw(&data(message))?;
        let r = self.reply()?;
        expect(&r, 250)
    }

    /// `QUIT`. The connection is done afterwards whatever the server says.
    pub fn quit(mut self) -> Result<()> {
        match self.command("QUIT") {
            Ok(_) | Err(Error::Closed) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// The `EHLO` argument: a domain or address literal, else `localhost`.
fn clean_helo(helo: &str) -> String {
    let ok =
        !helo.is_empty() && helo.len() <= 255 && helo.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'[' | b']' | b':'));
    if ok { helo.to_owned() } else { "localhost".to_owned() }
}

fn expect(r: &Reply, code: u16) -> Result<()> {
    if r.code == code { Ok(()) } else { Err(Error::Rejected { code: r.code, text: r.text() }) }
}

#[cfg(test)]
mod tests;
