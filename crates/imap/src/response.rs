//! Turning one framed response into a typed [`Response`] (RFC 9051 §7). Responses this client
//! does not use are kept as [`Untagged::Other`] rather than rejected.

use crate::wire::{Cursor, Value};
use crate::{Error, Result, utf7};

/// The status word of a status response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    No,
    Bad,
    PreAuth,
    Bye,
}

impl Status {
    fn parse(word: &str) -> Option<Self> {
        Some(match word.to_ascii_uppercase().as_str() {
            "OK" => Status::Ok,
            "NO" => Status::No,
            "BAD" => Status::Bad,
            "PREAUTH" => Status::PreAuth,
            "BYE" => Status::Bye,
            _ => return None,
        })
    }
}

/// A response code (`[UIDVALIDITY 3857529045]`, RFC 9051 §7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Code {
    UidValidity(u32),
    UidNext(u32),
    Capability(Vec<String>),
    PermanentFlags(Vec<String>),
    /// `READ-ONLY` / `READ-WRITE`, `ALERT`, `AUTHENTICATIONFAILED` and the rest, upper-cased with any argument.
    Other(String),
}

/// One folder from `LIST`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    /// The name as the server spells it (modified UTF-7), for use in commands.
    pub raw: String,
    /// The name for people (decoded).
    pub name: String,
    /// The hierarchy delimiter, if the server uses one.
    pub delimiter: Option<char>,
    /// Attributes such as `\Noselect`, `\HasChildren` and the RFC 6154 special uses (`\Sent`, `\Trash`).
    pub attributes: Vec<String>,
}

impl Mailbox {
    pub fn has(&self, attribute: &str) -> bool {
        self.attributes.iter().any(|a| a.eq_ignore_ascii_case(attribute))
    }

    /// Whether the mailbox can be selected (`\Noselect` / `\NonExistent` cannot).
    pub fn selectable(&self) -> bool {
        !self.has("\\Noselect") && !self.has("\\NonExistent")
    }
}

/// The data items of one `FETCH` response that this client asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fetch {
    pub seq: u32,
    pub uid: Option<u32>,
    pub flags: Option<Vec<String>>,
    pub size: Option<u32>,
    /// `INTERNALDATE` as written (`"17-Jul-1996 02:44:25 -0700"`).
    pub internal_date: Option<String>,
    /// The whole message (`BODY[]` or `RFC822`).
    pub body: Option<Vec<u8>>,
}

/// An untagged (`*`) response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Untagged {
    Status { status: Status, code: Option<Code>, text: String },
    Capability(Vec<String>),
    List(Mailbox),
    Flags(Vec<String>),
    Exists(u32),
    Recent(u32),
    Expunge(u32),
    Search(Vec<u32>),
    Fetch(Fetch),
    Other(String),
}

/// One response from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Tagged {
        tag: String,
        status: Status,
        code: Option<Code>,
        text: String,
    },
    Untagged(Untagged),
    /// `+`: the server waits for the rest of a command.
    Continue(String),
}

/// Parse one framed response (from [`crate::wire::Reader::response`]).
pub fn parse(bytes: &[u8]) -> Result<Response> {
    let mut c = Cursor::new(bytes);
    if c.peek() == Some(b'+') {
        let _ = c.word();
        c.spaces();
        return Ok(Response::Continue(c.rest()));
    }
    let tag = c.word();
    if tag.is_empty() {
        return Err(Error::Protocol("empty response".into()));
    }
    if tag == "*" {
        return untagged(&mut c).map(Response::Untagged);
    }
    let word = c.word();
    let status = Status::parse(&word).ok_or_else(|| Error::Protocol(format!("tagged response without a status: {word}")))?;
    let (code, text) = resp_text(&mut c);
    Ok(Response::Tagged { tag, status, code, text })
}

