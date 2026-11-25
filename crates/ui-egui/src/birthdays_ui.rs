//! Birthdays in the calendar (Tinta): each contact's birthday drawn as a festive chip, a cake and
//! a sprinkle of confetti, with the age they turn; a ribbon of them over the day and week views,
//! in the month's cells, a card in the agenda with Send wishes (a new message to them), a line in
//! My Day, and a row in the calendar list that shows or hides them. They come from the engine
//! ([`ibercorreo_engine::birthdays`]): nothing is written to a calendar.

use egui::{Color32, CornerRadius, Frame, Margin, Painter, Rect, RichText, Ui, pos2, vec2};
use ibercorreo_engine::birthdays::Birthday;

use crate::App;
use crate::chrome;
use crate::i18n::{tf, tl};
use crate::icons::{self, Icon};
use crate::theme::{self, Tokens};

pub(crate) const BIRTHDAYS: &str = "Birthdays";
pub(crate) const SHOW_TIP: &str = "Show or hide your contacts' birthdays";
pub(crate) const TURNS: &str = "turns {age}";
pub(crate) const TURNS_TODAY: &str = "turns {age} today";
pub(crate) const BIRTHDAY: &str = "Birthday";
pub(crate) const BIRTHDAY_TODAY: &str = "Birthday today";
pub(crate) const WISHES: &str = "Send wishes";
pub(crate) const WISHES_TIP: &str = "Write to them: a new message with a birthday greeting";
pub(crate) const SUBJECT: &str = "Happy birthday!";
pub(crate) const MORE: &str = "+{n} more";

#[cfg(test)]
const STRINGS: [&str; 10] = [BIRTHDAYS, SHOW_TIP, TURNS, TURNS_TODAY, BIRTHDAY, BIRTHDAY_TODAY, WISHES, WISHES_TIP, SUBJECT, MORE];

/// The birthdays' own colour, a warm rose that reads on light and dark themes alike.
pub(crate) const ROSE: Color32 = Color32::from_rgb(0xE0, 0x5A, 0x86);
/// The confetti: rose, gold and the sea teal of the app.
const CONFETTI: [Color32; 3] = [ROSE, Color32::from_rgb(0xE8, 0xB3, 0x3A), Color32::from_rgb(0x1F, 0x9A, 0x9C)];

/// "Name · 37" for a chip, or the name alone when the year is not known.
pub(crate) fn label(b: &Birthday) -> String {
    match b.age {
        Some(age) if age > 0 => format!("{} · {age}", b.name),
        _ => b.name.clone(),
    }
}

/// The label for a chip `width` wide: a narrow one names the first name only.
fn chip_label(b: &Birthday, width: f32) -> String {
    if width >= 150.0 {
        return label(b);
    }
    let first = b.name.split_whitespace().next().unwrap_or(&b.name).to_string();
    label(&Birthday { name: first, ..b.clone() })
}

/// A few dots of confetti in `rect`, placed the same way each time for the same `seed`.
pub(crate) fn confetti(p: &Painter, rect: Rect, seed: u64) {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let n = (rect.width() * rect.height() / 120.0).clamp(4.0, 12.0) as usize;
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let fx = (x % 1000) as f32 / 1000.0;
        let fy = ((x / 1000) % 1000) as f32 / 1000.0;
        let at = pos2(rect.left() + fx * rect.width(), rect.top() + fy * rect.height());
        let color = CONFETTI.get(i % CONFETTI.len()).copied().unwrap_or(ROSE);
        if i % 2 == 0 {
            p.circle_filled(at, 2.2, color);
        } else {
            p.rect_filled(Rect::from_center_size(at, vec2(4.4, 2.2)), 0.6, color);
        }
    }
}

/// One birthday as a chip: a soft rose pill, the cake, the name and age, confetti at its end.
pub(crate) fn chip(p: &Painter, rect: Rect, t: &Tokens, b: &Birthday) {
    p.rect_filled(rect, rect.height() / 2.0, theme::mix(t.card, ROSE, 0.2));
    let icon = Rect::from_center_size(pos2(rect.left() + rect.height() / 2.0 + 2.0, rect.center().y), vec2(rect.height() - 4.0, rect.height() - 4.0));
    icons::paint(p, icon, Icon::Cake, ROSE);
    let clip = p.with_clip_rect(rect.shrink2(vec2(rect.height() / 2.0, 0.0)));
    clip.text(
        pos2(icon.right() + 4.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        chip_label(b, rect.width()),
        egui::FontId::proportional(12.0),
        t.text,
    );
    if rect.width() > 90.0 {
        confetti(p, Rect::from_min_max(pos2(rect.right() - 26.0, rect.top() + 3.0), pos2(rect.right() - 8.0, rect.bottom() - 3.0)), b.contact);
    }
}

impl App {
    /// The birthday cards of `day` at the top of the agenda; returns whom to write to.
    pub(crate) fn birthday_cards(&mut self, ui: &mut Ui, day: ibercorreo_core::Date) {
        let t = self.tokens;
        let today = self.session.today();
        let mut write: Option<String> = None;
        for b in self.session.birthdays_between(day, day) {
            let frame =
                Frame::NONE.fill(theme::mix(t.card, ROSE, 0.14)).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(12, 10));
            let card = frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(34.0, 34.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 17.0, theme::mix(t.card, ROSE, 0.3));
                    icons::paint(ui.painter(), rect.shrink(6.0), Icon::Cake, ROSE);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&b.name).strong().color(t.text));
                        let line = match (b.age, day == today) {
                            (Some(age), true) if age > 0 => tf(TURNS_TODAY, &[("age", &age.to_string())]),
                            (Some(age), false) if age > 0 => tf(TURNS, &[("age", &age.to_string())]),
                            (_, true) => tl(BIRTHDAY_TODAY).to_string(),
                            _ => tl(BIRTHDAY).to_string(),
                        };
                        ui.label(RichText::new(line).small().color(ROSE));
                    });
                });
                if let Some(email) = &b.email
                    && day >= today
                {
                    ui.add_space(4.0);
                    if chrome::button(ui, &t, chrome::Kind::Secondary, Some(Icon::Pencil), WISHES, true).on_hover_text(tl(WISHES_TIP)).clicked() {
                        write = Some(email.clone());
                    }
                }
            });
            confetti(ui.painter(), Rect::from_min_size(card.response.rect.right_top() + vec2(-70.0, 10.0), vec2(60.0, 40.0)), b.contact);
            ui.add_space(8.0);
        }
        if let Some(to) = write {
            self.run("message.compose", serde_json::json!({ "to": to, "subject": tl(SUBJECT) }));
            self.run("view.mail", serde_json::json!({}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, has};

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

    #[test]
    fn a_chip_names_the_age_when_it_is_known() {
        let d = ibercorreo_core::Date::new(2026, 3, 9).unwrap();
        let b = Birthday { contact: 1, name: "Olena".into(), date: d, age: Some(37), email: None };
        assert_eq!(label(&b), "Olena · 37");
        assert_eq!(label(&Birthday { age: None, ..b.clone() }), "Olena");
        assert_eq!(label(&Birthday { age: Some(0), ..b.clone() }), "Olena", "born that day");
        let full = Birthday { name: "Olena Koval".into(), ..b };
        assert_eq!((chip_label(&full, 80.0), chip_label(&full, 200.0)), ("Olena · 37".to_string(), "Olena Koval · 37".to_string()));
    }
}
