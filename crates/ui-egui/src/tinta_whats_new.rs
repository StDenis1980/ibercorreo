//! Tinta Office: the "What's new in Preview 4" window. The desktop app turns it on with its
//! settings folder ([`enable`]); it then opens by itself once, the first time this release starts
//! (a small file in that folder remembers it, [`SEEN_FILE`]), and again whenever the What's new
//! command asks ([`request`]). Close, the ✕, Esc or a click outside closes it. Without [`enable`]
//! (upstream's builds, tests, the web, the command line) it never opens, and the command says so.
//!
//! The same file lives in all five Tinta apps, each with its own list, written from that app's
//! release notes (`docs/tinta-releases/preview-4.md`). Its strings are translated here, not in the
//! shared catalogues, so syncing with upstream never conflicts: the list in Spanish and Ukrainian,
//! as the notes are (Catalan, Galician and Basque read the Spanish list), the title and buttons in
//! every interface language Tinta ships; anything else reads English.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Id, RichText};

use crate::chrome::{self, Kind, Part};
use crate::theme::Tokens;

/// The release this list describes. A later release's list opens by itself once again.
pub const RELEASE: &str = "preview-4";
/// In the settings folder: the release whose list already opened by itself.
pub const SEEN_FILE: &str = "tinta-whats-new.txt";
/// The app the list is about.
const APP: &str = "Tinta Correo";
const MAX_WIDTH: f32 = 520.0;

/// Set by [`enable`]: the settings folder, or `None` when this run keeps no settings (the window
/// still opens on request, but never by itself).
static ENABLED: OnceLock<Option<PathBuf>> = OnceLock::new();
/// Asked for (by the command, or due at start) without a `Context` at hand: opens on the next frame.
static REQUESTED: AtomicBool = AtomicBool::new(false);

/// The desktop app: turn the window on. It opens on the first frame when this release's list has
/// not opened by itself before in `settings`.
pub fn enable(settings: Option<PathBuf>) {
    let due = settings.as_deref().is_some_and(|dir| !seen(dir));
    if ENABLED.set(settings).is_ok() && due {
        REQUESTED.store(true, Ordering::Relaxed);
    }
}

/// Whether this build shows the window (the desktop app turned it on).
pub fn available() -> bool {
    ENABLED.get().is_some()
}

/// The What's new command: open the window on the next frame. `false` when this build has no
/// such window (nothing opens).
pub fn request() -> bool {
    let on = available();
    if on {
        REQUESTED.store(true, Ordering::Relaxed);
    }
    on
}

/// Whether this release's list already opened by itself with these settings.
pub fn seen(settings: &Path) -> bool {
    std::fs::read_to_string(settings.join(SEEN_FILE)).is_ok_and(|s| s.trim() == RELEASE)
}

/// Remember that this release's list opened; `false` when the file could not be written (the
/// list then opens again next start).
pub fn remember(settings: &Path) -> bool {
    std::fs::create_dir_all(settings).and_then(|()| std::fs::write(settings.join(SEEN_FILE), RELEASE)).is_ok()
}

/// Each frame, after the window's own dialogs: opens what was asked for and draws the window.
pub fn show(ctx: &egui::Context, lang: &str, t: &Tokens) {
    if REQUESTED.swap(false, Ordering::Relaxed) {
        open(ctx);
        if let Some(Some(dir)) = ENABLED.get() {
            let _ = remember(dir);
        }
    }
    draw(ctx, lang, t);
}

fn id() -> Id {
    Id::new("tinta.whatsNew")
}

/// Open the window in `ctx` (its Close button takes the keyboard focus).
pub fn open(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(id(), true));
}

pub fn is_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(id())).is_some()
}

