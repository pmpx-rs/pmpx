//! The verbs a plugin is asked to translate.
//!
//! A closed set -- this is all the command line has -- and their numbering is part of the ABI, so it
//! is pinned against the ABI crate rather than chosen here.

use std::fmt;

use pmpx_plugin_abi as abi;

/// The verbs pmpx recognizes. A closed set -- this is all the command line has.
///
/// The numbers correspond one-to-one with the `PMPX_VERB_*` constants, and the order must not change:
/// a different numbering is an ABI break, and the surface snapshot in the ABI crate records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum Verb {
    /// Install dependencies. No argument = install everything in the lockfile, with arguments =
    /// add.
    Install = abi::PMPX_VERB_INSTALL,
    /// Remove dependencies.
    Remove = abi::PMPX_VERB_REMOVE,
    /// Run a script / target.
    Run = abi::PMPX_VERB_RUN,
    /// Build.
    Build = abi::PMPX_VERB_BUILD,
    /// Test.
    Test = abi::PMPX_VERB_TEST,
    /// Update dependencies.
    Update = abi::PMPX_VERB_UPDATE,
    /// Escape hatch: run an arbitrary command. Plugins that do not support it should report an
    /// error explicitly, see [`PackageManager::command`](crate::PackageManager::command).
    Exec = abi::PMPX_VERB_EXEC,
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

    /// Reconstruct from a cross-boundary number.
    ///
    /// `None` means "a verb this build does not know", and the shell answers that with
    /// [`PMPX_ERR_UNSUPPORTED_VERB`](abi::PMPX_ERR_UNSUPPORTED_VERB) rather than "invalid
    /// arguments": that is what keeps a *new* verb additive, because it leaves the host's
    /// degradation path open for `exec`.
    pub const fn from_abi(n: u32) -> Option<Verb> {
        match n {
            abi::PMPX_VERB_INSTALL => Some(Verb::Install),
            abi::PMPX_VERB_REMOVE => Some(Verb::Remove),
            abi::PMPX_VERB_RUN => Some(Verb::Run),
            abi::PMPX_VERB_BUILD => Some(Verb::Build),
            abi::PMPX_VERB_TEST => Some(Verb::Test),
            abi::PMPX_VERB_UPDATE => Some(Verb::Update),
            abi::PMPX_VERB_EXEC => Some(Verb::Exec),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every verb round-trips, and the numbering is the ABI's.
    #[test]
    fn every_verb_has_its_number() {
        for (verb, number) in [
            (Verb::Install, abi::PMPX_VERB_INSTALL),
            (Verb::Remove, abi::PMPX_VERB_REMOVE),
            (Verb::Run, abi::PMPX_VERB_RUN),
            (Verb::Build, abi::PMPX_VERB_BUILD),
            (Verb::Test, abi::PMPX_VERB_TEST),
            (Verb::Update, abi::PMPX_VERB_UPDATE),
            (Verb::Exec, abi::PMPX_VERB_EXEC),
        ] {
            assert_eq!(verb.to_abi(), number);
            assert_eq!(Verb::from_abi(number), Some(verb));
        }
        assert_eq!(Verb::ALL.len(), 7, "the closed set has seven verbs");
    }

    /// A verb this build does not know is not an error here: the shell decides, and it says
    /// "unsupported".
    #[test]
    fn an_unknown_number_has_no_verb() {
        assert_eq!(Verb::from_abi(999), None);
    }
}
