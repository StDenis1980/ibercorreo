//! OAuth 2.0 sign-in for mail accounts (Gmail, Microsoft 365), written from RFC 6749 (OAuth 2.0),
//! RFC 7636 (PKCE) and RFC 8252 (OAuth for native apps).
//!
//! The person signs in on the provider's own page in their browser; Tinta Correo never sees the
//! password. The browser comes back to a one-time listener on this machine (`127.0.0.1`, a free
//! port), which takes the authorization code; the code and the PKCE verifier are exchanged for an
//! access token (an hour) and a refresh token (kept in the system credential store by the app).
//! IMAP and SMTP then sign in with `XOAUTH2` ([`Secret::Bearer`]).
//!
//! Each provider needs a client ID registered with it ([`App`]): a build takes them from the
//! `IBERCORREO_GOOGLE_CLIENT_ID` (and `_CLIENT_SECRET`) and `IBERCORREO_MICROSOFT_CLIENT_ID`
//! environment variables at compile time, and the same variables at run time win. Without one,
//! that provider's sign-in reports [`Error::NotConfigured`]. See `docs/oauth.md`.
//!
//! The network steps are passed in ([`Post`], the browser opener), so the flow is tested without
//! a provider.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ibercorreo_core::mail::OAuthProvider;

use crate::Tls;

