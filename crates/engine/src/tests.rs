use ibercorreo_core::Message;
use serde_json::{Value, json};

use super::*;

fn today() -> Date {
    Date::new(2026, 10, 22).unwrap()
}

fn session() -> Session {
    Session::new(sample::store(today()).unwrap(), today())
}

#[test]
fn every_command_survives_hostile_params() {
    let junk: Vec<Value> = vec![
        Value::Null,
        json!({}),
        json!([]),
        json!("x"),
        json!(42),
        json!({"id": -1, "folder": 9_999_999_999u64, "date": "9999-12-31", "start": "", "query": "", "name": "", "path": ""}),
        json!({"id": "abc", "folder": {}, "to": "not an address", "text": "BEGIN:VCALENDAR", "raw": "\u{0}\u{ffff}", "summary": 5}),
        json!({"id": u64::MAX, "folder": "trash", "start": "2026-10-22T23:30", "end": "2026-10-22T23:00", "summary": "x"}),
        json!({"date": "today", "start": "9999-12-31", "summary": "edge", "email": "a@b, c@d", "name": " ", "zone": "\u{0}/../x"}),
        json!({"zone": "Pacific/Kiritimati", "start": "9999-12-31T23:30", "summary": "end of time", "repeat": "daily", "from": "view"}),
    ];
    for spec in commands::REGISTRY {
        for p in &junk {
            let mut s = session();
            // Only "path" could touch the disk; keep it off.
            if spec.id == "file.save" {
                continue;
            }
            if let Err(e) = s.run(spec.id, p) {
                assert!(!e.to_string().contains("failed unexpectedly"), "{} panicked on {p}", spec.id);
            }
        }
    }
}

#[test]
fn registry_ids_are_unique_and_documented() {
    let mut ids = std::collections::HashSet::new();
    for c in commands::REGISTRY {
        assert!(ids.insert(c.id), "duplicate {}", c.id);
        assert!(!c.label.is_empty());
        assert!(c.group.contains('/') || c.group == "Quick Access", "{}", c.id);
    }
}

#[test]
fn compose_send_and_undo() {
    let mut s = session();
    let id = s.run("message.compose", &json!({"to": "Ana <ana@example.org>", "subject": "Olá", "body": "Teste"})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(AccountId(0), FolderRole::Drafts).unwrap());
    s.run("message.send", &json!({"id": id})).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(AccountId(0), FolderRole::Outbox).unwrap());
    assert!(s.run("message.send", &json!({"id": id})).is_err(), "only drafts send");
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(AccountId(0), FolderRole::Drafts).unwrap());
    s.run("edit.redo", &Value::Null).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(AccountId(0), FolderRole::Outbox).unwrap());
    let empty = s.run("message.compose", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("message.send", &json!({"id": empty})).is_err(), "no recipients");
}

#[test]
fn a_failed_command_changes_nothing() {
    let mut s = session();
    let before = s.store.clone();
    assert!(s.run("message.move", &json!({"id": 0, "folder": 424242})).is_err());
    assert_eq!(s.store, before);
    assert!(!s.can_undo());
}

#[test]
fn delete_moves_to_trash_then_deletes() {
    let mut s = session();
    let first = s.store.messages_in(s.folder).first().unwrap().id;
    s.run("message.select", &json!({"id": first.0})).unwrap();
    s.run("message.delete", &Value::Null).unwrap();
    let trash = s.store.folder_by_role(AccountId(0), FolderRole::Trash).unwrap();
    assert_eq!(s.store.message(first).unwrap().folder, trash);
    assert_eq!(s.message, None);
    s.run("message.delete", &json!({"id": first.0})).unwrap();
    assert!(s.store.message(first).is_err());
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(s.store.message(first).is_ok());
}

#[test]
fn reply_all_skips_me_and_prefixes_once() {
    let mut s = session();
    let mut m = Message::new(MessageId(0), s.folder);
    m.from = Some(ibercorreo_core::Address::parse("Ana <ana@example.org>").unwrap());
    m.to = ibercorreo_core::Address::parse_list("denys@example.com, bob@example.org").unwrap();
    m.set_subject("RE: plano");
    let id = s.store.add_message(m).unwrap();
    let r = s.run("message.replyAll", &json!({"id": id.0})).unwrap()["id"].as_u64().unwrap();
    let draft = s.store.message(MessageId(r)).unwrap();
    assert_eq!(draft.subject, "RE: plano");
    assert_eq!(draft.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["ana@example.org"]);
    assert_eq!(draft.cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["bob@example.org"]);
}

#[test]
fn calendar_commands() {
    let mut s = session();
    let n = s.run("event.list", &json!({"date": "2026-10-22"})).unwrap().as_array().unwrap().len();
    assert_eq!(n, 4, "three appointments and the weekday stand-up");
    s.run("event.add", &json!({"summary": "Late call", "start": "2026-10-22T23:30"})).unwrap();
    let late = s.run("event.list", &json!({"date": "2026-10-23"})).unwrap();
    assert!(late.as_array().unwrap().iter().any(|e| e["summary"] == "Late call"), "an hour from 23:30 runs into the next day");
    s.run("calendar.goto", &json!({"date": "2026-01-31"})).unwrap();
    s.run("calendar.nextMonth", &Value::Null).unwrap();
    assert_eq!(s.day, Date::new(2026, 2, 28).unwrap());
    let ics = s.run("calendar.exportIcs", &Value::Null).unwrap();
    let mut fresh = Session::new(ibercorreo_store::Store::new(), today());
    let report = fresh.run("calendar.importIcs", &json!({"text": ics})).unwrap();
    assert_eq!(report["added"], json!(7), "five sample appointments, the stand-up and the late call");
}

