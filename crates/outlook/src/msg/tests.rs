use super::*;
use crate::cfb::Cfb;
use crate::testing::{Node, build, props, storage, stream, uni};

const LONG: u16 = PT_LONG;

fn s8(id: u16, bytes: &[u8]) -> Node {
    stream(&format!("__substg1.0_{id:04X}001E"), bytes.to_vec())
}

fn bin(id: u16, bytes: &[u8]) -> Node {
    stream(&format!("__substg1.0_{id:04X}0102"), bytes.to_vec())
}

fn recipient(n: u32, kind: u32, name: &str, email: &str, smtp: Option<&str>, addr_type: &str) -> Node {
    let mut c =
        vec![props(8, &[(tag::RECIPIENT_TYPE, LONG, u64::from(kind))]), uni(tag::DISPLAY_NAME, name), uni(tag::EMAIL, email), uni(0x3002, addr_type)];
    if let Some(s) = smtp {
        c.push(uni(tag::SMTP_ADDRESS, s));
    }
    storage(&format!("__recip_version1.0_#{n:08X}"), c)
}

/// 2026-10-30 09:15:00 UTC as a FILETIME.
const WHEN: u64 = (1_793_351_700 + 11_644_473_600) * 10_000_000;

fn embedded(subject: &str, inner: Vec<Node>) -> Node {
    let mut c = vec![props(24, &[]), uni(tag::MESSAGE_CLASS, "IPM.Note"), uni(tag::SUBJECT, subject)];
    c.extend(inner);
    storage("__substg1.0_3701000D", c)
}

fn full() -> Vec<u8> {
    let html = "<html><body><p>Hola <b>Ana</b>, adjunto la factura de octubre. Caf\u{e9}</p></body></html>";
    let html_1252: Vec<u8> = html.chars().map(|c| c as u32 as u8).collect();
    build(
        vec![
            props(32, &[(tag::DELIVERY_TIME, PT_TIME, WHEN), (tag::MESSAGE_FLAGS, LONG, 1), (tag::INTERNET_CODEPAGE, LONG, 1252)]),
            uni(tag::MESSAGE_CLASS, "IPM.Note"),
            uni(tag::SUBJECT, "Factura de octubre — 2026"),
            uni(tag::SENDER_NAME, "Joan Martí"),
            uni(tag::SENDER_EMAIL, "/O=EXCHANGELABS/OU=EXCHANGE ADMINISTRATIVE GROUP/CN=RECIPIENTS/CN=JOAN"),
            uni(0x0c1e, "EX"),
            uni(tag::SENDER_SMTP, "joan@empresa.es"),
            uni(tag::BODY, "Hola Ana,\r\nadjunto la factura de octubre. Café\r\n"),
            bin(tag::HTML, &html_1252),
            recipient(0, 1, "Ana Gil", "ana@contoso.es", None, "SMTP"),
            recipient(1, 2, "Pere", "/O=EXCHANGELABS/CN=PERE", Some("pere@empresa.es"), "EX"),
            recipient(2, 3, "archivo@empresa.es", "archivo@empresa.es", None, "SMTP"),
            recipient(3, 1, "Sin correo", "/O=EXCHANGELABS/CN=NOBODY", None, "EX"),
            storage(
                "__attach_version1.0_#00000000",
                vec![
                    props(8, &[(tag::ATTACH_METHOD, LONG, 1)]),
                    uni(tag::ATTACH_LONG_FILENAME, "Factura octubre.pdf"),
                    uni(tag::ATTACH_FILENAME, "FACTUR~1.PDF"),
                    uni(tag::ATTACH_MIME, "application/pdf"),
                    bin(tag::ATTACH_DATA, &vec![b'%'; 5000]),
                ],
            ),
            storage(
                "__attach_version1.0_#00000001",
                vec![props(8, &[(tag::ATTACH_METHOD, LONG, 1)]), uni(tag::ATTACH_MIME, "not a type"), bin(tag::ATTACH_DATA, &[1, 2, 3])],
            ),
            storage(
                "__attach_version1.0_#00000002",
                vec![
                    props(8, &[(tag::ATTACH_METHOD, LONG, 5)]),
                    embedded(
                        "Re: presupuesto",
                        vec![storage("__attach_version1.0_#00000000", vec![props(8, &[(tag::ATTACH_METHOD, LONG, 5)]), embedded("deeper", vec![])])],
                    ),
                ],
            ),
        ],
        9,
    )
}

