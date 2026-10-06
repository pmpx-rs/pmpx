# pmpx 重构：基于能力的插件 ABI 与八 crate 工作区

状态：**设计已冻结，实现已开始。** 正在进行的是 §6 的第 2 步：`crates/pmpx-plugin-abi` 已落地（零依赖、`no_std`，并带上一份自动生成、由测试守卫的 ABI 表面快照与 C 头），接下来是 `pmpx-plugin` v3 与 `pmpx-loader`。宿主目前仍是 `crates/pmpx`（单个二进制 crate）。

范围：一次**有意的不兼容重构**。向后兼容不是约束——插件作者需要重新编译，任何现有行为都不神圣。凡是"砍掉"或"重新决定"的能力，本文都会明说。

读者：实现它的人（或机器），以及决定"这套拆法值不值"的人。

---

## 1. 四条原则

**P1 —— 引擎是一条值流水线。** 一次运行是一串普通数据：

```
Options → Roots → Evidence → Decision → Invocation → Plan → Outcome
```

插件只在 `Invocation` 这一步被触碰。它之前不需要 `dlopen`，它之后不需要文件系统。今天这些阶段被融合在一个 `Session` 上帝对象里，而这个对象还同时持有全局配置、插件商店、遍历状态和 CLI 的 `quiet` 标志。

**P2 —— `unsafe` 只存在于 ABI 层。** 原始 ABI（`pmpx-plugin-abi`）、宿主加载器（`pmpx-loader`），以及契约里负责编解码的 `export` / `marshal` / `dispatch`。宿主侧的每个 crate 都 `#![forbid(unsafe_code)]`，并由 CI 强制。

**P3 —— 用能力协商，而不是一个整数版本号。** 宿主向插件索取具名能力，插件也向宿主索取；新增能力是**追加式**的，不需要整个生态重编译。"ABI 主版本号"仍然存在，但只用来守**语义**变化（某个键的含义变了、某个布局变了）。

**P4 —— 库不打印，也不认识生态。** 任何库里都不出现 `anstream`。宿主里不出现任何包管理器名称、参数或标记文件名：项目是什么、插件能看到什么，全部由插件自己的 manifest 声明。

---

## 2. 工作区布局

| crate | 类型 | MSRV | 直接依赖 | `unsafe` |
| --- | --- | --- | --- | --- |
| `crates/pmpx-plugin-abi` | lib，可 `no_std` | 1.82 | **无** | ✅（孤岛之一） |
| `crates/pmpx-plugin` | lib（插件作者用） | 1.82 | `pmpx-plugin-abi` | 仅外壳模块 |
| `crates/pmpx-loader` | lib（宿主侧 ABI） | 1.88 | `pmpx-plugin-abi`、`libloading` | ✅（孤岛之一） |
| `crates/pmpx-project` | lib | 1.88 | 无 | ❌ |
| `crates/pmpx-detect` | lib（纯函数） | 1.88 | 无 | ❌ |
| `crates/pmpx-engine` | lib | 1.88 | loader、project、detect、`serde`、`toml`、`directories`、`which`、`thiserror`；**可选** `crate-plugin-kit`（`store` 特性） | ❌ |
| `crates/pmpx` | bin（+ 供测试用的 lib） | 1.88 | engine（启用 `store`）、`clap`、`clap_complete`、`anstream`、`anstyle`、自更新栈 | ❌ |
| `crates/pmpx-testkit` | lib（dev-dependency） | 1.88 | `pmpx-plugin`、`pmpx-project`、`pmpx-detect`，可选 engine | ❌ |

```
pmpx-plugin-abi ←── pmpx-plugin ←──── pmpx-testkit
        ↑                                ↑
        └── pmpx-loader ──┐              │（可选，用于真实 fixture）
                          ├─ pmpx-engine ┘
pmpx-project ─┬───────────┘
pmpx-detect ──┘
                          pmpx-engine ←── pmpx (CLI)
```

规则，全部可机械检查（见 §7）：

1. 依赖只能单向。`pmpx-plugin-abi` 永远不依赖任何东西。
2. 任何库都不得**直接**依赖 `clap`、`anstream`、`anstyle`、`ureq`、`serde_json`、`tar`、`zip`、`flate2`。这些属于 CLI。
3. `unsafe` 只允许出现在 ABI 层：`pmpx-plugin-abi`、`pmpx-loader`，以及契约 crate 里负责编解码的那几个模块（`export` / `marshal` / `dispatch`）。宿主侧的一切（`pmpx-project`、`pmpx-detect`、`pmpx-engine`、CLI）一律 `#![forbid(unsafe_code)]` —— "引擎里不出现裸指针"才是这条规则真正要保住的东西。
4. 每个库都只产出"值 + 事件"；只有 `pmpx` 往终端写字。

表里有两个刻意的决定，值得单独说明：

- **`pmpx-loader` 用 `libloading`，不用 `crate-plugin-kit`。** 不安全孤岛应该只依赖又小又可审计的东西，并且**接手的是一个明确的库文件路径**：*找到*插件的库是商店的职责（平台命名规则、安装布局都在那儿）。kit 的策略层——registry、预编译包与源码构建、跨进程锁——不进入 ABI 层。
- **商店是一个特性（feature），不是核心能力。** `crate-plugin-kit` 为了访问 registry 会引入 `ureq` / `rustls` / `ring`；如果内嵌者只想要"检测 + 路由"，不该被顺带拖进一套网络栈。关掉 `store` 时，`pmpx-engine` 自己解析插件声明（`pmpx.toml` 本来就是 TOML），照常完成检测、加载与执行——没有 kit，也没有网络。安装与更新插件才需要 kit，CLI 永远打开这个特性。

