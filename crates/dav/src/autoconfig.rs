//! Mail account autoconfiguration: where to look for an address's server settings and how to read
//! what is found there.
//!
//! The format is Mozilla's `clientConfig` (the one Thunderbird reads, documented at
//! <https://wiki.mozilla.org/Thunderbird:Autoconfiguration:ConfigFileFormat>). It is looked for,
//! over HTTPS only, in order:
//! 1. at the provider: `https://autoconfig.<domain>/mail/config-v1.1.xml`;
//! 2. at the domain: `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`;
//! 3. in the public database of providers: `https://autoconfig.thunderbird.net/v1.1/<domain>`
//!    (which learns only the domain, never the address).
//!
//! Only IMAP and SMTP over TLS or STARTTLS are taken: a plain-text server is never offered.

use crate::xml::{self, Element};
use crate::{Error, Result};

/// Largest configuration file read: 256 KiB (real ones are a few KiB).
pub const MAX_CONFIG_BYTES: usize = 256 * 1024;
/// Most servers of each kind read from one file.
const MAX_SERVERS: usize = 8;

/// How the connection is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Socket {
    Tls,
    StartTls,
}

/// One server a configuration names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub socket: Socket,
    /// The sign-in name, with the address put in (`%EMAILADDRESS%` and the like).
    pub username: String,
    /// The provider signs in with OAuth2 only: a password will be refused (or needs an app
    /// password).
    pub oauth_only: bool,
}

/// What a configuration file says about one address.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// The provider's name, for showing ("Google Mail").
    pub provider: String,
    /// IMAP servers, best first.
    pub imap: Vec<Server>,
    /// SMTP submission servers, best first.
    pub smtp: Vec<Server>,
}

/// The local part and the domain of `email`, lowercased domain; `None` for anything that is not
/// one plain address.
pub fn split(email: &str) -> Option<(&str, String)> {
    let email = email.trim();
    let (local, domain) = email.rsplit_once('@')?;
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    let host_ok = !domain.is_empty()
        && domain.len() <= 253
        && domain.contains('.')
        && domain.split('.').all(|l| !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    let local_ok = !local.is_empty() && local.len() <= 64 && !local.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control() || b == b'@');
    (host_ok && local_ok).then_some((local, domain))
}

/// Where to look for `email`'s settings, in order (see the module notes). Empty for an address
/// that is not one.
pub fn urls(email: &str) -> Vec<String> {
    let Some((_, domain)) = split(email) else { return Vec::new() };
    let address: String = email
        .trim()
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b".-_".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") })
        .collect();
    vec![
        format!("https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress={address}"),
        format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
        format!("https://autoconfig.thunderbird.net/v1.1/{domain}"),
    ]
}

/// A host name: letters, digits, dots and hyphens.
fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && !h.starts_with(['.', '-']) && h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

/// Put `email`'s parts into a template (`%EMAILADDRESS%`, `%EMAILLOCALPART%`, `%EMAILDOMAIN%`).
fn fill(template: &str, email: &str, local: &str, domain: &str) -> String {
    template.trim().replace("%EMAILADDRESS%", email).replace("%EMAILLOCALPART%", local).replace("%EMAILDOMAIN%", domain)
}

fn server(e: &Element, email: &str, local: &str, domain: &str) -> Option<Server> {
    let host = fill(e.child_text("", "hostname")?, email, local, domain).to_ascii_lowercase();
    if !host_ok(&host) {
        return None;
    }
    let port: u16 = e.child_text("", "port")?.parse().ok().filter(|p| *p > 0)?;
    let socket = match e.child_text("", "socketType")?.to_ascii_uppercase().as_str() {
        "SSL" | "TLS" => Socket::Tls,
        "STARTTLS" => Socket::StartTls,
        // Plain text: never.
        _ => return None,
    };
    let username = e.child_text("", "username").map_or_else(|| email.to_string(), |u| fill(u, email, local, domain));
    if username.len() > 320 || username.chars().any(char::is_control) {
        return None;
    }
    let methods: Vec<String> = e.children_named("", "authentication").map(|a| a.text.trim().to_ascii_lowercase()).collect();
    let password = |m: &String| matches!(m.as_str(), "password-cleartext" | "plain" | "password-encrypted" | "secure");
    let oauth_only = !methods.is_empty() && methods.iter().any(|m| m == "oauth2") && !methods.iter().any(password);
    Some(Server { host, port, socket, username, oauth_only })
}

