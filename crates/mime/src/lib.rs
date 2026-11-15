//! Messages in and out, L1.
//!
//! [`parse`] reads a raw RFC 5322 message (an `.eml` file, an IMAP `BODY[]`) into a
//! [`Message`] through Stalwart's `mail-parser` (Apache-2.0 OR MIT). HTML is read into safe
//! formatted text by [`html`]; nothing remote is fetched. [`write`] turns a message being sent into RFC 5322 text:
//! UTF-8 headers as RFC 2047 encoded words, the body as base64 when it is not plain ASCII, and no
//! header value can carry a line break (header injection). A meeting invitation's `text/calendar`
//! part is kept in [`Message::invite`]; a message carrying one is written as
//! `multipart/alternative` with the calendar as its second part (iMIP, RFC 6047).
//! [`archive`] writes messages for keeping outside the app: `.eml` text and mbox files.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;

use ibercorreo_core::mail::{Attachment, MAX_INVITE_BYTES};
use ibercorreo_core::{Address, Content, Date, DateTime, Error, FolderId, MAX_FIELD_LEN, Message, MessageId, Result, clamp_text};
use mail_parser::{MessageParser, MimeHeaders, PartType};

pub mod archive;
pub mod html;

/// Largest message read: 64 MiB, above what common servers accept (Gmail 25 MB, Exchange Online 150 MB is rare).
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// Largest body text kept in the model; longer bodies are cut at a char boundary.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_ADDRESSES: usize = 2_000;
const MAX_ATTACHMENTS: usize = 1_000;

/// Read `raw` as one message placed in `folder` under `id`.
pub fn parse(raw: &[u8], id: MessageId, folder: FolderId) -> Result<Message> {
    read(raw, id, folder, false).map(|(m, _)| m)
}

/// [`parse`], and the contents of the message's attachments, one per [`Message::attachments`]
/// entry in the same order (decoded from their transfer encoding).
pub fn parse_with_files(raw: &[u8], id: MessageId, folder: FolderId) -> Result<(Message, Vec<Vec<u8>>)> {
    read(raw, id, folder, true)
}

fn read(raw: &[u8], id: MessageId, folder: FolderId, keep_files: bool) -> Result<(Message, Vec<Vec<u8>>)> {
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
    let html = parsed.html_part(0).and_then(|p| match &p.body {
        PartType::Html(h) => Some(h.as_ref()),
        _ => None,
    });
    let rich = html.map(html::to_rich);
    // HTML-only mail: the plain text comes from the same safe reading, without hidden preheaders.
    let text_is_html = parsed.text_part(0).is_some_and(|p| matches!(p.body, PartType::Html(_)));
    let body = match (&rich, text_is_html) {
        (Some(rich), true) => rich.to_text(),
        _ => parsed.body_text(0).map(|b| b.into_owned()).unwrap_or_default(),
    };
    let body = clamp_text(&body.replace("\r\n", "\n"), MAX_BODY_BYTES).to_string();
    let parts: Vec<&mail_parser::MessagePart<'_>> = parsed
        .attachments()
        // An invitation's calendar sent as an alternative to the text is not a file (it is kept
        // in `invite` below); one attached under a name still shows.
        .filter(|part| part.attachment_name().is_some() || !is_calendar(part))
        .take(MAX_ATTACHMENTS)
        .collect();
    let files = if keep_files { parts.iter().map(|part| part.contents().to_vec()).collect() } else { Vec::new() };
    m.attachments = parts
        .iter()
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
            sha256: None,
            content_id: part.content_id().and_then(ibercorreo_core::mail::content_id_key),
        })
        .collect();
    m.set_content(Arc::new(Content { body, rich, invite: invite(&parsed) }));
    Ok((m, files))
}

fn is_calendar(part: &mail_parser::MessagePart<'_>) -> bool {
    part.content_type().is_some_and(|ct| ct.ctype().eq_ignore_ascii_case("text") && ct.subtype().is_some_and(|s| s.eq_ignore_ascii_case("calendar")))
}

