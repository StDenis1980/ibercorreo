//! Mail over Exchange Web Services, for an Exchange Server run by the organisation itself: a
//! [`MailServer`] for the shared sync ([`crate::server_mail`]), and the Outbox sent with
//! `CreateItem` (`SendAndSaveCopy`), so Exchange files the copy in Sent Items.
//!
//! - Folders come from `SyncFolderHierarchy` under the mailbox's root, mail folders only
//!   (`IPF.Note`); the standard ones are found by their distinguished names (`inbox`,
//!   `sentitems`…) and Exchange's own Outbox is left out, the app has one.
//! - Messages are listed with `FindItem` (ids, read state, flag, time received) page by page and
//!   put newest first; each new one is read as MIME with `GetItem`.
//! - Changes made here go back as `UpdateItem` (read, flag), `MoveItem` and `DeleteItem`
//!   (`HardDelete`). EWS gives a moved message a new id: it is downloaded again where it went,
//!   as over IMAP.

use std::collections::BTreeMap;

use ibercorreo_dav::caldav::Http;
use ibercorreo_dav::ews::Ews;
use ibercorreo_dav::{Error, Result};
use ibercorreo_imap::Mailbox;

use ews::create_item::CreateItem;
use ews::delete_item::DeleteItem;
use ews::find_item::{FindItem, Traversal};
use ews::get_folder::GetFolder;
use ews::get_item::GetItem;
use ews::move_item::MoveItem;
use ews::sync_folder_hierarchy::{Change, SyncFolderHierarchy};
use ews::update_item::{ConflictResolution, ItemChange, ItemChangeDescription, ItemChangeInner, UpdateItem, Updates};
use ews::{
    BaseFolderId, BaseItemId, BasePoint, BaseShape, CopyMoveItemData, DeleteType, ExtendedFieldURI, ExtendedProperty, FlagStatus, Folder,
    FolderShape, ItemShape, Message, MessageDisposition, MimeContent, PathToElement, PropertyType, RealItem, View,
};

use crate::outbox::{Outgoing, Sent};
use crate::server_mail::{MAX_MESSAGES, MailServer, fatal};
use crate::{Batch, Limits, Progress, State};

/// How deep folders inside folders are named.
pub const MAX_DEPTH: usize = 10;
/// Items asked for in one `FindItem` page.
pub const PAGE: usize = 500;
/// Pages read from `SyncFolderHierarchy`.
const MAX_HIERARCHY_PAGES: usize = 50;

/// Exchange's Outbox: not synced, the app has its own.
const OUTBOX: &str = "\\Outbox";
/// Distinguished folder names and the special-use attribute each stands for.
const DISTINGUISHED: [(&str, &str); 6] =
    [("inbox", "\\Inbox"), ("sentitems", "\\Sent"), ("drafts", "\\Drafts"), ("deleteditems", "\\Trash"), ("junkemail", "\\Junk"), ("outbox", OUTBOX)];

fn field(uri: &str) -> PathToElement {
    PathToElement::FieldURI { field_URI: uri.to_string() }
}

/// `PidTagFlagStatus` (MS-OXOFLAG): 2 flagged for follow-up, 0 not flagged.
const FLAG_STATUS_TAG: &str = "0x1090";
const FLAGGED: u8 = 2;
const NOT_FLAGGED: u8 = 0;

fn flag_status() -> ExtendedFieldURI {
    ExtendedFieldURI {
        distinguished_property_set_id: None,
        property_set_id: None,
        property_tag: Some(FLAG_STATUS_TAG.to_string()),
        property_name: None,
        property_id: None,
        property_type: Some(PropertyType::Integer),
    }
}

fn item_id(id: &str) -> BaseItemId {
    BaseItemId::ItemId { id: id.to_string(), change_key: None }
}

/// The message inside any kind of item; `None` for a kind the `ews` crate adds later.
fn message(item: RealItem) -> Option<Message> {
    Some(match item {
        RealItem::Item(m)
        | RealItem::Message(m)
        | RealItem::CalendarItem(m)
        | RealItem::Contact(m)
        | RealItem::DistributionList(m)
        | RealItem::MeetingMessage(m)
        | RealItem::MeetingRequest(m)
        | RealItem::MeetingResponse(m)
        | RealItem::MeetingCancellation(m)
        | RealItem::Task(m)
        | RealItem::PostItem(m) => m,
        _ => return None,
    })
}

/// An EWS connection as a [`MailServer`].
pub struct EwsMail<'a, H>(pub &'a mut Ews<H>);

