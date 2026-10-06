//! What can go wrong: loading a plugin, and one call to it.

use std::fmt;
use std::path::PathBuf;

/// A plugin could not be loaded, or does not speak this ABI.
#[derive(Debug)]
pub enum LoadError {
    /// The library itself could not be opened: not a dynamic library, wrong architecture, a missing
    /// dependency of the plugin's own.
    Open {
        /// The file that was tried.
        path: PathBuf,
        /// The loader's own message.
        message: String,
    },

    /// The library opened, but exports no entry symbol.
    ///
    /// This is the clean failure for "built against a different contract": the symbol carries the
    /// root structure's layout, so an old host looking for the old name finds nothing instead of
    /// reading fields that moved.
    NoEntrySymbol {
        /// The file that was tried.
        path: PathBuf,
        /// The symbol that was looked for.
        symbol: &'static str,
    },

    /// The plugin reports a different semantic major version.
    Major {
        /// The file that was tried.
        path: PathBuf,
        /// What the plugin says.
        found: u32,
        /// What this host speaks.
        expected: u32,
    },

    /// The plugin does not provide a capability this host cannot work without.
    MissingCapability {
        /// The file that was tried.
        path: PathBuf,
        /// The capability's name.
        capability: &'static str,
    },

    /// The plugin provides a capability, but its table is smaller than this host knows how to read.
    ShortTable {
        /// The file that was tried.
        path: PathBuf,
        /// The capability's name.
        capability: &'static str,
        /// The size the plugin declared.
        found: usize,
        /// The size this host needs.
        expected: usize,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Open { path, message } => {
                write!(f, "cannot load the plugin at {}: {message}", path.display())
            }
            LoadError::NoEntrySymbol { path, symbol } => write!(
                f,
                "{} exports no {symbol}: it was built against a different version of the plugin \
                 contract, so it has to be rebuilt",
                path.display()
            ),
            LoadError::Major {
                path,
                found,
                expected,
            } => write!(
                f,
                "the plugin at {} speaks ABI major version {found}, this pmpx speaks {expected}; \
                 one of the two has to be updated",
                path.display()
            ),
            LoadError::MissingCapability { path, capability } => write!(
                f,
                "the plugin at {} provides no `{capability}` capability, which this pmpx needs",
                path.display()
            ),
            LoadError::ShortTable {
                path,
                capability,
                found,
                expected,
            } => write!(
                f,
                "the plugin at {} says its `{capability}` table is {found} bytes, but {expected} \
                 are needed to read it safely",
                path.display()
            ),
        }
    }
}

impl std::error::Error for LoadError {}

/// What one call to a plugin came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The plugin does not do that verb.
    ///
    /// Not a failure of the run: for `exec` the host may still run the user's command verbatim, and
    /// that decision belongs to the engine.
    UnsupportedVerb,

    /// The plugin thinks the call was malformed.
    InvalidArgs,

    /// The plugin failed internally, including a panic it contained itself.
    Internal,

    /// A code this host does not know.
    ///
    /// Kept as it came: the rule is that an unknown code is treated like [`CallError::Internal`],
    /// but a host that wants to log the number can.
    Unknown(u32),

    /// The plugin answered success but filled in a [`pmpx_plugin_abi::PmpxCommand`] smaller than
    /// this host knows how to read.
    ShortCommand {
        /// The size the plugin declared.
        found: usize,
        /// The size this host needs.
        expected: usize,
    },
}

impl CallError {
    /// The code the plugin answered with, for a message that wants to show it.
    pub fn code(&self) -> Option<u32> {
        match self {
            CallError::Unknown(code) => Some(*code),
            _ => None,
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::UnsupportedVerb => f.write_str("it does not support that verb"),
            CallError::InvalidArgs => f.write_str("it thinks the arguments are invalid"),
            CallError::Internal => {
                f.write_str("it failed internally or panicked (details on its own stderr)")
            }
            CallError::Unknown(code) => write!(f, "it answered with unknown code {code}"),
            CallError::ShortCommand { found, expected } => write!(
                f,
                "it answered with a {found}-byte command structure, but {expected} bytes are \
                 needed to read it safely"
            ),
        }
    }
}

impl std::error::Error for CallError {}
