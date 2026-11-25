//! Tinta Office: the desktop entry and the AppStream metadata are translated into every language
//! in `packaging/linux/tinta-languages.txt` (tintaoffice-web#43). `tinta-l10n.py` adds the rows of
//! `tinta-l10n.txt` at packaging time, keyed by the release's English; this test fails when an
//! English text has no row for some language, for example after upstream reworded it.

use std::collections::HashSet;
use std::path::PathBuf;

const DESKTOP: &str = "packaging/linux/io.github.stdenis1980.ibercorreo.desktop";
const METAINFO: &str = "packaging/linux/io.github.stdenis1980.ibercorreo.metainfo.xml.in";

fn repo_file(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()))
}

fn plain(xml: &str) -> String {
    let text = xml.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text of every `<tag>…</tag>` in `xml` (elements with attributes, such as `xml:lang`, are
/// not matched).
fn elements<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(&after[..end]);
        rest = &after[end + close.len()..];
    }
    found
}

fn english_texts() -> Vec<String> {
    let mut texts = Vec::new();
    for line in repo_file(DESKTOP).lines() {
        if line.starts_with('[') && line != "[Desktop Entry]" {
            break;
        }
        if let Some((key, value)) = line.split_once('=') {
            if ["GenericName", "Comment", "Keywords"].contains(&key) {
                texts.push(value.to_string());
            }
        }
    }
    let metainfo = repo_file(METAINFO);
    for tag in ["summary", "caption"] {
        texts.extend(elements(&metainfo, tag).into_iter().map(plain));
    }
    for description in elements(&metainfo, "description") {
        texts.extend(elements(description, "p").into_iter().map(plain));
    }
    for keywords in elements(&metainfo, "keywords") {
        let words: Vec<String> = elements(keywords, "keyword").into_iter().map(plain).collect();
        texts.push(format!("{};", words.join(";")));
    }
    texts
}

#[test]
fn store_text_is_translated_into_every_tinta_language() {
    let languages: Vec<String> = repo_file("packaging/linux/tinta-languages.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    assert!(languages.contains(&"uk".to_string()), "{languages:?}");
    let rows: HashSet<(String, String)> = repo_file("packaging/linux/tinta-l10n.txt")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.split('\t');
            Some((parts.next()?.to_string(), parts.next()?.to_string()))
        })
        .collect();
    let texts = english_texts();
    assert!(texts.len() >= 4, "found too few English texts: {texts:?}");
    let missing: Vec<String> = texts
        .iter()
        .flat_map(|text| languages.iter().filter(|lang| !rows.contains(&((*lang).clone(), text.clone()))).map(move |lang| format!("{lang}\t{text}")))
        .collect();
    assert!(missing.is_empty(), "add these rows to packaging/linux/tinta-l10n.txt:\n{}", missing.join("\n"));
}
