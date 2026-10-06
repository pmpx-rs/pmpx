//! What the host hands to a plugin: everything it knows about this call.
//!
//! Two kinds of thing live here, and the split is deliberate:
//!
//! - **What the project is** -- the root, the matched files, the `.pmpx.toml`s that were read, the
//!   `[scripts]` in them. These come from the detect layer's declarative allowlist, not from the
//!   plugin reading anything.
//! - **What the host decided** -- why this plugin was selected, with what score, and what the
//!   project config pins. These are the host's own state, and a plugin may report them ("you pinned
//!   me, but the newer one is installed") even though it cannot change them.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Why the host selected this plugin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SelectionReason {
    /// It won on evidence: the highest score in the winning family.
    ///
    /// Also the default: a hand-built context (in a plugin's own test) starts from "nothing was
    /// pinned and nobody named me", which is this.
    #[default]
    Scored,
    /// `.pmpx.toml` pins this plugin's family to it.
    Pinned,
    /// `-p/--plugin` named it. This overrides everything else, including a pin.
    Explicit,
    /// A reason this build of the contract does not know.
    ///
    /// A newer host may add one, and an older plugin must still run: the reason is information, not
    /// something to refuse a call over.
    Unknown,
}

impl SelectionReason {
    /// Read the number the host sent.
    pub const fn from_abi(raw: u32) -> Self {
        match raw {
            crate::abi::PMPX_REASON_SCORED => Self::Scored,
            crate::abi::PMPX_REASON_PINNED => Self::Pinned,
            crate::abi::PMPX_REASON_EXPLICIT => Self::Explicit,
            _ => Self::Unknown,
        }
    }

    /// The word this reason is called by, for a plugin's own messages.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scored => "scored",
            Self::Pinned => "pinned",
            Self::Explicit => "explicit",
            Self::Unknown => "unknown",
        }
    }

    /// The number this reason crosses the boundary as.
    ///
    /// [`SelectionReason::Unknown`] has none of its own -- it means "this build does not know that
    /// reason" and only ever comes *from* a host -- so it answers with the neutral
    /// [`PMPX_REASON_SCORED`](crate::abi::PMPX_REASON_SCORED).
    pub const fn to_abi(self) -> u32 {
        match self {
            Self::Scored | Self::Unknown => crate::abi::PMPX_REASON_SCORED,
            Self::Pinned => crate::abi::PMPX_REASON_PINNED,
            Self::Explicit => crate::abi::PMPX_REASON_EXPLICIT,
        }
    }
}

impl std::fmt::Display for SelectionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The context the host passes to the plugin. Read-only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    /// Project root: where the command runs unless the answer names a `cwd` of its own.
    ///
    /// For building log / error messages only -- it must not be used to read files, see the
    /// constraint list in the crate docs.
    pub project_root: PathBuf,

    /// The directory the person ran pmpx from (`-C`, or the process's own directory).
    ///
    /// It differs from `project_root` whenever the root was found by walking up, which makes this
    /// the only way to tell which package of a monorepo the command is for. **Not** where the
    /// command will run: that is the `cwd` of the answer, and it defaults to `project_root`.
    ///
    /// Empty when the host had nothing to say about it.
    pub start_dir: PathBuf,

    /// The files this detection matched, relative to `project_root`.
    /// This is the plugin's main channel for learning "what the project looks like". Example: the
    /// yarn plugin distinguishes classic from berry via `has_matched(".yarnrc.yml")` without
    /// reading a single file.
    pub matched: Vec<String>,

    /// Why this plugin was selected.
    pub reason: SelectionReason,

    /// The evidence score it won with. 0 when it was pinned or named outright, since neither needed
    /// evidence.
    pub score: u32,

    /// `[plugin]` pins from the project config: family → plugin name, nearest `.pmpx.toml` winning.
    ///
    /// The whole map, not just this plugin's family: it is how a plugin can notice that the project
    /// pins something for its family which is *not* installed, and say so.
    pub pins: BTreeMap<String, String>,

    /// `[scripts]` from those files.
    ///
    /// The host parses them and does not interpret them; what a script means is not defined yet, so
    /// this is for a plugin that wants to know what the project declares.
    pub scripts: BTreeMap<String, String>,

    /// The `.pmpx.toml` files that were actually read, nearest first.
    pub config_files: Vec<PathBuf>,
}

impl Context {
    /// Whether one of the matched files is this one -- the standard way for a plugin to branch on
    /// shape.
    pub fn has_matched(&self, file: &str) -> bool {
        self.matched.iter().any(|m| m == file)
    }

    /// Whether `.pmpx.toml` is why this plugin is the one being asked.
    ///
    /// Worth reporting to the user when the pin is the only reason: pinning is also how someone
    /// holds a plugin back on purpose.
    pub fn was_pinned(&self) -> bool {
        self.reason == SelectionReason::Pinned
    }

    /// What the project pins this plugin's `family` to, if anything.
    pub fn pinned_for(&self, family: &str) -> Option<&str> {
        self.pins.get(family).map(String::as_str)
    }
}
