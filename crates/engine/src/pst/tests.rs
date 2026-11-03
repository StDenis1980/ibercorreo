use ibercorreo_core::{Date, FolderRole};
use ibercorreo_outlook::testing::pst::{Folder, Msg, Options, Writer, long, uni};
use serde_json::json;

use super::*;
use crate::{Session, sample};

fn today() -> Date {
    Date::new(2026, 10, 22).unwrap()
}

fn mail(subject: &str, from: &str, day: u32) -> Msg {
    // A FILETIME on 2025-01-day.
    let when = (1_735_689_600 + u64::from(day) * 86_400 + 11_644_473_600) * 10_000_000;
    Msg {
        props: vec![
            uni(0x001a, "IPM.Note"),
            uni(0x0037, subject),
            uni(0x0c1a, from),
            uni(0x5d01, &format!("{from}@contoso.es")),
            ibercorreo_outlook::testing::pst::time(0x0e06, when),
            long(0x0e07, 1),
            uni(0x1000, "Hola"),
        ],
        ..Msg::default()
    }
}

fn archive(name: &str) -> Vec<u8> {
    let folders = vec![
        Folder {
            name: "Bandeja de entrada".into(),
            class: "IPF.Note".into(),
            messages: vec![mail("Factura", "ana", 1), mail("Pedido", "pere", 2)],
            folders: vec![Folder {
                name: "Proyectos/2025".into(),
                class: "IPF.Note".into(),
                messages: vec![mail("Plano", "joan", 3)],
                ..Folder::default()
            }],
        },
        Folder { name: "Bandeja de salida".into(), class: "IPF.Note".into(), ..Folder::default() },
        Folder {
            name: "Calendario".into(),
            class: "IPF.Appointment".into(),
            messages: vec![Msg { props: vec![uni(0x001a, "IPM.Appointment"), uni(0x0037, "Reunión")], ..Msg::default() }],
            ..Folder::default()
        },
        Folder {
            name: "Notas".into(),
            class: "IPF.Note".into(),
            messages: vec![Msg { props: vec![uni(0x001a, "IPM.StickyNote"), uni(0x0037, "Nota")], ..Msg::default() }, mail("Recibo", "ana", 4)],
            ..Folder::default()
        },
    ];
    Writer::new(Options::default()).archive(name, &folders)
}

fn names(s: &Session, prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = s.store.folders().iter().filter(|f| f.name.starts_with(prefix)).map(|f| f.name.clone()).collect();
    v.sort();
    v
}

#[test]
fn mail_folders_become_local_folders_once() {
    let mut s = Session::new(sample::store(today()).unwrap(), today());
    let pst = Pst::open(archive("Archivo 2025")).unwrap();
    let mut seen = Vec::new();
    let batch = read(&pst, |done, total| seen.push((done, total))).unwrap();
    assert_eq!(seen.last(), Some(&(6, 6)), "every item counted, the progress ends at the total");
    assert_eq!((batch.skipped, batch.failed), (2, 0), "the appointment and the sticky note");

    let r = s.apply_pst(None, batch).unwrap();
    assert_eq!(r, Report { folders: 3, messages: 4, duplicates: 0, skipped: 2, failed: 0, ..Report::default() });
    assert_eq!(
        names(&s, "Archivo 2025"),
        ["Archivo 2025/Bandeja de entrada", "Archivo 2025/Bandeja de entrada/Proyectos∕2025", "Archivo 2025/Notas"],
        "empty and non-mail folders are not made; a slash in a name does not read as a level"
    );
    let inbox = s
        .store
        .folders()
        .iter()
        .find(|f| f.name == "Archivo 2025/Bandeja de entrada")
        .map(|f| (f.id, f.role, f.account, f.remote.is_none()))
        .unwrap();
    assert_eq!((inbox.1, inbox.2, inbox.3), (FolderRole::Other, s.store.first_account(), true));
    let subjects: Vec<&str> = s.store.messages_in(inbox.0).iter().map(|m| m.subject.as_str()).collect();
    assert_eq!(subjects, ["Pedido", "Factura"]);
    assert_eq!(s.folder, inbox.0, "the first folder imported is shown");
    assert!(s.dirty && !s.can_undo());
    assert_eq!(s.status, "Imported 4 messages into 3 new folders, 2 tasks, notes or other items left out");

    let again = read(&Pst::open(archive("Archivo 2025")).unwrap(), |_, _| {}).unwrap();
    let r = s.apply_pst(None, again).unwrap();
    assert_eq!((r.folders, r.messages, r.duplicates), (0, 0, 4), "importing the archive again adds nothing");
    assert_eq!(names(&s, "Archivo 2025").len(), 3);
    assert!(s.status.contains("4 already here"), "{}", s.status);
}

