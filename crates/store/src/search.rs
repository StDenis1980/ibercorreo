//! Full-text search: an index of the words in every message, and Outlook-style queries over it.
//!
//! Words are folded before they are indexed or looked up: lower case, Latin accents dropped
//! (`camión` finds `camion` and the other way round, `ß` is `ss`), the Catalan middle dot joined
//! (`col·lecció` is `colleccio`). Cyrillic keeps its letters (`й` is not `и`, `ї` is not `і`), only
//! its case folds, and `ё` is `е`. A word with an apostrophe is indexed whole and in parts, so
//! `пʼять`, `п'ять` and `пять` find each other and `aigua` finds `l'aigua`. A query word matches
//! the words that start with it, as in Outlook: `fact` finds `factura`.
//!
//! # Queries
//! - `word word`: both must match, in any field (subject, sender, recipients, body, attachment
//!   names).
//! - `"exact phrase"`: the words next to each other, in this order.
//! - `-word`, `-from:ana`: must not match.
//! - `OR` (in capitals) between alternatives: `from:ana OR from:luis`.
//! - Fields: `from:`, `to:`, `cc:`, `subject:`, `body:`, `attachment:`, each with a value or a
//!   quoted phrase (`subject:"weekly report"`). Ukrainian and Spanish names work too: `від:`,
//!   `кому:`, `тема:`, `текст:`, `вкладення:`, `de:`, `para:`, `asunto:`, `cuerpo:`, `adjunto:`.
//! - `has:attachment`, `is:unread`, `is:read`, `is:flagged`.
//! - Tinta: `category:name` (in quotes when it has spaces): mail with that colour category.
//! - Dates (`YYYY-MM-DD`, UTC): `after:` (that day or later), `before:` (earlier than that day),
//!   `on:` (that day).
//!
//! Anything that is not one of these is searched for as words, so `10:30` or `http://x` never
//! fail. The index is brought up to date at each search, re-reading only the messages that
//! changed since the last one ([`Stamps`]).
//!
//! Tinta: most of the index lives on disk ([`crate::search_file`]): a save writes a segment of
//! every message's words once the messages it does not cover pass [`REWRITE_AFTER`], and the next
//! start opens it instead of reading every message's text again. Only the messages the segment
//! does not cover (new mail, changed drafts) are indexed in memory.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ibercorreo_core::{AccountId, Content, Date, FolderId, Message, MessageId};

use crate::search_file::{self, Segment};
use crate::shards::Reader;

/// Longest word kept, in characters; longer ones are cut (a query word is cut the same way).
pub const MAX_WORD: usize = 40;
/// Most distinct words indexed per message and field.
pub const MAX_WORDS_PER_FIELD: usize = 20_000;
/// Most conditions read from one query; the rest are ignored.
pub const MAX_CLAUSES: usize = 64;

/// Where a word was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    Subject,
    From,
    To,
    Cc,
    Body,
    Attachment,
}

impl Field {
    pub const ALL: [Field; 6] = [Field::Subject, Field::From, Field::To, Field::Cc, Field::Body, Field::Attachment];

    /// The field a `name:` prefix (lowercase, without the colon) stands for.
    fn named(name: &str) -> Option<Field> {
        Some(match name {
            "from" | "від" | "de" => Field::From,
            "to" | "кому" | "para" => Field::To,
            "cc" | "копія" => Field::Cc,
            "subject" | "тема" | "asunto" => Field::Subject,
            "body" | "текст" | "cuerpo" => Field::Body,
            "attachment" | "attachments" | "вкладення" | "adjunto" => Field::Attachment,
            _ => return None,
        })
    }

    /// The field's text in `m`, as one string per part (each address, each attachment name).
    /// The field's text in `m`, which says `content` (the body is not searched without it).
    fn texts<'a>(self, m: &'a Message, content: Option<&'a Content>) -> Vec<&'a str> {
        fn addresses(list: &[ibercorreo_core::Address]) -> Vec<&str> {
            let mut v = Vec::new();
            for a in list {
                v.push(a.email.as_str());
                if let Some(n) = &a.name {
                    v.push(n.as_str());
                }
            }
            v
        }
        match self {
            Field::Subject => vec![m.subject.as_str()],
            Field::From => addresses(m.from.as_slice()),
            Field::To => addresses(&m.to),
            Field::Cc => {
                let mut v = addresses(&m.cc);
                v.extend(addresses(&m.bcc));
                v
            }
            Field::Body => content.map(|c| c.body.as_str()).into_iter().collect(),
            Field::Attachment => m.attachments.iter().map(|a| a.name.as_str()).collect(),
        }
    }
}

// ---- words ----

/// `c` lowercased, without its Latin accent, written to `out`.
fn fold_char(c: char, out: &mut String) {
    if c == '·' {
        // The Catalan middle dot (`l·l`) joins its word.
        return;
    }
    for c in c.to_lowercase() {
        let plain = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
            'æ' => "ae",
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
            'ď' | 'đ' | 'ð' => "d",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
            'ĥ' | 'ħ' => "h",
            'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
            'ĵ' => "j",
            'ķ' => "k",
            'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
            'ñ' | 'ń' | 'ņ' | 'ň' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
            'œ' => "oe",
            'ŕ' | 'ŗ' | 'ř' => "r",
            'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => "s",
            'ß' => "ss",
            'ţ' | 'ť' | 'ŧ' | 'ț' => "t",
            'þ' => "th",
            'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
            'ŵ' => "w",
            'ý' | 'ÿ' | 'ŷ' => "y",
            'ź' | 'ż' | 'ž' => "z",
            'ё' => "е",
            _ => {
                out.push(c);
                continue;
            }
        };
        out.push_str(plain);
    }
}

/// `text` lowercased and without Latin accents.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        fold_char(c, &mut out);
    }
    out
}

/// Characters that join the parts of one word (`l'aigua`, `пʼять`).
fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '’' | 'ʼ' | '`')
}

