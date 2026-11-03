//! Named properties (MS-OXPROPS, MS-PST 2.4.7): properties such as an appointment's location or a
//! contact's e-mail addresses are named by a property set and a number, not by a fixed tag. Each
//! file maps the names it uses to tags from `0x8000` up; [`NameMap`] reads that map.

use crate::ndb::{u16_at, u32_at};
use crate::props::{Bag, PT_BINARY};

/// A property set's GUID as files store it (the first three fields little-endian).
pub(crate) type Guid = [u8; 16];

/// A GUID of the `{000620xx-0000-0000-C000-000000000046}` family most Outlook sets belong to.
const fn ole(data1: u32) -> Guid {
    let d = data1.to_le_bytes();
    [d[0], d[1], d[2], d[3], 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46]
}

pub(crate) const PS_MAPI: Guid = ole(0x0002_0328);
pub(crate) const PS_PUBLIC_STRINGS: Guid = ole(0x0002_0329);
pub(crate) const PSETID_APPOINTMENT: Guid = ole(0x0006_2002);
pub(crate) const PSETID_ADDRESS: Guid = ole(0x0006_2004);
pub(crate) const PSETID_COMMON: Guid = ole(0x0006_2008);
/// `{6ED8DA90-450B-101B-98DA-00AA003F1305}`.
pub(crate) const PSETID_MEETING: Guid = [0x90, 0xda, 0xd8, 0x6e, 0x0b, 0x45, 0x1b, 0x10, 0x98, 0xda, 0x00, 0xaa, 0x00, 0x3f, 0x13, 0x05];

/// The name-to-id map's properties: the GUID stream and the entry stream.
const GUID_STREAM: u16 = 0x0002;
const ENTRY_STREAM: u16 = 0x0003;

/// Numbered names a file maps to tags.
#[derive(Debug, Clone, Default)]
pub(crate) struct NameMap {
    entries: Vec<(Guid, u32, u16)>,
}

impl NameMap {
    /// The map in the properties of a `.pst` file's node `0x61`. Entries naming a GUID the file
    /// does not hold, and string names, are left out.
    pub(crate) fn from_bag(bag: &Bag) -> NameMap {
        let guids = bag.bytes(GUID_STREAM, PT_BINARY).unwrap_or(&[]);
        let stream = bag.bytes(ENTRY_STREAM, PT_BINARY).unwrap_or(&[]);
        let mut entries = Vec::new();
        for e in stream.as_chunks::<8>().0 {
            let (Some(id), Some(kind), Some(index)) = (u32_at(e, 0), u16_at(e, 4), u16_at(e, 6)) else { continue };
            // Bit 0: a string name (its id is an offset in the string stream).
            if kind & 1 != 0 {
                continue;
            }
            let guid = match kind >> 1 {
                1 => PS_MAPI,
                2 => PS_PUBLIC_STRINGS,
                n @ 3.. => {
                    let at = usize::from(n - 3) * 16;
                    let Some(g) = guids.get(at..at + 16).and_then(|g| Guid::try_from(g).ok()) else { continue };
                    g
                }
                _ => continue,
            };
            entries.push((guid, id, 0x8000u16.saturating_add(index)));
        }
        NameMap { entries }
    }

    /// The tag a file gives name `(set, id)`.
    pub(crate) fn tag(&self, set: &Guid, id: u32) -> Option<u16> {
        self.entries.iter().find(|(g, i, _)| g == set && *i == id).map(|e| e.2)
    }
}

#[cfg(test)]
mod tests;
