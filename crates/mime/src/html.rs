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
use ibercorreo_core::rich::{Block, BlockKind, Cell, Rich, SIZE_RANGE, Span, web_host};

/// Deepest element nesting tracked; deeper tags still give their text, without their style.
pub const MAX_DEPTH: usize = 256;
/// Most blocks kept; the rest of a longer message is cut.
pub const MAX_BLOCKS: usize = 20_000;
const MAX_SPANS: usize = 4_096;
const MAX_LINK: usize = 2_048;
const MAX_ALT: usize = 300;
const MAX_ATTRS: usize = 64;
const MAX_LEVEL: u8 = 8;
/// Tinta: most rows and columns of a table shown as a table; a bigger one flows as paragraphs.
const MAX_TABLE_ROWS: u16 = 1_000;
const MAX_TABLE_COLS: u16 = 24;

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
    while let Some(t) = b.tables.pop() {
        b.finish_table(&t);
    }
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
    /// `<font color>`, `<font size>`, `<font face>`.
    color: Option<String>,
    size: Option<String>,
    face: Option<String>,
    hidden: bool,
}

struct Open {
    name: String,
    hidden: bool,
    /// Text here is `font-size: 0`. Unlike `hidden`, a descendant can set a size again (layout
    /// templates zero the size on a cell to drop the gaps between columns and set it on the
    /// text inside), and images still show.
    tiny: bool,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    link: Option<String>,
    /// The next number, for an `<ol>`.
    next: u32,
    /// Tinta: the text colour, highlight and size set here; `size: Some(None)` is normal size again.
    color: Option<u32>,
    highlight: Option<u32>,
    size: Option<Option<u16>>,
    mono: bool,
}

/// Tinta: a table being read. Whether it holds data or only lays the message out is known at its
/// end ([`Builder::finish_table`]).
#[derive(Default)]
struct Table {
    id: u16,
    /// Where its element is on the stack.
    depth: usize,
    /// The first block read inside it.
    start: usize,
    row: Option<u16>,
    col: Option<u16>,
    header: bool,
    /// Rows with two cells or more.
    wide_rows: u16,
    last_wide: Option<u16>,
    /// It holds another table, or is too big: a layout table.
    layout: bool,
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
    /// Tinta: the cell of the block being read, and the tables open around it.
    cell: Option<Cell>,
    tables: Vec<Table>,
    next_table: u16,
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

