use super::*;
use crate::calendar::{Change, Pushed, State};
use ibercorreo_dav::{Response, Url};

const EWS: &str = "https://mail.contoso.es/EWS/Exchange.asmx";
const M: &str =
    r#"xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types""#;

struct Item {
    id: String,
    key: String,
    folder: String,
    /// The text (iCalendar or vCard) as Exchange exports it.
    text: String,
}

/// An Exchange mailbox with a calendar, a birthday calendar, contacts and their hidden recipient
/// cache, answering the operations calendars and contacts use.
#[derive(Default)]
struct Exchange {
    items: Vec<Item>,
    asked: Vec<(String, String)>,
    made: u32,
    /// An operation answered with this HTTP status instead.
    fail: Option<(&'static str, u16)>,
    /// Items listed but deleted before they are read.
    vanished: Vec<&'static str>,
}

fn ids<'b>(body: &'b str, element: &str) -> Vec<&'b str> {
    body.match_indices(&format!("{element} Id=\"")).filter_map(|(at, m)| body.get(at + m.len()..)).filter_map(|r| r.get(..r.find('"')?)).collect()
}

fn envelope(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Header><h:ServerVersionInfo MajorVersion="15" xmlns:h="http://schemas.microsoft.com/exchange/services/2006/types"/></s:Header><s:Body>{body}</s:Body></s:Envelope>"#
    )
}

fn ok(op: &str, inner: &str) -> String {
    format!(
        r#"<m:{op}Response {M}><m:ResponseMessages><m:{op}ResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode>{inner}</m:{op}ResponseMessage></m:ResponseMessages></m:{op}Response>"#
    )
}

fn not_found(op: &str) -> String {
    format!(
        r#"<m:{op}Response {M}><m:ResponseMessages><m:{op}ResponseMessage ResponseClass="Error"><m:MessageText>Not found.</m:MessageText><m:ResponseCode>ErrorItemNotFound</m:ResponseCode></m:{op}ResponseMessage></m:ResponseMessages></m:{op}Response>"#
    )
}

fn folder(kind: &str, id: &str, class: &str, name: &str) -> String {
    format!(
        r#"<t:Create><t:{kind}><t:FolderId Id="{id}" ChangeKey="k"/><t:ParentFolderId Id="ROOT"/><t:FolderClass>{class}</t:FolderClass><t:DisplayName>{name}</t:DisplayName></t:{kind}></t:Create>"#
    )
}

fn element(folder: &str) -> &'static str {
    if folder.starts_with("CA") || folder.starts_with("BD") { "CalendarItem" } else { "Contact" }
}

impl Exchange {
    fn with(mut self, id: &str, key: &str, folder: &str, text: &str) -> Self {
        self.items.push(Item { id: id.into(), key: key.into(), folder: folder.into(), text: text.into() });
        self
    }

    fn asked(&self, op: &str) -> Vec<&str> {
        self.asked.iter().filter(|(o, _)| o == op).map(|(_, b)| b.as_str()).collect()
    }

