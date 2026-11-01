//! Exchange Autodiscover (the plain-XML form, published as MS-OXDSCLI): where an address's
//! Exchange Web Services live. The client posts a small request naming the address, signed in
//! with the account's name and password, and reads the EWS address from the answer, or where to
//! ask next.
//!
//! It is asked, over HTTPS only, in order:
//! 1. `https://<domain>/autodiscover/autodiscover.xml`;
//! 2. `https://autodiscover.<domain>/autodiscover/autodiscover.xml`.
//!
//! The DNS `SRV` record and the plain-HTTP redirect that Outlook also tries are not used.

use crate::xml::{self, Element};
use crate::{Error, Result};

/// The namespaces of the answer.
const RESPONSE_NS: &str = "http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006";
const OUTLOOK_NS: &str = "http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a";
/// Largest answer read: 256 KiB (real ones are a few KiB).
pub const MAX_ANSWER_BYTES: usize = 256 * 1024;
/// Longest address kept from an answer.
const MAX_URL: usize = 2048;

/// What an Autodiscover answer says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The account's EWS address.
    Ews(String),
    /// Ask again for this address instead.
    RedirectAddr(String),
    /// Ask this Autodiscover address instead (`https://` only).
    RedirectUrl(String),
}

/// Where to ask about `email`, in order (see the module notes). Empty for an address that is not
/// one.
pub fn urls(email: &str) -> Vec<String> {
    let Some((_, domain)) = crate::autoconfig::split(email) else { return Vec::new() };
    vec![format!("https://{domain}/autodiscover/autodiscover.xml"), format!("https://autodiscover.{domain}/autodiscover/autodiscover.xml")]
}

/// The request body for `email`.
pub fn request(email: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006"><Request><EMailAddress>{}</EMailAddress><AcceptableResponseSchema>{OUTLOOK_NS}</AcceptableResponseSchema></Request></Autodiscover>"#,
        xml::escape(email.trim())
    )
    .into_bytes()
}

/// An `https://` address, or `None`.
fn https(text: &str) -> Option<String> {
    let t = text.trim();
    (t.len() <= MAX_URL && t.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) && crate::Url::parse(t).is_ok()).then(|| t.to_string())
}

/// Read an Autodiscover answer.
pub fn parse(body: &[u8]) -> Result<Answer> {
    if body.len() > MAX_ANSWER_BYTES {
        return Err(Error::TooLarge { what: "the Autodiscover answer", limit: MAX_ANSWER_BYTES });
    }
    let root = xml::parse(body)?;
    if !root.is(RESPONSE_NS, "Autodiscover") {
        return Err(Error::Protocol("not an Autodiscover answer".into()));
    }
    let response = root
        .child(OUTLOOK_NS, "Response")
        .or_else(|| root.child(RESPONSE_NS, "Response"))
        .ok_or_else(|| Error::Protocol("an Autodiscover answer without a response".into()))?;
    let error = response.child(OUTLOOK_NS, "Error").or_else(|| response.child(RESPONSE_NS, "Error"));
    if let Some(e) = error {
        let text = |name: &str| e.child_text(OUTLOOK_NS, name).or_else(|| e.child_text(RESPONSE_NS, name)).unwrap_or("");
        let what: String = format!("{} {}", text("ErrorCode"), text("Message")).trim().chars().take(300).collect();
        return Err(Error::Status { status: 404, what: format!("Autodiscover: {what}") });
    }
    let account = response.child(OUTLOOK_NS, "Account").ok_or_else(|| Error::Protocol("an Autodiscover answer without an account".into()))?;
    match account.child_text(OUTLOOK_NS, "Action").unwrap_or("settings") {
        "redirectAddr" => {
            let to = account.child_text(OUTLOOK_NS, "RedirectAddr").filter(|a| crate::autoconfig::split(a).is_some());
            to.map(|a| Answer::RedirectAddr(a.to_string())).ok_or_else(|| Error::Protocol("an Autodiscover redirect without an address".into()))
        }
        "redirectUrl" => account
            .child_text(OUTLOOK_NS, "RedirectUrl")
            .and_then(https)
            .map(Answer::RedirectUrl)
            .ok_or_else(|| Error::Protocol("an Autodiscover redirect without an https:// address".into())),
        _ => ews_url(account)
            .map(Answer::Ews)
            .ok_or_else(|| Error::Status { status: 404, what: "Autodiscover names no Exchange Web Services address".into() }),
    }
}

