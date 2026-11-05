//! The window around the views, in the manner of a modern desktop mail app: the theme's wash at
//! the top, a title bar with the search box in the middle, a light rail of views on the left,
//! toolbars of icon-and-label buttons, white cards for the content, and My Day (a month and the
//! coming days' events) on the right. Everything acts through commands (`App::run`).

use egui::{Align, Align2, CornerRadius, FontId, Frame, Layout, Margin, Rect, Response, RichText, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use ibercorreo_core::Date;
use ibercorreo_core::calendar::When;
use ibercorreo_engine::{CalendarMode, View};
use serde_json::json;

use crate::i18n::tl;
use crate::icons::{self, Icon};
use crate::theme::{self, Tokens};
use crate::{App, EventForm, long_date, month_title};

/// The title bar's height; toolbars are as tall.
pub(crate) const BAR_H: f32 = 48.0;
/// A toolbar button's height (Fluent's standard control is 32).
const TOOL_H: f32 = 34.0;
/// A toolbar button that shows only its icon.
pub(crate) const TOOL_ICON_W: f32 = 36.0;
/// The rail's width.
pub(crate) const RAIL_W: f32 = 60.0;
/// How far down the theme's wash reaches before it is the window's colour.
const BAND_H: f32 = 180.0;
/// The gap around cards.
pub(crate) const GAP: i8 = 10;
/// Days My Day looks ahead for events.
const MY_DAY_DAYS: i64 = 7;

/// A card: the white (or dark) rounded panel the list, the message, My Day and the calendar sit on.
pub(crate) fn card(t: &Tokens, inner: i8) -> Frame {
    Frame::NONE
        .fill(t.card)
        .corner_radius(CornerRadius::same(12))
        .stroke(Stroke::new(1.0, t.border.gamma_multiply(0.6)))
        .inner_margin(Margin::same(inner))
        .outer_margin(Margin { left: 0, right: GAP, top: 0, bottom: GAP })
}

/// Paints the window: its colour, with the theme's wash fading in from the top.
pub(crate) fn paint_window(ctx: &egui::Context, t: &Tokens) {
    let screen = ctx.content_rect();
    let p = ctx.layer_painter(egui::LayerId::background());
    p.rect_filled(screen, 0.0, t.window);
    let band = Rect::from_min_max(screen.min, pos2(screen.max.x, screen.min.y + BAND_H.min(screen.height())));
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(band.left_top(), t.band);
    mesh.colored_vertex(band.right_top(), t.band);
    mesh.colored_vertex(band.left_bottom(), t.window);
    mesh.colored_vertex(band.right_bottom(), t.window);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    p.add(egui::Shape::mesh(mesh));
}

impl App {
    /// A square button with an icon: hover and `on` shade it.
    pub(crate) fn icon_button(&self, ui: &mut Ui, icon: Icon, tip: &str, on: bool) -> Response {
        let t = self.tokens;
        let (rect, resp) = ui.allocate_exact_size(vec2(TOOL_H, TOOL_H), Sense::click());
        if on || resp.hovered() {
            ui.painter().rect_filled(rect, 8.0, if on { t.accent_soft } else { t.accent_soft.gamma_multiply(0.6) });
        }
        icons::paint(ui.painter(), rect.shrink(8.0), icon, t.accent);
        resp.on_hover_text(tl(tip))
    }

    /// How wide a toolbar button with a label `text_w` wide is.
    fn tool_width(text_w: f32) -> f32 {
        8.0 + 18.0 + 6.0 + text_w + 10.0
    }

    /// How wide toolbar buttons with these labels are, side by side, labels shown.
    pub(crate) fn tools_width(&self, ui: &Ui, labels: &[&str]) -> f32 {
        labels
            .iter()
            .map(|l| Self::tool_width(ui.painter().layout_no_wrap(tl(l).to_string(), FontId::proportional(theme::BODY), self.tokens.text).size().x))
            .sum()
    }

    /// A toolbar button: an icon and a label; greyed and inert when not `enabled`.
    pub(crate) fn tool(&self, ui: &mut Ui, icon: Icon, label: &str, enabled: bool) -> Response {
        let t = self.tokens;
        let color = if enabled { t.text } else { t.text_weak.gamma_multiply(0.6) };
        let galley = ui.painter().layout_no_wrap(tl(label).to_string(), FontId::proportional(theme::BODY), color);
        let compact = self.compact_tools;
        let size = if compact { vec2(TOOL_ICON_W, TOOL_H) } else { vec2(Self::tool_width(galley.size().x), TOOL_H) };
        let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
        if enabled && resp.hovered() {
            ui.painter().rect_filled(rect, 8.0, t.accent_soft);
        }
        let icon_x = if compact { rect.center().x } else { rect.left() + 17.0 };
        let icon_rect = Rect::from_center_size(pos2(icon_x, rect.center().y), vec2(18.0, 18.0));
        let icon_color = if icon == Icon::Flag && enabled { t.flag } else { color };
        icons::paint(ui.painter(), icon_rect, icon, icon_color);
        if compact {
            return resp.on_hover_text(tl(label));
        }
        ui.painter().galley(pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0), galley, color);
        resp
    }

    /// The New button at the top of a left pane (New email, New appointment, New contact): a
    /// white button with the accent's icon and label.
    pub(crate) fn new_button(&self, ui: &mut Ui, icon: Icon, label: &str, compact: bool) -> Response {
        let t = self.tokens;
        let galley = ui.painter().layout_no_wrap(tl(label).to_string(), FontId::proportional(theme::BODY), t.accent);
        let w = if compact { 40.0 } else { 14.0 + 18.0 + 8.0 + galley.size().x + 16.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(w, TOOL_H), Sense::click());
        let fill = if resp.hovered() { theme::mix(t.card, t.accent, 0.08) } else { t.card };
        ui.painter().rect(rect, 8.0, fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        let ix = if compact { rect.center().x } else { rect.left() + 23.0 };
        icons::paint(ui.painter(), Rect::from_center_size(pos2(ix, rect.center().y), vec2(17.0, 17.0)), icon, t.accent);
        if !compact {
            let y = rect.center().y - galley.size().y / 2.0;
            ui.painter().galley(pos2(rect.left() + 40.0, y), galley, t.accent);
        }
        if compact { resp.on_hover_text(tl(label)) } else { resp }
    }

    /// The left pane's head: the button that hides the pane, and the New button.
    pub(crate) fn pane_head(&mut self, ui: &mut Ui, icon: Icon, label: &str) -> bool {
        let mut new = false;
        ui.allocate_ui_with_layout(vec2(ui.available_width(), BAR_H), Layout::left_to_right(Align::Center), |ui| {
            if self.icon_button(ui, Icon::Menu, "Hide the folder pane", false).clicked() {
                self.run("view.folderPane", json!({ "visible": false }));
            }
            ui.add_space(6.0);
            new = self.new_button(ui, icon, label, false).clicked();
        });
        ui.add_space(4.0);
        new
    }

    /// With the left pane hidden, its two buttons start the toolbar: show the pane, New.
    pub(crate) fn hidden_pane_head(&mut self, ui: &mut Ui, icon: Icon, label: &str) -> bool {
        if !self.session.store.hide_folder_pane {
            return false;
        }
        if self.icon_button(ui, Icon::Menu, "Show the folder pane", false).clicked() {
            self.run("view.folderPane", json!({ "visible": true }));
        }
        let new = self.new_button(ui, icon, label, true).clicked();
        ui.add_space(12.0);
        new
    }

    /// The title bar: Undo and Redo on the left, the search box in the middle, the theme and My
    /// Day on the right.
    pub(crate) fn title_bar(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let (full, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
        // Room on the right for five buttons: feedback, error reports, language, theme, My Day.
        let search_w = (full.width() - 460.0).clamp(160.0, 640.0);
        let search = Rect::from_center_size(full.center(), vec2(search_w, 34.0));
        let left = Rect::from_min_max(pos2(full.left() + 12.0, full.top()), pos2(search.left() - 12.0, full.bottom()));
        ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| {
            if self.icon_button(ui, Icon::Undo, "Undo (Ctrl+Z)", false).clicked() && self.session.can_undo() {
                self.run("edit.undo", json!({}));
            }
            if self.icon_button(ui, Icon::Redo, "Redo (Ctrl+Y)", false).clicked() && self.session.can_redo() {
                self.run("edit.redo", json!({}));
            }
        });
        ui.scope_builder(UiBuilder::new().max_rect(search), |ui| {
            let frame = Frame::NONE
                .fill(t.card.gamma_multiply(0.85))
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(12, 7));
            frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    icons::paint(ui.painter(), r, Icon::Search, t.accent);
                    let edit = egui::TextEdit::singleline(&mut self.search)
                        .hint_text(tl("Search mail"))
                        .frame(Frame::NONE)
                        .desired_width(ui.available_width());
                    let r = ui.add(edit).on_hover_text(tl(crate::SEARCH_HELP));
                    if r.changed() {
                        if self.session.view != View::Mail {
                            self.run("view.mail", json!({}));
                        }
                        let q = self.search.clone();
                        self.run("search.messages", json!({ "query": q }));
                    }
                });
            });
        });
        let right = Rect::from_min_max(pos2(search.right() + 12.0, full.top()), pos2(full.right() - 12.0, full.bottom()));
        ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
            if self.session.view != View::Contacts {
                let shown = !self.session.store.hide_my_day;
                if self.icon_button(ui, Icon::PanelRight, if shown { "Hide My Day" } else { "Show My Day" }, shown).clicked() {
                    self.run("view.myDay", json!({}));
                }
            }
            self.theme_menu(ui);
            self.language_menu(ui);
            crate::tinta_reports_ui::menu(self, ui);
            self.feedback_button(ui);
        });
    }

    /// An icon button, like [`App::icon_button`], that opens a menu with `add` in it.
    pub(crate) fn icon_menu(&self, ui: &mut Ui, icon: Icon, tip: &str, add: impl FnOnce(&mut Ui)) {
        let t = self.tokens;
        let (rect, resp) = ui.allocate_exact_size(vec2(TOOL_H, TOOL_H), Sense::click());
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
        if open || resp.hovered() {
            ui.painter().rect_filled(rect, 8.0, t.accent_soft);
        }
        icons::paint(ui.painter(), rect.shrink(8.0), icon, t.accent);
        let resp = resp.on_hover_text(tip);
        egui::Popup::menu(&resp).show(add);
    }

    /// Send feedback (Tinta Office's dialog), in builds that can send.
    fn feedback_button(&mut self, ui: &mut Ui) {
        if !self.tinta_feedback.available {
            return;
        }
        let tip = crate::tinta_feedback::tr(crate::i18n::current().code(), crate::tinta_feedback::BUTTON);
        if self.icon_button(ui, Icon::Smile, tip, false).clicked() {
            self.tinta_feedback.start(ui.ctx());
        }
    }

    /// The theme picker: the six themes as swatches, and light, dark or the computer's choice.
    fn theme_menu(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let dark = ui.visuals().dark_mode;
        let chosen = self.session.store.theme.clone();
        let mode = self.session.store.appearance.clone();
        let mut pick: Option<(Option<&str>, Option<&str>)> = None;
        let mut zoom: Option<serde_json::Value> = None;
        let (rect, resp) = ui.allocate_exact_size(vec2(TOOL_H, TOOL_H), Sense::click());
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
        if open || resp.hovered() {
            ui.painter().rect_filled(rect, 8.0, t.accent_soft);
        }
        icons::paint(ui.painter(), rect.shrink(8.0), Icon::Palette, t.accent);
        let resp = resp.on_hover_text(tl("Theme"));
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(320.0);
            ui.label(RichText::new(tl("Theme")).size(theme::SUBTITLE).strong().color(t.text));
            ui.add_space(8.0);
            egui::Grid::new("themes").spacing(vec2(8.0, 8.0)).show(ui, |ui| {
                for (i, id) in theme::ids().enumerate() {
                    let (band, accent) = Tokens::swatch(id, dark);
                    let on = chosen.as_deref().unwrap_or("teal") == id;
                    let (cell, r) = ui.allocate_exact_size(vec2(96.0, 72.0), Sense::click());
                    let p = ui.painter();
                    if on || r.hovered() {
                        p.rect_stroke(cell, 8.0, Stroke::new(if on { 2.0 } else { 1.0 }, accent), egui::StrokeKind::Inside);
                    }
                    let chip = Rect::from_center_size(pos2(cell.center().x, cell.top() + 24.0), vec2(64.0, 34.0));
                    p.rect(chip, 6.0, band, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                    p.circle_filled(chip.right_bottom() - vec2(10.0, 10.0), 6.0, accent);
                    p.text(
                        pos2(cell.center().x, cell.bottom() - 11.0),
                        Align2::CENTER_CENTER,
                        tl(theme_name(id)),
                        FontId::proportional(13.0),
                        t.text,
                    );
                    if r.on_hover_text(tl(theme_name(id))).clicked() {
                        pick = Some((Some(id), None));
                    }
                    if i % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new(tl("Mode")).color(t.text_weak));
                for (value, label) in [("light", "Light"), ("dark", "Dark"), ("system", "Same as the computer")] {
                    let on = mode.as_deref().unwrap_or("system") == value;
                    if ui.selectable_label(on, tl(label)).clicked() {
                        pick = Some((None, Some(value)));
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(tl("Size")).color(t.text_weak));
                if ui.button("−").on_hover_text(tl("Smaller (Ctrl+-)")).clicked() {
                    zoom = Some(json!({ "step": -1 }));
                }
                ui.label(RichText::new(format!("{} %", self.session.store.zoom())).color(t.text));
                if ui.button("+").on_hover_text(tl("Larger (Ctrl+=)")).clicked() {
                    zoom = Some(json!({ "step": 1 }));
                }
                if self.session.store.zoom.is_some() && ui.button(tl("Reset")).on_hover_text("Ctrl+0").clicked() {
                    zoom = Some(json!({ "reset": true }));
                }
            });
        });
        if let Some(p) = zoom {
            self.run("view.zoom", p);
        }
        if let Some((theme, mode)) = pick {
            let mut p = serde_json::Map::new();
            if let Some(th) = theme {
                p.insert("theme".into(), json!(th));
            }
            if let Some(m) = mode {
                p.insert("mode".into(), json!(m));
            }
            self.run("view.theme", serde_json::Value::Object(p));
        }
    }

    /// The rail: the app's mark, then Mail, Calendar and People; Account settings at the foot.
    pub(crate) fn rail(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        ui.vertical_centered(|ui| {
            ui.add_space(4.0);
            let (mark, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            ui.painter().rect_filled(mark, 10.0, t.accent);
            icons::paint(ui.painter(), mark.shrink(8.0), Icon::Mail, t.on_accent);
            ui.add_space(16.0);
            for (icon, view, id, tip) in [
                (Icon::Mail, View::Mail, "view.mail", "Mail (Ctrl+1)"),
                (Icon::Calendar, View::Calendar, "view.calendar", "Calendar (Ctrl+2)"),
                (Icon::People, View::Contacts, "view.contacts", "People (Ctrl+3)"),
            ] {
                let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
                let active = self.session.view == view;
                if active {
                    ui.painter().rect(rect, 10.0, t.card, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 10.0, t.accent_soft.gamma_multiply(0.7));
                }
                icons::paint(ui.painter(), rect.shrink(11.0), icon, if active { t.accent } else { t.text_weak });
                if resp.on_hover_text(tl(tip)).clicked() {
                    self.run(id, json!({}));
                }
                ui.add_space(8.0);
            }
            let space = ui.available_height() - 52.0;
            if space > 0.0 {
                ui.add_space(space);
            }
            let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(rect, 10.0, t.accent_soft.gamma_multiply(0.7));
            }
            icons::paint(ui.painter(), rect.shrink(11.0), Icon::Gear, t.text_weak);
            if resp.on_hover_text(tl("The settings of the account whose folder is open")).clicked() {
                let current = (!self.session.store.accounts.is_empty()).then(|| self.session.current_account());
                self.open_account_settings(current, "");
            }
        });
    }

    /// My Day: a month to pick a day from, and the next days' events.
    pub(crate) fn my_day(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl("My Day")).size(theme::SUBTITLE).strong().color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.icon_button(ui, Icon::Plus, "New Appointment", false).clicked() {
                    self.event_form = EventForm { open: true, start: format!("{}T09:00", self.session.today()), ..EventForm::default() };
                }
            });
        });
        ui.add_space(8.0);
        self.mini_month(ui, true);
        ui.add_space(10.0);
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.coming_days(ui));
    }

    /// A month of days: today in the accent, the day shown in the calendar shaded; a click shows
    /// that day in the calendar (`to_calendar` also opens the calendar).
    pub(crate) fn mini_month(&mut self, ui: &mut Ui, to_calendar: bool) {
        let t = self.tokens;
        let day = self.session.day;
        let today = self.session.today();
        let first = day.first_of_month();
        let mut goto: Option<Date> = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(month_title(first)).strong().color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let small = |ui: &mut Ui, icon: Icon, tip: &str| {
                    let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 6.0, t.accent_soft);
                    }
                    icons::paint(ui.painter(), r.shrink(6.0), icon, t.text_weak);
                    resp.on_hover_text(tl(tip)).clicked()
                };
                if small(ui, Icon::ChevronRight, "Next month") {
                    goto = first.add_months(1).ok();
                }
                if small(ui, Icon::ChevronLeft, "Previous month") {
                    goto = first.add_months(-1).ok();
                }
            });
        });
        let Ok(start) = first.add_days(-i64::from(first.weekday().index())) else { return };
        let cell = ((ui.available_width() / 7.0).floor()).clamp(28.0, 40.0);
        let (grid, _) = ui.allocate_exact_size(vec2(cell * 7.0, cell * 7.0), Sense::hover());
        let font = FontId::proportional(13.0);
        for (i, name) in crate::WEEKDAYS.iter().enumerate() {
            let c = pos2(grid.left() + cell * (i as f32 + 0.5), grid.top() + cell * 0.5);
            let letter: String = tl(name).chars().take(2).collect();
            ui.painter().text(c, Align2::CENTER_CENTER, letter, font.clone(), t.text_weak);
        }
        let week_mode = matches!(self.session.calendar_mode, CalendarMode::Week | CalendarMode::WorkWeek);
        for n in 0..42i64 {
            let Ok(d) = start.add_days(n) else { continue };
            let (row, col) = (n / 7, n % 7);
            if row == 5 && d.month() != first.month() && col == 0 {
                break;
            }
            let rect = Rect::from_min_size(pos2(grid.left() + cell * col as f32, grid.top() + cell * (row as f32 + 1.0)), vec2(cell, cell));
            let resp = ui.interact(rect, ui.id().with(("mini-month", n)), Sense::click());
            let in_month = d.month() == first.month();
            let shown = !to_calendar && (d == day || (week_mode && same_week(d, day)));
            let p = ui.painter();
            if shown {
                p.rect_filled(rect.shrink(1.0), 4.0, t.accent_soft);
            } else if resp.hovered() {
                p.circle_filled(rect.center(), cell * 0.42, t.accent_soft);
            }
            let color = if d == today {
                p.circle_filled(rect.center(), cell * 0.42, t.today);
                t.on_accent
            } else if in_month {
                t.text
            } else {
                t.text_weak.gamma_multiply(0.7)
            };
            p.text(rect.center(), Align2::CENTER_CENTER, d.day().to_string(), font.clone(), color);
            if resp.clicked() {
                goto = Some(d);
                if to_calendar {
                    self.run("view.calendar", json!({}));
                }
            }
        }
        if let Some(d) = goto {
            self.run("calendar.goto", json!({ "date": d.to_string() }));
        }
    }

    /// The events of today and the next days, a heading for each day that has some.
    fn coming_days(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        let today = self.session.today();
        let zone = self.session.zone().clone();
        let mut any = false;
        for n in 0..MY_DAY_DAYS {
            let Ok(d) = today.add_days(n) else { break };
            let events: Vec<_> = self
                .session
                .store
                .events_on(d, &zone)
                .into_iter()
                .map(|o| (o.event.calendar, o.event.summary.clone(), o.event.location.clone(), o.when))
                .collect();
            if events.is_empty() && n > 0 {
                continue;
            }
            any = true;
            let heading = match n {
                0 => format!("{} • {}", tl("Today"), long_date(d)),
                1 => format!("{} • {}", tl("Tomorrow"), long_date(d)),
                _ => long_date(d),
            };
            ui.add_space(12.0);
            ui.label(RichText::new(heading).small().strong().color(if n == 0 { t.accent } else { t.text }));
            if events.is_empty() {
                ui.label(RichText::new(tl("No appointments")).small().color(t.text_weak));
            }
            for (calendar, summary, location, when) in events {
                let (time, length) = match when {
                    When::AllDay { .. } => (tl("All day").to_string(), String::new()),
                    When::Timed { start, end, .. } => {
                        let minutes =
                            (i64::from(end.hour()) * 60 + i64::from(end.minute())) - (i64::from(start.hour()) * 60 + i64::from(start.minute()));
                        (format!("{:02}:{:02}", start.hour(), start.minute()), if minutes > 0 { crate::span(minutes) } else { String::new() })
                    }
                };
                let color = self.calendar_color(calendar);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.allocate_ui_with_layout(vec2(58.0, 38.0), Layout::top_down(Align::Min), |ui| {
                        ui.label(RichText::new(&time).size(13.0).color(t.text));
                        if !length.is_empty() {
                            ui.label(RichText::new(&length).size(theme::CAPTION).color(t.text_weak));
                        }
                    });
                    let (bar, _) = ui.allocate_exact_size(vec2(3.0, 36.0), Sense::hover());
                    ui.painter().rect_filled(bar, 2.0, color);
                    ui.add_space(2.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&summary).size(theme::BODY).strong().color(t.text));
                        if !location.is_empty() {
                            ui.label(RichText::new(&location).size(theme::CAPTION).color(t.text_weak));
                        }
                    });
                });
            }
        }
        if !any {
            ui.label(RichText::new(tl("Nothing in the coming days")).small().color(t.text_weak));
        }
    }
}

