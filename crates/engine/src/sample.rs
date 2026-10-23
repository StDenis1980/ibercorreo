//! A sample mailbox and calendar for screenshots, demos and tests. All names, addresses
//! (`example.*`, RFC 2606) and text are invented for Iber Correo.

use ibercorreo_core::calendar::When;
use ibercorreo_core::mail::{Account, AccountKind, Attachment};
use ibercorreo_core::{Address, Contact, Date, DateTime, Event, EventId, FolderRole, Message, MessageId, Result};
use ibercorreo_store::Store;

struct Mail {
    from: &'static str,
    subject: &'static str,
    days_ago: i64,
    hour: u8,
    body: &'static str,
    seen: bool,
    flagged: bool,
    attachment: Option<&'static str>,
}

const INBOX: &[Mail] = &[
    Mail {
        from: "Marta Iglesias <marta@example.es>",
        subject: "Presupuesto del segundo trimestre",
        days_ago: 0,
        hour: 9,
        body: "Hola Denys,\n\nTe envío el presupuesto revisado. Las cifras de marketing bajaron un 4 %.\nLo vemos el lunes en la reunión.\n\nUn saludo,\nMarta",
        seen: false,
        flagged: true,
        attachment: Some("presupuesto-t2.xlsx"),
    },
    Mail {
        from: "Rui Carvalho <rui@example.pt>",
        subject: "Formação Iber Texto: datas",
        days_ago: 0,
        hour: 8,
        body: "Bom dia!\n\nConfirmo a formação para quinta e sexta, das 10h às 13h.\nPrecisamos de projetor na sala 2.\n\nAbraço,\nRui",
        seen: false,
        flagged: false,
        attachment: None,
    },
    Mail {
        from: "Олена Коваль <olena@example.com.ua>",
        subject: "Звіт за жовтень",
        days_ago: 1,
        hour: 17,
        body: "Добрий день!\n\nНадсилаю звіт за жовтень у PDF. Відкриється в Iber PDF.\n\nЗ повагою,\nОлена",
        seen: true,
        flagged: false,
        attachment: Some("zvit-zhovten.pdf"),
    },
    Mail {
        from: "Support desk <help@example.org>",
        subject: "Ticket #2041 resolved",
        days_ago: 3,
        hour: 11,
        body: "Your ticket about printer setup has been resolved.\nReply to this message to reopen it.",
        seen: true,
        flagged: false,
        attachment: None,
    },
];

/// An HTML newsletter, to show formatted mail with its remote images blocked.
const NEWSLETTER: &str = "<html><head><style>h1{color:#1F7A8C}</style></head><body>\
<div style=\"display:none\">Esta semana: novo menu de outono&zwnj;&nbsp;&zwnj;&nbsp;</div>\
<p><img src=\"https://cdn.example.net/cafe-azul-logo.png\" alt=\"Café Azul\" width=\"120\"></p>\
<h1>Novidades de outubro</h1>\
<p>Olá Denys,</p><p>O nosso <b>menu de outono</b> já chegou. Esta semana temos:</p>\
<ul><li>Sopa de abóbora com gengibre</li><li>Bolo de castanha, <i>feito na casa</i></li><li>Chá de maçã e canela</li></ul>\
<p>Reserve a sua mesa em <a href=\"https://cafe-azul.example.net/reservas\">cafe-azul.example.net</a>.</p>\
<blockquote>&ldquo;O melhor café do bairro.&rdquo; &mdash; um cliente</blockquote>\
<hr><p style=\"font-size:12px\">Café Azul &middot; Rua das Flores 12 &middot; <a href=\"https://cafe-azul.example.net/sair\">Cancelar subscrição</a></p>\
<img src=\"https://track.example.net/open.gif?u=denys\" width=\"1\" height=\"1\"></body></html>";

/// A phishing-shaped message: its link shows one site and goes to another.
const PHISHING: &str = "<p>Dear customer,</p><p>Your account will be <b>suspended</b> today. Sign in to keep it:</p>\
<p><a href=\"https://login.example.info/verify\">https://www.bank.example/secure</a></p><p>Bank Support</p>";

fn addr(s: &str) -> Result<Address> {
    Address::parse(s)
}

