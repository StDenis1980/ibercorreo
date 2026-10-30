//! Accounts, folders, messages and addresses.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{DateTime, Error, MAX_FIELD_LEN, Result, clamp_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FolderId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MessageId(pub u64);

/// An account in the store. Stores from before several accounts hold one, with id 0, which owns
/// every folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(pub u32);

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
    /// The account whose mail it holds.
    #[serde(default)]
    pub account: AccountId,
    pub name: String,
    pub role: FolderRole,
    /// The server mailbox this folder mirrors; `None` for a folder that only exists here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteFolder>,
}

/// Where a folder lives on the account's IMAP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFolder {
    /// The mailbox name as the server spells it (modified UTF-7), used in commands.
    pub path: String,
    /// `UIDVALIDITY`: when the server changes it, every UID it gave before is void (RFC 9051 §2.3.1.1).
    pub uid_validity: u32,
}

/// What the server last reported for a message. Where the message's own fields differ (it was
/// read, flagged or moved here), the change is still to be sent to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRef {
    /// The folder whose server mailbox holds the message.
    pub folder: FolderId,
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
}

/// How a connection to a server is encrypted. There is no unencrypted choice: passwords never
/// travel in the clear.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Security {
    /// TLS from the first byte (IMAP 993, SMTP 465); RFC 8314 recommends it.
    #[default]
    Tls,
    /// Plain first, then `STARTTLS` (IMAP 143, SMTP 587).
    StartTls,
}

impl Security {
    pub fn default_imap_port(self) -> u16 {
        match self {
            Security::Tls => 993,
            Security::StartTls => 143,
        }
    }

    pub fn default_smtp_port(self) -> u16 {
        match self {
            Security::Tls => 465,
            Security::StartTls => 587,
        }
    }
}

/// How an account reaches its server. The password is never here: it lives in the operating
/// system's credential store, under the account's user name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountKind {
    Local,
    Imap {
        imap_host: String,
        imap_port: u16,
        smtp_host: String,
        smtp_port: u16,
        /// The sign-in name; empty means the email address.
        #[serde(default)]
        user: String,
        #[serde(default)]
        imap_security: Security,
        #[serde(default)]
        smtp_security: Security,
        /// The OAuth 2.0 provider signed in with ([`OAUTH_PROVIDERS`]: `google`, `microsoft`);
        /// empty means a password.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        oauth: String,
    },
}

/// A provider whose servers take OAuth 2.0 sign-in (XOAUTH2) in place of a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OAuthProvider {
    /// The id kept in the account (`AccountKind::Imap::oauth`).
    pub id: &'static str,
    pub name: &'static str,
    /// The domains of its mail servers: a server is the provider's when its name is one of these
    /// or ends in `.` and one of these.
    pub domains: &'static [&'static str],
    /// Its SMTP server files what it sends in the Sent mailbox itself, so no copy is saved there.
    pub saves_sent: bool,
    /// Its IMAP server (port 993, TLS).
    pub imap_host: &'static str,
    /// Its SMTP server, and whether it takes STARTTLS on port 587 (else TLS on 465).
    pub smtp_host: &'static str,
    pub smtp_starttls: bool,
    /// Address domains whose mail it serves, so choosing it can fill in the servers.
    pub mail_domains: &'static [&'static str],
    /// Where a person makes an app password for it, when that is the way in offered first: no
    /// browser sign-in, so no OAuth app review by the provider is needed.
    pub app_password_url: Option<&'static str>,
    /// The provider has not reviewed this app's sign-in yet, so its page warns before it goes on.
    pub unverified: bool,
}

