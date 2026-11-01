use super::*;
use crate::outbox::Outcome;
use crate::server_mail::{UID_VALIDITY, with_bcc};
use crate::{Push, apply, state};
use ibercorreo_core::{AccountId, FolderRole, MessageId};
use ibercorreo_dav::{Response, Url};
use ibercorreo_store::Store;

const A: AccountId = AccountId(0);
const M: &str =
    r#"xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types""#;

struct Item {
    id: String,
    folder: String,
    read: bool,
    flagged: bool,
    received: String,
    mime: String,
}

/// An Exchange mailbox answering EWS operations, as Exchange 2016 words its replies. It hands
/// out at most two items per `FindItem` page, as a server with a low paging limit would.
#[derive(Default)]
struct Exchange {
    items: Vec<Item>,
    /// Operation names asked for, in order, with their request bodies.
    asked: Vec<(String, String)>,
    /// Operations answered with this HTTP status and body instead.
    fail: Vec<(&'static str, u16, String)>,
    /// A new id for a moved item.
    moves: u32,
    /// Items listed but deleted before they are read.
    vanished: Vec<&'static str>,
}

/// The value of the first `name="…"` attribute after `from` in `body`.
fn attr<'b>(body: &'b str, name: &str) -> Option<&'b str> {
    let at = body.find(&format!("{name}=\""))?;
    let rest = body.get(at + name.len() + 2..)?;
    rest.get(..rest.find('"')?)
}

/// Every `Id="…"` of an element named `element`.
fn ids<'b>(body: &'b str, element: &str) -> Vec<&'b str> {
    body.match_indices(&format!("{element} Id=\"")).filter_map(|(at, m)| body.get(at + m.len()..)).filter_map(|r| r.get(..r.find('"')?)).collect()
}

fn envelope(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Header><h:ServerVersionInfo MajorVersion="15" MinorVersion="1" xmlns:h="http://schemas.microsoft.com/exchange/services/2006/types"/></s:Header><s:Body>{body}</s:Body></s:Envelope>"#
    )
}

fn ok(op: &str, inner: &str) -> String {
    format!(
        r#"<m:{op}Response {M}><m:ResponseMessages><m:{op}ResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode>{inner}</m:{op}ResponseMessage></m:ResponseMessages></m:{op}Response>"#
    )
}

fn not_found(op: &str, code: &str) -> String {
    format!(
        r#"<m:{op}ResponseMessage ResponseClass="Error"><m:MessageText>Not found.</m:MessageText><m:ResponseCode>{code}</m:ResponseCode><m:DescriptiveLinkKey>0</m:DescriptiveLinkKey></m:{op}ResponseMessage>"#
    )
}

fn folder(kind: &str, id: &str, parent: &str, class: &str, name: &str) -> String {
    format!(
        r#"<t:Create><t:{kind}><t:FolderId Id="{id}" ChangeKey="k"/><t:ParentFolderId Id="{parent}" ChangeKey="k"/><t:FolderClass>{class}</t:FolderClass><t:DisplayName>{name}</t:DisplayName></t:{kind}></t:Create>"#
    )
}

impl Exchange {
    fn with(mut self, id: &str, folder: &str, read: bool, flagged: bool, received: &str) -> Self {
        let mime = format!("From: Ana <ana@contoso.es>\r\nSubject: subject {id}\r\nDate: Thu, 22 Oct 2026 09:00:00 +0000\r\n\r\nbody of {id}\r\n");
        self.items.push(Item { id: id.into(), folder: folder.into(), read, flagged, received: received.into(), mime });
        self
    }

    fn failing(mut self, op: &'static str, status: u16, body: &str) -> Self {
        self.fail.push((op, status, body.to_string()));
        self
    }

    fn asked(&self, op: &str) -> Vec<&str> {
        self.asked.iter().filter(|(o, _)| o == op).map(|(_, b)| b.as_str()).collect()
    }

