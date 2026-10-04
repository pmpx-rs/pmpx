//! What the host hands to a plugin: the project root and the matched files.

use std::path::PathBuf;

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
