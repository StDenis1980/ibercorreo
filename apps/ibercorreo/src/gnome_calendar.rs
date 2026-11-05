//! Tinta Correo's events in the GNOME Shell clock panel (and any other app that reads Evolution
//! Data Server, EDS).
//!
//! The panel shows the selected calendars of EDS, which every GNOME desktop runs even without
//! GNOME Calendar. Correo writes its events to `~/.local/share/tinta-correo/calendar.ics` and
//! registers that file once as a read-only EDS calendar, "Tinta Correo" (a `local` source with
//! `CustomFile`). EDS watches the file, so an event created, moved or deleted in Correo shows in
//! the panel within seconds; nothing is sent anywhere and nothing is read back.
//!
//! Only where EDS is in use (its sources folder exists). Inside Flatpak the paths are the host's,
//! reached through `--filesystem=xdg-config/evolution/sources:create` and
//! `--filesystem=xdg-data/tinta-correo:create`. While the registry runs, the source is created
//! through its D-Bus API (`SourceManager.CreateSources`), which it picks up at once; otherwise
//! the key file is written and read at the next login.
//!
//! Calendars that already reach EDS another way are left out of the file, so the panel never
//! shows an event twice: those of a CalDAV server Correo signs in to through GNOME Online
//! Accounts (`Account::goa`, `Account::dav_goa`), which EDS mirrors itself. For those, a change
//! made in Correo is sent to the server at once (a Send/Receive within seconds), and EDS is then
//! asked to fetch the calendar again ([`Exporter::refresh`]), so the panel shows it without
//! waiting for EDS's own refresh.
//!
//! Contacts go the same way, so GNOME Contacts, the panel's search and other GNOME apps find the
//! people kept in Correo: an EDS address book, "Tinta Correo" (a `local` source), whose contacts
//! Correo keeps equal to its own through the address book's D-Bus API (`CreateContacts`,
//! `ModifyContacts`, `RemoveContacts`, matched by the vCard's UID). The book is Correo's copy: a
//! change made to it elsewhere is replaced at Correo's next change. Address books EDS already
//! mirrors from a CardDAV server of a GNOME Online Accounts account are left out, as calendars
//! are.
//!
//! The writing and the D-Bus calls happen on a thread of their own; the window only compares its
//! events and contacts with the last ones written, once a second.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use ibercorreo_core::Contact;
use ibercorreo_core::calendar::Event;
use ibercorreo_engine::Session;
use ibercorreo_store::Store;

/// The EDS source's id and file name.
pub const SOURCE_UID: &str = "tinta-correo";
/// The EDS address book's source id and file name.
pub const BOOK_UID: &str = "tinta-correo-contacts";
/// A mirror of the contacts that could not be written (EDS not up yet) is tried again this often.
const RETRY_EVERY: Duration = Duration::from_secs(30);
/// Contacts sent to EDS in one D-Bus call.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))] // used only by the GNOME (Linux, BSD) code and the tests
const CHUNK: usize = 100;
/// How often the window looks for changed events.
const EVERY: Duration = Duration::from_secs(1);
/// A change to a calendar EDS mirrors itself is sent at once, but not more often than this (a
/// server out of reach is not asked every second).
const PUSH_EVERY: Duration = Duration::from_secs(15);

/// Where the files go: the host's folders, also inside Flatpak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `…/tinta-correo/calendar.ics`
    pub ics: PathBuf,
    /// `…/evolution/sources`
    pub sources: PathBuf,
}

impl Paths {
    /// From the environment; `None` without a home folder.
    pub fn from_env() -> Option<Paths> {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        Paths::resolve(var("HOME")?, var("FLATPAK_ID").is_some(), var("XDG_DATA_HOME"), var("XDG_CONFIG_HOME"))
    }

