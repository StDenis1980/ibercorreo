//! The command registry. Each entry is everything a front end needs to show and run one action.

use ibercorreo_core::calendar::When;
use ibercorreo_core::mail::{Account, AccountKind};
use ibercorreo_core::{Address, Contact, Date, DateTime, Error, Event, EventId, FolderId, FolderRole, Message, MessageId, Result, Security};
use serde_json::{Value, json};

use crate::{Session, View};

pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Ribbon tab and group, `Home/Delete`.
    pub group: &'static str,
    pub shortcut: Option<&'static str>,
    /// One line per parameter: `name: type — meaning`.
    pub params: &'static str,
    /// Whether the command can change the store (and so takes an undo step).
    pub edits: bool,
    pub run: fn(&mut Session, &Value) -> Result<Value>,
}

pub fn find(id: &str) -> Option<&'static CommandSpec> {
    REGISTRY.iter().find(|c| c.id == id)
}

pub static REGISTRY: &[CommandSpec] = &[
    CommandSpec {
        id: "view.mail",
        label: "Mail",
        group: "View/Navigate",
        shortcut: Some("Ctrl+1"),
        params: "",
        edits: false,
        run: |s, _| set_view(s, View::Mail),
    },
    CommandSpec {
        id: "view.calendar",
        label: "Calendar",
        group: "View/Navigate",
        shortcut: Some("Ctrl+2"),
        params: "",
        edits: false,
        run: |s, _| set_view(s, View::Calendar),
    },
    CommandSpec {
        id: "view.contacts",
        label: "People",
        group: "View/Navigate",
        shortcut: Some("Ctrl+3"),
        params: "",
        edits: false,
        run: |s, _| set_view(s, View::Contacts),
    },
    CommandSpec {
        id: "view.plainText",
        label: "Plain text",
        group: "View/Message",
        shortcut: None,
        params: "on?: bool — show HTML mail as plain text (default: toggle)",
        edits: false,
        run: view_plain_text,
    },
    CommandSpec { id: "folder.list", label: "List folders", group: "Folder/Folders", shortcut: None, params: "", edits: false, run: folder_list },
    CommandSpec {
        id: "folder.select",
        label: "Open folder",
        group: "Folder/Folders",
        shortcut: None,
        params: "folder: number | role (inbox, drafts, sent, outbox, archive, junk, trash)",
        edits: false,
        run: folder_select,
    },
    CommandSpec {
        id: "message.list",
        label: "List messages",
        group: "Home/Find",
        shortcut: None,
        params: "folder?: number | role — default: the open folder",
        edits: false,
        run: message_list,
    },
    CommandSpec {
        id: "message.select",
        label: "Open message",
        group: "Home/Find",
        shortcut: None,
        params: "id: number",
        edits: true,
        run: message_select,
    },
    CommandSpec {
        id: "message.markRead",
        label: "Mark as read",
        group: "Home/Tags",
        shortcut: Some("Ctrl+Q"),
        params: "id?: number — default: the open message\nread?: bool — default true",
        edits: true,
        run: message_mark_read,
    },
    CommandSpec {
        id: "message.flag",
        label: "Flag",
        group: "Home/Tags",
        shortcut: Some("Insert"),
        params: "id?: number\nflagged?: bool — default: toggle",
        edits: true,
        run: message_flag,
    },
    CommandSpec {
        id: "message.move",
        label: "Move",
        group: "Home/Move",
        shortcut: Some("Ctrl+Shift+V"),
        params: "id?: number\nfolder: number | role",
        edits: true,
        run: message_move,
    },
    CommandSpec {
        id: "message.archive",
        label: "Archive",
        group: "Home/Delete",
        shortcut: Some("Backspace"),
        params: "id?: number",
        edits: true,
        run: |s, p| move_to_role(s, p, FolderRole::Archive),
    },
    CommandSpec {
        id: "message.delete",
        label: "Delete",
        group: "Home/Delete",
        shortcut: Some("Delete"),
        params: "id?: number — moves to Trash; in Trash, deletes for good",
        edits: true,
        run: message_delete,
    },
    CommandSpec {
        id: "message.compose",
        label: "New Email",
        group: "Home/New",
        shortcut: Some("Ctrl+N"),
        params: "to?: string — comma-separated addresses\ncc?: string\nsubject?: string\nbody?: string",
        edits: true,
        run: message_compose,
    },
    CommandSpec {
        id: "message.update",
        label: "Edit draft",
        group: "Message/Basic Text",
        shortcut: None,
        params: "id?: number — a draft\nto?: string\ncc?: string\nsubject?: string\nbody?: string",
        edits: true,
        run: message_update,
    },
    CommandSpec {
        id: "message.reply",
        label: "Reply",
        group: "Home/Respond",
        shortcut: Some("Ctrl+R"),
        params: "id?: number",
        edits: true,
        run: |s, p| reply(s, p, false),
    },
    CommandSpec {
        id: "message.replyAll",
        label: "Reply All",
        group: "Home/Respond",
        shortcut: Some("Ctrl+Shift+R"),
        params: "id?: number",
        edits: true,
        run: |s, p| reply(s, p, true),
    },
    CommandSpec {
        id: "message.forward",
        label: "Forward",
        group: "Home/Respond",
        shortcut: Some("Ctrl+F"),
        params: "id?: number\nto?: string",
        edits: true,
        run: forward,
    },
    CommandSpec {
        id: "message.send",
        label: "Send",
        group: "Message/Send",
        shortcut: Some("Ctrl+Enter"),
        params: "id?: number — a draft; queued in Outbox until an account can send (M1)",
        edits: true,
        run: message_send,
    },
    CommandSpec {
        id: "message.read",
        label: "Read message",
        group: "Message/Actions",
        shortcut: None,
        params: "id?: number — the message with its text, and its formatted body made safe (`rich`) when it has HTML",
        edits: false,
        run: message_read,
    },
    CommandSpec {
        id: "message.source",
        label: "View source",
        group: "Message/Actions",
        shortcut: None,
        params: "id?: number — the message as RFC 5322 text",
        edits: false,
        run: message_source,
    },
    CommandSpec {
        id: "message.importEml",
        label: "Import .eml",
        group: "File/Open",
        shortcut: None,
        params: "raw: string — the message text\nfolder?: number | role — default inbox",
        edits: true,
        run: import_eml,
    },
    CommandSpec {
        id: "search.messages",
        label: "Search",
        group: "Home/Find",
        shortcut: Some("Ctrl+E"),
        params: "query: string",
        edits: false,
        run: search,
    },
    CommandSpec {
        id: "calendar.goto",
        label: "Go to date",
        group: "Calendar/Go To",
        shortcut: Some("Ctrl+G"),
        params: "date: YYYY-MM-DD | \"today\"",
        edits: false,
        run: calendar_goto,
    },
    CommandSpec {
        id: "calendar.nextMonth",
        label: "Next month",
        group: "Calendar/Go To",
        shortcut: Some("Alt+PageDown"),
        params: "",
        edits: false,
        run: |s, _| shift_month(s, 1),
    },
    CommandSpec {
        id: "calendar.prevMonth",
        label: "Previous month",
        group: "Calendar/Go To",
        shortcut: Some("Alt+PageUp"),
        params: "",
        edits: false,
        run: |s, _| shift_month(s, -1),
    },
    CommandSpec {
        id: "event.list",
        label: "List events",
        group: "Calendar/Arrange",
        shortcut: None,
        params: "date?: YYYY-MM-DD | \"today\" — default: the calendar's day",
        edits: false,
        run: event_list,
    },
    CommandSpec {
        id: "event.add",
        label: "New Appointment",
        group: "Calendar/New",
        shortcut: Some("Ctrl+Shift+A"),
        params: "summary: string\nstart: YYYY-MM-DD (all day) | YYYY-MM-DDTHH:MM\nend?: same form — default: one hour, or one day\nlocation?: string\ndescription?: string\nrepeat?: daily | weekdays | weekly | monthly | yearly\nrrule?: string — an iCalendar RRULE, instead of repeat\nuntil?: YYYY-MM-DD — the last day it repeats",
        edits: true,
        run: event_add,
    },
    CommandSpec {
        id: "event.delete",
        label: "Delete appointment",
        group: "Calendar/Actions",
        shortcut: None,
        params: "id?: number — default: the selected event\noccurrence?: YYYY-MM-DD — for a repeating event, delete only that day's occurrence",
        edits: true,
        run: event_delete,
    },
    CommandSpec {
        id: "calendar.importIcs",
        label: "Import calendar (.ics)",
        group: "File/Open",
        shortcut: None,
        params: "text: string — iCalendar\ncalendar?: number — default 0",
        edits: true,
        run: import_ics,
    },
    CommandSpec {
        id: "calendar.exportIcs",
        label: "Export calendar (.ics)",
        group: "File/Save",
        shortcut: None,
        params: "calendar?: number — default: all",
        edits: false,
        run: export_ics,
    },
    CommandSpec {
        id: "contact.add",
        label: "New Contact",
        group: "People/New",
        shortcut: None,
        params: "name: string\nemail?: string\nphone?: string\norganization?: string",
        edits: true,
        run: contact_add,
    },
    CommandSpec {
        id: "contact.list",
        label: "List contacts",
        group: "People/Find",
        shortcut: None,
        params: "query?: string",
        edits: false,
        run: contact_list,
    },
    CommandSpec {
        id: "edit.undo",
        label: "Undo",
        group: "Quick Access",
        shortcut: Some("Ctrl+Z"),
        params: "",
        edits: false,
        run: |s, _| s.undo().map(|()| json!({})),
    },
    CommandSpec {
        id: "edit.redo",
        label: "Redo",
        group: "Quick Access",
        shortcut: Some("Ctrl+Y"),
        params: "",
        edits: false,
        run: |s, _| s.redo().map(|()| json!({})),
    },
    CommandSpec {
        id: "account.set",
        label: "Account settings",
        group: "File/Account",
        shortcut: None,
        params: "email: string — the address mail is sent from\nname?: string — shown to recipients\nimapHost: string\nimapPort?: number — default 993 (TLS) or 143 (STARTTLS)\nimapSecurity?: \"tls\" | \"starttls\" — default tls\nsmtpHost: string\nsmtpPort?: number — default 465 (TLS) or 587 (STARTTLS)\nsmtpSecurity?: \"tls\" | \"starttls\" — default tls\nuser?: string — sign-in name, default the email address\n(The password is never a parameter: the app keeps it in the system's credential store.)",
        edits: true,
        run: account_set,
    },
    CommandSpec { id: "account.get", label: "Show account", group: "File/Account", shortcut: None, params: "", edits: false, run: account_get },
    CommandSpec {
        id: "account.sync",
        label: "Send/Receive",
        group: "Send-Receive/Send & Receive",
        shortcut: Some("F9"),
        params: "",
        edits: false,
        run: account_sync,
    },
    CommandSpec {
        id: "file.save",
        label: "Save",
        group: "File/Save",
        shortcut: Some("Ctrl+S"),
        params: "path?: string — default: where it was opened from",
        edits: false,
        run: file_save,
    },
];

