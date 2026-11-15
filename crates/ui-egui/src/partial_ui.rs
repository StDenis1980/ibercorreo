//! Mail downloaded header first (Tinta): the choice of how many months of mail come whole
//! (Account settings › Downloading, `account.fullMail`), and, for an older message that came as
//! its header only, its text fetched as soon as it is opened (`message.download`).

use egui::{RichText, Ui};
use ibercorreo_core::Message;
use serde_json::json;

use crate::App;
use crate::chrome;
use crate::i18n::tl;
use crate::theme::Tokens;

pub(crate) const SECTION: &str = "Download whole messages";
pub(crate) const HOVER: &str = "Older mail comes as its header, and its text is downloaded when you open it. For IMAP accounts.";
pub(crate) const NOTE: &str = "Only the header of this message was downloaded. Its text is on the server.";
pub(crate) const COMING: &str = "Downloading the message…";
pub(crate) const DOWNLOAD: &str = "Download message";
/// The choices, in months (0: all mail), and their names.
pub(crate) const CHOICES: [(u32, &str); 6] =
    [(0, "All mail"), (1, "Last month"), (3, "Last 3 months"), (6, "Last 6 months"), (12, "Last year"), (24, "Last 2 years")];

/// The strings this module shows, for the translation test.
#[cfg(test)]
const STRINGS: &[&str] = &[
    SECTION,
    HOVER,
    NOTE,
    COMING,
    DOWNLOAD,
    "All mail",
    "Last month",
    "Last 3 months",
    "Last 6 months",
    "Last year",
    "Last 2 years",
    "All mail is downloaded whole",
    "Older mail is downloaded as its header, and its text when opened",
];

/// The name of a choice of months; one set by a script shows as a number.
fn label(months: u32) -> String {
    CHOICES.iter().find(|(m, _)| *m == months).map_or_else(|| months.to_string(), |(_, name)| tl(name).to_string())
}

/// The choice, in Account settings; returns the months chosen when they changed.
pub(crate) fn choice(ui: &mut Ui, current: u32) -> Option<u32> {
    let mut chosen = current;
    egui::ComboBox::from_id_salt("full-mail").selected_text(label(current)).show_ui(ui, |ui| {
        for (m, _) in CHOICES {
            ui.selectable_value(&mut chosen, m, label(m));
        }
    });
    (chosen != current).then_some(chosen)
}

impl App {
    /// Under the headers of a message that came as its header only: why the text is missing, and
    /// its download asked for once, when it is first opened.
    pub(crate) fn partial_note(&mut self, ui: &mut Ui, t: &Tokens, m: &Message) -> bool {
        if !m.partial {
            return false;
        }
        let wanted = self.session.bodies_wanted.contains(&m.id);
        if !wanted && self.body_asked != Some(m.id) {
            self.body_asked = Some(m.id);
            return true;
        }
        ui.add_space(8.0);
        ui.label(RichText::new(tl(NOTE)).color(t.text_weak));
        if wanted {
            ui.label(RichText::new(tl(COMING)).small().color(t.text_weak));
        } else if chrome::button(ui, t, chrome::Kind::Secondary, None, DOWNLOAD, true).clicked() {
            return true;
        }
        false
    }

    /// Ask for the text of message `m`.
    pub(crate) fn download_body(&mut self, m: ibercorreo_core::MessageId) {
        self.run("message.download", json!({ "id": m.0 }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, has};

    #[test]
    fn every_string_is_translated() {
        for lang in Language::ALL {
            if lang == Language::En {
                continue;
            }
            for s in STRINGS {
                assert!(has(lang, s), "{lang:?}: {s}");
            }
        }
        assert_eq!(label(7), "7", "a number set by a script still shows");
    }
}
