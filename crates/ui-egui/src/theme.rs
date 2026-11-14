//! Colours, all in one place. Six themes (View › Theme), each light or dark: a tinted window with
//! a soft wash of the theme's colour at the top, white cards for the list, the message and My
//! Day, and the theme's accent. The default is the Tinta Correo teal on the Tinta Office ivory
//! family of neutrals.

use egui::Color32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    pub accent: Color32,
    /// The accent laid thinly over a card: selected rows, the chosen view, hover.
    pub accent_soft: Color32,
    /// The window behind the cards: title bar, rail, folder pane, toolbars.
    pub window: Color32,
    /// The theme's wash at the top of the window, fading into [`Tokens::window`].
    pub band: Color32,
    /// The cards: message list, reading pane, My Day, calendar grid, contact table.
    pub card: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub unread: Color32,
    pub flag: Color32,
    pub border: Color32,
    pub today: Color32,
    /// Light text on the accent (the chosen day, the New buttons).
    pub on_accent: Color32,
    /// A text field's soft fill, and the line along its foot that shows where it ends.
    pub field: Color32,
    pub field_line: Color32,
}

/// One theme's colours: the accent and the window, light and dark.
struct Theme {
    id: &'static str,
    accent: [u8; 3],
    accent_dark: [u8; 3],
    window: [u8; 3],
    band: [u8; 3],
    window_dark: [u8; 3],
    band_dark: [u8; 3],
}

/// The default theme, the Tinta Correo teal.
const TEAL: Theme = Theme {
    id: "teal",
    accent: [0x1B, 0x72, 0x83],
    accent_dark: [0x4F, 0xB3, 0xC4],
    window: [0xED, 0xF0, 0xF4],
    band: [0xD3, 0xE8, 0xEC],
    window_dark: [0x16, 0x1B, 0x20],
    band_dark: [0x1C, 0x2D, 0x33],
};

/// In the order of [`ibercorreo_store::THEMES`]; the first is the default.
const THEMES: [Theme; 6] = [
    TEAL,
    Theme {
        id: "terracotta",
        accent: [0xA8, 0x4E, 0x24],
        accent_dark: [0xE0, 0x8A, 0x5E],
        window: [0xF3, 0xEF, 0xEB],
        band: [0xF2, 0xD9, 0xCA],
        window_dark: [0x1D, 0x19, 0x16],
        band_dark: [0x33, 0x24, 0x1C],
    },
    Theme {
        id: "sea",
        accent: [0x2F, 0x5D, 0xA8],
        accent_dark: [0x6F, 0x9B, 0xE0],
        window: [0xEC, 0xEF, 0xF6],
        band: [0xD5, 0xE0, 0xF4],
        window_dark: [0x16, 0x1A, 0x22],
        band_dark: [0x1D, 0x26, 0x38],
    },
    Theme {
        id: "lavender",
        accent: [0x7A, 0x4E, 0x9C],
        accent_dark: [0xB3, 0x91, 0xD3],
        window: [0xF0, 0xEE, 0xF5],
        band: [0xE2, 0xD7, 0xF0],
        window_dark: [0x1A, 0x18, 0x20],
        band_dark: [0x2A, 0x22, 0x36],
    },
    Theme {
        id: "olive",
        accent: [0x4A, 0x73, 0x2B],
        accent_dark: [0x8D, 0xBA, 0x63],
        window: [0xEE, 0xF1, 0xEA],
        band: [0xD9, 0xE6, 0xCD],
        window_dark: [0x18, 0x1B, 0x16],
        band_dark: [0x24, 0x30, 0x1C],
    },
    Theme {
        id: "azulejo",
        accent: [0x1E, 0x25, 0x47],
        accent_dark: [0x8C, 0x9B, 0xE0],
        window: [0xED, 0xEF, 0xF4],
        band: [0xD6, 0xDC, 0xEC],
        window_dark: [0x17, 0x19, 0x23],
        band_dark: [0x22, 0x28, 0x3E],
    },
];

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// `a` moved `share` (0 to 1) of the way to `b`.
pub fn mix(a: Color32, b: Color32, share: f32) -> Color32 {
    let s = share.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * s).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()))
}