    /// Whether text here is too small to see: the innermost `font-size` is zero.
    fn tiny(&self) -> bool {
        self.stack.last().is_some_and(|o| o.tiny)
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
            s.code |= o.mono || matches!(o.name.as_str(), "code" | "tt" | "kbd" | "samp");
            s.color = o.color.or(s.color);
            s.highlight = o.highlight.or(s.highlight);
            if let Some(size) = o.size {
                s.size = size;
            }
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
        if raw.is_empty() || self.hidden() || self.tiny() || self.full {
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
            self.cell = self.current_cell();
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
                self.spans.push(Span { text: " ".into(), link: None, image: false, cid: None, src: None, ..style.clone() });
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
                    self.cell = self.current_cell();
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
        let cell = self.cell.take();
        if spans.is_empty() {
            return false;
        }
        for s in &mut spans {
            s.deceptive = deceptive(s);
        }
        let kind = self.kind.take().unwrap_or(if self.in_pre() { BlockKind::Pre } else { BlockKind::Paragraph });
        self.add(Block { kind, quote: self.quote, spans, cell });
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

    /// Tinta: the table cell text read now goes in, if any.
    fn current_cell(&self) -> Option<Cell> {
        let t = self.tables.last()?;
        let in_cell = self.stack.iter().skip(t.depth.saturating_add(1)).any(|o| o.name == "td" || o.name == "th");
        if !in_cell || t.layout {
            return None;
        }
        Some(Cell { table: t.id, row: t.row.unwrap_or(0), col: t.col?, header: t.header })
    }

    /// Tinta: tables whose element is no longer open are finished.
    fn settle_tables(&mut self) {
        while let Some(t) = self.tables.last() {
            if self.stack.get(t.depth).is_some_and(|o| o.name == "table") {
                break;
            }
            if let Some(t) = self.tables.pop() {
                self.finish_table(&t);
            }
        }
    }

    /// Tinta: a table of data (two rows of two cells or more, no table inside, not too big) keeps
    /// its cells; any other table only laid the message out, and its text flows as paragraphs.
    fn finish_table(&mut self, t: &Table) {
        if !t.layout && t.wide_rows >= 2 {
            return;
        }
        for b in self.out.blocks.iter_mut().skip(t.start) {
            if b.cell.is_some_and(|c| c.table == t.id) {
                b.cell = None;
            }
        }
        if self.cell.is_some_and(|c| c.table == t.id) {
            self.cell = None;
        }
    }

    /// Tinta: count the rows and cells of the innermost table.
    fn table_tag(&mut self, name: &str) {
        let Some(t) = self.tables.last_mut() else { return };
        match name {
            "tr" => {
                t.row = Some(t.row.map_or(0, |r| r.saturating_add(1)));
                t.col = None;
            }
            "td" | "th" => {
                let row = *t.row.get_or_insert(0);
                let col = t.col.map_or(0, |c| c.saturating_add(1));
                t.col = Some(col);
                t.header = name == "th";
                if col >= 1 && t.last_wide != Some(row) {
                    t.last_wide = Some(row);
                    t.wide_rows = t.wide_rows.saturating_add(1);
                }
            }
            _ => return,
        }
        if t.row.is_some_and(|r| r >= MAX_TABLE_ROWS) || t.col.is_some_and(|c| c >= MAX_TABLE_COLS) {
            t.layout = true;
        }
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
                self.settle_tables();
            }
        }
        if name == "body" {
            self.stack.retain(|o| o.name != "head");
            self.settle_tables();
        }
        let mut css = tag.style.as_deref().map(Css::parse).unwrap_or_default();
        if name == "font" {
            css.color = css.color.or_else(|| tag.color.as_deref().and_then(css_color));
            css.size = css.size.or_else(|| tag.size.as_deref().and_then(font_size_attr));
            css.mono |= tag.face.as_deref().is_some_and(monospace);
        }
        // Tinta: a highlight is an inline element's; the backgrounds of blocks and cells lay the
        // message out (often white), and are not drawn.
        if !INLINE.contains(&name) {
            css.highlight = None;
        }
        if name == "mark" {
            css.highlight = css.highlight.or(Some(0x00ff_e86b));
        }
        match name {
            "big" => css.size = css.size.or(Some(relative(120))),
            "small" => css.size = css.size.or(Some(relative(85))),
            _ => {}
        }
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
                let cell = self.current_cell();
                self.add(Block { kind: BlockKind::Rule, quote, spans: Vec::new(), cell });
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
            tiny: css.font_size.unwrap_or_else(|| self.tiny()),
            bold: css.bold || matches!(name, "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th" | "dt"),
            italic: css.italic || matches!(name, "i" | "em" | "cite" | "var" | "dfn" | "address"),
            underline: css.underline || matches!(name, "u" | "ins"),
            strike: css.strike || matches!(name, "s" | "strike" | "del"),
            link: None,
            next: 1,
            color: css.color,
            highlight: css.highlight,
            size: css.size,
            mono: css.mono,
        };
        if name == "a" {
            o.link = tag.href.as_deref().and_then(safe_link);
        }
        if name == "ol" {
            o.next = tag.start.as_deref().and_then(number).unwrap_or(1);
        }
        if name == "table" {
            if let Some(outer) = self.tables.last_mut() {
                outer.layout = true;
            }
            self.tables.push(Table { id: self.next_table, depth: self.stack.len(), start: self.out.blocks.len(), ..Table::default() });
            self.next_table = self.next_table.saturating_add(1);
        }
        self.table_tag(name);
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
        self.settle_tables();
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
        let cid = if src.starts_with("cid:") { tag.src.as_deref().and_then(ibercorreo_core::mail::content_id_key) } else { None };
        let src = if remote { tag.src.as_deref().and_then(picture_url) } else { None };
        self.push(text, Span { image: true, cid, src, ..style });
    }
}

