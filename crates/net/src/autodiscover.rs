//! Finding an address's Exchange Web Services address with Exchange Autodiscover
//! ([`dav::autodiscover`]): each place in turn, signed in with the account's name and password,
//! following the redirects an answer gives (to `https://` only, at most [`MAX_HOPS`]).
//!
//! The network step is passed in ([`Post`]), so the order and the redirects are tested without a
//! network; [`find`] wires in the real one.

use crate::dav::autodiscover::{self, Answer};

/// Redirects followed, of any kind, as Outlook does.
pub const MAX_HOPS: usize = 10;

/// What [`discover`] needs from the network.
pub trait Post {
    /// `POST` `body` to an `https://` address, signed in: the status, the `Location` header and
    /// the body.
    fn post(&mut self, url: &str, body: &[u8]) -> Result<(u16, Option<String>, Vec<u8>), String>;
}

/// The address an HTTP redirect from `url` goes to, `https://` only.
fn location(url: &str, next: &str) -> Option<String> {
    if next.starts_with('/') && !next.starts_with("//") {
        let origin = url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
        return Some(format!("{origin}{next}"));
    }
    next.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")).then(|| next.to_string())
}

/// `email`'s EWS address. Every place tried that failed is listed in the error.
pub fn discover(net: &mut dyn Post, email: &str) -> Result<String, Vec<String>> {
    let mut tried = Vec::new();
    let mut email = email.trim().to_string();
    let mut queue = autodiscover::urls(&email);
    if queue.is_empty() {
        return Err(vec![format!("{email:?} is not an email address")]);
    }
    let mut hops = 0usize;
    while !queue.is_empty() {
        let url = queue.remove(0);
        let answer = match net.post(&url, &autodiscover::request(&email)) {
            Ok((200, _, body)) => autodiscover::parse(&body).map_err(|e| format!("{url}: {e}")),
            Ok((301 | 302 | 303 | 307 | 308, Some(next), _)) => match location(&url, &next) {
                Some(next) => Ok(Answer::RedirectUrl(next)),
                None => Err(format!("{url}: redirected away from HTTPS")),
            },
            Ok((401, _, _)) => Err(format!("{url}: the server refused the name or password")),
            Ok((status, _, _)) => Err(format!("{url}: HTTP {status}")),
            Err(e) => Err(format!("{url}: {e}")),
        };
        let next = match answer {
            Ok(Answer::Ews(ews)) => return Ok(ews),
            Ok(other) => other,
            Err(e) => {
                tried.push(e);
                continue;
            }
        };
        hops = hops.saturating_add(1);
        if hops > MAX_HOPS {
            tried.push("too many Autodiscover redirects".into());
            break;
        }
        match next {
            Answer::RedirectUrl(next) => queue = vec![next],
            Answer::RedirectAddr(other) => {
                queue = autodiscover::urls(&other);
                email = other;
            }
            Answer::Ews(_) => {}
        }
    }
    Err(tried)
}

/// The network as [`discover`] uses it: signed in with `login` and `secret`, trusting `tls`.
struct Net<'a> {
    tls: &'a crate::Tls,
    login: &'a str,
    secret: &'a crate::oauth::Secret,
}

impl Post for Net<'_> {
    fn post(&mut self, url: &str, body: &[u8]) -> Result<(u16, Option<String>, Vec<u8>), String> {
        let url = crate::dav::Url::parse(url).map_err(|e| e.to_string())?;
        let mut client = crate::dav_with(&url, self.tls, self.login, self.secret);
        let headers = [("Content-Type", "text/xml; charset=utf-8"), ("Accept", "text/xml")];
        let r = client.send("POST", &url.path, &headers, body).map_err(|e| e.to_string())?;
        if r.body.len() > autodiscover::MAX_ANSWER_BYTES {
            return Err("the Autodiscover answer is too large".into());
        }
        Ok((r.status, r.header("location").map(str::to_string), r.body))
    }
}

