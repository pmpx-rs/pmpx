//! Turning an answer into a running process: resolving the program, starting it, and saying what
//! happened.
//!
//! # Who owns what
//!
//! A plugin answers "what to run" and nothing else. Everything about *how* it runs lives here:
//! resolving the real path, deciding which interpreter a script needs, inheriting stdio, waiting, and
//! translating the exit status. That is why there is exactly one implementation of each of those, and
//! why this crate is the only place a `Command` is built.
//!
//! # Windows needs the real path
//!
//! `Command::new("pnpm")` fails on Windows: `CreateProcessW` does a `PATH` lookup and appends `.exe`,
//! and nothing else. npm, pnpm, yarn and bun are all `.cmd` shims there. So the real path is resolved
//! first ([`resolve`]), and the spawn then depends on the kind: `.cmd`/`.bat` go through
//! `std::process`, which builds the `cmd.exe` line, and `.ps1` goes to `pwsh -NoProfile -File`. A
//! failed resolution is a [`EngineError::NotFound`] listing the near-matching names on `PATH`, because
//! "pnpm: not found" without the list is a puzzle rather than a message.
//!
//! # Nothing is printed
//!
//! What a run does is reported as [`Event`]s. The caller decides what to do with them: the CLI turns
//! them into `--debug` lines and the "pmpx -> ..." announcement, and a test asserts the sequence. The
//! process's own stdout and stderr are inherited, so whatever the backend prints goes straight to the
//! person -- this crate never captures it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod backend;
mod command;
mod error;
mod files;
mod log;
mod not_found;
mod resolve;

pub mod discovery;

/// The plugin store: what is installed, and installing more.
///
/// Behind a feature, and **off by default**, because it is the one part of this crate that reaches for
/// the network and the user's disk: an embedder that only runs commands someone else installed should
/// not pay for any of it. `crate-plugin-kit` -- and through it `ureq`, `rustls` and `ring` -- appears in
/// this module and nowhere else in the workspace.
#[cfg(feature = "store")]
pub mod store;

pub use backend::{Backend, Call, PluginIdentity};
pub use command::command_for;
pub use error::EngineError;
pub use files::{Declared, MAX_FILES, MAX_FILE_BYTES};
pub use log::Levels;
pub use resolve::{resolve, ProgramKind, Resolved};

use crate::resolve::resolve as resolve_program;

/// What to run, resolved from whatever a plugin answered or the project defined.
///
/// Deliberately this crate's own type rather than the contract's: execution needs three fields and no
/// knowledge of plugins, and keeping it that way is what lets the whole run path be tested without a
/// plugin at all.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// The executable, as written: a bare name (looked up on `PATH`), an absolute path, or one
    /// relative to [`Plan::cwd`].
    pub program: OsString,
    /// Arguments, in order, handed over verbatim.
    pub args: Vec<OsString>,
    /// Where to run it. `None` means the fallback the caller passes to [`run`].
    pub cwd: Option<PathBuf>,
}

impl Plan {
    /// Name the executable.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
        }
    }

    /// Append one argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append a batch of arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Override the working directory.
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// The working directory this will actually run in.
    pub fn working_dir<'a>(&'a self, fallback: &'a Path) -> &'a Path {
        self.cwd.as_deref().unwrap_or(fallback)
    }

    /// The whole command line, for a message or a test.
    pub fn display(&self) -> String {
        let mut line = self.program.to_string_lossy().into_owned();
        for arg in &self.args {
            line.push(' ');
            line.push_str(&arg.to_string_lossy());
        }
        line
    }
}