/// Tinta: the web address of a picture as it may be kept ([`Span::src`]): entities decoded,
/// `//host` taken as `https:`, only `http:` and `https:`, no spaces or control characters, at most
/// [`ibercorreo_core::rich::MAX_PICTURE_URL`] bytes.
fn picture_url(raw: &str) -> Option<String> {
    let url = decode_entities(raw.trim());
    let url = if url.starts_with("//") { format!("https:{url}") } else { url.to_string() };
    let lower = url.get(..8).unwrap_or("").to_ascii_lowercase();
    let scheme = lower.starts_with("https://") || lower.starts_with("http://");
    let clean = url.chars().all(|c| !c.is_control() && !c.is_whitespace());
    (scheme && clean && url.len() <= ibercorreo_core::rich::MAX_PICTURE_URL && url.len() > 8).then_some(url)
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
            "color" => &mut tag.color,
            "size" => &mut tag.size,
            "face" => &mut tag.face,
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
    /// `font-size`, when set: whether it is zero.
    font_size: Option<bool>,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    /// Tinta: `color`, `background-color`, `font-size` (`Some(None)`: about normal), a fixed-width
    /// `font-family`.
    color: Option<u32>,
    highlight: Option<u32>,
    size: Option<Option<u16>>,
    mono: bool,
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
                "font-size" if value != "inherit" => css.font_size = Some(zero),
                "max-height" | "max-width" | "opacity" => css.hidden |= zero,
                "font-weight" => css.bold |= value == "bold" || value == "bolder" || value.parse::<u32>().is_ok_and(|n| n >= 600),
                "font-style" => css.italic |= value == "italic" || value == "oblique",
                "text-decoration" | "text-decoration-line" => {
                    css.underline |= value.contains("underline");
                    css.strike |= value.contains("line-through");
                }
                "color" => css.color = css_color(&value).or(css.color),
                "background-color" => css.highlight = css_color(&value).or(css.highlight),
                "background" => css.highlight = value.split_whitespace().find_map(css_color).or(css.highlight),
                "font-family" => css.mono = monospace(&value),
                _ => {}
            }
            if prop == "font-size"
                && let Some(size) = css_size(&value)
            {
                css.size = Some(size);
            }
        }
        css
    }
}

/// Tinta: inline elements, whose background is a highlight of their text.
const INLINE: &[&str] = &["span", "font", "mark", "b", "strong", "i", "em", "u", "s", "a", "code", "small", "big", "sup", "sub"];

/// Tinta: a CSS colour as `0xRRGGBB`: `#rgb`, `#rrggbb` (an alpha part is ignored), `rgb()`,
/// `rgba()` or a common name. `transparent` and anything else give `None`.
fn css_color(value: &str) -> Option<u32> {
    let v = value.trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        let digits: Vec<u32> = hex.chars().map(|c| c.to_digit(16)).collect::<Option<_>>()?;
        return match digits.as_slice() {
            [r, g, b] | [r, g, b, _] => Some((r * 17) << 16 | (g * 17) << 8 | (b * 17)),
            [r1, r2, g1, g2, b1, b2] | [r1, r2, g1, g2, b1, b2, _, _] => Some((r1 * 16 + r2) << 16 | (g1 * 16 + g2) << 8 | (b1 * 16 + b2)),
            _ => None,
        };
    }
    if let Some(args) = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb(")) {
        let args = args.strip_suffix(')')?;
        let parts: Vec<&str> = args.split([',', ' ', '/']).filter(|p| !p.is_empty()).collect();
        if let Some(alpha) = parts.get(3) {
            let a = alpha.trim_end_matches('%').parse::<f32>().ok()?;
            if a <= 0.0 {
                return None;
            }
        }
        let channel = |p: &str| -> Option<u32> {
            let n = if let Some(pc) = p.strip_suffix('%') { pc.parse::<f32>().ok()? * 2.55 } else { p.parse::<f32>().ok()? };
            // A NaN gives 0 and anything out of range is clamped, so the cast always fits.
            Some(if n.is_nan() { 0 } else { n.clamp(0.0, 255.0).round() as u32 })
        };
        let (r, g, b) = (channel(parts.first()?)?, channel(parts.get(1)?)?, channel(parts.get(2)?)?);
        return Some(r << 16 | g << 8 | b);
    }
    let named = [
        ("black", 0x000000),
        ("white", 0xffffff),
        ("red", 0xff0000),
        ("green", 0x008000),
        ("blue", 0x0000ff),
        ("navy", 0x000080),
        ("gray", 0x808080),
        ("grey", 0x808080),
        ("silver", 0xc0c0c0),
        ("maroon", 0x800000),
        ("purple", 0x800080),
        ("teal", 0x008080),
        ("olive", 0x808000),
        ("orange", 0xffa500),
        ("yellow", 0xffff00),
        ("lime", 0x00ff00),
        ("aqua", 0x00ffff),
        ("cyan", 0x00ffff),
        ("fuchsia", 0xff00ff),
        ("magenta", 0xff00ff),
        ("darkred", 0x8b0000),
        ("darkgreen", 0x006400),
        ("darkblue", 0x00008b),
        ("darkgray", 0xa9a9a9),
        ("darkgrey", 0xa9a9a9),
        ("lightgray", 0xd3d3d3),
        ("lightgrey", 0xd3d3d3),
        ("crimson", 0xdc143c),
        ("brown", 0xa52a2a),
        ("gold", 0xffd700),
        ("pink", 0xffc0cb),
    ];
    named.iter().find(|(n, _)| *n == v).map(|(_, c)| *c)
}