/// A word cut to [`MAX_WORD`] characters.
fn cut(word: &str) -> String {
    word.chars().take(MAX_WORD).collect()
}

/// The folded words of `text`, in order (`parts`), and the whole forms of words written with an
/// apostrophe (`joined`: `laigua` for `l'aigua`).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Words {
    pub parts: Vec<String>,
    pub joined: Vec<String>,
}

pub fn words(text: &str) -> Words {
    let folded = fold(text);
    let mut w = Words::default();
    let mut cur = String::new();
    let mut whole = String::new();
    let mut pieces = 0usize;
    fn finish(cur: &mut String, whole: &mut String, pieces: &mut usize, w: &mut Words) {
        if !cur.is_empty() {
            w.parts.push(cut(cur));
            *pieces = pieces.saturating_add(1);
        }
        if *pieces > 1 {
            w.joined.push(cut(whole));
        }
        cur.clear();
        whole.clear();
        *pieces = 0;
    }
    for c in folded.chars() {
        if c.is_alphanumeric() {
            cur.push(c);
            whole.push(c);
        } else if is_apostrophe(c) && !cur.is_empty() {
            w.parts.push(cut(&cur));
            pieces = pieces.saturating_add(1);
            cur.clear();
        } else {
            finish(&mut cur, &mut whole, &mut pieces, &mut w);
        }
    }
    finish(&mut cur, &mut whole, &mut pieces, &mut w);
    w
}

// ---- change tracking ----

static NEXT_STAMP: AtomicU64 = AtomicU64::new(1);

/// A number never handed out before in this process.
fn fresh_stamp() -> u64 {
    NEXT_STAMP.fetch_add(1, Ordering::Relaxed)
}

/// Which version of each message the index has to match. A message changed through the store
/// gets a stamp of its own; the rest share the store's `base`, given when the store was created
/// or read from disk. Stamps are unique in the process, so a store put back by undo, or a copy,
/// never passes for one whose messages differ. Not part of the store's value (always equal).
#[derive(Debug, Clone)]
pub struct Stamps {
    base: u64,
    changed: HashMap<MessageId, u64>,
}

impl Default for Stamps {
    fn default() -> Stamps {
        Stamps { base: fresh_stamp(), changed: HashMap::new() }
    }
}

impl PartialEq for Stamps {
    fn eq(&self, _: &Stamps) -> bool {
        true
    }
}

impl Eq for Stamps {}

impl Stamps {
    /// `id` may have changed: the index reads it again.
    pub fn touch(&mut self, id: MessageId) {
        self.changed.insert(id, fresh_stamp());
    }

    /// `id` is gone.
    pub fn forget(&mut self, id: MessageId) {
        self.changed.remove(&id);
    }

    fn of(&self, id: MessageId) -> u64 {
        self.changed.get(&id).copied().unwrap_or(self.base)
    }

    /// The stamps of a store as opened, with `stale` marked as matching no version of it (0 is
    /// never handed out).
    fn marked(&self, stale: &[u64]) -> Stamps {
        Stamps { base: self.base, changed: stale.iter().map(|&id| (MessageId(id), 0)).collect() }
    }
}

// ---- the index ----

/// One message's words, read on its own (for rules): sorted, so a prefix is a binary search.
#[derive(Debug, Default)]
struct MessageWords {
    keys: Vec<(Field, String)>,
}

/// The distinct words of `m` (which says `content`) and the fields each is in, at most
/// [`MAX_WORDS_PER_FIELD`] per field.
fn read_words(m: &Message, content: Option<&Content>) -> BTreeMap<String, u8> {
    let mut found: BTreeMap<String, u8> = BTreeMap::new();
    for field in Field::ALL {
        let mut n = 0usize;
        for text in field.texts(m, content) {
            let w = words(text);
            for word in w.parts.into_iter().chain(w.joined) {
                if n >= MAX_WORDS_PER_FIELD {
                    break;
                }
                let mask = found.entry(word).or_default();
                if *mask & field.bit() == 0 {
                    *mask |= field.bit();
                    n = n.saturating_add(1);
                }
            }
        }
    }
    found
}

impl MessageWords {
    fn read(m: &Message, content: Option<&Content>) -> MessageWords {
        let mut keys: Vec<(Field, String)> = Vec::new();
        for (word, mask) in read_words(m, content) {
            for field in Field::ALL.into_iter().filter(|f| mask & f.bit() != 0) {
                keys.push((field, word.clone()));
            }
        }
        keys.sort();
        MessageWords { keys }
    }

    /// Whether a word in `field` starts with `prefix`.
    fn has_prefix(&self, field: Field, prefix: &str) -> bool {
        let at = self.keys.partition_point(|(f, w)| (*f, w.as_str()) < (field, prefix));
        self.keys.get(at).is_some_and(|(f, w)| *f == field && w.starts_with(prefix))
    }
}

impl Field {
    /// The field's bit in a posting's field mask.
    fn bit(self) -> u8 {
        match self {
            Field::Subject => 1,
            Field::From => 2,
            Field::To => 4,
            Field::Cc => 8,
            Field::Body => 16,
            Field::Attachment => 32,
        }
    }
}

/// Bits a posting keeps for the field mask, below the message id.
const MASK_BITS: u32 = 6;

/// A posting: the message id and the fields the word is in, in one number. `None` for an id too
/// big to pack (only a hand-edited store has one): that message is not found by its words.
fn pack(id: MessageId, mask: u8) -> Option<u64> {
    let shifted = id.0.checked_shl(MASK_BITS)?;
    (shifted >> MASK_BITS == id.0).then_some(shifted | u64::from(mask))
}

fn posting_id(p: u64) -> u64 {
    p >> MASK_BITS
}

/// One message's indexed words (as word numbers) and the stamp they were read at.
#[derive(Debug, Default)]
struct Entry {
    stamp: u64,
    words: Box<[u32]>,
}