    /// Inside Flatpak `XDG_DATA_HOME` and `XDG_CONFIG_HOME` point into the sandbox, which the
    /// host's EDS can't see, so the host's defaults are used there.
    fn resolve(home: PathBuf, flatpak: bool, data: Option<PathBuf>, config: Option<PathBuf>) -> Option<Paths> {
        if !home.is_absolute() {
            return None;
        }
        let pick = |own: Option<PathBuf>, default: &str| match own.filter(|p| p.is_absolute() && !flatpak) {
            Some(p) => p,
            None => home.join(default),
        };
        Some(Paths {
            ics: pick(data, ".local/share").join("tinta-correo").join("calendar.ics"),
            sources: pick(config, ".config").join("evolution").join("sources"),
        })
    }

    fn source_file(&self) -> PathBuf {
        self.sources.join(format!("{SOURCE_UID}.source"))
    }

    fn book_source_file(&self) -> PathBuf {
        self.sources.join(format!("{BOOK_UID}.source"))
    }
}

/// The EDS key file for the address book: a `local` book EDS keeps itself, filled by Correo.
pub fn book_key_file() -> String {
    "[Data Source]\nDisplayName=Tinta Correo\nEnabled=true\nParent=local-stub\n\n[Address Book]\nBackendName=local\n\n[Offline]\nStaySynchronized=false\n".to_string()
}

/// The EDS key file for the calendar file. Read-only for EDS: Correo is where events change.
pub fn source_key_file(ics: &Path) -> String {
    format!(
        "[Data Source]\nDisplayName=Tinta Correo\nEnabled=true\nParent=local-stub\n\n[Calendar]\nBackendName=local\nColor=#1c71d8\nSelected=true\n\n[Local Backend]\nCustomFile={}\n\n[Offline]\nStaySynchronized=false\n",
        file_uri(ics)
    )
}

/// `file://` and the path, percent-encoding what a URI can't hold.
fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The GNOME Online Accounts account whose CalDAV calendars EDS already shows (the one the
/// calendars sign in through), if any.
pub fn linked_goa(store: &Store) -> Option<String> {
    let a = store.accounts.iter().find(|a| !a.caldav.trim().is_empty())?;
    [&a.dav_goa, &a.goa].into_iter().find(|id| !id.is_empty()).cloned()
}

/// The calendar file's text: every event but those of the CalDAV calendars EDS shows itself.
fn export(store: &Store, linked: bool) -> String {
    let skipped: Vec<u32> = if linked {
        store.calendars().iter().filter(|c| c.remote.as_ref().is_some_and(|r| ibercorreo_sync::calendar::is_caldav(&r.href))).map(|c| c.id).collect()
    } else {
        Vec::new()
    };
    ibercorreo_ical::write(store.events().iter().filter(|e| !skipped.contains(&e.calendar)))
}

/// The contacts for EDS, by UID: every contact but those of the CardDAV address books EDS shows
/// itself. A contact without a UID gets one from its id.
fn export_contacts(store: &Store, linked: bool) -> Vec<(String, String)> {
    let skipped: Vec<u32> = if linked {
        store.books().iter().filter(|b| b.remote.as_ref().is_some_and(|r| ibercorreo_sync::calendar::is_caldav(&r.href))).map(|b| b.id).collect()
    } else {
        Vec::new()
    };
    store
        .all_contacts()
        .iter()
        .filter(|c| !skipped.contains(&c.book))
        .map(|c| {
            if c.uid.trim().is_empty() {
                let mut c = c.clone();
                c.uid = format!("tinta-correo-{}", c.id);
                (c.uid.clone(), ibercorreo_vcard::write_card(&c))
            } else {
                (c.uid.clone(), ibercorreo_vcard::write_card(c))
            }
        })
        .collect()
}

/// What to change in the EDS book so it holds `cards`: the UIDs to remove, then the cards to
/// create and to modify. `existing` are the UIDs the book has; `sent` what was last written, so a
/// card that did not change is not written again.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))] // used only by the GNOME (Linux, BSD) code and the tests
fn plan<'a>(existing: &[String], cards: &'a [(String, String)], sent: &HashMap<String, String>) -> (Vec<String>, Vec<&'a str>, Vec<&'a str>) {
    let ours: std::collections::HashSet<&str> = cards.iter().map(|(uid, _)| uid.as_str()).collect();
    let have: std::collections::HashSet<&str> = existing.iter().map(String::as_str).collect();
    let remove = existing.iter().filter(|u| !ours.contains(u.as_str())).cloned().collect();
    let mut create = Vec::new();
    let mut modify = Vec::new();
    // A UID twice (two books holding the same person) is written once.
    let mut seen = std::collections::HashSet::new();
    for (uid, card) in cards {
        if !seen.insert(uid.as_str()) {
            continue;
        }
        if !have.contains(uid.as_str()) {
            create.push(card.as_str());
        } else if sent.get(uid) != Some(card) {
            modify.push(card.as_str());
        }
    }
    (remove, create, modify)
}

