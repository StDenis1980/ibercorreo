use std::io::{self, Read, Write};

use super::caldav::{self, Http};
use super::*;

/// A stream that replays `reply` and keeps what was written.
struct Script {
    reply: io::Cursor<Vec<u8>>,
    written: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
}

impl Read for Script {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Two bytes at a time, so every reader loop is exercised.
        let n = buf.len().min(2);
        self.reply.read(&mut buf[..n])
    }
}

impl Write for Script {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

type Written = std::rc::Rc<std::cell::RefCell<Vec<u8>>>;
type Opened = std::rc::Rc<std::cell::Cell<usize>>;

/// A client whose connections replay `replies` in turn.
fn client(replies: Vec<&str>) -> (Client<Script, impl FnMut() -> io::Result<Script>>, Written, Opened) {
    let written: Written = Default::default();
    let opened: Opened = Default::default();
    let (w, o) = (written.clone(), opened.clone());
    let mut replies: std::collections::VecDeque<Vec<u8>> = replies.into_iter().map(|r| r.as_bytes().to_vec()).collect();
    let connect = move || {
        o.set(o.get() + 1);
        let reply = replies.pop_front().ok_or_else(|| io::Error::new(io::ErrorKind::ConnectionRefused, "no more"))?;
        Ok(Script { reply: io::Cursor::new(reply), written: w.clone() })
    };
    let url = Url::parse("https://cal.example.org/dav/").unwrap();
    (Client::new(connect, &url, "ana", "secret"), written, opened)
}

#[test]
fn urls_are_https_only_and_hrefs_stay_on_the_host() {
    let u = Url::parse("https://Cal.Example.org:8443/dav/ana/#x").unwrap();
    assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("cal.example.org", 8443, "/dav/ana/"));
    assert_eq!(u.host_header(), "cal.example.org:8443");
    assert_eq!(Url::parse("https://cal.example.org").unwrap().path, "/");
    assert!(Url::parse("http://cal.example.org/").is_err());
    assert!(Url::parse("https://ana:pw@cal.example.org/").is_err());
    assert!(Url::parse("https://cal example.org/").is_err());
    assert!(Url::parse("https://cal.example.org:0/").is_err());
    assert!(Url::parse("ftp://x/").is_err());
    assert_eq!(u.join("/dav/ana/work/").as_deref(), Some("/dav/ana/work/"));
    assert_eq!(u.join("work/").as_deref(), Some("/dav/ana/work/"));
    assert_eq!(u.join("https://cal.example.org:8443/x").as_deref(), Some("/x"));
    assert_eq!(u.join("https://evil.example.com:8443/x"), None);
    assert_eq!(u.join("https://cal.example.org/x"), None, "another port is another server");
    // A path can never break the request line.
    assert_eq!(u.join("/a b\r\nX-Evil: 1").as_deref(), Some("/a%20b%0D%0AX-Evil:%201"));
    assert_eq!(u.join("/календар/").as_deref(), Some("/%D0%BA%D0%B0%D0%BB%D0%B5%D0%BD%D0%B4%D0%B0%D1%80/"));
}

#[test]
fn requests_carry_auth_and_reject_header_injection() {
    let (mut c, written, _) = client(vec!["HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"]);
    let resp = c.send("PROPFIND", "/dav/", &[("Depth", "0")], b"<x/>").unwrap();
    assert_eq!((resp.status, resp.body.as_slice()), (200, &b"ok"[..]));
    let w = String::from_utf8(written.borrow().clone()).unwrap();
    assert!(w.starts_with("PROPFIND /dav/ HTTP/1.1\r\nHost: cal.example.org\r\n"), "{w}");
    assert!(w.contains("Authorization: Basic YW5hOnNlY3JldA==\r\n"), "{w}");
    assert!(w.contains("Depth: 0\r\nContent-Length: 4\r\n\r\n<x/>"), "{w}");
    assert!(c.send("GET", "/", &[("X", "a\r\nEvil: 1")], b"").is_err());
    assert!(c.send("GET", "/a b", &[], b"").is_err());
    assert!(c.send("get /x HTTP/1.1\r\n", "/", &[], b"").is_err());
}

