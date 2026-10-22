//! IMAP's modified UTF-7 for mailbox names (RFC 3501 §5.1.3): printable ASCII stands for itself
//! (`&` is written `&-`), everything else is UTF-16 in a modified base64 between `&` and `-`.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

fn value(b: u8) -> Option<u32> {
    ALPHABET.iter().position(|&a| a == b).and_then(|p| u32::try_from(p).ok())
}

/// Decode a mailbox name. Malformed shifts are kept as written (lossless for display) rather
/// than rejected: a server may not follow the RFC.
pub fn decode(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = String::with_capacity(name.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b != b'&' {
            // Copy the run up to the next `&` as text: a server with UTF8=ACCEPT sends raw UTF-8.
            let run_end = bytes.get(i..).and_then(|rest| rest.iter().position(|&c| c == b'&')).map_or(bytes.len(), |p| i + p);
            out.push_str(name.get(i..run_end).unwrap_or(""));
            i = run_end;
            continue;
        }
        let Some(end) = bytes.get(i + 1..).and_then(|rest| rest.iter().position(|&c| c == b'-')).map(|p| i + 1 + p) else {
            out.push_str(name.get(i..).unwrap_or(""));
            break;
        };
        let chunk = bytes.get(i + 1..end).unwrap_or(&[]);
        if chunk.is_empty() {
            out.push('&');
        } else if let Some(s) = decode_chunk(chunk) {
            out.push_str(&s);
        } else {
            out.push_str(name.get(i..=end).unwrap_or(""));
        }
        i = end + 1;
    }
    out
}

fn decode_chunk(chunk: &[u8]) -> Option<String> {
    let mut bits: u32 = 0;
    let mut nbits = 0;
    let mut units = Vec::new();
    for &c in chunk {
        bits = (bits << 6) | value(c)?;
        nbits += 6;
        if nbits >= 16 {
            nbits -= 16;
            units.push(u16::try_from((bits >> nbits) & 0xFFFF).ok()?);
            bits &= (1 << nbits) - 1;
        }
    }
    String::from_utf16(&units).ok()
}

/// Encode a mailbox name for a command.
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        out.push('&');
        let mut bits: u32 = 0;
        let mut nbits = 0;
        let sym = |v: u32| char::from(*ALPHABET.get((v & 63) as usize).unwrap_or(&b'A'));
        for &u in pending.iter() {
            bits = (bits << 16) | u32::from(u);
            nbits += 16;
            while nbits >= 6 {
                nbits -= 6;
                out.push(sym(bits >> nbits));
            }
            bits &= (1 << nbits) - 1;
        }
        if nbits > 0 {
            out.push(sym(bits << (6 - nbits)));
        }
        out.push('-');
        pending.clear();
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_examples_and_round_trips() {
        // RFC 3501 §5.1.3: "~peter/mail/台北/日本語"
        assert_eq!(decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-"), "~peter/mail/台北/日本語");
        assert_eq!(encode("~peter/mail/台北/日本語"), "~peter/mail/&U,BTFw-/&ZeVnLIqe-");
        for s in ["Вхідні", "Enviados & Recibidos", "Papelera/2026", "😀 emoji", "", "&"] {
            assert_eq!(decode(&encode(s)), s, "{s}");
        }
        assert_eq!(encode("A&B"), "A&-B");
    }

    #[test]
    fn malformed_input_is_kept() {
        for s in ["&", "&ZeVnLIqe", "&!!!-", "&2D3-", "a&-b&"] {
            let _ = decode(s);
        }
        assert_eq!(decode("&!!!-"), "&!!!-");
        assert_eq!(decode("Été&-Ω"), "Été&Ω");
    }
}
