//! The ecosystem family a plugin belongs to.
//!
//! An open type rather than a closed enum: the family decides how `plugin ls` groups plugins and
//! what the keys of `[plugin] <family> = "..."` are called, and a third-party plugin supporting a
//! new ecosystem must not have to wait for a pmpx release.

use std::fmt;

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