#[test]
fn a_received_message_reads_whole() {
    let m = read(&full(), MessageId(7), FolderId(3)).unwrap();
    assert_eq!((m.id, m.folder), (MessageId(7), FolderId(3)));
    assert_eq!(m.subject, "Factura de octubre — 2026");
    assert_eq!(m.from.as_ref().unwrap().to_string(), "Joan Martí <joan@empresa.es>", "the SMTP address, not the Exchange one");
    assert_eq!(m.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["ana@contoso.es"], "an Exchange-only address is left out");
    assert_eq!(m.cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["pere@empresa.es"]);
    assert_eq!(m.bcc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["archivo@empresa.es"]);
    assert_eq!(m.bcc[0].name, None, "a name that only repeats the address is dropped");
    assert_eq!(m.date.unwrap().to_unix(), 1_793_351_700);
    assert!(m.seen);
    let c = m.content().unwrap();
    assert_eq!(c.body, "Hola Ana,\nadjunto la factura de octubre. Café\n");
    let rich = c.rich.as_ref().unwrap().to_text();
    assert!(rich.contains("Café") && rich.contains("Ana"), "the HTML in its code page: {rich}");
    let names: Vec<(&str, &str, usize)> = m.attachments.iter().map(|a| (a.name.as_str(), a.mime_type.as_str(), a.size)).collect();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&("Factura octubre.pdf", "application/pdf", 5000)), "{names:?}");
    assert!(names.contains(&("attachment", "application/octet-stream", 3)), "an odd type is not kept: {names:?}");
    assert!(names.contains(&("Re: presupuesto.eml", "message/rfc822", 0)), "an attached message is named by its subject: {names:?}");
}

#[test]
fn eight_bit_text_follows_the_code_page_and_headers_fill_gaps() {
    let subject_1251: &[u8] = b"\xcf\xf0\xe8\xe2\xb3\xf2"; // Привіт
    let headers = "Received: from x\r\nFrom: Ana <ana@contoso.es>\r\nTo: Oleh <oleh@example.com.ua>, b@example.org\r\nCc: c@example.org\r\nDate: Fri, 30 Oct 2026 10:15:00 +0100\r\nSubject: =?utf-8?B?0J/RgNC40LLRltGC?=\r\n";
    let bytes = build(
        vec![
            props(32, &[(tag::MESSAGE_CODEPAGE, LONG, 1251)]),
            s8(tag::MESSAGE_CLASS, b"IPM.Note"),
            s8(tag::SUBJECT, subject_1251),
            s8(tag::SENDER_EMAIL, b"/O=ORG/CN=ANA"),
            s8(0x0c1e, b"EX"),
            s8(tag::TRANSPORT_HEADERS, headers.as_bytes()),
            recipient(0, 1, "Oleh", "/O=ORG/CN=OLEH", None, "EX"),
            uni(tag::HTML, "<p>Тільки <i>HTML</i></p>"),
        ],
        9,
    );
    let m = read(&bytes, MessageId(1), FolderId(1)).unwrap();
    assert_eq!(m.subject, "Привіт");
    assert_eq!(m.from.as_ref().unwrap().email, "ana@contoso.es", "from the original headers");
    assert_eq!(m.to.len(), 2, "recipients from the headers when none had an SMTP address");
    assert_eq!(m.cc.len(), 1);
    assert_eq!(m.date.unwrap().to_unix(), 1_793_351_700, "the header date, in UTC");
    assert!(!m.seen, "unread when Outlook says so");
    assert_eq!(m.content().unwrap().body.trim(), "Тільки HTML", "the text of an HTML-only message");
    assert!(m.attachments.is_empty());
}

#[test]
fn code_pages_decode() {
    assert_eq!(decode(b"Caf\xe9", 1252), "Café");
    assert_eq!(decode("Café".as_bytes(), 65001), "Café");
    assert_eq!(decode(b"\xcf\xf0\xe8\xe2\xb3\xf2", 1251), "Привіт");
    assert_eq!(decode(b"\xf0\xd2\xc9\xd7\xa6\xd4", 21866), "Привіт", "KOI8-U");
    assert_ne!(decode(b"\xa6", 20866), "і", "KOI8-R has no Ukrainian і");
    assert_eq!(decode(b"Gr\xfc\xdfe", 28591), "Grüße", "ISO-8859-1");
    assert_eq!(decode(b"Caf\xe9", 12345), "Café", "unknown code pages read as Windows-1252");
    assert_eq!(from_filetime(0), None);
    assert_eq!(from_filetime(116_444_736_000_000_000).unwrap().to_unix(), 0);
}

