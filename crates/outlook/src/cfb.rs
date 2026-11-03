//! Compound files (MS-CFB, "structured storage"): the container Outlook writes `.msg` files in.
//! A small file system inside one file: a FAT chains sectors into streams, a directory names the
//! streams and the storages that hold them, and streams under 4 KiB live in a mini stream with
//! its own mini FAT.
//!
//! Read-only, and every number in the file is treated as hostile: chains are walked at most once
//! through each sector, the directory tree with a seen set, and nothing reads past the file.

use ibercorreo_core::{Error, Result};

pub(crate) const SIGNATURE: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
pub(crate) const END_OF_CHAIN: u32 = 0xffff_fffe;
pub(crate) const FREE: u32 = 0xffff_ffff;
/// Highest sector number that names a sector.
const MAX_REGULAR: u32 = 0xffff_fffa;
pub(crate) const NO_STREAM: u32 = 0xffff_ffff;
pub(crate) const HEADER_DIFAT: usize = 109;
pub(crate) const ENTRY_LEN: usize = 128;
/// Largest compound file read.
pub const MAX_FILE: usize = 512 * 1024 * 1024;
/// Most directory entries read.
const MAX_ENTRIES: usize = 1_000_000;

fn bad(detail: &str) -> Error {
    Error::invalid("compound file", detail)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]))
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

pub(crate) fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u32_at(b, at)?) | (u64::from(u32_at(b, at.checked_add(4)?)?) << 32))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Storage,
    Stream,
    Root,
    /// A free directory slot.
    Unused,
}

/// One directory entry.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    pub size: u64,
}

/// An open compound file.
pub struct Cfb<'a> {
    data: &'a [u8],
    shift: u32,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    cutoff: u64,
    entries: Vec<Entry>,
}

impl<'a> Cfb<'a> {
    pub fn open(data: &'a [u8]) -> Result<Cfb<'a>> {
        if data.len() > MAX_FILE {
            return Err(Error::TooLarge { what: "compound file", size: data.len(), limit: MAX_FILE });
        }
        if data.get(..8) != Some(SIGNATURE.as_slice()) {
            return Err(bad("not a compound file"));
        }
        let shift = u32::from(u16_at(data, 0x1e).ok_or_else(|| bad("short header"))?);
        if shift != 9 && shift != 12 {
            return Err(bad("unknown sector size"));
        }
        let header = |at: usize| u32_at(data, at).ok_or_else(|| bad("short header"));
        let mut cfb = Cfb {
            data,
            shift,
            fat: Vec::new(),
            mini_fat: Vec::new(),
            mini_stream: Vec::new(),
            cutoff: u64::from(header(0x38)?),
            entries: Vec::new(),
        };
        // The FAT's own sectors: 109 in the header, the rest in a chain of DIFAT sectors.
        let mut fat_sectors = Vec::new();
        for i in 0..HEADER_DIFAT {
            let s = header(0x4c + i * 4)?;
            if s <= MAX_REGULAR {
                fat_sectors.push(s);
            }
        }
        let per_difat = cfb.sector_len() / 4 - 1;
        let mut difat = header(0x44)?;
        let mut seen = 0usize;
        while difat <= MAX_REGULAR {
            seen += 1;
            if seen > cfb.sector_count() {
                return Err(bad("DIFAT chain loops"));
            }
            let sector = cfb.sector(difat)?;
            for i in 0..per_difat {
                let s = u32_at(sector, i * 4).ok_or_else(|| bad("short DIFAT sector"))?;
                if s <= MAX_REGULAR {
                    fat_sectors.push(s);
                }
            }
            difat = u32_at(sector, per_difat * 4).ok_or_else(|| bad("short DIFAT sector"))?;
        }
        if fat_sectors.len() > cfb.sector_count() {
            return Err(bad("more FAT sectors than sectors"));
        }
        for s in fat_sectors {
            let sector = cfb.sector(s)?;
            cfb.fat.extend(sector.chunks_exact(4).filter_map(|c| u32_at(c, 0)));
        }
        // The directory, then the mini FAT and the mini stream (the root entry's stream).
        let dir = cfb.chain(header(0x30)?, None)?;
        for raw in dir.chunks_exact(ENTRY_LEN).take(MAX_ENTRIES) {
            cfb.entries.push(entry(raw));
        }
        if cfb.entries.first().map(|e| e.kind) != Some(Kind::Root) {
            return Err(bad("no root entry"));
        }
        let mini_fat = cfb.chain(header(0x3c)?, None)?;
        cfb.mini_fat = mini_fat.chunks_exact(4).filter_map(|c| u32_at(c, 0)).collect();
        if let Some(root) = cfb.entries.first() {
            let (start, size) = (root.start, root.size);
            cfb.mini_stream = cfb.chain(start, Some(size))?;
        }
        Ok(cfb)
    }

    fn sector_len(&self) -> usize {
        1usize << self.shift
    }

    /// Sectors in the file after the header.
    fn sector_count(&self) -> usize {
        self.data.len() >> self.shift
    }

    fn sector(&self, n: u32) -> Result<&'a [u8]> {
        let len = self.sector_len();
        let start = usize::try_from(n).ok().and_then(|n| n.checked_add(1)).and_then(|n| n.checked_mul(len)).ok_or_else(|| bad("sector number"))?;
        let data: &'a [u8] = self.data;
        // A file may end inside its last sector: what is there is read.
        data.get(start..start.saturating_add(len).min(data.len())).filter(|s| !s.is_empty()).ok_or_else(|| bad("sector past the end of the file"))
    }

