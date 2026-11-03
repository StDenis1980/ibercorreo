//! The lists, tables and properties layer of a `.pst` file (MS-PST 2.3): the heap a node's data
//! holds (HN), B-trees inside that heap (BTH), and the two structures built on them, property
//! contexts (one object's properties, PC) and table contexts (rows of properties, TC).

use crate::ndb::{Ndb, Source, Subnode, bad, u16_at, u32_at, u64_at};
use crate::props::{Bag, PT_BINARY, PT_BOOLEAN, PT_LONG, PT_OBJECT, PT_SHORT, Val};
use ibercorreo_core::Result;

const HN_SIG: u8 = 0xec;
const BTH_SIG: u8 = 0xb5;
pub(crate) const PC_SIG: u8 = 0xbc;
pub(crate) const TC_SIG: u8 = 0x7c;
/// Records of one B-tree-on-heap, at most.
const MAX_RECORDS: usize = 1 << 20;
/// Rows of one table, at most.
pub(crate) const MAX_ROWS: usize = 1 << 22;
/// The largest property value read (bodies are capped further up).
const MAX_VALUE: usize = 64 * 1024 * 1024;

/// A node's heap: its data blocks, each with a page map of allocations (MS-PST 2.3.1).
pub(crate) struct Heap {
    blocks: Vec<Vec<u8>>,
    pub(crate) client: u8,
    pub(crate) root: u32,
}

impl Heap {
    pub(crate) fn new(blocks: Vec<Vec<u8>>) -> Result<Heap> {
        let first = blocks.first().ok_or_else(|| bad("an empty heap"))?;
        if first.get(2) != Some(&HN_SIG) {
            return Err(bad("a node that is not a heap"));
        }
        let (Some(&client), Some(root)) = (first.get(3), u32_at(first, 4)) else {
            return Err(bad("a damaged heap"));
        };
        Ok(Heap { blocks, client, root })
    }

    /// The bytes of allocation `hid`; `None` for an id that names nothing.
    pub(crate) fn get(&self, hid: u32) -> Option<&[u8]> {
        if hid & 0x1f != 0 {
            return None;
        }
        let index = usize::try_from((hid >> 5) & 0x7ff).ok()?;
        let block = self.blocks.get(usize::try_from(hid >> 16).ok()?)?;
        // Every block starts with the offset of its page map.
        let map = usize::from(u16_at(block, 0)?);
        let count = usize::from(u16_at(block, map)?);
        if index == 0 || index > count {
            return None;
        }
        let start = usize::from(u16_at(block, map + 4 + (index - 1) * 2)?);
        let end = usize::from(u16_at(block, map + 4 + index * 2)?);
        block.get(start..end)
    }
}

/// A B-tree on the heap rooted at `hid` (MS-PST 2.3.2): its records as (key, data).
pub(crate) fn bth(heap: &Heap, hid: u32) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let h = heap.get(hid).ok_or_else(|| bad("a missing B-tree header"))?;
    let (Some(&BTH_SIG), Some(&key), Some(&data), Some(&levels), Some(root)) = (h.first(), h.get(1), h.get(2), h.get(3), u32_at(h, 4)) else {
        return Err(bad("a damaged B-tree header"));
    };
    let (key, data) = (usize::from(key), usize::from(data));
    if key == 0 || levels > 8 {
        return Err(bad("a damaged B-tree header"));
    }
    let mut out = Vec::new();
    if root != 0 {
        bth_into(heap, root, key, data, levels, &mut out)?;
    }
    Ok(out)
}

fn bth_into(heap: &Heap, hid: u32, key: usize, data: usize, level: u8, out: &mut Vec<(Vec<u8>, Vec<u8>)>) -> Result<()> {
    let records = heap.get(hid).ok_or_else(|| bad("a missing B-tree record"))?;
    let width = if level == 0 { key + data } else { key + 4 };
    for r in records.chunks_exact(width) {
        if out.len() >= MAX_RECORDS {
            return Err(bad("a B-tree with too many records"));
        }
        let (k, rest) = r.split_at(key);
        if level == 0 {
            out.push((k.to_vec(), rest.to_vec()));
        } else {
            let child = u32_at(rest, 0).ok_or_else(|| bad("a damaged B-tree record"))?;
            bth_into(heap, child, key, data, level - 1, out)?;
        }
    }
    Ok(())
}

