//! HTML mail made safe: [`to_rich`] reads the HTML of a message into [`Rich`], the formatted
//! text the interface draws. It is a reader, not a browser: scripts, styles, forms, frames, SVG
//! and hidden text (preheaders, `display:none`) are dropped, links keep only `http`, `https` and
//! `mailto` targets, and images are never loaded, only counted (remote) or named (inline).
//!
//! Mail HTML is often malformed, so the tokenizer is forgiving: unknown tags are ignored, unclosed
//! elements close at the end, stray `</x>` do nothing, and `<li>`, `<p>`, `<td>` and `<tr>` close
//! their open predecessors as browsers do. Everything is bounded: the input is cut at
//! [`crate::MAX_BODY_BYTES`], the element stack at [`MAX_DEPTH`], and the output at [`MAX_BLOCKS`].

use ibercorreo_core::clamp_text;
use ibercorreo_core::rich::{Block, BlockKind, Rich, Span, web_host};

/// Deepest element nesting tracked; deeper tags still give their text, without their style.
pub const MAX_DEPTH: usize = 256;
/// Most blocks kept; the rest of a longer message is cut.
pub const MAX_BLOCKS: usize = 20_000;
const MAX_SPANS: usize = 4_096;
const MAX_LINK: usize = 2_048;
const MAX_ALT: usize = 300;
const MAX_ATTRS: usize = 64;
const MAX_LEVEL: u8 = 8;

/// The safe, formatted content of an HTML document.
pub fn to_rich(html: &str) -> Rich {
    let html = clamp_text(html, crate::MAX_BODY_BYTES);
    let mut b = Builder::default();
    let mut i = 0;
    while i < html.len() && !b.full {
        let Some(rest) = html.get(i..) else { break };
        let Some(lt) = rest.find('<') else {
            b.text(rest);
            break;
        };
        b.text(rest.get(..lt).unwrap_or(""));
        i += lt;
        i = b.tag(html, i);
    }
    b.flush();
    b.out
}

/// One tag's name and the attributes the reader uses.
#[derive(Default)]
struct Tag {
    name: String,
    end: bool,
    href: Option<String>,
    src: Option<String>,
    alt: Option<String>,
    style: Option<String>,
    start: Option<String>,
    value: Option<String>,
    width: Option<String>,
    height: Option<String>,
    hidden: bool,
}

struct Open {
    name: String,
    hidden: bool,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    link: Option<String>,
    /// The next number, for an `<ol>`.
    next: u32,
}

#[derive(Default)]
struct Builder {
    out: Rich,
    spans: Vec<Span>,
    kind: Option<BlockKind>,
    quote: u8,
    space: bool,
    text_len: usize,
    stack: Vec<Open>,
    full: bool,
}

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];
/// Elements whose content is not markup and is never shown: skipped up to their end tag.
const RAW: &[&str] = &["script", "style", "title", "textarea", "xmp", "noembed", "noframes", "noscript", "iframe", "plaintext"];
/// Elements whose content is markup that is never shown.
const UNSHOWN: &[&str] = &["head", "svg", "math", "template", "object", "select", "audio", "video", "canvas", "map", "datalist"];
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "caption",
    "center",
    "dd",
    "details",
    "dialog",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "html",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];

impl Builder {
    fn hidden(&self) -> bool {
        self.stack.iter().any(|o| o.hidden)
    }

    fn in_pre(&self) -> bool {
        self.stack.iter().any(|o| o.name == "pre" || o.name == "listing")
    }

    fn style(&self) -> Span {
        let mut s = Span::default();
        for o in &self.stack {
            s.bold |= o.bold;
            s.italic |= o.italic;
            s.underline |= o.underline;
            s.strike |= o.strike;
            s.code |= matches!(o.name.as_str(), "code" | "tt" | "kbd" | "samp");
            if o.name == "a" {
                // The innermost link wins; a nested `<a>` without a safe target unlinks its text.
                s.link.clone_from(&o.link);
            }
        }
        s
    }

    fn levels(&self, name: &str) -> u8 {
        let n = self.stack.iter().filter(|o| o.name == name).count();
        u8::try_from(n).unwrap_or(u8::MAX).min(MAX_LEVEL)
    }

