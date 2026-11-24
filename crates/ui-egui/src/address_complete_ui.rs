//! Addresses (Tinta): Add to contacts beside the sender of the message being read (and on its
//! context menu), and the addresses offered under a To, Cc or Bcc field while it is typed, as in
//! Outlook: ↑ and ↓ choose, Enter or Tab puts the address in, Esc closes the list. What is offered
//! comes from the engine (`contact.suggest`, [`ibercorreo_engine::addresses::suggest`]).

use egui::{Align2, Key, Order, RichText, Ui};
use ibercorreo_engine::Session;
use ibercorreo_engine::addresses::{self, Suggestion};

use crate::chrome;
use crate::i18n::tl;
use crate::theme::Tokens;

pub(crate) const ADD: &str = "Add to contacts";
pub(crate) const ADD_TIP: &str = "Save the sender in your contacts";
pub(crate) const IN_CONTACTS: &str = "In your contacts";
pub(crate) const COPY: &str = "Copy address";
/// Said by `contact.addSender` (the engine) in the status bar, which translates it.
#[cfg(test)]
const ALREADY: &str = "Already in your contacts";
pub(crate) const HOW: &str = "↑↓ to choose, Enter to use";

#[cfg(test)]
const STRINGS: [&str; 6] = [ADD, ADD_TIP, IN_CONTACTS, COPY, ALREADY, HOW];

/// The most addresses the list under a field shows.
const SHOWN: usize = 8;

/// The list open under one recipient field.
#[derive(Default)]
pub(crate) struct Completion {
    /// Which field (`To`, `Cc`, `Bcc`) of which draft.
    field: Option<(u64, &'static str)>,
    /// What it was looked up for.
    typed: String,
    items: Vec<Suggestion>,
    selected: usize,
}

/// The field's text before its last part (up to and including the last comma or semicolon) and
/// that part, trimmed at its start.
pub(crate) fn split_last(text: &str) -> (&str, &str) {
    match text.rfind([',', ';']) {
        Some(at) => {
            let (head, tail) = text.split_at(at.saturating_add(1));
            (head, tail.trim_start())
        }
        None => ("", text.trim_start()),
    }
}

/// `text` with its last part replaced by `s`, ready for the next address.
pub(crate) fn accept(text: &str, s: &Suggestion) -> String {
    let (head, _) = split_last(text);
    let head = head.trim_end();
    if head.is_empty() { format!("{}, ", s.field_text()) } else { format!("{head} {}, ", s.field_text()) }
}

/// The id of recipient field `key`, one per draft and field.
pub(crate) fn field_id(key: (u64, &'static str)) -> egui::Id {
    egui::Id::new(("recipient-field", key.0, key.1))
}

/// Keys for the open list, taken before the field sees them; returns the address to put in.
fn keys(ui: &Ui, c: &mut Completion) -> Option<Suggestion> {
    let n = c.items.len();
    if n == 0 {
        return None;
    }
    let (down, up, enter, tab, esc) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, Key::Enter),
            i.consume_key(egui::Modifiers::NONE, Key::Tab),
            i.consume_key(egui::Modifiers::NONE, Key::Escape),
        )
    });
    if down {
        c.selected = (c.selected + 1) % n;
    }
    if up {
        c.selected = c.selected.checked_sub(1).unwrap_or(n - 1);
    }
    if esc {
        c.items.clear();
        return None;
    }
    if enter || tab { c.items.get(c.selected).cloned() } else { None }
}

