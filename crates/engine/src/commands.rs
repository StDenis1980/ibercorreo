//! The command registry. Each entry is everything a front end needs to show and run one action.

use ibercorreo_core::calendar::When;
use ibercorreo_core::mail::{Account, AccountKind};
use ibercorreo_core::{
    Address, Contact, Date, DateTime, Error, Event, EventId, FolderId, FolderRole, Message, MessageId, Phone, Result, Security, Zone,
};
use serde_json::{Value, json};

use crate::{CalendarMode, Session, View};

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
        id: "calendar.next",
        label: "Forward",
        group: "Calendar/Go To",
        shortcut: Some("Alt+Right"),
        params: "",
        edits: false,
        run: |s, _| shift_period(s, 1),
    },
    CommandSpec {
        id: "calendar.prev",
        label: "Back",
        group: "Calendar/Go To",
        shortcut: Some("Alt+Left"),
        params: "",
        edits: false,
        run: |s, _| shift_period(s, -1),
    },
    CommandSpec {
        id: "calendar.timeZone",
        label: "Time zone",
        group: "File/Options",
        shortcut: None,
        params: "zone?: an IANA id (Europe/Lisbon), a Windows name (GMT Standard Time) or \"system\" — default: just say which is in use",
        edits: true,
        run: time_zone,
    },
    CommandSpec {
        id: "calendar.viewDay",
        label: "Day",
        group: "Calendar/Arrange",
        shortcut: Some("Ctrl+Alt+1"),
        params: "",
        edits: false,
        run: |s, _| set_calendar_mode(s, CalendarMode::Day),
    },
    CommandSpec {
        id: "calendar.viewWorkWeek",
        label: "Work Week",
        group: "Calendar/Arrange",
        shortcut: Some("Ctrl+Alt+2"),
        params: "",
        edits: false,
        run: |s, _| set_calendar_mode(s, CalendarMode::WorkWeek),
    },
    CommandSpec {
        id: "calendar.viewWeek",
        label: "Week",
        group: "Calendar/Arrange",
        shortcut: Some("Ctrl+Alt+3"),
        params: "",
        edits: false,
        run: |s, _| set_calendar_mode(s, CalendarMode::Week),
    },
    CommandSpec {
        id: "calendar.viewMonth",
        label: "Month",
        group: "Calendar/Arrange",
        shortcut: Some("Ctrl+Alt+4"),
        params: "",
        edits: false,
        run: |s, _| set_calendar_mode(s, CalendarMode::Month),
    },
    CommandSpec {
        id: "event.list",
        label: "List events",
        group: "Calendar/Arrange",
        shortcut: None,
        params: "date?: YYYY-MM-DD | \"today\" — default: the calendar's day\nfrom?, to?: YYYY-MM-DD — every occurrence in the range instead (at most 366 days); \"view\" for from takes what the calendar shows",
        edits: false,
        run: event_list,
    },
    CommandSpec {
        id: "event.add",
        label: "New Appointment",
        group: "Calendar/New",
        shortcut: Some("Ctrl+Shift+A"),
        params: "summary: string\nstart: YYYY-MM-DD (all day) | YYYY-MM-DDTHH:MM\nend?: same form — default: one hour, or one day\nlocation?: string\ndescription?: string\nrepeat?: daily | weekdays | weekly | monthly | yearly\nrrule?: string — an iCalendar RRULE, instead of repeat\nuntil?: YYYY-MM-DD — the last day it repeats\nreminder?: minutes before | \"none\" — default 15, none for all-day\ncalendar?: number — the calendar's id (calendar.list), default the first one",
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
        id: "event.setReminder",
        label: "Reminder",
        group: "Appointment/Options",
        shortcut: None,
        params: "id?: number — default: the selected event\nreminder: minutes before the start (negative: after) | \"none\"",
        edits: true,
        run: event_set_reminder,
    },
    CommandSpec { id: "reminder.list", label: "Reminders", group: "View/Window", shortcut: None, params: "", edits: false, run: reminder_list },
    CommandSpec {
        id: "reminder.dismiss",
        label: "Dismiss",
        group: "Reminders/Reminder",
        shortcut: None,
        params: "id?: number, start?: as reminder.list gives it — default: every due reminder",
        edits: true,
        run: reminder_dismiss,
    },
    CommandSpec {
        id: "reminder.snooze",
        label: "Snooze",
        group: "Reminders/Reminder",
        shortcut: None,
        params: "id?: number, start?: as reminder.list gives it — default: every due reminder\nminutes?: number — default 5, at most a week",
        edits: true,
        run: reminder_snooze,
    },
    CommandSpec {
        id: "calendar.list",
        label: "Calendars",
        group: "Calendar/Manage Calendars",
        shortcut: None,
        params: "",
        edits: false,
        run: calendar_list,
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
        shortcut: Some("Ctrl+Shift+C"),
        params: CONTACT_FIELDS,
        edits: true,
        run: contact_add,
    },
    CommandSpec {
        id: "contact.update",
        label: "Edit Contact",
        group: "People/Current View",
        shortcut: None,
        params: "id: number\n(then any of the fields of contact.add; a field left out stays as it is, \"\" clears it)",
        edits: true,
        run: contact_update,
    },
    CommandSpec {
        id: "contact.delete",
        label: "Delete Contact",
        group: "People/Delete",
        shortcut: None,
        params: "id: number",
        edits: true,
        run: contact_delete,
    },
    CommandSpec {
        id: "contact.get",
        label: "Show contact",
        group: "People/Current View",
        shortcut: None,
        params: "id: number",
        edits: false,
        run: contact_get,
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
        id: "contact.importVcf",
        label: "Import contacts (.vcf)",
        group: "File/Open",
        shortcut: None,
        params: "text: string — vCard 2.1, 3.0 or 4.0; a card whose UID (or, without one, an address) is already here updates that contact",
        edits: true,
        run: contact_import,
    },
    CommandSpec {
        id: "contact.exportVcf",
        label: "Export contacts (.vcf)",
        group: "File/Save",
        shortcut: None,
        params: "id?: number — one contact (default: all)",
        edits: false,
        run: contact_export,
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
        params: "email: string — the address mail is sent from\nname?: string — shown to recipients\nimapHost: string\nimapPort?: number — default 993 (TLS) or 143 (STARTTLS)\nimapSecurity?: \"tls\" | \"starttls\" — default tls\nsmtpHost: string\nsmtpPort?: number — default 465 (TLS) or 587 (STARTTLS)\nsmtpSecurity?: \"tls\" | \"starttls\" — default tls\nuser?: string — sign-in name, default the email address\ncaldav?: string — CalDAV address (https://…: the server, your principal or one calendar), signed in to with the same name and password; omitted keeps the current one, \"\" stops syncing calendars\n(The password is never a parameter: the app keeps it in the system's credential store.)",
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
        id: "invite.respond",
        label: "Respond",
        group: "Home/Respond",
        shortcut: None,
        params: "response: \"accept\" | \"tentative\" | \"decline\"\nid?: number — the invitation (default: the open message)\nreply?: \"send\" | \"edit\" | \"none\" — send the answer to the organizer now (default), open it as a draft first, or answer nobody\ncomment?: string — added to the answer\ncalendar?: number — where an accepted meeting goes (default: where it already is, else the first calendar)",
        edits: true,
        run: invite_respond,
    },
    CommandSpec {
        id: "invite.remove",
        label: "Remove from Calendar",
        group: "Home/Respond",
        shortcut: None,
        params: "id?: number — the cancellation or invitation (default: the open message)",
        edits: true,
        run: invite_remove,
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

/// `reminder`: minutes before the start (negative: after), or `"none"` / `false` for none.
fn reminder_param(p: &Value) -> Result<Option<Vec<i32>>> {
    let bad = || Error::invalid("params", "reminder must be minutes before the start, or \"none\"");
    Ok(match p.get("reminder") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(false)) => Some(Vec::new()),
        Some(Value::String(s)) if s.trim().eq_ignore_ascii_case("none") => Some(Vec::new()),
        Some(Value::String(s)) => Some(ibercorreo_core::calendar::clean_reminders(vec![s.trim().parse::<i64>().map_err(|_| bad())?])),
        Some(Value::Number(n)) => Some(ibercorreo_core::calendar::clean_reminders(vec![n.as_i64().ok_or_else(bad)?])),
        Some(_) => return Err(bad()),
    })
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
        "date": m.date.map(|d| format!("{d}Z")),
        "seen": m.seen,
        "flagged": m.flagged,
        "attachments": m.attachments.len(),
        "html": m.rich.is_some(),
        "invite": m.invite.is_some(),
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
        "calendar": e.calendar, "repeat": e.rrule, "repeatUnderstood": e.rrule.is_none() || e.rule().is_some(),
        "timeZone": e.tzid, "timeZoneUnderstood": e.tzid.is_none() || e.zone().is_some(), "reminders": e.reminders,
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
        o.insert("invite".into(), invite_info(s, m).map(|i| i.to_json()).unwrap_or(Value::Null));
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

