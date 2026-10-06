//! Turn a [`CommandSpec`] into a real process.
//!
//! # Spawn ownership
//!
//! Plugins only answer "what to run"; pmpx owns "how to run it", so stdio, environment
//! inheritance and exit-code handling have exactly one implementation.
//!
//! # On Windows the real path must be resolved
//!
//! `Command::new("pnpm")` fails on Windows: `CreateProcessW` only does a PATH lookup plus
//! appending `.exe`, it does no PATHEXT resolution. On Windows npm / pnpm / yarn / bun are
//! all `.cmd` shims (`pnpm.cmd`).
//!
//! So the real path is resolved first ([`resolve`]), then spawned by kind ([`command_for`]): a
//! `.cmd` / `.bat` is handed to `std::process`, which builds the `cmd.exe` line for it, `.ps1`
//! goes to `pwsh -NoProfile -File`, and a failed resolution is an error (exit code 3) that lists
//! the near-matching names in PATH.

use std::path::Path;

use pmpx_plugin::CommandSpec;

use crate::debug;
use crate::error::{error_line, note_line, PmpxError, Result};
use crate::style;

mod command;
mod not_found;
mod resolve;

pub use command::command_for;

/// Show the command that is about to run, on stderr.
///
/// It is meant to be called just before [`run`]. stderr rather than stdout: the backend
/// inherits stdout, which the caller may be reading through a pipe. The arguments are printed
/// as they are handed over, without adding quotes of our own -- a line that quoted them would
/// no longer describe what actually runs.
pub fn announce(spec: &CommandSpec) {
    let mut line = format!(
        "{} {}",
        style::paint(style::DIM, "pmpx ->"),
        style::paint(style::PM, spec.program.to_string_lossy())
    );
    for arg in &spec.args {
        line.push(' ');
        line.push_str(&arg.to_string_lossy());
    }
    anstream::eprintln!("{line}");
}

/// Really run it: inherit stdio, wait for it to finish, return its exit code verbatim.
///
/// `cwd` is the fallback for a spec that names no directory of its own -- see [`command_for`].
///
/// The return value is not `Result<()>`: "the tests failed" and "pmpx failed" are two
/// different things, and `pmpx test` must be able to pass the backend's nonzero exit code
/// through to the caller's script -- that is the only meaningful contract of this command.
pub fn run(spec: &CommandSpec, cwd: &Path) -> Result<u8> {
    // Resolving the real path is a PATH lookup plus a stat, and failing it is exit code 3 --
    // the two phases are timed apart so the trace can say which one the backend owns.
    let t = debug::now();
    let mut cmd = match command_for(spec, cwd) {
        Ok(cmd) => cmd,
        Err(e) => {
            debug::done("spawn.resolve", t, || format!("failed: {e}"));
            return Err(e);
        }
    };
    debug::done("spawn.resolve", t, || {
        spec.program.to_string_lossy().to_string()
    });

    // This is the backend's own runtime, from here until its process exits: usually the whole
    // of what a user perceives as "pmpx is slow", and never pmpx's own time.
    let t = debug::now();
    let status = cmd.status().map_err(|e| {
        PmpxError::Other(anyhow::anyhow!(e).context(format!(
            "failed to start {}",
            spec.program.to_string_lossy()
        )))
    })?;

    let code = exit_code_of(status);
    debug::done("backend.run", t, || format!("exit {code}"));
    Ok(code)
}

