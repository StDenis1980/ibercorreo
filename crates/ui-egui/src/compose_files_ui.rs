//! Files attached to a draft (Tinta): Attach file beside Send, a chip for each file with its size
//! and a × that takes it off, and files dropped on the window while a draft is open. The window
//! only asks; the shell shows the system's file picker and reads the files
//! ([`ibercorreo_engine::attachments::read_to_attach`]), then attaches each one with
//! `message.attach`.

use egui::{CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use ibercorreo_core::MessageId;

use crate::App;
use crate::i18n::{tf, tl};
use crate::theme::Tokens;

pub(crate) const ATTACH: &str = "Attach file";
pub(crate) const ATTACH_TIP: &str = "Attach files to this message (or drop them on the window)";
pub(crate) const REMOVE_TIP: &str = "Remove this attachment";
pub(crate) const ATTACH_FAILED: &str = "Could not attach {name}: {error}";
pub(crate) const NO_PICKER: &str = "No file picker on this system: drop the files on the window instead";
pub(crate) const TOTAL: &str = "{count} files, {size}";
pub(crate) const ATTACHED: &str = "Attached {name}";
/// Said by `message.forward` (the engine) in the status bar, which translates it.
#[cfg(test)]
const LEFT_OUT: &str = "Some attachments are not kept on this computer and were left out";

#[cfg(test)]
const STRINGS: [&str; 8] = [ATTACH, ATTACH_TIP, REMOVE_TIP, ATTACH_FAILED, NO_PICKER, TOTAL, ATTACHED, LEFT_OUT];

impl App {
    /// The draft the person asked to attach files to since the last call. Taken once.
    pub fn take_attach_request(&mut self) -> Option<MessageId> {
        self.attach_wanted.take()
    }

    /// The draft being written, if one is open: files dropped on the window go to it.
    pub fn composing(&self) -> Option<MessageId> {
        self.draft.as_ref().map(|d| MessageId(d.id))
    }

    /// Attach a file the shell read to draft `id` (`name` and its contents).
    pub fn attach(&mut self, id: MessageId, name: &str, bytes: &[u8]) {
        match ibercorreo_engine::attachments::attach(&mut self.session, id, name, bytes) {
            Ok(()) => self.session.status = tf(ATTACHED, &[("name", name)]),
            Err(e) => self.attach_failed(name, &e.to_string()),
        }
    }

    /// Reading or attaching `name` failed.
    pub fn attach_failed(&mut self, name: &str, error: &str) {
        self.session.status = tf(ATTACH_FAILED, &[("name", name), ("error", error)]);
    }

    /// The system has no file picker to show.
    pub fn attach_no_picker(&mut self) {
        self.session.status = tl(NO_PICKER).to_string();
    }
}

/// The chips of the files on draft `id`, each with ×; returns the index the person took off.
pub(crate) fn chips(ui: &mut Ui, t: &Tokens, app: &App, id: MessageId) -> Option<usize> {
    let m = app.session.store.message(id).ok()?;
    if m.attachments.is_empty() {
        return None;
    }
    let mut removed = None;
    ui.horizontal_wrapped(|ui| {
        for (index, a) in m.attachments.iter().enumerate() {
            Frame::NONE.stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 4)).show(
                ui,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("📎 {}  ·  {}", a.name, crate::human_size(a.size))).small());
                        let x = egui::Button::new(RichText::new("×").small().color(t.text_weak)).frame(false);
                        if ui.add(x).on_hover_text(tl(REMOVE_TIP)).clicked() {
                            removed = Some(index);
                        }
                    });
                },
            );
        }
        if m.attachments.len() > 1 {
            let total = m.attachments.iter().map(|a| a.size).fold(0usize, usize::saturating_add);
            let count = m.attachments.len().to_string();
            ui.label(RichText::new(tf(TOTAL, &[("count", &count), ("size", &crate::human_size(total))])).small().color(t.text_weak));
        }
    });
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, has};

    /// Every string is translated in every language the app offers.
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