    /// Text between tags: entities decoded, whitespace collapsed (outside `<pre>`).
    fn text(&mut self, raw: &str) {
        if raw.is_empty() || self.hidden() || self.full {
            return;
        }
        let decoded = decode_entities(raw);
        let pre = self.in_pre();
        let mut piece = String::with_capacity(decoded.len());
        for c in decoded.chars() {
            if is_invisible(c) {
                continue;
            }
            if pre {
                if c != '\r' {
                    piece.push(c);
                }
            } else if c.is_whitespace() {
                self.space = true;
            } else {
                if self.space && (!piece.is_empty() || (self.has_text() && !self.at_line_start())) {
                    piece.push(' ');
                }
                self.space = false;
                piece.push(c);
            }
        }
        if !piece.is_empty() {
            let style = self.style();
            self.push(piece, style);
        }
    }

    /// Whether the block so far ends with a line break (`<br>`), where no space is wanted.
    fn at_line_start(&self) -> bool {
        self.spans.last().is_some_and(|s| s.text.ends_with('\n'))
    }

    fn has_text(&self) -> bool {
        self.spans.iter().any(|s| !s.text.is_empty())
    }

    fn push(&mut self, text: String, style: Span) {
        if self.text_len.saturating_add(text.len()) > crate::MAX_BODY_BYTES {
            self.full = true;
            return;
        }
        if self.spans.is_empty() {
            self.quote = self.levels("blockquote");
        }
        self.text_len += text.len();
        let mut text = text;
        // A space between differently styled runs belongs to the first, so a link or bold word
        // does not start with one.
        if let Some(last) = self.spans.last_mut()
            && !last.same_style(&style)
            && let Some(rest) = text.strip_prefix(' ')
        {
            if last.link.is_some() {
                // Not inside the link either: a run of its own.
                self.spans.push(Span { text: " ".into(), link: None, image: false, ..style.clone() });
            } else {
                last.text.push(' ');
            }
            text = rest.to_string();
            if text.is_empty() {
                return;
            }
        }
        match self.spans.last_mut() {
            Some(last) if last.same_style(&style) && !style.image => last.text.push_str(&text),
            _ => {
                if self.spans.len() >= MAX_SPANS {
                    self.flush();
                    self.quote = self.levels("blockquote");
                }
                self.spans.push(Span { text, ..style });
            }
        }
    }

    /// End the current block. Returns whether it had anything in it.
    fn flush(&mut self) -> bool {
        let pre = matches!(self.kind, Some(BlockKind::Pre));
        let mut spans = std::mem::take(&mut self.spans);
        self.space = false;
        if pre {
            if let Some(first) = spans.first_mut() {
                first.text = first.text.strip_prefix('\n').unwrap_or(&first.text).to_string();
            }
        } else if let Some(first) = spans.first_mut() {
            first.text = first.text.trim_start().to_string();
        }
        while let Some(last) = spans.last_mut() {
            let trimmed = last.text.trim_end().len();
            last.text.truncate(trimmed);
            if last.text.is_empty() && !last.image {
                spans.pop();
            } else {
                break;
            }
        }
        spans.retain(|s| !s.text.is_empty() || s.image);
        if spans.is_empty() {
            return false;
        }
        for s in &mut spans {
            s.deceptive = deceptive(s);
        }
        let kind = self.kind.take().unwrap_or(if self.in_pre() { BlockKind::Pre } else { BlockKind::Paragraph });
        self.add(Block { kind, quote: self.quote, spans });
        if self.in_pre() {
            self.kind = Some(BlockKind::Pre);
        }
        true
    }

    fn add(&mut self, block: Block) {
        if self.out.blocks.len() >= MAX_BLOCKS {
            self.full = true;
            return;
        }
        self.out.blocks.push(block);
    }

    /// Start a block of `kind`, after ending the current one.
    fn begin(&mut self, kind: BlockKind) {
        self.flush();
        self.kind = Some(kind);
    }

    /// Read the tag at `at` (a `<`); returns where reading continues.
    fn tag(&mut self, html: &str, at: usize) -> usize {
        let rest = html.get(at + 1..).unwrap_or("");
        if let Some(comment) = rest.strip_prefix("!--") {
            return match comment.find("-->") {
                Some(end) => at + 4 + end + 3,
                None => html.len(),
            };
        }
        let first = rest.chars().next();
        if matches!(first, Some('!' | '?')) {
            return rest.find('>').map_or(html.len(), |e| at + 1 + e + 1);
        }
        let end = first == Some('/');
        let name_from = if end { 1 } else { 0 };
        if !rest.get(name_from..).and_then(|r| r.chars().next()).is_some_and(|c| c.is_ascii_alphabetic()) {
            if end {
                // `</ 3>` and the like: skipped to the next `>`.
                return rest.find('>').map_or(html.len(), |e| at + 1 + e + 1);
            }
            self.text("<");
            return at + 1;
        }
        let (tag, used) = parse_tag(rest.get(name_from..).unwrap_or(""));
        let next = at + 1 + name_from + used;
        let tag = Tag { end, ..tag };
        if tag.end {
            self.close(&tag.name);
            next
        } else if RAW.contains(&tag.name.as_str()) {
            skip_raw(html, next, &tag.name)
        } else {
            self.open(tag);
            next
        }
    }

