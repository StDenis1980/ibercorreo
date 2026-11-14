//! Selecting a message's text in the reading pane. egui's own label selection follows only the
//! labels on screen, and lets go as soon as either end scrolls out of view, so a long message
//! could only be copied in parts. Here the pane keeps the selection itself, as two places in the
//! message's text, whatever is scrolled where: a drag past the pane's edge scrolls it, the wheel
//! works mid-drag, and Ctrl+A and Ctrl+C reach the whole message.

use std::sync::Arc;

use egui::text::{CCursor, Galley};
use egui::text_selection::CCursorRange;
use egui::text_selection::visuals::paint_text_selection;
use egui::{Color32, Event, Id, Key, Modifiers, Pos2, Rect, Response, ScrollArea, Sense, Ui, Vec2, vec2};

use crate::i18n::tl;

/// How close to the pane's top or bottom a drag starts it scrolling.
const EDGE: f32 = 24.0;
/// How far past that a drag has to go for the fastest scroll.
const MAX_REACH: f32 = 240.0;
/// Points scrolled per second for each point of reach.
const SPEED: f32 = 6.0;
/// The strip at the pane's right where a press belongs to the scroll bar.
const SCROLL_BAR: f32 = 12.0;

/// A place in the text: a piece, and a character in it.
type Spot = (usize, usize);

/// One run of text as it was laid out: a paragraph, or a part of one in a style of its own.
struct Piece {
    galley: Arc<Galley>,
    /// Where it sits, from the top left of the scrolling content.
    at: Vec2,
    /// What comes before it when copied: nothing inside a paragraph, line breaks between them.
    before: &'static str,
    /// What the reader sees beside it that is not selectable, such as a list's bullet.
    marker: Option<String>,
}

#[derive(Default)]
pub(crate) struct BodyText {
    /// Whose text this is (the message, and how it is shown): another one starts afresh.
    owner: Option<Id>,
    pieces: Vec<Piece>,
    /// The last frame's layout, where the pointer is looked up.
    laid_out: Vec<Piece>,
    origin: Pos2,
    /// Where the selection began, and where it ends now.
    anchor: Option<Spot>,
    head: Option<Spot>,
    dragging: bool,
    /// The marker of the next piece added.
    marker: Option<String>,
    /// How far the pane is scrolled.
    offset: f32,
}

