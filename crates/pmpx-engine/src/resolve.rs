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
use crate::error::{EngineError, Result};

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
/// - `program` containing a path separator means use it directly, without searching PATH (the
///   plugin pointed at a path). A relative one is resolved against `cwd` -- the directory the
///   command will actually run in -- and not against wherever pmpx happened to be started.
/// - Otherwise go through `which`. On Windows it does PATHEXT resolution, which is exactly
///   what is needed here.
pub fn resolve(program: &OsStr, cwd: &Path) -> Result<Resolved> {
    let as_path = Path::new(program);

    let has_separator = as_path.components().any(|c| {
        matches!(
            c,
            std::path::Component::RootDir | std::path::Component::ParentDir
        )
    }) || program.to_string_lossy().contains(['/', '\\']);

    let path = if has_separator {
        let candidate = if as_path.is_absolute() {
            as_path.to_path_buf()
        } else {
            cwd.join(as_path)
        };

        if candidate.is_file() {
            candidate
        } else {
            return Err(EngineError::not_found(format!(
                "cannot find {}: there is no file at {}.",
                as_path.display(),
                candidate.display()
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

    /// With a path separator it is used directly, without searching PATH -- relative to the
    /// directory the command runs in.
    #[test]
    fn an_explicit_path_is_used_as_is() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("thing");
        std::fs::write(&p, "").unwrap();

        let r = resolve(p.as_os_str(), tmp.path()).unwrap();
        assert_eq!(r.program, p);
    }

    /// A relative program belongs to the directory the process will run in: a plugin that says
    /// `node_modules/.bin/tsc` means the project's, not the one pmpx was started from. (Nothing in
    /// this crate changes its own directory, so the two can differ.)
    #[test]
    fn a_relative_program_is_resolved_against_the_given_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("tool"), "").unwrap();

        let r = resolve(OsStr::new("./tool"), &work).unwrap();
        assert!(r.program.is_file(), "{:?}", r.program);
        assert_eq!(r.program.file_name().unwrap(), "tool");

        // The same name against a directory that does not hold it fails, which is what shows the
        // answer came from the given directory rather than from this process's own.
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        assert!(resolve(OsStr::new("./tool"), &elsewhere).is_err());
    }

    #[test]
    fn an_explicit_path_that_does_not_exist_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("nope").join("thing");

        let err = resolve(p.as_os_str(), tmp.path()).unwrap_err();
        assert!(
            err.is_not_found(),
            "a missing program is the setup kind: {err:?}"
        );
        assert!(
            err.to_string().contains("nope"),
            "the message should name what it looked for: {err}"
        );
    }

    /// `cargo` is guaranteed to resolve right now -- we are running inside `cargo test`.
    #[test]
    fn resolves_a_real_program_from_path() {
        let tmp = tempfile::tempdir().unwrap();
        let r = resolve(OsStr::new("cargo"), tmp.path()).expect("cargo must be on PATH");
        assert!(r.program.is_absolute(), "{:?}", r.program);
    }

    #[test]
    fn a_missing_program_is_reported_as_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let err = resolve(
            OsStr::new("pmpx-definitely-not-a-real-program-xyz"),
            tmp.path(),
        )
        .unwrap_err();
        assert!(
            err.is_not_found(),
            "a missing program is the setup kind: {err:?}"
        );
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
