//! Pictures on the web in an HTML message (Tinta), downloaded only when the person asks
//! (`message.loadPictures`). The request tells the picture's server nothing but its address: no
//! cookies, no referrer, the same plain user agent for everyone, and no account's sign-in.
//!
//! - Only HTTPS: an `http:` address is asked for over `https:` (most servers answer both), so
//!   nobody on the way sees which message was opened.
//! - At most [`MAX_REDIRECTS`] redirects, each to HTTPS.
//! - Never a name or address on this computer or its local network (`localhost`, `192.168.…`,
//!   `fe80::…`, a bare host name): a message cannot make Correo knock on a router or a printer.
//! - At most [`MAX_PICTURE_BYTES`]; the front end decodes it with limits of its own.

use std::net::IpAddr;

/// Largest picture downloaded.
pub const MAX_PICTURE_BYTES: usize = 10 * 1024 * 1024;
/// Most redirects followed for one picture.
pub const MAX_REDIRECTS: usize = 3;

/// One GET: status, `Location`, body.
pub trait Get {
    fn get(&mut self, url: &str) -> Result<(u16, Option<String>, Vec<u8>), String>;
}

/// `url` made an `https:` address, or why it can't be asked for.
pub fn https_address(url: &str) -> Result<String, String> {
    let url = url.trim();
    let lower = url.get(..8).unwrap_or("").to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        url.get(8..).unwrap_or("")
    } else if lower.starts_with("http://") {
        url.get(7..).unwrap_or("")
    } else {
        return Err(format!("{url:?} is not a web address"));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("a picture address with a user name is not downloaded".into());
    }
    let host = host_of(authority);
    if is_local(&host) {
        return Err(format!("{host} is on this computer or its local network"));
    }
    Ok(format!("https://{rest}"))
}

/// The host of `host[:port]` or `[v6]:port`, lowercased.
fn host_of(authority: &str) -> String {
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// A name or address that is this computer or its local network.
pub fn is_local(host: &str) -> bool {
    if host.is_empty() || host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host.ends_with(".internal") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let [a, b, ..] = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
        }
        Ok(IpAddr::V6(ip)) => {
            let first = ip.segments().first().copied().unwrap_or(0);
            ip.is_loopback()
                || ip.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || ip.to_ipv4_mapped().is_some_and(|v4| is_local(&v4.to_string()))
        }
        // A bare name without a dot is a name on the local network.
        Err(_) => !host.contains('.'),
    }
}

/// Download the picture at `url` through `get`.
pub fn fetch_with(get: &mut dyn Get, url: &str) -> Result<Vec<u8>, String> {
    let mut url = https_address(url)?;
    for _ in 0..=MAX_REDIRECTS {
        let (status, location, body) = get.get(&url)?;
        match status {
            200 => {
                if body.len() > MAX_PICTURE_BYTES {
                    return Err("the picture is too large".into());
                }
                return Ok(body);
            }
            301 | 302 | 303 | 307 | 308 => {
                let next = location.ok_or_else(|| "a redirect without a Location".to_string())?;
                url = if next.starts_with("//") {
                    https_address(&format!("https:{next}"))?
                } else if next.starts_with('/') {
                    let origin = url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                    format!("{origin}{next}")
                } else {
                    https_address(&next)?
                };
            }
            s => return Err(format!("the picture's server answered HTTP {s}")),
        }
    }
    Err("too many redirects".into())
}

struct Net<'a> {
    tls: &'a crate::Tls,
}

impl Get for Net<'_> {
    fn get(&mut self, url: &str) -> Result<(u16, Option<String>, Vec<u8>), String> {
        let url = crate::dav::Url::parse(url).map_err(|e| e.to_string())?;
        let mut client = crate::dav_anonymous(&url, self.tls);
        let r = client.send("GET", &url.path, &[("Accept", "image/*")], b"").map_err(|e| e.to_string())?;
        Ok((r.status, r.header("location").map(str::to_string), r.body))
    }
}

/// Download the picture at `url` over the network.
pub fn fetch(url: &str, tls: &crate::Tls) -> Result<Vec<u8>, String> {
    fetch_with(&mut Net { tls }, url)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        answers: Vec<(u16, Option<&'static str>, Vec<u8>)>,
        asked: Vec<String>,
    }

    impl Get for Fake {
        fn get(&mut self, url: &str) -> Result<(u16, Option<String>, Vec<u8>), String> {
            self.asked.push(url.to_string());
            if self.answers.is_empty() {
                return Err("no more answers".into());
            }
            let (s, l, b) = self.answers.remove(0);
            Ok((s, l.map(str::to_string), b))
        }
    }

    #[test]
    fn pictures_come_over_https_and_follow_safe_redirects() {
        let mut fake = Fake { answers: vec![(302, Some("/img/b.png"), Vec::new()), (200, None, b"PNG".to_vec())], asked: Vec::new() };
        assert_eq!(fetch_with(&mut fake, "http://cdn.shop.example/a.png?u=1").unwrap(), b"PNG");
        assert_eq!(fake.asked, ["https://cdn.shop.example/a.png?u=1", "https://cdn.shop.example/img/b.png"], "http is asked over https");

        let mut fake = Fake { answers: vec![(301, Some("http://192.168.1.1/x"), Vec::new())], asked: Vec::new() };
        assert!(fetch_with(&mut fake, "https://cdn.example/a").is_err(), "a redirect into the local network");

        let loops = (0..=MAX_REDIRECTS).map(|_| (302, Some("/again"), Vec::new())).collect();
        let mut fake = Fake { answers: loops, asked: Vec::new() };
        assert_eq!(fetch_with(&mut fake, "https://cdn.example/a").unwrap_err(), "too many redirects");

        let mut fake = Fake { answers: vec![(200, None, vec![0; MAX_PICTURE_BYTES + 1])], asked: Vec::new() };
        assert!(fetch_with(&mut fake, "https://cdn.example/a").is_err(), "too large");
        let mut fake = Fake { answers: vec![(404, None, Vec::new())], asked: Vec::new() };
        assert!(fetch_with(&mut fake, "https://cdn.example/a").is_err());
    }

    #[test]
    fn nothing_on_this_computer_or_its_network_is_asked_for() {
        for url in [
            "https://localhost/a.png",
            "https://printer/a.png",
            "http://127.0.0.1:8080/a",
            "https://10.0.0.5/a",
            "https://192.168.1.1/a",
            "https://169.254.169.254/latest",
            "https://[::1]/a",
            "https://[fe80::1]/a",
            "https://[fd00::1]/a",
            "https://[::ffff:192.168.0.1]/a",
            "https://nas.local/a",
            "https://user@cdn.example/a",
            "ftp://cdn.example/a",
            "javascript:alert(1)",
        ] {
            assert!(https_address(url).is_err(), "{url}");
        }
        assert_eq!(https_address("https://CDN.example:8443/a").unwrap(), "https://CDN.example:8443/a");
        assert!(https_address("https://8.8.8.8/a").is_ok());
    }
}