// ---- meeting invitations (iTIP, RFC 5546, by mail: iMIP, RFC 6047) ----

/// What the reading pane shows about a message's invitation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteInfo {
    /// `REQUEST` (an invitation or an update), `CANCEL`, `REPLY`, `PUBLISH`…
    pub method: String,
    pub summary: String,
    pub location: String,
    /// The meeting's first event, as shown in the calendar's zone.
    pub when: Option<When>,
    pub repeats: bool,
    /// The organizer, as a display string.
    pub organizer: Option<String>,
    /// For a request: this account's answer so far as the organizer recorded it (`NEEDS-ACTION`
    /// …). For a reply: the attendee who answered and how.
    pub status: Option<String>,
    pub replier: Option<String>,
    /// Whether the meeting (its UID) is in a calendar here.
    pub in_calendar: bool,
}

impl InviteInfo {
    pub fn to_json(&self) -> Value {
        let (start, end, all_day) = match self.when {
            Some(When::AllDay { start, end }) => (Some(start.to_string()), Some(end.to_string()), true),
            Some(When::Timed { start, end, .. }) => (Some(start.to_string()), Some(end.to_string()), false),
            None => (None, None, false),
        };
        json!({
            "method": self.method, "summary": self.summary, "location": self.location, "start": start, "end": end,
            "allDay": all_day, "repeats": self.repeats, "organizer": self.organizer, "status": self.status,
            "replier": self.replier, "inCalendar": self.in_calendar,
        })
    }
}

