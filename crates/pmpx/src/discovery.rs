//! 项目根发现。
//!
//! 两个入口共用同一次上溯：`find_project_root` 回答"在哪执行"（只取最近命中的一层），
//! `collect_config_paths` 回答"按什么规则执行"（沿途每一份 `.pmpx.toml` 都收）。
//! 停止条件相同，结果是两种东西。
//!
//! ```text
//! ~/repo/.git
//! ~/repo/.pmpx.toml             [plugin] rust = "cargo"
//! ~/repo/crates/core/.pmpx.toml [plugin] node = "pnpm"
//! cwd = ~/repo/crates/core/src/
//!
//! find_project_root    → ~/repo/crates/core
//! collect_config_paths → [core/.pmpx.toml, repo/.pmpx.toml]  （两份都读得到）
//! ```

use std::path::{Path, PathBuf};

use crate::config::DiscoveryConfig;

/// 走出来的目录列表：**从起点到最远**（由近及远）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    /// 依次检查的目录，`[0]` 是起点。
    pub dirs: Vec<PathBuf>,
    /// 为什么停下来。`pmpx info` 会把它显示出来 ——
    /// "为什么没找到项目"最常见的原因就是撞上了其中某一个。
    pub stopped: StopReason,
}

/// 上溯停止的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// 走到文件系统根了。
    FilesystemRoot,
    /// 撞到了 `$HOME` —— 再往上不是用户的项目。
    Home,
    /// 撞到了 `.git`。
    GitRoot,
    /// 到了 `max_depth` 上限。
    MaxDepth,
    /// 调用方明确不要上溯（`--no-walk-up` 或 `[discovery] walk_up = false`）。
    WalkUpDisabled,
}

impl StopReason {
    /// 人类可读的解释，给 `pmpx info` 用。
    pub fn describe(self, max_depth: usize) -> String {
        match self {
            StopReason::FilesystemRoot => "到达文件系统根".into(),
            StopReason::Home => "到达 $HOME".into(),
            StopReason::GitRoot => "到达 .git".into(),
            StopReason::MaxDepth => format!("到达 max_depth 上限（{max_depth} 层）"),
            StopReason::WalkUpDisabled => "--no-walk-up / [discovery] walk_up = false".into(),
        }
    }
}

/// 从 `start` 向上走，收集要检查的目录。
///
/// `max_depth` 是**最多检查几个目录（含起点）**，不是"最多上溯几层"。
///
/// `$HOME` 与 `.git` 都是**检查完当前目录之后**才停：`$HOME` 本身仍然是一个候选，
/// 所以 `~/Cargo.toml` 或 `~/.pmpx.toml` 能生效。我们从不走到 `$HOME` 之上。
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

        // 顺序即语义：`.git` 与 `$HOME` 都是检查完当前目录之后才停。
        if cfg.stop_at_git && current.join(".git").exists() {
            break StopReason::GitRoot;
        }
        if Some(&current) == home.as_ref() {
            break StopReason::Home;
        }

        match current.parent() {
            // `parent() == Some(self)` 表示到了文件系统根（`/` 或 `C:\`）
            Some(parent) if parent != current => {
                current = parent.to_path_buf();
                dirs.push(current.clone());
            }
            _ => break StopReason::FilesystemRoot,
        }
    };

    Walk { dirs, stopped }
}

/// 找出项目根。
///
/// `is_root` 由调用方给，通常是"这个目录里有 `.pmpx.toml`，或者有任一已装插件
/// 声明的 detect 文件"。做成参数是为了让本模块不认识插件。
pub fn find_project_root(
    start: &Path,
    cfg: &DiscoveryConfig,
    is_root: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    walk(start, cfg).dirs.into_iter().find(|d| is_root(d))
}

/// 收集所有 `.pmpx.toml`，**从近到远**。
///
/// 停止条件与 [`find_project_root`] 一致；这个顺序就是
/// [`crate::config::MergedProjectConfig`] 的"近者优先"合并口径。
pub fn collect_config_paths(start: &Path, cfg: &DiscoveryConfig) -> Vec<PathBuf> {
    walk(start, cfg)
        .dirs
        .into_iter()
        .map(|d| d.join(".pmpx.toml"))
        .filter(|p| p.is_file())
        .collect()
}

/// 把一个目录规整成可以直接比较的形式。
///
/// Windows 上 `C:\Users\me` 与 `C:\Users\me\` 是同一个目录但 `PathBuf` 不相等，
/// 而 `$HOME` 判断用的正是相等；`.` 与 `..` 也要消掉。
fn normalize(p: &Path) -> PathBuf {
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };

    // 不用 `canonicalize`：它会解析符号链接（/tmp → /private/tmp），
    // 让报出来的路径和用户看到的不一致。所以只做词法消解。
    lexical_normalize(&absolute)
}

