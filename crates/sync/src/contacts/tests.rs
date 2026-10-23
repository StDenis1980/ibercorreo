use std::collections::BTreeMap;

use ibercorreo_dav::{Error, Response};

use super::*;

/// Card href → (etag, vCard text).
type Cards = BTreeMap<String, (String, String)>;

/// A small CardDAV server in memory: a principal, a home and address books of cards.
#[derive(Default)]
struct Server {
    /// Book href → (name, ctag, cards).
    books: BTreeMap<String, (String, String, Cards)>,
    /// The requests seen, as `METHOD path` (`REPORT* path` for a multiget).
    log: Vec<String>,
    refuse: bool,
    offline_writes: bool,
    next_etag: u32,
}

impl Server {
    /// Change the card `href`, as the phone would.
    fn write(&mut self, href: &str, data: Option<String>) {
        self.next_etag += 1;
        let etag = format!("\"s{}\"", self.next_etag);
        let Some((_, ctag, cards)) = self.book_of(href) else { return };
        *ctag = format!("{ctag}+");
        match data {
            Some(d) => {
                cards.insert(href.to_string(), (etag, d));
            }
            None => {
                cards.remove(href);
            }
        }
    }

    fn book_of(&mut self, href: &str) -> Option<&mut (String, String, Cards)> {
        self.books.iter_mut().find(|(b, _)| href.starts_with(b.as_str())).map(|(_, v)| v)
    }

    fn body(&self, href: &str) -> Option<&str> {
        self.books.values().find_map(|(_, _, cards)| cards.get(href)).map(|(_, d)| d.as_str())
    }
}

fn ok(body: String) -> Response {
    let body = format!(
        r#"<d:multistatus xmlns:d="DAV:" xmlns:r="urn:ietf:params:xml:ns:carddav" xmlns:cs="http://calendarserver.org/ns/">{body}</d:multistatus>"#
    );
    Response { status: 207, headers: Vec::new(), body: body.into_bytes(), close: false }
}

fn status(status: u16) -> Response {
    Response { status, headers: Vec::new(), body: Vec::new(), close: false }
}

const OK: &str = "<d:status>HTTP/1.1 200 OK</d:status>";

impl Http for Server {
    fn send(&mut self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> ibercorreo_dav::Result<Response> {
        let body = String::from_utf8_lossy(body);
        let header = |n: &str| headers.iter().find(|(h, _)| *h == n).map(|(_, v)| v.to_string());
        let multiget = body.contains("addressbook-multiget");
        self.log.push(format!("{method}{} {path}", if multiget { "*" } else { "" }));
        if self.refuse {
            return Ok(status(401));
        }
        if matches!(method, "PUT" | "DELETE") {
            if self.offline_writes {
                return Err(Error::Io(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "offline")));
            }
            self.next_etag += 1;
            let new_etag = format!("\"s{}\"", self.next_etag);
            let Some((_, ctag, cards)) = self.book_of(path) else { return Ok(status(409)) };
            let current = cards.get(path).map(|(e, _)| e.clone());
            if header("If-None-Match").is_some() && current.is_some() {
                return Ok(status(412));
            }
            if let Some(want) = header("If-Match")
                && current.as_ref() != Some(&want)
            {
                return Ok(status(if current.is_none() { 404 } else { 412 }));
            }
            *ctag = format!("{ctag}+");
            return Ok(if method == "PUT" {
                cards.insert(path.to_string(), (new_etag, body.into_owned()));
                status(201)
            } else if cards.remove(path).is_some() {
                status(204)
            } else {
                status(404)
            });
        }
        Ok(match (method, path) {
            ("PROPFIND", "/dav/") => ok(format!(
                "<d:response><d:href>/dav/</d:href><d:propstat><d:prop><r:addressbook-home-set><d:href>/dav/books/</d:href></r:addressbook-home-set></d:prop>{OK}</d:propstat></d:response>"
            )),
            ("PROPFIND", "/dav/books/") => {
                let mut out = format!(
                    "<d:response><d:href>/dav/books/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop>{OK}</d:propstat></d:response>"
                );
                for (href, (name, ctag, _)) in &self.books {
                    out.push_str(&format!("<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><r:addressbook/></d:resourcetype><d:displayname>{name}</d:displayname><cs:getctag>{ctag}</cs:getctag></d:prop>{OK}</d:propstat></d:response>"));
                }
                ok(out)
            }
            ("PROPFIND", book) => {
                let Some((_, _, cards)) = self.books.get(book) else { return Ok(status(404)) };
                let mut out = format!(
                    "<d:response><d:href>{book}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><r:addressbook/></d:resourcetype></d:prop>{OK}</d:propstat></d:response>"
                );
                for (href, (etag, _)) in cards {
                    out.push_str(&format!("<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype/><d:getetag>{etag}</d:getetag></d:prop>{OK}</d:propstat></d:response>"));
                }
                ok(out)
            }
            ("REPORT", book) if multiget => {
                let Some((_, _, cards)) = self.books.get(book) else { return Ok(status(404)) };
                let mut out = String::new();
                for (href, (etag, data)) in cards {
                    if body.contains(&format!("<d:href>{href}</d:href>")) {
                        out.push_str(&format!("<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:getetag>{etag}</d:getetag><r:address-data>{data}</r:address-data></d:prop>{OK}</d:propstat></d:response>"));
                    }
                }
                ok(out)
            }
            _ => status(404),
        })
    }
}

