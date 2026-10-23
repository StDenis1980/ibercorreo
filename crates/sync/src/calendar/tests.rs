use std::collections::BTreeMap;

use ibercorreo_dav::{Error, Response};

use super::*;

/// Item href → (etag, iCalendar text).
type Items = BTreeMap<String, (String, String)>;

/// A small CalDAV server in memory: a principal, a home and calendars of events.
#[derive(Default)]
struct Server {
    /// Calendar href → (name, ctag, items).
    calendars: BTreeMap<String, (String, String, Items)>,
    /// The requests seen, as `METHOD path` (`REPORT* path` for a multiget).
    log: Vec<String>,
    refuse: bool,
    fail_multiget: bool,
}

fn ok(body: String) -> Response {
    let body = format!(
        r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/">{body}</d:multistatus>"#
    );
    Response { status: 207, headers: Vec::new(), body: body.into_bytes(), close: false }
}

fn status(status: u16) -> Response {
    Response { status, headers: Vec::new(), body: Vec::new(), close: false }
}

const OK: &str = "<d:status>HTTP/1.1 200 OK</d:status>";

impl Http for Server {
    fn send(&mut self, method: &str, path: &str, _headers: &[(&str, &str)], body: &[u8]) -> ibercorreo_dav::Result<Response> {
        let body = String::from_utf8_lossy(body);
        let multiget = body.contains("calendar-multiget");
        self.log.push(format!("{method}{} {path}", if multiget { "*" } else { "" }));
        if self.refuse {
            return Ok(status(401));
        }
        Ok(match (method, path) {
            ("PROPFIND", "/dav/") => ok(format!(
                "<d:response><d:href>/dav/</d:href><d:propstat><d:prop><c:calendar-home-set><d:href>/dav/cals/</d:href></c:calendar-home-set></d:prop>{OK}</d:propstat></d:response>"
            )),
            ("PROPFIND", "/dav/cals/") => {
                let mut out = format!(
                    "<d:response><d:href>/dav/cals/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop>{OK}</d:propstat></d:response>"
                );
                for (href, (name, ctag, _)) in &self.calendars {
                    out.push_str(&format!("<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>{name}</d:displayname><cs:getctag>{ctag}</cs:getctag></d:prop>{OK}</d:propstat></d:response>"));
                }
                ok(out)
            }
            ("REPORT", cal) => {
                let Some((_, _, items)) = self.calendars.get(cal) else { return Ok(status(404)) };
                if multiget && self.fail_multiget {
                    return Ok(status(500));
                }
                let mut out = String::new();
                for (href, (etag, data)) in items {
                    if multiget && !body.contains(&format!("<d:href>{href}</d:href>")) {
                        continue;
                    }
                    let data = if multiget { format!("<c:calendar-data>{data}</c:calendar-data>") } else { String::new() };
                    out.push_str(&format!("<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:getetag>{etag}</d:getetag>{data}</d:prop>{OK}</d:propstat></d:response>"));
                }
                ok(out)
            }
            _ => status(404),
        })
    }
}

fn ics(uid: &str, summary: &str, day: u8) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:202611{day:02}T100000Z\r\nDTEND:202611{day:02}T110000Z\r\nSUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn server() -> Server {
    let mut s = Server::default();
    let mut work = BTreeMap::new();
    work.insert("/dav/cals/work/a.ics".to_string(), ("\"1\"".to_string(), ics("a", "Planning", 2)));
    // A series and one changed occurrence in one resource.
    let series = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:b\r\nDTSTART:20261103T090000Z\r\nDTEND:20261103T093000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Stand-up\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b\r\nRECURRENCE-ID:20261104T090000Z\r\nDTSTART:20261104T100000Z\r\nDTEND:20261104T103000Z\r\nSUMMARY:Stand-up (late)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    work.insert("/dav/cals/work/b.ics".to_string(), ("\"1\"".to_string(), series.to_string()));
    s.calendars.insert("/dav/cals/work/".into(), ("Work".into(), "c1".into(), work));
    s.calendars.insert("/dav/cals/home/".into(), ("Home".into(), "h1".into(), BTreeMap::new()));
    s
}

fn sync(server: &mut Server, store: &mut Store) -> Report {
    let url = Url::parse("https://cal.example.org/dav/").unwrap();
    let batch = run(server, &url, &state(store)).unwrap();
    apply(store, batch)
}

fn summaries(store: &Store, href: &str) -> Vec<String> {
    let id = store.calendar_by_href(href).unwrap();
    let mut v: Vec<String> = store.events().iter().filter(|e| e.calendar == id).map(|e| e.summary.clone()).collect();
    v.sort();
    v
}

