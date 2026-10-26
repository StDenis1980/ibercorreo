//! Messages in and out, L1.
//!
//! [`parse`] reads a raw RFC 5322 message (an `.eml` file, an IMAP `BODY[]`) into a
//! [`Message`] through Stalwart's `mail-parser` (Apache-2.0 OR MIT). HTML is read into safe
//! formatted text by [`html`]; nothing remote is fetched. [`write`] turns a message being sent into RFC 5322 text:
//! UTF-8 headers as RFC 2047 encoded words, the body as base64 when it is not plain ASCII, and no
//! header value can carry a line break (header injection). A meeting invitation's `text/calendar`
//! part is kept in [`Message::invite`]; a message carrying one is written as
//! `multipart/alternative` with the calendar as its second part (iMIP, RFC 6047).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use ibercorreo_core::mail::{Attachment, MAX_INVITE_BYTES};
use ibercorreo_core::{Address, Date, DateTime, Error, FolderId, MAX_FIELD_LEN, Message, MessageId, Result, clamp_text};
use mail_parser::{MessageParser, MimeHeaders, PartType};

pub mod html;

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
    let html = parsed.html_part(0).and_then(|p| match &p.body {
        PartType::Html(h) => Some(h.as_ref()),
        _ => None,
    });
    m.rich = html.map(html::to_rich);
    // HTML-only mail: the plain text comes from the same safe reading, without hidden preheaders.
    let text_is_html = parsed.text_part(0).is_some_and(|p| matches!(p.body, PartType::Html(_)));
    let body = match (&m.rich, text_is_html) {
        (Some(rich), true) => rich.to_text(),
        _ => parsed.body_text(0).map(|b| b.into_owned()).unwrap_or_default(),
    };
    m.body = clamp_text(&body.replace("\r\n", "\n"), MAX_BODY_BYTES).to_string();
    m.attachments = parsed
        .attachments()
        // An invitation's calendar sent as an alternative to the text is not a file (it is kept
        // in `invite` below); one attached under a name still shows.
        .filter(|part| part.attachment_name().is_some() || !is_calendar(part))
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
    m.invite = invite(&parsed);
    Ok(m)
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