fn party_display(p: &ibercorreo_ical::Party) -> String {
    match &p.name {
        Some(n) => format!("{n} <{}>", p.email),
        None => p.email.clone(),
    }
}

pub(crate) fn invite_info(s: &Session, m: &Message) -> Option<InviteInfo> {
    let inv = ibercorreo_ical::invite(m.invite.as_deref()?).ok()?;
    let me = the_user(s);
    let first = inv.events.iter().find(|e| e.recurrence_id.is_none()).or(inv.events.first());
    let when = first.map(|e| {
        let zone = e.tzid.as_deref().and_then(Zone::named);
        e.when.in_zone(zone.as_ref(), s.zone())
    });
    let uid = first.map(|e| e.uid.as_str());
    let (status, replier) = if inv.method == "REPLY" {
        let a = inv.attendees.first();
        (a.and_then(|a| a.status.clone()), a.map(party_display))
    } else {
        (inv.attendees.iter().find(|a| a.email.eq_ignore_ascii_case(&me.email)).and_then(|a| a.status.clone()), None)
    };
    Some(InviteInfo {
        method: inv.method.clone(),
        summary: first.map(|e| e.summary.clone()).unwrap_or_default(),
        location: first.map(|e| e.location.clone()).unwrap_or_default(),
        when,
        repeats: first.is_some_and(|e| e.rrule.is_some()),
        organizer: inv.organizer.as_ref().map(party_display),
        status,
        replier,
        in_calendar: uid.is_some_and(|u| s.store.events().iter().any(|e| e.uid == u)),
    })
}

/// The open (or `id`) message's invitation, read.
fn message_invite(s: &Session, p: &Value) -> Result<(MessageId, String, ibercorreo_ical::Invite)> {
    let id = target_message(s, p)?;
    let text = s.store.message(id)?.invite.clone().ok_or_else(|| Error::invalid("message", "not a meeting invitation"))?;
    let inv = ibercorreo_ical::invite(&text)?;
    if inv.events.is_empty() {
        return Err(Error::invalid("invitation", "it holds no meeting that can be read"));
    }
    Ok((id, text, inv))
}

/// Put the invitation's events in a calendar: a meeting already here (same UID and occurrence)
/// is updated where it is, keeping its reminders; a new one goes to `calendar`, or next to the
/// rest of its series, or to the first calendar. Returns the events' ids.
fn add_meeting(s: &mut Session, inv: &ibercorreo_ical::Invite, calendar: Option<u32>) -> Result<Vec<EventId>> {
    let first_uid = inv.events.first().map(|e| e.uid.clone()).unwrap_or_default();
    let calendar = match calendar {
        Some(c) => s.store.calendar(c)?.id,
        None => match s.store.events().iter().find(|e| e.uid == first_uid) {
            Some(e) => e.calendar,
            None => s.store.calendars().first().map_or(0, |c| c.id),
        },
    };
    let mut ids = Vec::new();
    for ie in &inv.events {
        let existing = s.store.events().iter().find(|e| e.uid == ie.uid && e.recurrence_id == ie.recurrence_id).map(|e| e.id);
        match existing {
            Some(id) => {
                let e = s.store.event_mut(id)?;
                e.summary = ie.summary.clone();
                e.location = ie.location.clone();
                e.description = ie.description.clone();
                e.when = ie.when;
                e.tzid = ie.tzid.clone();
                e.rrule = ie.rrule.clone();
                e.exdates = ie.exdates.clone();
                ids.push(id);
            }
            None => {
                let mut ev = ie.clone().into_event(EventId(0), calendar);
                // As Outlook: an accepted meeting reminds 15 minutes before unless it says otherwise.
                if ev.reminders.is_empty() && matches!(ev.when, When::Timed { .. }) {
                    ev.reminders = vec![15];
                }
                ids.push(s.store.add_event(ev)?);
            }
        }
    }
    Ok(ids)
}