impl Tokens {
    /// Theme `id` (an unknown or missing one is the default), light or dark.
    pub fn themed(id: Option<&str>, dark: bool) -> Tokens {
        let th = THEMES.iter().find(|t| Some(t.id) == id).unwrap_or(&TEAL);
        if dark {
            let accent = rgb(th.accent_dark);
            let card = Color32::from_rgb(0x24, 0x27, 0x30);
            let text_weak = Color32::from_rgb(0xB2, 0xB5, 0xC0);
            let field = mix(card, rgb(th.window_dark), 0.7);
            Tokens {
                accent,
                accent_soft: mix(card, accent, 0.22),
                window: rgb(th.window_dark),
                band: rgb(th.band_dark),
                card,
                text: Color32::from_rgb(0xEC, 0xE8, 0xDF),
                text_weak,
                unread: accent,
                flag: Color32::from_rgb(0xE0, 0x6A, 0x5C),
                border: Color32::from_rgb(0x3A, 0x3E, 0x4C),
                today: accent,
                on_accent: Color32::from_rgb(0x12, 0x14, 0x1A),
                field,
                field_line: mix(text_weak, card, FIELD_LINE_FADE),
            }
        } else {
            let accent = rgb(th.accent);
            let card = Color32::WHITE;
            let text_weak = Color32::from_rgb(0x5C, 0x63, 0x73);
            Tokens {
                accent,
                accent_soft: mix(card, accent, 0.13),
                window: rgb(th.window),
                band: rgb(th.band),
                card,
                text: Color32::from_rgb(0x1F, 0x24, 0x30),
                text_weak,
                unread: accent,
                flag: Color32::from_rgb(0xB0, 0x3A, 0x2E),
                border: Color32::from_rgb(0xE1, 0xE5, 0xEB),
                today: accent,
                on_accent: Color32::WHITE,
                field: rgb(th.window),
                field_line: mix(text_weak, card, FIELD_LINE_FADE),
            }
        }
    }

    pub fn light() -> Tokens {
        Tokens::themed(None, false)
    }

    pub fn dark() -> Tokens {
        Tokens::themed(None, true)
    }

    /// The colour shown for theme `id` in the theme picker: its band and its accent.
    pub fn swatch(id: &str, dark: bool) -> (Color32, Color32) {
        let t = Tokens::themed(Some(id), dark);
        (t.band, t.accent)
    }
}

/// The type ramp, after Fluent 2 (the Windows standard): captions 12, body 14, headings 20.
pub const CAPTION: f32 = 12.0;
pub const BODY: f32 = 14.0;
pub const SUBTITLE: f32 = 16.0;
pub const TITLE: f32 = 20.0;

/// Sizes, spacing and colours for everything egui draws itself (dialogs, menus, buttons,
/// fields, the reminder window), from theme `id`, light and dark alike. Spacing keeps to a 4
/// point grid; controls are at least 28 points high.
pub fn apply(ctx: &egui::Context, id: Option<&str>) {
    use egui::{CornerRadius, FontId, Margin, Shadow, Stroke, TextStyle, vec2};
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(CAPTION)),
            (TextStyle::Body, FontId::proportional(BODY)),
            (TextStyle::Button, FontId::proportional(BODY)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
            (TextStyle::Heading, FontId::proportional(TITLE)),
        ]
        .into();
        let sp = &mut s.spacing;
        sp.item_spacing = vec2(8.0, 6.0);
        sp.button_padding = vec2(12.0, 6.0);
        sp.interact_size = vec2(40.0, 32.0);
        sp.menu_margin = Margin::same(8);
        sp.window_margin = Margin::same(16);
        sp.icon_width = 16.0;
        sp.icon_width_inner = 10.0;
        sp.icon_spacing = 6.0;
        sp.combo_height = 320.0;

        let t = Tokens::themed(id, s.visuals.dark_mode);
        let v = &mut s.visuals;
        v.window_fill = t.card;
        v.panel_fill = t.window;
        v.window_stroke = Stroke::new(1.0, t.border);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(8);
        v.window_shadow = Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(if v.dark_mode { 110 } else { 40 }) };
        v.popup_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if v.dark_mode { 100 } else { 32 }) };
        v.hyperlink_color = t.accent;
        v.faint_bg_color = mix(t.card, t.window, 0.5);
        v.extreme_bg_color = t.card;
        v.text_edit_bg_color = Some(t.field);
        v.weak_text_color = Some(t.text_weak);
        v.selection.bg_fill = mix(t.card, t.accent, if v.dark_mode { 0.45 } else { 0.28 });
        v.selection.stroke = Stroke::new(1.0, t.accent);
        // Buttons, menus and lists have no frame, only a soft tint that deepens under the
        // pointer, like the toolbar's; a text field is a soft fill with a line at its foot ([`field`]), and a
        // checkbox's box is the stronger `bg_fill`.
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = t.card;
        w.noninteractive.weak_bg_fill = t.card;
        w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
        for (st, weak, strong) in [
            (&mut w.inactive, mix(t.card, t.accent, 0.06), mix(t.card, t.text_weak, 0.24)),
            (&mut w.hovered, t.accent_soft, mix(t.card, t.accent, 0.32)),
            (&mut w.active, mix(t.card, t.accent, 0.22), mix(t.card, t.accent, 0.42)),
            (&mut w.open, t.accent_soft, mix(t.card, t.accent, 0.32)),
        ] {
            st.bg_fill = strong;
            st.weak_bg_fill = weak;
            st.bg_stroke = Stroke::NONE;
            st.fg_stroke = Stroke::new(1.0, t.text);
            st.corner_radius = CornerRadius::same(8);
            st.expansion = 0.0;
        }
        w.noninteractive.corner_radius = CornerRadius::same(8);
    });
}

