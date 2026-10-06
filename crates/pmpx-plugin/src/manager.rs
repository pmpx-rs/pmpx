//! The trait a plugin implements: the one thing a plugin author writes by hand.

use std::ffi::OsString;

use crate::{CommandSpec, Context, Family, PluginError, Verb};

/// An implementation of one package manager backend, a purely synchronous interface: no `async`,
/// no callbacks, no I/O -- passing a `Future` across the `dlopen` boundary is the most fragile
/// part of this approach. It should only do mapping; see the crate docs for the constraints.
///
/// A panic in any of these methods is caught by the `export!` shell -- the `#[unsafe(no_mangle)]`
/// wrapper catches it before it can cross `extern "C"`, which would abort the host process. The
/// host then sees a failed call, or the [`PANIC_MARKER`](crate::shell::PANIC_MARKER) name, and
/// refuses to use the plugin; the panic message itself goes to stderr as usual.
pub trait PackageManager: Send + Sync {
    /// Plugin name, e.g. `"cargo"`. The host compares it against the name declared in the
    /// manifest and refuses to load on a mismatch.
    fn name(&self) -> &str;

    /// The ecosystem it belongs to.
    fn family(&self) -> Family;

    /// Translate "verb + arguments" into one concrete command.
    /// Return [`PluginError::UnsupportedVerb`] when a verb is not supported and do not improvise a
    /// near-equivalent command -- the host degrades `exec`, the other verbs report the error as-is,
    /// and either is better than guessing.
    fn command(
        &self,
        ctx: &Context,
        verb: Verb,
        args: &[OsString],
    ) -> Result<CommandSpec, PluginError>;
}
