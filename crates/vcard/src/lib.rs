//! vCard, L1: `.vcf` files and CardDAV address-book entries.
//!
//! Written from the RFCs: vCard 4.0 (RFC 6350), 3.0 (RFC 2426) and the 2.1 form Outlook still
//! exports (bare `TYPE` parameters, `QUOTED-PRINTABLE` values in a named `CHARSET`). [`parse`]
//! reads `FN` (or `N`), `EMAIL`, `TEL`, `ORG`, `TITLE`, `NOTE`, `BDAY` and `UID` into
//! [`Contact`]s; every other property line is kept as it came ([`Contact::kept`]) and written
//! back, so nothing is lost on a round trip. Lines that cannot be kept (beyond the caps, or
//! malformed) are counted in [`Parsed::skipped`], never silently dropped. [`write`] writes vCard
//! 3.0, the version every address book and CardDAV server reads, with escaping and 75-octet
//! folding.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use ibercorreo_core::{Address, Contact, Date, Error, MAX_FIELD_LEN, Phone, Result, clamp_text};

pub const MAX_VCF_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_CARDS: usize = 100_000;
/// Most addresses and most phones read per card.
const MAX_VALUES: usize = 50;
/// Most lines, and most bytes, kept unread per card (a photo larger than this is not kept).
const MAX_KEPT_LINES: usize = 200;
const MAX_KEPT_BYTES: usize = 256 * 1024;
const MAX_DEPTH: usize = 8;

#[derive(Debug, Default)]
pub struct Parsed {
    pub contacts: Vec<Contact>,
    /// Property lines neither read nor kept: malformed, unusable addresses, or beyond the caps.
    pub skipped: usize,
    /// Cards dropped, with the reason (no name, no address).
    pub rejected: Vec<String>,
}

/// One content line: `group.NAME;PARAM=V;…:VALUE`.
struct Line<'a> {
    name: String,
    params: Vec<(String, String)>,
    value: &'a str,
}

impl Line<'_> {
    fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Every `TYPE`, lower case: `TYPE=work,voice`, `TYPE=work;TYPE=pref` and 2.1's bare `;WORK`.
    fn types(&self) -> Vec<String> {
        self.params.iter().filter(|(k, _)| k == "TYPE").flat_map(|(_, v)| v.split(',').map(|t| t.trim().to_ascii_lowercase())).collect()
    }

    /// The value as text: quoted-printable decoded (in its `CHARSET`), still escaped.
    fn raw_text(&self) -> String {
        let qp = self.param("ENCODING").is_some_and(|e| e.eq_ignore_ascii_case("QUOTED-PRINTABLE"));
        if !qp {
            return self.value.to_string();
        }
        let bytes = quoted_printable(self.value);
        match self.param("CHARSET").map(str::to_ascii_lowercase).as_deref() {
            None | Some("utf-8" | "utf8") => String::from_utf8_lossy(&bytes).into_owned(),
            // ISO-8859-1 and Windows-1252 are Latin-1 for every letter in names.
            Some(_) => bytes.iter().map(|b| char::from(*b)).collect(),
        }
    }
}

/// The property name of a content line, upper case and without its group (`item1.EMAIL` →
/// `EMAIL`).
pub fn property_name(line: &str) -> Option<String> {
    split_line(line).map(|l| l.name)
}

fn quoted_printable(v: &str) -> Vec<u8> {
    let bytes = v.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'=' {
            let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(h) = hex {
                out.push(h);
                i += 3;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    out
}

/// Logical lines: folded lines joined (a leading space or tab continues the line), and a
/// quoted-printable value's soft line breaks (a trailing `=`) joined too.
fn logical_lines(text: &str) -> Vec<String> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.trim_end_matches('\r');
        if let Some(last) = out.last_mut() {
            if let Some(rest) = raw.strip_prefix([' ', '\t']) {
                last.push_str(rest);
                continue;
            }
            let head = last.split(':').next().unwrap_or("").to_ascii_uppercase();
            if head.contains("QUOTED-PRINTABLE") && last.ends_with('=') {
                last.pop();
                last.push_str(raw);
                continue;
            }
        }
        if !raw.trim().is_empty() {
            out.push(raw.to_string());
        }
    }
    out
}

fn split_line(line: &str) -> Option<Line<'_>> {
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let head = line.get(..colon)?;
    let value = line.get(colon + 1..)?;
    let mut parts = head.split(';');
    let name = parts.next()?.trim();
    let name = name.rsplit_once('.').map_or(name, |(_, n)| n).to_ascii_uppercase();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let params = parts
        .filter(|p| !p.trim().is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_uppercase(), v.trim().trim_matches('"').to_string()),
            // vCard 2.1: `TEL;WORK;VOICE:` names its types bare; `QUOTED-PRINTABLE` alone is the encoding.
            None if p.trim().eq_ignore_ascii_case("QUOTED-PRINTABLE") => ("ENCODING".into(), p.trim().to_string()),
            None => ("TYPE".into(), p.trim().to_string()),
        })
        .collect();
    Some(Line { name, params, value })
}

