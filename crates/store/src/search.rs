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
//! - Dates (`YYYY-MM-DD`, UTC): `after:` (that day or later), `before:` (earlier than that day),
//!   `on:` (that day).
//!
//! Anything that is not one of these is searched for as words, so `10:30` or `http://x` never
//! fail. The index lives beside the store in memory and is brought up to date at each search,
//! re-reading only the messages that changed since the last one ([`Stamps`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ibercorreo_core::{AccountId, Date, FolderId, Message, MessageId};

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
    fn texts(self, m: &Message) -> Vec<&str> {
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
            Field::Body => vec![m.body.as_str()],
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
}

// ---- the index ----

/// One message's indexed words, sorted, and the stamp they were read at.
#[derive(Debug, Default)]
struct Entry {
    stamp: u64,
    keys: Vec<(Field, String)>,
}

impl Entry {
    /// `m`'s words, read now.
    fn read(m: &Message, stamp: u64) -> Entry {
        let mut keys = BTreeSet::new();
        for field in Field::ALL {
            let mut n = 0usize;
            for text in field.texts(m) {
                let w = words(text);
                for word in w.parts.into_iter().chain(w.joined) {
                    if n >= MAX_WORDS_PER_FIELD {
                        break;
                    }
                    if keys.insert((field, word)) {
                        n = n.saturating_add(1);
                    }
                }
            }
        }
        Entry { stamp, keys: keys.into_iter().collect() }
    }

    /// Whether a word in `field` starts with `prefix`.
    fn has_prefix(&self, field: Field, prefix: &str) -> bool {
        let at = self.keys.partition_point(|(f, w)| (*f, w.as_str()) < (field, prefix));
        self.keys.get(at).is_some_and(|(f, w)| *f == field && w.starts_with(prefix))
    }
}

/// Words to messages, and each message's words.
#[derive(Debug, Default)]
pub struct Index {
    entries: HashMap<MessageId, Entry>,
    postings: BTreeMap<(Field, String), BTreeSet<MessageId>>,
}

impl Index {
    /// Read the messages that changed since the last call; drop the ones that are gone.
    fn refresh(&mut self, messages: &[Message], stamps: &Stamps) {
        for m in messages {
            let stamp = stamps.of(m.id);
            if self.entries.get(&m.id).is_some_and(|e| e.stamp == stamp) {
                continue;
            }
            self.remove(m.id);
            self.add(m, stamp);
        }
        // Ids are unique, so as many entries as messages means none is stale.
        if self.entries.len() != messages.len() {
            let live: std::collections::HashSet<MessageId> = messages.iter().map(|m| m.id).collect();
            let gone: Vec<MessageId> = self.entries.keys().filter(|id| !live.contains(id)).copied().collect();
            for id in gone {
                self.remove(id);
            }
        }
    }

    fn add(&mut self, m: &Message, stamp: u64) {
        let entry = Entry::read(m, stamp);
        for key in &entry.keys {
            self.postings.entry(key.clone()).or_default().insert(m.id);
        }
        self.entries.insert(m.id, entry);
    }

    fn remove(&mut self, id: MessageId) {
        let Some(entry) = self.entries.remove(&id) else { return };
        for key in entry.keys {
            if let Some(set) = self.postings.get_mut(&key) {
                set.remove(&id);
                if set.is_empty() {
                    self.postings.remove(&key);
                }
            }
        }
    }

    /// Messages with a word in one of `fields` that starts with `prefix`.
    fn lookup(&self, fields: &[Field], prefix: &str) -> BTreeSet<MessageId> {
        let mut out = BTreeSet::new();
        for &field in fields {
            for ((f, w), ids) in self.postings.range((field, prefix.to_string())..) {
                if *f != field || !w.starts_with(prefix) {
                    break;
                }
                out.extend(ids.iter().copied());
            }
        }
        out
    }

    /// How many messages are indexed.
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
                    if !key.is_empty() && (Field::named(&key).is_some() || matches!(key.as_str(), "has" | "is" | "after" | "before" | "on")) {
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

    /// Whether `m` matches, read on its own (no index): for rules, which look at one message at
    /// a time as it arrives.
    pub fn matches(&self, m: &Message) -> bool {
        if self.blank {
            return true;
        }
        let entry = Entry::read(m, 0);
        self.groups.iter().any(|g| g.iter().all(|c| matches(&c.cond, m, Some(&entry)) != c.not))
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

fn matches(cond: &Cond, m: &Message, entry: Option<&Entry>) -> bool {
    match cond {
        Cond::Words { fields, words, phrase } => {
            let Some(entry) = entry else { return false };
            let all = words.iter().all(|w| fields.iter().any(|f| entry.has_prefix(*f, w)));
            if !all {
                return false;
            }
            if *phrase && words.len() > 1 { fields.iter().any(|f| f.texts(m).iter().any(|t| has_phrase(t, words))) } else { true }
        }
        Cond::HasAttachment => !m.attachments.is_empty(),
        Cond::Unread => !m.seen,
        Cond::Read => m.seen,
        Cond::Flagged => m.flagged,
        Cond::After(d) => m.date.is_some_and(|t| t.date >= *d),
        Cond::Before(d) => m.date.is_some_and(|t| t.date < *d),
        Cond::On(d) => m.date.is_some_and(|t| t.date == *d),
    }
}

/// The messages of `messages` in `allowed` folders that match `query`, in no particular order.
pub(crate) fn run<'a>(
    cache: &SearchCache,
    stamps: &Stamps,
    messages: &'a [Message],
    allowed: &dyn Fn(FolderId) -> bool,
    query: &Query,
) -> Vec<&'a Message> {
    let in_scope = messages.iter().filter(|m| allowed(m.folder));
    if query.blank {
        return in_scope.collect();
    }
    if query.groups.is_empty() {
        return Vec::new();
    }
    // A poisoned lock means a search panicked half way: start the index again.
    let mut index = cache.0.lock().unwrap_or_else(|poisoned| {
        let mut guard = poisoned.into_inner();
        *guard = Index::default();
        guard
    });
    cache.0.clear_poison();
    index.refresh(messages, stamps);
    let mut hits: BTreeSet<MessageId> = BTreeSet::new();
    for group in &query.groups {
        // Narrow down with the index: every positive word must be somewhere.
        let mut candidates: Option<BTreeSet<MessageId>> = None;
        for clause in group.iter().filter(|c| !c.not) {
            if let Cond::Words { fields, words, .. } = &clause.cond {
                for w in words {
                    let found = index.lookup(fields, w);
                    candidates = Some(match candidates {
                        None => found,
                        Some(c) => c.intersection(&found).copied().collect(),
                    });
                }
            }
        }
        for m in in_scope.clone() {
            if hits.contains(&m.id) || candidates.as_ref().is_some_and(|c| !c.contains(&m.id)) {
                continue;
            }
            let entry = index.entries.get(&m.id);
            if group.iter().all(|c| matches(&c.cond, m, entry) != c.not) {
                hits.insert(m.id);
            }
        }
    }
    drop(index);
    messages.iter().filter(|m| hits.contains(&m.id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
