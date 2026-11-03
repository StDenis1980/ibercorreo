//! Outlook data files (`.pst`, MS-PST): the folders an archive holds and the mail in them.
//!
//! [`Pst::open`] reads the node database ([`crate::ndb`]); [`Pst::folders`] lists the folder tree
//! under the archive's top folder; [`Pst::messages`] lists a folder's items and [`Pst::message`]
//! reads one into the model through `props`, the same way as a `.msg` file. Calendar, contact,
//! task and note items are not taken for mail.

use ibercorreo_core::{Contact, Event, FolderId, Message, MessageId, Result, clamp_text};

use crate::items::{self, Kind};
use crate::ltp::{Context, MAX_ROWS, PC_SIG, ROW_ID, TC_SIG};
use crate::named::NameMap;
use crate::ndb::{Ndb, Subnode, bad, u32_at};
use crate::props::{self, Bag, EMBEDDED, MAX_ATTACHMENTS, MAX_DEPTH, MAX_RECIPIENTS, PT_BINARY, Raw, RawAttachment, tag};

pub use crate::ndb::{FileSource, Source};

/// The message store's properties.
const NID_MESSAGE_STORE: u32 = 0x21;
/// The root folder, above the top of the folder tree the person sees.
const NID_ROOT_FOLDER: u32 = 0x122;
/// The map from named properties to the tags this file gives them.
const NID_NAME_TO_ID_MAP: u32 = 0x61;
/// A message's recipients and attachments tables (subnodes of the message).
const NID_RECIPIENT_TABLE: u32 = 0x692;
const NID_ATTACHMENT_TABLE: u32 = 0x671;

const NID_TYPE_NORMAL_FOLDER: u32 = 0x02;
const NID_TYPE_HIERARCHY_TABLE: u32 = 0x0d;
const NID_TYPE_CONTENTS_TABLE: u32 = 0x0e;

/// `PidTagIpmSubtreeEntryId`: the top of the folders shown to the person.
const IPM_SUBTREE: u16 = 0x35e0;
/// `PidTagContainerClass`: what a folder holds (`IPF.Note`, `IPF.Appointment`…).
const CONTAINER_CLASS: u16 = 0x3613;
/// `PidTagContentCount`.
const CONTENT_COUNT: u16 = 0x3602;

/// Folders read from one archive, at most, and how deep.
pub const MAX_FOLDERS: usize = 10_000;
const MAX_FOLDER_DEPTH: usize = 64;

/// One folder of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PstFolder {
    /// Its node in the file.
    pub nid: u32,
    /// Its name and its parents' names, from the top folder down.
    pub path: Vec<String>,
    /// What it holds: `IPF.Note` (mail), `IPF.Appointment`, `IPF.Contact`, `IPF.Task`… Empty when
    /// the file does not say, which Outlook takes for mail.
    pub class: String,
    /// How many items the folder says it holds.
    pub count: u32,
}

impl PstFolder {
    /// Whether the folder holds mail (rather than calendar, contacts, tasks, notes or journal).
    pub fn holds_mail(&self) -> bool {
        let c = self.class.to_ascii_lowercase();
        c.is_empty() || c.starts_with("ipf.note")
    }

    /// Whether the folder is a calendar.
    pub fn holds_calendar(&self) -> bool {
        self.class.to_ascii_lowercase().starts_with("ipf.appointment")
    }

    /// Whether the folder holds contacts.
    pub fn holds_contacts(&self) -> bool {
        self.class.to_ascii_lowercase().starts_with("ipf.contact")
    }
}

/// An open Outlook data file.
pub struct Pst<S> {
    ndb: Ndb<S>,
    names: NameMap,
}

impl<S: Source> Pst<S> {
    pub fn open(src: S) -> Result<Self> {
        let mut pst = Pst { ndb: Ndb::open(src)?, names: NameMap::default() };
        // Without the map, mail still reads; calendar and contact details do not.
        if let Ok((_, bag)) = pst.pc(NID_NAME_TO_ID_MAP) {
            pst.names = NameMap::from_bag(&bag);
        }
        Ok(pst)
    }

    /// The node's property context.
    fn pc(&self, nid: u32) -> Result<(Context<'_, S>, Bag)> {
        let n = self.ndb.node(nid)?.ok_or_else(|| bad("a missing item"))?;
        let ctx = Context::open(&self.ndb, n.data, n.sub, PC_SIG)?;
        let bag = ctx.pc(&[tag::ATTACH_DATA])?;
        Ok((ctx, bag))
    }

    /// The row ids of the table in node `nid`; empty when the node is missing.
    fn table_ids(&self, nid: u32) -> Result<Vec<u32>> {
        let Some(n) = self.ndb.node(nid)? else { return Ok(Vec::new()) };
        let ctx = Context::open(&self.ndb, n.data, n.sub, TC_SIG)?;
        Ok(ctx.tc(MAX_ROWS)?.iter().filter_map(|r| r.int(ROW_ID)).collect())
    }

    /// The archive's name, as Outlook shows it (`Outlook Data File`, `Archive`…).
    pub fn name(&self) -> String {
        self.pc(NID_MESSAGE_STORE).ok().and_then(|(_, b)| text(&b, tag::DISPLAY_NAME)).unwrap_or_default()
    }

    /// The top of the folder tree the person sees: the store's IPM subtree, else the root folder.
    fn top(&self) -> Result<u32> {
        let (_, store) = self.pc(NID_MESSAGE_STORE)?;
        // An entry id: 4 bytes of flags, the store's 16-byte uid, the node id.
        Ok(store.bytes(IPM_SUBTREE, PT_BINARY).and_then(|e| u32_at(e, 20)).unwrap_or(NID_ROOT_FOLDER))
    }