// ---- parameters ----

fn str_param<'a>(p: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(Error::invalid("params", format!("{key} must be a string"))),
    }
}

fn required_str<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    str_param(p, key)?.ok_or_else(|| Error::invalid("params", format!("{key} is required")))
}

fn u64_param(p: &Value, key: &str) -> Result<Option<u64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_u64().map(Some).ok_or_else(|| Error::invalid("params", format!("{key} must be a whole number ≥ 0"))),
        Some(Value::String(s)) => s.trim().parse().map(Some).map_err(|_| Error::invalid("params", format!("{key} must be a whole number"))),
        Some(_) => Err(Error::invalid("params", format!("{key} must be a number"))),
    }
}

fn bool_param(p: &Value, key: &str) -> Result<Option<bool>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(Error::invalid("params", format!("{key} must be true or false"))),
    }
}

fn u32_param(p: &Value, key: &str) -> Result<Option<u32>> {
    u64_param(p, key)?.map(|v| u32::try_from(v).map_err(|_| Error::invalid("params", format!("{key} is too large")))).transpose()
}

/// The message a command acts on: `id`, or the open one.
fn target_message(s: &Session, p: &Value) -> Result<MessageId> {
    let id = match u64_param(p, "id")? {
        Some(id) => MessageId(id),
        None => s.message.ok_or_else(|| Error::invalid("params", "no message is open; pass id"))?,
    };
    s.store.message(id)?;
    Ok(id)
}

