use super::*;
use crate::Response;
use ::ews::get_folder::GetFolder;
use ::ews::{BaseFolderId, BaseShape, Folder, FolderShape};
use std::collections::VecDeque;

/// Method, path, headers and body of a request.
type Asked = (String, String, Vec<(String, String)>, String);

#[derive(Default)]
struct Queue {
    replies: VecDeque<Response>,
    asked: Vec<Asked>,
}

impl Http for Queue {
    fn send(&mut self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
        let headers = headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
        self.asked.push((method.to_string(), path.to_string(), headers, String::from_utf8_lossy(body).into_owned()));
        self.replies.pop_front().ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "no more")))
    }
}

fn reply(status: u16, body: &str) -> Response {
    Response { status, headers: Vec::new(), body: body.as_bytes().to_vec(), close: false }
}

fn ews(replies: Vec<Response>) -> Ews<Queue> {
    let url = Url::parse("https://mail.contoso.es/EWS/Exchange.asmx").unwrap();
    Ews::new(Queue { replies: replies.into(), asked: Vec::new() }, &url)
}

fn get_folder() -> GetFolder {
    GetFolder {
        folder_shape: FolderShape { base_shape: BaseShape::IdOnly },
        folder_ids: vec![BaseFolderId::new_distinguished("inbox"), BaseFolderId::new_distinguished("archive")],
    }
}

/// A reply as Exchange 2016 sends one, in `body`.
pub(crate) fn envelope(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
  <s:Header>
    <h:ServerVersionInfo MajorVersion="15" MinorVersion="1" MajorBuildNumber="2507" MinorBuildNumber="6" Version="V2017_07_11" xmlns:h="http://schemas.microsoft.com/exchange/services/2006/types" xmlns="http://schemas.microsoft.com/exchange/services/2006/types"/>
  </s:Header>
  <s:Body xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">{body}</s:Body>
</s:Envelope>"#
    )
}

const FOLDERS: &str = r#"<m:GetFolderResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <m:ResponseMessages>
    <m:GetFolderResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Folders><t:Folder><t:FolderId Id="AAMkIN" ChangeKey="AQAA"/></t:Folder></m:Folders>
    </m:GetFolderResponseMessage>
    <m:GetFolderResponseMessage ResponseClass="Error">
      <m:MessageText>The specified folder could not be found in the store.</m:MessageText>
      <m:ResponseCode>ErrorFolderNotFound</m:ResponseCode>
      <m:DescriptiveLinkKey>0</m:DescriptiveLinkKey>
    </m:GetFolderResponseMessage>
  </m:ResponseMessages>
</m:GetFolderResponse>"#;

