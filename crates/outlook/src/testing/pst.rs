//! Writes small `.pst` files for tests, from the structures MS-PST describes: blocks and pages,
//! the two B-trees, heaps, B-trees on heaps, property and table contexts, and the folders and
//! messages built from them. Options exercise what readers meet in real files: both formats, both
//! encodings, data trees, subnode trees of two levels, deeper B-trees and big values in subnodes.

use crate::ndb::{CRYPT, cyclic};

/// A property value to write.
#[derive(Debug, Clone)]
pub enum V {
    /// A 2- or 4-byte integer or a boolean, kept in the property's slot.
    Int(u32),
    /// An 8-byte value (a time, a 64-bit integer).
    Fixed(u64),
    /// Variable-size bytes (strings, binary).
    Bytes(Vec<u8>),
}

pub type Prop = (u16, u16, V);

/// A UTF-16 string property.
pub fn uni(id: u16, s: &str) -> Prop {
    (id, 0x001f, V::Bytes(s.encode_utf16().flat_map(u16::to_le_bytes).collect()))
}

/// An 8-bit string property.
pub fn s8(id: u16, b: &[u8]) -> Prop {
    (id, 0x001e, V::Bytes(b.to_vec()))
}

pub fn long(id: u16, v: u32) -> Prop {
    (id, 0x0003, V::Int(v))
}

pub fn bin(id: u16, b: &[u8]) -> Prop {
    (id, 0x0102, V::Bytes(b.to_vec()))
}

pub fn time(id: u16, filetime: u64) -> Prop {
    (id, 0x0040, V::Fixed(filetime))
}

#[derive(Debug, Clone, Default)]
pub struct Msg {
    pub props: Vec<Prop>,
    pub recipients: Vec<Vec<Prop>>,
    pub attachments: Vec<Att>,
}

#[derive(Debug, Clone, Default)]
pub struct Att {
    pub props: Vec<Prop>,
    pub embedded: Option<Msg>,
}

#[derive(Debug, Clone, Default)]
pub struct Folder {
    pub name: String,
    pub class: String,
    pub folders: Vec<Folder>,
    pub messages: Vec<Msg>,
}

/// How the file is written.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub unicode: bool,
    /// 0 none, 1 permute, 2 cyclic.
    pub crypt: u8,
    /// Heap blocks hold at most this many bytes (small values spread heaps over several blocks
    /// and node data over data trees).
    pub block: usize,
    /// Subnode trees of two levels (an SIBLOCK over SLBLOCKs).
    pub two_level_subnodes: bool,
    /// B-trees on heaps of two levels.
    pub two_level_bth: bool,
    /// Values larger than this go to subnodes.
    pub big: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { unicode: true, crypt: 1, block: 8000, two_level_subnodes: false, two_level_bth: false, big: 3580 }
    }
}

pub struct Sub {
    nid: u32,
    data: u64,
    sub: u64,
}

pub struct Writer {
    o: Options,
    /// (bid, data before encoding).
    blocks: Vec<(u64, Vec<u8>)>,
    /// (nid, data bid, subnode bid, parent nid).
    nodes: Vec<(u32, u64, u64, u32)>,
    next_bid: u64,
    next_internal_nid: u32,
}

impl Writer {
    pub fn new(o: Options) -> Self {
        Writer { o, blocks: Vec::new(), nodes: Vec::new(), next_bid: 4, next_internal_nid: 0x400 }
    }

    fn id(&self, v: u64) -> Vec<u8> {
        if self.o.unicode { v.to_le_bytes().to_vec() } else { (v as u32).to_le_bytes().to_vec() }
    }

    fn block(&mut self, data: Vec<u8>, internal: bool) -> u64 {
        assert!(data.len() <= 8176);
        let bid = self.next_bid | if internal { 2 } else { 0 };
        self.next_bid += 4;
        self.blocks.push((bid, data));
        bid
    }