/// Something that happened while running one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The program was resolved to something concrete.
    Resolved {
        /// The program, as it was asked for.
        program: OsString,
        /// The path that will be started, when the resolution found one.
        path: Option<PathBuf>,
        /// How it will be started.
        kind: ProgramKind,
    },

    /// About to start it.
    Starting {
        /// The whole command line, so a caller can show exactly what runs.
        plan: Plan,
    },

    /// It finished, with this exit code.
    Finished {
        /// The code pmpx will exit with, already translated.
        code: u8,
    },

    /// Something the person should know even without asking for detail.
    Warning(String),

    /// Detail for someone tracing the run.
    Note(String),

    /// Something went wrong, which did not stop the run either.
    Error(String),

    /// One phase of the run finished, with how long it took.
    ///
    /// The engine measures its own phases so that a caller's trace can attribute the time -- which is
    /// where a run spends everything that is not the backend's own runtime.
    Phase {
        /// The phase's name, stable enough for a caller to key on.
        name: &'static str,
        /// How long it took.
        micros: u128,
        /// What it did, in one line.
        detail: String,
    },

    /// The decision's notes, for whoever shows them: ties, and how to override the choice.
    Notes {
        /// The plugin that was selected.
        plugin: String,
        /// The notes themselves.
        notes: Vec<String>,
    },

    /// The plugin said something while it was being called.
    ///
    /// The level is the contract's number, and the text is exactly what the plugin wrote: how to show
    /// it (an id, a colour, a destination) is the caller's business.
    PluginMessage {
        /// The contract's level number.
        level: u32,
        /// The message itself.
        text: String,
    },
}

/// Really run it: inherit stdio, wait for it to finish, hand back its exit code verbatim.
///
/// `fallback_cwd` is used when the plan names no directory of its own.
///
/// The return value is not `Result<()>`: "the tests failed" and "pmpx failed" are two different
/// things, and `pmpx test` has to pass a nonzero exit code through to the caller's script -- that is
/// the only meaningful contract of this command.
pub fn run(
    plan: &Plan,
    fallback_cwd: &Path,
    events: &mut dyn FnMut(Event),
) -> Result<u8, EngineError> {
    // Resolving is a PATH lookup plus a stat, and failing it is the setup kind of error: the caller
    // decides the exit code, but the message names what was searched.
    let resolved = resolve_program(&plan.program, plan.working_dir(fallback_cwd))?;
    events(Event::Resolved {
        program: plan.program.clone(),
        path: Some(resolved.program.clone()),
        kind: resolved.kind,
    });

    events(Event::Starting { plan: plan.clone() });

    let mut cmd = command::command_for(plan, fallback_cwd)?;
    let status = cmd.status().map_err(|source| EngineError::Start {
        program: plan.program.clone(),
        source,
    })?;

    let code = exit_code_of(status, events);
    events(Event::Finished { code });
    Ok(code)
}