/// RFC 5322 text for `m`, ready for SMTP `DATA` or saving as `.eml`. Lines end in CRLF.
pub fn write(m: &Message, out: &Outgoing<'_>) -> Result<String> {
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
    match &m.invite {
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
            text_part(&mut s, "text/plain; charset=utf-8", &m.body, Some(&boundary));
            s.push_str(&format!("--{boundary}\r\n"));
            text_part(&mut s, &format!("text/calendar; charset=utf-8; method={method}"), clamp_text(cal, MAX_INVITE_BYTES), Some(&boundary));
            s.push_str(&format!("--{boundary}--\r\n"));
        }
        None => text_part(&mut s, "text/plain; charset=utf-8", &m.body, None),
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
    fn parses_a_multipart_message() {
        let m = parse(SAMPLE.as_bytes(), MessageId(7), FolderId(0)).unwrap();
        assert_eq!(m.from.as_ref().map(|a| a.display()), Some("Ana Souza"));
        assert_eq!(m.to.len(), 2);
        assert_eq!(m.subject, "Reunião de segunda");
        let date = m.date.unwrap();
        assert_eq!((date.date.day(), date.hour(), date.minute()), (22, 8, 30), "09:30 +0100 is 08:30 UTC");
        assert!(m.body.contains("Olá!"), "{:?}", m.body);
        assert!(!m.body.contains("tracker.example"), "remote image URL leaked into text: {:?}", m.body);
        assert_eq!(m.attachments.len(), 1);
        assert_eq!(m.attachments.first().map(|a| a.name.as_str()), Some("agenda.ics"));
    }

    #[test]
    fn html_is_kept_safe_and_plain_text_preferred() {
        let m = parse(SAMPLE.as_bytes(), MessageId(7), FolderId(0)).unwrap();
        let rich = m.rich.as_ref().unwrap();
        assert_eq!(rich.remote_images, 1);
        assert!(rich.blocks.iter().flat_map(|b| &b.spans).any(|s| s.image), "the image keeps a placeholder");
        let alternative = "Content-Type: multipart/alternative; boundary=\"a\"\r\n\r\n--a\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nThe plain words\r\n--a\r\n\
Content-Type: text/html; charset=utf-8\r\n\r\n<p>The <b>formatted</b> words<script>x()</script></p>\r\n--a--\r\n";
        let m = parse(alternative.as_bytes(), MessageId(8), FolderId(0)).unwrap();
        assert_eq!(m.body.trim(), "The plain words", "the sender's own text version wins");
        assert_eq!(m.rich.unwrap().to_text(), "The formatted words");
        let plain = parse(b"Subject: x\r\n\r\njust text", MessageId(9), FolderId(0)).unwrap();
        assert_eq!(plain.rich, None, "plain mail gets no formatted body");
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
    fn blind_copies_never_reach_the_headers() {
        let from = Address::parse("Denys <denys@example.com>").unwrap();
        let mut m = Message::new(MessageId(1), FolderId(3));
        m.bcc = Address::parse_list("Boss <boss@example.net>").unwrap();
        m.body = "only blind".into();
        let raw = write(&m, &Outgoing { from: &from, date: "Thu, 22 Oct 2026 14:05:00 +0000", message_id: "b" }).unwrap();
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
        assert_eq!(m.invite.as_deref(), Some(REQUEST));
        assert_eq!(m.body.trim(), "You are invited", "the calendar is not read as the body");
        let files: Vec<&str> = m.attachments.iter().map(|a| a.mime_type.as_str()).collect();
        assert_eq!(files, ["application/ics"], "the unnamed calendar alternative is not a file");
        // An attached .ics counts too.
        assert!(parse(SAMPLE.as_bytes(), MessageId(2), FolderId(0)).unwrap().invite.is_some());
        // Not a calendar, or too big: none.
        let plain = parse(b"Subject: x\r\n\r\nBEGIN:VCALENDAR", MessageId(3), FolderId(0)).unwrap();
        assert_eq!(plain.invite, None);
        let huge = format!("Content-Type: text/calendar\r\n\r\nBEGIN:VCALENDAR\r\n{}", "X:y\r\n".repeat(MAX_INVITE_BYTES / 4));
        assert_eq!(parse(huge.as_bytes(), MessageId(4), FolderId(0)).unwrap().invite, None);
    }

    #[test]
    fn replies_carry_their_calendar_as_an_alternative() {
        let from = Address::parse("Denys <denys@example.com>").unwrap();
        let mut m = Message::new(MessageId(1), FolderId(3));
        m.to = Address::parse_list("ana@example.org").unwrap();
        m.set_subject("Accepted: Reunião");
        m.body = "Até já.\n--=_ibercorreo_YWJj\n".into();
        m.invite = Some(REQUEST.replace("METHOD:REQUEST", "METHOD:REPLY"));
        let out = Outgoing { from: &from, date: "Thu, 22 Oct 2026 14:05:00 +0000", message_id: "abc" };
        let raw = write(&m, &out).unwrap();
        assert_eq!(raw, write(&m, &out).unwrap(), "the same bytes every time");
        assert!(raw.contains("Content-Type: multipart/alternative; boundary=\"=_ibercorreo_YWJj\""), "{raw}");
        assert!(raw.contains("Content-Type: text/calendar; charset=utf-8; method=REPLY"), "{raw}");
        let back = parse(raw.as_bytes(), MessageId(2), FolderId(0)).unwrap();
        assert_eq!(back.invite.as_deref(), m.invite.as_deref());
        assert_eq!(back.body.trim_end(), "Até já.\n--=_ibercorreo_YWJj", "a body holding the boundary does not break the message");
        // A text that looks like the boundary is base64, so it cannot end the part.
        m.body = "plain\n--=_ibercorreo_YWJj--\n".into();
        let raw = write(&m, &out).unwrap();
        assert_eq!(raw.matches("--=_ibercorreo_YWJj--").count(), 1, "{raw}");
        assert_eq!(parse(raw.as_bytes(), MessageId(3), FolderId(0)).unwrap().invite.as_deref(), m.invite.as_deref());
        assert_eq!(calendar_method("METHOD:cancel\r\n"), "CANCEL");
        assert_eq!(calendar_method("METHOD:a;b\r\n"), "REQUEST");
    }
}