fn vcf(uid: &str, name: &str, phone: &str) -> String {
    format!("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{name}\r\nTEL;TYPE=CELL:{phone}\r\nEND:VCARD\r\n")
}

fn server() -> Server {
    let mut s = Server::default();
    let mut cards = BTreeMap::new();
    let ana = vcf("ana", "Ana Souza", "+351 912 345 678").replace("END:VCARD", "ADR;TYPE=WORK:;;Rua das Flores 12;Lisboa;;;Portugal\r\nEND:VCARD");
    cards.insert("/dav/books/default/ana.vcf".to_string(), ("\"1\"".to_string(), ana));
    cards.insert("/dav/books/default/rui.vcf".to_string(), ("\"1\"".to_string(), vcf("rui", "Rui Costa", "+351 913 000 000")));
    s.books.insert("/dav/books/default/".into(), ("Phone".into(), "b1".into(), cards));
    s
}

fn sync(server: &mut Server, store: &mut Store) -> Report {
    let url = Url::parse("https://dav.example.org/dav/").unwrap();
    let batch = run(server, &url, &state(store)).unwrap();
    apply(store, batch)
}

fn names(store: &Store) -> Vec<String> {
    let id = store.book_by_href("/dav/books/default/").unwrap();
    let mut v: Vec<String> = store.all_contacts().iter().filter(|c| c.book == id).map(|c| c.name.clone()).collect();
    v.sort();
    v
}

fn contact_id(store: &Store, uid: &str) -> u64 {
    store.all_contacts().iter().find(|c| c.uid == uid).unwrap().id
}