/// Tinta: a size in percent of normal text as kept: near normal (85–118 %) is `None`, the rest is
/// clamped to [`SIZE_RANGE`].
fn relative(percent: u16) -> Option<u16> {
    (!(85..=118).contains(&percent)).then(|| percent.clamp(SIZE_RANGE.0, SIZE_RANGE.1))
}

/// Tinta: a CSS `font-size` as [`relative`] keeps it, taking 15 px as normal; `None` when it
/// cannot be read (or is zero, which hides the text instead).
fn css_size(value: &str) -> Option<Option<u16>> {
    let keyword = match value {
        "xx-small" => Some(60),
        "x-small" => Some(70),
        "small" | "smaller" => Some(85),
        "medium" => Some(100),
        "large" | "larger" => Some(120),
        "x-large" => Some(150),
        "xx-large" => Some(200),
        "xxx-large" => Some(250),
        _ => None,
    };
    if let Some(k) = keyword {
        return Some(relative(k));
    }
    let unit_at = value.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let n: f32 = value.get(..unit_at)?.parse().ok()?;
    let percent = match value.get(unit_at..)?.trim() {
        "px" => n / 15.0 * 100.0,
        "pt" => n * 4.0 / 3.0 / 15.0 * 100.0,
        "em" | "rem" => n * 100.0,
        "%" => n,
        _ => return None,
    };
    if !percent.is_finite() || percent <= 0.0 {
        return None;
    }
    // Clamped before the cast, so it always fits.
    Some(relative(percent.clamp(1.0, 1_000.0).round() as u16))
}

/// Tinta: `<font size="1">`…`7`, or `+n`/`-n` from 3.
fn font_size_attr(value: &str) -> Option<Option<u16>> {
    let v = value.trim();
    let n: i32 = if let Some(up) = v.strip_prefix('+') {
        3 + up.parse::<i32>().ok()?
    } else if v.starts_with('-') {
        3 + v.parse::<i32>().ok()?
    } else {
        v.parse().ok()?
    };
    let percent = [60, 82, 100, 113, 150, 200, 250].get(usize::try_from(n.clamp(1, 7) - 1).ok()?).copied()?;
    Some(relative(percent))
}

/// Tinta: a `font-family` (or `<font face>`) whose first choice is a fixed-width font.
fn monospace(value: &str) -> bool {
    let first = value.split(',').next().unwrap_or("").trim().trim_matches(['"', '\'']).to_ascii_lowercase();
    ["monospace", "courier", "consolas", "menlo", "monaco", "lucida console"].iter().any(|m| first.contains(m)) || first.ends_with(" mono")
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
