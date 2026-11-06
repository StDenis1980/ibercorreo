//! The icon set, drawn in code (original artwork, no files): envelope, calendar, people, the
//! small folder icons of the folder pane, and the toolbar's line icons.

use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Mail,
    Calendar,
    People,
    /// Folder pane: Inbox (a tray), Drafts (a pencil), Sent (a paper plane), Outbox (an arrow
    /// leaving a tray), Archive (a box), Junk (a crossed circle), Trash (a bin), any other folder,
    /// and Favorites (a star).
    Inbox,
    Drafts,
    Sent,
    Outbox,
    Archive,
    Junk,
    Trash,
    Folder,
    Star,
    /// Toolbars: reply (one arrow back), reply all (two), forward, a flag, an open envelope
    /// (unread / read), two turning arrows (Send/Receive), a funnel (rules), three lines (the
    /// folder pane), a palette (theme), a window with its right side shaded (My Day), plus,
    /// undo, redo, a magnifier, a gear, a folder with an arrow (move), chevrons, a person with a
    /// plus, a bell and a pencil (new message).
    Reply,
    ReplyAll,
    Forward,
    Flag,
    Unread,
    Sync,
    Rules,
    Menu,
    Palette,
    PanelRight,
    Plus,
    Undo,
    Redo,
    Search,
    Gear,
    Move,
    ChevronLeft,
    ChevronRight,
    PersonAdd,
    Bell,
    Pencil,
    /// The title bar: a smiling face (send feedback), a globe (the interface language) and a
    /// page with a mark on it (error reports).
    Smile,
    Globe,
    Report,
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let w = Stroke::new((s / 14.0).max(1.2), color);
    let at = |x: f32, y: f32| -> Pos2 { pos2(c.x + x * s, c.y + y * s) };
    match icon {
        Icon::Mail => {
            let r = Rect::from_center_size(c, vec2(s * 0.72, s * 0.52));
            p.rect_stroke(r, s * 0.06, w, StrokeKind::Middle);
            p.line_segment([r.left_top() + vec2(s * 0.03, s * 0.03), at(0.0, 0.04)], w);
            p.line_segment([at(0.0, 0.04), r.right_top() + vec2(-s * 0.03, s * 0.03)], w);
        }
        Icon::Calendar => {
            let r = Rect::from_center_size(at(0.0, 0.04), vec2(s * 0.66, s * 0.6));
            p.rect_stroke(r, s * 0.06, w, StrokeKind::Middle);
            p.line_segment([pos2(r.left(), r.top() + s * 0.16), pos2(r.right(), r.top() + s * 0.16)], w);
            for x in [-0.16, 0.16] {
                p.line_segment([at(x, -0.32), at(x, -0.2)], w);
            }
            for (x, y) in [(-0.16, 0.06), (0.0, 0.06), (0.16, 0.06), (-0.16, 0.2), (0.0, 0.2)] {
                p.circle_filled(at(x, y), s * 0.035, color);
            }
        }
        Icon::People => {
            p.circle_stroke(at(0.0, -0.14), s * 0.13, w);
            let body = [at(-0.26, 0.3), at(-0.2, 0.1), at(0.0, 0.04), at(0.2, 0.1), at(0.26, 0.3)];
            p.add(egui::Shape::line(body.to_vec(), w));
        }
        Icon::Inbox | Icon::Outbox => {
            // A tray: an open box with a dip in its floor.
            let tray = [at(-0.32, 0.0), at(-0.32, 0.28), at(0.32, 0.28), at(0.32, 0.0)];
            p.add(egui::Shape::line(tray.to_vec(), w));
            p.add(egui::Shape::line(vec![at(-0.32, 0.0), at(-0.12, 0.0), at(-0.08, 0.1), at(0.08, 0.1), at(0.12, 0.0), at(0.32, 0.0)], w));
            // In: an arrow down into it; out: an arrow up and away.
            let (from, to, head) = if icon == Icon::Inbox { (-0.36, -0.06, 0.1) } else { (-0.06, -0.36, -0.1) };
            p.line_segment([at(0.0, from), at(0.0, to)], w);
            p.line_segment([at(-0.1, to - head), at(0.0, to)], w);
            p.line_segment([at(0.1, to - head), at(0.0, to)], w);
        }
        Icon::Drafts => {
            p.add(egui::Shape::line(vec![at(-0.26, 0.3), at(-0.3, 0.3), at(-0.3, 0.26), at(0.16, -0.2), at(0.26, -0.1), at(-0.2, 0.36)], w));
            p.line_segment([at(0.08, -0.12), at(0.18, -0.02)], w);
        }
        Icon::Sent => {
            let plane = [at(-0.34, -0.02), at(0.34, -0.3), at(0.06, 0.34), at(-0.04, 0.08), at(-0.34, -0.02)];
            p.add(egui::Shape::line(plane.to_vec(), w));
            p.line_segment([at(-0.04, 0.08), at(0.34, -0.3)], w);
        }
        Icon::Archive => {
            p.rect_stroke(Rect::from_min_max(at(-0.34, -0.3), at(0.34, -0.14)), s * 0.03, w, StrokeKind::Middle);
            p.add(egui::Shape::line(vec![at(-0.28, -0.14), at(-0.28, 0.3), at(0.28, 0.3), at(0.28, -0.14)], w));
            p.line_segment([at(-0.1, 0.0), at(0.1, 0.0)], w);
        }
        Icon::Junk => {
            p.circle_stroke(c, s * 0.3, w);
            p.line_segment([at(-0.21, 0.21), at(0.21, -0.21)], w);
        }
        Icon::Trash => {
            p.line_segment([at(-0.34, -0.2), at(0.34, -0.2)], w);
            p.add(egui::Shape::line(vec![at(-0.1, -0.2), at(-0.1, -0.32), at(0.1, -0.32), at(0.1, -0.2)], w));
            p.add(egui::Shape::line(vec![at(-0.26, -0.2), at(-0.22, 0.32), at(0.22, 0.32), at(0.26, -0.2)], w));
            for x in [-0.08, 0.08] {
                p.line_segment([at(x, -0.06), at(x, 0.2)], w);
            }
        }
        Icon::Folder => {
            let shape = [at(-0.34, -0.24), at(-0.1, -0.24), at(-0.02, -0.14), at(0.34, -0.14), at(0.34, 0.28), at(-0.34, 0.28), at(-0.34, -0.24)];
            p.add(egui::Shape::line(shape.to_vec(), w));
        }
        Icon::Star => {
            let points: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let r = if i % 2 == 0 { 0.36 } else { 0.15 };
                    let a = std::f32::consts::PI * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
                    at(r * a.cos(), r * a.sin() + 0.03)
                })
                .collect();
            p.add(egui::Shape::line(points, w));
        }
        Icon::Reply | Icon::ReplyAll => {
            let dx = if icon == Icon::ReplyAll { 0.08 } else { 0.0 };
            p.add(egui::Shape::line(vec![at(-0.02 + dx, -0.26), at(-0.3 + dx, 0.0), at(-0.02 + dx, 0.26)], w));
            if icon == Icon::ReplyAll {
                p.add(egui::Shape::line(vec![at(-0.12, -0.26), at(-0.38, 0.0), at(-0.12, 0.26)], w));
            }
            arc(p, at(-0.3 + dx, 0.0), at(0.34, 0.32), w);
        }
        Icon::Forward => {
            p.add(egui::Shape::line(vec![at(0.04, -0.26), at(0.32, 0.0), at(0.04, 0.26)], w));
            arc(p, at(0.32, 0.0), at(-0.34, 0.32), w);
        }
        Icon::Flag => {
            p.line_segment([at(-0.24, -0.34), at(-0.24, 0.36)], w);
            p.add(egui::Shape::line(vec![at(-0.24, -0.3), at(0.3, -0.3), at(0.18, -0.12), at(0.3, 0.06), at(-0.24, 0.06)], w));
        }
        Icon::Unread => {
            p.add(egui::Shape::line(vec![at(-0.34, -0.06), at(0.0, -0.32), at(0.34, -0.06), at(0.34, 0.3), at(-0.34, 0.3), at(-0.34, -0.06)], w));
            p.add(egui::Shape::line(vec![at(-0.34, -0.06), at(0.0, 0.16), at(0.34, -0.06)], w));
        }
        Icon::Sync => {
            let r = s * 0.28;
            let pts = |from: f32, to: f32| -> Vec<Pos2> {
                (0..=12).map(|i| from + (to - from) * i as f32 / 12.0).map(|a: f32| pos2(c.x + r * a.cos(), c.y + r * a.sin())).collect()
            };
            let pi = std::f32::consts::PI;
            p.add(egui::Shape::line(pts(-pi * 0.95, -pi * 0.15), w));
            p.add(egui::Shape::line(pts(pi * 0.05, pi * 0.85), w));
            p.add(egui::Shape::line(vec![at(0.26, -0.3), at(0.26, -0.1), at(0.06, -0.1)], w));
            p.add(egui::Shape::line(vec![at(-0.26, 0.3), at(-0.26, 0.1), at(-0.06, 0.1)], w));
        }
        Icon::Rules => {
            p.add(egui::Shape::line(
                vec![at(-0.34, -0.28), at(0.34, -0.28), at(0.08, 0.04), at(0.08, 0.32), at(-0.08, 0.24), at(-0.08, 0.04), at(-0.34, -0.28)],
                w,
            ));
        }
        Icon::Menu => {
            for y in [-0.22, 0.0, 0.22] {
                p.line_segment([at(-0.32, y), at(0.32, y)], w);
            }
        }
        Icon::Palette => {
            // A painter's palette: an oval with a notch for the thumb at the lower right, and
            // four dabs of paint in the themes' colours (their light shades on a light stroke,
            // which means a dark window).
            let outline: Vec<Pos2> = (0..=60)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 60.0;
                    let d = (a - 0.8).sin().atan2((a - 0.8).cos());
                    let r = 0.36 * (1.0 - 0.4 * (-(d * d) / 0.05).exp());
                    at(a.cos() * r, a.sin() * r * 0.88)
                })
                .collect();
            p.add(egui::Shape::line(outline, w));
            let light = u16::from(color.r()) + u16::from(color.g()) + u16::from(color.b()) > 3 * 128;
            let dabs: [[u8; 3]; 4] = if light {
                [[0x4F, 0xB3, 0xC4], [0xE0, 0x8A, 0x5E], [0xB3, 0x91, 0xD3], [0x8D, 0xBA, 0x63]]
            } else {
                [[0x1B, 0x72, 0x83], [0xA8, 0x4E, 0x24], [0x7A, 0x4E, 0x9C], [0x4A, 0x73, 0x2B]]
            };
            for ((x, y), [r, g, b]) in [(-0.19, -0.03), (-0.08, -0.18), (0.09, -0.18), (-0.12, 0.15)].into_iter().zip(dabs) {
                p.circle_filled(at(x, y), s * 0.065, Color32::from_rgb(r, g, b));
            }
        }
        Icon::PanelRight => {
            let r = Rect::from_center_size(c, vec2(s * 0.72, s * 0.6));
            p.rect_stroke(r, s * 0.06, w, StrokeKind::Middle);
            p.line_segment([at(0.12, -0.3), at(0.12, 0.3)], w);
        }
        Icon::Plus => {
            p.line_segment([at(0.0, -0.3), at(0.0, 0.3)], w);
            p.line_segment([at(-0.3, 0.0), at(0.3, 0.0)], w);
        }
        Icon::Undo | Icon::Redo => {
            let m = if icon == Icon::Undo { 1.0 } else { -1.0 };
            p.add(egui::Shape::line(vec![at(-0.12 * m, -0.3), at(-0.32 * m, -0.1), at(-0.12 * m, 0.1)], w));
            p.add(egui::Shape::line(
                vec![
                    at(-0.32 * m, -0.1),
                    at(0.08 * m, -0.1),
                    at(0.22 * m, -0.04),
                    at(0.3 * m, 0.08),
                    at(0.26 * m, 0.22),
                    at(0.1 * m, 0.3),
                    at(-0.06 * m, 0.3),
                ],
                w,
            ));
        }
        Icon::Search => {
            p.circle_stroke(at(-0.06, -0.06), s * 0.22, w);
            p.line_segment([at(0.1, 0.1), at(0.32, 0.32)], w);
        }
        Icon::Gear => {
            p.circle_stroke(c, s * 0.12, w);
            for i in 0..8 {
                let a = std::f32::consts::PI * i as f32 / 4.0;
                p.line_segment([pos2(c.x + s * 0.2 * a.cos(), c.y + s * 0.2 * a.sin()), pos2(c.x + s * 0.32 * a.cos(), c.y + s * 0.32 * a.sin())], w);
            }
        }
        Icon::Move => {
            let shape = [at(-0.34, -0.24), at(-0.1, -0.24), at(-0.02, -0.14), at(0.34, -0.14), at(0.34, 0.28), at(-0.34, 0.28), at(-0.34, -0.24)];
            p.add(egui::Shape::line(shape.to_vec(), w));
            p.line_segment([at(-0.14, 0.08), at(0.16, 0.08)], w);
            p.add(egui::Shape::line(vec![at(0.06, -0.02), at(0.16, 0.08), at(0.06, 0.18)], w));
        }
        Icon::ChevronLeft => {
            p.add(egui::Shape::line(vec![at(0.1, -0.24), at(-0.12, 0.0), at(0.1, 0.24)], w));
        }
        Icon::ChevronRight => {
            p.add(egui::Shape::line(vec![at(-0.1, -0.24), at(0.12, 0.0), at(-0.1, 0.24)], w));
        }
        Icon::PersonAdd => {
            p.circle_stroke(at(-0.08, -0.12), s * 0.13, w);
            p.add(egui::Shape::line(vec![at(-0.34, 0.32), at(-0.28, 0.1), at(-0.08, 0.04), at(0.08, 0.08)], w));
            p.line_segment([at(0.22, 0.04), at(0.22, 0.32)], w);
            p.line_segment([at(0.08, 0.18), at(0.36, 0.18)], w);
        }
        Icon::Smile => {
            p.circle_stroke(c, s * 0.34, w);
            p.circle_filled(at(-0.12, -0.09), s * 0.045, color);
            p.circle_filled(at(0.12, -0.09), s * 0.045, color);
            let smile: Vec<Pos2> = (0..=8)
                .map(|i| {
                    let a = std::f32::consts::PI * (0.2 + 0.6 * i as f32 / 8.0);
                    at(a.cos() * -0.17, a.sin() * 0.17 + 0.03)
                })
                .collect();
            p.add(egui::Shape::line(smile, w));
        }
        Icon::Globe => {
            p.circle_stroke(c, s * 0.34, w);
            p.line_segment([at(-0.34, 0.0), at(0.34, 0.0)], w);
            p.line_segment([at(-0.29, -0.17), at(0.29, -0.17)], w);
            p.line_segment([at(-0.29, 0.17), at(0.29, 0.17)], w);
            // The meridian: an ellipse half as wide as the globe.
            let meridian: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 24.0;
                    at(a.cos() * 0.15, a.sin() * 0.34)
                })
                .collect();
            p.add(egui::Shape::line(meridian, w));
        }
        Icon::Report => {
            let page = [at(-0.24, -0.32), at(0.1, -0.32), at(0.24, -0.18), at(0.24, 0.32), at(-0.24, 0.32), at(-0.24, -0.32)];
            p.add(egui::Shape::line(page.to_vec(), w));
            p.line_segment([at(0.0, -0.14), at(0.0, 0.08)], w);
            p.circle_filled(at(0.0, 0.19), s * 0.045, color);
        }
        Icon::Bell => {
            p.add(egui::Shape::line(
                vec![
                    at(-0.26, 0.18),
                    at(-0.2, 0.08),
                    at(-0.2, -0.08),
                    at(-0.12, -0.24),
                    at(0.0, -0.3),
                    at(0.12, -0.24),
                    at(0.2, -0.08),
                    at(0.2, 0.08),
                    at(0.26, 0.18),
                    at(-0.26, 0.18),
                ],
                w,
            ));
            p.line_segment([at(-0.06, 0.3), at(0.06, 0.3)], w);
        }
        Icon::Pencil => {
            p.add(egui::Shape::line(vec![at(-0.3, 0.3), at(-0.3, 0.16), at(0.14, -0.28), at(0.28, -0.14), at(-0.16, 0.3), at(-0.3, 0.3)], w));
            p.line_segment([at(0.04, -0.18), at(0.18, -0.04)], w);
        }
    }
}

/// A quarter-ish curve from `from` to `to`, bending through the corner at `to`'s x and
/// `from`'s y (the tail of a reply or forward arrow).
fn arc(p: &Painter, from: Pos2, to: Pos2, w: Stroke) {
    let corner = pos2(to.x, from.y);
    let pts: Vec<Pos2> = (0..=10)
        .map(|i| {
            let t = i as f32 / 10.0;
            let a = from + (corner - from) * t;
            let b = corner + (to - corner) * t;
            a + (b - a) * t
        })
        .collect();
    p.add(egui::Shape::line(pts, w));
}