impl BodyText {
    /// The scrolling pane: `add` lays out the text with [`BodyText::add`].
    pub(crate) fn show(&mut self, ui: &mut Ui, owner: Id, add: impl FnOnce(&mut BodyText, &mut Ui)) {
        if self.owner != Some(owner) {
            *self = BodyText { owner: Some(owner), ..BodyText::default() };
        }
        let output = ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let viewport = ui.clip_rect();
            self.origin = ui.min_rect().min;
            // Under the text, so links keep their clicks.
            let background = ui.interact(viewport, ui.id().with("message-text"), Sense::click());
            self.input(ui, viewport);
            add(self, ui);
            self.laid_out = std::mem::take(&mut self.pieces);
            background.context_menu(|ui| {
                let selected = self.selected_text();
                if ui.add_enabled(selected.is_some(), egui::Button::new(tl("Copy"))).clicked() {
                    if let Some(text) = selected {
                        ui.ctx().copy_text(text);
                    }
                    ui.close();
                }
                if ui.button(tl("Copy all")).clicked() {
                    ui.ctx().copy_text(self.all_text());
                    ui.close();
                }
            });
        });
        self.offset = output.state.offset.y;
    }

    /// Mouse and keys, before the text is laid out, so the selection shows at once.
    fn input(&mut self, ui: &Ui, viewport: Rect) {
        let ctx = ui.ctx().clone();
        let (pressed, down, pos, dragged, double, triple, shift, dt) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.latest_pos().or_else(|| i.pointer.interact_pos()),
                i.pointer.is_decidedly_dragging(),
                i.pointer.button_double_clicked(egui::PointerButton::Primary),
                i.pointer.button_triple_clicked(egui::PointerButton::Primary),
                i.modifiers.shift,
                i.stable_dt,
            )
        });
        if pressed && let Some(p) = pos {
            let ours = ctx.layer_id_at(p) == Some(ui.layer_id());
            let on_text = viewport.contains(p) && p.x < viewport.right() - SCROLL_BAR;
            if ours && on_text {
                let spot = self.spot_at(p);
                if !(shift && self.anchor.is_some()) {
                    self.anchor = spot;
                }
                self.head = spot;
                self.dragging = spot.is_some();
            } else if ours && !viewport.contains(p) {
                self.clear();
            }
        }
        if self.dragging {
            if down {
                if let Some(p) = pos {
                    if let Some(spot) = self.spot_at(p) {
                        self.head = Some(spot);
                    }
                    if dragged {
                        let reach = if p.y > viewport.bottom() - EDGE {
                            -(p.y - (viewport.bottom() - EDGE))
                        } else if p.y < viewport.top() + EDGE {
                            viewport.top() + EDGE - p.y
                        } else {
                            0.0
                        };
                        if reach != 0.0 {
                            ui.scroll_with_delta(vec2(0.0, reach.clamp(-MAX_REACH, MAX_REACH) * SPEED * dt));
                        }
                    }
                }
                ctx.request_repaint();
            } else {
                self.dragging = false;
            }
        }
        if (double || triple)
            && let Some(p) = pos
            && viewport.contains(p)
            && ctx.layer_id_at(p) == Some(ui.layer_id())
            && let Some((piece, at)) = self.spot_at(p)
        {
            let (from, to) = if triple { (0, self.chars_in(piece)) } else { self.word_around(piece, at) };
            self.anchor = Some((piece, from));
            self.head = Some((piece, to));
        }
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            self.select_all();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.clear();
        }
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Copy | Event::Cut)))
            && let Some(text) = self.selected_text()
        {
            ctx.copy_text(text);
        }
    }

    /// Lay out one run of text with its share of the selection. `before` goes ahead of it when
    /// copied; `color` is for what the label does not colour itself.
    pub(crate) fn add(&mut self, ui: &mut Ui, label: egui::Label, color: Color32, before: &'static str) -> Response {
        let (pos, galley, response) = label.selectable(false).layout_in_ui(ui);
        let piece = self.pieces.len();
        if ui.is_rect_visible(response.rect) {
            let mut shown = galley.clone();
            if let Some(((from_piece, from), (to_piece, to))) = self.selection() {
                let start = if piece == from_piece { from } else { 0 };
                let end = if piece == to_piece { to } else { galley.text().chars().count() };
                if (from_piece..=to_piece).contains(&piece) && start < end {
                    paint_text_selection(&mut shown, ui.visuals(), &CCursorRange::two(CCursor::new(start), CCursor::new(end)), None);
                }
            }
            ui.painter().galley(pos, shown, color);
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), galley.text()));
        let marker = self.marker.take();
        self.pieces.push(Piece { galley, at: pos - self.origin, before, marker });
        response
    }

    /// The next piece has this beside it (a bullet, a number), copied with it.
    pub(crate) fn mark_next(&mut self, marker: String) {
        self.marker = Some(marker);
    }

    fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.dragging = false;
    }

    fn select_all(&mut self) {
        let Some(last) = self.laid_out.len().checked_sub(1) else { return };
        self.anchor = Some((0, 0));
        self.head = Some((last, self.chars_in(last)));
        self.dragging = false;
    }

    /// The two ends of the selection in reading order, unless it is empty.
    fn selection(&self) -> Option<(Spot, Spot)> {
        let (a, b) = (self.anchor?, self.head?);
        (a != b).then(|| if a < b { (a, b) } else { (b, a) })
    }

    fn selected_text(&self) -> Option<String> {
        let (from, to) = self.selection()?;
        Some(self.text_between(from, to))
    }

    fn all_text(&self) -> String {
        let last = self.laid_out.len().saturating_sub(1);
        self.text_between((0, 0), (last, usize::MAX))
    }

    /// The text between two places, with paragraphs and list markers as a reader expects.
    fn text_between(&self, (from_piece, from): Spot, (to_piece, to): Spot) -> String {
        let mut out = String::new();
        for (n, p) in self.laid_out.iter().enumerate() {
            if n < from_piece || n > to_piece {
                continue;
            }
            let start = if n == from_piece { from } else { 0 };
            let end = if n == to_piece { to } else { usize::MAX };
            if !out.is_empty() {
                out.push_str(p.before);
            }
            if start == 0
                && let Some(marker) = &p.marker
            {
                out.push_str(marker);
                out.push(' ');
            }
            out.extend(p.galley.text().chars().skip(start).take(end.saturating_sub(start)));
        }
        out
    }

    /// The place in the text nearest a point on screen, line first, then across it.
    fn spot_at(&self, pos: Pos2) -> Option<Spot> {
        let local = pos - self.origin;
        let mut best: Option<((f32, f32), usize, Vec2)> = None;
        for (n, p) in self.laid_out.iter().enumerate() {
            for row in &p.galley.rows {
                let r = row.rect().translate(p.at);
                let dy = (r.top() - local.y).max(local.y - r.bottom()).max(0.0);
                let dx = (r.left() - local.x).max(local.x - r.right()).max(0.0);
                if best.as_ref().is_none_or(|(d, _, _)| (dy, dx) < *d) {
                    best = Some(((dy, dx), n, p.at));
                }
            }
        }
        let (_, n, at) = best?;
        let cursor = self.laid_out.get(n)?.galley.cursor_from_pos(local - at);
        Some((n, cursor.index.into()))
    }

    fn chars_in(&self, piece: usize) -> usize {
        self.laid_out.get(piece).map_or(0, |p| p.galley.text().chars().count())
    }

    /// The word at a place in a piece, as characters.
    fn word_around(&self, piece: usize, at: usize) -> (usize, usize) {
        let Some(p) = self.laid_out.get(piece) else { return (at, at) };
        let chars: Vec<char> = p.galley.text().chars().collect();
        let word = |i: usize| chars.get(i).is_some_and(|c| c.is_alphanumeric());
        let (mut from, mut to) = (at, at);
        while from > 0 && word(from - 1) {
            from -= 1;
        }
        while word(to) {
            to += 1;
        }
        (from, to)
    }
}

