//! End to end: really run the `pmpx` binary.
//!
//! Unit tests verify each layer on its own; these files verify they are still right once
//! wired together, especially across `dlopen` -- which unit tests can never cover.
//!
//! Two premises:
//!
//! 1. **Sandboxed.** `PMPX_CONFIG_DIR` / `PMPX_DATA_DIR` point pmpx at a temporary directory;
//!    without them these tests would read and write the user's real `~/.config/pmpx` and
//!    `~/.pmpx`.
//! 2. **No dependency on which package manager is installed.** Every verb of the fake plugin
//!    maps to `cargo --version` -- we are running inside `cargo test` right now, so it is
//!    guaranteed to exist.
//!
//! The tests are grouped by area, and every area builds on the sandbox in [`support`].

mod config;
mod detection;
mod plugins;
mod self_update;
mod support;
mod surface;