#[test]
fn replies_are_read_by_length_chunks_or_close_and_connections_reused() {
    let replies = "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 207 Multi-Status\r\nTransfer-Encoding: chunked\r\n\r\n3;x=1\r\nabc\r\n2\r\nde\r\n0\r\nX-T: 1\r\n\r\n\
HTTP/1.1 204 No Content\r\nContent-Length: 99\r\n\r\n\
HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nxyz";
    let (mut c, _, opened) = client(vec![replies, "HTTP/1.0 200 OK\r\n\r\nto the end"]);
    assert_eq!(c.send("GET", "/a", &[], b"").unwrap().body, b"abcde");
    assert_eq!(c.send("DELETE", "/b", &[], b"").unwrap().body, b"", "204 has no body whatever it says");
    assert_eq!(c.send("GET", "/c", &[], b"").unwrap().body, b"xyz");
    assert_eq!(opened.get(), 1, "one kept-alive connection");
    // The first connection is now at its end: the client opens a new one and retries.
    let r = c.send("GET", "/d", &[], b"").unwrap();
    assert_eq!((r.body.as_slice(), r.close), (&b"to the end"[..], true));
    assert_eq!(opened.get(), 2);
}

#[test]
fn hostile_replies_are_errors_not_panics() {
    let trailers = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n{}\r\n", "T: 1\r\n".repeat(200));
    for reply in [
        "",
        "garbage\r\n\r\n",
        "HTTP/1.1 999 Odd\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffffff\r\n",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nabcdef\r\n0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4000000\r\n",
        "HTTP/1.1 200 OK\r\nX: unterminated",
        &trailers,
    ] {
        let (mut c, _, _) = client(vec![reply]);
        assert!(c.send("GET", "/", &[], b"").is_err(), "{reply:?}");
    }
    let huge = format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "a".repeat(70 * 1024));
    let (mut c, _, _) = client(vec![&huge]);
    assert!(matches!(c.send("GET", "/", &[], b""), Err(Error::TooLarge { .. })));
}

#[test]
fn xml_is_read_with_namespaces_entities_and_bounds() {
    let doc = br#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:response><D:href>/cal/a%20b.ics</D:href><D:propstat><D:prop><D:getetag>"1"</D:getetag><C:calendar-data><![CDATA[BEGIN:VCALENDAR]]>&#13;&amp;x</C:calendar-data></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
<D:propstat><D:prop><D:displayname/></D:prop><D:status>HTTP/1.1 404 Not Found</D:status></D:propstat></D:response>
<D:response><D:href>/cal/gone.ics</D:href><D:status>HTTP/1.1 404 Not Found</D:status></D:response>
</D:multistatus>"#;
    let rs = xml::multistatus(doc).unwrap();
    assert_eq!(rs.len(), 2);
    assert_eq!(rs[0].href, "/cal/a%20b.ics");
    assert_eq!(rs[0].prop_text(xml::DAV, "getetag"), Some("\"1\""));
    assert_eq!(rs[0].prop(xml::CALDAV, "calendar-data").unwrap().text, "BEGIN:VCALENDAR\r&x");
    assert!(rs[0].prop(xml::DAV, "displayname").is_none(), "404 propstats are left out");
    assert_eq!(rs[1].status, Some(404));

    let deep = format!("{}{}", "<a>".repeat(100), "</a>".repeat(100));
    assert!(matches!(xml::parse(deep.as_bytes()), Err(Error::TooLarge { .. })));
    assert!(xml::parse(b"<!DOCTYPE x [<!ENTITY a \"aaaa\">]><x>&a;</x>").is_err(), "no entity expansion");
    assert!(xml::parse(b"<x>&nope;</x>").is_err());
    assert!(xml::parse(b"<a><b></a>").is_err());
    assert!(xml::parse(b"<a>").is_err());
    assert!(xml::parse(&[0xff, 0xfe]).is_err());
    assert!(xml::multistatus(b"<html/>").is_err());
}