#[test]
fn other_outlook_items_are_not_taken_for_mail() {
    for class in ["IPM.Contact", "IPM.Appointment", "IPM.Task", "IPM.StickyNote", "ipm.distlist"] {
        let bytes = build(vec![props(32, &[]), uni(tag::MESSAGE_CLASS, class), uni(tag::SUBJECT, "x")], 9);
        let e = read(&bytes, MessageId(1), FolderId(1)).unwrap_err().to_string();
        assert!(e.contains("not a mail message"), "{class}: {e}");
    }
    for class in ["IPM.Note", "IPM.Schedule.Meeting.Request", "REPORT.IPM.Note.NDR", ""] {
        let bytes = build(vec![props(32, &[]), uni(tag::MESSAGE_CLASS, class)], 9);
        assert!(read(&bytes, MessageId(1), FolderId(1)).is_ok(), "{class}");
    }
    assert!(read(b"From: a@b.c\r\n\r\nnot a .msg", MessageId(1), FolderId(1)).is_err());
    assert!(matches!(read(&vec![0; MAX_MSG_BYTES + 1], MessageId(1), FolderId(1)), Err(Error::TooLarge { .. })));
}

#[test]
fn damaged_files_never_panic() {
    let good = full();
    let mut x = 0x2545_f491u32;
    for _ in 0..3000 {
        let mut b = good.clone();
        for _ in 0..6 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let i = x as usize % b.len();
            b[i] = (x >> 24) as u8;
        }
        let _ = read(&b, MessageId(1), FolderId(1));
    }
}

#[test]
fn the_send_time_stands_in_and_blank_html_is_no_html() {
    let bytes = build(
        vec![
            props(32, &[(tag::CLIENT_SUBMIT_TIME, PT_TIME, WHEN), (tag::CREATION_TIME, PT_TIME, WHEN + 36_000_000_000)]),
            uni(tag::BODY, "plain"),
            uni(tag::HTML, "  \r\n "),
        ],
        9,
    );
    let m = read(&bytes, MessageId(1), FolderId(1)).unwrap();
    assert_eq!(m.date.unwrap().to_unix(), 1_793_351_700, "sent time before creation time");
    assert!(m.content().unwrap().rich.is_none(), "blank HTML is not a formatted body");
}

#[test]
fn recipients_and_attachments_are_capped() {
    let mut nodes = vec![props(32, &[]), uni(tag::MESSAGE_CLASS, "IPM.Note")];
    for i in 0..=MAX_RECIPIENTS {
        nodes.push(storage(&format!("__recip_version1.0_#{i:08X}"), vec![props(8, &[]), uni(tag::SMTP_ADDRESS, &format!("r{i}@example.org"))]));
    }
    for i in 0..=MAX_ATTACHMENTS {
        nodes.push(storage(&format!("__attach_version1.0_#{i:08X}"), vec![props(8, &[])]));
    }
    let m = read(&build(nodes, 12), MessageId(1), FolderId(1)).unwrap();
    assert_eq!(m.to.len(), MAX_RECIPIENTS);
    assert_eq!(m.attachments.len(), MAX_ATTACHMENTS);
}

#[test]
fn messages_attached_in_a_loop_do_not_recurse_for_ever() {
    // An attached message whose own attachment holds the attached message again.
    let bytes = build(
        vec![
            props(32, &[]),
            uni(tag::MESSAGE_CLASS, "IPM.Note"),
            storage(
                "__attach_version1.0_#00000000",
                vec![
                    props(8, &[(tag::ATTACH_METHOD, LONG, 5)]),
                    embedded("loop", vec![storage("__attach_version1.0_#00000000", vec![props(8, &[(tag::ATTACH_METHOD, LONG, 5)])])]),
                ],
            ),
        ],
        9,
    );
    let c = Cfb::open(&bytes).unwrap();
    let outer = c.find(0, "__attach_version1.0_#00000000").unwrap();
    let object = c.find(outer, "__substg1.0_3701000D").unwrap();
    let inner = c.find(object, "__attach_version1.0_#00000000").unwrap();
    // Point the inner attachment's children at the outer attached message.
    let dir = 512 * (crate::cfb::u32_at(&bytes, 0x30).unwrap() as usize + 1);
    let mut looped = bytes.clone();
    let at = dir + 128 * inner + 76;
    looped[at..at + 4].copy_from_slice(&(object as u32).to_le_bytes());
    let m = read(&looped, MessageId(1), FolderId(1)).unwrap();
    assert_eq!(m.attachments[0].name, "loop.eml");
}