fn folder_param(s: &Session, p: &Value, key: &str) -> Result<Option<FolderId>> {
    let role = |name: &str| match name.trim().to_ascii_lowercase().as_str() {
        "inbox" => Some(FolderRole::Inbox),
        "drafts" => Some(FolderRole::Drafts),
        "sent" => Some(FolderRole::Sent),
        "outbox" => Some(FolderRole::Outbox),
        "archive" => Some(FolderRole::Archive),
        "junk" | "spam" => Some(FolderRole::Junk),
        "trash" | "deleted" => Some(FolderRole::Trash),
        _ => None,
    };
    let id = match p.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(name)) if role(name).is_some() => s.store.folder_by_role(role(name).unwrap_or(FolderRole::Inbox))?,
        _ => FolderId(u32_param(p, key)?.ok_or_else(|| Error::invalid("params", format!("{key} is required")))?),
    };
    s.store.folder(id)?;
    Ok(Some(id))
}

fn summary(m: &Message) -> Value {
    json!({
        "id": m.id.0,
        "folder": m.folder.0,
        "from": m.from.as_ref().map(ToString::to_string),
        "to": m.to.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "subject": m.subject,
        "date": m.date.map(|d| d.to_string()),
        "seen": m.seen,
        "flagged": m.flagged,
        "attachments": m.attachments.len(),
        "html": m.rich.is_some(),
    })
}

