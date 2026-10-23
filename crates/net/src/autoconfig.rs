//! Finding an address's mail servers on the network: the published configuration files first
//! ([`dav::autoconfig::urls`]), then the usual server names under the domain, each checked by
//! opening a secure connection and reading the server's greeting.
//!
//! The network steps are passed in ([`Probe`]), so the order and the fallbacks are tested
//! without a network; [`find`] wires in the real ones.

use crate::dav::autoconfig::{self, Server, Settings, Socket};
use crate::{Endpoint, Security, Tls};

/// Redirects followed for one configuration file.
const MAX_REDIRECTS: usize = 3;
/// Connecting to a guessed server gives up sooner than a configured one.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// Where the settings came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A configuration file at this address.
    Published(String),
    /// The usual server names, which answered.
    Guessed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub settings: Settings,
    pub source: Source,
}

/// What [`discover`] needs from the network.
pub trait Probe {
    /// `GET` an `https://` address: the status, the `Location` header and the body.
    fn get(&mut self, url: &str) -> Result<(u16, Option<String>, Vec<u8>), String>;
    /// Whether an IMAP (`smtp` false) or SMTP server answers at `server`.
    fn answers(&mut self, server: &Server, smtp: bool) -> bool;
}

/// One configuration file, following redirects (to `https://` only).
fn fetch(probe: &mut dyn Probe, url: &str) -> Result<Vec<u8>, String> {
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let (status, location, body) = probe.get(&url)?;
        match status {
            200 => return Ok(body),
            301 | 302 | 303 | 307 | 308 => {
                let next = location.ok_or_else(|| format!("{url}: a redirect without a Location"))?;
                url = if next.starts_with('/') {
                    let origin = url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                    format!("{origin}{next}")
                } else if next.to_ascii_lowercase().starts_with("https://") {
                    next
                } else {
                    return Err(format!("{url}: redirected away from HTTPS"));
                };
            }
            s => return Err(format!("{url}: HTTP {s}")),
        }
    }
    Err(format!("{url}: too many redirects"))
}

/// Find `email`'s servers. Every place tried that failed is listed in the error.
pub fn discover(probe: &mut dyn Probe, email: &str) -> Result<Found, Vec<String>> {
    let urls = autoconfig::urls(email);
    if urls.is_empty() {
        return Err(vec![format!("{email:?} is not an email address")]);
    }
    let mut tried = Vec::new();
    for url in urls {
        match fetch(probe, &url).and_then(|body| autoconfig::parse(&body, email).map_err(|e| format!("{url}: {e}"))) {
            Ok(settings) => return Ok(Found { settings, source: Source::Published(url) }),
            Err(e) => tried.push(e),
        }
    }
    let guesses = autoconfig::guesses(email);
    let imap = guesses.imap.iter().find(|s| probe.answers(s, false)).cloned();
    let smtp = guesses.smtp.iter().find(|s| probe.answers(s, true)).cloned();
    match (imap, smtp) {
        (Some(imap), Some(smtp)) => {
            Ok(Found { settings: Settings { provider: guesses.provider, imap: vec![imap], smtp: vec![smtp] }, source: Source::Guessed })
        }
        (imap, _) => {
            tried.push(format!("no {} server answered at the usual names", if imap.is_none() { "IMAP" } else { "SMTP" }));
            Err(tried)
        }
    }
}

/// The network as [`discover`] uses it, trusting `tls`.
pub(crate) struct Net<'a> {
    pub(crate) tls: &'a Tls,
}

impl Probe for Net<'_> {
    fn get(&mut self, url: &str) -> Result<(u16, Option<String>, Vec<u8>), String> {
        let url = crate::dav::Url::parse(url).map_err(|e| e.to_string())?;
        let mut client = crate::dav_anonymous(&url, self.tls);
        let r = client.send("GET", &url.path, &[("Accept", "application/xml, text/xml")], b"").map_err(|e| e.to_string())?;
        if r.body.len() > autoconfig::MAX_CONFIG_BYTES {
            return Err("the configuration file is too large".into());
        }
        Ok((r.status, r.header("location").map(str::to_string), r.body))
    }

    fn answers(&mut self, server: &Server, smtp: bool) -> bool {
        let security = match server.socket {
            Socket::Tls => Security::Tls,
            Socket::StartTls => Security::StartTls,
        };
        let endpoint = Endpoint { host: server.host.clone(), port: server.port, security };
        let Ok(tcp) = crate::tcp_with(&endpoint.host, endpoint.port, PROBE_TIMEOUT, PROBE_TIMEOUT) else { return false };
        if smtp {
            crate::smtp_over(tcp, &endpoint, self.tls, "localhost").map(|c| drop(c.quit())).is_ok()
        } else {
            crate::imap_over(tcp, &endpoint, self.tls).map(|c| drop(c.logout())).is_ok()
        }
    }
}

