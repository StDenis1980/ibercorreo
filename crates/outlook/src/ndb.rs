//! The node database of a `.pst` file (MS-PST 2.2): the header, the two B-trees that find nodes
//! (NBT) and blocks (BBT), data blocks with their encoding, and the trees of blocks and subnodes a
//! node's data can spread over.
//!
//! Both file formats are read: ANSI (Outlook 97–2002, 32-bit ids) and Unicode (Outlook 2003 and
//! later, 64-bit ids). The 4 KiB-page format of offline `.ost` caches and files protected with
//! Windows Information Protection are refused.

use std::sync::Mutex;

use ibercorreo_core::{Error, Result};

/// Where a `.pst` is read from: a file, or bytes in memory.
pub trait Source {
    /// The size in bytes.
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Fills `buf` from `offset`; an error when the file ends first.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;
}

impl Source for [u8] {
    fn len(&self) -> u64 {
        self.len() as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = usize::try_from(offset).map_err(|_| bad("an offset past the end of the file"))?;
        let end = start.checked_add(buf.len()).ok_or_else(|| bad("an offset past the end of the file"))?;
        let src = self.get(start..end).ok_or_else(|| bad("an offset past the end of the file"))?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

impl Source for Vec<u8> {
    fn len(&self) -> u64 {
        self.as_slice().len() as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.as_slice().read_at(offset, buf)
    }
}

/// A file on disk, read a piece at a time, so an archive of many gigabytes needs little memory.
pub struct FileSource {
    file: Mutex<std::fs::File>,
    len: u64,
}

impl FileSource {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(|e| Error::Storage(e.to_string()))?;
        let len = file.metadata().map_err(|e| Error::Storage(e.to_string()))?.len();
        Ok(FileSource { file: Mutex::new(file), len })
    }
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self.file.lock().map_err(|_| Error::Storage("the archive file is unavailable".into()))?;
        f.seek(SeekFrom::Start(offset)).map_err(|e| Error::Storage(e.to_string()))?;
        f.read_exact(buf).map_err(|e| Error::Storage(e.to_string()))
    }
}

pub(crate) fn bad(detail: &str) -> Error {
    Error::invalid(".pst file", detail)
}

/// The largest block: 8 KiB with its trailer.
const MAX_BLOCK: usize = 8192;
const PAGE: usize = 512;
/// B-trees deeper than this are taken for damage.
const MAX_LEVELS: u8 = 8;
/// Data blocks of one node, at most (8 GiB of data).
const MAX_BLOCKS: usize = 1 << 20;
/// Subnodes of one node, at most.
pub(crate) const MAX_SUBNODES: usize = 1 << 20;

const PTYPE_BBT: u8 = 0x80;
const PTYPE_NBT: u8 = 0x81;

const CRYPT_NONE: u8 = 0;
const CRYPT_PERMUTE: u8 = 1;
const CRYPT_CYCLIC: u8 = 2;

/// A node's entry in the node B-tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) nid: u32,
    pub(crate) data: u64,
    pub(crate) sub: u64,
    pub(crate) parent: u32,
}

/// A subnode: a node private to its parent (a message's recipients table, an attachment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Subnode {
    pub(crate) nid: u32,
    pub(crate) data: u64,
    pub(crate) sub: u64,
}

/// An open node database.
pub(crate) struct Ndb<S> {
    src: S,
    pub(crate) unicode: bool,
    crypt: u8,
    nbt: u64,
    bbt: u64,
}