/// The `rrule` for `event.add`: a raw rule, or one built from `repeat` (and `until`).
fn repeat_param(p: &Value, when: When) -> Result<Option<String>> {
    let rule = match (str_param(p, "rrule")?, str_param(p, "repeat")?) {
        (Some(r), _) => {
            let r = r.trim();
            if r.len() > 1024 || r.contains(['\r', '\n']) {
                return Err(Error::invalid("rrule", "one line of at most 1024 characters"));
            }
            ibercorreo_core::recur::Rule::parse(r)?
        }
        (None, None) => return Ok(None),
        (None, Some(kind)) => {
            let text = match kind.trim().to_ascii_lowercase().as_str() {
                "" | "none" | "never" => return Ok(None),
                "daily" => "FREQ=DAILY",
                "weekdays" => "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
                "weekly" => "FREQ=WEEKLY",
                "monthly" => "FREQ=MONTHLY",
                "yearly" => "FREQ=YEARLY",
                other => return Err(Error::invalid("repeat", format!("{other:?}: use daily, weekdays, weekly, monthly or yearly"))),
            };
            ibercorreo_core::recur::Rule::parse(text)?
        }
    };
    let mut rule = rule;
    if let Some(until) = str_param(p, "until")? {
        let until = Date::parse(until)?;
        if until < when.start_date() {
            return Err(Error::invalid("until", "before the start"));
        }
        rule.until = Some((until, None));
    }
    Ok(Some(rule.to_text()))
}

fn event_json(e: &Event, when: When) -> Value {
    let (start, end, all_day) = match when {
        When::AllDay { start, end } => (start.to_string(), end.to_string(), true),
        When::Timed { start, end, .. } => (start.to_string(), end.to_string(), false),
    };
    json!({
        "id": e.id.0, "summary": e.summary, "start": start, "end": end, "allDay": all_day, "location": e.location,
        "repeat": e.rrule, "repeatUnderstood": e.rrule.is_none() || e.rule().is_some(),
    })
}

// ---- mail ----

fn set_view(s: &mut Session, v: View) -> Result<Value> {
    s.view = v;
    Ok(json!({}))
}

fn folder_list(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(Value::Array(
        s.store
            .folders()
            .iter()
            .map(|f| json!({ "id": f.id.0, "name": f.name, "role": format!("{:?}", f.role), "unread": s.store.unread_count(f.id) }))
            .collect(),
    ))
}

fn folder_select(s: &mut Session, p: &Value) -> Result<Value> {
    let f = folder_param(s, p, "folder")?.ok_or_else(|| Error::invalid("params", "folder is required"))?;
    s.folder = f;
    s.view = View::Mail;
    s.message = None;
    Ok(json!({ "folder": f.0 }))
}

fn message_list(s: &mut Session, p: &Value) -> Result<Value> {
    let f = folder_param(s, p, "folder")?.unwrap_or(s.folder);
    Ok(Value::Array(s.store.messages_in(f).into_iter().map(summary).collect()))
}

fn message_select(s: &mut Session, p: &Value) -> Result<Value> {
    let id = MessageId(u64_param(p, "id")?.ok_or_else(|| Error::invalid("params", "id is required"))?);
    let m = s.store.message_mut(id)?;
    // Opening a message marks it read, as Outlook does by default.
    m.seen = true;
    let (folder, body) = (m.folder, m.body.clone());
    s.folder = folder;
    s.message = Some(id);
    let m = s.store.message(id)?;
    let mut v = summary(m);
    if let Some(o) = v.as_object_mut() {
        o.insert("body".into(), Value::String(body));
    }
    Ok(v)
}

fn message_mark_read(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_message(s, p)?;
    let read = bool_param(p, "read")?.unwrap_or(true);
    s.store.message_mut(id)?.seen = read;
    Ok(json!({ "id": id.0, "read": read }))
}

fn message_flag(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_message(s, p)?;
    let m = s.store.message_mut(id)?;
    m.flagged = bool_param(p, "flagged")?.unwrap_or(!m.flagged);
    Ok(json!({ "id": id.0, "flagged": m.flagged }))
}

fn message_move(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_message(s, p)?;
    let to = folder_param(s, p, "folder")?.ok_or_else(|| Error::invalid("params", "folder is required"))?;
    s.store.move_message(id, to)?;
    s.fix_selection();
    Ok(json!({ "id": id.0, "folder": to.0 }))
}

fn move_to_role(s: &mut Session, p: &Value, role: FolderRole) -> Result<Value> {
    let id = target_message(s, p)?;
    let to = s.store.folder_by_role(role)?;
    s.store.move_message(id, to)?;
    s.fix_selection();
    s.status = format!("Moved to {}", s.store.folder(to)?.name);
    Ok(json!({ "id": id.0, "folder": to.0 }))
}