#[test]
fn first_sync_mirrors_the_server_books_and_later_ones_fetch_only_changes() {
    let (mut server, mut store) = (server(), Store::new());
    let r = sync(&mut server, &mut store);
    assert_eq!((r.books_added, r.added, r.updated, r.removed), (1, 2, 0, 0), "{r:?}");
    assert_eq!(store.books().len(), 2, "the local book stays");
    assert_eq!(names(&store), ["Ana Souza", "Rui Costa"]);
    let ana = store.contact(contact_id(&store, "ana")).unwrap();
    assert_eq!(ana.phones[0].number, "+351 912 345 678");
    assert_eq!(ana.remote.as_ref().map(|r| (r.href.as_str(), r.etag.as_str(), r.dirty)), Some(("/dav/books/default/ana.vcf", "\"1\"", false)));
    let ana_id = ana.id;

    // Nothing changed: the ctags match, so the book is not even listed.
    server.log.clear();
    let r = sync(&mut server, &mut store);
    assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));
    assert_eq!(server.log, ["PROPFIND /dav/", "PROPFIND /dav/books/"]);

    // One card changed on the phone, one deleted, one added.
    server.write("/dav/books/default/ana.vcf", Some(vcf("ana", "Ana Maria Souza", "+351 912 345 678")));
    server.write("/dav/books/default/rui.vcf", None);
    server.write("/dav/books/default/eva.vcf", Some(vcf("eva", "Eva Lima", "+351 914 000 000")));
    server.log.clear();
    let r = sync(&mut server, &mut store);
    assert_eq!((r.added, r.updated, r.removed), (1, 1, 1), "{r:?}");
    assert_eq!(names(&store), ["Ana Maria Souza", "Eva Lima"]);
    assert_eq!(contact_id(&store, "ana"), ana_id, "an updated contact keeps its id");
    assert_eq!(server.log, ["PROPFIND /dav/", "PROPFIND /dav/books/", "PROPFIND /dav/books/default/", "REPORT* /dav/books/default/"]);
}

#[test]
fn contacts_made_here_are_sent_once() {
    let (mut server, mut store) = (server(), Store::new());
    sync(&mut server, &mut store);
    let book = store.book_by_href("/dav/books/default/").unwrap();
    let mut c = Contact::new("Olena Koval");
    c.book = book;
    c.uid = "olena/1@here".into();
    let id = store.add_contact(c);
    server.log.clear();
    let r = sync(&mut server, &mut store);
    assert_eq!((r.sent, r.added), (1, 0), "{r:?}");
    assert!(server.log.contains(&"PUT /dav/books/default/olena_1_here.vcf".to_string()), "{:?}", server.log);
    let body = server.body("/dav/books/default/olena_1_here.vcf").unwrap();
    assert!(body.contains("FN:Olena Koval") && body.contains("UID:olena/1@here"), "{body}");
    let mine: Vec<_> = store.all_contacts().iter().filter(|c| c.uid == "olena/1@here").collect();
    assert_eq!(mine.len(), 1, "linked, not duplicated");
    assert_eq!(mine[0].id, id);
    assert!(mine[0].remote.as_ref().is_some_and(|r| !r.etag.is_empty() && !r.dirty));

    // Nothing more to send; a contact in the local book never goes to the server.
    store.add_contact(Contact::new("Only here"));
    server.log.clear();
    let r = sync(&mut server, &mut store);
    assert_eq!(r.sent, 0);
    assert!(!server.log.iter().any(|l| l.starts_with("PUT")), "{:?}", server.log);
}

#[test]
fn changes_here_go_back_keeping_what_the_app_does_not_read() {
    let (mut server, mut store) = (server(), Store::new());
    sync(&mut server, &mut store);
    let id = contact_id(&store, "ana");
    store.contact_mut(id).unwrap().title = Some("Diretora".into());
    let r = sync(&mut server, &mut store);
    assert_eq!((r.sent, r.conflicts), (1, 0), "{r:?}");
    let body = server.body("/dav/books/default/ana.vcf").unwrap();
    assert!(body.contains("TITLE:Diretora"), "{body}");
    assert!(body.contains("ADR;TYPE=WORK:;;Rua das Flores 12;Lisboa;;;Portugal"), "the postal address goes back: {body}");
    assert!(!store.contact(id).unwrap().remote.as_ref().unwrap().dirty);
}