/// Whether `a` and `b` fall in the same Monday-to-Sunday week.
fn same_week(a: Date, b: Date) -> bool {
    let monday = |d: Date| d.add_days(-i64::from(d.weekday().index())).ok();
    monday(a).is_some() && monday(a) == monday(b)
}

/// A theme's name in the picker.
fn theme_name(id: &str) -> &'static str {
    match id {
        "terracotta" => "Terracotta",
        "sea" => "Sea",
        "lavender" => "Lavender",
        "olive" => "Olive",
        "azulejo" => "Azulejo",
        _ => "Teal",
    }
}

/// A person's colour for their initials: the same name always gets the same one, and white
/// initials stay readable on each.
pub(crate) fn avatar_color(name: &str) -> egui::Color32 {
    const COLORS: [[u8; 3]; 6] =
        [[0x1F, 0x7A, 0x8C], [0x7A, 0x4E, 0x9C], [0xA8, 0x4B, 0x24], [0x3E, 0x6B, 0x24], [0x2F, 0x5D, 0xA8], [0x8C, 0x3B, 0x5A]];
    let sum = name.chars().fold(0u32, |acc, c| acc.wrapping_mul(31).wrapping_add(u32::from(c)));
    let [r, g, b] = COLORS.get(usize::try_from(sum % 6).unwrap_or(0)).copied().unwrap_or([0x1F, 0x7A, 0x8C]);
    egui::Color32::from_rgb(r, g, b)
}
