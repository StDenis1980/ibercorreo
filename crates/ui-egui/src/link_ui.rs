//! Opening a deceptive link asks first (Tinta). A link whose text names one site while it goes to
//! another is the shape of phishing ([`ibercorreo_core::rich::Span::deceptive`]): it is marked ⚠
//! and shows its real address on hover, and a click on it no longer opens it straight away. The
//! window shows the real address and opens it only when the person says so; Escape and Enter keep
//! it closed. Every other link opens as before.

use egui::{Context, Id, Key, RichText, Ui};

use crate::App;
use crate::chrome;
use crate::i18n::tl;

pub(crate) const TITLE: &str = "Open this link?";
pub(crate) const ASK: &str = "The text of this link names one site, but it goes to another address. Open it only if you trust the sender.";
pub(crate) const OPEN: &str = "Open link";

/// The strings this module shows, for the translation test.
#[cfg(test)]
const STRINGS: &[&str] = &[TITLE, ASK, OPEN];

fn pending_id() -> Id {
    Id::new("tinta-deceptive-link")
}

/// A click on a deceptive link: remember its address for the question (the body is drawn by free
/// functions with no access to the window's state).
pub(crate) fn ask(ctx: &Context, url: &str) {
    ctx.data_mut(|d| d.insert_temp(pending_id(), url.to_string()));
}

/// The address waiting for an answer, if any.
pub(crate) fn pending(ctx: &Context) -> Option<String> {
    ctx.data(|d| d.get_temp::<String>(pending_id()))
}

fn clear(ctx: &Context) {
    ctx.data_mut(|d| d.remove::<String>(pending_id()));
}

impl App {
    /// The question, while one is open.
    pub(crate) fn link_window(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let Some(url) = pending(&ctx) else { return };
        let t = self.tokens;
        let mut open = true;
        let mut answer: Option<bool> = None;
        chrome::dialog(&ctx, &t, "deceptive-link", tl(TITLE), 480.0, &mut open, |ui, part| {
            use chrome::{Kind, Part};
            match part {
                Part::Footer => {
                    ui.horizontal(|ui| {
                        chrome::footer_right(ui, |ui| {
                            // Keeping the link closed is the safe choice, so it is the primary one.
                            if chrome::button(ui, &t, Kind::Primary, None, "Cancel", true).clicked() {
                                answer = Some(false);
                            }
                            if chrome::button(ui, &t, Kind::Secondary, None, OPEN, true).clicked() {
                                answer = Some(true);
                            }
                        });
                    });
                }
                _ => {
                    ui.add(egui::Label::new(RichText::new(tl(ASK)).color(t.text)).wrap());
                    ui.add_space(8.0);
                    ui.add(egui::Label::new(RichText::new(&url).monospace().color(t.flag)).wrap());
                }
            }
        });
        let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        if enter || escape || !open {
            answer = answer.or(Some(false));
        }
        match answer {
            Some(true) => {
                clear(&ctx);
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
            Some(false) => clear(&ctx),
            None => {}
        }
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
    }

    #[test]
    fn a_deceptive_link_waits_for_the_answer() {
        let ctx = Context::default();
        assert_eq!(pending(&ctx), None);
        ask(&ctx, "https://bank.example.net/login");
        assert_eq!(pending(&ctx).as_deref(), Some("https://bank.example.net/login"));
        clear(&ctx);
        assert_eq!(pending(&ctx), None);
    }
}