fn untagged(c: &mut Cursor) -> Result<Untagged> {
    let first = c.word();
    if let Ok(n) = first.parse::<u32>() {
        let what = c.word().to_ascii_uppercase();
        return Ok(match what.as_str() {
            "EXISTS" => Untagged::Exists(n),
            "RECENT" => Untagged::Recent(n),
            "EXPUNGE" => Untagged::Expunge(n),
            "FETCH" => Untagged::Fetch(fetch(n, c)?),
            _ => Untagged::Other(format!("{n} {what} {}", c.rest())),
        });
    }
    let upper = first.to_ascii_uppercase();
    if let Some(status) = Status::parse(&upper) {
        let (code, text) = resp_text(c);
        return Ok(Untagged::Status { status, code, text });
    }
    Ok(match upper.as_str() {
        "CAPABILITY" => Untagged::Capability(words(&c.rest())),
        "LIST" | "LSUB" => Untagged::List(list(c)?),
        "FLAGS" => Untagged::Flags(match c.value()? {
            Some(Value::List(items)) => items.iter().filter_map(Value::text).collect(),
            _ => Vec::new(),
        }),
        "SEARCH" => {
            let mut ids = Vec::new();
            while let Some(v) = c.value()? {
                // `(MODSEQ n)` (CONDSTORE) and anything else that is not a number is skipped.
                if let Some(n) = v.number() {
                    ids.push(n);
                }
            }
            Untagged::Search(ids)
        }
        _ => Untagged::Other(format!("{upper} {}", c.rest())),
    })
}

fn words(s: &str) -> Vec<String> {
    s.split_ascii_whitespace().map(str::to_owned).collect()
}

/// `resp-text`: an optional `[code]`, then human-readable text.
fn resp_text(c: &mut Cursor) -> (Option<Code>, String) {
    c.spaces();
    let mut code = None;
    if c.peek() == Some(b'[') {
        let _ = c.until(b'[');
        if let Some(inner) = c.until(b']') {
            code = Some(parse_code(&inner));
        }
        c.spaces();
    }
    (code, c.rest())
}

fn parse_code(inner: &str) -> Code {
    let (name, arg) = inner.split_once(' ').unwrap_or((inner, ""));
    let name = name.to_ascii_uppercase();
    let list = |arg: &str| words(arg.trim_start_matches('(').trim_end_matches(')'));
    match name.as_str() {
        "UIDVALIDITY" => arg.trim().parse().map_or_else(|_| Code::Other(name.clone()), Code::UidValidity),
        "UIDNEXT" => arg.trim().parse().map_or_else(|_| Code::Other(name.clone()), Code::UidNext),
        "CAPABILITY" => Code::Capability(words(arg)),
        "PERMANENTFLAGS" => Code::PermanentFlags(list(arg)),
        _ if arg.is_empty() => Code::Other(name),
        _ => Code::Other(format!("{name} {arg}")),
    }
}

fn list(c: &mut Cursor) -> Result<Mailbox> {
    let attributes = match c.value()? {
        Some(Value::List(items)) => items.iter().filter_map(Value::text).collect(),
        _ => return Err(Error::Protocol("LIST without attributes".into())),
    };
    let delimiter = match c.value()? {
        Some(Value::Nil) => None,
        Some(v) => v.text().and_then(|s| s.chars().next()),
        None => return Err(Error::Protocol("LIST without a delimiter".into())),
    };
    let raw = match c.value()? {
        Some(Value::Nil) | None => return Err(Error::Protocol("LIST without a mailbox".into())),
        Some(v) => v.text().ok_or_else(|| Error::Protocol("LIST mailbox is a list".into()))?,
    };
    // INBOX is case-insensitive (RFC 9051 §5.1); spell it one way.
    let raw = if raw.eq_ignore_ascii_case("INBOX") { "INBOX".to_owned() } else { raw };
    Ok(Mailbox { name: utf7::decode(&raw), raw, delimiter, attributes })
}

