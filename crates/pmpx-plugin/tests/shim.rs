//! 直接驱动 `export!` 生成的 C ABI 外壳。
//!
//! 这个测试扮演**宿主的角色**：按 `pmpx-plugin` 的约定填输入、读输出、释放内存。它不
//! `dlopen`（那件事在 `tests/cdylib.rs` 里做），但把每一根线都接上了 —— 包括跨边界内存的
//! 分配与归还。宏里生成的代码路径，只有这样"真的按 ABI 调一次"才能覆盖。

use std::ffi::OsString;

use pmpx_plugin::abi::{self, PmpxCommand, PmpxPluginV1, PmpxStr};
use pmpx_plugin::{CommandSpec, Context, Family, PackageManager, PluginError, Verb};

/// 一个用来被驱动的假插件。
struct Toy;

impl PackageManager for Toy {
    fn name(&self) -> &str {
        "toy"
    }

    fn family(&self) -> Family {
        Family::NODE
    }

    fn command(
        &self,
        ctx: &Context,
        verb: Verb,
        args: &[OsString],
    ) -> Result<CommandSpec, PluginError> {
        // 留一个必定 panic 的入口，用来验证 guard
        if args.iter().any(|a| a == "panic") {
            panic!("这个 panic 必须被 guard 抓住");
        }

        match verb {
            Verb::Install => Ok(CommandSpec::new("toy-bin").arg("add").args(args.iter())),

            // 用 matched 做形态分支 —— 全程不读任何文件
            Verb::Run if ctx.has_matched(".yarnrc.yml") => {
                Ok(CommandSpec::new("toy-bin").arg("berry"))
            }
            Verb::Run => Ok(CommandSpec::new("toy-bin").arg("classic")),

            // 明确声明不支持，让宿主去降级
            Verb::Exec => Err(PluginError::unsupported_verb(verb)),

            other => Err(PluginError::other(format!("{other} 还没实现"))),
        }
    }
}

/// 工厂函数。`export!` 要的就是这么一个路径。
fn create() -> Box<dyn PackageManager> {
    Box::new(Toy)
}

pmpx_plugin::export!(create);

/// 从 C 字符串读回 Rust 字符串。
///
/// # Safety
/// `s` 必须是本进程里有效的一段字节。
unsafe fn read(s: PmpxStr) -> String {
    if s.is_empty() {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    String::from_utf8(bytes.to_vec()).expect("测试里给的都是 UTF-8")
}

fn entry() -> &'static PmpxPluginV1 {
    // SAFETY: 入口返回的是一张 'static 表。
    unsafe { &*pmpx_plugin_entry_v1() }
}

/// 调用一次 `command`，把结果拷成 Rust 值，然后按约定释放插件的内存。
fn call_command(
    root: &str,
    matched: &[&str],
    verb: Option<Verb>,
    args: &[&str],
) -> Result<CommandSpec, u32> {
    let e = entry();

    let root_s = PmpxStr {
        ptr: root.as_ptr(),
        len: root.len(),
    };
    let matched_raw: Vec<PmpxStr> = matched
        .iter()
        .map(|s| PmpxStr {
            ptr: s.as_ptr(),
            len: s.len(),
        })
        .collect();
    let args_raw: Vec<PmpxStr> = args
        .iter()
        .map(|s| PmpxStr {
            ptr: s.as_ptr(),
            len: s.len(),
        })
        .collect();

    let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();

    // SAFETY: 输入由本函数分配并在调用期间保持存活；out 指向本地可写内存。
    // 动词用一个真实编号，或一个刻意越界的编号。
    let verb_code = verb.map(Verb::to_abi).unwrap_or(99);

    let code = unsafe {
        (e.command)(
            root_s,
            matched_raw.as_ptr(),
            matched_raw.len(),
            verb_code,
            args_raw.as_ptr(),
            args_raw.len(),
            out.as_mut_ptr(),
        )
    };

    if code != abi::PMPX_OK {
        return Err(code);
    }

    // SAFETY: code == PMPX_OK 时插件保证已填充 out。
    let mut cmd = unsafe { out.assume_init() };

    // 把这些内存**拷出来**再还给插件 —— 宿主自始至终只读。
    let spec = unsafe {
        let program = read(cmd.program);
        let cwd = if cmd.cwd.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(read(cmd.cwd)))
        };
        let args: Vec<OsString> = (0..cmd.args_len)
            .map(|i| OsString::from(read(*cmd.args.add(i))))
            .collect();

        CommandSpec {
            program: program.into(),
            args,
            cwd,
        }
    };

    // SAFETY: 这个结构体来自上面那次成功的调用，且只释放一次。
    unsafe { (e.free_command)(&mut cmd as *mut _) };

    Ok(spec)
}

#[test]
fn the_vtable_reports_a_compatible_abi_version() {
    assert_eq!(entry().abi_version, abi::ABI_VERSION);
}

#[test]
fn build_info_is_visible_for_diagnostics() {
    let e = entry();
    let rustc = unsafe { read(e.rustc_version) };
    let target = unsafe { read(e.target) };

    assert!(rustc.contains("rustc"), "rustc_version = {rustc:?}");
    assert!(target.contains('-'), "target = {target:?}");
}

