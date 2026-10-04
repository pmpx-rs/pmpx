//! 跨 `dlopen` 边界的**线格式**。
//!
//! 宿主与插件是两个独立编译的世界，所以穿过这条线的数据只能是 `#[repr(C)]` 的 POD 与普通
//! 整数：`String` / `Vec` / `Box` 的内存归哪个分配器没有保证，`toml::Value` / `anyhow::Error`
//! 的布局随依赖的小版本变化，trait object 的 vtable 归属两边也没有约定。
//! 代价是两边不共享分配器 —— **内存一律由分配方释放**：宿主传进来的输入只读，插件产出的
//! 输出（`PmpxCommand` 及其内部字符串、`name` / `family`）由宿主用 [`free_command`] /
//! [`free_str`] 还回去。所以 `free_*` 里绝不能用宿主的 `Box::from_raw` 去接插件给的内存。

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::{CommandSpec, Context, PackageManager, Verb};

// ---- 版本 ----

/// 跨边界布局的版本，**独立整数，与 crate 版本号彻底解耦**。
/// 只有 [`PmpxPluginV1`] / [`PmpxCommand`] / [`PmpxStr`] 的形状、动词编号或错误码语义真的
/// 变了才 `+1`。宿主拿它做**唯一**的硬校验，不相等就拒绝加载。
pub const ABI_VERSION: u32 = 1;

// ---- 错误码 ----

/// 成功。
pub const PMPX_OK: u32 = 0;

/// 这个后端不支持该动词。
/// 宿主对它有**特殊处理**：`pmpx exec` 收到这个码会退化成裸透传，其余动词则原样报错，
/// 所以它必须与 [`PMPX_ERR_INTERNAL`] 分开。
pub const PMPX_ERR_UNSUPPORTED_VERB: u32 = 1;

/// 输入不合法 —— 动词编号不认识、`out` 是空指针、`matched` 里有非 UTF-8。
pub const PMPX_ERR_INVALID_ARGS: u32 = 2;

/// 插件内部出错，或者它 panic 了（panic 被 [`guard`] 捕获后归到这里，细节在 stderr 上）。
pub const PMPX_ERR_INTERNAL: u32 = 3;

// ---- 动词编号 ----

/// [`Verb::Install`] 的编号。
pub const VERB_INSTALL: u32 = 0;
/// [`Verb::Remove`] 的编号。
pub const VERB_REMOVE: u32 = 1;
/// [`Verb::Run`] 的编号。
pub const VERB_RUN: u32 = 2;
/// [`Verb::Build`] 的编号。
pub const VERB_BUILD: u32 = 3;
/// [`Verb::Test`] 的编号。
pub const VERB_TEST: u32 = 4;
/// [`Verb::Update`] 的编号。
pub const VERB_UPDATE: u32 = 5;
/// [`Verb::Exec`] 的编号。
pub const VERB_EXEC: u32 = 6;

// ---- 数据结构 ----

/// 跨边界字符串：**指针 + 长度**，不要求 NUL 结尾。
/// `ptr` / `len` 是一段**裸字节**（可能是路径或命令行参数），Unix 上完全可以不是合法 UTF-8；
/// 只有明确要求是文本的地方才校验 UTF-8，失败返回 [`PMPX_ERR_INVALID_ARGS`] 而不是 UB。
/// 带长度而不是靠 NUL，是因为 `str::as_ptr()` 得到的指针不保证后面跟着 NUL。
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct PmpxStr {
    /// 起始地址。`len == 0` 时可以是空指针。
    pub ptr: *const u8,
    /// 字节长度。
    pub len: usize,
}

// SAFETY: `PmpxStr` 是"一段只读字节 + 长度"，跨线程共享它 unsafe 的地方只在于
// `ptr` 指向的内存必须仍然有效。而这个结构体的两份来源都是明确的：
//   - 宿主传进来的：在整个调用期间有效；
//   - 插件产出的：指向插件泄漏出来的 `Box<[u8]>`，在 `free_str` 之前一直有效。
// 两者都不会在共享期间被释放或写入。所以把它标成 `Sync` 是成立的。
unsafe impl Sync for PmpxStr {}

