//! Message bodies Outlook keeps as compressed RTF.
//!
//! Outlook often saves a message's formatted body only in `PidTagRtfCompressed`: RTF packed with
//! the LZ77 variant of MS-OXRTFCP. Mail that arrived as HTML is kept there too, wrapped in RTF as
//! MS-OXRTFEX describes (`\fromhtml1`, the original tags in `{\*\htmltag …}` groups, the RTF-only
//! parts marked `\htmlrtf`), and plain-text mail as `\fromtext`.
//!
//! [`decompress`] unpacks the stream; [`body`] takes the RTF apart again into the original HTML,
//! or the text of a message written as RTF. Formatting of RTF-native messages (bold, colours,
//! tables) is not kept: their text is.

use ibercorreo_core::{Error, Result};

/// Largest decompressed RTF read.
pub const MAX_RTF_BYTES: usize = 32 * 1024 * 1024;
/// Groups nested deeper than this are read as part of their parent.
const MAX_GROUPS: usize = 512;
/// The dictionary's size; positions wrap at it.
const DICT: usize = 4096;
/// `LZFu`: compressed.
const COMPRESSED: u32 = 0x7546_5a4c;
/// `MELA`: stored as is.
const UNCOMPRESSED: u32 = 0x414c_454d;

/// The dictionary's initial contents (MS-OXRTFCP 2.1.2.1), 207 bytes.
pub(crate) const PREFIX: &[u8] = b"{\\rtf1\\ansi\\mac\\deff0\\deftab720{\\fonttbl;}{\\f0\\fnil \\froman \\fswiss \\fmodern \\fscript \\fdecor MS Sans SerifSymbolArialTimes New RomanCourier{\\colortbl\\red0\\green0\\blue0\r\n\\par \\pard\\plain\\f0\\fs20\\b\\i\\u\\tab\\tx";

fn bad(detail: &str) -> Error {
    Error::invalid("compressed RTF", detail)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let bytes = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*bytes.first()?, *bytes.get(1)?, *bytes.get(2)?, *bytes.get(3)?]))
}

/// The CRC MS-OXRTFCP 3.1.3 checks compressed data with: CRC-32's table, starting from 0 and not
/// inverted at the end.
pub(crate) fn crc(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |crc, &b| {
        let mut c = (crc ^ u32::from(b)) & 0xff;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        c ^ (crc >> 8)
    })
}

/// Unpacks `PidTagRtfCompressed`. Data that fails its checksum is refused rather than read as
/// garbage.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let (Some(comp_size), Some(raw_size), Some(kind), Some(sum)) = (u32_at(data, 0), u32_at(data, 4), u32_at(data, 8), u32_at(data, 12)) else {
        return Err(bad("shorter than its header"));
    };
    let raw_size = usize::try_from(raw_size).unwrap_or(usize::MAX);
    if raw_size > MAX_RTF_BYTES {
        return Err(Error::TooLarge { what: "compressed RTF", size: raw_size, limit: MAX_RTF_BYTES });
    }
    // The size counts from after its own field, so it includes the rest of the header.
    let end = usize::try_from(comp_size).unwrap_or(usize::MAX).saturating_add(4).min(data.len());
    let payload = data.get(16..end).unwrap_or(&[]);
    match kind {
        UNCOMPRESSED => Ok(payload.get(..raw_size).unwrap_or(payload).to_vec()),
        COMPRESSED if crc(payload) != sum => Err(bad("the checksum does not match")),
        COMPRESSED => Ok(lzfu(payload, raw_size)),
        _ => Err(bad("not a known compression type")),
    }
}

/// MS-OXRTFCP 2.1.3.1: runs of eight items, each a literal byte or a 12-bit dictionary position
/// with a 4-bit length, until the reference that points at the write position.
fn lzfu(payload: &[u8], raw_size: usize) -> Vec<u8> {
    let mut dict = [0u8; DICT];
    for (slot, &b) in dict.iter_mut().zip(PREFIX) {
        *slot = b;
    }
    let mut write = PREFIX.len();
    let mut out = Vec::with_capacity(raw_size.min(payload.len().saturating_mul(8)));
    let mut input = payload.iter().copied();
    'runs: while let Some(control) = input.next() {
        for bit in 0..8 {
            if control & (1 << bit) == 0 {
                let Some(b) = input.next() else { break 'runs };
                if put(b, &mut out, &mut dict, &mut write, raw_size) {
                    break 'runs;
                }
                continue;
            }
            let (Some(hi), Some(lo)) = (input.next(), input.next()) else { break 'runs };
            let reference = u16::from_be_bytes([hi, lo]);
            let offset = usize::from(reference >> 4);
            if offset == write {
                break 'runs;
            }
            for k in 0..usize::from(reference & 0xf) + 2 {
                let b = *dict_get(&dict, offset + k);
                if put(b, &mut out, &mut dict, &mut write, raw_size) {
                    break 'runs;
                }
            }
        }
    }
    out
}

/// Writes one byte to the output and the dictionary; `true` once the output is full.
fn put(b: u8, out: &mut Vec<u8>, dict: &mut [u8; DICT], write: &mut usize, raw_size: usize) -> bool {
    out.push(b);
    if let Some(slot) = dict.get_mut(*write) {
        *slot = b;
    }
    *write = (*write + 1) % DICT;
    out.len() >= raw_size
}

fn dict_get(dict: &[u8; DICT], at: usize) -> &u8 {
    dict.get(at % DICT).unwrap_or(&0)
}

/// What an RTF body holds once taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// The original HTML of a message Outlook received or wrote as HTML.
    Html(String),
    /// The text of a plain-text message, or of a message written as RTF.
    Text(String),
}