fn fetch(seq: u32, c: &mut Cursor) -> Result<Fetch> {
    let Some(Value::List(items)) = c.value()? else { return Err(Error::Protocol("FETCH without a list".into())) };
    let mut out = Fetch { seq, ..Fetch::default() };
    let mut it = items.into_iter();
    while let Some(key) = it.next() {
        let Value::Atom(key) = key else { return Err(Error::Protocol("FETCH item name is not an atom".into())) };
        let Some(value) = it.next() else { return Err(Error::Protocol(format!("FETCH item {key} without a value"))) };
        let key = key.to_ascii_uppercase();
        match key.as_str() {
            "UID" => out.uid = value.number(),
            "FLAGS" => {
                if let Value::List(flags) = value {
                    out.flags = Some(flags.iter().filter_map(Value::text).collect());
                }
            }
            "RFC822.SIZE" => out.size = value.number(),
            "INTERNALDATE" => out.internal_date = value.text(),
            // Only the whole message: a partial `BODY[]<0>` or a section is not the message.
            "BODY[]" | "RFC822" => {
                if let Value::Str(bytes) = value {
                    out.body = Some(bytes);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_examples() {
        assert_eq!(
            parse(b"* OK [UIDVALIDITY 3857529045] UIDs valid\r\n").unwrap(),
            Response::Untagged(Untagged::Status { status: Status::Ok, code: Some(Code::UidValidity(3857529045)), text: "UIDs valid".into() })
        );
        assert_eq!(parse(b"* 172 EXISTS\r\n").unwrap(), Response::Untagged(Untagged::Exists(172)));
        assert_eq!(
            parse(b"a1 NO [AUTHENTICATIONFAILED] Invalid credentials\r\n").unwrap(),
            Response::Tagged {
                tag: "a1".into(),
                status: Status::No,
                code: Some(Code::Other("AUTHENTICATIONFAILED".into())),
                text: "Invalid credentials".into()
            }
        );
        assert_eq!(parse(b"+ Ready\r\n").unwrap(), Response::Continue("Ready".into()));
        assert_eq!(
            parse(b"* CAPABILITY IMAP4rev1 IMAP4rev2 MOVE UIDPLUS\r\n").unwrap(),
            Response::Untagged(Untagged::Capability(vec!["IMAP4rev1".into(), "IMAP4rev2".into(), "MOVE".into(), "UIDPLUS".into()]))
        );
        assert_eq!(parse(b"* SEARCH 2 84 882\r\n").unwrap(), Response::Untagged(Untagged::Search(vec![2, 84, 882])));
        assert_eq!(parse(b"* SEARCH\r\n").unwrap(), Response::Untagged(Untagged::Search(vec![])));
    }

    #[test]
    fn list_decodes_names_and_flags() {
        let Response::Untagged(Untagged::List(m)) =
            parse(format!("* LIST (\\HasNoChildren \\Sent) \"/\" \"{}\"\r\n", utf7::encode("Надіслані")).as_bytes()).unwrap()
        else {
            panic!()
        };
        assert_eq!(m.name, "Надіслані");
        assert_eq!(m.delimiter, Some('/'));
        assert!(m.has("\\sent") && m.selectable());
        let Response::Untagged(Untagged::List(m)) = parse(b"* LIST (\\Noselect) NIL inbox\r\n").unwrap() else { panic!() };
        assert_eq!((m.raw.as_str(), m.delimiter, m.selectable()), ("INBOX", None, false));
        let Response::Untagged(Untagged::List(m)) = parse(b"* LIST () \".\" {4}\r\nA\"B)\r\n").unwrap() else { panic!() };
        assert_eq!(m.raw, "A\"B)");
    }

    #[test]
    fn fetch_items() {
        let r = parse(b"* 12 FETCH (FLAGS (\\Seen) INTERNALDATE \"17-Jul-1996 02:44:25 -0700\" RFC822.SIZE 4286 UID 900 BODY[] {5}\r\nHi!\r\n)\r\n")
            .unwrap();
        let Response::Untagged(Untagged::Fetch(f)) = r else { panic!() };
        assert_eq!(f.seq, 12);
        assert_eq!(f.uid, Some(900));
        assert_eq!(f.size, Some(4286));
        assert_eq!(f.flags, Some(vec!["\\Seen".into()]));
        assert_eq!(f.internal_date.as_deref(), Some("17-Jul-1996 02:44:25 -0700"));
        assert_eq!(f.body.as_deref(), Some(&b"Hi!\r\n"[..]));
    }

    #[test]
    fn hostile_responses_do_not_panic() {
        for s in [
            &b""[..],
            b"\r\n",
            b"*",
            b"* \r\n",
            b"* 1 FETCH\r\n",
            b"* 1 FETCH (UID)\r\n",
            b"* 1 FETCH ((x) 1)\r\n",
            b"* 99999999999 EXISTS\r\n",
            b"* LIST\r\n",
            b"* LIST () \r\n",
            b"* OK [UIDVALIDITY x\r\n",
            b"* OK [\r\n",
            b"a1\r\n",
            b"a1 MAYBE\r\n",
            b"* FLAGS x\r\n",
            b"\xff\xfe \xff\r\n",
        ] {
            let _ = parse(s);
        }
    }
}