#[test]
fn an_operation_goes_as_soap_and_each_message_keeps_its_result() {
    let mut e = ews(vec![reply(200, &envelope(FOLDERS))]);
    let each = e.call_each(get_folder()).unwrap();
    assert_eq!(each.len(), 2);
    match &each[0] {
        Ok(m) => match m.folders.inner.first() {
            Some(Folder::Folder { folder_id: Some(id), .. }) => assert_eq!(id.id, "AAMkIN"),
            other => panic!("{other:?}"),
        },
        Err(e) => panic!("{e}"),
    }
    assert!(matches!(&each[1], Err(Error::Status { status: 404, what }) if what.contains("ErrorFolderNotFound")), "{:?}", each[1]);
    let (method, path, headers, body) = &e.http().asked[0];
    assert_eq!((method.as_str(), path.as_str()), ("POST", "/EWS/Exchange.asmx"));
    assert!(headers.iter().any(|(n, v)| n == "Content-Type" && v.starts_with("text/xml")), "{headers:?}");
    assert!(body.contains("RequestServerVersion") && body.contains("Exchange2013_SP1"), "{body}");
    assert!(body.contains("GetFolder") && body.contains(r#"Id="inbox""#), "{body}");
    // `call` stops at the first error.
    let mut e = ews(vec![reply(200, &envelope(FOLDERS))]);
    assert!(matches!(e.call(get_folder()), Err(Error::Status { status: 404, .. })));
}

#[test]
fn refusals_faults_and_odd_replies_are_errors_not_crashes() {
    let fault = |code: &str| {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault>
  <faultcode xmlns:a="http://schemas.microsoft.com/exchange/services/2006/types">a:{code}</faultcode>
  <faultstring xml:lang="en-US">The server cannot service this request right now.</faultstring>
  <detail><e:ResponseCode xmlns:e="http://schemas.microsoft.com/exchange/services/2006/errors">{code}</e:ResponseCode><e:Message xmlns:e="http://schemas.microsoft.com/exchange/services/2006/errors">Try again later.</e:Message></detail>
</s:Fault></s:Body></s:Envelope>"#
        )
    };
    let no_header = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body>"#
        .to_string()
        + FOLDERS
        + "</s:Body></s:Envelope>";
    let mut e = ews(vec![
        reply(401, ""),
        reply(503, ""),
        reply(500, &fault("ErrorServerBusy")),
        reply(500, &fault("ErrorSchemaValidation")),
        reply(200, &no_header),
        reply(200, "<html>a sign-in page</html>"),
        reply(500, "Internal Server Error"),
        reply(302, ""),
    ]);
    assert!(matches!(e.call(get_folder()), Err(Error::Auth(401))));
    assert!(matches!(e.call(get_folder()), Err(Error::Busy { .. })));
    assert!(matches!(e.call(get_folder()), Err(Error::Busy { .. })), "Exchange throttling");
    assert!(matches!(e.call(get_folder()), Err(Error::Status { status: 500, what }) if what.contains("cannot service")));
    // The `ews` crate would stop the program on a reply with no SOAP header.
    assert!(matches!(e.call(get_folder()), Err(Error::Protocol(_))));
    assert!(matches!(e.call(get_folder()), Err(Error::Protocol(_))));
    assert!(matches!(e.call(get_folder()), Err(Error::Status { status: 500, .. })));
    assert!(matches!(e.call(get_folder()), Err(Error::Status { status: 302, .. })));
}

#[test]
fn a_hand_written_operation_reads_its_messages_as_elements() {
    let created = r#"<m:CreateItemResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"><m:ResponseMessages>
      <m:CreateItemResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:Items><t:CalendarItem><t:ItemId Id="CAL1" ChangeKey="ck1"/></t:CalendarItem></m:Items></m:CreateItemResponseMessage>
      <m:CreateItemResponseMessage ResponseClass="Error"><m:MessageText>No such folder.</m:MessageText><m:ResponseCode>ErrorFolderNotFound</m:ResponseCode></m:CreateItemResponseMessage>
    </m:ResponseMessages></m:CreateItemResponse>"#;
    let busy = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault><faultcode>a:ErrorServerBusy</faultcode><faultstring>Busy.</faultstring><detail><e:ResponseCode xmlns:e="http://schemas.microsoft.com/exchange/services/2006/errors">ErrorServerBusy</e:ResponseCode></detail></s:Fault></s:Body></s:Envelope>"#;
    let mut e = ews(vec![reply(200, &envelope(created)), reply(500, busy), reply(200, "<html/>"), reply(500, "oops")]);
    let each = e.call_raw("<CreateItem/>").unwrap();
    let id = each[0]
        .as_ref()
        .unwrap()
        .child(MESSAGES_NS, "Items")
        .and_then(|i| i.children.first())
        .and_then(|c| c.child(TYPES_NS, "ItemId"))
        .and_then(|i| i.attr("Id"));
    assert_eq!(id, Some("CAL1"));
    assert!(matches!(&each[1], Err(Error::Status { status: 404, .. })), "{:?}", each[1]);
    let (_, _, _, body) = &e.http().asked[0];
    assert!(body.contains("<soap:Body><CreateItem/></soap:Body>") && body.contains("Exchange2013_SP1"), "{body}");
    assert!(matches!(e.call_raw("<CreateItem/>"), Err(Error::Busy { .. })));
    assert!(matches!(e.call_raw("<CreateItem/>"), Err(Error::Protocol(_))));
    assert!(matches!(e.call_raw("<CreateItem/>"), Err(Error::Status { status: 500, .. })));
}