    /// A node's data from its blocks: one block, or an XBLOCK over them.
    pub fn data(&mut self, blocks: Vec<Vec<u8>>) -> u64 {
        if blocks.is_empty() {
            return 0;
        }
        let total: usize = blocks.iter().map(Vec::len).sum();
        let bids: Vec<u64> = blocks.into_iter().map(|b| self.block(b, false)).collect();
        if bids.len() == 1 {
            return bids[0];
        }
        let mut x = vec![1u8, 1];
        x.extend_from_slice(&(bids.len() as u16).to_le_bytes());
        x.extend_from_slice(&(total as u32).to_le_bytes());
        for b in bids {
            x.extend(self.id(b));
        }
        self.block(x, true)
    }

    /// Bytes split into data blocks of at most the block size.
    fn bytes_data(&mut self, b: &[u8]) -> u64 {
        let size = self.o.block.min(8176);
        self.data(b.chunks(size).map(<[u8]>::to_vec).collect())
    }

    fn subnodes(&mut self, mut subs: Vec<Sub>) -> u64 {
        if subs.is_empty() {
            return 0;
        }
        subs.sort_by_key(|s| s.nid);
        let sl = |w: &mut Writer, part: &[Sub]| {
            let mut b = vec![2u8, 0];
            b.extend_from_slice(&(part.len() as u16).to_le_bytes());
            if w.o.unicode {
                b.extend_from_slice(&[0; 4]);
            }
            for s in part {
                b.extend(w.id(u64::from(s.nid)));
                b.extend(w.id(s.data));
                b.extend(w.id(s.sub));
            }
            w.block(b, true)
        };
        if !self.o.two_level_subnodes || subs.len() < 2 {
            return sl(self, &subs);
        }
        let half = subs.len() / 2;
        let parts = [&subs[..half], &subs[half..]];
        let mut si = vec![2u8, 1, 2, 0];
        if self.o.unicode {
            si.extend_from_slice(&[0; 4]);
        }
        for part in parts {
            let bid = sl(self, part);
            si.extend(self.id(u64::from(part[0].nid)));
            si.extend(self.id(bid));
        }
        self.block(si, true)
    }

    fn internal_nid(&mut self) -> u32 {
        self.next_internal_nid += 1;
        (self.next_internal_nid << 5) | 0x1f
    }

    /// A value's HNID: a heap allocation, or a subnode for a big value.
    fn hnid(&mut self, heap: &mut Heap, subs: &mut Vec<Sub>, b: &[u8]) -> u32 {
        if b.len() > self.o.big {
            let nid = self.internal_nid();
            let data = self.bytes_data(b);
            subs.push(Sub { nid, data, sub: 0 });
            nid
        } else {
            heap.alloc(b.to_vec())
        }
    }

    /// A B-tree on `heap` over sorted `records` (key + data): its header's HID.
    fn bth(&self, heap: &mut Heap, key: u8, data: u8, records: Vec<Vec<u8>>) -> u32 {
        let root = if records.is_empty() {
            0
        } else if self.o.two_level_bth && records.len() >= 2 {
            let half = records.len() / 2;
            let mut index = Vec::new();
            for part in [&records[..half], &records[half..]] {
                let hid = heap.alloc(part.concat());
                index.extend_from_slice(&part[0][..usize::from(key)]);
                index.extend_from_slice(&hid.to_le_bytes());
            }
            heap.alloc(index)
        } else {
            heap.alloc(records.concat())
        };
        let levels = u8::from(self.o.two_level_bth && records.len() >= 2);
        let mut h = vec![0xb5, key, data, levels];
        h.extend_from_slice(&root.to_le_bytes());
        heap.alloc(h)
    }

    /// A property context: its heap blocks and subnodes.
    fn pc(&mut self, props: &[Prop], subs: &mut Vec<Sub>) -> Vec<Vec<u8>> {
        let mut heap = Heap::new(self.o.block);
        let mut sorted = props.to_vec();
        sorted.sort_by_key(|p| p.0);
        let mut records = Vec::new();
        for (id, kind, v) in sorted {
            let slot = match v {
                V::Int(n) => n,
                V::Fixed(n) => heap.alloc(n.to_le_bytes().to_vec()),
                V::Bytes(b) => self.hnid(&mut heap, subs, &b),
            };
            let mut r = id.to_le_bytes().to_vec();
            r.extend_from_slice(&kind.to_le_bytes());
            r.extend_from_slice(&slot.to_le_bytes());
            records.push(r);
        }
        let root = self.bth(&mut heap, 2, 6, records);
        heap.finish(0xbc, root)
    }

