//! WebDAV XML: a small element tree read with bounded depth and size, and the `multistatus`
//! reply (RFC 4918 §13) on top of it.

use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

use crate::{Error, Result};

pub const DAV: &str = "DAV:";
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
pub const CALSERVER: &str = "http://calendarserver.org/ns/";
pub const APPLE_ICAL: &str = "http://apple.com/ns/ical/";

/// Deepest nesting read; multistatus replies are about 6 deep.
const MAX_DEPTH: usize = 64;
/// Most elements read from one reply.
const MAX_ELEMENTS: usize = 500_000;

/// One element: its namespace, local name, text (all text children joined) and child elements.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Element {
    pub ns: String,
    pub name: String,
    pub text: String,
    /// Attributes by local name (namespace dropped), as `comp name="VEVENT"` needs.
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
}

impl Element {
    pub fn is(&self, ns: &str, name: &str) -> bool {
        self.ns == ns && self.name == name
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// The first child named `ns`:`name`.
    pub fn child(&self, ns: &str, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.is(ns, name))
    }

    pub fn children_named<'a>(&'a self, ns: &'a str, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.is(ns, name))
    }

    /// The trimmed text of the first `ns`:`name` child that has one.
    pub fn child_text(&self, ns: &str, name: &str) -> Option<&str> {
        self.child(ns, name).map(|c| c.text.trim()).filter(|t| !t.is_empty())
    }
}

/// The document element of `body`.
pub fn parse(body: &[u8]) -> Result<Element> {
    let text = std::str::from_utf8(body).map_err(|_| Error::Protocol("the XML reply is not UTF-8".into()))?;
    let mut reader = NsReader::from_str(text);
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().check_end_names = true;
    let mut stack: Vec<Element> = Vec::new();
    let mut count = 0usize;
    let bad = |e: quick_xml::Error| Error::Protocol(format!("XML: {e}"));
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(bad)?;
        match event {
            Event::Start(start) => {
                count = count.saturating_add(1);
                if stack.len() >= MAX_DEPTH {
                    return Err(Error::TooLarge { what: "the XML nesting", limit: MAX_DEPTH });
                }
                if count > MAX_ELEMENTS {
                    return Err(Error::TooLarge { what: "the XML reply", limit: MAX_ELEMENTS });
                }
                let ns = match ns {
                    ResolveResult::Bound(n) => String::from_utf8_lossy(n.0).into_owned(),
                    _ => String::new(),
                };
                let name = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                let mut attrs = Vec::new();
                for a in start.attributes().flatten().take(32) {
                    let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
                    let value = a.decode_and_unescape_value(reader.decoder()).map(|v| v.into_owned()).unwrap_or_default();
                    attrs.push((key, value));
                }
                stack.push(Element { ns, name, attrs, ..Element::default() });
            }
            Event::End(_) => {
                let Some(done) = stack.pop() else { return Err(Error::Protocol("XML: unbalanced end tag".into())) };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => return Ok(done),
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.decode().map_err(|e| Error::Protocol(format!("XML: {e}")))?);
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.decode().map_err(|e| Error::Protocol(format!("XML: {e}")))?);
                }
            }
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref().map_err(bad)? {
                    Some(c) => c.to_string(),
                    None => {
                        let name = r.decode().map_err(|e| Error::Protocol(format!("XML: {e}")))?;
                        match quick_xml::escape::resolve_predefined_entity(&name) {
                            Some(s) => s.to_string(),
                            None => return Err(Error::Protocol(format!("XML: unknown entity &{name};"))),
                        }
                    }
                };
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&resolved);
                }
            }
            Event::DocType(_) => return Err(Error::Protocol("XML: a DOCTYPE is not allowed in a WebDAV reply".into())),
            Event::Eof => return Err(Error::Protocol("XML: the reply ended before its document element closed".into())),
            _ => {}
        }
    }
}

/// One `response` of a multistatus: the resource and its properties.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DavResponse {
    /// The `href`, as sent (percent-encoded).
    pub href: String,
    /// The status of the whole response, when it has one instead of propstats (a deleted item).
    pub status: Option<u16>,
    /// The properties found (from 2xx propstats only).
    pub props: Vec<Element>,
}

impl DavResponse {
    pub fn prop(&self, ns: &str, name: &str) -> Option<&Element> {
        self.props.iter().find(|p| p.is(ns, name))
    }

    pub fn prop_text(&self, ns: &str, name: &str) -> Option<&str> {
        self.prop(ns, name).map(|p| p.text.trim()).filter(|t| !t.is_empty())
    }

    /// The href inside property `ns`:`name` (`current-user-principal`, `calendar-home-set`).
    pub fn prop_href(&self, ns: &str, name: &str) -> Option<&str> {
        self.prop(ns, name)?.child_text(DAV, "href")
    }

    /// The resource is a collection whose `resourcetype` includes `ns`:`name`.
    pub fn has_type(&self, ns: &str, name: &str) -> bool {
        self.prop(DAV, "resourcetype").is_some_and(|t| t.child(ns, name).is_some())
    }
}

/// The `response` elements of a `multistatus` reply.
pub fn multistatus(body: &[u8]) -> Result<Vec<DavResponse>> {
    let root = parse(body)?;
    if !root.is(DAV, "multistatus") {
        return Err(Error::Protocol(format!("expected a multistatus reply, got <{}>", root.name)));
    }
    let mut out = Vec::new();
    for r in root.children_named(DAV, "response") {
        let Some(href) = r.child_text(DAV, "href") else { continue };
        let mut resp = DavResponse { href: href.to_string(), status: r.child_text(DAV, "status").and_then(status_code), props: Vec::new() };
        for ps in r.children_named(DAV, "propstat") {
            let ok = ps.child_text(DAV, "status").and_then(status_code).is_some_and(|s| (200..300).contains(&s));
            if let (true, Some(prop)) = (ok, ps.child(DAV, "prop")) {
                resp.props.extend(prop.children.iter().cloned());
            }
        }
        out.push(resp);
    }
    Ok(out)
}

/// `HTTP/1.1 404 Not Found` → 404.
pub fn status_code(line: &str) -> Option<u16> {
    line.split_whitespace().nth(1)?.parse().ok()
}

/// `s` with `&`, `<`, `>` and quotes escaped, for building request bodies.
pub fn escape(s: &str) -> String {
    quick_xml::escape::escape(s).into_owned()
}
