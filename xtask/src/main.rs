//! `cargo xtask <task>`: the quality gate and the checks it runs.
//!
//! - `ci`: fmt, clippy `-D warnings`, tests, layers, assets, wasm (in that order).
//! - `layers`: no crate depends on a crate of a higher layer, and nothing below L6 on egui/eframe.
//! - `assets`: every file under `assets/` and `packaging/` that is not code has a row in ATTRIBUTION.md.
//! - `wasm`: the crates below the UI build for `wasm32-unknown-unknown`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Layer of each workspace crate (see README §Architecture).
const LAYERS: &[(&str, u8)] = &[
    ("ibercorreo-core", 0),
    ("ibercorreo-mime", 1),
    ("ibercorreo-ical", 1),
    ("ibercorreo-store", 2),
    ("ibercorreo-imap", 2),
    ("ibercorreo-smtp", 2),
    ("ibercorreo-net", 3),
    ("ibercorreo-sync", 3),
    ("ibercorreo-engine", 4),
    ("ibercorreo-ui-egui", 6),
    ("ibercorreo", 7),
    ("ibercorreo-cli", 7),
];
const UI_CRATES: &[&str] = &["egui", "eframe", "winit", "rfd", "egui-winit"];
const WASM_CRATES: &[&str] = &[
    "ibercorreo-core",
    "ibercorreo-mime",
    "ibercorreo-ical",
    "ibercorreo-store",
    "ibercorreo-imap",
    "ibercorreo-smtp",
    "ibercorreo-sync",
    "ibercorreo-engine",
    "ibercorreo-ui-egui",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

fn cargo(args: &[&str]) -> Result<(), String> {
    eprintln!("$ cargo {}", args.join(" "));
    let status =
        Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())).args(args).current_dir(root()).status().map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("cargo {} failed", args.join(" "))) }
}

/// Dependency names in a Cargo.toml's `[dependencies]` tables (not dev- or build-dependencies).
fn dependencies(manifest: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_deps = l == "[dependencies]" || (l.starts_with("[target.") && l.ends_with(".dependencies]"));
            continue;
        }
        if in_deps && let Some((name, _)) = l.split_once('=') {
            // `egui.workspace = true` names `egui`.
            let name = name.trim().trim_matches('"').split('.').next().unwrap_or("");
            if !name.is_empty() && !name.starts_with('#') {
                out.push(name.to_string());
            }
        }
    }
    out
}

fn crate_name(manifest: &str) -> Option<String> {
    manifest.lines().find_map(|l| l.trim().strip_prefix("name = ").map(|n| n.trim_matches('"').to_string()))
}

fn layers() -> Result<(), String> {
    let mut errors = Vec::new();
    for dir in ["crates", "apps"] {
        let entries = std::fs::read_dir(root().join(dir)).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let Ok(manifest) = std::fs::read_to_string(entry.path().join("Cargo.toml")) else { continue };
            let Some(name) = crate_name(&manifest) else { continue };
            let Some(&(_, layer)) = LAYERS.iter().find(|(n, _)| *n == name) else {
                errors.push(format!("{name}: not in xtask's LAYERS table"));
                continue;
            };
            for dep in dependencies(&manifest) {
                if let Some(&(_, dl)) = LAYERS.iter().find(|(n, _)| *n == dep)
                    && dl >= layer
                    && dep != name
                {
                    errors.push(format!("{name} (L{layer}) depends on {dep} (L{dl})"));
                }
                if layer < 6 && UI_CRATES.contains(&dep.as_str()) {
                    errors.push(format!("{name} (L{layer}) depends on UI crate {dep}"));
                }
            }
        }
    }
    if errors.is_empty() {
        eprintln!("layers: ok");
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn assets() -> Result<(), String> {
    let attribution = std::fs::read_to_string(root().join("ATTRIBUTION.md")).map_err(|e| format!("ATTRIBUTION.md: {e}"))?;
    let mut files = Vec::new();
    walk(&root().join("assets"), &mut files);
    walk(&root().join("packaging"), &mut files);
    let code = ["py", "sh", "rs", "toml", "md"];
    let mut missing = Vec::new();
    for f in files {
        if f.extension().and_then(|e| e.to_str()).is_some_and(|e| code.contains(&e)) {
            continue;
        }
        let rel = f.strip_prefix(root()).unwrap_or(&f).to_string_lossy().replace('\\', "/");
        if !attribution.contains(&format!("`{rel}`")) {
            missing.push(rel);
        }
    }
    if missing.is_empty() {
        eprintln!("assets: ok");
        Ok(())
    } else {
        Err(format!("assets without an ATTRIBUTION.md row:\n{}", missing.join("\n")))
    }
}

fn wasm() -> Result<(), String> {
    let mut args = vec!["check", "--target", "wasm32-unknown-unknown"];
    for c in WASM_CRATES {
        args.push("-p");
        args.push(c);
    }
    cargo(&args)
}

fn ci() -> Result<(), String> {
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])?;
    cargo(&["test", "--workspace"])?;
    layers()?;
    assets()?;
    wasm()
}

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "ci" => ci(),
        "layers" => layers(),
        "assets" => assets(),
        "wasm" => wasm(),
        _ => Err("usage: cargo xtask ci | layers | assets | wasm".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_normal_dependencies() {
        let m = "[package]\nname = \"x\"\n[dependencies]\negui.workspace = true\nfoo = \"1\"\n[dev-dependencies]\nbar = \"1\"\n[target.'cfg(unix)'.dependencies]\nbaz = \"1\"\n";
        assert_eq!(dependencies(m), ["egui", "foo", "baz"]);
    }
}