/// Take the invitation's meeting out of the calendars: a whole series, or only the occurrences
/// it names (each also cancelled in its series). Returns how many events went.
fn remove_meeting(s: &mut Session, inv: &ibercorreo_ical::Invite) -> Result<usize> {
    let mut removed = 0usize;
    for ie in &inv.events {
        let ids: Vec<EventId> = s
            .store
            .events()
            .iter()
            .filter(|e| e.uid == ie.uid && (ie.recurrence_id.is_none() || e.recurrence_id == ie.recurrence_id))
            .map(|e| e.id)
            .collect();
        for id in ids {
            s.store.remove_event(id)?;
            removed = removed.saturating_add(1);
        }
        if let Some(day) = ie.recurrence_id {
            let series = s.store.events().iter().find(|e| e.uid == ie.uid && e.recurrence_id.is_none() && e.rrule.is_some()).map(|e| e.id);
            if let Some(id) = series {
                let e = s.store.event_mut(id)?;
                if !e.exdates.contains(&day) && e.exdates.len() < 1_000 {
                    e.exdates.push(day);
                    removed = removed.saturating_add(1);
                }
            }
        }
    }
    s.fix_selection();
    Ok(removed)
}

fn invite_respond(s: &mut Session, p: &Value) -> Result<Value> {
    let (id, text, inv) = message_invite(s, p)?;
    let (status, verb) = match required_str(p, "response")?.trim().to_ascii_lowercase().as_str() {
        "accept" | "accepted" => ("ACCEPTED", "Accepted"),
        "tentative" | "maybe" => ("TENTATIVE", "Tentative"),
        "decline" | "declined" => ("DECLINED", "Declined"),
        other => return Err(Error::invalid("response", format!("{other:?}: use accept, tentative or decline"))),
    };
    let reply = match str_param(p, "reply")?.map(|r| r.trim().to_ascii_lowercase()) {
        None => "send".to_string(),
        Some(r) if matches!(r.as_str(), "send" | "edit" | "none") => r,
        Some(r) => return Err(Error::invalid("reply", format!("{r:?}: use send, edit or none"))),
    };
    let comment = ibercorreo_core::clamp_text(str_param(p, "comment")?.unwrap_or(""), ibercorreo_core::MAX_FIELD_LEN).to_string();
    match inv.method.as_str() {
        "REQUEST" | "PUBLISH" => {}
        "CANCEL" => return Err(Error::invalid("invitation", "the meeting was cancelled; remove it from the calendar instead")),
        other => return Err(Error::invalid("invitation", format!("a {other} message is not an invitation to answer"))),
    }
    let calendar = u32_param(p, "calendar")?;
    let (events, removed) = if status == "DECLINED" { (Vec::new(), remove_meeting(s, &inv)?) } else { (add_meeting(s, &inv, calendar)?, 0) };
    let me = the_user(s);
    // A published event (no organizer to tell), or one this account organises, gets no answer.
    let organizer = inv.organizer.as_ref().filter(|o| inv.method == "REQUEST" && !o.email.eq_ignore_ascii_case(&me.email));
    let mut answer = None;
    if let (Some(org), false) = (organizer, reply == "none") {
        let mut m = Message::new(MessageId(0), s.store.folder_by_role(if reply == "send" { FolderRole::Outbox } else { FolderRole::Drafts })?);
        m.from = Some(me.clone());
        m.to = vec![Address::new(org.name.as_deref(), &org.email)?];
        let summary = inv.events.first().map(|e| e.summary.as_str()).unwrap_or("");
        m.set_subject(&format!("{verb}: {summary}"));
        m.body = comment.clone();
        m.seen = true;
        m.date = Some(s.now());
        m.invite = Some(ibercorreo_ical::reply(&text, &me, status, &comment, s.now())?);
        let mid = s.store.add_message(m)?;
        // Proves it can be written before it waits in the Outbox.
        render(s, mid)?;
        if reply == "edit" {
            s.folder = s.store.message(mid)?.folder;
            s.message = Some(mid);
            s.view = View::Mail;
        }
        answer = Some(mid);
    }
    s.status = match (answer, reply.as_str()) {
        (Some(_), "send") => format!("{verb}: the answer is sent at the next sync"),
        (Some(_), _) => format!("{verb}: edit the answer, then send it"),
        _ => verb.to_string(),
    };
    Ok(json!({
        "id": id.0, "response": status, "events": events.iter().map(|e| e.0).collect::<Vec<_>>(), "removed": removed,
        "reply": answer.map(|m| m.0), "queued": answer.is_some() && reply == "send",
    }))
}

fn invite_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let (id, _, inv) = message_invite(s, p)?;
    if inv.method == "REPLY" {
        return Err(Error::invalid("invitation", "a reply does not change the meeting here"));
    }
    let removed = remove_meeting(s, &inv)?;
    s.status = if removed == 0 { "The meeting was not in the calendar".into() } else { "Removed from the calendar".into() };
    Ok(json!({ "id": id.0, "removed": removed }))
}

// ---- calendar ----

