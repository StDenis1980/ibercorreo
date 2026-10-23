//! CardDAV (RFC 6352): finding a user's address books, listing the cards in one with their
//! ETags, and fetching and writing cards.
//!
//! Discovery follows the same path as CalDAV (RFC 6764 §6), so one address serves both: the
//! server (through `/.well-known/carddav`), a principal, an address-book home, one address book,
//! or even a calendar on the same server (its `current-user-principal` leads to the home). A
//! server that names no address-book home simply has none.

use crate::caldav::{FetchedItem, Http, RemoteItem, Written, multistatus, put_as};
use crate::xml::{self, CALSERVER, CARDDAV, DAV, DavResponse};
use crate::{Error, Result, Url};

/// Most address books read from one account.
pub const MAX_BOOKS: usize = 100;
/// Most cards read from one address book.
pub const MAX_CARDS: usize = 100_000;
/// Cards fetched per `addressbook-multiget` request.
const MULTIGET_CHUNK: usize = 100;

/// An address book on the server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteBook {
    /// Its path on the server (percent-encoded, ending in `/`).
    pub href: String,
    pub name: String,
    /// Changes whenever anything in the book changes (`getctag`, else `sync-token`).
    pub ctag: Option<String>,
}

const BOOK_PROPS: &str = "<d:resourcetype/><d:displayname/><d:current-user-principal/><r:addressbook-home-set/><cs:getctag/><d:sync-token/>";

fn propfind_body(props: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<d:propfind xmlns:d=\"{DAV}\" xmlns:r=\"{CARDDAV}\" xmlns:cs=\"{CALSERVER}\"><d:prop>{props}</d:prop></d:propfind>"
    )
}

fn book_from(r: &DavResponse, href: String) -> RemoteBook {
    let name = r
        .prop_text(DAV, "displayname")
        .map(str::to_string)
        .unwrap_or_else(|| href.trim_end_matches('/').rsplit('/').next().unwrap_or("Contacts").to_string());
    let name: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
    let ctag = r.prop_text(CALSERVER, "getctag").or_else(|| r.prop_text(DAV, "sync-token")).map(str::to_string);
    RemoteBook { href, name, ctag }
}

/// The address books at `url`. Empty when the server keeps none for this user.
pub fn discover(http: &mut dyn Http, url: &Url) -> Result<Vec<RemoteBook>> {
    let body = propfind_body(BOOK_PROPS);
    let (path, responses) = if url.path == "/" {
        match multistatus(http, url, "PROPFIND", "/.well-known/carddav", "0", &body) {
            Ok(found) => found,
            Err(Error::Status { .. }) => multistatus(http, url, "PROPFIND", "/", "0", &body)?,
            Err(e) => return Err(e),
        }
    } else {
        multistatus(http, url, "PROPFIND", &url.path, "0", &body)?
    };
    let me = responses.first().cloned().unwrap_or_default();
    if me.has_type(CARDDAV, "addressbook") {
        let href = url.join(&me.href).unwrap_or(path);
        return Ok(vec![book_from(&me, href)]);
    }
    let home = match me.prop_href(CARDDAV, "addressbook-home-set") {
        Some(home) => home.to_string(),
        None => {
            let Some(principal) = me.prop_href(DAV, "current-user-principal") else { return Ok(Vec::new()) };
            let principal = url.join(principal).ok_or_else(|| Error::Invalid("the principal is on another host".into()))?;
            let (_, found) = multistatus(http, url, "PROPFIND", &principal, "0", &propfind_body("<r:addressbook-home-set/>"))?;
            match found.first().and_then(|r| r.prop_href(CARDDAV, "addressbook-home-set")) {
                Some(home) => home.to_string(),
                None => return Ok(Vec::new()),
            }
        }
    };
    let home = url.join(&home).ok_or_else(|| Error::Invalid("the address-book home is on another host".into()))?;
    let (home, found) = multistatus(http, url, "PROPFIND", &home, "1", &body)?;
    let mut out: Vec<RemoteBook> = Vec::new();
    for r in &found {
        let Some(href) = url.join(&r.href) else { continue };
        if href == home || !r.has_type(CARDDAV, "addressbook") || out.iter().any(|b| b.href == href) {
            continue;
        }
        out.push(book_from(r, href));
        if out.len() >= MAX_BOOKS {
            break;
        }
    }
    Ok(out)
}

/// The change marker of one address book (see [`RemoteBook::ctag`]).
pub fn ctag(http: &mut dyn Http, url: &Url, book: &str) -> Result<Option<String>> {
    let (_, found) = multistatus(http, url, "PROPFIND", book, "0", &propfind_body("<cs:getctag/><d:sync-token/>"))?;
    Ok(found.first().and_then(|r| r.prop_text(CALSERVER, "getctag").or_else(|| r.prop_text(DAV, "sync-token")).map(str::to_string)))
}

/// Every card in `book` with its ETag (a `Depth: 1` listing, which every server answers).
pub fn list(http: &mut dyn Http, url: &Url, book: &str) -> Result<Vec<RemoteItem>> {
    let (book, found) = multistatus(http, url, "PROPFIND", book, "1", &propfind_body("<d:resourcetype/><d:getetag/>"))?;
    let mut out = Vec::new();
    for r in &found {
        let (Some(href), Some(etag)) = (url.join(&r.href), r.prop_text(DAV, "getetag")) else { continue };
        if href == book || r.has_type(DAV, "collection") {
            continue;
        }
        if out.len() >= MAX_CARDS {
            return Err(Error::TooLarge { what: "the address book", limit: MAX_CARDS });
        }
        out.push(RemoteItem { href, etag: etag.to_string() });
    }
    Ok(out)
}

/// The cards at `hrefs` in `book`, with their text and ETags. Cards the server no longer has
/// are left out.
pub fn fetch(http: &mut dyn Http, url: &Url, book: &str, hrefs: &[String]) -> Result<Vec<FetchedItem>> {
    let mut out = Vec::new();
    for chunk in hrefs.chunks(MULTIGET_CHUNK) {
        let mut body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<r:addressbook-multiget xmlns:d=\"{DAV}\" xmlns:r=\"{CARDDAV}\"><d:prop><d:getetag/><r:address-data/></d:prop>"
        );
        for href in chunk {
            body.push_str(&format!("<d:href>{}</d:href>", xml::escape(href)));
        }
        body.push_str("</r:addressbook-multiget>");
        let (_, found) = multistatus(http, url, "REPORT", book, "1", &body)?;
        for r in &found {
            let (Some(href), Some(data)) = (url.join(&r.href), r.prop(CARDDAV, "address-data")) else { continue };
            let etag = r.prop_text(DAV, "getetag").unwrap_or("").to_string();
            out.push(FetchedItem { href, etag, data: data.text.clone() });
        }
    }
    Ok(out)
}

/// Write the card `href`: a new one when `etag` is `None`, else only if the server's copy is
/// still that version.
pub fn put(http: &mut dyn Http, href: &str, body: &str, etag: Option<&str>) -> Result<Written> {
    put_as(http, href, body, etag, "text/vcard; charset=utf-8")
}
