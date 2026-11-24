//! Attachments out of the mailbox (Tinta): the window's Save and Open, and the file names the MCP
//! server's `save_attachment` uses. Correo keeps the contents of the attachments of mail that
//! arrives ([`ibercorreo_store::files`]); this module writes one to a new file.
//!
//! Writing files is not a command (as for [`crate::export`]): only the window, which the person
//! drives, and the command-line tools call these functions.
//!
//! - **Save** writes to the Downloads folder ([`downloads_dir`]) under the attachment's name made
//!   safe ([`file_name`]); a name already taken gets ` (2)`, ` (3)`… before its extension, so no
//!   file is ever replaced.
//! - **Open** writes a copy to Correo's own `Opened` folder ([`opened_dir`]), one folder per
//!   content, and the shell hands it to the program the system opens that kind of file with.
//!   Programs, scripts and shortcuts ([`risky`]) are never opened: they can only be saved, so the
//!   person decides what to run.

use std::io::Write;
use std::path::{Path, PathBuf};

use ibercorreo_core::{Error, MessageId, Result};
use ibercorreo_store::Store;

/// Most bytes of a file name made from an attachment's name.
const MAX_NAME_BYTES: usize = 200;
/// Most ` (n)` tried before giving up on a free name.
const MAX_TRIES: u32 = 999;

/// A Windows device name (`CON`, `NUL`, `COM1`… also with an extension), which names no file.
pub fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ').to_ascii_uppercase();
    let numbered = |prefix: &str| stem.strip_prefix(prefix).is_some_and(|n| n.len() == 1 && n.bytes().all(|b| (b'1'..=b'9').contains(&b)));
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$") || numbered("COM") || numbered("LPT")
}

/// A file name for an attachment called `name`: its last path part, without characters no file
/// system takes or a leading dot (a hidden file), cut at 200 bytes; `attachment` when nothing is
/// left. Never `.`, `..` or a Windows device name.
pub fn file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut out = String::new();
    for c in last.chars() {
        let c = if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c };
        if out.len().saturating_add(c.len_utf8()) > MAX_NAME_BYTES {
            break;
        }
        out.push(c);
    }
    let out = out.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    if out.is_empty() {
        return "attachment".into();
    }
    if is_device_name(&out) { format!("_{out}") } else { out }
}

/// Extensions of programs, scripts, installers and shortcuts: opening one runs it.
const RISKY: &[&str] = &[
    "ade",
    "adp",
    "app",
    "appimage",
    "application",
    "appref-ms",
    "apk",
    "bat",
    "bin",
    "cab",
    "chm",
    "cmd",
    "com",
    "command",
    "cpl",
    "deb",
    "desktop",
    "diagcab",
    "dll",
    "dmg",
    "elf",
    "exe",
    "flatpak",
    "flatpakref",
    "gadget",
    "hta",
    "inf",
    "ins",
    "iso",
    "img",
    "isp",
    "jar",
    "jnlp",
    "js",
    "jse",
    "ksh",
    "library-ms",
    "lnk",
    "mcf",
    "mht",
    "mhtml",
    "msc",
    "msh",
    "msi",
    "msix",
    "msixbundle",
    "msp",
    "mst",
    "pif",
    "pkg",
    "pl",
    "ps1",
    "ps1xml",
    "ps2",
    "psc1",
    "psm1",
    "py",
    "pyc",
    "pyw",
    "rb",
    "reg",
    "rpm",
    "run",
    "scf",
    "scr",
    "sct",
    "settingcontent-ms",
    "sh",
    "shb",
    "shs",
    "snap",
    "so",
    "svg",
    "sys",
    "terminal",
    "tool",
    "url",
    "vb",
    "vbe",
    "vbs",
    "vhd",
    "vhdx",
    "vsmacros",
    "vsw",
    "webloc",
    "ws",
    "wsc",
    "wsf",
    "wsh",
    "xbap",
    "xll",
    "xnk",
    "zsh",
];

/// Whether opening a file called `name` would run something (a program, a script, an installer,
/// a shortcut, a web page with scripts): such a file is only saved, never opened from Correo.
/// A name with no extension counts as one, since the system decides by its contents.
pub fn risky(name: &str) -> bool {
    let name = file_name(name).to_ascii_lowercase();
    let Some((_, ext)) = name.rsplit_once('.') else { return true };
    let ext = ext.trim();
    ext.is_empty() || RISKY.contains(&ext)
}

/// `dir` joined with `name`, or with `name (2)`, `name (3)`… before the extension: the first that
/// is not there (nor a link of that name).
pub fn free_path(dir: &Path, name: &str) -> Result<PathBuf> {
    let name = file_name(name);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.clone(), String::new()),
    };
    for n in 1..=MAX_TRIES {
        let candidate = if n == 1 { dir.join(&name) } else { dir.join(format!("{stem} ({n}){ext}")) };
        if std::fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    Err(Error::invalid("attachment", format!("no free name for {name} in {}", dir.display())))
}

