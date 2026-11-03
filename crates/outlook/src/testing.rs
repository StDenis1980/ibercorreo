//! Writing small compound files and `.msg` files, for tests here and in the crates that use
//! this one (the `testing` feature). Not for real files: nothing is checked.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

pub mod pst;

use crate::cfb::{END_OF_CHAIN, ENTRY_LEN, FREE, HEADER_DIFAT, NO_STREAM, SIGNATURE};

/// What a test compound file holds.
pub enum Node {
    Stream(String, Vec<u8>),
    Storage(String, Vec<Node>),
}

pub fn stream(name: &str, data: impl Into<Vec<u8>>) -> Node {
    Node::Stream(name.into(), data.into())
}

pub fn storage(name: &str, children: Vec<Node>) -> Node {
    Node::Storage(name.into(), children)
}

struct Flat {
    name: String,
    kind: u8,
    child: u32,
    right: u32,
    data: Vec<u8>,
    start: u32,
}

fn flatten(name: &str, kind: u8, children: Vec<Node>, out: &mut Vec<Flat>) -> u32 {
    let me = out.len();
    out.push(Flat { name: name.into(), kind, child: NO_STREAM, right: NO_STREAM, data: Vec::new(), start: END_OF_CHAIN });
    let mut ids = Vec::new();
    for c in children {
        ids.push(match c {
            Node::Stream(n, d) => {
                out.push(Flat { name: n, kind: 2, child: NO_STREAM, right: NO_STREAM, data: d, start: END_OF_CHAIN });
                (out.len() - 1) as u32
            }
            Node::Storage(n, cs) => flatten(&n, 1, cs, out),
        });
    }
    // Siblings as a chain of right links: a valid (if unbalanced) tree.
    if let Some(&first) = ids.first() {
        out[me].child = first;
    }
    for w in ids.windows(2) {
        out[w[0] as usize].right = w[1];
    }
    me as u32
}