/// Whether a property of type `kind` is stored in its 4-byte slot rather than elsewhere.
fn inline(kind: u16) -> bool {
    matches!(kind, PT_SHORT | PT_LONG | PT_BOOLEAN | 0x0004 | 0x000a)
}

/// A value of fixed size: its byte count, or `None` for variable-size types.
fn fixed_size(kind: u16) -> Option<usize> {
    match kind {
        PT_SHORT => Some(2),
        PT_LONG | 0x0004 | 0x000a => Some(4),
        PT_BOOLEAN => Some(1),
        0x0005 | 0x0006 | 0x0007 | 0x0014 | 0x0040 => Some(8),
        0x0048 => Some(16),
        _ => None,
    }
}

/// A node's heap and subnodes, through which its values are found.
pub(crate) struct Context<'n, S> {
    pub(crate) ndb: &'n Ndb<S>,
    pub(crate) heap: Heap,
    pub(crate) subnodes: Vec<Subnode>,
}

impl<'n, S: Source> Context<'n, S> {
    /// The heap of a node (or subnode) with data `data` and subnodes `sub`.
    pub(crate) fn open(ndb: &'n Ndb<S>, data: u64, sub: u64, sig: u8) -> Result<Self> {
        let heap = Heap::new(ndb.data(data)?)?;
        if heap.client != sig {
            return Err(bad("a node of the wrong kind"));
        }
        Ok(Context { ndb, heap, subnodes: ndb.subnodes(sub)? })
    }

    pub(crate) fn subnode(&self, nid: u32) -> Option<Subnode> {
        self.subnodes.iter().find(|s| s.nid == nid).copied()
    }

    /// The value an HNID names: a heap allocation, or a subnode's data. With `measure`, a
    /// subnode's value is only measured.
    fn hnid(&self, hnid: u32, measure: bool) -> Result<Val> {
        if hnid == 0 {
            return Ok(Val::Bytes(Vec::new()));
        }
        if hnid & 0x1f == 0 {
            let b = self.heap.get(hnid).ok_or_else(|| bad("a value that is not in the heap"))?;
            return Ok(if measure { Val::Size(b.len()) } else { Val::Bytes(b.to_vec()) });
        }
        let s = self.subnode(hnid).ok_or_else(|| bad("a value in a missing subnode"))?;
        if measure {
            return Ok(Val::Size(self.ndb.data_len(s.data)?));
        }
        let mut out = Vec::new();
        for block in self.ndb.data(s.data)? {
            if out.len().saturating_add(block.len()) > MAX_VALUE {
                return Err(bad("a value too large"));
            }
            out.extend_from_slice(&block);
        }
        Ok(Val::Bytes(out))
    }

    /// A property value: `slot` is the 4 bytes a PC keeps (the value, or where it is) for types
    /// that do not fit, the bytes of the value for those that do.
    fn value(&self, kind: u16, slot: u32, measure: bool) -> Result<Val> {
        if inline(kind) {
            let v = match kind {
                PT_BOOLEAN => slot & 0xff,
                PT_SHORT => slot & 0xffff,
                _ => slot,
            };
            return Ok(Val::Fixed(u64::from(v)));
        }
        match (self.hnid(slot, measure)?, fixed_size(kind)) {
            (Val::Bytes(b), Some(8)) => Ok(Val::Fixed(u64_at(&b, 0).unwrap_or(0))),
            (v, _) => Ok(v),
        }
    }

