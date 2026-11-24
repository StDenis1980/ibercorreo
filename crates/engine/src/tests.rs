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
    assert!(!s.sync_requested);
    s.run("message.send", &json!({"id": id})).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(AccountId(0), FolderRole::Outbox).unwrap());
    assert!(s.sync_requested, "a message sent goes out with a Send/Receive started now");
    assert!(s.run("message.send", &json!({"id": id})).is_err(), "only drafts send");
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(AccountId(0), FolderRole::Drafts).unwrap());
    s.run("edit.redo", &Value::Null).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(AccountId(0), FolderRole::Outbox).unwrap());
    let empty = s.run("message.compose", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("message.send", &json!({"id": empty})).is_err(), "no recipients");
}

#[test]
fn a_draft_is_sent_only_while_it_says_what_its_hash_says() {
    let mut s = session();
    let id =
        s.run("message.compose", &json!({"to": "ana@example.org", "subject": "Cita", "body": "El lunes a las 10."})).unwrap()["id"].as_u64().unwrap();
    let hash = s.run("message.read", &json!({"id": id})).unwrap()["contentHash"].as_str().unwrap().to_string();
    assert_eq!(hash, commands::content_hash(&s, MessageId(id)).unwrap(), "the same each time");
    let drafts = s.store.folder_by_role(AccountId(0), FolderRole::Drafts).unwrap();
    // Every part a person approves changes it.
    for change in [
        json!({"to": "otra@example.org"}),
        json!({"cc": "x@example.org"}),
        json!({"bcc": "y@example.org"}),
        json!({"subject": "Cita!"}),
        json!({"body": "El martes."}),
    ] {
        let mut t = s.clone();
        let mut p = change.clone();
        p["id"] = json!(id);
        t.run("message.update", &p).unwrap();
        let e = t.run("message.send", &json!({"id": id, "contentHash": hash})).unwrap_err();
        assert!(e.to_string().contains("changed"), "{change}: {e}");
        assert_eq!(t.store.message(MessageId(id)).unwrap().folder, drafts, "{change}: still a draft");
    }
    let mut t = s.clone();
    t.store.message_mut(MessageId(id)).unwrap().attachments.push(ibercorreo_core::mail::Attachment {
        name: "x.pdf".into(),
        mime_type: "application/pdf".into(),
        size: 1,
        sha256: None,
        content_id: None,
    });
    assert!(t.run("message.send", &json!({"id": id, "contentHash": hash})).is_err(), "an attachment added");
    assert!(s.run("message.send", &json!({"id": id, "contentHash": 7})).is_err());
    s.run("message.send", &json!({"id": id, "contentHash": hash.to_uppercase()})).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(AccountId(0), FolderRole::Outbox).unwrap());
}

#[test]
fn replies_take_a_text_and_lists_take_a_page() {
    let mut s = session();
    let all = s.run("message.list", &json!({"folder": "inbox"})).unwrap();
    let all = all.as_array().unwrap();
    let page = s.run("message.list", &json!({"folder": "inbox", "offset": 1, "limit": 2})).unwrap();
    assert_eq!(page.as_array().unwrap().as_slice(), all.get(1..3).unwrap());
    assert_eq!(s.run("message.list", &json!({"folder": "inbox", "offset": u64::MAX})).unwrap(), json!([]));
    assert_eq!(s.run("search.messages", &json!({"query": "a", "limit": 1})).unwrap().as_array().unwrap().len(), 1);
    let first = all.first().unwrap()["id"].as_u64().unwrap();
    for cmd in ["message.reply", "message.replyAll", "message.forward"] {
        let d = s.run(cmd, &json!({"id": first, "body": "Gracias."})).unwrap()["id"].as_u64().unwrap();
        let body = s.run("message.read", &json!({"id": d})).unwrap()["body"].as_str().unwrap().to_string();
        assert!(body.starts_with("Gracias.") && body.contains("> "), "{cmd}: {body}");
    }
}

#[test]
fn the_send_receive_period_is_kept_with_the_mailbox() {
    let mut s = session();
    assert_eq!(s.run("account.syncInterval", &Value::Null).unwrap(), json!({"minutes": 5, "default": true}));
    assert_eq!(s.run("account.syncInterval", &json!({"minutes": 15})).unwrap()["minutes"], 15);
    assert_eq!(s.store.sync_minutes(), 15);
    assert_eq!(s.run("account.syncInterval", &json!({"minutes": "0"})).unwrap()["minutes"], 0);
    assert!(s.status.contains("by hand"), "{}", s.status);
    for bad in [json!(-1), json!(1441), json!(u64::MAX), json!("often"), json!(true)] {
        assert!(s.run("account.syncInterval", &json!({"minutes": bad})).is_err(), "{bad}");
        assert_eq!(s.store.sync_minutes(), 0, "unchanged after {bad}");
    }
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.store.sync_minutes(), 15);
    assert_eq!(s.run("account.syncInterval", &json!({"minutes": "default"})).unwrap(), json!({"minutes": 5, "default": true}));
}