/// A compound file with `children` under the root, in sectors of `1 << shift` bytes.
pub fn build(children: Vec<Node>, shift: u32) -> Vec<u8> {
    let ss = 1usize << shift;
    let mut flat = Vec::new();
    flatten("Root Entry", 5, children, &mut flat);
    // Small streams into the mini stream.
    let mut mini = Vec::new();
    let mut mini_fat: Vec<u32> = Vec::new();
    for f in flat.iter_mut().filter(|f| f.kind == 2 && !f.data.is_empty() && f.data.len() < 4096) {
        let first = mini_fat.len() as u32;
        let n = f.data.len().div_ceil(64);
        for i in 0..n {
            mini_fat.push(if i + 1 == n { END_OF_CHAIN } else { first + i as u32 + 1 });
        }
        mini.extend_from_slice(&f.data);
        mini.resize(mini_fat.len() * 64, 0);
        f.start = first;
    }
    let mini_fat_bytes: Vec<u8> = mini_fat.iter().flat_map(|v| v.to_le_bytes()).collect();
    let dir_len = flat.len() * ENTRY_LEN;
    // Regions in sector order after the FAT and DIFAT sectors: directory, mini FAT, mini stream,
    // then each large stream.
    let large: Vec<usize> = (0..flat.len()).filter(|&i| flat[i].kind == 2 && flat[i].data.len() >= 4096).collect();
    let mut sizes = vec![dir_len, mini_fat_bytes.len(), mini.len()];
    sizes.extend(large.iter().map(|&i| flat[i].data.len()));
    let content: usize = sizes.iter().map(|s| s.div_ceil(ss)).sum();
    let per = ss / 4;
    let (mut fats, mut difats) = (1usize, 0usize);
    loop {
        let need = (content + fats + difats).div_ceil(per);
        let nd = need.saturating_sub(HEADER_DIFAT).div_ceil(per - 1);
        if need == fats && nd == difats {
            break;
        }
        (fats, difats) = (need, nd);
    }
    let total = fats + difats + content;
    let mut fat = vec![FREE; fats * per];
    fat[..fats].fill(0xffff_fffd);
    fat[fats..fats + difats].fill(0xffff_fffc);
    let mut next = fats + difats;
    let mut starts = Vec::new();
    for &len in &sizes {
        let n = len.div_ceil(ss);
        starts.push(if n == 0 { END_OF_CHAIN } else { next as u32 });
        for i in 0..n {
            fat[next + i] = if i + 1 == n { END_OF_CHAIN } else { (next + i + 1) as u32 };
        }
        next += n;
    }
    assert_eq!(next, total);
    flat[0].start = starts[2];
    flat[0].data = vec![0; mini.len()];
    let root_size = mini.len();
    for (k, &i) in large.iter().enumerate() {
        flat[i].start = starts[3 + k];
    }
    let mut dir = Vec::new();
    for f in &flat {
        let mut e = vec![0u8; ENTRY_LEN];
        let units: Vec<u8> = f.name.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        e[..units.len()].copy_from_slice(&units);
        e[64..66].copy_from_slice(&(units.len() as u16).to_le_bytes());
        e[66] = f.kind;
        e[67] = 1;
        e[68..72].copy_from_slice(&NO_STREAM.to_le_bytes());
        e[72..76].copy_from_slice(&f.right.to_le_bytes());
        e[76..80].copy_from_slice(&f.child.to_le_bytes());
        e[116..120].copy_from_slice(&f.start.to_le_bytes());
        let size = if f.kind == 5 { root_size } else { f.data.len() };
        e[120..128].copy_from_slice(&(size as u64).to_le_bytes());
        dir.extend_from_slice(&e);
    }
    let mut header = vec![0u8; ss.max(512)];
    header[..8].copy_from_slice(&SIGNATURE);
    header[0x18..0x1a].copy_from_slice(&0x3eu16.to_le_bytes());
    header[0x1a..0x1c].copy_from_slice(&(if shift == 9 { 3u16 } else { 4 }).to_le_bytes());
    header[0x1c..0x1e].copy_from_slice(&0xfffeu16.to_le_bytes());
    header[0x1e..0x20].copy_from_slice(&(shift as u16).to_le_bytes());
    header[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    header[0x2c..0x30].copy_from_slice(&(fats as u32).to_le_bytes());
    header[0x30..0x34].copy_from_slice(&starts[0].to_le_bytes());
    header[0x38..0x3c].copy_from_slice(&4096u32.to_le_bytes());
    header[0x3c..0x40].copy_from_slice(&starts[1].to_le_bytes());
    header[0x40..0x44].copy_from_slice(&(mini_fat.len().div_ceil(per) as u32).to_le_bytes());
    header[0x44..0x48].copy_from_slice(&(if difats > 0 { fats as u32 } else { END_OF_CHAIN }).to_le_bytes());
    header[0x48..0x4c].copy_from_slice(&(difats as u32).to_le_bytes());
    let fat_ids: Vec<u32> = (0..fats as u32).collect();
    for i in 0..HEADER_DIFAT {
        let v = fat_ids.get(i).copied().unwrap_or(FREE);
        header[0x4c + i * 4..0x50 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut out = header;
    for s in 0..fats {
        out.extend(fat[s * per..(s + 1) * per].iter().flat_map(|v| v.to_le_bytes()));
    }
    let rest: Vec<u32> = fat_ids.iter().skip(HEADER_DIFAT).copied().collect();
    for d in 0..difats {
        let mut sector: Vec<u32> = rest.iter().skip(d * (per - 1)).take(per - 1).copied().collect();
        sector.resize(per - 1, FREE);
        sector.push(if d + 1 == difats { END_OF_CHAIN } else { (fats + d + 1) as u32 });
        out.extend(sector.iter().flat_map(|v| v.to_le_bytes()));
    }
    let mut region = |bytes: &[u8]| {
        out.extend_from_slice(bytes);
        out.resize(out.len().div_ceil(ss) * ss, 0);
    };
    region(&dir);
    region(&mini_fat_bytes);
    region(&mini);
    for &i in &large {
        region(&flat[i].data);
    }
    out
}

/// A `.msg` property stream: a header of `header` bytes, then fixed-size properties
/// `(id, type, value)`.
pub fn props(header: usize, fixed: &[(u16, u16, u64)]) -> Node {
    let mut b = vec![0u8; header];
    for &(id, kind, value) in fixed {
        b.extend_from_slice(&((u32::from(id) << 16) | u32::from(kind)).to_le_bytes());
        b.extend_from_slice(&6u32.to_le_bytes());
        b.extend_from_slice(&value.to_le_bytes());
    }
    stream("__properties_version1.0", b)
}

/// A Unicode string property.
pub fn uni(id: u16, text: &str) -> Node {
    stream(&format!("__substg1.0_{id:04X}001F"), text.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>())
}

/// A small mail message saved from Outlook, from `sender` (an SMTP address) with `subject` and
/// `body`.
pub fn mail(subject: &str, sender: &str, body: &str) -> Vec<u8> {
    build(vec![props(32, &[]), uni(0x001a, "IPM.Note"), uni(0x0037, subject), uni(0x0c1f, sender), uni(0x0c1e, "SMTP"), uni(0x1000, body)], 9)
}

/// Packs `rtf` as `PidTagRtfCompressed` does (MS-OXRTFCP), with greedy dictionary references.
pub fn compress_rtf(rtf: &[u8]) -> Vec<u8> {
    const DICT: usize = 4096;
    let mut dict = [0u8; DICT];
    dict[..crate::rtf::PREFIX.len()].copy_from_slice(crate::rtf::PREFIX);
    let mut write = crate::rtf::PREFIX.len();
    let mut payload = Vec::new();
    let (mut control_at, mut bit) = (0usize, 8u8);
    let mut pos = 0;
    let mut item = |payload: &mut Vec<u8>, reference: bool, bytes: &[u8]| {
        if bit == 8 {
            control_at = payload.len();
            payload.push(0);
            bit = 0;
        }
        if reference {
            payload[control_at] |= 1 << bit;
        }
        bit += 1;
        payload.extend_from_slice(bytes);
    };
    while pos < rtf.len() {
        // The longest match, reading bytes this very copy writes as it goes.
        let (mut best, mut best_len) = (0, 0);
        for offset in (0..DICT).filter(|&o| o != write) {
            let mut len = 0;
            while len < 17 && pos + len < rtf.len() {
                let j = (offset + len) % DICT;
                let back = (j + DICT - write) % DICT;
                let b = if back < len { rtf[pos + back] } else { dict[j] };
                if b != rtf[pos + len] {
                    break;
                }
                len += 1;
            }
            if len > best_len {
                (best, best_len) = (offset, len);
            }
        }
        let n = if best_len >= 2 {
            item(&mut payload, true, &(((best as u16) << 4) | (best_len as u16 - 2)).to_be_bytes());
            best_len
        } else {
            item(&mut payload, false, &[rtf[pos]]);
            1
        };
        for &b in &rtf[pos..pos + n] {
            dict[write] = b;
            write = (write + 1) % DICT;
        }
        pos += n;
    }
    item(&mut payload, true, &((write as u16) << 4).to_be_bytes());
    let mut out = Vec::new();
    out.extend_from_slice(&(payload.len() as u32 + 12).to_le_bytes());
    out.extend_from_slice(&(rtf.len() as u32).to_le_bytes());
    out.extend_from_slice(b"LZFu");
    out.extend_from_slice(&crate::rtf::crc(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}
