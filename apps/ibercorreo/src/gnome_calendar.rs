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
//! The writing happens on a thread of its own; the window only compares its events with the last
//! ones written, once a second.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use ibercorreo_core::calendar::Event;
use ibercorreo_engine::Session;

/// The EDS source's id and file name.
pub const SOURCE_UID: &str = "tinta-correo";
/// How often the window looks for changed events.
const EVERY: Duration = Duration::from_secs(1);

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

/// The window's side: notices changed events and hands them to the writer thread.
pub struct Exporter {
    tx: Sender<String>,
    last: Option<Vec<Event>>,
    checked: Option<Instant>,
}

impl Exporter {
    /// Starts the writer when EDS is in use here; `None` otherwise (other desktops, Windows,
    /// macOS, no home folder).
    pub fn start() -> Option<Exporter> {
        let paths = Paths::from_env()?;
        if !paths.sources.is_dir() {
            return None;
        }
        let (tx, rx) = channel::<String>();
        std::thread::Builder::new()
            .name("gnome-calendar".into())
            .spawn(move || {
                let mut registered = false;
                while let Ok(mut ics) = rx.recv() {
                    // Only the newest of a burst of changes.
                    while let Ok(newer) = rx.try_recv() {
                        ics = newer;
                    }
                    if write_private(&paths.ics, ics.as_bytes()).is_err() {
                        continue;
                    }
                    if !registered {
                        registered = register(&paths, create_through_registry);
                    }
                }
            })
            .ok()?;
        Some(Exporter { tx, last: None, checked: None })
    }

    /// Each frame: once a second, writes the events again if they changed.
    pub fn tick(&mut self, session: &Session) {
        if self.checked.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.checked = Some(Instant::now());
        let events = session.store.events();
        if self.last.as_deref() == Some(events) {
            return;
        }
        self.last = Some(events.to_vec());
        let _ = self.tx.send(session.store.export_ics(None));
    }
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

/// Makes sure EDS knows the calendar: `true` once it does (or will at the next login).
fn register(paths: &Paths, create: fn(&str) -> bool) -> bool {
    let file = paths.source_file();
    if file.exists() {
        return true;
    }
    let data = source_key_file(&paths.ics);
    if create(&data) && file.exists() {
        return true;
    }
    write_private(&file, data.as_bytes()).is_ok()
}

/// `SourceManager.CreateSources` on the running source registry: it writes the key file and
/// shows the calendar at once.
#[cfg(all(unix, not(target_os = "macos")))]
fn create_through_registry(data: &str) -> bool {
    let Ok(bus) = zbus::blocking::Connection::session() else { return false };
    let sources = std::collections::HashMap::from([(SOURCE_UID, data)]);
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
fn create_through_registry(_data: &str) -> bool {
    false
}

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
        assert!(register(&paths, |_| false));
        let written = std::fs::read_to_string(paths.source_file()).unwrap();
        assert_eq!(written, source_key_file(&paths.ics));
        // Once there, it is left as it is (the person may have changed its colour in EDS).
        std::fs::write(paths.source_file(), "changed").unwrap();
        assert!(register(&paths, |_| panic!("not asked again")));
        assert_eq!(std::fs::read_to_string(paths.source_file()).unwrap(), "changed");
        let _ = std::fs::remove_dir_all(&root);
    }
}
