//! Tinta Office: the feedback dialog (the smiling-face button and Help › Send feedback), like
//! Office's Feedback. The person picks what kind of feedback it is, writes it and, only if they
//! tick them, adds a reply e-mail, a screenshot of the window (taken when they pressed the button,
//! shown before sending) and technical details. Pressing Send leaves a [`Draft`] that the desktop
//! app takes ([`Feedback::take_draft`]), sends to report.tintaoffice.com/v1/feedback and answers
//! with [`Feedback::finished`]. Builds that cannot send never show the button or the menu entry:
//! the desktop app sets [`Feedback::available`] then.
//!
//! The same file lives in all five Tinta apps; its strings are translated here, not in the shared
//! catalogues, so syncing with upstream never conflicts.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Color32, ColorImage, RichText, TextureHandle, Ui};

/// The longest feedback text, in characters (the server keeps this much).
pub const MAX_TEXT: usize = 4000;
/// A screenshot is scaled down to fit this box, and to fit [`MAX_SHOT_BYTES`] as a PNG.
const MAX_SHOT_SIDE: usize = 1600;
pub const MAX_SHOT_BYTES: usize = 2 * 1024 * 1024;
/// A screenshot larger than this is not one (and is not scaled).
const MAX_SOURCE_SIDE: usize = 16_384;
/// How long to wait for the window's screenshot before opening the dialog without one.
const CAPTURE_WAIT_SECS: f64 = 1.0;
/// Tells this dialog's screenshot from any other.
const SHOT_MARKER: &str = "tinta.feedback";
const PREVIEW: egui::Vec2 = egui::vec2(360.0, 220.0);

/// What the feedback is about; the server's names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Like,
    Dislike,
    Idea,
}

impl Kind {
    pub fn code(self) -> &'static str {
        match self {
            Kind::Like => "like",
            Kind::Dislike => "dislike",
            Kind::Idea => "idea",
        }
    }
}

/// What Send leaves for the desktop app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub kind: Kind,
    pub text: String,
    pub email: Option<String>,
    /// The window as a PNG, when the person kept "Include a screenshot" ticked.
    pub screenshot_png: Option<Vec<u8>>,
    /// The person ticked "Include technical details": the app adds its system and log lines.
    pub diagnostics: bool,
}

/// How sending went, from the desktop app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The server has it; its reference, when it gave one.
    Sent(Option<String>),
    /// No connection: saved, and sent later by the app.
    Queued,
    /// Neither sent nor saved.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    Editing,
    Sending,
    Done(Outcome),
}

struct Shot {
    png: Vec<u8>,
    texture: TextureHandle,
}

/// The dialog's state; one per app window.
pub struct Feedback {
    /// Set by the desktop app when this build can send feedback.
    pub available: bool,
    open: bool,
    /// When the screenshot was asked for (`InputState::time`), while waiting for it.
    capturing: Option<f64>,
    /// Where the window's screenshot is kept once it arrived.
    captured: Arc<Mutex<Option<Arc<ColorImage>>>>,
    /// Asked for without a `Context` at hand (a command): started on the next frame.
    requested: bool,
    kind: Kind,
    text: String,
    email: String,
    with_shot: bool,
    with_diagnostics: bool,
    shot: Option<Shot>,
    status: Status,
    draft: Option<Draft>,
}

impl Default for Feedback {
    fn default() -> Self {
        Self {
            available: false,
            open: false,
            capturing: None,
            captured: Arc::default(),
            requested: false,
            kind: Kind::Dislike,
            text: String::new(),
            email: String::new(),
            with_shot: false,
            with_diagnostics: true,
            shot: None,
            status: Status::Editing,
            draft: None,
        }
    }
}

impl std::fmt::Debug for Feedback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feedback").field("available", &self.available).field("open", &self.open).field("status", &self.status).finish_non_exhaustive()
    }
}

impl Feedback {
    /// Whether the dialog (or the wait for its screenshot) is showing.
    pub fn is_open(&self) -> bool {
        self.open || self.capturing.is_some()
    }

