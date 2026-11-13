use ibercorreo_core::rich::{Block, BlockKind, Rich, Span};

use super::*;

fn when() -> DateTime {
    DateTime::new(Date::new(2026, 10, 2).unwrap(), 9, 5, 7).unwrap()
}

fn message(id: u64) -> Message {
    let mut m = Message::new(MessageId(id), FolderId(1));
    m.from = Some(Address::new(Some("Ana Gil"), "ana@contoso.es").unwrap());
    m.to = vec![Address::new(Some("Denys Míshchenko"), "denys@example.com").unwrap()];
    m.subject = "Reunión del lunes".into();
    m.date = Some(when());
    m
}

fn text(body: &str) -> Content {
    Content { body: body.into(), ..Content::default() }
}

fn span(text: &str) -> Span {
    Span { text: text.into(), ..Span::default() }
}

fn block(kind: BlockKind, quote: u8, spans: Vec<Span>) -> Block {
    Block { kind, quote, spans }
}

#[test]
fn dates_are_written_as_mail_and_mbox_want_them() {
    assert_eq!(date_header(when()), "Fri, 2 Oct 2026 09:05:07 +0000");
    assert_eq!(asctime(when()), "Fri Oct  2 09:05:07 2026", "the day is padded with a space");
    let later = DateTime::new(Date::new(2026, 12, 31).unwrap(), 23, 59, 59).unwrap();
    assert_eq!(asctime(later), "Thu Dec 31 23:59:59 2026");
}

#[test]
fn a_plain_message_reads_back_the_same() {
    let mut m = message(7);
    m.cc = vec![Address::new(None, "pere@contoso.es").unwrap()];
    m.bcc = vec![Address::new(None, "hidden@contoso.es").unwrap()];
    m.seen = true;
    let raw = archived(&m, &text("Hola,\nnos vemos a las 10.\n"));
    assert!(raw.contains("Status: RO\r\nX-Mozilla-Status: 0001\r\n"), "{raw}");
    assert!(!raw.contains("X-Status"), "not flagged");
    assert!(!raw.contains("hidden@"), "blind copies are never written");
    assert!(raw.contains("Message-ID: <7.tinta-correo@localhost>\r\n"));
    assert_eq!(raw, archived(&m, &text("Hola,\nnos vemos a las 10.\n")), "the same bytes every time");
    let back = parse(raw.as_bytes(), MessageId(1), FolderId(0)).unwrap();
    assert_eq!(
        (back.from.clone(), back.to.clone(), back.cc.clone(), back.subject.clone(), back.date),
        (m.from.clone(), m.to.clone(), m.cc.clone(), m.subject.clone(), m.date)
    );
    assert_eq!(back.content().map(|c| c.body.trim_end().to_string()), Some("Hola,\nnos vemos a las 10.".to_string()));
    assert!(back.content().is_some_and(|c| c.rich.is_none()), "plain stays plain");
}

#[test]
fn flags_and_missing_fields_are_written_as_they_are() {
    let mut m = Message::new(MessageId(3), FolderId(1));
    m.flagged = true;
    m.subject = "Línea\r\nBcc: evil@example.org".into();
    let raw = archived(&m, &text(""));
    assert!(raw.contains("Status: O\r\nX-Status: F\r\nX-Mozilla-Status: 0004\r\n"), "{raw}");
    assert!(!raw.contains("\r\nFrom:") && !raw.starts_with("From:") && !raw.contains("Date:"), "{raw}");
    assert!(!raw.contains("\r\nBcc:"), "a subject can't start a header");
    let back = parse(raw.as_bytes(), MessageId(1), FolderId(0)).unwrap();
    assert_eq!((back.from, back.date), (None, None));
}

#[test]
fn the_formatted_body_and_an_invitation_are_alternatives() {
    let m = message(9);
    let rich = Rich {
        blocks: vec![
            block(BlockKind::Paragraph, 0, vec![span("Hola "), Span { bold: true, ..span("equipo") }]),
            block(BlockKind::Paragraph, 0, vec![Span { link: Some("https://contoso.es/a?b=1&c=2".into()), ..span("la agenda") }]),
        ],
        remote_images: 0,
    };
    let invite = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nEND:VCALENDAR\r\n";
    let content = Content { body: "Hola equipo\n\nla agenda".into(), rich: Some(rich), invite: Some(invite.into()) };
    let raw = archived(&m, &content);
    let parts: Vec<&str> = raw.split("--=_tinta_correo_9").collect();
    assert_eq!(parts.len(), 5, "preamble, three parts, end: {raw}");
    assert!(parts.get(1).is_some_and(|p| p.contains("text/plain")));
    assert!(parts.get(2).is_some_and(|p| p.contains("text/html")));
    assert!(parts.get(3).is_some_and(|p| p.contains("text/calendar; charset=utf-8; method=REQUEST")));
    assert!(parts.get(4).is_some_and(|p| p.starts_with("--\r\n")));
    let back = parse(raw.as_bytes(), MessageId(1), FolderId(0)).unwrap();
    let c = back.content().unwrap();
    let html = c.rich.as_ref().expect("the formatted body is read back");
    let link = html.blocks.iter().flat_map(|b| &b.spans).find_map(|s| s.link.clone());
    assert_eq!(link.as_deref(), Some("https://contoso.es/a?b=1&c=2"));
    assert!(html.blocks.iter().flat_map(|b| &b.spans).any(|s| s.bold && s.text.contains("equipo")));
    assert!(c.invite.as_deref().is_some_and(|i| i.contains("METHOD:REQUEST")));
}