    fn open(&mut self, tag: Tag) {
        let name = tag.name.as_str();
        // Elements that close an open one of their kind, as browsers do.
        let closes: &[&str] = match name {
            "li" => &["li"],
            "p" => &["p"],
            "td" | "th" => &["td", "th"],
            "tr" => &["tr", "td", "th"],
            "dt" | "dd" => &["dt", "dd"],
            _ => &[],
        };
        if !closes.is_empty() {
            let bounds: &[&str] = match name {
                "li" => &["ul", "ol"],
                "td" | "th" | "tr" => &["table"],
                _ => &["table", "ul", "ol", "blockquote", "div"],
            };
            if let Some(i) = self.stack.iter().rposition(|o| closes.contains(&o.name.as_str()) || bounds.contains(&o.name.as_str()))
                && self.stack.get(i).is_some_and(|o| closes.contains(&o.name.as_str()))
            {
                self.stack.truncate(i);
            }
        }
        if name == "body" {
            self.stack.retain(|o| o.name != "head");
        }
        let css = tag.style.as_deref().map(Css::parse).unwrap_or_default();
        let hidden = tag.hidden || css.hidden || UNSHOWN.contains(&name);
        if BLOCKS.contains(&name) && !self.hidden() {
            match name {
                "li" => {
                    let depth = self.levels("ul").saturating_add(self.levels("ol")).clamp(1, MAX_LEVEL);
                    let value = tag.value.as_deref().and_then(number);
                    let list = self.stack.iter_mut().rev().find(|o| o.name == "ul" || o.name == "ol");
                    let kind = match list {
                        Some(o) if o.name == "ol" => {
                            let n = value.unwrap_or(o.next);
                            o.next = n.saturating_add(1);
                            BlockKind::Numbered { depth, number: n }
                        }
                        _ => BlockKind::Bullet { depth },
                    };
                    self.begin(kind);
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level = name.get(1..).and_then(|n| n.parse().ok()).unwrap_or(1);
                    self.begin(BlockKind::Heading(level));
                }
                "pre" => self.begin(BlockKind::Pre),
                _ => {
                    if self.flush() {
                        self.kind = None;
                    }
                }
            }
        }
        match name {
            "br" if !self.hidden() => {
                let style = self.style();
                if self.in_pre() || self.has_text() {
                    self.space = false;
                    self.push("\n".into(), Span { link: None, ..style });
                }
            }
            "hr" if !self.hidden() => {
                self.flush();
                let quote = self.levels("blockquote");
                self.add(Block { kind: BlockKind::Rule, quote, spans: Vec::new() });
            }
            "img" if !self.hidden() && !hidden => self.image(&tag),
            _ => {}
        }
        if VOID.contains(&name) || self.stack.len() >= MAX_DEPTH {
            return;
        }
        let mut o = Open {
            name: tag.name.clone(),
            hidden,
            bold: css.bold || matches!(name, "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th" | "dt"),
            italic: css.italic || matches!(name, "i" | "em" | "cite" | "var" | "dfn" | "address"),
            underline: css.underline || matches!(name, "u" | "ins"),
            strike: css.strike || matches!(name, "s" | "strike" | "del"),
            link: None,
            next: 1,
        };
        if name == "a" {
            o.link = tag.href.as_deref().and_then(safe_link);
        }
        if name == "ol" {
            o.next = tag.start.as_deref().and_then(number).unwrap_or(1);
        }
        self.stack.push(o);
    }

    fn close(&mut self, name: &str) {
        let Some(i) = self.stack.iter().rposition(|o| o.name == name) else {
            // A stray `</p>` still ends the paragraph.
            if name == "p" {
                self.flush();
            }
            return;
        };
        let block = self.stack.get(i..).is_some_and(|open| open.iter().any(|o| BLOCKS.contains(&o.name.as_str())));
        if block && !self.hidden() {
            self.flush();
            self.kind = None;
        }
        self.stack.truncate(i);
    }

