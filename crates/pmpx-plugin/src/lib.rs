//! # pmpx-plugin
//!
//! **pmpx 的插件契约**：一个 trait，加一条稳定的 C ABI，把 trait 安全地送过 `dlopen` 边界。
//!
//! 插件作者只需要实现 [`PackageManager`]，然后用一行 [`export!`](macro@crate::export)
//! 生成整个 C ABI 外壳：
//!
//! ```ignore
//! pub fn create() -> Box<dyn PackageManager> { Box::new(CargoPlugin) }
//! pmpx_plugin::export!(create);
//! ```
//! # 插件能做什么，不能做什么
//!
//! [`PackageManager::command`] 只应该依据**入参**做映射：不读文件（**包括 `project_root`
//! 下的**）、不写文件、不读环境变量、不起子进程、不发网络请求。这样 `command()` 是完全纯的
//! （单测不需要任何 fixture 目录），插件也没法借文件读取去探测不该知道的东西 —— "项目长什么
//! 样"由宿主的检测层决定，再通过 `matched` 这份声明式、可审计的白名单交给插件。
//! 穿过 [`abi`] 的数据一律是 `#[repr(C)]` 的 POD，所以**两边不需要同一个 rustc**；详见
//! [`abi`] 的模块文档。
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod abi;

mod export;

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

// ---- Family ----

/// 生态分组。决定宿主 `plugin ls` 的分组标题、`plugin set` 的作用域，以及项目 `.pmpx.toml`
/// 里 `[plugin] <family> = "..."` 的键名。
/// **开放类型**而不是封闭 enum：已知生态有常量，未知生态用 [`Family::new`] 扩展，比较与排序
/// 都按字符串走 —— 第三方插件支持新生态时不需要改这个 crate，更不需要等宿主发版。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Family(std::borrow::Cow<'static, str>);

impl Family {
    /// Node / 前端生态。
    pub const NODE: Family = Family(std::borrow::Cow::Borrowed("node"));
    /// Rust 生态。
    pub const RUST: Family = Family(std::borrow::Cow::Borrowed("rust"));
    /// Python 生态。
    pub const PYTHON: Family = Family(std::borrow::Cow::Borrowed("python"));
    /// Go 生态。
    pub const GO: Family = Family(std::borrow::Cow::Borrowed("go"));
    /// JVM 生态。
    pub const JVM: Family = Family(std::borrow::Cow::Borrowed("jvm"));
    /// .NET 生态。
    pub const DOTNET: Family = Family(std::borrow::Cow::Borrowed("dotnet"));
    /// PHP 生态。
    pub const PHP: Family = Family(std::borrow::Cow::Borrowed("php"));
    /// Ruby 生态。
    pub const RUBY: Family = Family(std::borrow::Cow::Borrowed("ruby"));

    /// 用一个自定义名字构造。
    pub fn new(name: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Family(name.into())
    }

    /// 键名形态。`plugin ls` 的分组、`.pmpx.toml` 的键都用它。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 人类可读的分组标题；未知生态原样返回。
    pub fn display(&self) -> &str {
        match self.as_str() {
            "node" => "Node / 前端",
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

/// pmpx 认可的动词。**封闭集合** —— 命令行上就这么多。
/// 编号与 [`abi`] 里的 `VERB_*` 常量一一对应，且**顺序不许改**（改了就要
/// [`abi::ABI_VERSION`] +1）；`abi` 模块里有测试钉住这件事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum Verb {
    /// 装依赖。无参 = 按锁文件装齐，带参 = 添加。
    Install = abi::VERB_INSTALL,
    /// 卸依赖。
    Remove = abi::VERB_REMOVE,
    /// 跑脚本 / 目标。
    Run = abi::VERB_RUN,
    /// 构建。
    Build = abi::VERB_BUILD,
    /// 测试。
    Test = abi::VERB_TEST,
    /// 更新依赖。
    Update = abi::VERB_UPDATE,
    /// 逃生舱：跑任意命令。不支持的插件应当明确报错，见 [`PackageManager::command`]。
    Exec = abi::VERB_EXEC,
}

impl Verb {
    /// 全部动词，按编号顺序。
    pub const ALL: &'static [Verb] = &[
        Verb::Install,
        Verb::Remove,
        Verb::Run,
        Verb::Build,
        Verb::Test,
        Verb::Update,
        Verb::Exec,
    ];

    /// 转成跨边界用的编号。
    pub const fn to_abi(self) -> u32 {
        self as u32
    }

    /// 从跨边界编号还原；不认识就返回 `None`（宿主与插件版本不一致时会走到这里）。
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

    /// 命令行上写的那个词。
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
            .ok_or_else(|| PluginError::other(format!("不认识的动词：{s}")))
    }
}

// ---- CommandSpec ----