#[test]
fn deletions_here_are_deleted_on_the_server_and_the_servers_copy_wins_a_conflict() {
    let (mut server, mut store) = (server(), Store::new());
    sync(&mut server, &mut store);
    store.remove_contact(contact_id(&store, "rui")).unwrap();
    assert_eq!(store.card_deletions().len(), 1);
    let r = sync(&mut server, &mut store);
    assert_eq!(r.sent, 1, "{r:?}");
    assert!(server.body("/dav/books/default/rui.vcf").is_none());
    assert!(store.card_deletions().is_empty());

    store.contact_mut(contact_id(&store, "ana")).unwrap().name = "Mine".into();
    server.write("/dav/books/default/ana.vcf", Some(vcf("ana", "Theirs", "1")));
    let r = sync(&mut server, &mut store);
    assert_eq!((r.sent, r.conflicts), (0, 1), "{r:?}");
    assert_eq!(store.contact(contact_id(&store, "ana")).unwrap().name, "Theirs");

    // Deleted here, changed there: it comes back.
    store.remove_contact(contact_id(&store, "ana")).unwrap();
    server.write("/dav/books/default/ana.vcf", Some(vcf("ana", "Theirs again", "1")));
    let r = sync(&mut server, &mut store);
    assert_eq!(r.conflicts, 1, "{r:?}");
    assert_eq!(store.contact(contact_id(&store, "ana")).unwrap().name, "Theirs again");
    assert!(store.card_deletions().is_empty());
}

#[test]
fn offline_or_refused_syncs_change_nothing() {
    let (mut server, mut store) = (server(), Store::new());
    sync(&mut server, &mut store);
    let id = contact_id(&store, "ana");
    store.contact_mut(id).unwrap().note = "Later".into();
    let url = Url::parse("https://dav.example.org/dav/").unwrap();
    server.offline_writes = true;
    assert!(run(&mut server, &url, &state(&store)).is_err());
    server.offline_writes = false;
    server.refuse = true;
    assert!(matches!(run(&mut server, &url, &state(&store)), Err(Error::Auth(401))));
    assert!(store.contact(id).unwrap().remote.as_ref().unwrap().dirty, "still to send");
    server.refuse = false;
    let r = sync(&mut server, &mut store);
    assert_eq!(r.sent, 1);
    assert!(server.body("/dav/books/default/ana.vcf").unwrap().contains("NOTE:Later"));
}

#[test]
fn bad_cards_are_reported_books_gone_from_the_server_go_and_forgetting_empties_the_mirror() {
    let (mut server, mut store) = (server(), Store::new());
    server.books.get_mut("/dav/books/default/").unwrap().2.insert("/dav/books/default/x.vcf".into(), ("\"1\"".into(), "not a card".into()));
    server.books.insert("/dav/books/work/".into(), ("Work".into(), "w1".into(), BTreeMap::new()));
    let r = sync(&mut server, &mut store);
    assert_eq!(r.errors.len(), 1, "{r:?}");
    assert_eq!((r.books_added, r.added), (2, 2));

    server.books.remove("/dav/books/work/");
    let r = sync(&mut server, &mut store);
    assert_eq!(r.books_removed, 1);
    assert!(store.book_by_href("/dav/books/work/").is_none());

    store.add_contact(Contact::new("Only here"));
    let r = forget(&mut store);
    assert_eq!(r.books_removed, 1);
    assert_eq!(store.books().len(), 1);
    assert_eq!(store.all_contacts().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Only here"]);
}

#[test]
fn changes_never_leave_their_book() {
    let (mut server, mut store) = (server(), Store::new());
    sync(&mut server, &mut store);
    let url = Url::parse("https://dav.example.org/dav/").unwrap();
    let mut evil = state(&store);
    evil.changes = vec![
        Change::Delete { book: "/dav/books/default/".into(), href: "/elsewhere/x.vcf".into(), etag: String::new() },
        Change::Put {
            book: "/dav/books/default/".into(),
            href: "/dav/books/default/../../x.vcf".into(),
            etag: None,
            contact: 0,
            body: String::new(),
        },
    ];
    let batch = run(&mut server, &url, &evil).unwrap();
    assert_eq!(batch.errors.len(), 2, "{:?}", batch.errors);
    assert!(!server.log.iter().any(|l| l.starts_with("DELETE") || l.starts_with("PUT")));
}
