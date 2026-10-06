//! Building the [`Command`] for a program that has already been resolved.
//!
//! Kept apart from [`super::run`] for testability: tests need `output()` to capture output, while
//! `run` inherits stdio. One platform difference is left -- a `.ps1` goes through
//! `pwsh -NoProfile -File`. Why `.cmd` / `.bat` is deliberately *not* one of them is the one
//! thing worth reading in [`command_for`].

use std::path::Path;
use std::process::{Command, Stdio};

use pmpx_plugin::CommandSpec;

use super::resolve::{resolve, ProgramKind};
use crate::error::Result;

/// Build a [`Command`] for a [`Resolved`](super::resolve::Resolved) kind, without running it.
///
/// This step is split out for testability: tests need `output()` to capture output, while
/// [`run`](super::run) inherits stdio.
///
/// `cwd` is only the fallback: a working directory the plugin put in the spec **wins**, and it is
/// resolved here rather than by every caller, so a plugin's `cwd` cannot silently depend on the
/// caller remembering to apply it.
pub fn command_for(spec: &CommandSpec, cwd: &Path) -> Result<Command> {
    let resolved = resolve(&spec.program)?;
    let cwd = spec.cwd.as_deref().unwrap_or(cwd);

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
            c.args(&spec.args);
            c
        }

        ProgramKind::PowerShellShim => {
            // `-NoProfile` is deliberate: the user's PowerShell profile should not affect how
            // the package manager behaves, and it can be slow. `-ExecutionPolicy Bypass` is
            // not added -- changing security policy is not pmpx's job, and when a policy
            // blocks it PowerShell should report the real reason itself.
            let mut c = Command::new("pwsh");
            c.arg("-NoProfile").arg("-File").arg(&resolved.program);
            c.args(&spec.args);
            c
        }
    };

    cmd.current_dir(cwd);
    // The environment is inherited verbatim -- pmpx takes no part in proxies / mirrors /
    // registry switching; those are configured in the shell.
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_fails_before_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CommandSpec::new("pmpx-definitely-not-a-real-program-xyz");

        assert!(command_for(&spec, tmp.path()).is_err());
    }

    /// A `cwd` the plugin asked for has to win over the caller's directory, wherever the caller
    /// happens to be looking from.
    #[test]
    fn a_cwd_in_the_spec_overrides_the_callers_directory() {
        let outer = tempfile::tempdir().unwrap();
        let inner = tempfile::tempdir().unwrap();

        let spec = CommandSpec::new("cargo").arg("--version").cwd(inner.path());
        let cmd = command_for(&spec, outer.path()).unwrap();

        assert_eq!(cmd.get_current_dir(), Some(inner.path()));
    }

    /// Without one, the caller's directory is what is used.
    #[test]
    fn without_a_cwd_in_the_spec_the_callers_directory_is_used() {
        let outer = tempfile::tempdir().unwrap();
        let spec = CommandSpec::new("cargo").arg("--version");

        let cmd = command_for(&spec, outer.path()).unwrap();
        assert_eq!(cmd.get_current_dir(), Some(outer.path()));
    }
}
