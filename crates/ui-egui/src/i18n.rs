//! Interface languages. Strings in the code stay English and are the lookup keys; a catalog maps
//! them to display text when the UI is drawn. Command ids, document text, file names, the control
//! channel, the CLI and MCP never see translated text, so scripts and agents are unaffected. A
//! string without a translation shows in English, so coverage can grow one entry at a time, and a
//! missing translation never fails a build, a test or a release.
//!
//! Catalogs are `i18n/<code>.tsv` in the craft-i18n catalog format (`context <TAB> English <TAB>
//! translation`, `\n` `\t` `\\` escapes, `#` comments), the format the other Tinta Office apps
//! share. Only rows with an empty context are read here.
//!
//! The store's `language` is unset by default: the UI follows the operating system's language when
//! there is a catalog for it, and English otherwise. The status bar's language menu, or the
//! `app.language` command, picks one by hand.
//!
//! # Adding a language
//! Add `i18n/<code>.tsv`, a [`Language`] variant and its rows in [`Language::ALL`],
//! [`Language::code`], [`Language::name`] and [`Language::source`].

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::LazyLock;

/// The `language` setting that follows the system.
pub const AUTO: &str = "auto";

/// An interface language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    En,
    Uk,
    Es,
}

impl Language {
    /// Every language, English first: it is the source and the fallback.
    pub const ALL: [Language; 3] = [Language::En, Language::Uk, Language::Es];

    /// BCP 47 code, lowercase; also the `language` setting's value.
    pub const fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Uk => "uk",
            Language::Es => "es",
        }
    }

    /// The language's name in itself, for the picker.
    pub const fn name(self) -> &'static str {
        match self {
            Language::En => "English",
            Language::Uk => "Українська",
            Language::Es => "Español",
        }
    }

    /// The catalog source; `None` for English, which needs none.
    const fn source(self) -> Option<&'static str> {
        match self {
            Language::En => None,
            Language::Uk => Some(include_str!("i18n/uk.tsv")),
            Language::Es => Some(include_str!("i18n/es.tsv")),
        }
    }

    /// The language a BCP 47 or POSIX locale tag (`uk`, `uk-UA`, `uk_UA.UTF-8`, `uk_UA@euro`)
    /// asks for, when there is one.
    pub fn from_tag(tag: &str) -> Option<Language> {
        let primary = tag.split(['-', '_', '.', '@']).next()?.trim().to_ascii_lowercase();
        Language::ALL.into_iter().find(|l| l.code() == primary)
    }

    fn catalog(self) -> Option<&'static Catalog> {
        static UK: LazyLock<Catalog> = LazyLock::new(|| Catalog::parse(Language::Uk.source().unwrap_or_default()));
        static ES: LazyLock<Catalog> = LazyLock::new(|| Catalog::parse(Language::Es.source().unwrap_or_default()));
        match self {
            Language::En => None,
            Language::Uk => Some(&UK),
            Language::Es => Some(&ES),
        }
    }
}

/// The language the store's `language` resolves to: unset, `auto` or a code without a catalog
/// follows the system.
pub fn resolve(pref: Option<&str>) -> Language {
    pref.filter(|p| *p != AUTO).and_then(Language::from_tag).unwrap_or_else(system)
}

/// The operating system's interface language, when there is a catalog for it; English otherwise.
/// Tests always get English, whatever the machine's locale.
pub fn system() -> Language {
    #[cfg(any(test, target_arch = "wasm32"))]
    {
        Language::En
    }
    #[cfg(not(any(test, target_arch = "wasm32")))]
    {
        static SYSTEM: LazyLock<Language> =
            LazyLock::new(|| sys_locale::get_locales().take(16).find_map(|tag| Language::from_tag(&tag)).unwrap_or(Language::En));
        *SYSTEM
    }
}

thread_local! {
    /// The language the UI thread draws in; the app sets it from its setting every frame.
    static CURRENT: Cell<Language> = const { Cell::new(Language::En) };
}

pub fn set_current(language: Language) {
    CURRENT.with(|c| c.set(language));
}

pub fn current() -> Language {
    CURRENT.with(Cell::get)
}

/// `english` in the current language.
pub fn tl(english: &str) -> &str {
    tr(current(), english)
}

/// `english` in `language`: its translation, or `english` itself when there is none.
pub fn tr(language: Language, english: &str) -> &str {
    match language.catalog().and_then(|c| c.rows.get(english)) {
        Some(text) => text.as_str(),
        None => english,
    }
}

/// Whether `language` has a translation of `english`.
pub fn has(language: Language, english: &str) -> bool {
    language.catalog().is_some_and(|c| c.rows.contains_key(english))
}