    /// The property context of this node (MS-PST 2.3.3). `measure` names properties only measured
    /// (an attachment's contents).
    pub(crate) fn pc(&self, measure: &[u16]) -> Result<Bag> {
        let mut props = Vec::new();
        for (k, d) in bth(&self.heap, self.heap.root)? {
            let (Some(id), Some(kind), Some(slot)) = (u16_at(&k, 0), u16_at(&d, 0), u32_at(&d, 2)) else { continue };
            let measured = measure.contains(&id) && kind == PT_BINARY;
            // A value that cannot be read is left out, not the whole object.
            if let Ok(v) = self.value(kind, slot, measured) {
                props.push((id, kind, v));
            }
        }
        Ok(Bag(props))
    }

    /// The `(nid, size)` an object property points to: an attached message's subnode.
    pub(crate) fn object(&self, bag: &Bag, id: u16) -> Option<u32> {
        bag.bytes(id, PT_OBJECT).and_then(|b| u32_at(b, 0))
    }

    /// The rows of this node's table context (MS-PST 2.3.4), up to `limit`, each as its
    /// properties. The row id (`PidTagLtpRowId`) is property `0x67F2`.
    pub(crate) fn tc(&self, limit: usize) -> Result<Vec<Bag>> {
        let info = self.heap.get(self.heap.root).ok_or_else(|| bad("a missing table header"))?;
        let (Some(&TC_SIG), Some(&cols), Some(row_size), Some(rows)) = (info.first(), info.get(1), u16_at(info, 8), u32_at(info, 14)) else {
            return Err(bad("a damaged table header"));
        };
        let ceb = usize::from(u16_at(info, 6).unwrap_or(0));
        let row_size = usize::from(row_size);
        if row_size == 0 {
            return Err(bad("a damaged table header"));
        }
        let columns: Vec<(u16, u16, usize, usize, usize)> = (0..usize::from(cols))
            .filter_map(|i| {
                let at = 22 + i * 8;
                let tag = u32_at(info, at)?;
                Some((
                    (tag >> 16) as u16,
                    (tag & 0xffff) as u16,
                    usize::from(u16_at(info, at + 4)?),
                    usize::from(*info.get(at + 6)?),
                    usize::from(*info.get(at + 7)?),
                ))
            })
            .collect();
        // The row matrix: one heap allocation, or a subnode whose blocks each hold whole rows.
        let blocks: Vec<Vec<u8>> = if rows == 0 {
            Vec::new()
        } else if rows & 0x1f == 0 {
            vec![self.heap.get(rows).ok_or_else(|| bad("a missing table"))?.to_vec()]
        } else {
            let s = self.subnode(rows).ok_or_else(|| bad("a table in a missing subnode"))?;
            self.ndb.data(s.data)?
        };
        let mut out = Vec::new();
        for block in &blocks {
            for row in block.chunks_exact(row_size) {
                if out.len() >= limit.min(MAX_ROWS) {
                    return Ok(out);
                }
                let mut props = Vec::new();
                for &(id, kind, at, size, bit) in &columns {
                    let exists = row.get(ceb + bit / 8).is_some_and(|b| b & (0x80 >> (bit % 8)) != 0);
                    let Some(cell) = row.get(at..at + size).filter(|_| exists) else { continue };
                    let slot = match size {
                        1 => u32::from(cell.first().copied().unwrap_or(0)),
                        2 => u32::from(u16_at(cell, 0).unwrap_or(0)),
                        _ => u32_at(cell, 0).unwrap_or(0),
                    };
                    let v = match (fixed_size(kind), size) {
                        (Some(8), 8) => Ok(Val::Fixed(u64_at(cell, 0).unwrap_or(0))),
                        _ if inline(kind) => Ok(Val::Fixed(u64::from(slot))),
                        _ => self.hnid(slot, false),
                    };
                    if let Ok(v) = v {
                        props.push((id, kind, v));
                    }
                }
                out.push(Bag(props));
            }
        }
        Ok(out)
    }
}

/// `PidTagLtpRowId`: a row's id, the node it describes in a folder's tables.
pub(crate) const ROW_ID: u16 = 0x67f2;

#[cfg(test)]
mod tests;