/// How long the browser sign-in may take.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// An access token this close to expiring is renewed before use.
const RENEW_BEFORE: Duration = Duration::from_secs(120);
/// The largest request the loopback listener reads.
const MAX_REDIRECT_REQUEST: usize = 16 * 1024;
/// The largest token response accepted.
const MAX_TOKEN_RESPONSE: usize = 64 * 1024;
/// Access tokens longer than this are refused (real ones are 200 bytes to a few kilobytes).
const MAX_TOKEN: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// This build has no client ID for the provider.
    #[error("this build of Tinta Correo has no {0} client ID for OAuth sign-in (see docs/oauth.md)")]
    NotConfigured(&'static str),
    /// The grant is gone (revoked, expired, password changed): sign in again in the browser.
    #[error("{0} asks you to sign in again")]
    SignInAgain(&'static str),
    /// The provider refused the request.
    #[error("{provider} refused the sign-in: {detail}")]
    Refused { provider: &'static str, detail: String },
    /// The browser came back without a code, or the provider answered with something else.
    #[error("sign-in: {0}")]
    Protocol(String),
    /// The provider could not be reached.
    #[error("cannot reach {provider}: {detail}")]
    Network { provider: &'static str, detail: String },
    /// The sign-in took longer than [`SIGN_IN_TIMEOUT`] or was cancelled.
    #[error("the sign-in was not finished in the browser")]
    Cancelled,
    /// Random numbers or the listener on this machine were unavailable.
    #[error("sign-in: {0}")]
    Local(String),
}

/// A provider's OAuth endpoints and this app's registration with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub provider: &'static OAuthProvider,
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    /// Google issues one to desktop apps too; it is not secret there (RFC 8252 §8.5).
    pub client_secret: Option<String>,
    pub scope: &'static str,
    /// The host in the redirect address: what the provider's registration expects for a loopback
    /// redirect. The listener itself is always on 127.0.0.1.
    pub redirect_host: &'static str,
    /// Whether the token requests repeat the scope (Microsoft's v2 endpoint wants it).
    pub scope_on_token: bool,
    /// More parameters for the authorization request.
    pub extra: &'static [(&'static str, &'static str)],
}

struct Endpoints {
    id: &'static str,
    env: &'static str,
    auth_url: &'static str,
    token_url: &'static str,
    scope: &'static str,
    redirect_host: &'static str,
    scope_on_token: bool,
    extra: &'static [(&'static str, &'static str)],
    built_id: Option<&'static str>,
    built_secret: Option<&'static str>,
}

const ENDPOINTS: &[Endpoints] = &[
    Endpoints {
        id: "google",
        env: "GOOGLE",
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
        token_url: "https://oauth2.googleapis.com/token",
        scope: "https://mail.google.com/",
        redirect_host: "127.0.0.1",
        scope_on_token: false,
        // A refresh token, and one again on each sign-in.
        extra: &[("access_type", "offline"), ("prompt", "consent")],
        built_id: option_env!("IBERCORREO_GOOGLE_CLIENT_ID"),
        built_secret: option_env!("IBERCORREO_GOOGLE_CLIENT_SECRET"),
    },
    Endpoints {
        id: "microsoft",
        env: "MICROSOFT",
        auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        scope: "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access",
        redirect_host: "localhost",
        scope_on_token: true,
        extra: &[("prompt", "select_account")],
        built_id: option_env!("IBERCORREO_MICROSOFT_CLIENT_ID"),
        built_secret: option_env!("IBERCORREO_MICROSOFT_CLIENT_SECRET"),
    },
];

/// `provider`'s settings, with the client ID from the environment (`env`, then the build).
/// `IBERCORREO_<PROVIDER>_AUTH_URL` and `_TOKEN_URL` point the flow at another server (a test
/// server); the token address must be `https://`.
pub fn app_with(provider: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<App, Error> {
    let Some(p) = OAuthProvider::get(provider) else { return Err(Error::Protocol(format!("{provider:?} is not an OAuth provider"))) };
    let Some(e) = ENDPOINTS.iter().find(|e| e.id == p.id) else { return Err(Error::NotConfigured(p.name)) };
    let var = |suffix: &str| env(&format!("IBERCORREO_{}_{suffix}", e.env)).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let built = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    let client_id = var("CLIENT_ID").or_else(|| built(e.built_id)).ok_or(Error::NotConfigured(p.name))?;
    let token_url = var("TOKEN_URL").unwrap_or_else(|| e.token_url.to_string());
    if !token_url.to_ascii_lowercase().starts_with("https://") {
        return Err(Error::Protocol(format!("the {} token address must be https://", p.name)));
    }
    Ok(App {
        provider: p,
        auth_url: var("AUTH_URL").unwrap_or_else(|| e.auth_url.to_string()),
        token_url,
        client_id,
        client_secret: var("CLIENT_SECRET").or_else(|| built(e.built_secret)),
        scope: e.scope,
        redirect_host: e.redirect_host,
        scope_on_token: e.scope_on_token,
        extra: e.extra,
    })
}

/// [`app_with`] the process environment.
pub fn app(provider: &str) -> Result<App, Error> {
    app_with(provider, &|k| std::env::var(k).ok())
}

/// The providers this build (or the environment) has a client ID for.
pub fn configured() -> Vec<&'static OAuthProvider> {
    ibercorreo_core::mail::OAUTH_PROVIDERS.iter().filter(|p| app(p.id).is_ok()).collect()
}

/// Percent-encode for a query or form value (RFC 3986 unreserved characters stay).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decode a query or form value (`+` is a space). Malformed escapes stay as they are.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = |j: usize| bytes.get(j).and_then(|c| char::from(*c).to_digit(16));
        match b {
            b'%' => match (hex(i + 1), hex(i + 2)) {
                (Some(h), Some(l)) => {
                    out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
                    i += 3;
                    continue;
                }
                _ => out.push(b),
            },
            b'+' => out.push(b' '),
            _ => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Base64url without padding (RFC 7636 §3).
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = chunk.len() + 1;
        for k in 0..chars {
            let index = (n >> (18 - 6 * k)) & 63;
            if let Some(&c) = ALPHABET.get(index as usize) {
                out.push(char::from(c));
            }
        }
    }
    out
}

fn random(bytes: usize) -> Result<String, Error> {
    use ring::rand::SecureRandom;
    let mut buf = vec![0u8; bytes];
    ring::rand::SystemRandom::new().fill(&mut buf).map_err(|_| Error::Local("no random numbers".into()))?;
    Ok(base64url(&buf))
}

/// The PKCE challenge (`S256`) for a verifier.
pub fn challenge(verifier: &str) -> String {
    base64url(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()).as_ref())
}

