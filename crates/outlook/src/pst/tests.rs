use super::*;
use crate::testing::pst::{Att, Folder, Msg, Options, Writer, bin, long, s8, time, uni};

/// 2026-10-30 09:15:00 UTC as a FILETIME.
const WHEN: u64 = (1_793_351_700 + 11_644_473_600) * 10_000_000;

fn full_message() -> Msg {
    Msg {
        props: vec![
            uni(0x001a, "IPM.Note"),
            uni(0x0037, "\u{1}\u{4}RE: Factura de octubre"),
            uni(0x0c1a, "Joan Martí"),
            uni(0x0c1f, "/O=EXCHANGELABS/CN=JOAN"),
            uni(0x5d01, "joan@empresa.es"),
            time(0x0e06, WHEN),
            long(0x0e07, 1),
            uni(0x1000, "Hola Ana,\r\nadjunto la factura."),
            bin(0x1013, b"<p>Hola <b>Ana</b></p>"),
            long(0x3fde, 65001),
        ],
        recipients: vec![
            vec![long(0x0c15, 1), uni(0x3001, "Ana Gil"), uni(0x3003, "ana@contoso.es")],
            vec![long(0x0c15, 2), uni(0x3001, "Pere"), uni(0x3003, "/O=X/CN=PERE"), uni(0x39fe, "pere@empresa.es")],
        ],
        attachments: vec![
            Att {
                props: vec![long(0x3705, 1), uni(0x3707, "Factura.pdf"), uni(0x370e, "application/pdf"), bin(0x3701, &vec![b'%'; 9000])],
                embedded: None,
            },
            Att {
                props: vec![long(0x3705, 5)],
                embedded: Some(Msg { props: vec![uni(0x001a, "IPM.Note"), uni(0x0037, "Pedido 42")], ..Msg::default() }),
            },
        ],
    }
}

fn simple(subject: &str) -> Msg {
    Msg { props: vec![uni(0x001a, "IPM.Note"), uni(0x0037, subject), uni(0x0c1a, "Ana"), uni(0x5d01, "ana@contoso.es")], ..Msg::default() }
}

fn archive(o: Options) -> Vec<u8> {
    let big_body: String = (0..3000).map(|i| format!("línea {i}\n")).collect();
    let mut long_mail = simple("Informe largo");
    long_mail.props.push(uni(0x1000, &big_body));
    let folders = vec![
        Folder {
            name: "Bandeja de entrada".into(),
            class: "IPF.Note".into(),
            folders: vec![Folder { name: "Proyectos".into(), class: "IPF.Note".into(), messages: vec![long_mail], ..Folder::default() }],
            messages: vec![full_message(), simple("Segundo")],
        },
        Folder {
            name: "Calendario".into(),
            class: "IPF.Appointment".into(),
            messages: vec![Msg { props: vec![uni(0x001a, "IPM.Appointment"), uni(0x0037, "Reunión")], ..Msg::default() }],
            ..Folder::default()
        },
        Folder {
            name: "Надіслані".into(),
            class: String::new(),
            messages: vec![Msg {
                props: vec![s8(0x001a, b"IPM.Note"), s8(0x0037, b"\xcf\xf0\xe8\xe2\xb3\xf2"), long(0x3ffd, 1251), s8(0x0c1f, b"oleh@example.com.ua")],
                ..Msg::default()
            }],
            ..Folder::default()
        },
    ];
    Writer::new(o).archive("Archivo 2025", &folders)
}

