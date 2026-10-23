//! CalDAV (RFC 4791): finding a user's calendars, listing the events in one with their ETags,
//! and fetching events.
//!
//! Discovery (RFC 6764 §6): the address the user typed may be the server, a principal, a
//! calendar home or one calendar. We ask it for its type, the principal and the home; follow
//! `current-user-principal` → `calendar-home-set`; then list the home's calendars.

use crate::xml::{self, APPLE_ICAL, CALDAV, CALSERVER, DAV, DavResponse};
use crate::{Error, Response, Result, Url};

/// Anything that can send an HTTP request: [`crate::Client`], or a script in tests.
pub trait Http {
    fn send(&mut self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response>;
}

impl<S: std::io::Read + std::io::Write, C: FnMut() -> std::io::Result<S>> Http for crate::Client<S, C> {
    fn send(&mut self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
        crate::Client::send(self, method, path, headers, body)
    }
}

/// Most calendars read from one account.
pub const MAX_CALENDARS: usize = 200;
/// Most events read from one calendar.
pub const MAX_ITEMS: usize = 50_000;
/// Events fetched per `calendar-multiget` request.
const MULTIGET_CHUNK: usize = 100;
/// Redirects followed for one request.
const MAX_REDIRECTS: usize = 5;

/// A calendar on the server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteCalendar {
    /// Its path on the server (percent-encoded, ending in `/`).
    pub href: String,
    pub name: String,
    /// `#RRGGBB` or `#RRGGBBAA`, when the server keeps one (Apple's `calendar-color`).
    pub color: Option<String>,
    /// Changes whenever anything in the calendar changes (`getctag`, else `sync-token`); when it
    /// is the same as last time there is nothing to fetch.
    pub ctag: Option<String>,
}

/// An event resource and its version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteItem {
    pub href: String,
    pub etag: String,
}

/// An event resource with its iCalendar text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedItem {
    pub href: String,
    pub etag: String,
    pub data: String,
}

const CALENDAR_PROPS: &str = "<d:resourcetype/><d:displayname/><d:current-user-principal/><c:calendar-home-set/>\
<c:supported-calendar-component-set/><cs:getctag/><d:sync-token/><a:calendar-color/>";

fn propfind_body(props: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<d:propfind xmlns:d=\"{DAV}\" xmlns:c=\"{CALDAV}\" xmlns:cs=\"{CALSERVER}\" xmlns:a=\"{APPLE_ICAL}\"><d:prop>{props}</d:prop></d:propfind>"
    )
}

/// Send a request that answers with a multistatus, following same-host redirects. Returns the
/// path finally answered and its responses.
pub(crate) fn multistatus(http: &mut dyn Http, url: &Url, method: &str, path: &str, depth: &str, body: &str) -> Result<(String, Vec<DavResponse>)> {
    let mut path = path.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let headers = [("Depth", depth), ("Content-Type", "application/xml; charset=utf-8")];
        let resp = http.send(method, &path, &headers, body.as_bytes())?;
        match resp.status {
            207 => return Ok((path, xml::multistatus(&resp.body)?)),
            301 | 302 | 303 | 307 | 308 => {
                let location = resp.header("location").ok_or_else(|| Error::Protocol("a redirect without Location".into()))?;
                path = url.join(location).ok_or_else(|| Error::Invalid(format!("the server redirected to another host ({location})")))?;
            }
            401 => return Err(Error::Auth(401)),
            status => return Err(status_error(status, method, &path)),
        }
    }
    Err(Error::Protocol("too many redirects".into()))
}

pub(crate) fn status_error(status: u16, method: &str, path: &str) -> Error {
    Error::Status { status, what: format!("{method} {path}") }
}

