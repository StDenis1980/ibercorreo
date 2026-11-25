//! Tinta Office: Share Tinta Office copies the link to the Tinta Office website to the clipboard,
//! so the person can paste it in a message or an e-mail. Nothing is sent and nothing is counted in
//! the app: `ref=app` only lets the website's own page statistics tell visits from a shared link
//! apart from other visits.
//!
//! The same file lives in all five Tinta apps; its strings are translated here, not in the shared
//! catalogues, so syncing with upstream never conflicts.

/// The link that is copied.
pub const URL: &str = "https://tintaoffice.com/?ref=app";

/// The menu item and command label.
pub const MENU: &str = "Share Tinta Office";
/// Said once the link is on the clipboard.
pub const COPIED: &str = "Link to Tinta Office copied. Paste it in a message or an e-mail to share it.";

const STRINGS: [&str; 2] = [MENU, COPIED];

/// `text` (one of the constants above) in the language `lang` (`es`, `ca-valencia`, `pt_BR`…),
/// English when that language has no translation.
pub fn tr(lang: &str, text: &'static str) -> &'static str {
    let lang = lang.split(['-', '_']).next().unwrap_or(lang).to_ascii_lowercase();
    let Some(i) = STRINGS.iter().position(|s| *s == text) else { return text };
    TRANSLATIONS.iter().find(|(code, _)| *code == lang).and_then(|(_, t)| t.get(i).copied()).unwrap_or(text)
}

/// Puts [`URL`] on the clipboard and returns what to tell the person, in `lang`.
pub fn copy(ctx: &egui::Context, lang: &str) -> &'static str {
    ctx.copy_text(URL.to_owned());
    tr(lang, COPIED)
}

/// For apps whose commands run without the egui context: the command asks, the next frame copies.
#[derive(Debug, Default)]
pub struct Share {
    pending: bool,
}

impl Share {
    /// Copy the link on the next frame.
    pub fn request(&mut self) {
        self.pending = true;
    }

    /// Each frame: copies the link when it was asked for and returns what to tell the person.
    pub fn take(&mut self, ctx: &egui::Context, lang: &str) -> Option<&'static str> {
        std::mem::take(&mut self.pending).then(|| copy(ctx, lang))
    }
}

/// Each language's translations, in the order of [`STRINGS`].
const TRANSLATIONS: &[(&str, [&str; 2])] = &[
    ("es", ["Compartir Tinta Office", "Enlace a Tinta Office copiado. Pégalo en un mensaje o en un correo para compartirlo."]),
    ("ca", ["Comparteix Tinta Office", "S'ha copiat l'enllaç a Tinta Office. Enganxa'l en un missatge o en un correu per compartir-lo."]),
    ("gl", ["Compartir Tinta Office", "Copiouse a ligazón a Tinta Office. Pégaa nunha mensaxe ou nun correo para compartila."]),
    ("eu", ["Partekatu Tinta Office", "Tinta Officeren esteka kopiatu da. Itsatsi mezu batean edo posta elektroniko batean partekatzeko."]),
    ("uk", ["Поділитися Tinta Office", "Посилання на Tinta Office скопійовано. Вставте його в повідомлення чи лист, щоб поділитися."]),
    ("fr", ["Partager Tinta Office", "Lien vers Tinta Office copié. Collez-le dans un message ou un e-mail pour le partager."]),
    ("it", ["Condividi Tinta Office", "Link a Tinta Office copiato. Incollalo in un messaggio o in un'e-mail per condividerlo."]),
    ("de", ["Tinta Office teilen", "Link zu Tinta Office kopiert. Fügen Sie ihn in eine Nachricht oder E-Mail ein, um ihn zu teilen."]),
    ("pl", ["Udostępnij Tinta Office", "Skopiowano link do Tinta Office. Wklej go w wiadomości lub e-mailu, aby się nim podzielić."]),
    ("ro", ["Distribuie Tinta Office", "Linkul către Tinta Office a fost copiat. Lipește-l într-un mesaj sau într-un e-mail pentru a-l distribui."]),
    ("cs", ["Sdílet Tinta Office", "Odkaz na Tinta Office byl zkopírován. Vložte ho do zprávy nebo e-mailu a sdílejte ho."]),
    ("lt", ["Bendrinti Tinta Office", "Nuoroda į Tinta Office nukopijuota. Įklijuokite ją į žinutę ar el. laišką ir pasidalykite."]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copying_puts_the_link_on_the_clipboard() {
        let ctx = egui::Context::default();
        let mut said = "";
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| said = copy(ui.ctx(), "uk"));
        out.textures_delta.clear();
        let copied = out.platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == URL));
        assert!(copied, "{:?}", out.platform_output.commands);
        assert_ne!(said, COPIED, "the message is in the person's language");
    }

    #[test]
    fn a_request_copies_once() {
        let ctx = egui::Context::default();
        let mut share = Share::default();
        let mut said = Vec::new();
        share.request();
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| said.push(share.take(ui.ctx(), "es")));
            out.textures_delta.clear();
        }
        assert!(said[0].is_some() && said[1].is_none(), "{said:?}");
    }

    #[test]
    fn every_string_is_translated() {
        for (lang, t) in TRANSLATIONS {
            for (en, tr) in STRINGS.iter().zip(t) {
                assert!(!tr.is_empty() && tr != en, "{lang}: {en}");
                assert!(tr.contains("Tinta Office"), "{lang}: the brand stays as it is: {tr}");
            }
        }
        assert_eq!(tr("pt-BR", MENU), MENU);
        assert_eq!(tr("ca-valencia", MENU), tr("ca", MENU));
    }
}
