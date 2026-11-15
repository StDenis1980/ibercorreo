//! Pictures on the web in HTML mail (Tinta): never downloaded unless the person asks, for one
//! message (until Tinta Correo closes) or always for its sender (`message.loadPictures`). The
//! window asks the shell to download them ([`App::take_picture_requests`]) and hands back what
//! came ([`App::picture_loaded`]); tracking pixels are never downloaded at all.

use ibercorreo_core::MessageId;

use crate::App;

pub(crate) const WHY: &str =
    "Pictures on other servers can tell the sender that you opened the message, so Tinta Correo downloads them only when you ask.";
pub(crate) const DOWNLOAD: &str = "Download pictures";
pub(crate) const ALWAYS: &str = "Always for this sender";
pub(crate) const SHOWN: &str = "Pictures from the web are shown";
pub(crate) const STOP: &str = "Stop for this sender";

/// The strings this module shows, for the translation test.
#[cfg(test)]
const STRINGS: &[&str] = &[WHY, DOWNLOAD, ALWAYS, SHOWN, STOP];

impl App {
    /// Pictures on the web the person chose to see, to download: the message and each address.
    /// Taken once.
    pub fn take_picture_requests(&mut self) -> Vec<(MessageId, String)> {
        self.inline_images.take_requests()
    }

    /// The shell downloaded the picture at `url` for message `id`, or could not.
    pub fn picture_loaded(&mut self, id: MessageId, url: String, result: Result<Vec<u8>, String>) {
        self.inline_images.arrived(id, url, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, has};
    use ibercorreo_core::{Date, FolderRole};
    use ibercorreo_engine::Session;
    use serde_json::json;

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
    fn pictures_are_downloaded_only_after_the_person_asks() {
        let today = Date::new(2026, 11, 15).unwrap();
        let mut session = Session::new(ibercorreo_engine::sample::store(today).unwrap(), today);
        let inbox = session.store.folder_by_role(ibercorreo_core::AccountId(0), FolderRole::Inbox).unwrap();
        let raw = "From: Shop <News@Shop.example>\r\nSubject: Ofertas\r\nContent-Type: text/html\r\n\r\n\
                   <p><img src=\"https://cdn.shop.example/banner.png\" alt=\"Banner\">\
                   <img src=\"https://track.example/p.gif\" width=\"1\" height=\"1\"></p>";
        let id = session.store.import_eml(raw.as_bytes(), inbox).unwrap();
        session.message = Some(id);
        let mut app = App::new(session);
        let ctx = egui::Context::default();
        let read = |app: &mut App| {
            let m = app.session.store.message(id).unwrap().clone();
            let content = app.session.store.content(id).unwrap();
            app.inline_images.prepare(&ctx, &app.session, &m, content.rich.as_ref());
        };
        read(&mut app);
        assert!(app.take_picture_requests().is_empty(), "nothing before the person asks");

        app.run("message.loadPictures", json!({}));
        read(&mut app);
        assert_eq!(app.take_picture_requests(), [(id, "https://cdn.shop.example/banner.png".to_string())], "the banner, never the tracker");
        read(&mut app);
        assert!(app.take_picture_requests().is_empty(), "asked once");

        // A broken download keeps the placeholder; a picture shows.
        app.picture_loaded(id, "https://cdn.shop.example/banner.png".into(), Err("HTTP 404".into()));
        read(&mut app);
        assert!(app.inline_images.web("https://cdn.shop.example/banner.png").is_none());

        // Always for the sender: remembered in the mailbox, undoable, and "never" forgets it.
        app.run("message.loadPictures", json!({ "always": true }));
        assert_eq!(app.session.store.pictures_from, ["news@shop.example"]);
        app.run("message.loadPictures", json!({ "always": false }));
        assert!(app.session.store.pictures_from.is_empty());
        assert!(!app.session.pictures_allowed(id), "never again, also for this message");
    }
}