fn message_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_message(s, p)?;
    let trash = s.store.folder_by_role(FolderRole::Trash)?;
    if s.store.message(id)?.folder == trash {
        s.store.remove_message(id)?;
        s.fix_selection();
        s.status = "Deleted for good".into();
        Ok(json!({ "id": id.0, "deleted": true }))
    } else {
        move_to_role(s, p, FolderRole::Trash)
    }
}

pub(crate) fn the_user(s: &Session) -> Address {
    s.store.accounts.first().map(|a| a.address.clone()).unwrap_or_else(|| Address { name: Some("Me".into()), email: "me@localhost".into() })
}

fn new_draft(s: &mut Session, to: Vec<Address>, cc: Vec<Address>, subject: &str, body: &str) -> Result<Value> {
    let drafts = s.store.folder_by_role(FolderRole::Drafts)?;
    let mut m = Message::new(MessageId(0), drafts);
    m.from = Some(the_user(s));
    m.to = to;
    m.cc = cc;
    m.set_subject(subject);
    m.body = ibercorreo_core::clamp_text(body, ibercorreo_mime::MAX_BODY_BYTES).to_string();
    m.seen = true;
    m.date = Some(s.now());
    let id = s.store.add_message(m)?;
    s.folder = drafts;
    s.message = Some(id);
    s.view = View::Mail;
    Ok(json!({ "id": id.0 }))
}

fn message_compose(s: &mut Session, p: &Value) -> Result<Value> {
    let to = Address::parse_list(str_param(p, "to")?.unwrap_or(""))?;
    let cc = Address::parse_list(str_param(p, "cc")?.unwrap_or(""))?;
    new_draft(s, to, cc, str_param(p, "subject")?.unwrap_or(""), str_param(p, "body")?.unwrap_or(""))
}

fn message_update(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_message(s, p)?;
    let drafts = s.store.folder_by_role(FolderRole::Drafts)?;
    if s.store.message(id)?.folder != drafts {
        return Err(Error::invalid("message", "only drafts can be edited"));
    }
    // Parse everything before changing anything.
    let to = str_param(p, "to")?.map(Address::parse_list).transpose()?;
    let cc = str_param(p, "cc")?.map(Address::parse_list).transpose()?;
    let subject = str_param(p, "subject")?;
    let body = str_param(p, "body")?;
    let m = s.store.message_mut(id)?;
    if let Some(to) = to {
        m.to = to;
    }
    if let Some(cc) = cc {
        m.cc = cc;
    }
    if let Some(subject) = subject {
        m.set_subject(subject);
    }
    if let Some(body) = body {
        m.body = ibercorreo_core::clamp_text(body, ibercorreo_mime::MAX_BODY_BYTES).to_string();
    }
    Ok(json!({ "id": id.0 }))
}

fn quote(m: &Message) -> String {
    let who = m.from.as_ref().map(ToString::to_string).unwrap_or_default();
    let when = m.date.map(|d| d.to_string()).unwrap_or_default();
    let quoted: String = m.body.lines().map(|l| format!("> {l}\n")).collect();
    format!("\n\nOn {when}, {who} wrote:\n{quoted}")
}

fn prefixed(prefix: &str, subject: &str) -> String {
    let t = subject.trim();
    // Don't stack "Re: Re:"; also accept the Portuguese/Spanish "RE:" and "ENC:"/"RV:" forms.
    let lower = t.to_lowercase();
    if lower.starts_with(&format!("{}:", prefix.to_lowercase())) { t.to_string() } else { format!("{prefix}: {t}") }
}

fn reply(s: &mut Session, p: &Value, all: bool) -> Result<Value> {
    let id = target_message(s, p)?;
    let orig = s.store.message(id)?.clone();
    let me = the_user(s);
    let mut to: Vec<Address> = orig.from.iter().cloned().collect();
    let mut cc = Vec::new();
    if all {
        for a in orig.to.iter().chain(&orig.cc) {
            if a.email != me.email && !to.iter().any(|x| x.email == a.email) && !cc.iter().any(|x: &Address| x.email == a.email) {
                cc.push(a.clone());
            }
        }
    }
    if to.is_empty() {
        to = orig.to.clone();
    }
    new_draft(s, to, cc, &prefixed("Re", &orig.subject), &quote(&orig))
}

fn forward(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_message(s, p)?;
    let orig = s.store.message(id)?.clone();
    let to = Address::parse_list(str_param(p, "to")?.unwrap_or(""))?;
    new_draft(s, to, Vec::new(), &prefixed("Fwd", &orig.subject), &quote(&orig))
}

