//! Pictures inside an HTML message (Tinta): a logo or a photo that the message carries as a part
//! of its own and names with `cid:` (RFC 2392). The reader shows it in its place, as other mail
//! programs do, instead of an "image" placeholder. Pictures on the web are still never loaded.
//!
//! Only parts whose contents Correo keeps ([`ibercorreo_store::files`]) can be shown; mail that
//! arrived before contents were kept keeps its placeholder. A picture is decoded once per
//! message, with limits on its size and memory ([`decode`]): a hostile file gives a placeholder,
//! never a crash or a huge allocation. A part shown in the text has no chip of its own under the
//! headers; its context menu still saves it.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use ibercorreo_core::rich::Rich;
use ibercorreo_core::{Message, MessageId};
use ibercorreo_engine::Session;

/// Most pixels across or down a picture may have to be decoded at all.
const MAX_SIDE: u32 = 8_192;
/// Most bytes the decoder may allocate for one picture.
const MAX_ALLOC: u64 = 128 * 1024 * 1024;
/// Pictures wider or taller than this are scaled down before they become a texture.
const MAX_SHOWN: u32 = 2_048;
/// Most pictures shown in one message; the rest keep their placeholders.
const MAX_PICTURES: usize = 64;
/// Most bytes of a part read to show it.
const MAX_FILE: usize = 32 * 1024 * 1024;

/// The pictures of the message being read.
#[derive(Default)]
pub(crate) struct InlineImages {
    message: Option<MessageId>,
    /// Decoded pictures by content id; `None` for one that could not be shown.
    by_cid: HashMap<String, Option<TextureHandle>>,
    /// The parts shown in the text, by their place in the message's attachments.
    shown: HashSet<usize>,
    /// Where each content id is among the message's attachments.
    index: HashMap<String, usize>,
}

impl InlineImages {
    /// Decode the pictures `rich` names that `m` carries, once per message.
    pub(crate) fn prepare(&mut self, ctx: &Context, session: &Session, m: &Message, rich: Option<&Rich>) {
        if self.message != Some(m.id) {
            *self = InlineImages { message: Some(m.id), ..InlineImages::default() };
        }
        let Some(rich) = rich else { return };
        for cid in rich.blocks.iter().flat_map(|b| &b.spans).filter(|s| s.image).filter_map(|s| s.cid.as_deref()) {
            if self.by_cid.contains_key(cid) || self.by_cid.len() >= MAX_PICTURES {
                continue;
            }
            let found = m.attachments.iter().position(|a| a.content_id.as_deref() == Some(cid) && a.sha256.is_some());
            let texture = found.and_then(|index| {
                let a = m.attachments.get(index)?;
                if a.size > MAX_FILE || !a.mime_type.to_ascii_lowercase().starts_with("image/") {
                    return None;
                }
                let (_, bytes) = ibercorreo_engine::attachments::contents(&session.store, m.id, index).ok()?;
                let image = decode(&bytes)?;
                Some(ctx.load_texture(format!("cid:{}:{cid}", m.id.0), image, TextureOptions::LINEAR))
            });
            if let Some(index) = found
                && texture.is_some()
            {
                self.shown.insert(index);
                self.index.insert(cid.to_string(), index);
            }
            self.by_cid.insert(cid.to_string(), texture);
        }
    }

    /// The picture for `cid`, when it could be shown.
    pub(crate) fn get(&self, cid: &str) -> Option<&TextureHandle> {
        self.by_cid.get(cid).and_then(Option::as_ref)
    }

    /// Where the part `cid` names is among the message's attachments, when it is shown.
    pub(crate) fn attachment(&self, cid: &str) -> Option<usize> {
        self.index.get(cid).copied()
    }

    /// The attachments shown in the text, which need no chip.
    pub(crate) fn shown(&self) -> &HashSet<usize> {
        &self.shown
    }
}

/// A picture file (PNG, JPEG, GIF's first frame, BMP or WebP) as pixels, scaled to at most
/// [`MAX_SHOWN`] on its longer side. `None` for anything else, a broken file, or one whose size or
/// memory would pass the limits.
pub(crate) fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let picture = reader.decode().ok()?;
    let picture = if picture.width() > MAX_SHOWN || picture.height() > MAX_SHOWN { picture.thumbnail(MAX_SHOWN, MAX_SHOWN) } else { picture };
    let rgba = picture.to_rgba8();
    let size = [usize::try_from(rgba.width()).ok()?, usize::try_from(rgba.height()).ok()?];
    if size.contains(&0) {
        return None;
    }
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w`×`h` PNG, every pixel the same.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(&vec![200u8; (w * h * 4) as usize]).unwrap();
        writer.finish().unwrap();
        out
    }

    #[test]
    fn pictures_decode_and_large_ones_are_scaled_down() {
        let small = decode(&png(3, 2)).unwrap();
        assert_eq!(small.size, [3, 2]);
        let wide = decode(&png(4_000, 10)).unwrap();
        assert_eq!(wide.size.first(), Some(&2_048), "scaled to fit");
    }

    #[test]
    fn hostile_files_give_nothing_and_never_panic() {
        for bytes in [
            &b""[..],
            b"not a picture",
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\x7f\xff\xff\xff\x7f\xff\xff\xff\x08\x06\0\0\0",
            b"\xff\xd8\xff\xe0\0\x10JFIF",
        ] {
            assert!(decode(bytes).is_none(), "{bytes:?}");
        }
        let mut cut = png(20, 20);
        cut.truncate(cut.len() / 2);
        assert!(decode(&cut).is_none(), "a cut file");
    }

    #[test]
    fn a_message_shows_the_pictures_it_carries_and_only_those() {
        let today = ibercorreo_core::Date::new(2026, 11, 15).unwrap();
        let mut session = Session::new(ibercorreo_engine::sample::store(today).unwrap(), today);
        let id = session.store.all_messages().next().map(|m| m.id).unwrap();
        let hash = session.store.keep_file(png(2, 2));
        assert!(hash.is_some());
        let m = session.store.message_mut(id).unwrap();
        m.attachments = vec![
            ibercorreo_core::mail::Attachment {
                name: "logo.png".into(),
                mime_type: "image/png".into(),
                size: 10,
                sha256: hash,
                content_id: Some("logo@x".into()),
            },
            ibercorreo_core::mail::Attachment { name: "factura.pdf".into(), mime_type: "application/pdf".into(), ..Default::default() },
        ];
        let rich = ibercorreo_mime_free_rich(&["logo@x", "other@x"]);
        let ctx = Context::default();
        let mut images = InlineImages::default();
        let m = session.store.message(id).unwrap().clone();
        images.prepare(&ctx, &session, &m, Some(&rich));
        assert!(images.get("logo@x").is_some(), "the kept logo is drawn");
        assert!(images.get("other@x").is_none(), "a part the message lacks keeps its placeholder");
        assert_eq!(images.attachment("logo@x"), Some(0));
        assert_eq!(images.shown().iter().copied().collect::<Vec<_>>(), [0], "the logo needs no chip, the invoice does");
    }

    /// A formatted body with one picture per content id.
    fn ibercorreo_mime_free_rich(cids: &[&str]) -> Rich {
        use ibercorreo_core::rich::{Block, BlockKind, Span};
        let spans = cids.iter().map(|c| Span { text: "image".into(), image: true, cid: Some((*c).into()), ..Span::default() }).collect();
        Rich { blocks: vec![Block { kind: BlockKind::Paragraph, quote: 0, spans }], remote_images: 0 }
    }
}
