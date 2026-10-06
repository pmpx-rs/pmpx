//! The host side of the plugin ABI: load a library, agree on capabilities, call it.
//!
//! This crate is the only place in the host that touches plugin memory, and together with
//! `pmpx-plugin-abi` it is the only place that contains `unsafe` at all. Everything above it
//! (`pmpx-engine`, the CLI) works with `String`, `PathBuf` and `Vec<OsString>`.
//!
//! What it deliberately does **not** do:
//!
//! - **No file I/O.** A plugin may ask for the contents of a file it declared; this crate forwards
//!   that question to a [`ContextSource`]'s [`Files`] provider, and the engine answers it (reading
//!   files, with whatever limits it wants, is the engine's decision). That is what keeps the ABI
//!   layer free of file-system policy.
//! - **No policy.** It does not decide which plugin to load, what a verb means, or whether an empty
//!   program is acceptable. It loads, checks, calls, and reports what came back.
//! - **No printing.** Every failure is a typed error with a message; the CLI renders it.
//!
//! # Calling one plugin
//!
//! ```no_run
//! use pmpx_loader::{ContextSource, Plugin};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // SAFETY: loading a dynamic library runs whatever code is inside it.
//! let plugin = unsafe { Plugin::open(std::path::Path::new("/path/to/libpmpx_plugin_x.so"))? };
//! println!("{} ({})", plugin.name(), plugin.family());
//! # Ok(())
//! # }
//! ```
//!
//! # Panics
//!
//! A panic must not cross an `extern "C"` boundary: since Rust 1.81 that aborts the process at the
//! boundary, so a host frame can never catch it. The plugin's own shell wraps every entry point in
//! `catch_unwind`, and that is the only defence there is -- a host that must survive a hostile
//! plugin has to run it out of process, which is out of scope here.
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod context;
mod error;
mod plugin;

pub use context::{ContextSource, Files, NoFiles};
pub use error::{CallError, LoadError};
pub use plugin::{Command, Plugin};