impl<S: Source> Ndb<S> {
    pub(crate) fn open(src: S) -> Result<Self> {
        let mut h = [0u8; 564];
        let n = usize::try_from(src.len()).unwrap_or(usize::MAX).min(h.len());
        src.read_at(0, h.get_mut(..n).unwrap_or(&mut []))?;
        if h.get(..4) != Some(b"!BDN") || h.get(8..10) != Some(b"SM") {
            return Err(bad("not an Outlook data file"));
        }
        let ver = u16_at(&h, 10).unwrap_or(0);
        let unicode = match ver {
            14 | 15 => false,
            23 => true,
            36 => return Err(bad("an offline .ost cache, not an archive")),
            37 => return Err(bad("protected with Windows Information Protection")),
            _ => return Err(bad("an unknown version")),
        };
        // The root (MS-PST 2.2.2.5) holds where the two B-trees start.
        let (nbt, bbt, crypt) = if unicode {
            (u64_at(&h, 224), u64_at(&h, 240), h.get(513).copied())
        } else {
            (u32_at(&h, 188).map(u64::from), u32_at(&h, 196).map(u64::from), h.get(461).copied())
        };
        let (Some(nbt), Some(bbt), Some(crypt)) = (nbt, bbt, crypt) else {
            return Err(bad("shorter than its header"));
        };
        if !matches!(crypt, CRYPT_NONE | CRYPT_PERMUTE | CRYPT_CYCLIC) {
            return Err(bad("encrypted in a way that is not supported"));
        }
        Ok(Ndb { src, unicode, crypt, nbt, bbt })
    }

    fn id(&self, b: &[u8], at: usize) -> Option<u64> {
        if self.unicode { u64_at(b, at) } else { u32_at(b, at).map(u64::from) }
    }

    fn id_len(&self) -> usize {
        if self.unicode { 8 } else { 4 }
    }

    /// The entries of the B-tree page at `ib`, of type `ptype`: its level and each entry's bytes.
    fn page(&self, ib: u64, ptype: u8) -> Result<(u8, Vec<Vec<u8>>)> {
        let mut p = [0u8; PAGE];
        self.src.read_at(ib, &mut p)?;
        let (meta, trailer) = if self.unicode { (488, 496) } else { (496, 500) };
        if p.get(trailer) != Some(&ptype) || p.get(trailer + 1) != Some(&ptype) {
            return Err(bad("a B-tree page of the wrong type"));
        }
        let (Some(&count), Some(&size), Some(&level)) = (p.get(meta), p.get(meta + 2), p.get(meta + 3)) else {
            return Err(bad("a damaged B-tree page"));
        };
        let size = usize::from(size);
        if size == 0 {
            return Err(bad("a damaged B-tree page"));
        }
        let entries = (0..usize::from(count))
            .map_while(|i| p.get(i * size..(i + 1) * size).filter(|e| i * size + e.len() <= meta).map(<[u8]>::to_vec))
            .collect();
        Ok((level, entries))
    }

    /// Finds `key` in the B-tree rooted at `ib`: the leaf entry whose key it is.
    fn find(&self, mut ib: u64, ptype: u8, key: u64) -> Result<Option<Vec<u8>>> {
        let mut above = u8::MAX;
        for _ in 0..=MAX_LEVELS {
            let (level, entries) = self.page(ib, ptype)?;
            if level >= above {
                return Err(bad("a B-tree that does not get shallower"));
            }
            above = level;
            if level == 0 {
                return Ok(entries.into_iter().find(|e| self.id(e, 0).map(|k| key_of(ptype, k)) == Some(key)));
            }
            // The last child whose first key is not after the key.
            let child = entries.iter().take_while(|e| self.id(e, 0).is_some_and(|k| k <= key)).last();
            let Some(next) = child.and_then(|e| self.id(e, 2 * self.id_len())) else { return Ok(None) };
            ib = next;
        }
        Err(bad("a B-tree too deep"))
    }

    /// The node `nid`, if the file has it.
    pub(crate) fn node(&self, nid: u32) -> Result<Option<Node>> {
        Ok(self.find(self.nbt, PTYPE_NBT, u64::from(nid))?.and_then(|e| self.nbt_entry(&e)))
    }

    fn nbt_entry(&self, e: &[u8]) -> Option<Node> {
        let n = self.id_len();
        Some(Node {
            nid: u32::try_from(self.id(e, 0)? & 0xffff_ffff).ok()?,
            data: self.id(e, n)?,
            sub: self.id(e, 2 * n)?,
            parent: u32_at(e, 3 * n)?,
        })
    }