### 为什么不再多拆

`config` / `plugins` / `exec` 保持为 **`pmpx-engine` 内部的模块**：它们只有一个消费者，而引擎的关键不变量——"检测必须在 `dlopen` 之前完成"、"真正的调用即将发生之前不碰磁盘"、"宿主从不释放插件内存"——横跨它们。把它们切成 crate，只会把这些不变量降级成跨 crate 的君子协定。**出现第二个消费者再抽 crate，不要提前。**

### 为什么不要 `pmpx-share`

因为没有东西可共享。每个候选都有主人：样式与前缀属于 CLI，配置路径属于引擎，原子写属于引擎的 config 模块，版本解析属于自更新，PATH 解析属于引擎的 exec 模块。一个叫 `share` / `utils` 的 crate 只会把"这个概念归谁"这个问题藏到一个依赖磁铁背后——而 `pmpx-plugin-abi` / `pmpx-plugin` 必须保持零依赖，所以它对插件作者毫无帮助。当两个 crate 真的需要一个共同概念时，共享它的方式是**依赖倒置**（trait 归调用方，实现归 CLI），或者一个**窄而按概念命名**的 crate（`pmpx-term`、`pmpx-env`），绝不是一个大杂烩。

---

## 3. ABI v3

本章的一切都在 `pmpx-plugin-abi`（原始层）里，再由 `pmpx-plugin` 包装成类型化 API。

### 3.1 什么可以跨界

与今天相同，但因为后面的一切都建立在它上面，值得重申：

- 只允许 `#[repr(C)]` POD 与整数：没有 `String`、`Vec`、trait object 或 `toml::Value`。
- 内存由分配它的一侧释放：宿主释放自己分配的，插件通过自己的 `free` 能力释放自己的输出。
- 借出的视图只在一次调用期间有效，不得保存。
- panic 不得跨界：`export!` 生成的外壳在每个入口点都保留 `catch_unwind`。
- **每个结构体都以 `size: usize` 打头**——是**构造方**构造出的大小，而不是读取方期望的大小。读取方不得触碰超出 `size` 的字段。

### 3.2 上下文是键值袋，不是会不断变长的结构体

今天加一项上下文就要加一个字段，就要换一次结构体布局，就要升一次 `ABI_VERSION`，就要让生态里每个插件重编译。这是当前设计里最昂贵的一条性质，而它就此消失：

```rust
/// 宿主知道的关于本次调用的一切。
///
/// 标量放字段里，因为它们便宜且总有；其余全部通过访问器取得，
/// 于是新增一个键是追加式的，不需要升版本。
#[repr(C)]
pub struct PmpxContext {
    /// 宿主编译这个结构体时的大小。
    pub size: usize,
    /// 动词编号，见 `PMPX_VERB_*`。
    pub verb: u32,
    /// 为什么选中这个插件，见 `PMPX_REASON_*`。
    pub reason: u32,
    /// 它获胜时的证据分数。
    pub score: u32,

    /// `key` 有几个值。宿主有的标量是 1；宿主没有的一律是 0。
    pub count: unsafe extern "C" fn(*const PmpxContext, key: PmpxStr) -> usize,
    /// `key` 的第 `index` 个值；没有就返回空的 `PmpxStr`。
    pub get: unsafe extern "C" fn(*const PmpxContext, key: PmpxStr, index: usize) -> PmpxStr,
    /// `key` 的第 `index` 个**名字**：映射的键（pin 的家族、脚本的名字），不是值。
    pub name: unsafe extern "C" fn(*const PmpxContext, key: PmpxStr, index: usize) -> PmpxStr,
}
```

主版本 3 的键：

| 键 | `count` | `get(i)` | `name(i)` |
| --- | --- | --- | --- |
| `project.root` | 0/1 | 绝对路径（字节） | — |
| `project.start_dir` | 0/1 | 绝对路径（字节） | — |
| `project.matched` | n | 相对项目根的文件名 | — |
| `project.config_files` | n | 绝对路径，由近到远 | — |
| `args` | n | 用户原样输入的参数 | — |
| `config.pin` | n | 被 pin 的插件名 | 家族 |
| `file.<声明的名字>` | 0/1 | 文件字节 | — |

> `[scripts]`（项目配置里的命名命令）**不进这个键表**：它是用户的便利，由宿主侧解析执行，
> 插件没有任何理由知道它。

三条要写清楚的性质：

- **未知的键不是错误。** 插件向一个不知道这个键的宿主索取，会得到 `0` / 空指针，并且必须把这理解为"宿主不知道这件事"——`start_dir` 在宿主无话可说时也是同样的表现。
- **`file.<名字>` 是惰性的，且上限由声明决定。** 宿主只在插件真的索取时才去读那个被声明的文件，所以一个 manifest 列了很多文件在没人要之前不花任何代价，而当前"急切组数组"带来的上限与静默丢弃问题一并消失。插件**没有**声明过的名字返回空指针：**声明依旧是白名单**。
- **这些键字符串只由插件书写。** 常量放在 `pmpx-plugin-abi` 里，于是 Rust 侧的拼写错误是编译错误；手写 C 插件拼错一个键会**失败在安全的方向**（它看到的是"不存在"）。