/// The sample store, dated around `today`.
pub fn store(today: Date) -> Result<Store> {
    let mut s = Store::new();
    s.accounts.push(Account { name: "Work".into(), address: addr("Denys <denys@example.com>")?, kind: AccountKind::Local, caldav: String::new() });
    let inbox = s.folder_by_role(FolderRole::Inbox)?;
    let sent = s.folder_by_role(FolderRole::Sent)?;
    for (i, mail) in INBOX.iter().enumerate() {
        let mut m = Message::new(MessageId(0), inbox);
        m.from = Some(addr(mail.from)?);
        m.to = vec![addr("Denys <denys@example.com>")?];
        m.set_subject(mail.subject);
        m.date = Some(DateTime::new(today.add_days(-mail.days_ago)?, mail.hour, (i as u8).saturating_mul(7) % 60, 0)?);
        m.body = mail.body.into();
        m.seen = mail.seen;
        m.flagged = mail.flagged;
        if let Some(name) = mail.attachment {
            let mime_type =
                if name.ends_with(".pdf") { "application/pdf" } else { "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" };
            m.attachments.push(Attachment { name: name.into(), mime_type: mime_type.into(), size: 48_213 });
        }
        s.add_message(m)?;
    }
    for (role, from, subject, days_ago, html) in [
        (FolderRole::Inbox, "Café Azul <novidades@example.net>", "Novidades de outubro", 2, NEWSLETTER),
        (FolderRole::Junk, "Bank Support <support@example.info>", "Confirm your account", 1, PHISHING),
    ] {
        let mut m = Message::new(MessageId(0), s.folder_by_role(role)?);
        m.from = Some(addr(from)?);
        m.to = vec![addr("Denys <denys@example.com>")?];
        m.set_subject(subject);
        m.date = Some(DateTime::new(today.add_days(-days_ago)?, 7, 15, 0)?);
        let rich = ibercorreo_mime::html::to_rich(html);
        m.body = rich.to_text();
        m.rich = Some(rich);
        m.seen = role != FolderRole::Inbox;
        s.add_message(m)?;
    }
    let mut m = Message::new(MessageId(0), sent);
    m.from = Some(addr("Denys <denys@example.com>")?);
    m.to = vec![addr("Marta Iglesias <marta@example.es>")?];
    m.set_subject("Re: Agenda de la reunión");
    m.date = Some(DateTime::new(today.add_days(-2)?, 15, 40, 0)?);
    m.body = "Perfecto, nos vemos el lunes a las 10.\n\nDenys".into();
    m.seen = true;
    s.add_message(m)?;

    let event = |summary: &str, location: &str, when: When| Event {
        id: EventId(0),
        calendar: 0,
        uid: format!("sample-{summary}@ibercorreo.local"),
        summary: summary.into(),
        location: location.into(),
        description: String::new(),
        when,
        tzid: None,
        rrule: None,
        exdates: Vec::new(),
        recurrence_id: None,
        reminders: vec![15],
        remote: None,
    };
    let at = |d: Date, h: u8, m: u8| DateTime::new(d, h, m, 0);
    s.add_event(event("Reunión de presupuesto", "Sala 1", When::timed(at(today, 10, 0)?, at(today, 11, 0)?, false)?))?;
    // Overlaps the budget meeting, so the day and week views show two side by side.
    s.add_event(event("Chamada com a Lusodata", "Teams", When::timed(at(today, 10, 30)?, at(today, 11, 30)?, false)?))?;
    s.add_event(event("Almoço com Rui", "Café Azul", When::timed(at(today, 13, 0)?, at(today, 14, 0)?, false)?))?;
    let d2 = today.add_days(2)?;
    s.add_event(event("Formação Iber Texto", "Sala 2", When::timed(at(d2, 10, 0)?, at(d2, 13, 0)?, false)?))?;
    let d5 = today.add_days(5)?;
    let mut holiday = event("Відпустка", "", When::all_day(d5, d5.add_days(3)?)?);
    holiday.reminders.clear();
    s.add_event(holiday)?;
    // A repeating event: every weekday morning, starting a week ago.
    let first = today.add_days(-7)?;
    let mut standup = event("Stand-up", "Online", When::timed(at(first, 9, 15)?, at(first, 9, 30)?, false)?);
    standup.rrule = Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into());
    standup.reminders.clear();
    s.add_event(standup)?;

    for (name, email, org) in [
        ("Marta Iglesias", "marta@example.es", "Iglesias Consultores"),
        ("Rui Carvalho", "rui@example.pt", "Formação RC"),
        ("Олена Коваль", "olena@example.com.ua", "Коваль і партнери"),
    ] {
        s.add_contact(Contact {
            id: 0,
            name: name.into(),
            emails: vec![Address::new(Some(name), email)?],
            phone: None,
            organization: Some(org.into()),
        });
    }
    Ok(s)
}