/// The EWS address among the account's protocols: the one for outside the organisation's network
/// (`EXPR`) first, then the inside one (`EXCH`), then any.
fn ews_url(account: &Element) -> Option<String> {
    let protocols: Vec<&Element> = account.children_named(OUTLOOK_NS, "Protocol").collect();
    let of = |kind: Option<&str>| {
        protocols
            .iter()
            .filter(|p| kind.is_none_or(|k| p.child_text(OUTLOOK_NS, "Type") == Some(k)))
            .find_map(|p| p.child_text(OUTLOOK_NS, "EwsUrl").and_then(https))
    };
    of(Some("EXPR")).or_else(|| of(Some("EXCH"))).or_else(|| of(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(account: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006">
  <Response xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a">
    <User><DisplayName>Ana Gil</DisplayName><EMailAddress>ana@contoso.es</EMailAddress></User>
    <Account>{account}</Account>
  </Response>
</Autodiscover>"#
        )
    }

    #[test]
    fn the_ews_address_comes_from_the_outside_protocol_first() {
        let both = answer(
            "<AccountType>email</AccountType><Action>settings</Action>\
             <Protocol><Type>EXCH</Type><Server>EX01.contoso.local</Server><EwsUrl>https://ex01.contoso.local/EWS/Exchange.asmx</EwsUrl></Protocol>\
             <Protocol><Type>EXPR</Type><Server>mail.contoso.es</Server><EwsUrl>https://mail.contoso.es/EWS/Exchange.asmx</EwsUrl></Protocol>\
             <Protocol><Type>WEB</Type></Protocol>",
        );
        assert_eq!(parse(both.as_bytes()).unwrap(), Answer::Ews("https://mail.contoso.es/EWS/Exchange.asmx".into()));
        let inside =
            answer("<Action>settings</Action><Protocol><Type>EXCH</Type><EwsUrl>https://ex01.contoso.local/EWS/Exchange.asmx</EwsUrl></Protocol>");
        assert_eq!(parse(inside.as_bytes()).unwrap(), Answer::Ews("https://ex01.contoso.local/EWS/Exchange.asmx".into()));
        // A plain-HTTP address is not taken.
        let plain =
            answer("<Action>settings</Action><Protocol><Type>EXPR</Type><EwsUrl>http://mail.contoso.es/EWS/Exchange.asmx</EwsUrl></Protocol>");
        assert!(matches!(parse(plain.as_bytes()), Err(Error::Status { status: 404, .. })));
    }

    #[test]
    fn redirects_and_errors_are_read_and_odd_answers_refused() {
        let addr = answer("<Action>redirectAddr</Action><RedirectAddr>ana.gil@contoso.com</RedirectAddr>");
        assert_eq!(parse(addr.as_bytes()).unwrap(), Answer::RedirectAddr("ana.gil@contoso.com".into()));
        let url = answer("<Action>redirectUrl</Action><RedirectUrl>https://autodiscover.contoso.com/autodiscover/autodiscover.xml</RedirectUrl>");
        assert_eq!(parse(url.as_bytes()).unwrap(), Answer::RedirectUrl("https://autodiscover.contoso.com/autodiscover/autodiscover.xml".into()));
        let plain = answer("<Action>redirectUrl</Action><RedirectUrl>http://evil.example/autodiscover.xml</RedirectUrl>");
        assert!(matches!(parse(plain.as_bytes()), Err(Error::Protocol(_))));
        let bad_addr = answer("<Action>redirectAddr</Action><RedirectAddr>not an address</RedirectAddr>");
        assert!(matches!(parse(bad_addr.as_bytes()), Err(Error::Protocol(_))));
        let error = r#"<?xml version="1.0" encoding="utf-8"?><Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006"><Response><Error Time="10:00:00" Id="1"><ErrorCode>500</ErrorCode><Message>The email address can't be found.</Message></Error></Response></Autodiscover>"#;
        assert!(matches!(parse(error.as_bytes()), Err(Error::Status { status: 404, what }) if what.contains("can't be found")));
        for odd in ["", "<html>sign in</html>", "<Autodiscover xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006\"/>"] {
            assert!(parse(odd.as_bytes()).is_err(), "{odd}");
        }
        assert!(matches!(parse(&vec![b' '; MAX_ANSWER_BYTES + 1]), Err(Error::TooLarge { .. })));
    }

    #[test]
    fn the_request_names_the_address_and_the_places_follow_the_domain() {
        let body = String::from_utf8(request(" ana<x>@contoso.es ")).unwrap();
        assert!(body.contains("<EMailAddress>ana&lt;x&gt;@contoso.es</EMailAddress>"), "{body}");
        assert!(body.contains("outlook/responseschema/2006a"));
        assert_eq!(
            urls("Ana@Contoso.ES"),
            ["https://contoso.es/autodiscover/autodiscover.xml", "https://autodiscover.contoso.es/autodiscover/autodiscover.xml"]
        );
        assert!(urls("not an address").is_empty());
    }
}
