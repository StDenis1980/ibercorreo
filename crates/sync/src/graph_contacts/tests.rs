use super::*;
use crate::calendar::Known;
use ibercorreo_core::CalendarRemote;
use ibercorreo_dav::Response;

/// Method, path, headers and body of a request.
type Asked = (String, String, Vec<(String, String)>, String);

/// Answers from routes (method, a piece of the path, status, body), the first that matches.
#[derive(Default)]
struct Fake {
    routes: Vec<(&'static str, String, u16, String)>,
    asked: Vec<Asked>,
}

impl Fake {
    fn on(mut self, method: &'static str, path: &str, status: u16, body: &str) -> Self {
        self.routes.push((method, path.to_string(), status, body.to_string()));
        self
    }
}

impl Http for Fake {
    fn send(&mut self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
        let headers = headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
        self.asked.push((method.to_string(), path.to_string(), headers, String::from_utf8_lossy(body).into_owned()));
        let found = self.routes.iter().find(|(m, p, _, _)| *m == method && path.contains(p.as_str()));
        let (status, body) = found.map_or((404, String::new()), |(_, _, s, b)| (*s, b.clone()));
        Ok(Response { status, headers: Vec::new(), body: body.into_bytes(), close: false })
    }
}

fn ana() -> Value {
    json!({
        "id": "c1", "@odata.etag": "W/\"1\"", "displayName": "Ana Gil",
        "emailAddresses": [{"name": "Ana Gil", "address": "ana@contoso.es"}, {"name": "ana@casa.es", "address": "ana@casa.es"}],
        "mobilePhone": "+34 600 000 001", "businessPhones": ["+34 960 000 002"], "homePhones": [],
        "companyName": "Contoso", "jobTitle": "Gerente", "personalNotes": "Prefiere WhatsApp", "birthday": "1980-05-01T11:59:00Z"
    })
}

#[test]
fn outlook_contacts_read_and_write_back() {
    let c = contact(&ana()).unwrap();
    assert_eq!((c.uid.as_str(), c.name.as_str(), c.organization.as_deref(), c.title.as_deref()), ("c1", "Ana Gil", Some("Contoso"), Some("Gerente")));
    let emails: Vec<(&str, Option<&str>)> = c.emails.iter().map(|a| (a.email.as_str(), a.name.as_deref())).collect();
    assert_eq!(emails, [("ana@contoso.es", Some("Ana Gil")), ("ana@casa.es", None)]);
    let phones: Vec<(&str, &str)> = c.phones.iter().map(|p| (p.kind.as_str(), p.number.as_str())).collect();
    assert_eq!(phones, [("mobile", "+34 600 000 001"), ("work", "+34 960 000 002")]);
    assert_eq!((c.note.as_str(), c.birthday), ("Prefiere WhatsApp", Some(Date::new(1980, 5, 1).unwrap())));
    // As a vCard and back, nothing is lost.
    let card = ibercorreo_vcard::write_card(&c);
    let back = ibercorreo_vcard::parse(&card).unwrap().contacts.into_iter().next().unwrap();
    assert_eq!((back.name, back.emails.len(), back.phones.len(), back.birthday), (c.name.clone(), 2, 2, c.birthday));

    let (v, lost) = body(&c, false);
    assert!(lost.is_empty());
    assert_eq!(v["mobilePhone"], "+34 600 000 001");
    assert_eq!(v["businessPhones"], json!(["+34 960 000 002"]));
    assert_eq!(v["birthday"], "1980-05-01T11:59:00Z");
    assert!(v.get("givenName").is_none(), "a change keeps Outlook's own name parts");
    let (v, _) = body(&c, true);
    assert_eq!((v["givenName"].as_str(), v["surname"].as_str()), (Some("Ana"), Some("Gil")));
    // What Outlook cannot keep is said.
    let mut many = c.clone();
    many.emails = (0..5).map(|i| Address { name: None, email: format!("a{i}@x.es") }).collect();
    many.phones.push(Phone { kind: "mobile".into(), number: "+34 600 000 009".into() });
    let (v, lost) = body(&many, false);
    assert_eq!((v["emailAddresses"].as_array().unwrap().len(), lost.len()), (3, 1));
    assert_eq!(v["homePhones"], json!(["+34 600 000 009"]), "a second mobile is kept as another number");
    // Nothing usable: no contact.
    assert!(contact(&json!({"id": "x"})).is_none());
    assert!(contact(&json!({"displayName": "Sin id"})).is_none());
    assert_eq!(contact(&json!({"id": "y", "emailAddresses": [{"address": "solo@x.es"}]})).unwrap().name, "solo@x.es");
}

const MAIN: &str = "/v1.0/users/u1/contacts/";

fn state(items: &[(&str, &str)], changes: Vec<Change>) -> State {
    let mut s = State::default();
    s.books.insert(MAIN.into(), Known { ctag: None, items: items.iter().map(|(h, e)| (h.to_string(), e.to_string())).collect() });
    s.books.insert("/dav/ana/contacts/".into(), Known::default());
    s.changes.push(Change::Delete { book: "/dav/ana/contacts/".into(), href: "/dav/ana/contacts/x.vcf".into(), etag: "\"1\"".into() });
    s.changes.extend(changes);
    s
}

#[test]
fn a_sync_reads_every_folder_and_only_what_changed() {
    let main = json!({"value": [ana(), {"id": "c/2", "@odata.etag": "W/\"2\"", "displayName": "Rui Costa"}]});
    let fake = Fake::default()
        .on("GET", "/v1.0/me?$select=id", 200, r#"{"id":"u1"}"#)
        .on("GET", "/v1.0/users/u1/contactFolders?", 200, r#"{"value":[{"id":"F1","displayName":"Clientes"}]}"#)
        .on("GET", "/v1.0/users/u1/contacts/?", 200, &main.to_string())
        .on("GET", "/v1.0/users/u1/contactFolders/F1/contacts/?", 200, r#"{"value":[]}"#);
    let mut g = Graph::new(fake).without_sleeping();
    let s = state(&[(&format!("{MAIN}c1"), "W/\"1\""), (&format!("{MAIN}gone"), "W/\"3\"")], Vec::new());
    let batch = run(&mut g, &s).unwrap();
    assert_eq!(batch.scope.as_deref(), Some("/v1.0/users/u1/"));
    let names: Vec<(&str, &str)> = batch.books.iter().map(|b| (b.book.href.as_str(), b.book.name.as_str())).collect();
    assert_eq!(names, [(MAIN, "Contacts"), ("/v1.0/users/u1/contactFolders/F1/contacts/", "Clientes")]);
    let main = &batch.books[0];
    assert_eq!(main.fetched.iter().map(|f| f.href.as_str()).collect::<Vec<_>>(), [format!("{MAIN}c%2F2")]);
    assert!(main.fetched[0].data.contains("FN:Rui Costa"));
    assert_eq!(main.removed, [format!("{MAIN}gone")]);
    assert!(!g.http().asked.iter().any(|a| a.1.contains("/dav/")), "the CardDAV change is not Graph's");
}

#[test]
fn changes_go_back_as_post_patch_and_delete() {
    let mut new = contact(&ana()).unwrap();
    new.uid = "local".into();
    let card = ibercorreo_vcard::write_card(&new);
    let changes = vec![
        Change::Put { book: MAIN.into(), href: format!("{MAIN}local.vcf"), etag: None, contact: 7, body: card.clone() },
        Change::Put { book: MAIN.into(), href: format!("{MAIN}c1"), etag: Some("W/\"1\"".into()), contact: 8, body: card.clone() },
        Change::Put { book: MAIN.into(), href: format!("{MAIN}c2"), etag: Some("W/\"2\"".into()), contact: 9, body: card },
        Change::Delete { book: MAIN.into(), href: format!("{MAIN}c3"), etag: "W/\"3\"".into() },
    ];
    let fake = Fake::default()
        .on("GET", "/v1.0/me?$select=id", 200, r#"{"id":"u1"}"#)
        .on("POST", MAIN, 201, r#"{"id":"NEW=","@odata.etag":"W/\"5\""}"#)
        .on("PATCH", "/contacts/c1", 200, "{}")
        .on("PATCH", "/contacts/c2", 412, "{}")
        .on("DELETE", "/contacts/c3", 204, "")
        .on("GET", "/contactFolders?", 200, r#"{"value":[]}"#)
        .on("GET", "/v1.0/users/u1/contacts/?", 200, r#"{"value":[]}"#);
    let mut g = Graph::new(fake).without_sleeping();
    let batch = run(&mut g, &state(&[], changes)).unwrap();
    assert_eq!(
        batch.pushed,
        [
            Pushed::Put { href: format!("{MAIN}NEW="), contact: 7 },
            Pushed::Put { href: format!("{MAIN}c1"), contact: 8 },
            Pushed::Conflict { href: format!("{MAIN}c2") },
            Pushed::Deleted { href: format!("{MAIN}c3") },
        ]
    );
    let asked = &g.http().asked;
    let post = asked.iter().find(|a| a.0 == "POST").unwrap();
    assert_eq!(post.1, MAIN);
    assert!(post.3.contains("\"givenName\":\"Ana\""), "{}", post.3);
    let patch = asked.iter().find(|a| a.0 == "PATCH" && a.1.ends_with("/c1")).unwrap();
    assert!(patch.2.iter().any(|(n, v)| n == "If-Match" && v == "W/\"1\""));
    assert!(asked.iter().find(|a| a.0 == "DELETE").unwrap().2.iter().any(|(n, v)| n == "If-Match" && v == "W/\"3\""));
}

#[test]
fn graph_and_carddav_books_never_remove_each_other() {
    let mut store = ibercorreo_store::Store::new();
    let carddav = store.add_book("Familia", Some(CalendarRemote { href: "/dav/ana/contacts/".into(), ctag: None })).unwrap();
    let mine = store.add_book("Contacts", Some(CalendarRemote { href: MAIN.into(), ctag: None })).unwrap();
    let other = store.add_book("Otro", Some(CalendarRemote { href: "/v1.0/users/u2/contacts/".into(), ctag: None })).unwrap();
    let report = crate::contacts::apply(&mut store, Batch { scope: Some("/v1.0/users/u1/".into()), ..Batch::default() });
    assert_eq!(report.books_removed, 1);
    let ids: Vec<u32> = store.books().iter().map(|b| b.id).collect();
    assert!(ids.contains(&carddav) && ids.contains(&other) && !ids.contains(&mine), "{ids:?}");
    assert_eq!(crate::contacts::apply(&mut store, Batch::default()).books_removed, 1, "CardDAV: only its own");
    assert_eq!(crate::contacts::forget(&mut store).books_removed, 0);
    assert!(store.books().iter().any(|b| b.id == other));
}