/// Draw the window when it is open, in the app's dialog frame: the list, and Close.
pub fn draw(ctx: &egui::Context, lang: &str, t: &Tokens) {
    // `true` until Close has taken the focus once.
    let Some(focus) = ctx.data(|d| d.get_temp::<bool>(id())) else {
        return;
    };
    let list = list(lang);
    let (mut keep, mut closed) = (true, false);
    let modal = chrome::modal_dialog(ctx, t, "tinta.whatsNew", tr(lang, TITLE), MAX_WIDTH, Some(&mut keep), |ui, part| match part {
        Part::Body => {
            ui.label(RichText::new(APP).color(t.text_weak));
            ui.label(list.intro);
            ui.add_space(8.0);
            for (title, text) in list.items {
                ui.label(RichText::new(*title).strong().color(t.text));
                ui.label(*text);
                ui.add_space(6.0);
            }
        }
        Part::Footer => chrome::footer_right(ui, |ui| {
            let close = chrome::button(ui, t, Kind::Primary, None, tr(lang, CLOSE), true);
            if focus {
                close.request_focus();
            }
            closed |= close.clicked();
        }),
    });
    if !keep || closed || modal.should_close() {
        ctx.data_mut(|d| d.remove::<bool>(id()));
    } else if focus {
        ctx.data_mut(|d| d.insert_temp(id(), false));
    }
}

/// What's new: one release's highlights in one language.
struct List {
    /// What kind of version this is.
    intro: &'static str,
    /// Each change: a short title and a sentence or two.
    items: &'static [(&'static str, &'static str)],
}

/// The list in `lang`: Spanish also for Catalan, Galician and Basque, English when there is none.
fn list(lang: &str) -> &'static List {
    match base(lang).as_str() {
        "es" | "ca" | "gl" | "eu" => &ES,
        "uk" => &UK,
        _ => &EN,
    }
}

/// `es` for `es-ES`, `ca` for `ca-valencia`, `pt` for `pt_BR`.
fn base(lang: &str) -> String {
    lang.split(['-', '_']).next().unwrap_or(lang).to_ascii_lowercase()
}

/// English: the source.
const EN: List = List {
    intro: "A test version: keep your current mail app as well.",
    items: &[
        ("A new look", "Light and dark themes, text size, and a refreshed folder tree, dialogs and selection."),
        (
            "All Inboxes",
            "With several accounts, one folder in Favorites gathers every account's Inbox; a reply goes from the account the message came to.",
        ),
        ("Conversations and categories", "Messages are grouped into conversations, and colour categories help you sort them."),
        ("HTML mail closer to the original", "Tables, colours and text sizes show as they were sent; pictures from the web load only when you ask."),
        (
            "Deceptive links",
            "When a link's text names one site but it goes to another, Tinta Correo shows the real address and asks before opening it.",
        ),
        (
            "Attachments",
            "Open, save or Save as… from the reading pane; attach, drop or paste files while writing; a forgotten attachment is caught before sending.",
        ),
        (
            "Calendar",
            "Drag appointments to another time or day, change one occurrence or the whole series, and choose your work hours, working days and first day of the week.",
        ),
        ("Birthdays", "Your contacts' birthdays show in the calendar and My Day; Send wishes starts a message to them."),
        ("Contacts while writing", "Add to contacts beside the sender, and address suggestions in To, Cc and Bcc."),
        (
            "Faster with large mailboxes",
            "Older mail comes header first, a new account's Inbox comes first, and the search index on disk uses about ten times less memory.",
        ),
        ("Export everything", "Mail, calendar and contacts to mbox, iCalendar and vCard with one button."),
    ],
};

