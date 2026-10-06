//! Running one verb all the way: resolve -> load -> ask the plugin -> run the answer.
//!
//! The order is the whole design, and each step refuses to guess on the previous one's behalf:
//!
//! 1. **The root** comes from the walk-up [`Session`] already did. No root, and no `exec` escape hatch,
//!    means "nothing to run here", said once.
//! 2. **The selection** is made by the decision crate, on scores computed from the plugins' own declared
//!    markers.
//! 3. **The plugin** is loaded and asked. What it says while it runs arrives as events.
//! 4. **The answer** is a plan, and this crate runs it and passes the exit code through.
//!
//! `allow_exec_fallback` is the single exception: it is true only for `exec`, where an unsupported verb
//! degrades into running the user's own command line verbatim. The other six verbs keep "unsupported is
//! an error".

use std::ffi::OsString;
use std::path::Path;

use pmpx_plugin::Verb;

use crate::error::Result;
use crate::files::Declared;
use crate::session::Session;
use crate::{detect, Call, EngineError, Event, Plan};

/// Run one verb all the way.
pub fn run_verb(
    session: &Session,
    verb: Verb,
    args: &[OsString],
    allow_exec_fallback: bool,
    events: &mut dyn FnMut(Event),
) -> Result<u8> {
    let root = match session.project_root() {
        Some(root) => root.to_path_buf(),
        None => {
            // `exec` is the escape hatch: it has to work with zero plugins too, and with no project root
            // it falls back to the start directory.
            if allow_exec_fallback {
                return passthrough(&session.start_dir.clone(), args, events);
            }
            return Err(session.no_project_error());
        }
    };

    let selection = match session.select(&root, events) {
        Ok(selection) => selection,
        Err(failure) => {
            if allow_exec_fallback {
                return passthrough(&root, args, events);
            }
            // Every refusal to choose is a value, and the caller turns it into exit code 3.
            return Err(EngineError::Detect(failure));
        }
    };

    events(Event::Notes {
        plugin: selection.name.clone(),
        notes: selection.notes.clone(),
    });

    let backend = session.load_backend(&selection, events)?;

    // What the plugin declared in its manifest, ready to be read if it asks: the declaration is the
    // allowlist, and nothing on the filesystem is touched on its behalf.
    let files = Declared::new(&root, backend.wanted_files());

    let call = Call {
        root: &root,
        start_dir: &session.start_dir,
        matched: &selection.matched,
        config_files: &session.project.sources,
        pins: &session.project.plugin,
        args,
        verb,
        reason: detect::reason_of(selection.reason),
        score: selection.score,
        files: &files,
    };

    match backend.command(&call, events) {
        Ok(answer) => {
            let plan = plan_of(answer);
            crate::run(&plan, &root, events)
        }

        Err(pmpx_loader::CallError::UnsupportedVerb) if allow_exec_fallback => {
            passthrough(&root, args, events)
        }

        Err(error) => Err(EngineError::Call {
            plugin: selection.name.clone(),
            verb: verb.to_string(),
            error,
        }),
    }
}

/// Run one of the project's own named commands, if it defines one.
///
/// `[scripts]` is a convenience for the person: `pmpx run <name>` looks the name up here first and runs it
/// directly, so a project does not need a plugin at all to have a `pmpx fmt`. `None` means "no such name",
/// and the caller routes it to a plugin instead.
pub fn run_script(
    session: &Session,
    name: &str,
    extra: &[OsString],
    events: &mut dyn FnMut(Event),
) -> Option<Result<u8>> {
    let script = session.project.scripts.get(name)?;

    let mut tokens = script.tokens();
    if tokens.is_empty() {
        return Some(Err(EngineError::Usage(format!(
            "the [scripts] entry \"{name}\" is empty, so there is nothing to run"
        ))));
    }

    let plan = Plan::new(OsString::from(tokens.remove(0)))
        .args(tokens)
        .args(extra.iter());

    // Nothing is asked of a plugin, so this is the whole path: the run is announced from the plan, and the
    // exit code comes straight back.
    let cwd = session
        .project_root()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| session.start_dir.clone());

    Some(crate::run(&plan, &cwd, events))
}

/// Run the user's own command line, verbatim, in `cwd`.
///
/// Only reached for `exec`, or for another verb when `exec` was asked for and nothing could answer: the
/// escape hatch has to work with no plugins, no project, and no detection.
fn passthrough(cwd: &Path, args: &[OsString], events: &mut dyn FnMut(Event)) -> Result<u8> {
    let Some((program, rest)) = args.split_first() else {
        return Err(EngineError::Usage(
            "nothing to run: `pmpx exec` needs a command".to_string(),
        ));
    };

    let plan = Plan::new(program.clone()).args(rest.iter());
    crate::run(&plan, cwd, events)
}

/// A plugin's answer as the engine's plan: the same three fields, and nothing else crosses.
fn plan_of(answer: pmpx_plugin::CommandSpec) -> Plan {
    Plan {
        program: answer.program,
        args: answer.args,
        cwd: answer.cwd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan keeps the plugin's three fields, including a `cwd` of its own.
    #[test]
    fn an_answer_becomes_a_plan_unchanged() {
        let answer = pmpx_plugin::CommandSpec::new("cargo")
            .arg("add")
            .arg("serde")
            .cwd("/work/sub");

        let plan = plan_of(answer);

        assert_eq!(plan.program, OsString::from("cargo"));
        assert_eq!(
            plan.args,
            vec![OsString::from("add"), OsString::from("serde")]
        );
        assert_eq!(plan.cwd, Some(std::path::PathBuf::from("/work/sub")));
        assert_eq!(plan.display(), "cargo add serde");
    }

    /// `exec` without a command is a usage error, not a crash.
    #[test]
    fn an_empty_passthrough_is_a_usage_error() {
        let error = passthrough(Path::new("/tmp"), &[], &mut |_| {}).unwrap_err();

        assert!(matches!(error, EngineError::Usage(_)), "{error:?}");
    }
}
