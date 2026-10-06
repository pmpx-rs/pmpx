//! Project root discovery.
//!
//! One walk up answers two questions: [`Walk::project_root`] answers "where do I run" (only the
//! nearest match), and [`Walk::config_paths`] answers "which rules apply" (every `.pmpx.toml` along
//! the way). The stop conditions are identical; the results are two different things, which is why
//! the caller walks once and asks [`Walk`] both.
//!
//! ```text
//! ~/repo/.git
//! ~/repo/.pmpx.toml             [plugin] rust = "cargo"
//! ~/repo/crates/core/.pmpx.toml [plugin] node = "pnpm"
//! cwd = ~/repo/crates/core/src/
//!
//! Walk::project_root  → ~/repo/crates/core
//! Walk::config_paths  → [core/.pmpx.toml, repo/.pmpx.toml]  (both are read)
//! ```
//!
//! This file is the walk itself; why it stopped is [`StopReason`].

use std::path::{Path, PathBuf};

use pmpx_project::DiscoveryConfig;

mod stop;

pub use stop::dirs;
pub use stop::StopReason;

/// The directories walked: **from the start outward** (near to far).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    /// Directories checked in order, `[0]` is the start.
    pub dirs: Vec<PathBuf>,
    /// Why the walk stopped. `pmpx info` displays it —
    /// "why was no project found" is most often answered by having hit one of these.
    pub stopped: StopReason,
}

/// Walk up from `start`, collecting the directories to check.
///
/// `max_depth` is **how many directories may be checked at most (including the start)**, not
/// "how many levels to walk up".
///
/// Both `$HOME` and `.git` stop the walk **after the current directory has been checked**: `$HOME`
/// itself is still a candidate, so `~/Cargo.toml` or `~/.pmpx.toml` can take effect. We never walk
/// above `$HOME`.
pub fn walk(start: &Path, cfg: &DiscoveryConfig) -> Walk {
    let start = normalize(start);
    let mut dirs = vec![start.clone()];

    if !cfg.walk_up {
        return Walk {
            dirs,
            stopped: StopReason::WalkUpDisabled,
        };
    }

    let home = directories::UserDirs::new().map(|d| normalize(d.home_dir()));

    let mut current = start;

    let stopped = loop {
        if dirs.len() >= cfg.max_depth {
            break StopReason::MaxDepth;
        }

        // Order is semantics: both `.git` and `$HOME` stop only after the current directory has
        // been checked.
        if cfg.stop_at_git && current.join(".git").exists() {
            break StopReason::GitRoot;
        }
        if Some(&current) == home.as_ref() {
            break StopReason::Home;
        }

        match current.parent() {
            // `parent() == Some(self)` means the filesystem root (`/` or `C:\`)
            Some(parent) if parent != current => {
                current = parent.to_path_buf();
                dirs.push(current.clone());
            }
            _ => break StopReason::FilesystemRoot,
        }
    };

    Walk { dirs, stopped }
}

/// The two questions one walk up can answer.
///
/// Both read the same candidate list, so a caller that needs both walks once and asks twice; the
/// stop conditions cannot drift apart because there is only one traversal.
impl Walk {
    /// The project root: the nearest walked directory `is_root` accepts.
    ///
    /// `is_root` comes from the caller, usually "this directory has a `.pmpx.toml`, or has one of
    /// the detect files declared by an installed plugin". It is a parameter so that this module
    /// does not need to know about plugins.
    pub fn project_root(&self, is_root: impl Fn(&Path) -> bool) -> Option<PathBuf> {
        self.dirs.iter().find(|d| is_root(d)).cloned()
    }

    /// Collect every `.pmpx.toml` on this walk, **near to far**.
    ///
    /// The stop conditions match [`Walk::project_root`]; this order is the "nearest wins" merge rule
    /// of [`pmpx_project::MergedProjectConfig`].
    pub fn config_paths(&self) -> Vec<PathBuf> {
        self.dirs
            .iter()
            .map(|d| d.join(".pmpx.toml"))
            .filter(|p| p.is_file())
            .collect()
    }
}

/// Normalize a directory into a directly comparable form.
///
/// On Windows `C:\Users\me` and `C:\Users\me\` are the same directory but not equal as `PathBuf`s,
/// and the `$HOME` check is exactly that equality; `.` and `..` must be removed too.
fn normalize(p: &Path) -> PathBuf {
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };

    // Not `canonicalize`: it resolves symlinks (/tmp → /private/tmp), which would make the reported
    // path differ from what the user sees. Lexical resolution only.
    lexical_normalize(&absolute)
}

/// Remove `..` and `.` purely lexically, without touching the filesystem.
fn lexical_normalize(p: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop one level; keep it if already at the root
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests;
