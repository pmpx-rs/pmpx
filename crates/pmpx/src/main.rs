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
mod detect;
mod discovery;
mod error;
mod hints;
mod plugins;
mod runtime;
mod selfupdate;
mod spawn;
mod style;

use clap::Parser;

fn main() -> std::process::ExitCode {
    // The only thing that happens before the arguments are parsed. On Windows an update
    // cannot delete the binary it replaced while that binary is still running, so the
    // leftover `.old` is deleted on the next start instead -- silently, because a failure
    // only means "next time". It compiles to nothing on other platforms.
    selfupdate::cleanup_stale_old();

    let args = cli::Cli::parse();
    let code = dispatch(&args);
    std::process::ExitCode::from(code)
}

/// Run once and return the process exit code -- which may come from the backend.
fn dispatch(args: &cli::Cli) -> u8 {
    match commands::dispatch(args) {
        Ok(code) => code,
        Err(e) => {
            // One shared prefix so the user can spot pmpx's own words inside backend output.
            anstream::eprintln!(
                "{} {}",
                style::paint(style::ERROR, "pmpx:"),
                style::paint(style::ERROR_BODY, &e)
            );
            e.exit_code()
        }
    }
}