/// The address the browser opens.
pub fn authorize_url(app: &App, redirect_uri: &str, state: &str, challenge: &str, login_hint: &str) -> String {
    let mut params = vec![
        ("response_type", "code"),
        ("client_id", app.client_id.as_str()),
        ("redirect_uri", redirect_uri),
        ("scope", app.scope),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ];
    if !login_hint.is_empty() {
        params.push(("login_hint", login_hint));
    }
    params.extend_from_slice(app.extra);
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect();
    let sep = if app.auth_url.contains('?') { '&' } else { '?' };
    format!("{}{sep}{}", app.auth_url, query.join("&"))
}

/// What a token request gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access: String,
    /// Seconds the access token is good for.
    pub expires_in: u64,
    /// A new refresh token, when the provider issued one (the first sign-in; Microsoft on every
    /// renewal).
    pub refresh: Option<String>,
}

/// Read a token endpoint's answer.
pub fn parse_tokens(provider: &'static OAuthProvider, status: u16, body: &[u8]) -> Result<Tokens, Error> {
    if body.len() > MAX_TOKEN_RESPONSE {
        return Err(Error::Protocol("the token response is too large".into()));
    }
    let json: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| Error::Protocol(format!("{} answered HTTP {status} without a token", provider.name)))?;
    let text = |k: &str| json.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|v| !v.is_empty());
    if let Some(error) = text("error") {
        if error == "invalid_grant" {
            return Err(Error::SignInAgain(provider.name));
        }
        let detail = match text("error_description") {
            Some(d) => format!("{error}: {}", d.chars().take(300).collect::<String>()),
            None => error.chars().take(100).collect(),
        };
        return Err(Error::Refused { provider: provider.name, detail });
    }
    if status != 200 {
        return Err(Error::Protocol(format!("{} answered HTTP {status}", provider.name)));
    }
    let token_ok = |t: &str| t.len() <= MAX_TOKEN && t.bytes().all(|b| b.is_ascii_graphic());
    let access = text("access_token").filter(|t| token_ok(t)).ok_or_else(|| Error::Protocol("no usable access token in the answer".into()))?;
    if let Some(kind) = text("token_type")
        && !kind.eq_ignore_ascii_case("bearer")
    {
        return Err(Error::Protocol(format!("a {kind:?} token, not a bearer token")));
    }
    let expires_in = json.get("expires_in").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(3600);
    Ok(Tokens {
        access: access.to_string(),
        expires_in: expires_in.clamp(60, 24 * 3600),
        refresh: text("refresh_token").filter(|t| token_ok(t)).map(str::to_string),
    })
}

/// What the flow needs from the network: an HTTPS `POST` of a form.
pub trait Post {
    /// The status and body.
    fn post(&mut self, url: &str, form: &str) -> Result<(u16, Vec<u8>), String>;
}

/// The real network, trusting `tls`.
pub struct Net<'a> {
    pub tls: &'a Tls,
}

impl Post for Net<'_> {
    fn post(&mut self, url: &str, form: &str) -> Result<(u16, Vec<u8>), String> {
        let url = crate::dav::Url::parse(url).map_err(|e| e.to_string())?;
        let mut client = crate::dav_anonymous(&url, self.tls);
        let headers = [("Content-Type", "application/x-www-form-urlencoded"), ("Accept", "application/json")];
        let r = client.send("POST", &url.path, &headers, form.as_bytes()).map_err(|e| e.to_string())?;
        Ok((r.status, r.body))
    }
}

fn token_request<'a>(post: &mut dyn Post, app: &'a App, mut form: Vec<(&'a str, &'a str)>) -> Result<Tokens, Error> {
    form.push(("client_id", &app.client_id));
    if let Some(secret) = &app.client_secret {
        form.push(("client_secret", secret));
    }
    if app.scope_on_token {
        form.push(("scope", app.scope));
    }
    let body: Vec<String> = form.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect();
    let (status, reply) = post.post(&app.token_url, &body.join("&")).map_err(|detail| Error::Network { provider: app.provider.name, detail })?;
    parse_tokens(app.provider, status, &reply)
}

