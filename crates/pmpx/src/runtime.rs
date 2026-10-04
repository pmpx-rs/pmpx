//! 加载选中的插件，跨 ABI 调它。
//!
//! 校验发生在调用之前，而不是加载之时：`crate-plugin-kit` 的 `load()` 只保证"能
//! `dlopen`、能取到入口符号"，"这个插件能不能跟这个宿主说话"由 pmpx 判断 —— 拿到
//! vtable 之后立刻查 `abi_version == ABI_VERSION`（相等即可，不是 crate 版本全等）
//! 与 `name()` 是否和 manifest 声明的一致。
//!
//! 每一次对插件的调用都包在 `catch_unwind` 里，但那只是兜底 —— 主要防线在插件侧
//! `export!` 生成的 `extern "C"` 外壳里：从 Rust 1.81 起，让 panic 越过 `extern "C"`
//! 会直接 abort，宿主这边根本救不了。

use std::ffi::OsString;
use std::path::Path;

use crate_plugin_kit::{CratePluginKit, LoadedPlugin};
use pmpx_plugin::abi::{
    self, PmpxCommand, PmpxPluginV1, PmpxStr, ABI_VERSION, PMPX_ERR_INTERNAL,
    PMPX_ERR_INVALID_ARGS, PMPX_ERR_UNSUPPORTED_VERB, PMPX_OK,
};
use pmpx_plugin::{CommandSpec, Verb};

use crate::error::{PmpxError, Result};
use crate::plugins::InstalledPlugin;

/// 插件明确报出来的失败。
///
/// **与 [`PmpxError`] 分开**是因为含义不同：`PmpxError` 是"pmpx 这边出问题了"，
/// 这个是"插件正常地回答说这件事我做不了"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// 插件不支持这个动词。
    ///
    /// `pmpx exec` 收到它会退化成裸透传；其余六个动词维持"不支持就报错"。
    UnsupportedVerb,

    /// 插件说入参不对。
    InvalidArgs(String),

    /// 插件内部出错，或者它 panic 了。
    Internal(String),
}

impl BackendError {
    /// 对应的进程退出码。
    pub fn exit_code(&self) -> u8 {
        match self {
            // "你让我做的事这个后端做不了" —— 与用法错误同一个码
            BackendError::UnsupportedVerb | BackendError::InvalidArgs(_) => {
                crate::error::EXIT_USAGE
            }
            // 插件炸了 —— 算 pmpx 自身错误
            BackendError::Internal(_) => crate::error::EXIT_INTERNAL,
        }
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::UnsupportedVerb => f.write_str("这个后端不支持该动词"),
            BackendError::InvalidArgs(m) => write!(f, "后端拒绝了入参：{m}"),
            BackendError::Internal(m) => write!(f, "后端内部错误：{m}"),
        }
    }
}

/// 一个已经加载、已经通过 ABI 与名字校验的插件。
///
/// 它持有 `dlopen` 的句柄，所以它的生命周期内那个动态库不会被卸掉。
/// 绝不能 `Send`/`Sync`：vtable 里的函数指针归属那个动态库，跨线程调用它会让
/// "库还在不在"这件事失去保证。
pub struct Backend {
    loaded: LoadedPlugin<PmpxPluginV1>,
    /// manifest 里声明的插件名。
    pub name: String,
}

impl Backend {
    /// 加载一个插件并校验它。
    pub fn load(kit: &CratePluginKit<PmpxPluginV1>, plugin: &InstalledPlugin) -> Result<Self> {
        let loaded = kit.load(&plugin.crate_name).map_err(|e| {
            PmpxError::not_found(format!(
                "加载插件 {} 失败：{e}\n\
                 插件装在 {} —— 用 `pmpx plugin rm {}` 删掉后重装试试。",
                plugin.crate_name,
                plugin.dir.display(),
                plugin.name
            ))
        })?;

        let entry = unsafe { &*loaded.entry() };

        // ABI 版本必须相等
        if entry.abi_version != ABI_VERSION {
            return Err(PmpxError::not_found(format!(
                "插件 {} 的 ABI 版本是 {}，pmpx 需要 {}。\n\
                 它们是两个独立编译的世界，布局对不上就不能加载 —— 这比崩溃好。\n\
                 用 `pmpx plugin update {}` 升级插件，或把 pmpx 升到匹配的版本。",
                plugin.crate_name, entry.abi_version, ABI_VERSION, plugin.name
            )));
        }

        // 自报名必须与 manifest 声明的一致
        let self_reported = unsafe { read_plugin_str(entry.name, entry.free_str) };
        if self_reported != plugin.name {
            // 两个名字都要打出来 —— 只说"不一致"用户没法判断该信哪个
            return Err(PmpxError::not_found(format!(
                "插件自报名是 \"{self_reported}\"，但 manifest 声明的是 \"{}\" —— 拒绝加载。\n\
                 请删掉 {} 后重新安装。",
                plugin.name,
                plugin.dir.display()
            )));
        }

        Ok(Self {
            loaded,
            name: plugin.name.clone(),
        })
    }