/// Spanish (also read in Catalan, Galician and Basque).
const ES: List = List {
    intro: "Versión de prueba: conserve también su programa de correo actual.",
    items: &[
        ("Nuevo aspecto", "Temas claro y oscuro, tamaño del texto, árbol de carpetas, diálogos y selección renovados."),
        (
            "Todas las bandejas de entrada",
            "Con varias cuentas, una carpeta en Favoritos reúne la Bandeja de entrada de todas; al responder se usa la cuenta que recibió el mensaje.",
        ),
        ("Conversaciones y categorías", "Los mensajes se agrupan en conversaciones, y las categorías de colores ayudan a ordenarlos."),
        (
            "Correo HTML más fiel",
            "Tablas, colores y tamaños de letra se ven como se enviaron; las imágenes de internet solo se cargan cuando usted lo pide.",
        ),
        (
            "Enlaces engañosos",
            "Si el texto de un enlace nombra un sitio pero lleva a otro, Tinta Correo muestra la dirección real y pregunta antes de abrirlo.",
        ),
        (
            "Adjuntos",
            "Abrir, guardar o Guardar como… desde el panel de lectura; adjunte, arrastre o pegue archivos al escribir; un adjunto olvidado se detecta antes de enviar.",
        ),
        (
            "Calendario",
            "Arrastre citas a otra hora u otro día, cambie una sola vez o toda la serie, y elija su horario laboral, los días laborables y el primer día de la semana.",
        ),
        ("Cumpleaños", "Los cumpleaños de sus contactos aparecen en el calendario y en Mi día; «Felicitar» abre un mensaje para esa persona."),
        ("Contactos al escribir", "Añadir a contactos junto al remitente, y sugerencias de direcciones en Para, CC y CCO."),
        (
            "Más rápido con buzones grandes",
            "El correo antiguo llega primero con su encabezado, la Bandeja de entrada de una cuenta nueva llega antes que el resto, y el índice de búsqueda en disco usa unas 10 veces menos memoria.",
        ),
        ("Exportar todo", "Correo, calendario y contactos a mbox, iCalendar y vCard con un botón."),
    ],
};

/// Ukrainian.
const UK: List = List {
    intro: "Тестова версія: тримайте також свою поточну поштову програму.",
    items: &[
        ("Новий вигляд", "Світла й темна теми, розмір тексту, оновлені дерево папок, діалоги й виділення."),
        ("Усі вхідні", "Якщо акаунтів кілька, папка у «Вибраному» збирає «Вхідні» з усіх; відповідь іде з того акаунта, на який прийшов лист."),
        ("Розмови й категорії", "Листи групуються в розмови, а кольорові категорії допомагають їх упорядкувати."),
        (
            "HTML-листи ближче до оригіналу",
            "Таблиці, кольори й розміри шрифту виглядають так, як їх надіслали; зображення з інтернету завантажуються лише коли ви попросите.",
        ),
        (
            "Оманливі посилання",
            "Якщо текст посилання називає один сайт, а веде воно на інший, Tinta Correo показує справжню адресу й питає, перш ніж відкрити.",
        ),
        (
            "Вкладення",
            "Відкрити, зберегти чи «Зберегти як…» з панелі читання; вкладайте, перетягуйте чи вставляйте файли в лист; про забуте вкладення програма нагадає перед надсиланням.",
        ),
        (
            "Календар",
            "Перетягуйте зустрічі на інший час чи день, змінюйте один раз або всю серію, вибирайте робочі години, робочі дні й перший день тижня.",
        ),
        ("Дні народження", "Дні народження ваших контактів видно в календарі й у «Моєму дні»; «Привітати» відкриває лист цій людині."),
        ("Контакти під час написання", "«Додати до контактів» біля відправника й підказки адрес у полях «Кому», «Копія» і «Прихована»."),
        (
            "Швидше з великими скриньками",
            "Старі листи спершу приходять заголовками, «Вхідні» нового акаунта — першими, а пошуковий індекс на диску потребує приблизно вдесятеро менше пам'яті.",
        ),
        ("Експорт усього", "Пошта, календар і контакти в mbox, iCalendar і vCard однією кнопкою."),
    ],
};

pub const TITLE: &str = "What's new in Preview 4";
pub const CLOSE: &str = "Close";
/// The command's name in menus and on buttons.
pub const MENU: &str = "What's new";

/// The strings above, in the order of each row of [`TRANSLATIONS`].
const STRINGS: [&str; 3] = [TITLE, CLOSE, MENU];