/// Exchange the code the browser brought back for tokens.
pub fn exchange(post: &mut dyn Post, app: &App, code: &str, verifier: &str, redirect_uri: &str) -> Result<Tokens, Error> {
    let form = vec![("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect_uri), ("code_verifier", verifier)];
    token_request(post, app, form)
}

/// A new access token from a refresh token.
pub fn refresh(post: &mut dyn Post, app: &App, refresh_token: &str) -> Result<Tokens, Error> {
    token_request(post, app, vec![("grant_type", "refresh_token"), ("refresh_token", refresh_token)])
}

/// The one-time listener the browser comes back to.
pub struct Listener {
    listener: TcpListener,
    pub redirect_uri: String,
}

impl Listener {
    pub fn bind(app: &App) -> Result<Listener, Error> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| Error::Local(e.to_string()))?;
        let port = listener.local_addr().map_err(|e| Error::Local(e.to_string()))?.port();
        listener.set_nonblocking(true).map_err(|e| Error::Local(e.to_string()))?;
        Ok(Listener { listener, redirect_uri: format!("http://{}:{port}/", app.redirect_host) })
    }

    /// Wait for the browser to come back with `state`, until `timeout` or `stop()`. Requests that
    /// are not the redirect (a favicon, a stale tab of another sign-in) are answered and waited
    /// past; a redirect carrying an error ends the wait.
    pub fn wait(&self, state: &str, timeout: Duration, stop: &dyn Fn() -> bool) -> Result<String, Error> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline && !stop() {
            match self.listener.accept() {
                Ok((tcp, _)) => {
                    if let Some(outcome) = answer(tcp, state) {
                        return outcome;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => return Err(Error::Local(e.to_string())),
            }
        }
        Err(Error::Cancelled)
    }
}

const PAGE_DONE: &str = "Signed in. You can close this tab and go back to Tinta Correo.";
const PAGE_FAILED: &str = "The sign-in did not finish. Go back to Tinta Correo and try again.";

fn reply(tcp: &mut TcpStream, status: &str, text: &str) {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Tinta Correo</title></head>\
<body style=\"font-family:sans-serif;margin:3em\"><p>{text}</p></body></html>"
    );
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = tcp.write_all(head.as_bytes()).and_then(|()| tcp.write_all(body.as_bytes())).and_then(|()| tcp.flush());
}

/// Read one request on the listener: the code (or the error) when it is the redirect for `state`,
/// `None` when it is something else.
fn answer(mut tcp: TcpStream, state: &str) -> Option<Result<String, Error>> {
    let _ = tcp.set_nonblocking(false);
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < MAX_REDIRECT_REQUEST {
        match tcp.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let line = head.lines().next().unwrap_or("");
    let mut parts = line.split(' ');
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        reply(&mut tcp, "400 Bad Request", PAGE_FAILED);
        return None;
    };
    let query = target.split_once('?').map_or("", |(_, q)| q);
    let params: Vec<(String, String)> = query.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (decode(k), decode(v))).take(32).collect();
    let get = |k: &str| params.iter().find(|(name, _)| name == k).map(|(_, v)| v.as_str());
    if get("state") != Some(state) {
        reply(&mut tcp, "404 Not Found", PAGE_FAILED);
        return None;
    }
    if let Some(error) = get("error") {
        reply(&mut tcp, "200 OK", PAGE_FAILED);
        let detail = get("error_description").unwrap_or(error).chars().take(300).collect::<String>();
        return Some(if error == "access_denied" { Err(Error::Cancelled) } else { Err(Error::Protocol(detail)) });
    }
    match get("code").filter(|c| !c.is_empty() && c.len() <= MAX_TOKEN) {
        Some(code) => {
            reply(&mut tcp, "200 OK", PAGE_DONE);
            Some(Ok(code.to_string()))
        }
        None => {
            reply(&mut tcp, "200 OK", PAGE_FAILED);
            Some(Err(Error::Protocol("the browser came back without a code".into())))
        }
    }
}