#[test]
fn the_command_takes_the_file_and_an_account() {
    let mut s = Session::new(sample::store(today()).unwrap(), today());
    let data = ibercorreo_mime::base64(&archive(""));
    let second = s.store.accounts[1].id;
    let v = s.run("message.importPst", &json!({ "data": data, "account": second.0 })).unwrap();
    assert_eq!(v["folders"], 3);
    assert_eq!((v["messages"].as_u64(), v["skipped"].as_u64(), v["events"].as_u64()), (Some(4), Some(2), Some(0)));
    assert!(s.store.folders_of(second).iter().any(|f| f.name == "Outlook/Notas"), "an archive without a name is called Outlook");
    assert!(!s.store.folders_of(s.store.first_account()).iter().any(|f| f.name.starts_with("Outlook/")));

    assert!(s.run("message.importPst", &json!({ "data": data, "account": 999 })).is_err());
    assert!(s.run("message.importPst", &json!({ "data": "not base64!" })).is_err());
    let err = s.run("message.importPst", &json!({ "data": ibercorreo_mime::base64(b"plain text, not an archive") })).unwrap_err();
    assert!(err.to_string().contains(".pst"), "{err}");
}

#[test]
fn folder_names() {
    assert_eq!(folder_name(" ", &["Inbox".into()]), "Outlook/Inbox");
    assert_eq!(folder_name("A", &["".into(), " b/c ".into()]), "A/?/b∕c");
    assert!(folder_name("A", &["x".repeat(5000)]).len() <= 1024);
}

#[test]
fn a_server_folder_of_the_same_name_is_left_alone() {
    let mut s = Session::new(sample::store(today()).unwrap(), today());
    let account = s.store.first_account();
    let synced = s.store.add_folder(account, "Outlook/Notas", FolderRole::Other);
    s.store.folder_mut(synced).unwrap().remote = Some(ibercorreo_core::RemoteFolder { path: "Outlook/Notas".into(), uid_validity: 1 });
    let r = s.apply_pst(None, read(&Pst::open(archive("")).unwrap(), |_, _| {}).unwrap()).unwrap();
    assert_eq!(r.folders, 3, "local copies are made; imported mail is never put in a folder the server mirrors");
    assert!(s.store.messages_in(synced).is_empty());
}

