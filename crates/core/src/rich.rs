//! Formatted mail, kept safe: what an HTML message says (paragraphs, headings, lists, quotes, bold,
//! links, Tinta: tables of data, text colours and sizes), and nothing it could do. `ibercorreo-mime` builds it from HTML; there is no script, no
//! style sheet, no form and no remote address in it except link targets, which are `http`,
//! `https` or `mailto` only, and the addresses of pictures on the web ([`Span::src`]), which a
//! front end loads only when the person asks. Front ends draw it with their own fonts; a sender's colours are shown only where they stay
//! readable.

use serde::{Deserialize, Serialize};

/// Most bytes of a picture's web address kept ([`Span::src`]).
pub const MAX_PICTURE_URL: usize = 2048;

/// A formatted body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rich {
    pub blocks: Vec<Block>,
    /// Images on other servers that were not loaded (each one could tell the sender the message
    /// was opened). Tracking pixels are counted too but get no placeholder.
    #[serde(default)]
    pub remote_images: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    /// How many quotation levels (`<blockquote>`) the block is inside.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub quote: u8,
    pub spans: Vec<Span>,
    /// Tinta: the cell of a table of data the block is in. Tables that only lay a message out
    /// (one column, or tables inside tables) give none: their text flows as paragraphs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<Cell>,
}

/// Tinta: where a block sits in a table: blocks with the same `table` that follow each other make
/// one table; `row` and `col` count from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub table: u16,
    pub row: u16,
    pub col: u16,
    /// A heading cell (`<th>`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub header: bool,
}

/// Smallest and largest text size kept ([`Span::size`]), in percent of normal text.
pub const SIZE_RANGE: (u16, u16) = (60, 250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BlockKind {
    Paragraph,
    /// `<h1>`…`<h6>`.
    Heading(u8),
    /// A list item; `depth` 1 is the outermost list.
    Bullet {
        depth: u8,
    },
    Numbered {
        depth: u8,
        number: u32,
    },
    /// Preformatted text: whitespace kept, a fixed-width font.
    Pre,
    /// A horizontal line (`<hr>`); it has no spans.
    Rule,
}

/// A run of text in one style.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strike: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub code: bool,
    /// An image that was not shown; `text` is its description.
    #[serde(default, skip_serializing_if = "is_false")]
    pub image: bool,
    /// For an image: the part of this same message it shows (`cid:`), as
    /// [`crate::mail::content_id_key`] makes it. A front end draws that attachment in its place
    /// when it has the contents; nothing outside the message is ever named here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    /// Tinta: for an image on the web, its address (`http:` or `https:`, at most
    /// [`MAX_PICTURE_URL`] bytes). It is never loaded unless the person asks (for this message or
    /// for its sender). Tracking pixels get no span, so they are never loaded at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    /// Where the text links to: `http:`, `https:` or `mailto:` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// The text looks like a web address on a different site than the link goes to: the usual
    /// shape of a phishing link.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deceptive: bool,
    /// Tinta: the colour the sender gave the text, `0xRRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// Tinta: the colour behind the text (a highlight), `0xRRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<u32>,
    /// Tinta: a text size clearly larger or smaller than normal, in percent of it, within
    /// [`SIZE_RANGE`]. Sizes near normal are left out, so mail does not shrink by a pixel or two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u16>,
}

impl Span {
    /// Whether two spans can be one (the same style and link).
    pub fn same_style(&self, other: &Span) -> bool {
        (self.bold, self.italic, self.underline, self.strike, self.code, self.image, &self.link, self.color, self.highlight, self.size)
            == (
                other.bold,
                other.italic,
                other.underline,
                other.strike,
                other.code,
                other.image,
                &other.link,
                other.color,
                other.highlight,
                other.size,
            )
    }
}

impl Rich {
    /// The body as plain text, without image descriptions: for replies, previews and search.
    pub fn to_text(&self) -> String {
        let item = |b: &Block| matches!(b.kind, BlockKind::Bullet { .. } | BlockKind::Numbered { .. });
        let mut out = String::new();
        let mut previous: Option<&Block> = None;
        for b in &self.blocks {
            // Images are left out: their descriptions are not the message's words.
            let words: String = b.spans.iter().filter(|s| !s.image).map(|s| s.text.as_str()).collect();
            if words.trim().is_empty() && b.kind != BlockKind::Rule {
                continue;
            }
            if let Some(p) = previous {
                // List items follow each other line by line; everything else is a paragraph apart.
                // Tinta: the cells of a row are a tab apart, the rows of a table a line.
                let gap = match (p.cell, b.cell) {
                    (Some(a), Some(c)) if a.table == c.table && a.row == c.row && a.col != c.col => "\t",
                    (Some(a), Some(c)) if a.table == c.table => "\n",
                    _ if item(p) && item(b) => "\n",
                    _ => "\n\n",
                };
                out.push_str(gap);
            }
            previous = Some(b);
            for _ in 0..b.quote {
                out.push_str("> ");
            }
            match b.kind {
                BlockKind::Bullet { depth } => {
                    out.push_str(&"  ".repeat(usize::from(depth.saturating_sub(1))));
                    out.push_str("• ");
                }
                BlockKind::Numbered { depth, number } => {
                    out.push_str(&"  ".repeat(usize::from(depth.saturating_sub(1))));
                    out.push_str(&format!("{number}. "));
                }
                BlockKind::Rule => out.push_str("———"),
                _ => {}
            }
            out.push_str(&words);
        }
        out
    }
}