/// Write `bytes` to the new file `to`: refused if it is there already (or a link of that name),
/// and removed again if the write fails half way.
pub fn write_new(to: &Path, bytes: &[u8]) -> Result<()> {
    let storage = |e: std::io::Error| Error::Storage(format!("{}: {e}", to.display()));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(to).map_err(storage)?;
    if let Err(e) = f.write_all(bytes).and_then(|()| f.sync_all()) {
        drop(f);
        let _ = std::fs::remove_file(to);
        return Err(storage(e));
    }
    Ok(())
}

/// The kept contents of attachment `index` of message `id`, with its name.
pub fn contents(store: &Store, id: MessageId, index: usize) -> Result<(String, Vec<u8>)> {
    let m = store.message(id)?;
    let a = m.attachments.get(index).ok_or_else(|| Error::not_found("attachment", format!("{} of message {}", index, id.0)))?;
    let Some(hash) = &a.sha256 else {
        return Err(Error::invalid(
            "attachment",
            format!("Tinta Correo does not have the contents of {}: it keeps those of mail that arrives from now on", a.name),
        ));
    };
    Ok((a.name.clone(), store.file(hash)?))
}

/// Save attachment `index` of message `id` into `dir` (made if missing), under a free name
/// ([`free_path`]). Returns the file written.
pub fn save_into(store: &Store, id: MessageId, index: usize, dir: &Path) -> Result<PathBuf> {
    let (name, bytes) = contents(store, id, index)?;
    std::fs::create_dir_all(dir).map_err(|e| Error::Storage(format!("{}: {e}", dir.display())))?;
    let to = free_path(dir, &name)?;
    write_new(&to, &bytes)?;
    Ok(to)
}

/// Save attachment `index` of message `id` as the file `to`, a place the person chose in the
/// system's Save dialog (which asked before replacing a file): written beside it under a
/// temporary name first, then put in its place, so a failed write never leaves half a file.
pub fn save_as(store: &Store, id: MessageId, index: usize, to: &Path) -> Result<()> {
    let (_, bytes) = contents(store, id, index)?;
    let storage = |e: std::io::Error| Error::Storage(format!("{}: {e}", to.display()));
    if to.is_dir() {
        return Err(Error::invalid("file", format!("{}: a folder, not a file", to.display())));
    }
    let dir = to.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let shown = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = free_path(dir, &format!(".{shown}.part"))?;
    write_new(&temp, &bytes)?;
    std::fs::rename(&temp, to).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        storage(e)
    })
}

/// Write attachment `index` of message `id` to open it: into `opened` (Correo's `Opened` folder),
/// in a folder of its own named by the start of its SHA-256, so opening it again reuses the copy.
/// A risky file ([`risky`]) is refused. Returns the file to open.
pub fn copy_to_open(store: &Store, id: MessageId, index: usize, opened: &Path) -> Result<PathBuf> {
    let m = store.message(id)?;
    let a = m.attachments.get(index).ok_or_else(|| Error::not_found("attachment", format!("{} of message {}", index, id.0)))?;
    if risky(&a.name) {
        return Err(Error::invalid(
            "attachment",
            format!("{} could run a program, so Tinta Correo does not open it: save it and check it first", a.name),
        ));
    }
    let (name, bytes) = contents(store, id, index)?;
    let hash = ibercorreo_store::files::hash(&bytes);
    let dir = opened.join(hash.get(..16).unwrap_or(&hash));
    let to = dir.join(file_name(&name));
    if std::fs::read(&to).is_ok_and(|on_disk| on_disk == bytes) {
        return Ok(to);
    }
    std::fs::create_dir_all(&dir).map_err(|e| Error::Storage(format!("{}: {e}", dir.display())))?;
    // A copy changed by the program that opened it is kept: the new one goes beside it.
    let to = free_path(&dir, &name)?;
    write_new(&to, &bytes)?;
    Ok(to)
}

/// Where Save puts attachments: the Downloads folder (`Downloads` in the home folder, also on
/// Windows and macOS; the folder `XDG_DOWNLOAD_DIR` names in `user-dirs.dirs` on Linux, as a
/// file manager shows it), else the home folder.
pub fn downloads_dir(env: &dyn Fn(&str) -> Option<PathBuf>, read: &dyn Fn(&Path) -> Option<String>) -> Option<PathBuf> {
    let home = env("USERPROFILE").filter(|_| cfg!(windows)).or_else(|| env("HOME"))?;
    if cfg!(all(unix, not(target_os = "macos"))) {
        let config = env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        if let Some(dir) = read(&config.join("user-dirs.dirs")).and_then(|text| xdg_download(&text, &home)) {
            return Some(dir);
        }
    }
    Some(home.join("Downloads"))
}

