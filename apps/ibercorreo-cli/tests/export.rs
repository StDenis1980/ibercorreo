//! `ibercorreo-cli export` end to end: a saved store written out as mbox, iCalendar and vCard.

use std::process::Command;

#[test]
fn a_store_exports_to_a_new_folder() {
    let dir = std::env::temp_dir().join(format!("tinta-cli-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("store.json");
    let cli = env!("CARGO_BIN_EXE_ibercorreo-cli");
    let today = ibercorreo_core::Date::new(2026, 11, 12).unwrap();
    ibercorreo_engine::sample::store(today).unwrap().save(&store).unwrap();

    let out = Command::new(cli).args(["export", "--store"]).arg(&store).arg(dir.join("out")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["messages"].as_u64().unwrap() > 0 && report["events"].as_u64().unwrap() > 0 && report["contacts"].as_u64().unwrap() > 0,
        "{report}"
    );
    let folder = std::path::PathBuf::from(report["folder"].as_str().unwrap());
    assert!(
        folder.starts_with(dir.join("out")) && folder.join("Mail").is_dir() && folder.join("Calendars").is_dir() && folder.join("Contacts").is_dir()
    );

    for bad in [&["export"][..], &["export", "--store"], &["export", "out"], &["export", "--store", "nowhere.json", "out", "extra"]] {
        let out = Command::new(cli).args(bad).current_dir(&dir).output().unwrap();
        assert!(!out.status.success(), "{bad:?}");
    }
    let extra = Command::new(cli).args(["export", "--store"]).arg(&store).args(["out2", "extra"]).current_dir(&dir).output().unwrap();
    assert!(!extra.status.success() && !dir.join("out2").exists(), "a stray argument stops it before anything is written");
    let _ = std::fs::remove_dir_all(&dir);
}