/// The calendars at `url` (a server, principal, calendar home or single calendar) that can
/// hold events.
pub fn discover(http: &mut dyn Http, url: &Url) -> Result<Vec<RemoteCalendar>> {
    let body = propfind_body(CALENDAR_PROPS);
    // A bare server address: RFC 6764's well-known path usually redirects to the principal.
    let first = if url.path == "/" {
        match multistatus(http, url, "PROPFIND", "/.well-known/caldav", "0", &body) {
            Ok(found) => found,
            Err(Error::Status { .. }) => multistatus(http, url, "PROPFIND", "/", "0", &body)?,
            Err(e) => return Err(e),
        }
    } else {
        multistatus(http, url, "PROPFIND", &url.path, "0", &body)?
    };
    let (path, responses) = first;
    let me = responses.first().cloned().unwrap_or_default();
    if me.has_type(CALDAV, "calendar") {
        let href = url.join(&me.href).unwrap_or(path);
        return Ok(calendar_from(&me, href).into_iter().collect());
    }
    let home = match me.prop_href(CALDAV, "calendar-home-set") {
        Some(home) => home.to_string(),
        None => match me.prop_href(DAV, "current-user-principal") {
            Some(principal) => {
                let principal = url.join(principal).ok_or_else(|| Error::Invalid("the principal is on another host".into()))?;
                let (_, found) = multistatus(http, url, "PROPFIND", &principal, "0", &propfind_body("<c:calendar-home-set/>"))?;
                found
                    .first()
                    .and_then(|r| r.prop_href(CALDAV, "calendar-home-set"))
                    .map(str::to_string)
                    .ok_or_else(|| Error::Invalid("the server names no calendar home for this user".into()))?
            }
            // No principal either: take the address as the home itself.
            None => path,
        },
    };
    let home = url.join(&home).ok_or_else(|| Error::Invalid("the calendar home is on another host".into()))?;
    let (home, found) = multistatus(http, url, "PROPFIND", &home, "1", &body)?;
    let mut out: Vec<RemoteCalendar> = Vec::new();
    for r in &found {
        let Some(href) = url.join(&r.href) else { continue };
        if href == home || !r.has_type(CALDAV, "calendar") {
            continue;
        }
        if let Some(cal) = calendar_from(r, href)
            && !out.iter().any(|c| c.href == cal.href)
        {
            out.push(cal);
            if out.len() >= MAX_CALENDARS {
                break;
            }
        }
    }
    Ok(out)
}

/// The calendar `r` describes, unless it only holds tasks or journals.
fn calendar_from(r: &DavResponse, href: String) -> Option<RemoteCalendar> {
    if let Some(set) = r.prop(CALDAV, "supported-calendar-component-set") {
        let comps: Vec<&str> = set.children_named(CALDAV, "comp").filter_map(|c| c.attr("name")).collect();
        if !comps.is_empty() && !comps.iter().any(|c| c.eq_ignore_ascii_case("VEVENT")) {
            return None;
        }
    }
    let name = r
        .prop_text(DAV, "displayname")
        .map(str::to_string)
        .unwrap_or_else(|| href.trim_end_matches('/').rsplit('/').next().unwrap_or("Calendar").to_string());
    let name: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
    let color = r.prop_text(APPLE_ICAL, "calendar-color").filter(|c| is_color(c)).map(str::to_string);
    let ctag = r.prop_text(CALSERVER, "getctag").or_else(|| r.prop_text(DAV, "sync-token")).map(str::to_string);
    Some(RemoteCalendar { href, name, color, ctag })
}

