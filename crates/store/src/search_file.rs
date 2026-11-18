//! Tinta: the full-text index on disk (`store.json` → `store.index/`), so a big mailbox neither
//! keeps the whole index in memory nor reads every message's text again at each start.
//!
//! A segment file (`<generation>.idx`) holds, for each distinct word in byte order, its postings:
//! the message ids and field masks ([`crate::search`]), as gaps between packed numbers in
//! LEB128. After the postings come the dictionary (each word with where its postings are) and the
//! ids of the messages the segment covers. Only a sample of the dictionary (one word in
//! [`BLOCK`]) and the ids stay in memory; a lookup reads the dictionary from the word before the
//! prefix onwards, then the postings of the words that match.
//!
//! The file is checked as it is opened and read: a file cut short, an offset past its end, words
//! out of order or postings that do not add up are an error, and the index is then rebuilt from
//! the messages as before. Files are written under a temporary name and renamed into place.
//!
//! Beside a segment, `<generation>.stale` lists the ids of the messages it covers that changed
//! since it was written (their words are read again at the next start); the store file names the
//! generation it goes with, so a store saved by a program that did not write the index (an older
//! version) is never paired with a segment that does not match it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use ibercorreo_core::{Error, Result};

/// The first bytes of a segment file.
const MAGIC: &[u8; 8] = b"TCIDX\x00\x00\x01";
/// The header: the magic, then word count, dictionary offset and length, ids offset, length and
/// count, each a little-endian `u64`.
const HEADER: u64 = 8 + 6 * 8;
/// One dictionary word in this many is kept in memory.
pub const BLOCK: u64 = 64;
/// Longest word read back, in bytes ([`crate::search::MAX_WORD`] characters of up to 4 bytes).
const MAX_WORD_BYTES: u64 = 4 * crate::search::MAX_WORD as u64;
/// Largest segment file opened.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Largest stale list read.
const MAX_STALE_BYTES: u64 = 64 * 1024 * 1024;

/// The folder holding the index of the store saved at `path`.
pub fn dir(path: &Path) -> PathBuf {
    path.with_extension("index")
}

pub(crate) fn segment_path(dir: &Path, generation: &str) -> PathBuf {
    dir.join(format!("{generation}.idx"))
}

pub(crate) fn stale_path(dir: &Path, generation: &str) -> PathBuf {
    dir.join(format!("{generation}.stale"))
}

/// Whether `name` is a generation this module writes (32 lowercase hex digits).
pub(crate) fn is_generation(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn storage(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::Storage(format!("{}: {e}", path.display()))
}

fn corrupt(path: &Path, what: &str) -> Error {
    Error::Storage(format!("{}: not a whole search index ({what})", path.display()))
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// A LEB128 number from `bytes` at `*at`, moving past it; `None` past the end or beyond 64 bits.
fn get_varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *bytes.get(*at)?;
        *at = at.checked_add(1)?;
        let part = u64::from(b & 0x7f);
        if shift == 63 && part > 1 {
            return None;
        }
        v |= part << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// A LEB128 number read from a stream.
fn read_varint(r: &mut impl Read) -> std::io::Result<Option<u64>> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let mut b = [0u8; 1];
        if r.read(&mut b)? == 0 {
            return Ok(None);
        }
        let part = u64::from(b[0] & 0x7f);
        if shift == 63 && part > 1 {
            return Ok(None);
        }
        v |= part << shift;
        if b[0] & 0x80 == 0 {
            return Ok(Some(v));
        }
    }
    Ok(None)
}