/// `text` (one of the constants above) in the language `lang` (`es`, `ca-valencia`, `pt_BR`…),
/// English when that language has no translation.
pub fn tr(lang: &str, text: &'static str) -> &'static str {
    let lang = base(lang);
    let Some(i) = STRINGS.iter().position(|s| *s == text) else {
        return text;
    };
    TRANSLATIONS.iter().find(|(code, _)| *code == lang).and_then(|(_, t)| t.get(i).copied()).unwrap_or(text)
}

/// Each language's translations, in the order of [`STRINGS`].
const TRANSLATIONS: &[(&str, [&str; 3])] = &[
    ("es", ["Novedades de la vista previa 4", "Cerrar", "Novedades"]),
    ("ca", ["Novetats de la vista prèvia 4", "Tanca", "Novetats"]),
    ("gl", ["Novidades da vista previa 4", "Pechar", "Novidades"]),
    ("eu", ["4. aurrebistako berrikuntzak", "Itxi", "Berrikuntzak"]),
    ("uk", ["Що нового в попередній версії 4", "Закрити", "Що нового"]),
    ("it", ["Novità dell'anteprima 4", "Chiudi", "Novità"]),
    ("fr", ["Nouveautés de la préversion 4", "Fermer", "Nouveautés"]),
    ("de", ["Neu in Vorschau 4", "Schließen", "Neuigkeiten"]),
    ("pl", ["Co nowego w wersji zapoznawczej 4", "Zamknij", "Co nowego"]),
    ("ro", ["Noutăți în versiunea de previzualizare 4", "Închide", "Noutăți"]),
    ("cs", ["Novinky v náhledové verzi 4", "Zavřít", "Novinky"]),
    ("lt", ["Kas naujo 4-ojoje peržiūros versijoje", "Uždaryti", "Kas naujo"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, lang: &str) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| draw(ui.ctx(), lang, &Tokens::light()));
        out.textures_delta.clear();
    }

    fn escape() -> egui::Event {
        egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tinta-whats-new-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_list_opens_by_itself_once_per_release() {
        let dir = temp_dir("seen");
        assert!(!seen(&dir));
        assert!(remember(&dir));
        assert!(seen(&dir));
        // An older release's mark: the new list is due again.
        assert!(std::fs::write(dir.join(SEEN_FILE), "preview-3").is_ok());
        assert!(!seen(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn escape_closes_the_window() {
        let ctx = egui::Context::default();
        frame(&ctx, vec![], "en");
        assert!(!is_open(&ctx));
        open(&ctx);
        frame(&ctx, vec![], "en");
        frame(&ctx, vec![], "en");
        assert!(is_open(&ctx));
        frame(&ctx, vec![escape()], "en");
        assert!(!is_open(&ctx));
    }

    #[test]
    fn the_window_draws_in_every_language() {
        for lang in ["en", "es", "ca-valencia", "gl", "eu", "uk", "it", "fr", "de", "pl", "ro", "cs", "lt", "pt_BR"] {
            let ctx = egui::Context::default();
            open(&ctx);
            frame(&ctx, vec![], lang);
            frame(&ctx, vec![], lang);
            assert!(is_open(&ctx), "{lang}");
        }
    }

    #[test]
    fn every_list_has_every_item() {
        for list in [&ES, &UK] {
            assert_eq!(list.items.len(), EN.items.len());
            assert!(!list.intro.is_empty());
            assert!(list.items.iter().all(|(title, text)| !title.is_empty() && !text.is_empty()));
        }
        assert!(!std::ptr::eq(list("ca-valencia"), list("en")));
        assert!(std::ptr::eq(list("ca-valencia"), list("es")));
        assert!(std::ptr::eq(list("pt-BR"), list("en")));
    }

    #[test]
    fn every_string_is_translated() {
        for (lang, t) in TRANSLATIONS {
            for (en, tr) in STRINGS.iter().zip(t) {
                assert!(!tr.is_empty(), "{lang}: {en}");
            }
        }
        assert_ne!(tr("uk", CLOSE), CLOSE);
        assert_eq!(tr("pt-BR", CLOSE), CLOSE);
    }
}
