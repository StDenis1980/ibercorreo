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
        json!({"date": "today", "start": "9999-12-31", "summary": "edge", "email": "a@b, c@d", "name": " "}),
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
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(FolderRole::Drafts).unwrap());
    s.run("message.send", &json!({"id": id})).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(FolderRole::Outbox).unwrap());
    assert!(s.run("message.send", &json!({"id": id})).is_err(), "only drafts send");
    s.run("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(FolderRole::Drafts).unwrap());
    s.run("edit.redo", &Value::Null).unwrap();
    assert_eq!(s.store.message(MessageId(id)).unwrap().folder, s.store.folder_by_role(FolderRole::Outbox).unwrap());
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
    let trash = s.store.folder_by_role(FolderRole::Trash).unwrap();
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
    assert_eq!(n, 3, "two appointments and the weekday stand-up");
    s.run("event.add", &json!({"summary": "Late call", "start": "2026-10-22T23:30"})).unwrap();
    let late = s.run("event.list", &json!({"date": "2026-10-23"})).unwrap();
    assert!(late.as_array().unwrap().iter().any(|e| e["summary"] == "Late call"), "an hour from 23:30 runs into the next day");
    s.run("calendar.goto", &json!({"date": "2026-01-31"})).unwrap();
    s.run("calendar.nextMonth", &Value::Null).unwrap();
    assert_eq!(s.day, Date::new(2026, 2, 28).unwrap());
    let ics = s.run("calendar.exportIcs", &Value::Null).unwrap();
    let mut fresh = Session::new(ibercorreo_store::Store::new(), today());
    let report = fresh.run("calendar.importIcs", &json!({"text": ics})).unwrap();
    assert_eq!(report["added"], json!(6), "four sample appointments, the stand-up and the late call");
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
    let out = s.outbox();
    assert_eq!(out.len(), 4);
    let first = out.iter().find(|o| o.id == ok).unwrap();
    assert_eq!(first.to, ["ana@example.org"]);
    assert_eq!(first.from, "denys@example.com");
    let text = String::from_utf8(first.raw.clone()).unwrap();
    assert!(text.contains("Subject: a\r\n") && text.contains("Message-ID: <"), "{text}");
    assert_eq!(s.outbox().iter().find(|o| o.id == ok).unwrap().raw, first.raw, "the same bytes every time");

    s.apply_sent(&[
        Sent { id: ok, outcome: Outcome::Sent { saved: false } },
        Sent { id: saved, outcome: Outcome::Sent { saved: true } },
        Sent { id: later, outcome: Outcome::Later("451 try later".into()) },
        Sent { id: bad, outcome: Outcome::Failed("550 no such user".into()) },
    ]);
    let folder = |id| s.store.message(id).map(|m| m.folder);
    assert_eq!(folder(ok), s.store.folder_by_role(FolderRole::Sent));
    assert!(s.store.message(ok).unwrap().seen);
    assert!(s.store.message(saved).is_err(), "the server's Sent copy comes back with the next sync");
    assert_eq!(folder(later), s.store.folder_by_role(FolderRole::Outbox));
    assert_eq!(folder(bad), s.store.folder_by_role(FolderRole::Drafts));
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
    assert_eq!(s.store.accounts.len(), 1, "the first account is replaced");
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
    ] {
        assert!(s.run("account.set", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.run("account.get", &Value::Null).unwrap()["imapHost"], "imap.example.org");
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
