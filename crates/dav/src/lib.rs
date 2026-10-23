//! HTTP/1.1, WebDAV (RFC 4918), CalDAV (RFC 4791) and CardDAV (RFC 6352) for calendar and
//! contact sync, over any byte stream.
//! The stream is TLS in the app (`ibercorreo-net`) and a scripted buffer in tests; this crate
//! never opens a socket itself, so it builds for the browser too.
//!
//! - [`Url`]: an `https://` address, and hrefs resolved against it (other hosts refused, so a
//!   password never goes anywhere else; `.` and `..` resolved, so a path stays where it looks).
//! - [`Client`]: requests on one kept-alive connection, opened again when the server closes it.
//! - [`xml`]: the `multistatus` replies, read with bounded effort.
//! - [`caldav`]: finding the calendars, listing what is in one, fetching events.
//! - [`carddav`]: the same for address books and contact cards.
//! - [`autoconfig`]: where a mail address's server settings are published, and reading them.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod autoconfig;
pub mod caldav;
pub mod carddav;
mod http;
pub mod xml;

use std::io::{Read, Write};

pub use http::Response;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Io(#[from] std::io::Error),
    #[error("the server sent a malformed reply: {0}")]
    Protocol(String),
    /// The server refused the user name or password.
    #[error("sign-in refused by the calendar server (HTTP {0})")]
    Auth(u16),
    #[error("the calendar server answered HTTP {status}: {what}")]
    Status { status: u16, what: String },
    #[error("{0}")]
    Invalid(String),
    #[error("{what} is larger than the {limit} allowed")]
    TooLarge { what: &'static str, limit: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for ibercorreo_core::Error {
    fn from(e: Error) -> Self {
        ibercorreo_core::Error::invalid("calendar server", e.to_string())
    }
}

/// An `https://` address. Plain `http://` is refused: the password travels in every request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    /// The path and query, percent-encoded, starting with `/`.
    pub path: String,
}

/// Longest URL or href accepted.
const MAX_URL: usize = 4096;

impl Url {
    pub fn parse(s: &str) -> Result<Url> {
        let s = s.trim();
        if s.len() > MAX_URL {
            return Err(Error::TooLarge { what: "address", limit: MAX_URL });
        }
        let lower = s.to_ascii_lowercase();
        let rest = if lower.starts_with("https://") {
            s.get(8..).unwrap_or("")
        } else if lower.starts_with("http://") {
            return Err(Error::Invalid("the calendar address must start with https:// (the password would travel unencrypted)".into()));
        } else {
            return Err(Error::Invalid(format!("{s:?} is not an https:// address")));
        };
        let rest = rest.split('#').next().unwrap_or("");
        let (authority, path) = match rest.find('/') {
            Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("/")),
            None => (rest, "/"),
        };
        if authority.contains('@') {
            return Err(Error::Invalid("put the user name in the account, not in the address".into()));
        }
        let (host, port) = split_host_port(authority)?;
        Ok(Url { host, port, path: encode_path(path) })
    }

    /// The request path `href` names, resolved against this address: an absolute path, a
    /// relative one, or a full URL on the same host and port. Anything else is `None`.
    pub fn join(&self, href: &str) -> Option<String> {
        let href = href.trim();
        if href.is_empty() || href.len() > MAX_URL {
            return None;
        }
        let lower = href.to_ascii_lowercase();
        if lower.starts_with("https://") || lower.starts_with("http://") {
            let other = Url::parse(&href.replacen("http://", "https://", 1)).ok()?;
            return (other.host.eq_ignore_ascii_case(&self.host) && other.port == self.port).then_some(other.path);
        }
        if href.starts_with('/') {
            return Some(encode_path(href));
        }
        let base = self.path.split('?').next().unwrap_or("/");
        let dir = base.rfind('/').and_then(|i| base.get(..=i)).unwrap_or("/");
        Some(encode_path(&format!("{dir}{href}")))
    }

    /// The `Host` header value.
    pub fn host_header(&self) -> String {
        if self.port == 443 { self.host.clone() } else { format!("{}:{}", self.host, self.port) }
    }
}