/// Translate an `ExitStatus` into a process exit code.
fn exit_code_of(status: std::process::ExitStatus, events: &mut dyn FnMut(Event)) -> u8 {
    if let Some(code) = status.code() {
        if (0..=255).contains(&code) {
            return code as u8;
        }
        // On Windows an exit code can be any u32 -- `code()` reinterprets those bits as `i32`, so it is
        // reported back as unsigned, or `exit /b -1` would read as "-1" instead of 4294967295. Take the
        // low 8 bits instead of erroring: the user's script cares about "nonzero", not the value.
        events(Event::Warning(format!(
            "the backend exited with {}, which does not fit in 8 bits; passing through the low 8 bits \
             ({})",
            code as u32,
            (code & 0xFF) as u8
        )));
        return (code & 0xFF) as u8;
    }

    // Killed by a signal (Unix). Follow the shell convention: 128 + signal.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            events(Event::Error(format!(
                "the backend was killed by signal {sig}"
            )));
            return (128 + sig).clamp(0, 255) as u8;
        }
    }

    events(Event::Error(
        "cannot read the backend exit code, treating it as 1".to_string(),
    ));
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect the events one run produced.
    fn run_collecting(plan: &Plan, cwd: &Path) -> (Result<u8, EngineError>, Vec<Event>) {
        let mut events = Vec::new();
        let code = run(plan, cwd, &mut |event| events.push(event));
        (code, events)
    }

    #[test]
    fn a_run_reports_what_it_did_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = Plan::new("cargo").arg("--version");

        let (code, events) = run_collecting(&plan, tmp.path());

        assert_eq!(code.unwrap(), 0);
        assert!(
            matches!(&events[0], Event::Resolved { program, kind, .. }
                if program == &OsString::from("cargo") && *kind == ProgramKind::Native),
            "{events:?}"
        );
        assert!(
            matches!(&events[1], Event::Starting { plan }
                if plan.program == *"cargo"
                    && plan.args.len() == 1
                    && plan.cwd.is_none()
                    && plan.working_dir(tmp.path()) == tmp.path()),
            "{events:?}"
        );
        assert_eq!(events.last(), Some(&Event::Finished { code: 0 }));
        assert_eq!(events.len(), 3, "nothing else to report: {events:?}");
    }

    #[test]
    fn runs_a_native_command_and_returns_its_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = Plan::new("cargo").arg("--version");

        assert_eq!(run(&plan, tmp.path(), &mut |_| {}).unwrap(), 0);
    }

    #[test]
    fn a_nonzero_exit_code_is_passed_through() {
        let tmp = tempfile::tempdir().unwrap();

        #[cfg(windows)]
        let plan = Plan::new("cmd").arg("/c").arg("exit 7");
        #[cfg(not(windows))]
        let plan = Plan::new("sh").arg("-c").arg("exit 7");

        let (code, events) = run_collecting(&plan, tmp.path());

        assert_eq!(code.unwrap(), 7, "must pass through verbatim");
        assert_eq!(events.last(), Some(&Event::Finished { code: 7 }));
    }

    /// A program that is not there is the setup kind of failure, and the message lists what was near.
    #[test]
    fn a_missing_program_is_not_found_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = Plan::new("pmpx-definitely-not-a-real-program-xyz");

        let (code, events) = run_collecting(&plan, tmp.path());

        let error = code.expect_err("there is no such program");
        assert!(error.is_not_found(), "{error:?}");
        assert!(
            error
                .message()
                .contains("pmpx-definitely-not-a-real-program-xyz"),
            "{}",
            error.message()
        );
        assert!(
            events.is_empty(),
            "nothing ran, so nothing was reported: {events:?}"
        );
    }

    /// The working directory has to take effect -- both the `cwd` a plugin reports and the project
    /// root rely on it.
    #[test]
    fn the_working_directory_is_honoured() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = Plan::new("cargo").arg("--version").cwd(tmp.path());

        let mut cmd = command_for(&plan, Path::new("/definitely/not/here")).unwrap();
        assert!(cmd.output().unwrap().status.success());
    }

    // ---- really running a cmd shim (Windows) --------------------------------

    /// Really run a `.cmd` and confirm the arguments arrive.
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

        let plan = Plan::new(shim.as_os_str()).arg("add").arg("serde");
        let (code, events) = run_collecting(&plan, tmp.path());

        assert_eq!(code.unwrap(), 0);
        assert!(
            matches!(&events[0], Event::Resolved { kind, .. } if *kind == ProgramKind::CmdShim),
            "a `.cmd` is spawned through cmd.exe: {events:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&out_file).unwrap().trim(),
            "add serde"
        );
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

        let plan = Plan::new(shim.as_os_str());
        assert_eq!(run(&plan, tmp.path(), &mut |_| {}).unwrap(), 0);
    }

    /// An argument with spaces has to arrive verbatim too.
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

        let plan = Plan::new(shim.as_os_str()).arg("hello world");

        assert_eq!(run(&plan, tmp.path(), &mut |_| {}).unwrap(), 0);
        assert_eq!(
            std::fs::read_to_string(&out_file).unwrap().trim(),
            "hello world"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_passes_through_a_nonzero_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("probe.cmd");
        std::fs::write(&shim, "@echo off\r\nexit /b 42\r\n").unwrap();

        let plan = Plan::new(shim.as_os_str());
        assert_eq!(run(&plan, tmp.path(), &mut |_| {}).unwrap(), 42);
    }

    /// `&`, `|`, `>` and `^` are separators, pipes, redirections and escapes to `cmd.exe`, which
    /// re-parses the whole line before the batch file runs.
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
            let plan = Plan::new(shim.as_os_str()).arg(arg);
            assert_eq!(
                run(&plan, tmp.path(), &mut |_| {}).unwrap(),
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