这里没有任何东西规定宿主是谁：第二个宿主实现可以回答同样的键。

### 3.3 用能力协商取代固定 vtable

```rust
/// 插件：一次查找，加上它会的那些事。
#[repr(C)]
pub struct PmpxPlugin {
    /// 这个插件编译时所依据的**键与能力**的语义版本。
    pub abi_major: u32,
    /// 仅用于诊断。
    pub rustc_version: PmpxStr,
    /// 仅用于诊断。
    pub target: PmpxStr,
    /// 返回 `name` 对应的、以 `size` 打头的函数表指针；没有就是空。
    pub capability: unsafe extern "C" fn(name: PmpxStr) -> *const core::ffi::c_void,
}
```

宿主**必需**的能力（缺一个就不能用，且拒绝信息会指出缺的是哪一个）：

| 能力 | 函数表 | 内容 |
| --- | --- | --- |
| `identity` | `PmpxIdentity` | `name() -> PmpxStr`、`family() -> PmpxStr`、`free_str(PmpxStr)` |
| `command` | `PmpxCommandCap` | `run(ctx: *const PmpxContext, out: *mut PmpxCommand) -> u32`、`free_command(*mut PmpxCommand)` |

**可选**、由宿主在插件提供时调用的能力：

| 能力 | 函数表 | 内容 |
| --- | --- | --- |
| `attach` | `PmpxAttach` | `attach(host: *const PmpxHost)`——插件借此拿到宿主的钩子 |

宿主也有自己的表，于是这次交换是对称的：

```rust
#[repr(C)]
pub struct PmpxHost {
    pub abi_major: u32,
    pub size: usize,
    pub capability: unsafe extern "C" fn(name: PmpxStr) -> *const core::ffi::c_void,
}
```

| 能力 | 函数表 | 内容 |
| --- | --- | --- |
| `log` | `PmpxLog` | `write(level: u32, message: PmpxStr)`、`max_level: u32` |

**入口符号名随根结构布局变化（现在是 `pmpx_plugin_entry_v3`）。** 它比任何版本检查都更早生效：旧宿主找自己认识的那个名字会一无所获，于是得到"这个插件是按另一版契约构建的，需要重新编译"，而不是把一个移动过的字段当函数指针读。新增能力或键都不动它，只有 `PmpxPlugin` 本身的布局变化才动。

**兼容规则。** 宿主在下列情况下拒绝插件：`abi_major` 不同；某个**必需**能力缺失；某个能力指向的表小于宿主的期望。它**不会**因为插件多了能力而拒绝，也不会因为某个它用不到的键缺失而拒绝。于是新增一个能力、一个键、或函数表**末尾**的一个字段都是追加式的：不升主版本，不重编译。

**什么算破版，什么不算（冻结）**

| 变化 | 判定 |
| --- | --- |
| 结构体 / 函数表**末尾**追加字段 | 追加，不升版 |
| 新增一个键 | 追加，不升版 |
| 新增一个能力 | 追加，不升版 |
| 新增一个动词编号 | 追加，不升版（**前提见下**） |
| 新增一个错误码 | 追加，不升版（**前提见下**） |
| 键改名 / 删除 / 含义变化 | **升版** |
| 字段的类型、顺序、插入位置变化 | **升版** |
| 函数表布局变化（不是末尾追加：插字段、改类型） | **升版** |
| 动词、错误码的编号或含义变化 | **升版** |
| `PmpxStr` / `PmpxSlice` 这两个基础类型变化 | **升版** |
| 调用语义变化（谁分配谁释放、借出视图的生命周期、`command` 可否被重复调用） | **升版** |

表里那两条"前提"是**契约义务**，必须写进插件与宿主两侧的文档，否则新增动词/错误码就不是追加式的：

- 插件遇到**不认识的动词编号**，要回答 `unsupported`，而不是 `invalid args`——这样宿主在
  `exec` 上的降级路径仍然生效；
- 宿主遇到**不认识的错误码**，要当成 `internal` 处理，不得崩溃或误读。

**机制（不靠自觉）。** `pmpx-plugin-abi` 里提交一份机器可读的**表面对照快照**
（`abi/surface.toml`：键清单、能力名与函数表布局、结构体字段偏移与大小、动词编号、错误码，
外加一份人工维护的"语义变化"清单）。CI 里有一个测试重新生成快照并与提交版对比：

- 差异被判定为**追加** → 允许，快照随之更新；
- 差异被判定为**破坏** → 测试失败，**除非**同一个提交里 `abi_major` 递增；
- "语义变化"清单有任何改动 → 同样要求递增（人工维护，机器强制）。

### 3.4 日志

`log` 是宿主能力，`attach` 是插件找到它的方式。行为与今天的设计一致，这部分值得保留：

- 插件调用 `debug!` / `info!` / `warn!` / `error!` / `context()`，自己从不打印；
- **级别门控在宿主的表里**（`max_level`），插件在**格式化之前**先查它——所以不做 trace 的宿主一分钱不花；
- 插件 ID 与格式属于宿主，于是 `--debug` 永远不会成为插件能拿来分支的输入；
- 没有宿主时（插件自己的 `cargo test`），宏回退到 stderr。

`pmpx-plugin` 必须**先检查函数表的 `size`** 再读 `max_level`——这是当前代码还欠的一笔账（见 §5）。

