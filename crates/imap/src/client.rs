//! A blocking IMAP client over any byte stream. The stream is TLS in the app (`ibercorreo-net`)
//! and a scripted buffer in tests; this crate never opens a socket itself.

use std::io::{Read, Write};

use crate::command::{Command, Part, check_flags, check_set};
use crate::response::{self, Code, Fetch, Mailbox, Response, Status, Untagged};
use crate::wire::Reader;
use crate::{Error, Result};

/// The most untagged responses one command may produce before it is abandoned.
pub const MAX_UNTAGGED: usize = 1_000_000;
/// The most bytes one command's responses may add up to (fetch bodies in batches below this).
pub const MAX_COMMAND_BYTES: usize = 256 * 1024 * 1024;

/// What `SELECT` / `EXAMINE` report about a mailbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selected {
    pub exists: u32,
    pub uid_validity: Option<u32>,
    pub uid_next: Option<u32>,
    pub flags: Vec<String>,
    pub read_only: bool,
}

/// Which data items to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Items {
    /// UID, flags, size and arrival date: enough to list a folder.
    Summary,
    /// The summary plus the whole message, without marking it read (`BODY.PEEK[]`).
    Full,
}

impl Items {
    fn spec(self) -> &'static str {
        match self {
            Items::Summary => "(UID FLAGS RFC822.SIZE INTERNALDATE)",
            Items::Full => "(UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[])",
        }
    }
}

/// A finished command: the untagged responses it produced and the tagged `OK`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Done {
    pub untagged: Vec<Untagged>,
    pub code: Option<Code>,
    pub text: String,
}

/// An IMAP session. Mailbox arguments are names as the server spells them ([`Mailbox::raw`], or
/// [`crate::utf7::encode`] of a name a person typed).
#[derive(Debug)]
pub struct Client<S> {
    stream: S,
    reader: Reader,
    next_tag: u32,
    capabilities: Vec<String>,
    secure: bool,
    authenticated: bool,
    closed: bool,
    /// The tag of the `IDLE` command in progress.
    idling: Option<String>,
}

/// What the server reports while idling that means the mailbox changed.
pub fn is_news(u: &Untagged) -> bool {
    matches!(u, Untagged::Exists(_) | Untagged::Expunge(_) | Untagged::Fetch(_) | Untagged::Recent(_))
}

impl<S: Read + Write> Client<S> {
    /// Read the server greeting. `secure` says whether `stream` is already TLS: without it,
    /// [`Client::login`] refuses to send a password.
    pub fn new(stream: S, secure: bool) -> Result<Self> {
        let mut client = Client {
            stream,
            reader: Reader::new(),
            next_tag: 1,
            capabilities: Vec::new(),
            secure,
            authenticated: false,
            closed: false,
            idling: None,
        };
        match client.read()? {
            Response::Untagged(Untagged::Status { status, code, text }) => {
                match status {
                    Status::Ok => {}
                    Status::PreAuth => client.authenticated = true,
                    Status::Bye => return Err(Error::Bye(text)),
                    Status::No | Status::Bad => return Err(Error::Protocol(format!("greeting: {text}"))),
                }
                if let Some(Code::Capability(caps)) = code {
                    client.capabilities = caps;
                }
            }
            other => return Err(Error::Protocol(format!("unexpected greeting: {other:?}"))),
        }
        if client.capabilities.is_empty() {
            client.capability()?;
        }
        Ok(client)
    }