/// Read a configuration file for `email`. A file that names no usable IMAP or SMTP server is an
/// error, so the next place is tried.
pub fn parse(body: &[u8], email: &str) -> Result<Settings> {
    if body.len() > MAX_CONFIG_BYTES {
        return Err(Error::TooLarge { what: "the configuration file", limit: MAX_CONFIG_BYTES });
    }
    let (local, domain) = split(email).ok_or_else(|| Error::Invalid(format!("{email:?} is not an email address")))?;
    let email = email.trim();
    let root = xml::parse(body)?;
    if root.name != "clientConfig" {
        return Err(Error::Protocol("not a mail configuration file".into()));
    }
    let provider = root.child("", "emailProvider").ok_or_else(|| Error::Protocol("the configuration names no provider".into()))?;
    let mut out = Settings {
        provider: provider
            .child_text("", "displayName")
            .or_else(|| provider.child_text("", "displayShortName"))
            .unwrap_or(&domain)
            .chars()
            .filter(|c| !c.is_control())
            .take(100)
            .collect(),
        ..Settings::default()
    };
    for e in provider.children.iter().filter(|e| e.ns.is_empty()) {
        let kind = e.attr("type").unwrap_or("").to_ascii_lowercase();
        let list = match (e.name.as_str(), kind.as_str()) {
            ("incomingServer", "imap") => &mut out.imap,
            ("outgoingServer", "smtp") => &mut out.smtp,
            _ => continue,
        };
        if list.len() < MAX_SERVERS
            && let Some(s) = server(e, email, local, &domain)
            && !list.contains(&s)
        {
            list.push(s);
        }
    }
    if out.imap.is_empty() || out.smtp.is_empty() {
        return Err(Error::Protocol("the configuration names no secure IMAP and SMTP servers".into()));
    }
    Ok(out)
}

