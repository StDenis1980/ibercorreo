//! Accounts from GNOME Settings (GNOME Online Accounts, GOA).
//!
//! GOA keeps the accounts a person added under Settings → Online Accounts and hands them to apps
//! over D-Bus (`org.gnome.OnlineAccounts` on the session bus). Account settings lists them and
//! fills a Tinta Correo account from one: the mail servers (`Mail`), and the CalDAV/CardDAV address
//! (`Calendar`, `Contacts`). The account then remembers only the GOA id: its password or token is
//! asked of GOA at each sign-in ([`GoaSecret`]) and never kept by Correo, so a password changed or
//! an account signed in again in Settings is used at once, and removing it there stops Correo's
//! access too.
//!
//! What the providers give (checked against goa-daemon 3.50):
//! - Google: `Mail` (imap.gmail.com, smtp.gmail.com), `Calendar`, `Contacts`; an OAuth 2.0 access
//!   token (`OAuth2Based.GetAccessToken`) that signs in to IMAP and SMTP (XOAUTH2) and CalDAV.
//! - IMAP and SMTP: `Mail`; the two passwords (`PasswordBased.GetPassword("imap-password")` and
//!   `"smtp-password"`).
//! - Nextcloud, WebDAV: `Calendar`, `Contacts` (no mail); the password (`GetPassword`) with the
//!   account's own name (`Identity`).
//! - Microsoft 365 (`ms_graph`): files only in GNOME 46; whatever of `Mail`, `Calendar` and
//!   `Contacts` a newer GNOME adds is taken the same way, with its OAuth 2.0 token.
//!
//! Linux and the BSDs only; elsewhere there is nothing to list.

use std::collections::BTreeMap;

use ibercorreo_net::oauth::{Error, Secret, SecretSource};
use ibercorreo_ui_egui::{GnomeAccount, GnomeMail};

/// A D-Bus property value, as far as GOA's accounts need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prop {
    Str(String),
    Bool(bool),
}

/// An account's interfaces (the part after `org.gnome.OnlineAccounts.`) and their properties.
pub type Interfaces = BTreeMap<String, BTreeMap<String, Prop>>;

/// The longest GOA id taken (theirs are `account_<seconds>_<n>`).
const MAX_ID: usize = 128;

/// Whether `id` is an account id GOA could have made: also the last part of its object path.
pub fn plausible_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// What Account settings shows of a GOA account; `None` for one Correo can't use (no mail, no
/// calendars or contacts, or nothing but unencrypted mail).
pub fn account_from(ifaces: &Interfaces) -> Option<GnomeAccount> {
    let account = ifaces.get("Account")?;
    let text = |props: &BTreeMap<String, Prop>, key: &str| match props.get(key) {
        Some(Prop::Str(s)) => s.trim().to_string(),
        _ => String::new(),
    };
    let flag = |props: &BTreeMap<String, Prop>, key: &str| matches!(props.get(key), Some(Prop::Bool(true)));
    let id = text(account, "Id");
    if !plausible_id(&id) {
        return None;
    }
    let mail = ifaces.get("Mail").filter(|_| !flag(account, "MailDisabled")).and_then(|m| {
        let email = text(m, "EmailAddress");
        let imap_host = text(m, "ImapHost");
        let smtp_host = text(m, "SmtpHost");
        if !email.contains('@') || imap_host.is_empty() || smtp_host.is_empty() {
            return None;
        }
        // Correo signs in over TLS only: GOA's "no encryption" is left out.
        let imap_starttls = match (flag(m, "ImapUseSsl"), flag(m, "ImapUseTls")) {
            (true, _) => false,
            (false, true) => true,
            (false, false) => return None,
        };
        let smtp_starttls = match (flag(m, "SmtpUseSsl"), flag(m, "SmtpUseTls")) {
            (true, _) => false,
            (false, true) => true,
            (false, false) => return None,
        };
        let user = text(m, "ImapUserName");
        Some(GnomeMail {
            name: text(m, "Name"),
            user: if user.eq_ignore_ascii_case(&email) { String::new() } else { user },
            email,
            imap_host,
            imap_port: if imap_starttls { 143 } else { 993 },
            imap_starttls,
            smtp_host,
            smtp_port: if smtp_starttls { 587 } else { 465 },
            smtp_starttls,
        })
    });
    let uri = |iface: &str, disabled: &str| {
        ifaces.get(iface).filter(|_| !flag(account, disabled)).map(|p| without_user(&text(p, "Uri"))).filter(|u| u.starts_with("https://"))
    };
    // Correo finds the address books from the same address as the calendars.
    let dav = uri("Calendar", "CalendarDisabled").or_else(|| uri("Contacts", "ContactsDisabled"));
    if mail.is_none() && dav.is_none() {
        return None;
    }
    let identity = text(account, "PresentationIdentity");
    Some(GnomeAccount {
        provider: text(account, "ProviderName"),
        identity: if identity.is_empty() { text(account, "Identity") } else { identity },
        attention: flag(account, "AttentionNeeded"),
        mail,
        dav,
        id,
    })
}

