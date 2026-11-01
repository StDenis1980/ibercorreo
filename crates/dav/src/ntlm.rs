//! NTLM sign-in over HTTP (NTLMv2, from the published MS-NLMP specification), which on-premises
//! Exchange servers ask for when Basic sign-in is turned off.
//!
//! NTLM signs in a connection, not a request: the client sends a negotiate message
//! ([`negotiate`]), the server answers `401` with a challenge ([`Challenge::parse`]), and the
//! client answers it on the same connection ([`authenticate`]). Later requests on that connection
//! need no credentials.
//!
//! The answer is tied to the server's TLS certificate when the caller passes it
//! ([`tls_server_end_point`], RFC 5929), as Exchange's Extended Protection requires. The answer
//! carries no message integrity code: HTTP does not need one, and servers accept answers without.
//!
//! Randomness, the clock and the certificate come from the caller ([`Hooks`]), so this crate
//! still never touches the system.

use hmac::{KeyInit, Mac};
use md4::Digest as _;

use crate::{Error, Result};

const SIGNATURE: &[u8; 8] = b"NTLMSSP\0";
const NEGOTIATE_UNICODE: u32 = 0x0000_0001;
const REQUEST_TARGET: u32 = 0x0000_0004;
const NEGOTIATE_NTLM: u32 = 0x0000_0200;
const ALWAYS_SIGN: u32 = 0x0000_8000;
const EXTENDED_SESSION_SECURITY: u32 = 0x0008_0000;
const NEGOTIATE_TARGET_INFO: u32 = 0x0080_0000;
const NEGOTIATE_128: u32 = 0x2000_0000;
const NEGOTIATE_56: u32 = 0x8000_0000;
/// What the client asks for, and the most it agrees to.
const FLAGS: u32 = NEGOTIATE_UNICODE
    | REQUEST_TARGET
    | NEGOTIATE_NTLM
    | ALWAYS_SIGN
    | EXTENDED_SESSION_SECURITY
    | NEGOTIATE_TARGET_INFO
    | NEGOTIATE_128
    | NEGOTIATE_56;

/// Attribute ids in the challenge's target information.
const AV_EOL: u16 = 0;
const AV_TIMESTAMP: u16 = 7;
const AV_CHANNEL_BINDINGS: u16 = 10;
/// Largest challenge read (real ones are a few hundred bytes).
pub const MAX_CHALLENGE: usize = 16 * 1024;
/// Most attributes read from a challenge.
const MAX_ATTRIBUTES: usize = 64;

/// What the client needs from the system: none of it is reached from this crate.
pub struct Hooks<S> {
    /// Eight random bytes for the client challenge, or `None` when there is no random source.
    pub nonce: fn() -> Option<[u8; 8]>,
    /// The time now as a Windows FILETIME (100 ns since 1601), used when the server sends none.
    pub now: fn() -> Option<u64>,
    /// The `tls-server-end-point` channel binding of the connection ([`tls_server_end_point`]),
    /// or `None` to send none.
    pub binding: fn(&S) -> Option<Vec<u8>>,
}

impl<S> Clone for Hooks<S> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<S> Copy for Hooks<S> {}

/// Who signs in. `DOMAIN\user` is split; any other name (`ana@contoso.es`) goes as it is, with no
/// domain, which Windows reads as a user principal name.
#[derive(Clone, PartialEq, Eq)]
pub struct Identity {
    pub user: String,
    pub domain: String,
    pub password: String,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The password never reaches a log.
        f.debug_struct("Identity").field("user", &self.user).field("domain", &self.domain).finish_non_exhaustive()
    }
}

impl Identity {
    pub fn from_login(login: &str, password: &str) -> Self {
        let login = login.trim();
        let (domain, user) = login.split_once('\\').unwrap_or(("", login));
        Identity { user: user.to_string(), domain: domain.to_string(), password: password.to_string() }
    }
}

/// The negotiate message (type 1): no domain or workstation, so nothing about this computer.
pub fn negotiate() -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(SIGNATURE);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&FLAGS.to_le_bytes());
    // Empty domain and workstation fields (length, room, offset).
    out.extend_from_slice(&[0u8; 16]);
    out
}

/// The server's challenge (type 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub flags: u32,
    pub server_challenge: [u8; 8],
    /// The attributes as sent, without the closing end-of-list one.
    pub target_info: Vec<(u16, Vec<u8>)>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