fn rfc5322_date(dt: DateTime) -> String {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let d = dt.date;
    let day = DAYS.get(usize::from(d.weekday().index())).copied().unwrap_or("Mon");
    let month = MONTHS.get(usize::from(d.month()).saturating_sub(1)).copied().unwrap_or("Jan");
    format!("{day}, {} {month} {:04} {:02}:{:02}:{:02} +0000", d.day(), d.year(), dt.hour(), dt.minute(), dt.second())
}

/// The message as RFC 5322 text. The `Message-ID` is made from the message's id and its date, so
/// rendering the same queued message twice gives the same bytes (what is sent is what is saved).
pub(crate) fn render(s: &Session, id: MessageId) -> Result<String> {
    let m = s.store.message(id)?;
    let from = m.from.clone().unwrap_or_else(|| the_user(s));
    let when = m.date.unwrap_or_else(|| s.now());
    let date = rfc5322_date(when);
    let unique = format!("{}.{:05}.{}.ibercorreo", when.date.day_number(), u32::from(when.minute_of_day()) * 60 + u32::from(when.second()), id.0);
    ibercorreo_mime::write(m, &ibercorreo_mime::Outgoing { from: &from, date: &date, message_id: &unique })
}

fn message_send(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_message(s, p)?;
    let drafts = s.store.folder_by_role(FolderRole::Drafts)?;
    if s.store.message(id)?.folder != drafts {
        return Err(Error::invalid("message", "only drafts can be sent"));
    }
    // Fails on no recipients; proves the message can be written before it leaves Drafts.
    render(s, id)?;
    let outbox = s.store.folder_by_role(FolderRole::Outbox)?;
    s.store.message_mut(id)?.date = Some(s.now());
    s.store.move_message(id, outbox)?;
    s.fix_selection();
    s.status = "Queued in Outbox: it is sent at the next sync".into();
    Ok(json!({ "id": id.0, "folder": outbox.0, "queued": true }))
}

fn message_read(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_message(s, p)?;
    let m = s.store.message(id)?;
    let mut v = summary(m);
    if let Some(o) = v.as_object_mut() {
        o.insert("cc".into(), json!(m.cc.iter().map(ToString::to_string).collect::<Vec<_>>()));
        o.insert("body".into(), Value::String(m.body.clone()));
        let rich = m.rich.as_ref().map(serde_json::to_value).transpose().map_err(|e| Error::invalid("message", e.to_string()))?;
        o.insert("rich".into(), rich.unwrap_or(Value::Null));
    }
    Ok(v)
}

fn view_plain_text(s: &mut Session, p: &Value) -> Result<Value> {
    s.plain_text = match p.get("on") {
        None | Some(Value::Null) => !s.plain_text,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(Error::invalid("on", "expected true or false")),
    };
    s.status = if s.plain_text { "HTML mail is shown as plain text" } else { "HTML mail is shown formatted" }.into();
    Ok(json!({ "plainText": s.plain_text }))
}

fn message_source(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_message(s, p)?;
    Ok(Value::String(render(s, id)?))
}

fn import_eml(s: &mut Session, p: &Value) -> Result<Value> {
    let raw = required_str(p, "raw")?;
    let f = match folder_param(s, p, "folder")? {
        Some(f) => f,
        None => s.store.folder_by_role(FolderRole::Inbox)?,
    };
    let id = s.store.import_eml(raw.as_bytes(), f)?;
    Ok(summary(s.store.message(id)?))
}

fn search(s: &mut Session, p: &Value) -> Result<Value> {
    let q = required_str(p, "query")?;
    s.search = ibercorreo_core::clamp_text(q, 1024).to_string();
    Ok(Value::Array(s.store.search(&s.search).into_iter().take(500).map(summary).collect()))
}

// ---- calendar ----

fn calendar_goto(s: &mut Session, p: &Value) -> Result<Value> {
    let d = required_str(p, "date")?;
    s.day = if d.eq_ignore_ascii_case("today") { s.now().date } else { Date::parse(d)? };
    s.view = View::Calendar;
    Ok(json!({ "date": s.day.to_string() }))
}

fn shift_month(s: &mut Session, delta: i32) -> Result<Value> {
    s.day = s.day.add_months(delta)?;
    Ok(json!({ "date": s.day.to_string() }))
}

fn event_list(s: &mut Session, p: &Value) -> Result<Value> {
    let day = match str_param(p, "date")? {
        Some(d) if d.eq_ignore_ascii_case("today") => s.now().date,
        Some(d) => Date::parse(d)?,
        None => s.day,
    };
    Ok(Value::Array(s.store.events_on(day).into_iter().map(|o| event_json(o.event, o.when)).collect()))
}