impl PmpxStr {
    /// 空。`len == 0` 且指针为空 —— 在 `cwd` 里表示"没有覆盖"。
    pub const EMPTY: PmpxStr = PmpxStr {
        ptr: std::ptr::null(),
        len: 0,
    };

    /// 从一段 `'static` 文本构造（`const` 是为了 `export!` 的 `static` vtable 能在编译期填好）。
    pub const fn from_static(s: &'static str) -> Self {
        Self {
            ptr: s.as_ptr(),
            len: s.len(),
        }
    }

    /// 是不是空的。
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// 跨边界的命令描述。由插件填充、由插件释放（[`free_command`]）；宿主只读。
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct PmpxCommand {
    /// 可执行文件。
    pub program: PmpxStr,
    /// 参数数组，元素个数为 `args_len`。
    pub args: *const PmpxStr,
    /// `args` 的元素个数。
    pub args_len: usize,
    /// 工作目录覆盖。`len == 0` 表示用宿主给的项目根。
    pub cwd: PmpxStr,
}

// SAFETY: 同 `PmpxStr` —— 这个结构体只是"若干只读字节的引用 + 一个数组长度"。
unsafe impl Sync for PmpxCommand {}

/// 插件导出的**唯一**结构，就是那张函数指针表；宿主取到它之后所有交互都走这里的函数指针，
/// 没有 trait object，也没有 vtable 互转那类 UB。
#[repr(C)]
pub struct PmpxPluginV1 {
    /// 必须等于 [`ABI_VERSION`]。宿主第一件事就是比对这个字段。
    pub abi_version: u32,

    /// 编出这个插件的 rustc 版本，由 `pmpx-plugin` 的 build.rs 注入。
    /// **只用于诊断展示，不做硬校验** —— 不同 rustc 编出来的插件在这套 C ABI 下可以安全加载。
    pub rustc_version: PmpxStr,

    /// 编出这个插件的 target triple。同样只做诊断。
    pub target: PmpxStr,

    /// 插件名。内存归插件，宿主读完用 [`free_str`] 释放，并与 manifest 里声明的名字比对
    /// —— 不一致说明装错了东西。
    pub name: unsafe extern "C" fn() -> PmpxStr,

    /// 生态分组。内存归插件，宿主读完用 [`free_str`] 释放。
    pub family: unsafe extern "C" fn() -> PmpxStr,

    /// 把「动词 + 参数」翻译成一条命令。返回 [`PMPX_OK`] 时 `out` 被填充、宿主用完调
    /// [`free_command`]；否则返回 `PMPX_ERR_*`，`out` 不动。
    ///
    /// # Safety
    /// - `project_root` / `matched` / `args` 必须由宿主分配、在调用期间有效且只读；
    /// - `out` 必须指向一块可写的 [`PmpxCommand`]；
    /// - **panic 不许穿过这个边界**：从 Rust 1.81 起越过 `extern "C"` 会直接 abort，宿主侧的
    ///   `catch_unwind` 救不了，所以 `export!` 统一包了 `catch_unwind`。
    pub command: unsafe extern "C" fn(
        project_root: PmpxStr,
        matched: *const PmpxStr,
        matched_len: usize,
        verb: u32,
        args: *const PmpxStr,
        args_len: usize,
        out: *mut PmpxCommand,
    ) -> u32,

    /// 释放 [`PmpxPluginV1::name`] / [`PmpxPluginV1::family`] 返回值占的内存。
    ///
    /// # Safety
    /// `s` 必须来自**同一个插件**的产出，且只能释放一次。
    pub free_str: unsafe extern "C" fn(PmpxStr),