/// Answers each request from a table of (method, path) → reply, recording what was asked.
struct Fake {
    routes: Vec<(&'static str, &'static str, Response)>,
    asked: Vec<(String, String, String)>,
}

impl Http for Fake {
    fn send(&mut self, method: &str, path: &str, _headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
        self.asked.push((method.to_string(), path.to_string(), String::from_utf8_lossy(body).into_owned()));
        let found = self.routes.iter().find(|(m, p, _)| *m == method && *p == path);
        Ok(found.map_or_else(|| reply(404, "", &[]), |(_, _, r)| r.clone()))
    }
}

fn reply(status: u16, body: &str, headers: &[(&str, &str)]) -> Response {
    Response {
        status,
        headers: headers.iter().map(|(n, v)| (n.to_ascii_lowercase(), v.to_string())).collect(),
        body: body.as_bytes().to_vec(),
        close: false,
    }
}

fn ms(responses: &str) -> Response {
    reply(
        207,
        &format!(
            r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:a="http://apple.com/ns/ical/">{responses}</d:multistatus>"#
        ),
        &[],
    )
}

const OK: &str = "<d:status>HTTP/1.1 200 OK</d:status>";

#[test]
fn discovery_follows_well_known_principal_and_home() {
    let mut fake = Fake {
        routes: vec![
            ("PROPFIND", "/.well-known/caldav", reply(301, "", &[("Location", "https://cal.example.org/dav/")])),
            (
                "PROPFIND",
                "/dav/",
                ms(&format!(
                    "<d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/dav/ana/</d:href></d:current-user-principal></d:prop>{OK}</d:propstat></d:response>"
                )),
            ),
            (
                "PROPFIND",
                "/dav/ana/",
                ms(&format!(
                    "<d:response><d:href>/dav/ana/</d:href><d:propstat><d:prop><c:calendar-home-set><d:href>https://cal.example.org/dav/ana/cals/</d:href></c:calendar-home-set></d:prop>{OK}</d:propstat></d:response>"
                )),
            ),
            (
                "PROPFIND",
                "/dav/ana/cals/",
                ms(&format!(
                    "<d:response><d:href>/dav/ana/cals/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop>{OK}</d:propstat></d:response>\
<d:response><d:href>/dav/ana/cals/work/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>Work</d:displayname><a:calendar-color>#1F7A8CFF</a:calendar-color><cs:getctag>c1</cs:getctag>\
<c:supported-calendar-component-set><c:comp name=\"VEVENT\"/><c:comp name=\"VTODO\"/></c:supported-calendar-component-set></d:prop>{OK}</d:propstat></d:response>\
<d:response><d:href>/dav/ana/cals/tasks/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><c:supported-calendar-component-set><c:comp name=\"VTODO\"/></c:supported-calendar-component-set></d:prop>{OK}</d:propstat></d:response>\
<d:response><d:href>/dav/ana/cals/home/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><a:calendar-color>javascript:x</a:calendar-color><d:sync-token>s9</d:sync-token></d:prop>{OK}</d:propstat></d:response>\
<d:response><d:href>https://evil.example.com/x/</d:href><d:propstat><d:prop><d:resourcetype><c:calendar/></d:resourcetype></d:prop>{OK}</d:propstat></d:response>"
                )),
            ),
        ],
        asked: Vec::new(),
    };
    let url = Url::parse("https://cal.example.org").unwrap();
    let cals = caldav::discover(&mut fake, &url).unwrap();
    assert_eq!(
        cals,
        vec![
            caldav::RemoteCalendar {
                href: "/dav/ana/cals/work/".into(),
                name: "Work".into(),
                color: Some("#1F7A8CFF".into()),
                ctag: Some("c1".into())
            },
            caldav::RemoteCalendar { href: "/dav/ana/cals/home/".into(), name: "home".into(), color: None, ctag: Some("s9".into()) },
        ],
        "task-only calendars, bad colours and other hosts are left out"
    );
    let paths: Vec<&str> = fake.asked.iter().map(|(_, p, _)| p.as_str()).collect();
    assert_eq!(paths, ["/.well-known/caldav", "/dav/", "/dav/ana/", "/dav/ana/cals/"]);
}