/// `https://ana@cloud.example.org/dav` without the `ana@`: the name goes in the sign-in, not
/// the address (an `@` further on, in the path, stays).
fn without_user(uri: &str) -> String {
    let Some((scheme, rest)) = uri.split_once("://") else { return uri.to_string() };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = (rest.get(..end).unwrap_or(""), rest.get(end..).unwrap_or(""));
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    format!("{scheme}://{host}{path}")
}

/// The accounts GOA has, in its order; an error when GOA does not answer (not running, not
/// installed, no session bus).
pub fn list() -> Result<Vec<GnomeAccount>, String> {
    let mut accounts: Vec<GnomeAccount> = managed_objects()?.values().filter_map(account_from).collect();
    accounts.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(accounts)
}

/// The password or token of a GOA account, asked at each sign-in.
pub struct GoaSecret {
    pub id: String,
    /// For the calendars and contacts: the account's own password and sign-in name.
    pub dav: bool,
}

impl SecretSource for GoaSecret {
    fn secret(&self) -> Result<Secret, Error> {
        ask(&self.id, if self.dav { "password" } else { "imap-password" })
    }

    fn smtp_secret(&self) -> Result<Secret, Error> {
        ask(&self.id, "smtp-password")
    }

    fn login(&self) -> Option<String> {
        if !self.dav {
            return None;
        }
        identity(&self.id).ok().filter(|i| !i.is_empty())
    }

    fn name(&self) -> &'static str {
        "GNOME Settings"
    }
}

/// How GOA's refusals read in Correo: Account settings shows them.
fn refused(detail: &str) -> Error {
    Error::External(format!("GNOME Settings could not sign in ({detail}): sign in again under Online Accounts in GNOME Settings"))
}

#[cfg(all(unix, not(target_os = "macos")))]
mod bus {
    use std::collections::HashMap;

    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    use super::{Error, Interfaces, Prop, Secret, plausible_id, refused};

    const SERVICE: &str = "org.gnome.OnlineAccounts";
    const PREFIX: &str = "org.gnome.OnlineAccounts.";

    fn connection() -> Result<zbus::blocking::Connection, Error> {
        zbus::blocking::Connection::session().map_err(|e| Error::External(format!("GNOME Settings can't be reached: {e}")))
    }

    fn path(id: &str) -> Result<String, Error> {
        if !plausible_id(id) {
            return Err(Error::External("not a GNOME Online Accounts id".into()));
        }
        Ok(format!("/org/gnome/OnlineAccounts/Accounts/{id}"))
    }

    /// The D-Bus error name of a failed call.
    fn error_name(e: &zbus::Error) -> Option<&str> {
        match e {
            zbus::Error::MethodError(name, _, _) => Some(name.as_str()),
            _ => None,
        }
    }

    fn not_there(e: &zbus::Error) -> bool {
        error_name(e).is_some_and(|n| {
            matches!(
                n,
                "org.freedesktop.DBus.Error.UnknownMethod"
                    | "org.freedesktop.DBus.Error.UnknownInterface"
                    | "org.freedesktop.DBus.Error.UnknownObject"
            )
        })
    }