/// Escaped text unescaped; with `split`, cut into components at unescaped `;` first.
fn components(v: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n' | 'N') => push_char(&mut out, '\n'),
                Some(other) => push_char(&mut out, other),
                None => {}
            },
            ';' if out.len() < 16 => out.push(String::new()),
            c => push_char(&mut out, c),
        }
    }
    out.into_iter().map(|s| clamp_text(s.trim(), MAX_FIELD_LEN).to_string()).collect()
}

fn push_char(out: &mut [String], c: char) {
    if let Some(last) = out.last_mut() {
        last.push(c);
    }
}

fn text(v: &str) -> String {
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    // Line breaks as `\n`, however the sender wrote them (2.1 encodes CRLF as `=0D=0A`).
    let out = out.replace("\r\n", "\n").replace('\r', "\n");
    clamp_text(out.trim(), MAX_FIELD_LEN).to_string()
}

fn phone_kind(types: &[String]) -> &'static str {
    let has = |t: &str| types.iter().any(|x| x == t);
    if has("cell") || has("mobile") {
        "mobile"
    } else if has("fax") {
        "fax"
    } else if has("work") {
        "work"
    } else if has("home") {
        "home"
    } else {
        ""
    }
}

#[derive(Default)]
struct Draft {
    full_name: Option<String>,
    structured: Option<String>,
    emails: Vec<String>,
    phones: Vec<Phone>,
    organization: Option<String>,
    title: Option<String>,
    note: String,
    birthday: Option<Date>,
    uid: String,
    kept: Vec<String>,
    kept_bytes: usize,
}

impl Draft {
    fn keep(&mut self, line: &str, skipped: &mut usize) {
        if self.kept.len() < MAX_KEPT_LINES && self.kept_bytes.saturating_add(line.len()) <= MAX_KEPT_BYTES {
            self.kept_bytes = self.kept_bytes.saturating_add(line.len());
            self.kept.push(line.to_string());
        } else {
            *skipped = skipped.saturating_add(1);
        }
    }

    fn finish(self, skipped: &mut usize) -> Result<Contact> {
        let name = self
            .full_name
            .filter(|n| !n.is_empty())
            .or(self.structured.filter(|n| !n.is_empty()))
            .or_else(|| self.emails.first().cloned())
            .ok_or_else(|| Error::invalid("contact", "a card with no name and no address"))?;
        let mut c = Contact::new(&name);
        for e in &self.emails {
            match Address::new(Some(&name), e) {
                Ok(a) if !c.emails.iter().any(|x| x.email.eq_ignore_ascii_case(&a.email)) => c.emails.push(a),
                Ok(_) => {}
                Err(_) => *skipped = skipped.saturating_add(1),
            }
        }
        c.uid = self.uid;
        c.phones = self.phones;
        c.organization = self.organization;
        c.title = self.title;
        c.note = self.note;
        c.birthday = self.birthday;
        c.kept = self.kept;
        Ok(c)
    }
}