#[test]
fn discovery_of_a_single_calendar_and_errors() {
    let mut fake = Fake {
        routes: vec![(
            "PROPFIND",
            "/cal/work/",
            ms(&format!(
                "<d:response><d:href>/cal/work/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>Work</d:displayname></d:prop>{OK}</d:propstat></d:response>"
            )),
        )],
        asked: Vec::new(),
    };
    let cals = caldav::discover(&mut fake, &Url::parse("https://h/cal/work/").unwrap()).unwrap();
    assert_eq!(cals.len(), 1);
    assert_eq!(cals[0].href, "/cal/work/");

    let mut denied = Fake { routes: vec![("PROPFIND", "/x/", reply(401, "", &[]))], asked: Vec::new() };
    assert!(matches!(caldav::discover(&mut denied, &Url::parse("https://h/x/").unwrap()), Err(Error::Auth(401))));
    let mut away = Fake { routes: vec![("PROPFIND", "/x/", reply(302, "", &[("Location", "https://elsewhere/x/")]))], asked: Vec::new() };
    assert!(matches!(caldav::discover(&mut away, &Url::parse("https://h/x/").unwrap()), Err(Error::Invalid(_))));
    let mut lp = Fake { routes: vec![("PROPFIND", "/x/", reply(302, "", &[("Location", "/x/")]))], asked: Vec::new() };
    assert!(caldav::discover(&mut lp, &Url::parse("https://h/x/").unwrap()).is_err());
    assert!(lp.asked.len() <= 6, "redirect loops are bounded");
}

#[test]
fn listing_and_fetching_events() {
    let mut fake = Fake {
        routes: vec![(
            "REPORT",
            "/cal/work/",
            ms(&format!(
                "<d:response><d:href>/cal/work/</d:href><d:propstat><d:prop><d:getetag>\"c\"</d:getetag></d:prop>{OK}</d:propstat></d:response>\
<d:response><d:href>/cal/work/a.ics</d:href><d:propstat><d:prop><d:getetag>\"1\"</d:getetag><c:calendar-data>BEGIN:VCALENDAR&#13;\nEND:VCALENDAR</c:calendar-data></d:prop>{OK}</d:propstat></d:response>\
<d:response><d:href>/cal/work/noetag.ics</d:href><d:propstat><d:prop></d:prop>{OK}</d:propstat></d:response>"
            )),
        )],
        asked: Vec::new(),
    };
    let url = Url::parse("https://h/cal/work/").unwrap();
    let items = caldav::list(&mut fake, &url, "/cal/work/").unwrap();
    assert_eq!(items, vec![caldav::RemoteItem { href: "/cal/work/a.ics".into(), etag: "\"1\"".into() }]);
    assert!(fake.asked[0].2.contains("<c:comp-filter name=\"VEVENT\"/>"));

    let hrefs: Vec<String> = (0..150).map(|i| format!("/cal/work/{i}&.ics")).collect();
    let got = caldav::fetch(&mut fake, &url, "/cal/work/", &hrefs).unwrap();
    assert_eq!(fake.asked.len(), 3, "150 items take two multigets");
    assert!(fake.asked[1].2.contains("<d:href>/cal/work/0&amp;.ics</d:href>"), "hrefs are escaped");
    assert_eq!(got[0].data, "BEGIN:VCALENDAR\r\nEND:VCALENDAR");
}