    fn image(&mut self, tag: &Tag) {
        let src = tag.src.as_deref().unwrap_or("").trim().to_ascii_lowercase();
        let remote = src.starts_with("http:") || src.starts_with("https:") || src.starts_with("//");
        let tiny = |v: &Option<String>| v.as_deref().and_then(number).is_some_and(|n| n <= 2);
        let tracker = tiny(&tag.width) || tiny(&tag.height);
        if remote {
            self.out.remote_images = self.out.remote_images.saturating_add(1);
        }
        if tracker {
            return;
        }
        let alt = decode_entities(tag.alt.as_deref().unwrap_or(""));
        let alt: String = alt.split_whitespace().collect::<Vec<_>>().join(" ");
        let alt = clamp_text(&alt, MAX_ALT);
        if alt.is_empty() && !remote && !src.starts_with("cid:") {
            return;
        }
        let text = if alt.is_empty() { "image".to_string() } else { alt.to_string() };
        if self.space && self.has_text() {
            let style = self.style();
            self.push(" ".into(), style);
        }
        self.space = false;
        let style = self.style();
        self.push(text, Span { image: true, ..style });
    }
}

/// Parse a start or end tag after `<` (and `/`): its name and attributes, and the bytes used up
/// to and including `>`.
fn parse_tag(s: &str) -> (Tag, usize) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while bytes.get(i).is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'_')) {
        i += 1;
    }
    let mut tag = Tag { name: s.get(..i.min(32)).unwrap_or("").to_ascii_lowercase(), ..Tag::default() };
    let mut attrs = 0;
    loop {
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace() || *b == b'/') {
            i += 1;
        }
        match bytes.get(i) {
            None => return (tag, i),
            Some(b'>') => return (tag, i + 1),
            Some(_) => {}
        }
        let name_start = i;
        while bytes.get(i).is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'=' | b'>' | b'/')) {
            i += 1;
        }
        // An attribute name starting with `=` or `/` moves on by one byte at least.
        if i == name_start {
            i += 1;
            continue;
        }
        let name = s.get(name_start..i).unwrap_or("").to_ascii_lowercase();
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let mut value = None;
        if bytes.get(i) == Some(&b'=') {
            i += 1;
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            match bytes.get(i) {
                Some(&q @ (b'"' | b'\'')) => {
                    let from = i + 1;
                    let len = s.get(from..).and_then(|r| r.find(char::from(q))).unwrap_or(s.len().saturating_sub(from));
                    value = s.get(from..from + len).map(str::to_string);
                    i = from + len + 1;
                }
                _ => {
                    let from = i;
                    while bytes.get(i).is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>') {
                        i += 1;
                    }
                    value = s.get(from..i).map(str::to_string);
                }
            }
        }
        attrs += 1;
        if attrs > MAX_ATTRS {
            continue;
        }
        let slot = match name.as_str() {
            "href" => &mut tag.href,
            "src" => &mut tag.src,
            "alt" => &mut tag.alt,
            "style" => &mut tag.style,
            "start" => &mut tag.start,
            "value" => &mut tag.value,
            "width" => &mut tag.width,
            "height" => &mut tag.height,
            "hidden" => {
                tag.hidden = true;
                continue;
            }
            _ => continue,
        };
        if slot.is_none() {
            *slot = Some(value.unwrap_or_default());
        }
    }
}

/// Continue after the end tag of a raw-text element (`</script>`), or at the end.
fn skip_raw(html: &str, from: usize, name: &str) -> usize {
    let mut i = from;
    while let Some(rest) = html.get(i..) {
        let Some(pos) = rest.find("</") else { break };
        let after = i + pos + 2;
        let candidate = html.as_bytes().get(after..after + name.len());
        if candidate.is_some_and(|c| c.eq_ignore_ascii_case(name.as_bytes())) {
            return html.get(after..).and_then(|r| r.find('>')).map_or(html.len(), |e| after + e + 1);
        }
        i = after;
    }
    html.len()
}

