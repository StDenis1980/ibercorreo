//! HTTP/1.1 requests and replies (RFC 9112), with every size the server controls capped.

use std::io::{Read, Write};

use crate::{Error, Result};

/// Longest status line plus headers accepted.
const MAX_HEAD: usize = 64 * 1024;
/// Largest reply body accepted (a calendar of a few thousand events is a few MiB).
pub(crate) const MAX_BODY: usize = 64 * 1024 * 1024;
/// Most trailer lines after a chunked body.
const MAX_TRAILERS: usize = 100;
/// Most informational (1xx) replies skipped before the real one.
const MAX_INFORMATIONAL: usize = 8;

/// One reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Header names in lower case, in the order received.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The server closes the connection after this reply.
    pub close: bool,
}

impl Response {
    /// The first header named `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as text, with anything that is not UTF-8 replaced.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// The bytes of one request. Header values with CR or LF are refused.
pub(crate) fn request(method: &str, path: &str, host: &str, auth: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Vec<u8>> {
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase() || b == b'-') {
        return Err(Error::Invalid(format!("{method:?} is not an HTTP method")));
    }
    if !path.starts_with('/') || !path.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(Error::Invalid(format!("{path:?} is not a request path")));
    }
    let mut out =
        format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: {auth}\r\nUser-Agent: IberCorreo\r\nAccept-Encoding: identity\r\n");
    for (name, value) in headers {
        let name_ok = !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !name_ok || value.bytes().any(|b| b == b'\r' || b == b'\n') {
            return Err(Error::Invalid(format!("bad request header {name:?}")));
        }
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() || matches!(method, "PUT" | "POST" | "PROPFIND" | "REPORT" | "PROPPATCH" | "MKCALENDAR") {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("\r\n");
    if out.contains('\0') {
        return Err(Error::Invalid("a request header holds a NUL byte".into()));
    }
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    Ok(bytes)
}

/// One connection: what was read past the last reply stays buffered for the next.
pub(crate) struct Conn<S> {
    stream: S,
    buf: Vec<u8>,
}

impl<S: Read + Write> Conn<S> {
    pub(crate) fn new(stream: S) -> Self {
        Conn { stream, buf: Vec::new() }
    }

    /// Write `request` and read its reply. `head_only` for a HEAD request, whose reply has no
    /// body whatever its headers say.
    pub(crate) fn exchange(&mut self, request: &[u8], head_only: bool) -> Result<Response> {
        self.stream.write_all(request)?;
        self.stream.flush()?;
        for _ in 0..=MAX_INFORMATIONAL {
            let head = self.read_head()?;
            let (status, headers, http10) = parse_head(&head)?;
            if (100..200).contains(&status) && status != 101 {
                continue;
            }
            let header = |n: &str| headers.iter().find(|(h, _)| h == n).map(|(_, v)| v.as_str());
            let connection = header("connection").unwrap_or("").to_ascii_lowercase();
            let mut close = connection.split(',').any(|t| t.trim() == "close") || (http10 && !connection.contains("keep-alive"));
            let no_body = head_only || status == 204 || status == 304;
            let body = if no_body {
                Vec::new()
            } else if header("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked")) {
                self.read_chunked()?
            } else if let Some(len) = header("content-length") {
                let len: usize = len.trim().parse().map_err(|_| Error::Protocol(format!("Content-Length {len:?}")))?;
                if len > MAX_BODY {
                    return Err(Error::TooLarge { what: "the reply", limit: MAX_BODY });
                }
                self.read_exact_body(len)?
            } else {
                close = true;
                self.read_to_end()?
            };
            return Ok(Response { status, headers, body, close });
        }
        Err(Error::Protocol("too many informational replies".into()))
    }

    /// Read more into the buffer; `false` at the end of the stream.
    fn fill(&mut self) -> Result<bool> {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    self.buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                    return Ok(true);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// The status line and headers, up to the blank line, removed from the buffer.
    fn read_head(&mut self) -> Result<Vec<u8>> {
        loop {
            if let Some(end) = find(&self.buf, b"\r\n\r\n") {
                let head: Vec<u8> = self.buf.drain(..end + 4).collect();
                return Ok(head);
            }
            if self.buf.len() > MAX_HEAD {
                return Err(Error::TooLarge { what: "the reply headers", limit: MAX_HEAD });
            }
            if !self.fill()? {
                return Err(if self.buf.is_empty() {
                    Error::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the server closed the connection"))
                } else {
                    Error::Protocol("the reply ended inside its headers".into())
                });
            }
        }
    }

    /// One line (without CRLF), removed from the buffer.
    fn read_line(&mut self, limit: usize) -> Result<Vec<u8>> {
        loop {
            if let Some(end) = find(&self.buf, b"\r\n") {
                let mut line: Vec<u8> = self.buf.drain(..end + 2).collect();
                line.truncate(end);
                return Ok(line);
            }
            if self.buf.len() > limit {
                return Err(Error::Protocol("a chunk line is too long".into()));
            }
            if !self.fill()? {
                return Err(Error::Protocol("the reply ended inside a chunk".into()));
            }
        }
    }

    fn read_exact_body(&mut self, len: usize) -> Result<Vec<u8>> {
        while self.buf.len() < len {
            if !self.fill()? {
                return Err(Error::Protocol("the reply ended before its Content-Length".into()));
            }
        }
        Ok(self.buf.drain(..len).collect())
    }

    fn read_to_end(&mut self) -> Result<Vec<u8>> {
        while self.fill()? {
            if self.buf.len() > MAX_BODY {
                return Err(Error::TooLarge { what: "the reply", limit: MAX_BODY });
            }
        }
        Ok(std::mem::take(&mut self.buf))
    }

    fn read_chunked(&mut self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        loop {
            let line = self.read_line(4096)?;
            let line = String::from_utf8_lossy(&line);
            let size = line.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size, 16).map_err(|_| Error::Protocol(format!("chunk size {size:?}")))?;
            if size == 0 {
                // Trailers, up to the blank line.
                for _ in 0..MAX_TRAILERS {
                    if self.read_line(MAX_HEAD)?.is_empty() {
                        return Ok(body);
                    }
                }
                return Err(Error::Protocol("too many trailer lines".into()));
            }
            if body.len().saturating_add(size) > MAX_BODY {
                return Err(Error::TooLarge { what: "the reply", limit: MAX_BODY });
            }
            let chunk = self.read_exact_body(size)?;
            body.extend_from_slice(&chunk);
            if !self.read_line(16)?.is_empty() {
                return Err(Error::Protocol("a chunk is longer than its size".into()));
            }
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Status, headers (lower-case names) and whether the reply is HTTP/1.0.
type Head = (u16, Vec<(String, String)>, bool);

fn parse_head(head: &[u8]) -> Result<Head> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") {
        return Err(Error::Protocol(format!("not an HTTP reply: {:?}", status_line.chars().take(80).collect::<String>())));
    }
    let status = parts.next().and_then(|s| s.parse::<u16>().ok()).filter(|s| (100..600).contains(s));
    let status = status.ok_or_else(|| Error::Protocol(format!("bad status line {:?}", status_line.chars().take(80).collect::<String>())))?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    Ok((status, headers, version == "HTTP/1.0"))
}
