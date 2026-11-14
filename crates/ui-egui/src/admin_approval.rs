//! Microsoft's "Need admin approval" (Tinta). Some organisations let only an administrator approve
//! a new program. Microsoft's page then asks the person to find one, and Tinta Correo cannot sign
//! in until an administrator has approved it once. While the browser sign-in is waiting, and after
//! Microsoft answered that way, Account settings says what to do: an administrator opens
//! Microsoft's approval page for the organisation; anybody else copies a short message for one.
//! Then the person signs in again.
//!
//! The approval page's address names the organisation by its mail domain and Tinta Correo by its
//! application id, and nothing else: not the person's address or name.

use egui::{RichText, Ui};

use crate::App;
use crate::chrome::{self, Kind};
use crate::i18n::{tf, tl};
use crate::theme::Tokens;

pub(crate) const QUESTION: &str = "Does Microsoft say “Need admin approval”?";
pub(crate) const EXPLAIN: &str = "Your organisation lets only its administrator approve new programs. An administrator approves Tinta Correo once on Microsoft's page; then sign in again here.";
pub(crate) const COPY: &str = "Copy a message for your administrator";
pub(crate) const OPEN: &str = "I am an administrator: open the approval page";
pub(crate) const NOTE: &str = "Microsoft needs an administrator of your organisation to approve Tinta Correo first. Then sign in again.";
pub(crate) const MESSAGE: &str = "Hello,\n\nPlease approve the mail program Tinta Correo for our organisation in Microsoft Entra, so that we can use our mail, calendar and contacts in it. An administrator opens this page once and accepts:\n{link}\n\nTinta Correo is free and open source (https://tintaoffice.com). Its application ID is {id}.\n\nThank you.";

/// The strings this module shows, for the translation test.
#[cfg(test)]
const STRINGS: &[&str] = &[QUESTION, EXPLAIN, COPY, OPEN, NOTE, MESSAGE];

/// The application id in a Microsoft sign-in address, when it is one.
pub(crate) fn client_id(sign_in_url: &str) -> Option<String> {
    let rest = sign_in_url.strip_prefix("https://login.microsoftonline.com/")?;
    let query = rest.split_once('?')?.1;
    let id = query.split('&').find_map(|kv| kv.strip_prefix("client_id="))?;
    // An application id is a GUID: anything else is not passed on.
    (id.len() <= 64 && !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')).then(|| id.to_ascii_lowercase())
}

/// Microsoft's page where an administrator of `email`'s organisation approves the application
/// `client_id` for everyone in it. `None` when the address has no usable domain.
pub(crate) fn approval_url(email: &str, client_id: &str) -> Option<String> {
    let domain = email.trim().rsplit_once('@')?.1.trim().to_ascii_lowercase();
    let ok = !domain.is_empty()
        && domain.len() <= 253
        && domain.contains('.')
        && domain.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && !domain.starts_with(['.', '-']);
    ok.then(|| format!("https://login.microsoftonline.com/{domain}/adminconsent?client_id={client_id}"))
}

/// The message for an administrator, in the interface's language.
pub(crate) fn message(email: &str, client_id: &str) -> String {
    let link = approval_url(email, client_id).unwrap_or_else(|| "https://entra.microsoft.com".into());
    tf(MESSAGE, &[("link", &link), ("id", client_id)])
}

impl App {
    /// Microsoft answered that an administrator has to approve Tinta Correo first.
    pub fn apply_oauth_needs_admin(&mut self) {
        let id = self.oauth_running.as_deref().and_then(client_id);
        self.oauth_running = None;
        if id.is_some() {
            self.account_form.admin_client = id;
        }
        self.account_form.note = tl(NOTE).to_string();
    }
}

/// What to do about it, for the application `client_id` and the account `email`. `open` asks for
/// the approval page; the message goes to the clipboard here.
pub(crate) fn help(ui: &mut Ui, t: &Tokens, email: &str, client_id: &str, open: &mut Option<String>) {
    ui.add(egui::Label::new(RichText::new(tl(QUESTION)).strong().color(t.text)).wrap());
    ui.add(egui::Label::new(RichText::new(tl(EXPLAIN)).small().color(t.text_weak)).wrap());
    ui.horizontal_wrapped(|ui| {
        if chrome::button(ui, t, Kind::Quiet, None, COPY, true).clicked() {
            ui.ctx().copy_text(message(email, client_id));
        }
        if let Some(url) = approval_url(email, client_id)
            && chrome::button(ui, t, Kind::Quiet, None, OPEN, true).clicked()
        {
            *open = Some(url);
        }
    });
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
    fn the_approval_page_names_the_organisation_and_the_program_only() {
        let url = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?response_type=code&client_id=A6A32CFA-d28b-49f6-8012-bf7bc300c210&login_hint=ana%40empresa.es";
        let id = client_id(url).unwrap();
        assert_eq!(id, "a6a32cfa-d28b-49f6-8012-bf7bc300c210");
        assert_eq!(
            approval_url(" Ana@Empresa.ES ", &id).as_deref(),
            Some("https://login.microsoftonline.com/empresa.es/adminconsent?client_id=a6a32cfa-d28b-49f6-8012-bf7bc300c210")
        );
        assert!(!message("ana@empresa.es", &id).contains("ana@"), "the person's address is not passed on");
        assert!(message("ana@empresa.es", &id).contains("/empresa.es/adminconsent"));
        // Not Microsoft, or a hostile id or domain: nothing.
        assert!(client_id("https://accounts.google.com/o/oauth2/v2/auth?client_id=1").is_none());
        assert!(client_id("https://login.microsoftonline.com/x?client_id=a&b=c").is_some());
        assert!(client_id("https://login.microsoftonline.com/x?client_id=a%2F..%2Fevil").is_none());
        for bad in ["ana", "ana@", "ana@localhost", "ana@-x.es", "ana@x.es/../y", "ana@x.es?y"] {
            assert!(approval_url(bad, &id).is_none(), "{bad}");
        }
    }
}
