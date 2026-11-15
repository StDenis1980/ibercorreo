//! Colour categories in the window (Tinta): the Categorize menu (toolbar and a message's context
//! menu), the coloured names under a message's headers and dots in the message list, and the
//! Categories window, where they are made, renamed, recoloured and deleted. Everything goes
//! through the `category.*` and `message.categorize` commands.

use egui::{Color32, FontId, Key, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use ibercorreo_core::mail::{CATEGORY_COLORS, MAX_CATEGORY_NAME};
use ibercorreo_core::{Category, MessageId};
use serde_json::json;

use crate::App;
use crate::chrome;
use crate::i18n::tl;
use crate::icons::{self, Icon};
use crate::theme::{self, Tokens};

pub(crate) const CATEGORIZE: &str = "Categorize";
pub(crate) const MANAGE: &str = "Manage categories…";
pub(crate) const TITLE: &str = "Categories";
pub(crate) const NONE_YET: &str = "No categories yet. Name one below and choose its colour.";
pub(crate) const NEW_NAME: &str = "New category";
pub(crate) const ADD: &str = "Add";
pub(crate) const DELETE: &str = "Delete category";
pub(crate) const RENAME: &str = "Press Enter to rename";
/// The colours' names, as [`CATEGORY_COLORS`] orders them.
pub(crate) const COLOR_NAMES: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Teal", "Blue", "Purple", "Grey"];

/// The strings this module shows, for the translation test.
#[cfg(test)]
const STRINGS: &[&str] = &[
    CATEGORIZE,
    MANAGE,
    TITLE,
    NONE_YET,
    NEW_NAME,
    ADD,
    DELETE,
    RENAME,
    "Red",
    "Orange",
    "Yellow",
    "Green",
    "Teal",
    "Blue",
    "Purple",
    "Grey",
    "Category created",
    "Category changed",
    "Category deleted",
];

/// Our shades of the category colours: mid tones that read on light and dark themes alike.
const SHADES: [Color32; 8] = [
    Color32::from_rgb(0xd9, 0x4c, 0x45),
    Color32::from_rgb(0xe0, 0x82, 0x2f),
    Color32::from_rgb(0xc9, 0xa2, 0x08),
    Color32::from_rgb(0x3c, 0x9a, 0x55),
    Color32::from_rgb(0x1c, 0x98, 0x98),
    Color32::from_rgb(0x3a, 0x78, 0xd0),
    Color32::from_rgb(0x8e, 0x62, 0xc9),
    Color32::from_rgb(0x84, 0x88, 0x90),
];

/// The colour of a category's index; out of range is grey.
pub(crate) fn shade(index: u8) -> Color32 {
    SHADES.get(usize::from(index)).copied().unwrap_or(Color32::GRAY)
}

/// The colours of the categories `names` (in the store's list), in the message's order.
pub(crate) fn colors_of(all: &[Category], names: &[String]) -> Vec<Color32> {
    names.iter().map(|n| all.iter().find(|c| c.name.to_lowercase() == n.to_lowercase()).map_or(Color32::GRAY, |c| shade(c.color))).collect()
}

/// The Categories window's state.
#[derive(Default)]
pub(crate) struct CategoriesForm {
    pub(crate) open: bool,
    new_name: String,
    new_color: u8,
    /// The category being renamed, and the name typed so far.
    renaming: Option<(String, String)>,
}

/// One category in a menu: a tick when the message has it, its colour, its name.
fn category_item(ui: &mut Ui, t: &Tokens, name: &str, color: Color32, on: bool) -> bool {
    let galley = ui.painter().layout_no_wrap(name.to_string(), FontId::proportional(theme::BODY), t.text);
    let width = (galley.size().x + 64.0).max(ui.available_width()).max(220.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, t.accent_soft);
    }
    if on {
        icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(16.0, 16.0)), Icon::Check, t.accent);
    }
    ui.painter().circle_filled(pos2(rect.left() + 40.0, rect.center().y), 6.0, color);
    ui.painter().galley(pos2(rect.left() + 54.0, rect.center().y - galley.size().y / 2.0), galley, t.text);
    resp.clicked()
}

/// The names of the categories `names`, each on its colour, as a row under a message's headers.
pub(crate) fn pills(ui: &mut Ui, all: &[Category], names: &[String]) {
    if names.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        for (name, color) in names.iter().zip(colors_of(all, names)) {
            let text = if color.r() as u16 + color.g() as u16 + color.b() as u16 > 3 * 170 { Color32::BLACK } else { Color32::WHITE };
            egui::Frame::new().fill(color).corner_radius(10).inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
                ui.label(RichText::new(name).size(theme::CAPTION).color(text));
            });
        }
    });
}

/// Dots for the categories of a message in the list, right-aligned at `right`.
pub(crate) fn dots(painter: &egui::Painter, right: egui::Pos2, colors: &[Color32]) {
    let shown = colors.len().min(6);
    for (i, color) in colors.iter().take(shown).enumerate() {
        // The first category leftmost, as under the headers.
        #[allow(clippy::cast_precision_loss)]
        let x = right.x - 5.0 - (shown - 1 - i) as f32 * 13.0;
        painter.circle_filled(pos2(x, right.y), 5.0, *color);
    }
}

impl App {
    /// The Categorize menu for message `id`: each category, ticked when the message has it, then
    /// the Categories window.
    pub(crate) fn categorize_menu(&mut self, ui: &mut Ui, id: MessageId) {
        let t = self.tokens;
        let has: Vec<String> = self.session.store.message(id).map(|m| m.categories.clone()).unwrap_or_default();
        let all = self.session.store.categories.clone();
        ui.set_max_width(280.0);
        chrome::menu_title(ui, &t, tl(CATEGORIZE));
        for c in &all {
            let on = has.iter().any(|h| h.to_lowercase() == c.name.to_lowercase());
            if category_item(ui, &t, &c.name, shade(c.color), on) {
                self.run("message.categorize", json!({ "id": id.0, "category": c.name, "on": !on }));
                ui.close();
            }
        }
        if !all.is_empty() {
            ui.separator();
        }
        if chrome::menu_item(ui, &t, tl(MANAGE), false) {
            self.categories_form.open = true;
            ui.close();
        }
    }

