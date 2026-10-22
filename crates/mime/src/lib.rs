//! Messages in and out, L1.
//!
//! [`parse`] reads a raw RFC 5322 message (an `.eml` file, an IMAP `BODY[]`) into a
//! [`Message`] through Stalwart's `mail-parser` (Apache-2.0 OR MIT). HTML-only mail is reduced to
//! plain text; nothing remote is fetched. [`write`] turns a message being sent into RFC 5322 text:
//! UTF-8 headers as RFC 2047 encoded words, the body as base64 when it is not plain ASCII, and no
//! header value can carry a line break (header injection).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use ibercorreo_core::mail::Attachment;
use ibercorreo_core::{Address, Date, DateTime, Error, FolderId, MAX_FIELD_LEN, Message, MessageId, Result, clamp_text};
use mail_parser::{MessageParser, MimeHeaders};

/// Largest message read: 64 MiB, above what common servers accept (Gmail 25 MB, Exchange Online 150 MB is rare).
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// Largest body text kept in the model; longer bodies are cut at a char boundary.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_ADDRESSES: usize = 2_000;
const MAX_ATTACHMENTS: usize = 1_000;

/// Read `raw` as one message placed in `folder` under `id`.
pub fn parse(raw: &[u8], id: MessageId, folder: FolderId) -> Result<Message> {
    if raw.len() > MAX_MESSAGE_BYTES {
        return Err(Error::TooLarge { what: "message", size: raw.len(), limit: MAX_MESSAGE_BYTES });
    }
    let parsed = MessageParser::default().parse(raw).ok_or_else(|| Error::invalid("message", "not an RFC 5322 message"))?;
    let mut m = Message::new(id, folder);
    m.from = parsed.from().and_then(|a| addresses(a).into_iter().next());
    m.to = parsed.to().map(addresses).unwrap_or_default();
    m.cc = parsed.cc().map(addresses).unwrap_or_default();
    m.set_subject(parsed.subject().unwrap_or(""));
    m.date = parsed.date().and_then(convert_date);
    let body = parsed.body_text(0).map(|b| b.into_owned()).unwrap_or_default();
    m.body = clamp_text(&body.replace("\r\n", "\n"), MAX_BODY_BYTES).to_string();
    m.attachments = parsed
        .attachments()
        .take(MAX_ATTACHMENTS)
        .map(|part| Attachment {
            name: clamp_text(part.attachment_name().unwrap_or("attachment"), 1024).to_string(),
            mime_type: part
                .content_type()
                .map(|ct| match ct.subtype() {
                    Some(sub) => format!("{}/{}", ct.ctype(), sub),
                    None => ct.ctype().to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".into()),
            size: part.contents().len(),
        })
        .collect();
    Ok(m)
}

fn addresses(a: &mail_parser::Address<'_>) -> Vec<Address> {
    a.iter()
        .take(MAX_ADDRESSES)
        .filter_map(|addr| {
            let email = addr.address()?;
            Address::new(addr.name(), email).ok()
        })
        .collect()
}

/// The sender's wall-clock time as written in the `Date:` header (the offset is not applied, so the
/// list shows what the sender saw; time zones arrive in M3).
fn convert_date(d: &mail_parser::DateTime) -> Option<DateTime> {
    let date = Date::new(i32::from(d.year), d.month, d.day).ok()?;
    DateTime::new(date, d.hour, d.minute, d.second).ok()
}

/// What [`write`] needs besides the message itself.
pub struct Outgoing<'a> {
    pub from: &'a Address,
    /// The `Date:` header, already formatted per RFC 5322 (`Thu, 22 Oct 2026 14:05:00 +0000`).
    pub date: &'a str,
    /// Unique part of the `Message-ID:` (the domain comes from the sender's address).
    pub message_id: &'a str,
}

/// RFC 5322 text for `m`, ready for SMTP `DATA` or saving as `.eml`. Lines end in CRLF.
pub fn write(m: &Message, out: &Outgoing<'_>) -> Result<String> {
    if m.to.is_empty() && m.cc.is_empty() {
        return Err(Error::invalid("message", "no recipients"));
    }
    let domain = out.from.email.rsplit_once('@').map(|(_, d)| d).unwrap_or("localhost");
    let mut s = String::new();
    header(&mut s, "From", &address_list(std::slice::from_ref(out.from)));
    if !m.to.is_empty() {
        header(&mut s, "To", &address_list(&m.to));
    }
    if !m.cc.is_empty() {
        header(&mut s, "Cc", &address_list(&m.cc));
    }
    header(&mut s, "Subject", &encode_word(&single_line(clamp_text(&m.subject, MAX_FIELD_LEN))));
    header(&mut s, "Date", &single_line(out.date));
    header(&mut s, "Message-ID", &format!("<{}@{}>", single_line(out.message_id).replace(['<', '>', '@', ' '], ""), domain));
    header(&mut s, "MIME-Version", "1.0");
    let body = m.body.replace("\r\n", "\n");
    if body.is_ascii() && body.lines().all(|l| l.len() <= 998) {
        header(&mut s, "Content-Type", "text/plain; charset=utf-8");
        header(&mut s, "Content-Transfer-Encoding", "7bit");
        s.push_str("\r\n");
        for line in body.lines() {
            // SMTP dot-stuffing is the transport's job; a lone "." stays as written here.
            s.push_str(line);
            s.push_str("\r\n");
        }
    } else {
        header(&mut s, "Content-Type", "text/plain; charset=utf-8");
        header(&mut s, "Content-Transfer-Encoding", "base64");
        s.push_str("\r\n");
        let encoded = base64(body.replace('\n', "\r\n").as_bytes());
        for chunk in encoded.as_bytes().chunks(76) {
            s.push_str(std::str::from_utf8(chunk).unwrap_or(""));
            s.push_str("\r\n");
        }
    }
    Ok(s)
}

fn header(s: &mut String, name: &str, value: &str) {
    s.push_str(name);
    s.push_str(": ");
    s.push_str(value);
    s.push_str("\r\n");
}

/// Line breaks and other control characters out: a subject can never start a new header.
fn single_line(v: &str) -> String {
    v.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

fn address_list(list: &[Address]) -> String {
    list.iter()
        .map(|a| match &a.name {
            Some(n) if !n.is_ascii() => format!("{} <{}>", encode_word(&single_line(n)), a.email),
            _ => single_line(&a.to_string()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// RFC 2047 `=?utf-8?B?…?=` words for non-ASCII text, each under 76 characters, split on char
/// boundaries.
fn encode_word(v: &str) -> String {
    if v.is_ascii() {
        return v.to_string();
    }
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in v.chars() {
        // 45 bytes of UTF-8 become 60 base64 characters: 72 with the wrapper.
        if chunk.len() + c.len_utf8() > 45 {
            words.push(format!("=?utf-8?B?{}?=", base64(chunk.as_bytes())));
            chunk.clear();
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        words.push(format!("=?utf-8?B?{}?=", base64(chunk.as_bytes())));
    }
    words.join("\r\n ")
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let sym = |i: u32| char::from(*ALPHABET.get((i & 63) as usize).unwrap_or(&b'A'));
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (b(0) << 16) | (b(1) << 8) | b(2);
        out.push(sym(n >> 18));
        out.push(sym(n >> 12));
        out.push(if chunk.len() > 1 { sym(n >> 6) } else { '=' });
        out.push(if chunk.len() > 2 { sym(n) } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "From: \"Ana Souza\" <ana@example.org>\r\n\
To: Denys <denys@example.com>, bob@example.org\r\n\
Subject: =?utf-8?B?UmV1bmnDo28gZGUgc2VndW5kYQ==?=\r\n\
Date: Thu, 22 Oct 2026 09:30:00 +0100\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>Ol&aacute;! <img src=\"https://tracker.example/x.png\">Agenda anexa.</p>\r\n\
--b\r\n\
Content-Type: text/calendar; name=\"agenda.ics\"\r\n\
Content-Disposition: attachment; filename=\"agenda.ics\"\r\n\
\r\n\
BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n\
--b--\r\n";

    #[test]
    fn parses_a_multipart_message() {
        let m = parse(SAMPLE.as_bytes(), MessageId(7), FolderId(0)).unwrap();
        assert_eq!(m.from.as_ref().map(|a| a.display()), Some("Ana Souza"));
        assert_eq!(m.to.len(), 2);
        assert_eq!(m.subject, "Reunião de segunda");
        let date = m.date.unwrap();
        assert_eq!((date.date.day(), date.hour(), date.minute()), (22, 9, 30));
        assert!(m.body.contains("Olá!"), "{:?}", m.body);
        assert!(!m.body.contains("tracker.example"), "remote image URL leaked into text: {:?}", m.body);
        assert_eq!(m.attachments.len(), 1);
        assert_eq!(m.attachments.first().map(|a| a.name.as_str()), Some("agenda.ics"));
    }

    #[test]
    fn hostile_input_never_panics() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            b"\r\n\r\n".to_vec(),
            vec![0xff; 4096],
            b"From: <<<@@@>>>\r\nDate: Thu, 99 Foo 99999 99:99:99 +9999\r\n\r\n".to_vec(),
            b"Content-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\n--x\r\n--x--".to_vec(),
            "Subject: =?utf-8?B?////?=\r\n\r\nї".as_bytes().to_vec(),
        ];
        for raw in cases {
            let _ = parse(&raw, MessageId(1), FolderId(0));
        }
        let mut nested = String::new();
        for i in 0..500 {
            nested.push_str(&format!("Content-Type: multipart/mixed; boundary=\"b{i}\"\r\n\r\n--b{i}\r\n"));
        }
        let _ = parse(nested.as_bytes(), MessageId(1), FolderId(0));
    }

    #[test]
    fn writes_mail_that_reads_back_and_blocks_header_injection() {
        let from = Address::parse("Denys <denys@example.com>").unwrap();
        let mut m = Message::new(MessageId(1), FolderId(3));
        m.to = Address::parse_list("Ana Souza <ana@example.org>").unwrap();
        m.cc = vec![Address::new(Some("José Ñúñez"), "jose@example.es").unwrap()];
        m.set_subject("Привіт\r\nBcc: victim@example.net");
        m.body = "Bom dia!\nДо зустрічі.\n".into();
        let raw = write(&m, &Outgoing { from: &from, date: "Thu, 22 Oct 2026 14:05:00 +0000", message_id: "abc\r\n123" }).unwrap();
        assert!(!raw.contains("\r\nBcc:"), "{raw}");
        assert!(raw.contains("Message-ID: <abc  123@example.com>") || raw.contains("Message-ID: <abc123@example.com>"), "{raw}");
        let back = parse(raw.as_bytes(), MessageId(2), FolderId(0)).unwrap();
        assert_eq!(back.subject, "Привіт  Bcc: victim@example.net");
        assert_eq!(back.cc.first().and_then(|a| a.name.as_deref()), Some("José Ñúñez"));
        assert_eq!(back.body.trim_end(), "Bom dia!\nДо зустрічі.");
        assert!(write(&Message::new(MessageId(3), FolderId(0)), &Outgoing { from: &from, date: "x", message_id: "y" }).is_err());
    }

    #[test]
    fn long_subjects_fold_into_several_words() {
        let s = encode_word(&"ї".repeat(100));
        assert!(s.lines().all(|l| l.trim_start().len() <= 76), "{s}");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b""), "");
    }
}