    /// 指向 vtable 的引用。
    fn entry(&self) -> &PmpxPluginV1 {
        // SAFETY: `load()` 校验过它非空，而且 `self.loaded` 持有那个动态库。
        unsafe { &*self.loaded.entry() }
    }

    /// 插件自报的生态。
    ///
    /// 加载之前用的是 manifest 里的值（检测阶段只能读它），加载之后这里拿到的是
    /// 插件自己说的。两者不一致不是致命问题（所以不拒绝加载），但值得显示出来。
    pub fn family(&self) -> String {
        let e = self.entry();
        unsafe { read_plugin_str(e.family, e.free_str) }
    }

    /// 插件编译时用的 rustc 版本。只用于诊断。
    pub fn rustc_version(&self) -> String {
        unsafe { read_bytes(self.entry().rustc_version) }
    }

    /// 插件编译时的 target triple。只用于诊断。
    pub fn target(&self) -> String {
        unsafe { read_bytes(self.entry().target) }
    }

    /// 把「动词 + 参数」交给插件翻译成一条命令。
    ///
    /// 返回值是 `Result<Result<...>>`：外层 [`PmpxError`] 表示跨边界这件事本身失败了
    /// （不该发生）；内层 [`BackendError`] 表示插件正常地回答说它做不了 —— 那是业务
    /// 结果，不是故障。
    pub fn command(
        &self,
        project_root: &Path,
        matched: &[String],
        verb: Verb,
        args: &[OsString],
    ) -> Result<std::result::Result<CommandSpec, BackendError>> {
        let entry = self.entry();

        // ---------------------------------------------------------------
        // 输入方向：宿主分配、宿主释放、插件只读 —— 这些 Vec 活到本函数结束，
        // 插件被要求在这之前读完。
        // ---------------------------------------------------------------
        let root_bytes = abi::os_to_bytes(project_root.as_os_str());
        let root = PmpxStr {
            ptr: root_bytes.as_ptr(),
            len: root_bytes.len(),
        };

        let matched_bytes: Vec<Vec<u8>> = matched.iter().map(|m| m.as_bytes().to_vec()).collect();
        let matched_raw: Vec<PmpxStr> = matched_bytes
            .iter()
            .map(|b| PmpxStr {
                ptr: b.as_ptr(),
                len: b.len(),
            })
            .collect();

        let arg_bytes: Vec<Vec<u8>> = args
            .iter()
            .map(|a| abi::os_to_bytes(a.as_os_str()))
            .collect();
        let args_raw: Vec<PmpxStr> = arg_bytes
            .iter()
            .map(|b| PmpxStr {
                ptr: b.as_ptr(),
                len: b.len(),
            })
            .collect();

        // ---------------------------------------------------------------
        // 输出方向：插件分配、插件释放，宿主只读
        // ---------------------------------------------------------------
        let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();

        let code = {
            let out_ptr = out.as_mut_ptr();
            // 兜底的那层 catch_unwind。主要防线在插件侧的 export! 外壳里。
            abi::guard(move || {
                // SAFETY: 输入由本函数分配并在调用期间保持存活；out 指向本地可写内存。
                // 其余约定由 `PmpxPluginV1::command` 的 Safety 段规定。
                unsafe {
                    (entry.command)(
                        root,
                        matched_raw.as_ptr(),
                        matched_raw.len(),
                        verb.to_abi(),
                        args_raw.as_ptr(),
                        args_raw.len(),
                        out_ptr,
                    )
                }
            })
        };

        match code {
            PMPX_OK => {}
            PMPX_ERR_UNSUPPORTED_VERB => return Ok(Err(BackendError::UnsupportedVerb)),
            PMPX_ERR_INVALID_ARGS => {
                return Ok(Err(BackendError::InvalidArgs(format!(
                    "插件 {} 认为参数不合法",
                    self.name
                ))))
            }
            PMPX_ERR_INTERNAL => {
                return Ok(Err(BackendError::Internal(format!(
                    "插件 {} 内部出错或 panic 了（细节在它自己的 stderr 上）",
                    self.name
                ))))
            }
            other => {
                return Ok(Err(BackendError::Internal(format!(
                    "插件 {} 返回了未知错误码 {other}",
                    self.name
                ))))
            }
        }

        // SAFETY: `PMPX_OK` 时插件保证已填充 out。
        let mut cmd = unsafe { out.assume_init() };

        // 把这些内存拷出来，然后按约定还给插件 —— 宿主自始至终只读。
        let spec = unsafe {
            let program = abi::read_os(cmd.program);
            let cwd = if cmd.cwd.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(abi::read_os(cmd.cwd)))
            };
            let mut argv = Vec::with_capacity(cmd.args_len);
            for i in 0..cmd.args_len {
                argv.push(abi::read_os(*cmd.args.add(i)));
            }
            CommandSpec {
                program,
                args: argv,
                cwd,
            }
        };

        // SAFETY: 来自上面那次成功的调用，且只释放一次。
        unsafe { (entry.free_command)(&mut cmd as *mut _) };

        Ok(Ok(spec))
    }

    /// 给 `pmpx info` 用的诊断信息。
    pub fn diagnostics(&self) -> BackendDiagnostics {
        BackendDiagnostics {
            name: self.name.clone(),
            family: self.family(),
            rustc_version: self.rustc_version(),
            target: self.target(),
        }
    }
}

