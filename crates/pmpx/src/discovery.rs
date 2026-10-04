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

use std::path::{Path, PathBuf};

use crate::config::DiscoveryConfig;

/// The directories walked: **from the start outward** (near to far).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    /// Directories checked in order, `[0]` is the start.
    pub dirs: Vec<PathBuf>,
    /// Why the walk stopped. `pmpx info` displays it —
    /// "why was no project found" is most often answered by having hit one of these.
    pub stopped: StopReason,
}

/// Why walking up stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Reached the filesystem root.
    FilesystemRoot,
    /// Hit `$HOME` — anything above it is no longer the user's project.
    Home,
    /// Hit `.git`.
    GitRoot,
    /// Reached the `max_depth` limit.
    MaxDepth,
    /// The caller explicitly asked not to walk up (`--no-walk-up` or `[discovery] walk_up = false`).
    WalkUpDisabled,
}

impl StopReason {
    /// Human-readable explanation, for `pmpx info`.
    pub fn describe(self, max_depth: usize) -> String {
        match self {
            StopReason::FilesystemRoot => "reached the filesystem root".into(),
            StopReason::Home => "reached $HOME".into(),
            StopReason::GitRoot => "reached .git".into(),
            StopReason::MaxDepth => format!("reached the max_depth limit ({})", dirs(max_depth)),
            StopReason::WalkUpDisabled => "--no-walk-up / [discovery] walk_up = false".into(),
        }
    }
}