/// The whole browser sign-in: listen, open `authorize_url` with `open`, wait for the code,
/// exchange it. `open` gets the address to show the person too, in case no browser opens.
pub fn sign_in_with(
    post: &mut dyn Post,
    app: &App,
    login_hint: &str,
    open: &mut dyn FnMut(&str) -> Result<(), String>,
    stop: &dyn Fn() -> bool,
) -> Result<Tokens, Error> {
    let listener = Listener::bind(app)?;
    let verifier = random(32)?;
    let state = random(16)?;
    let url = authorize_url(app, &listener.redirect_uri, &state, &challenge(&verifier), login_hint);
    open(&url).map_err(Error::Local)?;
    let code = listener.wait(&state, SIGN_IN_TIMEOUT, stop)?;
    let tokens = exchange(post, app, &code, &verifier, &listener.redirect_uri)?;
    if tokens.refresh.is_none() {
        return Err(Error::Protocol(format!("{} gave no refresh token", app.provider.name)));
    }
    Ok(tokens)
}

/// The program and arguments that open `url`: `$BROWSER` when set (its first entry; `%s` stands
/// for the address, else the address goes last), else the system's opener.
fn browser_command(browser: Option<&str>, url: &str) -> (String, Vec<String>) {
    if let Some(first) = browser.and_then(|b| b.split(':').map(str::trim).find(|b| !b.is_empty())) {
        let mut words = first.split_whitespace().map(str::to_string);
        let program = words.next().unwrap_or_default();
        let mut args: Vec<String> = words.collect();
        if args.iter().any(|a| a.contains("%s")) {
            args = args.into_iter().map(|a| a.replace("%s", url)).collect();
        } else {
            args.push(url.to_string());
        }
        return (program, args);
    }
    if cfg!(target_os = "windows") {
        ("rundll32".into(), vec!["url.dll,FileProtocolHandler".into(), url.into()])
    } else if cfg!(target_os = "macos") {
        ("open".into(), vec![url.into()])
    } else {
        ("xdg-open".into(), vec![url.into()])
    }
}

/// Open `url` in the person's browser. No shell is involved, so the address is one argument.
pub fn open_browser(url: &str) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let (program, args) = browser_command(std::env::var("BROWSER").ok().as_deref(), url);
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| format!("could not open a browser: {e}"))
}

/// What a sign-in hands the server.
#[derive(Clone, PartialEq, Eq)]
pub enum Secret {
    Password(String),
    /// An OAuth 2.0 access token (`XOAUTH2`).
    Bearer(String),
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Secret::Password(_) => "Password(…)",
            Secret::Bearer(_) => "Bearer(…)",
        })
    }
}

/// An OAuth grant: the refresh token and the access token in use.
pub struct Grant {
    app: App,
    refresh: String,
    access: Option<(String, Instant)>,
    /// The provider replaced the refresh token; the app stores the new one.
    rotated: bool,
    /// The provider said the grant is gone: sign in again.
    revoked: bool,
}

/// How an account signs in, shared by the threads that use it (Send/Receive, the Inbox watch).
#[derive(Clone)]
pub enum Credential {
    Password(String),
    OAuth(Arc<Mutex<Grant>>),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Credential::Password(_) => "Credential::Password(…)",
            Credential::OAuth(_) => "Credential::OAuth(…)",
        })
    }
}

impl Credential {
    /// A grant from a stored refresh token; the first use fetches an access token.
    pub fn oauth(app: App, refresh: String) -> Credential {
        Credential::OAuth(Arc::new(Mutex::new(Grant { app, refresh, access: None, rotated: false, revoked: false })))
    }

    /// A grant straight after a sign-in, with its access token.
    pub fn signed_in(app: App, tokens: Tokens) -> Credential {
        let refresh = tokens.refresh.clone().unwrap_or_default();
        let access = Some((tokens.access, Instant::now() + Duration::from_secs(tokens.expires_in)));
        Credential::OAuth(Arc::new(Mutex::new(Grant { app, refresh, access, rotated: false, revoked: false })))
    }

