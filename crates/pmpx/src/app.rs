//! 把各层接起来：一次运行的上下文，以及从 argv 到"跑一条命令"的完整流程。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use crate_plugin_kit::{CratePluginKit, KitConfig};
use pmpx_plugin::abi::PmpxPluginV1;
use pmpx_plugin::Verb;

use crate::cli::Cli;
use crate::config::{GlobalConfig, MergedProjectConfig};
use crate::detect::{self, DetectFailure, ScoredPlugin, Selection};
use crate::discovery::{self, StopReason, Walk};
use crate::error::PmpxError;
use crate::plugins::PluginSet;
use crate::runtime::{Backend, BackendError};
use crate::spawn;

/// 一次运行的全部上下文。一次运行读一次配置。
///
/// 没有"按需读"的空间：`[discovery]` 影响所有命令，包括那些看起来跟插件无关的。
pub struct Session {
    /// 起点目录（`-C` 指定的，或 cwd）。
    pub start_dir: PathBuf,
    /// 全局配置。
    pub global: GlobalConfig,
    /// 合并后的项目配置（多层 `.pmpx.toml`）。
    pub project: MergedProjectConfig,
    /// 已安装插件的清单（只读 manifest，没有 dlopen）。
    pub plugins: PluginSet,
    /// 插件库句柄。
    pub kit: CratePluginKit<PmpxPluginV1>,
    /// 上溯走过的目录与停止原因。
    pub walk: Walk,
    /// 项目根。在 `open` 里就算好 —— 它是"在哪执行"的唯一答案，不每次重算。
    pub project_root: Option<PathBuf>,
    /// `-p/--plugin` 的值。
    pub wanted_plugin: Option<String>,
    /// `--quiet`：关掉 stderr 上的提示。
    pub quiet: bool,
}

impl Session {
    /// 读配置、扫插件、走上溯。不选插件，也不加载任何代码。
    pub fn open(args: &Cli) -> Result<Self> {
        let start_dir = resolve_start_dir(args.dir.as_deref())?;
        let global = GlobalConfig::load()?;

        // `--no-walk-up` 只能收紧 `[discovery] walk_up`，不能放宽。
        let mut discovery_cfg = global.discovery.clone();
        if args.no_walk_up {
            discovery_cfg.walk_up = false;
        }

        let data_dir = global.plugin_store.effective_data_dir()?;
        let mut kit_cfg = KitConfig::new("pmpx").with_data_dir(&data_dir);

        kit_cfg.prefer_prebuilt = global.plugin_store.effective_prefer_prebuilt();
        let kit = CratePluginKit::<PmpxPluginV1>::new(kit_cfg)
            .with_context(|| format!("初始化插件库失败：{}", data_dir.display()))?;

        let plugins = PluginSet::load(&kit)?;

        let project_root =
            discovery::find_project_root(&start_dir, &discovery_cfg, |d| plugins.marks_root(d));
        let config_paths = discovery::collect_config_paths(&start_dir, &discovery_cfg);
        let project = MergedProjectConfig::from_paths_near_to_far(&config_paths)?;

        // 上溯路径与停止原因 —— 给 `info` 与"为什么没找到"用
        let walk = discovery::walk(&start_dir, &discovery_cfg);

        Ok(Self {
            start_dir,
            global,
            project,
            plugins,
            kit,
            walk,
            project_root,
            wanted_plugin: args.plugin.clone(),
            quiet: args.quiet,
        })
    }

    /// 项目根。找不到就是找不到 —— 不退回 cwd。
    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// 检测不到项目时的报错（退出码 3）。
    ///
    /// `hints` 那张静态表只陈述事实，不推荐装哪个插件。
    pub fn no_project_error(&self) -> PmpxError {
        let mut msg = format!("在 {} 里检测不到项目类型。", self.start_dir.display());

        // 上溯停在哪 —— 这决定了是"找过了但没有"还是"根本没找几层"
        msg.push_str(&format!(
            "\n上溯了 {} 个目录，停止原因：{}。",
            self.walk.dirs.len(),
            self.walk.stopped.describe(self.global.discovery.max_depth)
        ));

        if self.walk.stopped == StopReason::WalkUpDisabled {
            msg.push_str(
                "\n（`--no-walk-up` 或全局配置的 `[discovery] walk_up = false` 关掉了上溯）",
            );
        }

        let installed: Vec<String> = self.plugins.usable().map(|p| p.name.clone()).collect();
        if installed.is_empty() {
            msg.push_str(
                "\n一个插件都没装。pmpx 靠**已安装插件**声明的特征文件来判断项目类型，\
                 所以现在任何项目都检测不出来。",
            );
        } else {
            msg.push_str(&format!("\n已装插件：{}", installed.join(", ")));
        }

        let hints = crate::hints::probe(&self.start_dir);
        if !hints.is_empty() {
            msg.push_str("\n\n这里的文件看起来像：");
            for h in &hints {
                msg.push_str(&format!("\n  · {}（{}）", h.family, h.matched.join(", ")));
            }
        }

        msg.push_str(
            "\n\n可以在当前目录写一个 .pmpx.toml 显式声明，例如：\n  [plugin]\n  rust = \"cargo\"",
        );

        PmpxError::not_found(msg)
    }