    /// The button or the menu entry was pressed: take a screenshot of the window as it is now,
    /// then open the dialog. Does nothing in builds that cannot send.
    pub fn start(&mut self, ctx: &egui::Context) {
        if !self.available || self.is_open() {
            return;
        }
        let available = self.available;
        *self = Self { available, ..Self::default() };
        self.capturing = Some(ctx.input(|i| i.time));
        // egui 0.36 answers with an `Event::Screenshot` carrying this marker ([`Feedback::show`]).
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(SHOT_MARKER)));
        ctx.request_repaint();
    }

    /// Open it on the next frame (from a command, which has no `Context`).
    pub fn request(&mut self) {
        self.requested = self.available;
    }

    /// The feedback the person sent, once.
    pub fn take_draft(&mut self) -> Option<Draft> {
        self.draft.take()
    }

    /// How sending the last draft went.
    pub fn finished(&mut self, outcome: Outcome) {
        if self.status == Status::Sending {
            self.status = Status::Done(outcome);
        }
    }

    /// Each frame: collects the screenshot and shows the dialog while it is open.
    pub fn show(&mut self, ctx: &egui::Context, lang: &str) {
        if std::mem::take(&mut self.requested) {
            self.start(ctx);
        }
        if self.capturing.is_some() {
            let ours = ctx.input(|i| {
                i.raw.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. }
                        if user_data.data.as_ref().and_then(|d| d.downcast_ref::<&str>()) == Some(&SHOT_MARKER) =>
                    {
                        Some(Arc::clone(image))
                    }
                    _ => None,
                })
            });
            if let Some(image) = ours {
                *self.captured.lock().unwrap_or_else(PoisonError::into_inner) = Some(image);
            }
        }
        if let Some(since) = self.capturing {
            let image = self.captured.lock().unwrap_or_else(PoisonError::into_inner).take();
            let waited = ctx.input(|i| i.time) - since;
            if image.is_none() && waited < CAPTURE_WAIT_SECS {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            self.capturing = None;
            self.open = true;
            if let Some(png) = image.as_deref().and_then(screenshot_png)
                && let Some(preview) = image.as_deref().and_then(|i| downscale(i, 720))
            {
                self.shot = Some(Shot { png, texture: ctx.load_texture("tinta.feedback.shot", preview, egui::TextureOptions::LINEAR) });
                self.with_shot = true;
            }
        }
        if !self.open {
            return;
        }
        let modal = egui::Modal::new(egui::Id::new("tinta.feedback")).show(ctx, |ui| {
            ui.set_width(480.0);
            self.contents(ui, lang);
        });
        // Escape closes it; a click beside it doesn't, so typed text isn't lost by accident.
        if modal.should_close() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.status != Status::Sending {
            self.open = false;
        }
    }

    fn contents(&mut self, ui: &mut Ui, lang: &str) {
        ui.label(RichText::new(tr(lang, TITLE)).strong().size(17.0));
        ui.add_space(8.0);
        if let Status::Done(outcome) = &self.status {
            match outcome {
                Outcome::Sent(id) => {
                    ui.label(tr(lang, SENT));
                    if let Some(id) = id {
                        ui.label(RichText::new(tr(lang, REFERENCE).replace("{id}", id)).small().monospace());
                    }
                }
                Outcome::Queued => {
                    ui.label(tr(lang, QUEUED));
                }
                Outcome::Failed => {
                    ui.label(tr(lang, FAILED));
                }
            }
            ui.add_space(12.0);
            if ui.button(tr(lang, CLOSE)).clicked() {
                self.open = false;
            }
            return;
        }
        let editing = self.status == Status::Editing;
        ui.add_enabled_ui(editing, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (kind, label) in [(Kind::Like, LIKE), (Kind::Dislike, DISLIKE), (Kind::Idea, IDEA)] {
                    ui.selectable_value(&mut self.kind, kind, tr(lang, label));
                }
            });
            ui.add_space(6.0);
            let ask = match self.kind {
                Kind::Like => ASK_LIKE,
                Kind::Dislike => ASK_DISLIKE,
                Kind::Idea => ASK_IDEA,
            };
            ui.label(tr(lang, ask));
            ui.add(
                egui::TextEdit::multiline(&mut self.text)
                    .char_limit(MAX_TEXT)
                    .desired_rows(6)
                    .desired_width(f32::INFINITY)
                    .id(egui::Id::new("tinta.feedback.text")),
            );
            ui.label(RichText::new(format!("{} / {MAX_TEXT}", self.text.chars().count())).small().weak());
            ui.add_space(6.0);
            match &self.shot {
                Some(shot) => {
                    ui.checkbox(&mut self.with_shot, tr(lang, SCREENSHOT));
                    if self.with_shot {
                        let size = shot.texture.size_vec2();
                        let scale = (PREVIEW.x / size.x.max(1.0)).min(PREVIEW.y / size.y.max(1.0)).min(1.0);
                        ui.add(egui::Image::new((shot.texture.id(), size * scale)));
                    }
                }
                None => {
                    ui.add_enabled(false, egui::Checkbox::new(&mut false, tr(lang, NO_SCREENSHOT)));
                }
            }
            ui.checkbox(&mut self.with_diagnostics, tr(lang, DIAGNOSTICS));
            ui.add_space(6.0);
            ui.label(tr(lang, EMAIL));
            ui.add(
                egui::TextEdit::singleline(&mut self.email).char_limit(254).desired_width(f32::INFINITY).id(egui::Id::new("tinta.feedback.email")),
            );
        });
        let email_ok = self.email.trim().is_empty() || looks_like_email(self.email.trim());
        if !email_ok {
            ui.colored_label(ui.visuals().error_fg_color, tr(lang, EMAIL_BAD));
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tr(lang, PRIVACY)).small().weak());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if self.status == Status::Sending {
                ui.spinner();
                ui.label(tr(lang, SENDING));
                return;
            }
            let ready = !self.text.trim().is_empty() && email_ok;
            if ui.add_enabled(ready, egui::Button::new(tr(lang, SEND))).clicked() {
                self.send();
            }
            if ui.button(tr(lang, CANCEL)).clicked() {
                self.open = false;
            }
        });
    }

    fn send(&mut self) {
        let email = self.email.trim();
        self.draft = Some(Draft {
            kind: self.kind,
            text: self.text.trim().chars().take(MAX_TEXT).collect(),
            email: (!email.is_empty()).then(|| email.to_string()),
            screenshot_png: self.shot.as_ref().filter(|_| self.with_shot).map(|s| s.png.clone()),
            diagnostics: self.with_diagnostics,
        });
        self.status = Status::Sending;
    }
}

