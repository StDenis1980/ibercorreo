//! Repeating appointments in the calendar (Tinta), as in Outlook: one dragged to another time or
//! day asks whether only that occurrence moves or the whole series, and the agenda offers Edit
//! for this occurrence or for the series. The question comes from the window only: `event.move`
//! and `event.update` from a script, the CLI or MCP say which in their params.

use egui::{Key, RichText, Ui};
use ibercorreo_core::{Date, EventId};
use serde_json::json;

use crate::App;
use crate::chrome;
use crate::i18n::tl;

pub(crate) const TITLE: &str = "Move a repeating appointment";
pub(crate) const ASK: &str = "Move only this occurrence, or every one in the series?";
pub(crate) const THIS_ONE: &str = "This occurrence";
pub(crate) const SERIES: &str = "The whole series";
pub(crate) const EDIT: &str = "Edit appointment";
pub(crate) const EDIT_ONE: &str = "Edit this occurrence";
pub(crate) const EDIT_SERIES: &str = "Edit the series";
pub(crate) const EDIT_TITLE: &str = "Edit Appointment";

#[cfg(test)]
const STRINGS: &[&str] = &[TITLE, ASK, THIS_ONE, SERIES, EDIT, EDIT_ONE, EDIT_SERIES, EDIT_TITLE];

/// An occurrence of a series dropped somewhere, waiting for the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingMove {
    pub id: EventId,
    /// The occurrence's day, as the calendar shows it.
    pub occurrence: Date,
    /// Where it goes, as `event.move` takes it.
    pub start: String,
    pub end: Option<String>,
}

impl App {
    /// Move appointment `id` as dropped: at once, or after the question when it repeats.
    pub(crate) fn move_dropped(&mut self, id: EventId, occurrence: Date, start: String, end: Option<String>) {
        let repeats = self.session.store.event(id).is_ok_and(|e| e.rrule.is_some() && e.recurrence_id.is_none());
        if repeats {
            self.series_move = Some(PendingMove { id, occurrence, start, end });
            return;
        }
        let mut p = json!({ "id": id.0, "start": start });
        if let (Some(end), Some(o)) = (end, p.as_object_mut()) {
            o.insert("end".into(), json!(end));
        }
        self.run("event.move", p);
    }

    /// The question, while one is open: Enter moves this occurrence, Escape leaves it.
    pub(crate) fn series_window(&mut self, ui: &mut Ui) {
        let Some(pending) = self.series_move.clone() else { return };
        if self.session.store.event(pending.id).is_err() {
            self.series_move = None;
            return;
        }
        let t = self.tokens;
        let mut open = true;
        let mut answer: Option<Option<bool>> = None;
        chrome::dialog(ui.ctx(), &t, "series-move", tl(TITLE), 480.0, &mut open, |ui, part| {
            use chrome::{Kind, Part};
            match part {
                Part::Footer => {
                    ui.horizontal(|ui| {
                        chrome::footer_right(ui, |ui| {
                            if chrome::button(ui, &t, Kind::Primary, None, THIS_ONE, true).clicked() {
                                answer = Some(Some(false));
                            }
                            if chrome::button(ui, &t, Kind::Secondary, None, SERIES, true).clicked() {
                                answer = Some(Some(true));
                            }
                            if chrome::button(ui, &t, Kind::Secondary, None, "Cancel", true).clicked() {
                                answer = Some(None);
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
            answer = answer.or(Some(Some(false)));
        }
        if escape || !open {
            answer = answer.or(Some(None));
        }
        let Some(choice) = answer else { return };
        self.series_move = None;
        let Some(series) = choice else { return };
        let mut p = json!({ "id": pending.id.0, "start": pending.start, "occurrence": pending.occurrence.to_string(), "series": series });
        if let (Some(end), Some(o)) = (pending.end, p.as_object_mut()) {
            o.insert("end".into(), json!(end));
        }
        self.run("event.move", p);
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
}
