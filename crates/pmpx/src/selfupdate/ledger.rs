//! Whether `cargo install` owns this binary.
//!
//! The authority is cargo's own record, not the directory the binary happens to sit in: a
//! `cargo binstall`, a hand-copied release archive and a symlink all land in the same `bin`
//! directory as cargo's own installs, and those are exactly the installations that *should*
//! be able to replace themselves.

use std::fs;
use std::path::Path;
/// Whether this binary was put here by `cargo install`.
///
/// The authority is **cargo's own record**, not the directory the binary happens to sit in.
/// A `cargo binstall`, a hand-copied release archive and a symlink all land in the same
/// `bin` directory as cargo's own installs, and those are exactly the installations that
/// *should* be able to replace themselves. `cargo install` is the one that keeps a ledger,
/// so the ledger is what decides.
///
/// cargo writes it beside the `bin` directory it installs into: `$CARGO_HOME` holds
/// `.crates2.json` (and `.crates.toml`) with `bin/` next to them, and `cargo install --root
/// DIR` does the same under `DIR`. So both the binary's own directory and the one above it
/// are checked.
pub(super) fn install_kind(exe: &Path) -> InstallKind {
    let Some(dir) = exe.parent() else {
        return InstallKind::Standalone;
    };

    for candidate in [Some(dir), dir.parent()].into_iter().flatten() {
        match cargo_record(candidate) {
            // An unreadable ledger counts as cargo's: it cannot prove the binary is not
            // cargo's, and guessing the other way would overwrite a managed install.
            CargoRecord::ListsCrate | CargoRecord::Unreadable => return InstallKind::Cargo,
            CargoRecord::Absent | CargoRecord::ListsSomethingElse => {}
        }
    }

    InstallKind::Standalone
}

/// What cargo's install record in a directory says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CargoRecord {
    /// No `.crates2.json` and no `.crates.toml` there.
    Absent,
    /// A record that lists this crate.
    ListsCrate,
    /// A record that exists and lists other crates.
    ListsSomethingElse,
    /// A record that exists but cannot be read or parsed.
    Unreadable,
}

pub(super) fn cargo_record(dir: &Path) -> CargoRecord {
    let mut found = false;

    // What current cargo writes: {"installs": {"pmpx 0.1.0 (registry+...)": {...}}}.
    if let Ok(text) = fs::read_to_string(dir.join(".crates2.json")) {
        found = true;
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(json) => {
                let lists = json
                    .get("installs")
                    .and_then(|value| value.as_object())
                    .map(|installs| installs.keys().any(|key| names_this_crate(key)))
                    .unwrap_or(false);
                if lists {
                    return CargoRecord::ListsCrate;
                }
            }
            Err(_) => return CargoRecord::Unreadable,
        }
    }

    // The older spelling of the same record.
    if let Ok(text) = fs::read_to_string(dir.join(".crates.toml")) {
        found = true;
        match toml::from_str::<toml::Table>(&text) {
            Ok(table) => {
                let lists = table
                    .get("v1")
                    .and_then(|value| value.as_table())
                    .map(|v1| v1.keys().any(|key| names_this_crate(key)))
                    .unwrap_or(false);
                if lists {
                    return CargoRecord::ListsCrate;
                }
            }
            Err(_) => return CargoRecord::Unreadable,
        }
    }

    if found {
        CargoRecord::ListsSomethingElse
    } else {
        CargoRecord::Absent
    }
}

/// A key in cargo's record reads `pmpx 0.1.0 (registry+https://...)`.
pub(super) fn names_this_crate(key: &str) -> bool {
    key.split_once(' ')
        .map(|(name, _)| name == env!("CARGO_PKG_NAME"))
        .unwrap_or(false)
}

/// Where this installation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InstallKind {
    /// `cargo install` put it there, and keeps a record of doing so.
    Cargo,
    /// Anything else: the install script, a release archive, a build in a checkout.
    Standalone,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record sits beside `bin/`, which is where `$CARGO_HOME` and
    /// `cargo install --root DIR` both put it.
    #[test]
    fn the_record_beside_the_bin_directory_is_what_makes_it_cargos() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(
            tmp.path().join(".crates2.json"),
            r#"{"installs":{"pmpx 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["pmpx"]}}}"#,
        )
        .unwrap();

        assert_eq!(install_kind(&bin.join("pmpx")), InstallKind::Cargo);
    }

    #[test]
    fn the_older_record_works_too() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(
            tmp.path().join(".crates.toml"),
            "[v1]\n\"pmpx 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)\" = [\"pmpx\"]\n",
        )
        .unwrap();

        assert_eq!(install_kind(&bin.join("pmpx")), InstallKind::Cargo);
    }

    /// A record *in* the binary's own directory counts as well -- that is where an install
    /// with a custom layout can leave it.
    #[test]
    fn a_record_beside_the_binary_itself_counts() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".crates.toml"),
            "[v1]\n\"pmpx 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)\" = [\"pmpx\"]\n",
        )
        .unwrap();

        assert_eq!(install_kind(&tmp.path().join("pmpx")), InstallKind::Cargo);
    }

    /// `cargo binstall` and a hand-copied release archive land in the same directory
    /// without cargo knowing anything about them -- and those are exactly the
    /// installations that should be able to replace themselves.
    #[test]
    fn another_crates_record_does_not_claim_this_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(
            tmp.path().join(".crates2.json"),
            r#"{"installs":{"cargo-bumpp 0.3.2 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["bumpp"]}}}"#,
        )
        .unwrap();

        assert_eq!(install_kind(&bin.join("pmpx")), InstallKind::Standalone);
    }

    /// An unreadable ledger cannot prove the binary is not cargo's, so it is treated as if
    /// it were: overwriting a managed installation is the worse mistake.
    #[test]
    fn an_unreadable_record_is_treated_as_cargos() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(tmp.path().join(".crates2.json"), "not json at all").unwrap();

        assert_eq!(install_kind(&bin.join("pmpx")), InstallKind::Cargo);
    }

    #[test]
    fn no_record_at_all_is_standalone() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();

        assert_eq!(install_kind(&bin.join("pmpx")), InstallKind::Standalone);
    }
}
