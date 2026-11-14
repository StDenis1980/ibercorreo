//! The folders `ibercorreo-cli mcp` may write to (`--automation-write-root`, Tinta, Vilna OS
//! request tintaoffice-web#33): where `save_attachment` puts a file. The rules follow the other
//! Tinta apps' `--automation-write-root` (Tinta Texto's `file_access.rs`), so a host such as Vilna
//! OS's Herkos passes the same flags to all of them:
//!
//! - Each flag may be given more than once, as `--flag DIR` or `--flag=DIR`; every folder must
//!   exist when the server starts, and is compared with its links followed.
//! - A file is written only when the folder it goes in, its links followed, lies inside one of the
//!   roots, compared by whole components (`/work2` is not inside `/work`), without regard to case
//!   on Windows. So `..` or a link that leads out is refused, and so is a missing folder.
//! - A file that is there already (or a link, even a broken one) is never replaced.
//! - The file is written beside its place under a temporary name, then linked into place in one
//!   step that fails if the name was taken meanwhile, so no one sees half a file.
//!
//! A path is checked, then written: a program that swaps a folder for a link in between can still
//! win that race. The roots keep an assistant to the folders it was given; they do not guard
//! against other programs on the computer.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Parse `--automation-write-root` / `--automation-read-root` (`--flag DIR` or `--flag=DIR`).
/// `Ok(None)` when `arg` is neither; the folder named otherwise, with whether it is the write one.
pub fn consume(arg: &str, args: &mut dyn Iterator<Item = &String>) -> Result<Option<(bool, PathBuf)>, String> {
    let (flag, inline) = arg.split_once('=').map_or((arg, None), |(flag, value)| (flag, Some(value)));
    let write = match flag {
        "--automation-write-root" => true,
        "--automation-read-root" => false,
        _ => return Ok(None),
    };
    let value = match inline {
        Some(value) => value.to_string(),
        None => {
            let value = args.next().ok_or_else(|| format!("{flag} needs a folder"))?;
            if value.starts_with('-') {
                return Err(format!("{flag} needs a folder; use {flag}=<dir> for a folder starting with '-'"));
            }
            value.clone()
        }
    };
    if value.is_empty() {
        return Err(format!("{flag} needs a non-empty folder"));
    }
    Ok(Some((write, PathBuf::from(value))))
}

/// A root folder: as compared (absolute, links followed) and as shown to people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    resolved: PathBuf,
    shown: PathBuf,
}

impl Root {
    /// The folder at `path`, which must exist.
    pub fn open(path: &Path, flag: &str) -> Result<Root, String> {
        let resolved = std::fs::canonicalize(path)
            .and_then(|p| if p.is_dir() { Ok(p) } else { Err(std::io::ErrorKind::NotADirectory.into()) })
            .map_err(|e| format!("{flag} `{}`: {e}", path.display()))?;
        Ok(Root { shown: plain(resolved.clone()), resolved })
    }

    pub fn shown(&self) -> &Path {
        &self.shown
    }
}