/// 纯词法地把 `..` 与 `.` 消掉，不碰文件系统。
fn lexical_normalize(p: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // 消掉一层；已经在根就保留
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

    /// 造一棵目录树（`dirs` 是相对 `<tmp>` 的目录）。
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

        // 逐层上溯，一层都不能跳
        for pair in w.dirs.windows(2) {
            assert_eq!(
                pair[1].as_path(),
                pair[0].parent().unwrap(),
                "从 {} 应当走到它的父目录",
                pair[0].display()
            );
        }

        // 停在哪取决于测试跑在哪 —— 临时目录可能先撞上 max_depth，也可能先撞上 $HOME，
        // 所以只断言"是其中一个"，不写死一个。
        assert!(w.dirs.len() <= cfg().max_depth);
        assert!(
            matches!(
                w.stopped,
                StopReason::MaxDepth
                    | StopReason::FilesystemRoot
                    | StopReason::Home
                    | StopReason::GitRoot
            ),
            "意外停在 {:?}",
            w.stopped
        );
    }

    /// 上限足够大时，一定会走到 `$HOME` 或文件系统根 —— 不会无限上溯。
    #[test]
    fn a_generous_max_depth_still_terminates() {
        let tmp = tree(&["a/b/c"]);
        let mut c = cfg();
        c.max_depth = 4096;

        let w = walk(&tmp.path().join("a/b/c"), &c);
        assert!(
            matches!(w.stopped, StopReason::Home | StopReason::FilesystemRoot),
            "意外停在 {:?}",
            w.stopped
        );
        assert!(w.dirs.len() < 4096, "不该真的走满上限");
    }

    /// `walk_up = false`（或 `--no-walk-up`）：**只有起点这一个候选**。
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
        assert_eq!(w.stopped.describe(3), "到达 max_depth 上限（3 层）");
    }

    /// `.git` 之后不再往上 —— 仓库根再往上不属于本项目。
    #[test]
    fn stops_after_a_directory_containing_git() {
        let tmp = tree(&["repo/web/src"]);
        std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();

        let w = walk(&tmp.path().join("repo/web/src"), &cfg());

        assert_eq!(w.stopped, StopReason::GitRoot);
        // repo 是最后一个候选，**它本身要被检查**
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

        // 越过了含 .git 的 repo
        assert!(w.dirs.len() > 3);
        assert_ne!(w.stopped, StopReason::GitRoot);
    }

    /// `.git` 是个**文件**（worktree / submodule）时同样算数。
    #[test]
    fn git_file_also_stops_the_walk() {
        let tmp = tree(&["repo/web/src"]);
        touch(&tmp.path().join("repo/.git"));

        let w = walk(&tmp.path().join("repo/web/src"), &cfg());
        assert_eq!(w.stopped, StopReason::GitRoot);
    }

    #[test]
    fn find_project_root_picks_the_nearest_hit() {
        let tmp = tree(&["repo/web/src"]);
        touch(&tmp.path().join("repo/Cargo.toml"));
        touch(&tmp.path().join("repo/web/package.json"));

        let root = find_project_root(&tmp.path().join("repo/web/src"), &cfg(), |d| {
            d.join("Cargo.toml").exists() || d.join("package.json").exists()
        })
        .unwrap();

        assert!(root.ends_with("web"), "根应当是 web，实际 {root:?}");
    }

    #[test]
    fn find_project_root_returns_none_when_nothing_matches() {
        let tmp = tree(&["repo/some/dir"]);
        std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();

        let root = find_project_root(&tmp.path().join("repo/some/dir"), &cfg(), |d| {
            d.join("Cargo.toml").exists()
        });
        assert!(root.is_none());
    }

    #[test]
    fn the_start_directory_itself_can_be_the_root() {
        let tmp = tree(&["proj"]);
        touch(&tmp.path().join("proj/Cargo.toml"));

        let root = find_project_root(&tmp.path().join("proj"), &cfg(), |d| {
            d.join("Cargo.toml").exists()
        })
        .unwrap();
        assert_eq!(root, normalize(&tmp.path().join("proj")));
    }

    /// 配置能看到项目根之外的那一层。
    #[test]
    fn config_collection_reaches_above_the_project_root() {
        let tmp = tree(&["repo/crates/core/src"]);
        std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();
        touch(&tmp.path().join("repo/.pmpx.toml"));
        touch(&tmp.path().join("repo/crates/core/.pmpx.toml"));
        touch(&tmp.path().join("repo/crates/core/Cargo.toml"));

        let start = tmp.path().join("repo/crates/core/src");

        let root = find_project_root(&start, &cfg(), |d| d.join("Cargo.toml").exists()).unwrap();
        assert!(root.ends_with("core"));

        // 两份都要看得到，从近到远
        let cfgs = collect_config_paths(&start, &cfg());
        assert_eq!(cfgs.len(), 2, "{cfgs:?}");
        assert!(cfgs[0].ends_with("core/.pmpx.toml"));
        assert!(cfgs[1].ends_with("repo/.pmpx.toml"));

        assert!(
            cfgs.iter().any(|p| p.ends_with("repo/.pmpx.toml")),
            "项目根之上那一层的配置必须可见"
        );
    }

    #[test]
    fn config_collection_skips_directories_without_a_config() {
        let tmp = tree(&["repo/a/b/c"]);
        touch(&tmp.path().join("repo/.pmpx.toml"));

        let cfgs = collect_config_paths(&tmp.path().join("repo/a/b/c"), &cfg());
        assert_eq!(cfgs.len(), 1);
        assert!(cfgs[0].ends_with("repo/.pmpx.toml"));
    }

    #[test]
    fn walking_from_a_relative_path_works() {
        // 相对路径要先接到 cwd 上，否则 `..` 会被消错
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