pub const OAUTH_PROVIDERS: &[OAuthProvider] = &[
    OAuthProvider {
        id: "google",
        name: "Google",
        domains: &["gmail.com", "googlemail.com"],
        saves_sent: true,
        imap_host: "imap.gmail.com",
        smtp_host: "smtp.gmail.com",
        smtp_starttls: false,
        mail_domains: &["gmail.com", "googlemail.com"],
        app_password_url: Some("https://myaccount.google.com/apppasswords"),
        // Google's review of full mail access waits until after the public release.
        unverified: true,
    },
    OAuthProvider {
        id: "microsoft",
        name: "Microsoft",
        domains: &["office365.com", "outlook.com"],
        saves_sent: false,
        imap_host: "outlook.office365.com",
        smtp_host: "smtp.office365.com",
        smtp_starttls: true,
        mail_domains: &["outlook.com", "hotmail.com", "live.com", "msn.com", "outlook.es", "hotmail.es"],
        // Outlook.com no longer takes passwords over IMAP: browser sign-in only.
        app_password_url: None,
        unverified: false,
    },
];

impl OAuthProvider {
    pub fn get(id: &str) -> Option<&'static OAuthProvider> {
        OAUTH_PROVIDERS.iter().find(|p| p.id == id)
    }

    /// The provider that serves mail for `email` (`ana@gmail.com` is Google's), by its domain.
    pub fn for_address(email: &str) -> Option<&'static OAuthProvider> {
        let domain = email.trim().rsplit_once('@')?.1.trim_end_matches('.').to_ascii_lowercase();
        OAUTH_PROVIDERS.iter().find(|p| p.mail_domains.iter().any(|d| domain == *d))
    }

    /// The provider whose mail server `host` is (`imap.gmail.com` is Google's).
    pub fn for_host(host: &str) -> Option<&'static OAuthProvider> {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        OAUTH_PROVIDERS.iter().find(|p| p.domains.iter().any(|d| host == *d || host.strip_suffix(d).is_some_and(|rest| rest.ends_with('.'))))
    }
}