/// What the writer thread does.
enum Task {
    Write(String),
    /// Make the EDS address book hold these contacts (UID, vCard).
    Contacts(Vec<(String, String)>),
    /// Ask EDS to fetch the calendars of this GNOME Online Accounts account again.
    Refresh(String),
}

/// The window's side: notices changed events and hands them to the writer thread.
pub struct Exporter {
    tx: Sender<Task>,
    last: Option<Vec<Event>>,
    last_contacts: Option<Vec<Contact>>,
    checked: Option<Instant>,
    /// When a Send/Receive was last asked for, to send a change to a linked server.
    pushed: Option<Instant>,
    /// A change to a linked server waits for its Send/Receive.
    push_pending: bool,
}

impl Exporter {
    /// Starts the writer when EDS is in use here; `None` otherwise (other desktops, Windows,
    /// macOS, no home folder).
    pub fn start() -> Option<Exporter> {
        let paths = Paths::from_env()?;
        if !paths.sources.is_dir() {
            return None;
        }
        let (tx, rx) = channel::<Task>();
        std::thread::Builder::new()
            .name("gnome-calendar".into())
            .spawn(move || {
                let mut registered = false;
                let mut book_registered = false;
                // What the EDS book was last given, and contacts still to give it.
                let mut sent: HashMap<String, String> = HashMap::new();
                let mut waiting: Option<Vec<(String, String)>> = None;
                loop {
                    let first = match rx.recv_timeout(RETRY_EVERY) {
                        Ok(task) => Some(task),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    };
                    // Only the newest file and contacts of a burst of changes; each refresh once.
                    let (mut ics, mut refresh) = (None, Vec::new());
                    for task in first.into_iter().chain(rx.try_iter()) {
                        match task {
                            Task::Write(text) => ics = Some(text),
                            Task::Contacts(cards) => waiting = Some(cards),
                            Task::Refresh(id) if !refresh.contains(&id) => refresh.push(id),
                            Task::Refresh(_) => {}
                        }
                    }
                    if let Some(ics) = ics
                        && write_private(&paths.ics, ics.as_bytes()).is_ok()
                        && !registered
                    {
                        registered = register(&paths.source_file(), &source_key_file(&paths.ics), SOURCE_UID, create_through_registry);
                    }
                    if let Some(cards) = waiting.take() {
                        if !book_registered {
                            book_registered = register(&paths.book_source_file(), &book_key_file(), BOOK_UID, create_through_registry);
                        }
                        // Not written (EDS not running yet): tried again in a while.
                        if !mirror_through_eds(&cards, &mut sent) {
                            waiting = Some(cards);
                        }
                    }
                    for id in refresh {
                        refresh_through_eds(&id);
                    }
                }
            })
            .ok()?;
        Some(Exporter { tx, last: None, last_contacts: None, checked: None, pushed: None, push_pending: false })
    }