#[test]
fn an_imported_folder_can_be_deleted_and_brought_back() {
    let mut s = Session::new(sample::store(today()).unwrap(), today());
    s.apply_pst(None, read(&Pst::open(archive("A")).unwrap(), |_, _| {}).unwrap()).unwrap();
    let inbox = s.store.folders().iter().find(|f| f.name == "A/Bandeja de entrada").map(|f| f.id).unwrap();
    s.run("folder.favorite", &json!({ "folder": inbox.0 })).unwrap();
    assert_eq!(s.folder, inbox);
    let v = s.run("folder.delete", &json!({})).unwrap();
    assert_eq!(v, json!({ "folder": inbox.0, "messages": 2 }));
    assert!(s.store.folder(inbox).is_err() && !s.store.is_favorite(inbox));
    assert_ne!(s.folder, inbox, "the selection moves off the deleted folder");
    assert!(s.store.all_messages().all(|m| m.folder != inbox));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.store.messages_in(inbox).len(), 2, "undo brings the folder and its mail back");

    let real_inbox = s.store.folder_by_role(s.store.first_account(), FolderRole::Inbox).unwrap();
    assert!(s.run("folder.delete", &json!({ "folder": real_inbox.0 })).is_err(), "a standard folder stays");
    let synced = s.store.add_folder(s.store.first_account(), "Server", FolderRole::Other);
    s.store.folder_mut(synced).unwrap().remote = Some(ibercorreo_core::RemoteFolder { path: "Server".into(), uid_validity: 1 });
    assert!(s.run("folder.delete", &json!({ "folder": synced.0 })).is_err(), "a folder the server mirrors stays");
    assert!(s.run("folder.delete", &json!({ "folder": 9999 })).is_err());
}

mod calendars_and_contacts {
    use super::*;
    use ibercorreo_core::calendar::When;
    use ibercorreo_outlook::testing::pst::{ADDRESS, APPOINTMENT, MEETING, bin, boolean, named, time};

    const NAMES: [([u8; 16], u32); 5] = [(APPOINTMENT, 0x820d), (APPOINTMENT, 0x820e), (APPOINTMENT, 0x8215), (MEETING, 0x0023), (ADDRESS, 0x8083)];

    /// 2025-03-day at 08:00 UTC as a FILETIME.
    fn at(day: u64) -> u64 {
        (1_740_787_200 + (day - 1) * 86_400 + 8 * 3600 + 11_644_473_600) * 10_000_000
    }

    fn appointment(subject: &str, day: u64, goid: Option<&[u8]>) -> Msg {
        let mut props = vec![uni(0x001a, "IPM.Appointment"), uni(0x0037, subject), time(named(0), at(day)), time(named(1), at(day) + 36_000_000_000)];
        if let Some(g) = goid {
            props.push(bin(named(3), g));
        }
        Msg { props, ..Msg::default() }
    }

    fn contact(name: &str, email: &str) -> Msg {
        Msg { props: vec![uni(0x001a, "IPM.Contact"), uni(0x3001, name), uni(named(4), email)], ..Msg::default() }
    }

    fn archive() -> Vec<u8> {
        let folders = vec![
            Folder {
                name: "Calendario".into(),
                class: "IPF.Appointment".into(),
                messages: vec![
                    appointment("Dentista", 3, Some(&[1, 2, 3])),
                    appointment("Sin id", 4, None),
                    Msg {
                        props: vec![uni(0x001a, "IPM.Appointment"), uni(0x0037, "Festivo"), time(named(0), at(18)), boolean(named(2), true)],
                        ..Msg::default()
                    },
                ],
                folders: vec![Folder { name: "Vacío".into(), class: "IPF.Appointment".into(), ..Folder::default() }],
            },
            Folder {
                name: "Contactos".into(),
                class: "IPF.Contact".into(),
                messages: vec![
                    contact("Ana Gil", "ana@contoso.es"),
                    contact("Pere", "pere@contoso.es"),
                    Msg { props: vec![uni(0x001a, "IPM.DistList")], ..Msg::default() },
                ],
                folders: vec![Folder { name: "Sin nadie".into(), class: "IPF.Contact".into(), ..Folder::default() }],
            },
            Folder {
                name: "Tareas".into(),
                class: "IPF.Task".into(),
                messages: vec![Msg { props: vec![uni(0x001a, "IPM.Task"), uni(0x0037, "Llamar")], ..Msg::default() }],
                ..Folder::default()
            },
        ];
        Writer::new(Options::default()).names(&NAMES).archive("Archivo", &folders)
    }

