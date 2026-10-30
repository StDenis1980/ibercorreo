//! Microsoft Graph (`https://graph.microsoft.com/v1.0/`): the JSON API behind the calendar,
//! contacts and mail of Microsoft 365 and Outlook.com accounts, written from Microsoft's public
//! REST documentation. It replaces EWS, which Exchange Online is switching off (2026–2027).
//!
//! - [`Graph::call`]: one request, its JSON reply, and Graph's error object turned into an
//!   [`Error`]. A throttled request (`429`, `503`, `504`) waits for `Retry-After` and is sent again,
//!   a few times and never for long ([`MAX_WAIT`]); after that it is [`Error::Busy`].
//! - [`Graph::list`]: a collection read page by page (`@odata.nextLink`), with its
//!   `@odata.deltaLink` when it is a delta query. Every page stays on the Graph host.
//! - [`Graph::me`]: who the token belongs to.
//!
//! The access token is a separate one from the mail token (Microsoft issues a token for one
//! resource at a time); `ibercorreo-net` gets it and opens the connection.

use std::time::Duration;

use serde_json::Value;

use crate::caldav::Http;
use crate::{Error, Result, Url};

/// Where every request goes.
pub const BASE: &str = "https://graph.microsoft.com/v1.0/";
/// Most pages read for one list.
pub const MAX_PAGES: usize = 2000;
/// Times a throttled request is sent again.
const MAX_RETRIES: u32 = 3;
/// Longest wait for a throttled request, in seconds; a longer `Retry-After` is [`Error::Busy`].
pub const MAX_WAIT: u64 = 60;
/// Longest error text kept from a reply.
const MAX_DETAIL: usize = 300;

/// Requests to Microsoft Graph over `http` (a [`crate::Client`] signed in with the Graph token).
pub struct Graph<H> {
    http: H,
    base: Url,
    sleep: fn(Duration),
    /// Time spent waiting for throttled requests.
    pub waited: Duration,
}

/// A collection read to its end.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    pub items: Vec<Value>,
    /// Where to ask for what changed since (delta queries only).
    pub delta: Option<String>,
}

/// The signed-in user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    /// The mail address (`mail`, else the sign-in name).
    pub email: String,
}

impl<H: Http> Graph<H> {
    pub fn new(http: H) -> Self {
        // Provably infallible: BASE is a fixed, valid https address.
        let base = Url::parse(BASE).unwrap_or(Url { host: "graph.microsoft.com".into(), port: 443, path: "/v1.0/".into() });
        Graph { http, base, sleep: std::thread::sleep, waited: Duration::ZERO }
    }

    /// The connection the requests go over.
    pub fn http(&self) -> &H {
        &self.http
    }

    /// The same, counting waits in [`Graph::waited`] without sleeping (for tests).
    pub fn without_sleeping(mut self) -> Self {
        self.sleep = |_| {};
        self
    }

    /// The request path for `path`: relative to [`BASE`] (`me/events`), absolute (`/v1.0/me`), or
    /// a full address on the Graph host (a next or delta link). Anything else is refused, so a
    /// reply can never send the token elsewhere.
    pub fn path(&self, path: &str) -> Result<String> {
        self.base
            .join(path)
            .ok_or_else(|| Error::Invalid(format!("{:?} is not a Microsoft Graph address", path.chars().take(200).collect::<String>())))
    }

    /// Send one request with an optional JSON body; the reply's JSON (`None` when it has no
    /// body, as `204 No Content`).
    pub fn call(&mut self, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&Value>) -> Result<Option<Value>> {
        let path = self.path(path)?;
        let body = match body {
            Some(v) => serde_json::to_vec(v).map_err(|e| Error::Invalid(e.to_string()))?,
            None => Vec::new(),
        };
        let mut all: Vec<(&str, &str)> = vec![("Accept", "application/json")];
        if !body.is_empty() {
            all.push(("Content-Type", "application/json"));
        }
        all.extend_from_slice(headers);
        let mut attempt: u32 = 0;
        loop {
            let resp = self.http.send(method, &path, &all, &body)?;
            if resp.is_success() {
                if resp.body.iter().all(u8::is_ascii_whitespace) {
                    return Ok(None);
                }
                return serde_json::from_slice(&resp.body)
                    .map(Some)
                    .map_err(|e| Error::Protocol(format!("Microsoft Graph sent JSON that does not read: {e}")));
            }
            if !matches!(resp.status, 429 | 503 | 504) {
                return Err(error(&resp));
            }
            let retry_after = resp.header("retry-after").and_then(|v| v.trim().parse::<u64>().ok());
            let wait = retry_after.unwrap_or(2u64.saturating_pow(attempt.saturating_add(1)));
            if attempt >= MAX_RETRIES || wait > MAX_WAIT {
                return Err(Error::Busy { retry_after });
            }
            (self.sleep)(Duration::from_secs(wait));
            self.waited = self.waited.saturating_add(Duration::from_secs(wait));
            attempt += 1;
        }
    }