fn split_host_port(authority: &str) -> Result<(String, u16)> {
    let bad = || Error::Invalid(format!("{authority:?} is not a host name"));
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(bad)?;
        let host = rest.get(..end).ok_or_else(bad)?;
        let after = rest.get(end + 1..).unwrap_or("");
        (format!("[{host}]"), after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (authority.to_string(), None),
        }
    };
    let ok = !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'[' | b']' | b':'));
    if !ok {
        return Err(bad());
    }
    let port = match port {
        Some(p) => p.parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(bad)?,
        None => 443,
    };
    Ok((host.to_ascii_lowercase(), port))
}

/// Percent-encode everything outside visible ASCII (spaces, controls, non-ASCII), so a path can
/// never break the request line or add a header.
pub(crate) fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if (0x21..=0x7e).contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    if !out.starts_with('/') {
        out.insert(0, '/');
    }
    remove_dots(&out)
}

/// `path` with its `.` and `..` segments resolved (RFC 3986 §5.2.4), so a path that looks inside
/// a collection is inside it. Percent-encoded dots (`%2e`) count as dots, since servers decode
/// them. A `..` above the root stays at the root.
fn remove_dots(path: &str) -> String {
    let (path, query) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    let dots = |seg: &str| {
        let s = seg.to_ascii_lowercase().replace("%2e", ".");
        if s == "." {
            Some(1)
        } else if s == ".." {
            Some(2)
        } else {
            None
        }
    };
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    let last = segments.len().saturating_sub(1);
    for (i, seg) in segments.iter().enumerate() {
        match dots(seg) {
            Some(n) => {
                if n == 2 {
                    out.pop();
                }
                // A trailing dot segment names the directory: keep its slash.
                if i == last {
                    out.push("");
                }
            }
            None => out.push(seg),
        }
    }
    let mut s = format!("/{}", out.join("/"));
    if let Some(q) = query {
        s.push('?');
        s.push_str(q);
    }
    s
}

/// HTTP requests over one connection at a time, opened with `connect` when there is none (at
/// first, and again after the server closes it).
pub struct Client<S, C> {
    connect: C,
    conn: Option<http::Conn<S>>,
    host: String,
    auth: String,
}

impl<S: Read + Write, C: FnMut() -> std::io::Result<S>> Client<S, C> {
    /// A client for `url`'s server signing in as `user` with HTTP Basic authentication (on TLS
    /// only, which `connect` provides).
    pub fn new(connect: C, url: &Url, user: &str, password: &str) -> Self {
        let token = ibercorreo_mime::base64(format!("{user}:{password}").as_bytes());
        Client { connect, conn: None, host: url.host_header(), auth: format!("Basic {token}") }
    }

    /// A client signing in with an OAuth 2.0 access token (`Authorization: Bearer`). A token
    /// that could break the header is not sent at all.
    pub fn bearer(connect: C, url: &Url, token: &str) -> Self {
        let ok = !token.is_empty() && token.bytes().all(|b| b.is_ascii_graphic());
        Client { connect, conn: None, host: url.host_header(), auth: if ok { format!("Bearer {token}") } else { String::new() } }
    }

    /// A client that sends no credentials (for public files such as a mail configuration).
    pub fn anonymous(connect: C, url: &Url) -> Self {
        Client { connect, conn: None, host: url.host_header(), auth: String::new() }
    }

    /// Send one request and read the reply, whatever its status. A connection the server had
    /// quietly closed is opened again once.
    pub fn send(&mut self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
        let request = http::request(method, path, &self.host, &self.auth, headers, body)?;
        let head_only = method.eq_ignore_ascii_case("HEAD");
        for attempt in 0..2 {
            let fresh = self.conn.is_none();
            let conn = match self.conn.take() {
                Some(c) => c,
                None => http::Conn::new((self.connect)()?),
            };
            let mut conn = conn;
            match conn.exchange(&request, head_only) {
                Ok(resp) => {
                    if !resp.close {
                        self.conn = Some(conn);
                    }
                    return Ok(resp);
                }
                // A kept-alive connection may have been closed by the server; try a new one.
                Err(Error::Io(_)) | Err(Error::Protocol(_)) if !fresh && attempt == 0 => continue,
                Err(e) => return Err(e),
            }
        }
        Err(Error::Protocol("the server closed the connection twice".into()))
    }
}

#[cfg(test)]
mod tests;
