//! Load the selected plugin and call it across the ABI.
//!
//! Validation happens before the call, not at load time: `crate-plugin-kit`'s `load()` only
//! guarantees "it can be `dlopen`ed and the entry symbol can be fetched"; whether this
//! plugin can talk to this host is decided by pmpx -- right after getting the vtable it
//! checks `abi_version == ABI_VERSION` (equality is enough, not full crate version equality)
//! and that `name()` matches what the manifest declared.
//!
//! Every call into the plugin is wrapped in `catch_unwind` on **both** sides: the `extern "C"`
//! shell that the plugin-side `export!` generates is the main defence (it covers `name`, `family`
//! and `command` alike), and this side wraps one more layer for the cases where the plugin forgot
//! to, or was built with `panic = "abort"`. A panic that crosses `extern "C"` since Rust 1.81
//! aborts outright and the host cannot rescue it, which is why the plugin side cannot be skipped.
//!
//! [`backend`] is the loaded plugin, [`error`] the failures it can report, and [`strings`] the
//! reading of the memory it hands back.

mod backend;
mod error;
mod files;
mod log;
mod strings;

pub use backend::{Backend, Invocation};
pub use error::BackendError;
pub(crate) use files::read as read_context_files;
pub(crate) use log::{hooks, set_current_plugin};

/// What the plugin reports about itself, as shown by `pmpx info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendDiagnostics {
    /// The plugin's self-reported name.
    pub name: String,
    /// The family the plugin reports for itself.
    pub family: String,
    /// The rustc that compiled it.
    pub rustc_version: String,
    /// The target triple it was compiled for.
    pub target: String,
}
