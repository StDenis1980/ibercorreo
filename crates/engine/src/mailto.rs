//! `mailto:` links (RFC 6068), as the desktop passes them when Tinta Correo is the system's mail
//! app: `mailto:ana@example.org,luis@example.org?cc=…&subject=Hola&body=…`.
//!
//! Only the fields a new message has are taken: the addresses before `?` and the `to`, `cc`,
//! `bcc`, `subject` and `body` fields (names in any case). Everything else is left out on
//! purpose, above all `attach`/`attachment`, which some mail apps honour and a web page could use
//! to send a person's files away. A link never sends anything: it only opens a draft.

use ibercorreo_core::{Error, Result};

/// A longer link isn't one a person clicked (browsers stop at about 2 MB; this is plenty for a
/// prefilled body).
pub const MAX_URI_BYTES: usize = 256 * 1024;

/// The fields of a new message a `mailto:` link fills in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mailto {
    /// Comma-separated, as `message.compose` takes them.
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
}

/// Reads a `mailto:` link. The scheme is required (any case); percent-escapes are decoded as
/// UTF-8 (invalid sequences become U+FFFD), and `+` stays a plus, as RFC 6068 says.
pub fn parse(uri: &str) -> Result<Mailto> {
    if uri.len() > MAX_URI_BYTES {
        return Err(Error::invalid("uri", "the link is too long"));
    }
    let rest = uri.trim();
    let rest = match rest.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("mailto:") => rest.get(7..).unwrap_or(""),
        _ => return Err(Error::invalid("uri", "not a mailto: link")),
    };
    // A fragment is not part of the message.
    let rest = rest.split_once('#').map_or(rest, |(r, _)| r);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut m = Mailto::default();
    let mut to = vec![decode(path)];
    let (mut cc, mut bcc, mut body) = (Vec::new(), Vec::new(), Vec::new());
    for field in query.split('&') {
        let (name, value) = field.split_once('=').unwrap_or((field, ""));
        let value = decode(value);
        match name.to_ascii_lowercase().as_str() {
            "to" => to.push(value),
            "cc" => cc.push(value),
            "bcc" => bcc.push(value),
            "subject" if m.subject.is_empty() => m.subject = value,
            "body" => body.push(value),
            _ => {}
        }
    }
    let join = |parts: Vec<String>| parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", ");
    m.to = join(to);
    m.cc = join(cc);
    m.bcc = join(bcc);
    // Line breaks come as %0D%0A; the body keeps plain `\n`.
    m.body = body.join("\n").replace("\r\n", "\n");
    // A subject is one line.
    m.subject = m.subject.replace(['\r', '\n'], " ");
    Ok(m)
}

/// The local path of a `file://` URI (`file:///home/ana/cita.ics`, or with `localhost`), as the
/// desktop passes files to an app whose launcher takes URLs; `None` for anything else, a remote
/// host or a relative path. The path is percent-decoded bytes (paths need not be UTF-8).
pub fn file_uri_path(uri: &str) -> Option<Vec<u8>> {
    let rest = match uri.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("file://") => uri.get(7..)?,
        _ => return None,
    };
    let path = if rest.starts_with('/') {
        rest
    } else {
        let slash = rest.find('/')?;
        if !rest.get(..slash)?.eq_ignore_ascii_case("localhost") {
            return None;
        }
        rest.get(slash..)?
    };
    let path = path.split_once(['?', '#']).map_or(path, |(p, _)| p);
    Some(decode_bytes(path))
}

/// Percent-decoding; a `%` not followed by two hex digits is kept as it is.
fn decode(s: &str) -> String {
    String::from_utf8_lossy(&decode_bytes(s)).into_owned()
}

fn decode_bytes(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = |at: usize| bytes.get(at).and_then(|c| (*c as char).to_digit(16));
        match (b, hex(i + 1), hex(i + 2)) {
            (b'%', Some(hi), Some(lo)) => {
                // Two hex digits: at most 0xFF, so the cast is exact.
                out.push((hi * 16 + lo) as u8);
                i += 3;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_fields_of_a_new_message() {
        let m = parse("mailto:ana@example.org,luis@example.org?cc=marta%40example.org&BCC=x@example.org&subject=Factura%20n%C2%BA%207&body=Hola%2C%0D%0Aadjunto+la+factura.").unwrap();
        assert_eq!(m.to, "ana@example.org,luis@example.org");
        assert_eq!(m.cc, "marta@example.org");
        assert_eq!(m.bcc, "x@example.org");
        assert_eq!(m.subject, "Factura nº 7");
        assert_eq!(m.body, "Hola,\nadjunto+la+factura.");
    }

    #[test]
    fn to_fields_join_the_address_and_other_fields_are_left_out() {
        let m = parse("MAILTO:?to=ana@example.org&to=luis@example.org&attach=/home/ana/.ssh/id_rsa&attachment=x&in-reply-to=%3Cid%3E").unwrap();
        assert_eq!(m, Mailto { to: "ana@example.org, luis@example.org".into(), ..Default::default() });
        assert_eq!(parse("mailto:").unwrap(), Mailto::default());
        assert_eq!(parse("  mailto:ana@example.org#frag ").unwrap().to, "ana@example.org");
        // Only the first subject counts, and it stays one line.
        assert_eq!(parse("mailto:?subject=a%0Ab&subject=c").unwrap().subject, "a b");
    }

    #[test]
    fn file_uris_become_local_paths() {
        assert_eq!(file_uri_path("file:///home/ana/Cita%20m%C3%A9dica.ics").as_deref(), Some("/home/ana/Cita médica.ics".as_bytes()));
        assert_eq!(file_uri_path("FILE://localhost/tmp/a.vcf").as_deref(), Some(&b"/tmp/a.vcf"[..]));
        assert_eq!(file_uri_path("file:///tmp/%FF.ics").as_deref(), Some(&b"/tmp/\xff.ics"[..]));
        assert_eq!(file_uri_path("file://server/share/a.ics"), None);
        assert_eq!(file_uri_path("file://localhost"), None);
        assert_eq!(file_uri_path("/tmp/a.ics"), None);
        assert_eq!(file_uri_path("file:"), None);
        assert_eq!(file_uri_path("file://é/x"), None);
    }

    #[test]
    fn malformed_links_never_panic() {
        assert!(parse("http://example.org").is_err());
        assert!(parse("mail").is_err());
        assert!(parse("").is_err());
        assert!(parse(&format!("mailto:{}", "a".repeat(MAX_URI_BYTES))).is_err());
        assert_eq!(parse("mailto:%").unwrap().to, "%");
        assert_eq!(parse("mailto:%4").unwrap().to, "%4");
        assert_eq!(parse("mailto:%zz%41").unwrap().to, "%zzA");
        assert_eq!(parse("mailto:?subject=%FF%FE").unwrap().subject, "\u{fffd}\u{fffd}");
        assert_eq!(parse("mailto:é?&&=&subject").unwrap().to, "é");
        assert!(parse("méilto:x").is_err());
    }
}
