//! The icon set, drawn in code (original artwork, no files): envelope, calendar, people, and
//! the small folder icons of the folder pane.

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
    }
}