/// The settings to try when no configuration is published: the usual names under the domain, TLS
/// before STARTTLS, the whole address as the sign-in name.
pub fn guesses(email: &str) -> Settings {
    let Some((_, domain)) = split(email) else { return Settings::default() };
    let s = |host: String, port, socket| Server { host, port, socket, username: email.trim().to_string(), oauth_only: false };
    Settings {
        provider: domain.clone(),
        imap: vec![
            s(format!("imap.{domain}"), 993, Socket::Tls),
            s(format!("mail.{domain}"), 993, Socket::Tls),
            s(format!("imap.{domain}"), 143, Socket::StartTls),
            s(format!("mail.{domain}"), 143, Socket::StartTls),
        ],
        smtp: vec![
            s(format!("smtp.{domain}"), 465, Socket::Tls),
            s(format!("smtp.{domain}"), 587, Socket::StartTls),
            s(format!("mail.{domain}"), 465, Socket::Tls),
            s(format!("mail.{domain}"), 587, Socket::StartTls),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the public database serves (an invented provider).
    const CONFIG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="example.pt">
    <domain>example.pt</domain>
    <displayName>Exemplo Correio</displayName>
    <incomingServer type="pop3"><hostname>pop.example.pt</hostname><port>995</port><socketType>SSL</socketType></incomingServer>
    <incomingServer type="imap">
      <hostname>imap.%EMAILDOMAIN%</hostname><port>993</port><socketType>SSL</socketType>
      <username>%EMAILLOCALPART%</username><authentication>password-cleartext</authentication>
    </incomingServer>
    <incomingServer type="imap"><hostname>imap.example.pt</hostname><port>143</port><socketType>plain</socketType></incomingServer>
    <incomingServer type="imap"><hostname>evil host</hostname><port>993</port><socketType>SSL</socketType></incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.example.pt</hostname><port>587</port><socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username><authentication>OAuth2</authentication>
    </outgoingServer>
    <outgoingServer type="smtp"><hostname>smtp.example.pt</hostname><port>99999</port><socketType>SSL</socketType></outgoingServer>
  </emailProvider>
</clientConfig>"#;

    #[test]
    fn reads_the_secure_servers_and_fills_in_the_address() {
        let s = parse(CONFIG.as_bytes(), " Ana.Souza@Example.PT ").unwrap();
        assert_eq!(s.provider, "Exemplo Correio");
        assert_eq!(
            s.imap,
            [Server { host: "imap.example.pt".into(), port: 993, socket: Socket::Tls, username: "Ana.Souza".into(), oauth_only: false }],
            "POP, plain text and bad host names are left out"
        );
        assert_eq!(
            s.smtp,
            [Server {
                host: "smtp.example.pt".into(),
                port: 587,
                socket: Socket::StartTls,
                username: "Ana.Souza@Example.PT".into(),
                oauth_only: true
            }]
        );
    }

    #[test]
    fn files_without_secure_servers_or_of_another_kind_are_errors() {
        let plain = CONFIG.replace("<socketType>SSL</socketType>", "<socketType>plain</socketType>");
        assert!(parse(plain.as_bytes(), "a@example.pt").is_err());
        for bad in ["", "<html/>", "<clientConfig/>", "<!DOCTYPE x><clientConfig/>", "<clientConfig><emailProvider>"] {
            assert!(parse(bad.as_bytes(), "a@example.pt").is_err(), "{bad}");
        }
        assert!(parse(CONFIG.as_bytes(), "not an address").is_err());
        assert!(parse(&vec![b' '; MAX_CONFIG_BYTES + 1], "a@example.pt").is_err());
    }

    /// Real files from the public database, when a folder of them is given:
    /// `IBERCORREO_TEST_ISPDB_DIR=dir`, each file named `<domain>.xml` (never committed).
    #[test]
    fn real_database_files_when_configured() {
        let Ok(dir) = std::env::var("IBERCORREO_TEST_ISPDB_DIR") else { return };
        let mut read = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let Some(domain) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".xml")) else { continue };
            let s = parse(&std::fs::read(&path).unwrap(), &format!("ana@{domain}")).unwrap_or_else(|e| panic!("{domain}: {e}"));
            println!("{domain}: {} imap {:?} smtp {:?}", s.provider, s.imap.first(), s.smtp.first());
            read += 1;
        }
        assert!(read > 0);
    }

    #[test]
    fn where_to_look_and_what_to_guess() {
        assert_eq!(
            urls("ana+x@Example.org"),
            [
                "https://autoconfig.example.org/mail/config-v1.1.xml?emailaddress=ana%2Bx%40Example.org",
                "https://example.org/.well-known/autoconfig/mail/config-v1.1.xml",
                "https://autoconfig.thunderbird.net/v1.1/example.org",
            ]
        );
        for bad in ["", "ana", "@example.org", "ana@", "ana@localhost", "ana@exa mple.org", "ana@a/b.org", "ana@x..org", "a b@example.org"] {
            assert!(urls(bad).is_empty(), "{bad}");
        }
        let g = guesses("ana@example.org");
        assert_eq!(g.imap.first().map(|s| (s.host.as_str(), s.port)), Some(("imap.example.org", 993)));
        assert_eq!(g.smtp.first().map(|s| (s.host.as_str(), s.port)), Some(("smtp.example.org", 465)));
        assert!(guesses("nobody").imap.is_empty());
    }
}