    /// The secret for a sign-in now, renewing the access token over `post` when it is about to
    /// expire. Threads wait for each other's renewal.
    pub fn secret_with(&self, post: &mut dyn Post) -> Result<Secret, Error> {
        let grant = match self {
            Credential::Password(p) => return Ok(Secret::Password(p.clone())),
            Credential::OAuth(g) => g,
        };
        let mut g = grant.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((token, until)) = &g.access
            && Instant::now() + RENEW_BEFORE < *until
        {
            return Ok(Secret::Bearer(token.clone()));
        }
        let tokens = match refresh(post, &g.app, &g.refresh) {
            Ok(t) => t,
            Err(e) => {
                g.revoked = matches!(e, Error::SignInAgain(_));
                return Err(e);
            }
        };
        g.access = Some((tokens.access.clone(), Instant::now() + Duration::from_secs(tokens.expires_in)));
        if let Some(new) = tokens.refresh
            && new != g.refresh
        {
            g.refresh = new;
            g.rotated = true;
        }
        Ok(Secret::Bearer(tokens.access))
    }

    /// [`Credential::secret_with`] the network.
    pub fn secret(&self, tls: &Tls) -> Result<Secret, Error> {
        self.secret_with(&mut Net { tls })
    }

    /// Drop the access token (the server refused it), so the next use renews it.
    pub fn forget_access(&self) {
        if let Credential::OAuth(g) = self {
            g.lock().unwrap_or_else(PoisonError::into_inner).access = None;
        }
    }

