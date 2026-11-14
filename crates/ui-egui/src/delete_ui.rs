//! Deleting in two steps (Tinta). Delete moves a message to Deleted Items, which can be undone
//! and keeps it on the server. Deleted Items is the last place: deleting there removes the
//! message for good, also from the server at the next Send/Receive, so the window asks first. The
//! question comes from the window only: `message.delete` run by a script, the CLI or MCP does what
//! it says, as every command does.

use egui::{Key, RichText, Ui};
use ibercorreo_core::{FolderRole, MessageId};
use serde_json::json;

use crate::App;
use crate::chrome;
use crate::i18n::tl;

pub(crate) const TITLE: &str = "Delete for good?";
pub(crate) const ASK: &str = "Deleting this message here removes it for good, also from the server at the next Send/Receive.";
pub(crate) const DELETE: &str = "Delete for good";

/// The strings this module shows, for the translation test.
#[cfg(test)]
const STRINGS: &[&str] = &[TITLE, ASK, DELETE];

impl App {
    /// Delete the open message as the person asked in the window: to Deleted Items at once, or,
    /// from Deleted Items, after the question.
    pub(crate) fn delete_asking(&mut self) {
        let Some(id) = self.session.message else {
            self.run("message.delete", json!({}));
            return;
        };
        if self.in_deleted_items(id) {
            self.confirm_delete = Some(id);
        } else {
            self.run("message.delete", json!({ "id": id.0 }));
        }
    }

    fn in_deleted_items(&self, id: MessageId) -> bool {
        let store = &self.session.store;
        store.message(id).ok().and_then(|m| store.folder(m.folder).ok()).is_some_and(|f| f.role == FolderRole::Trash)
    }

    /// The question, while one is open. Enter deletes, Escape keeps the message.
    pub(crate) fn delete_window(&mut self, ui: &mut Ui) {
        let Some(id) = self.confirm_delete else { return };
        // The message went away meanwhile (a sync, an undo): nothing to ask about.
        if self.session.store.message(id).is_err() {
            self.confirm_delete = None;
            return;
        }
        let t = self.tokens;
        let mut open = true;
        let mut answer: Option<bool> = None;
        chrome::dialog(ui.ctx(), &t, "delete-for-good", tl(TITLE), 460.0, &mut open, |ui, part| {
            use chrome::{Kind, Part};
            match part {
                Part::Footer => {
                    ui.horizontal(|ui| {
                        chrome::footer_right(ui, |ui| {
                            if chrome::button(ui, &t, Kind::Primary, None, DELETE, true).clicked() {
                                answer = Some(true);
                            }
                            if chrome::button(ui, &t, Kind::Secondary, None, "Cancel", true).clicked() {
                                answer = Some(false);
                            }
                        });
                    });
                }
                _ => {
                    ui.add(egui::Label::new(RichText::new(tl(ASK)).color(t.text)).wrap());
                }
            }
        });
        let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        if enter {
            answer = answer.or(Some(true));
        }
        if escape || !open {
            answer = answer.or(Some(false));
        }
        match answer {
            Some(true) => {
                self.confirm_delete = None;
                self.run("message.delete", json!({ "id": id.0 }));
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, has};
    use ibercorreo_core::Date;
    use ibercorreo_engine::Session;

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
    fn deleting_from_deleted_items_asks_first_and_elsewhere_does_not() {
        let today = Date::new(2026, 11, 15).unwrap();
        let mut app = App::new(Session::new(ibercorreo_engine::sample::store(today).unwrap(), today));
        let id = app.session.store.all_messages().next().map(|m| m.id).unwrap();
        app.session.message = Some(id);
        app.delete_asking();
        assert!(app.confirm_delete.is_none(), "to Deleted Items without a question");
        assert!(app.in_deleted_items(id), "moved to Deleted Items");
        app.session.message = Some(id);
        app.delete_asking();
        assert_eq!(app.confirm_delete, Some(id), "from Deleted Items it asks");
        assert!(app.session.store.message(id).is_ok(), "nothing gone before the answer");
    }
}