    fn answer(&mut self, op: &str, body: &str) -> String {
        match op {
            "SyncFolderHierarchy" => {
                let f = [
                    folder("Folder", "IN", "IPF.Note", "Bandeja de entrada"),
                    folder("CalendarFolder", "CA/1", "IPF.Appointment", "Calendario"),
                    folder("CalendarFolder", "BD", "IPF.Appointment.Birthday", "Cumpleaños"),
                    folder("ContactsFolder", "CO", "IPF.Contact", "Contactos"),
                    folder("ContactsFolder", "RC", "IPF.Contact.RecipientCache", "Recipient Cache"),
                ];
                ok(
                    "SyncFolderHierarchy",
                    &format!(
                        "<m:SyncState>s</m:SyncState><m:IncludesLastFolderInRange>true</m:IncludesLastFolderInRange><m:Changes>{}</m:Changes>",
                        f.concat()
                    ),
                )
            }
            "FindItem" => {
                let parent = ids(body, "FolderId").first().map(|s| s.to_string()).unwrap_or_default();
                let items: String = self
                    .items
                    .iter()
                    .filter(|i| i.folder == parent)
                    .map(|i| format!(r#"<t:{0}><t:ItemId Id="{1}" ChangeKey="{2}"/></t:{0}>"#, element(&i.folder), i.id, i.key))
                    .collect();
                ok("FindItem", &format!(r#"<m:RootFolder IncludesLastItemInRange="true"><t:Items>{items}</t:Items></m:RootFolder>"#))
            }
            "GetItem" => {
                let id = ids(body, "ItemId").first().copied().unwrap_or("");
                let Some(i) = self.items.iter().find(|i| i.id == id && !self.vanished.contains(&id)) else { return not_found("GetItem") };
                let mime = if body.contains("IncludeMimeContent>true<") {
                    format!(r#"<t:MimeContent CharacterSet="UTF-8">{}</t:MimeContent>"#, ibercorreo_mime::base64(i.text.as_bytes()))
                } else {
                    String::new()
                };
                ok(
                    "GetItem",
                    &format!(r#"<m:Items><t:{0}>{mime}<t:ItemId Id="{1}" ChangeKey="{2}"/></t:{0}></m:Items>"#, element(&i.folder), i.id, i.key),
                )
            }
            "CreateItem" => {
                let folder = ids(body, "FolderId").first().map(|s| s.to_string()).unwrap_or_default();
                let mime = body.split("CharacterSet=\"UTF-8\">").nth(1).and_then(|r| r.split('<').next()).unwrap_or("");
                let text = String::from_utf8(ibercorreo_mime::unbase64(mime).unwrap_or_default()).unwrap_or_default();
                self.made += 1;
                let id = format!("NEW+{}", self.made);
                let kind = element(&folder);
                self.items.push(Item { id: id.clone(), key: "n1".into(), folder, text });
                ok("CreateItem", &format!(r#"<m:Items><t:{kind}><t:ItemId Id="{id}" ChangeKey="n1"/></t:{kind}></m:Items>"#))
            }
            "DeleteItem" => {
                let id = ids(body, "ItemId").first().copied().unwrap_or("").to_string();
                let before = self.items.len();
                self.items.retain(|i| i.id != id);
                if self.items.len() == before { not_found("DeleteItem") } else { ok("DeleteItem", "") }
            }
            _ => String::new(),
        }
    }
}

impl Http for Exchange {
    fn send(&mut self, _method: &str, _path: &str, _headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
        let body = String::from_utf8_lossy(body).into_owned();
        let after = body.split("Body>").nth(1).unwrap_or("");
        let op: String = after.trim_start_matches('<').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        self.asked.push((op.clone(), body.clone()));
        if let Some((_, status)) = self.fail.filter(|(o, _)| *o == op) {
            return Ok(Response { status, headers: Vec::new(), body: Vec::new(), close: false });
        }
        let reply = envelope(&self.answer(&op, &body));
        Ok(Response { status: 200, headers: Vec::new(), body: reply.into_bytes(), close: false })
    }
}

fn ews(x: Exchange) -> Ews<Exchange> {
    Ews::new(x, &Url::parse(EWS).unwrap())
}

fn ics(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:Microsoft Exchange Server 2016\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\nDTSTART:20261102T080000Z\r\nDTEND:20261102T090000Z\r\nDTSTAMP:20261101T000000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

/// A calendar item as some Exchange versions export it: a message with the calendar as a part.
fn as_message(ics: &str) -> String {
    format!(
        "From: ana@contoso.es\r\nSubject: x\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nx\r\n--b\r\nContent-Type: text/calendar; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--b--\r\n",
        ibercorreo_mime::base64(ics.as_bytes())
    )
}

fn vcard(uid: &str, name: &str) -> String {
    format!("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{name}\r\nN:{name};;;;\r\nEMAIL:{uid}@contoso.es\r\nEND:VCARD\r\n")
}

fn cal() -> String {
    format!("{}CA%2F1/", prefix(EWS))
}

fn state(items: &[(&str, &str)], changes: Vec<Change>) -> State {
    let mut s = State::default();
    s.calendars.insert(cal(), Known { ctag: None, items: items.iter().map(|(h, k)| (format!("{}{h}", cal()), k.to_string())).collect() });
    // A CalDAV calendar in the same store: never sent to Exchange.
    s.calendars.insert("/dav/ana/work/".into(), Known::default());
    s.changes.push(Change::Delete { calendar: "/dav/ana/work/".into(), href: "/dav/ana/work/x.ics".into(), etag: "\"1\"".into() });
    s.changes.extend(changes);
    s
}

#[test]
fn calendars_come_as_icalendar_and_only_what_changed() {
    let x = Exchange::default()
        .with("E1", "k1", "CA/1", &ics("e1", "Reunión"))
        .with("E2", "k2", "CA/1", &as_message(&ics("e2", "Comida")))
        .with("E3", "k3", "CA/1", &ics("e3", "Igual"))
        .with("B1", "b1", "BD", &ics("b1", "Cumpleaños de Luis"));
    let mut e = ews(x);
    let batch = calendar(&mut e, EWS, &state(&[("E3", "k3"), ("GONE", "k9")], Vec::new())).unwrap();
    assert_eq!(batch.scope.as_deref(), Some("/ews/mail.contoso.es%2Fews%2Fexchange.asmx/"));
    assert!(batch.errors.is_empty() && batch.pushed.is_empty(), "{batch:?}");
    let [c] = batch.calendars.as_slice() else { panic!("only the calendar of exactly IPF.Appointment: {batch:?}") };
    assert_eq!((c.calendar.href.as_str(), c.calendar.name.as_str()), (cal().as_str(), "Calendario"));
    let fetched: Vec<(&str, &str)> = c.fetched.iter().map(|f| (f.href.as_str(), f.etag.as_str())).collect();
    assert_eq!(fetched, [(format!("{}E1", cal()).as_str(), "k1"), (format!("{}E2", cal()).as_str(), "k2")], "E3 is unchanged");
    assert!(c.fetched[0].data.starts_with("BEGIN:VCALENDAR") && c.fetched[0].data.contains("SUMMARY:Reunión"));
    assert!(c.fetched[1].data.contains("SUMMARY:Comida"), "the calendar part of a message: {}", c.fetched[1].data);
    assert_eq!(c.removed, [format!("{}GONE", cal())]);
    assert!(!e.http().asked.iter().any(|(_, b)| b.contains("/dav/")), "the CalDAV change stays for CalDAV");

    // Into the store as CalDAV events are.
    let mut store = ibercorreo_store::Store::new();
    let report = crate::calendar::apply(&mut store, batch);
    assert_eq!((report.calendars_added, report.added), (1, 2), "{report:?}");
}

#[test]
fn changes_replace_the_item_only_over_the_version_last_seen() {
    let x = Exchange::default()
        .with("E1", "k1", "CA/1", &ics("e1", "Reunión"))
        .with("E2", "k2-new", "CA/1", &ics("e2", "Comida"))
        .with("E3", "k3", "CA/1", &ics("e3", "Borrar"))
        .with("E5", "k5-new", "CA/1", &ics("e5", "Movida"));
    let c = cal();
    let changes = vec![
        Change::Put { calendar: c.clone(), href: format!("{c}local-1.ics"), etag: None, uid: "local-1".into(), body: ics("local-1", "Nueva") },
        Change::Put { calendar: c.clone(), href: format!("{c}E1"), etag: Some("k1".into()), uid: "e1".into(), body: ics("e1", "Cambiada") },
        // Changed in Outlook meanwhile: Outlook's copy wins.
        Change::Put { calendar: c.clone(), href: format!("{c}E2"), etag: Some("k2".into()), uid: "e2".into(), body: ics("e2", "Mía") },
        Change::Delete { calendar: c.clone(), href: format!("{c}E3"), etag: "k3".into() },
        Change::Delete { calendar: c.clone(), href: format!("{c}E4"), etag: "k4".into() },
        // Changed in Outlook since: not deleted.
        Change::Delete { calendar: c.clone(), href: format!("{c}E5"), etag: "k5".into() },
    ];
    let mut e = ews(x);
    let batch = calendar(&mut e, EWS, &state(&[("E1", "k1"), ("E2", "k2"), ("E3", "k3"), ("E5", "k5")], changes)).unwrap();
    assert_eq!(
        batch.pushed,
        [
            Pushed::Put { calendar: c.clone(), href: format!("{c}NEW%2B1"), uid: "local-1".into() },
            Pushed::Put { calendar: c.clone(), href: format!("{c}NEW%2B2"), uid: "e1".into() },
            Pushed::Conflict { href: format!("{c}E2") },
            Pushed::Deleted { href: format!("{c}E3") },
            Pushed::Deleted { href: format!("{c}E4") },
            Pushed::Conflict { href: format!("{c}E5") },
        ]
    );
    assert!(batch.errors.is_empty(), "{:?}", batch.errors);
    let created = e.http().asked("CreateItem");
    assert_eq!(created.len(), 2);
    assert!(created.iter().all(|b| b.contains(r#"SendMeetingInvitations="SendToNone""#)
        && b.contains(r#"<t:FolderId Id="CA/1"/>"#)
        && b.contains("<t:CalendarItem><t:MimeContent")));
    let deleted = e.http().asked("DeleteItem");
    assert!(
        deleted.iter().all(|b| b.contains(r#"DeleteType="MoveToDeletedItems""#) && b.contains(r#"SendMeetingCancellations="SendToNone""#)),
        "{deleted:?}"
    );
    let on_server: Vec<(&str, &str)> =
        e.http().items.iter().map(|i| (i.id.as_str(), i.text.lines().find(|l| l.starts_with("SUMMARY")).unwrap_or(""))).collect();
    assert_eq!(
        on_server,
        [("E2", "SUMMARY:Comida"), ("E5", "SUMMARY:Movida"), ("NEW+1", "SUMMARY:Nueva"), ("NEW+2", "SUMMARY:Cambiada")],
        "E1 replaced, E3 deleted, E2 and E5 kept as Outlook has them"
    );
    // What was written and the conflict come down again in the same sync; E1's old href goes.
    let [cs] = batch.calendars.as_slice() else { panic!() };
    let fetched: Vec<&str> = cs.fetched.iter().map(|f| f.href.as_str()).collect();
    assert_eq!(fetched, [format!("{c}E2"), format!("{c}E5"), format!("{c}NEW%2B1"), format!("{c}NEW%2B2")]);
    assert!(cs.removed.is_empty(), "{:?}", cs.removed);
}

#[test]
fn contacts_come_as_vcards_and_go_back_the_same_way() {
    let x = Exchange::default().with("C1", "k1", "CO", &vcard("luis", "Luis Pérez")).with("R1", "r1", "RC", &vcard("cache", "Someone"));
    let mut e = ews(x);
    let book = format!("{}CO/", prefix(EWS));
    let mut s = contacts::State::default();
    s.changes.push(contacts::Change::Put {
        book: book.clone(),
        href: format!("{book}marta.vcf"),
        etag: None,
        contact: 7,
        body: vcard("marta", "Marta Gil"),
    });
    let batch = contacts(&mut e, EWS, &s).unwrap();
    assert!(batch.errors.is_empty(), "{:?}", batch.errors);
    assert_eq!(batch.pushed, [contacts::Pushed::Put { href: format!("{book}NEW%2B1"), contact: 7 }]);
    let [b] = batch.books.as_slice() else { panic!("the recipient cache is left out: {batch:?}") };
    assert_eq!(b.book.name, "Contactos");
    let names: Vec<bool> = b.fetched.iter().map(|f| f.data.starts_with("BEGIN:VCARD")).collect();
    assert_eq!(names, [true, true], "Luis, and Marta as Exchange now has her");
    let created = e.http().asked("CreateItem");
    assert!(created[0].contains("<t:Contact><t:MimeContent") && !created[0].contains("SendMeetingInvitations"), "{}", created[0]);
}

#[test]
fn a_refused_sign_in_ends_the_sync_and_odd_items_are_reported() {
    struct Refuse;
    impl Http for Refuse {
        fn send(&mut self, _: &str, _: &str, _: &[(&str, &str)], _: &[u8]) -> Result<Response> {
            Ok(Response { status: 401, headers: Vec::new(), body: Vec::new(), close: false })
        }
    }
    let mut e = Ews::new(Refuse, &Url::parse(EWS).unwrap());
    assert!(matches!(calendar(&mut e, EWS, &State::default()), Err(Error::Auth(401))));
    // Throttled while sending a change: the sync stops, the change goes next time.
    let change = Change::Delete { calendar: cal(), href: format!("{}E1", cal()), etag: "k1".into() };
    let x = Exchange { fail: Some(("GetItem", 503)), ..Exchange::default() };
    assert!(matches!(calendar(&mut ews(x), EWS, &state(&[], vec![change])), Err(Error::Busy { .. })));
    // Listed, then deleted in Outlook before it was read: gone here too, and no error.
    let mut x = Exchange::default().with("E1", "k1", "CA/1", &ics("e1", "x"));
    x.vanished.push("E1");
    let batch = calendar(&mut ews(x), EWS, &state(&[("E1", "k0")], Vec::new())).unwrap();
    assert!(batch.errors.is_empty(), "{:?}", batch.errors);
    assert_eq!(batch.calendars[0].removed, [format!("{}E1", cal())]);
    // An item whose content is no calendar: reported, the rest still come.
    let x = Exchange::default().with("E1", "k1", "CA/1", "not a calendar").with("E2", "k2", "CA/1", &ics("e2", "Bien"));
    let batch = calendar(&mut ews(x), EWS, &State::default()).unwrap();
    assert_eq!(batch.errors.len(), 1, "{:?}", batch.errors);
    assert_eq!(batch.calendars[0].fetched.len(), 1);
    // A change whose href is not inside its calendar is not sent.
    let bad = Change::Put { calendar: cal(), href: "/elsewhere/E1".into(), etag: Some("k1".into()), uid: "e1".into(), body: ics("e1", "x") };
    let mut e = ews(Exchange::default());
    let batch = calendar(&mut e, EWS, &state(&[], vec![bad])).unwrap();
    assert!(batch.pushed.is_empty() && batch.errors.len() == 1, "{batch:?}");
    assert!(e.http().asked("CreateItem").is_empty());
}

#[test]
fn ids_survive_the_trip_through_a_path() {
    for id in ["AAMkAD+/=", "a%b", "ñ", ""] {
        assert_eq!(unsegment(&segment(id)).as_deref(), Some(id));
    }
    assert_eq!(unsegment("%G1"), None);
    assert_eq!(unsegment("%4"), None);
    assert_eq!(folder_id("/ews/x/", "/ews/x/CA%2F1/").as_deref(), Some("CA/1"));
    assert_eq!(folder_id("/ews/x/", "/ews/x/a/b/"), None);
    assert_eq!(item_id("/ews/x/C/", "/ews/x/C/"), None);
    assert!(calendar::is_caldav("/dav/ana/") && !calendar::is_caldav(&cal()));
}