    /// The folders under the top folder, parents before their children, up to [`MAX_FOLDERS`].
    /// A folder that cannot be read is left out with its subfolders.
    pub fn folders(&self) -> Result<Vec<PstFolder>> {
        let mut out = Vec::new();
        let mut seen = Vec::new();
        let top = self.top()?;
        let mut stack: Vec<(u32, Vec<String>)> = self.children(top)?.into_iter().rev().map(|c| (c, Vec::new())).collect();
        seen.push(top);
        while let Some((nid, parent)) = stack.pop() {
            if out.len() >= MAX_FOLDERS || seen.contains(&nid) || parent.len() >= MAX_FOLDER_DEPTH {
                continue;
            }
            seen.push(nid);
            let Ok((_, bag)) = self.pc(nid) else { continue };
            let mut path = parent;
            path.push(clamp_text(&text(&bag, tag::DISPLAY_NAME).unwrap_or_default(), 256).to_string());
            let class = text(&bag, CONTAINER_CLASS).unwrap_or_default();
            let count = bag.int(CONTENT_COUNT).unwrap_or(0);
            for c in self.children(nid).unwrap_or_default().into_iter().rev() {
                stack.push((c, path.clone()));
            }
            out.push(PstFolder { nid, path, class, count });
        }
        Ok(out)
    }

    /// The subfolders of folder `nid`, from its hierarchy table.
    fn children(&self, nid: u32) -> Result<Vec<u32>> {
        let ids = self.table_ids((nid & !0x1f) | NID_TYPE_HIERARCHY_TABLE)?;
        Ok(ids.into_iter().filter(|c| c & 0x1f == NID_TYPE_NORMAL_FOLDER).collect())
    }

    /// The items of `folder`, from its contents table: their node ids, for [`Pst::message`].
    pub fn messages(&self, folder: &PstFolder) -> Result<Vec<u32>> {
        self.table_ids((folder.nid & !0x1f) | NID_TYPE_CONTENTS_TABLE)
    }

    /// Item `nid` as a message in `folder`; `None` for an item that is not mail (an appointment,
    /// a contact, a task, a note).
    pub fn message(&self, nid: u32, id: MessageId, folder: FolderId) -> Result<Option<Message>> {
        let n = self.ndb.node(nid)?.ok_or_else(|| bad("a missing message"))?;
        let raw = self.raw(n.data, n.sub, 0)?;
        if props::mail_class(&raw).is_err() {
            return Ok(None);
        }
        props::build(&raw, id, folder).map(Some)
    }

    /// Item `nid` as events: an appointment, with the changed occurrences of its series. `None`
    /// for an item that is not an appointment, or one without a start.
    pub fn appointment(&self, nid: u32) -> Result<Option<Vec<Event>>> {
        let raw = self.item(nid)?;
        Ok(if items::kind(&raw) == Kind::Appointment { items::appointment(&raw, &self.names) } else { None })
    }

    /// Item `nid` as a contact; `None` for an item that is not one (a contact group is not).
    pub fn contact(&self, nid: u32) -> Result<Option<Contact>> {
        let raw = self.item(nid)?;
        Ok(if items::kind(&raw) == Kind::Contact { items::contact(&raw, &self.names) } else { None })
    }

    fn item(&self, nid: u32) -> Result<Raw> {
        let n = self.ndb.node(nid)?.ok_or_else(|| bad("a missing item"))?;
        self.raw(n.data, n.sub, 0)
    }

    /// A message's properties, recipients and attachments: the message node, or an attached
    /// message's subnode.
    fn raw(&self, data: u64, sub: u64, depth: usize) -> Result<Raw> {
        let ctx = Context::open(&self.ndb, data, sub, PC_SIG)?;
        let mut raw = Raw { props: ctx.pc(&[tag::ATTACH_DATA])?, ..Raw::default() };
        if let Some(t) = ctx.subnode(NID_RECIPIENT_TABLE) {
            raw.recipients = Context::open(&self.ndb, t.data, t.sub, TC_SIG)?.tc(MAX_RECIPIENTS)?;
        }
        if let Some(t) = ctx.subnode(NID_ATTACHMENT_TABLE) {
            let rows = Context::open(&self.ndb, t.data, t.sub, TC_SIG)?.tc(MAX_ATTACHMENTS)?;
            for row in rows {
                // Each row names the attachment's own subnode, with its properties.
                let Some(a) = row.int(ROW_ID).and_then(|n| ctx.subnode(n)) else { continue };
                raw.attachments.push(self.attachment(a, depth).unwrap_or_default());
            }
        }
        Ok(raw)
    }

    fn attachment(&self, a: Subnode, depth: usize) -> Result<RawAttachment> {
        let ctx = Context::open(&self.ndb, a.data, a.sub, PC_SIG)?;
        let props = ctx.pc(&[tag::ATTACH_DATA])?;
        let embedded = if props.int(tag::ATTACH_METHOD) == Some(EMBEDDED) && depth < MAX_DEPTH {
            // The attached message is a subnode of the attachment, with its own subnodes.
            match ctx.object(&props, tag::ATTACH_DATA).and_then(|n| ctx.subnode(n)) {
                Some(m) => self.raw(m.data, m.sub, depth + 1).ok().map(Box::new),
                None => None,
            }
        } else {
            None
        };
        Ok(RawAttachment { props, embedded })
    }
}

fn text(bag: &Bag, id: u16) -> Option<String> {
    props::text_of(bag, id)
}

#[cfg(test)]
mod tests;