#[test]
fn name_and_family_come_back_and_are_freed() {
    let e = entry();

    // SAFETY: 返回的内存归插件，读完用 free_str 还回去。
    unsafe {
        let name = (e.name)();
        assert_eq!(read(name), "toy");
        (e.free_str)(name);

        let family = (e.family)();
        assert_eq!(read(family), "node");
        (e.free_str)(family);
    }
}

#[test]
fn install_maps_and_forwards_args() {
    let spec = call_command("/proj", &[], Some(Verb::Install), &["serde", "anyhow"])
        .expect("install 应当成功");

    assert_eq!(spec.program, OsString::from("toy-bin"));
    assert_eq!(
        spec.args,
        vec![OsString::from("add"), "serde".into(), "anyhow".into()]
    );
    assert_eq!(spec.cwd, None, "没设 cwd 时应当是 None");
}

#[test]
fn install_with_no_args_still_maps() {
    let spec = call_command("/proj", &[], Some(Verb::Install), &[]).expect("应当成功");
    assert_eq!(spec.args, vec![OsString::from("add")]);
}

/// 这是 `Context::matched` 存在的理由：插件靠它做形态分支，**不读任何文件**。
#[test]
fn matched_drives_a_shape_branch() {
    let classic = call_command("/proj", &["yarn.lock"], Some(Verb::Run), &[]).unwrap();
    assert_eq!(classic.args, vec![OsString::from("classic")]);

    let berry = call_command("/proj", &["yarn.lock", ".yarnrc.yml"], Some(Verb::Run), &[]).unwrap();
    assert_eq!(berry.args, vec![OsString::from("berry")]);
}

#[test]
fn unsupported_verb_has_its_own_code() {
    let code = call_command("/proj", &[], Some(Verb::Exec), &[]).unwrap_err();
    assert_eq!(code, abi::PMPX_ERR_UNSUPPORTED_VERB);

    // 与"其它错误"必须分得开 —— 宿主靠这个区分来决定要不要降级
    let other = call_command("/proj", &[], Some(Verb::Build), &[]).unwrap_err();
    assert_eq!(other, abi::PMPX_ERR_INTERNAL);
}

#[test]
fn an_unknown_verb_number_is_rejected() {
    let code = call_command("/proj", &[], None, &[]).unwrap_err();
    assert_eq!(code, abi::PMPX_ERR_INVALID_ARGS);
}

#[test]
fn a_null_out_pointer_is_rejected() {
    let e = entry();
    let root = PmpxStr {
        ptr: "/proj".as_ptr(),
        len: 5,
    };
    let empty: Vec<PmpxStr> = Vec::new();

    // SAFETY: 这里刻意传 null，就是要它拒绝。
    let code = unsafe {
        (e.command)(
            root,
            empty.as_ptr(),
            0,
            Verb::Install.to_abi(),
            empty.as_ptr(),
            0,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(code, abi::PMPX_ERR_INVALID_ARGS);
}

/// panic 必须被 `guard` 抓住并变成错误码 —— 绝不能穿过 `extern "C"`。从 Rust 1.81 起让
/// panic 越过去会直接 abort，那时这个测试进程会整个消失，所以"测试能跑完"本身就是结论。
///
/// 注：默认 panic hook 会把消息打到 stderr，测试输出里那行 "这个 panic 必须被 guard 抓住"
/// 是**预期**的，不是失败。
#[test]
fn a_panicking_plugin_does_not_take_the_host_down() {
    let code = call_command("/proj", &[], Some(Verb::Install), &["panic"]).unwrap_err();
    assert_eq!(code, abi::PMPX_ERR_INTERNAL);
}

#[test]
fn empty_matched_and_args_are_accepted() {
    let spec = call_command("", &[], Some(Verb::Run), &[]).expect("空输入应当也能用");
    assert_eq!(spec.args, vec![OsString::from("classic")]);
}

/// 非 UTF-8 的路径/参数必须能原样穿过边界（Unix 上）。
#[cfg(unix)]
#[test]
fn non_utf8_input_survives_the_boundary() {
    use std::os::unix::ffi::OsStrExt;

    // 0xFF 不是合法 UTF-8，但它是个合法的路径字节
    let raw = [b'/', b'x', 0xFF];
    let root = PmpxStr {
        ptr: raw.as_ptr(),
        len: raw.len(),
    };
    let e = entry();
    let empty: Vec<PmpxStr> = Vec::new();
    let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();

    // SAFETY: 按 ABI 调用，输入在整个调用期间有效。
    let code = unsafe {
        (e.command)(
            root,
            empty.as_ptr(),
            0,
            Verb::Run.to_abi(),
            empty.as_ptr(),
            0,
            out.as_mut_ptr(),
        )
    };
    assert_eq!(code, abi::PMPX_OK);

    // 这个假插件不看 project_root，所以这里只证明"传得进去、不炸"；真正的无损往返在
    // abi 的单测里（read_os_keeps_arbitrary_bytes）。
    let cmd = unsafe { out.assume_init() };
    unsafe { (e.free_command)(&mut cmd as *mut _) };

    let os = std::ffi::OsStr::from_bytes(&raw);
    assert_eq!(os.as_bytes(), raw);
}
