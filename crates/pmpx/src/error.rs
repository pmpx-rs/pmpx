//! Error types and exit codes.
//!
//! Exit codes are an interface user scripts can rely on (`pmpx test || echo failed` must
//! work), so they are part of the error type instead of being guessed from a matched string
//! in `main`.
//!
//! The only exception is a backend process exit code, which is passed through verbatim:
//! that is not an error path but a normal outcome, so it travels as `Ok(code)`.

/// pmpx's success/failure result.
pub type Result<T> = std::result::Result<T, PmpxError>;

/// Exit code 0: success.
pub const EXIT_OK: u8 = 0;
/// Exit code 1: a pmpx error of its own (config parse failure, file lock timeout, plugin
/// removal conflict, and so on).
pub const EXIT_INTERNAL: u8 = 1;
/// Exit code 2: usage error / unsupported verb.
pub const EXIT_USAGE: u8 = 2;
/// Exit code 3: no project type detected / no plugin can handle the family / backend
/// executable not found.
pub const EXIT_NOT_FOUND: u8 = 3;

/// A pmpx error.
///
/// Each variant maps to one class of exit code produced by pmpx itself; a backend exit code
/// is not an error, see the module docs.
#[derive(Debug, thiserror::Error)]
pub enum PmpxError {
    /// Usage error, or the plugin explicitly does not support this verb.
    ///
    /// "Unsupported" belongs here too: the verb the user typed is valid, this backend cannot
    /// do it.
    #[error("{0}")]
    Usage(String),

    /// No project type detected / no plugin can handle the family / backend executable not
    /// found.
    ///
    /// The common thread is that pmpx itself is fine and the environment is missing
    /// something; the message carries what to do next.
    #[error("{0}")]
    NotFound(String),

    /// pmpx itself failed. This is the catch-all; normally it should not be reached.
    #[error(transparent)]
    Other(#[from] anyhow::Error),

    /// A failure reported by the **backend**; the exit code is the backend's, not pmpx's.
    ///
    /// It cannot be folded into [`PmpxError::Usage`]: then "the plugin does not support this
    /// verb" and "the plugin blew up" would be indistinguishable, and scripts need to handle
    /// them separately.
    #[error("{0}")]
    Backend(String, u8),
}

impl PmpxError {
    /// Which code this error should make the process exit with.
    pub fn exit_code(&self) -> u8 {
        match self {
            PmpxError::Usage(_) => EXIT_USAGE,
            PmpxError::NotFound(_) => EXIT_NOT_FOUND,
            PmpxError::Other(_) => EXIT_INTERNAL,
            PmpxError::Backend(_, code) => *code,
        }
    }

    /// Build an "the environment is missing something" error.
    pub fn not_found(message: impl Into<String>) -> Self {
        PmpxError::NotFound(message.into())
    }
}

/// A failure reported by the plugin store (installing, removing, reading a manifest, a crates.io
/// query) lands in [`PmpxError::Other`] — but as a **wrapped error**, not as a formatted string.
///
/// `KitError` carries `#[source]` detail (`ManifestParse` on a broken manifest, `PluginInUse` on a
/// library still loaded, and so on); turning it into text at the boundary would throw that away for
/// every later reader. The message shown to the user is unchanged either way.
impl From<crate_plugin_kit::KitError> for PmpxError {
    fn from(e: crate_plugin_kit::KitError) -> Self {
        PmpxError::Other(anyhow::Error::new(e))
    }
}