/// The parts of an inline `style` that change what a reader sees.
#[derive(Default)]
struct Css {
    hidden: bool,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

impl Css {
    fn parse(style: &str) -> Css {
        let mut css = Css::default();
        for decl in style.split(';').take(256) {
            let Some((prop, value)) = decl.split_once(':') else { continue };
            let prop = prop.trim().to_ascii_lowercase();
            let value = value.trim().trim_end_matches("!important").trim().to_ascii_lowercase();
            let zero = value.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%').parse::<f32>().is_ok_and(|n| n <= 0.0);
            match prop.as_str() {
                "display" => css.hidden |= value == "none",
                "visibility" => css.hidden |= value == "hidden" || value == "collapse",
                "font-size" | "max-height" | "max-width" | "opacity" => css.hidden |= zero,
                "font-weight" => css.bold |= value == "bold" || value == "bolder" || value.parse::<u32>().is_ok_and(|n| n >= 600),
                "font-style" => css.italic |= value == "italic" || value == "oblique",
                "text-decoration" | "text-decoration-line" => {
                    css.underline |= value.contains("underline");
                    css.strike |= value.contains("line-through");
                }
                _ => {}
            }
        }
        css
    }
}

/// A link target that is safe to offer: `http:`, `https:` or `mailto:`, without control
/// characters, and not too long.
fn safe_link(href: &str) -> Option<String> {
    let decoded = decode_entities(href);
    // Browsers drop tabs and line breaks inside addresses; `java\nscript:` must not get through as text.
    let cleaned: String = decoded.trim().chars().filter(|c| !c.is_control()).collect();
    if cleaned.len() > MAX_LINK || (cleaned.contains(char::is_whitespace) && !cleaned.to_ascii_lowercase().starts_with("mailto:")) {
        return None;
    }
    let lower = cleaned.to_ascii_lowercase();
    let ok = lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:");
    ok.then_some(cleaned)
}

/// Whether a span's text shows one site while its link goes to another.
fn deceptive(s: &Span) -> bool {
    let (Some(link), Some(shown)) = (s.link.as_deref(), web_host(&s.text)) else { return false };
    match web_host(link) {
        Some(target) => target != shown && !target.ends_with(&format!(".{shown}")),
        // Text that looks like a web address on a `mailto:` link.
        None => true,
    }
}

/// The leading decimal number of an attribute (`"600px"` is 600).
fn number(v: &str) -> Option<u32> {
    let v = v.trim();
    let end = v.find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len());
    v.get(..end)?.parse().ok()
}

/// Characters that take no space: zero-width joiners and spaces, the soft hyphen, the byte-order
/// mark. Newsletters pad their hidden preview text with them.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{00AD}' | '\u{034F}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}')
}

