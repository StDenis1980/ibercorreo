//! IMAP's wire syntax (RFC 9051 §4, §9): reading one response with its literals off a byte
//! stream, and splitting it into values. Everything here takes bytes from the server, so every
//! size is capped and nothing indexes by a server-supplied position.

use std::io::Read;

use crate::{Error, Result};

/// The longest line (without literals) accepted from a server: 1 MiB.
pub const MAX_LINE: usize = 1024 * 1024;
/// The largest literal accepted: one message as big as `ibercorreo_mime` will parse.
pub const MAX_LITERAL: usize = ibercorreo_mime::MAX_MESSAGE_BYTES;
/// The largest whole response (lines plus literals): 80 MiB.
pub const MAX_RESPONSE: usize = 80 * 1024 * 1024;
/// How deeply parenthesised lists may nest (BODYSTRUCTURE nests; nothing legitimate nests this far).
pub const MAX_DEPTH: usize = 64;
/// The most items one list may hold.
pub const MAX_ITEMS: usize = 1_000_000;

/// One value of a response, as RFC 9051 §4 defines them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// An atom (`OK`, `\Seen`, `BODY[]`). Text inside `[...]` is kept with the atom.
    Atom(String),
    /// A number (`number64`).
    Number(u64),
    /// A quoted string or a literal, as raw bytes (a literal may hold any octets).
    Str(Vec<u8>),
    /// A parenthesised list.
    List(Vec<Value>),
    /// `NIL`.
    Nil,
}

impl Value {
    /// The value as text, for atoms, numbers and strings. Non-UTF-8 strings are converted lossily.
    pub fn text(&self) -> Option<String> {
        match self {
            Value::Atom(a) => Some(a.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Str(s) => Some(String::from_utf8_lossy(s).into_owned()),
            Value::List(_) | Value::Nil => None,
        }
    }

    /// The value as a 32-bit number (sequence numbers, UIDs, counts).
    pub fn number(&self) -> Option<u32> {
        match self {
            Value::Number(n) => u32::try_from(*n).ok(),
            _ => None,
        }
    }
}

/// A buffered reader that splits a stream into IMAP responses.
#[derive(Debug)]
pub struct Reader {
    buf: Vec<u8>,
    start: usize,
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader {
    pub fn new() -> Self {
        Reader { buf: Vec::new(), start: 0 }
    }

    /// Whether bytes the server sent are buffered but not yet read as a response.
    pub fn has_pending(&self) -> bool {
        !self.pending().is_empty()
    }

    fn pending(&self) -> &[u8] {
        self.buf.get(self.start..).unwrap_or(&[])
    }

    fn compact(&mut self) {
        if self.start > 0 {
            self.buf.drain(..self.start.min(self.buf.len()));
            self.start = 0;
        }
    }

    fn fill<S: Read>(&mut self, stream: &mut S) -> Result<()> {
        self.compact();
        let mut chunk = [0u8; 16 * 1024];
        let n = stream.read(&mut chunk).map_err(|e| Error::Io(e.to_string()))?;
        if n == 0 {
            return Err(Error::Closed);
        }
        self.buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
        Ok(())
    }

    /// One line including its CRLF (a bare LF is accepted too).
    fn line<S: Read>(&mut self, stream: &mut S) -> Result<Vec<u8>> {
        loop {
            if let Some(p) = self.pending().iter().position(|&b| b == b'\n') {
                let end = self.start.saturating_add(p).saturating_add(1);
                let line = self.buf.get(self.start..end).unwrap_or(&[]).to_vec();
                self.start = end;
                return Ok(line);
            }
            if self.pending().len() > MAX_LINE {
                return Err(Error::TooLarge("response line"));
            }
            self.fill(stream)?;
        }
    }

    fn exact<S: Read>(&mut self, stream: &mut S, n: usize) -> Result<Vec<u8>> {
        while self.pending().len() < n {
            self.fill(stream)?;
        }
        let end = self.start.saturating_add(n);
        let bytes = self.buf.get(self.start..end).unwrap_or(&[]).to_vec();
        self.start = end;
        Ok(bytes)
    }

    /// Read one whole response: a line, plus every literal it announces and the line after it.
    pub fn response<S: Read>(&mut self, stream: &mut S) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let line = self.line(stream)?;
            let announced = literal_at_end(&line)?;
            out.extend_from_slice(&line);
            let Some(n) = announced else { return Ok(out) };
            if n > MAX_LITERAL {
                return Err(Error::TooLarge("literal"));
            }
            if out.len().saturating_add(n) > MAX_RESPONSE {
                return Err(Error::TooLarge("response"));
            }
            out.extend_from_slice(&self.exact(stream, n)?);
        }
    }
}

