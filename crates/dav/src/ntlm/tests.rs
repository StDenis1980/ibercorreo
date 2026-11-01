use super::*;

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// A challenge message with `flags`, the specification's server challenge and `info` as its
/// target information.
pub(crate) fn challenge_message(flags: u32, info: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut ti = Vec::new();
    for (id, v) in info {
        ti.extend_from_slice(&id.to_le_bytes());
        ti.extend_from_slice(&(v.len() as u16).to_le_bytes());
        ti.extend_from_slice(v);
    }
    ti.extend_from_slice(&[0, 0, 0, 0]);
    let mut m = SIGNATURE.to_vec();
    m.extend_from_slice(&2u32.to_le_bytes());
    m.extend_from_slice(&[0, 0, 0, 0, 48, 0, 0, 0]); // empty target name
    m.extend_from_slice(&flags.to_le_bytes());
    m.extend_from_slice(&hex("0123456789abcdef"));
    m.extend_from_slice(&[0u8; 8]);
    m.extend_from_slice(&(ti.len() as u16).to_le_bytes());
    m.extend_from_slice(&(ti.len() as u16).to_le_bytes());
    m.extend_from_slice(&48u32.to_le_bytes());
    m.extend_from_slice(&ti);
    m
}

/// The specification's example: "Domain" and "Server" as the target information.
pub(crate) fn spec_info() -> Vec<(u16, Vec<u8>)> {
    vec![(2, utf16("Domain")), (1, utf16("Server"))]
}

fn spec_identity() -> Identity {
    Identity::from_login("Domain\\User", "Password")
}

/// A field (length, room, offset) of an authenticate message, read back.
fn field(m: &[u8], at: usize) -> Vec<u8> {
    let len = u16_at(m, at).unwrap() as usize;
    let off = u32_at(m, at + 4).unwrap() as usize;
    m[off..off + len].to_vec()
}

#[test]
fn the_keys_match_the_specification() {
    // MS-NLMP 4.2.2.1.2 and 4.2.4.1.1.
    assert_eq!(nt_hash("Password").to_vec(), hex("a4f49c406510bdcab6824ee7c30fd852"));
    assert_eq!(nt_hash("").to_vec(), hex("31d6cfe0d16ae931b73c59d7e0c089c0"), "MD4 of nothing");
    assert_eq!(ntowf_v2(&spec_identity()).to_vec(), hex("0c868a403bfd7a93a3001ef22ef02e3f"));
}

#[test]
fn the_answer_matches_the_specification() {
    // MS-NLMP 4.2.4: no server time, so the LMv2 answer is computed.
    let challenge = Challenge::parse(&challenge_message(0xe28a8233, &spec_info())).unwrap();
    assert_eq!(challenge.server_challenge.to_vec(), hex("0123456789abcdef"));
    assert_eq!(challenge.target_info, spec_info());
    let id = spec_identity();
    let m = authenticate(&challenge, &Answer { identity: &id, client_challenge: [0xaa; 8], now: 0, binding: None }).unwrap();
    assert_eq!(&m[..12], b"NTLMSSP\0\x03\0\0\0");
    assert_eq!(field(&m, 12), hex("86c35097ac9cec102554764a57cccc19 aaaaaaaaaaaaaaaa"), "LMv2");
    let nt = field(&m, 20);
    assert_eq!(nt[..16].to_vec(), hex("68cd0ab851e51c96aabc927bebef6a1c"), "NTProofStr");
    let mut temp = hex("0101000000000000 0000000000000000 aaaaaaaaaaaaaaaa 00000000");
    temp.extend_from_slice(&hex("02000c00 44006f006d00610069006e00 01000c00 530065007200760065007200 00000000 00000000"));
    assert_eq!(nt[16..].to_vec(), temp);
    assert_eq!(field(&m, 28), utf16("Domain"));
    assert_eq!(field(&m, 36), utf16("User"));
    assert!(field(&m, 44).is_empty() && field(&m, 52).is_empty(), "no workstation, no session key");
    assert_eq!(u32_at(&m, 60), Some(0xe28a8233 & FLAGS), "only flags both sides agree");
}

#[test]
fn the_server_time_and_the_certificate_go_into_the_answer() {
    let time = 0x01d9_0000_1234_5678u64.to_le_bytes().to_vec();
    let mut info = spec_info();
    info.push((AV_TIMESTAMP, time.clone()));
    // A binding the server sent itself is replaced by ours.
    info.push((AV_CHANNEL_BINDINGS, vec![9; 16]));
    let challenge = Challenge::parse(&challenge_message(FLAGS, &info)).unwrap();
    let id = Identity::from_login("ana@contoso.es", "pw");
    assert_eq!((id.user.as_str(), id.domain.as_str()), ("ana@contoso.es", ""));
    let app = tls_server_end_point(&[7u8; 32]);
    let m = authenticate(&challenge, &Answer { identity: &id, client_challenge: [1; 8], now: 5, binding: Some(&app) }).unwrap();
    assert_eq!(field(&m, 12), vec![0u8; 24], "with the server's time the LMv2 answer is zeros");
    let nt = field(&m, 20);
    assert_eq!(&nt[24..32], time.as_slice(), "the server's time, not ours");
    let mut want = vec![0u8; 16];
    want.extend_from_slice(&(app.len() as u32).to_le_bytes());
    want.extend_from_slice(&app);
    let mut cb = vec![10, 0, 16, 0];
    cb.extend_from_slice(&md5::Md5::digest(&want));
    let at = nt.windows(cb.len()).position(|w| w == cb.as_slice());
    assert!(at.is_some(), "the channel binding is in the answer");
    assert_eq!(nt.windows(4).filter(|w| *w == [10, 0, 16, 0]).count(), 1, "once");
    assert!(nt.ends_with(&[0, 0, 0, 0, 0, 0, 0, 0]), "the list is closed");
    assert_eq!(&app[..21], b"tls-server-end-point:");

    // Without our clock's time the answer differs from one with it.
    let no_time = Challenge::parse(&challenge_message(FLAGS, &spec_info())).unwrap();
    let a = authenticate(&no_time, &Answer { identity: &id, client_challenge: [1; 8], now: 5, binding: None }).unwrap();
    assert_eq!(&field(&a, 20)[24..32], 5u64.to_le_bytes().as_slice(), "our time when the server sends none");
}