impl<H: Http> EwsMail<'_, H> {
    /// The ids of the standard folders, and the attribute each stands for.
    fn roles(&mut self) -> Result<BTreeMap<String, &'static str>> {
        let ids = DISTINGUISHED.iter().map(|(name, _)| BaseFolderId::new_distinguished(*name)).collect();
        let each = self.0.call_each(GetFolder { folder_shape: FolderShape { base_shape: BaseShape::IdOnly }, folder_ids: ids })?;
        let mut roles = BTreeMap::new();
        for ((_, attribute), found) in DISTINGUISHED.iter().zip(each) {
            let folders = match found {
                Ok(m) => m.folders.inner,
                Err(e) if fatal(&e) => return Err(e),
                // A mailbox without one of them.
                Err(_) => continue,
            };
            for f in folders {
                if let Some(id) = folder_parts(&f).0 {
                    roles.insert(id.to_string(), *attribute);
                }
            }
        }
        Ok(roles)
    }
}

/// A folder's id, parent id, class and name, whatever kind it is; `true` for a mail folder.
fn folder_parts(f: &Folder) -> (Option<&str>, Option<&str>, Option<&str>, bool) {
    match f {
        Folder::Folder { folder_id, parent_folder_id, folder_class, display_name, .. } => (
            folder_id.as_ref().map(|i| i.id.as_str()),
            parent_folder_id.as_ref().map(|i| i.id.as_str()),
            display_name.as_deref(),
            folder_class.as_deref().is_none_or(|c| c == "IPF.Note" || c.starts_with("IPF.Note.")),
        ),
        Folder::CalendarFolder { folder_id, .. }
        | Folder::ContactsFolder { folder_id, .. }
        | Folder::SearchFolder { folder_id, .. }
        | Folder::TasksFolder { folder_id, .. } => (folder_id.as_ref().map(|i| i.id.as_str()), None, None, false),
    }
}