    /// Each frame: once a second, writes the events again if they changed. A change to a calendar
    /// EDS mirrors from the server asks for a Send/Receive, so it reaches the server (and then the
    /// panel) at once.
    pub fn tick(&mut self, session: &mut Session, syncing: bool) {
        if self.checked.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.checked = Some(Instant::now());
        let linked = linked_goa(&session.store).is_some();
        let events = session.store.events();
        if self.last.as_deref() != Some(events) {
            self.last = Some(events.to_vec());
            let _ = self.tx.send(Task::Write(export(&session.store, linked)));
            // Only changes made here wait to be sent; what a Send/Receive brought does not.
            self.push_pending = linked && !session.calendar_sync_state().changes.is_empty();
        }
        let contacts = session.store.all_contacts();
        if self.last_contacts.as_deref() != Some(contacts) {
            self.last_contacts = Some(contacts.to_vec());
            let _ = self.tx.send(Task::Contacts(export_contacts(&session.store, linked)));
        }
        if self.push_pending && !syncing && self.pushed.is_none_or(|t| t.elapsed() >= PUSH_EVERY) {
            self.push_pending = false;
            self.pushed = Some(Instant::now());
            session.sync_requested = true;
        }
    }

    /// A change waits to be sent: the window wakes for it.
    pub fn waiting(&self) -> bool {
        self.push_pending
    }

    /// After a Send/Receive that sent calendar changes to a linked server: EDS fetches that
    /// account's calendars again.
    pub fn refresh(&self, goa: &str) {
        let _ = self.tx.send(Task::Refresh(goa.to_string()));
    }
}

/// The EDS calendars made from GNOME Online Accounts account `goa`, by their EDS ids: the
/// children of its collection source (`[GNOME Online Accounts] AccountId=…`), given each
/// source's id and key file.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))] // used only by the GNOME (Linux, BSD) code and the tests
pub fn goa_calendars(sources: &[(String, String)], goa: &str) -> Vec<String> {
    let value = |data: &str, section: &str, key: &str| -> Option<String> {
        let mut inside = false;
        for line in data.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                inside = line == format!("[{section}]");
            } else if inside
                && let Some((k, v)) = line.split_once('=')
                && k.trim() == key
            {
                return Some(v.trim().to_string());
            }
        }
        None
    };
    let has = |data: &str, section: &str| data.lines().any(|l| l.trim() == format!("[{section}]"));
    let collections: Vec<&str> = sources
        .iter()
        .filter(|(_, data)| value(data, "GNOME Online Accounts", "AccountId").as_deref() == Some(goa))
        .map(|(uid, _)| uid.as_str())
        .collect();
    sources
        .iter()
        .filter(|(_, data)| has(data, "Calendar") && value(data, "Data Source", "Parent").is_some_and(|p| collections.contains(&p.as_str())))
        .map(|(uid, _)| uid.clone())
        .collect()
}

/// Writes through a temporary file and a rename (EDS never reads half a file), readable only by
/// the person.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("ics.part");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// Makes sure EDS knows source `uid` (key file `file`, text `data`): `true` once it does (or will
/// at the next login).
fn register(file: &Path, data: &str, uid: &str, create: fn(&str, &str) -> bool) -> bool {
    if file.exists() {
        return true;
    }
    if create(uid, data) && file.exists() {
        return true;
    }
    write_private(file, data.as_bytes()).is_ok()
}

