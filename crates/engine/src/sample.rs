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

fn addr(s: &str) -> Result<Address> {
    Address::parse(s)
}

/// The sample store, dated around `today`.
pub fn store(today: Date) -> Result<Store> {
    let mut s = Store::new();
    s.accounts.push(Account { name: "Work".into(), address: addr("Denys <denys@example.com>")?, kind: AccountKind::Local });
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
    };
    let at = |d: Date, h: u8, m: u8| DateTime::new(d, h, m, 0);
    s.add_event(event("Reunión de presupuesto", "Sala 1", When::timed(at(today, 10, 0)?, at(today, 11, 0)?, false)?))?;
    s.add_event(event("Almoço com Rui", "Café Azul", When::timed(at(today, 13, 0)?, at(today, 14, 0)?, false)?))?;
    let d2 = today.add_days(2)?;
    s.add_event(event("Formação Iber Texto", "Sala 2", When::timed(at(d2, 10, 0)?, at(d2, 13, 0)?, false)?))?;
    let d5 = today.add_days(5)?;
    s.add_event(event("Відпустка", "", When::all_day(d5, d5.add_days(3)?)?))?;

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