impl Account {
    /// The OAuth 2.0 provider this account signs in with, if it does not use a password.
    pub fn oauth(&self) -> Option<&'static OAuthProvider> {
        match &self.kind {
            AccountKind::Imap { oauth, .. } if !oauth.is_empty() => OAuthProvider::get(oauth),
            _ => None,
        }
    }

    /// The sign-in name for an IMAP account: `user`, or the address when that is empty.
    pub fn login(&self) -> Option<&str> {
        match &self.kind {
            AccountKind::Imap { user, .. } if !user.is_empty() => Some(user),
            AccountKind::Imap { .. } => Some(&self.address.email),
            AccountKind::Local => None,
        }
    }

    /// Where this account's secret lives in the system credential store: the service name and
    /// the entry's key (a password under the sign-in name; an OAuth refresh token under
    /// `provider:login`). The app and `ibercorreo-cli` both read it here, so they find the same
    /// entry. `None` for an account with no sign-in.
    pub fn credential_entry(&self) -> Option<(&'static str, String)> {
        let login = self.login()?;
        Some(match self.oauth() {
            Some(p) => (OAUTH_KEYRING_SERVICE, format!("{}:{login}", p.id)),
            None => (KEYRING_SERVICE, login.to_string()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub id: AccountId,
    pub name: String,
    pub address: Address,
    pub kind: AccountKind,
    /// The account's CalDAV address (`https://…`: a server, principal or calendar), signed in to
    /// with the same name and password as the mail; empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub caldav: String,
    /// Text added to new messages, replies and forwards from this account (plain text, at most
    /// [`MAX_SIGNATURE_LEN`] bytes); empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature: String,
}

/// The credential store's service name for account passwords. It keeps the app's first name
/// ("Iber Correo"), so passwords saved before the rename to Tinta Correo are still found.
pub const KEYRING_SERVICE: &str = "Iber Correo";
/// The credential store's service name for OAuth refresh tokens (`provider:login`), kept like
/// [`KEYRING_SERVICE`].
pub const OAUTH_KEYRING_SERVICE: &str = "Iber Correo OAuth";

/// The longest signature kept.
pub const MAX_SIGNATURE_LEN: usize = 8 * 1024;

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

/// Largest invitation kept with a message (a real one is a few KiB, with its time zones).
pub const MAX_INVITE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub mime_type: String,
    pub size: usize,
}

/// What a message says, apart from its headers, so a store can leave it on disk until it is read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    /// Plain-text body. HTML-only mail is reduced to text by `ibercorreo-mime`; remote content is
    /// never fetched.
    pub body: String,
    /// The formatted body of an HTML message, made safe ([`crate::rich`]); `None` for plain mail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rich: Option<crate::rich::Rich>,
    /// The iCalendar part of a meeting invitation, a reply to one or a cancellation (iMIP, RFC
    /// 6047), as text: at most [`MAX_INVITE_BYTES`]. A message being sent carries its reply here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
}

/// Most characters kept as a message's [`Message::preview`] while its content is on disk.
pub const SNIPPET_CHARS: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub folder: FolderId,
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    /// Blind copies: sent to, never written into the message's headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<Address>,
    pub subject: String,
    /// When it was sent, in the sender's wall-clock time (UTC offset applied by the parser).
    pub date: Option<DateTime>,
    /// What the message says. `None` while a store keeps it on disk: the store reads it back
    /// when asked (`Store::content`). Written in line with the other fields when present.
    #[serde(flatten)]
    content: Option<Arc<Content>>,
    /// The start of the body, for the message list, kept while the content is on disk; with the
    /// content here, the preview is read from it instead.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    snippet: String,
    /// Whether the content has a formatted body and an invitation, kept like `snippet`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    html: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    has_invite: bool,
    pub attachments: Vec<Attachment>,
    pub seen: bool,
    pub flagged: bool,
    /// The server copy, for a synced message; `None` for one that only exists here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteRef>,
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
            bcc: Vec::new(),
            subject: String::new(),
            date: None,
            content: Some(Arc::default()),
            snippet: String::new(),
            html: false,
            has_invite: false,
            attachments: Vec::new(),
            seen: false,
            flagged: false,
            remote: None,
        }
    }

    pub fn set_subject(&mut self, s: &str) {
        self.subject = clamp_text(s, MAX_FIELD_LEN).to_string();
    }

    /// What the message says, if it is here: `None` while a store keeps it on disk.
    pub fn content(&self) -> Option<&Content> {
        self.content.as_deref()
    }

    /// The content shared, for a store that keeps it apart (an `Arc` copy, not the text).
    pub fn shared_content(&self) -> Option<&Arc<Content>> {
        self.content.as_ref()
    }

    /// The content to change, if it is here (copied first if it is shared).
    pub fn content_mut(&mut self) -> Option<&mut Content> {
        self.content.as_mut().map(Arc::make_mut)
    }

    /// Give the message its content (one read back from disk, or new).
    pub fn set_content(&mut self, content: Arc<Content>) {
        self.content = Some(content);
    }

    /// The message without its content, as a store writes it beside the content it keeps on
    /// disk: the preview is kept, so the message list needs no content.
    pub fn headers_only(&self) -> Message {
        let snippet = self.preview(SNIPPET_CHARS);
        Message { content: None, snippet, html: self.is_html(), has_invite: self.has_invite(), ..self.clone() }
    }

    /// Whether the message has a formatted (HTML) body.
    pub fn is_html(&self) -> bool {
        self.content.as_ref().map_or(self.html, |c| c.rich.is_some())
    }

    /// Whether the message carries a meeting invitation (or a reply to one).
    pub fn has_invite(&self) -> bool {
        self.content.as_ref().map_or(self.has_invite, |c| c.invite.is_some())
    }

    /// The first line of the body, for the message list preview.
    pub fn preview(&self, max_chars: usize) -> String {
        let line = match &self.content {
            Some(c) => c.body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(""),
            None => self.snippet.as_str(),
        };
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
    fn credential_entries_are_where_the_app_keeps_them() {
        let mut a = Account {
            id: AccountId(0),
            name: String::new(),
            address: Address::parse("ana@gmail.com").unwrap(),
            kind: AccountKind::Local,
            caldav: String::new(),
            signature: String::new(),
        };
        assert_eq!(a.credential_entry(), None);
        a.kind = AccountKind::Imap {
            imap_host: "imap.gmail.com".into(),
            imap_port: 993,
            smtp_host: "smtp.gmail.com".into(),
            smtp_port: 465,
            user: "login".into(),
            imap_security: Default::default(),
            smtp_security: Default::default(),
            oauth: String::new(),
        };
        assert_eq!(a.credential_entry(), Some((KEYRING_SERVICE, "login".to_string())));
        if let AccountKind::Imap { oauth, .. } = &mut a.kind {
            *oauth = "google".into();
        }
        assert_eq!(a.credential_entry(), Some((OAUTH_KEYRING_SERVICE, "google:login".to_string())));
    }

    #[test]
    fn oauth_providers_by_server_name() {
        assert_eq!(OAuthProvider::for_host("imap.gmail.com").map(|p| p.id), Some("google"));
        assert_eq!(OAuthProvider::for_address("Ana@GMail.com").map(|p| p.id), Some("google"));
        assert_eq!(OAuthProvider::for_address("ana@hotmail.com").map(|p| p.id), Some("microsoft"));
        for other in ["ana@example.org", "ana", "ana@", "@gmail.com.evil.org", "gmail.com"] {
            assert_eq!(OAuthProvider::for_address(other).map(|p| p.id), None, "{other}");
        }
        for p in OAUTH_PROVIDERS {
            assert_eq!(OAuthProvider::for_host(p.imap_host).map(|x| x.id), Some(p.id));
            assert_eq!(OAuthProvider::for_host(p.smtp_host).map(|x| x.id), Some(p.id));
        }
        assert_eq!(OAuthProvider::for_host("SMTP.Office365.com.").map(|p| p.id), Some("microsoft"));
        assert_eq!(OAuthProvider::for_host("outlook.com").map(|p| p.id), Some("microsoft"));
        // Only whole labels: a look-alike domain is not the provider.
        for other in ["imap.notgmail.com", "gmail.com.evil.org", "mail.example.org", ""] {
            assert_eq!(OAuthProvider::for_host(other), None, "{other}");
        }
        assert_eq!(OAuthProvider::get("google").map(|p| p.name), Some("Google"));
        assert!(OAuthProvider::get("").is_none());
    }

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
        m.content_mut().unwrap().body = "\n\n  Olá, equipa! Segue a agenda da reunião.\nSecond".into();
        assert_eq!(m.preview(10), "Olá, equip…");
        let away = m.headers_only();
        assert_eq!(away.content(), None);
        assert_eq!(away.preview(10), "Olá, equip…", "the preview stays with the headers");
        assert!(!away.is_html() && !away.has_invite());
        m.content_mut().unwrap().invite = Some("BEGIN:VCALENDAR".into());
        m.content_mut().unwrap().rich = Some(crate::rich::Rich::default());
        let away = m.headers_only();
        assert!(away.is_html() && away.has_invite(), "kept with the headers too");
        m.content_mut().unwrap().body.clear();
        assert_eq!(m.preview(10), "");
    }

    /// A message is written with its content in line, as before content could stay on disk; one
    /// written without it reads back without it, never with an empty body.
    #[test]
    fn content_is_written_in_line_and_may_be_left_out() {
        let mut m = Message::new(MessageId(1), FolderId(0));
        m.set_subject("Hola");
        let c = m.content_mut().unwrap();
        c.body = "Bom dia".into();
        c.invite = Some("BEGIN:VCALENDAR".into());
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["body"], "Bom dia");
        assert_eq!(json["invite"], "BEGIN:VCALENDAR");
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back, m);

        let headers = serde_json::to_value(m.headers_only()).unwrap();
        assert!(headers.get("body").is_none() && headers.get("invite").is_none(), "{headers}");
        let back: Message = serde_json::from_value(headers).unwrap();
        assert_eq!(back.content(), None);
        assert_eq!(back.subject, "Hola");
        assert_eq!(back.preview(20), "Bom dia");
    }
}