/// A text field: 15 points on a soft fill with no border, as tall as the other controls, and a
/// line along its foot that marks where it ends, in the accent under the pointer. The accent's
/// ring shows while it has the keyboard.
pub fn field(ui: &mut egui::Ui, edit: egui::TextEdit<'_>) -> egui::Response {
    let r = ui.add(edit.font(egui::FontId::proportional(15.0)).margin(egui::Margin::symmetric(10, 8)));
    if !r.has_focus() {
        let v = ui.visuals();
        let color = if r.hovered() { v.selection.stroke.color } else { mix(v.weak_text_color(), v.window_fill, FIELD_LINE_FADE) };
        let x = egui::Rangef::new(r.rect.left() + 3.0, r.rect.right() - 3.0);
        ui.painter().hline(x, r.rect.bottom() - 0.5, egui::Stroke::new(1.0, color));
    }
    r
}

/// How far a field's line ([`Tokens::field_line`]) fades from the weak text towards the card.
const FIELD_LINE_FADE: f32 = 0.15;

/// The two ends of a filled button's gentle vertical gradient: the top a little lighter.
pub fn gradient(base: Color32) -> (Color32, Color32) {
    (mix(base, Color32::WHITE, GRADIENT_LIFT), base)
}

/// How much lighter a filled button's top is than its bottom.
pub const GRADIENT_LIFT: f32 = 0.08;

/// Theme ids the interface knows, in the store's order.
pub fn ids() -> impl Iterator<Item = &'static str> {
    THEMES.iter().map(|t| t.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance.
    fn lum(c: Color32) -> f32 {
        let f = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn every_theme_the_store_knows_has_colours_and_none_more() {
        assert_eq!(ids().collect::<Vec<_>>(), ibercorreo_store::THEMES.to_vec());
    }

    /// Text stays readable in every theme, light and dark: body text and the accent (unread
    /// subjects, links) on the cards and the window, weak text on the cards.
    #[test]
    fn every_theme_keeps_text_readable() {
        for id in ids() {
            for dark in [false, true] {
                let t = Tokens::themed(Some(id), dark);
                for (what, fg, bg, min) in [
                    ("text on card", t.text, t.card, 7.0),
                    ("text on window", t.text, t.window, 7.0),
                    ("weak text on card", t.text_weak, t.card, 4.5),
                    ("weak text on selection", t.text_weak, t.accent_soft, 4.5),
                    ("accent on card", t.accent, t.card, 4.5),
                    ("accent on window", t.accent, t.window, 4.5),
                    ("on-accent on accent", t.on_accent, t.accent, 4.5),
                    ("on-accent on a primary button's lighter top", t.on_accent, gradient(t.accent).0, 4.5),
                    ("text on a secondary button", t.text, gradient(mix(t.card, t.accent, 0.1)).0, 7.0),
                    ("text in a field", t.text, t.field, 7.0),
                    ("hint in a field", t.text_weak, t.field, 4.5),
                    ("a field's line on a card", t.field_line, t.card, 3.0),
                    ("a field's line on its fill", t.field_line, t.field, 3.0),
                    ("a dangerous action on a card", t.flag, t.card, 4.5),
                ] {
                    let c = contrast(fg, bg);
                    assert!(c >= min, "{id} dark={dark}: {what} is {c:.2}:1");
                }
            }
        }
    }

    /// Dialogs and menus egui draws itself (the reminder window among them) take the theme's
    /// colours, light and dark, and the larger type.
    #[test]
    fn windows_and_menus_follow_the_theme() {
        let ctx = egui::Context::default();
        apply(&ctx, Some("sea"));
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            ctx.set_theme(theme);
            let t = Tokens::themed(Some("sea"), theme == egui::Theme::Dark);
            let s = ctx.global_style();
            assert_eq!(s.visuals.window_fill, t.card);
            assert_eq!(s.visuals.window_stroke.color, t.border);
            assert_eq!(s.visuals.hyperlink_color, t.accent);
            assert_eq!(s.text_styles.get(&egui::TextStyle::Body).map(|f| f.size), Some(BODY));
            assert_eq!(s.text_styles.get(&egui::TextStyle::Small).map(|f| f.size), Some(CAPTION));
        }
    }

    #[test]
    fn an_unknown_theme_is_the_default() {
        assert_eq!(Tokens::themed(Some("neon"), false), Tokens::light());
        assert_eq!(Tokens::themed(Some("terracotta"), true).window, Color32::from_rgb(0x1D, 0x19, 0x16));
    }
}