/// `template` translated, with each `{name}` replaced by its value. Translators may reorder the
/// placeholders.
pub fn tf(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = tl(template).to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// One parsed catalog: plain rows only, keyed by their English.
struct Catalog {
    rows: HashMap<String, String>,
}

impl Catalog {
    /// Lenient: a malformed row (fewer than three columns, an empty translation) is skipped and
    /// that string shows in English; the first of two rows for the same English wins.
    fn parse(source: &str) -> Catalog {
        let mut rows = HashMap::new();
        for line in source.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let mut cols = line.splitn(3, '\t');
            let (Some(context), Some(english), Some(text)) = (cols.next(), cols.next(), cols.next()) else {
                continue;
            };
            let text = unescape(text.trim_end_matches('\r'));
            if !context.is_empty() || english.is_empty() || text.is_empty() {
                continue;
            }
            rows.entry(unescape(english)).or_insert(text);
        }
        Catalog { rows }
    }
}

/// The catalog escapes: `\n`, `\t` and `\\`. Any other backslash stays as it is.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn locale_tags_reduce_to_a_language() {
        for tag in ["uk", "uk-UA", "uk_UA.UTF-8", "UK-ua", "uk_UA@euro"] {
            assert_eq!(Language::from_tag(tag), Some(Language::Uk), "{tag}");
        }
        assert_eq!(Language::from_tag("en-GB"), Some(Language::En));
        assert_eq!(Language::from_tag("es_ES.UTF-8"), Some(Language::Es));
        assert_eq!(Language::from_tag("ru-RU"), None);
        assert_eq!(Language::from_tag(""), None);
        assert_eq!(resolve(Some("uk")), Language::Uk);
        assert_eq!(resolve(None), Language::En, "tests see an English system");
        assert_eq!(resolve(Some(AUTO)), Language::En);
        assert_eq!(resolve(Some("xx")), Language::En);
    }

    #[test]
    fn lookups_translate_and_fall_back_to_english() {
        assert_eq!(tr(Language::Uk, "Reply"), "Відповісти");
        assert_eq!(tr(Language::En, "Reply"), "Reply");
        assert_eq!(tr(Language::Es, "Reply"), "Responder");
        assert_eq!(tr(Language::Es, "Inbox"), "Bandeja de entrada");
        assert_eq!(tr(Language::Uk, "no such string anywhere"), "no such string anywhere");
        set_current(Language::Uk);
        assert_eq!(tl("Inbox"), "Вхідні");
        assert_eq!(tf("{n} Reminders", &[("n", "3")]), "Нагадувань: 3");
        set_current(Language::En);
        assert_eq!(tl("Inbox"), "Inbox");
        assert_eq!(tf("{n} Reminders", &[("n", "3")]), "3 Reminders");
    }

    #[test]
    fn hostile_catalog_rows_are_skipped() {
        let c = Catalog::parse("# comment\n\nonly two\tcolumns\n\tempty\t\nctx\tHome\tX\n\tA\\nB\tА\\nБ\n\tA\\nB\tsecond\n\ttrail\\\tx\\q\\\n");
        assert_eq!(c.rows.get("A\nB").map(String::as_str), Some("А\nБ"));
        assert!(!c.rows.contains_key("Home"), "a context row is not a plain row");
        assert!(!c.rows.contains_key("empty"));
        assert_eq!(c.rows.len(), 2);
    }

    /// Every row of every catalog: three columns, no context, an English key once, a non-empty
    /// translation, the same `{placeholders}` and the same trailing `…` as its English.
    #[test]
    fn catalogs_are_well_formed() {
        for language in Language::ALL {
            let Some(source) = language.source() else { continue };
            let mut seen = std::collections::HashSet::new();
            for (n, line) in source.lines().enumerate() {
                if line.trim().is_empty() || line.starts_with('#') {
                    continue;
                }
                let cols: Vec<&str> = line.split('\t').collect();
                assert_eq!(cols.len(), 3, "{}.tsv:{}: three columns", language.code(), n + 1);
                let (english, text) = (cols[1], cols[2]);
                assert!(cols[0].is_empty(), "{}.tsv:{}: no context", language.code(), n + 1);
                assert!(!text.trim().is_empty(), "{}.tsv:{}: empty translation", language.code(), n + 1);
                assert!(seen.insert(english), "{}.tsv:{}: {english:?} twice", language.code(), n + 1);
                assert_eq!(placeholders(english), placeholders(text), "{}.tsv:{}: placeholders of {english:?}", language.code(), n + 1);
                assert_eq!(english.ends_with('…'), text.ends_with('…'), "{}.tsv:{}: trailing … of {english:?}", language.code(), n + 1);
            }
        }
    }

    fn placeholders(s: &str) -> Vec<&str> {
        let mut v: Vec<&str> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(name, _)| name)).collect();
        v.sort_unstable();
        v
    }
}
