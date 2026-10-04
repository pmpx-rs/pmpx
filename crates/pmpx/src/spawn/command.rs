//! Building the [`Command`] for a program that has already been resolved.
//!
//! Kept apart from [`super::run`] for testability: tests need `output()` to capture output, while
//! `run` inherits stdio. The platform differences all live here: a `.cmd` / `.bat` goes through
//! `cmd /d /s /c`, a `.ps1` through `pwsh -NoProfile -File`, and everything else is spawned
//! directly.

use std::path::Path;
use std::process::{Command, Stdio};

use pmpx_plugin::CommandSpec;

#[cfg(windows)]
use super::quoting::cmd_raw_command_line;
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
        ProgramKind::Native => {
            let mut c = Command::new(&resolved.program);
            c.args(&spec.args);
            c
        }

        ProgramKind::CmdShim => {
            #[cfg(windows)]
            {
                // The whole command line has to be built here, and it must go through
                // `raw_arg`.
                //
                // Passing the arguments one by one does not work: `cmd /c` has its own
                // quoting rules and strips the leading and trailing quotes of the whole
                // string under some conditions, so it has to be
                // `cmd /d /s /c "<path> <args>"` -- the outer pair is left there for `/s`
                // to strip.
                // Passing the built string through `arg` does not work either: it adds
                // another layer of quotes and escapes our `"` into `\"`, which cmd no longer
                // recognises. This needs verbatim delivery, which is what `raw_arg` exists
                // for.
                use std::os::windows::process::CommandExt;

                let mut c = Command::new("cmd");
                c.raw_arg(cmd_raw_command_line(&resolved.program, &spec.args));
                c
            }

            #[cfg(not(windows))]
            {
                // `.cmd` / `.bat` are Windows-only forms and there is no cmd.exe here.
                // But `kind_of` still classifies them as CmdShim (the classification is
                // cross-platform), so this branch has to exist. If it is really reached, the
                // program is started as an ordinary program -- it will fail with a system
                // error such as Exec format error, and that is the truth.
                let mut c = Command::new(&resolved.program);
                c.args(&spec.args);
                c
            }
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
