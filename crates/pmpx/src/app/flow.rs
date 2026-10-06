//! Running one verb all the way: resolve -> load -> ask the plugin -> spawn -> pass the exit code
//! through.

use std::ffi::OsString;
use std::path::Path;

use pmpx_plugin::Verb;

use super::Session;
use crate::debug;
use crate::error::{PmpxError, Result};
use crate::runtime::Invocation;
use crate::spawn;

/// Run one verb all the way: resolve -> load -> ask the plugin -> spawn -> pass the exit
/// code through.
///
/// `allow_exec_fallback` is the single exception: it is true only for `exec`, where an
/// unsupported verb degrades into pmpx passing the command through verbatim itself; the
/// other six verbs keep "unsupported is an error".
pub fn run_verb(
    session: &Session,
    verb: Verb,
    args: &[OsString],
    allow_exec_fallback: bool,
) -> Result<u8> {
    let root = match session.project_root() {
        Some(r) => r.to_path_buf(),
        None => {
            // `exec` is the escape hatch: it has to work with zero plugins too. With no
            // project root it falls back to the start directory.
            if allow_exec_fallback {
                let cwd = session.start_dir.clone();
                return passthrough(&cwd, args, session.quiet);
            }
            return Err(session.no_project_error());
        }
    };

    let selection = match session.select(&root) {
        Ok(s) => s,
        Err(failure) => {
            if allow_exec_fallback {
                return passthrough(&root, args, session.quiet);
            }
            // Every `DetectFailure` is exit code 3
            return Err(failure.into());
        }
    };

    session.emit_notes(&selection);

    let backend = session.load_backend(&selection)?;

    // The evidence the plugin is given is the evidence that selected it -- carried by the
    // resolution itself, not re-read from the filesystem here.
    let t = debug::now();
    // The plugin is told the same facts the host decided on: the evidence that selected it, why it
    // was the one selected, and the project config as it was read. Assembled here rather than
    // re-derived over there.
    // What the plugin declared in its manifest, ready to be read if it asks: the declaration is the
    // allowlist, and nothing on the filesystem is touched on its behalf.
    let files = crate::runtime::Declared::new(&root, backend.wanted_files());

    let invocation = Invocation {
        root: &root,
        start_dir: &session.start_dir,
        matched: &selection.matched,
        reason: crate::detect::reason_of(selection.reason),
        score: selection.score,
        pins: &session.project.plugin,
        config_files: &session.project.sources,
        args,
        files: &files,
    };
    let answer = backend.command(&invocation, verb);
    // Pure mapping on the other side of the ABI: no file is read and no process is started
    // here, so this is the cost of the call itself, not of what it decides.
    debug::done("plugin.command", t, || verb.to_string());

    match answer {
        Ok(spec) => {
            announce(session.quiet, &spec);
            // A `cwd` the plugin set is applied inside `spawn`, so passing the project root here is
            // the fallback, not a decision this layer has to make.
            spawn::run(&spec, &root)
        }

        Err(pmpx_loader::CallError::UnsupportedVerb) if allow_exec_fallback => {
            passthrough(&root, args, session.quiet)
        }

        Err(e) => Err(PmpxError::Backend(
            format!("plugin {} cannot do `{verb}`: {e}", selection.name),
            backend_exit_code(&e),
        )),
    }
}

/// The process exit code for a plugin that answered "I cannot".
///
/// "This backend cannot do what you asked" is a usage error; a plugin that failed, panicked, or
/// answered with something malformed is counted as a pmpx error of its own.
fn backend_exit_code(error: &pmpx_loader::CallError) -> u8 {
    match error {
        pmpx_loader::CallError::UnsupportedVerb
        | pmpx_loader::CallError::InvalidArgs
        | pmpx_loader::CallError::ShortCommand { .. } => crate::error::EXIT_USAGE,
        pmpx_loader::CallError::Internal | pmpx_loader::CallError::Unknown(_) => {
            crate::error::EXIT_INTERNAL
        }
    }
}

/// Pass through verbatim: run the command the user gave, cwd = project root.
///
/// This is the one exception to "unsupported is an error" and happens only for `exec`.
fn passthrough(cwd: &Path, args: &[OsString], quiet: bool) -> Result<u8> {
    let Some((program, rest)) = args.split_first() else {
        return Err(PmpxError::Usage(
            "`pmpx exec` needs a command, for example `pmpx exec ls`".to_string(),
        ));
    };

    let spec = pmpx_plugin::CommandSpec {
        program: program.clone(),
        args: rest.to_vec(),
        cwd: Some(cwd.to_path_buf()),
    };
    announce(quiet, &spec);
    spawn::run(&spec, cwd)
}

/// Show the command that is about to run, unless `--quiet` is set.
fn announce(quiet: bool, spec: &pmpx_plugin::CommandSpec) {
    if quiet {
        return;
    }
    spawn::announce(spec);
}
