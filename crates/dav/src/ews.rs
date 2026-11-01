//! Exchange Web Services (EWS): the SOAP API of Exchange Server run by an organisation itself
//! (on-premises), at an address like `https://mail.example.org/EWS/Exchange.asmx`. Requests and
//! replies are the types of Thunderbird's `ews` crate (MPL-2.0, used unmodified); this module
//! sends them over [`Http`] and turns EWS's errors into [`Error`]s.
//!
//! - [`Ews::call`]: one operation, its response messages, the first error one carries as an
//!   [`Error`]. [`Ews::call_each`] keeps each message's own result, for requests about several
//!   items at once.
//! - A refused sign-in is [`Error::Auth`]; Exchange saying it is busy (HTTP 503, or
//!   `ErrorServerBusy`) is [`Error::Busy`]; an item or folder that is not there is
//!   [`Error::Status`] 404, as over Graph.
//! - A reply is checked to be a SOAP envelope with a header (or a fault) before the `ews` crate
//!   reads it: that crate assumes the header is there.

use ::ews::response::{ResponseCode, ResponseError};
use ::ews::server_version::ExchangeServerVersion;
use ::ews::soap::{Envelope, Header};
use ::ews::{Operation, OperationResponse, ResponseClass};

use crate::caldav::Http;
use crate::xml::Element;
use crate::{Error, Result, Url};

/// The SOAP 1.1 envelope namespace.
const SOAP_NS: &str = "http://schemas.xmlsoap.org/soap/envelope/";
/// Longest error text kept from a reply.
const MAX_DETAIL: usize = 300;

/// The messages an operation's response holds.
pub type Messages<Op> = <<Op as Operation>::Response as OperationResponse>::Message;

/// Requests to one EWS address over `http` (a [`crate::Client`] signed in with the account's
/// name and password).
pub struct Ews<H> {
    http: H,
    path: String,
}

impl<H: Http> Ews<H> {
    pub fn new(http: H, url: &Url) -> Self {
        Ews { http, path: url.path.clone() }
    }

    /// The connection the requests go over.
    pub fn http(&self) -> &H {
        &self.http
    }

    pub fn http_mut(&mut self) -> &mut H {
        &mut self.http
    }

    /// Send `op`; its response messages, or the first error among them.
    pub fn call<Op: Operation>(&mut self, op: Op) -> Result<Vec<Messages<Op>>> {
        self.call_each(op)?.into_iter().collect()
    }

    /// Send `op`; each response message with its own result (one per item asked about).
    pub fn call_each<Op: Operation>(&mut self, op: Op) -> Result<Vec<Result<Messages<Op>>>> {
        let envelope = Envelope { headers: vec![Header::RequestServerVersion { version: ExchangeServerVersion::Exchange2013_SP1 }], body: op };
        let body = envelope.as_xml_document().map_err(|e| Error::Invalid(format!("an EWS request that does not write: {e}")))?;
        let resp = self.post(&body)?;
        soap_checked(&resp.body)
            .map_err(|e| if resp.status == 500 { Error::Status { status: 500, what: "the Exchange server failed".into() } } else { e })?;
        let read = Envelope::<Op::Response>::from_xml_document(&resp.body);
        let envelope = match read {
            Ok(e) => e,
            Err(::ews::Error::RequestFault(fault)) => {
                let code = fault.detail.as_ref().and_then(|d| d.response_code);
                if code.is_some_and(busy) {
                    return Err(Error::Busy { retry_after: None });
                }
                return Err(Error::Status { status: 500, what: clip(&fault.faultstring) });
            }
            Err(e) => return Err(Error::Protocol(format!("an EWS reply that does not read: {}", clip(&e.to_string())))),
        };
        Ok(envelope
            .body
            .into_response_messages()
            .into_iter()
            .map(|m| match m {
                ResponseClass::Success(m) | ResponseClass::Warning(m) => Ok(m),
                ResponseClass::Error(e) => Err(response_error(&e)),
            })
            .collect())
    }
}

impl<H: Http> Ews<H> {
    /// Post a SOAP request; refusals and throttling as errors.
    fn post(&mut self, body: &[u8]) -> Result<crate::Response> {
        let headers = [("Content-Type", "text/xml; charset=utf-8"), ("Accept", "text/xml")];
        let resp = self.http.send("POST", &self.path, &headers, body)?;
        match resp.status {
            401 => Err(Error::Auth(401)),
            429 | 503 => Err(Error::Busy { retry_after: resp.header("retry-after").and_then(|v| v.trim().parse().ok()) }),
            // A SOAP fault comes as 500; anything else is not EWS speaking.
            200 | 500 => Ok(resp),
            status => Err(Error::Status { status, what: "not an EWS reply".into() }),
        }
    }