    fn convert(e: zbus::Error) -> Error {
        match &e {
            // GDBus says UnknownMethod for an object that is gone.
            _ if not_there(&e) => Error::External("the account is no longer under Online Accounts in GNOME Settings".into()),
            zbus::Error::MethodError(_, detail, _) => refused(detail.as_deref().unwrap_or("refused")),
            _ => Error::External(format!("GNOME Settings can't be reached: {e}")),
        }
    }

    pub fn ask(id: &str, purpose: &str) -> Result<Secret, Error> {
        let bus = connection()?;
        let path = path(id)?;
        match bus.call_method(Some(SERVICE), path.as_str(), Some("org.gnome.OnlineAccounts.OAuth2Based"), "GetAccessToken", &()) {
            Ok(reply) => {
                let (token, _expires_in): (String, i32) = reply.body().deserialize().map_err(convert)?;
                return if token.is_empty() { Err(refused("no token")) } else { Ok(Secret::Bearer(token)) };
            }
            // A password account has no OAuth2Based (and one that is gone has neither).
            Err(e) if not_there(&e) => {}
            Err(e) => return Err(convert(e)),
        }
        let reply = bus.call_method(Some(SERVICE), path.as_str(), Some("org.gnome.OnlineAccounts.PasswordBased"), "GetPassword", &(purpose,));
        let (password,): (String,) = reply.and_then(|r| r.body().deserialize()).map_err(convert)?;
        if password.is_empty() { Err(refused("no password")) } else { Ok(Secret::Password(password)) }
    }

    pub fn identity(id: &str) -> Result<String, Error> {
        let bus = connection()?;
        let path = path(id)?;
        let reply = bus
            .call_method(
                Some(SERVICE),
                path.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.gnome.OnlineAccounts.Account", "Identity"),
            )
            .map_err(convert)?;
        let (value,): (OwnedValue,) = reply.body().deserialize().map_err(convert)?;
        Ok(match &*value {
            Value::Str(s) => s.to_string(),
            _ => String::new(),
        })
    }

