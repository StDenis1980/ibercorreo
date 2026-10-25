//! `ibercorreo-cli --version`: the packaging scripts run it as a smoke test of the built binary.

use std::process::Command;

#[test]
fn version_names_the_product_and_the_workspace_version() {
    for arg in ["--version", "version"] {
        let out = Command::new(env!("CARGO_BIN_EXE_ibercorreo-cli")).arg(arg).output().unwrap();
        assert!(out.status.success(), "{arg}: {out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("Tinta Correo {}", env!("CARGO_PKG_VERSION")), "{arg}");
    }
}
