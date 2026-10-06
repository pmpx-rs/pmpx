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
//! file reads to probe things it should not know -- "what the project looks like" is decided by
//! the host's detect layer and handed to the plugin through `matched`, a declarative, auditable
//! allowlist.
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
//! Data crossing [`abi`] is always `#[repr(C)]` POD, so the two sides need not share a rustc; see
//! the module docs of [`abi`].
//!
//! The contract's parts live in sibling modules and are re-exported here, so every path that
//! starts with `pmpx_plugin::` is stable: [`PackageManager`] and the [`Context`] it is called
//! with, the [`CommandSpec`] it answers with, the [`Verb`]s, the [`Family`] and the
//! [`PluginError`] it may report.
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod abi;
pub mod debug;

mod context;
mod error;
mod export;
mod family;
mod manager;
mod spec;
mod verb;

pub use context::{Context, SelectionReason};
pub use error::PluginError;
pub use family::Family;
pub use manager::PackageManager;
pub use spec::CommandSpec;
pub use verb::Verb;
