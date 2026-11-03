//! Outlook `.msg` files (MS-OXMSG): one message saved from Outlook, as a compound file
//! ([`crate::cfb`]) of property streams.
//!
//! [`read`] turns a mail message (`IPM.Note`, a meeting request, a report) into the model: the
//! sender and recipients (the SMTP address where Exchange gives one, else what the original
//! headers say), subject, date, read state, plain and HTML body, and the attachment list.
//! Contacts, appointments and tasks saved as `.msg` are refused, so they are not taken for mail.
//! A body kept only as compressed RTF is not read yet: the plain text Outlook also saves is used.

use std::sync::Arc;

use ibercorreo_core::mail::Attachment;
use ibercorreo_core::{Address, Content, DateTime, Error, FolderId, Message, MessageId, Result, clamp_text};

use crate::cfb::{Cfb, Kind, u32_at, u64_at};

/// Largest `.msg` read.
pub const MAX_MSG_BYTES: usize = 64 * 1024 * 1024;
/// Most recipients and attachments read.
pub(crate) const MAX_RECIPIENTS: usize = 2_000;
pub(crate) const MAX_ATTACHMENTS: usize = 1_000;
/// Embedded messages inside embedded messages, at most.
const MAX_DEPTH: usize = 8;

const PT_SHORT: u16 = 0x0002;
const PT_LONG: u16 = 0x0003;
const PT_OBJECT: u16 = 0x000d;
const PT_STRING8: u16 = 0x001e;
const PT_UNICODE: u16 = 0x001f;
const PT_TIME: u16 = 0x0040;
const PT_BINARY: u16 = 0x0102;

mod tag {
    pub const MESSAGE_CLASS: u16 = 0x001a;
    pub const SUBJECT: u16 = 0x0037;
    pub const CLIENT_SUBMIT_TIME: u16 = 0x0039;
    pub const SENT_REPRESENTING_NAME: u16 = 0x0042;
    pub const SENT_REPRESENTING_EMAIL: u16 = 0x0065;
    pub const TRANSPORT_HEADERS: u16 = 0x007d;
    pub const SENDER_NAME: u16 = 0x0c1a;
    pub const SENDER_EMAIL: u16 = 0x0c1f;
    pub const RECIPIENT_TYPE: u16 = 0x0c15;
    pub const DELIVERY_TIME: u16 = 0x0e06;
    pub const MESSAGE_FLAGS: u16 = 0x0e07;
    pub const BODY: u16 = 0x1000;
    pub const HTML: u16 = 0x1013;
    pub const DISPLAY_NAME: u16 = 0x3001;
    pub const EMAIL: u16 = 0x3003;
    pub const CREATION_TIME: u16 = 0x3007;
    pub const ATTACH_DATA: u16 = 0x3701;
    pub const ATTACH_FILENAME: u16 = 0x3704;
    pub const ATTACH_METHOD: u16 = 0x3705;
    pub const ATTACH_LONG_FILENAME: u16 = 0x3707;
    pub const ATTACH_MIME: u16 = 0x370e;
    pub const SMTP_ADDRESS: u16 = 0x39fe;
    pub const INTERNET_CODEPAGE: u16 = 0x3fde;
    pub const MESSAGE_CODEPAGE: u16 = 0x3ffd;
    pub const SENDER_SMTP: u16 = 0x5d01;
    pub const SENT_REPRESENTING_SMTP: u16 = 0x5d02;
}

/// `MSGFLAG_READ`.
const READ: u32 = 0x1;
/// `afEmbeddedMessage`: the attachment is a message.
const EMBEDDED: u32 = 5;

/// One object's properties: the fixed-size ones from its property stream, the others found by
/// their stream names when asked for.
struct Props<'c, 'a> {
    cfb: &'c Cfb<'a>,
    storage: usize,
    fixed: Vec<(u16, u16, u64)>,
    codepage: u32,
}

impl<'c, 'a> Props<'c, 'a> {
    /// The properties of `storage`, whose property stream has a header of `header` bytes.
    fn new(cfb: &'c Cfb<'a>, storage: usize, header: usize, codepage: u32) -> Result<Self> {
        let mut fixed = Vec::new();
        if let Some(stream) = cfb.find(storage, "__properties_version1.0") {
            let bytes = cfb.read(stream)?;
            for e in bytes.get(header..).unwrap_or(&[]).as_chunks::<16>().0 {
                let (Some(tag), Some(value)) = (u32_at(e, 0), u64_at(e, 8)) else { continue };
                fixed.push(((tag >> 16) as u16, (tag & 0xffff) as u16, value));
            }
        }
        let mut props = Props { cfb, storage, fixed, codepage };
        props.codepage = props.int(tag::MESSAGE_CODEPAGE).or_else(|| props.int(tag::INTERNET_CODEPAGE)).unwrap_or(codepage);
        Ok(props)
    }

    fn value(&self, id: u16, kinds: &[u16]) -> Option<u64> {
        self.fixed.iter().find(|(i, k, _)| *i == id && kinds.contains(k)).map(|&(_, _, v)| v)
    }

