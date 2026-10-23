//! Iber Correo's connections to mail servers, L4: TCP with timeouts, TLS through rustls (ring
//! provider, certificates checked by the operating system's verifier), and opening an IMAP
//! or SMTP session with implicit TLS (IMAP 993, SMTP 465) or STARTTLS (IMAP 143, SMTP 587), and
//! CalDAV and CardDAV clients over HTTPS, finding an address's servers ([`autoconfig`]) and OAuth
//! 2.0 sign-in ([`oauth`]).
//!
//! Desktop only: the web build reaches servers through a proxy instead (planned), so this crate
//! is not part of the wasm check.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use thiserror::Error;

pub mod autoconfig;
pub mod idle;
pub mod job;
pub mod oauth;

pub use ibercorreo_dav as dav;
pub use ibercorreo_imap as imap;
pub use ibercorreo_smtp as smtp;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    /// The host name is not a valid DNS name or IP address.
    #[error("invalid server name: {0}")]
    Host(String),
    /// The name did not resolve, or no address accepted the connection.
    #[error("cannot connect to {host}: {detail}")]
    Connect { host: String, detail: String },
    /// The TLS handshake failed: an untrusted or mismatched certificate, or no common protocol.
    #[error("secure connection failed: {0}")]
    Tls(String),
    /// Reading or writing failed after connecting.
    #[error("connection: {0}")]
    Io(String),
    #[error(transparent)]
    Imap(#[from] imap::Error),
    #[error(transparent)]
    Smtp(#[from] smtp::Error),
    /// The OAuth sign-in could not give an access token.
    #[error(transparent)]
    OAuth(#[from] oauth::Error),
}

impl From<Error> for ibercorreo_core::Error {
    fn from(e: Error) -> Self {
        ibercorreo_core::Error::Network(e.to_string())
    }
}

/// How a connection is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// TLS from the first byte (IMAP 993, SMTP 465). RFC 8314 recommends this.
    Tls,
    /// Plain first, then `STARTTLS`; fails if the server does not offer it.
    StartTls,
    /// No encryption. Only for a server on this machine: passwords are refused over it.
    Plain,
}

/// Where a server is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

/// Connection timeouts: connecting, and each read or write after that.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
pub const IO_TIMEOUT: Duration = Duration::from_secs(120);

/// The TLS settings: which certificates are trusted.
#[derive(Debug, Clone)]
pub struct Tls {
    config: Arc<ClientConfig>,
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

impl Tls {
    /// Trust what the operating system trusts (its certificate store and revocation checks).
    pub fn platform() -> Result<Self> {
        use rustls_platform_verifier::BuilderVerifierExt;
        let config = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .and_then(|b| b.with_platform_verifier())
            .map_err(|e| Error::Tls(e.to_string()))?
            .with_no_client_auth();
        Ok(Tls { config: Arc::new(config) })
    }

    /// Trust only `roots` (DER certificates): a company's own CA, or a test server.
    pub fn with_roots(roots: &[Vec<u8>]) -> Result<Self> {
        let mut store = RootCertStore::empty();
        for der in roots {
            store.add(CertificateDer::from(der.clone())).map_err(|e| Error::Tls(e.to_string()))?;
        }
        let config = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Tls(e.to_string()))?
            .with_root_certificates(store)
            .with_no_client_auth();
        Ok(Tls { config: Arc::new(config) })
    }
}

/// A connection to a server, encrypted or not.
#[derive(Debug)]
pub enum Stream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Stream {
    pub fn is_tls(&self) -> bool {
        matches!(self, Stream::Tls(_))
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// A TCP connection to `host:port`, trying each address it resolves to.
pub fn tcp(host: &str, port: u16) -> Result<TcpStream> {
    tcp_with(host, port, CONNECT_TIMEOUT, IO_TIMEOUT)
}

/// [`tcp`] with other timeouts: for connecting, and for each read or write after that.
pub fn tcp_with(host: &str, port: u16, connect: Duration, io: Duration) -> Result<TcpStream> {
    let failed = |detail: String| Error::Connect { host: format!("{host}:{port}"), detail };
    let addrs = (host, port).to_socket_addrs().map_err(|e| failed(e.to_string()))?;
    let mut last = String::from("no address");
    for addr in addrs.take(16) {
        match TcpStream::connect_timeout(&addr, connect) {
            Ok(s) => {
                s.set_read_timeout(Some(io)).map_err(|e| Error::Io(e.to_string()))?;
                s.set_write_timeout(Some(io)).map_err(|e| Error::Io(e.to_string()))?;
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(failed(last))
}

/// Run the TLS handshake on `tcp`, checking the certificate against `host`.
pub fn secure(tcp: TcpStream, host: &str, tls: &Tls) -> Result<Stream> {
    let name = ServerName::try_from(host.to_owned()).map_err(|_| Error::Host(host.to_owned()))?;
    let conn = ClientConnection::new(Arc::clone(&tls.config), name).map_err(|e| Error::Tls(e.to_string()))?;
    let mut stream = StreamOwned::new(conn, tcp);
    // Finish the handshake now, so a bad certificate is reported as such and not as a read error later.
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock).map_err(|e| tls_error(&e))?;
    }
    Ok(Stream::Tls(Box::new(stream)))
}

fn tls_error(e: &std::io::Error) -> Error {
    match e.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
        Some(t) => Error::Tls(t.to_string()),
        None if e.kind() == std::io::ErrorKind::InvalidData => Error::Tls(e.to_string()),
        None => Error::Io(e.to_string()),
    }
}

/// Connect to an IMAP server and read its greeting. The session is not logged in yet.
pub fn imap(endpoint: &Endpoint, tls: &Tls) -> Result<imap::Client<Stream>> {
    imap_over(tcp(&endpoint.host, endpoint.port)?, endpoint, tls)
}

/// [`imap`] on a connection already open to `endpoint`.
pub fn imap_over(tcp: TcpStream, endpoint: &Endpoint, tls: &Tls) -> Result<imap::Client<Stream>> {
    match endpoint.security {
        Security::Tls => Ok(imap::Client::new(secure(tcp, &endpoint.host, tls)?, true)?),
        Security::Plain => Ok(imap::Client::new(Stream::Plain(tcp), false)?),
        Security::StartTls => {
            let client = imap::Client::new(Stream::Plain(tcp), false)?;
            match client.starttls()? {
                Stream::Plain(tcp) => Ok(imap::Client::resume(secure(tcp, &endpoint.host, tls)?)?),
                Stream::Tls(_) => Err(Error::Tls("already encrypted".into())),
            }
        }
    }
}

/// Connect to an SMTP submission server and introduce this machine as `helo`. Not logged in yet.
pub fn smtp(endpoint: &Endpoint, tls: &Tls, helo: &str) -> Result<smtp::Client<Stream>> {
    smtp_over(tcp(&endpoint.host, endpoint.port)?, endpoint, tls, helo)
}

/// [`smtp`] on a connection already open to `endpoint`.
pub fn smtp_over(tcp: TcpStream, endpoint: &Endpoint, tls: &Tls, helo: &str) -> Result<smtp::Client<Stream>> {
    match endpoint.security {
        Security::Tls => Ok(smtp::Client::new(secure(tcp, &endpoint.host, tls)?, true, helo)?),
        Security::Plain => Ok(smtp::Client::new(Stream::Plain(tcp), false, helo)?),
        Security::StartTls => {
            let client = smtp::Client::new(Stream::Plain(tcp), false, helo)?;
            match client.starttls()? {
                Stream::Plain(tcp) => Ok(smtp::Client::resume(secure(tcp, &endpoint.host, tls)?, helo)?),
                Stream::Tls(_) => Err(Error::Tls("already encrypted".into())),
            }
        }
    }
}

/// Sign in to an IMAP session: `LOGIN` with a password, `AUTHENTICATE XOAUTH2` with a token.
pub fn imap_sign_in<S: Read + Write>(client: &mut imap::Client<S>, user: &str, secret: &oauth::Secret) -> Result<(), imap::Error> {
    match secret {
        oauth::Secret::Password(p) => client.login(user, p),
        oauth::Secret::Bearer(t) => client.authenticate_xoauth2(user, t),
    }
}

/// Sign in to an SMTP session: `AUTH PLAIN` (or `LOGIN`) with a password, `AUTH XOAUTH2` with a token.
pub fn smtp_sign_in<S: Read + Write>(client: &mut smtp::Client<S>, user: &str, secret: &oauth::Secret) -> Result<(), smtp::Error> {
    match secret {
        oauth::Secret::Password(p) => client.login(user, p),
        oauth::Secret::Bearer(t) => client.login_xoauth2(user, t),
    }
}

/// A CalDAV/CardDAV client signing in with `secret`: HTTP Basic with a password, a bearer token
/// with OAuth.
pub fn dav_with(
    url: &dav::Url,
    tls: &Tls,
    user: &str,
    secret: &oauth::Secret,
) -> dav::Client<Stream, impl FnMut() -> std::io::Result<Stream> + use<>> {
    // `[::1]` in a URL is `::1` to the resolver and the certificate check.
    let host = url.host.trim_start_matches('[').trim_end_matches(']').to_string();
    let (port, tls) = (url.port, tls.clone());
    let connect = move || {
        let tcp = tcp(&host, port).map_err(|e| std::io::Error::other(e.to_string()))?;
        secure(tcp, &host, &tls).map_err(|e| std::io::Error::other(e.to_string()))
    };
    match secret {
        oauth::Secret::Password(p) => dav::Client::new(connect, url, user, p),
        oauth::Secret::Bearer(t) => dav::Client::bearer(connect, url, t),
    }
}

/// A CalDAV client for `url`'s server signing in as `user`. Every connection it opens is TLS,
/// checked against the host name.
pub fn dav(url: &dav::Url, tls: &Tls, user: &str, password: &str) -> dav::Client<Stream, impl FnMut() -> std::io::Result<Stream> + use<>> {
    dav_with(url, tls, user, &oauth::Secret::Password(password.to_string()))
}

/// An HTTPS client for `url`'s server that sends no credentials.
pub fn dav_anonymous(url: &dav::Url, tls: &Tls) -> dav::Client<Stream, impl FnMut() -> std::io::Result<Stream> + use<>> {
    let host = url.host.trim_start_matches('[').trim_end_matches(']').to_string();
    let (port, tls) = (url.port, tls.clone());
    let connect = move || {
        let tcp = tcp(&host, port).map_err(|e| std::io::Error::other(e.to_string()))?;
        secure(tcp, &host, &tls).map_err(|e| std::io::Error::other(e.to_string()))
    };
    dav::Client::anonymous(connect, url)
}

#[cfg(test)]
mod tests;