fn calendar_goto(s: &mut Session, p: &Value) -> Result<Value> {
    let d = required_str(p, "date")?;
    s.day = if d.eq_ignore_ascii_case("today") { s.today() } else { Date::parse(d)? };
    s.view = View::Calendar;
    Ok(calendar_state(s))
}

fn shift_month(s: &mut Session, delta: i32) -> Result<Value> {
    s.day = s.day.add_months(delta)?;
    Ok(calendar_state(s))
}

/// What the calendar shows: its day, mode and first and last date.
fn calendar_state(s: &Session) -> Value {
    let (from, to) = s.calendar_range();
    json!({ "date": s.day.to_string(), "mode": s.calendar_mode.id(), "from": from.to_string(), "to": to.to_string() })
}

/// Show or choose the zone the calendar and mail times are shown in.
fn time_zone(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(z) = str_param(p, "zone")? {
        s.store.time_zone = if z.eq_ignore_ascii_case("system") {
            None
        } else {
            let zone = Zone::named(z).ok_or_else(|| Error::not_found("time zone", ibercorreo_core::clamp_text(z, 64)))?;
            Some(zone.name().to_string())
        };
    }
    // The session follows the store after the command; say what it will be.
    let zone = s.store.time_zone.as_deref().and_then(Zone::named).unwrap_or_else(|| s.system_zone().clone());
    Ok(json!({
        "zone": zone.name(),
        "offset": zone.offset_label(s.now()),
        "system": s.system_zone().name(),
        "followsSystem": s.store.time_zone.is_none(),
    }))
}

/// One day, week or month forward (`delta` 1) or back (-1), as the calendar mode says.
fn shift_period(s: &mut Session, delta: i32) -> Result<Value> {
    s.day = match s.calendar_mode {
        CalendarMode::Day => s.day.add_days(i64::from(delta))?,
        CalendarMode::WorkWeek | CalendarMode::Week => s.day.add_days(i64::from(delta).saturating_mul(7))?,
        CalendarMode::Month => s.day.add_months(delta)?,
    };
    Ok(calendar_state(s))
}

fn set_calendar_mode(s: &mut Session, mode: CalendarMode) -> Result<Value> {
    s.calendar_mode = mode;
    s.view = View::Calendar;
    Ok(calendar_state(s))
}

/// The longest range `event.list` expands, in days.
const MAX_LIST_DAYS: i64 = 366;

fn event_list(s: &mut Session, p: &Value) -> Result<Value> {
    let date = |s: &Session, d: &str| if d.eq_ignore_ascii_case("today") { Ok(s.today()) } else { Date::parse(d) };
    let (from, to) = match (str_param(p, "from")?, str_param(p, "to")?) {
        (Some(f), _) if f.eq_ignore_ascii_case("view") => s.calendar_range(),
        (Some(f), Some(t)) => (date(s, f)?, date(s, t)?),
        (Some(_), None) | (None, Some(_)) => return Err(Error::invalid("params", "pass both from and to")),
        (None, None) => {
            let day = match str_param(p, "date")? {
                Some(d) => date(s, d)?,
                None => s.day,
            };
            (day, day)
        }
    };
    if to < from {
        return Err(Error::invalid("params", format!("to {to} is before from {from}")));
    }
    let days = to.day_number().saturating_sub(from.day_number()).saturating_add(1);
    if days > MAX_LIST_DAYS {
        return Err(Error::invalid("params", format!("{days} days is more than {MAX_LIST_DAYS}; list a shorter range")));
    }
    // A long event shows once, on the first listed day it touches.
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for n in 0..days {
        let day = from.add_days(n)?;
        for o in s.store.events_on(day, s.zone()) {
            if seen.insert((o.event.id, o.when.start_date())) {
                out.push(event_json(o.event, o.when));
            }
        }
    }
    Ok(Value::Array(out))
}