/// `1 directory` / `6 directories`.
///
/// Every count around here is routinely 1, and "Walked up 1 directories" is exactly the kind of
/// thing users report as a bug.
pub(crate) fn dirs(n: usize) -> String {
    format!("{n} director{}", if n == 1 { "y" } else { "ies" })
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
    /// of [`crate::config::MergedProjectConfig`].
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
mod tests {
    use super::*;

    fn cfg() -> DiscoveryConfig {
        DiscoveryConfig::default()
    }

    fn cfg_at_most(n: usize) -> DiscoveryConfig {
        DiscoveryConfig {
            max_depth: n,
            ..DiscoveryConfig::default()
        }
    }

    /// Build a directory tree (`dirs` are relative to `<tmp>`).
    fn tree(dirs: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for d in dirs {
            std::fs::create_dir_all(tmp.path().join(d)).unwrap();
        }
        tmp
    }

    fn touch(path: &Path) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn starts_with_the_start_directory() {
        let tmp = tree(&["a/b/c"]);
        let w = walk(&tmp.path().join("a/b/c"), &cfg());

        assert_eq!(w.dirs[0], normalize(&tmp.path().join("a/b/c")));
    }

    #[test]
    fn walks_up_in_order_from_near_to_far() {
        let tmp = tree(&["a/b/c"]);
        let w = walk(&tmp.path().join("a/b/c"), &cfg());

        assert!(w.dirs[0].ends_with("c"), "{:?}", w.dirs[0]);
        assert!(w.dirs[1].ends_with("b"), "{:?}", w.dirs[1]);
        assert!(w.dirs[2].ends_with("a"), "{:?}", w.dirs[2]);

        // Walk up level by level; not a single level may be skipped
        for pair in w.dirs.windows(2) {
            assert_eq!(
                pair[1].as_path(),
                pair[0].parent().unwrap(),
                "{} should walk up to its parent directory",
                pair[0].display()
            );
        }

        // Where it stops depends on where the test runs — a temp directory may hit max_depth or
        // $HOME first, so only assert "one of them", never a single one.
        assert!(w.dirs.len() <= cfg().max_depth);
        assert!(
            matches!(
                w.stopped,
                StopReason::MaxDepth
                    | StopReason::FilesystemRoot
                    | StopReason::Home
                    | StopReason::GitRoot
            ),
            "unexpectedly stopped at {:?}",
            w.stopped
        );
    }

    /// With a large enough limit the walk always reaches `$HOME` or the filesystem root — it never
    /// walks up forever.
    #[test]
    fn a_generous_max_depth_still_terminates() {
        let tmp = tree(&["a/b/c"]);
        let mut c = cfg();
        c.max_depth = 4096;

        let w = walk(&tmp.path().join("a/b/c"), &c);
        assert!(
            matches!(w.stopped, StopReason::Home | StopReason::FilesystemRoot),
            "unexpectedly stopped at {:?}",
            w.stopped
        );
        assert!(
            w.dirs.len() < 4096,
            "should not actually use up the whole limit"
        );
    }

    /// `walk_up = false` (or `--no-walk-up`): **the start is the only candidate**.
    #[test]
    fn walk_up_disabled_returns_only_the_start() {
        let tmp = tree(&["a/b/c"]);
        let start = tmp.path().join("a/b/c");
        let mut c = cfg();
        c.walk_up = false;

        let w = walk(&start, &c);
        assert_eq!(w.dirs, vec![normalize(&start)]);
        assert_eq!(w.stopped, StopReason::WalkUpDisabled);
    }

    #[test]
    fn max_depth_limits_how_many_directories_are_checked() {
        let tmp = tree(&["a/b/c/d/e"]);
        let w = walk(&tmp.path().join("a/b/c/d/e"), &cfg_at_most(3));

        assert_eq!(w.dirs.len(), 3);
        assert_eq!(w.stopped, StopReason::MaxDepth);
        assert_eq!(
            w.stopped.describe(3),
            "reached the max_depth limit (3 directories)"
        );
    }

    /// 1 must not come out as "1 directories" -- these counts are routinely 1, and it is exactly
    /// the kind of thing users report as a bug.
    #[test]
    fn a_single_directory_is_not_plural() {
        assert_eq!(dirs(1), "1 directory");
        assert_eq!(dirs(0), "0 directories");
        assert_eq!(dirs(6), "6 directories");
        assert_eq!(
            StopReason::MaxDepth.describe(1),
            "reached the max_depth limit (1 directory)"
        );
    }

    /// Nothing above `.git` — anything above the repository root is not part of this project.
    #[test]
    fn stops_after_a_directory_containing_git() {
        let tmp = tree(&["repo/web/src"]);
        std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();

        let w = walk(&tmp.path().join("repo/web/src"), &cfg());

        assert_eq!(w.stopped, StopReason::GitRoot);
        // repo is the last candidate, and **it is checked itself**
        assert!(w.dirs.last().unwrap().ends_with("repo"));
        let above_tmp = normalize(tmp.path());
        let above_tmp = above_tmp.parent().unwrap();
        assert!(!w.dirs.iter().any(|d| d.as_path() == above_tmp));
    }

    #[test]
    fn stop_at_git_can_be_turned_off() {
        let tmp = tree(&["repo/web/src"]);
        std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();

        let mut c = cfg();
        c.stop_at_git = false;
        let w = walk(&tmp.path().join("repo/web/src"), &c);

        // walked past the repo containing .git
        assert!(w.dirs.len() > 3);
        assert_ne!(w.stopped, StopReason::GitRoot);
    }

    /// A `.git` **file** (worktree / submodule) counts just the same.
    #[test]
    fn git_file_also_stops_the_walk() {
        let tmp = tree(&["repo/web/src"]);
        touch(&tmp.path().join("repo/.git"));

        let w = walk(&tmp.path().join("repo/web/src"), &cfg());
        assert_eq!(w.stopped, StopReason::GitRoot);
    }

    #[test]
    fn the_project_root_is_the_nearest_hit() {
        let tmp = tree(&["repo/web/src"]);
        touch(&tmp.path().join("repo/Cargo.toml"));
        touch(&tmp.path().join("repo/web/package.json"));

        let root = walk(&tmp.path().join("repo/web/src"), &cfg())
            .project_root(|d| d.join("Cargo.toml").exists() || d.join("package.json").exists())
            .unwrap();

        assert!(
            root.ends_with("web"),
            "the root should be web, got {root:?}"
        );
    }

    #[test]
    fn no_root_is_found_when_nothing_matches() {
        let tmp = tree(&["repo/some/dir"]);
        std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();

        let root = walk(&tmp.path().join("repo/some/dir"), &cfg())
            .project_root(|d| d.join("Cargo.toml").exists());
        assert!(root.is_none());
    }

    #[test]
    fn the_start_directory_itself_can_be_the_root() {
        let tmp = tree(&["proj"]);
        touch(&tmp.path().join("proj/Cargo.toml"));

        let root = walk(&tmp.path().join("proj"), &cfg())
            .project_root(|d| d.join("Cargo.toml").exists())
            .unwrap();
        assert_eq!(root, normalize(&tmp.path().join("proj")));
    }

    /// Config can see the layer above the project root.
    #[test]
    fn config_collection_reaches_above_the_project_root() {
        let tmp = tree(&["repo/crates/core/src"]);
        std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();
        touch(&tmp.path().join("repo/.pmpx.toml"));
        touch(&tmp.path().join("repo/crates/core/.pmpx.toml"));
        touch(&tmp.path().join("repo/crates/core/Cargo.toml"));

        let start = tmp.path().join("repo/crates/core/src");
        let w = walk(&start, &cfg());

        let root = w.project_root(|d| d.join("Cargo.toml").exists()).unwrap();
        assert!(root.ends_with("core"));

        // Both must be visible, near to far
        let cfgs = w.config_paths();
        assert_eq!(cfgs.len(), 2, "{cfgs:?}");
        assert!(cfgs[0].ends_with("core/.pmpx.toml"));
        assert!(cfgs[1].ends_with("repo/.pmpx.toml"));

        assert!(
            cfgs.iter().any(|p| p.ends_with("repo/.pmpx.toml")),
            "the config one layer above the project root must be visible"
        );
    }

    #[test]
    fn config_collection_skips_directories_without_a_config() {
        let tmp = tree(&["repo/a/b/c"]);
        touch(&tmp.path().join("repo/.pmpx.toml"));

        let cfgs = walk(&tmp.path().join("repo/a/b/c"), &cfg()).config_paths();
        assert_eq!(cfgs.len(), 1);
        assert!(cfgs[0].ends_with("repo/.pmpx.toml"));
    }

    /// Both answers must be available from **one** traversal: that is what `Session::open` relies
    /// on, and the reason `Walk` carries the two methods at all.
    #[test]
    fn one_walk_answers_both_questions() {
        let tmp = tree(&["repo/crates/core/src"]);
        std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();
        touch(&tmp.path().join("repo/.pmpx.toml"));
        touch(&tmp.path().join("repo/crates/core/.pmpx.toml"));
        touch(&tmp.path().join("repo/crates/core/Cargo.toml"));

        let start = tmp.path().join("repo/crates/core/src");
        let w = walk(&start, &cfg());

        let root = w
            .project_root(|d| d.join("Cargo.toml").exists())
            .expect("the core crate should be the project root");
        assert!(root.ends_with("core"), "{root:?}");

        let cfgs = w.config_paths();
        assert_eq!(cfgs.len(), 2, "{cfgs:?}");
        assert!(cfgs[0].ends_with("core/.pmpx.toml"));
        assert!(cfgs[1].ends_with("repo/.pmpx.toml"));
    }

    #[test]
    fn walking_from_a_relative_path_works() {
        // A relative path must first be joined onto the cwd, otherwise `..` is removed wrongly
        let w = walk(Path::new("."), &cfg());
        assert!(w.dirs[0].is_absolute(), "{:?}", w.dirs[0]);
        assert_eq!(w.dirs[0], normalize(&std::env::current_dir().unwrap()));
    }

    #[test]
    fn lexical_normalize_removes_dots_without_touching_the_fs() {
        assert_eq!(
            lexical_normalize(Path::new("/a/b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(
            lexical_normalize(Path::new("/a/./b")),
            PathBuf::from("/a/b")
        );
        assert_eq!(
            lexical_normalize(Path::new("/a/b/../..")),
            PathBuf::from("/")
        );
    }

    #[test]
    fn every_stop_reason_has_a_description() {
        for (reason, max) in [
            (StopReason::FilesystemRoot, 8),
            (StopReason::Home, 8),
            (StopReason::GitRoot, 8),
            (StopReason::MaxDepth, 8),
            (StopReason::WalkUpDisabled, 8),
        ] {
            assert!(!reason.describe(max).is_empty());
        }
    }
}