    /// `GET` a single resource.
    pub fn get(&mut self, path: &str) -> Result<Value> {
        self.call("GET", path, &[], None)?.ok_or_else(|| Error::Protocol("Microsoft Graph sent an empty reply".into()))
    }

    /// Read the collection at `path` page by page, keeping at most `max` items (more is
    /// [`Error::TooLarge`]).
    pub fn list(&mut self, path: &str, headers: &[(&str, &str)], max: usize) -> Result<Listing> {
        let mut out = Listing::default();
        let mut next = self.path(path)?;
        for _ in 0..MAX_PAGES {
            let page = self.call("GET", &next, headers, None)?.unwrap_or(Value::Null);
            let Some(items) = page.get("value").and_then(Value::as_array) else {
                return Err(Error::Protocol("a Microsoft Graph list without a \"value\" array".into()));
            };
            if out.items.len().saturating_add(items.len()) > max {
                return Err(Error::TooLarge { what: "the list", limit: max });
            }
            out.items.extend(items.iter().cloned());
            if let Some(delta) = page.get("@odata.deltaLink").and_then(Value::as_str) {
                out.delta = Some(delta.to_string());
            }
            match page.get("@odata.nextLink").and_then(Value::as_str) {
                Some(link) => {
                    let link = self.path(link)?;
                    if link == next {
                        return Err(Error::Protocol("a Microsoft Graph page links to itself".into()));
                    }
                    next = link;
                }
                None => return Ok(out),
            }
        }
        Err(Error::TooLarge { what: "the number of pages", limit: MAX_PAGES })
    }

    /// Who the token belongs to.
    pub fn me(&mut self) -> Result<Profile> {
        let v = self.get("me?$select=displayName,mail,userPrincipalName")?;
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        Ok(Profile { name: text("displayName").unwrap_or_default(), email: text("mail").or_else(|| text("userPrincipalName")).unwrap_or_default() })
    }
}