#[test]
fn html_keeps_the_shape_of_the_body() {
    let rich = Rich {
        blocks: vec![
            block(BlockKind::Heading(0), 0, vec![span("Título <1>")]),
            block(BlockKind::Heading(9), 0, vec![span("Pequeño")]),
            block(BlockKind::Bullet { depth: 1 }, 0, vec![span("uno")]),
            block(BlockKind::Bullet { depth: 2 }, 0, vec![span("uno.a")]),
            block(BlockKind::Numbered { depth: 1, number: 3 }, 0, vec![span("tres")]),
            block(BlockKind::Paragraph, 2, vec![span("citado"), Span { image: true, ..span("logo") }]),
            block(BlockKind::Bullet { depth: 1 }, 2, vec![Span { italic: true, code: true, ..span("a\"b'c") }]),
            block(BlockKind::Rule, 0, Vec::new()),
            block(BlockKind::Pre, 0, vec![span("x = 1\ny = 2")]),
        ],
        remote_images: 2,
    };
    let html = to_html(&rich);
    let body = html.split("<body>\n").nth(1).unwrap();
    assert_eq!(
        body,
        "<h1>Título &lt;1&gt;</h1>\n<h6>Pequeño</h6>\n<ul>\n<li>uno</li>\n<ul>\n<li>uno.a</li>\n</ul>\n</ul>\n<ol>\n<li value=\"3\">tres</li>\n</ol>\n\
<blockquote>\n<blockquote>\n<p>citado</p>\n<ul>\n<li><i><code>a&quot;b&#39;c</code></i></li>\n</ul>\n</blockquote>\n</blockquote>\n<hr>\n\
<pre>x = 1<br>y = 2</pre>\n</body></html>\n"
    );
    assert!(to_html(&Rich::default()).ends_with("<body>\n</body></html>\n"));
    let quoted_list = Rich {
        blocks: vec![block(BlockKind::Bullet { depth: 1 }, 0, vec![span("a")]), block(BlockKind::Bullet { depth: 1 }, 1, vec![span("b")])],
        remote_images: 0,
    };
    assert!(
        to_html(&quoted_list).contains("<ul>\n<li>a</li>\n</ul>\n<blockquote>\n<ul>\n<li>b</li>\n</ul>\n</blockquote>\n"),
        "a list closes before a quote opens: {}",
        to_html(&quoted_list)
    );
}

#[test]
fn hostile_bodies_stay_bounded_html() {
    let deep = Rich {
        blocks: vec![
            block(BlockKind::Bullet { depth: 255 }, 255, vec![span("x")]),
            block(BlockKind::Numbered { depth: 0, number: u32::MAX }, 0, vec![span("y")]),
        ],
        remote_images: 0,
    };
    let html = to_html(&deep);
    assert_eq!(html.matches("<blockquote>").count(), 255);
    assert_eq!(html.matches("</blockquote>").count(), 255);
    assert_eq!(html.matches("<ul>").count(), 16, "list depth is capped");
    assert_eq!(html.matches("<ul>").count(), html.matches("</ul>").count());
    assert!(html.contains("<li value=\"4294967295\">y</li>"));
}

#[test]
fn an_mbox_keeps_messages_apart_and_quotes_from_lines() {
    let mut mbox = String::new();
    push_mbox(&mut mbox, &message(1), &text("From here on\n>From quoted\nok"));
    let mut anonymous = Message::new(MessageId(2), FolderId(1));
    anonymous.subject = "sin remitente".into();
    push_mbox(&mut mbox, &anonymous, &text("hola"));
    let mut spaced = message(3);
    spaced.from = Some(Address { name: None, email: "a b@c".into() });
    push_mbox(&mut mbox, &spaced, &text("x"));
    assert!(mbox.starts_with("From ana@contoso.es Fri Oct  2 09:05:07 2026\nFrom: "), "{mbox}");
    assert!(mbox.contains("\n>From here on\n>>From quoted\nok\n"), "{mbox}");
    assert!(mbox.contains("\nFrom MAILER-DAEMON Thu Jan  1 00:00:00 1970\n"), "{mbox}");
    assert!(mbox.contains("\nFrom MAILER-DAEMON Fri Oct  2 09:05:07 2026\n"), "an odd sender address is not put in the From line");
    assert!(!mbox.contains('\r'), "LF line ends");
    let starts: Vec<usize> = mbox.match_indices("\nFrom ").map(|(i, _)| i).collect();
    assert_eq!(starts.len(), 2, "only the separators start with From: {mbox}");
    for (i, (from, to)) in
        std::iter::once(0).chain(starts.iter().map(|i| i + 1)).zip(starts.iter().map(|i| i + 1).chain(std::iter::once(mbox.len()))).enumerate()
    {
        let one = mbox.get(from..to).unwrap();
        assert!(one.ends_with("\n\n"), "message {i} ends with a blank line");
        let raw: String = one
            .split_once('\n')
            .unwrap()
            .1
            .lines()
            .map(|l| l.strip_prefix('>').filter(|r| r.trim_start_matches('>').starts_with("From ")).unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\r\n");
        assert!(parse(raw.as_bytes(), MessageId(1), FolderId(0)).is_ok(), "message {i} reads back");
    }
}