#[test]
fn odd_challenges_are_refused_without_panicking() {
    let good = challenge_message(FLAGS, &spec_info());
    for cut in 0..good.len() {
        let _ = Challenge::parse(&good[..cut]);
    }
    assert!(Challenge::parse(&good[..good.len() - 4]).is_err(), "an unclosed list");
    let mut wrong = good.clone();
    wrong[8] = 3;
    assert!(Challenge::parse(&wrong).is_err(), "not a challenge");
    let mut out_of_bounds = good.clone();
    out_of_bounds[44] = 0xff;
    assert!(Challenge::parse(&out_of_bounds).is_err());
    let many: Vec<(u16, Vec<u8>)> = (0..100).map(|i| (3, vec![i as u8])).collect();
    assert!(Challenge::parse(&challenge_message(FLAGS, &many)).is_err(), "too many attributes");
    assert!(matches!(Challenge::parse(&vec![0; MAX_CHALLENGE + 1]), Err(Error::TooLarge { .. })));
    let oem = Challenge::parse(&challenge_message(FLAGS & !NEGOTIATE_UNICODE, &[])).unwrap();
    let id = spec_identity();
    assert!(authenticate(&oem, &Answer { identity: &id, client_challenge: [0; 8], now: 0, binding: None }).is_err());
    // Random bytes never panic.
    let mut x = 0x1234_5678u32;
    for _ in 0..2000 {
        let mut m = good.clone();
        for _ in 0..4 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let i = (x as usize >> 8) % m.len();
            m[i] = (x >> 3) as u8;
        }
        if let Ok(c) = Challenge::parse(&m) {
            let _ = authenticate(&c, &Answer { identity: &id, client_challenge: [0; 8], now: 0, binding: None });
        }
    }
}

#[test]
fn the_negotiate_message_says_nothing_about_this_computer() {
    let m = negotiate();
    assert_eq!(m.len(), 32);
    assert_eq!(&m[..8], SIGNATURE);
    assert_eq!(u32_at(&m, 8), Some(1));
    assert_eq!(u32_at(&m, 12), Some(FLAGS));
    assert!(m[16..].iter().all(|b| *b == 0));
    assert!(format!("{:?}", spec_identity()).contains("User") && !format!("{:?}", spec_identity()).contains("Password"));
}

#[test]
fn the_certificate_hash_follows_its_signature() {
    // A certificate shell: SEQUENCE { tbs SEQUENCE {}, SEQUENCE { OID }, BIT STRING }.
    let cert = |oid: &[u8]| {
        let mut alg = vec![0x06, oid.len() as u8];
        alg.extend_from_slice(oid);
        let mut body = vec![0x30, 0x00, 0x30, alg.len() as u8];
        body.extend_from_slice(&alg);
        body.extend_from_slice(&[0x03, 0x01, 0x00]);
        let mut c = vec![0x30, 0x81, body.len() as u8];
        c.extend_from_slice(&body);
        c
    };
    let sha256_rsa = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
    let sha384_rsa = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
    let sha512_rsa = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
    let sha384_ec = [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
    let sha512_ec = [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];
    let sha1_rsa = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x05];
    assert_eq!(cert_hash(&cert(&sha256_rsa)), CertHash::Sha256);
    assert_eq!(cert_hash(&cert(&sha384_rsa)), CertHash::Sha384);
    assert_eq!(cert_hash(&cert(&sha512_rsa)), CertHash::Sha512);
    assert_eq!(cert_hash(&cert(&sha384_ec)), CertHash::Sha384);
    assert_eq!(cert_hash(&cert(&sha512_ec)), CertHash::Sha512);
    assert_eq!(cert_hash(&cert(&sha1_rsa)), CertHash::Sha256, "SHA-1 is raised to SHA-256");
    let c = cert(&sha384_ec);
    for cut in 0..c.len() {
        assert_eq!(cert_hash(&c[..cut]), CertHash::Sha256, "cut at {cut}");
    }
    assert_eq!(cert_hash(&[0x30, 0x85, 1, 1, 1, 1, 1]), CertHash::Sha256);
    let mut not_an_oid = cert(&sha384_ec);
    not_an_oid[7] = 0x04;
    assert_eq!(cert_hash(&not_an_oid), CertHash::Sha256, "the algorithm must be an OID");
}

#[test]
fn the_challenge_is_found_among_the_offers() {
    let resp = |values: &[&str]| crate::Response {
        status: 401,
        headers: values.iter().map(|v| ("www-authenticate".to_string(), v.to_string())).collect(),
        body: Vec::new(),
        close: false,
    };
    assert!(offered(&resp(&["Negotiate", "NTLM"])));
    assert!(offered(&resp(&["Negotiate, ntlm"])));
    assert!(!offered(&resp(&["Basic realm=\"mail\""])));
    assert!(!offered(&resp(&["NTLMX"])));
    assert_eq!(challenge_in(&resp(&["Negotiate", "NTLM TlRMTQ=="])), Some(b"NTLM".to_vec()));
    assert_eq!(challenge_in(&resp(&["NTLM"])), None, "an offer is not a challenge");
    assert_eq!(challenge_in(&resp(&["Basic TlRMTQ=="])), None);
}