/// The first `text/calendar` part, inline or attached (iMIP, RFC 6047 §2.4), when it fits.
fn invite(parsed: &mail_parser::Message<'_>) -> Option<String> {
    let part = parsed.parts.iter().find(|p| is_calendar(p))?;
    let bytes = part.contents();
    if bytes.len() > MAX_INVITE_BYTES {
        return None;
    }
    let text = String::from_utf8_lossy(bytes);
    text.contains("BEGIN:VCALENDAR").then(|| text.into_owned())
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
/// The `Date:` header as UTC: its offset is taken off, so mail from every zone sorts and shows
/// right.
fn convert_date(d: &mail_parser::DateTime) -> Option<DateTime> {
    let date = Date::new(i32::from(d.year), d.month, d.day).ok()?;
    let local = DateTime::new(date, d.hour, d.minute, d.second).ok()?;
    // Offsets beyond ±18 hours are not real; read them as none.
    let offset = (i64::from(d.tz_hour) * 3600 + i64::from(d.tz_minute) * 60).min(18 * 3600);
    let offset = if d.tz_before_gmt { -offset } else { offset };
    DateTime::from_unix(local.to_unix().checked_sub(offset)?).ok()
}

/// What [`write`] needs besides the message itself.
pub struct Outgoing<'a> {
    pub from: &'a Address,
    /// The `Date:` header, already formatted per RFC 5322 (`Thu, 22 Oct 2026 14:05:00 +0000`).
    pub date: &'a str,
    /// Unique part of the `Message-ID:` (the domain comes from the sender's address).
    pub message_id: &'a str,
}