    /// The bytes of the chain starting at `start`, cut to `size` when given.
    fn chain(&self, start: u32, size: Option<u64>) -> Result<Vec<u8>> {
        let want = size.map(|s| usize::try_from(s).unwrap_or(usize::MAX));
        if want.is_some_and(|w| w > self.data.len()) {
            return Err(bad("a stream larger than the file"));
        }
        let mut out = Vec::with_capacity(want.unwrap_or(0));
        let mut next = start;
        let mut seen = vec![false; self.fat.len()];
        while next != END_OF_CHAIN && next != FREE {
            if want.is_some_and(|w| out.len() >= w) {
                break;
            }
            if visited(&mut seen, next) {
                return Err(bad("sector chain loops or breaks"));
            }
            out.extend_from_slice(self.sector(next)?);
            next = *self.fat.get(usize::try_from(next).unwrap_or(usize::MAX)).ok_or_else(|| bad("sector chain leaves the FAT"))?;
        }
        if let Some(w) = want {
            if out.len() < w {
                return Err(bad("a stream shorter than its size"));
            }
            out.truncate(w);
        }
        Ok(out)
    }

    fn mini_chain(&self, start: u32, size: u64) -> Result<Vec<u8>> {
        const MINI: usize = 64;
        let want = usize::try_from(size).map_err(|_| bad("stream size"))?;
        if want > self.mini_stream.len() {
            return Err(bad("a stream larger than the mini stream"));
        }
        let mut out = Vec::with_capacity(want.min(self.mini_stream.len()));
        let mut next = start;
        let mut seen = vec![false; self.mini_fat.len()];
        while out.len() < want {
            if visited(&mut seen, next) {
                return Err(bad("mini sector chain loops or breaks"));
            }
            let at = usize::try_from(next).ok().and_then(|n| n.checked_mul(MINI)).ok_or_else(|| bad("mini sector"))?;
            let sector = self.mini_stream.get(at..at.saturating_add(MINI).min(self.mini_stream.len())).filter(|s| !s.is_empty());
            out.extend_from_slice(sector.ok_or_else(|| bad("mini sector past the mini stream"))?);
            next = *self.mini_fat.get(usize::try_from(next).unwrap_or(usize::MAX)).ok_or_else(|| bad("mini chain leaves the mini FAT"))?;
        }
        out.truncate(want);
        Ok(out)
    }

    pub fn entry(&self, id: usize) -> Option<&Entry> {
        self.entries.get(id)
    }

    /// The entries directly inside storage `id` (the root is 0), in no particular order.
    pub fn children(&self, id: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let Some(first) = self.entries.get(id).map(|e| e.child) else { return out };
        let mut stack = vec![first];
        let mut seen = vec![false; self.entries.len()];
        while let Some(n) = stack.pop() {
            let Ok(i) = usize::try_from(n) else { continue };
            let Some(e) = self.entries.get(i) else { continue };
            if std::mem::replace(seen.get_mut(i).unwrap_or(&mut true), true) {
                continue;
            }
            out.push(i);
            stack.push(e.left);
            stack.push(e.right);
        }
        out
    }

    /// The entry named `name` (any case) inside storage `id`.
    pub fn find(&self, id: usize, name: &str) -> Option<usize> {
        self.children(id).into_iter().find(|&c| self.entries.get(c).is_some_and(|e| e.name.eq_ignore_ascii_case(name)))
    }

    /// The bytes of stream `id`.
    pub fn read(&self, id: usize) -> Result<Vec<u8>> {
        let e = self.entries.get(id).ok_or_else(|| bad("no such entry"))?;
        if e.kind != Kind::Stream {
            return Err(bad("not a stream"));
        }
        if e.size < self.cutoff { self.mini_chain(e.start, e.size) } else { self.chain(e.start, Some(e.size)) }
    }
}

/// Mark sector `n` seen; `true` when it already was, or is outside the table.
fn visited(seen: &mut [bool], n: u32) -> bool {
    match usize::try_from(n).ok().and_then(|i| seen.get_mut(i)) {
        Some(s) => std::mem::replace(s, true),
        None => true,
    }
}

fn entry(raw: &[u8]) -> Entry {
    let name_len = usize::from(u16_at(raw, 64).unwrap_or(0)).min(64);
    let units: Vec<u16> = raw.get(..name_len).unwrap_or(&[]).chunks_exact(2).filter_map(|c| u16_at(c, 0)).take_while(|&u| u != 0).collect();
    let kind = match raw.get(66) {
        Some(1) => Kind::Storage,
        Some(2) => Kind::Stream,
        Some(5) => Kind::Root,
        _ => Kind::Unused,
    };
    let at = |o| u32_at(raw, o).unwrap_or(NO_STREAM);
    Entry {
        name: String::from_utf16_lossy(&units),
        kind,
        left: at(68),
        right: at(72),
        child: at(76),
        start: at(116),
        // Version 3 files may leave junk in the high half of the size.
        size: u64::from(at(120)),
    }
}

#[cfg(test)]
mod tests;
