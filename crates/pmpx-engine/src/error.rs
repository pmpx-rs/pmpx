//! Why a command could not be started.
//!
//! Two outcomes, because the next step differs: "there is no such program" is a setup problem the
//! user can fix (and the message lists the near misses), while "it is there and would not start" is
//! something to report with its own cause. Neither is this crate's decision to print.

use std::ffi::{OsStr, OsString};
use std::fmt;

/// A program that could not be started.
#[derive(Debug)]
pub enum EngineError {
    /// The plugin store refused -- installing, removing, or looking something up -- and the kit's own
    /// message is the whole explanation; this crate has nothing to add to it.
    ///
    /// Behind the `store` feature, because the kit is an optional dependency: a host that only runs
    /// plugins does not pay for `ureq`, `rustls` and `ring`, and neither does its error type.
    #[cfg(feature = "store")]
    Kit(crate_plugin_kit::KitError),

    /// The program was not found: either the path a plugin pointed at does not exist, or no name on
    /// `PATH` matches.
    ///
    /// The message is built here rather than by the caller because only this crate knows what was
    /// searched, and it is the part a person needs to see.
    NotFound(String),

    /// The user asked for something impossible (`-C` at a directory that is not there).
    Usage(String),

    /// The setup could not be completed: the store, the manifest list, the project root.
    Setup(String),

    /// No project type was detected, or the pinned plugin cannot answer. Exit code 3.
    NoProject(String),

    /// The decision refused to choose, with the reason the decision crate reported.
    Detect(pmpx_detect::DetectFailure),

    /// The plugin answered that it cannot do this.
    Call {
        /// The plugin's name.
        plugin: String,
        /// The verb it refused.
        verb: String,
        /// What it said.
        error: pmpx_loader::CallError,
    },

    /// It was found, and starting it failed anyway.
    Start {
        /// The program, as it was about to be started.
        program: OsString,
        /// What the operating system said.
        source: std::io::Error,
    },
}

impl EngineError {
    /// Refuse a program, with the message a person needs.
    pub(crate) fn not_found(message: String) -> Self {
        Self::NotFound(message)
    }

    /// The full explanation, for whoever is showing it.
    pub fn message(&self) -> String {
        match self {
            #[cfg(feature = "store")]
            Self::Kit(error) => error.to_string(),
            Self::NotFound(message) | Self::Usage(message) | Self::Setup(message) => {
                message.clone()
            }
            Self::NoProject(message) => message.clone(),
            Self::Detect(failure) => failure.message(),
            Self::Call {
                plugin,
                verb,
                error,
            } => format!("plugin {plugin} cannot do `{verb}`: {error}"),
            Self::Start { program, source } => {
                format!("failed to start {}: {source}", program.to_string_lossy())
            }
        }
    }

    /// The program this is about, when there is one.
    pub fn program(&self) -> Option<&OsStr> {
        match self {
            Self::Start { program, .. } => Some(program),
            _ => None,
        }
    }

    /// Whether this is "there is no such program", which is a setup problem rather than a failure.
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::NotFound(_) | Self::NoProject(_) | Self::Setup(_)
        )
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Start { source, .. } => Some(source),
            #[cfg(feature = "store")]
            Self::Kit(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(feature = "store")]
impl From<crate_plugin_kit::KitError> for EngineError {
    fn from(error: crate_plugin_kit::KitError) -> Self {
        Self::Kit(error)
    }
}

/// The crate's own result, so the ported helpers did not have to change shape.
pub(crate) type Result<T> = std::result::Result<T, EngineError>;
