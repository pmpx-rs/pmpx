//! Running one verb all the way: resolve -> load -> ask the plugin -> spawn -> pass the exit code
//! through.

use std::ffi::OsString;
use std::path::Path;

use pmpx_plugin::Verb;

use super::Session;
use crate::error::{PmpxError, Result};
use crate::runtime::BackendError;
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
    match backend.command(&root, &selection.matched, verb, args) {
        Ok(Ok(spec)) => {
            announce(session.quiet, &spec);
            // A `cwd` the plugin set is applied inside `spawn`, so passing the project root here is
            // the fallback, not a decision this layer has to make.
            spawn::run(&spec, &root)
        }

        Ok(Err(BackendError::UnsupportedVerb)) if allow_exec_fallback => {
            passthrough(&root, args, session.quiet)
        }

        Ok(Err(e)) => Err(PmpxError::Backend(
            format!("{} cannot do `{verb}`: {e}", selection.name),
            e.exit_code(),
        )),

        Err(e) => Err(e),
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