fn event_add(s: &mut Session, p: &Value) -> Result<Value> {
    let summary = required_str(p, "summary")?;
    let start = required_str(p, "start")?;
    let end = str_param(p, "end")?;
    let when = if start.contains('T') {
        // Times typed without `Z` are in the zone the calendar is shown in.
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
    let calendar = match u32_param(p, "calendar")? {
        Some(c) => s.store.calendar(c)?.id,
        None => s.store.calendars().first().map_or(0, |c| c.id),
    };
    let e = Event {
        id: EventId(0),
        calendar,
        uid: new_uid(s, when, summary),
        summary: ibercorreo_core::clamp_text(summary, ibercorreo_core::MAX_FIELD_LEN).to_string(),
        location: ibercorreo_core::clamp_text(str_param(p, "location")?.unwrap_or(""), ibercorreo_core::MAX_FIELD_LEN).to_string(),
        description: ibercorreo_core::clamp_text(str_param(p, "description")?.unwrap_or(""), ibercorreo_core::MAX_FIELD_LEN).to_string(),
        when,
        tzid: match when {
            When::Timed { utc: false, .. } if !s.zone().is_utc() => Some(s.zone().name().to_string()),
            _ => None,
        },
        rrule: repeat_param(p, when)?,
        exdates: Vec::new(),
        recurrence_id: None,
        // As Outlook: 15 minutes before an appointment, none for an all-day event.
        reminders: match reminder_param(p)? {
            Some(r) => r,
            None if matches!(when, When::Timed { .. }) => vec![15],
            None => Vec::new(),
        },
        remote: None,
    };
    let id = s.store.add_event(e)?;
    s.event = Some(id);
    s.day = when.start_date();
    let e = s.store.event(id)?;
    Ok(event_json(e, e.when.in_zone(e.zone().as_ref(), s.zone())))
}

/// A UID unique enough to share a CalDAV calendar with other devices: the time it was made, the
/// store's next event number and a hash of them with the title, as RFC 5545 §3.8.4.7 suggests.
fn new_uid(s: &Session, when: When, summary: &str) -> String {
    use std::hash::{Hash, Hasher};
    let unix = s.now().to_unix();
    let seq = s.store.events().len();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (unix, seq, when.start_date().to_string(), summary).hash(&mut h);
    format!("{}-{unix}-{seq}-{:016x}@ibercorreo", when.start_date(), h.finish())
}

fn calendar_list(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(Value::Array(
        s.store
            .calendars()
            .iter()
            .map(|c| json!({ "id": c.id, "name": c.name, "color": c.color, "server": c.remote.as_ref().map(|r| r.href.clone()) }))
            .collect(),
    ))
}

fn event_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let id = match u64_param(p, "id")? {
        Some(id) => EventId(id),
        None => s.event.ok_or_else(|| Error::invalid("params", "no event is selected; pass id"))?,
    };
    if let Some(day) = str_param(p, "occurrence")? {
        // `day` is as the calendar shows it; EXDATE names the day in the event's own zone.
        let day = Date::parse(day)?;
        let zone = s.zone().clone();
        let e = s.store.event_mut(id)?;
        if e.rrule.is_none() {
            return Err(Error::invalid("occurrence", "the event does not repeat; delete it without occurrence"));
        }
        // The occurrence starting on `day`, or the one running through it.
        let Some((own, _)) = e.occurrences_in(day, day, &zone).into_iter().next() else {
            return Err(Error::invalid("occurrence", format!("the event has no occurrence on {day}")));
        };
        if e.exdates.len() >= 1_000 {
            return Err(Error::invalid("occurrence", "too many deleted occurrences; delete the series"));
        }
        let start = own.start_date();
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

// ---- reminders ----

/// The reminders due now, as the reminder window lists them.
fn reminder_list(s: &mut Session, _: &Value) -> Result<Value> {
    let now = s.now();
    Ok(Value::Array(
        s.store
            .reminders_due(now, s.zone())
            .into_iter()
            .map(|d| {
                let mut v = event_json(d.event, d.when);
                if let Some(o) = v.as_object_mut() {
                    o.insert("minutesUntil".into(), json!((d.start_utc.to_unix() - now.to_unix()).div_euclid(60)));
                    o.insert("startUtc".into(), json!(format!("{}Z", d.start_utc)));
                }
                v
            })
            .collect(),
    ))
}

/// The due reminders `p` names: `id` and `start` (as `reminder.list` gives it), only `id`
/// (every due one of that event), or nothing (all of them).
fn chosen_reminders(s: &Session, p: &Value) -> Result<Vec<(String, DateTime)>> {
    let id = u64_param(p, "id")?.map(EventId);
    let start = str_param(p, "start")?
        .map(|t| if t.contains('T') { DateTime::parse(t).map(|(d, _)| d) } else { Date::parse(t).map(DateTime::midnight) })
        .transpose()?;
    let picked: Vec<(String, DateTime)> = s
        .store
        .reminders_due(s.now(), s.zone())
        .into_iter()
        .filter(|d| id.is_none_or(|id| d.event.id == id))
        .filter(|d| {
            start.is_none_or(|t| match d.when {
                When::Timed { start, .. } => start == t,
                When::AllDay { start, .. } => DateTime::midnight(start) == t,
            })
        })
        .map(|d| (d.event.uid.clone(), d.start_utc))
        .collect();
    if picked.is_empty() && (id.is_some() || start.is_some()) {
        return Err(Error::invalid("reminder", "no such reminder is due"));
    }
    Ok(picked)
}

fn reminder_dismiss(s: &mut Session, p: &Value) -> Result<Value> {
    let now = s.now();
    let picked = chosen_reminders(s, p)?;
    for (uid, start) in &picked {
        s.store.mark_reminder(uid, *start, None, now);
    }
    Ok(json!({ "dismissed": picked.len() }))
}

/// Longest snooze: a week.
const MAX_SNOOZE_MINUTES: u64 = 7 * 24 * 60;

fn reminder_snooze(s: &mut Session, p: &Value) -> Result<Value> {
    let minutes = u64_param(p, "minutes")?.unwrap_or(5);
    if minutes == 0 || minutes > MAX_SNOOZE_MINUTES {
        return Err(Error::invalid("minutes", format!("snooze for 1 to {MAX_SNOOZE_MINUTES} minutes")));
    }
    let now = s.now();
    // Provably small: at most a week of seconds.
    let until = DateTime::from_unix(now.to_unix().saturating_add(i64::try_from(minutes * 60).unwrap_or(0)))?;
    let picked = chosen_reminders(s, p)?;
    for (uid, start) in &picked {
        s.store.mark_reminder(uid, *start, Some(until), now);
    }
    Ok(json!({ "snoozed": picked.len(), "until": format!("{until}Z") }))
}

fn event_set_reminder(s: &mut Session, p: &Value) -> Result<Value> {
    let id = match u64_param(p, "id")? {
        Some(id) => EventId(id),
        None => s.event.ok_or_else(|| Error::invalid("params", "no event is selected; pass id"))?,
    };
    let reminders = reminder_param(p)?.ok_or_else(|| Error::invalid("params", "reminder is required (minutes, or \"none\")"))?;
    let e = s.store.event_mut(id)?;
    e.reminders = reminders;
    let e = s.store.event(id)?;
    Ok(event_json(e, e.when.in_zone(e.zone().as_ref(), s.zone())))
}

// ---- people ----

const CONTACT_FIELDS: &str = "name: string\nemail?: string — one address or several, comma-separated\nphone?: string — the main phone\nphoneKind?: \"mobile\" | \"work\" | \"home\" | \"fax\" | \"\" — what the main phone is\nphones?: [{kind, number}] — every phone, instead of phone\norganization?: string\ntitle?: string — job title\nnote?: string\nbirthday?: YYYY-MM-DD";

fn contact_json(c: &Contact) -> Value {
    json!({
        "id": c.id, "uid": c.uid, "name": c.name, "emails": c.emails.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "phones": c.phones.iter().map(|p| json!({ "kind": p.kind, "number": p.number })).collect::<Vec<_>>(),
        "organization": c.organization, "title": c.title, "note": c.note, "birthday": c.birthday.map(|b| b.to_string()),
        "kept": c.kept.len(),
    })
}

/// A short text field: trimmed, at most 1024 bytes, `None` when empty.
fn short(v: &str) -> Option<String> {
    Some(ibercorreo_core::clamp_text(v.trim(), 1024).to_string()).filter(|v| !v.is_empty())
}

fn phone_param(p: &Value) -> Result<Option<Vec<Phone>>> {
    let one = |kind: &str, number: &str| -> Result<Phone> {
        let kind = kind.trim().to_ascii_lowercase();
        if !ibercorreo_core::contact::PHONE_KINDS.contains(&kind.as_str()) {
            return Err(Error::invalid("phoneKind", format!("{kind:?}: use mobile, work, home, fax or \"\"")));
        }
        if number.contains(['\r', '\n']) {
            return Err(Error::invalid("phone", "one line"));
        }
        Ok(Phone { kind, number: ibercorreo_core::clamp_text(number.trim(), 256).to_string() })
    };
    match p.get("phones") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            let mut out = Vec::new();
            for item in list.iter().take(50) {
                let number = item.get("number").and_then(Value::as_str).ok_or_else(|| Error::invalid("phones", "each needs a number"))?;
                let kind = item.get("kind").and_then(Value::as_str).unwrap_or("");
                if !number.trim().is_empty() {
                    out.push(one(kind, number)?);
                }
            }
            return Ok(Some(out));
        }
        Some(_) => return Err(Error::invalid("phones", "a list of {kind, number}")),
    }
    match str_param(p, "phone")? {
        None => Ok(None),
        Some(n) if n.trim().is_empty() => Ok(Some(Vec::new())),
        Some(n) => Ok(Some(vec![one(str_param(p, "phoneKind")?.unwrap_or(""), n)?])),
    }
}

