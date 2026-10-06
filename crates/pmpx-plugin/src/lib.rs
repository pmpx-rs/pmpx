//! # pmpx-plugin
//!
//! The pmpx plugin contract: one trait plus a stable C ABI that carries the trait safely across
//! the `dlopen` boundary.
//!
//! A plugin author only implements [`PackageManager`] and then uses the one-line
//! [`export!`](macro@crate::export) to generate the whole C ABI shell:
//!
//! ```ignore
//! pub fn create() -> Box<dyn PackageManager> { Box::new(CargoPlugin) }
//! pmpx_plugin::export!(create);
//! ```
//! # What a plugin may and may not do
//!
//! [`PackageManager::command`] should only map from its inputs: it does not read files (including
//! anything under `project_root`), does not write files, does not read environment variables,
//! does not spawn child processes, and does not make network requests. That keeps `command()`
//! completely pure (unit tests need no fixture directory at all) and stops a plugin from using
//! file reads to probe things it should not know.
//!
//! "What the project looks like" reaches a plugin through its **manifest**, in two declarative,
//! auditable forms: the file *names* in `[detect]`, handed over as [`Context::matched`], and the
//! file *contents* declared in `[context] files`, asked for one at a time through [`Context::file`]
//! (or [`Context::file_str`]). A plugin that needs to know what a lockfile pins, or whether
//! `package.json` mentions `packageManager`, declares that file and parses it itself; it never
//! reaches for the filesystem, and pmpx never learns what is inside.
//!
//! Declaring a file is the *allowlist*, not a delivery: a name the manifest did not declare is
//! never readable, and a file nobody asks for is never read.
//! # Saying something
//!
//! A plugin that wants to explain itself calls [`debug!`](macro@crate::debug) /
//! [`info!`](macro@crate::info) / [`warn!`](macro@crate::warn) / [`error!`](macro@crate::error),
//! and the host decides what to print, adding the plugin's id. That is not only tidier than
//! printing directly: since the *host* holds the switch, `--debug` never becomes an input a plugin
//! could branch on, so a debug run executes the same command as any other. See
//! [`debug`](mod@crate::debug) for the details, including what happens with no host installed (a
//! plugin's own `cargo test`).
//!
//! Data crossing the boundary is always `#[repr(C)]` POD, so the two sides need not share a rustc;
//! see the module docs of [`abi`]. A host asks the plugin for the **capabilities** it supports --
//! `identity`, `command`, and optionally `attach` -- and passes a context whose every value is read
//! through an accessor, which is what lets either side gain a key or a capability without a new
//! contract version. [`shell`] is the plugin's end of that; `pmpx-loader` is the host's.
//!
//! The contract's parts live in sibling modules and are re-exported here, so every path that
//! starts with `pmpx_plugin::` is stable: [`PackageManager`] and the [`Context`] it is called
//! with, the [`CommandSpec`] it answers with, the [`Verb`]s, the [`Family`] and the
//! [`PluginError`] it may report.
#![deny(missing_docs)]
#![warn(clippy::all)]

/// The wire format, for a plugin that needs to talk to it directly (a hand-written shell, a test).
pub use pmpx_plugin_abi as abi;

pub mod debug;
pub mod shell;

mod context;
mod error;
mod export;
mod family;
mod manager;
mod spec;
mod verb;

pub use context::{Context, ContextBuilder, ContextFile, SelectionReason};
pub use error::PluginError;
pub use family::Family;
pub use manager::PackageManager;
pub use spec::CommandSpec;
pub use verb::Verb;