    /// The stream back, e.g. to close it.
    pub fn into_stream(self) -> S {
        self.stream
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c.eq_ignore_ascii_case(capability))
    }

    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    pub fn is_secure(&self) -> bool {
        self.secure
    }

    fn read(&mut self) -> Result<Response> {
        let bytes = match self.reader.response(&mut self.stream) {
            Ok(b) => b,
            Err(e) => {
                self.closed = true;
                return Err(e);
            }
        };
        response::parse(&bytes)
    }

    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let r = self.stream.write_all(bytes).and_then(|()| self.stream.flush());
        r.map_err(|e| {
            self.closed = true;
            Error::Io(e.to_string())
        })
    }

    /// Send one command and collect its responses until the tagged reply.
    pub fn run(&mut self, command: &Command) -> Result<Done> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.idling.is_some() {
            return Err(Error::Invalid("the session is idling: end it with idle_done first".into()));
        }
        let tag = format!("a{}", self.next_tag);
        self.next_tag = self.next_tag.wrapping_add(1).max(1);
        let mut untagged = Vec::new();
        let mut out = format!("{tag} ").into_bytes();
        for part in &command.parts {
            match part {
                Part::Text(t) => out.extend_from_slice(t),
                Part::Literal(bytes) => {
                    let plus = self.has("LITERAL+") || (self.has("LITERAL-") && bytes.len() <= 4096);
                    out.extend_from_slice(format!("{{{}{}}}\r\n", bytes.len(), if plus { "+" } else { "" }).as_bytes());
                    if !plus {
                        self.send(&out)?;
                        out.clear();
                        self.continuation(&tag, &mut untagged)?;
                    }
                    out.extend_from_slice(bytes);
                }
            }
        }
        out.extend_from_slice(b"\r\n");
        self.send(&out)?;
        self.finish(&tag, untagged)
    }

    /// Wait for `+` after a synchronising literal.
    fn continuation(&mut self, tag: &str, untagged: &mut Vec<Untagged>) -> Result<()> {
        loop {
            match self.read()? {
                Response::Continue(_) => return Ok(()),
                Response::Untagged(u) => self.keep(u, untagged)?,
                Response::Tagged { tag: t, status, text, .. } if t == tag => return Err(Self::refusal(status, text)),
                Response::Tagged { .. } => {}
            }
        }
    }

    fn keep(&mut self, u: Untagged, untagged: &mut Vec<Untagged>) -> Result<()> {
        match &u {
            Untagged::Capability(caps) => self.capabilities.clone_from(caps),
            Untagged::Status { status: Status::Bye, .. } => self.closed = true,
            _ => {}
        }
        if untagged.len() >= MAX_UNTAGGED {
            self.closed = true;
            return Err(Error::TooLarge("command responses"));
        }
        untagged.push(u);
        Ok(())
    }

    fn refusal(status: Status, text: String) -> Error {
        match status {
            Status::No => Error::No(text),
            Status::Bye => Error::Bye(text),
            _ => Error::Bad(text),
        }
    }

    fn finish(&mut self, tag: &str, mut untagged: Vec<Untagged>) -> Result<Done> {
        let mut bytes = 0usize;
        loop {
            let raw = match self.reader.response(&mut self.stream) {
                Ok(b) => b,
                Err(e) => {
                    self.closed = true;
                    return Err(e);
                }
            };
            bytes = bytes.saturating_add(raw.len());
            if bytes > MAX_COMMAND_BYTES {
                self.closed = true;
                return Err(Error::TooLarge("command responses"));
            }
            match response::parse(&raw)? {
                Response::Untagged(u) => self.keep(u, &mut untagged)?,
                Response::Tagged { tag: t, status, code, text } if t == tag => {
                    if let Some(Code::Capability(caps)) = &code {
                        self.capabilities.clone_from(caps);
                    }
                    return match status {
                        Status::Ok => Ok(Done { untagged, code, text }),
                        other => Err(Self::refusal(other, text)),
                    };
                }
                // A stray `+` or another command's tag: not ours, ignore it.
                Response::Tagged { .. } | Response::Continue(_) => {}
            }
        }
    }

    /// `CAPABILITY`.
    pub fn capability(&mut self) -> Result<&[String]> {
        self.run(&Command::new("CAPABILITY"))?;
        Ok(&self.capabilities)
    }

    /// `NOOP`: keeps the connection alive and collects news (`EXISTS`, `EXPUNGE`, flag changes).
    pub fn noop(&mut self) -> Result<Vec<Untagged>> {
        self.run(&Command::new("NOOP")).map(|d| d.untagged)
    }

    /// `LOGIN`. Refused on a connection that is not encrypted, and when the server disables it.
    pub fn login(&mut self, user: &str, password: &str) -> Result<()> {
        if !self.secure {
            return Err(Error::Insecure);
        }
        if self.has("LOGINDISABLED") {
            return Err(Error::Unsupported("the server disables LOGIN"));
        }
        let done = self.run(&Command::new("LOGIN").astring(user.as_bytes()).astring(password.as_bytes()))?;
        self.authenticated = true;
        // Capabilities change after login; take them from the reply or ask again.
        if !matches!(done.code, Some(Code::Capability(_))) {
            self.capability()?;
        }
        Ok(())
    }

    /// Send `STARTTLS` and hand the stream back for a TLS handshake (then [`Client::resume`]).
    pub fn starttls(mut self) -> Result<S> {
        if !self.has("STARTTLS") {
            return Err(Error::Unsupported("the server does not offer STARTTLS"));
        }
        self.run(&Command::new("STARTTLS"))?;
        // Anything already buffered was sent before encryption and could be an attacker's
        // injected response (RFC 9051 §6.2.1): refuse rather than carry it over.
        if self.reader.has_pending() {
            return Err(Error::Protocol("data after STARTTLS before the TLS handshake".into()));
        }
        Ok(self.stream)
    }

    /// Continue on an encrypted stream after [`Client::starttls`]. Capabilities are asked again:
    /// the ones from before TLS may have been tampered with.
    pub fn resume(stream: S) -> Result<Self> {
        let mut client = Client {
            stream,
            reader: Reader::new(),
            next_tag: 1,
            capabilities: Vec::new(),
            secure: true,
            authenticated: false,
            closed: false,
            idling: None,
        };
        client.capability()?;
        Ok(client)
    }

    /// Every mailbox (`LIST "" "*"`).
    pub fn list(&mut self) -> Result<Vec<Mailbox>> {
        let done = self.run(&Command::new("LIST").astring(b"").astring(b"*"))?;
        Ok(done
            .untagged
            .into_iter()
            .filter_map(|u| match u {
                Untagged::List(m) => Some(m),
                _ => None,
            })
            .collect())
    }

    /// Create a mailbox (`CREATE`), e.g. a folder someone made in the folder pane.
    pub fn create(&mut self, mailbox: &str) -> Result<()> {
        self.run(&Command::new("CREATE").mailbox(mailbox)).map(drop)
    }

    /// `SELECT` (read-write) or, with `read_only`, `EXAMINE`.
    pub fn select(&mut self, mailbox: &str, read_only: bool) -> Result<Selected> {
        let done = self.run(&Command::new(if read_only { "EXAMINE" } else { "SELECT" }).mailbox(mailbox))?;
        let mut s = Selected { read_only, ..Selected::default() };
        for u in done.untagged {
            match u {
                Untagged::Exists(n) => s.exists = n,
                Untagged::Flags(f) => s.flags = f,
                Untagged::Status { code: Some(Code::UidValidity(v)), .. } => s.uid_validity = Some(v),
                Untagged::Status { code: Some(Code::UidNext(v)), .. } => s.uid_next = Some(v),
                _ => {}
            }
        }
        if matches!(&done.code, Some(Code::Other(c)) if c == "READ-ONLY") {
            s.read_only = true;
        }
        Ok(s)
    }

    /// `UID SEARCH UID <set>`: the UIDs in `set` that exist (`1:*` for all).
    pub fn uid_search(&mut self, set: &str) -> Result<Vec<u32>> {
        let done = self.run(&Command::new("UID SEARCH UID").raw(check_set(set)?))?;
        let mut uids: Vec<u32> = done
            .untagged
            .into_iter()
            .flat_map(|u| match u {
                Untagged::Search(ids) => ids,
                _ => Vec::new(),
            })
            .collect();
        uids.sort_unstable();
        uids.dedup();
        Ok(uids)
    }

    /// `UID FETCH <set> <items>`. Responses without a UID (unsolicited flag changes) are dropped.
    pub fn uid_fetch(&mut self, set: &str, items: Items) -> Result<Vec<Fetch>> {
        let done = self.run(&Command::new("UID FETCH").raw(check_set(set)?).raw(items.spec()))?;
        Ok(done
            .untagged
            .into_iter()
            .filter_map(|u| match u {
                Untagged::Fetch(f) if f.uid.is_some() => Some(f),
                _ => None,
            })
            .collect())
    }

    /// Add (`add`) or remove flags on messages, without the server echoing them back.
    pub fn uid_store(&mut self, set: &str, add: bool, flags: &[&str]) -> Result<()> {
        let what = if add { "+FLAGS.SILENT" } else { "-FLAGS.SILENT" };
        self.run(&Command::new("UID STORE").raw(check_set(set)?).raw(what).raw(&check_flags(flags)?)).map(drop)
    }

    /// Remove messages flagged `\Deleted`: only `set` when the server has UIDPLUS, else all of them.
    pub fn uid_expunge(&mut self, set: &str) -> Result<()> {
        let set = check_set(set)?;
        let command = if self.has("UIDPLUS") { Command::new("UID EXPUNGE").raw(set) } else { Command::new("EXPUNGE") };
        self.run(&command).map(drop)
    }

    /// Move messages to another mailbox: `MOVE` (RFC 6851) when offered, else copy, flag and expunge.
    pub fn uid_move(&mut self, set: &str, mailbox: &str) -> Result<()> {
        let set = check_set(set)?;
        if self.has("MOVE") {
            return self.run(&Command::new("UID MOVE").raw(set).mailbox(mailbox)).map(drop);
        }
        self.run(&Command::new("UID COPY").raw(set).mailbox(mailbox))?;
        self.uid_store(set, true, &["\\Deleted"])?;
        self.uid_expunge(set)
    }

    /// Store a message in a mailbox (`APPEND`), e.g. a sent message in Sent.
    pub fn append(&mut self, mailbox: &str, flags: &[&str], message: &[u8]) -> Result<()> {
        if message.len() > crate::wire::MAX_LITERAL {
            return Err(Error::TooLarge("message"));
        }
        let mut command = Command::new("APPEND").mailbox(mailbox);
        if !flags.is_empty() {
            command = command.raw(&check_flags(flags)?);
        }
        self.run(&command.literal(message)).map(drop)
    }

    /// Start `IDLE` (RFC 2177) on the selected mailbox: the server then reports changes as they
    /// happen. Wait with [`Client::idle_wait`] and end it with [`Client::idle_done`] (servers
    /// drop an idle session after 30 minutes, so end and start it again well before). Returns
    /// what the server sent before it began idling.
    pub fn idle_start(&mut self) -> Result<Vec<Untagged>> {
        if !self.has("IDLE") {
            return Err(Error::Unsupported("the server does not offer IDLE"));
        }
        if self.closed {
            return Err(Error::Closed);
        }
        if self.idling.is_some() {
            return Ok(Vec::new());
        }
        let tag = format!("a{}", self.next_tag);
        self.next_tag = self.next_tag.wrapping_add(1).max(1);
        self.send(format!("{tag} IDLE\r\n").as_bytes())?;
        let mut news = Vec::new();
        loop {
            match self.read()? {
                Response::Continue(_) => break,
                Response::Untagged(u) => self.keep(u, &mut news)?,
                Response::Tagged { tag: t, status, text, .. } if t == tag => return Err(Self::refusal(status, text)),
                Response::Tagged { .. } => {}
            }
        }
        self.idling = Some(tag);
        Ok(news)
    }

    /// Wait for the next thing the server sends while idling, up to the connection's read
    /// timeout: what arrived (empty when nothing did). Keep-alive `OK` lines count as nothing.
    pub fn idle_wait(&mut self) -> Result<Vec<Untagged>> {
        let Some(tag) = self.idling.clone() else { return Err(Error::Invalid("not idling".into())) };
        let bytes = match self.reader.response(&mut self.stream) {
            Ok(b) => b,
            Err(Error::Timeout) => return Ok(Vec::new()),
            Err(e) => {
                self.closed = true;
                return Err(e);
            }
        };
        let mut news = Vec::new();
        match response::parse(&bytes)? {
            Response::Untagged(Untagged::Status { status: Status::Ok, .. }) => {}
            Response::Untagged(u) => self.keep(u, &mut news)?,
            // The server ended the idle itself (a refusal, or shutting down).
            Response::Tagged { tag: t, status, text, .. } if t == tag => {
                self.idling = None;
                if status != Status::Ok {
                    return Err(Self::refusal(status, text));
                }
            }
            Response::Tagged { .. } | Response::Continue(_) => {}
        }
        if self.closed {
            return Err(Error::Closed);
        }
        Ok(news)
    }

    /// End `IDLE` (`DONE`) and collect what arrived until the server confirms.
    pub fn idle_done(&mut self) -> Result<Vec<Untagged>> {
        let Some(tag) = self.idling.take() else { return Ok(Vec::new()) };
        self.send(b"DONE\r\n")?;
        self.finish(&tag, Vec::new()).map(|d| d.untagged)
    }

    /// Whether `IDLE` is in progress.
    pub fn is_idling(&self) -> bool {
        self.idling.is_some()
    }

    /// `LOGOUT`. The connection is closed afterwards whatever the server says.
    pub fn logout(mut self) -> Result<()> {
        if self.idling.is_some() {
            let _ = self.idle_done();
        }
        let r = self.run(&Command::new("LOGOUT"));
        match r {
            Ok(_) | Err(Error::Closed) => Ok(()),
            Err(e) => Err(e),
        }
    }
}