impl Challenge {
    pub fn parse(b: &[u8]) -> Result<Challenge> {
        let bad = |what: &str| Error::Protocol(format!("NTLM challenge: {what}"));
        if b.len() > MAX_CHALLENGE {
            return Err(Error::TooLarge { what: "the NTLM challenge", limit: MAX_CHALLENGE });
        }
        if b.get(..8) != Some(SIGNATURE.as_slice()) || u32_at(b, 8) != Some(2) {
            return Err(bad("not a challenge message"));
        }
        let flags = u32_at(b, 20).ok_or_else(|| bad("too short"))?;
        let mut server_challenge = [0u8; 8];
        server_challenge.copy_from_slice(b.get(24..32).ok_or_else(|| bad("too short"))?);
        let mut target_info = Vec::new();
        // The target information fields are at 40; an old server may stop before them.
        if flags & NEGOTIATE_TARGET_INFO != 0 && b.len() >= 48 {
            let len = usize::from(u16_at(b, 40).ok_or_else(|| bad("too short"))?);
            let at = usize::try_from(u32_at(b, 44).ok_or_else(|| bad("too short"))?).map_err(|_| bad("offset"))?;
            let info = at.checked_add(len).and_then(|end| b.get(at..end)).ok_or_else(|| bad("target information out of bounds"))?;
            let mut pos = 0usize;
            loop {
                let id = u16_at(info, pos).ok_or_else(|| bad("target information not closed"))?;
                let n = usize::from(u16_at(info, pos.saturating_add(2)).ok_or_else(|| bad("target information not closed"))?);
                if id == AV_EOL {
                    break;
                }
                let start = pos.saturating_add(4);
                let value = start.checked_add(n).and_then(|end| info.get(start..end)).ok_or_else(|| bad("attribute out of bounds"))?;
                if target_info.len() >= MAX_ATTRIBUTES {
                    return Err(bad("too many attributes"));
                }
                target_info.push((id, value.to_vec()));
                pos = start.saturating_add(n);
            }
        }
        Ok(Challenge { flags, server_challenge, target_info })
    }

    /// The server's time, when it sent one.
    fn timestamp(&self) -> Option<[u8; 8]> {
        self.target_info.iter().find(|(id, _)| *id == AV_TIMESTAMP).and_then(|(_, v)| <[u8; 8]>::try_from(v.as_slice()).ok())
    }
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn hmac_md5(key: &[u8], parts: &[&[u8]]) -> [u8; 16] {
    // HMAC accepts a key of any length, so this never fails; the fallback key is never used.
    let mut mac =
        <hmac::Hmac<md5::Md5> as KeyInit>::new_from_slice(key).unwrap_or_else(|_| <hmac::Hmac<md5::Md5> as KeyInit>::new(&Default::default()));
    for p in parts {
        mac.update(p);
    }
    mac.finalize().into_bytes().into()
}

/// The NT hash of a password: MD4 of its UTF-16 form.
pub fn nt_hash(password: &str) -> [u8; 16] {
    md4::Md4::digest(utf16(password)).into()
}

/// The NTLMv2 key of an identity (`NTOWFv2`).
pub fn ntowf_v2(id: &Identity) -> [u8; 16] {
    let name = format!("{}{}", id.user.to_uppercase(), id.domain);
    hmac_md5(&nt_hash(&id.password), &[&utf16(&name)])
}

/// The `tls-server-end-point` application data (RFC 5929) for a certificate hash.
pub fn tls_server_end_point(cert_hash: &[u8]) -> Vec<u8> {
    let mut out = b"tls-server-end-point:".to_vec();
    out.extend_from_slice(cert_hash);
    out
}

/// The hash RFC 5929 names for a certificate: its signature's hash, with MD5 and SHA-1 raised
/// to SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertHash {
    Sha256,
    Sha384,
    Sha512,
}

/// One DER element at the start of `b`: its tag, its contents and what follows.
fn der(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *b.first()?;
    let first = *b.get(1)?;
    let (len, head) = if first < 0x80 {
        (usize::from(first), 2usize)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..n {
            len = len.checked_mul(256)?.checked_add(usize::from(*b.get(2usize.checked_add(i)?)?))?;
        }
        (len, n.checked_add(2)?)
    };
    let end = head.checked_add(len)?;
    Some((tag, b.get(head..end)?, b.get(end..)?))
}

/// Which hash binds a channel to the certificate `cert` (DER). An unreadable certificate gets
/// SHA-256, the usual one.
pub fn cert_hash(cert: &[u8]) -> CertHash {
    const SEQUENCE: u8 = 0x30;
    const OID: u8 = 0x06;
    let oid = || {
        let (tag, certificate, _) = der(cert)?;
        let (_, _, rest) = der(certificate).filter(|_| tag == SEQUENCE)?;
        let (tag, algorithm, _) = der(rest)?;
        let (oid_tag, oid, _) = der(algorithm).filter(|_| tag == SEQUENCE)?;
        (oid_tag == OID).then_some(oid)
    };
    match oid() {
        // sha384WithRSAEncryption, ecdsa-with-SHA384
        Some([0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c] | [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03]) => CertHash::Sha384,
        // sha512WithRSAEncryption, ecdsa-with-SHA512
        Some([0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d] | [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04]) => CertHash::Sha512,
        _ => CertHash::Sha256,
    }
}