/// One recipient field (in `font`, `width` wide) with its list. Returns whether the text changed by an address put in.
pub(crate) fn field(
    ui: &mut Ui,
    t: &Tokens,
    session: &Session,
    c: &mut Completion,
    key: (u64, &'static str),
    text: &mut String,
    (font, width): (egui::FontId, f32),
) -> (egui::Response, bool) {
    let id = field_id(key);
    let mine = c.field == Some(key);
    let focused = ui.memory(|m| m.has_focus(id));
    let mut chosen = if mine && focused { keys(ui, c) } else { None };
    let resp = ui.add(egui::TextEdit::singleline(text).id(id).frame(egui::Frame::NONE).font(font).desired_width(width));
    let typed = split_last(text).1.to_string();
    if resp.has_focus() && (!mine || c.typed != typed) && chosen.is_none() {
        c.field = Some(key);
        c.items = addresses::suggest(session, &typed, SHOWN);
        // An address typed in full needs no list.
        c.items.retain(|s| !s.email.eq_ignore_ascii_case(typed.trim()));
        c.typed = typed;
        c.selected = 0;
    }
    let mine = c.field == Some(key);
    if mine && !c.items.is_empty() && (resp.has_focus() || focused) {
        let area = egui::Area::new(id.with("list")).order(Order::Foreground).pivot(Align2::LEFT_TOP).fixed_pos(resp.rect.left_bottom());
        let shown = area.show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(resp.rect.width().clamp(220.0, 420.0));
                for (i, s) in c.items.iter().enumerate() {
                    let label = match &s.name {
                        Some(n) => RichText::new(format!("{n}   {}", s.email)),
                        None => RichText::new(&s.email),
                    };
                    let r = ui.selectable_label(i == c.selected, label.color(t.text));
                    if r.hovered() {
                        c.selected = i;
                    }
                    if r.clicked() {
                        chosen = Some(s.clone());
                    }
                }
                ui.label(RichText::new(tl(HOW)).small().color(t.text_weak));
            });
        });
        if !resp.has_focus() && !shown.response.contains_pointer() && chosen.is_none() {
            c.items.clear();
        }
    } else if mine && !resp.has_focus() {
        c.items.clear();
    }
    let Some(s) = chosen else { return (resp, false) };
    *text = accept(text, &s);
    c.typed = split_last(text).1.to_string();
    c.items.clear();
    // Back in the field, the cursor after the address.
    resp.request_focus();
    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
        let end = egui::text::CCursor::new(text.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ui.ctx(), id);
    }
    (resp, true)
}

/// Add to contacts (or a note that the sender is one) beside the sender's name.
pub(crate) fn add_sender_button(ui: &mut Ui, t: &Tokens, session: &Session, email: &str) -> bool {
    if addresses::contact_with(session, email).is_some() {
        ui.label(RichText::new(format!("✓ {}", tl(IN_CONTACTS))).small().color(t.text_weak));
        return false;
    }
    chrome::button(ui, t, chrome::Kind::Quiet, None, ADD, true).on_hover_text(tl(ADD_TIP)).clicked()
}

/// The sender's context menu: Add to contacts and Copy address. Returns whether Add was chosen.
pub(crate) fn sender_menu(resp: &egui::Response, session: &Session, email: &str) -> bool {
    let mut add = false;
    let known = addresses::contact_with(session, email).is_some();
    resp.context_menu(|ui| {
        if ui.add_enabled(!known, egui::Button::new(tl(ADD))).clicked() {
            add = true;
            ui.close();
        }
        if ui.button(tl(COPY)).clicked() {
            ui.ctx().copy_text(email.to_string());
            ui.close();
        }
    });
    add
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, has};

    /// Every string is translated in every language the app offers.
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
    fn the_last_part_is_replaced_and_the_rest_kept() {
        let s = Suggestion { name: Some("Marta Iglesias".into()), email: "marta@example.es".into(), contact: Some(1) };
        assert_eq!(split_last("ana@example.org, mar"), ("ana@example.org,", "mar"));
        assert_eq!(split_last("  mar"), ("", "mar"));
        assert_eq!(accept("mar", &s), "Marta Iglesias <marta@example.es>, ");
        assert_eq!(accept("ana@example.org;  mar", &s), "ana@example.org; Marta Iglesias <marta@example.es>, ");
        let bare = Suggestion { name: None, email: "x@example.org".into(), contact: None };
        assert_eq!(accept("Ana <a@b.c>, x", &bare), "Ana <a@b.c>, x@example.org, ");
    }
}