/// The error a failed reply stands for: `401` is a refused token; otherwise Graph's
/// `{"error":{"code","message"}}`, when it sent one.
fn error(resp: &crate::Response) -> Error {
    if resp.status == 401 {
        return Error::Auth(401);
    }
    let json: Option<Value> = serde_json::from_slice(&resp.body).ok();
    let field =
        |k: &str| json.as_ref().and_then(|j| j.get("error")).and_then(|e| e.get(k)).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let what = match (field("code"), field("message")) {
        (Some(c), Some(m)) => format!("{c}: {m}"),
        (Some(c), None) => c.to_string(),
        (None, Some(m)) => m.to_string(),
        (None, None) => "no details".to_string(),
    };
    Error::Status { status: resp.status, what: what.chars().take(MAX_DETAIL).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Response;
    use std::collections::VecDeque;

    /// Method, path, headers and body of a request.
    type Asked = (String, String, Vec<(String, String)>, String);

    /// Replies in turn, recording each request.
    #[derive(Default)]
    struct Queue {
        replies: VecDeque<Response>,
        asked: Vec<Asked>,
    }

    impl Http for Queue {
        fn send(&mut self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
            let headers = headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
            self.asked.push((method.to_string(), path.to_string(), headers, String::from_utf8_lossy(body).into_owned()));
            self.replies.pop_front().ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "no more")))
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

    fn graph(replies: Vec<Response>) -> Graph<Queue> {
        Graph::new(Queue { replies: replies.into(), asked: Vec::new() }).without_sleeping()
    }

    #[test]
    fn the_profile_and_the_request() {
        let mut g = graph(vec![reply(200, r#"{"displayName":"Ana Gil","mail":null,"userPrincipalName":"ana@contoso.es"}"#, &[])]);
        assert_eq!(g.me().unwrap(), Profile { name: "Ana Gil".into(), email: "ana@contoso.es".into() });
        let (method, path, headers, body) = &g.http.asked[0];
        assert_eq!((method.as_str(), path.as_str(), body.as_str()), ("GET", "/v1.0/me?$select=displayName,mail,userPrincipalName", ""));
        assert_eq!(headers, &[("Accept".to_string(), "application/json".to_string())]);
    }

    #[test]
    fn a_json_body_goes_with_its_type_and_no_content_is_none() {
        let mut g = graph(vec![reply(204, "", &[])]);
        let body = serde_json::json!({"subject": "Reunión"});
        assert_eq!(g.call("PATCH", "me/events/AAk=", &[("If-Match", "W/\"1\"")], Some(&body)).unwrap(), None);
        let (method, path, headers, sent) = &g.http.asked[0];
        assert_eq!((method.as_str(), path.as_str()), ("PATCH", "/v1.0/me/events/AAk="));
        assert!(headers.iter().any(|(n, v)| n == "Content-Type" && v == "application/json"), "{headers:?}");
        assert!(headers.iter().any(|(n, v)| n == "If-Match" && v == "W/\"1\""), "{headers:?}");
        assert_eq!(sent, r#"{"subject":"Reunión"}"#);
    }

    #[test]
    fn errors_carry_graphs_code_and_a_refused_token_is_auth() {
        let mut g = graph(vec![
            reply(401, r#"{"error":{"code":"InvalidAuthenticationToken","message":"Access token has expired."}}"#, &[]),
            reply(403, r#"{"error":{"code":"ErrorAccessDenied","message":"Access is denied."}}"#, &[]),
            reply(404, "<html>not json</html>", &[]),
            reply(200, "{not json", &[]),
        ]);
        assert!(matches!(g.get("me"), Err(Error::Auth(401))));
        match g.get("me/calendars") {
            Err(Error::Status { status: 403, what }) => assert_eq!(what, "ErrorAccessDenied: Access is denied."),
            other => panic!("{other:?}"),
        }
        assert!(matches!(g.get("me/x"), Err(Error::Status { status: 404, .. })));
        assert!(matches!(g.get("me"), Err(Error::Protocol(_))));
    }

    #[test]
    fn throttled_requests_wait_and_are_sent_again() {
        let busy = |after: &str| reply(429, r#"{"error":{"code":"TooManyRequests"}}"#, &[("Retry-After", after)]);
        let mut g = graph(vec![busy("5"), reply(503, "", &[]), reply(200, r#"{"id":"1"}"#, &[])]);
        assert_eq!(g.get("me").unwrap()["id"], "1");
        assert_eq!(g.http.asked.len(), 3, "sent again until it went through");
        assert_eq!(g.waited, Duration::from_secs(5 + 4), "Retry-After, then the back-off");
        // Too long a wait, or still busy after the retries: the caller tries later.
        let mut g = graph(vec![busy("3600")]);
        assert!(matches!(g.get("me"), Err(Error::Busy { retry_after: Some(3600) })));
        assert_eq!(g.waited, Duration::ZERO);
        let mut g = graph(vec![busy("1"), busy("1"), busy("1"), busy("1"), reply(200, "{}", &[])]);
        assert!(matches!(g.get("me"), Err(Error::Busy { retry_after: Some(1) })));
        assert_eq!(g.http.asked.len(), 4);
    }

    #[test]
    fn lists_follow_next_links_on_the_graph_host_only() {
        let mut g = graph(vec![
            reply(
                200,
                r#"{"value":[{"id":"a"},{"id":"b"}],"@odata.nextLink":"https://graph.microsoft.com/v1.0/me/events/delta?$skiptoken=X2"}"#,
                &[],
            ),
            reply(200, r#"{"value":[{"id":"c"}],"@odata.deltaLink":"https://graph.microsoft.com/v1.0/me/events/delta?$deltatoken=D1"}"#, &[]),
        ]);
        let l = g.list("me/events/delta", &[("Prefer", "odata.maxpagesize=2")], 10).unwrap();
        let ids: Vec<&str> = l.items.iter().filter_map(|v| v["id"].as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(l.delta.as_deref(), Some("https://graph.microsoft.com/v1.0/me/events/delta?$deltatoken=D1"));
        assert_eq!(g.http.asked[1].1, "/v1.0/me/events/delta?$skiptoken=X2");
        assert!(g.http.asked[1].2.iter().any(|(n, _)| n == "Prefer"), "every page asks the same way");

        // A link to another host would carry the token there.
        let mut g = graph(vec![reply(200, r#"{"value":[],"@odata.nextLink":"https://evil.example.com/v1.0/me/events"}"#, &[])]);
        assert!(matches!(g.list("me/events", &[], 10), Err(Error::Invalid(_))));
        assert_eq!(g.http.asked.len(), 1);
        assert!(g.path("https://graph.microsoft.com.evil.example/v1.0/").is_err());
        assert!(g.path("http://graph.microsoft.com/v1.0/me").is_ok(), "same host: sent over https anyway");

        // A page that links to itself, too many items, no array.
        let looping = r#"{"value":[{"id":"a"}],"@odata.nextLink":"https://graph.microsoft.com/v1.0/me/events"}"#;
        let mut g = graph(vec![reply(200, looping, &[])]);
        assert!(matches!(g.list("me/events", &[], 10), Err(Error::Protocol(_))));
        let mut g = graph(vec![reply(200, r#"{"value":[1,2,3]}"#, &[])]);
        assert!(matches!(g.list("me/events", &[], 2), Err(Error::TooLarge { limit: 2, .. })));
        let mut g = graph(vec![reply(200, r#"{"value":"no"}"#, &[])]);
        assert!(matches!(g.list("me/events", &[], 2), Err(Error::Protocol(_))));
    }
}