#[test]
fn day_week_and_month_views_step_and_list_their_range() {
    let mut s = session();
    let state = s.run("calendar.viewWeek", &Value::Null).unwrap();
    assert_eq!(s.view, View::Calendar);
    // 2026-10-22 is a Thursday; weeks start on Monday.
    assert_eq!(state, json!({"date": "2026-10-22", "mode": "week", "from": "2026-10-19", "to": "2026-10-25"}));
    let week = s.run("event.list", &json!({"from": "view"})).unwrap();
    let names: Vec<_> = week.as_array().unwrap().iter().map(|e| e["summary"].as_str().unwrap().to_string()).collect();
    assert_eq!(names.iter().filter(|n| *n == "Stand-up").count(), 5, "one stand-up each weekday: {names:?}");
    assert_eq!(names.len(), 9, "five stand-ups, three on Thursday, the training on Saturday: {names:?}");
    assert!(!names.iter().any(|n| n == "Відпустка"), "the holiday starts next week");
    assert_eq!(s.run("calendar.next", &Value::Null).unwrap()["from"], "2026-10-26");
    let next = s.run("event.list", &json!({"from": "view"})).unwrap();
    let holiday = next.as_array().unwrap().iter().filter(|e| e["summary"] == "Відпустка").count();
    assert_eq!(holiday, 1, "a three-day holiday is listed once");

    assert_eq!(s.run("calendar.viewWorkWeek", &Value::Null).unwrap()["to"], "2026-10-30");
    s.run("calendar.prev", &Value::Null).unwrap();
    assert_eq!(s.day, today());
    let day = s.run("calendar.viewDay", &Value::Null).unwrap();
    assert_eq!((day["from"].as_str(), day["to"].as_str()), (Some("2026-10-22"), Some("2026-10-22")));
    s.run("calendar.next", &Value::Null).unwrap();
    assert_eq!(s.day, Date::new(2026, 10, 23).unwrap());
    s.run("calendar.viewMonth", &Value::Null).unwrap();
    s.run("calendar.goto", &json!({"date": "2028-01-31"})).unwrap();
    assert_eq!(s.run("calendar.next", &Value::Null).unwrap()["to"], "2028-02-29", "a leap February");

    assert!(s.run("event.list", &json!({"from": "2026-10-22"})).is_err(), "from without to");
    assert!(s.run("event.list", &json!({"from": "2026-10-22", "to": "2026-10-21"})).is_err());
    assert!(s.run("event.list", &json!({"from": "2026-01-01", "to": "2027-01-02"})).is_err(), "367 days");
    assert!(s.run("event.list", &json!({"from": "2026-01-01", "to": "2026-12-31"})).is_ok());
    let edge = Session::run(&mut s, "calendar.goto", &json!({"date": "9999-12-31"}));
    assert!(edge.is_ok());
    assert!(s.run("calendar.next", &Value::Null).is_err(), "no month after 9999-12");
    s.run("calendar.viewWeek", &Value::Null).unwrap();
    assert!(s.run("event.list", &json!({"from": "view"})).is_ok(), "a week clipped at the end of time still lists");
}