/// Replace character references (`&amp;`, `&#233;`, `&#x1F600;`) with their characters. Unknown
/// names stay as written; invalid numbers become U+FFFD.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(rest.get(..amp).unwrap_or(""));
        let after = rest.get(amp + 1..).unwrap_or("");
        let (decoded, used) = entity(after);
        match decoded {
            Some(c) => {
                out.push(c);
                rest = after.get(used..).unwrap_or("");
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The character a reference after `&` names, and how many bytes it used (with `;`).
fn entity(s: &str) -> (Option<char>, usize) {
    if let Some(num) = s.strip_prefix('#') {
        let (hex, digits) = match num.strip_prefix(['x', 'X']) {
            Some(h) => (true, h),
            None => (false, num),
        };
        let len = digits.find(|c: char| if hex { !c.is_ascii_hexdigit() } else { !c.is_ascii_digit() }).unwrap_or(digits.len());
        if len == 0 {
            return (None, 0);
        }
        let text = digits.get(..len.min(8)).unwrap_or("");
        let value = u32::from_str_radix(text, if hex { 16 } else { 10 }).ok();
        let c = match value {
            _ if len > 8 => '\u{FFFD}',
            Some(0) | None => '\u{FFFD}',
            // Windows-1252 bytes written as numbers, as browsers read them.
            Some(v @ 0x80..=0x9F) => cp1252(v),
            Some(v) => char::from_u32(v).unwrap_or('\u{FFFD}'),
        };
        let mut used = 1 + usize::from(hex) + len;
        if digits.get(len..).is_some_and(|r| r.starts_with(';')) {
            used += 1;
        }
        return (Some(c), used);
    }
    let len = s.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(s.len());
    let Some(name) = s.get(..len.min(32)) else { return (None, 0) };
    let semicolon = s.get(len..).is_some_and(|r| r.starts_with(';'));
    match named(name) {
        Some(c) if semicolon => (Some(c), len + 1),
        // Old mail writes `&nbsp` and `&amp` without the semicolon.
        Some(c) if matches!(name, "amp" | "lt" | "gt" | "quot" | "nbsp" | "copy" | "reg") => (Some(c), len),
        _ => (None, 0),
    }
}

fn cp1252(v: u32) -> char {
    match v {
        0x80 => '€',
        0x82 => '‚',
        0x83 => 'ƒ',
        0x84 => '„',
        0x85 => '…',
        0x86 => '†',
        0x87 => '‡',
        0x88 => 'ˆ',
        0x89 => '‰',
        0x8A => 'Š',
        0x8B => '‹',
        0x8C => 'Œ',
        0x8E => 'Ž',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x98 => '˜',
        0x99 => '™',
        0x9A => 'š',
        0x9B => '›',
        0x9C => 'œ',
        0x9E => 'ž',
        0x9F => 'Ÿ',
        _ => '\u{FFFD}',
    }
}

/// Named character references used in mail. The full HTML list has 2,231; these cover what mail
/// software writes, and the Latin-1 letters by rule.
fn named(name: &str) -> Option<char> {
    let c = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{A0}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "thinsp" => '\u{2009}',
        "zwnj" => '\u{200C}',
        "zwj" => '\u{200D}',
        "lrm" => '\u{200E}',
        "rlm" => '\u{200F}',
        "shy" => '\u{AD}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "minus" => '−',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "sbquo" => '‚',
        "ldquo" => '\u{201C}',
        "rdquo" => '\u{201D}',
        "bdquo" => '„',
        "laquo" => '«',
        "raquo" => '»',
        "lsaquo" => '‹',
        "rsaquo" => '›',
        "bull" => '•',
        "middot" => '·',
        "dagger" => '†',
        "Dagger" => '‡',
        "permil" => '‰',
        "prime" => '′',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "curren" => '¤',
        "deg" => '°',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "sect" => '§',
        "para" => '¶',
        "micro" => 'µ',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "sup1" => '¹',
        "sup2" => '²',
        "sup3" => '³',
        "ordf" => 'ª',
        "ordm" => 'º',
        "iexcl" => '¡',
        "iquest" => '¿',
        "brvbar" => '¦',
        "not" => '¬',
        "macr" => '¯',
        "acute" => '´',
        "cedil" => '¸',
        "uml" => '¨',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "harr" => '↔',
        "check" => '✓',
        "hearts" => '♥',
        "star" => '☆',
        "szlig" => 'ß',
        "AElig" => 'Æ',
        "aelig" => 'æ',
        "OElig" => 'Œ',
        "oelig" => 'œ',
        "ETH" => 'Ð',
        "eth" => 'ð',
        "THORN" => 'Þ',
        "thorn" => 'þ',
        "Oslash" => 'Ø',
        "oslash" => 'ø',
        "Scaron" => 'Š',
        "scaron" => 'š',
        "Zcaron" => 'Ž',
        "zcaron" => 'ž',
        "Yuml" => 'Ÿ',
        _ => return latin_letter(name),
    };
    Some(c)
}

/// `&eacute;`, `&Ccedil;`, `&atilde;` and the rest of the accented Latin-1 letters.
fn latin_letter(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let letter = chars.next()?;
    let mark = chars.as_str();
    let table: &[(char, &str)] = match mark {
        "acute" => &[
            ('a', "á"),
            ('e', "é"),
            ('i', "í"),
            ('o', "ó"),
            ('u', "ú"),
            ('y', "ý"),
            ('A', "Á"),
            ('E', "É"),
            ('I', "Í"),
            ('O', "Ó"),
            ('U', "Ú"),
            ('Y', "Ý"),
        ],
        "grave" => &[('a', "à"), ('e', "è"), ('i', "ì"), ('o', "ò"), ('u', "ù"), ('A', "À"), ('E', "È"), ('I', "Ì"), ('O', "Ò"), ('U', "Ù")],
        "circ" => &[('a', "â"), ('e', "ê"), ('i', "î"), ('o', "ô"), ('u', "û"), ('A', "Â"), ('E', "Ê"), ('I', "Î"), ('O', "Ô"), ('U', "Û")],
        "uml" => {
            &[('a', "ä"), ('e', "ë"), ('i', "ï"), ('o', "ö"), ('u', "ü"), ('y', "ÿ"), ('A', "Ä"), ('E', "Ë"), ('I', "Ï"), ('O', "Ö"), ('U', "Ü")]
        }
        "tilde" => &[('a', "ã"), ('n', "ñ"), ('o', "õ"), ('A', "Ã"), ('N', "Ñ"), ('O', "Õ")],
        "cedil" => &[('c', "ç"), ('C', "Ç")],
        "ring" => &[('a', "å"), ('A', "Å")],
        _ => return None,
    };
    table.iter().find(|(l, _)| *l == letter).and_then(|(_, s)| s.chars().next())
}

#[cfg(test)]
mod tests;