    /// A table context with columns `cols` (id, type) and rows of values; row ids come first.
    fn tc(&mut self, cols: &[(u16, u16)], rows: &[(u32, Vec<Option<V>>)], subs: &mut Vec<Sub>) -> Vec<Vec<u8>> {
        let mut heap = Heap::new(self.o.block);
        let size = |kind: u16| match kind {
            0x0002 => 2,
            0x000b => 1,
            0x0040 | 0x0014 => 8,
            _ => 4,
        };
        // Row id at 0 and bit 0; then 8-, 4-, 2- and 1-byte columns.
        let mut all: Vec<(u16, u16)> = vec![(0x67f2, 0x0003)];
        all.extend_from_slice(cols);
        let mut layout = vec![(0usize, 4usize, 0usize)];
        let mut at = 4;
        for want in [8, 4, 2, 1] {
            for (i, c) in cols.iter().enumerate() {
                if size(c.1) == want {
                    layout.push((i + 1, want, at));
                    at += want;
                }
            }
        }
        let ends = {
            let end_of = |w: usize| layout.iter().filter(|l| l.1 >= w).map(|l| l.2 + l.1).max().unwrap_or(4);
            [end_of(4), end_of(2), end_of(1)]
        };
        let ceb = ends[2];
        let row_size = ceb + all.len().div_ceil(8);
        let mut matrix = Vec::new();
        for (rid, values) in rows {
            let mut row = vec![0u8; row_size];
            row[..4].copy_from_slice(&rid.to_le_bytes());
            row[ceb] |= 0x80;
            for &(col, width, off) in &layout[1..] {
                let Some(Some(v)) = values.get(col - 1) else { continue };
                let bytes: Vec<u8> = match v {
                    V::Int(n) => n.to_le_bytes()[..width].to_vec(),
                    V::Fixed(n) => n.to_le_bytes().to_vec(),
                    V::Bytes(b) => self.hnid(&mut heap, subs, b).to_le_bytes().to_vec(),
                };
                row[off..off + width].copy_from_slice(&bytes);
                row[ceb + col / 8] |= 0x80 >> (col % 8);
            }
            matrix.push(row);
        }
        // Row index: row id → row number.
        let mut index: Vec<Vec<u8>> = rows
            .iter()
            .enumerate()
            .map(|(i, (rid, _))| {
                let mut r = rid.to_le_bytes().to_vec();
                if self.o.unicode {
                    r.extend_from_slice(&(i as u32).to_le_bytes());
                } else {
                    r.extend_from_slice(&(i as u16).to_le_bytes());
                }
                r
            })
            .collect();
        index.sort();
        let row_index = self.bth(&mut heap, 4, if self.o.unicode { 4 } else { 2 }, index);
        let total = matrix.len() * row_size;
        let rows_hnid = if matrix.is_empty() {
            0
        } else if total <= self.o.big {
            heap.alloc(matrix.concat())
        } else {
            let per = (8176 / row_size).max(1);
            let blocks: Vec<Vec<u8>> = matrix.chunks(per).map(|c| c.concat()).collect();
            let nid = self.internal_nid();
            let data = self.data(blocks);
            subs.push(Sub { nid, data, sub: 0 });
            nid
        };
        let mut info = vec![0x7c, all.len() as u8];
        for e in ends {
            info.extend_from_slice(&(e as u16).to_le_bytes());
        }
        info.extend_from_slice(&(row_size as u16).to_le_bytes());
        info.extend_from_slice(&row_index.to_le_bytes());
        info.extend_from_slice(&rows_hnid.to_le_bytes());
        info.extend_from_slice(&0u32.to_le_bytes());
        let mut descs: Vec<(u32, u16, u8, u8)> = Vec::new();
        for (i, (id, kind)) in all.iter().enumerate() {
            let l = layout.iter().find(|l| l.0 == i).copied().unwrap_or((i, 4, 0));
            descs.push(((u32::from(*id) << 16) | u32::from(*kind), l.2 as u16, l.1 as u8, i as u8));
        }
        descs.sort_by_key(|d| d.0);
        for (tag, off, width, bit) in descs {
            info.extend_from_slice(&tag.to_le_bytes());
            info.extend_from_slice(&off.to_le_bytes());
            info.push(width);
            info.push(bit);
        }
        let root = heap.alloc(info);
        heap.finish(0x7c, root)
    }