/// The channel binding hash: MD5 of a GSS channel bindings structure with no addresses and the
/// application data.
fn binding_hash(application_data: &[u8]) -> [u8; 16] {
    let mut s = vec![0u8; 16];
    s.extend_from_slice(&u32::try_from(application_data.len()).unwrap_or(u32::MAX).to_le_bytes());
    s.extend_from_slice(application_data);
    md5::Md5::digest(&s).into()
}

/// What the client brings to its answer.
pub struct Answer<'a> {
    pub identity: &'a Identity,
    pub client_challenge: [u8; 8],
    /// FILETIME now, used when the challenge carries no time.
    pub now: u64,
    /// `tls-server-end-point` application data, when the connection is TLS.
    pub binding: Option<&'a [u8]>,
}

/// The authenticate message (type 3) answering `challenge`.
pub fn authenticate(challenge: &Challenge, answer: &Answer<'_>) -> Result<Vec<u8>> {
    if challenge.flags & NEGOTIATE_UNICODE == 0 {
        return Err(Error::Protocol("the server offers NTLM without Unicode names".into()));
    }
    let key = ntowf_v2(answer.identity);
    let server_time = challenge.timestamp();
    let time = server_time.unwrap_or(answer.now.to_le_bytes());
    // The server's attributes, then the channel binding, then the end of the list.
    let mut info = Vec::new();
    let mut push = |id: u16, value: &[u8]| {
        info.extend_from_slice(&id.to_le_bytes());
        info.extend_from_slice(&u16::try_from(value.len()).unwrap_or(u16::MAX).to_le_bytes());
        info.extend_from_slice(value.get(..usize::from(u16::MAX)).unwrap_or(value));
    };
    for (id, value) in &challenge.target_info {
        if *id != AV_CHANNEL_BINDINGS {
            push(*id, value);
        }
    }
    if let Some(app) = answer.binding {
        push(AV_CHANNEL_BINDINGS, &binding_hash(app));
    }
    push(AV_EOL, &[]);
    let mut temp = vec![1u8, 1, 0, 0, 0, 0, 0, 0];
    temp.extend_from_slice(&time);
    temp.extend_from_slice(&answer.client_challenge);
    temp.extend_from_slice(&[0u8; 4]);
    temp.extend_from_slice(&info);
    temp.extend_from_slice(&[0u8; 4]);
    let proof = hmac_md5(&key, &[&challenge.server_challenge, &temp]);
    let mut nt = proof.to_vec();
    nt.extend_from_slice(&temp);
    // With the server's time the LMv2 answer is left empty (zeros), as the specification says.
    let lm = if server_time.is_some() {
        vec![0u8; 24]
    } else {
        let mut lm = hmac_md5(&key, &[&challenge.server_challenge, &answer.client_challenge]).to_vec();
        lm.extend_from_slice(&answer.client_challenge);
        lm
    };
    let fields: [Vec<u8>; 6] = [lm, nt, utf16(&answer.identity.domain), utf16(&answer.identity.user), Vec::new(), Vec::new()];
    const HEAD: usize = 64;
    let mut out = Vec::with_capacity(HEAD.saturating_add(fields.iter().map(Vec::len).sum()));
    out.extend_from_slice(SIGNATURE);
    out.extend_from_slice(&3u32.to_le_bytes());
    let mut offset = HEAD;
    for f in &fields {
        let len = u16::try_from(f.len()).map_err(|_| Error::Invalid("the NTLM sign-in name is too long".into()))?;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&u32::try_from(offset).map_err(|_| Error::Invalid("NTLM answer too large".into()))?.to_le_bytes());
        offset = offset.saturating_add(f.len());
    }
    out.extend_from_slice(&(challenge.flags & FLAGS).to_le_bytes());
    for f in &fields {
        out.extend_from_slice(f);
    }
    Ok(out)
}

/// The NTLM challenge in a `401` reply's `WWW-Authenticate` headers, decoded.
pub fn challenge_in(resp: &crate::Response) -> Option<Vec<u8>> {
    resp.headers.iter().filter(|(n, _)| n == "www-authenticate").flat_map(|(_, v)| v.split(',')).find_map(|offer| {
        let mut words = offer.split_whitespace();
        let scheme = words.next()?;
        let data = words.next()?;
        scheme.eq_ignore_ascii_case("NTLM").then(|| ibercorreo_mime::unbase64(data)).flatten()
    })
}

/// Whether a `401` reply offers NTLM.
pub fn offered(resp: &crate::Response) -> bool {
    resp.headers
        .iter()
        .filter(|(n, _)| n == "www-authenticate")
        .flat_map(|(_, v)| v.split(','))
        .any(|offer| offer.split_whitespace().next().is_some_and(|s| s.eq_ignore_ascii_case("NTLM")))
}

#[cfg(test)]
pub(crate) mod tests;
