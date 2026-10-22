//! Accounts, folders, messages and addresses.

use serde::{Deserialize, Serialize};

use crate::{DateTime, Error, MAX_FIELD_LEN, Result, clamp_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FolderId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MessageId(pub u64);

/// What a folder is for, so the interface can name and order it in the person's language whatever
/// the server calls it (RFC 6154 special-use).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FolderRole {
    Inbox,
    Drafts,
    Sent,
    Archive,
    Junk,
    Trash,
    Outbox,
    Other,
}

impl FolderRole {
    /// Sort key for the folder pane: Inbox first, then the usual Outlook order.
    pub fn order(self) -> u8 {
        match self {
            FolderRole::Inbox => 0,
            FolderRole::Drafts => 1,
            FolderRole::Sent => 2,
            FolderRole::Outbox => 3,
            FolderRole::Archive => 4,
            FolderRole::Junk => 5,
            FolderRole::Trash => 6,
            FolderRole::Other => 7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
    pub role: FolderRole,
}

/// How an account reaches its server. Only `Local` works in M0; the others are recorded so
/// settings files written now stay valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountKind {
    Local,
    Imap { imap_host: String, imap_port: u16, smtp_host: String, smtp_port: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub address: Address,
    pub kind: AccountKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

impl Address {
    pub fn new(name: Option<&str>, email: &str) -> Result<Address> {
        let email = email.trim();
        if !is_plausible_email(email) {
            return Err(Error::invalid("address", format!("{email:?}")));
        }
        let name = name.map(|n| clamp_text(n.trim(), 1024).to_string()).filter(|n| !n.is_empty());
        Ok(Address { name, email: email.to_string() })
    }

    /// `Name <user@host>`, `"Name" <user@host>` or a bare `user@host`.
    pub fn parse(s: &str) -> Result<Address> {
        let s = s.trim();
        match (s.rfind('<'), s.rfind('>')) {
            (Some(lt), Some(gt)) if lt < gt => {
                let email = s.get(lt + 1..gt).unwrap_or("");
                let name = s.get(..lt).unwrap_or("").trim().trim_matches('"').trim();
                Address::new(Some(name), email)
            }
            _ => Address::new(None, s),
        }
    }

    /// A comma-separated list. Commas inside quotes belong to the name.
    pub fn parse_list(s: &str) -> Result<Vec<Address>> {
        let mut out = Vec::new();
        let mut start = 0;
        let mut quoted = false;
        for (i, c) in s.char_indices() {
            match c {
                '"' => quoted = !quoted,
                ',' | ';' if !quoted => {
                    push_part(&mut out, s.get(start..i).unwrap_or(""))?;
                    start = i + c.len_utf8();
                }
                _ => {}
            }
        }
        push_part(&mut out, s.get(start..).unwrap_or(""))?;
        Ok(out)
    }

    /// What the message list shows: the name if there is one, otherwise the address.
    pub fn display(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.email)
    }
}

fn push_part(out: &mut Vec<Address>, part: &str) -> Result<()> {
    const MAX_RECIPIENTS: usize = 10_000;
    if part.trim().is_empty() {
        return Ok(());
    }
    if out.len() >= MAX_RECIPIENTS {
        return Err(Error::TooLarge { what: "recipient list", size: out.len() + 1, limit: MAX_RECIPIENTS });
    }
    out.push(Address::parse(part)?);
    Ok(())
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(n) if n.contains([',', '"', '<', '>', ';', '@']) => write!(f, "\"{}\" <{}>", n.replace('"', "'"), self.email),
            Some(n) => write!(f, "{n} <{}>", self.email),
            None => f.write_str(&self.email),
        }
    }
}

/// One `@`, something on each side, no spaces or angle brackets, at most 254 bytes (RFC 5321).
fn is_plausible_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else { return false };
    s.len() <= 254
        && !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && !s.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';' | '"'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub mime_type: String,
    pub size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub folder: FolderId,
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    /// When it was sent, in the sender's wall-clock time (UTC offset applied by the parser).
    pub date: Option<DateTime>,
    /// Plain-text body. HTML-only mail is reduced to text by `ibercorreo-mime`; remote content is
    /// never fetched.
    pub body: String,
    pub attachments: Vec<Attachment>,
    pub seen: bool,
    pub flagged: bool,
}

impl Message {
    /// A blank message in `folder`; callers fill the rest.
    pub fn new(id: MessageId, folder: FolderId) -> Message {
        Message {
            id,
            folder,
            from: None,
            to: Vec::new(),
            cc: Vec::new(),
            subject: String::new(),
            date: None,
            body: String::new(),
            attachments: Vec::new(),
            seen: false,
            flagged: false,
        }
    }

    pub fn set_subject(&mut self, s: &str) {
        self.subject = clamp_text(s, MAX_FIELD_LEN).to_string();
    }

    /// The first line of the body, for the message list preview.
    pub fn preview(&self, max_chars: usize) -> String {
        let line = self.body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        let mut out: String = line.chars().take(max_chars).collect();
        if line.chars().count() > max_chars {
            out.push('…');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addresses() {
        let a = Address::parse("Ana Souza <ana@example.org>").unwrap();
        assert_eq!(a.name.as_deref(), Some("Ana Souza"));
        assert_eq!(a.email, "ana@example.org");
        let b = Address::parse("  bob@example.org ").unwrap();
        assert_eq!(b.name, None);
        assert_eq!(b.display(), "bob@example.org");
        let c = Address::parse("\"Silva, João\" <joao@example.pt>").unwrap();
        assert_eq!(c.to_string(), "\"Silva, João\" <joao@example.pt>");
        for bad in ["", "no-at-sign", "@example.org", "a@", "a@b@c", "a b@c.d", "<>", "Name <>", "><"] {
            assert!(Address::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parses_lists_with_quoted_commas() {
        let l = Address::parse_list("\"Silva, João\" <joao@example.pt>, ana@example.org; ,").unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l.first().and_then(|a| a.name.as_deref()), Some("Silva, João"));
        assert!(Address::parse_list("").unwrap().is_empty());
        assert!(Address::parse_list("ok@x.y, broken").is_err());
    }

    #[test]
    fn preview_takes_first_text_line() {
        let mut m = Message::new(MessageId(1), FolderId(0));
        m.body = "\n\n  Olá, equipa! Segue a agenda da reunião.\nSecond".into();
        assert_eq!(m.preview(10), "Olá, equip…");
        m.body.clear();
        assert_eq!(m.preview(10), "");
    }
}