fn event_add(s: &mut Session, p: &Value) -> Result<Value> {
    let summary = required_str(p, "summary")?;
    let start = required_str(p, "start")?;
    let end = str_param(p, "end")?;
    let when = if start.contains('T') {
        let (st, utc) = DateTime::parse(start)?;
        let en = match end {
            Some(e) => DateTime::parse(e)?.0,
            None => {
                let next_hour = st.hour().saturating_add(1);
                if next_hour > 23 {
                    DateTime::new(st.date.add_days(1)?, 0, st.minute(), 0)?
                } else {
                    DateTime::new(st.date, next_hour, st.minute(), 0)?
                }
            }
        };
        When::timed(st, en, utc)?
    } else {
        let st = Date::parse(start)?;
        let en = match end {
            Some(e) => Date::parse(e)?,
            None => st.add_days(1)?,
        };
        When::all_day(st, en)?
    };
    let seq = s.store.events().len();
    let e = Event {
        id: EventId(0),
        calendar: 0,
        uid: format!("{}-{seq}@ibercorreo.local", when.start_date()),
        summary: ibercorreo_core::clamp_text(summary, ibercorreo_core::MAX_FIELD_LEN).to_string(),
        location: ibercorreo_core::clamp_text(str_param(p, "location")?.unwrap_or(""), ibercorreo_core::MAX_FIELD_LEN).to_string(),
        description: ibercorreo_core::clamp_text(str_param(p, "description")?.unwrap_or(""), ibercorreo_core::MAX_FIELD_LEN).to_string(),
        when,
        tzid: None,
        rrule: repeat_param(p, when)?,
        exdates: Vec::new(),
        recurrence_id: None,
    };
    let id = s.store.add_event(e)?;
    s.event = Some(id);
    s.day = when.start_date();
    let e = s.store.event(id)?;
    Ok(event_json(e, e.when))
}

fn event_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let id = match u64_param(p, "id")? {
        Some(id) => EventId(id),
        None => s.event.ok_or_else(|| Error::invalid("params", "no event is selected; pass id"))?,
    };
    if let Some(day) = str_param(p, "occurrence")? {
        let day = Date::parse(day)?;
        let e = s.store.event_mut(id)?;
        if e.rrule.is_none() {
            return Err(Error::invalid("occurrence", "the event does not repeat; delete it without occurrence"));
        }
        if !e.occurs_on(day) {
            return Err(Error::invalid("occurrence", format!("the event has no occurrence on {day}")));
        }
        if e.exdates.len() >= 1_000 {
            return Err(Error::invalid("occurrence", "too many deleted occurrences; delete the series"));
        }
        // The occurrence starting on `day`, or the one running through it.
        let start = e.occurrence_on(day).map_or(day, |w| w.start_date());
        e.exdates.push(start);
        return Ok(json!({ "id": id.0, "deleted": start.to_string() }));
    }
    s.store.remove_event(id)?;
    s.fix_selection();
    Ok(json!({ "id": id.0, "deleted": true }))
}

fn import_ics(s: &mut Session, p: &Value) -> Result<Value> {
    let text = required_str(p, "text")?;
    let cal = u32_param(p, "calendar")?.unwrap_or(0);
    let r = s.store.import_ics(text, cal)?;
    s.status = format!("Imported {} new and {} updated events", r.added, r.updated);
    serde_json::to_value(&r).map_err(|e| Error::invalid("report", e.to_string()))
}

fn export_ics(s: &mut Session, p: &Value) -> Result<Value> {
    let cal = u32_param(p, "calendar")?;
    if let Some(c) = cal {
        s.store.calendar(c)?;
    }
    Ok(Value::String(s.store.export_ics(cal)))
}

// ---- people ----

fn contact_add(s: &mut Session, p: &Value) -> Result<Value> {
    let name = required_str(p, "name")?.trim();
    if name.is_empty() {
        return Err(Error::invalid("params", "name is empty"));
    }
    let emails = match str_param(p, "email")? {
        Some(e) => Address::parse_list(e)?,
        None => Vec::new(),
    };
    let clamp = |v: Option<&str>| v.map(|v| ibercorreo_core::clamp_text(v.trim(), 1024).to_string()).filter(|v| !v.is_empty());
    let c = Contact {
        id: 0,
        name: ibercorreo_core::clamp_text(name, 1024).to_string(),
        emails,
        phone: clamp(str_param(p, "phone")?),
        organization: clamp(str_param(p, "organization")?),
    };
    Ok(json!({ "id": s.store.add_contact(c) }))
}

fn contact_list(s: &mut Session, p: &Value) -> Result<Value> {
    let q = str_param(p, "query")?.unwrap_or("");
    Ok(Value::Array(
        s.store
            .contacts(q)
            .into_iter()
            .map(|c| json!({ "id": c.id, "name": c.name, "emails": c.emails.iter().map(ToString::to_string).collect::<Vec<_>>(), "phone": c.phone, "organization": c.organization }))
            .collect(),
    ))
}

