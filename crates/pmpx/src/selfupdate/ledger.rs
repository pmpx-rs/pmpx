//! Whether `cargo install` owns this binary.
//!
//! The authority is cargo's own record, not the directory the binary happens to sit in: a
//! `cargo binstall`, a hand-copied release archive and a symlink all land in the same `bin`
//! directory as cargo's own installs, and those are exactly the installations that *should*
//! be able to replace themselves.

use std::fs;
use std::io::ErrorKind;
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
    let json = read_json_record(&dir.join(".crates2.json"));
    let toml = read_toml_record(&dir.join(".crates.toml"));

    // `ListsCrate` wins over everything: one of the two spellings naming this crate is proof.
    if json == OneRecord::ThisCrate || toml == OneRecord::ThisCrate {
        return CargoRecord::ListsCrate;
    }

    // A record that exists but cannot be read or understood is not evidence of absence. Treating
    // it as "not cargo's" is the one mistake this module exists to prevent, so it counts as
    // cargo's instead -- the conservative direction.
    if json == OneRecord::Unreadable || toml == OneRecord::Unreadable {
        return CargoRecord::Unreadable;
    }

    if json == OneRecord::OtherCrates || toml == OneRecord::OtherCrates {
        return CargoRecord::ListsSomethingElse;
    }

    CargoRecord::Absent
}

/// What one of cargo's record files says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OneRecord {
    /// The file is not there at all.
    Missing,
    /// It lists this crate.
    ThisCrate,
    /// It is readable and understood, and lists other crates.
    OtherCrates,
    /// It is there but cannot be read, parsed, or understood.
    Unreadable,
}

/// What current cargo writes: `{"installs": {"pmpx 0.1.0 (registry+...)": {...}}}`.
fn read_json_record(path: &Path) -> OneRecord {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return OneRecord::Missing,
        // Permission denied, not UTF-8, a directory in the file's place: none of them prove the
        // binary is not cargo's.
        Err(_) => return OneRecord::Unreadable,
    };

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return OneRecord::Unreadable;
    };

    // Valid JSON without the container cargo writes: a schema this pmpx does not know, which is
    // again not proof of anything.
    let Some(installs) = json.get("installs").and_then(|value| value.as_object()) else {
        return OneRecord::Unreadable;
    };

    if installs.keys().any(|key| names_this_crate(key)) {
        OneRecord::ThisCrate
    } else {
        OneRecord::OtherCrates
    }
}

/// The older spelling of the same record.
fn read_toml_record(path: &Path) -> OneRecord {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return OneRecord::Missing,
        Err(_) => return OneRecord::Unreadable,
    };

    let Ok(table) = toml::from_str::<toml::Table>(&text) else {
        return OneRecord::Unreadable;
    };

    let Some(v1) = table.get("v1").and_then(|value| value.as_table()) else {
        return OneRecord::Unreadable;
    };

    if v1.keys().any(|key| names_this_crate(key)) {
        OneRecord::ThisCrate
    } else {
        OneRecord::OtherCrates
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

    /// A record that cannot even be read is not evidence that cargo does not own the binary --
    /// concluding `Standalone` from it would overwrite an installation cargo manages.
    #[test]
    fn a_record_that_cannot_be_read_is_treated_as_cargos() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Not valid UTF-8: `read_to_string` fails on it.
        std::fs::write(tmp.path().join(".crates2.json"), [0xff, 0xfe, 0xfd]).unwrap();

        assert_eq!(install_kind(&bin.join("pmpx")), InstallKind::Cargo);
    }

    /// The same for a record that parses but does not have the shape this pmpx knows: a schema
    /// change in cargo must not silently hand the binary over.
    #[test]
    fn a_record_with_an_unknown_shape_is_treated_as_cargos() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(
            tmp.path().join(".crates2.json"),
            r#"{"something_else":{"pmpx 0.1.0":{}}}"#,
        )
        .unwrap();

        assert_eq!(install_kind(&bin.join("pmpx")), InstallKind::Cargo);
    }

    /// An empty-looking `.crates.toml` (no `[v1]`) is the same case as above, one file over.
    #[test]
    fn a_toml_record_without_v1_is_treated_as_cargos() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(tmp.path().join(".crates.toml"), "[other]\nx = 1\n").unwrap();

        assert_eq!(install_kind(&bin.join("pmpx")), InstallKind::Cargo);
    }
}