/// RFC 5322 text for `m`, which says `content`, ready for SMTP `DATA` or saving as `.eml`. Lines
/// end in CRLF.
pub fn write(m: &Message, content: &Content, out: &Outgoing<'_>) -> Result<String> {
    // Bcc is never written: blind copies go only in the SMTP envelope.
    if m.to.is_empty() && m.cc.is_empty() && m.bcc.is_empty() {
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
    match &content.invite {
        // An invitation or a reply to one: the text and the calendar as alternatives (RFC 6047
        // §2.4), the calendar last as the richest. The boundary comes from the Message-ID, so the
        // same message always renders to the same bytes.
        Some(cal) => {
            let unique: String = base64(out.message_id.as_bytes()).chars().filter(char::is_ascii_alphanumeric).take(40).collect();
            let boundary = format!("=_ibercorreo_{unique}");
            let method = calendar_method(cal);
            header(&mut s, "Content-Type", &format!("multipart/alternative; boundary=\"{boundary}\""));
            s.push_str("\r\n");
            s.push_str(&format!("--{boundary}\r\n"));
            text_part(&mut s, "text/plain; charset=utf-8", &content.body, Some(&boundary));
            s.push_str(&format!("--{boundary}\r\n"));
            text_part(&mut s, &format!("text/calendar; charset=utf-8; method={method}"), clamp_text(cal, MAX_INVITE_BYTES), Some(&boundary));
            s.push_str(&format!("--{boundary}--\r\n"));
        }
        None => text_part(&mut s, "text/plain; charset=utf-8", &content.body, None),
    }
    Ok(s)
}

/// The `METHOD` of a calendar object, for its `Content-Type` (`REQUEST` when it names none or
/// something odd).
fn calendar_method(cal: &str) -> String {
    let method = cal
        .lines()
        .find_map(|l| l.trim().strip_prefix("METHOD:"))
        .map(|v| v.trim().to_ascii_uppercase())
        .filter(|v| !v.is_empty() && v.len() <= 32 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    method.unwrap_or_else(|| "REQUEST".into())
}

/// Content headers and the text of one part: 7bit when it is short-lined ASCII, base64 otherwise
/// (and when the text holds the multipart `boundary`, which base64 never can).
fn text_part(s: &mut String, content_type: &str, text: &str, boundary: Option<&str>) {
    let text = text.replace("\r\n", "\n");
    header(s, "Content-Type", content_type);
    if text.is_ascii() && text.lines().all(|l| l.len() <= 998) && !boundary.is_some_and(|b| text.contains(b)) {
        header(s, "Content-Transfer-Encoding", "7bit");
        s.push_str("\r\n");
        for line in text.lines() {
            // SMTP dot-stuffing is the transport's job; a lone "." stays as written here.
            s.push_str(line);
            s.push_str("\r\n");
        }
    } else {
        header(s, "Content-Transfer-Encoding", "base64");
        s.push_str("\r\n");
        let encoded = base64(text.replace('\n', "\r\n").as_bytes());
        for chunk in encoded.as_bytes().chunks(76) {
            s.push_str(std::str::from_utf8(chunk).unwrap_or(""));
            s.push_str("\r\n");
        }
    }
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

/// Tinta: what a server that is not IMAP (Exchange, Microsoft Graph) tells of a message without
/// its MIME text, for mail downloaded header first.
#[derive(Clone, Debug, Default)]
pub struct Envelope {
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    /// When it was sent, UTC.
    pub date: Option<DateTime>,
    /// `<id@host>`; one that is not plain ASCII is left out.
    pub message_id: Option<String>,
}

/// The header of a message from its [`Envelope`], as RFC 5322 text that ends with the empty
/// line, so it parses like an IMAP `BODY[HEADER]`.
pub fn envelope_header(e: &Envelope) -> Vec<u8> {
    let mut s = String::new();
    if let Some(from) = &e.from {
        header(&mut s, "From", &address_list(std::slice::from_ref(from)));
    }
    if !e.to.is_empty() {
        header(&mut s, "To", &address_list(&e.to));
    }
    if !e.cc.is_empty() {
        header(&mut s, "Cc", &address_list(&e.cc));
    }
    let subject = single_line(clamp_text(&e.subject, MAX_FIELD_LEN));
    if !subject.trim().is_empty() {
        header(&mut s, "Subject", &encode_word(&subject));
    }
    if let Some(date) = e.date {
        header(&mut s, "Date", &archive::date_header(date));
    }
    if let Some(id) = e.message_id.as_deref().map(str::trim).filter(|id| id.len() <= 998 && id.bytes().all(|b| b.is_ascii_graphic())) {
        header(&mut s, "Message-ID", id);
    }
    s.push_str("\r\n");
    s.into_bytes()
}

/// Standard base64 (RFC 4648 §4) on one line, with padding.
pub fn base64(bytes: &[u8]) -> String {
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

/// Read standard base64 (RFC 4648 §4), skipping whitespace and line breaks; `None` for anything
/// else that is not base64 or for misplaced padding.
pub fn unbase64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(u32::from(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        }))
    };
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits, mut padding) = (0u32, 0u32, 0usize);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        if c == b'=' {
            padding += 1;
            continue;
        }
        if padding > 0 {
            return None;
        }
        acc = ((acc << 6) | value(c)?) & 0xFF_FFFF;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    // Two or three characters of a last group and its padding; a single character is not base64.
    (padding <= 2 && bits != 6 && (padding == 0 || bits > 0)).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_reads_back() {
        for sample in [&b""[..], b"f", b"fo", b"foo", b"foob", "Reunión €".as_bytes(), &[0, 255, 128, 7]] {
            assert_eq!(unbase64(&base64(sample)).as_deref(), Some(sample), "{sample:?}");
        }
        assert_eq!(unbase64("Zm9v\r\nYmFy").as_deref(), Some(&b"foobar"[..]), "line breaks are skipped");
        assert_eq!(unbase64("Zm9vYg"), Some(b"foob".to_vec()), "missing padding is read");
        for bad in ["Zm9v!", "Zg==Zg==", "Z", "Zm9vY", "Zg==="] {
            assert_eq!(unbase64(bad), None, "{bad}");
        }
    }

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
    fn dates_are_kept_in_utc() {
        let at = |header: &str| {
            let raw = format!("From: a@b.example\r\nDate: {header}\r\nSubject: x\r\n\r\nhi\r\n");
            parse(raw.as_bytes(), MessageId(1), FolderId(0)).unwrap().date.map(|d| d.to_string())
        };
        assert_eq!(at("Thu, 22 Oct 2026 21:00:00 -0330").as_deref(), Some("2026-10-23T00:30"), "St John's evening is the next UTC day");
        assert_eq!(at("Fri, 1 Jan 2027 01:00:00 +0300").as_deref(), Some("2026-12-31T22:00"), "back across a year");
        assert_eq!(at("Thu, 22 Oct 2026 09:30:00 +0000").as_deref(), Some("2026-10-22T09:30"));
        assert_eq!(at("Thu, 22 Oct 2026 09:30:00 GMT").as_deref(), Some("2026-10-22T09:30"));
        // A nonsense offset does not move the time by days.
        let wild = at("Thu, 22 Oct 2026 09:30:00 +9959").unwrap();
        assert!(wild.starts_with("2026-10-2"), "{wild}");
    }

    #[test]
    fn inline_pictures_are_tied_to_their_parts() {
        // multipart/related: the HTML names its logo by Content-ID, as mail programs send it.
        let raw = "From: shop@example.com\r\nSubject: Factura\r\nMIME-Version: 1.0\r\n\
                   Content-Type: multipart/related; boundary=\"b\"\r\n\r\n\
                   --b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
                   <p>Hola <img src=\"cid:Logo%40shop\" alt=\"Shop\"> <img src=\"cid:missing\"></p>\r\n\
                   --b\r\nContent-Type: image/png\r\nContent-ID: <logo@SHOP>\r\n\
                   Content-Disposition: inline\r\nContent-Transfer-Encoding: base64\r\n\r\n\
                   iVBORw0KGgo=\r\n--b--\r\n";
        let (m, files) = parse_with_files(raw.as_bytes(), MessageId(1), FolderId(0)).unwrap();
        assert_eq!(m.attachments.len(), 1);
        assert_eq!(m.attachments.first().and_then(|a| a.content_id.as_deref()), Some("logo@shop"));
        assert_eq!(files.first().map(Vec::len), Some(8), "the picture's bytes are kept to show it");
        let content = m.content().unwrap();
        let rich = content.rich.as_ref().unwrap();
        let cids: Vec<_> = rich.blocks.iter().flat_map(|b| &b.spans).filter(|s| s.image).map(|s| s.cid.as_deref()).collect();
        assert_eq!(cids, [Some("logo@shop"), Some("missing")], "both pictures name their part, found or not");
        // A picture on the web never names a part, so nothing of it can be drawn.
        let web = html::to_rich("<img src=\"https://cdn.example/a.png\" alt=\"A\">");
        assert!(web.blocks.iter().flat_map(|b| &b.spans).all(|s| s.cid.is_none()));
    }

    #[test]
    fn content_ids_compare_as_mail_programs_write_them() {
        use ibercorreo_core::mail::content_id_key;
        assert_eq!(content_id_key(" <Part1.ABC@x> ").as_deref(), Some("part1.abc@x"));
        assert_eq!(content_id_key("CID:part1.abc%40x").as_deref(), Some("part1.abc@x"));
        assert_eq!(content_id_key("cid:"), None);
        assert_eq!(content_id_key("<>"), None);
        assert_eq!(content_id_key(&"a".repeat(300)), None, "a hostile id is not kept");
        assert_eq!(content_id_key("%zz%4").as_deref(), Some("%zz%4"), "a broken escape stays as written");
    }

    #[test]
    fn parses_a_multipart_message() {
        let m = parse(SAMPLE.as_bytes(), MessageId(7), FolderId(0)).unwrap();
        assert_eq!(m.from.as_ref().map(|a| a.display()), Some("Ana Souza"));
        assert_eq!(m.to.len(), 2);
        assert_eq!(m.subject, "Reunião de segunda");
        let date = m.date.unwrap();
        assert_eq!((date.date.day(), date.hour(), date.minute()), (22, 8, 30), "09:30 +0100 is 08:30 UTC");
        assert!(m.content().unwrap().body.contains("Olá!"), "{:?}", m.content().unwrap().body);
        assert!(!m.content().unwrap().body.contains("tracker.example"), "remote image URL leaked into text: {:?}", m.content().unwrap().body);
        assert_eq!(m.attachments.len(), 1);
        assert_eq!(m.attachments.first().map(|a| a.name.as_str()), Some("agenda.ics"));
    }

    #[test]
    fn html_is_kept_safe_and_plain_text_preferred() {
        let m = parse(SAMPLE.as_bytes(), MessageId(7), FolderId(0)).unwrap();
        let rich = m.content().unwrap().rich.as_ref().unwrap();
        assert_eq!(rich.remote_images, 1);
        assert!(rich.blocks.iter().flat_map(|b| &b.spans).any(|s| s.image), "the image keeps a placeholder");
        let alternative = "Content-Type: multipart/alternative; boundary=\"a\"\r\n\r\n--a\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nThe plain words\r\n--a\r\n\
Content-Type: text/html; charset=utf-8\r\n\r\n<p>The <b>formatted</b> words<script>x()</script></p>\r\n--a--\r\n";
        let m = parse(alternative.as_bytes(), MessageId(8), FolderId(0)).unwrap();
        assert_eq!(m.content().unwrap().body.trim(), "The plain words", "the sender's own text version wins");
        assert_eq!(m.content().unwrap().rich.clone().unwrap().to_text(), "The formatted words");
        let plain = parse(b"Subject: x\r\n\r\njust text", MessageId(9), FolderId(0)).unwrap();
        assert_eq!(plain.content().unwrap().rich, None, "plain mail gets no formatted body");
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
        m.content_mut().unwrap().body = "Bom dia!\nДо зустрічі.\n".into();
        let raw =
            write(&m, m.content().unwrap(), &Outgoing { from: &from, date: "Thu, 22 Oct 2026 14:05:00 +0000", message_id: "abc\r\n123" }).unwrap();
        assert!(!raw.contains("\r\nBcc:"), "{raw}");
        assert!(raw.contains("Message-ID: <abc  123@example.com>") || raw.contains("Message-ID: <abc123@example.com>"), "{raw}");
        let back = parse(raw.as_bytes(), MessageId(2), FolderId(0)).unwrap();
        assert_eq!(back.subject, "Привіт  Bcc: victim@example.net");
        assert_eq!(back.cc.first().and_then(|a| a.name.as_deref()), Some("José Ñúñez"));
        assert_eq!(back.content().unwrap().body.trim_end(), "Bom dia!\nДо зустрічі.");
        assert!(write(&Message::new(MessageId(3), FolderId(0)), &Content::default(), &Outgoing { from: &from, date: "x", message_id: "y" }).is_err());
    }

    #[test]
    fn blind_copies_never_reach_the_headers() {
        let from = Address::parse("Denys <denys@example.com>").unwrap();
        let mut m = Message::new(MessageId(1), FolderId(3));
        m.bcc = Address::parse_list("Boss <boss@example.net>").unwrap();
        m.content_mut().unwrap().body = "only blind".into();
        let raw = write(&m, m.content().unwrap(), &Outgoing { from: &from, date: "Thu, 22 Oct 2026 14:05:00 +0000", message_id: "b" }).unwrap();
        assert!(!raw.contains("boss@") && !raw.contains("Bcc") && !raw.contains("\r\nTo:"), "{raw}");
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

    const REQUEST: &str = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:m1\r\nDTSTART:20261102T100000Z\r\nSUMMARY:Reunião\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn invitations_are_kept_from_any_calendar_part() {
        // Outlook and Gmail: the calendar as an alternative to the text.
        let alternative = format!(
            "From: ana@example.org\r\nSubject: Invitation\r\nContent-Type: multipart/mixed; boundary=\"m\"\r\n\r\n--m\r\n\
Content-Type: multipart/alternative; boundary=\"a\"\r\n\r\n--a\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nYou are invited\r\n\
--a\r\nContent-Type: text/calendar; charset=utf-8; method=REQUEST\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--a--\r\n\
--m\r\nContent-Type: application/ics; name=invite.ics\r\nContent-Disposition: attachment\r\n\r\nX\r\n--m--\r\n",
            base64(REQUEST.as_bytes())
        );
        let m = parse(alternative.as_bytes(), MessageId(1), FolderId(0)).unwrap();
        assert_eq!(m.content().unwrap().invite.as_deref(), Some(REQUEST));
        assert_eq!(m.content().unwrap().body.trim(), "You are invited", "the calendar is not read as the body");
        let files: Vec<&str> = m.attachments.iter().map(|a| a.mime_type.as_str()).collect();
        assert_eq!(files, ["application/ics"], "the unnamed calendar alternative is not a file");
        // An attached .ics counts too.
        assert!(parse(SAMPLE.as_bytes(), MessageId(2), FolderId(0)).unwrap().content().unwrap().invite.is_some());
        // Not a calendar, or too big: none.
        let plain = parse(b"Subject: x\r\n\r\nBEGIN:VCALENDAR", MessageId(3), FolderId(0)).unwrap();
        assert_eq!(plain.content().unwrap().invite, None);
        let huge = format!("Content-Type: text/calendar\r\n\r\nBEGIN:VCALENDAR\r\n{}", "X:y\r\n".repeat(MAX_INVITE_BYTES / 4));
        assert_eq!(parse(huge.as_bytes(), MessageId(4), FolderId(0)).unwrap().content().unwrap().invite, None);
    }

    #[test]
    fn replies_carry_their_calendar_as_an_alternative() {
        let from = Address::parse("Denys <denys@example.com>").unwrap();
        let mut m = Message::new(MessageId(1), FolderId(3));
        m.to = Address::parse_list("ana@example.org").unwrap();
        m.set_subject("Accepted: Reunião");
        m.content_mut().unwrap().body = "Até já.\n--=_ibercorreo_YWJj\n".into();
        m.content_mut().unwrap().invite = Some(REQUEST.replace("METHOD:REQUEST", "METHOD:REPLY"));
        let out = Outgoing { from: &from, date: "Thu, 22 Oct 2026 14:05:00 +0000", message_id: "abc" };
        let raw = write(&m, m.content().unwrap(), &out).unwrap();
        assert_eq!(raw, write(&m, m.content().unwrap(), &out).unwrap(), "the same bytes every time");
        assert!(raw.contains("Content-Type: multipart/alternative; boundary=\"=_ibercorreo_YWJj\""), "{raw}");
        assert!(raw.contains("Content-Type: text/calendar; charset=utf-8; method=REPLY"), "{raw}");
        let back = parse(raw.as_bytes(), MessageId(2), FolderId(0)).unwrap();
        assert_eq!(back.content().unwrap().invite.as_deref(), m.content().unwrap().invite.as_deref());
        assert_eq!(back.content().unwrap().body.trim_end(), "Até já.\n--=_ibercorreo_YWJj", "a body holding the boundary does not break the message");
        // A text that looks like the boundary is base64, so it cannot end the part.
        m.content_mut().unwrap().body = "plain\n--=_ibercorreo_YWJj--\n".into();
        let raw = write(&m, m.content().unwrap(), &out).unwrap();
        assert_eq!(raw.matches("--=_ibercorreo_YWJj--").count(), 1, "{raw}");
        assert_eq!(
            parse(raw.as_bytes(), MessageId(3), FolderId(0)).unwrap().content().unwrap().invite.as_deref(),
            m.content().unwrap().invite.as_deref()
        );
        assert_eq!(calendar_method("METHOD:cancel\r\n"), "CANCEL");
        assert_eq!(calendar_method("METHOD:a;b\r\n"), "REQUEST");
    }

    #[test]
    fn an_envelope_becomes_a_header_that_parses_back() {
        let e = Envelope {
            from: Some(Address::new(Some("Марта"), "marta@example.org").unwrap()),
            to: vec![Address::new(None, "you@example.org").unwrap()],
            cc: Vec::new(),
            subject: "Presupuesto\r\nBcc: evil@example.org ✓".into(),
            date: DateTime::parse("2025-03-11T09:19:00").ok().map(|(d, _)| d),
            message_id: Some("<bad id>".into()),
        };
        let raw = envelope_header(&e);
        let text = String::from_utf8(raw.clone()).unwrap();
        assert!(text.ends_with("\r\n\r\n") && !text.contains("Bcc:") && !text.contains("Message-ID"), "{text}");
        let m = parse(&raw, MessageId(1), FolderId(0)).unwrap();
        assert_eq!(m.subject, "Presupuesto  Bcc: evil@example.org ✓");
        assert_eq!(m.from.as_ref().and_then(|a| a.name.as_deref()), Some("Марта"));
        assert_eq!(m.date.map(|d| d.to_string()).as_deref(), Some("2025-03-11T09:19"));
        assert!(m.preview(40).is_empty());
    }
}