/// Find `email`'s EWS address over the network, signed in as `login` with `password`.
pub fn find(email: &str, login: &str, password: &str, tls: &crate::Tls) -> Result<String, Vec<String>> {
    let secret = crate::oauth::Secret::Password(password.to_string());
    discover(&mut Net { tls, login, secret: &secret }, email)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers by address; every request is kept.
    #[derive(Default)]
    struct Fake {
        routes: Vec<(String, u16, Option<String>, String)>,
        asked: Vec<(String, String)>,
    }

    impl Fake {
        fn on(mut self, url: &str, status: u16, location: Option<&str>, body: &str) -> Self {
            self.routes.push((url.into(), status, location.map(str::to_string), body.into()));
            self
        }
    }

    impl Post for Fake {
        fn post(&mut self, url: &str, body: &[u8]) -> Result<(u16, Option<String>, Vec<u8>), String> {
            self.asked.push((url.to_string(), String::from_utf8_lossy(body).into_owned()));
            assert!(self.asked.len() < 100, "asked for ever");
            match self.routes.iter().find(|r| r.0 == url) {
                Some((_, s, l, b)) => Ok((*s, l.clone(), b.clone().into_bytes())),
                None => Err("connection refused".into()),
            }
        }
    }

    fn answer(account: &str) -> String {
        format!(
            r#"<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006"><Response xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a"><Account>{account}</Account></Response></Autodiscover>"#
        )
    }

    fn ews(url: &str) -> String {
        answer(&format!("<Action>settings</Action><Protocol><Type>EXPR</Type><EwsUrl>{url}</EwsUrl></Protocol>"))
    }

    #[test]
    fn the_second_place_answers_after_the_first_fails() {
        let mut f = Fake::default().on("https://contoso.es/autodiscover/autodiscover.xml", 404, None, "").on(
            "https://autodiscover.contoso.es/autodiscover/autodiscover.xml",
            200,
            None,
            &ews("https://mail.contoso.es/EWS/Exchange.asmx"),
        );
        assert_eq!(discover(&mut f, "ana@contoso.es").unwrap(), "https://mail.contoso.es/EWS/Exchange.asmx");
        assert!(f.asked[1].1.contains("<EMailAddress>ana@contoso.es</EMailAddress>"));
    }

    #[test]
    fn redirects_are_followed_to_https_only_and_not_for_ever() {
        // An HTTP redirect, then another address, then the answer.
        let mut f = Fake::default()
            .on("https://contoso.es/autodiscover/autodiscover.xml", 302, Some("/new/autodiscover.xml"), "")
            .on(
                "https://contoso.es/new/autodiscover.xml",
                200,
                None,
                &answer("<Action>redirectAddr</Action><RedirectAddr>ana@corp.contoso.com</RedirectAddr>"),
            )
            .on("https://corp.contoso.com/autodiscover/autodiscover.xml", 200, None, &ews("https://ex.corp.contoso.com/EWS/Exchange.asmx"));
        assert_eq!(discover(&mut f, "ana@contoso.es").unwrap(), "https://ex.corp.contoso.com/EWS/Exchange.asmx");
        assert!(f.asked[2].1.contains("ana@corp.contoso.com"), "asked for the new address");

        let mut f = Fake::default().on("https://contoso.es/autodiscover/autodiscover.xml", 302, Some("http://contoso.es/a.xml"), "");
        let tried = discover(&mut f, "ana@contoso.es").unwrap_err();
        assert!(tried.iter().any(|t| t.contains("away from HTTPS")), "{tried:?}");
        assert!(!f.asked.iter().any(|(u, _)| u.starts_with("http:")), "the password never goes over plain HTTP");

        // A server that sends us round in circles.
        let mut f = Fake::default().on(
            "https://contoso.es/autodiscover/autodiscover.xml",
            200,
            None,
            &answer("<Action>redirectUrl</Action><RedirectUrl>https://contoso.es/autodiscover/autodiscover.xml</RedirectUrl>"),
        );
        let tried = discover(&mut f, "ana@contoso.es").unwrap_err();
        assert!(tried.iter().any(|t| t.contains("too many")), "{tried:?}");
        assert_eq!(f.asked.len(), MAX_HOPS + 1);
    }

    #[test]
    fn every_failure_is_listed() {
        let mut f = Fake::default().on("https://contoso.es/autodiscover/autodiscover.xml", 401, None, "");
        let tried = discover(&mut f, "ana@contoso.es").unwrap_err();
        assert_eq!(tried.len(), 2, "{tried:?}");
        assert!(tried[0].contains("name or password") && tried[1].contains("connection refused"), "{tried:?}");
        assert!(discover(&mut Fake::default(), "nobody").is_err());
    }
}
