//! The verbs a plugin is asked to translate.
//!
//! A closed set -- this is all the command line has -- and their numbering is part of the ABI, so
//! it is pinned against [`crate::abi`] rather than chosen here.

use std::fmt;
use std::str::FromStr;

use crate::abi;
use crate::PluginError;

/// The verbs pmpx recognizes. A closed set -- this is all the command line has.
/// The numbers correspond one-to-one with the `VERB_*` constants in [`abi`], and the order must
/// not change (changing it requires [`abi::ABI_VERSION`] + 1); a test in the `abi` module pins
/// this down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum Verb {
    /// Install dependencies. No argument = install everything in the lockfile, with arguments =
    /// add.
    Install = abi::VERB_INSTALL,
    /// Remove dependencies.
    Remove = abi::VERB_REMOVE,
    /// Run a script / target.
    Run = abi::VERB_RUN,
    /// Build.
    Build = abi::VERB_BUILD,
    /// Test.
    Test = abi::VERB_TEST,
    /// Update dependencies.
    Update = abi::VERB_UPDATE,
    /// Escape hatch: run an arbitrary command. Plugins that do not support it should report an
    /// error explicitly, see [`PackageManager::command`](crate::PackageManager::command).
    Exec = abi::VERB_EXEC,
}

impl Verb {
    /// All verbs, in numbering order.
    pub const ALL: &'static [Verb] = &[
        Verb::Install,
        Verb::Remove,
        Verb::Run,
        Verb::Build,
        Verb::Test,
        Verb::Update,
        Verb::Exec,
    ];

    /// Convert to the number used across the boundary.
    pub const fn to_abi(self) -> u32 {
        self as u32
    }

    /// Reconstruct from a cross-boundary number; returns `None` for an unknown one (this is where
    /// a host and a plugin of mismatched versions land).
    pub const fn from_abi(n: u32) -> Option<Verb> {
        match n {
            abi::VERB_INSTALL => Some(Verb::Install),
            abi::VERB_REMOVE => Some(Verb::Remove),
            abi::VERB_RUN => Some(Verb::Run),
            abi::VERB_BUILD => Some(Verb::Build),
            abi::VERB_TEST => Some(Verb::Test),
            abi::VERB_UPDATE => Some(Verb::Update),
            abi::VERB_EXEC => Some(Verb::Exec),
            _ => None,
        }
    }

    /// The word written on the command line.
    pub const fn as_str(self) -> &'static str {
        match self {
            Verb::Install => "install",
            Verb::Remove => "remove",
            Verb::Run => "run",
            Verb::Build => "build",
            Verb::Test => "test",
            Verb::Update => "update",
            Verb::Exec => "exec",
        }
    }
}

impl fmt::Display for Verb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Verb {
    type Err = PluginError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Verb::ALL
            .iter()
            .copied()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| PluginError::other(format!("unknown verb: {s}")))
    }
}
