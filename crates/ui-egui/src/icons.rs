//! The icon set, drawn in code (original artwork, no files): envelope, calendar, people.

use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Mail,
    Calendar,
    People,
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
    }
}
