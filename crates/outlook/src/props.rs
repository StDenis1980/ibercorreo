//! Outlook's message properties (MS-OXPROPS, MS-OXCMSG), whatever file they came from.
//!
//! A `.msg` file ([`crate::msg`]) and a `.pst` archive ([`crate::pst`]) store the same properties
//! in different containers. Each reader collects a message's properties into a [`Raw`] message,
//! and [`build`] turns that into the model: sender and recipients (the SMTP address where Exchange
//! gives one, else what the original headers say), subject, date, read state, plain and HTML body
//! (also from compressed RTF, [`crate::rtf`]), and the attachment list.

use std::sync::Arc;

use ibercorreo_core::mail::Attachment;
use ibercorreo_core::{Address, Content, DateTime, Error, FolderId, Message, MessageId, Result, clamp_text};

use crate::rtf;

/// Most recipients and attachments read.
pub(crate) const MAX_RECIPIENTS: usize = 2_000;
pub(crate) const MAX_ATTACHMENTS: usize = 1_000;
/// Embedded messages inside embedded messages, at most.
pub(crate) const MAX_DEPTH: usize = 8;

pub(crate) const PT_SHORT: u16 = 0x0002;
pub(crate) const PT_LONG: u16 = 0x0003;
pub(crate) const PT_BOOLEAN: u16 = 0x000b;
pub(crate) const PT_OBJECT: u16 = 0x000d;
pub(crate) const PT_STRING8: u16 = 0x001e;
pub(crate) const PT_UNICODE: u16 = 0x001f;
pub(crate) const PT_TIME: u16 = 0x0040;
pub(crate) const PT_BINARY: u16 = 0x0102;

pub(crate) mod tag {
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
    pub const RTF_COMPRESSED: u16 = 0x1009;
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
pub(crate) const EMBEDDED: u32 = 5;

/// A property's value as a reader found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Val {
    /// A fixed-size value (up to 8 bytes), little-endian in a `u64`.
    Fixed(u64),
    /// A variable-size value's bytes.
    Bytes(Vec<u8>),
    /// A value not read, only measured (an attachment's contents).
    Size(usize),
}

/// One object's properties: id, type, value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Bag(pub(crate) Vec<(u16, u16, Val)>);

impl Bag {
    pub(crate) fn fixed(&self, id: u16, kinds: &[u16]) -> Option<u64> {
        self.0.iter().find_map(|(i, k, v)| match v {
            Val::Fixed(n) if *i == id && kinds.contains(k) => Some(*n),
            _ => None,
        })
    }

    pub(crate) fn bytes(&self, id: u16, kind: u16) -> Option<&[u8]> {
        self.0.iter().find_map(|(i, k, v)| match v {
            Val::Bytes(b) if *i == id && *k == kind => Some(b.as_slice()),
            _ => None,
        })
    }

    pub(crate) fn size(&self, id: u16, kind: u16) -> Option<usize> {
        self.0.iter().find_map(|(i, k, v)| match v {
            Val::Bytes(b) if *i == id && *k == kind => Some(b.len()),
            Val::Size(n) if *i == id && *k == kind => Some(*n),
            _ => None,
        })
    }

    pub(crate) fn int(&self, id: u16) -> Option<u32> {
        self.fixed(id, &[PT_LONG, PT_SHORT]).map(|v| v as u32)
    }
}

/// A message's properties, recipients and attachments, as a reader collected them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Raw {
    pub(crate) props: Bag,
    pub(crate) recipients: Vec<Bag>,
    pub(crate) attachments: Vec<RawAttachment>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RawAttachment {
    pub(crate) props: Bag,
    /// The attached message, for an attachment that is one (read up to [`MAX_DEPTH`] deep).
    pub(crate) embedded: Option<Box<Raw>>,
}

/// A bag read with its object's code pages.
struct View<'b> {
    bag: &'b Bag,
    /// The code page of 8-bit text properties.
    codepage: u32,
    /// The code page of the HTML body, which can differ (`PidTagInternetCodepage`).
    internet: u32,
}

impl<'b> View<'b> {
    fn new(bag: &'b Bag, codepage: u32) -> Self {
        let codepage = bag.int(tag::MESSAGE_CODEPAGE).or_else(|| bag.int(tag::INTERNET_CODEPAGE)).unwrap_or(codepage);
        View { bag, codepage, internet: bag.int(tag::INTERNET_CODEPAGE).unwrap_or(codepage) }
    }

    fn int(&self, id: u16) -> Option<u32> {
        self.bag.int(id)
    }

    fn time(&self, id: u16) -> Option<DateTime> {
        self.bag.fixed(id, &[PT_TIME]).and_then(from_filetime)
    }

    /// A text property, whichever of the two string types it was saved as.
    fn text(&self, id: u16) -> Option<String> {
        let text = if let Some(b) = self.bag.bytes(id, PT_UNICODE) { utf16(b) } else { decode(self.bag.bytes(id, PT_STRING8)?, self.codepage) };
        let text = text.trim_end_matches('\0');
        Some(clamp_text(text, ibercorreo_mime::MAX_BODY_BYTES).to_string())
    }

    fn binary(&self, id: u16) -> Option<&[u8]> {
        self.bag.bytes(id, PT_BINARY)
    }
}

/// A text property of `bag`, 8-bit text read as Windows-1252 unless the bag names a code page.
pub(crate) fn text_of(bag: &Bag, id: u16) -> Option<String> {
    View::new(bag, 1252).text(id)
}

