//! Mail kept outside the app: a message as RFC 5322 text for an `.eml` file or an mbox, and mbox
//! files themselves (the `mboxrd` form of RFC 4155, which Thunderbird, Apple Mail, Evolution and
//! mutt read). What the store holds is what is written: the headers, the text, the formatted body
//! as HTML and an invitation's calendar. Attachment files are not kept by the store, so they are
//! not written either.

use ibercorreo_core::rich::{Block, BlockKind, Rich, Span};

use super::*;

/// The `Date:` header for `dt`, a UTC time (`Thu, 22 Oct 2026 14:05:00 +0000`).
pub fn date_header(dt: DateTime) -> String {
    let (day, month) = names(dt.date);
    format!("{day}, {} {month} {:04} {:02}:{:02}:{:02} +0000", dt.date.day(), dt.date.year(), dt.hour(), dt.minute(), dt.second())
}

/// The date of an mbox `From ` line, as C's `asctime` writes it (`Thu Oct 22 14:05:00 2026`).
fn asctime(dt: DateTime) -> String {
    let (day, month) = names(dt.date);
    format!("{day} {month} {:>2} {:02}:{:02}:{:02} {:04}", dt.date.day(), dt.hour(), dt.minute(), dt.second(), dt.date.year())
}

fn names(d: Date) -> (&'static str, &'static str) {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let day = DAYS.get(usize::from(d.weekday().index())).copied().unwrap_or("Mon");
    let month = MONTHS.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or("Jan");
    (day, month)
}

/// RFC 5322 text for keeping `m`, which says `content`: unlike [`write`] it needs no sender or
/// recipient, keeps the formatted body as a `text/html` alternative and says whether the message
/// was read or flagged (the `Status`, `X-Status` and `X-Mozilla-Status` headers mail programs
/// read when they import an mbox). The same message always gives the same bytes. Lines end in
/// CRLF.
pub fn archived(m: &Message, content: &Content) -> String {
    let mut s = String::new();
    if let Some(from) = &m.from {
        header(&mut s, "From", &address_list(std::slice::from_ref(from)));
    }
    if !m.to.is_empty() {
        header(&mut s, "To", &address_list(&m.to));
    }
    if !m.cc.is_empty() {
        header(&mut s, "Cc", &address_list(&m.cc));
    }
    header(&mut s, "Subject", &encode_word(&single_line(clamp_text(&m.subject, MAX_FIELD_LEN))));
    if let Some(date) = m.date {
        header(&mut s, "Date", &date_header(date));
    }
    header(&mut s, "Message-ID", &format!("<{}.tinta-correo@localhost>", m.id.0));
    header(&mut s, "MIME-Version", "1.0");
    header(&mut s, "Status", if m.seen { "RO" } else { "O" });
    if m.flagged {
        header(&mut s, "X-Status", "F");
    }
    let mozilla = u16::from(m.seen) | if m.flagged { 0x0004 } else { 0 };
    header(&mut s, "X-Mozilla-Status", &format!("{mozilla:04x}"));
    let html = content.rich.as_ref().map(to_html);
    let invite = content.invite.as_deref().map(|cal| clamp_text(cal, MAX_INVITE_BYTES));
    if html.is_none() && invite.is_none() {
        text_part(&mut s, "text/plain; charset=utf-8", &content.body, None);
        return s;
    }
    // The text, the formatted body and the calendar as alternatives, richest last (RFC 2046
    // §5.1.4, RFC 6047 §2.4). The boundary comes from the message id, which is the same every time.
    let boundary = format!("=_tinta_correo_{}", m.id.0);
    header(&mut s, "Content-Type", &format!("multipart/alternative; boundary=\"{boundary}\""));
    s.push_str("\r\n");
    s.push_str(&format!("--{boundary}\r\n"));
    text_part(&mut s, "text/plain; charset=utf-8", &content.body, Some(&boundary));
    if let Some(html) = &html {
        s.push_str(&format!("--{boundary}\r\n"));
        text_part(&mut s, "text/html; charset=utf-8", html, Some(&boundary));
    }
    if let Some(cal) = invite {
        s.push_str(&format!("--{boundary}\r\n"));
        text_part(&mut s, &format!("text/calendar; charset=utf-8; method={}", calendar_method(cal)), cal, Some(&boundary));
    }
    s.push_str(&format!("--{boundary}--\r\n"));
    s
}