fn is_color(c: &str) -> bool {
    c.strip_prefix('#').is_some_and(|hex| matches!(hex.len(), 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The change marker of one calendar (see [`RemoteCalendar::ctag`]).
pub fn ctag(http: &mut dyn Http, url: &Url, calendar: &str) -> Result<Option<String>> {
    let (_, found) = multistatus(http, url, "PROPFIND", calendar, "0", &propfind_body("<cs:getctag/><d:sync-token/>"))?;
    Ok(found.first().and_then(|r| r.prop_text(CALSERVER, "getctag").or_else(|| r.prop_text(DAV, "sync-token")).map(str::to_string)))
}

/// Every event in `calendar` with its ETag.
pub fn list(http: &mut dyn Http, url: &Url, calendar: &str) -> Result<Vec<RemoteItem>> {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<c:calendar-query xmlns:d=\"{DAV}\" xmlns:c=\"{CALDAV}\"><d:prop><d:getetag/></d:prop>\
<c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VEVENT\"/></c:comp-filter></c:filter></c:calendar-query>"
    );
    let (calendar, found) = multistatus(http, url, "REPORT", calendar, "1", &body)?;
    let mut out = Vec::new();
    for r in &found {
        let (Some(href), Some(etag)) = (url.join(&r.href), r.prop_text(DAV, "getetag")) else { continue };
        if href == calendar {
            continue;
        }
        if out.len() >= MAX_ITEMS {
            return Err(Error::TooLarge { what: "the calendar", limit: MAX_ITEMS });
        }
        out.push(RemoteItem { href, etag: etag.to_string() });
    }
    Ok(out)
}

/// The events at `hrefs` in `calendar`, with their text and ETags. Items the server no longer
/// has are left out.
pub fn fetch(http: &mut dyn Http, url: &Url, calendar: &str, hrefs: &[String]) -> Result<Vec<FetchedItem>> {
    let mut out = Vec::new();
    for chunk in hrefs.chunks(MULTIGET_CHUNK) {
        let mut body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<c:calendar-multiget xmlns:d=\"{DAV}\" xmlns:c=\"{CALDAV}\"><d:prop><d:getetag/><c:calendar-data/></d:prop>"
        );
        for href in chunk {
            body.push_str(&format!("<d:href>{}</d:href>", xml::escape(href)));
        }
        body.push_str("</c:calendar-multiget>");
        let (_, found) = multistatus(http, url, "REPORT", calendar, "1", &body)?;
        for r in &found {
            let (Some(href), Some(data)) = (url.join(&r.href), r.prop(CALDAV, "calendar-data")) else { continue };
            let etag = r.prop_text(DAV, "getetag").unwrap_or("").to_string();
            out.push(FetchedItem { href, etag, data: data.text.clone() });
        }
    }
    Ok(out)
}

/// What happened to a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    /// Done. The new ETag, when the server says it (servers that change the text on the way in
    /// leave it out, RFC 4791 §5.3.4).
    Done { etag: Option<String> },
    /// The server's copy changed since the ETag given (or, for a new resource, one already
    /// exists): nothing was written.
    Conflict,
    /// The resource is not on the server any more.
    Gone,
}

pub(crate) fn written(resp: &Response, method: &str, href: &str) -> Result<Written> {
    match resp.status {
        200..=299 => Ok(Written::Done { etag: resp.header("etag").map(str::to_string).filter(|e| !e.is_empty() && e.len() <= 512) }),
        412 => Ok(Written::Conflict),
        404 | 410 => Ok(Written::Gone),
        401 => Err(Error::Auth(401)),
        status => Err(status_error(status, method, href)),
    }
}

/// Write the event resource `href`: a new one when `etag` is `None` (refused if it exists), else
/// only if the server's copy is still that version.
pub fn put(http: &mut dyn Http, href: &str, body: &str, etag: Option<&str>) -> Result<Written> {
    put_as(http, href, body, etag, "text/calendar; charset=utf-8")
}

/// [`put`] with another media type (`text/vcard` for CardDAV).
pub(crate) fn put_as(http: &mut dyn Http, href: &str, body: &str, etag: Option<&str>, content_type: &str) -> Result<Written> {
    let precondition = match etag {
        Some(e) => ("If-Match", e),
        None => ("If-None-Match", "*"),
    };
    let headers = [("Content-Type", content_type), precondition];
    let resp = http.send("PUT", href, &headers, body.as_bytes())?;
    written(&resp, "PUT", href)
}

/// Delete the resource `href` (an event, or a contact card) if the server's copy is still version `etag` (any version
/// when `etag` is empty).
pub fn delete(http: &mut dyn Http, href: &str, etag: &str) -> Result<Written> {
    let headers: &[(&str, &str)] = if etag.is_empty() { &[] } else { &[("If-Match", etag)] };
    let resp = http.send("DELETE", href, headers, b"")?;
    written(&resp, "DELETE", href)
}
