//! One Send/Receive for an account, start to finish on the network: send the Outbox over SMTP,
//! save the copies to the server's Sent, sync over IMAP, then sync the calendars over CalDAV and
//! the contacts over CardDAV. It takes a snapshot of what the store needs ([`Job`]) and returns
//! what happened ([`Finished`]), so it can run on a background thread while the window stays
//! responsive; the front end applies the result to the session.

use ibercorreo_core::mail::{Account, AccountKind};
use ibercorreo_sync::outbox::{self, Outcome, Outgoing, Sent};
use ibercorreo_sync::{Batch, Limits, State, calendar, contacts};

use crate::{Endpoint, Security, Tls};

#[derive(Debug, Clone)]
pub struct Job {
    pub account: Account,
    /// The password, or the OAuth grant (renewed here when its access token runs out).
    pub credential: crate::oauth::Credential,
    pub tls: Tls,
    pub state: State,
    pub outbox: Vec<Outgoing>,
    /// The server mailbox for sent copies, once the Sent folder is linked to one.
    pub sent_path: Option<String>,
    pub limits: Limits,
    /// What the store holds of the CalDAV calendars (used when the account has a CalDAV address).
    pub calendar: calendar::State,
    /// What the store holds of the CardDAV address books (same server address as the calendars).
    pub contacts: contacts::State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub sent: Vec<Sent>,
    pub batch: Result<Batch, String>,
    /// The calendar sync, when the account has a CalDAV address.
    pub calendar: Option<Result<calendar::Batch, String>>,
    /// The contact sync, when the account has a CalDAV address (it serves CardDAV too).
    pub contacts: Option<Result<contacts::Batch, String>>,
}

fn security(s: ibercorreo_core::Security) -> Security {
    match s {
        ibercorreo_core::Security::Tls => Security::Tls,
        ibercorreo_core::Security::StartTls => Security::StartTls,
    }
}

/// Run the job. Nothing here panics or gives up early on one failure: a failed send leaves the
/// mail queued and the receive still runs.
pub fn run(job: &Job) -> Finished {
    let AccountKind::Imap { imap_host, imap_port, smtp_host, smtp_port, imap_security, smtp_security, .. } = &job.account.kind else {
        return Finished { sent: Vec::new(), batch: Err("this account has no mail server".into()), calendar: None, contacts: None };
    };
    let Some(login) = job.account.login() else {
        return Finished { sent: Vec::new(), batch: Err("this account has no sign-in name".into()), calendar: None, contacts: None };
    };

    let secret = match job.credential.secret(&job.tls) {
        Ok(s) => s,
        Err(e) => {
            // A grant that is gone is a sign-in to do again; a provider out of reach is not.
            let e = match e {
                crate::oauth::Error::Network { .. } => e.to_string(),
                other => format!("sign-in: {other}"),
            };
            let sent = job.outbox.iter().map(|m| Sent { id: m.id, outcome: Outcome::Later(e.clone()) }).collect();
            return Finished { sent, batch: Err(e), calendar: None, contacts: None };
        }
    };
    // A token the server refuses is renewed at the next try.
    let refused = |e: String| {
        job.credential.forget_access();
        e
    };

    let mut sent = Vec::new();
    if !job.outbox.is_empty() {
        let endpoint = Endpoint { host: smtp_host.clone(), port: *smtp_port, security: security(*smtp_security) };
        let session = crate::smtp(&endpoint, &job.tls, "localhost")
            .map_err(|e| e.to_string())
            .and_then(|mut c| crate::smtp_sign_in(&mut c, login, &secret).map(|()| c).map_err(|e| refused(format!("sign-in to {smtp_host}: {e}"))));
        match session {
            Ok(mut c) => {
                sent = outbox::send(&mut c, &job.outbox);
                let _ = c.quit();
            }
            Err(e) => sent = job.outbox.iter().map(|m| Sent { id: m.id, outcome: Outcome::Later(e.clone()) }).collect(),
        }
    }

    let endpoint = Endpoint { host: imap_host.clone(), port: *imap_port, security: security(*imap_security) };
    let batch = crate::imap(&endpoint, &job.tls)
        .map_err(|e| e.to_string())
        .and_then(|mut c| crate::imap_sign_in(&mut c, login, &secret).map(|()| c).map_err(|e| refused(format!("sign-in to {imap_host}: {e}"))))
        .and_then(|mut c| {
            // Gmail files what it sends itself: a saved copy would be a second one.
            if ibercorreo_core::mail::OAuthProvider::for_host(smtp_host).is_some_and(|p| p.saves_sent) {
                outbox::server_saved(&mut sent);
            } else if let Some(path) = &job.sent_path {
                outbox::save_sent(&mut c, path, &job.outbox, &mut sent);
            }
            let batch = ibercorreo_sync::run(&mut c, &job.state, &job.limits).map_err(|e| e.to_string());
            let _ = c.logout();
            batch
        });
    let (calendar, contacts) = if job.account.caldav.trim().is_empty() {
        (None, None)
    } else {
        let (calendar, contacts) = sync_dav(job, login, &secret);
        (Some(calendar), Some(contacts))
    };
    Finished { sent, batch, calendar, contacts }
}

/// The CalDAV and CardDAV part, on its own connection; mail failing does not stop it, and the
/// calendars failing does not stop the contacts.
fn sync_dav(job: &Job, login: &str, secret: &crate::oauth::Secret) -> (Result<calendar::Batch, String>, Result<contacts::Batch, String>) {
    let url = match crate::dav::Url::parse(&job.account.caldav) {
        Ok(url) => url,
        Err(e) => return (Err(e.to_string()), Err(e.to_string())),
    };
    let mut client = crate::dav_with(&url, &job.tls, login, secret);
    let calendar = calendar::run(&mut client, &url, &job.calendar).map_err(|e| e.to_string());
    let contacts = contacts::run(&mut client, &url, &job.contacts).map_err(|e| e.to_string());
    (calendar, contacts)
}
