//! # pmpx-plugin
//!
//! The pmpx plugin contract: one trait plus a stable C ABI that carries the trait safely across
//! the `dlopen` boundary.
//!
//! A plugin author only implements [`PackageManager`] and then uses the one-line
//! [`export!`](macro@crate::export) to generate the whole C ABI shell:
//!
//! ```ignore
//! pub fn create() -> Box<dyn PackageManager> { Box::new(CargoPlugin) }
//! pmpx_plugin::export!(create);
//! ```
//! # What a plugin may and may not do
//!
//! [`PackageManager::command`] should only map from its inputs: it does not read files (including
//! anything under `project_root`), does not write files, does not read environment variables,
//! does not spawn child processes, and does not make network requests. That keeps `command()`
//! completely pure (unit tests need no fixture directory at all) and stops a plugin from using
//! file reads to probe things it should not know -- "what the project looks like" is decided by
//! the host's detect layer and handed to the plugin through `matched`, a declarative, auditable
//! allowlist.
//! Data crossing [`abi`] is always `#[repr(C)]` POD, so the two sides need not share a rustc; see
//! the module docs of [`abi`].
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod abi;

mod export;

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

// ---- Family ----

/// Ecosystem family. Decides the grouping headers of the host's `plugin ls`, the scope of
/// `plugin set`, and the key names of `[plugin] <family> = "..."` in a project's `.pmpx.toml`.
/// An open type rather than a closed enum: known ecosystems have constants, unknown ones extend
/// via [`Family::new`], and comparison and ordering go by string -- a third-party plugin
/// supporting a new ecosystem needs neither a change to this crate nor a host release.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Family(std::borrow::Cow<'static, str>);

impl Family {
    /// Node / frontend ecosystem.
    pub const NODE: Family = Family(std::borrow::Cow::Borrowed("node"));
    /// Rust ecosystem.
    pub const RUST: Family = Family(std::borrow::Cow::Borrowed("rust"));
    /// Python ecosystem.
    pub const PYTHON: Family = Family(std::borrow::Cow::Borrowed("python"));
    /// Go ecosystem.
    pub const GO: Family = Family(std::borrow::Cow::Borrowed("go"));
    /// JVM ecosystem.
    pub const JVM: Family = Family(std::borrow::Cow::Borrowed("jvm"));
    /// .NET ecosystem.
    pub const DOTNET: Family = Family(std::borrow::Cow::Borrowed("dotnet"));
    /// PHP ecosystem.
    pub const PHP: Family = Family(std::borrow::Cow::Borrowed("php"));
    /// Ruby ecosystem.
    pub const RUBY: Family = Family(std::borrow::Cow::Borrowed("ruby"));

    /// Construct from a custom name.
    pub fn new(name: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Family(name.into())
    }

    /// Key form. Used for the `plugin ls` grouping and for `.pmpx.toml` keys.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Human-readable grouping header; unknown ecosystems are returned as-is.
    pub fn display(&self) -> &str {
        match self.as_str() {
            "node" => "Node / frontend",
            "rust" => "Rust",
            "python" => "Python",
            "go" => "Go",
            "jvm" => "JVM",
            "dotnet" => ".NET",
            "php" => "PHP",
            "ruby" => "Ruby",
            other => other,
        }
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&'static str> for Family {
    fn from(s: &'static str) -> Self {
        Family::new(s)
    }
}

impl AsRef<str> for Family {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

// ---- Verb ----

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
    /// error explicitly, see [`PackageManager::command`].
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

// ---- CommandSpec ----

/// One command waiting to be executed, pure data -- a plugin only describes "what to run" and the
/// actual spawn is done by the host, so stdio, environment, and exit-code handling have exactly
/// one implementation. Construction is a consuming chain (`self` -> `Self`), so it can produce a
/// value directly:
/// ```ignore
/// let spec = CommandSpec::new("cargo").arg("add").args(args.iter()).cwd("/somewhere");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Executable. The host resolves it to a real path with `which` and decides per platform
    /// whether to wrap it in `cmd /C` (on Windows `pnpm` is really `pnpm.cmd`, and spawning it
    /// directly fails).
    pub program: OsString,

    /// Arguments, in order.
    pub args: Vec<OsString>,

    /// Working-directory override. `None` = use the project root given by the host.
    pub cwd: Option<PathBuf>,
}

impl CommandSpec {
    /// Specify the executable.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
        }
    }

    /// Append one argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append a batch of arguments; `args.iter()` can be passed straight in and stays lossless
    /// (`&OsString: Into<OsString>`), unlike `String`, which would corrupt non-UTF-8 arguments on
    /// Unix.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Override the working directory.
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }
}

// ---- Context ----

/// The context the host passes to the plugin. Read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// Project root. For building log / error messages only -- it must not be used to read files,
    /// see the constraint list in the crate docs.
    pub project_root: PathBuf,

    /// The files this detection matched, relative to `project_root`.
    /// This is the plugin's only channel for learning "what the project looks like". Example: the
    /// yarn plugin distinguishes classic from berry via `has_matched(".yarnrc.yml")` without
    /// reading a single file.
    pub matched: Vec<String>,
}

impl Context {
    /// Whether one of the matched files is this one -- the standard way for a plugin to branch on
    /// shape.
    pub fn has_matched(&self, file: &str) -> bool {
        self.matched.iter().any(|m| m == file)
    }
}

// ---- PluginError ----

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

// ---- PackageManager ----

/// An implementation of one package manager backend, a purely synchronous interface: no `async`,
/// no callbacks, no I/O -- passing a `Future` across the `dlopen` boundary is the most fragile
/// part of this approach. It should only do mapping; see the crate docs for the constraints.
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
