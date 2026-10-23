//! One Send/Receive for an account, start to finish on the network: send the Outbox over SMTP,
//! save the copies to the server's Sent, sync over IMAP, then sync the calendars over CalDAV. It takes a snapshot of what the
//! store needs ([`Job`]) and returns what happened ([`Finished`]), so it can run on a background
//! thread while the window stays responsive; the front end applies the result to the session.

use ibercorreo_core::mail::{Account, AccountKind};
use ibercorreo_sync::outbox::{self, Outcome, Outgoing, Sent};
use ibercorreo_sync::{Batch, Limits, State, calendar};

use crate::{Endpoint, Security, Tls};

#[derive(Debug, Clone)]
pub struct Job {
    pub account: Account,
    pub password: String,
    pub tls: Tls,
    pub state: State,
    pub outbox: Vec<Outgoing>,
    /// The server mailbox for sent copies, once the Sent folder is linked to one.
    pub sent_path: Option<String>,
    pub limits: Limits,
    /// What the store holds of the CalDAV calendars (used when the account has a CalDAV address).
    pub calendar: calendar::State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub sent: Vec<Sent>,
    pub batch: Result<Batch, String>,
    /// The calendar sync, when the account has a CalDAV address.
    pub calendar: Option<Result<calendar::Batch, String>>,
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
        return Finished { sent: Vec::new(), batch: Err("this account has no mail server".into()), calendar: None };
    };
    let Some(login) = job.account.login() else {
        return Finished { sent: Vec::new(), batch: Err("this account has no sign-in name".into()), calendar: None };
    };

    let mut sent = Vec::new();
    if !job.outbox.is_empty() {
        let endpoint = Endpoint { host: smtp_host.clone(), port: *smtp_port, security: security(*smtp_security) };
        let session = crate::smtp(&endpoint, &job.tls, "localhost")
            .map_err(|e| e.to_string())
            .and_then(|mut c| c.login(login, &job.password).map(|()| c).map_err(|e| format!("sign-in to {smtp_host}: {e}")));
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
        .and_then(|mut c| c.login(login, &job.password).map(|()| c).map_err(|e| format!("sign-in to {imap_host}: {e}")))
        .and_then(|mut c| {
            if let Some(path) = &job.sent_path {
                outbox::save_sent(&mut c, path, &job.outbox, &mut sent);
            }
            let batch = ibercorreo_sync::run(&mut c, &job.state, &job.limits).map_err(|e| e.to_string());
            let _ = c.logout();
            batch
        });
    let calendar = (!job.account.caldav.trim().is_empty()).then(|| sync_calendars(job, login));
    Finished { sent, batch, calendar }
}

/// The CalDAV part, on its own connection; mail failing does not stop it.
fn sync_calendars(job: &Job, login: &str) -> Result<calendar::Batch, String> {
    let url = crate::dav::Url::parse(&job.account.caldav).map_err(|e| e.to_string())?;
    let mut client = crate::dav(&url, &job.tls, login, &job.password);
    calendar::run(&mut client, &url, &job.calendar).map_err(|e| e.to_string())
}