    fn int(&self, id: u16) -> Option<u32> {
        self.value(id, &[PT_LONG, PT_SHORT]).map(|v| v as u32)
    }

    fn time(&self, id: u16) -> Option<DateTime> {
        self.value(id, &[PT_TIME]).and_then(from_filetime)
    }

    fn stream(&self, id: u16, kind: u16) -> Option<Vec<u8>> {
        let name = format!("__substg1.0_{id:04X}{kind:04X}");
        let at = self.cfb.find(self.storage, &name)?;
        self.cfb.read(at).ok()
    }

    /// A text property, whichever of the two string types it was saved as.
    fn text(&self, id: u16) -> Option<String> {
        let text = if let Some(b) = self.stream(id, PT_UNICODE) {
            let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            String::from_utf16_lossy(&units)
        } else {
            decode(&self.stream(id, PT_STRING8)?, self.codepage)
        };
        let text = text.trim_end_matches('\0');
        Some(clamp_text(text, ibercorreo_mime::MAX_BODY_BYTES).to_string())
    }

    fn binary(&self, id: u16) -> Option<Vec<u8>> {
        self.stream(id, PT_BINARY)
    }

    /// The size of a binary property's stream, without reading it.
    fn binary_len(&self, id: u16) -> Option<usize> {
        let at = self.cfb.find(self.storage, &format!("__substg1.0_{id:04X}{PT_BINARY:04X}"))?;
        usize::try_from(self.cfb.entry(at)?.size).ok()
    }
}