/// Postings (ascending packed numbers) as LEB128 gaps.
fn encode_postings(postings: &[u64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(postings.len() * 2);
    let mut prev = 0u64;
    for &p in postings {
        put_varint(&mut out, p.wrapping_sub(prev));
        prev = p;
    }
    out
}

/// `count` postings from `bytes`; `None` when they do not add up or do not rise.
fn decode_postings(bytes: &[u8], count: u64) -> Option<Vec<u64>> {
    let mut out = Vec::with_capacity(usize::try_from(count).ok()?.min(bytes.len()));
    let (mut at, mut prev) = (0usize, 0u64);
    for i in 0..count {
        let gap = get_varint(bytes, &mut at)?;
        if i > 0 && gap == 0 {
            return None;
        }
        prev = prev.checked_add(gap)?;
        out.push(prev);
    }
    (at == bytes.len()).then_some(out)
}

/// Writes a segment: words in byte order, each with its postings, then the ids it covers.
pub(crate) struct Writer {
    file: BufWriter<File>,
    tmp: PathBuf,
    to: PathBuf,
    dict: Vec<u8>,
    words: u64,
    written: u64,
    last: Option<Box<str>>,
}

impl Writer {
    /// A new segment to be `to` once [`Writer::finish`]ed.
    pub(crate) fn create(to: &Path) -> Result<Writer> {
        let tmp = to.with_extension("idx.tmp");
        let file = File::create(&tmp).map_err(|e| storage(&tmp, e))?;
        let mut file = BufWriter::with_capacity(1 << 20, file);
        file.write_all(&[0u8; HEADER as usize]).map_err(|e| storage(&tmp, e))?;
        Ok(Writer { file, tmp, to: to.to_path_buf(), dict: Vec::new(), words: 0, written: 0, last: None })
    }

    /// Add `word` with its `postings` (ascending, none twice). Words must come in byte order.
    pub(crate) fn word(&mut self, word: &str, postings: &[u64]) -> Result<()> {
        if postings.is_empty() {
            return Ok(());
        }
        if self.last.as_deref().is_some_and(|l| l >= word) || word.is_empty() || word.len() as u64 > MAX_WORD_BYTES {
            return Err(Error::Storage(format!("search index: word {word:?} out of order")));
        }
        let bytes = encode_postings(postings);
        self.file.write_all(&bytes).map_err(|e| storage(&self.tmp, e))?;
        put_varint(&mut self.dict, word.len() as u64);
        self.dict.extend_from_slice(word.as_bytes());
        put_varint(&mut self.dict, self.written);
        put_varint(&mut self.dict, bytes.len() as u64);
        put_varint(&mut self.dict, postings.len() as u64);
        self.written = self.written.saturating_add(bytes.len() as u64);
        self.words = self.words.saturating_add(1);
        self.last = Some(word.into());
        Ok(())
    }

    /// Write the dictionary and `ids` (ascending), then the header, and put the file in place.
    pub(crate) fn finish(mut self, ids: &[u64]) -> Result<()> {
        let mut id_bytes = Vec::with_capacity(ids.len() * 2);
        let mut prev = 0u64;
        for &id in ids {
            put_varint(&mut id_bytes, id.wrapping_sub(prev));
            prev = id;
        }
        let dict_off = HEADER.saturating_add(self.written);
        let ids_off = dict_off.saturating_add(self.dict.len() as u64);
        let tmp = self.tmp.clone();
        let io = |e: std::io::Error| storage(&tmp, e);
        self.file.write_all(&self.dict).map_err(io)?;
        self.file.write_all(&id_bytes).map_err(io)?;
        let mut header = Vec::with_capacity(HEADER as usize);
        header.extend_from_slice(MAGIC);
        for v in [self.words, dict_off, self.dict.len() as u64, ids_off, id_bytes.len() as u64, ids.len() as u64] {
            header.extend_from_slice(&v.to_le_bytes());
        }
        let mut file = self.file.into_inner().map_err(|e| storage(&tmp, e.error()))?;
        file.seek(SeekFrom::Start(0)).map_err(io)?;
        file.write_all(&header).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        std::fs::rename(&tmp, &self.to).map_err(io)
    }

    /// Give up: the temporary file goes.
    pub(crate) fn abandon(self) {
        let tmp = self.tmp.clone();
        drop(self);
        let _ = std::fs::remove_file(tmp);
    }
}

/// One dictionary entry: where a word's postings are, and how many.
#[derive(Debug, Clone, Copy)]
struct Place {
    offset: u64,
    len: u64,
    count: u64,
}

/// A segment file open for lookups.
#[derive(Debug)]
pub(crate) struct Segment {
    path: PathBuf,
    file: Mutex<File>,
    dict_off: u64,
    dict_len: u64,
    /// Every [`BLOCK`]th word and where its entry starts in the dictionary.
    sample: Vec<(Box<str>, u64)>,
    /// The ids the segment covers, ascending.
    ids: Vec<u64>,
}

impl Segment {
    /// Open and check the segment at `path`.
    pub(crate) fn open(path: &Path) -> Result<Segment> {
        let mut file = File::open(path).map_err(|e| storage(path, e))?;
        let size = file.metadata().map_err(|e| storage(path, e))?.len();
        if !(HEADER..=MAX_FILE_BYTES).contains(&size) {
            return Err(corrupt(path, "size"));
        }
        let mut header = [0u8; HEADER as usize];
        file.read_exact(&mut header).map_err(|e| storage(path, e))?;
        if header.get(..8) != Some(&MAGIC[..]) {
            return Err(corrupt(path, "not an index file"));
        }
        let field = |i: usize| -> u64 {
            let mut b = [0u8; 8];
            if let Some(s) = header.get(8 + i * 8..16 + i * 8) {
                b.copy_from_slice(s);
            }
            u64::from_le_bytes(b)
        };
        let (words, dict_off, dict_len, ids_off, ids_len, ids_count) = (field(0), field(1), field(2), field(3), field(4), field(5));
        if dict_off < HEADER || dict_off.checked_add(dict_len) != Some(ids_off) || ids_off.checked_add(ids_len) != Some(size) || ids_count > ids_len {
            return Err(corrupt(path, "offsets"));
        }
        // The ids, kept.
        let mut id_bytes = vec![0u8; usize::try_from(ids_len).map_err(|_| corrupt(path, "ids"))?];
        file.seek(SeekFrom::Start(ids_off)).map_err(|e| storage(path, e))?;
        file.read_exact(&mut id_bytes).map_err(|e| storage(path, e))?;
        let mut ids = Vec::with_capacity(usize::try_from(ids_count).unwrap_or(0));
        let (mut at, mut prev) = (0usize, 0u64);
        for i in 0..ids_count {
            let gap = get_varint(&id_bytes, &mut at).ok_or_else(|| corrupt(path, "ids"))?;
            if i > 0 && gap == 0 {
                return Err(corrupt(path, "ids"));
            }
            prev = prev.checked_add(gap).ok_or_else(|| corrupt(path, "ids"))?;
            ids.push(prev);
        }
        if at != id_bytes.len() {
            return Err(corrupt(path, "ids"));
        }
        drop(id_bytes);
        // The dictionary, read through once to check it and keep a sample.
        file.seek(SeekFrom::Start(dict_off)).map_err(|e| storage(path, e))?;
        let mut r = BufReader::with_capacity(1 << 16, (&mut file).take(dict_len));
        let postings_len = dict_off - HEADER;
        let mut sample = Vec::new();
        let mut pos = 0u64;
        let mut last: Option<Vec<u8>> = None;
        let mut expected = 0u64;
        for n in 0..words {
            let start = pos;
            let (word, place, used) = read_entry(&mut r).map_err(|e| storage(path, e))?.ok_or_else(|| corrupt(path, "dictionary"))?;
            pos = pos.saturating_add(used);
            if last.as_deref().is_some_and(|l| l >= word.as_slice()) || place.offset != expected || place.count == 0 || place.count > place.len {
                return Err(corrupt(path, "dictionary"));
            }
            expected = place.offset.checked_add(place.len).ok_or_else(|| corrupt(path, "dictionary"))?;
            if expected > postings_len {
                return Err(corrupt(path, "dictionary"));
            }
            if n % BLOCK == 0 {
                let w = String::from_utf8(word.clone()).map_err(|_| corrupt(path, "dictionary"))?;
                sample.push((w.into_boxed_str(), start));
            }
            last = Some(word);
        }
        if pos != dict_len || expected != postings_len {
            return Err(corrupt(path, "dictionary"));
        }
        drop(r);
        Ok(Segment { path: path.to_path_buf(), file: Mutex::new(file), dict_off, dict_len, sample, ids })
    }

    fn lock(&self) -> MutexGuard<'_, File> {
        let guard = self.file.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.file.clear_poison();
        guard
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The ids the segment covers, ascending.
    pub(crate) fn ids(&self) -> &[u64] {
        &self.ids
    }

    pub(crate) fn contains(&self, id: u64) -> bool {
        self.ids.binary_search(&id).is_ok()
    }

    /// The postings of every word that starts with `prefix`, in word order.
    pub(crate) fn lookup(&self, prefix: &str) -> Result<Vec<Vec<u64>>> {
        let first = self.sample.partition_point(|(w, _)| w.as_bytes() < prefix.as_bytes()).saturating_sub(1);
        let Some(&(_, start)) = self.sample.get(first) else { return Ok(Vec::new()) };
        let mut file = self.lock();
        let path = self.path.clone();
        let io = |e: std::io::Error| storage(&path, e);
        let mut places = Vec::new();
        {
            file.seek(SeekFrom::Start(self.dict_off.saturating_add(start))).map_err(io)?;
            let mut r = BufReader::with_capacity(1 << 16, (&mut *file).take(self.dict_len.saturating_sub(start)));
            while let Some((word, place, _)) = read_entry(&mut r).map_err(io)? {
                if word.starts_with(prefix.as_bytes()) {
                    places.push(place);
                } else if word.as_slice() > prefix.as_bytes() {
                    break;
                }
            }
        }
        let mut out = Vec::with_capacity(places.len());
        for place in places {
            out.push(read_postings(&mut file, &self.path, place)?);
        }
        Ok(out)
    }

    /// Every word with its postings, in word order, through a handle of its own (for merging).
    pub(crate) fn words(&self) -> Result<Words> {
        let open = |p: &Path| File::open(p).map_err(|e| storage(p, e));
        let mut dict = open(&self.path)?;
        dict.seek(SeekFrom::Start(self.dict_off)).map_err(|e| storage(&self.path, e))?;
        let dict = BufReader::with_capacity(1 << 16, dict.take(self.dict_len));
        Ok(Words { path: self.path.clone(), dict, postings: open(&self.path)? })
    }
}

/// One dictionary entry from `r`: the word, its place and the bytes the entry took; `None` at the
/// end.
fn read_entry(r: &mut impl Read) -> std::io::Result<Option<(Vec<u8>, Place, u64)>> {
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "dictionary entry");
    let Some(len) = read_varint(r)? else { return Ok(None) };
    if len == 0 || len > MAX_WORD_BYTES {
        return Err(bad());
    }
    let mut word = vec![0u8; len as usize];
    r.read_exact(&mut word)?;
    let mut next = || -> std::io::Result<u64> { read_varint(r)?.ok_or_else(bad) };
    let place = Place { offset: next()?, len: next()?, count: next()? };
    let mut used = Vec::new();
    for v in [len, place.offset, place.len, place.count] {
        put_varint(&mut used, v);
    }
    Ok(Some((word, place, (used.len() as u64).saturating_add(len))))
}

