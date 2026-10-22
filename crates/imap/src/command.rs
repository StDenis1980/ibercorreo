//! Writing commands (RFC 9051 §6, §9). Every argument that comes from a person or a server is
//! quoted or sent as a literal, so a name or password can never end the command early and smuggle
//! a second one in.

use crate::{Error, Result, utf7};

/// A piece of a command: text, or a literal the server may have to accept first (`+`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Part {
    Text(Vec<u8>),
    Literal(Vec<u8>),
}

/// A command without its tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub(crate) parts: Vec<Part>,
}

/// `ATOM-CHAR` (RFC 9051 §9): printable ASCII except the specials.
fn atom_char(b: u8) -> bool {
    (0x21..0x7f).contains(&b) && !matches!(b, b'(' | b')' | b'{' | b'%' | b'*' | b'"' | b'\\' | b']')
}

impl Command {
    /// A command starting with its name (`SELECT`, `UID FETCH`).
    pub fn new(name: &'static str) -> Self {
        Command { parts: vec![Part::Text(name.as_bytes().to_vec())] }
    }

    fn text(&mut self, bytes: &[u8]) {
        if let Some(Part::Text(last)) = self.parts.last_mut() {
            last.extend_from_slice(bytes);
        } else {
            self.parts.push(Part::Text(bytes.to_vec()));
        }
    }

    /// Text this crate wrote itself (keywords, validated sets and flags), after a space.
    pub(crate) fn raw(mut self, s: &str) -> Self {
        self.text(b" ");
        self.text(s.as_bytes());
        self
    }

    /// An `astring`: an atom when that is safe, else a quoted string, else a literal.
    pub fn astring(mut self, value: &[u8]) -> Self {
        self.text(b" ");
        if !value.is_empty() && value.iter().all(|&b| atom_char(b)) {
            self.text(value);
        } else if value.len() <= 1000 && value.iter().all(|&b| (0x20..0x7f).contains(&b)) {
            let mut q = Vec::with_capacity(value.len().saturating_add(2));
            q.push(b'"');
            for &b in value {
                if b == b'"' || b == b'\\' {
                    q.push(b'\\');
                }
                q.push(b);
            }
            q.push(b'"');
            self.text(&q);
        } else {
            self.parts.push(Part::Literal(value.to_vec()));
        }
        self
    }

    /// A mailbox name, encoded in modified UTF-7.
    pub fn mailbox(self, name: &str) -> Self {
        let encoded = if name.eq_ignore_ascii_case("INBOX") { "INBOX".to_owned() } else { utf7::encode(name) };
        self.astring(encoded.as_bytes())
    }

    /// A message for `APPEND`: always a literal.
    pub fn literal(mut self, bytes: &[u8]) -> Self {
        self.text(b" ");
        self.parts.push(Part::Literal(bytes.to_vec()));
        self
    }

    /// The command as one line, for logs: literals are shown by size only (they may hold a password).
    pub fn redacted(&self) -> String {
        self.parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => String::from_utf8_lossy(t).into_owned(),
                Part::Literal(l) => format!("{{{}}}", l.len()),
            })
            .collect()
    }
}

/// A UID set from UIDs, with runs written as ranges (`1:3,7`). Empty input is an error: IMAP has no empty set.
pub fn uid_set(uids: &[u32]) -> Result<String> {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = String::new();
    let mut it = sorted.into_iter().peekable();
    while let Some(start) = it.next() {
        let mut end = start;
        while let Some(&next) = it.peek() {
            if Some(next) != end.checked_add(1) {
                break;
            }
            end = next;
            it.next();
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == end {
            out.push_str(&start.to_string());
        } else {
            out.push_str(&format!("{start}:{end}"));
        }
    }
    if out.is_empty() { Err(Error::Invalid("empty UID set".into())) } else { Ok(out) }
}

/// Check a sequence set someone else wrote (`1:*`, `4,7:9`) before it goes into a command.
pub fn check_set(set: &str) -> Result<&str> {
    let ok = !set.is_empty()
        && set.split(',').all(|item| {
            let mut ends = item.split(':');
            let one = |s: Option<&str>| {
                s.is_some_and(|s| s == "*" || (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && s.parse::<u32>().is_ok_and(|n| n > 0)))
            };
            let first = ends.next();
            match (ends.next(), ends.next()) {
                (None, _) => one(first),
                (Some(second), None) => one(first) && one(Some(second)),
                _ => false,
            }
        });
    if ok { Ok(set) } else { Err(Error::Invalid(format!("not a sequence set: {set}"))) }
}

/// Check flags (`\Seen`, `$Forwarded`, keywords) before they go into a command.
pub fn check_flags(flags: &[&str]) -> Result<String> {
    for f in flags {
        let body = f.strip_prefix('\\').unwrap_or(f);
        if body.is_empty() || !body.bytes().all(atom_char) {
            return Err(Error::Invalid(format!("not a flag: {f}")));
        }
    }
    Ok(format!("({})", flags.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_cannot_break_out() {
        let c = Command::new("LOGIN").astring(b"denys").astring(b"pa\"ss\\word").astring(b"two\r\nlines").astring(b"");
        assert_eq!(
            c.parts,
            vec![Part::Text(b"LOGIN denys \"pa\\\"ss\\\\word\" ".to_vec()), Part::Literal(b"two\r\nlines".to_vec()), Part::Text(b" \"\"".to_vec())]
        );
        assert_eq!(c.redacted(), "LOGIN denys \"pa\\\"ss\\\\word\" {10} \"\"");
        let c = Command::new("SELECT").mailbox("Вхідні & Co");
        assert_eq!(c.redacted(), format!("SELECT \"{}\"", utf7::encode("Вхідні & Co")));
        assert_eq!(Command::new("SELECT").mailbox("inbox").redacted(), "SELECT INBOX");
        assert_eq!(Command::new("X").astring("пароль".as_bytes()).parts.len(), 2);
    }

    #[test]
    fn sets_and_flags() {
        assert_eq!(uid_set(&[7, 1, 2, 3, 3, 9, 10, u32::MAX]).unwrap(), format!("1:3,7,9:10,{}", u32::MAX));
        assert!(uid_set(&[]).is_err());
        for good in ["1", "1:*", "4,7:9", "*"] {
            assert!(check_set(good).is_ok(), "{good}");
        }
        for bad in ["", "0", "1:", "a", "1 2", "1\r\nA DELETE x", "1:2:3", ",", "99999999999"] {
            assert!(check_set(bad).is_err(), "{bad}");
        }
        assert_eq!(check_flags(&["\\Seen", "$Forwarded"]).unwrap(), "(\\Seen $Forwarded)");
        for bad in ["", "\\", "a b", "x)", "\\Seen\r\n"] {
            assert!(check_flags(&[bad]).is_err(), "{bad}");
        }
    }
}