#[test]
fn first_sync_mirrors_the_server_calendars() {
    let (mut server, mut store) = (server(), Store::new());
    let r = sync(&mut server, &mut store);
    assert_eq!((r.calendars_added, r.added, r.updated, r.removed), (2, 2, 0, 0), "{r:?}");
    assert_eq!(store.calendars().len(), 3, "the local calendar stays");
    assert_eq!(summaries(&store, "/dav/cals/work/"), ["Planning", "Stand-up", "Stand-up (late)"]);
    let work = store.calendar(store.calendar_by_href("/dav/cals/work/").unwrap()).unwrap();
    assert_eq!((work.name.as_str(), work.remote.as_ref().unwrap().ctag.as_deref()), ("Work", Some("c1")));
    let a = store.events().iter().find(|e| e.uid == "a").unwrap();
    assert_eq!(a.remote, Some(EventRemote { href: "/dav/cals/work/a.ics".into(), etag: "\"1\"".into() }));
}

#[test]
fn later_syncs_fetch_only_what_changed() {
    let (mut server, mut store) = (server(), Store::new());
    sync(&mut server, &mut store);
    let planning_id = store.events().iter().find(|e| e.uid == "a").unwrap().id;

    // Nothing changed: the ctags match, so no calendar is even listed.
    server.log.clear();
    let r = sync(&mut server, &mut store);
    assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));
    assert_eq!(server.log, ["PROPFIND /dav/", "PROPFIND /dav/cals/"]);

    // One event changed, one removed, one added.
    let (_, ctag, items) = server.calendars.get_mut("/dav/cals/work/").unwrap();
    *ctag = "c2".into();
    items.insert("/dav/cals/work/a.ics".into(), ("\"2\"".into(), ics("a", "Planning (moved)", 5)));
    items.remove("/dav/cals/work/b.ics");
    items.insert("/dav/cals/work/c.ics".into(), ("\"1\"".into(), ics("c", "Review", 6)));
    server.log.clear();
    let r = sync(&mut server, &mut store);
    assert_eq!((r.added, r.updated, r.removed), (1, 1, 1), "{r:?}");
    assert_eq!(summaries(&store, "/dav/cals/work/"), ["Planning (moved)", "Review"]);
    assert_eq!(store.events().iter().find(|e| e.uid == "a").unwrap().id, planning_id, "an updated event keeps its id");
    assert_eq!(server.log, ["PROPFIND /dav/", "PROPFIND /dav/cals/", "REPORT /dav/cals/work/", "REPORT* /dav/cals/work/"]);
}

#[test]
fn a_failed_fetch_is_retried_and_calendars_gone_from_the_server_go() {
    let (mut server, mut store) = (server(), Store::new());
    server.fail_multiget = true;
    let r = sync(&mut server, &mut store);
    assert_eq!(r.added, 0);
    assert_eq!(r.errors.len(), 1, "{r:?}");
    let work = store.calendar_by_href("/dav/cals/work/").unwrap();
    assert_eq!(store.calendar(work).unwrap().remote.as_ref().unwrap().ctag, None, "the marker is not kept, so the next sync lists again");

    server.fail_multiget = false;
    let r = sync(&mut server, &mut store);
    assert_eq!(r.added, 2, "{r:?}");

    server.calendars.remove("/dav/cals/work/");
    let r = sync(&mut server, &mut store);
    assert_eq!(r.calendars_removed, 1);
    assert!(store.calendar_by_href("/dav/cals/work/").is_none());
    assert!(store.events().iter().all(|e| e.uid != "a"), "its events go with it");
}

#[test]
fn a_refused_sign_in_changes_nothing() {
    let (mut server, mut store) = (server(), Store::new());
    sync(&mut server, &mut store);
    let before = store.clone();
    server.refuse = true;
    let url = Url::parse("https://cal.example.org/dav/").unwrap();
    assert!(matches!(run(&mut server, &url, &state(&store)), Err(Error::Auth(401))));
    assert_eq!(store, before);
}

#[test]
fn bad_calendar_data_is_reported_and_forgetting_empties_the_mirror() {
    let (mut server, mut store) = (server(), Store::new());
    server.calendars.get_mut("/dav/cals/home/").unwrap().2.insert("/dav/cals/home/x.ics".into(), ("\"1\"".into(), "not a calendar".into()));
    let r = sync(&mut server, &mut store);
    assert_eq!(r.errors.len(), 1, "{r:?}");
    assert_eq!(r.added, 2);

    let r = forget(&mut store);
    assert_eq!(r.calendars_removed, 2);
    assert_eq!(store.calendars().len(), 1);
    assert!(store.events().iter().all(|e| e.remote.is_none()));
}