    /// Where block `bid` is and its size.
    fn locate(&self, bid: u64) -> Result<(u64, usize)> {
        let key = bid & !1;
        let e = self.find(self.bbt, PTYPE_BBT, key)?.ok_or_else(|| bad("a block that is not in the file"))?;
        let n = self.id_len();
        let (Some(ib), Some(cb)) = (self.id(&e, n), u16_at(&e, 2 * n)) else {
            return Err(bad("a damaged block entry"));
        };
        Ok((ib, usize::from(cb)))
    }

    /// The bytes of block `bid`, decoded.
    pub(crate) fn block(&self, bid: u64) -> Result<Vec<u8>> {
        let (ib, cb) = self.locate(bid)?;
        if cb > MAX_BLOCK {
            return Err(bad("a block larger than blocks can be"));
        }
        let mut data = vec![0u8; cb];
        self.src.read_at(ib, &mut data)?;
        // Internal blocks (the trees below) are never encoded.
        if bid & 2 == 0 {
            match self.crypt {
                CRYPT_PERMUTE => permute(&mut data),
                CRYPT_CYCLIC => cyclic(&mut data, bid as u32),
                _ => {}
            }
        }
        Ok(data)
    }

    /// The data blocks of a node's data, in order: the block itself, or the blocks an XBLOCK or
    /// XXBLOCK lists (MS-PST 2.2.2.8.3.2). Empty for no data.
    pub(crate) fn data(&self, bid: u64) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        if bid != 0 {
            self.data_into(bid, 2, &mut out)?;
        }
        Ok(out)
    }

    fn data_into(&self, bid: u64, levels: u8, out: &mut Vec<Vec<u8>>) -> Result<()> {
        let block = self.block(bid)?;
        if bid & 2 == 0 {
            out.push(block);
            return Ok(());
        }
        let (Some(&1), Some(&level), Some(count)) = (block.first(), block.get(1), u16_at(&block, 2)) else {
            return Err(bad("an internal block that is not a data tree"));
        };
        if level == 0 || level > levels {
            return Err(bad("a data tree of the wrong depth"));
        }
        for i in 0..usize::from(count) {
            if out.len() >= MAX_BLOCKS {
                return Err(bad("more data than a node can hold"));
            }
            let child = self.id(&block, 8 + i * self.id_len()).ok_or_else(|| bad("a damaged data tree"))?;
            if level == 1 {
                if child & 2 != 0 {
                    return Err(bad("a damaged data tree"));
                }
                out.push(self.block(child)?);
            } else {
                self.data_into(child, level - 1, out)?;
            }
        }
        Ok(())
    }

    /// The size of a node's data without reading it all.
    pub(crate) fn data_len(&self, bid: u64) -> Result<usize> {
        if bid == 0 {
            return Ok(0);
        }
        if bid & 2 == 0 {
            return Ok(self.locate(bid)?.1);
        }
        let block = self.block(bid)?;
        u32_at(&block, 4).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("a damaged data tree"))
    }

    /// The subnodes listed by the subnode tree at `bid` (MS-PST 2.2.2.8.3.3). Empty for none.
    pub(crate) fn subnodes(&self, bid: u64) -> Result<Vec<Subnode>> {
        let mut out = Vec::new();
        if bid != 0 {
            self.subnodes_into(bid, 2, &mut out)?;
        }
        Ok(out)
    }

    fn subnodes_into(&self, bid: u64, levels: u8, out: &mut Vec<Subnode>) -> Result<()> {
        let block = self.block(bid)?;
        let (Some(&2), Some(&level), Some(count)) = (block.first(), block.get(1), u16_at(&block, 2)) else {
            return Err(bad("a damaged subnode tree"));
        };
        if level >= levels {
            return Err(bad("a subnode tree of the wrong depth"));
        }
        let n = self.id_len();
        let start = if self.unicode { 8 } else { 4 };
        let width = if level == 0 { 3 * n } else { 2 * n };
        for i in 0..usize::from(count) {
            if out.len() >= MAX_SUBNODES {
                return Err(bad("more subnodes than a node can hold"));
            }
            let at = start + i * width;
            let (Some(nid), Some(b)) = (self.id(&block, at), self.id(&block, at + n)) else {
                return Err(bad("a damaged subnode tree"));
            };
            if level == 0 {
                let sub = self.id(&block, at + 2 * n).ok_or_else(|| bad("a damaged subnode tree"))?;
                out.push(Subnode { nid: (nid & 0xffff_ffff) as u32, data: b, sub });
            } else {
                self.subnodes_into(b, level, out)?;
            }
        }
        Ok(())
    }
}