    /// 释放 [`PmpxPluginV1::command`] 填充的 [`PmpxCommand`] 占的内存，**不释放它本身**
    /// （那个结构体在宿主那边）。
    ///
    /// # Safety
    /// `c` 必须来自**同一个插件**的一次成功 `command` 调用，且只能释放一次。
    pub free_command: unsafe extern "C" fn(*mut PmpxCommand),
}

// SAFETY: 这个结构体是编译期常量填出来的只读表：几个整数、两段 `'static` 字节、
// 五个函数指针。填好之后从不修改。函数指针本身是 `Sync` 的。
unsafe impl Sync for PmpxPluginV1 {}

/// 唯一入口符号的名字。
/// ⚠️ 符号本身由 `pmpx_plugin::export!` 在插件里定义，**不在这里** —— 这个 crate 会被链接进
/// 每一个插件，它若自己定义同名 `#[no_mangle]` 符号就会和 `export!` 生成的那个撞车。形状是
/// `extern "C" fn() -> *const PmpxPluginV1`。
pub const ENTRY_SYMBOL: &str = "pmpx_plugin_entry_v1";

// ---- 构建信息（诊断用）----

/// 编译期注入的 rustc 版本。`const fn` 是刻意的：`export!` 生成的 `static` vtable 要在编译期
/// 求值。
pub const fn build_rustc() -> PmpxStr {
    PmpxStr::from_static(env!("PMPX_BUILD_RUSTC"))
}

/// 编译期注入的 target triple。
pub const fn build_target() -> PmpxStr {
    PmpxStr::from_static(env!("PMPX_BUILD_TARGET"))
}

// ---- 内存：分配与释放 ----

/// 把一段字节泄漏成 [`PmpxStr`]，交给边界对面去读。
/// **所有**从插件流出的字符串都用同一种分配（`Box<[u8]>`），这样 [`free_str`] 只有一条路径，
/// 不会出现"按 `Box<str>` 释放 `Box<[u8]>`"这种 UB。
pub fn leak_bytes(bytes: &[u8]) -> PmpxStr {
    let boxed: Box<[u8]> = bytes.to_vec().into_boxed_slice();
    let out = PmpxStr {
        ptr: boxed.as_ptr(),
        len: boxed.len(),
    };
    std::mem::forget(boxed);
    out
}

/// [`leak_bytes`] 的 `&str` 版本。
pub fn leak_str(s: &str) -> PmpxStr {
    leak_bytes(s.as_bytes())
}

/// 释放一个由本侧 [`leak_bytes`] / [`leak_str`] 产出的 [`PmpxStr`]。
/// 空指针直接返回（[`PmpxStr::EMPTY`] 就是这么用的）；长度为 0 但指针非空是合法分配，会正常
/// 走 `Box::from_raw`。
///
/// # Safety
/// - `s` 必须来自**本侧**的 `leak_*`，不能是宿主传来的输入；
/// - 只能释放一次。
pub unsafe fn free_str(s: PmpxStr) {
    if s.ptr.is_null() {
        return;
    }
    let raw = std::ptr::slice_from_raw_parts_mut(s.ptr as *mut u8, s.len);
    // 与 leak_bytes 里的 Box<[u8]> 严格配对。
    drop(unsafe { Box::from_raw(raw) });
}

/// 释放一个由本侧 [`write_command`] 填充的 [`PmpxCommand`] 的内容，**不释放 `c` 本身**
/// （那个结构体在宿主那边，通常是栈上）。
/// # Safety
/// `c` 必须来自本侧一次成功的 `command` 调用，且只能释放一次。
pub unsafe fn free_command(c: *mut PmpxCommand) {
    if c.is_null() {
        return;
    }
    let cmd = unsafe { &*c };

    unsafe { free_str(cmd.program) };
    unsafe { free_str(cmd.cwd) };

    if !cmd.args.is_null() && cmd.args_len > 0 {
        // 与 write_command 里的 Box<[PmpxStr]> 严格配对。
        let raw = std::ptr::slice_from_raw_parts_mut(cmd.args as *mut PmpxStr, cmd.args_len);
        let args = unsafe { Box::from_raw(raw) };
        for s in args.iter() {
            unsafe { free_str(*s) };
        }
    }
}

// ---- 输入方向：字节 ↔ OsString ----

/// 把宿主传来的字节读成 `OsString`。
/// Unix 上路径与命令行参数**可以不是合法 UTF-8**，用 `String` 只能有损转换，会把
/// `pmpx exec some-tool /latin1/path` 这类调用悄悄改坏，`OsString` 才是无损的裸字节。
/// Windows 上 `OsString` 底层是 WTF-8，未配对代理项会退化成有损替换 —— 那是平台的边界。
///
/// # Safety
/// `s` 必须描述一段在本次调用期间有效的只读内存，或 `len == 0`。
pub unsafe fn read_os(s: PmpxStr) -> OsString {
    if s.len == 0 {
        return OsString::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    bytes_to_os(bytes)
}

/// 把宿主传来的字节读成 `&str`，**校验 UTF-8**；失败返回 [`PMPX_ERR_INVALID_ARGS`]，绝不
/// `from_utf8_unchecked` —— 那等于假定宿主永远正确，而 ABI 的职责恰恰是不做这种假定。
/// # Safety
/// 同 [`read_os`]。
pub unsafe fn read_str<'a>(s: PmpxStr) -> Result<&'a str, u32> {
    if s.len == 0 {
        return Ok("");
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    std::str::from_utf8(bytes).map_err(|_| PMPX_ERR_INVALID_ARGS)
}

/// 把裸字节转成 `OsString`。
/// 公开是因为宿主侧也要做同一件事（把 `project_root` 与 `args` 变成字节送过边界），各写一份
/// 平台 cfg 是必然漂移的重复。Unix 无损；其它平台上 `OsString` 底层是 WTF-8，非 UTF-8 会
/// 退化成 U+FFFD。
#[cfg(unix)]
pub fn bytes_to_os(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

/// 见 [`bytes_to_os`] 的平台说明。
#[cfg(not(unix))]
pub fn bytes_to_os(bytes: &[u8]) -> OsString {
    String::from_utf8_lossy(bytes).into_owned().into()
}

/// 把 `OsStr` 转成裸字节。与 [`bytes_to_os`] 严格配对：Unix 无损，其它平台经
/// `to_string_lossy`，非 UTF-8 会退化成 U+FFFD。
#[cfg(unix)]
pub fn os_to_bytes(s: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}

/// 见 [`os_to_bytes`] 的平台说明。
#[cfg(not(unix))]
pub fn os_to_bytes(s: &OsStr) -> Vec<u8> {
    s.to_string_lossy().into_owned().into_bytes()
}

// ---- 输出方向 ----

/// 把一个 [`CommandSpec`] 写成跨边界的形式，**内存由本侧分配**。
/// # Safety
/// `out` 必须指向一块可写的 [`PmpxCommand`]。
pub unsafe fn write_command(out: *mut PmpxCommand, spec: CommandSpec) {
    let program = leak_bytes(&os_to_bytes(&spec.program));

    let args: Vec<PmpxStr> = spec
        .args
        .iter()
        .map(|a| leak_bytes(&os_to_bytes(a)))
        .collect();
    let args_boxed: Box<[PmpxStr]> = args.into_boxed_slice();
    let args_len = args_boxed.len();
    let args_ptr = args_boxed.as_ptr();
    std::mem::forget(args_boxed);

    let cwd = match &spec.cwd {
        Some(p) => leak_bytes(&os_to_bytes(p.as_os_str())),
        None => PmpxStr::EMPTY,
    };

    unsafe {
        *out = PmpxCommand {
            program,
            args: args_ptr,
            args_len,
            cwd,
        };
    }
}

// ---- 调度 ----

/// 一次 `command` 调用的全部接线：读输入 → 调 [`crate::PackageManager::command`] → 写输出。
/// 这段逻辑住在这里而不是在 `export!` 宏里，是为了它能被直接测试。
/// # Safety
/// 见 [`PmpxPluginV1::command`] 的 Safety 段。此外 `plugin` 必须是本进程里有效的实例。
#[allow(clippy::too_many_arguments)]
pub unsafe fn dispatch_command(
    plugin: &dyn PackageManager,
    project_root: PmpxStr,
    matched: *const PmpxStr,
    matched_len: usize,
    verb: u32,
    args: *const PmpxStr,
    args_len: usize,
    out: *mut PmpxCommand,
) -> u32 {
    if out.is_null() {
        return PMPX_ERR_INVALID_ARGS;
    }

    let Some(verb) = Verb::from_abi(verb) else {
        return PMPX_ERR_INVALID_ARGS;
    };

    let project_root = PathBuf::from(unsafe { read_os(project_root) });

    // `matched` 是**文本**（manifest 里声明的文件名），所以这里要校验 UTF-8。
    let mut matched_names = Vec::with_capacity(matched_len);
    for i in 0..matched_len {
        let raw = unsafe { *matched.add(i) };
        match unsafe { read_str(raw) } {
            Ok(s) => matched_names.push(s.to_string()),
            Err(code) => return code,
        }
    }

    // `args` 是**参数**，可以是任意字节 —— 原样转成 OsString，无损。
    let mut arg_list = Vec::with_capacity(args_len);
    for i in 0..args_len {
        let raw = unsafe { *args.add(i) };
        arg_list.push(unsafe { read_os(raw) });
    }

    let ctx = Context {
        project_root,
        matched: matched_names,
    };

    match plugin.command(&ctx, verb, &arg_list) {
        Ok(spec) => {
            unsafe { write_command(out, spec) };
            PMPX_OK
        }
        Err(e) => e.code(),
    }
}

/// 把一次跨边界调用包进 `catch_unwind`。
/// 从 Rust 1.81 起，**让 panic 越过 `extern "C"` 边界会直接 abort**，宿主那边的
/// `catch_unwind` 完全救不了，所以必须由插件自己兜住。宿主侧还会再包一层，那是为了兜
/// "插件忘了包"或"插件用 `panic=abort` 编的"。
pub fn guard(f: impl FnOnce() -> u32) -> u32 {
    // `AssertUnwindSafe`：拿到 PMPX_ERR_INTERNAL 之后调用方就会中止这次操作，不会继续碰捕获现场。
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(PMPX_ERR_INTERNAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verb_numbers_match_the_public_enum() {
        // 编号一旦错位，宿主与插件对"install"的理解就会分叉，而且不会有任何编译错误。
        assert_eq!(Verb::Install.to_abi(), VERB_INSTALL);
        assert_eq!(Verb::Remove.to_abi(), VERB_REMOVE);
        assert_eq!(Verb::Run.to_abi(), VERB_RUN);
        assert_eq!(Verb::Build.to_abi(), VERB_BUILD);
        assert_eq!(Verb::Test.to_abi(), VERB_TEST);
        assert_eq!(Verb::Update.to_abi(), VERB_UPDATE);
        assert_eq!(Verb::Exec.to_abi(), VERB_EXEC);
    }

    #[test]
    fn verb_round_trips() {
        for v in Verb::ALL {
            assert_eq!(Verb::from_abi(v.to_abi()), Some(*v));
        }
        assert_eq!(Verb::from_abi(99), None);
    }

    #[test]
    fn empty_str_reads_as_empty() {
        assert_eq!(unsafe { read_os(PmpxStr::EMPTY) }, OsString::new());
        assert_eq!(unsafe { read_str(PmpxStr::EMPTY) }.unwrap(), "");
    }

    #[test]
    fn leak_and_free_round_trip() {
        let s = leak_str("hello");
        assert_eq!(s.len, 5);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(s.ptr, s.len) },
            b"hello"
        );
        unsafe { free_str(s) };
    }

    #[test]
    fn free_str_tolerates_null() {
        // EMPTY 会被 free_command 无条件传给 free_str
        unsafe { free_str(PmpxStr::EMPTY) };
    }

    #[test]
    fn leak_and_free_an_empty_string() {
        // 空串的 Box<[u8]> 是一个悬垂指针（非空、len 0），必须能正常释放
        let s = leak_str("");
        assert_eq!(s.len, 0);
        assert!(!s.ptr.is_null(), "空 Box 的指针是悬垂但非空的");
        unsafe { free_str(s) };
    }

    #[test]
    fn read_str_rejects_invalid_utf8() {
        let bytes = [0xff, 0xfe];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        assert_eq!(unsafe { read_str(s) }, Err(PMPX_ERR_INVALID_ARGS));
    }

    #[test]
    fn read_os_round_trips_valid_utf8() {
        let bytes = "/tmp/项目/ünïcode".as_bytes();
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        let got = unsafe { read_os(s) };
        assert_eq!(os_to_bytes(&got), bytes);
    }

    /// Unix 上路径与参数可以是**任意字节**（0xFF 不是合法 UTF-8，但它是合法的路径字节），
    /// `OsString` 必须无损保住它们。
    #[cfg(unix)]
    #[test]
    fn read_os_keeps_arbitrary_bytes_on_unix() {
        let bytes = [0x2f, 0x62, 0x61, 0x64, 0xff];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        let got = unsafe { read_os(s) };
        assert_eq!(os_to_bytes(&got), bytes, "Unix 上必须无损");
    }

    /// 非 Unix 上 `OsString` 底层是 WTF-8，非 UTF-8 会退化成 U+FFFD —— 这是**平台边界**，
    /// 测试把它钉成"已知行为"而不是假装无事。
    #[cfg(not(unix))]
    #[test]
    fn read_os_replaces_invalid_utf8_off_unix() {
        let bytes = [0x2f, 0x62, 0xff];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        let got = unsafe { read_os(s) };
        let expected = String::from_utf8_lossy(&bytes).into_owned().into_bytes();
        assert_eq!(os_to_bytes(&got), expected);
        assert_ne!(os_to_bytes(&got), bytes, "非 Unix 上确实是有损的");
    }

    #[test]
    fn writes_and_frees_a_command() {
        let spec = CommandSpec::new("cargo")
            .arg("add")
            .arg("serde")
            .cwd("/tmp/project");

        let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();
        unsafe { write_command(out.as_mut_ptr(), spec) };
        let mut cmd = unsafe { out.assume_init() };

        assert_eq!(cmd.args_len, 2);
        let program = unsafe { std::slice::from_raw_parts(cmd.program.ptr, cmd.program.len) };
        assert_eq!(program, b"cargo");

        let arg0 = unsafe { *cmd.args.add(0) };
        let a0 = unsafe { std::slice::from_raw_parts(arg0.ptr, arg0.len) };
        assert_eq!(a0, b"add");

        let cwd = unsafe { std::slice::from_raw_parts(cmd.cwd.ptr, cmd.cwd.len) };
        assert_eq!(cwd, b"/tmp/project");

        unsafe { free_command(&mut cmd as *mut _) };
    }

    #[test]
    fn writes_a_command_with_no_args_and_no_cwd() {
        let spec = CommandSpec::new("cargo");

        let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();
        unsafe { write_command(out.as_mut_ptr(), spec) };
        let mut cmd = unsafe { out.assume_init() };

        assert_eq!(cmd.args_len, 0);
        assert!(cmd.cwd.is_empty(), "没有 cwd 时应当是 EMPTY");

        unsafe { free_command(&mut cmd as *mut _) };
    }

    #[test]
    fn free_command_tolerates_null() {
        unsafe { free_command(std::ptr::null_mut()) };
    }

    #[test]
    fn guard_turns_a_panic_into_internal_error() {
        assert_eq!(guard(|| PMPX_OK), PMPX_OK);
        assert_eq!(guard(|| panic!("插件炸了")), PMPX_ERR_INTERNAL);
    }

    #[test]
    fn build_info_is_populated() {
        let rustc = build_rustc();
        let target = build_target();
        assert!(rustc.len > 0);
        assert!(target.len > 0);

        let rustc = unsafe { std::slice::from_raw_parts(rustc.ptr, rustc.len) };
        let target = unsafe { std::slice::from_raw_parts(target.ptr, target.len) };
        assert!(
            std::str::from_utf8(rustc).unwrap().contains("rustc"),
            "rustc_version 应当是 `rustc 1.x.y (...)` 这种形式"
        );
        assert!(std::str::from_utf8(target).unwrap().contains('-'));
    }
}
