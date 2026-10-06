//! Building the [`Command`] for a program that has already been resolved.
//!
//! Kept apart from [`super::run`] for testability: tests need `output()` to capture output, while
//! `run` inherits stdio. One platform difference is left -- a `.ps1` goes through
//! `pwsh -NoProfile -File`. Why `.cmd` / `.bat` is deliberately *not* one of them is the one
//! thing worth reading in [`command_for`].

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{ChildOutput, Plan};

use super::resolve::{resolve, ProgramKind};
use crate::error::{EngineError, Result};

/// Build a [`Command`] for a [`Resolved`](super::resolve::Resolved) kind, without running it.
///
/// This step is split out for testability: tests need `output()` to capture output, while
/// [`run`](super::run) inherits stdio.
///
/// `cwd` is only the fallback: a working directory the plugin put in the spec **wins**, and it is
/// applied here rather than by every caller, so a plugin's `cwd` cannot silently depend on the
/// caller remembering to apply it. It is also decided *before* the program is resolved, so a
/// relative program is looked up in the directory the process will actually run in.
pub fn command_for(plan: &Plan, cwd: &Path, child_output: ChildOutput) -> Result<Command> {
    let cwd = plan.cwd.as_deref().unwrap_or(cwd);
    let resolved = resolve(&plan.program, cwd)?;

    let mut cmd = match resolved.kind {
        // A batch file is started by handing the *file* to `Command`: std knows that a
        // `.cmd` / `.bat` cannot be started by `CreateProcess` directly, and builds the
        // `cmd.exe` line for it -- forcing quotes around every batch argument, doubling inner
        // quotes and neutralising `%`.
        //
        // Building that line here instead was measurably wrong. `cmd.exe` re-parses the whole
        // line before the batch file ever runs, so a hand-built line delivered `a&b` as `a`
        // (the tail ran as a second command), broke the line on `a|b`, expanded `%TEMP%` into
        // a path and ate the caret of `^caret` -- while `pmpx exec` / `run` forward the user's
        // own argv.
        ProgramKind::Native | ProgramKind::CmdShim => {
            let mut c = Command::new(&resolved.program);
            c.args(&plan.args);
            c
        }

        ProgramKind::PowerShellShim => {
            // `-NoProfile` is deliberate: the user's PowerShell profile should not affect how
            // the package manager behaves, and it can be slow. `-ExecutionPolicy Bypass` is
            // not added -- changing security policy is not pmpx's job, and when a policy
            // blocks it PowerShell should report the real reason itself.
            //
            // The interpreter is looked up like any other tool, so a machine without one gets the
            // same exit-3 "not on PATH" error as a missing backend instead of a bare
            // "failed to start pwsh".
            let interpreter = resolve_powershell()?;
            let mut c = Command::new(interpreter);
            c.arg("-NoProfile").arg("-File").arg(&resolved.program);
            c.args(&plan.args);
            c
        }
    };

    cmd.current_dir(cwd);
    // The environment is inherited verbatim -- pmpx takes no part in proxies / mirrors /
    // registry switching; those are configured in the shell.
    cmd.stdin(Stdio::inherit())
        .stdout(match child_output {
            ChildOutput::Inherit => Stdio::inherit(),
            // The child's stdout becomes *our* stderr: live, visible, and out of the JSON stream.
            ChildOutput::OnStderr => our_stderr()?,
        })
        .stderr(Stdio::inherit());

    Ok(cmd)
}

/// pmpx's own stderr, as something a child can write to.
///
/// A duplicated handle rather than a captured pipe, so the backend keeps writing live and keeps
/// believing it has a terminal. Both branches are safe: `try_clone_to_owned` gives an owned
/// descriptor, and `Stdio` takes it from there.
fn our_stderr() -> Result<Stdio> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        return Ok(Stdio::from(
            std::io::stderr()
                .as_fd()
                .try_clone_to_owned()
                .map_err(|error| {
                    EngineError::Setup(format!("cannot redirect the backend output: {error}"))
                })?,
        ));
    }

    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        return Ok(Stdio::from(
            std::io::stderr()
                .as_handle()
                .try_clone_to_owned()
                .map_err(|error| {
                    EngineError::Setup(format!("cannot redirect the backend output: {error}"))
                })?,
        ));
    }

    #[allow(unreachable_code)]
    Ok(Stdio::inherit())
}

/// PowerShell to run a `.ps1` shim with: PowerShell 7 first, then the one Windows ships.
///
/// `.ps1` only ever wins a PATH lookup when `PATHEXT` has been extended with it, so this is a rare
/// path -- but a rare path with an unusable error message is still worth ten lines.
fn resolve_powershell() -> Result<PathBuf> {
    for name in ["pwsh", "powershell"] {
        if let Ok(path) = which::which(name) {
            return Ok(path);
        }
    }

    Err(EngineError::not_found(
        "pmpx runs .ps1 shims with PowerShell, and neither `pwsh` nor `powershell` is on PATH."
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_fails_before_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = Plan::new("pmpx-definitely-not-a-real-program-xyz");

        assert!(command_for(&spec, tmp.path(), ChildOutput::Inherit).is_err());
    }

    /// A `cwd` the plugin asked for has to win over the caller's directory, wherever the caller
    /// happens to be looking from.
    #[test]
    fn a_cwd_in_the_spec_overrides_the_callers_directory() {
        let outer = tempfile::tempdir().unwrap();
        let inner = tempfile::tempdir().unwrap();

        let spec = Plan::new("cargo").arg("--version").cwd(inner.path());
        let cmd = command_for(&spec, outer.path(), ChildOutput::Inherit).unwrap();

        assert_eq!(cmd.get_current_dir(), Some(inner.path()));
    }

    /// Without one, the caller's directory is what is used.
    #[test]
    fn without_a_cwd_in_the_spec_the_callers_directory_is_used() {
        let outer = tempfile::tempdir().unwrap();
        let spec = Plan::new("cargo").arg("--version");

        let cmd = command_for(&spec, outer.path(), ChildOutput::Inherit).unwrap();
        assert_eq!(cmd.get_current_dir(), Some(outer.path()));
    }
}