/// Destinations whose text is not part of the body.
const SKIPPED: &[&[u8]] = &[
    b"fonttbl",
    b"colortbl",
    b"stylesheet",
    b"info",
    b"pict",
    b"object",
    b"header",
    b"headerl",
    b"headerr",
    b"headerf",
    b"footer",
    b"footerl",
    b"footerr",
    b"footerf",
    b"listtable",
    b"listoverridetable",
    b"revtbl",
    b"rsidtbl",
    b"filetbl",
    b"fldinst",
    b"xmlnstbl",
    b"themedata",
    b"colorschememapping",
    b"latentstyles",
    b"datastore",
    b"pntext",
    b"generator",
];

#[derive(Clone, Copy)]
struct Group {
    /// Nothing in the group is part of the body.
    skip: bool,
    /// An `\htmltag` group: its contents are HTML, whatever `\htmlrtf` says.
    tag: bool,
    /// `\htmlrtf` is on: what follows exists only for RTF readers.
    rtf_only: bool,
    /// `\ucN`: how many fallback characters follow each `\uN`.
    uc: usize,
    /// The group's first token is still to come (it may name a destination).
    first: bool,
    /// `{\*` was just read: the next control word names an optional destination.
    star: bool,
}

/// Collects output text, decoding runs of code-page bytes together.
struct Out {
    text: String,
    bytes: Vec<u8>,
    codepage: u32,
    high: Option<u16>,
}

impl Out {
    fn flush(&mut self) {
        if !self.bytes.is_empty() {
            let decoded = crate::props::decode(&self.bytes, self.codepage);
            self.text.push_str(&decoded);
            self.bytes.clear();
        }
    }
    fn byte(&mut self, b: u8) {
        self.high = None;
        self.bytes.push(b);
    }
    fn str(&mut self, s: &str) {
        self.flush();
        self.high = None;
        self.text.push_str(s);
    }
    fn unit(&mut self, u: u16) {
        self.flush();
        match (self.high.take(), u) {
            (_, 0xd800..=0xdbff) => self.high = Some(u),
            (Some(h), 0xdc00..=0xdfff) => self.text.extend(char::decode_utf16([h, u]).flatten()),
            (_, u) => self.text.push(char::from_u32(u32::from(u)).unwrap_or(char::REPLACEMENT_CHARACTER)),
        }
    }
    fn len(&self) -> usize {
        self.text.len().saturating_add(self.bytes.len())
    }
}