    /// The refresh token, when the provider replaced it since the last call: to be stored.
    pub fn take_rotated(&self) -> Option<String> {
        let Credential::OAuth(g) = self else { return None };
        let mut g = g.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut g.rotated).then(|| g.refresh.clone())
    }

    /// Whether the provider said this grant is gone (the stored refresh token is useless).
    pub fn revoked(&self) -> bool {
        match self {
            Credential::Password(_) => false,
            Credential::OAuth(g) => g.lock().unwrap_or_else(PoisonError::into_inner).revoked,
        }
    }

    /// The provider's name, for messages (`None` for a password).
    pub fn provider(&self) -> Option<&'static str> {
        match self {
            Credential::Password(_) => None,
            Credential::OAuth(g) => Some(g.lock().unwrap_or_else(PoisonError::into_inner).app.provider.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| pairs.iter().find(|(name, _)| *name == k).map(|(_, v)| v.to_string())
    }

    fn google() -> App {
        app_with("google", &env(&[("IBERCORREO_GOOGLE_CLIENT_ID", "id-1.apps"), ("IBERCORREO_GOOGLE_CLIENT_SECRET", "s")])).unwrap()
    }

    #[test]
    fn pkce_matches_the_rfc_example() {
        // RFC 7636 appendix B.
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"\xfb\xff"), "-_8");
        let (a, b) = (random(32).unwrap(), random(32).unwrap());
        assert_eq!(a.len(), 43, "32 bytes make a 43-character verifier");
        assert_ne!(a, b);
    }

    #[test]
    fn client_ids_come_from_the_environment() {
        let built = ENDPOINTS.iter().any(|e| e.built_id.is_some());
        if !built {
            assert_eq!(app_with("google", &env(&[])), Err(Error::NotConfigured("Google")));
        }
        let g = google();
        assert_eq!((g.client_id.as_str(), g.client_secret.as_deref()), ("id-1.apps", Some("s")));
        assert_eq!(g.token_url, "https://oauth2.googleapis.com/token");
        let m = app_with("microsoft", &env(&[("IBERCORREO_MICROSOFT_CLIENT_ID", " ms-id ")])).unwrap();
        assert_eq!((m.client_id.as_str(), m.redirect_host, m.scope_on_token), ("ms-id", "localhost", true));
        let local = env(&[("IBERCORREO_GOOGLE_CLIENT_ID", "x"), ("IBERCORREO_GOOGLE_TOKEN_URL", "http://127.0.0.1/token")]);
        assert!(matches!(app_with("google", &local), Err(Error::Protocol(_))), "tokens never travel over plain HTTP");
        assert!(app_with("yahoo", &env(&[])).is_err());
    }

    #[test]
    fn the_authorization_address() {
        let url = authorize_url(&google(), "http://127.0.0.1:5000/", "st", "ch", "ana+x@gmail.com");
        assert_eq!(
            url,
            "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id=id-1.apps\
&redirect_uri=http%3A%2F%2F127.0.0.1%3A5000%2F&scope=https%3A%2F%2Fmail.google.com%2F&state=st\
&code_challenge=ch&code_challenge_method=S256&login_hint=ana%2Bx%40gmail.com&access_type=offline&prompt=consent"
        );
        assert_eq!(decode("a%2Bb+c%zz%4"), "a+b c%zz%4");
    }

    #[test]
    fn the_browser_to_open() {
        let url = "https://x/?a=1&b=2";
        assert_eq!(browser_command(Some("firefox"), url), ("firefox".into(), vec![url.to_string()]));
        assert_eq!(browser_command(Some(" :chromium --new-window %s:lynx"), url), ("chromium".into(), vec!["--new-window".into(), url.to_string()]));
        let (program, args) = browser_command(Some(""), url);
        assert!(!program.is_empty() && args.last().map(String::as_str) == Some(url), "the system's opener");
    }

    #[test]
    fn token_answers() {
        let p = google().provider;
        let ok = br#"{"access_token":"ya29.a","expires_in":3599,"refresh_token":"1//r","token_type":"Bearer","scope":"https://mail.google.com/"}"#;
        assert_eq!(parse_tokens(p, 200, ok).unwrap(), Tokens { access: "ya29.a".into(), expires_in: 3599, refresh: Some("1//r".into()) });
        let short = br#"{"access_token":"t","expires_in":"5"}"#;
        assert_eq!(parse_tokens(p, 200, short).unwrap().expires_in, 60, "a few seconds count as a minute");
        assert_eq!(
            parse_tokens(p, 400, br#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#),
            Err(Error::SignInAgain("Google"))
        );
        assert!(matches!(parse_tokens(p, 401, br#"{"error":"invalid_client"}"#), Err(Error::Refused { .. })));
        for bad in [&b"<html>"[..], b"{}", br#"{"access_token":"a b"}"#, br#"{"access_token":"t","token_type":"mac"}"#, b"[1,2]"] {
            assert!(parse_tokens(p, 200, bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
        assert!(parse_tokens(p, 500, ok).is_err(), "only a 200 carries tokens");
        assert!(parse_tokens(p, 200, &vec![b' '; MAX_TOKEN_RESPONSE + 1]).is_err());
    }

    /// A token endpoint answering from a list, recording the forms.
    struct Fake {
        answers: Vec<(u16, &'static str)>,
        forms: Vec<String>,
    }

    impl Post for Fake {
        fn post(&mut self, url: &str, form: &str) -> Result<(u16, Vec<u8>), String> {
            assert!(url.starts_with("https://"));
            self.forms.push(form.to_string());
            if self.answers.is_empty() {
                return Err("offline".into());
            }
            let (status, body) = self.answers.remove(0);
            Ok((status, body.as_bytes().to_vec()))
        }
    }

    #[test]
    fn the_browser_comes_back_to_the_listener() {
        let app = google();
        let mut fake = Fake { answers: vec![(200, r#"{"access_token":"A1","expires_in":3600,"refresh_token":"R1"}"#)], forms: Vec::new() };
        // The "browser": first a stray request, then one from another sign-in, then the redirect.
        let mut open = |url: &str| -> Result<(), String> {
            let query = url.split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
            let param = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).map(decode).unwrap_or_default();
            let (redirect, state) = (param("redirect_uri"), param("state"));
            assert!(redirect.starts_with("http://127.0.0.1:") && param("code_challenge").len() == 43);
            let port = redirect.trim_start_matches("http://127.0.0.1:").trim_end_matches('/').to_string();
            std::thread::spawn(move || {
                for path in ["/favicon.ico".to_string(), "/?state=old&code=stale".to_string(), format!("/?state={state}&code=4%2F0Ab")] {
                    let mut s = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
                    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()).unwrap();
                    let mut page = String::new();
                    let _ = s.read_to_string(&mut page);
                }
            });
            Ok(())
        };
        let tokens = sign_in_with(&mut fake, &app, "ana@gmail.com", &mut open, &|| false).unwrap();
        assert_eq!((tokens.access.as_str(), tokens.refresh.as_deref()), ("A1", Some("R1")));
        let form = &fake.forms[0];
        assert!(form.starts_with("grant_type=authorization_code&code=4%2F0Ab&redirect_uri=http%3A%2F%2F127.0.0.1%3A"), "{form}");
        assert!(form.contains("&code_verifier=") && form.contains("&client_id=id-1.apps&client_secret=s"), "{form}");
    }

    #[test]
    fn a_cancelled_or_abandoned_sign_in() {
        let app = google();
        let mut fake = Fake { answers: Vec::new(), forms: Vec::new() };
        let mut deny = |url: &str| -> Result<(), String> {
            let query = url.split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
            let param = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).map(decode).unwrap_or_default();
            let port = param("redirect_uri").trim_start_matches("http://127.0.0.1:").trim_end_matches('/').to_string();
            let state = param("state");
            std::thread::spawn(move || {
                let mut s = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
                s.write_all(format!("GET /?error=access_denied&state={state} HTTP/1.1\r\n\r\n").as_bytes()).unwrap();
            });
            Ok(())
        };
        assert_eq!(sign_in_with(&mut fake, &app, "", &mut deny, &|| false), Err(Error::Cancelled));
        assert!(fake.forms.is_empty(), "nothing is exchanged");
        // Stopped while waiting.
        assert_eq!(sign_in_with(&mut fake, &app, "", &mut |_| Ok(()), &|| true), Err(Error::Cancelled));
        // No browser at all.
        assert!(matches!(sign_in_with(&mut fake, &app, "", &mut |_| Err("no browser".into()), &|| false), Err(Error::Local(_))));
    }

    #[test]
    fn access_tokens_are_renewed_and_rotated_refresh_tokens_kept() {
        let app = app_with("microsoft", &env(&[("IBERCORREO_MICROSOFT_CLIENT_ID", "ms")])).unwrap();
        let credential = Credential::oauth(app, "R0".into());
        let mut fake = Fake {
            answers: vec![(200, r#"{"access_token":"A1","expires_in":3600,"refresh_token":"R1"}"#), (400, r#"{"error":"invalid_grant"}"#)],
            forms: Vec::new(),
        };
        assert_eq!(credential.secret_with(&mut fake).unwrap(), Secret::Bearer("A1".into()));
        assert_eq!(credential.secret_with(&mut fake).unwrap(), Secret::Bearer("A1".into()), "still good: not renewed");
        assert_eq!(fake.forms.len(), 1);
        assert!(fake.forms[0].starts_with("grant_type=refresh_token&refresh_token=R0&client_id=ms&scope="), "{}", fake.forms[0]);
        assert_eq!(credential.take_rotated().as_deref(), Some("R1"));
        assert_eq!(credential.take_rotated(), None, "handed out once");
        credential.forget_access();
        assert!(!credential.revoked());
        assert_eq!(credential.secret_with(&mut fake), Err(Error::SignInAgain("Microsoft")));
        assert!(credential.revoked());
        assert!(fake.forms[1].contains("refresh_token=R1"), "the rotated token is the one used");
        assert!(matches!(credential.secret_with(&mut fake), Err(Error::Network { .. })));
        assert_eq!(Credential::Password("p".into()).secret_with(&mut fake).unwrap(), Secret::Password("p".into()));
        assert_eq!(format!("{:?}", Secret::Bearer("A1".into())), "Bearer(…)", "tokens never reach logs");
    }
}
