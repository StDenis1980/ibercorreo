//! Push mail: an IMAP session left open on the Inbox with `IDLE` (RFC 2177), so a new message
//! starts a Send/Receive within seconds instead of at the next five-minute round.
//!
//! [`watch`] runs on its own thread with its own connection. It returns when the Inbox changed,
//! when asked to stop, or on an error; the caller then syncs, waits or reconnects. The session is
//! read-only (`EXAMINE`), so it never changes what the sync sees.

use std::time::{Duration, Instant};

use crate::{Endpoint, Error, Result, Tls, imap};

/// How long one wait on the socket lasts before checking whether to stop.
pub const SLICE: Duration = Duration::from_secs(20);
/// `IDLE` is ended and started again this often: servers drop idle sessions after 30 minutes.
pub const RESTART: Duration = Duration::from_secs(25 * 60);

/// Why [`watch`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// The Inbox changed: a message arrived, went, or its flags changed elsewhere.
    Changed,
    /// `stop` said so.
    Stopped,
}

/// Idle on `mailbox` (as the server names it, usually `INBOX`) until it changes or `stop()` is
/// true. An error means the session ended: a refused sign-in, a server without `IDLE`
/// (`Error::Imap(Unsupported)`), or a lost connection.
pub fn watch(endpoint: &Endpoint, tls: &Tls, user: &str, password: &str, mailbox: &str, stop: &dyn Fn() -> bool) -> Result<Wake> {
    let tcp = crate::tcp_with(&endpoint.host, endpoint.port, crate::CONNECT_TIMEOUT, SLICE)?;
    let mut client = crate::imap_over(tcp, endpoint, tls)?;
    client.login(user, password)?;
    let wake = idle_on(&mut client, mailbox, stop);
    let _ = client.logout();
    wake
}

/// The loop of [`watch`], on a signed-in session.
pub fn idle_on<S: std::io::Read + std::io::Write>(client: &mut imap::Client<S>, mailbox: &str, stop: &dyn Fn() -> bool) -> Result<Wake> {
    if !client.has("IDLE") {
        return Err(Error::Imap(imap::Error::Unsupported("the server does not offer IDLE")));
    }
    client.select(mailbox, true)?;
    loop {
        let started = Instant::now();
        if client.idle_start()?.iter().any(imap::is_news) {
            client.idle_done()?;
            return Ok(Wake::Changed);
        }
        while started.elapsed() < RESTART {
            if stop() {
                client.idle_done()?;
                return Ok(Wake::Stopped);
            }
            let news = client.idle_wait()?;
            if news.iter().any(imap::is_news) {
                client.idle_done()?;
                return Ok(Wake::Changed);
            }
            // The server ended the idle on its own: start it again.
            if !client.is_idling() {
                break;
            }
        }
        if client.idle_done()?.iter().any(imap::is_news) {
            return Ok(Wake::Changed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io::{Read, Write};

    use super::*;

    /// A server that sends one burst per read and times out between them.
    struct Bursts(std::collections::VecDeque<Option<&'static str>>, Vec<u8>);

    impl Read for Bursts {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop_front() {
                Some(Some(text)) => {
                    let bytes = text.replace('\n', "\r\n").into_bytes();
                    let n = bytes.len().min(buf.len());
                    buf.get_mut(..n).unwrap().copy_from_slice(bytes.get(..n).unwrap());
                    Ok(n)
                }
                Some(None) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "quiet")),
                None => Ok(0),
            }
        }
    }

    impl Write for Bursts {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.1.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn client(after_greeting: &[Option<&'static str>]) -> imap::Client<Bursts> {
        let mut script = vec![Some("* PREAUTH [CAPABILITY IMAP4rev1 IDLE] ready\n"), Some("* 2 EXISTS\na1 OK [READ-ONLY] done\n")];
        script.extend_from_slice(after_greeting);
        imap::Client::new(Bursts(script.into(), Vec::new()), true).unwrap()
    }

    #[test]
    fn a_new_message_wakes_the_watch() {
        let mut c = client(&[Some("+ idling\n"), None, Some("* OK still here\n"), None, Some("* 3 EXISTS\n"), Some("a2 OK done\n")]);
        assert_eq!(idle_on(&mut c, "INBOX", &|| false).unwrap(), Wake::Changed);
        let sent = String::from_utf8(c.into_stream().1).unwrap();
        assert_eq!(sent, "a1 EXAMINE INBOX\r\na2 IDLE\r\nDONE\r\n");
    }

    #[test]
    fn stopping_ends_the_idle_and_errors_end_the_watch() {
        let checks = Cell::new(0);
        let stop = || {
            checks.set(checks.get() + 1);
            checks.get() > 2
        };
        let mut c = client(&[Some("+ idling\n"), None, None, Some("a2 OK done\n")]);
        assert_eq!(idle_on(&mut c, "INBOX", &stop).unwrap(), Wake::Stopped);
        assert!(String::from_utf8(c.into_stream().1).unwrap().ends_with("DONE\r\n"));

        let mut c = client(&[Some("+ idling\n"), None]);
        assert!(idle_on(&mut c, "INBOX", &|| false).is_err(), "the server hung up");
        let mut c = imap::Client::new(Bursts(vec![Some("* PREAUTH [CAPABILITY IMAP4rev1] ready\n")].into(), Vec::new()), true).unwrap();
        assert!(matches!(idle_on(&mut c, "INBOX", &|| false), Err(Error::Imap(imap::Error::Unsupported(_)))));
    }
}