    pub fn node(&mut self, nid: u32, heap: Vec<Vec<u8>>, subs: Vec<Sub>, parent: u32) {
        let data = self.data(heap);
        let sub = self.subnodes(subs);
        self.nodes.push((nid, data, sub, parent));
    }

    /// A message's node data and subnodes (also used for attached messages).
    fn message(&mut self, m: &Msg, subs: &mut Vec<Sub>) -> Vec<Vec<u8>> {
        let heap = self.pc(&m.props, subs);
        if !m.recipients.is_empty() {
            let cols: Vec<(u16, u16)> = columns(&m.recipients);
            let rows: Vec<(u32, Vec<Option<V>>)> = m.recipients.iter().enumerate().map(|(i, r)| (i as u32, values(&cols, r))).collect();
            let mut tsubs = Vec::new();
            let t = self.tc(&cols, &rows, &mut tsubs);
            let data = self.data(t);
            let sub = self.subnodes(tsubs);
            subs.push(Sub { nid: 0x692, data, sub });
        }
        if !m.attachments.is_empty() {
            let mut rows = Vec::new();
            for (i, a) in m.attachments.iter().enumerate() {
                let nid = ((0x100 + i as u32) << 5) | 0x05;
                let mut asubs = Vec::new();
                let mut props = a.props.clone();
                if let Some(inner) = &a.embedded {
                    let mut isubs = Vec::new();
                    let iheap = self.message(inner, &mut isubs);
                    let data = self.data(iheap);
                    let sub = self.subnodes(isubs);
                    let inid = self.internal_nid();
                    asubs.push(Sub { nid: inid, data, sub });
                    let mut obj = inid.to_le_bytes().to_vec();
                    obj.extend_from_slice(&0u32.to_le_bytes());
                    props.push((0x3701, 0x000d, V::Bytes(obj)));
                }
                let aheap = self.pc(&props, &mut asubs);
                let data = self.data(aheap);
                let sub = self.subnodes(asubs);
                subs.push(Sub { nid, data, sub });
                rows.push((nid, vec![Some(V::Int(i as u32))]));
            }
            let mut tsubs = Vec::new();
            let t = self.tc(&[(0x0e21, 0x0003)], &rows, &mut tsubs);
            let data = self.data(t);
            let sub = self.subnodes(tsubs);
            subs.push(Sub { nid: 0x671, data, sub });
        }
        heap
    }