fn read_postings(file: &mut File, path: &Path, place: Place) -> Result<Vec<u64>> {
    let mut bytes = vec![0u8; usize::try_from(place.len).map_err(|_| corrupt(path, "postings"))?];
    file.seek(SeekFrom::Start(HEADER.saturating_add(place.offset))).map_err(|e| storage(path, e))?;
    file.read_exact(&mut bytes).map_err(|e| storage(path, e))?;
    decode_postings(&bytes, place.count).ok_or_else(|| corrupt(path, "postings"))
}

/// A segment's words and postings in order ([`Segment::words`]).
pub(crate) struct Words {
    path: PathBuf,
    dict: BufReader<std::io::Take<File>>,
    postings: File,
}

impl Words {
    /// The next word and its postings; `None` at the end.
    pub(crate) fn next_word(&mut self) -> Result<Option<(String, Vec<u64>)>> {
        let Some((word, place, _)) = read_entry(&mut self.dict).map_err(|e| storage(&self.path, e))? else { return Ok(None) };
        let word = String::from_utf8(word).map_err(|_| corrupt(&self.path, "dictionary"))?;
        let postings = read_postings(&mut self.postings, &self.path, place)?;
        Ok(Some((word, postings)))
    }
}

/// Write an in-memory map of words to postings as a segment at `to`, covering `ids`.
pub(crate) fn write_map(to: &Path, map: &BTreeMap<String, Vec<u64>>, ids: &[u64]) -> Result<()> {
    let mut w = Writer::create(to)?;
    for (word, postings) in map {
        if let Err(e) = w.word(word, postings) {
            w.abandon();
            return Err(e);
        }
    }
    w.finish(ids)
}