#[test]
fn a_mailto_link_opens_a_draft_and_never_sends() {
    let mut s = session();
    let id = s
        .run("message.composeMailto", &json!({"uri": "mailto:ana@example.org?cc=luis@example.org&subject=Cita&body=Hola%0D%0A&attach=/etc/passwd"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let m = s.store.message(MessageId(id)).unwrap();
    assert_eq!(m.folder, s.store.folder_by_role(AccountId(0), FolderRole::Drafts).unwrap());
    assert_eq!(m.to.len(), 1);
    assert_eq!(m.cc.len(), 1);
    assert!(s.store.content(MessageId(id)).unwrap().body.starts_with("Hola\n"));
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(s.store.message(MessageId(id)).is_err());
    assert!(s.run("message.composeMailto", &json!({"uri": "https://example.org"})).is_err());
    assert!(s.run("message.composeMailto", &json!({})).is_err());
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
fn theme_and_mode_are_saved_not_undone_and_refuse_unknown_values() {
    let mut s = session();
    let out = s.run("view.theme", &Value::Null).unwrap();
    assert_eq!((out["theme"].as_str(), out["mode"].as_str()), (Some("teal"), Some("system")));
    assert!(!s.dirty, "asking changes nothing");
    let out = s.run("view.theme", &json!({"theme": " Terracotta ", "mode": "dark"})).unwrap();
    assert_eq!((out["theme"].as_str(), out["mode"].as_str()), (Some("terracotta"), Some("dark")));
    assert_eq!((s.store.theme.as_deref(), s.store.appearance.as_deref()), (Some("terracotta"), Some("dark")));
    assert!(s.dirty, "saved with the store");
    assert!(!s.can_undo(), "Undo is for mail");
    for bad in [json!({"theme": "neon"}), json!({"theme": 3}), json!({"mode": "sepia"}), json!({"theme": "sea", "mode": "x"})] {
        assert!(s.run("view.theme", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.store.theme.as_deref(), Some("terracotta"), "a refused theme changes nothing");
    s.run("view.theme", &json!({"theme": "teal", "mode": "system"})).unwrap();
    assert_eq!((s.store.theme.as_ref(), s.store.appearance.as_ref()), (None, None), "the default is kept as none");
}

#[test]
fn folder_pane_and_my_day_turn_over_or_take_a_value() {
    let mut s = session();
    assert_eq!(s.run("view.folderPane", &Value::Null).unwrap()["visible"], false);
    assert!(s.store.hide_folder_pane && s.dirty);
    assert_eq!(s.run("view.folderPane", &Value::Null).unwrap()["visible"], true);
    assert_eq!(s.run("view.myDay", &json!({"visible": false})).unwrap()["visible"], false);
    assert_eq!(s.run("view.myDay", &json!({"visible": false})).unwrap()["visible"], false, "a value is not a toggle");
    assert!(s.store.hide_my_day && !s.store.hide_folder_pane);
    assert!(s.run("view.myDay", &json!({"visible": "yes"})).is_err());
    assert!(!s.can_undo());
}

#[test]
fn calendars_are_shown_and_hidden_saved_not_undone() {
    let mut s = session();
    let first = s.store.calendars().first().map(|c| c.id).unwrap();
    let second = s.store.add_calendar("Birthdays", "#c0392b", None).unwrap();
    let hidden = s.run("calendar.show", &json!({ "id": second })).unwrap();
    assert_eq!(hidden["hidden"], json!([second]), "a click hides a shown calendar");
    assert!(s.dirty && !s.can_undo());
    let listed = s.run("calendar.list", &Value::Null).unwrap();
    assert_eq!(listed.as_array().unwrap().iter().map(|c| c["shown"].clone()).collect::<Vec<_>>(), [json!(true), json!(false)]);
    s.run("calendar.show", &json!({ "id": second })).unwrap();
    assert!(s.store.hidden_calendars.is_empty(), "and shows it again");
    s.run("calendar.show", &json!({ "id": second, "visible": true })).unwrap();
    assert!(s.store.hidden_calendars.is_empty(), "visible: true keeps it shown");
    s.run("calendar.show", &json!({ "id": second, "only": true })).unwrap();
    assert_eq!(s.store.hidden_calendars, [first]);
    s.run("calendar.show", &json!({ "all": true })).unwrap();
    assert!(s.store.hidden_calendars.is_empty());
    for bad in [json!({}), json!({ "id": 999 }), json!({ "id": -1 }), json!({ "id": "x" })] {
        assert!(s.run("calendar.show", &bad).is_err(), "{bad}");
    }
}

#[test]
fn what_is_folded_in_the_folder_pane_is_saved_not_undone() {
    let mut s = session();
    let asked = s.run("view.folded", &Value::Null).unwrap();
    assert_eq!((asked["keys"].clone(), asked["kept"].clone()), (Value::Null, json!(false)));
    assert!(!s.dirty, "asking changes nothing");
    let set = s.run("view.folded", &json!({ "keys": ["t:0:clients", "h:4294967295", "t:0:clients"] })).unwrap();
    assert_eq!(set["keys"], json!(["h:4294967295", "t:0:clients"]), "sorted, once each");
    assert!(s.dirty && !s.can_undo());
    s.dirty = false;
    s.run("view.folded", &json!({ "keys": ["t:0:clients", "h:4294967295"] })).unwrap();
    assert!(!s.dirty, "the same again changes nothing");
    s.run("view.folded", &json!({ "keys": [] })).unwrap();
    assert_eq!(s.store.folded, Some(Vec::new()), "nothing folded is kept too: the pane no longer decides");
    let long = "x".repeat(ibercorreo_store::MAX_FOLDED_LEN + 1);
    let many: Vec<String> = (0..=ibercorreo_store::MAX_FOLDED).map(|n| n.to_string()).collect();
    for bad in [json!({"keys": "t:0:a"}), json!({"keys": [1]}), json!({"keys": [""]}), json!({"keys": [long]}), json!({ "keys": many })] {
        assert!(s.run("view.folded", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.store.folded, Some(Vec::new()), "a refused list changes nothing");
}

#[test]
fn interface_size_steps_by_ten_stays_in_range_and_is_saved_not_undone() {
    let mut s = session();
    assert_eq!(s.run("view.zoom", &Value::Null).unwrap()["percent"], 110);
    assert!(!s.dirty, "asking changes nothing");
    assert_eq!(s.run("view.zoom", &json!({"step": 1})).unwrap()["percent"], 120);
    assert_eq!(s.store.zoom, Some(120));
    assert!(s.dirty && !s.can_undo());
    assert_eq!(s.run("view.zoom", &json!({"percent": "115%"})).unwrap()["percent"], 115);
    assert_eq!(s.run("view.zoom", &json!({"step": -1})).unwrap()["percent"], 110, "115 rounds to 120, then down");
    assert_eq!(s.store.zoom, None, "the default is not stored");
    assert_eq!(s.run("view.zoom", &json!({"percent": 200, "step": 5})).unwrap()["percent"], 200, "never past the largest");
    assert_eq!(s.run("view.zoom", &json!({"percent": 80, "step": -1})).unwrap()["percent"], 80, "never below the smallest");
    for bad in [json!({"percent": 79}), json!({"percent": 201}), json!({"percent": "big"}), json!({"percent": f64::MAX}), json!({"step": "up"})] {
        assert!(s.run("view.zoom", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.store.zoom(), 80, "a refused size changes nothing");
    assert_eq!(s.run("view.zoom", &json!({"reset": true})).unwrap()["percent"], 110);
    s.store.zoom = Some(9999);
    assert_eq!(s.store.zoom(), 200, "a hand-edited size is kept in range");
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
        scope: None,
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
    s.store.all_messages().find(|m| m.has_invite()).map(|m| m.id).unwrap()
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
    let reply = ibercorreo_ical::invite(back.content().unwrap().invite.as_deref().unwrap()).unwrap();
    assert_eq!(reply.method, "REPLY");
    assert_eq!((reply.attendees[0].email.as_str(), reply.attendees[0].status.as_deref()), ("denys@example.com", Some("ACCEPTED")));
    assert_eq!(s.outbox(AccountId(0)).len(), 1);

    // An update from the organizer moves the meeting; answering again changes it in place.
    let moved = s
        .store
        .content(id)
        .unwrap()
        .invite
        .clone()
        .unwrap()
        .replace("T160000", "T180000")
        .replace("T170000", "T190000")
        .replace("SEQUENCE:0", "SEQUENCE:1");
    s.store.content_mut(id).unwrap().invite = Some(moved);
    s.store.event_mut(event).unwrap().reminders = vec![30];
    let out = s.run("invite.respond", &json!({"id": id.0, "response": "tentative", "reply": "none"})).unwrap();
    assert_eq!(out["events"][0].as_u64(), Some(event.0), "the same event");
    assert_eq!(out["reply"], Value::Null);
    let e = s.store.event(event).unwrap();
    assert_eq!(e.when.minutes_on(e.when.start_date()), Some((18 * 60, 19 * 60)));
    assert_eq!(e.reminders, [30], "your own reminder stays");
    assert_eq!(s.store.events().iter().filter(|e| e.uid == "sample-budget-review@example.es").count(), 1);

    // Cancelled: it can be removed, not accepted.
    let cancel = s.store.content(id).unwrap().invite.clone().unwrap().replace("METHOD:REQUEST", "METHOD:CANCEL");
    s.store.content_mut(id).unwrap().invite = Some(cancel);
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
    assert_eq!(back.content().unwrap().body.trim(), "Lo siento, estoy de viaje.");
    assert!(back.content().unwrap().invite.as_deref().unwrap().contains("PARTSTAT=DECLINED"));
    for bad in [json!({"id": id.0}), json!({"id": id.0, "response": "maybe later"}), json!({"id": id.0, "response": "accept", "reply": "fax"})] {
        assert!(s.run("invite.respond", &bad).is_err(), "{bad}");
    }
    let plain = s.store.all_messages().find(|m| !m.has_invite()).unwrap().id;
    assert!(s.run("invite.respond", &json!({"id": plain.0, "response": "accept"})).is_err());
}

#[test]
fn declining_one_occurrence_cancels_only_that_day() {
    let mut s = session();
    let id = invitation(&s);
    let series = s.store.content(id).unwrap().invite.clone().unwrap().replace("SUMMARY:", "RRULE:FREQ=DAILY;COUNT=5\r\nSUMMARY:");
    s.store.content_mut(id).unwrap().invite = Some(series.clone());
    let event = EventId(s.run("invite.respond", &json!({"id": id.0, "response": "accept", "reply": "none"})).unwrap()["events"][0].as_u64().unwrap());
    // The organizer cancels the third day only.
    let one = series
        .replace("METHOD:REQUEST", "METHOD:CANCEL")
        .replace("RRULE:FREQ=DAILY;COUNT=5\r\n", "RECURRENCE-ID;TZID=Europe/Madrid:20261025T160000\r\n");
    s.store.content_mut(id).unwrap().invite = Some(one);
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
        scope: None,
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
fn an_account_can_sign_in_through_gnome_settings() {
    let mut s = session();
    let work = s.store.first_account();
    let mut p = s.run("account.get", &json!({"id": work.0})).unwrap();
    p["imapHost"] = json!("imap.gmail.com");
    p["smtpHost"] = json!("smtp.gmail.com");
    p["goa"] = json!("account_1700000000_2");
    p["caldav"] = json!("https://cloud.example.org/remote.php/dav");
    p["davGoa"] = json!("account_1700000000_0");
    let v = s.run("account.set", &p).unwrap();
    assert_eq!((v["goa"].as_str(), v["davGoa"].as_str()), (Some("account_1700000000_2"), Some("account_1700000000_0")));
    let a = s.store.account(work).unwrap();
    assert_eq!(a.credential_entry(), None, "GNOME keeps the secret");
    // Left out, both are kept; the form that saves without them does not unlink the account.
    let mut p = s.run("account.get", &json!({"id": work.0})).unwrap();
    for k in ["goa", "davGoa"] {
        p.as_object_mut().unwrap().remove(k);
    }
    s.run("account.set", &p).unwrap();
    assert_eq!(s.store.account(work).unwrap().dav_goa, "account_1700000000_0");
    // No calendars server: nothing to sign in to through another account.
    p["caldav"] = json!("");
    s.run("account.set", &p).unwrap();
    assert_eq!(s.store.account(work).unwrap().dav_goa, "");
    assert_eq!(s.store.account(work).unwrap().goa, "account_1700000000_2");
    for bad in [json!({"goa": "../x"}), json!({"goa": "a b"}), json!({"goa": "a".repeat(200)}), json!({"goa": 7}), json!({"oauth": "google"})] {
        let mut q = p.clone();
        for (k, v) in bad.as_object().unwrap() {
            q[k] = v.clone();
        }
        assert!(s.run("account.set", &q).is_err(), "{bad}");
    }
    p["goa"] = json!("");
    s.run("account.set", &p).unwrap();
    assert_eq!(s.store.account(work).unwrap().goa, "");
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
    let body = |s: &Session, v: Value| s.store.content(MessageId(v["id"].as_u64().unwrap())).unwrap().body.clone();

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

/// Tinta: All Inboxes lists every account's Inbox together, newest first; opening one of its
/// messages keeps it open and makes that message's account the current one; any other folder
/// closes it.
#[test]
fn all_inboxes_list_every_account_together() {
    let mut s = session();
    let (work, home) = (s.store.first_account(), s.store.accounts.get(1).unwrap().id);
    let inbox = |s: &Session, a| s.store.folder_by_role(a, FolderRole::Inbox).unwrap();
    let (work_inbox, home_inbox) = (inbox(&s, work), inbox(&s, home));
    let v = s.run("folder.select", &json!({ "folder": "allInboxes" })).unwrap();
    assert_eq!(v["inboxes"], json!([work_inbox.0, home_inbox.0]));
    assert!(s.all_inboxes);
    let listed: Vec<Value> = s.run("message.list", &Value::Null).unwrap().as_array().unwrap().clone();
    let both = s.store.messages_in(work_inbox).len() + s.store.messages_in(home_inbox).len();
    assert_eq!(listed.len(), both);
    assert!(listed.iter().any(|m| m["folder"] == work_inbox.0) && listed.iter().any(|m| m["folder"] == home_inbox.0));
    let dates: Vec<_> = s.listed().iter().map(|m| m.date).collect();
    assert!(dates.windows(2).all(|w| w[0] >= w[1]), "newest first across accounts");

    // A message from the second account: still All Inboxes, and replies go from that account.
    let theirs = s.store.messages_in(home_inbox).first().unwrap().id;
    s.run("message.select", &json!({ "id": theirs.0 })).unwrap();
    assert!(s.all_inboxes);
    assert_eq!(s.current_account(), home);
    assert_eq!(s.message, Some(theirs));
    // Searching "this folder" looks in every Inbox, and nowhere else.
    s.search_scope = SearchScope::Folder;
    assert!(s.search_results("").iter().all(|m| m.folder == work_inbox || m.folder == home_inbox));
    assert_eq!(s.search_results("").len(), both);

    s.run("folder.select", &json!({ "folder": "sent" })).unwrap();
    assert!(!s.all_inboxes, "another folder closes it");
    s.run("folder.select", &json!({ "folder": "all inboxes" })).unwrap();
    s.run("folder.select", &json!({ "folder": work_inbox.0 })).unwrap();
    assert!(!s.all_inboxes, "one Inbox alone is that Inbox");
    assert!(s.run("folder.select", &json!({ "folder": "allInbox" })).is_err());
}

#[test]
fn mail_over_graph_needs_the_microsoft_sign_in_and_forgets_the_imap_copy() {
    let mut s = session();
    let ms = json!({"email": "ana@contoso.es", "imapHost": "outlook.office365.com", "smtpHost": "smtp.office365.com", "oauth": "microsoft"});
    let v = s.run("account.add", &ms).unwrap();
    assert_eq!(v["graphMail"], false, "IMAP by default");
    let a = AccountId(u32::try_from(v["id"].as_u64().unwrap()).unwrap());
    let with = |extra: Value| {
        let mut p = ms.clone();
        p.as_object_mut().unwrap().insert("id".into(), json!(a.0));
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p
    };
    for bad in [json!({"graphMail": "yes"}), json!({"graphMail": true, "oauth": "google"}), json!({"graphMail": true, "oauth": ""})] {
        assert!(s.run("account.set", &with(bad.clone())).is_err(), "{bad}");
    }
    let inbox = s.store.folder_by_role(a, FolderRole::Inbox).unwrap();
    s.store.folder_mut(inbox).unwrap().remote = Some(ibercorreo_core::RemoteFolder { path: "INBOX".into(), uid_validity: 1 });
    let mut m = Message::new(MessageId(0), inbox);
    m.remote = Some(ibercorreo_core::RemoteRef { folder: inbox, uid: 1, seen: false, flagged: false, id: None });
    let synced = s.store.add_message(m).unwrap();
    // Graph's folders and ids are not IMAP's: the IMAP copy goes, so the two never mix.
    assert_eq!(s.run("account.set", &with(json!({"graphMail": true}))).unwrap()["graphMail"], true);
    assert!(s.store.message(synced).is_err());
    assert_eq!(s.store.folder(inbox).unwrap().remote, None);
    // Left out, it stays as it is.
    assert_eq!(s.run("account.set", &with(json!({"name": "Ana Gil"}))).unwrap()["graphMail"], true);
    assert!(s.store.account(a).unwrap().mail_over_graph());
}

#[test]
fn mail_over_ews_takes_the_exchange_address_and_forgets_the_imap_copy() {
    let mut s = session();
    // Just the Exchange server: its usual EWS address, and it stands in for IMAP and SMTP.
    let v = s.run("account.add", &json!({"email": "ana@contoso.es", "ews": " Mail.Contoso.es ", "smtpHost": ""})).unwrap();
    assert_eq!(v["ews"], "https://mail.contoso.es/EWS/Exchange.asmx");
    assert_eq!((v["imapHost"].as_str(), v["smtpHost"].as_str()), (Some("mail.contoso.es"), Some("mail.contoso.es")));
    let a = AccountId(u32::try_from(v["id"].as_u64().unwrap()).unwrap());
    assert_eq!(s.store.account(a).unwrap().mail_over_ews(), Some("https://mail.contoso.es/EWS/Exchange.asmx"));
    let with = |extra: Value| {
        let mut p = json!({"id": a.0, "email": "ana@contoso.es"});
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p
    };
    let full = s.run("account.set", &with(json!({"ews": "https://mail.contoso.es:8443/ews/exchange.asmx"}))).unwrap();
    assert_eq!(full["ews"], "https://mail.contoso.es:8443/ews/exchange.asmx");
    for bad in [
        json!({"ews": "http://mail.contoso.es/EWS/Exchange.asmx"}),
        json!({"ews": "https://ana@mail.contoso.es/"}),
        json!({"ews": 7}),
        json!({"ews": "mail.contoso.es", "oauth": "microsoft"}),
        json!({"ews": "mail.contoso.es", "oauth": "microsoft", "graphMail": true}),
    ] {
        assert!(s.run("account.set", &with(bad.clone())).is_err(), "{bad}");
    }
    // Left out, it stays as it is.
    assert_eq!(s.run("account.set", &with(json!({"name": "Ana Gil"}))).unwrap()["ews"], "https://mail.contoso.es:8443/ews/exchange.asmx");

    // Exchange's folders and ids are not IMAP's: switching forgets the copy synced the other way.
    let inbox = s.store.folder_by_role(a, FolderRole::Inbox).unwrap();
    s.store.folder_mut(inbox).unwrap().remote = Some(ibercorreo_core::RemoteFolder { path: "AAMkIN".into(), uid_validity: 1 });
    let mut m = Message::new(MessageId(0), inbox);
    m.remote = Some(ibercorreo_core::RemoteRef { folder: inbox, uid: 1, seen: false, flagged: false, id: Some("AAMk1".into()) });
    let synced = s.store.add_message(m).unwrap();
    // A calendar mirrored from that Exchange server, and one from CalDAV.
    let prefix = ibercorreo_sync::ews_pim::prefix("https://mail.contoso.es:8443/ews/exchange.asmx");
    let exchange =
        s.store.add_calendar("Calendario", "#123456", Some(ibercorreo_core::CalendarRemote { href: format!("{prefix}CA/"), ctag: None })).unwrap();
    let dav = s.store.add_calendar("Casa", "#654321", Some(ibercorreo_core::CalendarRemote { href: "/dav/casa/".into(), ctag: None })).unwrap();
    let imap = s.run("account.set", &with(json!({"ews": "", "imapHost": "mail.contoso.es", "smtpHost": "mail.contoso.es"}))).unwrap();
    let left: Vec<u32> = s.store.calendars().iter().map(|c| c.id).collect();
    assert!(!left.contains(&exchange) && left.contains(&dav), "Exchange's calendars go with it: {left:?}");
    assert_eq!(imap["ews"], "");
    assert!(s.store.message(synced).is_err());
    assert_eq!(s.store.folder(inbox).unwrap().remote, None);
    // Without EWS the servers are needed again.
    assert!(s.run("account.add", &json!({"email": "luis@contoso.es"})).is_err());
}

#[test]
fn another_mailbox_for_an_account_forgets_the_old_mail() {
    let mut s = session();
    let base = json!({"email": "ana@example.org", "imapHost": "imap.example.org", "smtpHost": "smtp.example.org"});
    let a = AccountId(u32::try_from(s.run("account.add", &base).unwrap()["id"].as_u64().unwrap()).unwrap());
    let inbox = s.store.folder_by_role(a, FolderRole::Inbox).unwrap();
    s.store.folder_mut(inbox).unwrap().remote = Some(ibercorreo_core::RemoteFolder { path: "INBOX".into(), uid_validity: 1 });
    let mut m = Message::new(MessageId(0), inbox);
    m.remote = Some(ibercorreo_core::RemoteRef { folder: inbox, uid: 1, seen: false, flagged: false, id: None });
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

#[test]
fn search_finds_folded_words_in_a_scope_and_follows_undo() {
    let mut s = session();
    let subjects = |v: Value| v.as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    // Accents and case are ignored; words match from their start.
    assert_eq!(subjects(s.run("search.messages", &json!({"query": "\"PRESUPUESTO del segun\""})).unwrap()), ["Presupuesto del segundo trimestre"]);
    assert_eq!(subjects(s.run("search.messages", &json!({"query": "formacao"})).unwrap()), ["Formação Tinta Texto: datas"]);
    assert_eq!(subjects(s.run("search.messages", &json!({"query": "звіт"})).unwrap()), ["Звіт за жовтень"]);
    assert_eq!(s.search_scope, SearchScope::All);
    // A new draft is found at once, and gone again after undo.
    let id = s.run("message.compose", &json!({"to": "ana@example.org", "subject": "Ñandú quarterly"})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(subjects(s.run("search.messages", &json!({"query": "nandu"})).unwrap()), ["Ñandú quarterly"]);
    // Scopes: the Inbox does not hold the draft; Drafts does.
    s.run("folder.select", &json!({"folder": "inbox"})).unwrap();
    assert!(s.run("search.messages", &json!({"query": "nandu", "scope": "folder"})).unwrap().as_array().unwrap().is_empty());
    assert_eq!(s.search_scope, SearchScope::Folder);
    let drafts = s.store.message(MessageId(id)).unwrap().folder;
    s.run("folder.select", &json!({"folder": drafts.0})).unwrap();
    assert_eq!(s.run("search.messages", &json!({"query": "nandu"})).unwrap().as_array().unwrap().len(), 1);
    assert_eq!(s.run("search.messages", &json!({"query": "nandu", "scope": "MAILBOX"})).unwrap().as_array().unwrap().len(), 1);
    assert!(s.run("search.messages", &json!({"query": "nandu", "scope": "everywhere"})).is_err());
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(s.run("search.messages", &json!({"query": "nandu", "scope": "all"})).unwrap().as_array().unwrap().is_empty());
    s.run("edit.redo", &Value::Null).unwrap();
    assert_eq!(s.run("search.messages", &json!({"query": "nandu"})).unwrap().as_array().unwrap().len(), 1);
}

/// Tinta: an appointment moves and stretches as dragging it does, keeping its own zone.
#[test]
fn the_work_week_and_its_first_day_are_chosen() {
    let mut s = session();
    s.day = Date::new(2026, 11, 25).unwrap(); // a Wednesday
    assert_eq!(s.run("calendar.workWeek", &json!({})).unwrap()["days"], json!(["monday", "tuesday", "wednesday", "thursday", "friday"]));
    s.run("calendar.viewWorkWeek", &json!({})).unwrap();
    assert_eq!(s.calendar_range(), (Date::new(2026, 11, 23).unwrap(), Date::new(2026, 11, 27).unwrap()));
    // Tuesday to Saturday, weeks from Sunday.
    let r = s.run("calendar.workWeek", &json!({"days": ["tue", "wed", "Thursday", "fr", "sat"], "firstDay": "sunday"})).unwrap();
    assert_eq!(r["firstDay"], "sunday");
    let days = s.calendar_days();
    assert_eq!((days.len(), days[0], days[4]), (5, Date::new(2026, 11, 24).unwrap(), Date::new(2026, 11, 28).unwrap()));
    s.run("calendar.viewWeek", &json!({})).unwrap();
    assert_eq!(s.calendar_range(), (Date::new(2026, 11, 22).unwrap(), Date::new(2026, 11, 28).unwrap()), "Sunday to Saturday");
    s.day = Date::new(2026, 11, 22).unwrap(); // the Sunday itself starts its week
    assert_eq!(s.calendar_range().0, Date::new(2026, 11, 22).unwrap());
    // Days that are not next to each other.
    s.run("calendar.workWeek", &json!({"days": ["mon", "wed", "fri"], "firstDay": "default"})).unwrap();
    s.run("calendar.viewWorkWeek", &json!({})).unwrap();
    assert_eq!(s.calendar_days().iter().map(|d| d.day()).collect::<Vec<_>>(), vec![16, 18, 20]);
    for bad in [
        json!({"days": []}),
        json!({"days": ["m"]}),
        json!({"days": ["funday"]}),
        json!({"days": "mon"}),
        json!({"days": [1]}),
        json!({"firstDay": 0}),
        json!({"days": ["mon"], "firstDay": "x"}),
        json!({"days": vec!["mon"; 8]}),
    ] {
        assert!(s.run("calendar.workWeek", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.store.work_days(), 0b10101, "a refused change keeps the days, even next to a good one");
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!((s.store.work_days(), s.store.week_start()), (0b011_1110, 6), "undone like any edit");
    s.run("calendar.workWeek", &json!({"days": "default", "firstDay": "default"})).unwrap();
    assert_eq!((s.store.work_days(), s.store.week_start()), (0b001_1111, 0));
    s.store.work_days = Some(0);
    s.store.week_start = Some(9);
    assert_eq!((s.store.work_days(), s.store.week_start()), (0b001_1111, 0), "nonsense saved falls back");
}

#[test]
fn work_hours_are_chosen_and_checked() {
    let mut s = session();
    assert_eq!(s.run("calendar.workHours", &json!({})).unwrap(), json!({"start": "08:00", "end": "17:00", "default": true}));
    let r = s.run("calendar.workHours", &json!({"start": "07:30", "end": "15:00"})).unwrap();
    assert_eq!((r["start"].as_str(), r["end"].as_str(), r["default"].as_bool()), (Some("07:30"), Some("15:00"), Some(false)));
    assert_eq!(s.store.work_hours(), (450, 900));
    assert_eq!(s.run("calendar.workHours", &json!({"end": "24:00"})).unwrap()["end"], "24:00", "the start is kept");
    for bad in [
        json!({"start": "18:00", "end": "09:00"}),
        json!({"start": "25:00"}),
        json!({"end": "24:30"}),
        json!({"start": "9"}),
        json!({"start": "09:60"}),
        json!({"start": 9}),
    ] {
        assert!(s.run("calendar.workHours", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.store.work_hours(), (450, 1440), "a refused change changes nothing");
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.store.work_hours(), (450, 900), "undone like any edit");
    s.run("calendar.workHours", &json!({"start": "default"})).unwrap();
    assert_eq!(s.store.work_hours(), (480, 1020));
    // A saved span that makes no sense falls back to the default.
    s.store.work_hours = Some((900, 300));
    assert_eq!(s.store.work_hours(), (480, 1020));
}

#[test]
fn an_appointment_moves_and_stretches() {
    let mut s = session();
    s.run("calendar.timeZone", &json!({"zone": "Europe/Madrid"})).unwrap();
    let e = s.run("event.add", &json!({"summary": "Gestoría", "start": "2026-10-22T10:00", "end": "2026-10-22T11:30"})).unwrap();
    let id = e["id"].as_u64().unwrap();
    // Moved: the same length, on another day.
    let m = s.run("event.move", &json!({"id": id, "start": "2026-10-23T16:15"})).unwrap();
    assert_eq!((m["start"].as_str().unwrap(), m["end"].as_str().unwrap()), ("2026-10-23T16:15", "2026-10-23T17:45"));
    assert_eq!(s.day, Date::new(2026, 10, 23).unwrap());
    // Stretched: a new end.
    let m = s.run("event.move", &json!({"id": id, "start": "2026-10-23T16:15", "end": "2026-10-23T18:00"})).unwrap();
    assert_eq!(m["end"], "2026-10-23T18:00");
    assert!(s.run("event.move", &json!({"id": id, "start": "2026-10-23T16:15", "end": "2026-10-23T16:15"})).is_err(), "no empty span");
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(
        s.run("event.list", &json!({"date": "2026-10-23"})).unwrap().as_array().unwrap().iter().find(|e| e["id"] == id).unwrap()["end"],
        "2026-10-23T17:45"
    );
    // Seen from another zone, the instants are kept in the event's own zone.
    s.run("calendar.timeZone", &json!({"zone": "Europe/Lisbon"})).unwrap();
    s.run("event.move", &json!({"id": id, "start": "2026-10-23T09:00"})).unwrap();
    let ev = s.store.event(EventId(id)).unwrap();
    assert_eq!(ev.tzid.as_deref(), Some("Europe/Madrid"));
    assert!(
        matches!(ev.when, ibercorreo_core::calendar::When::Timed { start, .. } if start.hour() == 10),
        "09:00 in Lisbon is 10:00 in Madrid: {:?}",
        ev.when
    );
    // Repeating appointments are not moved this way; an all-day one moves by whole days.
    let weekly = s.run("event.add", &json!({"summary": "Clase", "start": "2026-10-22T18:00", "repeat": "weekly"})).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("event.move", &json!({"id": weekly, "start": "2026-10-22T19:00"})).is_err());
    let day = s.run("event.add", &json!({"summary": "Fiesta", "start": "2026-10-24"})).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("event.move", &json!({"id": day, "start": "2026-10-25T10:00"})).is_err(), "a date, not a time");
    let m = s.run("event.move", &json!({"id": day, "start": "2026-10-27"})).unwrap();
    assert_eq!((m["start"].as_str(), m["end"].as_str(), m["allDay"].as_bool()), (Some("2026-10-27"), Some("2026-10-28"), Some(true)));
    let m = s.run("event.move", &json!({"id": day, "start": "2026-10-27", "end": "2026-10-30"})).unwrap();
    assert_eq!(m["end"], "2026-10-30", "stretched to three days");
    let m = s.run("event.move", &json!({"id": day, "start": "2026-11-02"})).unwrap();
    assert_eq!(m["end"], "2026-11-05", "keeps its three days");
    for bad in [
        json!({"id": day, "start": "2026-11-02", "end": "2026-11-02"}),
        json!({"id": day, "start": "2026-11-02", "end": "2028-01-01"}),
        json!({"id": day, "start": "nope"}),
    ] {
        assert!(s.run("event.move", &bad).is_err(), "{bad}");
    }
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.store.event(EventId(day)).unwrap().when.start_date().to_string(), "2026-10-27", "undone like any edit");
    assert!(s.run("event.move", &json!({"id": id, "start": "not a time"})).is_err());
}

/// Tinta: date words seen from today, and `in:` a folder by name or role.
#[test]
fn search_takes_date_words_and_folders() {
    let mut s = session();
    // Tuesday 24 November 2026, midday.
    s.set_now(DateTime::new(Date::new(2026, 11, 24).unwrap(), 11, 0, 0).unwrap());
    let inbox = s.store.folder_by_role(AccountId(0), FolderRole::Inbox).unwrap();
    let sent = s.store.folder_by_role(AccountId(0), FolderRole::Sent).unwrap();
    for (date, subject, folder) in [
        ("Mon, 23 Nov 2026 09:00:00 +0000", "Quokka yesterday", inbox),
        ("Wed, 18 Nov 2026 09:00:00 +0000", "Quokka last week", inbox),
        ("Thu, 15 Oct 2026 09:00:00 +0000", "Quokka last month", sent),
    ] {
        let raw = format!("From: ana@example.org\r\nTo: me@example.com\r\nDate: {date}\r\nSubject: {subject}\r\n\r\nHola.\r\n");
        s.store.import_eml(raw.as_bytes(), folder).unwrap();
    }
    let found = |s: &mut Session, q: &str| -> Vec<String> {
        let mut v: Vec<String> = s
            .run("search.messages", &json!({"query": format!("quokka {q}"), "scope": "all"}))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["subject"].as_str().unwrap().to_string())
            .collect();
        v.sort();
        v
    };
    assert_eq!(found(&mut s, "received:yesterday"), ["Quokka yesterday"]);
    assert_eq!(found(&mut s, "отримано:вчора"), ["Quokka yesterday"]);
    assert_eq!(found(&mut s, "received:this-week"), ["Quokka yesterday"], "weeks start on Monday");
    assert_eq!(found(&mut s, "on:\"last week\""), ["Quokka last week"]);
    assert_eq!(found(&mut s, "recibido:\"semana pasada\""), ["Quokka last week"]);
    assert_eq!(found(&mut s, "received:\"минулого місяця\""), ["Quokka last month"]);
    assert_eq!(found(&mut s, "after:\"last week\""), ["Quokka last week", "Quokka yesterday"]);
    assert_eq!(found(&mut s, "before:this-month"), ["Quokka last month"]);
    assert_eq!(found(&mut s, "received:2026-11-18"), ["Quokka last week"]);
    assert_eq!(found(&mut s, "in:sent"), ["Quokka last month"]);
    assert_eq!(found(&mut s, "папка:вхідні").len(), 2);
    let name = s.store.folder(inbox).unwrap().name.clone();
    assert_eq!(found(&mut s, &format!("in:\"{name}\"")).len(), 2, "by its name");
    assert_eq!(found(&mut s, "-in:inbox"), ["Quokka last month"]);
    assert!(found(&mut s, "in:nowhere").is_empty(), "a folder no one has finds nothing");
    assert_eq!(found(&mut s, "received:someday").len(), 0, "an unknown date word is searched as words");
}

#[test]
fn rules_are_made_listed_reordered_and_run_on_a_folder_with_undo() {
    let mut s = session();
    let a = s.run("rule.add", &json!({"query": "presupuesto", "flag": true, "markRead": true})).unwrap();
    assert_eq!(a["name"], "presupuesto");
    assert_eq!((a["flag"].as_bool(), a["markRead"].as_bool(), a["moveTo"].is_null()), (Some(true), Some(true), true));
    let b = s.run("rule.add", &json!({"name": "Café", "query": "from:cafe", "moveTo": "archive", "stop": true})).unwrap();
    assert_eq!(b["moveToName"], "Archive");
    assert!(b["account"].is_number(), "a move rule takes the folder's account");
    let (a, b) = (a["id"].as_u64().unwrap(), b["id"].as_u64().unwrap());
    assert!(s.run("rule.add", &json!({"query": "x"})).is_err(), "no action");
    assert!(s.run("rule.add", &json!({"query": "x", "moveTo": 99999})).is_err());
    s.run("rule.move", &json!({"id": b})).unwrap();
    let order: Vec<u64> = s.run("rule.list", &Value::Null).unwrap().as_array().unwrap().iter().map(|r| r["id"].as_u64().unwrap()).collect();
    assert_eq!(order, [b, a]);
    // Run on the Inbox: the Café Azul newsletter is archived, the budget mail flagged and read.
    s.run("folder.select", &json!({"folder": "inbox"})).unwrap();
    let report = s.run("rule.run", &Value::Null).unwrap();
    assert_eq!(report["moved"], 1);
    assert!(report["flagged"].as_u64().unwrap() >= 1);
    assert!(s.status.starts_with("Rules matched"), "{}", s.status);
    let archive = s.store.folder_by_role(s.current_account(), FolderRole::Archive).unwrap();
    assert!(s.store.messages_in(archive).iter().any(|m| m.subject == "Novidades de outubro"));
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(!s.store.messages_in(archive).iter().any(|m| m.subject == "Novidades de outubro"));
    // Edit: drop the move, turn it off; then delete.
    let edited = s.run("rule.set", &json!({"id": b, "moveTo": false, "flag": true, "enabled": false})).unwrap();
    assert_eq!((edited["moveTo"].is_null(), edited["enabled"].as_bool(), edited["stop"].as_bool()), (true, Some(false), Some(true)));
    assert!(s.run("rule.set", &json!({"id": b, "flag": false})).is_err(), "a rule needs an action");
    s.run("rule.delete", &json!({"id": a})).unwrap();
    assert!(s.run("rule.delete", &json!({"id": a})).is_err());
    assert_eq!(s.run("rule.list", &Value::Null).unwrap().as_array().unwrap().len(), 1);
}

#[test]
fn rules_run_on_mail_that_arrives_by_sync_only() {
    use ibercorreo_sync::{Batch, FolderSync, NewMessage};
    let mut s = session();
    let account = s.current_account();
    s.run("rule.add", &json!({"query": "from:promo", "delete": true})).unwrap();
    let raw = |from: &str, subject: &str| {
        format!("From: {from}\r\nSubject: {subject}\r\nDate: Thu, 22 Oct 2026 09:00:00 +0000\r\n\r\nbody\r\n").into_bytes()
    };
    let batch = |uids: &[(u32, &str)]| Batch {
        mailboxes: vec![ibercorreo_imap::Mailbox { raw: "INBOX".into(), name: "INBOX".into(), delimiter: Some('/'), attributes: Vec::new() }],
        folders: vec![FolderSync {
            path: "INBOX".into(),
            uid_validity: 1,
            new: uids
                .iter()
                .map(|(uid, subject)| NewMessage {
                    uid: *uid,
                    flags: Vec::new(),
                    raw: raw("Promo <promo@spam.test>", subject),
                    id: None,
                    arrived: true,
                    partial: false,
                })
                .collect(),
            ..FolderSync::default()
        }],
        ..Batch::default()
    };
    let trash = s.store.folder_by_role(account, FolderRole::Trash).unwrap();
    let in_trash = |s: &Session, subject: &str| s.store.messages_in(trash).iter().any(|m| m.subject == subject);
    // The first download keeps old mail where it is.
    s.apply_sync(account, batch(&[(1, "old offer")]));
    assert!(!in_trash(&s, "old offer"));
    // New mail after that goes through the rules.
    s.apply_sync(account, batch(&[(2, "new offer")]));
    assert!(in_trash(&s, "new offer"));
    assert!(s.status.contains("Rules matched 1 messages: 1 deleted"), "{}", s.status);
}

/// An undo step keeps the store as it was without copying its mail: messages a command did not
/// change are shared with the step, so with a big mailbox each command stays quick.
#[test]
fn undo_steps_share_the_messages_a_command_left_alone() {
    let mut s = session();
    let a = s.run("message.compose", &json!({"subject": "one"})).unwrap()["id"].as_u64().unwrap();
    let b = s.run("message.compose", &json!({"subject": "two"})).unwrap()["id"].as_u64().unwrap();
    s.run("message.flag", &json!({"id": a})).unwrap();
    let step = s.undo.last().unwrap();
    assert!(std::ptr::eq(step.message(MessageId(b)).unwrap(), s.store.message(MessageId(b)).unwrap()));
    assert!(!step.message(MessageId(a)).unwrap().flagged);
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(!s.store.message(MessageId(a)).unwrap().flagged);
    s.run("edit.redo", &Value::Null).unwrap();
    assert!(s.store.message(MessageId(a)).unwrap().flagged);
}

/// Reopened from disk, messages have only their headers in memory: reading, replying, answering
/// an invitation, editing a draft and sending read the text from the store's content files.
#[test]
fn commands_read_message_text_kept_on_disk() {
    let dir = std::env::temp_dir().join(format!("ibercorreo-engine-content-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("store.json");
    let s = session();
    s.store.save(&path).unwrap();
    let mut s = Session::new(ibercorreo_store::Store::load(&path).unwrap(), today());
    assert!(s.store.all_messages().all(|m| m.content().is_none()), "nothing read at open");

    let inbox = s.store.folder_by_role(AccountId(0), FolderRole::Inbox).unwrap();
    let first = s.store.messages_in(inbox).into_iter().find(|m| m.preview(80).starts_with("Hola Denys")).map(|m| m.id).unwrap();
    let read = s.run("message.read", &json!({"id": first.0})).unwrap();
    assert!(read["body"].as_str().unwrap().contains("presupuesto revisado"), "{read}");
    let selected = s.run("message.select", &json!({"id": first.0})).unwrap();
    assert!(selected["body"].as_str().unwrap().contains("presupuesto revisado"), "{selected}");
    let listed = s.run("message.list", &json!({})).unwrap();
    assert!(listed.as_array().unwrap().iter().any(|m| m["invite"] == true), "the list knows without reading: {listed}");

    let v = s.run("message.reply", &json!({"id": first.0})).unwrap();
    let draft = MessageId(v["id"].as_u64().unwrap());
    assert!(s.store.content(draft).unwrap().body.contains("> Te envío el presupuesto revisado"));

    let id = invitation(&s);
    assert_eq!(s.invite_info(id).unwrap().summary, "Revisión del presupuesto");
    let out = s.run("invite.respond", &json!({"id": id.0, "response": "accept"})).unwrap();
    assert!(commands::render(&s, MessageId(out["reply"].as_u64().unwrap())).unwrap().contains("method=REPLY"));

    // A draft saved, reopened and edited: only the body changes, and it is what is sent.
    s.run("message.update", &json!({"id": draft.0, "subject": "Re: cifras"})).unwrap();
    s.store.save(&path).unwrap();
    let mut s = Session::new(ibercorreo_store::Store::load(&path).unwrap(), today());
    assert!(s.store.content(draft).unwrap().body.contains("> Te envío"), "a subject change keeps the text");
    s.run("message.update", &json!({"id": draft.0, "body": "Las cifras están bien."})).unwrap();
    let raw = commands::render(&s, draft).unwrap();
    let sent = ibercorreo_mime::parse(raw.as_bytes(), MessageId(99), FolderId(0)).unwrap();
    assert_eq!((sent.subject.as_str(), sent.content().unwrap().body.trim()), ("Re: cifras", "Las cifras están bien."));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Tinta Office: the error-report answer is kept and saved, never undone, and Forget moves the
/// id to the deletion the desktop app sends.
#[test]
fn error_reports_answer_is_kept_and_forget_moves_the_id() {
    let mut s = session();
    assert_eq!(s.store.error_reports.consent, None);
    s.dirty = false;
    let v = s.run("file.errorReports", &json!({"value": true})).unwrap();
    assert_eq!(v["value"], json!(true));
    assert!(s.dirty, "the answer is saved");
    assert!(s.run("edit.undo", &Value::Null).is_err() || s.store.error_reports.consent == Some(true));
    assert!(s.run("file.errorReports", &json!({"value": "yes"})).is_err());
    assert!(s.run("file.errorReports", &json!({"forget": 1})).is_err());
    let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
    s.store.error_reports.install_id = id.into();
    let v = s.run("file.errorReports", &json!({"forget": true})).unwrap();
    assert_eq!(v["forgetPending"], json!(true));
    assert_eq!(s.store.error_reports.forget_pending.as_deref(), Some(id));
    assert!(s.store.error_reports.install_id.is_empty());
}

#[test]
fn an_outlook_msg_imports_into_the_inbox_and_undoes() {
    let mut s = session();
    let bytes = ibercorreo_outlook::testing::mail("Presupuesto 2027", "joan@empresa.es", "Hola,\r\nel presupuesto.");
    let data = ibercorreo_mime::base64(&bytes);
    let r = s.run("message.importMsg", &json!({ "data": data })).unwrap();
    let id = MessageId(r["id"].as_u64().unwrap());
    let m = s.store.message(id).unwrap();
    assert_eq!(m.subject, "Presupuesto 2027");
    assert_eq!(m.from.as_ref().unwrap().email, "joan@empresa.es");
    assert_eq!(m.folder, s.store.folder_by_role(AccountId(0), FolderRole::Inbox).unwrap());
    assert_eq!(s.store.content(id).unwrap().body, "Hola,\nel presupuesto.");
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(s.store.message(id).is_err(), "undone");

    let r = s.run("message.importMsg", &json!({ "data": data, "folder": "sent" })).unwrap();
    let m = s.store.message(MessageId(r["id"].as_u64().unwrap())).unwrap();
    assert_eq!(m.folder, s.store.folder_by_role(AccountId(0), FolderRole::Sent).unwrap(), "into the folder asked for");
    assert!(s.run("message.importMsg", &json!({ "data": "not base64!" })).is_err());
    assert!(s.run("message.importMsg", &json!({ "data": ibercorreo_mime::base64(b"From: a@b.c\r\n\r\nx") })).is_err(), "not a .msg");
}

/// Tinta: a reply, also one in Sent, is in its message's conversation; the list setting toggles.
#[test]
fn conversations_gather_replies_across_folders() {
    let mut s = session();
    let first = s.store.all_messages().find(|m| !m.subject.is_empty()).map(|m| (m.id, m.folder, m.subject.clone())).unwrap();
    let account = s.store.account_of(first.1).unwrap();
    let sent = s.store.folder_by_role(account, FolderRole::Sent).unwrap();
    let mut reply = Message::new(MessageId(0), sent);
    reply.set_subject(&format!("RE: {}", first.2));
    let reply = s.store.add_message(reply).unwrap();
    let v = s.run("message.conversation", &json!({ "id": first.0 .0 })).unwrap();
    let ids: Vec<u64> = v["messages"].as_array().unwrap().iter().filter_map(|m| m["id"].as_u64()).collect();
    assert!(ids.contains(&reply.0) && ids.contains(&first.0.0), "{v}");
    assert_eq!(s.run("view.conversations", &json!({})).unwrap()["on"], true);
    assert!(s.store.conversations);
    assert_eq!(s.run("view.conversations", &json!({ "on": false })).unwrap()["on"], false);
}

/// Tinta: opening a header-only message asks for its text at the next Send/Receive, and the
/// window of months sets which mail comes whole.
#[test]
fn header_only_mail_gets_its_text_on_request() {
    use ibercorreo_sync::{Batch, FolderSync, NewMessage};
    let mut s = session();
    let account = s.current_account();
    let head = b"From: Ana <ana@example.org>\r\nSubject: Old report\r\nDate: Thu, 22 Jan 2026 09:00:00 +0000\r\n\r\n".to_vec();
    let mailbox = ibercorreo_imap::Mailbox { raw: "INBOX".into(), name: "INBOX".into(), delimiter: Some('/'), attributes: Vec::new() };
    s.apply_sync(
        account,
        Batch {
            mailboxes: vec![mailbox.clone()],
            folders: vec![FolderSync {
                path: "INBOX".into(),
                uid_validity: 1,
                new: vec![NewMessage { uid: 7, flags: Vec::new(), raw: head, id: None, arrived: false, partial: true }],
                ..FolderSync::default()
            }],
            ..Batch::default()
        },
    );
    let id = s.store.all_messages().find(|m| m.subject == "Old report").map(|m| m.id).unwrap();
    let other = s.store.all_messages().find(|m| !m.partial).map(|m| m.id).unwrap();
    assert_eq!(s.run("message.download", &json!({ "id": other.0 })).unwrap()["requested"], false);
    assert!(s.sync_state(account).bodies.is_empty());
    assert_eq!(s.run("message.download", &json!({ "id": id.0 })).unwrap()["requested"], true);
    assert!(s.sync_requested);
    assert_eq!(s.sync_state(account).bodies, [("INBOX".to_string(), 7)]);
    let whole = b"From: Ana <ana@example.org>\r\nSubject: Old report\r\nDate: Thu, 22 Jan 2026 09:00:00 +0000\r\n\r\nThe numbers.\r\n".to_vec();
    s.apply_sync(
        account,
        Batch {
            mailboxes: vec![mailbox],
            folders: vec![FolderSync { path: "INBOX".into(), uid_validity: 1, bodies: vec![(7, whole)], ..FolderSync::default() }],
            ..Batch::default()
        },
    );
    assert!(!s.store.message(id).unwrap().partial);
    assert!(s.bodies_wanted.is_empty(), "nothing left to ask for");

    assert!(s.full_since().is_none(), "all mail whole by default");
    assert_eq!(s.run("account.fullMail", &json!({ "months": 3 })).unwrap()["since"], "2026-07-22");
    assert_eq!(s.sync_limits().full_since.map(|d| d.to_string()).as_deref(), Some("2026-07-22"), "the sync is told");
    assert!(s.run("account.fullMail", &json!({ "months": 500 })).is_err());
    s.run("account.fullMail", &json!({ "months": 0 })).unwrap();
    assert!(s.full_since().is_none());
}

#[test]
fn files_attach_to_drafts_and_go_with_the_message() {
    let mut s = session();
    let id = s.run("message.compose", &json!({"to": "ana@example.org", "subject": "Factura", "body": "Adjunta."})).unwrap()["id"].as_u64().unwrap();
    let before = commands::content_hash(&s, MessageId(id)).unwrap();
    let pdf = b"%PDF-1.7 factura".to_vec();
    let out = s.run("message.attach", &json!({"id": id, "name": "../factura:11.pdf", "data": ibercorreo_mime::base64(&pdf)})).unwrap();
    assert_eq!(out["index"], 0);
    assert_eq!(out["attachmentList"][0]["name"], "factura_11.pdf", "a name made safe: {out}");
    assert_eq!(out["attachmentList"][0]["type"], "application/pdf");
    assert_eq!(out["attachmentList"][0]["kept"], true);
    s.run("message.attach", &json!({"id": id, "name": "notas.txt", "data": ibercorreo_mime::base64("Привіт".as_bytes()), "type": "text/plain"}))
        .unwrap();
    assert_ne!(commands::content_hash(&s, MessageId(id)).unwrap(), before, "a changed attachment list changes the hash");
    let raw = commands::render(&s, MessageId(id)).unwrap();
    let (back, contents) = ibercorreo_mime::parse_with_files(raw.as_bytes(), MessageId(99), FolderId(0)).unwrap();
    assert_eq!(back.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["factura_11.pdf", "notas.txt"]);
    assert_eq!(contents.first(), Some(&pdf));
    // Removed again; undo puts it back.
    let out = s.run("message.detach", &json!({"id": id, "index": 1})).unwrap();
    assert_eq!(out["attachmentList"].as_array().map(Vec::len), Some(1));
    assert!(s.run("message.detach", &json!({"id": id, "index": 1})).is_err());
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().attachments.len(), 2);
    // Sending puts the files in what goes out.
    s.run("message.send", &json!({"id": id})).unwrap();
    let outbox = s.outbox(AccountId(0));
    let raw = &outbox.iter().find(|o| o.id == MessageId(id)).unwrap().raw;
    assert_eq!(ibercorreo_mime::parse(raw, MessageId(98), FolderId(0)).unwrap().attachments.len(), 2);
    // Only drafts take files; bad data is refused.
    assert!(s.run("message.attach", &json!({"id": id, "name": "x", "data": "eA=="})).is_err(), "not a draft any more");
    let other = s.run("message.compose", &json!({"to": "ana@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("message.attach", &json!({"id": other, "name": "x", "data": "not base64!"})).is_err());
    assert!(s.run("message.attach", &json!({"id": other, "data": "eA=="})).is_err(), "a name is needed");
}

#[test]
fn a_draft_can_not_carry_more_than_a_message_holds() {
    let mut s = session();
    let id = s.run("message.compose", &json!({"to": "ana@example.org"})).unwrap()["id"].as_u64().unwrap();
    let big = vec![0u8; commands::MAX_DRAFT_FILES_BYTES / 2 + 1];
    s.run("message.attach", &json!({"id": id, "name": "a.bin", "data": ibercorreo_mime::base64(&big)})).unwrap();
    let e = s.run("message.attach", &json!({"id": id, "name": "b.bin", "data": ibercorreo_mime::base64(&big)})).unwrap_err();
    assert!(e.to_string().contains("too large") || e.to_string().contains("larger"), "{e}");
    assert_eq!(s.store.message(MessageId(id)).unwrap().attachments.len(), 1);
}

/// Tinta: a draft that says a file is attached but has none is flagged; with the file it is not.
#[test]
fn a_draft_that_promises_a_file_says_it_has_none() {
    let mut s = session();
    let id = s.run("message.compose", &json!({"to": "ana@example.org", "subject": "Octubre", "body": "Hola Ana,\nte adjunto la factura."})).unwrap()
        ["id"]
        .as_u64()
        .unwrap();
    assert_eq!(s.run("message.read", &json!({"id": id})).unwrap()["forgotAttachment"], json!(true));
    s.run("message.attach", &json!({"id": id, "name": "factura.pdf", "data": ibercorreo_mime::base64(b"%PDF-1.7")})).unwrap();
    assert_eq!(s.run("message.read", &json!({"id": id})).unwrap()["forgotAttachment"], json!(false));
    let plain = s.run("message.compose", &json!({"to": "ana@example.org", "body": "Nos vemos el lunes."})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.run("message.read", &json!({"id": plain})).unwrap()["forgotAttachment"], json!(false));
    s.run("message.send", &json!({"id": plain})).unwrap();
    assert!(s.run("message.read", &json!({"id": plain})).unwrap().get("forgotAttachment").is_none(), "only for drafts");
}

#[test]
fn forwarding_carries_the_attachments_kept() {
    let mut s = session();
    let raw = "From: ana@example.org\r\nTo: me@example.com\r\nSubject: Fotos\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: text/plain\r\n\r\nAhí van.\r\n--b\r\nContent-Type: image/png; name=uno.png\r\nContent-Disposition: attachment; filename=uno.png\r\n\
Content-Transfer-Encoding: base64\r\n\r\niVBORw0K\r\n--b--\r\n";
    let inbox = s.store.folder_by_role(AccountId(0), FolderRole::Inbox).unwrap();
    let orig = s.store.import_eml(raw.as_bytes(), inbox).unwrap();
    // One older message's file, whose contents were never kept.
    s.store.message_mut(orig).unwrap().attachments.push(ibercorreo_core::mail::Attachment { name: "old.doc".into(), ..Default::default() });
    let fwd = s.run("message.forward", &json!({"id": orig.0, "to": "luis@example.org"})).unwrap()["id"].as_u64().unwrap();
    let names: Vec<String> = s.store.message(MessageId(fwd)).unwrap().attachments.iter().map(|a| a.name.clone()).collect();
    assert_eq!(names, ["uno.png"]);
    assert!(s.status.contains("left out"), "{}", s.status);
    let raw = commands::render(&s, MessageId(fwd)).unwrap();
    let (_, contents) = ibercorreo_mime::parse_with_files(raw.as_bytes(), MessageId(97), FolderId(0)).unwrap();
    assert_eq!(contents.first().map(|c| c.starts_with(b"\x89PNG")), Some(true));
}

#[test]
fn files_are_read_to_attach_only_when_they_are_files() {
    let dir = std::env::temp_dir().join(format!("tinta-attach-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("informe.odt"), b"odt").unwrap();
    let (name, bytes) = attachments::read_to_attach(&dir.join("informe.odt")).unwrap();
    assert_eq!((name.as_str(), bytes.as_slice()), ("informe.odt", &b"odt"[..]));
    assert!(attachments::read_to_attach(&dir).is_err(), "a folder is not a file");
    assert!(attachments::read_to_attach(&dir.join("missing")).is_err());
    let mut s = session();
    let id = s.run("message.compose", &json!({"to": "ana@example.org"})).unwrap()["id"].as_u64().unwrap();
    attachments::attach(&mut s, MessageId(id), &name, &bytes).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().attachments.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_sender_becomes_a_contact_and_addresses_are_suggested() {
    let mut s = session();
    let from = |s: &Session, email: &str| s.store.all_messages().find(|m| m.from.as_ref().is_some_and(|a| a.email == email)).map(|m| m.id.0).unwrap();
    // The support desk is not a contact yet: one step adds it, named as the message names it.
    let help = from(&s, "help@example.org");
    let before = s.store.contact_count();
    let c = s.run("contact.addSender", &json!({"id": help})).unwrap();
    assert_eq!((c["name"].as_str(), c["existing"].as_bool()), (Some("Support desk"), Some(false)));
    assert_eq!(c["emails"][0], "help@example.org");
    assert_eq!(s.store.contact_count(), before + 1);
    // Again, or a sender already there (any case): no copy.
    assert_eq!(s.run("contact.addSender", &json!({"id": help})).unwrap()["existing"], true);
    assert_eq!(s.status, "Already in your contacts");
    let marta = from(&s, "marta@example.es");
    s.message = Some(MessageId(marta));
    assert_eq!(s.run("contact.addSender", &json!({})).unwrap()["existing"], true, "the open message by default");
    assert_eq!(s.store.contact_count(), before + 1);
    for _ in 0..3 {
        if s.store.contact_count() > before {
            s.run("edit.undo", &Value::Null).unwrap();
        }
    }
    assert_eq!(s.store.contact_count(), before, "undone");
    // A draft has no sender to add.
    let draft = s.run("message.compose", &json!({})).unwrap()["id"].as_u64().unwrap();
    s.store.message_mut(MessageId(draft)).unwrap().from = None;
    assert!(s.run("contact.addSender", &json!({"id": draft})).is_err());

    // Suggestions: contacts first, from the start of a name word or an address part, accents ignored.
    let text = |v: &Value| v.as_array().unwrap().iter().map(|x| x["text"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(text(&s.run("contact.suggest", &json!({"text": "igle"})).unwrap()), ["Marta Iglesias <marta@example.es>"]);
    assert_eq!(text(&s.run("contact.suggest", &json!({"text": "ОЛЕ"})).unwrap()), ["Олена Коваль <olena@example.com.ua>"]);
    assert_eq!(text(&s.run("contact.suggest", &json!({"text": "rui@"})).unwrap()), ["Rui Carvalho <rui@example.pt>"]);
    assert!(text(&s.run("contact.suggest", &json!({"text": "arta"})).unwrap()).is_empty(), "not from the middle of a word");
    assert!(text(&s.run("contact.suggest", &json!({"text": "  "})).unwrap()).is_empty());
    // Then the people written to, the most written to first; never the person's own address.
    let sent = s.store.folders().into_iter().find(|f| f.role == FolderRole::Sent).unwrap().id;
    for (to, n) in [("Martín Ruiz <martin@example.es>", 1), ("María López <maria@example.es>", 3), ("Denys <denys@example.com>", 2)] {
        for _ in 0..n {
            let mut m = Message::new(MessageId(0), sent);
            m.to = vec![ibercorreo_core::Address::parse_list(to).unwrap().remove(0)];
            s.store.add_message(m).unwrap();
        }
    }
    let found = s.run("contact.suggest", &json!({"text": "MAR"})).unwrap();
    assert_eq!(text(&found), ["Marta Iglesias <marta@example.es>", "María López <maria@example.es>", "Martín Ruiz <martin@example.es>"]);
    assert_eq!((found[0]["contact"].is_u64(), found[1]["contact"].is_null()), (true, true));
    assert!(text(&s.run("contact.suggest", &json!({"text": "denys"})).unwrap()).is_empty(), "own address left out");
    assert_eq!(text(&s.run("contact.suggest", &json!({"text": "mar", "limit": 1})).unwrap()).len(), 1);
    assert!(s.run("contact.suggest", &json!({})).is_err());
}