    /// The finished file.
    pub fn finish(mut self) -> Vec<u8> {
        let o = self.o;
        let header = if o.unicode { 564 } else { 512 };
        let mut file = vec![0u8; header.max(1024)];
        // Blocks, each aligned to 64 bytes with a trailer.
        let mut bbt: Vec<(u64, u64, u16)> = Vec::new();
        let blocks = std::mem::take(&mut self.blocks);
        for (bid, mut data) in blocks {
            let cb = data.len();
            if bid & 2 == 0 {
                match o.crypt {
                    1 => data.iter_mut().for_each(|b| *b = CRYPT[usize::from(*b)]),
                    2 => cyclic(&mut data, bid as u32),
                    _ => {}
                }
            }
            let at = file.len().div_ceil(64) * 64;
            file.resize(at, 0);
            file.extend_from_slice(&data);
            let trailer = if o.unicode { 16 } else { 12 };
            let end = (at + cb + trailer).div_ceil(64) * 64;
            file.resize(end - trailer, 0);
            file.extend_from_slice(&(cb as u16).to_le_bytes());
            file.extend_from_slice(&[0; 2]);
            if o.unicode {
                file.extend_from_slice(&[0; 4]);
                file.extend_from_slice(&bid.to_le_bytes());
            } else {
                file.extend_from_slice(&(bid as u32).to_le_bytes());
                file.extend_from_slice(&[0; 4]);
            }
            bbt.push((bid, at as u64, cb as u16));
        }
        bbt.sort();
        let mut nodes = std::mem::take(&mut self.nodes);
        nodes.sort();
        let bbt_entries: Vec<(u64, Vec<u8>)> = bbt
            .iter()
            .map(|&(bid, ib, cb)| {
                let mut e = self.id(bid);
                e.extend(self.id(ib));
                e.extend_from_slice(&cb.to_le_bytes());
                e.extend_from_slice(&2u16.to_le_bytes());
                if o.unicode {
                    e.extend_from_slice(&[0; 4]);
                }
                (bid, e)
            })
            .collect();
        let nbt_entries: Vec<(u64, Vec<u8>)> = nodes
            .iter()
            .map(|&(nid, data, sub, parent)| {
                let mut e = self.id(u64::from(nid));
                e.extend(self.id(data));
                e.extend(self.id(sub));
                e.extend_from_slice(&parent.to_le_bytes());
                if o.unicode {
                    e.extend_from_slice(&[0; 4]);
                }
                (u64::from(nid), e)
            })
            .collect();
        let bbt_root = self.tree(&mut file, bbt_entries, 0x80);
        let nbt_root = self.tree(&mut file, nbt_entries, 0x81);
        file[0..4].copy_from_slice(b"!BDN");
        file[8..10].copy_from_slice(b"SM");
        if o.unicode {
            file[10..12].copy_from_slice(&23u16.to_le_bytes());
            file[224..232].copy_from_slice(&nbt_root.to_le_bytes());
            file[240..248].copy_from_slice(&bbt_root.to_le_bytes());
            file[513] = o.crypt;
        } else {
            file[10..12].copy_from_slice(&14u16.to_le_bytes());
            file[188..192].copy_from_slice(&(nbt_root as u32).to_le_bytes());
            file[196..200].copy_from_slice(&(bbt_root as u32).to_le_bytes());
            file[461] = o.crypt;
        }
        file
    }

    /// Writes a B-tree of `entries` (key, bytes) as pages; the root page's offset.
    fn tree(&mut self, file: &mut Vec<u8>, entries: Vec<(u64, Vec<u8>)>, ptype: u8) -> u64 {
        let (room, meta, trailer) = if self.o.unicode { (488, 488, 496) } else { (496, 496, 500) };
        let mut level = 0u8;
        let mut current = entries;
        loop {
            let size = current.first().map_or(if self.o.unicode { 24 } else { 12 }, |e| e.1.len());
            let per = room / size;
            let mut parents = Vec::new();
            for chunk in current.chunks(per.max(1)) {
                let at = file.len().div_ceil(512) * 512;
                file.resize(at + 512, 0);
                for (i, (_, e)) in chunk.iter().enumerate() {
                    file[at + i * size..at + (i + 1) * size].copy_from_slice(e);
                }
                file[at + meta] = chunk.len() as u8;
                file[at + meta + 1] = per as u8;
                file[at + meta + 2] = size as u8;
                file[at + meta + 3] = level;
                file[at + trailer] = ptype;
                file[at + trailer + 1] = ptype;
                let key = chunk.first().map_or(0, |c| c.0);
                let mut e = self.id(key);
                e.extend(self.id(0));
                e.extend(self.id(at as u64));
                parents.push((key, e, at as u64));
            }
            if parents.len() == 1 {
                return parents[0].2;
            }
            current = parents.into_iter().map(|(k, e, _)| (k, e)).collect();
            level += 1;
        }
    }

    /// A whole archive: the store, the root folder, the top folder and `folders` under it.
    pub fn archive(mut self, name: &str, folders: &[Folder]) -> Vec<u8> {
        let top: u32 = (0x10 << 5) | 0x02;
        let mut entry = vec![0u8; 20];
        entry.extend_from_slice(&top.to_le_bytes());
        let mut subs = Vec::new();
        let store = self.pc(&[uni(0x3001, name), bin(0x35e0, &entry)], &mut subs);
        self.node(0x21, store, subs, 0);
        let root = 0x122;
        self.folder_node(root, "", "", &[top], &[], 0);
        let mut next = 0x20u32;
        let children = self.folders(folders, top, &mut next);
        self.folder_node(top, "Top of Personal Folders", "", &children, &[], root);
        self.finish()
    }