### 3.5 C 头文件，以及非 Rust 插件

`pmpx-plugin-abi` 是纯 POD、不含 Rust 专有类型，因此**自己**就能生成 C 头文件：`cargo run -p pmpx-plugin-abi --example gen-abi-files` 写出 `include/pmpx_plugin.h`（连同 `abi/surface.toml`，两者都**提交进仓库**，由测试与 CI 检查是否最新）。不用 `cbindgen`：布局数字直接从 `size_of` / `offset_of!` 算出，并在头文件里以 `_Static_assert` 表达，于是 C 编译器本身就是布局的独立验证者。这才让"用 C、Zig 或 Go 写插件"成为受支持的目标而不是空想：五个函数、一次查找、同一套所有权规则。Rust 专有的部分（trait、`export!`、类型化 `Context`）留在 `pmpx-plugin` 里，那些作者只是用不到而已。

---

## 4. 流水线

每个阶段是某个 crate 里的模块；下面这些类型就是阶段之间的接口。

```rust
// pmpx-project —— IO 外壳。产出证据，不做任何判断。
pub struct WalkPolicy { pub walk_up: bool, pub max_depth: usize, pub stop_at_git: bool }
pub struct Roots {
    pub dirs: Vec<PathBuf>,          // 由近到远
    pub stopped: StopReason,
    pub root: Option<PathBuf>,
    pub configs: Vec<PathBuf>,       // 沿途找到的 .pmpx.toml
}
pub struct Evidence { pub markers: BTreeSet<String> }   // 在根目录看到的标记文件名
pub fn probe(dir: &Path, markers: &BTreeSet<String>) -> Evidence;
pub struct ReadLimits { pub per_file: u64, pub files: usize }
pub fn read_declared(names: &[String], root: &Path, limits: ReadLimits) -> FileBag;

// pmpx-detect —— 纯函数。没有文件系统、没有 ABI、没有策略默认值。
pub struct Declaration { pub crate_name: String, pub name: String, pub family: String,
                         pub strong: Vec<String>, pub weak: Vec<String>, pub wants: Vec<String> }
pub struct Policy { pub family_priority: Vec<String>, pub priority: Vec<String>, pub pin_floor: u32 }
pub struct Decision { pub plugin: Declaration, pub family: String, pub score: u32,
                      pub reason: Reason, pub matched: Vec<String>, pub candidates: Vec<Candidate>,
                      pub notes: Vec<String> }
pub fn decide(evidence: &Evidence, declared: &[Declaration], policy: &Policy,
              explicit: Option<&str>) -> Result<Decision, DetectFailure>;

// pmpx-engine —— 策略、商店、执行，以及唯一的门面。
pub struct Settings { /* 环境变量 + 全局配置 + 分层项目配置 + CLI 覆盖，一次性解析完 */ }
pub struct Plan { pub program: OsString, pub args: Vec<OsString>, pub cwd: PathBuf,
                  pub source: PlanSource, pub decision: Decision }
pub enum PlanSource { Plugin, Passthrough, Alias }
pub enum Outcome { Exited(u8), Signalled(u8), Passthrough(u8) }
pub enum Event { Phase { name: &'static str, elapsed: Duration, detail: String },
                 Note { text: String }, Plugin { level: u32, text: String },
                 Plan(Plan), Finished(Outcome) }

impl Engine {
    pub fn open(options: Options, events: &dyn EventSink) -> Result<Engine>;
    pub fn settings(&self) -> &Settings;
    /// 除了"真的跑"以外的一切：决策与将要执行的命令，插件不会被调用两次，进程不会启动。
    pub fn plan(&self, verb: Verb, args: &[OsString]) -> Result<Plan>;
    pub fn run(&self, plan: &Plan) -> Result<Outcome>;
}
```

`plan` 正是当前代码给不出的阶段，因为"问插件"和"跑它的回答"挤在同一个函数里。有了它：`pmpx --explain --json`、编辑器集成、CI 里断言命令而不真的执行、以及一个使用**真实检测**的插件测试台。

`pmpx`（CLI）于是很薄：解析 argv → `Engine::open` → `plan` → 打印 → `run` → 把 outcome 映射成退出码。所有给人看的字符串都归它。

---

## 5. 被删除或重新决定的东西