/// The host part of an `http(s)` address or of a bare `www.` name, lowercased, without `www.`.
pub fn web_host(s: &str) -> Option<String> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    let rest = ["https://", "http://"].iter().find_map(|p| lower.strip_prefix(p)).or_else(|| lower.starts_with("www.").then_some(lower.as_str()))?;
    let end = rest.find(['/', '?', '#', ':']).unwrap_or(rest.len());
    let host = rest.get(..end)?;
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host).trim_end_matches('.');
    (host.contains('.') && !host.contains(char::is_whitespace)).then(|| host.to_string())
}

/// A run of a plain text: as it is, or an address the text spells out and where it leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run<'a> {
    pub text: &'a str,
    /// `https://…` (a bare `www.` name gets `https://`), `http://…` or `mailto:…`.
    pub link: Option<String>,
}

/// The web addresses (`http://`, `https://`, `www.`) and e-mail addresses in a plain text, so a
/// reader can follow them. An address starts a word, never inside a longer one, and ends before a
/// space, `<`, `>` or `"`; full stops, commas and other punctuation after it, and a bracket it did
/// not open, are left out. The runs put together are the text.
pub fn linkify(text: &str) -> Vec<Run<'_>> {
    let mut out = Vec::new();
    let mut plain = 0;
    let mut i = 0;
    let mut prev: Option<char> = None;
    while let Some(rest) = text.get(i..) {
        let Some(c) = rest.chars().next() else { break };
        let starts_word = prev.is_none_or(|p| !(p.is_alphanumeric() || "._%+-@/:&=?#".contains(p)));
        if starts_word && let Some((len, link)) = web_address(rest).or_else(|| mail_address(rest)) {
            if let Some(before) = text.get(plain..i).filter(|b| !b.is_empty()) {
                out.push(Run { text: before, link: None });
            }
            if let Some(found) = text.get(i..i + len) {
                out.push(Run { text: found, link: Some(link) });
            }
            i += len;
            plain = i;
            prev = text.get(..i).and_then(|t| t.chars().next_back());
            continue;
        }
        i += c.len_utf8();
        prev = Some(c);
    }
    if let Some(rest) = text.get(plain..).filter(|r| !r.is_empty()) {
        out.push(Run { text: rest, link: None });
    }
    out
}

/// A web address at the start of `s`: its length and where it leads.
fn web_address(s: &str) -> Option<(usize, String)> {
    let lower: String = s.chars().take(8).collect::<String>().to_ascii_lowercase();
    let www = lower.starts_with("www.");
    if !(lower.starts_with("https://") || lower.starts_with("http://") || www) {
        return None;
    }
    let end = s.find(|c: char| c.is_whitespace() || c.is_control() || "<>\"".contains(c)).unwrap_or(s.len());
    let len = trim_tail(s.get(..end)?);
    let found = s.get(..len)?;
    let host_start = if www { 0 } else { found.find("://")? + 3 };
    let host = found.get(host_start..)?.split(['/', '?', '#']).next().unwrap_or("");
    let host_ok = host.contains('.') && host.chars().next().is_some_and(char::is_alphanumeric) && !host.ends_with('.');
    // `www.` alone, or `www.example`, is not yet an address.
    let www_ok = !www || host.matches('.').count() >= 2;
    (host_ok && www_ok).then(|| (len, if www { format!("https://{found}") } else { found.to_string() }))
}

/// An e-mail address at the start of `s`: its length and its `mailto:` link.
fn mail_address(s: &str) -> Option<(usize, String)> {
    let local = s.find(|c: char| !(c.is_alphanumeric() || "._%+-".contains(c))).unwrap_or(s.len());
    if local == 0 || !s.get(local..)?.starts_with('@') {
        return None;
    }
    let domain_start = local + 1;
    let domain_len = s.get(domain_start..)?.find(|c: char| !(c.is_alphanumeric() || ".-".contains(c))).unwrap_or(s.len() - domain_start);
    let domain = s.get(domain_start..domain_start + domain_len)?.trim_end_matches(['.', '-']);
    let tld = domain.rsplit('.').next().unwrap_or("");
    let ok = domain.contains('.') && !domain.starts_with(['.', '-']) && tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic);
    let len = domain_start + domain.len();
    ok.then(|| s.get(..len).map(|a| (len, format!("mailto:{a}")))).flatten()
}