    #[test]
    fn calendars_and_contacts_become_local_ones_once() {
        let mut s = Session::new(sample::store(today()).unwrap(), today());
        let (calendars, books) = (s.store.calendars().len(), s.store.books().len());
        let batch = read(&Pst::open(archive()).unwrap(), |_, _| {}).unwrap();
        assert_eq!((batch.skipped, batch.failed), (2, 0), "the contact group and the task");
        let r = s.apply_pst(None, batch).unwrap();
        assert_eq!(
            (r.calendars, r.events, r.books, r.contacts, r.folders, r.messages),
            (1, 3, 1, 2, 0, 0),
            "an empty calendar or address book is not made"
        );
        assert_eq!((s.store.calendars().len(), s.store.books().len()), (calendars + 1, books + 1));
        let cal = s.store.calendars().iter().find(|c| c.name == "Archivo/Calendario").cloned().unwrap();
        assert!(cal.remote.is_none());
        let events: Vec<&ibercorreo_core::Event> = s.store.events().iter().filter(|e| e.calendar == cal.id).collect();
        let dentist = events.iter().find(|e| e.summary == "Dentista").unwrap();
        assert_eq!(dentist.uid, "010203");
        let no_id = events.iter().find(|e| e.summary == "Sin id").unwrap();
        assert!(no_id.uid.starts_with("outlook-"), "{}", no_id.uid);
        let holiday = events.iter().find(|e| e.summary == "Festivo").unwrap();
        assert!(matches!(holiday.when, When::AllDay { .. }));
        let book = s.store.books().iter().find(|b| b.name == "Archivo/Contactos").map(|b| b.id).unwrap();
        let mut names: Vec<&str> = s.store.all_contacts().iter().filter(|c| c.book == book).map(|c| c.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["Ana Gil", "Pere"]);
        assert!(s.status.contains("3 events into 1 new calendars") && s.status.contains("2 contacts into 1 new address books"), "{}", s.status);

        let r = s.apply_pst(None, read(&Pst::open(archive()).unwrap(), |_, _| {}).unwrap()).unwrap();
        assert_eq!((r.calendars, r.events, r.books, r.contacts, r.duplicates), (0, 0, 0, 0, 5), "a second import adds nothing");
        assert_eq!(s.store.calendars().len(), calendars + 1);
    }

    #[test]
    fn a_server_calendar_of_the_same_name_is_left_alone() {
        let mut s = Session::new(sample::store(today()).unwrap(), today());
        let synced = s
            .store
            .add_calendar("Archivo/Calendario", "#000000", Some(ibercorreo_core::CalendarRemote { href: "/dav/x/".into(), ctag: None }))
            .unwrap();
        let synced_book =
            s.store.add_book("Archivo/Contactos", Some(ibercorreo_core::CalendarRemote { href: "/dav/y/".into(), ctag: None })).unwrap();
        let r = s.apply_pst(None, read(&Pst::open(archive()).unwrap(), |_, _| {}).unwrap()).unwrap();
        assert_eq!((r.calendars, r.books), (1, 1));
        assert!(s.store.events().iter().all(|e| e.calendar != synced));
        assert!(s.store.all_contacts().iter().all(|c| c.book != synced_book));
    }

    #[test]
    fn made_uids_stay_the_same() {
        let e = ibercorreo_core::Event {
            id: ibercorreo_core::EventId(0),
            calendar: 0,
            uid: String::new(),
            summary: "Sin id".into(),
            location: String::new(),
            description: String::new(),
            when: When::AllDay { start: Date::new(2025, 3, 4).unwrap(), end: Date::new(2025, 3, 5).unwrap() },
            tzid: None,
            rrule: None,
            exdates: Vec::new(),
            recurrence_id: None,
            reminders: Vec::new(),
            remote: None,
        };
        assert_eq!(made_uid(&e), "outlook-334b9254340a3ded", "across versions of the app, so a later import finds the event");
    }
}