/// Tinta: the segment on disk, and the versions of the messages it has.
#[derive(Debug)]
struct Base {
    seg: Segment,
    generation: String,
    /// The folder it is in: a store saved somewhere else writes a segment of its own.
    dir: PathBuf,
    /// The stamps the messages had when the segment was written: one whose stamp differs now
    /// changed since, and its words are read again into memory.
    valid: Stamps,
    /// The ids the stale list on disk already names.
    stale_saved: BTreeSet<u64>,
}

impl Base {
    /// Whether the segment has `id` as it is now (`stamps`).
    fn has(&self, id: MessageId, stamps: &Stamps) -> bool {
        stamps.of(id) == self.valid.of(id) && self.seg.contains(id.0)
    }
}

/// Words to messages, and each message's words. Each distinct word is kept once (`dict`), and a
/// word's postings are packed numbers in id order, so the index stays a fraction of the mail's
/// size: a mailbox of 60,000 messages took gigabytes when every message kept its own copy of
/// its words. Tinta: messages the segment on disk has as they are now (`base`) are not here.
#[derive(Debug, Default)]
pub struct Index {
    entries: HashMap<MessageId, Entry>,
    dict: HashMap<Box<str>, u32>,
    postings: Vec<Vec<u64>>,
    base: Option<Base>,
}

impl Index {
    /// Read the messages that changed since the last call (their content through `contents`);
    /// drop the ones that are gone.
    fn refresh(&mut self, messages: &[Arc<Message>], stamps: &Stamps, contents: &mut Reader) {
        let mut here = 0usize;
        for m in messages {
            if self.base.as_ref().is_some_and(|b| b.has(m.id, stamps)) {
                if self.entries.contains_key(&m.id) {
                    self.remove(m.id);
                }
                continue;
            }
            here = here.saturating_add(1);
            let stamp = stamps.of(m.id);
            if self.entries.get(&m.id).is_some_and(|e| e.stamp == stamp) {
                continue;
            }
            self.remove(m.id);
            let content = contents.get(m);
            self.add(m, content.as_deref(), stamp);
        }
        // Ids are unique, so as many entries as messages kept here means none is stale.
        if self.entries.len() != here {
            let live: std::collections::HashSet<MessageId> = messages.iter().map(|m| m.id).collect();
            let gone: Vec<MessageId> = self.entries.keys().filter(|id| !live.contains(id)).copied().collect();
            for id in gone {
                self.remove(id);
            }
        }
    }

    /// The number of `word`, given one if it is new; `None` once `u32` numbers run out.
    fn word_number(&mut self, word: String) -> Option<u32> {
        if let Some(&n) = self.dict.get(word.as_str()) {
            return Some(n);
        }
        let n = u32::try_from(self.postings.len()).ok()?;
        self.postings.push(Vec::new());
        self.dict.insert(word.into_boxed_str(), n);
        Some(n)
    }

    fn add(&mut self, m: &Message, content: Option<&Content>, stamp: u64) {
        let mut numbers = Vec::new();
        for (word, mask) in read_words(m, content) {
            let Some(p) = pack(m.id, mask) else { break };
            let Some(n) = self.word_number(word) else { break };
            let Some(list) = self.postings.get_mut(n as usize) else { continue };
            // New mail has the highest ids, so this is nearly always a push.
            if list.last().is_none_or(|&q| posting_id(q) < m.id.0) {
                list.push(p);
            } else {
                let at = list.partition_point(|&q| posting_id(q) < m.id.0);
                list.insert(at, p);
            }
            numbers.push(n);
        }
        self.entries.insert(m.id, Entry { stamp, words: numbers.into_boxed_slice() });
    }

    /// Remove many messages at once: one pass over the postings, where one at a time would move
    /// each long list once per message.
    fn remove_many(&mut self, ids: &[MessageId]) {
        let mut gone: HashSet<u64> = HashSet::with_capacity(ids.len());
        let mut words: Vec<u32> = Vec::new();
        for id in ids {
            if let Some(entry) = self.entries.remove(id) {
                gone.insert(id.0);
                words.extend(entry.words.iter().copied());
            }
        }
        if gone.is_empty() {
            return;
        }
        words.sort_unstable();
        words.dedup();
        for n in words {
            if let Some(list) = self.postings.get_mut(n as usize) {
                list.retain(|&p| !gone.contains(&posting_id(p)));
            }
        }
    }

    fn remove(&mut self, id: MessageId) {
        let Some(entry) = self.entries.remove(&id) else { return };
        for &n in entry.words.iter() {
            if let Some(list) = self.postings.get_mut(n as usize) {
                let at = list.partition_point(|&q| posting_id(q) < id.0);
                if list.get(at).is_some_and(|&q| posting_id(q) == id.0) {
                    list.remove(at);
                }
            }
        }
    }

    /// Messages with a word in one of `fields` that starts with `prefix`: in memory, and in the
    /// segment on disk as the messages are now (`stamps`). `None` when the segment could not be
    /// read: it is dropped, and the caller indexes again.
    fn lookup(&mut self, fields: &[Field], prefix: &str, stamps: &Stamps) -> Option<BTreeSet<MessageId>> {
        let mask = fields.iter().fold(0u8, |m, f| m | f.bit());
        let mut out = self.lookup_here(mask, prefix);
        if let Some(b) = &self.base {
            match b.seg.lookup(prefix) {
                Ok(lists) => {
                    for p in lists.into_iter().flatten() {
                        let id = MessageId(posting_id(p));
                        if p & u64::from(mask) != 0 && stamps.of(id) == b.valid.of(id) && !self.entries.contains_key(&id) {
                            out.insert(id);
                        }
                    }
                }
                Err(_) => {
                    self.base = None;
                    return None;
                }
            }
        }
        Some(out)
    }