/// `path` without Windows' `\\?\` prefix (which `canonicalize` adds), for people to read.
fn plain(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

/// Is `path` (resolved) `root` or inside it, component by component?
fn inside(path: &Path, root: &Path) -> bool {
    let mut path = path.components();
    root.components().all(|r| path.next().is_some_and(|p| same_component(p, r)))
}

fn same_component(a: Component, b: Component) -> bool {
    if a == b {
        return true;
    }
    if !cfg!(windows) {
        return false;
    }
    match (a.as_os_str().to_str(), b.as_os_str().to_str()) {
        (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
        _ => false,
    }
}

/// A Windows device name (`CON`, `NUL`, `COM1`… also with an extension), which names no file.
fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ').to_ascii_uppercase();
    let numbered = |prefix: &str| stem.strip_prefix(prefix).is_some_and(|n| n.len() == 1 && n.bytes().all(|b| (b'1'..=b'9').contains(&b)));
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$") || numbered("COM") || numbered("LPT")
}

/// Most bytes of a file name made from an attachment's name.
const MAX_NAME_BYTES: usize = 200;

/// A file name for an attachment called `name`: its last path part, without characters no file
/// system takes or a leading dot (a hidden file), cut at [`MAX_NAME_BYTES`]; `attachment` when
/// nothing is left. Never `.`, `..` or a Windows device name.
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

/// Where `path` (absolute, or relative to the first root) would be written, after checking that it
/// lies in one of `roots` and is free. A folder inside a root takes `name` ([`file_name`]).
pub fn target(roots: &[Root], path: Option<&str>, name: &str) -> Result<PathBuf, String> {
    let first = roots.first().ok_or("saving files is off on this server: start it with --automation-write-root FOLDER")?;
    let shown_roots = || roots.iter().map(|r| r.shown.display().to_string()).collect::<Vec<_>>().join(", ");
    let given = match path {
        None => first.resolved.clone(),
        Some("") => return Err("`path` is empty".into()),
        Some(p) if cfg!(windows) && Path::new(p).components().any(|c| matches!(c, Component::Normal(n) if is_device_name(&n.to_string_lossy()))) => {
            return Err(format!("{p}: reserved device names are not allowed"));
        }
        Some(p) => first.resolved.join(p),
    };
    // A folder that is there takes the attachment's own name.
    let given = if std::fs::metadata(&given).is_ok_and(|m| m.is_dir()) { given.join(file_name(name)) } else { given };
    let shown = path.map_or_else(|| plain(given.clone()).display().to_string(), str::to_string);
    let (Some(folder), Some(leaf)) = (given.parent(), given.file_name()) else {
        return Err(format!("{shown}: not a file name"));
    };
    let folder = std::fs::canonicalize(folder).map_err(|e| format!("{shown}: the folder it goes in cannot be used ({e}); it must exist"))?;
    let entry = folder.join(leaf);
    if !roots.iter().any(|r| inside(&entry, &r.resolved)) {
        return Err(format!("{shown}: outside the write root{} {}", if roots.len() > 1 { "s" } else { "" }, shown_roots()));
    }
    if std::fs::symlink_metadata(&entry).is_ok() {
        return Err(format!("{}: a file of that name is there already, and it is never replaced: give another `path`", plain(entry).display()));
    }
    Ok(entry)
}

/// `to` as people read it (without Windows' `\\?\` prefix).
pub fn shown(to: &Path) -> String {
    plain(to.to_path_buf()).display().to_string()
}

/// Write `bytes` to `to`, which must not exist: through a temporary file beside it, linked into
/// place in one step (copied in, on a file system without links).
pub fn write_new(to: &Path, bytes: &[u8]) -> Result<(), String> {
    let shown = plain(to.to_path_buf());
    let folder = to.parent().ok_or_else(|| format!("{}: not a file name", shown.display()))?;
    let tmp = folder.join(format!(".tinta-correo-{}-{}.tmp", std::process::id(), crate::mcp::unique()));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| format!("{}: {e}", shown.display()))?;
    let written = f.write_all(bytes).and_then(|()| f.sync_all());
    drop(f);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", shown.display()));
    }
    let linked = std::fs::hard_link(&tmp, to);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(format!("{}: a file of that name appeared meanwhile, and it is never replaced", shown.display()))
        }
        // No links here (some file systems and portals): write in place, still never replacing.
        Err(_) => {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(to).map_err(|e| format!("{}: {e}", shown.display()))?;
            if let Err(e) = f.write_all(bytes).and_then(|()| f.sync_all()) {
                drop(f);
                let _ = std::fs::remove_file(to);
                return Err(format!("{}: {e}", shown.display()));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let long = "я".repeat(300);
        let cut = file_name(&long);
        assert!(cut.len() <= MAX_NAME_BYTES && cut.chars().all(|c| c == 'я'));
    }

    #[test]
    fn root_flags_take_both_spellings_and_refuse_bad_operands() {
        let rest = ["out".to_string(), "x".to_string()];
        let mut it = rest.iter();
        assert_eq!(consume("--automation-write-root", &mut it).unwrap(), Some((true, PathBuf::from("out"))));
        assert_eq!(consume("--automation-read-root=in", &mut it).unwrap(), Some((false, PathBuf::from("in"))));
        assert_eq!(consume("--sample", &mut it).unwrap(), None);
        assert_eq!(it.next().map(String::as_str), Some("x"));
        assert!(consume("--automation-write-root=", &mut std::iter::empty()).is_err());
        assert!(consume("--automation-write-root", &mut std::iter::empty()).is_err());
        let flag = ["--sample".to_string()];
        assert!(consume("--automation-write-root", &mut flag.iter()).is_err());
        assert_eq!(consume("--automation-write-root=-dir", &mut std::iter::empty()).unwrap(), Some((true, PathBuf::from("-dir"))));
    }
}