impl<H: Http> MailServer for EwsMail<'_, H> {
    fn name(&self) -> &'static str {
        "Exchange"
    }

    fn folders(&mut self, max: usize) -> Result<Vec<Mailbox>> {
        let roles = self.roles()?;
        // id → (parent id, name), mail folders only.
        let mut all: BTreeMap<String, (Option<String>, String)> = BTreeMap::new();
        let mut sync_state = None;
        for _ in 0..MAX_HIERARCHY_PAGES {
            let op = SyncFolderHierarchy {
                folder_shape: FolderShape { base_shape: BaseShape::AllProperties },
                sync_folder_id: Some(BaseFolderId::new_distinguished("msgfolderroot")),
                sync_state: sync_state.take(),
            };
            let Some(page) = self.0.call(op)?.into_iter().next() else { break };
            for change in page.changes.inner {
                let (Change::Create { folder } | Change::Update { folder }) = change else { continue };
                let (Some(id), parent, name, true) = folder_parts(&folder) else { continue };
                all.insert(id.to_string(), (parent.map(str::to_string), name.unwrap_or(id).to_string()));
            }
            if page.includes_last_folder_in_range || all.len() > max.saturating_mul(4) {
                break;
            }
            sync_state = Some(page.sync_state);
        }
        let mut out: Vec<Mailbox> = Vec::new();
        for (id, (parent, name)) in &all {
            let attribute = roles.get(id).copied();
            if attribute == Some(OUTBOX) {
                continue;
            }
            // The names of the folders it is inside, while they are mail folders here.
            let mut path = vec![name.as_str()];
            let mut up = parent.as_deref();
            while let Some((p, (grand, pname))) = up.and_then(|u| all.get_key_value(u)) {
                if path.len() > MAX_DEPTH || roles.get(p) == Some(&OUTBOX) {
                    break;
                }
                path.push(pname);
                up = grand.as_deref();
            }
            path.reverse();
            out.push(Mailbox {
                raw: id.clone(),
                name: path.join("/"),
                delimiter: Some('/'),
                attributes: attribute.map(str::to_string).into_iter().collect(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out.truncate(max);
        Ok(out)
    }

    fn list(&mut self, folder: &str) -> Result<Vec<(String, Vec<String>)>> {
        let mut items = Vec::new();
        let mut offset = 0usize;
        loop {
            let op = FindItem {
                traversal: Traversal::Shallow,
                item_shape: ItemShape {
                    base_shape: BaseShape::IdOnly,
                    include_mime_content: None,
                    additional_properties: Some(vec![field("message:IsRead"), field("item:Flag"), field("item:DateTimeReceived")]),
                },
                view: Some(View::IndexedPageItemView { max_entries_returned: Some(PAGE), base_point: BasePoint::Beginning, offset }),
                parent_folder_ids: vec![BaseFolderId::new_folder(folder)],
            };
            let Some(page) = self.0.call(op)?.into_iter().next() else { break };
            let got = page.root_folder.items.inner.len();
            for m in page.root_folder.items.inner.into_iter().filter_map(message) {
                let Some(id) = m.item_id.map(|i| i.id) else { continue };
                let mut flags = Vec::new();
                if m.is_read == Some(true) {
                    flags.push("\\Seen".to_string());
                }
                if m.flag.and_then(|f| f.flag_status) == Some(FlagStatus::Flagged) {
                    flags.push("\\Flagged".to_string());
                }
                items.push((m.date_time_received.map(|d| d.0), id, flags));
            }
            if items.len() > MAX_MESSAGES {
                return Err(Error::TooLarge { what: "the folder", limit: MAX_MESSAGES });
            }
            if got == 0 || page.root_folder.includes_last_item_in_range != Some(false) {
                break;
            }
            // Where the server says the next page starts; one that does not move on ends the list.
            let next = page.root_folder.indexed_paging_offset.unwrap_or(offset.saturating_add(got));
            if next <= offset {
                break;
            }
            offset = next;
        }
        // Newest first; an item without a time last.
        items.sort_by_key(|i| std::cmp::Reverse(i.0));
        Ok(items.into_iter().map(|(_, id, flags)| (id, flags)).collect())
    }

    fn fetch(&mut self, id: &str) -> Result<Vec<u8>> {
        let op = GetItem {
            item_shape: ItemShape { base_shape: BaseShape::IdOnly, include_mime_content: Some(true), additional_properties: None },
            item_ids: vec![item_id(id)],
        };
        let item = self.0.call(op)?.into_iter().next().and_then(|m| m.items.inner.into_iter().next()).and_then(message);
        let mime = item.and_then(|m| m.mime_content).ok_or_else(|| Error::Protocol("Exchange sent the message without its MIME content".into()))?;
        ibercorreo_mime::unbase64(&mime.content).ok_or_else(|| Error::Protocol("Exchange sent MIME content that is not base64".into()))
    }

    fn set_flags(&mut self, id: &str, seen: bool, flagged: bool) -> Result<()> {
        // The flag goes as its MAPI property: the `ews` crate writes `item:Flag`'s status
        // outside the EWS types namespace, which Exchange refuses.
        let flag = ExtendedProperty { extended_field_URI: flag_status(), value: (if flagged { FLAGGED } else { NOT_FLAGGED }).to_string() };
        let change = ItemChange {
            item_change: ItemChangeInner {
                item_id: item_id(id),
                updates: Updates {
                    inner: vec![
                        ItemChangeDescription::SetItemField {
                            field_uri: field("message:IsRead"),
                            message: Message { is_read: Some(seen), ..Message::default() },
                        },
                        ItemChangeDescription::SetItemField {
                            field_uri: PathToElement::ExtendedFieldURI {
                                distinguished_property_set_id: None,
                                property_set_id: None,
                                property_tag: flag.extended_field_URI.property_tag.clone(),
                                property_name: None,
                                property_id: None,
                                property_type: PropertyType::Integer,
                            },
                            message: Message { extended_property: Some(vec![flag]), ..Message::default() },
                        },
                    ],
                },
            },
        };
        let op = UpdateItem {
            message_disposition: MessageDisposition::SaveOnly,
            conflict_resolution: Some(ConflictResolution::AlwaysOverwrite),
            item_changes: vec![change],
        };
        self.0.call(op).map(|_| ())
    }

    fn move_to(&mut self, id: &str, folder: &str) -> Result<()> {
        let op = MoveItem {
            inner: CopyMoveItemData { to_folder_id: BaseFolderId::new_folder(folder), item_ids: vec![item_id(id)], return_new_item_ids: None },
        };
        self.0.call(op).map(|_| ())
    }

    fn delete(&mut self, id: &str) -> Result<()> {
        let op = DeleteItem {
            delete_type: DeleteType::HardDelete,
            send_meeting_cancellations: None,
            affected_task_occurrences: None,
            suppress_read_receipts: Some(true),
            item_ids: vec![item_id(id)],
        };
        self.0.call(op).map(|_| ())
    }

    fn send(&mut self, raw: &[u8]) -> Result<()> {
        let item = Message { mime_content: Some(MimeContent { character_set: None, content: ibercorreo_mime::base64(raw) }), ..Message::default() };
        let op = CreateItem {
            message_disposition: Some(MessageDisposition::SendAndSaveCopy),
            saved_item_folder_id: Some(BaseFolderId::new_distinguished("sentitems")),
            items: vec![RealItem::Message(item)],
        };
        self.0.call(op).map(|_| ())
    }
}

/// Send local changes, then fetch the account's mail ([`crate::server_mail::run_with`]).
pub fn run<H: Http>(e: &mut Ews<H>, state: &State, limits: &Limits) -> Result<Batch> {
    run_with(e, state, limits, &mut |_| {})
}

/// [`run`], telling `progress` as each folder starts and as its new mail comes in.
pub fn run_with<H: Http>(e: &mut Ews<H>, state: &State, limits: &Limits, progress: &mut dyn FnMut(&Progress)) -> Result<Batch> {
    crate::server_mail::run_with(&mut EwsMail(e), state, limits, progress)
}

/// Send each message; Exchange files it in Sent Items itself.
pub fn send<H: Http>(e: &mut Ews<H>, mail: &[Outgoing]) -> Vec<Sent> {
    crate::server_mail::send(&mut EwsMail(e), mail)
}

#[cfg(test)]
mod tests;