#[cfg(test)]
mod tests {
    use egui::{PointerButton, RawInput, RichText};

    use super::*;

    const LINES: usize = 200;

    fn input(events: Vec<Event>) -> RawInput {
        RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 300.0))), events, ..Default::default() }
    }

    fn frame(ctx: &egui::Context, body: &mut BodyText, events: Vec<Event>) -> egui::FullOutput {
        let mut out = ctx.run_ui(input(events), |ui| {
            body.show(ui, Id::new("a message"), |body, ui| {
                for n in 0..LINES {
                    body.add(ui, egui::Label::new(RichText::new(format!("Synthetic line {n}"))), Color32::BLACK, "\n");
                }
            });
        });
        out.textures_delta.clear();
        out
    }

    fn laid_out() -> (egui::Context, BodyText) {
        let ctx = egui::Context::default();
        let mut body = BodyText::default();
        frame(&ctx, &mut body, Vec::new());
        frame(&ctx, &mut body, Vec::new());
        (ctx, body)
    }

    fn press(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    fn copied(out: &egui::FullOutput) -> Option<String> {
        out.platform_output.commands.iter().find_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
    }

    /// A drag past the pane's bottom scrolls it and the selection grows beyond what was on
    /// screen; its start, long out of view by then, still holds.
    #[test]
    fn a_drag_below_the_pane_scrolls_and_extends_the_selection() {
        let (ctx, mut body) = laid_out();
        let start = pos2_on_first_line(&body);
        frame(&ctx, &mut body, vec![Event::PointerMoved(start), press(start, true)]);
        let below = Pos2::new(start.x, 400.0);
        frame(&ctx, &mut body, vec![Event::PointerMoved(below)]);
        for _ in 0..240 {
            frame(&ctx, &mut body, vec![Event::PointerMoved(below)]);
        }
        assert!(body.offset > 300.0, "scrolled {}", body.offset);
        let text = body.selected_text().unwrap();
        assert!(text.starts_with("Synthetic line 0") || text.starts_with("ynthetic line 0"), "{}", &text[..30]);
        assert!(text.contains("Synthetic line 30"), "{text}");
        // The mouse wheel in the middle of a drag keeps it too.
        let before = body.selection();
        frame(
            &ctx,
            &mut body,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 40.0),
                modifiers: Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            }],
        );
        assert_eq!(before.map(|s| s.0), body.selection().map(|s| s.0));
        frame(&ctx, &mut body, vec![press(below, false)]);
        assert!(!body.dragging);
        assert!(body.selection().is_some(), "letting go keeps the selection");
    }

    fn pos2_on_first_line(body: &BodyText) -> Pos2 {
        let p = body.laid_out.first().unwrap();
        body.origin + p.at + vec2(3.0, 6.0)
    }

    /// Ctrl+A selects every line, on screen or not, and Ctrl+C copies them all.
    #[test]
    fn ctrl_a_and_ctrl_c_reach_the_whole_message() {
        let (ctx, mut body) = laid_out();
        let keys = Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND };
        frame(&ctx, &mut body, vec![keys]);
        let out = frame(&ctx, &mut body, vec![Event::Copy]);
        let text = copied(&out).expect("Ctrl+C copies");
        assert!(text.starts_with("Synthetic line 0\nSynthetic line 1\n"), "{}", &text[..40]);
        assert!(text.ends_with(&format!("Synthetic line {}", LINES - 1)));
        assert_eq!(text.lines().count(), LINES);
    }

    /// Copy all takes the whole text with its paragraphs and bullets, selected or not.
    #[test]
    fn copy_all_is_the_whole_plain_text() {
        let ctx = egui::Context::default();
        let mut body = BodyText::default();
        for _ in 0..2 {
            ctx.run_ui(input(Vec::new()), |ui| {
                body.show(ui, Id::new("a message"), |body, ui| {
                    body.add(ui, egui::Label::new("First paragraph"), Color32::BLACK, "\n\n");
                    body.add(ui, egui::Label::new("Second paragraph"), Color32::BLACK, "\n\n");
                    body.mark_next("•".into());
                    body.add(ui, egui::Label::new("an item"), Color32::BLACK, "\n");
                    body.add(ui, egui::Label::new(" and more"), Color32::BLACK, "");
                });
            })
            .textures_delta
            .clear();
        }
        assert_eq!(body.selected_text(), None);
        assert_eq!(body.all_text(), "First paragraph\n\nSecond paragraph\n• an item and more");
    }

    /// Another message starts with nothing selected; Escape lets go of a selection.
    #[test]
    fn a_selection_belongs_to_one_message() {
        let (ctx, mut body) = laid_out();
        body.select_all();
        assert!(body.selection().is_some());
        frame(&ctx, &mut body, vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        assert!(body.selection().is_none());
        body.select_all();
        ctx.run_ui(input(Vec::new()), |ui| body.show(ui, Id::new("another message"), |_, _| {})).textures_delta.clear();
        assert!(body.selection().is_none());
    }
}