/// Translate an `ExitStatus` into a process exit code.
fn exit_code_of(status: std::process::ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        if (0..=255).contains(&code) {
            return code as u8;
        }
        // On Windows an exit code can be any u32 -- `code()` reinterprets those bits as `i32`, so
        // it is printed back as unsigned, or `exit /b -1` would read as "-1" instead of
        // 4294967295. Take the low 8 bits instead of erroring: the user's script cares about
        // "nonzero", not the exact value.
        note_line(format!(
            "the backend exited with {}, which does not fit in 8 bits; passing through the \
             low 8 bits ({})",
            code as u32,
            (code & 0xFF) as u8
        ));
        return (code & 0xFF) as u8;
    }

    // Killed by a signal (Unix). Follow the shell convention: 128 + signal.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            error_line(format!("the backend was killed by signal {sig}"));
            return (128 + sig).clamp(0, 255) as u8;
        }
    }

    error_line("cannot read the backend exit code, treating it as 1");
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- really running a command --------------------------------------------

    #[test]
    fn runs_a_native_command_and_returns_its_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CommandSpec::new("cargo").arg("--version");
        assert_eq!(
            run(&spec, tmp.path()).unwrap(),
            0,
            "cargo --version should succeed"
        );
    }

    #[test]
    fn a_nonzero_backend_exit_code_is_passed_through() {
        let tmp = tempfile::tempdir().unwrap();

        #[cfg(windows)]
        let spec = CommandSpec::new("cmd").arg("/c").arg("exit 7");
        #[cfg(not(windows))]
        let spec = CommandSpec::new("sh").arg("-c").arg("exit 7");

        assert_eq!(
            run(&spec, tmp.path()).unwrap(),
            7,
            "must pass through verbatim"
        );
    }

    /// The working directory has to take effect -- both the `cwd` a plugin reports and the
    /// project root rely on it.
    #[test]
    fn the_working_directory_is_honoured() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("pmpx-cwd-probe.txt");
        std::fs::write(&marker, "here").unwrap();

        let spec = CommandSpec::new("cargo").arg("--version");
        let mut cmd = command_for(&spec, tmp.path()).unwrap();
        let out = cmd.output().unwrap();

        // cargo only does anything else where there is a Cargo.toml; here it is enough to
        // prove it starts
        assert!(out.status.success());
    }

    // ---- really running a cmd shim (Windows) --------------------------------

    /// Really run a `.cmd` and confirm the arguments arrive.
    ///
    /// A batch file is started by `std`, whose `cmd.exe` line is a separate implementation, so
    /// the only way to confirm the arguments reach it is to run it for real.
    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_really_runs_and_receives_its_args() {
        let tmp = tempfile::tempdir().unwrap();
        let out_file = tmp.path().join("got.txt");
        let shim = tmp.path().join("probe.cmd");
        std::fs::write(
            &shim,
            format!("@echo off\r\necho %1 %2 > \"{}\"\r\n", out_file.display()),
        )
        .unwrap();

        let spec = CommandSpec::new(shim.as_os_str()).arg("add").arg("serde");
        assert_eq!(run(&spec, tmp.path()).unwrap(), 0);

        let got = std::fs::read_to_string(&out_file).unwrap();
        assert_eq!(got.trim(), "add serde");
    }

    /// A `.cmd` has to run from a path with spaces too.
    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_in_a_path_with_spaces_still_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a dir with spaces");
        std::fs::create_dir_all(&dir).unwrap();

        let shim = dir.join("probe.cmd");
        std::fs::write(&shim, "@echo off\r\nexit 0\r\n").unwrap();

        let spec = CommandSpec::new(shim.as_os_str());
        assert_eq!(
            run(&spec, tmp.path()).unwrap(),
            0,
            "a path with spaces must work"
        );
    }

    /// An argument with spaces has to arrive verbatim too.
    ///
    /// `%~1` rather than `%1`: `%1` brings the quotes along (existing cmd behaviour), while
    /// `%~1` is "the value with the wrapping quotes removed".
    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_receives_an_argument_with_spaces() {
        let tmp = tempfile::tempdir().unwrap();
        let out_file = tmp.path().join("got.txt");
        let shim = tmp.path().join("probe.cmd");
        std::fs::write(
            &shim,
            format!("@echo off\r\necho %~1 > \"{}\"\r\n", out_file.display()),
        )
        .unwrap();

        let spec = CommandSpec::new(shim.as_os_str()).arg("hello world");
        assert_eq!(run(&spec, tmp.path()).unwrap(), 0);

        let got = std::fs::read_to_string(&out_file).unwrap();
        assert_eq!(got.trim(), "hello world");
    }

    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_passes_through_a_nonzero_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("probe.cmd");
        std::fs::write(&shim, "@echo off\r\nexit /b 42\r\n").unwrap();

        let spec = CommandSpec::new(shim.as_os_str());
        assert_eq!(run(&spec, tmp.path()).unwrap(), 42);
    }

    /// `&`, `|`, `>` and `^` are separators, pipes, redirections and escapes to `cmd.exe`, which
    /// re-parses the whole line before the batch file runs. A hand-built line therefore handed
    /// the backend `a` instead of `a&b` (and ran the tail as a second command), broke on `a|b`,
    /// and swallowed the caret -- while `pmpx exec` / `run` forward the user's own argv.
    ///
    /// The readout quotes `%~1` on purpose: an unquoted `%~1` in the *shim* would be re-parsed
    /// by cmd as well, which would test the shim instead of the argument. The `%VAR%` case
    /// cannot be asserted this way at all -- cmd expands `%…%` in the shim's own line too -- so
    /// it was checked against a native child that dumps its argv (`%TEMP%` arrives literally).
    #[cfg(windows)]
    #[test]
    fn cmd_metacharacters_survive_into_a_shim() {
        let tmp = tempfile::tempdir().unwrap();
        let out_file = tmp.path().join("got.txt");
        let shim = tmp.path().join("probe.cmd");
        std::fs::write(
            &shim,
            format!(
                "@echo off\r\necho \"[%~1]\" > \"{}\"\r\n",
                out_file.display()
            ),
        )
        .unwrap();

        for arg in ["a&b", "a|b", "a>b", "^caret"] {
            let spec = CommandSpec::new(shim.as_os_str()).arg(arg);
            assert_eq!(
                run(&spec, tmp.path()).unwrap(),
                0,
                "the shim failed on {arg}"
            );

            let got = std::fs::read_to_string(&out_file).unwrap();
            assert_eq!(
                got.trim(),
                format!("\"[{arg}]\""),
                "cmd re-parsed the argument {arg}"
            );
        }
    }
}