    fn answer(&mut self, op: &str, body: &str) -> String {
        match op {
            // Drafts and Junk Email are missing in this mailbox.
            "GetFolder" => {
                let messages: String = ids(body, "DistinguishedFolderId")
                    .iter()
                    .map(|name| match *name {
                        "inbox" | "sentitems" | "deleteditems" | "outbox" => {
                            let id = name.get(..2).unwrap_or(name).to_uppercase();
                            format!(r#"<m:GetFolderResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:Folders><t:Folder><t:FolderId Id="{id}" ChangeKey="k"/></t:Folder></m:Folders></m:GetFolderResponseMessage>"#)
                        }
                        _ => not_found("GetFolder", "ErrorFolderNotFound"),
                    })
                    .collect();
                format!(r#"<m:GetFolderResponse {M}><m:ResponseMessages>{messages}</m:ResponseMessages></m:GetFolderResponse>"#)
            }
            // Two pages; the second holds a calendar and a folder inside Inbox.
            "SyncFolderHierarchy" => {
                let (state, last, changes) = if body.contains("SyncState>") {
                    let calendar = folder("CalendarFolder", "CA", "ROOT", "IPF.Appointment", "Calendario");
                    let notes = folder("Folder", "NO", "ROOT", "IPF.StickyNote", "Notas");
                    ("s2", true, format!("{calendar}{notes}{}", folder("Folder", "PR", "IN", "IPF.Note", "Proyectos")))
                } else {
                    let f = [("IN", "Bandeja de entrada"), ("SE", "Elementos enviados"), ("DE", "Elementos eliminados"), ("OU", "Bandeja de salida")];
                    ("s1", false, f.iter().map(|(id, name)| folder("Folder", id, "ROOT", "IPF.Note", name)).collect())
                };
                ok(
                    "SyncFolderHierarchy",
                    &format!(
                        "<m:SyncState>{state}</m:SyncState><m:IncludesLastFolderInRange>{last}</m:IncludesLastFolderInRange><m:Changes>{changes}</m:Changes>"
                    ),
                )
            }
            "FindItem" => {
                let parent = ids(body, "FolderId").first().map(|s| s.to_string()).unwrap_or_default();
                let offset: usize = attr(body, "Offset").and_then(|o| o.parse().ok()).unwrap_or(0);
                let all: Vec<&Item> = self.items.iter().filter(|i| i.folder == parent).collect();
                let page: String = all
                    .iter()
                    .skip(offset)
                    .take(2)
                    .map(|i| {
                        let status = if i.flagged { "Flagged" } else { "NotFlagged" };
                        format!(r#"<t:Message><t:ItemId Id="{}" ChangeKey="k"/><t:DateTimeReceived>{}</t:DateTimeReceived><t:IsRead>{}</t:IsRead><t:Flag><t:FlagStatus>{status}</t:FlagStatus></t:Flag></t:Message>"#, i.id, i.received, i.read)
                    })
                    .collect();
                let next = (offset + 2).min(all.len());
                let last = next >= all.len();
                ok(
                    "FindItem",
                    &format!(
                        r#"<m:RootFolder IndexedPagingOffset="{next}" TotalItemsInView="{}" IncludesLastItemInRange="{last}"><t:Items>{page}</t:Items></m:RootFolder>"#,
                        all.len()
                    ),
                )
            }
            "GetItem" => {
                let id = ids(body, "ItemId").first().copied().unwrap_or("");
                match self.items.iter().find(|i| i.id == id && !self.vanished.contains(&id)) {
                    Some(i) => ok(
                        "GetItem",
                        &format!(
                            r#"<m:Items><t:Message><t:MimeContent CharacterSet="UTF-8">{}</t:MimeContent><t:ItemId Id="{id}" ChangeKey="k"/></t:Message></m:Items>"#,
                            ibercorreo_mime::base64(i.mime.as_bytes())
                        ),
                    ),
                    None => format!(
                        r#"<m:GetItemResponse {M}><m:ResponseMessages>{}</m:ResponseMessages></m:GetItemResponse>"#,
                        not_found("GetItem", "ErrorItemNotFound")
                    ),
                }
            }
            "UpdateItem" => {
                let id = ids(body, "ItemId").first().copied().unwrap_or("").to_string();
                if let Some(i) = self.items.iter_mut().find(|i| i.id == id) {
                    i.read = body.contains("IsRead>true<");
                    i.flagged = body.contains(r#"<t:ExtendedFieldURI PropertyTag="0x1090" PropertyType="Integer"/><t:Value>2</t:Value>"#);
                }
                ok("UpdateItem", r#"<m:Items><t:Message><t:ItemId Id="x" ChangeKey="k"/></t:Message></m:Items>"#)
            }
            "MoveItem" => {
                let to = ids(body, "FolderId").first().copied().unwrap_or("").to_string();
                let id = ids(body, "ItemId").first().copied().unwrap_or("").to_string();
                self.moves += 1;
                let moved = self.moves;
                if let Some(i) = self.items.iter_mut().find(|i| i.id == id) {
                    i.folder = to;
                    i.id = format!("{id}-moved{moved}");
                }
                ok("MoveItem", "<m:Items/>")
            }
            "DeleteItem" => {
                let id = ids(body, "ItemId").first().copied().unwrap_or("").to_string();
                let before = self.items.len();
                self.items.retain(|i| i.id != id);
                if self.items.len() == before {
                    return format!(
                        r#"<m:DeleteItemResponse {M}><m:ResponseMessages>{}</m:ResponseMessages></m:DeleteItemResponse>"#,
                        not_found("DeleteItem", "ErrorItemNotFound")
                    );
                }
                ok("DeleteItem", "")
            }
            "CreateItem" => ok("CreateItem", "<m:Items/>"),
            _ => String::new(),
        }
    }
}

impl Http for Exchange {
    fn send(&mut self, method: &str, path: &str, _headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
        assert_eq!((method, path), ("POST", "/EWS/Exchange.asmx"));
        let body = String::from_utf8_lossy(body).into_owned();
        // The operation is the first element in the SOAP body.
        let after = body.split("Body>").nth(1).unwrap_or("");
        let op: String = after.trim_start_matches('<').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        self.asked.push((op.clone(), body.clone()));
        if let Some((_, status, reply)) = self.fail.iter().find(|(o, _, _)| *o == op) {
            return Ok(Response { status: *status, headers: Vec::new(), body: reply.clone().into_bytes(), close: false });
        }
        let reply = envelope(&self.answer(&op, &body));
        Ok(Response { status: 200, headers: Vec::new(), body: reply.into_bytes(), close: false })
    }
}

fn ews(x: Exchange) -> Ews<Exchange> {
    Ews::new(x, &Url::parse("https://mail.contoso.es/EWS/Exchange.asmx").unwrap())
}

fn find(store: &Store, subject: &str) -> MessageId {
    store.all_messages().find(|m| m.subject == subject).unwrap().id
}

/// An Inbox of three, listed by Exchange oldest-received first and over two pages.
fn mailbox() -> Exchange {
    Exchange::default().with("m1", "IN", false, false, "2026-10-20T08:00:00Z").with("m3", "IN", false, true, "2026-10-22T08:00:00Z").with(
        "m2",
        "IN",
        true,
        false,
        "2026-10-21T08:00:00Z",
    )
}

#[test]
fn mail_comes_over_ews_newest_first_and_changes_go_back() {
    let mut store = Store::new();
    let limits = Limits { new_per_folder: 2, ..Limits::default() };
    let mut e = ews(mailbox());
    let batch = run(&mut e, &state(&store, A), &limits).unwrap();
    assert!(batch.errors.is_empty(), "{:?}", batch.errors);
    let names: Vec<(&str, &str, Vec<String>)> = batch.mailboxes.iter().map(|m| (m.raw.as_str(), m.name.as_str(), m.attributes.clone())).collect();
    assert_eq!(
        names,
        [
            ("IN", "Bandeja de entrada", vec!["\\Inbox".to_string()]),
            ("PR", "Bandeja de entrada/Proyectos", vec![]),
            ("DE", "Elementos eliminados", vec!["\\Trash".to_string()]),
            ("SE", "Elementos enviados", vec!["\\Sent".to_string()]),
        ],
        "mail folders only, Exchange's Outbox left out, both hierarchy pages read"
    );
    let inbox = batch.folders.iter().find(|f| f.path == "IN").unwrap();
    assert_eq!(inbox.remaining, 1, "m1 waits for the next sync; the newest come first");
    let fetched: Vec<(u32, Option<&str>)> = inbox.new.iter().map(|n| (n.uid, n.id.as_deref())).collect();
    assert_eq!(fetched, [(1, Some("m2")), (2, Some("m3"))], "numbered oldest first, as UIDs grow");
    assert_eq!(e.http().asked("FindItem").iter().filter(|b| b.contains(r#"Id="IN""#)).count(), 2, "both pages of the Inbox");
    assert!(e.http().asked("GetItem").iter().all(|b| b.contains("IncludeMimeContent>true<")));

    let report = apply(&mut store, A, batch);
    assert_eq!((report.added, report.remaining, report.folders_added), (2, 1, 1));
    let ib = store.folder_by_role(A, FolderRole::Inbox).unwrap();
    assert_eq!(store.folder(ib).unwrap().remote.as_ref().map(|r| r.path.as_str()), Some("IN"), "the standard Inbox mirrors Exchange's");
    let trash = store.folder_by_role(A, FolderRole::Trash).unwrap();
    assert_eq!(store.folder(trash).unwrap().remote.as_ref().map(|r| r.path.as_str()), Some("DE"));
    let m3 = store.message(find(&store, "subject m3")).unwrap();
    assert_eq!((m3.seen, m3.flagged, m3.remote.as_ref().and_then(|r| r.id.as_deref())), (false, true, Some("m3")));
    assert!(store.message(find(&store, "subject m2")).unwrap().seen);

    // The next sync brings m1; then changes made here: m3 read and unflagged, m2 filed in
    // Proyectos, m1 deleted for good.
    let batch = run(&mut e, &state(&store, A), &limits).unwrap();
    assert_eq!(apply(&mut store, A, batch).added, 1);
    let projects = store.folder_by_path(A, "PR").unwrap();
    assert_eq!(store.folder(projects).unwrap().name, "Bandeja de entrada/Proyectos");
    let (m1, m2, m3) = (find(&store, "subject m1"), find(&store, "subject m2"), find(&store, "subject m3"));
    store.message_mut(m3).unwrap().seen = true;
    store.message_mut(m3).unwrap().flagged = false;
    store.move_message(m2, projects).unwrap();
    store.remove_message(m1).unwrap();
    let s = state(&store, A);
    assert_eq!(s.pushes.len(), 3);
    e.http_mut().asked.clear();
    let batch = run(&mut e, &s, &limits).unwrap();
    assert!(batch.pushed.iter().all(|(_, r)| r.is_ok()), "{:?}", batch.pushed);
    let update = e.http().asked("UpdateItem");
    assert!(
        update.len() == 1
            && update[0].contains(r#"ItemId Id="m3""#)
            && update[0].contains("IsRead>true<")
            && update[0].contains(r#"PropertyTag="0x1090" PropertyType="Integer"/><t:Value>0</t:Value>"#),
        "{update:?}"
    );
    assert!(!update[0].contains("<FlagStatus"), "a flag status outside the EWS types namespace fails schema validation");
    let moved = e.http().asked("MoveItem");
    assert!(moved.len() == 1 && moved[0].contains(r#"ItemId Id="m2""#) && moved[0].contains(r#"FolderId Id="PR""#), "{moved:?}");
    let deleted = e.http().asked("DeleteItem");
    assert!(deleted.len() == 1 && deleted[0].contains(r#"DeleteType="HardDelete""#) && deleted[0].contains(r#"ItemId Id="m1""#), "{deleted:?}");
    let report = apply(&mut store, A, batch);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(store.tombstones().is_empty());
    assert_eq!(store.messages_in(projects).len(), 1, "the moved message is in Proyectos once, under its new id");
    let ib_mail = store.messages_in(ib);
    assert_eq!(ib_mail.len(), 1);
    assert!(ib_mail.first().is_some_and(|m| m.seen && !m.flagged));
    let on_server: Vec<(&str, &str, bool, bool)> = e.http().items.iter().map(|i| (i.id.as_str(), i.folder.as_str(), i.read, i.flagged)).collect();
    assert_eq!(on_server, [("m3", "IN", true, false), ("m2-moved1", "PR", true, false)]);
}

#[test]
fn a_folder_that_fails_alone_is_reported_and_a_refused_sign_in_ends_the_sync() {
    let store = Store::new();
    let x = mailbox().failing(
        "FindItem",
        500,
        &envelope(r#"<s:Fault><faultcode>s:Client</faultcode><faultstring>The request failed schema validation.</faultstring></s:Fault>"#),
    );
    let batch = run(&mut ews(x), &state(&store, A), &Limits::default()).unwrap();
    assert_eq!(batch.errors.len(), 4, "every folder failed on its own: {:?}", batch.errors);
    assert!(batch.errors[0].starts_with("Bandeja de entrada:"), "{:?}", batch.errors);

    let x = mailbox().failing("FindItem", 401, "");
    assert!(matches!(run(&mut ews(x), &state(&store, A), &Limits::default()), Err(Error::Auth(401))));
    let x = mailbox().failing("GetFolder", 503, "");
    assert!(matches!(run(&mut ews(x), &state(&store, A), &Limits::default()), Err(Error::Busy { .. })));
}

#[test]
fn an_item_gone_before_it_is_read_waits_for_the_next_sync() {
    let mut x = mailbox();
    x.vanished.push("m3");
    let batch = run(&mut ews(x), &state(&Store::new(), A), &Limits::default()).unwrap();
    assert!(batch.errors.is_empty(), "{:?}", batch.errors);
    let ids: Vec<Option<&str>> = batch.folders.iter().flat_map(|f| &f.new).map(|n| n.id.as_deref()).collect();
    assert_eq!(ids, [Some("m1"), Some("m2")]);
}

#[test]
fn the_outbox_goes_out_with_create_item_and_a_copy_in_sent_items() {
    let out = |id: u64, to: &[&str]| Outgoing {
        id: MessageId(id),
        from: "ana@contoso.es".into(),
        to: to.iter().map(|s| s.to_string()).collect(),
        raw: format!("From: ana@contoso.es\r\nTo: luis@contoso.es\r\nSubject: {id}\r\n\r\nhola\r\n").into_bytes(),
    };
    let mail = [out(1, &["luis@contoso.es", "jefe@contoso.es"]), out(2, &["luis@contoso.es"])];
    let mut e = ews(Exchange::default());
    let sent = send(&mut e, &mail);
    assert!(sent.iter().all(|s| s.outcome == Outcome::Sent { saved: true }), "{sent:?}");
    let created = e.http().asked("CreateItem");
    assert_eq!(created.len(), 2);
    assert!(created[0].contains(r#"MessageDisposition="SendAndSaveCopy""#), "{}", created[0]);
    assert!(created[0].contains(r#"DistinguishedFolderId Id="sentitems""#), "{}", created[0]);
    assert!(
        created[0].contains(&ibercorreo_mime::base64(&with_bcc(&mail[0].raw, &mail[0].to))),
        "the blind copy goes in a Bcc header: {}",
        created[0]
    );

    // A message Exchange refuses goes back to be fixed; a refused sign-in leaves the rest for later.
    let refused = ok("CreateItem", "").replace(
        r#"ResponseClass="Success"><m:ResponseCode>NoError"#,
        r#"ResponseClass="Error"><m:MessageText>Bad MIME.</m:MessageText><m:ResponseCode>ErrorMimeContentConversionFailed"#,
    );
    let mut e = ews(Exchange::default().failing("CreateItem", 200, &envelope(&refused)));
    assert!(send(&mut e, &mail).iter().all(|s| matches!(s.outcome, Outcome::Failed(_))));
    let mut e = ews(Exchange::default().failing("CreateItem", 401, ""));
    let sent = send(&mut e, &mail);
    assert!(sent.iter().all(|s| matches!(s.outcome, Outcome::Later(_))), "{sent:?}");
    assert_eq!(e.http().asked("CreateItem").len(), 1, "the second is not tried after the refusal");
}

#[test]
fn a_change_without_an_exchange_id_is_reported_not_sent() {
    let s = State {
        known: vec![crate::Known { path: "IN".into(), uid_validity: UID_VALIDITY, uids: [7].into(), ids: BTreeMap::new() }],
        pushes: vec![Push::Flags { path: "IN".into(), uid: 7, seen: true, flagged: false }],
    };
    let mut e = ews(mailbox());
    let batch = run(&mut e, &s, &Limits::default()).unwrap();
    assert!(batch.pushed.first().is_some_and(|(_, r)| r.as_ref().is_err_and(|e| e.contains("Exchange id"))), "{:?}", batch.pushed);
    assert!(e.http().asked("UpdateItem").is_empty());
}

#[test]
fn a_server_paging_offset_that_does_not_move_on_ends_the_list() {
    // FindItem always answering the same page must not loop for ever.
    let page = ok(
        "FindItem",
        r#"<m:RootFolder IndexedPagingOffset="0" IncludesLastItemInRange="false"><t:Items><t:Message><t:ItemId Id="m1" ChangeKey="k"/></t:Message></t:Items></m:RootFolder>"#,
    );
    let mut e = ews(Exchange::default().failing("FindItem", 200, &envelope(&page)));
    let list = EwsMail(&mut e).list("IN").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(e.http().asked("FindItem").len(), 1);
}

#[test]
fn marking_unread_and_flagged_goes_back_as_set_fields() {
    let mut e = ews(mailbox());
    EwsMail(&mut e).set_flags("m2", false, true).unwrap();
    let m2 = e.http().items.iter().find(|i| i.id == "m2").unwrap();
    assert_eq!((m2.read, m2.flagged), (false, true));
    // One the server no longer has is "gone", as over Graph.
    let r = EwsMail(&mut e).delete("nope");
    assert!(matches!(r, Err(Error::Status { status: 404, .. })), "{r:?}");
}