    fn folders(&mut self, folders: &[Folder], parent: u32, next: &mut u32) -> Vec<u32> {
        let mut nids = Vec::new();
        for f in folders {
            *next += 1;
            let nid = (*next << 5) | 0x02;
            let children = self.folders(&f.folders, nid, next);
            let mut messages = Vec::new();
            for m in &f.messages {
                *next += 1;
                let mid = (*next << 5) | 0x04;
                let mut subs = Vec::new();
                let heap = self.message(m, &mut subs);
                self.node(mid, heap, subs, nid);
                messages.push(mid);
            }
            self.folder_node(nid, &f.name, &f.class, &children, &messages, parent);
            nids.push(nid);
        }
        nids
    }

    fn folder_node(&mut self, nid: u32, name: &str, class: &str, children: &[u32], messages: &[u32], parent: u32) {
        let mut props = vec![uni(0x3001, name), long(0x3602, messages.len() as u32), long(0x360a, u32::from(!children.is_empty()))];
        if !class.is_empty() {
            props.push(uni(0x3613, class));
        }
        let mut subs = Vec::new();
        let pc = self.pc(&props, &mut subs);
        self.node(nid, pc, subs, parent);
        for (kind, ids) in [(0x0d, children), (0x0e, messages)] {
            let rows: Vec<(u32, Vec<Option<V>>)> = ids.iter().map(|&c| (c, vec![Some(V::Int(c))])).collect();
            let mut tsubs = Vec::new();
            let t = self.tc(&[(0x0e17, 0x0003)], &rows, &mut tsubs);
            self.node((nid & !0x1f) | kind, t, tsubs, 0);
        }
    }
}

fn columns(rows: &[Vec<Prop>]) -> Vec<(u16, u16)> {
    let mut cols: Vec<(u16, u16)> = Vec::new();
    for r in rows {
        for (id, kind, _) in r {
            if !cols.contains(&(*id, *kind)) {
                cols.push((*id, *kind));
            }
        }
    }
    cols
}

fn values(cols: &[(u16, u16)], row: &[Prop]) -> Vec<Option<V>> {
    cols.iter().map(|c| row.iter().find(|p| p.0 == c.0 && p.1 == c.1).map(|p| p.2.clone())).collect()
}

/// A heap being built: allocations in blocks of at most `limit` bytes.
struct Heap {
    blocks: Vec<Vec<Vec<u8>>>,
    limit: usize,
}

impl Heap {
    fn new(limit: usize) -> Self {
        Heap { blocks: vec![Vec::new()], limit }
    }

    fn alloc(&mut self, b: Vec<u8>) -> u32 {
        let used = |allocs: &Vec<Vec<u8>>| 12 + allocs.iter().map(Vec::len).sum::<usize>() + 4 + 2 * (allocs.len() + 2);
        let last = self.blocks.len() - 1;
        if !self.blocks[last].is_empty() && used(&self.blocks[last]) + b.len() > self.limit {
            self.blocks.push(Vec::new());
        }
        let block = self.blocks.len() - 1;
        self.blocks[block].push(b);
        ((self.blocks[block].len() as u32) << 5) | ((block as u32) << 16)
    }

    fn finish(self, client: u8, root: u32) -> Vec<Vec<u8>> {
        self.blocks
            .into_iter()
            .enumerate()
            .map(|(i, allocs)| {
                let head = if i == 0 { 12 } else { 2 };
                let mut b = vec![0u8; head];
                if i == 0 {
                    b[2] = 0xec;
                    b[3] = client;
                    b[4..8].copy_from_slice(&root.to_le_bytes());
                }
                let mut offsets = Vec::new();
                for a in &allocs {
                    offsets.push(b.len() as u16);
                    b.extend_from_slice(a);
                }
                offsets.push(b.len() as u16);
                if b.len() % 2 == 1 {
                    b.push(0);
                }
                let map = b.len() as u16;
                b[0..2].copy_from_slice(&map.to_le_bytes());
                b.extend_from_slice(&(allocs.len() as u16).to_le_bytes());
                b.extend_from_slice(&0u16.to_le_bytes());
                for o in offsets {
                    b.extend_from_slice(&o.to_le_bytes());
                }
                b
            })
            .collect()
    }
}