/// UTF-16LE text, an odd last byte dropped.
pub(crate) fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    String::from_utf16_lossy(&units)
}

/// Text in a Windows code page, through the MIME reader's character sets (UTF-8 and Windows-1252
/// for a code page it does not know).
pub(crate) fn decode(bytes: &[u8], codepage: u32) -> String {
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
pub(crate) fn from_filetime(t: u64) -> Option<DateTime> {
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

/// The message class of `raw`, and an error when it is not mail.
pub(crate) fn mail_class(raw: &Raw) -> Result<String> {
    let class = View::new(&raw.props, 1252).text(tag::MESSAGE_CLASS).unwrap_or_default();
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
            return Err(Error::invalid("Outlook item", format!("this is {what} ({class}), not a mail message")));
        }
    }
    Ok(class)
}

/// The message `raw` describes, in `folder`. Contacts, appointments, tasks and notes are refused,
/// so they are not taken for mail.
pub(crate) fn build(raw: &Raw, id: MessageId, folder: FolderId) -> Result<Message> {
    mail_class(raw)?;
    let p = View::new(&raw.props, 1252);
    let mut m = Message::new(id, folder);
    let original = p.text(tag::TRANSPORT_HEADERS).and_then(|h| headers(&h));
    m.from = address(p.text(tag::SENDER_NAME), p.text(tag::SENDER_SMTP), p.text(tag::SENDER_EMAIL))
        .or_else(|| address(p.text(tag::SENT_REPRESENTING_NAME), p.text(tag::SENT_REPRESENTING_SMTP), p.text(tag::SENT_REPRESENTING_EMAIL)))
        .or_else(|| original.as_ref().and_then(|o| o.from.clone()));
    for r in raw.recipients.iter().take(MAX_RECIPIENTS) {
        let rp = View::new(r, p.codepage);
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
    m.set_subject(&subject(p.text(tag::SUBJECT).unwrap_or_default()));
    m.date = p
        .time(tag::DELIVERY_TIME)
        .or_else(|| p.time(tag::CLIENT_SUBMIT_TIME))
        .or_else(|| original.as_ref().and_then(|o| o.date))
        .or_else(|| p.time(tag::CREATION_TIME));
    m.seen = p.int(tag::MESSAGE_FLAGS).is_some_and(|f| f & READ != 0);

    let filled = |t: &String| !t.trim().is_empty();
    let mut html = p.binary(tag::HTML).map(|b| decode(b, p.internet)).or_else(|| p.text(tag::HTML)).filter(filled);
    let mut text = p.text(tag::BODY).filter(filled);
    // Outlook often keeps the formatted body only as compressed RTF, HTML mail wrapped in it.
    if html.is_none() {
        match p.binary(tag::RTF_COMPRESSED).and_then(|c| rtf::decompress(c).ok()).and_then(|r| rtf::body(&r)) {
            Some(rtf::Body::Html(h)) => html = Some(h).filter(filled),
            Some(rtf::Body::Text(t)) if text.is_none() => text = Some(t).filter(filled),
            _ => {}
        }
    }
    let rich = html.as_deref().map(ibercorreo_mime::html::to_rich);
    let body = match (text, &rich) {
        (Some(b), _) => b,
        (None, Some(r)) => r.to_text(),
        (None, None) => String::new(),
    };
    let body = clamp_text(&body.replace("\r\n", "\n"), ibercorreo_mime::MAX_BODY_BYTES).to_string();
    m.attachments = raw.attachments.iter().take(MAX_ATTACHMENTS).map(|a| attachment(a, p.codepage)).collect();
    m.set_content(Arc::new(Content { body, rich, invite: None }));
    Ok(m)
}

/// A subject without the prefix marker a `.pst` can keep in front of it (MS-PST 2.5.3.1.1.1:
/// `\u{1}` and a character giving the prefix's length).
fn subject(s: String) -> String {
    match s.strip_prefix('\u{1}') {
        Some(rest) => rest.chars().skip(1).collect(),
        None => s,
    }
}

fn attachment(a: &RawAttachment, codepage: u32) -> Attachment {
    let ap = View::new(&a.props, codepage);
    let name = ap
        .text(tag::ATTACH_LONG_FILENAME)
        .or_else(|| ap.text(tag::ATTACH_FILENAME))
        .or_else(|| ap.text(tag::DISPLAY_NAME))
        .filter(|n| !n.trim().is_empty());
    let (name, mime_type, size) = if ap.int(tag::ATTACH_METHOD) == Some(EMBEDDED) {
        // A message attached to the message: its subject names it.
        let subject = a.embedded.as_deref().and_then(|inner| View::new(&inner.props, codepage).text(tag::SUBJECT)).map(subject);
        let name = name.or(subject.filter(|s| !s.trim().is_empty())).unwrap_or_else(|| "message".into());
        (format!("{}.eml", name.trim_end_matches(".msg")), "message/rfc822".to_string(), 0)
    } else {
        let mime = ap.text(tag::ATTACH_MIME).filter(|t| t.contains('/') && !t.contains(char::is_whitespace));
        (
            name.unwrap_or_else(|| "attachment".into()),
            mime.unwrap_or_else(|| "application/octet-stream".into()),
            a.props.size(tag::ATTACH_DATA, PT_BINARY).unwrap_or(0),
        )
    };
    Attachment { name: clamp_text(&name, 1024).to_string(), mime_type: clamp_text(&mime_type, 255).to_string(), size, sha256: None }
}