    pub fn managed_objects() -> Result<HashMap<String, Interfaces>, String> {
        let bus = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
        let reply = bus
            .call_method(Some(SERVICE), "/org/gnome/OnlineAccounts", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &())
            .map_err(|e| e.to_string())?;
        let objects: HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> =
            reply.body().deserialize().map_err(|e| e.to_string())?;
        Ok(objects
            .into_iter()
            .map(|(path, ifaces)| {
                let ifaces = ifaces
                    .into_iter()
                    .filter_map(|(name, props)| {
                        let short = name.strip_prefix(PREFIX)?.to_string();
                        let props = props
                            .into_iter()
                            .filter_map(|(k, v)| match &*v {
                                Value::Str(s) => Some((k, Prop::Str(s.to_string()))),
                                Value::Bool(b) => Some((k, Prop::Bool(*b))),
                                _ => None,
                            })
                            .collect();
                        Some((short, props))
                    })
                    .collect();
                (path.to_string(), ifaces)
            })
            .collect())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
use bus::{ask, identity, managed_objects};

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn ask(_id: &str, _purpose: &str) -> Result<Secret, Error> {
    Err(Error::External("GNOME Settings is not on this system".into()))
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn identity(_id: &str) -> Result<String, Error> {
    Err(Error::External("GNOME Settings is not on this system".into()))
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn managed_objects() -> Result<std::collections::HashMap<String, Interfaces>, String> {
    Err("GNOME Settings is not on this system".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, Prop)]) -> BTreeMap<String, Prop> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn s(v: &str) -> Prop {
        Prop::Str(v.into())
    }

    const YES: Prop = Prop::Bool(true);
    const NO: Prop = Prop::Bool(false);

    /// As goa-daemon 3.50 lists them (`GetManagedObjects`), trimmed.
    fn google() -> Interfaces {
        BTreeMap::from([
            (
                "Account".into(),
                props(&[
                    ("Id", s("account_1700000000_2")),
                    ("ProviderType", s("google")),
                    ("ProviderName", s("Google")),
                    ("Identity", s("ana@gmail.com")),
                    ("PresentationIdentity", s("ana@gmail.com")),
                    ("AttentionNeeded", NO),
                    ("MailDisabled", NO),
                    ("CalendarDisabled", NO),
                ]),
            ),
            (
                "Mail".into(),
                props(&[
                    ("EmailAddress", s("ana@gmail.com")),
                    ("Name", s("")),
                    ("ImapHost", s("imap.gmail.com")),
                    ("ImapUseSsl", YES),
                    ("ImapUseTls", NO),
                    ("ImapUserName", s("ana@gmail.com")),
                    ("SmtpHost", s("smtp.gmail.com")),
                    ("SmtpUseSsl", YES),
                    ("SmtpUseTls", YES),
                ]),
            ),
            ("Calendar".into(), props(&[("Uri", s("https://apidata.googleusercontent.com/caldav/v2/ana@gmail.com/user"))])),
            ("Contacts".into(), props(&[("Uri", s("https://www.googleapis.com/.well-known/carddav"))])),
            ("OAuth2Based".into(), props(&[])),
        ])
    }

    fn nextcloud() -> Interfaces {
        BTreeMap::from([
            (
                "Account".into(),
                props(&[
                    ("Id", s("account_1700000000_0")),
                    ("ProviderName", s("Nextcloud")),
                    ("Identity", s("ana")),
                    ("PresentationIdentity", s("ana@cloud.example.org")),
                    ("AttentionNeeded", YES),
                ]),
            ),
            ("Calendar".into(), props(&[("Uri", s("https://ana@cloud.example.org/remote.php/dav"))])),
            ("Contacts".into(), props(&[("Uri", s("https://ana@cloud.example.org/remote.php/dav"))])),
            ("Files".into(), props(&[("Uri", s("davs://ana@cloud.example.org/remote.php/webdav"))])),
            ("PasswordBased".into(), props(&[])),
        ])
    }

    #[test]
    fn a_google_account_gives_its_servers_and_calendar() {
        let a = account_from(&google()).unwrap();
        assert_eq!(
            (a.id.as_str(), a.provider.as_str(), a.identity.as_str(), a.attention),
            ("account_1700000000_2", "Google", "ana@gmail.com", false)
        );
        let m = a.mail.unwrap();
        assert_eq!((m.email.as_str(), m.user.as_str()), ("ana@gmail.com", ""), "the sign-in name is the address");
        assert_eq!((m.imap_host.as_str(), m.imap_port, m.imap_starttls), ("imap.gmail.com", 993, false));
        // Both TLS and STARTTLS said: TLS on 465.
        assert_eq!((m.smtp_host.as_str(), m.smtp_port, m.smtp_starttls), ("smtp.gmail.com", 465, false));
        assert_eq!(a.dav.as_deref(), Some("https://apidata.googleusercontent.com/caldav/v2/ana@gmail.com/user"), "an @ in the path stays");
    }

    #[test]
    fn a_nextcloud_account_gives_its_calendars_and_contacts() {
        let a = account_from(&nextcloud()).unwrap();
        assert!(a.mail.is_none());
        assert_eq!(a.dav.as_deref(), Some("https://cloud.example.org/remote.php/dav"), "the name goes in the sign-in");
        assert!(a.attention);
        assert_eq!(a.identity, "ana@cloud.example.org");
    }

    #[test]
    fn what_correo_cannot_use_is_left_out() {
        let mut g = google();
        g.get_mut("Account").unwrap().insert("MailDisabled".into(), YES);
        g.get_mut("Account").unwrap().insert("CalendarDisabled".into(), YES);
        // Contacts stay: their address is the one left.
        assert_eq!(account_from(&g).unwrap().dav.as_deref(), Some("https://www.googleapis.com/.well-known/carddav"));
        assert!(account_from(&g).unwrap().mail.is_none());
        g.get_mut("Account").unwrap().insert("ContactsDisabled".into(), YES);
        assert_eq!(account_from(&g), None, "nothing left to use");

        let mut plain = google();
        let mail = plain.get_mut("Mail").unwrap();
        mail.insert("ImapUseSsl".into(), NO);
        assert_eq!(account_from(&plain).unwrap().mail, None, "unencrypted IMAP");
        plain.get_mut("Mail").unwrap().insert("ImapUseTls".into(), YES);
        assert_eq!(account_from(&plain).unwrap().mail.map(|m| (m.imap_port, m.imap_starttls)), Some((143, true)));

        let mut odd = nextcloud();
        odd.get_mut("Account").unwrap().insert("Id".into(), s("../../etc"));
        assert_eq!(account_from(&odd), None);
        odd.get_mut("Account").unwrap().insert("Id".into(), Prop::Bool(true));
        assert_eq!(account_from(&odd), None);
        let mut http = nextcloud();
        http.get_mut("Calendar").unwrap().insert("Uri".into(), s("http://cloud.example.org/dav"));
        http.remove("Contacts");
        assert_eq!(account_from(&http), None, "not over https");
        assert_eq!(account_from(&Interfaces::new()), None);
    }

    /// Against a running goa-daemon, when `TINTA_GOA_LIVE` is set: the synthetic accounts and
    /// secrets of `docs/gnome.md` (Testing). Skipped otherwise.
    #[test]
    fn a_running_goa_daemon_answers() {
        if std::env::var_os("TINTA_GOA_LIVE").is_none() {
            return;
        }
        let accounts = list().unwrap();
        let by_id = |id: &str| accounts.iter().find(|a| a.id == id).cloned().unwrap();
        let nc = by_id("account_1700000000_0");
        assert_eq!(
            (nc.provider.as_str(), nc.dav.as_deref(), nc.mail.is_none()),
            ("Nextcloud", Some("https://cloud.example.org/remote.php/dav"), true)
        );
        let imap = by_id("account_1700000000_1");
        assert_eq!(
            imap.mail.as_ref().map(|m| (m.imap_host.as_str(), m.smtp_port, m.smtp_starttls, m.user.as_str())),
            Some(("imap.example.org", 587, true, "ana"))
        );
        let dav = GoaSecret { id: nc.id.clone(), dav: true };
        assert_eq!(dav.secret().unwrap(), Secret::Password("nc-secret".into()));
        assert_eq!(dav.login().as_deref(), Some("ana"));
        let mail = GoaSecret { id: imap.id.clone(), dav: false };
        assert_eq!(mail.secret().unwrap(), Secret::Password("imap-secret".into()));
        assert_eq!(mail.smtp_secret().unwrap(), Secret::Password("smtp-secret".into()));
        assert_eq!(mail.login(), None);
        // Google with no live grant: GNOME's refusal, said as such.
        let google = GoaSecret { id: "account_1700000000_2".into(), dav: false };
        let e = google.secret().unwrap_err().to_string();
        assert!(e.contains("GNOME Settings"), "{e}");
        let gone = GoaSecret { id: "account_1".into(), dav: false };
        let e = gone.secret().unwrap_err().to_string();
        assert!(e.contains("no longer under Online Accounts"), "{e}");
    }

    #[test]
    fn user_names_come_out_of_addresses() {
        assert_eq!(without_user("https://ana@cloud.example.org/remote.php/dav"), "https://cloud.example.org/remote.php/dav");
        assert_eq!(without_user("https://a:b@c@host:8443"), "https://host:8443");
        assert_eq!(without_user("https://host/x@y?u=a@b"), "https://host/x@y?u=a@b");
        assert_eq!(without_user("no scheme@here"), "no scheme@here");
        assert_eq!(without_user(""), "");
        assert!(plausible_id("account_1700000000_0"));
        assert!(!plausible_id("") && !plausible_id("a/b") && !plausible_id(&"a".repeat(129)));
    }
}
