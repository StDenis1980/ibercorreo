//! Formatted mail, kept safe: what an HTML message says (paragraphs, headings, lists, quotes, bold,
//! links), and nothing it could do. `ibercorreo-mime` builds it from HTML; there is no script, no
//! style sheet, no form and no remote address in it except link targets, which are `http`,
//! `https` or `mailto` only. Front ends draw it with their own fonts and colours.

use serde::{Deserialize, Serialize};

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
}

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
    /// Where the text links to: `http:`, `https:` or `mailto:` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// The text looks like a web address on a different site than the link goes to: the usual
    /// shape of a phishing link.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deceptive: bool,
}

impl Span {
    /// Whether two spans can be one (the same style and link).
    pub fn same_style(&self, other: &Span) -> bool {
        (self.bold, self.italic, self.underline, self.strike, self.code, self.image, &self.link)
            == (other.bold, other.italic, other.underline, other.strike, other.code, other.image, &other.link)
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
                out.push_str(if item(p) && item(b) { "\n" } else { "\n\n" });
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

    #[test]
    fn plain_text_keeps_lists_and_quotes() {
        let span = |t: &str| Span { text: t.into(), ..Span::default() };
        let r = Rich {
            blocks: vec![
                Block { kind: BlockKind::Heading(1), quote: 0, spans: vec![span("Agenda")] },
                Block { kind: BlockKind::Numbered { depth: 1, number: 1 }, quote: 0, spans: vec![span("Budget")] },
                Block { kind: BlockKind::Bullet { depth: 2 }, quote: 0, spans: vec![span("Q3")] },
                Block { kind: BlockKind::Paragraph, quote: 1, spans: vec![span("Earlier")] },
                Block { kind: BlockKind::Paragraph, quote: 0, spans: vec![Span { image: true, ..span("Logo") }] },
            ],
            remote_images: 0,
        };
        assert_eq!(r.to_text(), "Agenda\n\n1. Budget\n  • Q3\n\n> Earlier");
    }
}