/// Text in a Windows code page, through the MIME reader's character sets (UTF-8 and Windows-1252
/// for a code page it does not know).
fn decode(bytes: &[u8], codepage: u32) -> String {
    let name = match codepage {
        65001 | 0 => return String::from_utf8_lossy(bytes).into_owned(),
        874 => "windows-874".to_string(),
        932 => "shift_jis".to_string(),
        936 => "gbk".to_string(),
        949 => "ks_c_5601-1987".to_string(),
        950 => "big5".to_string(),
        20866 => "koi8-r".to_string(),
        21866 => "koi8-u".to_string(),
        n @ 28591..=28605 => format!("iso-8859-{}", n - 28590),
        n @ (850 | 866 | 1250..=1258) => format!("cp{n}"),
        _ => "cp1252".to_string(),
    };
    match mail_parser::decoders::charsets::map::charset_decoder(name.as_bytes()) {
        Some(f) => f(bytes),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// A Windows FILETIME (100 ns since 1601) as a time; `None` for 0 and out-of-range values.
fn from_filetime(t: u64) -> Option<DateTime> {
    const UNIX: i64 = 11_644_473_600;
    if t == 0 {
        return None;
    }
    let secs = i64::try_from(t / 10_000_000).ok()?.checked_sub(UNIX)?;
    DateTime::from_unix(secs).ok()
}

/// An address: the SMTP one when there is one, else the e-mail address when it looks like one
/// (Exchange's internal `/O=…` addresses do not, and are left out).
fn address(name: Option<String>, smtp: Option<String>, email: Option<String>) -> Option<Address> {
    let email = smtp.filter(|s| !s.trim().is_empty()).or(email)?;
    Address::new(name.as_deref().filter(|n| n.trim() != email.trim()), &email).ok()
}

/// What the original headers say, for mail Outlook received.
fn headers(text: &str) -> Option<Message> {
    let raw = format!("{}\r\n\r\n", text.trim_end());
    ibercorreo_mime::parse(raw.as_bytes(), MessageId(0), FolderId(0)).ok()
}

/// Read a `.msg` file as a message in `folder`.
pub fn read(data: &[u8], id: MessageId, folder: FolderId) -> Result<Message> {
    if data.len() > MAX_MSG_BYTES {
        return Err(Error::TooLarge { what: ".msg file", size: data.len(), limit: MAX_MSG_BYTES });
    }
    let cfb = Cfb::open(data)?;
    message(&cfb, 0, 32, id, folder, 0)
}

/// The message in `storage` (the root, or an embedded message's storage).
fn message(cfb: &Cfb<'_>, storage: usize, header: usize, id: MessageId, folder: FolderId, depth: usize) -> Result<Message> {
    let p = Props::new(cfb, storage, header, 1252)?;
    let class = p.text(tag::MESSAGE_CLASS).unwrap_or_default();
    let lower = class.to_ascii_lowercase();
    for (prefix, what) in [
        ("ipm.contact", "a contact"),
        ("ipm.distlist", "a contact group"),
        ("ipm.appointment", "an appointment"),
        ("ipm.task", "a task"),
        ("ipm.stickynote", "a note"),
        ("ipm.activity", "a journal entry"),
    ] {
        if lower.starts_with(prefix) {
            return Err(Error::invalid(".msg file", format!("this is {what} ({class}), not a mail message")));
        }
    }
    let mut m = Message::new(id, folder);
    let original = p.text(tag::TRANSPORT_HEADERS).and_then(|h| headers(&h));
    m.from = address(p.text(tag::SENDER_NAME), p.text(tag::SENDER_SMTP), p.text(tag::SENDER_EMAIL))
        .or_else(|| address(p.text(tag::SENT_REPRESENTING_NAME), p.text(tag::SENT_REPRESENTING_SMTP), p.text(tag::SENT_REPRESENTING_EMAIL)))
        .or_else(|| original.as_ref().and_then(|o| o.from.clone()));
    let mut seen = 0usize;
    for r in cfb.children(storage) {
        let Some(e) = cfb.entry(r) else { continue };
        if e.kind != Kind::Storage || !e.name.to_ascii_lowercase().starts_with("__recip_version1.0_") {
            continue;
        }
        seen += 1;
        if seen > MAX_RECIPIENTS {
            break;
        }
        let rp = Props::new(cfb, r, 8, p.codepage)?;
        let Some(a) = address(rp.text(tag::DISPLAY_NAME), rp.text(tag::SMTP_ADDRESS), rp.text(tag::EMAIL)) else {
            continue;
        };
        match rp.int(tag::RECIPIENT_TYPE).map(|t| t & 0xf) {
            Some(2) => m.cc.push(a),
            Some(3) => m.bcc.push(a),
            _ => m.to.push(a),
        }
    }
    if let Some(o) = &original {
        // Exchange-only recipients have no SMTP address saved; the headers have them.
        if m.to.is_empty() {
            m.to = o.to.clone();
        }
        if m.cc.is_empty() {
            m.cc = o.cc.clone();
        }
    }
    m.set_subject(&p.text(tag::SUBJECT).unwrap_or_default());
    m.date = p
        .time(tag::DELIVERY_TIME)
        .or_else(|| p.time(tag::CLIENT_SUBMIT_TIME))
        .or_else(|| original.as_ref().and_then(|o| o.date))
        .or_else(|| p.time(tag::CREATION_TIME));
    m.seen = p.int(tag::MESSAGE_FLAGS).is_some_and(|f| f & READ != 0);

    let html = p.binary(tag::HTML).map(|b| decode(&b, p.codepage)).or_else(|| p.text(tag::HTML)).filter(|h| !h.trim().is_empty());
    let rich = html.as_deref().map(ibercorreo_mime::html::to_rich);
    let body = match (p.text(tag::BODY).filter(|b| !b.trim().is_empty()), &rich) {
        (Some(b), _) => b,
        (None, Some(r)) => r.to_text(),
        (None, None) => String::new(),
    };
    let body = clamp_text(&body.replace("\r\n", "\n"), ibercorreo_mime::MAX_BODY_BYTES).to_string();
    m.attachments = attachments(cfb, storage, p.codepage, depth)?;
    m.set_content(Arc::new(Content { body, rich, invite: None }));
    Ok(m)
}

fn attachments(cfb: &Cfb<'_>, storage: usize, codepage: u32, depth: usize) -> Result<Vec<Attachment>> {
    let mut out = Vec::new();
    for a in cfb.children(storage) {
        let Some(e) = cfb.entry(a) else { continue };
        if e.kind != Kind::Storage || !e.name.to_ascii_lowercase().starts_with("__attach_version1.0_") {
            continue;
        }
        if out.len() >= MAX_ATTACHMENTS {
            break;
        }
        let ap = Props::new(cfb, a, 8, codepage)?;
        let name = ap
            .text(tag::ATTACH_LONG_FILENAME)
            .or_else(|| ap.text(tag::ATTACH_FILENAME))
            .or_else(|| ap.text(tag::DISPLAY_NAME))
            .filter(|n| !n.trim().is_empty());
        let embedded = ap.int(tag::ATTACH_METHOD) == Some(EMBEDDED);
        let (name, mime_type, size) = if embedded {
            // A message attached to the message: its subject names it.
            let inner = cfb.find(a, &format!("__substg1.0_{:04X}{PT_OBJECT:04X}", tag::ATTACH_DATA));
            let subject = match inner {
                Some(i) if depth < MAX_DEPTH => message(cfb, i, 24, MessageId(0), FolderId(0), depth + 1).ok().map(|m| m.subject),
                _ => None,
            };
            let name = name.or(subject.filter(|s| !s.trim().is_empty())).unwrap_or_else(|| "message".into());
            (format!("{}.eml", name.trim_end_matches(".msg")), "message/rfc822".to_string(), 0)
        } else {
            let mime = ap.text(tag::ATTACH_MIME).filter(|t| t.contains('/') && !t.contains(char::is_whitespace));
            (
                name.unwrap_or_else(|| "attachment".into()),
                mime.unwrap_or_else(|| "application/octet-stream".into()),
                ap.binary_len(tag::ATTACH_DATA).unwrap_or(0),
            )
        };
        out.push(Attachment { name: clamp_text(&name, 1024).to_string(), mime_type: clamp_text(&mime_type, 255).to_string(), size });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
