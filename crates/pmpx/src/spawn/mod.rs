//! Running the command a plugin answered with, as *this* host shows it.
//!
//! The mechanics -- resolving the real path, choosing the interpreter, inheriting stdio, translating
//! the exit status -- live in `pmpx-engine`, which reports them as events. This module is the half that
//! turns those events into what a person sees: the `--debug` trace and the two kinds of note, plus the
//! "pmpx -> ..." line printed before anything starts.

use std::path::Path;
use std::time::Instant;

use pmpx_engine::{EngineError, Event, Plan};
use pmpx_plugin::CommandSpec;

use crate::debug;
use crate::error::{error_line, note_line, PmpxError, Result};
use crate::style;

/// Show the command that is about to run, on stderr.
///
/// It is meant to be called just before [`run`]. stderr rather than stdout: the backend inherits
/// stdout, which the caller may be reading through a pipe. The arguments are printed as they are handed
/// over, without adding quotes of our own -- a line that quoted them would no longer describe what
/// actually runs.
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

/// Really run it, and pass the backend's exit code through verbatim.
///
/// The return value is not `Result<()>`: "the tests failed" and "pmpx failed" are two different things,
/// and `pmpx test` must be able to pass a nonzero exit code to the caller's script.
pub fn run(spec: &CommandSpec, cwd: &Path) -> Result<u8> {
    let plan = plan_for(spec);

    // Two phases, timed apart so the trace can say which one the backend owns: the PATH lookup, and the
    // backend's own runtime.
    let started = Instant::now();
    let mut resolved_at: Option<Instant> = None;
    let mut running_at: Option<Instant> = None;

    let mut render = |event: Event| match event {
        Event::Resolved { path, kind, .. } => {
            let t = started;
            resolved_at = Some(Instant::now());
            debug::done("spawn.resolve", t, || match path {
                Some(path) => format!("{} ({kind:?})", path.display()),
                None => format!("{kind:?}"),
            });
        }
        Event::Starting { program, .. } => {
            running_at = Some(Instant::now());
            let _ = program;
        }
        Event::Finished { code } => {
            let t = running_at.or(resolved_at).unwrap_or(started);
            debug::done("backend.run", t, || format!("exit {code}"));
        }
        // A note the host did not anticipate (the exit code did not fit in 8 bits, a signal, …). It is
        // shown, not swallowed: a silently changed exit code is exactly what a script would trip over.
        Event::Note(text) => note_line(text),
        Event::Error(text) => error_line(text),
    };

    match pmpx_engine::run(&plan, cwd, &mut render) {
        Ok(code) => Ok(code),
        Err(error) => Err(translate(error)),
    }
}

/// A plugin's answer as the engine's plan: the same three fields, and nothing else crosses.
fn plan_for(spec: &CommandSpec) -> Plan {
    Plan {
        program: spec.program.clone(),
        args: spec.args.clone(),
        cwd: spec.cwd.clone(),
    }
}

/// The engine's failure as pmpx's: "there is no such program" is exit code 3, which is the code a
/// script uses to tell an incomplete setup from a broken pmpx.
fn translate(error: EngineError) -> PmpxError {
    if error.is_not_found() {
        PmpxError::NotFound(error.message())
    } else {
        PmpxError::Other(anyhow::anyhow!(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_carries_the_answers_three_fields() {
        let spec = CommandSpec::new("cargo")
            .arg("add")
            .arg("serde")
            .cwd("/work");

        let plan = plan_for(&spec);

        assert_eq!(plan.program, spec.program);
        assert_eq!(plan.args, spec.args);
        assert_eq!(plan.cwd, spec.cwd);
        assert_eq!(plan.display(), "cargo add serde");
    }

    #[test]
    fn a_missing_program_becomes_the_not_found_exit_code() {
        let error = translate(EngineError::NotFound(
            "cannot find executable nope".to_string(),
        ));

        assert_eq!(error.exit_code(), crate::error::EXIT_NOT_FOUND);
        assert!(error.to_string().contains("nope"));
    }

    #[test]
    fn a_start_failure_is_an_internal_error() {
        let error = translate(EngineError::Start {
            program: "nope".into(),
            source: std::io::Error::other("boom"),
        });

        assert_eq!(error.exit_code(), crate::error::EXIT_INTERNAL);
    }

    #[test]
    fn running_really_runs_and_passes_the_code_through() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CommandSpec::new("cargo").arg("--version");

        assert_eq!(run(&spec, tmp.path()).unwrap(), 0);
    }
}