/// The key a leaf entry is found by: a block's id without its reserved low bit.
fn key_of(ptype: u8, k: u64) -> u64 {
    if ptype == PTYPE_BBT { k & !1 } else { k }
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(<[u8; 2]>::try_from(b.get(at..at.checked_add(2)?)?).ok()?))
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(<[u8; 4]>::try_from(b.get(at..at.checked_add(4)?)?).ok()?))
}

pub(crate) fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(<[u8; 8]>::try_from(b.get(at..at.checked_add(8)?)?).ok()?))
}

/// MS-PST 5.1's table: three permutations of the byte values. The second is its own inverse and
/// the third the inverse of the first.
pub(crate) const CRYPT: [u8; 768] = [
    65, 54, 19, 98, 168, 33, 110, 187, 244, 22, 204, 4, 127, 100, 232, 93, 30, 242, 203, 42, 116, 197, 94, 53, 210, 149, 71, 158, 150, 45, 154, 136,
    76, 125, 132, 63, 219, 172, 49, 182, 72, 95, 246, 196, 216, 57, 139, 231, 35, 59, 56, 142, 200, 193, 223, 37, 177, 32, 165, 70, 96, 78, 156, 251,
    170, 211, 86, 81, 69, 124, 85, 0, 7, 201, 43, 157, 133, 155, 9, 160, 143, 173, 179, 15, 99, 171, 137, 75, 215, 167, 21, 90, 113, 102, 66, 191,
    38, 74, 107, 152, 250, 234, 119, 83, 178, 112, 5, 44, 253, 89, 58, 134, 126, 206, 6, 235, 130, 120, 87, 199, 141, 67, 175, 180, 28, 212, 91, 205,
    226, 233, 39, 79, 195, 8, 114, 128, 207, 176, 239, 245, 40, 109, 190, 48, 77, 52, 146, 213, 14, 60, 34, 50, 229, 228, 249, 159, 194, 209, 10,
    129, 18, 225, 238, 145, 131, 118, 227, 151, 230, 97, 138, 23, 121, 164, 183, 220, 144, 122, 92, 140, 2, 166, 202, 105, 222, 80, 26, 17, 147, 185,
    82, 135, 88, 252, 237, 29, 55, 73, 27, 106, 224, 41, 51, 153, 189, 108, 217, 148, 243, 64, 84, 111, 240, 198, 115, 184, 214, 62, 101, 24, 68, 31,
    221, 103, 16, 241, 12, 25, 236, 174, 3, 161, 20, 123, 169, 11, 255, 248, 163, 192, 162, 1, 247, 46, 188, 36, 104, 117, 13, 254, 186, 47, 181,
    208, 218, 61, 20, 83, 15, 86, 179, 200, 122, 156, 235, 101, 72, 23, 22, 21, 159, 2, 204, 84, 124, 131, 0, 13, 12, 11, 162, 98, 168, 118, 219,
    217, 237, 199, 197, 164, 220, 172, 133, 116, 214, 208, 167, 155, 174, 154, 150, 113, 102, 195, 99, 153, 184, 221, 115, 146, 142, 132, 125, 165,
    94, 209, 93, 147, 177, 87, 81, 80, 128, 137, 82, 148, 79, 78, 10, 107, 188, 141, 127, 110, 71, 70, 65, 64, 68, 1, 17, 203, 3, 63, 247, 244, 225,
    169, 143, 60, 58, 249, 251, 240, 25, 48, 130, 9, 46, 201, 157, 160, 134, 73, 238, 111, 77, 109, 196, 45, 129, 52, 37, 135, 27, 136, 170, 252, 6,
    161, 18, 56, 253, 76, 66, 114, 100, 19, 55, 36, 106, 117, 119, 67, 255, 230, 180, 75, 54, 92, 228, 216, 53, 61, 69, 185, 44, 236, 183, 49, 43,
    41, 7, 104, 163, 14, 105, 123, 24, 158, 33, 57, 190, 40, 26, 91, 120, 245, 35, 202, 42, 176, 175, 62, 254, 4, 140, 231, 229, 152, 50, 149, 211,
    246, 74, 232, 166, 234, 233, 243, 213, 47, 112, 32, 242, 31, 5, 103, 173, 85, 16, 206, 205, 227, 39, 59, 218, 186, 215, 194, 38, 212, 145, 29,
    210, 28, 34, 51, 248, 250, 241, 90, 239, 207, 144, 182, 139, 181, 189, 192, 191, 8, 151, 30, 108, 226, 97, 224, 198, 193, 89, 171, 187, 88, 222,
    95, 223, 96, 121, 126, 178, 138, 71, 241, 180, 230, 11, 106, 114, 72, 133, 78, 158, 235, 226, 248, 148, 83, 224, 187, 160, 2, 232, 90, 9, 171,
    219, 227, 186, 198, 124, 195, 16, 221, 57, 5, 150, 48, 245, 55, 96, 130, 140, 201, 19, 74, 107, 29, 243, 251, 143, 38, 151, 202, 145, 23, 1, 196,
    50, 45, 110, 49, 149, 255, 217, 35, 209, 0, 94, 121, 220, 68, 59, 26, 40, 197, 97, 87, 32, 144, 61, 131, 185, 67, 190, 103, 210, 70, 66, 118,
    192, 109, 91, 126, 178, 15, 22, 41, 60, 169, 3, 84, 13, 218, 93, 223, 246, 183, 199, 98, 205, 141, 6, 211, 105, 92, 134, 214, 20, 247, 165, 102,
    117, 172, 177, 233, 69, 33, 112, 12, 135, 159, 116, 164, 34, 76, 111, 191, 31, 86, 170, 46, 179, 120, 51, 80, 176, 163, 146, 188, 207, 25, 28,
    167, 99, 203, 30, 77, 62, 75, 27, 155, 79, 231, 240, 238, 173, 58, 181, 89, 4, 234, 64, 85, 37, 81, 229, 122, 137, 56, 104, 82, 123, 252, 39,
    174, 215, 189, 250, 7, 244, 204, 142, 95, 239, 53, 156, 132, 43, 21, 213, 119, 52, 73, 182, 18, 10, 127, 113, 136, 253, 157, 24, 65, 125, 147,
    216, 88, 44, 206, 254, 36, 175, 222, 184, 54, 200, 161, 128, 166, 153, 152, 168, 47, 14, 129, 101, 115, 228, 194, 162, 138, 212, 225, 17, 208, 8,
    139, 42, 242, 237, 154, 100, 63, 193, 108, 249, 236,
];

fn table(n: usize, b: u8) -> u8 {
    CRYPT.get(n * 256 + usize::from(b)).copied().unwrap_or(b)
}

/// Decodes `NDB_CRYPT_PERMUTE` data (MS-PST 5.1): each byte through the inverse permutation.
pub(crate) fn permute(data: &mut [u8]) {
    for b in data {
        *b = table(2, *b);
    }
}

/// Encodes or decodes `NDB_CRYPT_CYCLIC` data (MS-PST 5.2), keyed by the block id's low 32 bits;
/// the same steps do both.
pub(crate) fn cyclic(data: &mut [u8], key: u32) {
    let mut w = (key ^ (key >> 16)) as u16;
    for b in data {
        let mut x = b.wrapping_add(w as u8);
        x = table(0, x);
        x = x.wrapping_add((w >> 8) as u8);
        x = table(1, x);
        x = x.wrapping_sub((w >> 8) as u8);
        x = table(2, x);
        *b = x.wrapping_sub(w as u8);
        w = w.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests;