/// The ids a stale list names (`<generation>.stale`: LEB128 gaps); empty when there is none.
pub(crate) fn read_stale(path: &Path) -> Result<Vec<u64>> {
    let bytes = match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_STALE_BYTES => return Err(corrupt(path, "stale list size")),
        Ok(_) => std::fs::read(path).map_err(|e| storage(path, e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(storage(path, e)),
    };
    let (mut at, mut prev, mut out) = (0usize, 0u64, Vec::new());
    while at < bytes.len() {
        let gap = get_varint(&bytes, &mut at).ok_or_else(|| corrupt(path, "stale list"))?;
        prev = prev.checked_add(gap).ok_or_else(|| corrupt(path, "stale list"))?;
        out.push(prev);
    }
    Ok(out)
}

/// Write the stale list `ids` (ascending) to `path`, through a temporary file.
pub(crate) fn write_stale(path: &Path, ids: &[u64]) -> Result<()> {
    let mut bytes = Vec::with_capacity(ids.len() * 2);
    let mut prev = 0u64;
    for &id in ids {
        put_varint(&mut bytes, id.wrapping_sub(prev));
        prev = id;
    }
    crate::shards::write_file(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tinta-index-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_segment_reads_back_what_was_written() {
        let dir = temp("roundtrip");
        let path = dir.join("a.idx");
        let mut map = BTreeMap::new();
        for i in 0..1000u64 {
            map.insert(format!("w{i:04}"), vec![(i << 6) | 1, ((i + 5000) << 6) | 16]);
        }
        map.insert("factura".into(), vec![(7 << 6) | 1]);
        map.insert("facturas".into(), vec![(9 << 6) | 16]);
        map.insert("fax".into(), vec![(11 << 6) | 2]);
        write_map(&path, &map, &[7, 9, 11, 40_000]).unwrap();
        let seg = Segment::open(&path).unwrap();
        assert_eq!(seg.ids(), &[7, 9, 11, 40_000]);
        assert!(seg.contains(9) && !seg.contains(8));
        assert_eq!(seg.lookup("factur").unwrap(), vec![vec![(7 << 6) | 1], vec![(9 << 6) | 16]]);
        assert_eq!(seg.lookup("fa").unwrap().len(), 3);
        assert_eq!(seg.lookup("w0999").unwrap(), vec![vec![(999 << 6) | 1, (5999 << 6) | 16]]);
        assert_eq!(seg.lookup("w").unwrap().len(), 1000);
        assert!(seg.lookup("zzz").unwrap().is_empty());
        assert!(seg.lookup("a").unwrap().is_empty());
        let mut words = seg.words().unwrap();
        let mut n = 0;
        while let Some((w, p)) = words.next_word().unwrap() {
            assert_eq!(map.get(&w), Some(&p));
            n += 1;
        }
        assert_eq!(n, map.len());
    }

    #[test]
    fn a_damaged_file_is_refused_and_never_panics() {
        let dir = temp("damaged");
        let path = dir.join("a.idx");
        let mut map = BTreeMap::new();
        map.insert("hola".to_string(), vec![(1 << 6) | 1, (2 << 6) | 1]);
        map.insert("mundo".to_string(), vec![(2 << 6) | 16]);
        write_map(&path, &map, &[1, 2]).unwrap();
        let good = std::fs::read(&path).unwrap();
        // Every byte changed in turn, and every length cut: never a panic, and a changed file
        // either is refused or still reads.
        for i in 0..good.len() {
            for flip in [0x01u8, 0x80, 0xff] {
                let mut bad = good.clone();
                bad[i] ^= flip;
                std::fs::write(&path, &bad).unwrap();
                if let Ok(seg) = Segment::open(&path) {
                    let _ = seg.lookup("h");
                    let _ = seg.lookup("");
                }
            }
        }
        for cut in 0..good.len() {
            std::fs::write(&path, &good[..cut]).unwrap();
            assert!(Segment::open(&path).is_err(), "cut at {cut}");
        }
        assert!(Segment::open(&dir.join("missing.idx")).is_err());
    }

    #[test]
    fn stale_lists_round_trip() {
        let dir = temp("stale");
        let path = dir.join("a.stale");
        assert!(read_stale(&path).unwrap().is_empty());
        write_stale(&path, &[3, 9, 1_000_000]).unwrap();
        assert_eq!(read_stale(&path).unwrap(), [3, 9, 1_000_000]);
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read_stale(&path).is_err());
    }

    #[test]
    fn numbers_round_trip() {
        for v in [0u64, 1, 127, 128, 300, u64::from(u32::MAX), u64::MAX] {
            let mut b = Vec::new();
            put_varint(&mut b, v);
            let mut at = 0;
            assert_eq!(get_varint(&b, &mut at), Some(v));
            assert_eq!(at, b.len());
        }
        assert_eq!(get_varint(&[0xff; 11], &mut 0), None);
        assert_eq!(decode_postings(&[1, 0], 2), None, "postings must rise");
    }
}
