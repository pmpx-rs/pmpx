//! 端到端：把 `export!` 生成的外壳编成**真的 cdylib**，用**宿主真正会用的那个库**加载它，
//! 然后跨 `dlopen` 边界调用。
//!
//! 这是唯一能证明下面四处**互相一致**的测试 —— 任何一处写错，宿主都加载不了插件，而它们
//! 在单测里各测各的，谁也发现不了：
//!
//! ```text
//! KitConfig::new("pmpx") 推导出的入口符号   pmpx_plugin_entry_v1
//! export! 生成的导出符号                     pmpx_plugin_entry_v1
//! KitConfig::new("pmpx") 推导出的库文件名    libpmpx_plugin_toy.so / pmpx_plugin_toy.dll
//! fixture 的 [lib] name                      pmpx_plugin_toy
//! ```
//!
//! 覆盖路径：`cargo build --release` → 铺一个插件库目录 → `CratePluginKit::load()` 真的
//! dlopen 取符号 → 校验 `abi_version` → 跨边界调 `command()` → `free_command()`。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate_plugin_kit::{CratePluginKit, KitConfig};
use pmpx_plugin::abi::{self, PmpxCommand, PmpxPluginV1, PmpxStr};
use pmpx_plugin::Verb;

/// `KitConfig::new("pmpx")` 推导出的 crate 名（`crate_prefix` + 短名）。
const CRATE_NAME: &str = "pmpx-plugin-toy";

/// `KitConfig::new("pmpx")` 推导出的 manifest 文件名。
const MANIFEST_NAME: &str = "pmpx-plugin.toml";

const MANIFEST: &str = r#"
[plugin]
name    = "toy"
version = "0.1.0"
abi     = 1
family  = "node"

# 宿主自己的段 —— 这个 crate 不认识它，也不该动它
[detect]
strong = [".yarnrc.yml"]
weak   = ["package.json"]
"#;

/// 编译 fixture，返回（保活用的临时目录，cdylib 路径）。
///
/// 设了 `PMPX_TEST_PREBUILT_LIB` 就直接用它、不编译 —— 这是给 CI 的跨工具链 job 开的口子：
/// 先用旧工具链把 fixture 编好，再用当前工具链跑测试，于是宿主与插件来自两个不同的 rustc。
///
/// 不设它时就地编译（**不复制出去**，因为 fixture 的 `Cargo.toml` 有一条指向 `../../..`
/// 的路径依赖，复制到别处就断了），target 目录指到临时目录，免得在仓库里留下 `target/`。
fn build_fixture() -> (tempfile::TempDir, PathBuf) {
    if let Ok(prebuilt) = std::env::var("PMPX_TEST_PREBUILT_LIB") {
        let path = PathBuf::from(&prebuilt);
        assert!(
            path.is_file(),
            "PMPX_TEST_PREBUILT_LIB 指向的文件不存在：{prebuilt}"
        );
        println!("用预编译的插件产物（跨工具链模式）：{}", path.display());
        return (tempfile::tempdir().expect("应当能建临时目录"), path);
    }

    let tmp = tempfile::tempdir().expect("应当能建临时目录");

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("toy-plugin");

    let target_dir = tmp.path().join("target");

    // 直接叫 `cargo`：此刻正跑在 cargo test 里，它一定在 PATH 上。
    let status = Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(fixture.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir)
        .status()
        .expect("应当能起 cargo");
    assert!(status.success(), "fixture 应当编译成功");

    let lib = crate_plugin_kit::find_library(&target_dir.join("release"), "pmpx_plugin_toy")
        .expect("应当在产物里找到 fixture 的 cdylib");

    (tmp, lib)
}

/// 铺一个插件库目录，返回（保活用的临时目录，kit）。
fn store_with_fixture(lib: &Path) -> (tempfile::TempDir, CratePluginKit<PmpxPluginV1>) {
    let tmp = tempfile::tempdir().expect("应当能建临时目录");
    let root = tmp.path().join("store");

    let plugin_dir = root.join("plugins").join(CRATE_NAME);
    std::fs::create_dir_all(&plugin_dir).expect("应当能建插件目录");
    std::fs::write(plugin_dir.join(MANIFEST_NAME), MANIFEST).expect("应当能写 manifest");
    std::fs::copy(lib, plugin_dir.join(lib.file_name().unwrap())).expect("应当能复制 cdylib");

    let cfg = KitConfig::new("pmpx")
        .with_data_dir(&root)
        .with_lock_timeout(Duration::from_millis(500));

    // 用默认值就够了：入口符号、库名前缀、manifest 名全由 `id = "pmpx"` 推导，
    // 而 fixture 正是照那套约定写的。
    let kit = CratePluginKit::<PmpxPluginV1>::new(cfg).expect("应当能建起 kit");
    (tmp, kit)
}

