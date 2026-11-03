//! Outlook `.msg` files (MS-OXMSG): one message saved from Outlook, as a compound file
//! ([`crate::cfb`]) of property streams.
//!
//! [`read`] turns a mail message (`IPM.Note`, a meeting request, a report) into the model through
//! [`crate::props`]. Contacts, appointments and tasks saved as `.msg` are refused, so they are not
//! taken for mail.

use ibercorreo_core::{Error, FolderId, Message, MessageId, Result};

use crate::cfb::{Cfb, Kind, u32_at, u64_at};
#[cfg(test)]
pub(crate) use crate::props::*;
use crate::props::{self, Bag, MAX_ATTACHMENTS, MAX_DEPTH, MAX_RECIPIENTS, PT_BINARY, PT_OBJECT, Raw, RawAttachment, Val, tag};

/// Largest `.msg` read.
pub const MAX_MSG_BYTES: usize = 64 * 1024 * 1024;

/// Read a `.msg` file as a message in `folder`.
pub fn read(data: &[u8], id: MessageId, folder: FolderId) -> Result<Message> {
    if data.len() > MAX_MSG_BYTES {
        return Err(Error::TooLarge { what: ".msg file", size: data.len(), limit: MAX_MSG_BYTES });
    }
    let cfb = Cfb::open(data)?;
    let raw = raw(&cfb, 0, 32, 0)?;
    props::build(&raw, id, folder).map_err(|e| match e {
        Error::Invalid { detail, .. } => Error::invalid(".msg file", detail),
        e => e,
    })
}

/// The properties of `storage`, whose property stream has a header of `header` bytes: the
/// fixed-size ones from the property stream, the others from their `__substg1.0_IIIITTTT`
/// streams. An attachment's contents are only measured.
fn bag(cfb: &Cfb<'_>, storage: usize, header: usize) -> Result<Bag> {
    let mut props = Vec::new();
    if let Some(stream) = cfb.find(storage, "__properties_version1.0") {
        let bytes = cfb.read(stream)?;
        for e in bytes.get(header..).unwrap_or(&[]).as_chunks::<16>().0 {
            let (Some(t), Some(value)) = (u32_at(e, 0), u64_at(e, 8)) else { continue };
            props.push(((t >> 16) as u16, (t & 0xffff) as u16, Val::Fixed(value)));
        }
    }
    for c in cfb.children(storage) {
        let Some(e) = cfb.entry(c) else { continue };
        let Some(name) = e.name.strip_prefix("__substg1.0_") else { continue };
        if e.kind != Kind::Stream || name.len() != 8 {
            continue;
        }
        let (Ok(id), Ok(kind)) = (u16::from_str_radix(name.get(..4).unwrap_or(""), 16), u16::from_str_radix(name.get(4..).unwrap_or(""), 16)) else {
            continue;
        };
        let value = if id == tag::ATTACH_DATA && kind == PT_BINARY {
            Val::Size(usize::try_from(e.size).unwrap_or(usize::MAX))
        } else {
            Val::Bytes(cfb.read(c)?)
        };
        props.push((id, kind, value));
    }
    Ok(Bag(props))
}

/// The message in `storage` (the root, or an embedded message's storage).
fn raw(cfb: &Cfb<'_>, storage: usize, header: usize, depth: usize) -> Result<Raw> {
    let mut out = Raw { props: bag(cfb, storage, header)?, ..Raw::default() };
    for c in cfb.children(storage) {
        let Some(e) = cfb.entry(c) else { continue };
        if e.kind != Kind::Storage {
            continue;
        }
        let name = e.name.to_ascii_lowercase();
        if name.starts_with("__recip_version1.0_") && out.recipients.len() < MAX_RECIPIENTS {
            out.recipients.push(bag(cfb, c, 8)?);
        } else if name.starts_with("__attach_version1.0_") && out.attachments.len() < MAX_ATTACHMENTS {
            let inner = cfb.find(c, &format!("__substg1.0_{:04X}{PT_OBJECT:04X}", tag::ATTACH_DATA));
            let embedded = match inner {
                Some(i) if depth < MAX_DEPTH => Some(Box::new(raw(cfb, i, 24, depth + 1)?)),
                _ => None,
            };
            out.attachments.push(RawAttachment { props: bag(cfb, c, 8)?, embedded });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