/// `XDG_DOWNLOAD_DIR="$HOME/…"` (or an absolute path) from a `user-dirs.dirs` file.
fn xdg_download(text: &str, home: &Path) -> Option<PathBuf> {
    let line = text.lines().map(str::trim).find(|l| l.starts_with("XDG_DOWNLOAD_DIR="))?;
    let value = line.strip_prefix("XDG_DOWNLOAD_DIR=")?.trim().trim_matches('"');
    let dir = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None if value.starts_with('/') => PathBuf::from(value),
        None => return None,
    };
    // `$HOME/` alone means the desktop has no Downloads folder: the home folder itself.
    Some(dir)
}

/// Where Open puts its copies: `Opened` in Correo's data folder.
pub fn opened_dir() -> Option<PathBuf> {
    crate::instance::data_dir().map(|d| d.join("Opened"))
}

/// Tinta: the name and contents of the file at `path`, to attach to a draft ([`attach`]): a
/// regular file no larger than a message can carry ([`crate::commands::MAX_DRAFT_FILES_BYTES`]),
/// checked before it is read.
pub fn read_to_attach(path: &Path) -> Result<(String, Vec<u8>)> {
    let meta = std::fs::metadata(path).map_err(|e| Error::Storage(format!("{}: {e}", path.display())))?;
    if !meta.is_file() {
        return Err(Error::invalid("file", format!("{}: not a file", path.display())));
    }
    let limit = crate::commands::MAX_DRAFT_FILES_BYTES;
    if meta.len() > limit as u64 {
        return Err(Error::TooLarge { what: "attachment", size: usize::try_from(meta.len()).unwrap_or(usize::MAX), limit });
    }
    let bytes = std::fs::read(path).map_err(|e| Error::Storage(format!("{}: {e}", path.display())))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok((file_name(&name), bytes))
}