#[test]
fn interface_language_is_chosen_normalised_and_follows_the_computer_by_default() {
    let mut s = session();
    assert_eq!(s.run("app.language", &Value::Null).unwrap()["language"], "auto");
    assert_eq!(s.run("app.language", &json!({"language": "uk_UA.UTF-8"})).unwrap()["language"], "uk");
    assert_eq!(s.store.language.as_deref(), Some("uk"));
    for bad in [json!({"language": "klingon"}), json!({"language": "u1"}), json!({"language": 7})] {
        assert!(s.run("app.language", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.store.language.as_deref(), Some("uk"), "a refused language changes nothing");
    assert_eq!(s.run("app.language", &json!({"language": "auto"})).unwrap()["followsSystem"], true);
    assert_eq!(s.store.language, None);
}

#[test]
fn time_zones_choose_convert_and_follow_the_computer() {
    let mut s = session();
    // Late evening UTC on the 22nd: already the 23rd in Kyiv.
    s.set_now(DateTime::new(today(), 22, 30, 0).unwrap());
    s.set_system_zone(Zone::named("Europe/Kyiv").unwrap());
    assert_eq!(s.today(), Date::new(2026, 10, 23).unwrap());
    assert_eq!(s.run("calendar.goto", &json!({"date": "today"})).unwrap()["date"], "2026-10-23");
    let tz = s.run("calendar.timeZone", &Value::Null).unwrap();
    assert_eq!(tz, json!({"zone": "Europe/Kyiv", "offset": "UTC+03:00", "system": "Europe/Kyiv", "followsSystem": true}));
    // An appointment typed in Kyiv is kept in Kyiv time; in Lisbon it is two hours earlier, so
    // 01:00 there is 23:00 the evening before.
    let v = s.run("event.add", &json!({"summary": "Call", "start": "2026-10-28T01:00", "repeat": "daily", "until": "2026-10-30"})).unwrap();
    assert_eq!((v["start"].as_str(), v["timeZone"].as_str()), (Some("2026-10-28T01:00"), Some("Europe/Kyiv")));
    let id = v["id"].as_u64().unwrap();
    let chosen = s.run("calendar.timeZone", &json!({"zone": "GMT Standard Time"})).unwrap();
    assert_eq!((chosen["zone"].as_str(), chosen["followsSystem"].as_bool()), (Some("Europe/London"), Some(false)));
    assert_eq!(s.zone().name(), "Europe/London");
    s.run("calendar.timeZone", &json!({"zone": "Europe/Lisbon"})).unwrap();
    let call = |s: &mut Session, d: &str| {
        s.run("event.list", &json!({"date": d})).unwrap().as_array().unwrap().iter().find(|e| e["summary"] == "Call").map(|e| e["start"].clone())
    };
    assert_eq!(call(&mut s, "2026-10-27"), Some(json!("2026-10-27T23:00")));
    assert_eq!(call(&mut s, "2026-10-30"), None, "the last one, the 30th in Kyiv, is the 29th here");
    // Deleting the occurrence the Lisbon calendar shows on the 28th removes the 29th in Kyiv.
    let deleted = s.run("event.delete", &json!({"id": id, "occurrence": "2026-10-28"})).unwrap();
    assert_eq!(deleted["deleted"], "2026-10-29");
    assert_eq!(call(&mut s, "2026-10-28"), None);
    assert_eq!(call(&mut s, "2026-10-27"), Some(json!("2026-10-27T23:00")));
    assert_eq!(call(&mut s, "2026-10-29"), Some(json!("2026-10-29T23:00")));
    // The choice is the store's, so undo takes it back; "system" follows the computer again.
    s.run("edit.undo", &Value::Null).unwrap();
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.zone().name(), "Europe/London");
    s.run("calendar.timeZone", &json!({"zone": "system"})).unwrap();
    assert_eq!(s.zone().name(), "Europe/Kyiv");
    for bad in ["Mars/Base", "", "Customized Time Zone"] {
        assert!(s.run("calendar.timeZone", &json!({"zone": bad})).is_err(), "{bad:?}");
    }
    assert_eq!(s.zone().name(), "Europe/Kyiv", "a refused zone changes nothing");
    // Mail dates are listed in UTC, marked.
    let first = s.run("message.list", &Value::Null).unwrap()[0]["date"].as_str().unwrap().to_string();
    assert!(first.ends_with('Z'), "{first}");
}

#[test]
fn reminders_fire_snooze_dismiss_and_stay_dismissed() {
    let mut s = session();
    let at = |h: u8, m: u8| DateTime::new(today(), h, m, 0).unwrap();
    let due = |s: &mut Session| -> Vec<(String, i64)> {
        let v = s.run("reminder.list", &Value::Null).unwrap();
        v.as_array().unwrap().iter().map(|r| (r["summary"].as_str().unwrap().to_string(), r["minutesUntil"].as_i64().unwrap())).collect()
    };
    s.set_now(at(9, 44));
    assert!(due(&mut s).is_empty(), "the 10:00 meeting's reminder fires at 09:45");
    s.set_now(at(9, 50));
    assert_eq!(due(&mut s), [("Reunión de presupuesto".to_string(), 10)]);
    assert_eq!(s.run("reminder.snooze", &json!({"minutes": 5})).unwrap()["snoozed"], 1);
    assert!(due(&mut s).is_empty(), "snoozed");
    s.set_now(at(9, 56));
    assert_eq!(due(&mut s).len(), 1, "back after five minutes");
    let id = s.run("reminder.list", &Value::Null).unwrap()[0]["id"].as_u64().unwrap();
    s.run("reminder.dismiss", &json!({"id": id, "start": "2026-10-22T10:00"})).unwrap();
    s.set_now(at(10, 20));
    assert_eq!(due(&mut s), [("Chamada com a Lusodata".to_string(), 10)], "dismissed stays dismissed; the next one fired");
    // Dismissals are saved with the store.
    let reloaded: ibercorreo_store::Store = serde_json::from_str(&serde_json::to_string(&s.store).unwrap()).unwrap();
    let mut again = Session::new(reloaded, today());
    again.set_now(at(10, 20));
    assert_eq!(due(&mut again).len(), 1);
    s.run("reminder.dismiss", &Value::Null).unwrap();
    assert!(due(&mut s).is_empty());
    // A missed reminder waits until dismissed, for two days.
    s.set_now(DateTime::new(Date::new(2026, 10, 23).unwrap(), 8, 0, 0).unwrap());
    assert_eq!(due(&mut s).iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["Almoço com Rui"]);
    s.set_now(DateTime::new(Date::new(2026, 10, 25).unwrap(), 8, 0, 0).unwrap());
    assert!(due(&mut s).iter().all(|d| d.0 != "Almoço com Rui"), "older than two days is let go");

    // event.add: 15 minutes by default, none on request, none for all-day; event.setReminder.
    let a = s.run("event.add", &json!({"summary": "x", "start": "2026-11-02T10:00"})).unwrap();
    assert_eq!(a["reminders"], json!([15]));
    let b = s.run("event.add", &json!({"summary": "y", "start": "2026-11-02T11:00", "reminder": "none"})).unwrap();
    assert_eq!(b["reminders"], json!([]));
    assert_eq!(s.run("event.add", &json!({"summary": "z", "start": "2026-11-02"})).unwrap()["reminders"], json!([]));
    let c = s.run("event.setReminder", &json!({"id": b["id"], "reminder": 99_999_999})).unwrap();
    assert_eq!(c["reminders"], json!([ibercorreo_core::calendar::MAX_REMINDER_MINUTES]), "clamped to four weeks");
    for bad in [json!({"reminder": "soon"}), json!({"reminder": [1]}), json!({"reminder": 1.5})] {
        assert!(s.run("event.setReminder", &bad).is_err(), "{bad}");
    }
    assert!(s.run("reminder.snooze", &json!({"minutes": 0})).is_err());
    assert!(s.run("reminder.snooze", &json!({"minutes": 99_999_999})).is_err());
    assert!(s.run("reminder.dismiss", &json!({"id": 424_242})).is_err(), "not due");
}

#[test]
fn outbox_renders_and_sending_results_move_mail() {
    use ibercorreo_sync::outbox::{Outcome, Sent};
    let mut s = session();
    let mut queue = |to: &str, subject: &str| {
        let id = s.run("message.compose", &json!({"to": to, "subject": subject, "body": "Hi"})).unwrap()["id"].as_u64().unwrap();
        s.run("message.send", &json!({"id": id})).unwrap();
        MessageId(id)
    };
    let (ok, saved, later, bad) =
        (queue("ana@example.org", "a"), queue("bob@example.org", "b"), queue("c@example.org", "c"), queue("d@example.org", "d"));
    let out = s.outbox(AccountId(0));
    assert_eq!(out.len(), 4);
    let first = out.iter().find(|o| o.id == ok).unwrap();
    assert_eq!(first.to, ["ana@example.org"]);
    assert_eq!(first.from, "denys@example.com");
    let text = String::from_utf8(first.raw.clone()).unwrap();
    assert!(text.contains("Subject: a\r\n") && text.contains("Message-ID: <"), "{text}");
    assert_eq!(s.outbox(AccountId(0)).iter().find(|o| o.id == ok).unwrap().raw, first.raw, "the same bytes every time");

    s.apply_sent(&[
        Sent { id: ok, outcome: Outcome::Sent { saved: false } },
        Sent { id: saved, outcome: Outcome::Sent { saved: true } },
        Sent { id: later, outcome: Outcome::Later("451 try later".into()) },
        Sent { id: bad, outcome: Outcome::Failed("550 no such user".into()) },
    ]);
    let folder = |id| s.store.message(id).map(|m| m.folder);
    assert_eq!(folder(ok), s.store.folder_by_role(AccountId(0), FolderRole::Sent));
    assert!(s.store.message(ok).unwrap().seen);
    assert!(s.store.message(saved).is_err(), "the server's Sent copy comes back with the next sync");
    assert_eq!(folder(later), s.store.folder_by_role(AccountId(0), FolderRole::Outbox));
    assert_eq!(folder(bad), s.store.folder_by_role(AccountId(0), FolderRole::Drafts));
    assert!(!s.can_undo(), "undo must never put sent mail back in the Outbox");
    assert!(s.status.contains("Sent 2") && s.status.contains("550"), "{}", s.status);
}

#[test]
fn account_settings_and_sync_request() {
    let mut s = session();
    assert!(s.run("account.sync", &Value::Null).is_err(), "the sample account is local only");
    assert!(!s.sync_requested);
    let v = s
        .run("account.set", &json!({"email": "denys@example.org", "name": "Denys", "imapHost": "IMAP.Example.org.", "smtpHost": "smtp.example.org", "smtpSecurity": "starttls"}))
        .unwrap();
    assert_eq!(v["imapHost"], "imap.example.org");
    assert_eq!((v["imapPort"].as_u64(), v["smtpPort"].as_u64()), (Some(993), Some(587)));
    assert_eq!(v["user"], "denys@example.org");
    assert_eq!(s.store.accounts.len(), 2, "the open folder's account is changed, not another added");
    s.run("account.sync", &Value::Null).unwrap();
    assert!(s.sync_requested);
    for bad in [
        json!({"email": "x", "imapHost": "a", "smtpHost": "b"}),
        json!({"email": "a@b.c", "imapHost": "bad host", "smtpHost": "b"}),
        json!({"email": "a@b.c", "imapHost": "a\r\nb", "smtpHost": "b"}),
        json!({"email": "a@b.c", "imapHost": "a", "smtpHost": "b", "imapPort": 0}),
        json!({"email": "a@b.c", "imapHost": "a", "smtpHost": "b", "imapPort": 70000}),
        json!({"email": "a@b.c", "imapHost": "a", "smtpHost": "b", "imapSecurity": "none"}),
        json!({"email": "a@b.c", "imapHost": "a", "smtpHost": "b", "user": "a\u{0}b"}),
        json!({"email": "a@b.c", "imapHost": "a", "smtpHost": "b", "oauth": "yahoo"}),
        json!({"email": "a@b.c", "imapHost": "a", "smtpHost": "b", "oauth": 3}),
    ] {
        assert!(s.run("account.set", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.run("account.get", &Value::Null).unwrap()["imapHost"], "imap.example.org");
    assert_eq!(s.run("account.get", &Value::Null).unwrap()["oauth"], "", "a password by default");
    let gmail = json!({"email": "ana@gmail.com", "imapHost": "imap.gmail.com", "smtpHost": "smtp.gmail.com", "oauth": "google"});
    assert_eq!(s.run("account.set", &gmail).unwrap()["oauth"], "google");
    assert_eq!(s.store.accounts.first().and_then(|a| a.oauth()).map(|p| p.name), Some("Google"));
}

#[test]
fn caldav_address_is_checked_kept_and_forgotten() {
    let mut s = session();
    let base = json!({"email": "denys@example.org", "imapHost": "imap.example.org", "smtpHost": "smtp.example.org"});
    let with = |extra: Value| {
        let mut p = base.clone();
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p
    };
    for bad in ["http://cal.example.org/", "cal.example.org", "https://ana:pw@cal.example.org/", "https://cal example.org/"] {
        assert!(s.run("account.set", &with(json!({ "caldav": bad }))).is_err(), "{bad}");
    }
    let v = s.run("account.set", &with(json!({"caldav": " https://cal.example.org/dav/ "}))).unwrap();
    assert_eq!(v["caldav"], "https://cal.example.org/dav/");
    assert_eq!(s.run("account.set", &base).unwrap()["caldav"], "https://cal.example.org/dav/", "omitted keeps it");

    // A synced calendar, then the address removed: the mirrored calendar goes.
    let batch = ibercorreo_sync::calendar::Batch {
        calendars: vec![ibercorreo_sync::calendar::CalendarSync {
            calendar: ibercorreo_dav::caldav::RemoteCalendar { href: "/dav/work/".into(), name: "Work".into(), color: None, ctag: Some("1".into()) },
            unchanged: false,
            fetched: vec![ibercorreo_dav::caldav::FetchedItem {
                href: "/dav/work/a.ics".into(),
                etag: "e".into(),
                data: "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261102T100000Z\r\nDTEND:20261102T110000Z\r\nSUMMARY:Planning\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".into(),
            }],
            removed: Vec::new(),
        }],
        errors: Vec::new(),
        pushed: Vec::new(),
    };
    let calendars = s.store.calendars().len();
    let r = s.apply_calendar_sync(batch);
    assert_eq!((r.calendars_added, r.added), (1, 1));
    assert!(s.status.contains("calendars: 1 new"), "{}", s.status);
    assert!(!s.can_undo());
    assert_eq!(s.calendar_sync_state().calendars.len(), 1);
    s.run("account.set", &with(json!({"caldav": ""}))).unwrap();
    assert_eq!(s.store.calendars().len(), calendars);
    assert!(s.store.events().iter().all(|e| e.uid != "a"));
}

#[test]
fn html_mail_reads_safely_and_can_show_as_plain_text() {
    let mut s = session();
    let list = s.run("message.list", &Value::Null).unwrap();
    let id = list.as_array().unwrap().iter().find(|m| m["html"] == true).unwrap()["id"].as_u64().unwrap();
    let v = s.run("message.read", &json!({ "id": id })).unwrap();
    assert_eq!(v["rich"]["remoteImages"], 2);
    let blocks = v["rich"]["blocks"].as_array().unwrap();
    assert!(blocks.iter().any(|b| b["kind"] == json!({"bullet": {"depth": 1}})), "{blocks:?}");
    assert!(!v["body"].as_str().unwrap().contains("menu de outono&"), "plain text is decoded");
    assert!(!v["body"].as_str().unwrap().contains("Esta semana: novo"), "the hidden preheader is not in the text");
    assert!(!s.plain_text);
    assert_eq!(s.run("view.plainText", &Value::Null).unwrap()["plainText"], true);
    assert_eq!(s.run("view.plainText", &json!({ "on": false })).unwrap()["plainText"], false);
    assert!(s.run("view.plainText", &json!({ "on": "yes" })).is_err());
}

#[test]
fn repeating_appointments_and_deleting_one_occurrence() {
    let mut s = session();
    let v = s.run("event.add", &json!({"summary": "Gym", "start": "2026-10-20T18:00", "repeat": "weekly", "until": "2026-11-10"})).unwrap();
    assert_eq!(v["repeat"], "FREQ=WEEKLY;UNTIL=20261110");
    let id = v["id"].as_u64().unwrap();
    let gym_on = |s: &mut Session, d: &str| {
        s.run("event.list", &json!({ "date": d })).unwrap().as_array().unwrap().iter().filter(|e| e["summary"] == "Gym").cloned().collect::<Vec<_>>()
    };
    let on_27 = gym_on(&mut s, "2026-10-27");
    assert_eq!(on_27.len(), 1);
    assert_eq!(on_27[0]["start"], "2026-10-27T18:00", "the occurrence's own date, not the series start");
    assert!(gym_on(&mut s, "2026-11-17").is_empty(), "until is the last day");
    s.run("event.delete", &json!({ "id": id, "occurrence": "2026-10-27" })).unwrap();
    assert!(gym_on(&mut s, "2026-10-27").is_empty());
    assert_eq!(gym_on(&mut s, "2026-11-03").len(), 1, "the rest of the series stays");
    assert!(s.run("event.delete", &json!({ "id": id, "occurrence": "2026-10-28" })).is_err(), "no occurrence that day");
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(gym_on(&mut s, "2026-10-27").len(), 1, "undo brings the occurrence back");
    for bad in [
        json!({"repeat": "hourly"}),
        json!({"rrule": "FREQ=HOURLY"}),
        json!({"rrule": "FREQ=DAILY\r\nX:1"}),
        json!({"repeat": "daily", "until": "2026-01-01"}),
    ] {
        let mut p = json!({"summary": "x", "start": "2026-10-20T18:00"});
        p.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        assert!(s.run("event.add", &p).is_err(), "{p}");
    }
    // The sample's weekday stand-up shows on a weekday, not at the weekend.
    let list = |s: &mut Session, d: &str| s.run("event.list", &json!({ "date": d })).unwrap();
    assert!(list(&mut s, "2026-10-23").as_array().unwrap().iter().any(|e| e["summary"] == "Stand-up"));
    assert!(!list(&mut s, "2026-10-24").as_array().unwrap().iter().any(|e| e["summary"] == "Stand-up"));
}

/// The sample invitation's message id.
fn invitation(s: &Session) -> MessageId {
    s.store.all_messages().iter().find(|m| m.invite.is_some()).map(|m| m.id).unwrap()
}

#[test]
fn invitations_are_accepted_answered_updated_and_cancelled() {
    let mut s = session();
    let id = invitation(&s);
    let info = s.invite_info(id).unwrap();
    assert_eq!(
        (info.method.as_str(), info.summary.as_str(), info.status.as_deref(), info.in_calendar),
        ("REQUEST", "Revisión del presupuesto", Some("NEEDS-ACTION"), false)
    );
    assert_eq!(info.organizer.as_deref(), Some("Marta Iglesias <marta@example.es>"));
    let read = s.run("message.read", &json!({"id": id.0})).unwrap();
    assert_eq!(read["invite"]["start"], "2026-10-23T14:00", "16:00 in Madrid, shown in UTC: {read}");

    // Accept: the meeting goes in the calendar and the answer waits in the Outbox.
    let out = s.run("invite.respond", &json!({"id": id.0, "response": "accept", "comment": "Allí estaré"})).unwrap();
    assert_eq!(out["queued"], true);
    let event = EventId(out["events"][0].as_u64().unwrap());
    let e = s.store.event(event).unwrap().clone();
    assert_eq!((e.uid.as_str(), e.tzid.as_deref(), e.reminders.as_slice()), ("sample-budget-review@example.es", Some("Europe/Madrid"), &[15][..]));
    assert!(s.invite_info(id).unwrap().in_calendar);
    let answer = MessageId(out["reply"].as_u64().unwrap());
    let m = s.store.message(answer).unwrap();
    assert_eq!(m.folder, s.store.folder_by_role(AccountId(0), FolderRole::Outbox).unwrap());
    assert_eq!(m.subject, "Accepted: Revisión del presupuesto");
    assert_eq!(m.to[0].email, "marta@example.es");
    let raw = commands::render(&s, answer).unwrap();
    assert!(raw.contains("method=REPLY"), "{raw}");
    let back = ibercorreo_mime::parse(raw.as_bytes(), MessageId(99), FolderId(0)).unwrap();
    let reply = ibercorreo_ical::invite(back.invite.as_deref().unwrap()).unwrap();
    assert_eq!(reply.method, "REPLY");
    assert_eq!((reply.attendees[0].email.as_str(), reply.attendees[0].status.as_deref()), ("denys@example.com", Some("ACCEPTED")));
    assert_eq!(s.outbox(AccountId(0)).len(), 1);

    // An update from the organizer moves the meeting; answering again changes it in place.
    let moved = s
        .store
        .message(id)
        .unwrap()
        .invite
        .clone()
        .unwrap()
        .replace("T160000", "T180000")
        .replace("T170000", "T190000")
        .replace("SEQUENCE:0", "SEQUENCE:1");
    s.store.message_mut(id).unwrap().invite = Some(moved);
    s.store.event_mut(event).unwrap().reminders = vec![30];
    let out = s.run("invite.respond", &json!({"id": id.0, "response": "tentative", "reply": "none"})).unwrap();
    assert_eq!(out["events"][0].as_u64(), Some(event.0), "the same event");
    assert_eq!(out["reply"], Value::Null);
    let e = s.store.event(event).unwrap();
    assert_eq!(e.when.minutes_on(e.when.start_date()), Some((18 * 60, 19 * 60)));
    assert_eq!(e.reminders, [30], "your own reminder stays");
    assert_eq!(s.store.events().iter().filter(|e| e.uid == "sample-budget-review@example.es").count(), 1);

    // Cancelled: it can be removed, not accepted.
    let cancel = s.store.message(id).unwrap().invite.clone().unwrap().replace("METHOD:REQUEST", "METHOD:CANCEL");
    s.store.message_mut(id).unwrap().invite = Some(cancel);
    assert!(s.run("invite.respond", &json!({"id": id.0, "response": "accept"})).is_err());
    assert_eq!(s.run("invite.remove", &json!({"id": id.0})).unwrap()["removed"], 1);
    assert!(s.store.event(event).is_err());
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(s.store.event(event).is_ok(), "undo puts it back");
}

#[test]
fn declining_removes_the_meeting_and_can_be_edited_first() {
    let mut s = session();
    let id = invitation(&s);
    s.run("invite.respond", &json!({"id": id.0, "response": "accept", "reply": "none"})).unwrap();
    assert_eq!(s.outbox(AccountId(0)).len(), 0, "no answer asked for");
    let out = s.run("invite.respond", &json!({"id": id.0, "response": "decline", "reply": "edit"})).unwrap();
    assert_eq!(out["removed"], 1);
    assert!(!s.invite_info(id).unwrap().in_calendar);
    let draft = MessageId(out["reply"].as_u64().unwrap());
    assert_eq!(s.message, Some(draft), "the answer opens to edit");
    assert_eq!(s.store.message(draft).unwrap().folder, s.store.folder_by_role(AccountId(0), FolderRole::Drafts).unwrap());
    s.run("message.update", &json!({"body": "Lo siento, estoy de viaje."})).unwrap();
    s.run("message.send", &Value::Null).unwrap();
    let sent = &s.outbox(AccountId(0))[0];
    let text = String::from_utf8_lossy(&sent.raw).to_string();
    let back = ibercorreo_mime::parse(text.as_bytes(), MessageId(99), FolderId(0)).unwrap();
    assert_eq!(back.body.trim(), "Lo siento, estoy de viaje.");
    assert!(back.invite.unwrap().contains("PARTSTAT=DECLINED"));
    for bad in [json!({"id": id.0}), json!({"id": id.0, "response": "maybe later"}), json!({"id": id.0, "response": "accept", "reply": "fax"})] {
        assert!(s.run("invite.respond", &bad).is_err(), "{bad}");
    }
    let plain = s.store.all_messages().iter().find(|m| m.invite.is_none()).unwrap().id;
    assert!(s.run("invite.respond", &json!({"id": plain.0, "response": "accept"})).is_err());
}

#[test]
fn declining_one_occurrence_cancels_only_that_day() {
    let mut s = session();
    let id = invitation(&s);
    let series = s.store.message(id).unwrap().invite.clone().unwrap().replace("SUMMARY:", "RRULE:FREQ=DAILY;COUNT=5\r\nSUMMARY:");
    s.store.message_mut(id).unwrap().invite = Some(series.clone());
    let event = EventId(s.run("invite.respond", &json!({"id": id.0, "response": "accept", "reply": "none"})).unwrap()["events"][0].as_u64().unwrap());
    // The organizer cancels the third day only.
    let one = series
        .replace("METHOD:REQUEST", "METHOD:CANCEL")
        .replace("RRULE:FREQ=DAILY;COUNT=5\r\n", "RECURRENCE-ID;TZID=Europe/Madrid:20261025T160000\r\n");
    s.store.message_mut(id).unwrap().invite = Some(one);
    s.run("invite.remove", &json!({"id": id.0})).unwrap();
    let e = s.store.event(event).unwrap();
    assert_eq!(e.exdates, [Date::new(2026, 10, 25).unwrap()]);
}

#[test]
fn contacts_are_added_edited_deleted_and_travel_as_vcards() {
    let mut s = session();
    let c = s
        .run(
            "contact.add",
            &json!({"name": "Taras Shevchuk", "email": "taras@example.com.ua, t.shevchuk@example.org", "phone": "+380 44 000 0000", "phoneKind": "work", "title": "Інженер", "birthday": "1990-03-09"}),
        )
        .unwrap();
    let id = c["id"].as_u64().unwrap();
    assert!(c["uid"].as_str().unwrap().ends_with("@ibercorreo"));
    assert_eq!(c["phones"][0]["kind"], "work");
    assert_eq!(c["emails"].as_array().unwrap().len(), 2);
    // A field left out stays; "" clears.
    let u = s.run("contact.update", &json!({"id": id, "organization": "Lviv Soft", "title": "", "phones": [{"kind": "mobile", "number": "+380 67 000 0000"}, {"number": "+380 44 111 1111"}]})).unwrap();
    assert_eq!((u["organization"].as_str(), u["title"].is_null(), u["birthday"].as_str()), (Some("Lviv Soft"), true, Some("1990-03-09")));
    assert_eq!(u["phones"].as_array().unwrap().len(), 2);
    for bad in [
        json!({"id": id, "name": " "}),
        json!({"id": id, "phoneKind": "pager", "phone": "1"}),
        json!({"id": id, "birthday": "soon"}),
        json!({"id": 999, "name": "x"}),
        json!({"id": id, "email": "broken"}),
    ] {
        assert!(s.run("contact.update", &bad).is_err(), "{bad}");
    }
    // Out as a vCard, and back in: the same person, updated rather than copied.
    let card = s.run("contact.exportVcf", &json!({"id": id})).unwrap().as_str().unwrap().to_string();
    assert!(card.contains("FN:Taras Shevchuk") && card.contains("TEL;TYPE=CELL:+380 67 000 0000"), "{card}");
    let edited = card.replace("ORG:Lviv Soft", "ORG:Kyiv Soft");
    let r = s.run("contact.importVcf", &json!({"text": edited})).unwrap();
    assert_eq!((r["added"].as_u64(), r["updated"].as_u64()), (Some(0), Some(1)));
    assert_eq!(s.run("contact.get", &json!({"id": id})).unwrap()["organization"], "Kyiv Soft");
    // Renaming drops the old written-out name parts, so the card says the new name.
    s.run("contact.importVcf", &json!({"text": card.replace("N:Shevchuk;Taras;;;", "N:Shevchuk;Taras;Hryhorovych;;")})).unwrap();
    assert!(s.store.contact(id).unwrap().kept.iter().any(|l| l.starts_with("N:")));
    s.run("contact.update", &json!({"id": id, "name": "Taras H. Shevchuk"})).unwrap();
    let card = s.run("contact.exportVcf", &json!({"id": id})).unwrap().as_str().unwrap().to_string();
    assert!(card.contains("N:Shevchuk;Taras H.;;;"), "{card}");
    s.run("contact.delete", &json!({"id": id})).unwrap();
    assert!(s.run("contact.get", &json!({"id": id})).is_err());
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.run("contact.get", &json!({"id": id})).unwrap()["name"], "Taras H. Shevchuk");
    assert_eq!(s.run("contact.list", &json!({"query": "067"})).unwrap().as_array().unwrap().len(), 0);
    assert_eq!(s.run("contact.list", &json!({"query": "67 000"})).unwrap().as_array().unwrap().len(), 1, "phones are searched");
}

#[test]
fn server_address_books_sync_and_contacts_move_between_books() {
    let mut s = session();
    s.run("account.set", &json!({"email": "denys@example.org", "imapHost": "imap.example.org", "smtpHost": "smtp.example.org", "caldav": "https://dav.example.org/dav/"})).unwrap();
    let batch = ibercorreo_sync::contacts::Batch {
        books: vec![ibercorreo_sync::contacts::BookSync {
            book: ibercorreo_dav::carddav::RemoteBook { href: "/dav/books/phone/".into(), name: "Phone".into(), ctag: Some("1".into()) },
            unchanged: false,
            fetched: vec![ibercorreo_dav::caldav::FetchedItem {
                href: "/dav/books/phone/ana.vcf".into(),
                etag: "e".into(),
                data: "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:ana\r\nFN:Ana Souza\r\nTEL;TYPE=CELL:+351 912 345 678\r\nEND:VCARD\r\n".into(),
            }],
            removed: Vec::new(),
        }],
        errors: Vec::new(),
        pushed: Vec::new(),
    };
    let r = s.apply_contacts_sync(batch);
    assert_eq!((r.books_added, r.added), (1, 1));
    assert!(s.status.contains("contacts: 1 new"), "{}", s.status);
    assert!(!s.can_undo());
    let books = s.run("contact.listBooks", &Value::Null).unwrap();
    let phone = books.as_array().unwrap().iter().find(|b| b["server"] == "/dav/books/phone/").unwrap().clone();
    assert_eq!((phone["name"].as_str(), phone["contacts"].as_u64()), (Some("Phone"), Some(1)));
    let phone = phone["id"].as_u64().unwrap();

    // Added to the server book: it is sent at the next sync.
    let olena = s.run("contact.add", &json!({"name": "Olena Koval", "book": phone})).unwrap();
    assert_eq!((olena["book"].as_u64(), olena["synced"].as_bool()), (Some(phone), Some(false)));
    assert!(s.run("contact.add", &json!({"name": "Nowhere", "book": 99})).is_err());
    assert!(s.run("contact.add", &json!({"name": "Nowhere", "book": u64::MAX})).is_err());
    let state = s.contacts_sync_state();
    assert_eq!(state.changes.len(), 1, "{state:?}");
    assert_eq!(s.run("contact.list", &json!({"book": phone})).unwrap().as_array().unwrap().len(), 2);

    // Ana moved to the local book: deleted on the server, kept here.
    let ana = s.store.all_contacts().iter().find(|c| c.uid == "ana").unwrap().id;
    let local = s.store.books().first().unwrap().id;
    let moved = s.run("contact.update", &json!({"id": ana, "book": local, "title": "Diretora"})).unwrap();
    assert_eq!((moved["book"].as_u64(), moved["synced"].as_bool(), moved["title"].as_str()), (Some(u64::from(local)), Some(false), Some("Diretora")));
    assert_eq!(s.store.card_deletions().len(), 1);
    // A bad field changes nothing, not even the book (a failed command is undone whole).
    assert!(s.run("contact.update", &json!({"id": ana, "book": phone, "birthday": "soon"})).is_err());
    assert_eq!(s.store.contact(ana).unwrap().book, local);

    // Another server: its address books go, the contacts here stay.
    s.run("account.set", &json!({"email": "denys@example.org", "imapHost": "imap.example.org", "smtpHost": "smtp.example.org", "caldav": ""}))
        .unwrap();
    assert!(s.store.books().iter().all(|b| b.remote.is_none()));
    assert!(s.store.contact(ana).is_ok());
}

#[test]
fn blind_copies_are_sent_to_but_never_written() {
    let mut s = session();
    let id = s
        .run(
            "message.compose",
            &json!({"to": "ana@example.org", "bcc": "boss@example.net, Rui <rui@example.pt>", "subject": "hola", "signature": false}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.run("message.send", &json!({"id": id})).unwrap();
    let out = s.outbox(s.current_account());
    let o = out.iter().find(|o| o.id == MessageId(id)).unwrap();
    assert_eq!(o.to, ["ana@example.org", "boss@example.net", "rui@example.pt"], "the envelope has them all");
    let raw = String::from_utf8(o.raw.clone()).unwrap();
    assert!(!raw.contains("boss@") && !raw.contains("rui@") && !raw.to_ascii_lowercase().contains("bcc"), "{raw}");
    let read = s.run("message.read", &json!({"id": id})).unwrap();
    assert_eq!(read["bcc"].as_array().map(Vec::len), Some(2), "kept here, as Outlook shows them in Sent");

    // Bcc alone is enough to send; a draft's Bcc can be changed.
    let only = s.run("message.compose", &json!({"bcc": "boss@example.net"})).unwrap()["id"].as_u64().unwrap();
    s.run("message.update", &json!({"id": only, "bcc": "rui@example.pt"})).unwrap();
    assert_eq!(s.store.message(MessageId(only)).unwrap().bcc.first().map(|a| a.email.as_str()), Some("rui@example.pt"));
    assert!(s.run("message.send", &json!({"id": only})).is_ok());
    assert!(s.run("message.compose", &json!({"bcc": "not an address"})).is_err());
}

#[test]
fn each_account_signs_its_own_mail_above_the_quote() {
    let mut s = session();
    let work = s.store.first_account();
    let mut p = s.run("account.get", &json!({"id": work.0})).unwrap();
    p["imapHost"] = json!("imap.example.com");
    p["smtpHost"] = json!("smtp.example.com");
    p["signature"] = json!("Denys\r\nTinta Office\n\n");
    s.run("account.set", &p).unwrap();
    assert_eq!(s.store.account(work).unwrap().signature, "Denys\nTinta Office", "line ends kept as \\n, trailing blank lines dropped");
    let body = |s: &Session, v: Value| s.store.message(MessageId(v["id"].as_u64().unwrap())).unwrap().body.clone();

    let v = s.run("message.compose", &json!({"to": "ana@example.org"})).unwrap();
    assert_eq!(body(&s, v), "\n\nDenys\nTinta Office");
    let v = s.run("message.compose", &json!({"to": "ana@example.org", "body": "Hola", "signature": false})).unwrap();
    assert_eq!(body(&s, v), "Hola", "a script can leave it out");

    let inbox = s.store.folder_by_role(work, FolderRole::Inbox).unwrap();
    let first = s.store.messages_in(inbox).first().map(|m| m.id).unwrap();
    let v = s.run("message.reply", &json!({"id": first.0})).unwrap();
    let reply = body(&s, v);
    let at = |needle: &str| reply.find(needle).unwrap();
    assert!(at("Tinta Office") < at("wrote:"), "the signature sits above the quote: {reply}");

    // The other account has none; changing other settings keeps a signature.
    let personal = s.store.accounts.get(1).unwrap().id;
    let home = s.store.folder_by_role(personal, FolderRole::Inbox).unwrap();
    let theirs = s.store.messages_in(home).first().map(|m| m.id).unwrap();
    let v = s.run("message.reply", &json!({"id": theirs.0})).unwrap();
    assert!(!body(&s, v).contains("Tinta Office"));
    let mut p = s.run("account.get", &json!({"id": work.0})).unwrap();
    p.as_object_mut().unwrap().remove("signature");
    p["name"] = json!("Denys M.");
    s.run("account.set", &p).unwrap();
    assert_eq!(s.store.account(work).unwrap().signature, "Denys\nTinta Office");
    p["signature"] = json!("x".repeat(9000));
    assert!(s.run("account.set", &p).is_err());
}

#[test]
fn several_accounts_each_send_from_their_own_folders() {
    let mut s = session();
    let ana = json!({"email": "ana@example.org", "name": "Ana", "imapHost": "imap.example.org", "smtpHost": "smtp.example.org"});
    let v = s.run("account.add", &ana).unwrap();
    let a = AccountId(u32::try_from(v["id"].as_u64().unwrap()).unwrap());
    assert_eq!(s.current_account(), a, "the new account's Inbox opens");
    assert!(s.run("account.add", &ana).is_err(), "the same mailbox twice");
    let list = s.run("account.list", &Value::Null).unwrap();
    assert_eq!(list.as_array().map(Vec::len), Some(3));
    assert_eq!(list.as_array().unwrap().iter().filter(|x| x["current"] == true).count(), 1);
    // A message written here comes from this account and leaves through its Outbox.
    let id = s.run("message.compose", &json!({"to": "bo@example.net", "subject": "hola"})).unwrap()["id"].as_u64().unwrap();
    let m = s.store.message(MessageId(id)).unwrap();
    assert_eq!(m.from.as_ref().map(|f| f.email.as_str()), Some("ana@example.org"));
    assert_eq!(m.folder, s.store.folder_by_role(a, FolderRole::Drafts).unwrap());
    s.run("message.send", &json!({"id": id})).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(a, FolderRole::Outbox).unwrap());
    assert!(s.outbox(AccountId(0)).iter().all(|o| o.id != MessageId(id)), "not in the other account's Outbox");
    assert_eq!(s.outbox(a).len(), 1);
    s.apply_sent(&[ibercorreo_sync::outbox::Sent { id: MessageId(id), outcome: ibercorreo_sync::outbox::Outcome::Sent { saved: false } }]);
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(a, FolderRole::Sent).unwrap());
    // Composing for another account by id; folder names reach the open account, or the one named.
    let other = s.run("message.compose", &json!({"account": 0, "subject": "work"})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.store.message(MessageId(other)).unwrap().from.as_ref().map(|f| f.email.as_str()), Some("denys@example.com"));
    s.run("folder.select", &json!({"folder": "inbox", "account": a.0})).unwrap();
    assert_eq!(s.folder, s.store.folder_by_role(a, FolderRole::Inbox).unwrap());
    assert!(s.run("message.compose", &json!({"account": 77})).is_err());
    // Delete goes to the message's own Trash; moving across accounts is refused.
    s.run("message.delete", &json!({"id": other})).unwrap();
    assert_eq!(s.store.message(MessageId(other)).unwrap().folder, s.store.folder_by_role(AccountId(0), FolderRole::Trash).unwrap());
    let theirs = s.store.folder_by_role(a, FolderRole::Archive).unwrap();
    assert!(s.run("message.move", &json!({"id": other, "folder": theirs.0})).is_err());
    // Removing an account takes its folders; undo brings it back.
    s.run("account.remove", &json!({"id": a.0})).unwrap();
    assert!(s.store.account(a).is_err() && s.store.message(MessageId(id)).is_err());
    assert_eq!(s.current_account(), AccountId(0), "the selection moves to a folder that is left");
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(s.store.account(a).is_ok() && s.store.message(MessageId(id)).is_ok());
}

#[test]
fn folders_go_in_and_out_of_favorites_with_undo() {
    let mut s = session();
    let work = s.store.first_account();
    let (inbox, sent) = (s.store.folder_by_role(work, FolderRole::Inbox).unwrap(), s.store.folder_by_role(work, FolderRole::Sent).unwrap());
    assert_eq!(s.store.favorites(), [inbox, sent], "the sample starts as Outlook does");
    let personal = s.store.accounts.get(1).unwrap().id;
    let theirs = s.store.folder_by_role(personal, FolderRole::Inbox).unwrap();
    s.run("folder.favorite", &json!({ "folder": theirs.0 })).unwrap();
    s.run("folder.favorite", &json!({ "folder": "sent", "favorite": false })).unwrap();
    assert_eq!(s.store.favorites(), [inbox, theirs]);
    let list = s.run("folder.list", &Value::Null).unwrap();
    assert!(list.as_array().unwrap().iter().any(|f| f["id"] == theirs.0 && f["favorite"] == true));
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.store.favorites(), [inbox, sent, theirs], "undo brings Sent back");
    assert!(s.run("folder.favorite", &json!({ "folder": 9999 })).is_err());
    assert!(s.run("folder.favorite", &json!({ "favorite": "yes" })).is_err());
}

#[test]
fn another_mailbox_for_an_account_forgets_the_old_mail() {
    let mut s = session();
    let base = json!({"email": "ana@example.org", "imapHost": "imap.example.org", "smtpHost": "smtp.example.org"});
    let a = AccountId(u32::try_from(s.run("account.add", &base).unwrap()["id"].as_u64().unwrap()).unwrap());
    let inbox = s.store.folder_by_role(a, FolderRole::Inbox).unwrap();
    s.store.folder_mut(inbox).unwrap().remote = Some(ibercorreo_core::RemoteFolder { path: "INBOX".into(), uid_validity: 1 });
    let mut m = Message::new(MessageId(0), inbox);
    m.remote = Some(ibercorreo_core::RemoteRef { folder: inbox, uid: 1, seen: false, flagged: false });
    let synced = s.store.add_message(m).unwrap();
    // A new name for the same mailbox keeps the mail.
    s.run(
        "account.set",
        &json!({"id": a.0, "email": "ana@example.org", "name": "Ana G.", "imapHost": "imap.example.org", "smtpHost": "smtp.example.org"}),
    )
    .unwrap();
    assert!(s.store.message(synced).is_ok());
    // Another server (as when Outlook replaces Gmail) does not mix the two.
    s.run("account.set", &json!({"id": a.0, "email": "ana@outlook.com", "imapHost": "outlook.office365.com", "smtpHost": "smtp.office365.com"}))
        .unwrap();
    assert!(s.store.message(synced).is_err());
    assert_eq!(s.store.folder(inbox).unwrap().remote, None);
}

#[test]
fn only_one_account_syncs_calendars() {
    let mut s = session();
    let dav =
        json!({"email": "ana@example.org", "imapHost": "imap.example.org", "smtpHost": "smtp.example.org", "caldav": "https://dav.example.org/"});
    s.run("account.add", &dav).unwrap();
    let mut second = dav.clone();
    second["email"] = json!("bo@example.net");
    assert!(s.run("account.add", &second).is_err(), "a second CalDAV account");
    second["caldav"] = json!("");
    s.run("account.add", &second).unwrap();
}
