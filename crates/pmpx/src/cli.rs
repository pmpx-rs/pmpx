//! 命令行表面。
//!
//! 这个文件只做解析：把 `argv` 变成一个结构体。所有语义（检测、裁决、spawn）
//! 都在别处。

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// 全局参数。它们对所有子命令生效，所以标了 `global`。
#[derive(Debug, Parser)]
#[command(
    name = "pmpx",
    version,
    about = "一个命令面，识别项目类型，转发给真正的工具",
    long_about = None,
    arg_required_else_help = false,
)]
pub struct Cli {
    /// 临时指定插件，压过 `.pmpx.toml`
    ///
    /// 一次性覆盖，不落盘。要固化请用 `pmpx plugin set`。
    #[arg(short = 'p', long = "plugin", global = true, value_name = "NAME")]
    pub plugin: Option<String>,

    /// 在指定目录操作（等价于先 cd 过去）
    #[arg(short = 'C', long = "dir", global = true, value_name = "PATH")]
    pub dir: Option<PathBuf>,

    /// 只检查当前目录，不向上找项目根
    #[arg(long = "no-walk-up", global = true)]
    pub no_walk_up: bool,

    /// 关掉 stderr 上的提示（检测歧义、未安装候选等）
    #[arg(short = 'q', long = "quiet", global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// 子命令表。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 装依赖。无参 = 按锁文件装齐；带参 = 添加
    #[command(visible_aliases = ["i", "add"])]
    Install {
        /// 要添加的依赖；留空表示按锁文件装齐
        #[arg(value_name = "PKG")]
        packages: Vec<OsString>,
    },

    /// 卸依赖
    #[command(visible_aliases = ["rm", "uninstall"])]
    Remove {
        /// 要卸载的依赖
        #[arg(value_name = "PKG", required = true)]
        packages: Vec<OsString>,
    },

    /// 跑脚本 / 目标
    #[command(visible_alias = "r")]
    Run {
        /// 脚本名 / 目标名。cargo 下通常留空
        #[arg(value_name = "TARGET")]
        target: Option<OsString>,

        /// `--` 之后的内容原样传给后端，不做任何解释
        #[arg(last = true, value_name = "ARG")]
        args: Vec<OsString>,
    },

    /// 构建
    #[command(visible_alias = "b")]
    Build {
        /// `--` 之后的内容原样传给后端
        #[arg(last = true, value_name = "ARG")]
        args: Vec<OsString>,
    },

    /// 测试
    #[command(visible_alias = "t")]
    Test {
        /// `--` 之后的内容原样传给后端
        #[arg(last = true, value_name = "ARG")]
        args: Vec<OsString>,
    },

    /// 更新依赖
    #[command(visible_alias = "up")]
    Update {
        /// 只更新这些依赖；留空表示全部
        #[arg(value_name = "PKG")]
        packages: Vec<OsString>,
    },

    /// 逃生舱：跑任意命令。插件不支持时 pmpx 自己做裸透传
    #[command(visible_alias = "x")]
    Exec {
        /// 命令与它的参数
        #[arg(
            value_name = "CMD",
            required = true,
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        command: Vec<OsString>,
    },

    /// 详细：项目根、候选与得分、插件版本、ABI 诊断
    Info,

    /// 管理插件
    #[command(subcommand)]
    Plugin(PluginCommand),

    /// 读写全局配置
    #[command(subcommand)]
    Config(ConfigCommand),

    /// 输出 shell 补全脚本到 stdout，用户自己重定向
    Completion {
        /// 目标 shell
        #[arg(value_name = "SHELL")]
        shell: clap_complete::Shell,
    },
}

/// `pmpx plugin <...>`
#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    /// 按生态分组列出已安装的插件（只读 manifest，不 dlopen）
    #[command(visible_alias = "list")]
    Ls {
        /// 平铺输出，不分组
        #[arg(long)]
        flat: bool,
    },

    /// 各生态的当前插件，以及候选与得分
    Current,

    /// 把某个插件固化到 `.pmpx.toml`（只影响它所属的那个生态）
    Set {
        /// 插件名，例如 `pnpm`
        #[arg(value_name = "NAME")]
        name: String,
    },

    /// 删掉 `.pmpx.toml` 里的固化项
    Unset {
        /// 只删这个生态；留空表示全部（此时要求 `--yes`）
        #[arg(value_name = "FAMILY")]
        family: Option<String>,

        /// 确认删除全部
        #[arg(long)]
        yes: bool,
    },

    /// 装插件
    Add {
        /// 插件名，例如 `pnpm`（会展开成 `pmpx-plugin-pnpm`）
        #[arg(value_name = "NAME", required = true)]
        names: Vec<String>,

        /// 指定版本；留空表示最新
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
    },

    /// 卸插件
    Rm {
        /// 插件名
        #[arg(value_name = "NAME", required = true)]
        names: Vec<String>,
    },

    /// 更新插件
    Update {
        /// 只更新这个插件；留空表示全部
        #[arg(value_name = "NAME")]
        names: Vec<String>,

        /// 指定版本；只能与单个插件名同用
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
    },

    /// 在 crates.io 上搜索插件
    Search {
        /// 关键词
        #[arg(value_name = "KEYWORD", required = true)]
        keyword: String,

        /// 最多显示几条
        #[arg(long, default_value_t = 20, value_name = "N")]
        limit: usize,
    },

    /// 看某个插件在 crates.io 上的信息
    Info {
        /// 插件名
        #[arg(value_name = "NAME")]
        name: String,
    },
}

/// `pmpx config <...>`
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// 读一个键。支持点号路径，例如 `plugin.family_priority`
    Get {
        /// 键名
        #[arg(value_name = "KEY")]
        key: String,
    },

    /// 写一个键。只写全局配置，绝不碰项目里的 `.pmpx.toml`
    Set {
        /// 键名
        #[arg(value_name = "KEY")]
        key: String,

        /// 值。会按现有类型解析（数组 / 布尔 / 数字 / 字符串）
        #[arg(value_name = "VALUE")]
        value: String,
    },
}

/// 输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// 人类可读
    Text,
    /// 机器可读
    Json,
}