/// Tinta: attach `bytes`, called `name`, to draft `id` through `message.attach` (undoable, saved
/// like any edit).
pub fn attach(s: &mut crate::Session, id: MessageId, name: &str, bytes: &[u8]) -> Result<()> {
    let data = ibercorreo_mime::base64(bytes);
    s.run("message.attach", &serde_json::json!({ "id": id.0, "name": name, "data": data })).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibercorreo_core::FolderRole;

    #[test]
    fn attachment_names_become_safe_file_names() {
        assert_eq!(file_name("factura 12.pdf"), "factura 12.pdf");
        assert_eq!(file_name("../../.ssh/id_rsa"), "id_rsa");
        assert_eq!(file_name("C:\\Windows\\evil.exe"), "evil.exe");
        assert_eq!(file_name(".bashrc"), "bashrc");
        assert_eq!(file_name(".."), "attachment");
        assert_eq!(file_name(""), "attachment");
        assert_eq!(file_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(file_name("line\nbreak.txt"), "line_break.txt");
        assert_eq!(file_name("NUL.txt"), "_NUL.txt");
        assert_eq!(file_name("com1"), "_com1");
        assert_eq!(file_name("COM10.txt"), "COM10.txt");
        let cut = file_name(&"я".repeat(300));
        assert!(cut.len() <= MAX_NAME_BYTES && cut.chars().all(|c| c == 'я'));
    }

    #[test]
    fn programs_scripts_and_shortcuts_are_risky() {
        for name in ["setup.exe", "INVOICE.PDF.EXE", "run.sh", "x.desktop", "a.lnk", "macro.js", "page.svg", "noext", "dot.", "app.AppImage"] {
            assert!(risky(name), "{name}");
        }
        for name in ["factura.pdf", "presupuesto.xlsx", "foto.JPG", "notas.txt", "zvit.docx", "agenda.ics"] {
            assert!(!risky(name), "{name}");
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tinta-attachments-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The sample Inbox message with the October report (kept) and the spreadsheet (not kept).
    fn sample() -> (Store, MessageId, MessageId) {
        let store = crate::sample::store(ibercorreo_core::Date::new(2026, 11, 14).unwrap()).unwrap();
        let inbox = store.folder_by_role(store.first_account(), FolderRole::Inbox).unwrap();
        let find = |n: &str| store.messages_in(inbox).iter().find(|m| m.attachments.iter().any(|a| a.name == n)).map(|m| m.id).unwrap();
        let (pdf, xlsx) = (find("zvit-zhovten.pdf"), find("presupuesto-t2.xlsx"));
        (store, pdf, xlsx)
    }

    #[test]
    fn save_writes_a_new_file_and_never_replaces_one() {
        let dir = temp("save");
        let (store, pdf, xlsx) = sample();
        let first = save_into(&store, pdf, 0, &dir).unwrap();
        assert_eq!(first, dir.join("zvit-zhovten.pdf"));
        assert_eq!(std::fs::read(&first).unwrap(), crate::sample::sample_pdf());
        std::fs::write(&first, b"mine").unwrap();
        let second = save_into(&store, pdf, 0, &dir).unwrap();
        assert_eq!(second, dir.join("zvit-zhovten (2).pdf"));
        assert_eq!(std::fs::read(&first).unwrap(), b"mine", "the first is left alone");
        assert_eq!(save_into(&store, pdf, 0, &dir).unwrap(), dir.join("zvit-zhovten (3).pdf"));
        // Into a folder that is not there yet.
        assert!(save_into(&store, pdf, 0, &dir.join("new")).unwrap().is_file());
        let e = save_into(&store, xlsx, 0, &dir).unwrap_err().to_string();
        assert!(e.contains("does not have the contents"), "{e}");
        assert!(save_into(&store, pdf, 5, &dir).is_err());
        assert!(save_into(&store, MessageId(999_999), 0, &dir).is_err());
    }

    #[test]
    fn open_reuses_its_copy_and_refuses_programs() {
        let dir = temp("open");
        let (mut store, pdf, _) = sample();
        let a = copy_to_open(&store, pdf, 0, &dir).unwrap();
        assert!(a.starts_with(&dir) && a.ends_with("zvit-zhovten.pdf"));
        assert_eq!(copy_to_open(&store, pdf, 0, &dir).unwrap(), a, "the same copy again");
        std::fs::write(&a, b"edited by the viewer").unwrap();
        let b = copy_to_open(&store, pdf, 0, &dir).unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&a).unwrap(), b"edited by the viewer", "an edited copy is kept");
        // A program is never opened, even when it is kept.
        let hash = store.keep_file(b"MZ".to_vec());
        store.message_mut(pdf).unwrap().attachments.push(ibercorreo_core::mail::Attachment {
            name: "factura.pdf.exe".into(),
            mime_type: "application/octet-stream".into(),
            size: 2,
            sha256: hash,
            content_id: None,
        });
        let e = copy_to_open(&store, pdf, 1, &dir).unwrap_err().to_string();
        assert!(e.contains("could run a program"), "{e}");
        assert!(save_into(&store, pdf, 1, &dir).is_ok(), "it can still be saved");
    }

    #[test]
    fn downloads_follow_the_desktop_setting() {
        let home = PathBuf::from("/home/ana");
        let env = |k: &str| (k == "HOME").then(|| home.clone());
        let none = |_: &Path| None;
        let unix = cfg!(all(unix, not(target_os = "macos")));
        if !cfg!(windows) {
            assert_eq!(downloads_dir(&env, &none), Some(home.join("Downloads")));
        }
        let dirs = |_: &Path| Some("# comment\nXDG_DESKTOP_DIR=\"$HOME/Escritorio\"\nXDG_DOWNLOAD_DIR=\"$HOME/Descargas\"\n".to_string());
        if unix {
            assert_eq!(downloads_dir(&env, &dirs), Some(home.join("Descargas")));
            let abs = |_: &Path| Some("XDG_DOWNLOAD_DIR=\"/data/dl\"".to_string());
            assert_eq!(downloads_dir(&env, &abs), Some(PathBuf::from("/data/dl")));
            let bad = |_: &Path| Some("XDG_DOWNLOAD_DIR=\"relative\"".to_string());
            assert_eq!(downloads_dir(&env, &bad), Some(home.join("Downloads")));
        }
        assert_eq!(downloads_dir(&|_| None, &none), None);
    }

    #[test]
    fn save_as_writes_the_chosen_file_and_replaces_one_there() {
        let mut s = crate::Session::new(Store::new(), ibercorreo_core::Date::EPOCH);
        let id = MessageId(s.run("message.compose", &serde_json::json!({})).unwrap()["id"].as_u64().unwrap());
        attach(&mut s, id, "factura.pdf", b"%PDF-1.7 one").unwrap();
        let dir = std::env::temp_dir().join(format!("correo-save-as-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let to = dir.join("Factura noviembre.pdf");
        std::fs::write(&to, b"old").unwrap();
        save_as(&s.store, id, 0, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"%PDF-1.7 one", "replaced: the dialog asked");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 1, "no temporary file left: {left:?}");
        assert!(save_as(&s.store, id, 0, &dir).is_err(), "a folder is not a file");
        assert!(save_as(&s.store, id, 7, &to).is_err(), "no such attachment");
        assert!(save_as(&s.store, id, 0, &dir.join("missing").join("x.pdf")).is_err(), "no such folder");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