/// The size of a `{n}` (or `{n+}`) literal that ends `line`, if it ends with one.
fn literal_at_end(line: &[u8]) -> Result<Option<usize>> {
    let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
    let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
    let Some(body) = trimmed.strip_suffix(b"}") else { return Ok(None) };
    let Some(open) = body.iter().rposition(|&b| b == b'{') else { return Ok(None) };
    let digits = body.get(open.saturating_add(1)..).unwrap_or(&[]);
    let digits = digits.strip_suffix(b"+").unwrap_or(digits);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Ok(None);
    }
    let n = std::str::from_utf8(digits).ok().and_then(|s| s.parse::<usize>().ok()).ok_or(Error::TooLarge("literal"))?;
    Ok(Some(n))
}

/// A cursor over one response's bytes.
#[derive(Debug)]
pub struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Cursor { bytes, pos: 0 }
    }

    pub fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) {
        self.pos = self.pos.saturating_add(1);
    }

    /// Skip spaces.
    pub fn spaces(&mut self) {
        while self.peek() == Some(b' ') {
            self.bump();
        }
    }

    /// Whether only the line ending (or nothing) is left.
    pub fn at_end(&self) -> bool {
        matches!(self.peek(), None | Some(b'\r') | Some(b'\n'))
    }

    /// The rest of the line as text, without the line ending.
    pub fn rest(&mut self) -> String {
        let rest = self.bytes.get(self.pos..).unwrap_or(&[]);
        let end = rest.iter().position(|&b| b == b'\r' || b == b'\n').unwrap_or(rest.len());
        self.pos = self.pos.saturating_add(end);
        String::from_utf8_lossy(rest.get(..end).unwrap_or(&[])).into_owned()
    }

    /// Text up to (not including) `stop` on this line, consuming `stop` too.
    pub fn until(&mut self, stop: u8) -> Option<String> {
        let rest = self.bytes.get(self.pos..)?;
        let end = rest.iter().position(|&b| b == stop || b == b'\r' || b == b'\n')?;
        if rest.get(end) != Some(&stop) {
            return None;
        }
        self.pos = self.pos.saturating_add(end).saturating_add(1);
        Some(String::from_utf8_lossy(rest.get(..end).unwrap_or(&[])).into_owned())
    }

    /// The next space-delimited word, for tags and keywords.
    pub fn word(&mut self) -> String {
        self.spaces();
        let rest = self.bytes.get(self.pos..).unwrap_or(&[]);
        let end = rest.iter().position(|&b| matches!(b, b' ' | b'\r' | b'\n' | b'[' | b'(')).unwrap_or(rest.len());
        self.pos = self.pos.saturating_add(end);
        String::from_utf8_lossy(rest.get(..end).unwrap_or(&[])).into_owned()
    }

    /// The next value, or `None` at the end of the line or of a list.
    pub fn value(&mut self) -> Result<Option<Value>> {
        self.value_at(0)
    }

    fn value_at(&mut self, depth: usize) -> Result<Option<Value>> {
        self.spaces();
        let Some(b) = self.peek() else { return Ok(None) };
        match b {
            b'\r' | b'\n' | b')' => Ok(None),
            b'(' => {
                if depth >= MAX_DEPTH {
                    return Err(Error::Protocol("lists nested too deeply".into()));
                }
                self.bump();
                let mut items = Vec::new();
                loop {
                    match self.value_at(depth.saturating_add(1))? {
                        Some(v) => {
                            if items.len() >= MAX_ITEMS {
                                return Err(Error::TooLarge("list"));
                            }
                            items.push(v);
                        }
                        None => {
                            if self.peek() != Some(b')') {
                                return Err(Error::Protocol("unclosed list".into()));
                            }
                            self.bump();
                            return Ok(Some(Value::List(items)));
                        }
                    }
                }
            }
            b'"' => self.quoted().map(Some),
            b'{' => self.literal().map(Some),
            _ => Ok(Some(self.atom())),
        }
    }

    fn quoted(&mut self) -> Result<Value> {
        self.bump();
        let mut out = Vec::new();
        loop {
            let Some(b) = self.peek() else { return Err(Error::Protocol("unterminated string".into())) };
            self.bump();
            match b {
                b'"' => return Ok(Value::Str(out)),
                b'\\' => {
                    let Some(next) = self.peek() else { return Err(Error::Protocol("unterminated string".into())) };
                    self.bump();
                    out.push(next);
                }
                b'\r' | b'\n' => return Err(Error::Protocol("line break in a quoted string".into())),
                _ => out.push(b),
            }
        }
    }

    fn literal(&mut self) -> Result<Value> {
        self.bump();
        let Some(size) = self.until(b'}') else { return Err(Error::Protocol("bad literal".into())) };
        let n: usize = size.trim_end_matches('+').parse().map_err(|_| Error::Protocol("bad literal size".into()))?;
        if self.peek() == Some(b'\r') {
            self.bump();
        }
        if self.peek() != Some(b'\n') {
            return Err(Error::Protocol("literal without a line break".into()));
        }
        self.bump();
        let end = self.pos.checked_add(n).ok_or(Error::TooLarge("literal"))?;
        let bytes = self.bytes.get(self.pos..end).ok_or_else(|| Error::Protocol("literal cut short".into()))?;
        self.pos = end;
        Ok(Value::Str(bytes.to_vec()))
    }

    /// An atom; `[...]` sections (which may hold spaces and lists) stay part of it.
    fn atom(&mut self) -> Value {
        let start = self.pos;
        let mut brackets = 0usize;
        while let Some(b) = self.peek() {
            match b {
                b'[' => brackets = brackets.saturating_add(1),
                b']' if brackets > 0 => brackets -= 1,
                b'\r' | b'\n' => break,
                b' ' | b'(' | b')' | b'"' | b'{' if brackets == 0 => break,
                _ => {}
            }
            self.bump();
        }
        let text = String::from_utf8_lossy(self.bytes.get(start..self.pos).unwrap_or(&[])).into_owned();
        if text.eq_ignore_ascii_case("NIL") {
            Value::Nil
        } else if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            text.parse().map_or(Value::Atom(text), Value::Number)
        } else {
            Value::Atom(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(s: &[u8]) -> Vec<Value> {
        let mut c = Cursor::new(s);
        let mut out = Vec::new();
        while let Some(v) = c.value().unwrap() {
            out.push(v);
        }
        out
    }

    #[test]
    fn values_parse() {
        let v = values(b"1 FETCH (UID 42 FLAGS (\\Seen $Junk) BODY[HEADER.FIELDS (FROM)] {5}\r\nhello NIL \"a \\\"q\\\"\")\r\n");
        assert_eq!(v.first(), Some(&Value::Number(1)));
        assert_eq!(v.get(1), Some(&Value::Atom("FETCH".into())));
        let Some(Value::List(items)) = v.get(2) else { panic!("{v:?}") };
        assert_eq!(items.get(1), Some(&Value::Number(42)));
        assert_eq!(items.get(3), Some(&Value::List(vec![Value::Atom("\\Seen".into()), Value::Atom("$Junk".into())])));
        assert_eq!(items.get(4), Some(&Value::Atom("BODY[HEADER.FIELDS (FROM)]".into())));
        assert_eq!(items.get(5), Some(&Value::Str(b"hello".to_vec())));
        assert_eq!(items.get(6), Some(&Value::Nil));
        assert_eq!(items.get(7), Some(&Value::Str(b"a \"q\"".to_vec())));
    }

    #[test]
    fn reader_frames_literals() {
        let mut stream: &[u8] = b"* 1 FETCH (BODY[] {8}\r\nab\r\ncd\r\n)\r\n* OK done\r\n";
        let mut r = Reader::new();
        let first = r.response(&mut stream).unwrap();
        assert_eq!(first, b"* 1 FETCH (BODY[] {8}\r\nab\r\ncd\r\n)\r\n");
        assert_eq!(r.response(&mut stream).unwrap(), b"* OK done\r\n");
        assert_eq!(r.response(&mut stream), Err(Error::Closed));
    }

    #[test]
    fn hostile_input_is_rejected_not_panicked() {
        let deep = "(".repeat(MAX_DEPTH + 5);
        assert!(Cursor::new(deep.as_bytes()).value().is_err());
        for s in [&b"{99}\r\nshort"[..], b"{x}\r\n", b"\"open", b"(a b", b"{18446744073709551616}\r\n", b"{5}"] {
            let _ = Cursor::new(s).value();
        }
        let mut huge: &[u8] = b"* 1 FETCH (BODY[] {999999999999}\r\n";
        assert_eq!(Reader::new().response(&mut huge), Err(Error::TooLarge("literal")));
        let mut cut: &[u8] = b"* 1 FETCH (BODY[] {10}\r\nabc";
        assert_eq!(Reader::new().response(&mut cut), Err(Error::Closed));
    }
}