#[test]
fn bodies_kept_only_as_compressed_rtf_are_read() {
    let html_rtf = crate::testing::compress_rtf(
        b"{\\rtf1\\ansi\\ansicpg1252\\fromhtml1 {\\*\\htmltag19 <html><body>}{\\*\\htmltag64 <p><b>}\\htmlrtf {\\b\\htmlrtf0 Caf\\'e9 menu\\htmlrtf}\\htmlrtf0 {\\*\\htmltag72 </b></p></body></html>}}",
    );
    let note = |extra: Vec<Node>| {
        let mut nodes = vec![props(32, &[]), uni(tag::MESSAGE_CLASS, "IPM.Note")];
        nodes.extend(extra);
        let m = read(&build(nodes, 9), MessageId(1), FolderId(1)).unwrap();
        let c = m.content().unwrap();
        (c.body.clone(), c.rich.as_ref().map(|r| r.to_text()))
    };
    // HTML mail: the HTML comes out of the RTF, the plain text Outlook also kept stays the text.
    let (body, rich) = note(vec![uni(tag::BODY, "Cafe menu (plain)"), bin(tag::RTF_COMPRESSED, &html_rtf)]);
    assert_eq!(body, "Cafe menu (plain)");
    assert_eq!(rich.as_deref().map(str::trim), Some("Café menu"));
    // With no plain text, the formatted body gives it.
    let (body, rich) = note(vec![bin(tag::RTF_COMPRESSED, &html_rtf)]);
    assert_eq!((body.trim(), rich.is_some()), ("Café menu", true));
    // An HTML property wins over the RTF.
    let (_, rich) = note(vec![uni(tag::HTML, "<p>from the HTML property</p>"), bin(tag::RTF_COMPRESSED, &html_rtf)]);
    assert_eq!(rich.as_deref().map(str::trim), Some("from the HTML property"));
    // Plain-text and RTF-native mail give text only.
    let text_rtf = crate::testing::compress_rtf(b"{\\rtf1\\ansi\\fromtext \\pard Line one\\par Line two}");
    let (body, rich) = note(vec![bin(tag::RTF_COMPRESSED, &text_rtf)]);
    assert_eq!((body.as_str(), rich), ("Line one\nLine two", None));
    let (body, _) = note(vec![uni(tag::BODY, "saved text"), bin(tag::RTF_COMPRESSED, &text_rtf)]);
    assert_eq!(body, "saved text", "the plain text Outlook saved wins over the RTF's");
    // Damaged RTF leaves the plain text alone.
    let mut broken = html_rtf.clone();
    let last = broken.len() - 1;
    broken[last] ^= 1;
    let (body, rich) = note(vec![uni(tag::BODY, "plain"), bin(tag::RTF_COMPRESSED, &broken)]);
    assert_eq!((body.as_str(), rich), ("plain", None));
}

#[test]
fn the_html_body_follows_the_internet_code_page() {
    // Outlook saved the text properties in Windows-1251 but the HTML in KOI8-U, as received.
    let bytes = build(
        vec![
            props(32, &[(tag::MESSAGE_CODEPAGE, LONG, 1251), (tag::INTERNET_CODEPAGE, LONG, 21866)]),
            s8(tag::MESSAGE_CLASS, b"IPM.Note"),
            s8(tag::SUBJECT, b"\xcf\xf0\xe8\xe2\xb3\xf2"),
            bin(tag::HTML, b"<p>\xf0\xd2\xc9\xd7\xa6\xd4</p>"),
        ],
        9,
    );
    let m = read(&bytes, MessageId(1), FolderId(1)).unwrap();
    assert_eq!(m.subject, "Привіт", "text properties in the message code page");
    assert_eq!(m.content().unwrap().body.trim(), "Привіт", "the HTML in the Internet code page");
}