/// Find `email`'s servers over the network.
pub fn find(email: &str, tls: &Tls) -> Result<Found, Vec<String>> {
    discover(&mut Net { tls }, email)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "<clientConfig><emailProvider id=\"x\"><displayName>X Mail</displayName>\
<incomingServer type=\"imap\"><hostname>imap.x.org</hostname><port>993</port><socketType>SSL</socketType></incomingServer>\
<outgoingServer type=\"smtp\"><hostname>smtp.x.org</hostname><port>465</port><socketType>SSL</socketType></outgoingServer></emailProvider></clientConfig>";

    #[derive(Default)]
    struct Fake {
        /// url → (status, location, body).
        pages: Vec<(&'static str, u16, Option<&'static str>, &'static str)>,
        up: Vec<&'static str>,
        asked: Vec<String>,
    }

    impl Probe for Fake {
        fn get(&mut self, url: &str) -> Result<(u16, Option<String>, Vec<u8>), String> {
            self.asked.push(url.to_string());
            match self.pages.iter().find(|p| p.0 == url) {
                Some((_, s, l, b)) => Ok((*s, l.map(str::to_string), b.as_bytes().to_vec())),
                None => Err(format!("{url}: cannot connect")),
            }
        }
        fn answers(&mut self, server: &Server, smtp: bool) -> bool {
            let key = format!("{}{}:{}", if smtp { "smtp " } else { "imap " }, server.host, server.port);
            self.asked.push(key.clone());
            self.up.contains(&key.as_str())
        }
    }

    #[test]
    fn the_providers_own_file_comes_first_and_redirects_are_followed() {
        let mut f = Fake {
            pages: vec![
                ("https://autoconfig.x.org/mail/config-v1.1.xml?emailaddress=ana%40x.org", 301, Some("/config.xml"), ""),
                ("https://autoconfig.x.org/config.xml", 200, None, CONFIG),
            ],
            ..Fake::default()
        };
        let found = discover(&mut f, "ana@x.org").unwrap();
        assert_eq!(found.source, Source::Published("https://autoconfig.x.org/mail/config-v1.1.xml?emailaddress=ana%40x.org".into()));
        assert_eq!(found.settings.imap[0].host, "imap.x.org");
        assert_eq!(f.asked.len(), 2);
    }

    #[test]
    fn then_the_domain_then_the_public_database_then_guesses() {
        let mut f = Fake { pages: vec![("https://autoconfig.thunderbird.net/v1.1/x.org", 200, None, CONFIG)], ..Fake::default() };
        assert_eq!(discover(&mut f, "ana@x.org").unwrap().source, Source::Published("https://autoconfig.thunderbird.net/v1.1/x.org".into()));
        assert_eq!(f.asked.len(), 3);

        // Nothing published: the usual names that answer.
        let mut f = Fake { up: vec!["imap mail.x.org:993", "smtp smtp.x.org:587"], ..Fake::default() };
        let found = discover(&mut f, "ana@x.org").unwrap();
        assert_eq!(found.source, Source::Guessed);
        assert_eq!((found.settings.imap[0].host.as_str(), found.settings.smtp[0].port), ("mail.x.org", 587));

        let mut f = Fake { up: vec!["imap imap.x.org:993"], ..Fake::default() };
        let err = discover(&mut f, "ana@x.org").unwrap_err();
        assert_eq!(err.len(), 4, "{err:?}");
        assert!(err.last().unwrap().contains("SMTP"));
        assert!(discover(&mut Fake::default(), "nobody").is_err());
    }

    #[test]
    fn redirects_stay_on_https_and_are_bounded() {
        let first = "https://autoconfig.x.org/mail/config-v1.1.xml?emailaddress=ana%40x.org";
        let mut f = Fake { pages: vec![(first, 302, Some("http://autoconfig.x.org/plain.xml"), "")], ..Fake::default() };
        assert!(fetch(&mut f, first).unwrap_err().contains("HTTPS"));
        let mut f = Fake { pages: vec![(first, 302, Some(first), "")], ..Fake::default() };
        assert!(fetch(&mut f, first).unwrap_err().contains("too many"));
        assert_eq!(f.asked.len(), MAX_REDIRECTS + 1);
        // A file that is not a configuration is passed over.
        let mut f = Fake {
            pages: vec![(first, 200, None, "<html>parked domain</html>"), ("https://autoconfig.thunderbird.net/v1.1/x.org", 200, None, CONFIG)],
            ..Fake::default()
        };
        assert_eq!(discover(&mut f, "ana@x.org").unwrap().settings.provider, "X Mail");
    }
}