/// Every card in `text`.
pub fn parse(text_in: &str) -> Result<Parsed> {
    if text_in.len() > MAX_VCF_BYTES {
        return Err(Error::TooLarge { what: "contacts file", size: text_in.len(), limit: MAX_VCF_BYTES });
    }
    let mut out = Parsed::default();
    let mut depth = 0usize;
    let mut draft: Option<Draft> = None;
    let mut saw_card = false;
    for raw in logical_lines(text_in) {
        let Some(line) = split_line(&raw) else {
            out.skipped = out.skipped.saturating_add(1);
            continue;
        };
        match line.name.as_str() {
            "BEGIN" if line.value.trim().eq_ignore_ascii_case("VCARD") => {
                depth = depth.saturating_add(1);
                if depth > MAX_DEPTH {
                    return Err(Error::invalid("contacts file", "cards nested too deep"));
                }
                if depth == 1 {
                    saw_card = true;
                    draft = Some(Draft::default());
                }
            }
            "END" if line.value.trim().eq_ignore_ascii_case("VCARD") => {
                if depth == 0 {
                    return Err(Error::invalid("contacts file", "END:VCARD without BEGIN:VCARD"));
                }
                depth -= 1;
                if depth == 0
                    && let Some(d) = draft.take()
                {
                    if out.contacts.len() >= MAX_CARDS {
                        return Err(Error::TooLarge { what: "contacts", size: out.contacts.len() + 1, limit: MAX_CARDS });
                    }
                    match d.finish(&mut out.skipped) {
                        Ok(c) => out.contacts.push(c),
                        Err(e) => out.rejected.push(e.to_string()),
                    }
                }
            }
            // A card inside a card (2.1's AGENT) is not read.
            _ if depth != 1 => out.skipped = out.skipped.saturating_add(1),
            name => {
                let Some(d) = draft.as_mut() else { continue };
                let value = line.raw_text();
                match name {
                    "VERSION" | "PRODID" | "REV" => {}
                    "FN" => d.full_name = Some(text(&value)),
                    "N" => {
                        // Family; Given; Additional; Prefixes; Suffixes → "Prefix Given Additional Family Suffix".
                        let parts = components(&value);
                        let at = |i: usize| parts.get(i).map(String::as_str).unwrap_or("");
                        let full: Vec<&str> = [at(3), at(1), at(2), at(0), at(4)].into_iter().filter(|p| !p.is_empty()).collect();
                        d.structured = Some(full.join(" "));
                        // Kept, so the parts are written back as they were.
                        d.keep(&raw, &mut out.skipped);
                    }
                    "EMAIL" if d.emails.len() < MAX_VALUES => d.emails.push(text(&value)),
                    "TEL" if d.phones.len() < MAX_VALUES => {
                        let number = text(&value);
                        let number = number.strip_prefix("tel:").unwrap_or(&number).trim().to_string();
                        if number.is_empty() || number.contains('\n') {
                            out.skipped = out.skipped.saturating_add(1);
                        } else {
                            d.phones.push(Phone { kind: phone_kind(&line.types()).to_string(), number: clamp_text(&number, 256).to_string() });
                        }
                    }
                    "ORG" => {
                        let parts: Vec<String> = components(&value).into_iter().filter(|p| !p.is_empty()).collect();
                        d.organization = Some(parts.join(", ")).filter(|o| !o.is_empty());
                    }
                    "TITLE" => d.title = Some(text(&value)).filter(|t| !t.is_empty()),
                    "NOTE" => d.note = text(&value),
                    "UID" => d.uid = clamp_text(text(&value).trim(), 1024).to_string(),
                    // A full date; a birthday without a year (`--1231`) is kept as it came.
                    "BDAY" => match Date::parse(value.split('T').next().unwrap_or("")) {
                        Ok(day) => d.birthday = Some(day),
                        Err(_) => d.keep(&raw, &mut out.skipped),
                    },
                    _ => d.keep(&raw, &mut out.skipped),
                }
            }
        }
    }
    if !saw_card {
        return Err(Error::invalid("contacts file", "no BEGIN:VCARD"));
    }
    if depth != 0 {
        return Err(Error::invalid("contacts file", "a card is not closed (END:VCARD)"));
    }
    Ok(out)
}

fn escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Fold to lines of at most 75 octets, never inside a UTF-8 sequence.
fn push_folded(out: &mut String, line: &str) {
    let mut width = 0;
    for c in line.chars() {
        if c == '\r' || c == '\n' {
            continue;
        }
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
}

/// The `N` written for a name that came without one: the last word is the family name.
fn structured_name(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    match words.split_last() {
        Some((family, given)) if !given.is_empty() => format!("{};{};;;", escape(family), escape(&given.join(" "))),
        _ => format!(";{};;;", escape(name.trim())),
    }
}

/// One card, as vCard 3.0.
pub fn write_card(c: &Contact) -> String {
    let mut out = String::new();
    for l in ["BEGIN:VCARD", "VERSION:3.0", "PRODID:-//IberOffice//Iber Correo//EN"] {
        push_folded(&mut out, l);
    }
    if !c.uid.is_empty() {
        push_folded(&mut out, &format!("UID:{}", escape(&c.uid)));
    }
    push_folded(&mut out, &format!("FN:{}", escape(&c.name)));
    let kept_n = c.kept.iter().any(|l| property_name(l).as_deref() == Some("N"));
    if !kept_n {
        push_folded(&mut out, &format!("N:{}", structured_name(&c.name)));
    }
    for (i, e) in c.emails.iter().enumerate() {
        let pref = if i == 0 && c.emails.len() > 1 { ",PREF" } else { "" };
        push_folded(&mut out, &format!("EMAIL;TYPE=INTERNET{pref}:{}", escape(&e.email)));
    }
    for p in &c.phones {
        let kind = match p.kind.as_str() {
            "mobile" => ";TYPE=CELL",
            "work" => ";TYPE=WORK,VOICE",
            "home" => ";TYPE=HOME,VOICE",
            "fax" => ";TYPE=FAX",
            _ => "",
        };
        push_folded(&mut out, &format!("TEL{kind}:{}", escape(&p.number)));
    }
    if let Some(o) = &c.organization {
        push_folded(&mut out, &format!("ORG:{}", escape(o)));
    }
    if let Some(t) = &c.title {
        push_folded(&mut out, &format!("TITLE:{}", escape(t)));
    }
    if !c.note.is_empty() {
        push_folded(&mut out, &format!("NOTE:{}", escape(&c.note)));
    }
    if let Some(b) = c.birthday {
        push_folded(&mut out, &format!("BDAY:{b}"));
    }
    for l in &c.kept {
        push_folded(&mut out, l);
    }
    push_folded(&mut out, "END:VCARD");
    out
}

/// Every contact, one card after another (a `.vcf` file).
pub fn write<'a>(contacts: impl IntoIterator<Item = &'a Contact>) -> String {
    contacts.into_iter().map(write_card).collect()
}

#[cfg(test)]
mod tests;
