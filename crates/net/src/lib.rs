//! Iber Correo's connections to mail servers, L3: TCP with timeouts, TLS through rustls (ring
//! provider, certificates checked by the operating system's verifier), and opening an IMAP
//! session with implicit TLS (port 993) or STARTTLS (port 143).
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

pub use ibercorreo_imap as imap;

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
    let failed = |detail: String| Error::Connect { host: format!("{host}:{port}"), detail };
    let addrs = (host, port).to_socket_addrs().map_err(|e| failed(e.to_string()))?;
    let mut last = String::from("no address");
    for addr in addrs.take(16) {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                s.set_read_timeout(Some(IO_TIMEOUT)).map_err(|e| Error::Io(e.to_string()))?;
                s.set_write_timeout(Some(IO_TIMEOUT)).map_err(|e| Error::Io(e.to_string()))?;
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
    let tcp = tcp(&endpoint.host, endpoint.port)?;
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

#[cfg(test)]
mod tests;
