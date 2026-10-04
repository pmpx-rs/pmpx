//! The three errors a plugin can report.
//!
//! Deliberately only three: that is what the host needs to distinguish -- the verb is not
//! supported (so it can degrade to passing through verbatim), the arguments are wrong, and
//! everything else. The human-readable description travels in the payload.

use std::fmt;

use crate::abi;
use crate::Verb;

/// The errors a plugin can report.
/// Deliberately only three -- because the host only needs to distinguish three: the verb is not
/// supported (so it can degrade to passing through verbatim), the arguments are wrong, and
/// everything else. The human-readable description goes in the payload and the host prints it to
/// stderr as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginError {
    /// This backend does not support that verb. For `pmpx exec` the host degrades to passing
    /// through verbatim.
    UnsupportedVerb(Verb),

    /// Invalid arguments.
    InvalidArgs(String),

    /// Anything else. Includes a plugin panic -- the host only needs to know "it blew up".
    Other(String),
}

impl PluginError {
    /// Construct "unsupported verb".
    pub fn unsupported_verb(verb: Verb) -> Self {
        PluginError::UnsupportedVerb(verb)
    }

    /// Construct "invalid arguments".
    pub fn invalid_args(message: impl Into<String>) -> Self {
        PluginError::InvalidArgs(message.into())
    }

    /// Construct some other error.
    pub fn other(message: impl Into<String>) -> Self {
        PluginError::Other(message.into())
    }

    /// The corresponding cross-boundary error code.
    pub fn code(&self) -> u32 {
        match self {
            PluginError::UnsupportedVerb(_) => abi::PMPX_ERR_UNSUPPORTED_VERB,
            PluginError::InvalidArgs(_) => abi::PMPX_ERR_INVALID_ARGS,
            PluginError::Other(_) => abi::PMPX_ERR_INTERNAL,
        }
    }

    /// Human-readable description.
    pub fn message(&self) -> String {
        match self {
            PluginError::UnsupportedVerb(v) => format!("unsupported verb {v}"),
            PluginError::InvalidArgs(m) => m.clone(),
            PluginError::Other(m) => m.clone(),
        }
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

// Hand-written rather than thiserror: this crate is deliberately dependency-free, and all `Error`
// needs is the line below.
impl std::error::Error for PluginError {}

impl From<String> for PluginError {
    fn from(message: String) -> Self {
        PluginError::Other(message)
    }
}

impl From<&str> for PluginError {
    fn from(message: &str) -> Self {
        PluginError::Other(message.to_string())
    }
}

impl From<std::io::Error> for PluginError {
    fn from(e: std::io::Error) -> Self {
        PluginError::Other(e.to_string())
    }
}
