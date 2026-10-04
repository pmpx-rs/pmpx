//! Finding the real executable behind a bare command name, and classifying it.
//!
//! # On Windows the real path must be resolved
//!
//! `Command::new("pnpm")` fails on Windows: `CreateProcessW` only does a PATH lookup plus
//! appending `.exe`, it does no PATHEXT resolution. On Windows npm / pnpm / yarn / bun are
//! all `.cmd` shims (`pnpm.cmd`).
//!
//! So the real path is resolved here first, and a failed lookup becomes an error (exit code 3)
//! carrying the PATH hints of [`super::not_found`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::not_found::not_found_error;
use crate::error::{PmpxError, Result};

/// The kind of backend executable that was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramKind {
    /// A real executable. Spawned directly.
    Native,
    /// `.cmd` / `.bat`. Needs `cmd.exe` as the interpreter.
    CmdShim,
    /// `.ps1`. Needs PowerShell.
    PowerShellShim,
}

/// A resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The real path.
    pub program: PathBuf,
    /// Which kind it is.
    pub kind: ProgramKind,
}

/// Resolve the real path of a backend executable.
///
/// - `program` containing a path separator means use it directly, without searching PATH
///   (the user pointed at a path explicitly).
/// - Otherwise go through `which`. On Windows it does PATHEXT resolution, which is exactly
///   what is needed here.
pub fn resolve(program: &OsStr) -> Result<Resolved> {
    let as_path = Path::new(program);

    let has_separator = as_path.components().any(|c| {
        matches!(
            c,
            std::path::Component::RootDir | std::path::Component::ParentDir
        )
    }) || program.to_string_lossy().contains(['/', '\\']);

    let path = if has_separator {
        if as_path.is_file() {
            as_path.to_path_buf()
        } else {
            return Err(PmpxError::not_found(format!(
                "cannot find {}. It looks like a path, but there is no file there.",
                as_path.display()
            )));
        }
    } else {
        which::which(program).map_err(|_| not_found_error(program))?
    };

    Ok(Resolved {
        kind: kind_of(&path),
        program: path,
    })
}

/// Decide how to spawn based on the extension.
fn kind_of(path: &Path) -> ProgramKind {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "cmd" | "bat" => ProgramKind::CmdShim,
        "ps1" => ProgramKind::PowerShellShim,
        _ => ProgramKind::Native,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- kind classification -------------------------------------------------

    #[test]
    fn exe_and_extensionless_are_native() {
        assert_eq!(kind_of(Path::new("C:/x/cargo.exe")), ProgramKind::Native);
        assert_eq!(kind_of(Path::new("/usr/bin/cargo")), ProgramKind::Native);
        assert_eq!(kind_of(Path::new("C:/x/tool.bin")), ProgramKind::Native);
    }

    #[test]
    fn cmd_and_bat_are_shims() {
        assert_eq!(kind_of(Path::new("C:/x/pnpm.cmd")), ProgramKind::CmdShim);
        assert_eq!(kind_of(Path::new("C:/x/old.bat")), ProgramKind::CmdShim);
    }

    /// Extension matching is case-insensitive -- `.CMD` and `.cmd` are the same thing on
    /// Windows.
    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(kind_of(Path::new("C:/x/PNPM.CMD")), ProgramKind::CmdShim);
        assert_eq!(
            kind_of(Path::new("C:/x/Run.Ps1")),
            ProgramKind::PowerShellShim
        );
    }

    #[test]
    fn ps1_needs_powershell() {
        assert_eq!(
            kind_of(Path::new("C:/x/x.ps1")),
            ProgramKind::PowerShellShim
        );
    }

    // ---- resolution ----------------------------------------------------------

    /// With a path separator it is used directly, without searching PATH.
    #[test]
    fn an_explicit_path_is_used_as_is() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("thing");
        std::fs::write(&p, "").unwrap();

        let r = resolve(p.as_os_str()).unwrap();
        assert_eq!(r.program, p);
    }

    #[test]
    fn an_explicit_path_that_does_not_exist_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("nope").join("thing");

        let err = resolve(p.as_os_str()).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_NOT_FOUND);
        assert!(err.to_string().contains("path"));
    }

    /// `cargo` is guaranteed to resolve right now -- we are running inside `cargo test`.
    #[test]
    fn resolves_a_real_program_from_path() {
        let r = resolve(OsStr::new("cargo")).expect("cargo must be on PATH");
        assert!(r.program.is_absolute(), "{:?}", r.program);
    }

    #[test]
    fn a_missing_program_gives_exit_code_three() {
        let err = resolve(OsStr::new("pmpx-definitely-not-a-real-program-xyz")).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_NOT_FOUND);
    }

    /// `which` does PATHEXT resolution on Windows, which is how `pnpm` resolves to
    /// `pnpm.cmd`.
    ///
    /// This test asks `which` directly (using `which_in` with an explicit search directory)
    /// rather than going through [`resolve`]: `resolve` reads the process-level PATH, and
    /// mutating that variable in concurrently running tests is unsafe.
    #[cfg(windows)]
    #[test]
    fn which_does_pathex_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("pmpxprobe.cmd");
        std::fs::write(&shim, "@echo off\r\n").unwrap();

        let found = which::which_in("pmpxprobe", Some(tmp.path()), tmp.path())
            .expect("which should find the .cmd via PATHEXT");

        assert_eq!(
            found
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase(),
            "pmpxprobe.cmd"
        );
        assert_eq!(kind_of(&found), ProgramKind::CmdShim);
    }
}