| 今天 | 之后 | 为什么 |
| --- | --- | --- |
| `hints.rs`——宿主里 8 个生态的标记文件表 | **删除** | 宿主不该认识生态；"没有装插件"就给一句诚实的话。如果确实舍不得首次运行的提示，它以**数据文件**的形式回来（用户或发行版提供），而不是编译进二进制 |
| 契约里的 `Family::display()` 与家族常量 | 搬到 CLI | `"Node / frontend"` 是 `plugin ls` 的标题，不是契约材料；契约只保留不透明的家族名 |
| `ABI_VERSION` 相等检查 | 能力协商（§3.3） | 往 ABI 里加任何东西都不再需要重编译整个生态 |
| `PmpxContextV1` 不断增长的字段表 | 键值袋（§3.2） | 同上，外加惰性文件访问 |
| `PluginError` 的 payload：文档说过界，实际从不过界 | 要么给它真正的消息通道（`error!` 其实已经是），要么删掉这个说法 | 现在的文档在说谎；诚实的方案是"过界的是 code，文本走日志" |
| `Session` | 消失：`Settings` + `Roots` + `Engine` | 一个对象同时持有配置、商店、遍历、选择结果和 `quiet`，这正是各层无法单独测试的原因 |
| `[scripts]`：宿主解析、交给插件，却没有任何东西使用 | **改造成宿主侧别名**：`pmpx run <名字>` 先查这张表，命中就直接执行，未命中才走插件路由；表项接受 TOML 数组或字符串；不再进入插件上下文（§3.2 的键表里没有它） | 它是用户便利，不是生态知识。解析/合并/保留那半边代码已经写好了，缺的只是语义——顺便把"用户自己写的 [scripts] 永远被 pin 操作保留"这条保住 |
| `exec` 直通藏在 `run_verb` 的布尔参数后 | `Settings` 里一个显式的值 | 一个用户可能想关掉的策略，值得有名字 |
| 插件 manifest 身兼三职 | `pmpx.toml`：面向宿主的**能力请求**（`[detect]`、`[context]`、`[capabilities]`）；kit 保留打包元数据（name、version、abi） | "检测必须在 `dlopen` 之前完成"是**声明**的性质，不是打包的性质 |
| `MAX_FILES` 与静默丢弃 | 取消上限：`file.<名字>` 按需回答 | 插件永远能区分"缺席"与"空" |
| `--debug` 从引擎深处打印 | 引擎发 `Event::Phase`，CLI 排版 | 库不打印；而且阶段序列变成可断言的，不必捕获 stderr |

---

## 6. 迁移顺序

每一步结束时都是绿的（`cargo fmt --check`、`clippy -D warnings`、`cargo test`，以及 §7 的 CI 守卫），并且每一步都小到可以单独评审。第 2 步是唯一一次破版；它之后全是机械动作。

| # | 步骤 | 完成标准 |
| --- | --- | --- |
| 1 | 冻结本文档；把 §7 的守卫以"只报告"模式加进 CI | 守卫存在，并在今天的代码树上通过（已知违规——`hints.rs` 与那几处打印缝——被显式列出） |
| 2 | `pmpx-plugin-abi` + `pmpx-plugin` v3：键、能力、C 头 | 一个 fixture 插件能用新外壳构建并加载；能力/键协商测试（缺必需能力、函数表过短、未知键、空数组、panic）全绿；C 头重新生成后逐字节一致 |
| 3 | `pmpx-loader`：宿主侧 ABI，全部 | 现有的 shim 测试迁移到 loader；宿主侧 crate 里的 `unsafe` 数量为零 |
| 4 | `pmpx-project` + `pmpx-detect`：纯决策 | 检测测试在**没有文件系统**的情况下运行（证据是数据）；今天的排序/pin/同分测试原样迁移；`pmpx-detect` 零依赖 —— **两步都已完成** |
| 5 | `pmpx-engine`：设置、商店、执行、事件、门面 | 进程内测试断言 `Plan` 与 `Event` 序列；现有 CLI 套件不变通过（同一个二进制、同样的输出）—— **已全部完成**：设置（`Session::open` 走 `Options`，不依赖 clap）、商店（`store` 特性，默认关闭，关掉时依赖树里没有 kit/ureq/rustls/ring）、执行与事件、门面（`run_verb`/`run_script`） |
| 6 | `pmpx` CLI：薄外壳、`--explain`、`--json`、别名展开 | 端到端测试跑真实二进制；所有人类可见输出都在这个 crate；库依赖守卫通过 |
| 7 | `pmpx-testkit`（`pmpx plugin new` / `plugin test` **决定不做**，见下） | 插件能对着 fixture 目录、也能对着内存文件表，用真实检测完成测试，代码一屏写得下 —— 已完成 |

进度：第 1 步的守卫并入了第 2/3 步（先有 crate，才守得住）。