/// The smiling-face button for the top right of the window, drawn in code. `None` in builds that
/// cannot send.
pub fn button(ui: &mut Ui, feedback: &mut Feedback, lang: &str) -> Option<egui::Response> {
    if !feedback.available {
        return None;
    }
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 4.0, visuals.bg_fill);
    }
    let c = rect.center();
    let r = 7.5;
    let stroke = egui::Stroke::new(1.4, visuals.fg_stroke.color);
    painter.circle_stroke(c, r, stroke);
    painter.circle_filled(c + egui::vec2(-2.6, -2.0), 1.1, visuals.fg_stroke.color);
    painter.circle_filled(c + egui::vec2(2.6, -2.0), 1.1, visuals.fg_stroke.color);
    let smile: Vec<egui::Pos2> = (0..=8)
        .map(|i| {
            let a = std::f32::consts::PI * (0.2 + 0.6 * i as f32 / 8.0);
            c + egui::vec2(a.cos() * -3.8, a.sin() * 3.8 + 0.6)
        })
        .collect();
    painter.add(egui::Shape::line(smile, stroke));
    let response = response.on_hover_text(tr(lang, BUTTON));
    if response.clicked() {
        feedback.start(ui.ctx());
    }
    Some(response)
}

/// The Help menu entry; `None` in builds that cannot send.
pub fn menu_entry(ui: &mut Ui, feedback: &mut Feedback, lang: &str) -> Option<egui::Response> {
    if !feedback.available {
        return None;
    }
    let response = ui.button(tr(lang, MENU));
    if response.clicked() {
        feedback.start(ui.ctx());
        ui.close();
    }
    Some(response)
}