/// Apply the fields present in `p` to `c` (all parsed before anything changes).
fn apply_contact_fields(c: &mut Contact, p: &Value) -> Result<()> {
    let name = str_param(p, "name")?.map(str::trim);
    if name.is_some_and(str::is_empty) {
        return Err(Error::invalid("name", "a contact needs a name"));
    }
    let emails = str_param(p, "email")?.map(Address::parse_list).transpose()?;
    let phones = phone_param(p)?;
    let organization = str_param(p, "organization")?;
    let title = str_param(p, "title")?;
    let note = str_param(p, "note")?;
    let birthday = match str_param(p, "birthday")?.map(str::trim) {
        None => None,
        Some("") => Some(None),
        Some(b) => Some(Some(Date::parse(b)?)),
    };
    if let Some(name) = name {
        let name = ibercorreo_core::clamp_text(name, 1024).to_string();
        if name != c.name {
            // The written-out parts of the old name (vCard N) no longer apply.
            c.kept.retain(|l| ibercorreo_vcard::property_name(l).as_deref() != Some("N"));
        }
        c.name = name;
    }
    if let Some(mut emails) = emails {
        emails.truncate(50);
        c.emails = emails;
    }
    if let Some(phones) = phones {
        c.phones = phones;
    }
    if let Some(o) = organization {
        c.organization = short(o);
    }
    if let Some(t) = title {
        c.title = short(t);
    }
    if let Some(n) = note {
        c.note = ibercorreo_core::clamp_text(n.trim(), ibercorreo_core::MAX_FIELD_LEN).to_string();
    }
    if let Some(b) = birthday {
        c.birthday = b;
    }
    Ok(())
}