/// Adds `m` to the mbox text `out`: a `From ` line with the sender and the date, the message with
/// LF line ends, every line that starts with `From ` after any number of `>` quoted with one more
/// `>` (mboxrd, so a reader can undo it exactly), and a blank line.
pub fn push_mbox(out: &mut String, m: &Message, content: &Content) {
    let sender = m.from.as_ref().map(|a| a.email.as_str()).filter(|e| !e.is_empty() && !e.chars().any(|c| c.is_whitespace() || c.is_control()));
    let when = m.date.map(asctime).unwrap_or_else(|| "Thu Jan  1 00:00:00 1970".to_string());
    out.push_str(&format!("From {} {when}\n", sender.unwrap_or("MAILER-DAEMON")));
    for line in archived(m, content).split("\r\n") {
        if line.trim_start_matches('>').starts_with("From ") {
            out.push('>');
        }
        out.push_str(line);
        out.push('\n');
    }
    // `archived` ends in CRLF, so the split's last piece was empty and the loop already wrote the
    // blank line that ends the message.
}

/// A formatted body as an HTML document: paragraphs, headings, lists, quotes, preformatted text,
/// rules and styled, linked text. Images that were not shown are left out, as in
/// [`Rich::to_text`].
pub fn to_html(rich: &Rich) -> String {
    let mut out = String::from("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"></head><body>\n");
    let mut quote = 0u8;
    // The lists open around the current item: their kinds, outermost first.
    let mut lists: Vec<&'static str> = Vec::new();
    for b in &rich.blocks {
        let (depth, tag) = match b.kind {
            BlockKind::Bullet { depth } => (usize::from(depth.clamp(1, 16)), "ul"),
            BlockKind::Numbered { depth, .. } => (usize::from(depth.clamp(1, 16)), "ol"),
            _ => (0, ""),
        };
        let quote_changes = b.quote != quote;
        while lists.len() > depth || (quote_changes && !lists.is_empty()) || lists.last().is_some_and(|t| lists.len() == depth && *t != tag) {
            if let Some(t) = lists.pop() {
                out.push_str(&format!("</{t}>\n"));
            }
        }
        while quote < b.quote {
            out.push_str("<blockquote>\n");
            quote = quote.saturating_add(1);
        }
        while quote > b.quote {
            out.push_str("</blockquote>\n");
            quote = quote.saturating_sub(1);
        }
        while lists.len() < depth {
            out.push_str(&format!("<{tag}>\n"));
            lists.push(tag);
        }
        block(&mut out, b);
    }
    while let Some(t) = lists.pop() {
        out.push_str(&format!("</{t}>\n"));
    }
    for _ in 0..quote {
        out.push_str("</blockquote>\n");
    }
    out.push_str("</body></html>\n");
    out
}

fn block(out: &mut String, b: &Block) {
    let (open, close) = match b.kind {
        BlockKind::Paragraph => ("<p>".to_string(), "</p>".to_string()),
        BlockKind::Heading(n) => {
            let n = n.clamp(1, 6);
            (format!("<h{n}>"), format!("</h{n}>"))
        }
        BlockKind::Bullet { .. } => ("<li>".to_string(), "</li>".to_string()),
        BlockKind::Numbered { number, .. } => (format!("<li value=\"{number}\">"), "</li>".to_string()),
        BlockKind::Pre => ("<pre>".to_string(), "</pre>".to_string()),
        BlockKind::Rule => {
            out.push_str("<hr>\n");
            return;
        }
    };
    out.push_str(&open);
    for s in b.spans.iter().filter(|s| !s.image) {
        span(out, s);
    }
    out.push_str(&close);
    out.push('\n');
}

fn span(out: &mut String, s: &Span) {
    let styles = [(s.bold, "b"), (s.italic, "i"), (s.underline, "u"), (s.strike, "s"), (s.code, "code")];
    if let Some(link) = &s.link {
        out.push_str(&format!("<a href=\"{}\">", escape(link)));
    }
    for (_, tag) in styles.iter().filter(|(on, _)| *on) {
        out.push_str(&format!("<{tag}>"));
    }
    out.push_str(&escape(&s.text).replace('\n', "<br>"));
    for (_, tag) in styles.iter().rev().filter(|(on, _)| *on) {
        out.push_str(&format!("</{tag}>"));
    }
    if s.link.is_some() {
        out.push_str("</a>");
    }
}

/// Text and attribute values with the five characters HTML gives a meaning escaped.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests;
