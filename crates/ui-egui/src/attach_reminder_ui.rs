//! The forgotten-attachment reminder (Tinta), as in Outlook: Send on a draft whose words say a
//! file is attached but which carries none ([`ibercorreo_engine::attach_reminder`]) asks first.
//! **Attach file** opens the file picker and keeps the draft open, **Send anyway** sends it, and
//! **Don't send** (also Escape) goes back to the draft. Assistants are not asked: `message.read`
//! tells them (`forgotAttachment`).

use egui::{Context, Id, Key, RichText, Ui};
use serde_json::json;

use ibercorreo_core::MessageId;

use crate::App;
use crate::chrome;
use crate::i18n::tl;

pub(crate) const TITLE: &str = "Did you forget the attachment?";
pub(crate) const ASK: &str = "Your message says a file is attached, but it has no attachment.";
pub(crate) const SEND_ANYWAY: &str = "Send anyway";
pub(crate) const DONT_SEND: &str = "Don't send";

/// The strings this module shows, for the translation test.
#[cfg(test)]
const STRINGS: &[&str] = &[TITLE, ASK, SEND_ANYWAY, DONT_SEND];

fn pending_id() -> Id {
    Id::new("tinta-attach-reminder")
}

/// Send was pressed on draft `id`, which promises a file it has not: ask.
pub(crate) fn ask(ctx: &Context, id: u64) {
    ctx.data_mut(|d| d.insert_temp(pending_id(), id));
}

/// The draft waiting for an answer, if any.
pub(crate) fn pending(ctx: &Context) -> Option<u64> {
    ctx.data(|d| d.get_temp::<u64>(pending_id()))
}

fn clear(ctx: &Context) {
    ctx.data_mut(|d| d.remove::<u64>(pending_id()));
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Answer {
    Attach,
    Send,
    Back,
}

impl App {
    /// The question, while one is open.
    pub(crate) fn attach_reminder_window(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let Some(id) = pending(&ctx) else { return };
        let t = self.tokens;
        let mut open = true;
        let mut answer: Option<Answer> = None;
        chrome::dialog(&ctx, &t, "attach-reminder", tl(TITLE), 460.0, &mut open, |ui, part| {
            use chrome::{Kind, Part};
            match part {
                Part::Footer => {
                    ui.horizontal(|ui| {
                        chrome::footer_right(ui, |ui| {
                            if chrome::button(ui, &t, Kind::Primary, None, crate::compose_files_ui::ATTACH, true).clicked() {
                                answer = Some(Answer::Attach);
                            }
                            if chrome::button(ui, &t, Kind::Secondary, None, SEND_ANYWAY, true).clicked() {
                                answer = Some(Answer::Send);
                            }
                            if chrome::button(ui, &t, Kind::Secondary, None, DONT_SEND, true).clicked() {
                                answer = Some(Answer::Back);
                            }
                        });
                    });
                }
                _ => {
                    ui.add(egui::Label::new(RichText::new(tl(ASK)).color(t.text)).wrap());
                }
            }
        });
        if ui.input(|i| i.key_pressed(Key::Escape)) || !open {
            answer = answer.or(Some(Answer::Back));
        }
        let Some(answer) = answer else { return };
        clear(&ctx);
        match answer {
            Answer::Attach => self.attach_wanted = Some(MessageId(id)),
            Answer::Send => {
                self.run("message.send", json!({ "id": id }));
            }
            Answer::Back => {}
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
    fn the_draft_waits_for_the_answer() {
        let ctx = Context::default();
        assert_eq!(pending(&ctx), None);
        ask(&ctx, 42);
        assert_eq!(pending(&ctx), Some(42));
        clear(&ctx);
        assert_eq!(pending(&ctx), None);
    }
}