    /// The messages indexed in memory with a word in the `mask` fields that starts with `prefix`.
    fn lookup_here(&self, mask: u8, prefix: &str) -> BTreeSet<MessageId> {
        let mut out = BTreeSet::new();
        // A scan of the distinct words: a few milliseconds even for a big mailbox, and it keeps
        // adding a word to the index a hash lookup.
        for (_, &n) in self.dict.iter().filter(|(word, _)| word.starts_with(prefix)) {
            for &p in self.postings.get(n as usize).map(Vec::as_slice).unwrap_or_default() {
                if p & u64::from(mask) != 0 {
                    out.insert(MessageId(posting_id(p)));
                }
            }
        }
        out
    }

    /// How many messages are indexed in memory.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The index kept beside a store, shared by its copies (undo snapshots): entries are matched by
/// stamp, so a copy only re-reads the messages it differs in. Not part of the store's value.
#[derive(Clone, Default)]
pub struct SearchCache(Arc<Mutex<Index>>);

impl std::fmt::Debug for SearchCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SearchCache")
    }
}

impl SearchCache {
    /// The index, locked. A poisoned lock means a search panicked half way: start it again.
    fn lock(&self) -> std::sync::MutexGuard<'_, Index> {
        let guard = self.0.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            *guard = Index::default();
            guard
        });
        self.0.clear_poison();
        guard
    }

    /// How many messages the index has read into memory.
    #[cfg(test)]
    pub(crate) fn indexed(&self) -> usize {
        self.lock().len()
    }

    /// The generation of the segment on disk in use, if any.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> Option<String> {
        self.lock().base.as_ref().map(|b| b.generation.clone())
    }
}

impl PartialEq for SearchCache {
    fn eq(&self, _: &SearchCache) -> bool {
        true
    }
}

impl Eq for SearchCache {}

// ---- queries ----

#[derive(Debug, Clone, PartialEq, Eq)]
enum Cond {
    /// Every word, in `fields`; with `phrase`, also next to each other in one of them.
    Words {
        fields: Vec<Field>,
        words: Vec<String>,
        phrase: bool,
    },
    HasAttachment,
    Unread,
    Read,
    Flagged,
    /// Tinta: has a colour category whose name is this (folded).
    Category(String),
    /// Sent on this day or later.
    After(Date),
    /// Sent before this day.
    Before(Date),
    On(Date),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Clause {
    not: bool,
    cond: Cond,
}

/// A parsed query: alternatives (`OR`), each a list of conditions that must all hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    groups: Vec<Vec<Clause>>,
    /// The text was blank: everything matches.
    blank: bool,
}

/// One item of the query text: an optional `-`, an optional `name:`, the value, and whether the
/// value was quoted.
struct Item {
    not: bool,
    name: Option<String>,
    value: String,
    quoted: bool,
}

fn lex(text: &str) -> Vec<Item> {
    let mut items = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else { break };
        let mut not = false;
        if first == '-' {
            chars.next();
            not = true;
        }
        let mut name = None;
        let mut value = String::new();
        let mut quoted = false;
        if chars.next_if_eq(&'"').is_some() {
            quoted = true;
            value.extend(chars.by_ref().take_while(|c| *c != '"'));
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                value.push(c);
                if c == ':' && name.is_none() {
                    let key = value.trim_end_matches(':').to_lowercase();
                    if !key.is_empty()
                        && (Field::named(&key).is_some() || matches!(key.as_str(), "has" | "is" | "after" | "before" | "on" | "category"))
                    {
                        name = Some(key);
                        value.clear();
                        if chars.next_if_eq(&'"').is_some() {
                            quoted = true;
                            value.extend(chars.by_ref().take_while(|c| *c != '"'));
                            break;
                        }
                    }
                }
            }
        }
        items.push(Item { not, name, value, quoted });
        if items.len() > MAX_CLAUSES.saturating_mul(2) {
            break;
        }
    }
    items
}

impl Query {
    pub fn parse(text: &str) -> Query {
        if text.trim().is_empty() {
            return Query { groups: Vec::new(), blank: true };
        }
        let mut groups: Vec<Vec<Clause>> = vec![Vec::new()];
        let mut count = 0usize;
        for item in lex(text) {
            if count >= MAX_CLAUSES {
                break;
            }
            if !item.not && !item.quoted && item.name.is_none() && item.value == "OR" {
                if groups.last().is_some_and(|g| !g.is_empty()) {
                    groups.push(Vec::new());
                }
                continue;
            }
            let Some(cond) = Self::cond(&item) else { continue };
            if let Some(g) = groups.last_mut() {
                g.push(Clause { not: item.not, cond });
                count = count.saturating_add(1);
            }
        }
        groups.retain(|g| !g.is_empty());
        Query { groups, blank: false }
    }

    fn cond(item: &Item) -> Option<Cond> {
        let words_in = |fields: Vec<Field>, text: &str| -> Option<Cond> {
            let parts = words(text).parts;
            if parts.is_empty() {
                return None;
            }
            Some(Cond::Words { fields, words: parts, phrase: item.quoted })
        };
        let value = item.value.trim();
        let Some(name) = item.name.as_deref() else { return words_in(Field::ALL.to_vec(), value) };
        let special = match name {
            "has" => matches!(fold(value).as_str(), "attachment" | "attachments").then_some(Cond::HasAttachment),
            "is" => match fold(value).as_str() {
                "unread" => Some(Cond::Unread),
                "read" => Some(Cond::Read),
                "flagged" => Some(Cond::Flagged),
                _ => None,
            },
            "category" => Some(fold(value)).filter(|v| !v.is_empty()).map(Cond::Category),
            "after" => Date::parse(value).ok().map(Cond::After),
            "before" => Date::parse(value).ok().map(Cond::Before),
            "on" => Date::parse(value).ok().map(Cond::On),
            _ => match Field::named(name) {
                Some(field) => return words_in(vec![field], value),
                None => None,
            },
        };
        // An unknown value (`is:later`, `after:yesterday`) is searched for as words.
        special.or_else(|| words_in(Field::ALL.to_vec(), &format!("{name} {value}")))
    }