// ---- account ----

/// A server name: letters, digits, dots, hyphens (or an IPv6 literal's colons), at most 253 bytes.
fn host_param(p: &Value, key: &str) -> Result<String> {
    let h = required_str(p, key)?.trim().trim_end_matches('.');
    let ok = !h.is_empty() && h.len() <= 253 && h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'));
    if ok { Ok(h.to_ascii_lowercase()) } else { Err(Error::invalid("params", format!("{key} is not a server name"))) }
}

fn security_param(p: &Value, key: &str) -> Result<Security> {
    match str_param(p, key)?.map(str::to_ascii_lowercase).as_deref() {
        None | Some("tls" | "ssl") => Ok(Security::Tls),
        Some("starttls") => Ok(Security::StartTls),
        Some(other) => Err(Error::invalid("params", format!("{key} must be tls or starttls, not {other:?}"))),
    }
}

fn port_param(p: &Value, key: &str, default: u16) -> Result<u16> {
    match u64_param(p, key)? {
        None => Ok(default),
        Some(n) => u16::try_from(n).ok().filter(|&n| n > 0).ok_or_else(|| Error::invalid("params", format!("{key} must be 1–65535"))),
    }
}

fn account_set(s: &mut Session, p: &Value) -> Result<Value> {
    let email = required_str(p, "email")?;
    let name = str_param(p, "name")?.map(str::trim).filter(|n| !n.is_empty());
    let address = Address::new(name, email)?;
    let imap_security = security_param(p, "imapSecurity")?;
    let smtp_security = security_param(p, "smtpSecurity")?;
    let user = str_param(p, "user")?.map(str::trim).unwrap_or("");
    if user.len() > 320 || user.chars().any(char::is_control) {
        return Err(Error::invalid("params", "user is not a sign-in name"));
    }
    let kind = AccountKind::Imap {
        imap_host: host_param(p, "imapHost")?,
        imap_port: port_param(p, "imapPort", imap_security.default_imap_port())?,
        smtp_host: host_param(p, "smtpHost")?,
        smtp_port: port_param(p, "smtpPort", smtp_security.default_smtp_port())?,
        user: user.to_string(),
        imap_security,
        smtp_security,
    };
    let account = Account { name: address.display().to_string(), address, kind };
    // One account for now (several arrive in M3): the first is replaced.
    match s.store.accounts.first_mut() {
        Some(a) => *a = account,
        None => s.store.accounts.push(account),
    }
    s.status = "Account saved".into();
    account_get(s, &Value::Null)
}

fn account_get(s: &mut Session, _: &Value) -> Result<Value> {
    let a = s.store.accounts.first().ok_or_else(|| Error::not_found("account", "none set up"))?;
    let mut v = json!({ "name": a.name, "email": a.address.email, "displayName": a.address.name });
    if let (AccountKind::Imap { imap_host, imap_port, smtp_host, smtp_port, imap_security, smtp_security, .. }, Some(obj)) =
        (&a.kind, v.as_object_mut())
    {
        let sec = |x: &Security| if *x == Security::Tls { "tls" } else { "starttls" };
        for (k, val) in [
            ("imapHost", json!(imap_host)),
            ("imapPort", json!(imap_port)),
            ("imapSecurity", json!(sec(imap_security))),
            ("smtpHost", json!(smtp_host)),
            ("smtpPort", json!(smtp_port)),
            ("smtpSecurity", json!(sec(smtp_security))),
            ("user", json!(a.login())),
        ] {
            obj.insert(k.into(), val);
        }
    }
    Ok(v)
}

/// Ask the front end to send the Outbox and sync: the engine never touches the network itself.
fn account_sync(s: &mut Session, _: &Value) -> Result<Value> {
    let has_server = s.store.accounts.first().is_some_and(|a| matches!(a.kind, AccountKind::Imap { .. }));
    if !has_server {
        return Err(Error::invalid("account", "no mail server set up: open Account settings"));
    }
    s.sync_requested = true;
    s.status = "Sending and receiving…".into();
    Ok(json!({ "requested": true }))
}

// ---- file ----

fn file_save(s: &mut Session, p: &Value) -> Result<Value> {
    let path = match str_param(p, "path")? {
        Some(p) => std::path::PathBuf::from(p),
        None => s.path.clone().ok_or_else(|| Error::invalid("params", "no file yet; pass path"))?,
    };
    s.store.save(&path)?;
    s.path = Some(path.clone());
    s.dirty = false;
    s.status = format!("Saved {}", path.display());
    Ok(json!({ "path": path.display().to_string() }))
}
