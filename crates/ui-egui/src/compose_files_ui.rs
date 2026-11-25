//! Files attached to a draft (Tinta): Attach file beside Send, a chip for each file with its size
//! and a × that takes it off, and files dropped on the window while a draft is open. The window
//! only asks; the shell shows the system's file picker and reads the files
//! ([`ibercorreo_engine::attachments::read_to_attach`]), then attaches each one with
//! `message.attach`. Paste beside them attaches the files or the picture on the clipboard: the
//! shell reads it ([`App::take_paste_request`]) and hands on a picture's pixels
//! ([`App::paste_picture`]), saved as a PNG named after the time it was pasted.

use egui::{CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use ibercorreo_core::MessageId;

use crate::App;
use crate::i18n::{tf, tl};
use crate::theme::Tokens;

pub(crate) const ATTACH: &str = "Attach file";
pub(crate) const ATTACH_TIP: &str = "Attach files to this message (or drop them on the window)";
pub(crate) const REMOVE_TIP: &str = "Remove this attachment";
pub(crate) const ATTACH_FAILED: &str = "Could not attach {name}: {error}";
pub(crate) const NO_PICKER: &str = "No file picker on this system: drop the files on the window instead";
pub(crate) const TOTAL: &str = "{count} files, {size}";
pub(crate) const ATTACHED: &str = "Attached {name}";
pub(crate) const PASTE: &str = "Paste";
pub(crate) const PASTE_TIP: &str = "Attach the files or the picture you copied";
pub(crate) const NOTHING: &str = "Nothing to paste: copy a file or a picture first";
pub(crate) const PICTURE: &str = "Picture {time}";
/// Said by `message.forward` (the engine) in the status bar, which translates it.
#[cfg(test)]
const LEFT_OUT: &str = "Some attachments are not kept on this computer and were left out";

#[cfg(test)]
const STRINGS: [&str; 12] = [ATTACH, ATTACH_TIP, REMOVE_TIP, ATTACH_FAILED, NO_PICKER, TOTAL, ATTACHED, LEFT_OUT, PASTE, PASTE_TIP, NOTHING, PICTURE];

/// The most pixels a pasted picture may have (a 16K screen's worth); its PNG must still fit the
/// draft's limit.
const MAX_PIXELS: usize = 16_384 * 8_192;

impl App {
    /// The draft the person asked to attach files to since the last call. Taken once.
    pub fn take_attach_request(&mut self) -> Option<MessageId> {
        self.attach_wanted.take()
    }

    /// The draft being written, if one is open: files dropped on the window go to it.
    pub fn composing(&self) -> Option<MessageId> {
        self.draft.as_ref().map(|d| MessageId(d.id))
    }

    /// Attach a file the shell read to draft `id` (`name` and its contents).
    pub fn attach(&mut self, id: MessageId, name: &str, bytes: &[u8]) {
        match ibercorreo_engine::attachments::attach(&mut self.session, id, name, bytes) {
            Ok(()) => self.session.status = tf(ATTACHED, &[("name", name)]),
            Err(e) => self.attach_failed(name, &e.to_string()),
        }
    }

    /// Reading or attaching `name` failed.
    pub fn attach_failed(&mut self, name: &str, error: &str) {
        self.session.status = tf(ATTACH_FAILED, &[("name", name), ("error", error)]);
    }

    /// Ask to paste the clipboard into draft `id`, as the Paste button does (Ctrl+V with no text
    /// on the clipboard, which the window itself ignores).
    pub fn request_paste(&mut self, id: MessageId) {
        self.paste_wanted = Some(id);
    }

    /// The draft the person asked to paste the clipboard into since the last call. Taken once.
    pub fn take_paste_request(&mut self) -> Option<MessageId> {
        self.paste_wanted.take()
    }

    /// Attach a picture from the clipboard to draft `id`: `width`×`height` pixels of RGBA.
    pub fn paste_picture(&mut self, id: MessageId, width: usize, height: usize, rgba: &[u8]) {
        let now = self.session.zone().from_utc(self.session.now());
        let time = format!("{} {:02}.{:02}.{:02}", now.date, now.hour(), now.minute(), now.second());
        let name = format!("{}.png", tf(PICTURE, &[("time", &time)]));
        match encode_png(width, height, rgba) {
            Ok(bytes) => self.attach(id, &name, &bytes),
            Err(e) => self.attach_failed(&name, &e),
        }
    }

    /// The clipboard held neither files nor a picture.
    pub fn paste_nothing(&mut self) {
        self.session.status = tl(NOTHING).to_string();
    }

    /// The system has no file picker to show.
    pub fn attach_no_picker(&mut self) {
        self.session.status = tl(NO_PICKER).to_string();
    }
}

/// `width`×`height` RGBA pixels as a PNG file.
pub(crate) fn encode_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let pixels = width.checked_mul(height).filter(|&n| n > 0 && n <= MAX_PIXELS).ok_or("the picture is empty or too large")?;
    if pixels.checked_mul(4) != Some(rgba.len()) {
        return Err("the picture's pixels do not match its size".into());
    }
    let (w, h) = (u32::try_from(width).map_err(|e| e.to_string())?, u32::try_from(height).map_err(|e| e.to_string())?);
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// The chips of the files on draft `id`, each with ×; returns the index the person took off.
pub(crate) fn chips(ui: &mut Ui, t: &Tokens, app: &App, id: MessageId) -> Option<usize> {
    let m = app.session.store.message(id).ok()?;
    if m.attachments.is_empty() {
        return None;
    }
    let mut removed = None;
    ui.horizontal_wrapped(|ui| {
        for (index, a) in m.attachments.iter().enumerate() {
            Frame::NONE.stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 4)).show(
                ui,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("📎 {}  ·  {}", a.name, crate::human_size(a.size))).small());
                        let x = egui::Button::new(RichText::new("×").small().color(t.text_weak)).frame(false);
                        if ui.add(x).on_hover_text(tl(REMOVE_TIP)).clicked() {
                            removed = Some(index);
                        }
                    });
                },
            );
        }
        if m.attachments.len() > 1 {
            let total = m.attachments.iter().map(|a| a.size).fold(0usize, usize::saturating_add);
            let count = m.attachments.len().to_string();
            ui.label(RichText::new(tf(TOTAL, &[("count", &count), ("size", &crate::human_size(total))])).small().color(t.text_weak));
        }
    });
    removed
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
    fn a_pasted_picture_is_attached_as_a_png_named_after_the_time() {
        let day = ibercorreo_core::Date::new(2026, 11, 24).unwrap();
        let mut s = ibercorreo_engine::Session::new(ibercorreo_store::Store::new(), day);
        s.set_now(ibercorreo_core::DateTime::new(day, 22, 41, 5).unwrap());
        s.run("calendar.timeZone", &serde_json::json!({ "zone": "UTC" })).unwrap();
        let id = MessageId(s.run("message.compose", &serde_json::json!({})).unwrap()["id"].as_u64().unwrap());
        let mut app = App::new(s);
        app.paste_picture(id, 3, 2, &[255u8; 24]);
        let m = app.session.store.message(id).unwrap();
        assert_eq!(m.attachments.len(), 1, "{}", app.session.status);
        assert_eq!(m.attachments[0].name, "Picture 2026-11-24 22.41.05.png");
        let (_, bytes) = ibercorreo_engine::attachments::contents(&app.session.store, id, 0).unwrap();
        assert!(bytes.starts_with(b"\x89PNG"), "a PNG");
        // Pixels that do not match the size, an empty or a huge picture: refused, never a panic.
        for (w, h, len) in [(3, 2, 23), (0, 0, 0), (usize::MAX, 2, 8), (40_000, 40_000, 4)] {
            app.paste_picture(id, w, h, &vec![0u8; len]);
        }
        assert_eq!(app.session.store.message(id).unwrap().attachments.len(), 1, "nothing more attached");
        assert!(app.session.status.starts_with("Could not attach"), "{}", app.session.status);
        app.paste_nothing();
        assert_eq!(app.session.status, NOTHING);
    }
}