    /// Whether the query text was blank (everything matches).
    pub fn is_blank(&self) -> bool {
        self.blank
    }

    /// Whether `m`, which says `content`, matches, read on its own (no index): for rules, which
    /// look at one message at a time as it arrives.
    pub fn matches(&self, m: &Message, content: Option<&Content>) -> bool {
        if self.blank {
            return true;
        }
        let own = MessageWords::read(m, content);
        self.groups.iter().any(|g| g.iter().all(|c| matches(&c.cond, m, content, &|fields, w| fields.iter().any(|f| own.has_prefix(*f, w))) != c.not))
    }

    /// Whether the query looks at a message's body.
    pub fn reads_body(&self) -> bool {
        self.groups.iter().flatten().any(|c| matches!(&c.cond, Cond::Words { fields, .. } if fields.contains(&Field::Body)))
    }
}

/// Which messages a search looks through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// Every folder of every account.
    #[default]
    All,
    /// Every folder of one account.
    Account(AccountId),
    /// One folder.
    Folder(FolderId),
}

/// Whether `words`, in this order, are next to each other in `text` (the last one may be the
/// start of a word).
fn has_phrase(text: &str, phrase: &[String]) -> bool {
    let Some((last, first)) = phrase.split_last() else { return false };
    let parts = words(text).parts;
    parts
        .windows(phrase.len())
        .any(|w| w.split_last().is_some_and(|(wl, wf)| wl.starts_with(last.as_str()) && wf.iter().zip(first).all(|(a, b)| a == b)))
}

/// Whether `m` meets `cond`; `has` says whether `m` has a word in one of the fields that starts
/// with the given one.
fn matches(cond: &Cond, m: &Message, content: Option<&Content>, has: &dyn Fn(&[Field], &str) -> bool) -> bool {
    match cond {
        Cond::Words { fields, words, phrase } => {
            let all = words.iter().all(|w| has(fields, w));
            if !all {
                return false;
            }
            if *phrase && words.len() > 1 { fields.iter().any(|f| f.texts(m, content).iter().any(|t| has_phrase(t, words))) } else { true }
        }
        Cond::HasAttachment => !m.attachments.is_empty(),
        Cond::Unread => !m.seen,
        Cond::Read => m.seen,
        Cond::Flagged => m.flagged,
        Cond::Category(name) => m.categories.iter().any(|c| fold(c) == *name),
        Cond::After(d) => m.date.is_some_and(|t| t.date >= *d),
        Cond::Before(d) => m.date.is_some_and(|t| t.date < *d),
        Cond::On(d) => m.date.is_some_and(|t| t.date == *d),
    }
}

/// Bring the index up to date with `messages`, so the next search only reads what changed after.
pub(crate) fn warm(cache: &SearchCache, stamps: &Stamps, messages: &[Arc<Message>], contents: &mut Reader) {
    cache.lock().refresh(messages, stamps, contents);
}

/// The messages of `messages` in `allowed` folders that match `query`, in no particular order.
pub(crate) fn run<'a>(
    cache: &SearchCache,
    stamps: &Stamps,
    messages: &'a [Arc<Message>],
    contents: &mut Reader,
    allowed: &dyn Fn(FolderId) -> bool,
    query: &Query,
) -> Vec<&'a Message> {
    let in_scope = messages.iter().map(|m| &**m).filter(|m| allowed(m.folder));
    if query.blank {
        return in_scope.collect();
    }
    if query.groups.is_empty() {
        return Vec::new();
    }
    let mut index = cache.lock();
    index.refresh(messages, stamps, contents);
    let mut hits: BTreeSet<MessageId> = BTreeSet::new();
    for group in &query.groups {
        // Each word of the group looked up once: the messages that have it in its fields.
        let mut found: HashMap<(Vec<Field>, String), BTreeSet<MessageId>> = HashMap::new();
        // Narrow down with the index: every positive word must be somewhere.
        let mut candidates: Option<BTreeSet<MessageId>> = None;
        for clause in group {
            if let Cond::Words { fields, words, .. } = &clause.cond {
                for w in words {
                    let key = (fields.clone(), w.clone());
                    if !found.contains_key(&key) {
                        let set = match index.lookup(fields, w, stamps) {
                            Some(set) => set,
                            None => {
                                // The segment on disk could not be read: index in memory instead.
                                index.refresh(messages, stamps, contents);
                                index.lookup(fields, w, stamps).unwrap_or_default()
                            }
                        };
                        found.insert(key.clone(), set);
                    }
                    let Some(set) = found.get(&key) else { continue };
                    if !clause.not {
                        candidates = Some(match candidates {
                            None => set.clone(),
                            Some(c) => c.intersection(set).copied().collect(),
                        });
                    }
                }
            }
        }
        // A phrase in the body is checked in the text, read for the messages that have its words.
        let phrase_in_body =
            group.iter().any(|c| matches!(&c.cond, Cond::Words { fields, words, phrase: true } if words.len() > 1 && fields.contains(&Field::Body)));
        for m in in_scope.clone() {
            if hits.contains(&m.id) || candidates.as_ref().is_some_and(|c| !c.contains(&m.id)) {
                continue;
            }
            let has = |fields: &[Field], w: &str| found.get(&(fields.to_vec(), w.to_string())).is_some_and(|set| set.contains(&m.id));
            let content = if phrase_in_body { contents.get(m) } else { None };
            if group.iter().all(|c| matches(&c.cond, m, content.as_deref(), &has) != c.not) {
                hits.insert(m.id);
            }
        }
    }
    drop(index);
    messages.iter().map(|m| &**m).filter(|m| hits.contains(&m.id)).collect()
}

// ---- on disk (Tinta) ----