/// 按 ABI 调用一次 `command`，把结果拷成 Rust 值后释放插件的内存 ——
/// 就是 pmpx 那边对应代码的缩影。
fn call(
    entry: &PmpxPluginV1,
    project_root: &Path,
    matched: &[&str],
    verb: Verb,
    args: &[&str],
) -> Result<(OsString, Vec<OsString>, Option<PathBuf>), u32> {
    let root_bytes = project_root.to_string_lossy().into_owned().into_bytes();
    let root_s = PmpxStr {
        ptr: root_bytes.as_ptr(),
        len: root_bytes.len(),
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
    let code = unsafe {
        (entry.command)(
            root_s,
            matched_raw.as_ptr(),
            matched_raw.len(),
            verb.to_abi(),
            args_raw.as_ptr(),
            args_raw.len(),
            out.as_mut_ptr(),
        )
    };

    if code != abi::PMPX_OK {
        return Err(code);
    }

    let mut cmd = unsafe { out.assume_init() };

    // SAFETY: 一次成功的调用之后，这些字节都有效。
    let result = unsafe {
        let read = |s: PmpxStr| -> String {
            if s.is_empty() {
                return String::new();
            }
            let bytes = std::slice::from_raw_parts(s.ptr, s.len);
            String::from_utf8_lossy(bytes).into_owned()
        };

        let program: OsString = read(cmd.program).into();
        let cwd = if cmd.cwd.is_empty() {
            None
        } else {
            Some(PathBuf::from(read(cmd.cwd)))
        };
        let args: Vec<OsString> = (0..cmd.args_len)
            .map(|i| OsString::from(read(*cmd.args.add(i))))
            .collect();

        (program, args, cwd)
    };

    // SAFETY: 来自上面那次成功的调用，只释放一次。
    unsafe { (entry.free_command)(&mut cmd as *mut _) };

    Ok(result)
}

#[test]
fn loads_a_real_cdylib_and_drives_the_vtable() {
    let (_build_tmp, lib) = build_fixture();
    let (_store_tmp, kit) = store_with_fixture(&lib);

    // list：只读 manifest，不 dlopen
    let all = kit.list().expect("list 应当成功");
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].name, "toy");
    assert_eq!(all[0].crate_name, CRATE_NAME);

    // load：真的 dlopen，真的取到 pmpx_plugin_entry_v1
    let loaded = kit.load("toy").expect("应当能加载");
    let entry = loaded.entry();
    assert!(!entry.is_null());

    // SAFETY: `PmpxPluginV1` 就是插件导出那张表的类型，两边是同一个 crate 定义的。
    let entry = unsafe { &*entry };

    // ABI 版本：宿主加载后的第一件事
    assert_eq!(
        entry.abi_version,
        abi::ABI_VERSION,
        "插件报的 ABI 版本必须与宿主一致"
    );

    // name / family：跨边界读字符串，再按约定还回去
    // SAFETY: 返回的内存归插件，读完用 free_str 还。
    unsafe {
        let name = (entry.name)();
        let bytes = std::slice::from_raw_parts(name.ptr, name.len);
        assert_eq!(bytes, b"toy");
        (entry.free_str)(name);

        let family = (entry.family)();
        let bytes = std::slice::from_raw_parts(family.ptr, family.len);
        assert_eq!(bytes, b"node");
        (entry.free_str)(family);
    }

    // command：真的跨边界调一次
    let root = PathBuf::from("/tmp/project");
    let (program, args, cwd) = call(entry, &root, &[], Verb::Install, &["serde"]).expect("install");

    assert_eq!(program, OsString::from("toy-bin"));
    assert_eq!(args, vec![OsString::from("add"), OsString::from("serde")]);
    assert_eq!(cwd, None);

    // Context::matched 真的传进去了
    let (_, args, _) = call(entry, &root, &[".yarnrc.yml"], Verb::Install, &[]).expect("berry");
    assert!(
        args.iter().any(|a| a == "--berry"),
        "命中 .yarnrc.yml 时插件应当能看见：{args:?}"
    );

    // project_root 也真的传进去了
    let (_, args, _) = call(entry, &root, &[], Verb::Run, &[]).expect("run");
    assert_eq!(args, vec![root.clone().into_os_string()]);
}

#[test]
fn unsupported_verb_keeps_its_code_across_the_boundary() {
    let (_build_tmp, lib) = build_fixture();
    let (_store_tmp, kit) = store_with_fixture(&lib);

    let loaded = kit.load("toy").expect("应当能加载");
    // SAFETY: 同上面的测试。
    let entry = unsafe { &*loaded.entry() };

    let code = call(entry, Path::new("/tmp/p"), &[], Verb::Exec, &[]).unwrap_err();
    assert_eq!(code, abi::PMPX_ERR_UNSUPPORTED_VERB);
}

/// panic **绝不能**穿过 `extern "C"`：从 Rust 1.81 起让 panic 越过去会直接 abort，那时这个
/// 测试进程会整个消失，而不是报一个失败。所以"测试能跑完、且拿到 INTERNAL"就是结论。
///
/// 注：默认 panic hook 会把消息打到 stderr，测试输出里那行 "这个 panic 必须被 guard 抓住"
/// 是**预期**的。
#[test]
fn a_panicking_plugin_does_not_take_the_host_down() {
    let (_build_tmp, lib) = build_fixture();
    let (_store_tmp, kit) = store_with_fixture(&lib);

    let loaded = kit.load("toy").expect("应当能加载");
    // SAFETY: 同上。
    let entry = unsafe { &*loaded.entry() };

    let code = call(entry, Path::new("/tmp/p"), &[], Verb::Test, &[]).unwrap_err();
    assert_eq!(code, abi::PMPX_ERR_INTERNAL);
}