/// The length of `s` without the punctuation a sentence puts after an address, and without a
/// closing bracket the address did not open.
fn trim_tail(s: &str) -> usize {
    let mut end = s.len();
    loop {
        let Some(last) = s.get(..end).and_then(|t| t.chars().next_back()) else { return end };
        let head = s.get(..end).unwrap_or("");
        let unopened = |open: char| head.matches(open).count() < head.matches(last).count();
        let drop = ".,;:!?'\"*".contains(last) || (last == ')' && unopened('(')) || (last == ']' && unopened('[')) || (last == '}' && unopened('{'));
        if !drop {
            return end;
        }
        end -= last.len_utf8();
    }
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_of_web_addresses() {
        assert_eq!(web_host("https://www.Bank.example/login").as_deref(), Some("bank.example"));
        assert_eq!(web_host("http://user@evil.example:8080/x").as_deref(), Some("evil.example"));
        assert_eq!(web_host("www.shop.example").as_deref(), Some("shop.example"));
        assert_eq!(web_host("Click here"), None);
        assert_eq!(web_host("mailto:a@b.c"), None);
        assert_eq!(web_host("https://"), None);
    }

    /// Each address the text spells out, and only those; the runs put back together are the text.
    #[test]
    fn addresses_in_plain_text() {
        let links = |t: &str| -> Vec<(String, String)> {
            let runs = linkify(t);
            assert_eq!(runs.iter().map(|r| r.text).collect::<String>(), t, "the runs are the text");
            runs.into_iter().filter_map(|r| r.link.map(|l| (r.text.to_string(), l))).collect()
        };
        let one = |t: &str, text: &str, link: &str| assert_eq!(links(t), [(text.to_string(), link.to_string())], "{t}");
        one("See https://example.org/a?b=1&c=2.", "https://example.org/a?b=1&c=2", "https://example.org/a?b=1&c=2");
        one("(details at http://example.org/x)", "http://example.org/x", "http://example.org/x");
        one("Wiki: https://en.example.org/wiki/Foo_(bar), ok", "https://en.example.org/wiki/Foo_(bar)", "https://en.example.org/wiki/Foo_(bar)");
        one("go to www.example.org!", "www.example.org", "https://www.example.org");
        one("<https://example.org/path>", "https://example.org/path", "https://example.org/path");
        one("Write to ana.gil+news@mail.example.org, please.", "ana.gil+news@mail.example.org", "mailto:ana.gil+news@mail.example.org");
        one("Пишіть: олена@приклад.укр.", "олена@приклад.укр", "mailto:олена@приклад.укр");
        one("HTTPS://EXAMPLE.ORG", "HTTPS://EXAMPLE.ORG", "HTTPS://EXAMPLE.ORG");
        for none in [
            "nothing here",
            "xhttps://example.org",
            "notwww.example.org",
            "www.example",
            "https://",
            "https://.",
            "a@b",
            "name@example.c0m",
            "user@localhost",
            "path/www.example.org",
            "@example.org",
            "1.5 www. example",
        ] {
            assert!(links(none).is_empty(), "{none}: {:?}", links(none));
        }
        assert_eq!(links("a https://a.example b ana@b.example c").len(), 2);
        assert!(linkify("").is_empty());
    }

    #[test]
    fn plain_text_keeps_lists_and_quotes() {
        let span = |t: &str| Span { text: t.into(), ..Span::default() };
        let r = Rich {
            blocks: vec![
                Block { kind: BlockKind::Heading(1), quote: 0, spans: vec![span("Agenda")], cell: None },
                Block { kind: BlockKind::Numbered { depth: 1, number: 1 }, quote: 0, spans: vec![span("Budget")], cell: None },
                Block { kind: BlockKind::Bullet { depth: 2 }, quote: 0, spans: vec![span("Q3")], cell: None },
                Block { kind: BlockKind::Paragraph, quote: 1, spans: vec![span("Earlier")], cell: None },
                Block { kind: BlockKind::Paragraph, quote: 0, spans: vec![Span { image: true, ..span("Logo") }], cell: None },
            ],
            remote_images: 0,
        };
        assert_eq!(r.to_text(), "Agenda\n\n1. Budget\n  • Q3\n\n> Earlier");
    }

    #[test]
    fn plain_text_keeps_table_rows_and_cells() {
        let cell = |row, col, t: &str| Block {
            kind: BlockKind::Paragraph,
            quote: 0,
            spans: vec![Span { text: t.into(), ..Span::default() }],
            cell: Some(Cell { table: 0, row, col, header: row == 0 }),
        };
        let r = Rich { blocks: vec![cell(0, 0, "Item"), cell(0, 1, "Price"), cell(1, 0, "Tea"), cell(1, 1, "3 €")], remote_images: 0 };
        assert_eq!(r.to_text(), "Item\tPrice\nTea\t3 €");
    }
}