/// A save writes a new segment once this many messages (or a tenth of the mailbox, if more) are
/// not in the one on disk as they are now.
pub const REWRITE_AFTER: usize = 2_000;
/// Messages whose words are gathered in memory before they are written out as a run.
const RUN: usize = 4_000;
/// How old a file of another generation must be before it is removed (another program may be
/// writing it for a store file it has not saved yet).
const PRUNE_AFTER: std::time::Duration = std::time::Duration::from_secs(60 * 60);

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// A generation name no other save will take: 32 hex digits from the time, the process and a
/// counter.
fn new_generation(dir: &Path) -> String {
    use sha2::Digest;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let seed = format!("{now}/{}/{}/{}", std::process::id(), NEXT_GENERATION.fetch_add(1, Ordering::Relaxed), dir.display());
    sha2::Sha256::digest(seed.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// At load: use the segment `generation` beside the store saved at `store_path` (`stamps` are
/// the store's as opened), if it is there and whole; otherwise the index is built in memory.
pub(crate) fn open_base(cache: &SearchCache, stamps: &Stamps, store_path: &Path, generation: &str) {
    if !search_file::is_generation(generation) {
        return;
    }
    let dir = search_file::dir(store_path);
    let Ok(seg) = Segment::open(&search_file::segment_path(&dir, generation)) else { return };
    let Ok(stale) = search_file::read_stale(&search_file::stale_path(&dir, generation)) else { return };
    let valid = stamps.marked(&stale);
    prune(&dir, generation);
    cache.lock().base = Some(Base { seg, generation: generation.to_string(), dir, valid, stale_saved: stale.into_iter().collect() });
}

/// Remove the files of other generations, and unfinished ones, older than [`PRUNE_AFTER`].
fn prune(dir: &Path, keep: &str) {
    let Ok(list) = std::fs::read_dir(dir) else { return };
    for entry in list.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let generation = name.split('.').next().unwrap_or("");
        if !search_file::is_generation(generation) || generation == keep {
            continue;
        }
        let old = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age >= PRUNE_AFTER);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// At a save of `messages` (`stamps` as saved) to `store_path`: keep the index on disk in step,
/// and return the generation the store file names. The stale list is brought up to date; a new
/// segment is written when too many messages are not in the one there is. `None` when there is
/// none to name (it could not be written): the next start indexes in memory.
pub(crate) fn persist(cache: &SearchCache, stamps: &Stamps, messages: &[Arc<Message>], contents: &mut Reader, store_path: &Path) -> Option<String> {
    let dir = search_file::dir(store_path);
    {
        let mut index = cache.lock();
        if let Some(b) = index.base.as_mut().filter(|b| b.dir == dir) {
            let missing = messages.iter().filter(|m| !b.has(m.id, stamps)).count();
            if missing <= REWRITE_AFTER.max(messages.len() / 10) {
                let mut stale = b.stale_saved.clone();
                stale.extend(b.seg.ids().iter().copied().filter(|&id| stamps.of(MessageId(id)) != b.valid.of(MessageId(id))));
                if stale != b.stale_saved {
                    let list: Vec<u64> = stale.iter().copied().collect();
                    search_file::write_stale(&search_file::stale_path(&dir, &b.generation), &list).ok()?;
                    b.stale_saved = stale;
                }
                return Some(b.generation.clone());
            }
        }
    }
    rewrite(cache, stamps, messages, contents, &dir).ok()
}

/// One source of words for a merge, with its next word ready.
struct Source {
    words: search_file::Words,
    head: Option<(String, Vec<u64>)>,
    /// Postings of other messages than these are left out (the old segment's changed ones).
    keep: Option<Arc<HashSet<u64>>>,
}

impl Source {
    fn advance(&mut self) -> ibercorreo_core::Result<()> {
        loop {
            self.head = self.words.next_word()?;
            match (&mut self.head, &self.keep) {
                (Some((_, postings)), Some(keep)) => {
                    postings.retain(|&p| keep.contains(&posting_id(p)));
                    if !postings.is_empty() {
                        return Ok(());
                    }
                }
                _ => return Ok(()),
            }
        }
    }
}

/// Write a new segment of `messages` (`stamps` as saved): the old segment's postings of the
/// messages it has as they are, and the words of the rest read now, a run at a time. Then it
/// takes the old one's place, in memory and on disk.
fn rewrite(cache: &SearchCache, stamps: &Stamps, messages: &[Arc<Message>], contents: &mut Reader, dir: &Path) -> ibercorreo_core::Result<String> {
    let storage = |e: std::io::Error| ibercorreo_core::Error::Storage(format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(storage)?;
    let generation = new_generation(dir);
    let live: HashSet<u64> = messages.iter().map(|m| m.id.0).collect();
    // What the old segment keeps, read through a handle of its own.
    let (old, kept) = {
        let index = cache.lock();
        match index.base.as_ref().filter(|b| b.dir == dir) {
            Some(b) => {
                let kept: HashSet<u64> =
                    b.seg.ids().iter().copied().filter(|&id| live.contains(&id) && stamps.of(MessageId(id)) == b.valid.of(MessageId(id))).collect();
                (Some((b.seg.words()?, b.seg.path().to_path_buf())), kept)
            }
            None => (None, HashSet::new()),
        }
    };
    let mut runs: Vec<PathBuf> = Vec::new();
    let cleanup = |runs: &[PathBuf]| {
        for r in runs {
            let _ = std::fs::remove_file(r);
        }
    };
    let mut ids: Vec<u64> = kept.iter().copied().collect();
    // What memory already holds as the messages are now goes out as it is, without reading the
    // mail again (after the first start indexed it, that is all of it).
    let mut held: HashSet<u64> = HashSet::new();
    {
        let index = cache.lock();
        for m in messages {
            if !kept.contains(&m.id.0) && index.entries.get(&m.id).is_some_and(|e| e.stamp == stamps.of(m.id)) {
                held.insert(m.id.0);
            }
        }
        if !held.is_empty() {
            let run = dir.join(format!("{generation}.run{}.idx", runs.len()));
            let mut run_ids: Vec<u64> = held.iter().copied().collect();
            run_ids.sort_unstable();
            write_held(&index, &held, &run, &run_ids)?;
            runs.push(run);
            ids.extend(run_ids);
        }
    }
    let to_read: Vec<&Arc<Message>> = messages.iter().filter(|m| !kept.contains(&m.id.0) && !held.contains(&m.id.0)).collect();
    for chunk in to_read.chunks(RUN) {
        let mut map: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        let mut run_ids = Vec::with_capacity(chunk.len());
        let mut sorted: Vec<&&Arc<Message>> = chunk.iter().collect();
        sorted.sort_by_key(|m| m.id);
        for m in sorted {
            let content = contents.get(m);
            for (word, mask) in read_words(m, content.as_deref()) {
                if let Some(p) = pack(m.id, mask) {
                    map.entry(word).or_default().push(p);
                }
            }
            run_ids.push(m.id.0);
        }
        let run = dir.join(format!("{generation}.run{}.idx", runs.len()));
        if let Err(e) = search_file::write_map(&run, &map, &run_ids) {
            cleanup(&runs);
            return Err(e);
        }
        runs.push(run);
        ids.extend(run_ids);
    }
    ids.sort_unstable();
    ids.dedup();
    let merged = merge(old.map(|(w, _)| w), Arc::new(kept), &runs, &search_file::segment_path(dir, &generation), &ids);
    cleanup(&runs);
    merged?;
    let seg = Segment::open(&search_file::segment_path(dir, &generation))?;
    // In place of the old one; what memory held of the messages it now has goes.
    let gone = {
        let mut index = cache.lock();
        let gone = index.base.take().map(|b| (b.seg.path().to_path_buf(), search_file::stale_path(&b.dir, &b.generation)));
        let base = Base { seg, generation: generation.clone(), dir: dir.to_path_buf(), valid: stamps.clone(), stale_saved: BTreeSet::new() };
        let covered: Vec<MessageId> =
            index.entries.iter().filter(|(id, e)| e.stamp == base.valid.of(**id) && base.seg.contains(id.0)).map(|(id, _)| *id).collect();
        index.remove_many(&covered);
        // Words no message here has any more take no room.
        if index.entries.is_empty() {
            index.dict = HashMap::new();
            index.postings = Vec::new();
        }
        index.base = Some(base);
        gone
    };
    if let Some((seg, stale)) = gone.filter(|(seg, _)| seg.parent() == Some(dir)) {
        // Windows keeps a file open elsewhere; then the next start removes it.
        let _ = std::fs::remove_file(seg);
        let _ = std::fs::remove_file(stale);
    }
    Ok(generation)
}

/// Write the postings memory holds of the `held` messages, word by word in order, as a run.
fn write_held(index: &Index, held: &HashSet<u64>, to: &Path, ids: &[u64]) -> ibercorreo_core::Result<()> {
    let mut words: Vec<(&str, u32)> = index.dict.iter().map(|(w, &n)| (&**w, n)).collect();
    words.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let mut w = search_file::Writer::create(to)?;
    let mut postings = Vec::new();
    for (word, n) in words {
        postings.clear();
        if let Some(list) = index.postings.get(n as usize) {
            postings.extend(list.iter().copied().filter(|&p| held.contains(&posting_id(p))));
        }
        if postings.is_empty() {
            continue;
        }
        if let Err(e) = w.word(word, &postings) {
            w.abandon();
            return Err(e);
        }
    }
    w.finish(ids)
}

/// Merge the old segment's words (only the postings of `kept` messages) and the runs into a new
/// segment at `to` covering `ids`.
fn merge(old: Option<search_file::Words>, kept: Arc<HashSet<u64>>, runs: &[PathBuf], to: &Path, ids: &[u64]) -> ibercorreo_core::Result<()> {
    let mut sources = Vec::new();
    if let Some(words) = old {
        sources.push(Source { words, head: None, keep: Some(kept) });
    }
    for run in runs {
        sources.push(Source { words: Segment::open(run)?.words()?, head: None, keep: None });
    }
    for s in &mut sources {
        s.advance()?;
    }
    let mut w = search_file::Writer::create(to)?;
    while let Some(word) = sources.iter().filter_map(|s| s.head.as_ref().map(|(w, _)| w)).min().cloned() {
        let mut postings = Vec::new();
        for s in &mut sources {
            if s.head.as_ref().is_some_and(|(w, _)| *w == word) {
                if let Some((_, p)) = s.head.take() {
                    postings.extend(p);
                }
                if let Err(e) = s.advance() {
                    w.abandon();
                    return Err(e);
                }
            }
        }
        postings.sort_unstable();
        postings.dedup_by_key(|p| posting_id(*p));
        if let Err(e) = w.word(&word, &postings) {
            w.abandon();
            return Err(e);
        }
    }
    w.finish(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(ix: &mut Index, fields: &[Field], prefix: &str) -> BTreeSet<MessageId> {
        ix.lookup(fields, prefix, &Stamps::default()).unwrap_or_default()
    }

    /// Each distinct word is kept once however many messages have it, and a message's postings
    /// say which fields the word is in, so the index stays small next to the mail.
    #[test]
    fn the_index_keeps_each_word_once() {
        let mut ix = Index::default();
        let mut a = Message::new(MessageId(1), FolderId(0));
        a.set_subject("Budget");
        a.content_mut().unwrap().body = "budget budget review".into();
        let mut b = Message::new(MessageId(2), FolderId(0));
        b.content_mut().unwrap().body = "budget".into();
        ix.add(&a, a.content(), 1);
        ix.add(&b, b.content(), 1);
        assert_eq!(ix.dict.len(), 2, "budget and review");
        assert_eq!(look(&mut ix, &[Field::Subject], "bud").into_iter().collect::<Vec<_>>(), [MessageId(1)]);
        assert_eq!(look(&mut ix, &[Field::Body], "bud").len(), 2);
        assert_eq!(look(&mut ix, &Field::ALL, "rev").into_iter().collect::<Vec<_>>(), [MessageId(1)]);
        // A message changed out of order goes back in its place, and its old words go.
        ix.remove(MessageId(1));
        assert_eq!(look(&mut ix, &Field::ALL, "rev").len(), 0);
        a.content_mut().unwrap().body = "review".into();
        ix.add(&a, a.content(), 2);
        assert_eq!(ix.postings.iter().map(Vec::len).sum::<usize>(), 3);
        assert!(ix.postings.iter().all(|l| l.is_sorted_by_key(|&p| posting_id(p))));
        // An id too big to pack (a hand-edited store) is not found by its words, and nothing breaks.
        let mut huge = Message::new(MessageId(u64::MAX), FolderId(0));
        huge.content_mut().unwrap().body = "budget".into();
        ix.add(&huge, huge.content(), 1);
        assert_eq!(look(&mut ix, &Field::ALL, "budget").len(), 2);
        ix.remove(MessageId(u64::MAX));
    }

    #[test]
    fn folding_drops_accents_and_case_but_keeps_cyrillic_letters() {
        assert_eq!(fold("Camión ÑANDÚ Straße"), "camion nandu strasse");
        assert_eq!(fold("Col·lecció"), "colleccio");
        assert_eq!(fold("Їжак Йод Ёлка"), "їжак йод елка");
    }

    #[test]
    fn words_split_and_keep_apostrophe_forms() {
        let w = words("L'aigua, п’ять — ana.garcia@example.com!");
        assert_eq!(w.parts, ["l", "aigua", "п", "ять", "ana", "garcia", "example", "com"]);
        assert_eq!(w.joined, ["laigua", "пять"]);
        assert_eq!(words("").parts.len(), 0);
        assert_eq!(words("'''  ``").parts.len(), 0);
        let long = "x".repeat(500);
        assert_eq!(words(&long).parts, [cut(&long)]);
        assert_eq!(words(&long).parts.first().map(|w| w.chars().count()), Some(MAX_WORD));
    }

    #[test]
    fn queries_parse_fields_phrases_negation_and_or() {
        let q = Query::parse(r#"from:ana subject:"weekly report" -draft OR is:unread has:attachment after:2026-10-01"#);
        assert_eq!(q.groups.len(), 2);
        let first = q.groups.first().unwrap();
        assert_eq!(first.len(), 3);
        assert_eq!(first[0].cond, Cond::Words { fields: vec![Field::From], words: vec!["ana".into()], phrase: false });
        assert_eq!(first[1].cond, Cond::Words { fields: vec![Field::Subject], words: vec!["weekly".into(), "report".into()], phrase: true });
        assert!(first[2].not);
        let second = &q.groups[1];
        assert_eq!(
            second.iter().map(|c| c.cond.clone()).collect::<Vec<_>>(),
            vec![Cond::Unread, Cond::HasAttachment, Cond::After(Date::new(2026, 10, 1).unwrap())]
        );
        // Localised field names.
        let q = Query::parse("від:Олена asunto:factura");
        assert_eq!(q.groups[0][0].cond, Cond::Words { fields: vec![Field::From], words: vec!["олена".into()], phrase: false });
        assert_eq!(q.groups[0][1].cond, Cond::Words { fields: vec![Field::Subject], words: vec!["factura".into()], phrase: false });
    }

    #[test]
    fn anything_unknown_is_searched_as_words() {
        let q = Query::parse("10:30 http://x is:later after:yesterday");
        let words: Vec<String> = q.groups[0]
            .iter()
            .flat_map(|c| match &c.cond {
                Cond::Words { words, .. } => words.clone(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(words, ["10", "30", "http", "x", "is", "later", "after", "yesterday"]);
        assert!(Query::parse("   ").is_blank());
        // Only punctuation: nothing to look for, and nothing matches.
        let q = Query::parse("!!! - \"\" OR OR");
        assert!(!q.is_blank());
        assert!(q.groups.is_empty());
    }

    #[test]
    fn hostile_queries_never_panic() {
        let nasty =
            ["\"", "\"\"\"", "-", "--", "from:", "from:\"", ":::", "on:9999-99-99", "after:0000-00-00", "OR", "a OR", "Ǆ̈́͠ 𝔘𝔫𝔦𝔠𝔬𝔡𝔢 \u{0}\u{FFFF}"];
        for q in nasty {
            let _ = Query::parse(q);
        }
        let long = "a ".repeat(10_000);
        let q = Query::parse(&long);
        assert!(q.groups.iter().map(Vec::len).sum::<usize>() <= MAX_CLAUSES);
    }

    #[test]
    fn phrases_need_the_words_together_and_in_order() {
        let p = |s: &str| words(s).parts;
        assert!(has_phrase("The Weekly Report, attached", &p("weekly report")));
        assert!(has_phrase("The Weekly Report", &p("weekly rep")));
        assert!(!has_phrase("Report weekly", &p("weekly report")));
        assert!(!has_phrase("weekly sales report", &p("weekly report")));
        assert!(!has_phrase("anything", &[]));
    }

    #[test]
    fn stamps_are_unique_and_copies_compare_equal() {
        let a = Stamps::default();
        let b = Stamps::default();
        assert_ne!(a.base, b.base);
        assert_eq!(a, b);
        let mut c = a.clone();
        c.touch(MessageId(1));
        assert_ne!(c.of(MessageId(1)), a.of(MessageId(1)));
        assert_eq!(c.of(MessageId(2)), a.of(MessageId(2)));
        c.forget(MessageId(1));
        assert_eq!(c.of(MessageId(1)), a.of(MessageId(1)));
    }
}
