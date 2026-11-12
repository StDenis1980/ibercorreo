use std::fs;
use std::path::{Path, PathBuf};

use ibercorreo_core::{Date, FolderRole, Message};
use serde_json::json;

use super::*;
use crate::{Session, sample};

fn session() -> Session {
    let today = Date::new(2026, 10, 22).unwrap();
    Session::new(sample::store(today).unwrap(), today)
}

/// A new empty folder for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tinta-export-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

/// Messages in an mbox: lines that start with `From `.
fn separators(mbox: &str) -> usize {
    mbox.lines().filter(|l| l.starts_with("From ")).count()
}

#[test]
fn everything_goes_to_a_new_folder_and_nothing_is_overwritten() {
    let mut s = session();
    let account = s.store.first_account();
    // Two folders whose names become the same file name, and one name Windows reserves.
    for name in ["Proyectos: 2026", "Proyectos? 2026", "con", "Clientes/Valencia"] {
        let f = s.store.add_folder(account, name, FolderRole::Other);
        let mut m = Message::new(s.store.next_message_id(), f);
        m.subject = format!("en {name}");
        s.store.add_message(m).unwrap();
    }
    let parent = scratch("all");
    let mut seen = Vec::new();
    let r = write_all(&s.store, &parent, "2026-11-12", |done, total| seen.push((done, total))).unwrap();
    let total = s.store.all_messages().len();
    assert_eq!(r.dir, parent.join("Tinta Correo 2026-11-12"));
    assert_eq!((r.messages, r.failed), (total, 0));
    assert_eq!(seen.last(), Some(&(total, total)), "progress reaches the end");
    let events = s.store.events().len();
    let contacts = s.store.all_contacts().len();
    assert!(events > 0 && contacts > 0, "the sample has a calendar and contacts");
    assert_eq!((r.events, r.contacts), (events, contacts));

    let list = files(&r.dir);
    let account_name = safe_name(&s.store.account(account).unwrap().name);
    for want in [
        format!("Mail/{account_name}/Proyectos_ 2026.mbox"),
        format!("Mail/{account_name}/Proyectos_ 2026 (2).mbox"),
        format!("Mail/{account_name}/_con.mbox"),
        format!("Mail/{account_name}/Clientes/Valencia.mbox"),
    ] {
        assert!(list.contains(&want), "{want} in {list:?}");
    }
    assert_eq!(list.iter().filter(|f| f.ends_with(".mbox")).count(), r.mailboxes);
    assert_eq!(list.iter().filter(|f| f.starts_with("Calendars/") && f.ends_with(".ics")).count(), r.calendars);
    assert_eq!(list.iter().filter(|f| f.starts_with("Contacts/") && f.ends_with(".vcf")).count(), r.books);
    let in_files: usize = list.iter().filter(|f| f.ends_with(".mbox")).map(|f| separators(&fs::read_to_string(r.dir.join(f)).unwrap())).sum();
    assert_eq!(in_files, total, "every message once");
    let vcf: String = list.iter().filter(|f| f.ends_with(".vcf")).map(|f| fs::read_to_string(r.dir.join(f)).unwrap()).collect();
    assert_eq!(vcf.matches("BEGIN:VCARD").count(), contacts);

    // A second export, and a file someone left where the third would go: both stay.
    fs::write(parent.join("Tinta Correo 2026-11-12 (3)"), "mine").unwrap();
    let again = write_all(&s.store, &parent, "2026-11-12", |_, _| {}).unwrap();
    assert_eq!(again.dir, parent.join("Tinta Correo 2026-11-12 (2)"));
    let third = write_all(&s.store, &parent, "2026-11-12", |_, _| {}).unwrap();
    assert_eq!(third.dir, parent.join("Tinta Correo 2026-11-12 (4)"));
    assert_eq!(fs::read_to_string(parent.join("Tinta Correo 2026-11-12 (3)")).unwrap(), "mine");
    assert_eq!(files(&r.dir), list, "the first export is untouched");
    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn an_empty_store_makes_an_empty_folder() {
    let parent = scratch("empty");
    let r = write_all(&ibercorreo_store::Store::new(), &parent.join("a/b"), "x/../y", |_, _| {}).unwrap();
    assert_eq!(r.dir, parent.join("a/b").join("Tinta Correo x_.._y"), "the date can't leave the folder");
    assert!(files(&r.dir).is_empty());
    assert_eq!((r.mailboxes, r.calendars, r.books), (0, 0, 0));
    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn an_unusable_place_is_an_error() {
    let parent = scratch("file");
    fs::create_dir_all(&parent).unwrap();
    let file = parent.join("a file");
    fs::write(&file, "x").unwrap();
    assert!(write_all(&session().store, &file, "2026-11-12", |_, _| {}).is_err());
    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn names_are_safe_everywhere() {
    for (name, want) in [
        ("Bandeja de entrada", "Bandeja de entrada"),
        ("a/b\\c:d*e?f\"g<h>i|j", "a_b_c_d_e_f_g_h_i_j"),
        ("  .oculto. ", "oculto"),
        ("...", "_"),
        ("", "_"),
        ("tab\there", "tab_here"),
        ("NUL", "_NUL"),
        ("com1.txt", "_com1.txt"),
        ("LPT9", "_LPT9"),
        ("COMx", "COMx"),
        ("Console", "Console"),
        ("Привіт", "Привіт"),
    ] {
        assert_eq!(safe_name(name), want, "{name:?}");
    }
    assert_eq!(safe_name(&"ñ".repeat(500)).chars().count(), MAX_NAME_CHARS);
}

#[test]
fn a_folder_exports_as_mbox_text() {
    let mut s = session();
    let inbox = s.store.folder_by_role(s.store.first_account(), FolderRole::Inbox).unwrap();
    let n = s.store.messages_in(inbox).len();
    assert!(n > 1);
    let v = s.run("folder.exportMbox", &json!({ "folder": "inbox" })).unwrap();
    assert_eq!((v["messages"].as_u64(), v["failed"].as_u64()), (Some(n as u64), Some(0)));
    let text = v["mbox"].as_str().unwrap();
    assert_eq!(separators(text), n);
    let newest = s.store.messages_in(inbox).first().map(|m| m.subject.clone()).unwrap();
    let last_subject = text.lines().rev().find_map(|l| l.strip_prefix("Subject: ")).unwrap();
    assert!(newest.is_ascii() && last_subject == newest, "oldest first: {last_subject} vs {newest}");
    s.folder = inbox;
    assert_eq!(s.run("folder.exportMbox", &json!({})).unwrap()["messages"].as_u64(), Some(n as u64), "the current folder");
    assert!(s.run("folder.exportMbox", &json!({ "folder": 9999 })).is_err());
}

#[test]
fn a_huge_folder_is_refused_as_text() {
    let s = session();
    let inbox = s.store.folder_by_role(s.store.first_account(), FolderRole::Inbox).unwrap();
    let (text, n, _) = mbox_within(&s.store, inbox, usize::MAX).unwrap();
    assert!(mbox_within(&s.store, inbox, text.len()).is_ok_and(|(_, m, _)| m == n), "the limit itself is fine");
    assert!(matches!(mbox_within(&s.store, inbox, text.len() - 1), Err(Error::TooLarge { .. })));
}