/// 一条待执行的命令，**纯数据** —— 插件只描述"跑什么"，真正的 spawn 由宿主做，这样 stdio、
/// 环境、退出码的处理只有一处实现。构造是消费式链式的（`self` → `Self`），所以可以直接产出
/// 一个值：
/// ```ignore
/// let spec = CommandSpec::new("cargo").arg("add").args(args.iter()).cwd("/somewhere");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// 可执行文件。宿主会用 `which` 解析成真实路径，并按平台决定要不要包一层 `cmd /C`
    ///（Windows 上 `pnpm` 实际是 `pnpm.cmd`，直接 spawn 会失败）。
    pub program: OsString,

    /// 参数，按顺序。
    pub args: Vec<OsString>,

    /// 工作目录覆盖。`None` = 用宿主给的项目根。
    pub cwd: Option<PathBuf>,
}

impl CommandSpec {
    /// 指定可执行文件。
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
        }
    }

    /// 追加一个参数。
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// 追加一批参数；`args.iter()` 可以直接传进来且无损（`&OsString: Into<OsString>`），不会像
    /// `String` 那样在 Unix 上把非 UTF-8 参数改坏。
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// 覆盖工作目录。
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }
}

// ---- Context ----

/// 宿主传给插件的上下文。**只读。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// 项目根目录。**仅供拼日志 / 错误信息用** —— 不许拿它去读文件，见 crate 文档的约束表。
    pub project_root: PathBuf,

    /// 本次检测命中的文件，相对 `project_root`。
    /// **这是插件了解"项目长什么样"的唯一渠道。** 例：yarn 插件靠 `has_matched(".yarnrc.yml")`
    /// 区分 classic 与 berry，全过程不读一个文件。
    pub matched: Vec<String>,
}

impl Context {
    /// 命中的文件里有没有这一个 —— 插件做形态分支的标准写法。
    pub fn has_matched(&self, file: &str) -> bool {
        self.matched.iter().any(|m| m == file)
    }
}

// ---- PluginError ----

/// 插件能报出来的错误。
/// 刻意只有三种 —— 因为宿主只**需要**区分三种：不支持这个动词（它可以退化成裸透传）、入参
/// 不对、以及其它一切。人类可读的描述放在 payload 里，宿主原样打到 stderr。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginError {
    /// 这个后端不支持该动词。宿主对 `pmpx exec` 会退化成裸透传。
    UnsupportedVerb(Verb),

    /// 入参不合法。
    InvalidArgs(String),

    /// 其它任何问题。**也包括插件 panic** —— 宿主只需要知道"它炸了"。
    Other(String),
}

impl PluginError {
    /// 构造"不支持这个动词"。
    pub fn unsupported_verb(verb: Verb) -> Self {
        PluginError::UnsupportedVerb(verb)
    }

    /// 构造"入参不合法"。
    pub fn invalid_args(message: impl Into<String>) -> Self {
        PluginError::InvalidArgs(message.into())
    }

    /// 构造一个其它错误。
    pub fn other(message: impl Into<String>) -> Self {
        PluginError::Other(message.into())
    }

    /// 对应的跨边界错误码。
    pub fn code(&self) -> u32 {
        match self {
            PluginError::UnsupportedVerb(_) => abi::PMPX_ERR_UNSUPPORTED_VERB,
            PluginError::InvalidArgs(_) => abi::PMPX_ERR_INVALID_ARGS,
            PluginError::Other(_) => abi::PMPX_ERR_INTERNAL,
        }
    }

    /// 人类可读的描述。
    pub fn message(&self) -> String {
        match self {
            PluginError::UnsupportedVerb(v) => format!("不支持动词 {v}"),
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

// 手写而不用 thiserror：这个 crate 刻意零依赖，而 `Error` 需要的就只有下面这一行。
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

/// 一个包管理器后端的实现，纯同步接口：没有 `async`、没有回调、没有 I/O —— 跨 `dlopen` 边界
/// 传 `Future` 是这套方案里最脆的地方。它只应该做映射，约束见 crate 文档。
pub trait PackageManager: Send + Sync {
    /// 插件名，例如 `"cargo"`。宿主会拿它与 manifest 里声明的名字比对，不一致就拒绝加载。
    fn name(&self) -> &str;

    /// 所属生态。
    fn family(&self) -> Family;

    /// 把「动词 + 参数」翻译成一条具体命令。
    /// 不支持某个动词时返回 [`PluginError::UnsupportedVerb`]，**不要**去凑一个近似命令 ——
    /// 宿主对 `exec` 有降级处理，其余动词会原样报错，两种情况都比"猜一个"好。
    fn command(
        &self,
        ctx: &Context,
        verb: Verb,
        args: &[OsString],
    ) -> Result<CommandSpec, PluginError>;
}