- **第 2、3 步已完成，而且是同一次破版提交完成的**：`pmpx-plugin-abi`（键、能力、C 头、表面快照）、`pmpx-plugin` v3（`export!` 生成能力查找与三张表、类型化 `Context`、`attach` 装日志并**校验表长**）、`pmpx-loader`（协商、调用与复制、错误类型），以及宿主整个切过去。v2 的那套 vtable、`marshal`、`dispatch` 与宿主的 `runtime/backend.rs` 旧实现**全部删除**，没有留下并行路径。
- loader 里"表"（`Tables`）与"库句柄"（`Plugin`）分开：嵌入同进程的宿主或测试不需要 `dlopen` 就能走完全同一条代码路径；契约自己的测试因此直接用 `Tables` 驱动，不再手搓第二份宿主角色。
- 迁移中按实际需要调整了三处文档原本的判断：`scripts` 彻底不进上下文（它是宿主的别名表，测试里现在有一条**反向断言**守着它）；`family` 的显示名搬进 CLI（`style::family_label`）；清单声明的 `abi` 降级为**诊断**——真正执法的是 loader 对插件自报主版本的检查，两者不一致时宿主警告而不拒绝（那正是"包装过期的安装"的样子）。
- **第 4 步的 `pmpx-detect` 已完成**：`STRONG_SCORE`/`WEAK_SCORE`/`PIN_FLOOR`、计分、家族与插件的两层层级、每一种失败原因都搬了进去，宿主只剩一个适配器（`InstalledPlugin` → `Candidate`、目录 → `Presence`、两层配置 → pins 与排序表、`Reason` → 契约的 `SelectionReason`）。决策现在吃数据：`Presence` 只有一个方法，"这个文件在不在"，测试用一个名字集合回答，于是整套排序/pin/同分/失败测试**不需要文件系统**；另有一个集成测试连"问了哪些文件"都断言成声明的那些标记名。零依赖，并已加进 CI 的零依赖门。
- **第 4 步的 `pmpx-project` 也已完成，`[scripts]` 同时落地**：`atomic_write`、`paths`（含两个环境变量覆盖）、`.pmpx.toml` 的分层合并、全局配置（含默认排序表）都搬进 crate 并保留各自的测试；宿主只剩 `pub use`。`[scripts]` 按 §5 的决定实现成**宿主侧别名**：`pmpx run <名字>` 先查合并后的表，命中就直接执行（不需要任何插件，也不需要检测），`--` 之后的参数追加给它，未命中才走插件路由。字符串形式的分词规则：空白分隔、`"`/`'` 成组、未闭合的引号取到行尾、**没有 shell**（`$VAR`、`&&`、管道都是普通字符，这是唯一能在三个平台一致的行为）。CLI 套件里加了 5 个端到端测试，其中一个专门验证"项目自己定义的名字不需要插件"，另一个验证"没有定义的名字照旧交给插件"。
- **第 5 步的执行半边已落地**：`pmpx-engine` 现在拥有"把答案变成进程"的全部机制——`resolve`（PATH/PATHEXT、`.cmd`/`.ps1` 的种类判定）、`command_for`（按种类选解释器、构建 `cmd.exe` 行）、`run`（继承 stdio、等待、把退出状态翻译成退出码）、以及 `EngineError`（"没有这个程序"与"有但起不来"是两件事）。它自己的 `Plan`/`Event` 类型让它**完全不依赖契约**：`Plan` 就三个字段，`Event` 是 `Resolved` / `Starting` / `Finished` / `Note` / `Error`，于是整条执行路径可以在**没有插件**的情况下测试（新增的 `a_run_reports_what_it_did_in_order` 断言完整事件序列）。宿主剩下 100 行的渲染器：把事件变成 `--debug` 的两条轨迹、"pmpx -> …"的宣告，以及两种提示；退出码翻译（`NotFound` → 3）也在那里。
- **第 5 步（`pmpx-engine` 的设置/商店/门面）已完成**：`Session::open` 吃 `Options`（argv 事实，不依赖 clap），商店（`store` 特性默认关闭）与 walk-up 都在引擎里，`run_verb`/`run_script` 也在；CLI 只剩 `cli.rs` + `commands/*` + `style.rs` + 事件渲染 + `selfupdate`。
- **`hints.rs` 已删除**，"没检测到"的消息改为列出已安装插件与它们声明的标记文件。
- **`pmpx run` 的 argv 边界已修**：`--` 分隔符原样交给插件（`args=some-target,--,--release`），插件才分得清"目标自己的参数"与"透传参数"；此前拍平后这个区分不可恢复。
- **`pmpx-testkit` 已落地**（第 7 步的一半）：两个入口都有——手工构造 `Context`（`context()`）与**真实检测**跑 fixture 目录（`Fixture`，用 `pmpx-detect` 与清单声明的标记），外加一个假宿主 `capture()` 让插件断言自己写的日志行（每个级别一张表，和真实宿主同一套机制）。它自己的测试就按插件作者的用法写。
- **版本号已到 0.3.0**（工作区统一版本；这次 ABI 是破版，与 `ABI_VERSION` 1→2 时一样，已发布的插件需要重编译）。
- **`pmpx plugin new` / `pmpx plugin test`：决定不做。** 理由：
  - 插件 crate 只有三个文件（`Cargo.toml`、`src/lib.rs`、`pmpx-plugin.toml`），脚手架的收益低于它随契约变化而过时的速度；作者写一个 starter 仓库更合适。
  - `plugin test` 除了包一层 `cargo test` 之外，唯一不冗余的内容只有三件单测在原理上碰不到的事：清单（TOML 不参与编译）、"以 cdylib 走宿主 loader 真的能加载"（作者不写 wrapper，且漏了 `export!` 的 rlib 照样编译通过）、"真实检测会不会选中它"（手工 `Context` 是故意绕过检测的）。这三件事有同一个根因——**作者今天无法在发布前本地装一次**（`plugin add` 只从 registry 装），所以该补的是那条回路，而不是再造一个测试命令。
  - 那条回路的形态是 `pmpx plugin add --path <本地目录>`：kit 的 `pack::build(cfg, plugin_dir, out_dir, options)` 已公开且就是吃本地 checkout（内部走 `PluginSource::Path`，wrapper 逻辑不必重写），缺的只是"按安装布局放进商店并记录"与"装前校验清单"两步。本次也不做这条。
  - 作者的一天因此是：`cargo test`（`pmpx-testkit` 让构造 Context / 跑 fixture 检测 / 断言日志行都是一屏代码）→ `cargo publish` → 用户 `pmpx plugin add <名字>`。
