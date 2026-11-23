//! Tables and colours in an HTML message (Tinta). A table of data (an order, a timetable, an
//! invoice's lines) is drawn as a grid in our own style; tables that only lay a message out never
//! get here ([`ibercorreo_core::rich::Cell`]). A sender's text colour is shown only where it is
//! readable on what is behind it, so a message written for a white page still reads in the dark
//! theme; a highlight brings its own background and the text on it is made readable.

use egui::{Color32, Stroke, Ui, vec2};
use ibercorreo_core::rich::Block;

use crate::body_text::BodyText;
use crate::theme::Tokens;
use crate::{Pictures, rich_block};

/// Least contrast between a sender's text colour and what is behind it (WCAG's for large text).
const MIN_CONTRAST: f32 = 3.0;
/// Narrowest column drawn.
const MIN_COLUMN: f32 = 48.0;
/// A cell background this close to white only matched a white page, and is not drawn.
const PAGE_WHITE: f32 = 1.05;

/// The background to draw behind a cell: the sender's, unless it is about white.
pub(crate) fn cell_background(sender: Option<u32>) -> Option<Color32> {
    sender.map(rgb).filter(|c| contrast(*c, Color32::WHITE) >= PAGE_WHITE)
}

/// `0xRRGGBB` as a colour.
pub(crate) fn rgb(c: u32) -> Color32 {
    let [_, r, g, b] = c.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

/// Relative luminance (WCAG 2).
fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// Contrast ratio between two colours, 1 to 21.
pub(crate) fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The colour to draw text in: the sender's when it reads well on `behind`, else `usual` when that
/// does, else black or white, whichever reads better.
pub(crate) fn text_color(sender: Option<u32>, usual: Color32, behind: Color32) -> Color32 {
    if let Some(c) = sender.map(rgb).filter(|c| contrast(*c, behind) >= MIN_CONTRAST) {
        return c;
    }
    if contrast(usual, behind) >= MIN_CONTRAST {
        return usual;
    }
    if contrast(Color32::BLACK, behind) >= contrast(Color32::WHITE, behind) { Color32::BLACK } else { Color32::WHITE }
}

/// The blocks of one table, `first` being where it starts among the message's blocks: a grid, a
/// column per cell, rows striped. A merged cell (`colspan`) keeps the columns it covers empty, so
/// the cells after it stay in theirs; a cell with a background has it drawn behind its text. When
/// copied, cells are a tab apart and rows a line.
pub(crate) fn table(ui: &mut Ui, t: &Tokens, blocks: &[Block], first: usize, body: &mut BodyText, pictures: &mut Pictures<'_>) {
    let cols = blocks.iter().filter_map(|b| b.cell).map(|c| usize::from(c.col) + usize::from(c.columns())).max().unwrap_or(1);
    let gap = 14.0;
    let room = ui.available_width();
    // Precision does not matter for a count of columns.
    #[allow(clippy::cast_precision_loss)]
    let column = ((room - gap * (cols.saturating_sub(1)) as f32) / cols as f32).max(MIN_COLUMN);
    let frame = egui::Frame::new().stroke(Stroke::new(1.0, t.border)).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 6));
    frame.show(ui, |ui| {
        egui::Grid::new(ui.id().with(("rich-table", first))).striped(true).spacing(vec2(gap, 6.0)).min_col_width(0.0).show(ui, |ui| {
            let mut i = 0;
            let mut first_cell = true;
            while let Some(cell) = blocks.get(i).and_then(|b| b.cell) {
                let row = cell.row;
                let mut next_col = 0u16;
                while let Some(c) = blocks.get(i).and_then(|b| b.cell).filter(|c| c.row == row) {
                    // A row with fewer cells leaves the gap empty.
                    while next_col < c.col {
                        ui.label("");
                        next_col = next_col.saturating_add(1);
                    }
                    let len = blocks
                        .get(i..)
                        .map_or(0, |rest| rest.iter().take_while(|b| b.cell.is_some_and(|d| d.row == row && d.col == c.col)).count())
                        .max(1);
                    let lead = if first_cell {
                        "\n\n"
                    } else if c.col == 0 {
                        "\n"
                    } else {
                        "\t"
                    };
                    first_cell = false;
                    let fill = cell_background(c.background);
                    // Painted once the cell's size is known, under its text.
                    let slot = ui.painter().add(egui::Shape::Noop);
                    let width = ui.available_width().min(column);
                    let cell = ui.vertical(|ui| {
                        ui.set_max_width(column);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for (k, b) in blocks.get(i..i + len).unwrap_or(&[]).iter().enumerate() {
                            rich_block(ui, t, b, body, pictures, Some(if k == 0 { lead } else { "\n" }), fill.unwrap_or(t.card));
                        }
                    });
                    if let Some(fill) = fill {
                        let mut rect = cell.response.rect;
                        rect.set_width(rect.width().max(width));
                        ui.painter().set(slot, egui::Shape::rect_filled(rect.expand2(vec2(5.0, 3.0)), 3, fill));
                    }
                    next_col = c.col.saturating_add(c.columns());
                    i += len;
                }
                ui.end_row();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_stay_readable_on_either_theme() {
        let white = Color32::WHITE;
        let dark = Color32::from_rgb(0x20, 0x20, 0x24);
        let grey_text = Color32::from_rgb(0x30, 0x30, 0x30);
        let light_text = Color32::from_rgb(0xe8, 0xe8, 0xe8);
        // A red the sender chose shows on white; dark grey on a dark page falls back to the theme's text.
        assert_eq!(text_color(Some(0xcc0000), grey_text, white), rgb(0xcc0000));
        assert_eq!(text_color(Some(0x333333), light_text, dark), light_text);
        // White text meant for a coloured button is unreadable on white: the usual colour instead.
        assert_eq!(text_color(Some(0xffffff), grey_text, white), grey_text);
        // On a yellow highlight in the dark theme, light text would vanish: black instead.
        assert_eq!(text_color(None, light_text, rgb(0xffff00)), Color32::BLACK);
        assert!(contrast(Color32::BLACK, white) > 20.0);
        assert!((contrast(white, white) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_white_cell_background_is_not_drawn_a_coloured_one_is() {
        assert_eq!(cell_background(Some(0xffffff)), None);
        assert_eq!(cell_background(Some(0xfdfdfd)), None);
        assert_eq!(cell_background(None), None);
        assert_eq!(cell_background(Some(0xdddddd)), Some(rgb(0xdddddd)));
        // Light text in the dark theme turns dark on a light cell.
        let light_text = Color32::from_rgb(0xe8, 0xe8, 0xe8);
        assert_eq!(text_color(None, light_text, rgb(0xdddddd)), Color32::BLACK);
    }
}