    /// Send an operation the `ews` crate cannot write: `op` is its XML, with the messages
    /// namespace as default and `t:` for the types one (values escaped by the caller). Each
    /// response message comes back as an element, or as the error it carries.
    pub fn call_raw(&mut self, op: &str) -> Result<Vec<Result<Element>>> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:soap="{SOAP_NS}" xmlns:t="{TYPES_NS}"><soap:Header><t:RequestServerVersion Version="Exchange2013_SP1"/></soap:Header><soap:Body>{op}</soap:Body></soap:Envelope>"#
        );
        let resp = self.post(body.as_bytes())?;
        let failed = |e: Error| if resp.status == 500 { Error::Status { status: 500, what: "the Exchange server failed".into() } } else { e };
        let root = crate::xml::parse(&resp.body).map_err(failed)?;
        let body = root
            .child(SOAP_NS, "Body")
            .filter(|_| root.is(SOAP_NS, "Envelope"))
            .ok_or_else(|| failed(Error::Protocol("the EWS reply is not a SOAP envelope".into())))?;
        if let Some(fault) = body.child(SOAP_NS, "Fault") {
            let code = fault.child("", "detail").and_then(|d| d.child(ERRORS_NS, "ResponseCode")).map_or("", |c| c.text.trim());
            if busy_code(code) {
                return Err(Error::Busy { retry_after: None });
            }
            return Err(Error::Status { status: 500, what: clip(fault.child_text("", "faultstring").unwrap_or("a SOAP fault")) });
        }
        let messages = body
            .children
            .first()
            .and_then(|r| r.child(MESSAGES_NS, "ResponseMessages"))
            .ok_or_else(|| failed(Error::Protocol("an EWS reply without response messages".into())))?;
        Ok(messages
            .children
            .iter()
            .map(|m| match m.attr("ResponseClass") {
                Some("Success" | "Warning") => Ok(m.clone()),
                _ => Err(code_error(
                    m.child_text(MESSAGES_NS, "ResponseCode").unwrap_or("ErrorUnknown"),
                    m.child_text(MESSAGES_NS, "MessageText").unwrap_or(""),
                )),
            })
            .collect())
    }
}

/// The EWS namespaces.
pub const MESSAGES_NS: &str = "http://schemas.microsoft.com/exchange/services/2006/messages";
pub const TYPES_NS: &str = "http://schemas.microsoft.com/exchange/services/2006/types";
const ERRORS_NS: &str = "http://schemas.microsoft.com/exchange/services/2006/errors";

fn busy_code(code: &str) -> bool {
    matches!(code, "ErrorServerBusy" | "ErrorTimeoutExpired" | "ErrorInternalServerTransientError")
}

/// The error a response code (`ErrorItemNotFound`) stands for.
fn code_error(code: &str, text: &str) -> Error {
    let what = clip(&format!("{code}: {text}"));
    match code {
        "ErrorItemNotFound" | "ErrorFolderNotFound" => Error::Status { status: 404, what },
        "ErrorAccessDenied" => Error::Status { status: 403, what },
        code if busy_code(code) => Error::Busy { retry_after: None },
        _ => Error::Status { status: 400, what },
    }
}

/// Whether Exchange is throttling or briefly unavailable.
fn busy(code: ResponseCode) -> bool {
    matches!(code, ResponseCode::ErrorServerBusy | ResponseCode::ErrorTimeoutExpired | ResponseCode::ErrorInternalServerTransientError)
}

fn clip(s: &str) -> String {
    s.trim().chars().take(MAX_DETAIL).collect()
}

/// The error an EWS response message stands for.
pub fn response_error(e: &ResponseError) -> Error {
    // The code's name is the one EWS sends.
    code_error(&format!("{:?}", e.response_code), &e.message_text)
}

/// `body` is a SOAP envelope with a header, or with a fault in its body.
fn soap_checked(body: &[u8]) -> Result<()> {
    let root = crate::xml::parse(body)?;
    if !root.is(SOAP_NS, "Envelope") {
        return Err(Error::Protocol("the EWS reply is not a SOAP envelope".into()));
    }
    let fault = root.child(SOAP_NS, "Body").and_then(|b| b.child(SOAP_NS, "Fault")).is_some();
    if root.child(SOAP_NS, "Header").is_none() && !fault {
        return Err(Error::Protocol("the EWS reply has no SOAP header".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