- **kit 0.3.0 已发布，pmpx 已更新**：依赖从 `0.2.0` 抬到 `0.3.0`；`PluginSet::load` 改成从 `PluginInfo::extra`（kit 在 `list()` 里带回的宿主段落）读 `[detect]`/`[context]`，因此**不再第二次读清单**，`pmpx-engine` 的 `store` 特性也去掉了 `toml` 直接依赖；`describe_source` 增加 `Local` 分支（本地安装会显示"built from <目录>"），`InstallSource` 失去 `Copy` 由编译器逐个指出。
- **`pmpx plugin add <路径>` 可用**（kit 的 `install_from_path`）：参数"是路径"由内容决定——能解析成含 `pmpx-plugin.toml` 的目录就是 checkout，其余照旧是插件名（`plugin add pnpm` 含义不变）。装前校验：ABI 不匹配**拒绝**（宿主反正会拒绝加载），没有 family / 没有 `[detect]` 标记**警告**（这些是"装上了但永远不会被选中"的静默失败）。端到端测试真编译一次夹具 checkout，再让真实检测选中它并走完一次调用。
- **顺带修正了 README 的作者指南**：真实的已发布插件（`pmpx-plugin-cargo`）是**纯 rlib + `pub fn create()`**，`export!` 由安装/打包时生成的 wrapper 调用；README 此前让作者自己在插件里写 `export!`，那会与 wrapper 的同名导出符号**重复定义**（链接期 LNK2005，本地安装时第一次暴露出来）。
- 仍然待做：`--explain` / `--json`（第 6 步里唯一还没做的两项）。

### 待办：一次代码审查留下的两条（中等以上）

按"每个 commit 一条"的顺序执行，已完成的两条与额外发现见下。

| # | 项 | 状态 |
| --- | --- | --- |
| 1 | `config set` 写前按类型校验 | **已完成**（`b349f23`）：写入前用 `GlobalConfig` 反序列化整份文档，失败即 `Usage` + "Nothing was written"；三个端到端测试（坏值被拒且文件未创建、好值写回读一致、未知键仍允许） |
| 2 | `-C` 不再引入 Windows verbatim 路径 | **已完成**（`cf55717`）：改用 `discovery::normalize`（`canonicalize` 与 `std::path::absolute` 在 Windows 上都会给出 `\\?\…`，而那条路径会进入交给插件的上下文）；`normalize` 提为 `pub(crate)` 作为引擎唯一的路径归一 |
| — | CI 从不编译 store 门控测试 | **已完成**（`db01982`）：工作区默认特性不含 `store`，因此 `pmpx-engine` 的 store 测试在 CI 里从不运行；现在多一步 `cargo test -p pmpx-engine --features store --locked`。这也是第 2 条第一次提交时带着失败断言溜过去的原因 |
| 3 | CLI 仍直接命名 kit 类型做商店操作 | 待做，计划见下 |
| 4 | `InstalledPlugin` 复制 `PluginInfo` 六字段并丢掉 `extra` | **已完成**：现在是 `{ info: PluginInfo, family, strong, weak, wanted }`，`name`/`crate_name`/`version`/`abi`/`dir` 走访问器，`extra` 不再被丢掉 |

**#4（先做，定类型形状）**：`InstalledPlugin` 改为组合——
```rust
pub struct InstalledPlugin {
    /// 清单读到的全部内容，宿主段落（`extra`）也在里面，不再丢掉。
    pub info: PluginInfo,
    /// family 保留为派生的 `Option<Family>`：`PluginInfo::family` 是 `Option<String>`，
    /// 而 `plugin ls` 按 `Option<&Family>` 分组，改成 `info.family` 会让每次分组都分配。
    pub family: Option<Family>,
    pub strong: Vec<String>,
    pub weak: Vec<String>,
    pub wanted: Vec<String>,
}
```
`name` / `crate_name` / `version` / `abi` / `dir` 改成访问器（`self.info.*`）。**改动面**：约 57 处访问点，分布在引擎的 `store/{mod,manifest,tests}.rs`、`detect.rs`、`session.rs` 与 CLI 的 `commands/{plugin,info,plugin_store,plugin_pin}.rs`、`app/mod.rs`。门禁：`cargo test --locked` + `cargo test -p pmpx-engine --features store --locked` + `clippy --all-targets -D warnings`。

**#3（后做，搬操作）**：把商店操作与清单契约校验收进 `pmpx-engine::store`，让 CLI 不再命名 kit 类型（引擎文档 `lib.rs:63` 已经承诺"kit 只出现在这个模块"，目前 `plugin_store.rs` 13 处、`error.rs` 的 `From<KitError>`、`selfupdate/release.rs` 的 `TARGET_TRIPLE`/`parse_github_repo` 都还直接用它）。做法：`store` 增加 `install` / `update` / `uninstall` / `search` / `view` / `library_of(plugin)` / `vet(manifest) -> Result<(), String>`，CLI 只留消息与呈现；`selfupdate/release.rs` 那两个纯 helper 要么留在原处并把这句文档改准确，要么自己算 target triple。先做 #4 是因为 #3 的新代码应当一次性写在最终的类型形状上。

回滚：第 3–7 步都是追加式搬迁，任何一步都可以在不碰 ABI 的前提下回退。第 2 步不可逆——它之后所有已发布插件都必须重编译，正如 `ABI_VERSION` 1 → 2 已经要求过一次的那样。

---

## 7. CI 守卫

