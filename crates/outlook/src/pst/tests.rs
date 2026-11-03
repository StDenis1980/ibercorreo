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
                let _ = pst.appointment(n);
                let _ = pst.contact(n);
            }
        }
    }
}

mod items {
    use super::*;
    use crate::testing::pst::{ADDRESS, APPOINTMENT, COMMON, MEETING, Prop, boolean, named};
    use ibercorreo_core::calendar::When;
    use ibercorreo_core::{Date, DateTime};

    const NAMES: [([u8; 16], u32); 15] = [
        (APPOINTMENT, 0x820d), // 0 start
        (APPOINTMENT, 0x820e), // 1 end
        (APPOINTMENT, 0x8208), // 2 location
        (APPOINTMENT, 0x8215), // 3 all day
        (APPOINTMENT, 0x8216), // 4 recurrence
        (APPOINTMENT, 0x8223), // 5 recurring
        (APPOINTMENT, 0x8228), // 6 exception replaces
        (APPOINTMENT, 0x825e), // 7 time zone
        (COMMON, 0x8501),      // 8 reminder minutes
        (COMMON, 0x8503),      // 9 reminder set
        (MEETING, 0x0023),     // 10 clean global id
        (ADDRESS, 0x8083),     // 11 e-mail 1
        (ADDRESS, 0x8080),     // 12 e-mail 1 display name
        (ADDRESS, 0x8092),     // 13 e-mail 2 type
        (ADDRESS, 0x8093),     // 14 e-mail 2
    ];

    fn ft(y: i32, m: u8, d: u8, h: u8, min: u8) -> u64 {
        let unix = DateTime::new(Date::new(y, m, d).unwrap(), h, min, 0).unwrap().to_unix();
        (u64::try_from(unix).unwrap() + 11_644_473_600) * 10_000_000
    }

    fn minutes(y: i32, m: u8, d: u8) -> u32 {
        u32::try_from(ft(y, m, d, 0, 0) / 600_000_000).unwrap()
    }

    fn zone(key: &str) -> Vec<u8> {
        let mut b = vec![2, 1, 0, 0, 2, 0];
        b.extend_from_slice(&(key.encode_utf16().count() as u16).to_le_bytes());
        b.extend(key.encode_utf16().flat_map(u16::to_le_bytes));
        b.extend_from_slice(&0u16.to_le_bytes());
        b
    }