fn contact_add(s: &mut Session, p: &Value) -> Result<Value> {
    let name = required_str(p, "name")?;
    if s.store.contact_count() >= ibercorreo_store::Store::MAX_CONTACTS {
        return Err(Error::invalid("contact", "the address book is full"));
    }
    let mut c = Contact::new(name);
    apply_contact_fields(&mut c, p)?;
    c.uid = {
        use std::hash::{Hash, Hasher};
        let unix = s.now().to_unix();
        let seq = s.store.contact_count();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (unix, seq, &c.name).hash(&mut h);
        format!("{unix}-{seq}-{:016x}@ibercorreo", h.finish())
    };
    let id = s.store.add_contact(c);
    s.status = "Contact added".into();
    Ok(contact_json(s.store.contact(id)?))
}

fn contact_id(p: &Value) -> Result<u64> {
    u64_param(p, "id")?.ok_or_else(|| Error::invalid("params", "id is required"))
}

fn contact_update(s: &mut Session, p: &Value) -> Result<Value> {
    let id = contact_id(p)?;
    apply_contact_fields(s.store.contact_mut(id)?, p)?;
    Ok(contact_json(s.store.contact(id)?))
}

fn contact_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let c = s.store.remove_contact(contact_id(p)?)?;
    s.status = format!("Deleted {}", c.name);
    Ok(json!({ "id": c.id, "deleted": true }))
}

fn contact_get(s: &mut Session, p: &Value) -> Result<Value> {
    Ok(contact_json(s.store.contact(contact_id(p)?)?))
}

fn contact_list(s: &mut Session, p: &Value) -> Result<Value> {
    let q = str_param(p, "query")?.unwrap_or("");
    Ok(Value::Array(s.store.contacts(q).into_iter().map(contact_json).collect()))
}

fn contact_import(s: &mut Session, p: &Value) -> Result<Value> {
    let r = s.store.import_vcf(required_str(p, "text")?)?;
    s.status = format!("Imported {} new and {} updated contacts", r.added, r.updated);
    serde_json::to_value(&r).map_err(|e| Error::invalid("report", e.to_string()))
}

fn contact_export(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = match u64_param(p, "id")? {
        Some(id) => vec![s.store.contact(id)?.id],
        None => Vec::new(),
    };
    Ok(Value::String(s.store.export_vcf(&ids)))
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
    let old_caldav = s.store.accounts.first().map(|a| a.caldav.clone()).unwrap_or_default();
    let caldav = match str_param(p, "caldav")?.map(str::trim) {
        None => old_caldav.clone(),
        Some("") => String::new(),
        Some(url) => {
            ibercorreo_dav::Url::parse(url).map_err(|e| Error::invalid("caldav", e.to_string()))?;
            url.to_string()
        }
    };
    let kind = AccountKind::Imap {
        imap_host: host_param(p, "imapHost")?,
        imap_port: port_param(p, "imapPort", imap_security.default_imap_port())?,
        smtp_host: host_param(p, "smtpHost")?,
        smtp_port: port_param(p, "smtpPort", smtp_security.default_smtp_port())?,
        user: user.to_string(),
        imap_security,
        smtp_security,
    };
    let server_changed = !old_caldav.is_empty() && old_caldav != caldav;
    let account = Account { name: address.display().to_string(), address, kind, caldav };
    // One account for now (several arrive in M3): the first is replaced.
    match s.store.accounts.first_mut() {
        Some(a) => *a = account,
        None => s.store.accounts.push(account),
    }
    // Another server (or none): the calendars mirrored from the old one go.
    if server_changed {
        ibercorreo_sync::calendar::forget(&mut s.store);
        s.fix_selection();
    }
    s.status = "Account saved".into();
    account_get(s, &Value::Null)
}

fn account_get(s: &mut Session, _: &Value) -> Result<Value> {
    let a = s.store.accounts.first().ok_or_else(|| Error::not_found("account", "none set up"))?;
    let mut v = json!({ "name": a.name, "email": a.address.email, "displayName": a.address.name, "caldav": a.caldav });
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
