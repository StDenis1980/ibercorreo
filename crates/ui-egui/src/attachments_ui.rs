//! Attachments in the reading pane (Tinta): each one a chip with its name and size, and Open and
//! Save beside it. The window only asks; the shell writes the file
//! ([`ibercorreo_engine::attachments`]): Save to the Downloads folder under a free name, Open as a
//! copy handed to the program the system opens that kind of file with. A chip whose contents
//! Correo does not keep (mail that arrived before it kept them) shows the buttons greyed, with
//! the reason on hover, and a program or script shows no Open.

use egui::{CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use ibercorreo_core::{Message, MessageId};

use crate::App;
use crate::i18n::{tf, tl};
use crate::theme::Tokens;

pub(crate) const OPEN: &str = "Open";
pub(crate) const SAVE: &str = "Save";
pub(crate) const OPEN_TIP: &str = "Open it with the program this kind of file opens with";
pub(crate) const SAVE_TIP: &str = "Save a copy in the Downloads folder";
pub(crate) const NOT_KEPT: &str = "This attachment arrived before Tinta Correo began keeping attachments, so it can't be opened or saved here.";
pub(crate) const RISKY: &str = "This file could run a program: save it and check it before you open it.";
pub(crate) const SAVED: &str = "Saved {name} in {folder}";
pub(crate) const OPENING: &str = "Opening {name}…";
pub(crate) const FAILED: &str = "Could not save {name}: {error}";

#[cfg(test)]
const STRINGS: [&str; 9] = [OPEN, SAVE, OPEN_TIP, SAVE_TIP, NOT_KEPT, RISKY, SAVED, OPENING, FAILED];

/// What the person asked for with an attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentAction {
    Open,
    Save,
}

/// An attachment of a message and what to do with it, for the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachmentRequest {
    pub action: AttachmentAction,
    pub message: MessageId,
    /// Its place in [`Message::attachments`].
    pub index: usize,
}

impl App {
    /// The attachments the person asked to open or save since the last call. Taken once.
    pub fn take_attachment_requests(&mut self) -> Vec<AttachmentRequest> {
        std::mem::take(&mut self.attachment_requests)
    }

    /// The shell saved an attachment called `name` in `folder` (its name only, for the status bar).
    pub fn attachment_saved(&mut self, name: &str, folder: &str) {
        self.session.status = tf(SAVED, &[("name", name), ("folder", folder)]);
    }

    /// The shell handed `name` to the system to open.
    pub fn attachment_opening(&mut self, name: &str) {
        self.session.status = tf(OPENING, &[("name", name)]);
    }

    /// Saving or opening `name` failed.
    pub fn attachment_failed(&mut self, name: &str, error: &str) {
        self.session.status = tf(FAILED, &[("name", name), ("error", error)]);
    }
}

/// The chips of `m`'s attachments, each with Open and Save; a click goes to `requests`.
/// Tinta: the attachments in `shown` are pictures drawn in the text and get no chip.
pub(crate) fn chips(ui: &mut Ui, t: &Tokens, m: &Message, shown: &std::collections::HashSet<usize>, requests: &mut Vec<AttachmentRequest>) {
    ui.horizontal_wrapped(|ui| {
        for (index, a) in m.attachments.iter().enumerate().filter(|(i, _)| !shown.contains(i)) {
            let kept = a.sha256.is_some();
            let risky = ibercorreo_engine::attachments::risky(&a.name);
            Frame::NONE.stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 4)).show(
                ui,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("📎 {}  ·  {}", a.name, crate::human_size(a.size))).small());
                        let mut ask = |ui: &mut Ui, action: AttachmentAction, label: &str, tip: &str, enabled: bool| {
                            let button = egui::Button::new(RichText::new(tl(label)).small().color(t.accent)).frame(false);
                            let resp = ui.add_enabled(enabled, button);
                            let resp = if enabled { resp.on_hover_text(tl(tip)) } else { resp.on_disabled_hover_text(tl(NOT_KEPT)) };
                            if resp.clicked() {
                                requests.push(AttachmentRequest { action, message: m.id, index });
                            }
                        };
                        if risky {
                            ui.label(RichText::new("⚠").small().color(t.flag)).on_hover_text(tl(RISKY));
                        } else {
                            ask(ui, AttachmentAction::Open, OPEN, OPEN_TIP, kept);
                        }
                        ask(ui, AttachmentAction::Save, SAVE, SAVE_TIP, kept);
                    });
                },
            );
        }
    });
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
