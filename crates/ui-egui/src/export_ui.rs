//! Export everything (M5.3): a button at the foot of the rail with a short menu that asks the
//! shell to write all mail, calendars and contacts to a new folder
//! ([`ibercorreo_engine::export::write_all`]), and opens that folder once it is written. The
//! shell does the writing on another thread (a mailbox runs to gigabytes) and says how far it got
//! in the status bar; this crate only asks and shows.

use std::path::PathBuf;

use egui::{RichText, Sense, Ui, vec2};

use crate::App;
use crate::chrome::{self, Kind};
use crate::i18n::tl;
use crate::icons::{self, Icon};

pub(crate) const TITLE: &str = "Export everything";
pub(crate) const WHAT: &str = "All mail (one .mbox file per folder), calendars (.ics) and contacts (.vcf) go to a new folder in Documents. Thunderbird, Apple Mail, Outlook and Google can import them. Nothing already there is overwritten.";
pub(crate) const ATTACHMENTS: &str = "Attachment files are not included: Tinta Correo keeps only their names.";
pub(crate) const EXPORT: &str = "Export";
pub(crate) const RUNNING: &str = "Exporting…";
pub(crate) const OPEN: &str = "Open the exported folder";

#[cfg(test)]
const STRINGS: [&str; 6] = [TITLE, WHAT, ATTACHMENTS, EXPORT, RUNNING, OPEN];

/// What the export button knows: whether the person asked, whether the shell is writing, and
/// where the last export went.
#[derive(Debug, Default)]
pub(crate) struct ExportState {
    wanted: bool,
    running: bool,
    last: Option<PathBuf>,
}

impl App {
    /// Whether the person asked to export everything since the last call. Taken once.
    pub fn take_export_request(&mut self) -> bool {
        std::mem::take(&mut self.export.wanted)
    }

    /// The shell started writing an export (the button waits).
    pub fn export_started(&mut self) {
        self.export.running = true;
    }

    /// The shell finished an export: `dir` is the folder written, `None` when it failed (the
    /// status bar says why).
    pub fn export_finished(&mut self, dir: Option<PathBuf>) {
        self.export.running = false;
        if dir.is_some() {
            self.export.last = dir;
        }
    }
}

/// The rail's export button and its menu.
pub(crate) fn rail_button(app: &mut App, ui: &mut Ui) {
    let t = app.tokens;
    let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    if open || resp.hovered() {
        ui.painter().rect_filled(rect, 10.0, t.accent_soft.gamma_multiply(0.7));
    }
    icons::paint(ui.painter(), rect.shrink(11.0), Icon::Outbox, t.text_weak);
    let resp = resp.on_hover_text(tl(TITLE));
    let (running, last) = (app.export.running, app.export.last.clone());
    let mut wanted = false;
    let mut open_last = None;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_max_width(380.0);
        chrome::menu_title(ui, &t, tl(TITLE));
        ui.add(egui::Label::new(RichText::new(tl(WHAT)).small().color(t.text_weak)).wrap());
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(tl(ATTACHMENTS)).small().color(t.text_weak)).wrap());
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let label = if running { RUNNING } else { EXPORT };
            if chrome::button(ui, &t, Kind::Primary, Some(Icon::Outbox), label, !running).clicked() {
                wanted = true;
            }
            if let Some(dir) = &last
                && chrome::button(ui, &t, Kind::Quiet, None, OPEN, true).clicked()
            {
                open_last = Some(dir.clone());
            }
        });
    });
    if wanted {
        app.export.wanted = true;
    }
    if let Some(dir) = open_last {
        app.links.push(format!("file://{}", dir.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, has};

    fn app() -> App {
        App::new(ibercorreo_engine::Session::new(ibercorreo_store::Store::new(), ibercorreo_core::Date::EPOCH))
    }

    /// A request is taken once; the button waits while the shell writes and offers the folder after.
    #[test]
    fn a_request_is_taken_once_and_the_folder_remembered() {
        let mut a = app();
        assert!(!a.take_export_request());
        a.export.wanted = true;
        assert!(a.take_export_request());
        assert!(!a.take_export_request(), "taken once");
        a.export_started();
        assert!(a.export.running);
        a.export_finished(Some(PathBuf::from("/tmp/Tinta Correo 2026-11-13")));
        assert_eq!((a.export.running, a.export.last.as_deref()), (false, Some(std::path::Path::new("/tmp/Tinta Correo 2026-11-13"))));
        a.export_started();
        a.export_finished(None);
        assert!(!a.export.running && a.export.last.is_some(), "a failed export keeps the last folder");
    }

    /// The button and its menu draw without a panic, in a frame with no window.
    #[test]
    fn the_button_draws() {
        let mut a = app();
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| rail_button(&mut a, ui));
        // No renderer here: the font atlas upload is dropped on purpose.
        out.textures_delta.clear();
        assert!(!a.take_export_request(), "drawing asks nothing");
    }

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