/// `SourceManager.CreateSources` on the running source registry: it writes the key file and
/// shows the calendar at once.
#[cfg(all(unix, not(target_os = "macos")))]
fn create_through_registry(uid: &str, data: &str) -> bool {
    let Ok(bus) = zbus::blocking::Connection::session() else { return false };
    let sources = std::collections::HashMap::from([(uid, data)]);
    bus.call_method(
        Some("org.gnome.evolution.dataserver.Sources5"),
        "/org/gnome/evolution/dataserver/SourceManager",
        Some("org.gnome.evolution.dataserver.SourceManager"),
        "CreateSources",
        &(sources,),
    )
    .is_ok()
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn create_through_registry(_uid: &str, _data: &str) -> bool {
    false
}

/// Makes the EDS address book hold `cards`, through the address book factory; `sent` keeps what
/// it was given. `false` when EDS could not be reached (nothing is lost: the caller tries again).
#[cfg(all(unix, not(target_os = "macos")))]
fn mirror_through_eds(cards: &[(String, String)], sent: &mut HashMap<String, String>) -> bool {
    let Ok(bus) = zbus::blocking::Connection::session() else { return false };
    // One connection for the whole exchange: EDS closes a book when the client that opened it
    // goes away.
    let opened = bus.call_method(
        Some("org.gnome.evolution.dataserver.AddressBook10"),
        "/org/gnome/evolution/dataserver/AddressBookFactory",
        Some("org.gnome.evolution.dataserver.AddressBookFactory"),
        "OpenAddressBook",
        &(BOOK_UID,),
    );
    let Ok((path, name)) = opened.and_then(|r| r.body().deserialize::<(String, String)>()) else { return false };
    const BOOK: &str = "org.gnome.evolution.dataserver.AddressBook";
    if bus.call_method(Some(name.as_str()), path.as_str(), Some(BOOK), "Open", &()).is_err() {
        return false;
    }
    let existing = bus
        .call_method(Some(name.as_str()), path.as_str(), Some(BOOK), "GetContactListUids", &("",))
        .and_then(|r| r.body().deserialize::<Vec<String>>());
    let ok = match existing {
        Ok(existing) => {
            let (remove, create, modify) = plan(&existing, cards, sent);
            let flags: u32 = 0;
            let mut ok = true;
            for chunk in remove.chunks(CHUNK) {
                ok &= bus.call_method(Some(name.as_str()), path.as_str(), Some(BOOK), "RemoveContacts", &(chunk, flags)).is_ok();
            }
            for chunk in create.chunks(CHUNK) {
                ok &= bus.call_method(Some(name.as_str()), path.as_str(), Some(BOOK), "CreateContacts", &(chunk, flags)).is_ok();
            }
            for chunk in modify.chunks(CHUNK) {
                ok &= bus.call_method(Some(name.as_str()), path.as_str(), Some(BOOK), "ModifyContacts", &(chunk, flags)).is_ok();
            }
            if ok {
                *sent = cards.iter().cloned().collect();
            }
            ok
        }
        Err(_) => false,
    };
    let _ = bus.call_method(Some(name.as_str()), path.as_str(), Some(BOOK), "Close", &());
    ok
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn mirror_through_eds(_cards: &[(String, String)], _sent: &mut HashMap<String, String>) -> bool {
    true
}

/// Asks the calendar factory to open each of the account's EDS calendars and fetch it again
/// from the server (`Calendar.Refresh`). What fails is left: EDS refreshes on its own later.
#[cfg(all(unix, not(target_os = "macos")))]
fn refresh_through_eds(goa: &str) {
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    let Ok(bus) = zbus::blocking::Connection::session() else { return };
    let Ok(reply) = bus.call_method(
        Some("org.gnome.evolution.dataserver.Sources5"),
        "/org/gnome/evolution/dataserver/SourceManager",
        Some("org.freedesktop.DBus.ObjectManager"),
        "GetManagedObjects",
        &(),
    ) else {
        return;
    };
    let Ok(objects) = reply.body().deserialize::<HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>>() else { return };
    let text = |props: &HashMap<String, OwnedValue>, key: &str| match props.get(key).map(|v| &**v) {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    };
    let sources: Vec<(String, String)> = objects
        .values()
        .filter_map(|ifaces| ifaces.get("org.gnome.evolution.dataserver.Source"))
        .map(|props| (text(props, "UID"), text(props, "Data")))
        .collect();
    for uid in goa_calendars(&sources, goa) {
        let opened = bus.call_method(
            Some("org.gnome.evolution.dataserver.Calendar8"),
            "/org/gnome/evolution/dataserver/CalendarFactory",
            Some("org.gnome.evolution.dataserver.CalendarFactory"),
            "OpenCalendar",
            &(uid.as_str(),),
        );
        let Ok((path, name)) = opened.and_then(|r| r.body().deserialize::<(String, String)>()) else { continue };
        let call = |method: &str| bus.call_method(Some(name.as_str()), path.as_str(), Some("org.gnome.evolution.dataserver.Calendar"), method, &());
        if call("Open").is_ok() {
            let _ = call("Refresh");
        }
        let _ = call("Close");
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn refresh_through_eds(_goa: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_the_hosts_also_inside_flatpak() {
        let home = PathBuf::from("/home/ana");
        let p = Paths::resolve(home.clone(), false, None, None).unwrap();
        assert_eq!(p.ics, PathBuf::from("/home/ana/.local/share/tinta-correo/calendar.ics"));
        assert_eq!(p.sources, PathBuf::from("/home/ana/.config/evolution/sources"));
        let own = Paths::resolve(home.clone(), false, Some("/data".into()), Some("/cfg".into())).unwrap();
        assert_eq!(own.ics, PathBuf::from("/data/tinta-correo/calendar.ics"));
        assert_eq!(own.sources, PathBuf::from("/cfg/evolution/sources"));
        let sandbox =
            Paths::resolve(home.clone(), true, Some("/home/ana/.var/app/x/data".into()), Some("/home/ana/.var/app/x/config".into())).unwrap();
        assert_eq!(sandbox, p);
        assert_eq!(Paths::resolve(home, false, Some("relative".into()), None).unwrap(), p);
        assert_eq!(Paths::resolve("relative".into(), false, None, None), None);
    }

    #[test]
    fn the_source_points_eds_at_the_file() {
        let data = source_key_file(Path::new("/home/ana maría/.local/share/tinta-correo/calendar.ics"));
        assert!(data.contains("BackendName=local\n"), "{data}");
        assert!(data.contains("Selected=true\n"), "{data}");
        assert!(data.contains("CustomFile=file:///home/ana%20mar%C3%ADa/.local/share/tinta-correo/calendar.ics\n"), "{data}");
        assert!(data.starts_with("[Data Source]\n"));
    }

    #[test]
    fn eds_calendars_of_a_gnome_account_are_found_by_their_collection() {
        let sources = vec![
            ("goa-1".to_string(), "[Data Source]\nDisplayName=ana@cloud\nParent=\n\n[Collection]\nBackendName=webdav\n\n[GNOME Online Accounts]\nAccountId=account_1700000000_0\n".to_string()),
            ("cal-1".to_string(), "[Data Source]\nParent=goa-1\n\n[Calendar]\nBackendName=caldav\n".to_string()),
            ("book-1".to_string(), "[Data Source]\nParent=goa-1\n\n[Address Book]\nBackendName=carddav\n".to_string()),
            ("other".to_string(), "[Data Source]\nParent=goa-2\n\n[Calendar]\n".to_string()),
            ("goa-2".to_string(), "[GNOME Online Accounts]\nAccountId=account_1700000000_2\n".to_string()),
            // The id named in another section is not the account's.
            ("decoy".to_string(), "[Data Source]\nParent=x\nAccountId=account_1700000000_0\n\n[Calendar]\n".to_string()),
        ];
        assert_eq!(goa_calendars(&sources, "account_1700000000_0"), ["cal-1"]);
        assert_eq!(goa_calendars(&sources, "account_1700000000_2"), ["other"]);
        assert!(goa_calendars(&sources, "nobody").is_empty());
        assert!(goa_calendars(&[], "account_1700000000_0").is_empty());
    }

    #[test]
    fn calendars_eds_mirrors_itself_are_left_out_of_the_file() {
        use ibercorreo_core::CalendarRemote;
        use serde_json::json;
        let today = ibercorreo_core::Date::new(2026, 11, 3).unwrap();
        let mut s = Session::new(ibercorreo_engine::sample::store(today).unwrap(), today);
        let local = s.store.calendars().first().unwrap().id;
        let remote = s
            .store
            .add_calendar("Nextcloud", "#0082c9", Some(CalendarRemote { href: "/remote.php/dav/calendars/ana/personal/".into(), ctag: None }))
            .unwrap();
        s.run("event.add", &json!({"summary": "Dentista", "start": "2026-11-04T10:00", "calendar": local})).unwrap();
        s.run("event.add", &json!({"summary": "Reunión de equipo", "start": "2026-11-04T12:00", "calendar": remote})).unwrap();
        // No account signs in to its calendars through GNOME: everything goes in the file.
        assert_eq!(linked_goa(&s.store), None);
        let all = export(&s.store, false);
        assert!(all.contains("Dentista") && all.contains("Reunión de equipo"), "{all}");
        let id = s.store.first_account();
        let a = s.store.account_mut(id).unwrap();
        a.caldav = "https://cloud.example.org/remote.php/dav".into();
        a.dav_goa = "account_1700000000_0".into();
        assert_eq!(linked_goa(&s.store).as_deref(), Some("account_1700000000_0"));
        let file = export(&s.store, true);
        assert!(file.contains("Dentista"), "Correo's own calendar stays: {file}");
        assert!(!file.contains("Reunión de equipo"), "EDS shows it from GNOME's account: {file}");
        // The new event waits to be sent, so the exporter asks for a Send/Receive at once, and
        // not again for what that Send/Receive brings.
        assert!(!s.calendar_sync_state().changes.is_empty());
        let (tx, rx) = channel();
        let mut e = Exporter { tx, last: None, last_contacts: None, checked: None, pushed: None, push_pending: false };
        s.sync_requested = false;
        e.tick(&mut s, true);
        assert!(!s.sync_requested && e.waiting(), "a Send/Receive running: after it");
        e.checked = None;
        e.tick(&mut s, false);
        assert!(s.sync_requested && !e.waiting());
        assert!(matches!(rx.try_recv(), Ok(Task::Write(text)) if text.contains("Dentista") && !text.contains("Reunión de equipo")));
        assert!(matches!(rx.try_recv(), Ok(Task::Contacts(cards)) if !cards.is_empty()));
        s.sync_requested = false;
        e.checked = None;
        e.tick(&mut s, false);
        assert!(!s.sync_requested, "nothing changed since");
        e.refresh("account_1700000000_0");
        assert!(matches!(rx.try_recv(), Ok(Task::Refresh(id)) if id == "account_1700000000_0"));
    }

    #[test]
    fn events_are_written_and_the_source_once() {
        let root = std::env::temp_dir().join(format!("tinta-gnome-cal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths { ics: root.join("data/tinta-correo/calendar.ics"), sources: root.join("config/evolution/sources") };
        std::fs::create_dir_all(&paths.sources).unwrap();
        write_private(&paths.ics, b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap();
        assert_eq!(std::fs::read(&paths.ics).unwrap(), b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n");
        assert!(!paths.ics.with_extension("ics.part").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&paths.ics).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // No registry running: the key file is written for the next login.
        assert!(register(&paths.source_file(), &source_key_file(&paths.ics), SOURCE_UID, |_, _| false));
        let written = std::fs::read_to_string(paths.source_file()).unwrap();
        assert_eq!(written, source_key_file(&paths.ics));
        // Once there, it is left as it is (the person may have changed its colour in EDS).
        std::fs::write(paths.source_file(), "changed").unwrap();
        assert!(register(&paths.source_file(), &source_key_file(&paths.ics), SOURCE_UID, |_, _| panic!("not asked again")));
        assert_eq!(std::fs::read_to_string(paths.source_file()).unwrap(), "changed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn contacts_go_to_eds_by_uid_and_only_what_changed() {
        use serde_json::json;
        let today = ibercorreo_core::Date::new(2026, 11, 4).unwrap();
        let mut s = Session::new(ibercorreo_engine::sample::store(today).unwrap(), today);
        s.run("contact.add", &json!({"name": "Ana Ruiz", "email": "ana@example.org"})).unwrap();
        let cards = export_contacts(&s.store, false);
        assert_eq!(cards.len(), s.store.all_contacts().len());
        assert!(cards.iter().all(|(uid, card)| !uid.is_empty() && card.contains(&format!("UID:{uid}"))), "{cards:?}");
        let ana = cards.iter().find(|(_, c)| c.contains("FN:Ana Ruiz")).unwrap().clone();
        // Empty book: everything is created.
        let (remove, create, modify) = plan(&[], &cards, &HashMap::new());
        assert!(remove.is_empty() && modify.is_empty() && create.len() == cards.len());
        // All written; then Ana changes and someone else's card is in the book.
        let sent: HashMap<String, String> = cards.iter().cloned().collect();
        let mut existing: Vec<String> = cards.iter().map(|(u, _)| u.clone()).collect();
        existing.push("from-elsewhere".into());
        let mut now = cards.clone();
        if let Some(c) = now.iter_mut().find(|(u, _)| *u == ana.0) {
            c.1 = c.1.replace("Ana Ruiz", "Ana Ruiz Gil");
        }
        let (remove, create, modify) = plan(&existing, &now, &sent);
        assert_eq!(remove, ["from-elsewhere"]);
        assert!(create.is_empty());
        assert_eq!(modify.len(), 1);
        assert!(modify.first().unwrap().contains("Ana Ruiz Gil"));
        // The same UID twice is written once.
        let twice = vec![ana.clone(), ana.clone()];
        assert_eq!(plan(&[], &twice, &HashMap::new()).1.len(), 1);
        // A CardDAV book EDS shows from GNOME's account is left out.
        let book = s
            .store
            .add_book("Nextcloud", Some(ibercorreo_core::CalendarRemote { href: "/remote.php/dav/addressbooks/ana/c/".into(), ctag: None }))
            .unwrap();
        s.run("contact.add", &json!({"name": "Zoe Quintana", "book": book})).unwrap();
        assert!(export_contacts(&s.store, false).iter().any(|(_, c)| c.contains("Zoe Quintana")));
        assert!(!export_contacts(&s.store, true).iter().any(|(_, c)| c.contains("Zoe Quintana")));
    }

    #[test]
    fn the_address_book_source_is_a_local_eds_book() {
        let data = book_key_file();
        assert!(data.starts_with("[Data Source]\n") && data.contains("[Address Book]\nBackendName=local\n"), "{data}");
        assert!(data.contains("Parent=local-stub\n"));
    }

    /// Against a running Evolution Data Server (`TINTA_EDS_TEST=1`, a session bus whose
    /// sources folder has the Tinta Correo book; recipe in docs/gnome.md): contacts reach it,
    /// change and go.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn contacts_reach_a_running_eds() {
        if std::env::var_os("TINTA_EDS_TEST").is_none() {
            return;
        }
        let card = |uid: &str, name: &str| (uid.to_string(), format!("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{name}\r\nEND:VCARD\r\n"));
        let list = || -> Vec<String> {
            let bus = zbus::blocking::Connection::session().unwrap();
            let r = bus
                .call_method(
                    Some("org.gnome.evolution.dataserver.AddressBook10"),
                    "/org/gnome/evolution/dataserver/AddressBookFactory",
                    Some("org.gnome.evolution.dataserver.AddressBookFactory"),
                    "OpenAddressBook",
                    &(BOOK_UID,),
                )
                .unwrap();
            let (path, name): (String, String) = r.body().deserialize().unwrap();
            let book = "org.gnome.evolution.dataserver.AddressBook";
            bus.call_method(Some(name.as_str()), path.as_str(), Some(book), "Open", &()).unwrap();
            let r = bus.call_method(Some(name.as_str()), path.as_str(), Some(book), "GetContactList", &("",)).unwrap();
            let mut cards: Vec<String> = r.body().deserialize().unwrap();
            cards.sort();
            cards
        };
        let mut sent = HashMap::new();
        assert!(mirror_through_eds(&[card("t-1", "Ana Ruiz"), card("t-2", "Rui Carvalho")], &mut sent));
        let got = list();
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.iter().any(|c| c.contains("FN:Ana Ruiz")) && got.iter().any(|c| c.contains("FN:Rui Carvalho")));
        assert!(mirror_through_eds(&[card("t-1", "Ana Ruiz Gil")], &mut sent));
        let got = list();
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got.first().unwrap().contains("FN:Ana Ruiz Gil"));
        assert!(mirror_through_eds(&[], &mut sent));
        assert!(list().is_empty());
    }
}