    /// 裁决出该用哪个插件。
    pub fn select(&self, root: &Path) -> std::result::Result<Selection, DetectFailure> {
        detect::select(
            &self.plugins,
            root,
            &self.project,
            &self.global,
            self.wanted_plugin.as_deref(),
        )
    }

    /// 打印裁决过程中的提示。`--quiet` 关掉它们。
    pub fn emit_notes(&self, selection: &Selection) {
        if self.quiet {
            return;
        }
        for note in &selection.notes {
            eprintln!("pmpx: {note}");
        }
        if !selection.notes.is_empty() {
            eprintln!(
                "pmpx: 用 `pmpx -p <name>` 临时覆盖，或 `pmpx plugin set {}` 固化到 .pmpx.toml",
                selection.name
            );
        }
    }

    /// 项目根里命中选中插件声明的那些文件 —— 这就是送进 `Context::matched` 的东西。
    ///
    /// 相对 `project_root`，已排序去重。
    pub fn matched_for(&self, root: &Path, selection: &Selection) -> Vec<String> {
        let Some(plugin) = self.plugins.by_crate_name(&selection.crate_name) else {
            return Vec::new();
        };
        ScoredPlugin::score(plugin, root)
            .all_hits()
            .map(str::to_string)
            .collect()
    }

    /// 加载选中的插件。
    pub fn load_backend(&self, selection: &Selection) -> crate::error::Result<Backend> {
        let Some(plugin) = self.plugins.by_crate_name(&selection.crate_name) else {
            return Err(PmpxError::not_found(format!(
                "插件 {} 不在清单里（两次读之间被删了？）",
                selection.crate_name
            )));
        };
        Backend::load(&self.kit, plugin)
    }
}

/// 算出起点目录。
///
/// `-C` 指定的目录必须存在，否则是一条明确的用法错误 —— 悄悄退回 cwd 会让
/// "我在别的目录跑了命令"变得难以察觉。
fn resolve_start_dir(dir: Option<&Path>) -> Result<PathBuf> {
    match dir {
        Some(d) => {
            let abs = if d.is_absolute() {
                d.to_path_buf()
            } else {
                std::env::current_dir()?.join(d)
            };
            if !abs.is_dir() {
                anyhow::bail!("-C 指定的目录不存在：{}", abs.display());
            }
            Ok(abs)
        }
        None => Ok(std::env::current_dir()?),
    }
}

/// 把一次动词调用跑到底：裁决 → 加载 → 问插件 → spawn → 透传退出码。
///
/// `allow_exec_fallback` 是唯一例外：只有 `exec` 为真，插件不支持时会退化成
/// pmpx 自己做裸透传；其余六个动词维持"不支持就报错"。
pub fn run_verb(
    session: &Session,
    verb: Verb,
    args: &[OsString],
    allow_exec_fallback: bool,
) -> crate::error::Result<u8> {
    let root = match session.project_root() {
        Some(r) => r.to_path_buf(),
        None => {
            // `exec` 是逃生舱：零插件也要能用。没有项目根就退回起点目录。
            if allow_exec_fallback {
                let cwd = session.start_dir.clone();
                return passthrough(&cwd, args);
            }
            return Err(session.no_project_error());
        }
    };

    let selection = match session.select(&root) {
        Ok(s) => s,
        Err(failure) => {
            if allow_exec_fallback {
                return passthrough(&root, args);
            }
            // `DetectFailure` 都是退出码 3
            return Err(failure.into());
        }
    };

    session.emit_notes(&selection);

    let backend = session.load_backend(&selection)?;
    let matched = session.matched_for(&root, &selection);

    match backend.command(&root, &matched, verb, args) {
        Ok(Ok(spec)) => {
            let cwd = spec.cwd.clone().unwrap_or_else(|| root.clone());
            spawn::run(&spec, &cwd)
        }

        Ok(Err(BackendError::UnsupportedVerb)) if allow_exec_fallback => passthrough(&root, args),

        Ok(Err(e)) => Err(PmpxError::Backend(
            format!("{} 做不到 `{verb}`：{e}", selection.name),
            e.exit_code(),
        )),

        Err(e) => Err(e),
    }
}

/// 裸透传：直接跑用户给的那条命令，cwd = 项目根。
///
/// 这是"不支持就报错"的唯一例外，只在 `exec` 上发生。
fn passthrough(cwd: &Path, args: &[OsString]) -> crate::error::Result<u8> {
    let Some((program, rest)) = args.split_first() else {
        return Err(PmpxError::Usage(
            "`pmpx exec` 需要一个命令，例如 `pmpx exec ls`".to_string(),
        ));
    };

    let spec = pmpx_plugin::CommandSpec {
        program: program.clone(),
        args: rest.to_vec(),
        cwd: Some(cwd.to_path_buf()),
    };
    spawn::run(&spec, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_start_dir_rejects_a_missing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");

        let err = resolve_start_dir(Some(&missing)).unwrap_err();
        assert!(err.to_string().contains("-C"), "{err}");
    }

    #[test]
    fn resolve_start_dir_accepts_an_existing_one() {
        let tmp = tempfile::tempdir().unwrap();
        let got = resolve_start_dir(Some(tmp.path())).unwrap();
        assert_eq!(got, tmp.path());
    }

    #[test]
    fn resolve_start_dir_makes_relative_paths_absolute() {
        let got = resolve_start_dir(Some(Path::new("."))).unwrap();
        assert!(got.is_absolute());
    }
}