    /// The Categories window, while it is open.
    pub(crate) fn categories_window(&mut self, ui: &mut Ui) {
        if !self.categories_form.open {
            return;
        }
        let t = self.tokens;
        let all = self.session.store.categories.clone();
        let mut open = true;
        let mut commands: Vec<(&'static str, serde_json::Value)> = Vec::new();
        let form = &mut self.categories_form;
        chrome::dialog(ui.ctx(), &t, "categories", tl(TITLE), 460.0, &mut open, |ui, part| {
            use chrome::{Kind, Part};
            if let Part::Footer = part {
                return;
            }
            if all.is_empty() {
                ui.label(RichText::new(tl(NONE_YET)).color(t.text_weak));
            }
            for c in &all {
                ui.horizontal(|ui| {
                    color_choice(ui, ("category-color", c.name.as_str()), c.color, |color| {
                        commands.push(("category.edit", json!({ "name": c.name, "color": color })));
                    });
                    let renaming = form.renaming.as_mut().filter(|(from, _)| *from == c.name);
                    match renaming {
                        Some((_, text)) => {
                            let edit = ui.add(egui::TextEdit::singleline(text).char_limit(MAX_CATEGORY_NAME).desired_width(260.0));
                            if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                commands.push(("category.edit", json!({ "name": c.name, "to": text.clone() })));
                            }
                            edit.request_focus();
                            edit.on_hover_text(tl(RENAME));
                        }
                        None => {
                            let label =
                                ui.add(egui::Label::new(RichText::new(&c.name).color(t.text)).sense(Sense::click())).on_hover_text(tl(RENAME));
                            if label.clicked() {
                                form.renaming = Some((c.name.clone(), c.name.clone()));
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if chrome::icon_action(ui, &t, Icon::Trash, tl(DELETE), true).clicked() {
                            commands.push(("category.delete", json!({ "name": c.name })));
                        }
                    });
                });
            }
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                let color = &mut form.new_color;
                color_choice(ui, "new-category-color", *color, |chosen| *color = chosen);
                let edit =
                    ui.add(egui::TextEdit::singleline(&mut form.new_name).hint_text(tl(NEW_NAME)).char_limit(MAX_CATEGORY_NAME).desired_width(240.0));
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let ready = !form.new_name.trim().is_empty();
                if (chrome::button(ui, &t, Kind::Primary, None, ADD, ready).clicked() || enter) && ready {
                    commands.push(("category.create", json!({ "name": form.new_name.trim(), "color": form.new_color })));
                    form.new_name.clear();
                    form.new_color = form.new_color.saturating_add(1) % 8;
                }
            });
        });
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            if self.categories_form.renaming.is_some() {
                self.categories_form.renaming = None;
            } else {
                open = false;
            }
        }
        for (id, params) in commands {
            if id == "category.edit" && params.get("to").is_some() {
                self.categories_form.renaming = None;
            }
            self.run(id, params);
        }
        if !open {
            self.categories_form = CategoriesForm::default();
        }
    }
}

/// A coloured square that opens the eight colours; `chosen` is called with the one picked.
fn color_choice(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, current: u8, mut chosen: impl FnMut(u8)) {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    ui.painter().rect_filled(rect.shrink(2.0), 5.0, shade(current));
    if resp.hovered() {
        ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, ui.visuals().strong_text_color()), egui::StrokeKind::Inside);
    }
    let resp = resp.on_hover_text(tl(COLOR_NAMES.get(usize::from(current)).copied().unwrap_or("Grey")));
    egui::Popup::menu(&resp).id(egui::Id::new(id)).show(|ui| {
        ui.horizontal(|ui| {
            for (i, name) in COLOR_NAMES.iter().enumerate().take(CATEGORY_COLORS.len()) {
                let Ok(index) = u8::try_from(i) else { continue };
                let (r, swatch) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                ui.painter().rect_filled(r.shrink(2.0), 5.0, shade(index));
                if index == current {
                    ui.painter().rect_stroke(r, 6.0, Stroke::new(2.0, ui.visuals().strong_text_color()), egui::StrokeKind::Inside);
                }
                if swatch.on_hover_text(tl(name)).clicked() {
                    chosen(index);
                    ui.close();
                }
            }
        });
    });
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
        assert_eq!(COLOR_NAMES.len(), CATEGORY_COLORS.len());
    }

    #[test]
    fn categories_show_in_their_colours() {
        let today = Date::new(2026, 11, 15).unwrap();
        let mut app = App::new(Session::new(ibercorreo_engine::sample::store(today).unwrap(), today));
        let id = app.session.store.all_messages().next().map(|m| m.id).unwrap();
        app.run("category.create", json!({ "name": "Facturas", "color": "green" }));
        app.run("message.categorize", json!({ "id": id.0, "category": "facturas" }));
        app.run("message.categorize", json!({ "id": id.0, "category": "Viaje" }));
        let m = app.session.store.message(id).unwrap();
        let colors = colors_of(&app.session.store.categories, &m.categories);
        assert_eq!(colors.first(), Some(&shade(3)), "green");
        assert_eq!(colors.len(), 2);
        assert_ne!(colors.first(), colors.get(1), "a new category takes a colour not used yet");
        assert_eq!(shade(200), Color32::GRAY, "out of range is grey, never a panic");
    }
}