fn check(bytes: Vec<u8>, what: &str) {
    let pst = Pst::open(bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(pst.name(), "Archivo 2025", "{what}");
    let folders = pst.folders().unwrap();
    let paths: Vec<String> = folders.iter().map(|f| f.path.join("/")).collect();
    assert_eq!(paths, ["Bandeja de entrada", "Bandeja de entrada/Proyectos", "Calendario", "Надіслані"], "{what}");
    assert_eq!(folders.iter().map(PstFolder::holds_mail).collect::<Vec<_>>(), [true, true, false, true], "{what}");
    assert_eq!(folders[0].count, 2);

    let inbox = pst.messages(&folders[0]).unwrap();
    assert_eq!(inbox.len(), 2, "{what}");
    let m = pst.message(inbox[0], MessageId(9), FolderId(4)).unwrap().unwrap();
    assert_eq!((m.id, m.folder), (MessageId(9), FolderId(4)));
    assert_eq!(m.subject, "RE: Factura de octubre", "{what}: the prefix marker is dropped");
    assert_eq!(m.from.as_ref().unwrap().email, "joan@empresa.es");
    assert_eq!(m.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["ana@contoso.es"]);
    assert_eq!(m.cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["pere@empresa.es"], "{what}: SMTP before the Exchange address");
    assert_eq!(m.date.unwrap().to_unix(), 1_793_351_700);
    assert!(m.seen);
    let c = m.content().unwrap();
    assert_eq!(c.body, "Hola Ana,\nadjunto la factura.");
    assert_eq!(c.rich.as_ref().map(|r| r.to_text().trim().to_string()).as_deref(), Some("Hola Ana"));
    assert_eq!(m.attachments.len(), 2, "{what}");
    assert_eq!(
        (m.attachments[0].name.as_str(), m.attachments[0].mime_type.as_str(), m.attachments[0].size),
        ("Factura.pdf", "application/pdf", 9000),
        "{what}"
    );
    assert_eq!((m.attachments[1].name.as_str(), m.attachments[1].mime_type.as_str()), ("Pedido 42.eml", "message/rfc822"), "{what}");
    assert_eq!(pst.message(inbox[1], MessageId(1), FolderId(1)).unwrap().unwrap().subject, "Segundo");

    let long = pst.messages(&folders[1]).unwrap();
    let body = pst.message(long[0], MessageId(1), FolderId(1)).unwrap().unwrap().content().unwrap().body.clone();
    assert!(body.starts_with("línea 0\nlínea 1\n") && body.ends_with("línea 2999\n"), "{what}: a body kept in a subnode");

    let calendar = pst.messages(&folders[2]).unwrap();
    assert_eq!(calendar.len(), 1);
    assert!(pst.message(calendar[0], MessageId(1), FolderId(1)).unwrap().is_none(), "{what}: not mail");

    let sent = pst.messages(&folders[3]).unwrap();
    let m = pst.message(sent[0], MessageId(1), FolderId(1)).unwrap().unwrap();
    assert_eq!(m.subject, "Привіт", "{what}: 8-bit text in the message's code page");
    assert_eq!(m.from.unwrap().email, "oleh@example.com.ua");
}

#[test]
fn archives_read_in_every_format_and_encoding() {
    for unicode in [true, false] {
        for crypt in [0, 1, 2] {
            check(archive(Options { unicode, crypt, ..Options::default() }), &format!("unicode {unicode}, crypt {crypt}"));
        }
    }
}

#[test]
fn heaps_and_trees_of_several_blocks_and_levels_read() {
    for unicode in [true, false] {
        let o = Options { unicode, block: 300, two_level_subnodes: true, two_level_bth: true, big: 200, ..Options::default() };
        check(archive(o), &format!("small blocks, unicode {unicode}"));
    }
}

#[test]
fn big_b_trees_read() {
    // Enough messages for node and block B-trees of two levels.
    let messages: Vec<Msg> = (0..120).map(|i| simple(&format!("Mensaje {i}"))).collect();
    for unicode in [true, false] {
        let bytes = Writer::new(Options { unicode, ..Options::default() })
            .archive("x", &[Folder { name: "Inbox".into(), messages: messages.clone(), ..Folder::default() }]);
        let pst = Pst::open(bytes).unwrap();
        let f = pst.folders().unwrap();
        let ids = pst.messages(&f[0]).unwrap();
        assert_eq!(ids.len(), 120);
        let subjects: Vec<String> = ids.iter().map(|&n| pst.message(n, MessageId(1), FolderId(1)).unwrap().unwrap().subject).collect();
        assert_eq!(subjects[0], "Mensaje 0");
        assert_eq!(subjects[119], "Mensaje 119");
    }
}

#[test]
fn other_files_are_refused() {
    assert!(Pst::open(b"not a pst".to_vec()).is_err());
    let mut ost = archive(Options::default());
    ost[10..12].copy_from_slice(&36u16.to_le_bytes());
    assert!(Pst::open(ost).err().unwrap().to_string().contains(".ost"));
    let mut wip = archive(Options::default());
    wip[513] = 0x10;
    assert!(Pst::open(wip).is_err());
}

#[test]
fn damaged_archives_never_panic() {
    for unicode in [true, false] {
        let good = archive(Options { unicode, block: 400, two_level_subnodes: true, two_level_bth: true, big: 300, ..Options::default() });
        let mut x = 0x9e37_79b9u32;
        for round in 0..400 {
            let mut b = good.clone();
            for _ in 0..1 + round % 4 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let at = (x as usize) % b.len();
                b[at] = (x >> 8) as u8;
            }
            exercise(b);
        }
        for cut in (0..good.len()).step_by(good.len() / 97 + 1) {
            exercise(good[..cut].to_vec());
        }
    }
}

fn exercise(bytes: Vec<u8>) {
    let Ok(pst) = Pst::open(bytes) else { return };
    let _ = pst.name();
    let Ok(folders) = pst.folders() else { return };
    for f in &folders {
        if let Ok(ids) = pst.messages(f) {
            for n in ids.into_iter().take(10) {
                let _ = pst.message(n, MessageId(1), FolderId(1));
            }
        }
    }
}
