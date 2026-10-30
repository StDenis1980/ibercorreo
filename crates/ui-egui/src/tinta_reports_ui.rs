//! Tinta Office: the first-start question about automatic error reports, and the status bar's
//! Error reports menu that changes it ([`ibercorreo_store::tinta_reports`] keeps the choice in the
//! store, `file.errorReports` changes it; the desktop app writes and sends the reports). Both show
//! only in builds that can send reports: the desktop app sets [`App::error_reports_dir`] then.

use egui::{RichText, Ui};
use serde_json::json;

use crate::App;
use crate::i18n::{tf, tl};

pub(crate) const TITLE: &str = "Send error reports automatically?";
pub(crate) const WHAT: &str = "When the app crashes or stops responding, it can send a report so the problem gets fixed without you writing to us. A report holds the app version, the system, the language, where in the code the error happened and the last lines of the technical log. Never the text of your documents, file names, folders, e-mail addresses or passwords.";
pub(crate) const RIGHTS: &str = "Reports go to Tinta Office (report.tintaoffice.com, stored in the EU) and are deleted after 90 days. You can change this at any time from “Error reports” in the status bar and delete your reports from the server there. Details: tintaoffice.com/aviso-legal";
pub(crate) const YES: &str = "Yes, send reports";
pub(crate) const NO: &str = "No";
pub(crate) const SECTION: &str = "Error reports";
pub(crate) const CHECKBOX: &str = "Send error reports automatically";
pub(crate) const ID: &str = "Installation id: {id}";
pub(crate) const FORGET: &str = "Delete my reports from the server";
pub(crate) const PENDING: &str = "Deletion requested: it completes the next time the app is online.";
pub(crate) const FOLDER: &str = "Open the reports folder";

#[cfg(test)]
const STRINGS: [&str; 11] = [TITLE, WHAT, RIGHTS, YES, NO, SECTION, CHECKBOX, ID, FORGET, PENDING, FOLDER];

/// Whether the question is due: a build that sends, and no answer yet.
pub(crate) fn asks(app: &App) -> bool {
    app.error_reports_dir.is_some() && app.session.store.error_reports.consent.is_none()
}

/// The question, over everything, until it is answered once.
pub(crate) fn first_start(app: &mut App, ctx: &egui::Context) {
    if !asks(app) {
        return;
    }
    egui::Modal::new(egui::Id::new("tinta.errorReports")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.label(RichText::new(tl(TITLE)).strong().size(17.0));
        ui.add_space(8.0);
        ui.label(tl(WHAT));
        ui.add_space(6.0);
        ui.label(RichText::new(tl(RIGHTS)).small().weak());
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button(tl(YES)).clicked() {
                app.run("file.errorReports", json!({"value": true}));
            }
            if ui.button(tl(NO)).clicked() {
                app.run("file.errorReports", json!({"value": false}));
            }
        });
    });
}

/// The status bar's Error reports menu.
pub(crate) fn menu(app: &mut App, ui: &mut Ui) {
    let Some(dir) = app.error_reports_dir.clone() else { return };
    let r = app.session.store.error_reports.clone();
    let mut action = None;
    ui.menu_button(RichText::new(tl(SECTION)).small(), |ui| {
        ui.set_max_width(420.0);
        let mut on = r.consent == Some(true);
        if ui.checkbox(&mut on, tl(CHECKBOX)).changed() {
            action = Some(json!({"value": on}));
        }
        ui.label(RichText::new(tl(WHAT)).small().weak());
        ui.label(RichText::new(tl(RIGHTS)).small().weak());
        if !r.install_id.is_empty() {
            ui.label(RichText::new(tf(ID, &[("id", &r.install_id)])).small().monospace());
        }
        if ui.add_enabled(!r.install_id.is_empty(), egui::Button::new(tl(FORGET))).clicked() {
            action = Some(json!({"forget": true}));
        }
        if r.forget_pending.is_some() {
            ui.label(RichText::new(tl(PENDING)).small().weak());
        }
        if ui.button(tl(FOLDER)).clicked() {
            let _ = std::fs::create_dir_all(&dir);
            ui.ctx().open_url(egui::OpenUrl::new_tab(format!("file://{}", dir.display())));
        }
    });
    if let Some(params) = action {
        app.run("file.errorReports", params);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, has};

    fn app(dir: Option<std::path::PathBuf>) -> App {
        let session = ibercorreo_engine::Session::new(ibercorreo_store::Store::new(), ibercorreo_core::Date::EPOCH);
        let mut app = App::new(session);
        app.error_reports_dir = dir;
        app
    }

    /// Only a build that sends asks, and only until the question is answered.
    #[test]
    fn asks_once_and_only_in_builds_that_send() {
        assert!(!asks(&app(None)));
        let mut a = app(Some(std::env::temp_dir().join("tinta-reports-test")));
        assert!(asks(&a));
        a.run("file.errorReports", json!({"value": false}));
        assert!(!asks(&a));
        assert!(a.session.dirty, "the answer is saved with the mailbox");
    }

    /// The question and the menu draw without a panic, in a frame with no window.
    #[test]
    fn the_question_and_the_menu_draw() {
        let mut a = app(Some(std::env::temp_dir().join("tinta-reports-test")));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            first_start(&mut a, ui.ctx());
            menu(&mut a, ui);
        });
        // No renderer here: the font atlas upload is dropped on purpose.
        out.textures_delta.clear();
        assert!(asks(&a));
    }

    /// Every string is translated in every language the app offers.
    #[test]
    fn every_string_is_translated() {
        for lang in Language::ALL {
            if lang == Language::En {
                continue;
            }
            for s in STRINGS {
                assert!(has(lang, s), "{}: {s:?}", lang.code());
            }
        }
    }
}