| 守卫 | 检查方式 |
| --- | --- |
| 不安全孤岛 | `unsafe` 只出现在 `pmpx-plugin-abi`、`pmpx-loader`、`pmpx-plugin` 的 `shell` / `export` / `debug` / `context`（类型化上下文是外壳的另一半，它负责读访问器），以及 `pmpx-engine` 的 `backend.rs`（一次加载调用）与 `log.rs`（宿主自己导出的两个 C 回调）。CI 用一条 grep 守着（只看真正的代码行，注释里的"unsafe"不算）：其余任何地方出现 `unsafe` 都失败 |
| 库不打印 | `pmpx-project`、`pmpx-detect`、`pmpx-engine`、`pmpx-plugin`、`pmpx-plugin-abi` 的**直接**依赖里没有 `anstream`、`anstyle`、`clap`、`ureq`、`serde_json`、`tar`、`zip`、`flate2` |
| 无商店的引擎依旧干净 | 关掉 `store` 特性时，`pmpx-engine` 的依赖树里没有 `crate-plugin-kit` / `ureq` |
| ABI crate 零依赖 | `pmpx-plugin-abi` 的依赖列表为空 |
| C 头可编译且最新 | 同一个 `--check` 保证头文件与定义一致；再用 `cc -std=c11 -Wall -Wextra -Werror -fsyntax-only` 编译一次，头里的 `_Static_assert` 就是 C 侧对布局的独立验证 |
| ABI 表面快照 | `cargo run -p pmpx-plugin-abi --example gen-abi-files -- --check` 必须干净：**追加**允许，**破坏**必须在同一提交里递增 `abi_major`，人工"语义变化"清单改动同样要求递增（§3.3） |
| 每个键都有回答 | 一个测试枚举所有 `PMPX_KEY_*` 常量，断言宿主对每个键都作答（`count`/`get` 不 panic，且自洽：标量 `count > 0` 当且仅当 `get(0)` 非空） |
| 各 crate 的 MSRV | `pmpx-plugin-abi`/`pmpx-plugin` 在 1.82 上可构建；其余用工作区 MSRV |

> 注意"直接依赖"这个限定：`crate-plugin-kit` 自身会传递引入 `ureq`/`rustls`/`ring`，所以守卫必须用 `cargo tree --depth 1` 这一级别的检查，而不是整棵依赖树。

---

## 8. 测试策略

| 层 | 怎么测 |
| --- | --- |
| `pmpx-detect` | 表驱动，**无文件系统**：`Evidence` 与 `Declaration` 都是普通值，于是每条排序规则、同分裁决、pin 地板、失败模式都是一个单测 |
| `pmpx-project` | `tempfile` 目录；遍历的停止条件、标记探测、声明文件的上限与路径拒绝（`..`、绝对路径） |
| `pmpx-plugin-abi` | 纯编解码测试；外加一个 fuzz 目标，把任意 POD 上下文灌进插件外壳，要求"不 UB，返回错误码可以接受" |
| `pmpx-loader` | 今天的 `tests/shim` + `tests/cdylib`，但从宿主侧测：能力协商、名字不符、panic 标记、所有权、日志钩子 |
| `pmpx-engine` | 进程内：断言 `Plan`、`Event` 序列、以及针对临时数据目录的商店操作 |
| `pmpx`（CLI） | 跑真实二进制：stdout/stderr/退出码就是 UX 契约；`--explain --json` 与 golden 文件比对 |
| `pmpx-testkit` | 它自己的测试就按插件作者的用法写 |

---

## 9. 风险

| 风险 | 缓解 |
| --- | --- |
| 能力查找把错误藏到运行时（键拼错只表现为"不存在"） | 键常量放在 ABI crate；"每个键都有回答"测试；`pmpx plugin doctor` 打印插件与宿主各自同意了什么 |
| 把 `unsafe` 集中到少数几个地方后，这些地方更难评审 | 它们小且自洽；上面的 fuzz 目标；对编解码测试跑 `cargo miri` |
| 8 个 crate 的版本与发布编排 | 由 `cargo-bumpp` + `harbor` 统一管理：一次发布时统一设置全部版本、统一发布（与前端 monorepo 的做法一致）。哪个 crate 暂时不发布，只是它自己 `publish` 标志的事，不构成流程负担 |
| 迁移触及每个文件 | 第 3–7 步是"带着测试搬家"；在第 6 步之前，可执行行为预期逐字节不变 |
| CLI 侧范围膨胀（`--explain`、别名、JSON） | 那些是第 6 步，删掉也不影响第 1–5 步 |

---

## 10. 决定记录

| # | 问题 | 结论 |
| --- | --- | --- |
| 1 | `abi_major` 的语义边界 | **已冻结**：§3.3 的判定表 + 表面快照守卫，不靠自觉 |
| 2 | 动词：编号还是字符串 | 编号 + `unsupported`；并把"不认识的动词编号 → `unsupported`"写成契约义务（§3.3） |
| 3 | `[scripts]` | 改为宿主侧别名（见 §5），已按此实现 |
| 4 | 自更新 | CLI crate 的一个模块；出现第二个消费者再抽成 crate |
| 5 | 商店 | `pmpx-engine` 的 `store` 特性（默认关，CLI 打开）：内嵌者不带 kit 与网络栈 |
| 6 | `pmpx-testkit` 的范围 | 两个入口都要：手工构造 `Context`（快，依赖 `pmpx-plugin`）+ 用真实检测跑 fixture（真，依赖 detect/engine） |
| 7 | C 头 | 提交进仓库 + diff 守卫 |
| 8 | 包名 | `pmpx-engine` |

仍然属于实现细节、不需要你决策的：表面快照的文件格式、`[scripts]` 字符串形式的分词规则、
`pmpx plugin doctor` 的输出格式。