    /// Every Monday from 2025-03-03, the 17th deleted and the 24th moved.
    fn weekly() -> Vec<u8> {
        let mut b = Vec::new();
        for v in [0x3004u16, 0x3004, 0x200b, 0x0001, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let deleted = [minutes(2025, 3, 17), minutes(2025, 3, 24)];
        let mut words = vec![minutes(2025, 3, 2), 1, 0, 0x02, 0x2023, 10, 1, 2];
        words.extend(deleted);
        words.extend([1, minutes(2025, 3, 24), minutes(2025, 3, 3), 0x5ae9_80df]);
        for w in words {
            b.extend_from_slice(&w.to_le_bytes());
        }
        b
    }

    fn appointment(subject: &str, extra: Vec<Prop>) -> Msg {
        let mut props = vec![uni(0x001a, "IPM.Appointment"), uni(0x0037, subject)];
        props.extend(extra);
        Msg { props, ..Msg::default() }
    }

    fn archive(unicode: bool) -> Vec<u8> {
        let madrid = zone("Romance Standard Time");
        let calendar = vec![
            appointment(
                "Revisión trimestral",
                vec![
                    time(named(0), ft(2025, 3, 10, 8, 0)),
                    time(named(1), ft(2025, 3, 10, 9, 0)),
                    uni(named(2), "Sala 2"),
                    bin(named(7), &madrid),
                    boolean(named(9), true),
                    long(named(8), 15),
                    bin(named(10), &[0x04, 0x00, 0xab]),
                    uni(0x1000, "Orden del día\r\n1. Cuentas"),
                ],
            ),
            appointment(
                "Vacaciones",
                vec![
                    time(named(0), ft(2025, 3, 31, 22, 0)),
                    time(named(1), ft(2025, 4, 2, 22, 0)),
                    boolean(named(3), true),
                    boolean(named(9), false),
                ],
            ),
            Msg {
                attachments: vec![
                    Att {
                        props: vec![long(0x3705, 5)],
                        embedded: Some(appointment(
                            "Equipo (movida)",
                            vec![
                                time(named(6), ft(2025, 3, 24, 8, 30)),
                                time(named(0), ft(2025, 3, 24, 10, 0)),
                                time(named(1), ft(2025, 3, 24, 11, 0)),
                            ],
                        )),
                    },
                    // A change the pattern no longer lists (left behind by an older edit).
                    Att {
                        props: vec![long(0x3705, 5)],
                        embedded: Some(appointment(
                            "Equipo (vieja)",
                            vec![time(named(6), ft(2025, 3, 31, 8, 30)), time(named(0), ft(2025, 3, 31, 12, 0))],
                        )),
                    },
                    // A file attached to the meeting.
                    Att { props: vec![long(0x3705, 1), uni(0x3707, "agenda.pdf")], embedded: None },
                ],
                ..appointment(
                    "Equipo",
                    vec![
                        time(named(0), ft(2025, 3, 3, 8, 30)),
                        time(named(1), ft(2025, 3, 3, 9, 0)),
                        uni(named(2), "Online"),
                        bin(named(7), &madrid),
                        boolean(named(5), true),
                        bin(named(4), &weekly()),
                    ],
                )
            },
            appointment("Sin fecha", vec![]),
            // All day, saved with no end.
            appointment(
                "Fiesta",
                vec![time(named(0), ft(2025, 5, 4, 22, 0)), boolean(named(3), true), bin(named(7), &zone("Romance Standard Time"))],
            ),
            // No zone, and an end before the start.
            appointment("Llamada", vec![time(named(0), ft(2025, 6, 1, 7, 0)), time(named(1), ft(2025, 6, 1, 6, 0))]),
        ];
        let contacts = vec![
            Msg {
                props: vec![
                    uni(0x001a, "IPM.Contact"),
                    uni(0x3001, "Ana Gil"),
                    uni(named(11), "ana@contoso.es"),
                    uni(named(12), "Ana Gil (ana@contoso.es)"),
                    uni(named(13), "EX"),
                    uni(named(14), "/O=CONTOSO/CN=ANA"),
                    uni(0x3a1c, "+34 600 000 000"),
                    uni(0x3a08, "+34 960 000 000"),
                    uni(0x3a1b, "+34 960 000 000"),
                    uni(0x3a16, "Contoso, S.L."),
                    uni(0x3a17, "Gerente"),
                    time(0x3a42, ft(1990, 5, 19, 22, 0)),
                    uni(0x3a29, "C/ Mayor, 1"),
                    uni(0x3a27, "Sagunto"),
                    uni(0x3a26, "España"),
                    uni(0x1000, "Cliente desde 2019"),
                ],
                ..Msg::default()
            },
            Msg { props: vec![uni(0x001a, "IPM.DistList"), uni(0x3001, "Equipo")], ..Msg::default() },
            Msg { props: vec![uni(0x001a, "IPM.Contact"), uni(0x3a06, "Олег"), uni(0x3a11, "Коваль")], ..Msg::default() },
        ];
        let folders = vec![
            Folder { name: "Calendario".into(), class: "IPF.Appointment".into(), messages: calendar, ..Folder::default() },
            Folder { name: "Contactos".into(), class: "IPF.Contact".into(), messages: contacts, ..Folder::default() },
        ];
        Writer::new(Options { unicode, ..Options::default() }).names(&NAMES).archive("x", &folders)
    }

    fn at(y: i32, m: u8, d: u8, h: u8, min: u8) -> DateTime {
        DateTime::new(Date::new(y, m, d).unwrap(), h, min, 0).unwrap()
    }

    #[test]
    fn appointments_read_with_their_zone_series_and_changes() {
        for unicode in [true, false] {
            let pst = Pst::open(archive(unicode)).unwrap();
            let folders = pst.folders().unwrap();
            assert!(folders[0].holds_calendar() && !folders[0].holds_mail() && !folders[0].holds_contacts());
            let ids = pst.messages(&folders[0]).unwrap();
            assert_eq!(ids.len(), 6);

            let e = pst.appointment(ids[0]).unwrap().unwrap();
            assert_eq!(e.len(), 1);
            let e = &e[0];
            assert_eq!((e.summary.as_str(), e.location.as_str()), ("Revisión trimestral", "Sala 2"));
            assert_eq!(e.when, When::Timed { start: at(2025, 3, 10, 9, 0), end: at(2025, 3, 10, 10, 0), utc: false }, "Madrid time");
            assert_eq!(e.tzid.as_deref(), Some("Romance Standard Time"));
            assert_eq!(e.reminders, [15]);
            assert_eq!(e.uid, "0400AB");
            assert_eq!(e.description, "Orden del día\n1. Cuentas");

            let e = &pst.appointment(ids[1]).unwrap().unwrap()[0];
            let d = |m, d| Date::new(2025, m, d).unwrap();
            assert_eq!(e.when, When::AllDay { start: d(4, 1), end: d(4, 3) }, "all-day days from times saved at midnight in Madrid");
            assert!(e.reminders.is_empty());

            let series = pst.appointment(ids[2]).unwrap().unwrap();
            assert_eq!(series.len(), 2, "the series and its moved occurrence, not a change the pattern no longer lists");
            let (m, x) = (&series[0], &series[1]);
            assert_eq!(m.rrule.as_deref(), Some("FREQ=WEEKLY;INTERVAL=1;BYDAY=MO;WKST=MO"));
            assert_eq!(m.exdates, [d(3, 17)]);
            assert_eq!(m.when, When::Timed { start: at(2025, 3, 3, 9, 30), end: at(2025, 3, 3, 10, 0), utc: false });
            assert_eq!(x.recurrence_id, Some(d(3, 24)));
            assert_eq!((x.summary.as_str(), x.location.as_str()), ("Equipo (movida)", "Online"), "what the change leaves out comes from the series");
            assert_eq!(x.when, When::Timed { start: at(2025, 3, 24, 11, 0), end: at(2025, 3, 24, 12, 0), utc: false });
            assert_eq!((x.rrule.as_ref(), x.uid.as_str()), (None, m.uid.as_str()));

            assert_eq!(pst.appointment(ids[3]).unwrap(), None, "no start");
            let e = &pst.appointment(ids[4]).unwrap().unwrap()[0];
            assert_eq!(e.when, When::AllDay { start: d(5, 5), end: d(5, 6) }, "at least one day");
            let e = &pst.appointment(ids[5]).unwrap().unwrap()[0];
            assert_eq!(
                e.when,
                When::Timed { start: at(2025, 6, 1, 7, 0), end: at(2025, 6, 1, 7, 0), utc: true },
                "UTC without a zone; no end before the start"
            );
            assert_eq!(e.tzid, None);
            let contacts = pst.messages(&folders[1]).unwrap();
            assert_eq!(pst.appointment(contacts[0]).unwrap(), None, "a contact is not an appointment");
            assert!(pst.message(ids[0], MessageId(1), FolderId(1)).unwrap().is_none(), "nor is an appointment mail");
        }
    }

    #[test]
    fn contacts_read_with_addresses_phones_and_more() {
        for unicode in [true, false] {
            let pst = Pst::open(archive(unicode)).unwrap();
            let folders = pst.folders().unwrap();
            assert!(folders[1].holds_contacts());
            let ids = pst.messages(&folders[1]).unwrap();
            let c = pst.contact(ids[0]).unwrap().unwrap();
            assert_eq!(c.name, "Ana Gil");
            assert_eq!(
                c.emails.iter().map(|a| (a.name.as_deref(), a.email.as_str())).collect::<Vec<_>>(),
                [(None, "ana@contoso.es")],
                "an Exchange address is left out"
            );
            assert_eq!(
                c.phones.iter().map(|p| (p.kind.as_str(), p.number.as_str())).collect::<Vec<_>>(),
                [("mobile", "+34 600 000 000"), ("work", "+34 960 000 000")]
            );
            assert_eq!((c.organization.as_deref(), c.title.as_deref()), (Some("Contoso, S.L."), Some("Gerente")));
            assert_eq!(c.birthday, Some(Date::new(1990, 5, 20).unwrap()));
            assert_eq!(c.kept, ["ADR;TYPE=WORK:;;C/ Mayor\\, 1;Sagunto;;;España"]);
            assert_eq!(c.note, "Cliente desde 2019");
            assert_eq!(pst.contact(ids[1]).unwrap(), None, "a contact group");
            assert_eq!(pst.contact(ids[2]).unwrap().unwrap().name, "Олег Коваль", "the name from its parts");
        }
    }

    #[test]
    fn damaged_calendars_and_contacts_never_panic() {
        let good = archive(true);
        let mut x = 0x1234_5678u32;
        for round in 0..300 {
            let mut b = good.clone();
            for _ in 0..1 + round % 3 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let at = (x as usize) % b.len();
                b[at] = (x >> 8) as u8;
            }
            exercise(b);
        }
    }
}