/// `pmpx info` 显示的插件自报信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendDiagnostics {
    /// 插件自报名。
    pub name: String,
    /// 插件自报的生态。
    pub family: String,
    /// 编译它的 rustc。
    pub rustc_version: String,
    /// 编译它的 target triple。
    pub target: String,
}

/// 调一次返回 [`PmpxStr`] 的 vtable 函数，读成 Rust 字符串，**按约定释放**。
///
/// # Safety
///
/// `f` 与 `free` 必须来自同一张、仍然有效的 vtable。
unsafe fn read_plugin_str(
    f: unsafe extern "C" fn() -> PmpxStr,
    free: unsafe extern "C" fn(PmpxStr),
) -> String {
    let s = unsafe { f() };
    let out = unsafe { read_bytes(s) };
    // 内存归插件，读完必须还回去 —— 宿主从不 free 自己没分配的东西。
    unsafe { free(s) };
    out
}

/// 读一段 [`PmpxStr`]，不释放。
///
/// # Safety
///
/// `s` 必须描述一段在调用期间有效的只读内存。
unsafe fn read_bytes(s: PmpxStr) -> String {
    if s.ptr.is_null() || s.len == 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_error_exit_codes_match_the_table() {
        assert_eq!(
            BackendError::UnsupportedVerb.exit_code(),
            crate::error::EXIT_USAGE
        );
        assert_eq!(
            BackendError::InvalidArgs("x".into()).exit_code(),
            crate::error::EXIT_USAGE
        );
        assert_eq!(
            BackendError::Internal("x".into()).exit_code(),
            crate::error::EXIT_INTERNAL
        );
    }

    #[test]
    fn backend_error_messages_name_the_backend_situation() {
        assert!(BackendError::UnsupportedVerb.to_string().contains("不支持"));
        assert!(BackendError::InvalidArgs("a".into())
            .to_string()
            .contains('a'));
        assert!(BackendError::Internal("b".into()).to_string().contains('b'));
    }

    #[test]
    fn reading_an_empty_string_is_safe() {
        assert_eq!(unsafe { read_bytes(PmpxStr::EMPTY) }, "");
    }

    #[test]
    fn reading_a_null_pointer_is_safe() {
        let s = PmpxStr {
            ptr: std::ptr::null(),
            len: 99,
        };
        assert_eq!(unsafe { read_bytes(s) }, "", "空指针不该被解引用");
    }

    #[test]
    fn reading_non_utf8_bytes_does_not_panic() {
        let bytes = [0xff, 0xfe, 0xfd];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        // 诊断信息而已，有损转换就够了 —— 但不能 panic
        assert!(!unsafe { read_bytes(s) }.is_empty());
    }
}