/// Enough of an address to answer: one `@`, something before it, a dot in what follows, no
/// spaces. The server checks again.
pub fn looks_like_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else { return false };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && s.len() <= 254
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The image scaled down (box filter, whole factors) so neither side passes `max_side`.
pub fn downscale(image: &ColorImage, max_side: usize) -> Option<ColorImage> {
    let [w, h] = image.size;
    if w == 0 || h == 0 || w > MAX_SOURCE_SIDE || h > MAX_SOURCE_SIDE || image.pixels.len() != w.checked_mul(h)? || max_side == 0 {
        return None;
    }
    let f = w.div_ceil(max_side).max(h.div_ceil(max_side)).max(1);
    let (ow, oh) = ((w / f).max(1), (h / f).max(1));
    let mut rgba = Vec::with_capacity(ow.checked_mul(oh)?.checked_mul(4)?);
    for y in 0..oh {
        for x in 0..ow {
            let mut sum = [0u32; 4];
            let mut n = 0u32;
            for dy in 0..f {
                for dx in 0..f {
                    let (sx, sy) = (x * f + dx, y * f + dy);
                    if sx < w
                        && let Some(p) = image.pixels.get(sy * w + sx)
                    {
                        for (s, v) in sum.iter_mut().zip(p.to_array()) {
                            *s += u32::from(v);
                        }
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            rgba.extend(sum.iter().map(|s| u8::try_from(s / n).unwrap_or(u8::MAX)));
        }
    }
    Some(ColorImage::from_rgba_premultiplied([ow, oh], &rgba))
}

/// An opaque RGB PNG of the image.
pub fn encode_png(image: &ColorImage) -> Option<Vec<u8>> {
    let [w, h] = image.size;
    let rgb: Vec<u8> = image.pixels.iter().flat_map(|p: &Color32| [p.r(), p.g(), p.b()]).collect();
    if rgb.len() != w.checked_mul(h)?.checked_mul(3)? {
        return None;
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(&rgb).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// The screenshot as sent: scaled to [`MAX_SHOT_SIDE`], then halved until it fits
/// [`MAX_SHOT_BYTES`].
pub fn screenshot_png(image: &ColorImage) -> Option<Vec<u8>> {
    let mut side = MAX_SHOT_SIDE;
    for _ in 0..4 {
        let png = encode_png(&downscale(image, side)?)?;
        if png.len() <= MAX_SHOT_BYTES {
            return Some(png);
        }
        side /= 2;
    }
    None
}

/// Standard base64 with padding, for the screenshot in the JSON body.
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ABC.get(((n >> shift) & 63) as usize).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub(crate) const BUTTON: &str = "Send feedback";
pub(crate) const MENU: &str = "Send feedback…";
pub(crate) const TITLE: &str = "Send feedback to Tinta Office";
pub(crate) const LIKE: &str = "I like something";
pub(crate) const DISLIKE: &str = "Something is wrong";
pub(crate) const IDEA: &str = "I have a suggestion";
pub(crate) const ASK_LIKE: &str = "What do you like? Tell us, so we keep it.";
pub(crate) const ASK_DISLIKE: &str = "What went wrong? Tell us what you did, what you expected and what happened.";
pub(crate) const ASK_IDEA: &str = "What would you like the app to do?";
pub(crate) const SCREENSHOT: &str = "Include a screenshot of the window";
pub(crate) const NO_SCREENSHOT: &str = "No screenshot available";
pub(crate) const DIAGNOSTICS: &str =
    "Include technical details (version, system and the last lines of the technical log, without file names or document text)";
pub(crate) const EMAIL: &str = "E-mail (optional, only so we can reply to you)";
pub(crate) const EMAIL_BAD: &str = "This doesn't look like an e-mail address.";
pub(crate) const PRIVACY: &str = "Your feedback goes to Tinta Office (report.tintaoffice.com, stored in the EU) and is deleted after 12 months. Please don't include passwords or other personal data. Details: tintaoffice.com/aviso-legal";
pub(crate) const SEND: &str = "Send";
pub(crate) const CANCEL: &str = "Cancel";
pub(crate) const CLOSE: &str = "Close";
pub(crate) const SENDING: &str = "Sending…";
pub(crate) const SENT: &str = "Thank you! We have received your feedback.";
pub(crate) const REFERENCE: &str = "Reference: {id}";
pub(crate) const QUEUED: &str = "There is no connection right now. Your feedback is saved and will be sent automatically when the app is online.";
pub(crate) const FAILED: &str = "Your feedback could not be sent. Please try again later or write to hola@tintaoffice.com.";

const STRINGS: [&str; 23] = [
    BUTTON,
    MENU,
    TITLE,
    LIKE,
    DISLIKE,
    IDEA,
    ASK_LIKE,
    ASK_DISLIKE,
    ASK_IDEA,
    SCREENSHOT,
    NO_SCREENSHOT,
    DIAGNOSTICS,
    EMAIL,
    EMAIL_BAD,
    PRIVACY,
    SEND,
    CANCEL,
    CLOSE,
    SENDING,
    SENT,
    REFERENCE,
    QUEUED,
    FAILED,
];

/// `text` (one of the constants above) in the language `lang` (`es`, `ca-valencia`, `pt_BR`…),
/// English when that language has no translation.
pub fn tr(lang: &str, text: &'static str) -> &'static str {
    let lang = lang.split(['-', '_']).next().unwrap_or(lang).to_ascii_lowercase();
    let Some(i) = STRINGS.iter().position(|s| std::ptr::eq(*s, text) || *s == text) else { return text };
    TRANSLATIONS.iter().find(|(code, _)| *code == lang).and_then(|(_, t)| t.get(i).copied()).unwrap_or(text)
}

/// Each language's translations, in the order of [`STRINGS`].
const TRANSLATIONS: &[(&str, [&str; 23])] = &[
    (
        "es",
        [
            "Enviar comentarios",
            "Enviar comentarios…",
            "Enviar comentarios a Tinta Office",
            "Me gusta algo",
            "Algo no funciona bien",
            "Tengo una sugerencia",
            "¿Qué te gusta? Cuéntanoslo para que lo mantengamos.",
            "¿Qué ha fallado? Cuéntanos qué hiciste, qué esperabas y qué pasó.",
            "¿Qué te gustaría que hiciera el programa?",
            "Incluir una captura de la ventana",
            "No hay captura disponible",
            "Incluir datos técnicos (versión, sistema y últimas líneas del registro técnico, sin nombres de archivos ni texto de documentos)",
            "Correo electrónico (opcional, solo para responderte)",
            "Esto no parece una dirección de correo.",
            "Tus comentarios van a Tinta Office (report.tintaoffice.com, almacenados en la UE) y se borran a los 12 meses. No incluyas contraseñas ni otros datos personales. Detalles: tintaoffice.com/aviso-legal",
            "Enviar",
            "Cancelar",
            "Cerrar",
            "Enviando…",
            "¡Gracias! Hemos recibido tus comentarios.",
            "Referencia: {id}",
            "Ahora no hay conexión. Tus comentarios se han guardado y se enviarán automáticamente cuando el programa tenga conexión.",
            "No se han podido enviar tus comentarios. Inténtalo más tarde o escribe a hola@tintaoffice.com.",
        ],
    ),
    (
        "ca",
        [
            "Envia comentaris",
            "Envia comentaris…",
            "Envia comentaris a Tinta Office",
            "M'agrada una cosa",
            "Alguna cosa no funciona bé",
            "Tinc un suggeriment",
            "Què us agrada? Expliqueu-nos-ho perquè ho mantinguem.",
            "Què ha fallat? Expliqueu-nos què heu fet, què esperàveu i què ha passat.",
            "Què voldríeu que fes el programa?",
            "Inclou una captura de la finestra",
            "No hi ha cap captura disponible",
            "Inclou dades tècniques (versió, sistema i últimes línies del registre tècnic, sense noms de fitxers ni text de documents)",
            "Adreça electrònica (opcional, només per respondre-us)",
            "Això no sembla una adreça electrònica.",
            "Els vostres comentaris van a Tinta Office (report.tintaoffice.com, emmagatzemats a la UE) i s'esborren als 12 mesos. No hi incloeu contrasenyes ni altres dades personals. Detalls: tintaoffice.com/aviso-legal",
            "Envia",
            "Cancel·la",
            "Tanca",
            "S'està enviant…",
            "Gràcies! Hem rebut els vostres comentaris.",
            "Referència: {id}",
            "Ara no hi ha connexió. Els vostres comentaris s'han desat i s'enviaran automàticament quan el programa tingui connexió.",
            "No s'han pogut enviar els vostres comentaris. Torneu-ho a provar més tard o escriviu a hola@tintaoffice.com.",
        ],
    ),
    (
        "gl",
        [
            "Enviar comentarios",
            "Enviar comentarios…",
            "Enviar comentarios a Tinta Office",
            "Gústame algo",
            "Algo non funciona ben",
            "Teño unha suxestión",
            "Que che gusta? Cóntanolo para que o manteñamos.",
            "Que fallou? Cóntanos que fixeches, que esperabas e que pasou.",
            "Que che gustaría que fixese o programa?",
            "Incluír unha captura da xanela",
            "Non hai ningunha captura dispoñible",
            "Incluír datos técnicos (versión, sistema e últimas liñas do rexistro técnico, sen nomes de ficheiros nin texto de documentos)",
            "Correo electrónico (opcional, só para responderche)",
            "Isto non parece un enderezo de correo.",
            "Os teus comentarios van a Tinta Office (report.tintaoffice.com, almacenados na UE) e bórranse aos 12 meses. Non inclúas contrasinais nin outros datos persoais. Detalles: tintaoffice.com/aviso-legal",
            "Enviar",
            "Cancelar",
            "Pechar",
            "Enviando…",
            "Grazas! Recibimos os teus comentarios.",
            "Referencia: {id}",
            "Agora non hai conexión. Os teus comentarios gardáronse e enviaranse automaticamente cando o programa teña conexión.",
            "Non se puideron enviar os teus comentarios. Téntao máis tarde ou escribe a hola@tintaoffice.com.",
        ],
    ),
    (
        "eu",
        [
            "Bidali iritzia",
            "Bidali iritzia…",
            "Bidali iritzia Tinta Office-ri",
            "Zerbait gustatzen zait",
            "Zerbait ez dabil ondo",
            "Iradokizun bat dut",
            "Zer gustatzen zaizu? Esaiguzu, mantendu dezagun.",
            "Zerk huts egin du? Esaiguzu zer egin duzun, zer espero zenuen eta zer gertatu den.",
            "Zer egitea nahiko zenuke programak?",
            "Sartu leihoaren pantaila-argazki bat",
            "Ez dago pantaila-argazkirik",
            "Sartu datu teknikoak (bertsioa, sistema eta erregistro teknikoaren azken lerroak, fitxategi-izenik eta dokumentuen testurik gabe)",
            "Helbide elektronikoa (aukerakoa, erantzuteko soilik)",
            "Honek ez du helbide elektroniko baten itxurarik.",
            "Zure iritzia Tinta Office-ra doa (report.tintaoffice.com, EBn gordeta) eta 12 hilabeteren buruan ezabatzen da. Ez sartu pasahitzik edo bestelako datu pertsonalik. Xehetasunak: tintaoffice.com/aviso-legal",
            "Bidali",
            "Utzi",
            "Itxi",
            "Bidaltzen…",
            "Eskerrik asko! Zure iritzia jaso dugu.",
            "Erreferentzia: {id}",
            "Orain ez dago konexiorik. Zure iritzia gorde da, eta automatikoki bidaliko da programak konexioa duenean.",
            "Ezin izan da zure iritzia bidali. Saiatu berriro geroago edo idatzi hola@tintaoffice.com helbidera.",
        ],
    ),
    (
        "uk",
        [
            "Надіслати відгук",
            "Надіслати відгук…",
            "Надіслати відгук у Tinta Office",
            "Мені щось подобається",
            "Щось працює не так",
            "У мене є пропозиція",
            "Що вам подобається? Розкажіть, щоб ми це зберегли.",
            "Що пішло не так? Розкажіть, що ви робили, чого очікували і що сталося.",
            "Що, на вашу думку, має вміти програма?",
            "Додати знімок вікна",
            "Знімок недоступний",
            "Додати технічні дані (версія, система й останні рядки технічного журналу, без назв файлів і тексту документів)",
            "Email (необов'язково, лише щоб ми могли відповісти)",
            "Це не схоже на адресу email.",
            "Ваш відгук надходить до Tinta Office (report.tintaoffice.com, зберігається в ЄС) і видаляється через 12 місяців. Не додавайте паролів чи інших особистих даних. Докладніше: tintaoffice.com/aviso-legal",
            "Надіслати",
            "Скасувати",
            "Закрити",
            "Надсилання…",
            "Дякуємо! Ми отримали ваш відгук.",
            "Номер відгуку: {id}",
            "Зараз немає з'єднання. Відгук збережено, і його буде надіслано автоматично, коли програма матиме з'єднання.",
            "Не вдалося надіслати відгук. Спробуйте пізніше або напишіть на hola@tintaoffice.com.",
        ],
    ),
    (
        "fr",
        [
            "Envoyer des commentaires",
            "Envoyer des commentaires…",
            "Envoyer des commentaires à Tinta Office",
            "J'aime quelque chose",
            "Quelque chose ne va pas",
            "J'ai une suggestion",
            "Qu'est-ce qui vous plaît ? Dites-le-nous pour que nous le gardions.",
            "Qu'est-ce qui n'a pas marché ? Dites-nous ce que vous avez fait, ce que vous attendiez et ce qui s'est passé.",
            "Que voudriez-vous que l'application fasse ?",
            "Joindre une capture de la fenêtre",
            "Aucune capture disponible",
            "Joindre des données techniques (version, système et dernières lignes du journal technique, sans noms de fichiers ni texte des documents)",
            "Adresse e-mail (facultative, uniquement pour vous répondre)",
            "Cela ne ressemble pas à une adresse e-mail.",
            "Vos commentaires sont envoyés à Tinta Office (report.tintaoffice.com, stockés dans l'UE) et supprimés au bout de 12 mois. N'y mettez pas de mots de passe ni d'autres données personnelles. Détails : tintaoffice.com/aviso-legal",
            "Envoyer",
            "Annuler",
            "Fermer",
            "Envoi…",
            "Merci ! Nous avons bien reçu vos commentaires.",
            "Référence : {id}",
            "Pas de connexion pour le moment. Vos commentaires sont enregistrés et seront envoyés automatiquement dès que l'application sera en ligne.",
            "Impossible d'envoyer vos commentaires. Réessayez plus tard ou écrivez à hola@tintaoffice.com.",
        ],
    ),
    (
        "it",
        [
            "Invia commenti",
            "Invia commenti…",
            "Invia commenti a Tinta Office",
            "Mi piace qualcosa",
            "Qualcosa non va",
            "Ho un suggerimento",
            "Che cosa ti piace? Raccontacelo, così lo manteniamo.",
            "Che cosa non ha funzionato? Raccontaci che cosa hai fatto, che cosa ti aspettavi e che cosa è successo.",
            "Che cosa vorresti che facesse il programma?",
            "Includi uno screenshot della finestra",
            "Nessuno screenshot disponibile",
            "Includi dati tecnici (versione, sistema e ultime righe del registro tecnico, senza nomi di file né testo dei documenti)",
            "E-mail (facoltativa, solo per risponderti)",
            "Questo non sembra un indirizzo e-mail.",
            "I tuoi commenti vanno a Tinta Office (report.tintaoffice.com, conservati nell'UE) e vengono cancellati dopo 12 mesi. Non includere password o altri dati personali. Dettagli: tintaoffice.com/aviso-legal",
            "Invia",
            "Annulla",
            "Chiudi",
            "Invio in corso…",
            "Grazie! Abbiamo ricevuto i tuoi commenti.",
            "Riferimento: {id}",
            "Al momento non c'è connessione. I tuoi commenti sono stati salvati e verranno inviati automaticamente quando il programma sarà online.",
            "Non è stato possibile inviare i tuoi commenti. Riprova più tardi o scrivi a hola@tintaoffice.com.",
        ],
    ),
    (
        "de",
        [
            "Feedback senden",
            "Feedback senden…",
            "Feedback an Tinta Office senden",
            "Mir gefällt etwas",
            "Etwas funktioniert nicht",
            "Ich habe einen Vorschlag",
            "Was gefällt Ihnen? Sagen Sie es uns, damit wir es beibehalten.",
            "Was ist schiefgelaufen? Beschreiben Sie, was Sie getan haben, was Sie erwartet haben und was passiert ist.",
            "Was sollte das Programm Ihrer Meinung nach können?",
            "Screenshot des Fensters anhängen",
            "Kein Screenshot verfügbar",
            "Technische Daten anhängen (Version, System und die letzten Zeilen des technischen Protokolls, ohne Dateinamen oder Dokumenttext)",
            "E-Mail (optional, nur für eine Antwort an Sie)",
            "Das sieht nicht wie eine E-Mail-Adresse aus.",
            "Ihr Feedback geht an Tinta Office (report.tintaoffice.com, Speicherung in der EU) und wird nach 12 Monaten gelöscht. Bitte fügen Sie keine Passwörter oder andere persönliche Daten ein. Details: tintaoffice.com/aviso-legal",
            "Senden",
            "Abbrechen",
            "Schließen",
            "Wird gesendet…",
            "Vielen Dank! Wir haben Ihr Feedback erhalten.",
            "Referenz: {id}",
            "Zurzeit besteht keine Verbindung. Ihr Feedback wurde gespeichert und wird automatisch gesendet, sobald das Programm online ist.",
            "Ihr Feedback konnte nicht gesendet werden. Versuchen Sie es später erneut oder schreiben Sie an hola@tintaoffice.com.",
        ],
    ),
    (
        "pl",
        [
            "Wyślij opinię",
            "Wyślij opinię…",
            "Wyślij opinię do Tinta Office",
            "Coś mi się podoba",
            "Coś nie działa",
            "Mam sugestię",
            "Co ci się podoba? Napisz nam, żebyśmy to zachowali.",
            "Co poszło nie tak? Opisz, jakie kroki wykonano, czego oczekiwano i co się stało.",
            "Czego brakuje ci w programie?",
            "Dołącz zrzut ekranu okna",
            "Brak dostępnego zrzutu ekranu",
            "Dołącz dane techniczne (wersja, system i ostatnie wiersze dziennika technicznego, bez nazw plików i tekstu dokumentów)",
            "E-mail (opcjonalnie, tylko po to, żebyśmy mogli odpowiedzieć)",
            "To nie wygląda na adres e-mail.",
            "Twoja opinia trafia do Tinta Office (report.tintaoffice.com, przechowywana w UE) i jest usuwana po 12 miesiącach. Nie podawaj haseł ani innych danych osobowych. Szczegóły: tintaoffice.com/aviso-legal",
            "Wyślij",
            "Anuluj",
            "Zamknij",
            "Wysyłanie…",
            "Dziękujemy! Otrzymaliśmy twoją opinię.",
            "Numer referencyjny: {id}",
            "Brak połączenia. Twoja opinia została zapisana i zostanie wysłana automatycznie, gdy program będzie online.",
            "Nie udało się wysłać opinii. Spróbuj później lub napisz na hola@tintaoffice.com.",
        ],
    ),
    (
        "ro",
        [
            "Trimite feedback",
            "Trimite feedback…",
            "Trimite feedback către Tinta Office",
            "Îmi place ceva",
            "Ceva nu funcționează bine",
            "Am o sugestie",
            "Ce vă place? Spuneți-ne, ca să păstrăm asta.",
            "Ce nu a mers? Spuneți-ne ce ați făcut, ce vă așteptați și ce s-a întâmplat.",
            "Ce ați vrea să facă programul?",
            "Include o captură a ferestrei",
            "Nicio captură disponibilă",
            "Include date tehnice (versiunea, sistemul și ultimele rânduri din jurnalul tehnic, fără nume de fișiere sau text din documente)",
            "E-mail (opțional, doar ca să vă putem răspunde)",
            "Aceasta nu pare o adresă de e-mail.",
            "Feedbackul dumneavoastră ajunge la Tinta Office (report.tintaoffice.com, stocat în UE) și este șters după 12 luni. Nu includeți parole sau alte date personale. Detalii: tintaoffice.com/aviso-legal",
            "Trimite",
            "Anulează",
            "Închide",
            "Se trimite…",
            "Vă mulțumim! Am primit feedbackul dumneavoastră.",
            "Referință: {id}",
            "Acum nu există conexiune. Feedbackul a fost salvat și va fi trimis automat când programul va fi online.",
            "Feedbackul nu a putut fi trimis. Încercați mai târziu sau scrieți la hola@tintaoffice.com.",
        ],
    ),
    (
        "cs",
        [
            "Odeslat zpětnou vazbu",
            "Odeslat zpětnou vazbu…",
            "Odeslat zpětnou vazbu do Tinta Office",
            "Něco se mi líbí",
            "Něco nefunguje",
            "Mám návrh",
            "Co se vám líbí? Napište nám to, ať to zachováme.",
            "Co se pokazilo? Napište nám, co jste dělali, co jste očekávali a co se stalo.",
            "Co byste chtěli, aby program uměl?",
            "Přiložit snímek okna",
            "Snímek není k dispozici",
            "Přiložit technické údaje (verze, systém a poslední řádky technického protokolu, bez názvů souborů a textu dokumentů)",
            "E-mail (nepovinný, jen abychom vám mohli odpovědět)",
            "Toto nevypadá jako e-mailová adresa.",
            "Vaše zpětná vazba jde do Tinta Office (report.tintaoffice.com, uloženo v EU) a po 12 měsících se smaže. Neuvádějte hesla ani jiné osobní údaje. Podrobnosti: tintaoffice.com/aviso-legal",
            "Odeslat",
            "Zrušit",
            "Zavřít",
            "Odesílání…",
            "Děkujeme! Vaši zpětnou vazbu jsme obdrželi.",
            "Číslo zpětné vazby: {id}",
            "Teď není k dispozici připojení. Zpětná vazba je uložená a odešle se automaticky, až bude program online.",
            "Zpětnou vazbu se nepodařilo odeslat. Zkuste to později nebo napište na hola@tintaoffice.com.",
        ],
    ),
    (
        "lt",
        [
            "Siųsti atsiliepimą",
            "Siųsti atsiliepimą…",
            "Siųsti atsiliepimą Tinta Office",
            "Man kažkas patinka",
            "Kažkas neveikia",
            "Turiu pasiūlymą",
            "Kas jums patinka? Papasakokite, kad tai išsaugotume.",
            "Kas nepavyko? Papasakokite, ką darėte, ko tikėjotės ir kas nutiko.",
            "Ką norėtumėte, kad programa darytų?",
            "Pridėti lango ekrano kopiją",
            "Ekrano kopijos nėra",
            "Pridėti techninius duomenis (versija, sistema ir paskutinės techninio žurnalo eilutės, be failų pavadinimų ir dokumentų teksto)",
            "El. paštas (neprivaloma, tik kad galėtume atsakyti)",
            "Tai nepanašu į el. pašto adresą.",
            "Jūsų atsiliepimas siunčiamas Tinta Office (report.tintaoffice.com, saugoma ES) ir ištrinamas po 12 mėnesių. Nenurodykite slaptažodžių ar kitų asmens duomenų. Išsamiau: tintaoffice.com/aviso-legal",
            "Siųsti",
            "Atšaukti",
            "Uždaryti",
            "Siunčiama…",
            "Ačiū! Gavome jūsų atsiliepimą.",
            "Atsiliepimo numeris: {id}",
            "Šiuo metu nėra ryšio. Atsiliepimas išsaugotas ir bus išsiųstas automatiškai, kai programa prisijungs prie interneto.",
            "Nepavyko išsiųsti atsiliepimo. Bandykite vėliau arba rašykite hola@tintaoffice.com.",
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, time: f64, fb: &mut Feedback) {
        let input = egui::RawInput { time: Some(time), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| fb.show(&ui.ctx().clone(), "es"));
        // A test has no renderer to hand the font and screenshot textures to.
        out.textures_delta.clear();
    }

    fn shot(w: usize, h: usize) -> Arc<ColorImage> {
        let pixels = (0..w * h).map(|i| Color32::from_rgb((i % 251) as u8, (i % 13) as u8 * 19, 200)).collect();
        Arc::new(ColorImage { size: [w, h], pixels, ..ColorImage::example() })
    }

    /// What the renderer does with the screenshot command.
    fn deliver(fb: &Feedback, image: Arc<ColorImage>) {
        *fb.captured.lock().unwrap() = Some(image);
    }

    #[test]
    fn builds_that_cannot_send_never_open() {
        let ctx = egui::Context::default();
        let mut fb = Feedback::default();
        fb.start(&ctx);
        fb.request();
        frame(&ctx, 0.0, &mut fb);
        assert!(!fb.is_open());
    }

    #[test]
    fn a_command_opens_it_on_the_next_frame() {
        let ctx = egui::Context::default();
        let mut fb = Feedback { available: true, ..Feedback::default() };
        fb.request();
        frame(&ctx, 0.0, &mut fb);
        assert!(fb.is_open());
    }

    #[test]
    fn the_dialog_opens_with_the_window_screenshot() {
        let ctx = egui::Context::default();
        let mut fb = Feedback { available: true, ..Feedback::default() };
        frame(&ctx, 0.0, &mut fb);
        fb.start(&ctx);
        assert!(fb.is_open());
        deliver(&fb, shot(40, 30));
        frame(&ctx, 0.1, &mut fb);
        assert!(fb.open);
        assert!(fb.with_shot);
        assert!(fb.shot.as_ref().is_some_and(|s| s.png.starts_with(&[0x89, b'P', b'N', b'G'])));
    }

    #[test]
    fn without_a_screenshot_it_opens_after_a_moment() {
        let ctx = egui::Context::default();
        let mut fb = Feedback { available: true, ..Feedback::default() };
        frame(&ctx, 0.0, &mut fb);
        fb.start(&ctx);
        frame(&ctx, 0.5, &mut fb);
        assert!(!fb.open);
        frame(&ctx, 1.5, &mut fb);
        assert!(fb.open);
        assert!(fb.shot.is_none());
    }

    #[test]
    fn send_leaves_one_draft_and_waits_for_the_outcome() {
        let mut fb = Feedback { available: true, open: true, ..Feedback::default() };
        fb.kind = Kind::Idea;
        fb.text = "  Más plantillas, por favor  ".into();
        fb.email = " ana@example.es ".into();
        fb.send();
        let d = fb.take_draft().unwrap();
        assert_eq!(
            d,
            Draft {
                kind: Kind::Idea,
                text: "Más plantillas, por favor".into(),
                email: Some("ana@example.es".into()),
                screenshot_png: None,
                diagnostics: true
            }
        );
        assert!(fb.take_draft().is_none());
        assert_eq!(fb.status, Status::Sending);
        fb.finished(Outcome::Queued);
        assert_eq!(fb.status, Status::Done(Outcome::Queued));
        // A late answer for an old draft changes nothing.
        fb.finished(Outcome::Failed);
        assert_eq!(fb.status, Status::Done(Outcome::Queued));
    }

    #[test]
    fn an_unticked_screenshot_is_not_sent() {
        let ctx = egui::Context::default();
        let mut fb = Feedback { available: true, ..Feedback::default() };
        frame(&ctx, 0.0, &mut fb);
        fb.start(&ctx);
        deliver(&fb, shot(8, 8));
        frame(&ctx, 0.1, &mut fb);
        fb.text = "x".into();
        fb.with_shot = false;
        fb.send();
        assert!(fb.take_draft().unwrap().screenshot_png.is_none());
    }

    #[test]
    fn the_dialog_draws_in_every_language_and_state() {
        let ctx = egui::Context::default();
        for lang in ["en", "es", "ca-valencia", "uk", "lt", "pt_BR"] {
            for status in [
                Status::Editing,
                Status::Sending,
                Status::Done(Outcome::Sent(Some("abc".into()))),
                Status::Done(Outcome::Queued),
                Status::Done(Outcome::Failed),
            ] {
                let mut fb = Feedback { available: true, open: true, status, email: "bad address".into(), ..Feedback::default() };
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    fb.show(&ui.ctx().clone(), lang);
                    let _ = button(ui, &mut fb, lang);
                });
                out.textures_delta.clear();
            }
        }
    }

    #[test]
    fn email_check() {
        for ok in ["a@b.es", "ana.garcia+tinta@correo.example.com"] {
            assert!(looks_like_email(ok), "{ok}");
        }
        for bad in ["", "ana", "@b.es", "a@b", "a@.es", "a@b.", "a b@c.es", "a@b@c.es"] {
            assert!(!looks_like_email(bad), "{bad}");
        }
    }

    #[test]
    fn screenshots_are_scaled_and_bounded() {
        let big = shot(3300, 1700);
        let small = downscale(&big, MAX_SHOT_SIDE).unwrap();
        assert_eq!(small.size, [1100, 566]);
        assert!(screenshot_png(&big).unwrap().len() <= MAX_SHOT_BYTES);
        assert!(downscale(&ColorImage { size: [0, 5], pixels: vec![], ..ColorImage::example() }, 10).is_none());
        assert!(downscale(&ColorImage { size: [5, 5], pixels: vec![Color32::RED; 3], ..ColorImage::example() }, 10).is_none());
        assert!(downscale(&ColorImage { size: [20_000, 1], pixels: vec![Color32::RED; 20_000], ..ColorImage::example() }, 10).is_none());
    }

    #[test]
    fn base64_matches_the_standard() {
        for (input, out) in
            [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
        {
            assert_eq!(base64(input.as_bytes()), out);
        }
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn every_string_is_translated() {
        for (lang, t) in TRANSLATIONS {
            for (en, tr) in STRINGS.iter().zip(t) {
                assert!(!tr.is_empty(), "{lang}: {en}");
                assert_eq!(en.contains("{id}"), tr.contains("{id}"), "{lang}: {en}");
            }
        }
        assert_eq!(tr("ca-valencia", SEND), "Envia");
        assert_eq!(tr("pt-BR", SEND), "Send");
        assert_eq!(tr("UK", TITLE), "Надіслати відгук у Tinta Office");
    }
}
