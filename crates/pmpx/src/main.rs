//! pmpx -- one command surface that detects the project type and forwards to the real tool.
//!
//! This binary contains no plugin code: support for cargo / pnpm lives in the plugin crates
//! themselves and is `dlopen`ed at runtime.
//!
//! # Layers
//! ```text
//! main.rs / cli.rs                    process boundary and argument parsing
//! commands.rs / app.rs                subcommand handling and flow orchestration
//! config.rs / discovery.rs            config reading and project root discovery
//! plugins.rs / detect.rs / hints.rs   plugin manifests, resolution, hints with zero plugins
//! runtime.rs / spawn.rs               load the selected plugin, start the process
//! debug.rs                            `--debug`: where each of the above spent its time
//! ```
//!
//! One hard rule: detection never loads any plugin code (`plugins.rs` only reads manifests)
//! -- running `pmpx` in a repo that never installed a plugin executes no third-party code.

#![deny(missing_docs)]
#![warn(clippy::all)]

mod app;
mod cli;
mod commands;
mod config;
mod debug;
mod detect_types;
mod error;
mod runtime;
mod selfupdate;
mod style;

use clap::Parser;

fn main() -> std::process::ExitCode {
    // Before anything else can be timed: every phase reported below is measured from here,
    // and the startup before the first of them (the dynamic loader, the C runtime) is what
    // `total` covers but no phase can.
    debug::mark_start();

    // The only thing that happens before the arguments are parsed. On Windows an update
    // cannot delete the binary it replaced while that binary is still running, so the
    // leftover `.old` is deleted on the next start instead -- silently, because a failure
    // only means "next time". A `.new` that never made it into place goes the same way, on
    // every platform.
    let t = debug::now();
    selfupdate::cleanup_stale_old();
    debug::done("self-update", t, || "stale .old cleanup");

    let t = debug::now();
    let args = cli::Cli::parse();

    // The flag can only be read once parsing is done, which is why turning the trace on is a
    // step of its own rather than something `mark_start` could have decided.
    if args.debug {
        debug::enable();
        debug::header();
    }
    debug::done("cli.parse", t, || "argv -> Cli");

    // Set before any command runs: everything from here on renders through `runtime::render_event`.
    runtime::set_json(args.json);

    let code = dispatch(&args);
    debug::total(code);
    std::process::ExitCode::from(code)
}

/// Run once and return the process exit code -- which may come from the backend.
fn dispatch(args: &cli::Cli) -> u8 {
    // `--json` is a contract, so it is either honoured or refused: a command whose result is a human
    // table says so, rather than quietly mixing prose into the stream a caller is parsing.
    if (args.json || args.explain)
        && !matches!(
            args.command,
            // No subcommand at all is clap printing help, which is not a result a script asked for.
            None | Some(
                cli::Command::Install { .. }
                    | cli::Command::Remove { .. }
                    | cli::Command::Run { .. }
                    | cli::Command::Build { .. }
                    | cli::Command::Test { .. }
                    | cli::Command::Update { .. }
                    | cli::Command::Exec { .. }
            )
        )
    {
        error::error_line(error::PmpxError::Usage(
            "`--json` is not supported by this command yet".to_string(),
        ));
        return error::EXIT_USAGE;
    }

    // `--explain` is answered here, before anything is loaded or run: the report is the whole command.
    if args.explain {
        return match app::explain(args) {
            Ok(code) => code,
            Err(e) => {
                error::error_line(&e);
                e.exit_code()
            }
        };
    }

    match commands::dispatch(args) {
        Ok(code) => code,
        Err(e) => {
            // One shared prefix so the user can spot pmpx's own words inside backend output.
            error::error_line(&e);
            e.exit_code()
        }
    }
}
