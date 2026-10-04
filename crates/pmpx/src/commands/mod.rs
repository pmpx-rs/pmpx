//! Subcommand handling.
//!
//! This layer only does "read the context -> do the work -> print"; the decision logic lives
//! in [`crate::app`] and below. [`dispatch`] is the whole router; each family of subcommands
//! lives in its own file:
//!
//! | Module | Subcommands |
//! | --- | --- |
//! | [`info`] | nothing, and `info` -- what pmpx sees in this directory |
//! | [`plugin`] | `plugin ls / current` -- which plugin answers |
//! | [`plugin_pin`] | `plugin set / unset` -- pinning that choice in `.pmpx.toml` |
//! | [`plugin_store`] | `plugin add / rm / update / search / info` -- the plugin store and crates.io |
//! | [`config`] | `config get / set` -- the global config file |
//!
//! `self` and `completion` stay here: each is a handful of lines of glue with no state of its
//! own, and a file apiece would be more index than content.

use std::ffi::OsString;

use clap::CommandFactory;

use crate::app::{self, Session};
use crate::cli::{Cli, Command, SelfCommand};
use crate::error::{PmpxError, EXIT_OK};

mod config;
mod info;
mod plugin;
mod plugin_pin;
mod plugin_store;

use self::config::config_cmd;
use self::info::{show_detection, show_info};
use self::plugin::plugin_cmd;

/// Dispatch by argv.
pub fn dispatch(args: &Cli) -> crate::error::Result<u8> {
    let Some(command) = &args.command else {
        return show_detection(args);
    };

    match command {
        Command::Install { packages } => {
            run_verb(args, pmpx_plugin::Verb::Install, packages.clone(), false)
        }
        Command::Remove { packages } => {
            run_verb(args, pmpx_plugin::Verb::Remove, packages.clone(), false)
        }
        Command::Update { packages } => {
            run_verb(args, pmpx_plugin::Verb::Update, packages.clone(), false)
        }
        Command::Build { args: rest } => {
            run_verb(args, pmpx_plugin::Verb::Build, rest.clone(), false)
        }
        Command::Test { args: rest } => {
            run_verb(args, pmpx_plugin::Verb::Test, rest.clone(), false)
        }

        Command::Run { target, args: rest } => {
            // `pmpx run <target> -- <args>`: the target and what follows `--` go to the
            // plugin together; how to arrange them is up to the plugin (cargo, for example,
            // uses `cargo run -- ...`).
            let argv: Vec<OsString> = target.iter().cloned().chain(rest.iter().cloned()).collect();
            run_verb(args, pmpx_plugin::Verb::Run, argv, false)
        }

        // `exec` is the only verb allowed to degrade
        Command::Exec { command } => run_verb(args, pmpx_plugin::Verb::Exec, command.clone(), true),

        Command::Info => show_info(args),

        Command::Plugin(sub) => plugin_cmd(args, sub),
        Command::Config(sub) => config_cmd(sub),
        Command::Completion { shell } => completion(*shell),
        Command::SelfTool { command } => self_cmd(command),
    }
}

/// The shared verb entry point: open a session and hand over to [`app::run_verb`].
fn run_verb(
    args: &Cli,
    verb: pmpx_plugin::Verb,
    argv: Vec<OsString>,
    allow_exec_fallback: bool,
) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;
    app::run_verb(&session, verb, &argv, allow_exec_fallback)
}

// No subcommand: show the detection result

/// `pmpx self update`: the one command that writes pmpx's own binary.
///
/// It deliberately does not open a [`Session`]: updating must work when the plugin store
/// or the project configuration is what is broken.
fn self_cmd(cmd: &SelfCommand) -> crate::error::Result<u8> {
    match cmd {
        SelfCommand::Update {
            check,
            version,
            force,
        } => crate::selfupdate::run(&crate::selfupdate::Request {
            check: *check,
            version: version.clone(),
            force: *force,
        }),
    }
}

// completion

fn completion(shell: clap_complete::Shell) -> crate::error::Result<u8> {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    // Print to stdout and let the user redirect -- pmpx does not guess which file to write.
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
    Ok(EXIT_OK)
}