/// Takes an RTF body apart (MS-OXRTFEX 2.1.3): the HTML of `\fromhtml1` RTF, the text of
/// `\fromtext` and of any other RTF. `None` when it is not RTF.
pub fn body(rtf: &[u8]) -> Option<Body> {
    let start = rtf.iter().position(|&b| b == b'{')?;
    let rtf = rtf.get(start..)?;
    if !rtf.starts_with(b"{\\rtf") {
        return None;
    }
    let limit = ibercorreo_mime::MAX_BODY_BYTES;
    let mut html = false;
    let mut out = Out { text: String::new(), bytes: Vec::new(), codepage: 1252, high: None };
    let base = Group { skip: false, tag: false, rtf_only: false, uc: 1, first: false, star: false };
    let mut stack: Vec<Group> = Vec::new();
    let mut deeper = 0usize;
    let mut g = base;
    // Fallback characters still to skip after a `\uN`.
    let mut fallback = 0usize;
    let mut i = 0usize;
    while i < rtf.len() && out.len() < limit {
        let Some(&c) = rtf.get(i) else { break };
        i += 1;
        let first = std::mem::replace(&mut g.first, false);
        let star = std::mem::replace(&mut g.star, false);
        let emit = !g.skip && (g.tag || !g.rtf_only);
        match c {
            b'{' => {
                fallback = 0;
                if stack.len() < MAX_GROUPS {
                    stack.push(g);
                    g.first = true;
                } else {
                    deeper += 1;
                }
            }
            b'}' => {
                fallback = 0;
                if deeper > 0 {
                    deeper -= 1;
                } else if let Some(parent) = stack.pop() {
                    g = parent;
                    if stack.is_empty() {
                        // The document's own group closed: anything after it is not RTF.
                        break;
                    }
                } else {
                    break;
                }
            }
            b'\r' | b'\n' => {}
            b'\\' => {
                let Some(&next) = rtf.get(i) else { break };
                if next.is_ascii_alphabetic() {
                    let name_end = rtf.get(i..).map_or(i, |r| i + r.iter().take(32).take_while(|b| b.is_ascii_alphabetic()).count());
                    let name = rtf.get(i..name_end).unwrap_or(&[]);
                    i = name_end;
                    let neg = rtf.get(i) == Some(&b'-');
                    let digits_from = i + usize::from(neg);
                    let digits = rtf.get(digits_from..).map_or(0, |r| r.iter().take(10).take_while(|b| b.is_ascii_digit()).count());
                    let param = if digits > 0 {
                        let text = rtf.get(digits_from..digits_from + digits).unwrap_or(&[]);
                        let n = std::str::from_utf8(text).ok().and_then(|t| t.parse::<i64>().ok()).unwrap_or(0);
                        i = digits_from + digits;
                        Some(if neg { n.saturating_neg() } else { n })
                    } else {
                        None
                    };
                    if rtf.get(i) == Some(&b' ') {
                        i += 1;
                    }
                    if fallback > 0 {
                        fallback -= 1;
                        continue;
                    }
                    if star {
                        if name == b"htmltag" {
                            g.tag = true;
                        } else {
                            g.skip = true;
                        }
                        continue;
                    }
                    if first && SKIPPED.contains(&name) {
                        g.skip = true;
                        continue;
                    }
                    word(name, param, emit, &mut g, &mut out, &mut html, &mut fallback);
                } else {
                    i += 1;
                    match next {
                        b'*' if first => g.star = true,
                        b'\'' => {
                            let hex = rtf.get(i..i + 2).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                            if let Some(b) = hex {
                                i += 2;
                                if fallback > 0 {
                                    fallback -= 1;
                                } else if emit {
                                    out.byte(b);
                                }
                            }
                        }
                        _ if fallback > 0 => fallback -= 1,
                        b'\\' | b'{' | b'}' if emit => out.byte(next),
                        b'~' if emit => out.str("\u{a0}"),
                        b'_' if emit => out.str("\u{2011}"),
                        b'\r' | b'\n' if emit => out.str("\n"),
                        b'\t' if emit => out.str("\t"),
                        _ => {}
                    }
                }
            }
            _ if fallback > 0 => fallback -= 1,
            _ if emit => out.byte(c),
            _ => {}
        }
    }
    out.flush();
    let text = out.text;
    Some(if html { Body::Html(text) } else { Body::Text(text.replace("\r\n", "\n")) })
}

/// One control word.
fn word(name: &[u8], param: Option<i64>, emit: bool, g: &mut Group, out: &mut Out, html: &mut bool, fallback: &mut usize) {
    match name {
        b"fromhtml" => *html = param != Some(0),
        b"htmlrtf" => g.rtf_only = param != Some(0),
        b"ansicpg" => {
            out.flush();
            out.codepage = param.and_then(|p| u32::try_from(p).ok()).unwrap_or(1252);
        }
        b"uc" => g.uc = param.and_then(|p| usize::try_from(p).ok()).unwrap_or(1).min(16),
        b"u" => {
            if emit {
                let p = param.unwrap_or(0);
                let unit = if p < 0 { p.saturating_add(65_536) } else { p };
                out.unit(u16::try_from(unit).unwrap_or(0xfffd));
            }
            *fallback = g.uc;
        }
        _ if !emit => {}
        b"par" | b"line" | b"row" => out.str("\r\n"),
        b"tab" | b"cell" => out.str("\t"),
        b"emdash" => out.str("\u{2014}"),
        b"endash" => out.str("\u{2013}"),
        b"bullet" => out.str("\u{2022}"),
        b"lquote" => out.str("\u{2018}"),
        b"rquote" => out.str("\u{2019}"),
        b"ldblquote" => out.str("\u{201c}"),
        b"rdblquote" => out.str("\u{201d}"),
        b"emspace" | b"enspace" | b"qmspace" => out.str(" "),
        _ => {}
    }
}

#[cfg(test)]
mod tests;
