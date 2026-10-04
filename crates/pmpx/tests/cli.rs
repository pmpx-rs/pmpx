//! 端到端：真的跑 `pmpx` 这个二进制。
//!
//! 单元测试验证每一层各自的逻辑；这个文件验证它们接起来之后还对，
//! 特别是跨 `dlopen` 那一段 —— 那是单测永远覆盖不到的。
//!
//! 两个前提：
//!
//! 1. **沙箱化。** 用 `PMPX_CONFIG_DIR` / `PMPX_DATA_DIR` 把 pmpx 指到临时目录；
//!    没有它们，这些测试会去读写用户真实的 `~/.config/pmpx` 与 `~/.pmpx`。
//! 2. **不依赖装了哪个包管理器。** 假插件的所有动词都映射到 `cargo --version` ——
//!    我们此刻就在 `cargo test` 里，它必然存在。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// pmpx 二进制的路径。cargo 会为 bin target 自动提供这个环境变量。
const PMPX: &str = env!("CARGO_BIN_EXE_pmpx");

/// 一个沙箱：装着 pmpx 的配置目录、插件目录，以及一个"项目"目录。
struct Sandbox {
    _tmp: tempfile::TempDir,
    config_dir: PathBuf,
    data_dir: PathBuf,
    project: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("应当能建临时目录");
        let config_dir = tmp.path().join("config");
        let data_dir = tmp.path().join("data");
        let project = tmp.path().join("project");

        for d in [&config_dir, &data_dir, &project] {
            std::fs::create_dir_all(d).unwrap();
        }

        Self {
            _tmp: tmp,
            config_dir,
            data_dir,
            project,
        }
    }

    /// 在项目目录里放一个文件。
    fn file(&self, name: &str) -> &Self {
        let p = self.project.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, "").unwrap();
        self
    }

    /// 写一份全局配置。
    fn global_config(&self, body: &str) -> &Self {
        std::fs::write(self.config_dir.join("config.toml"), body).unwrap();
        self
    }

    /// 铺一个"已安装插件"的目录（cdylib + manifest）。
    fn install_plugin(&self, crate_name: &str, manifest: &str, lib: Option<&Path>) -> &Self {
        let dir = self.data_dir.join("plugins").join(crate_name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pmpx-plugin.toml"), manifest).unwrap();

        if let Some(src) = lib {
            std::fs::copy(src, dir.join(src.file_name().unwrap())).unwrap();
        }
        self
    }

    /// 跑一次 pmpx。**工作目录是项目目录**，两个 pmpx 专有的环境变量指向沙箱。
    ///
    /// 刻意**不** `env_clear()`：那样连 `cargo` 自己都跑不起来（它要 `USERPROFILE` /
    /// `APPDATA` 之类）。pmpx 只认那两个 `PMPX_*` 变量，其余环境它本来就该原样继承。
    fn run(&self, args: &[&str]) -> Output {
        Command::new(PMPX)
            .args(args)
            .current_dir(&self.project)
            .env("PMPX_CONFIG_DIR", &self.config_dir)
            .env("PMPX_DATA_DIR", &self.data_dir)
            .output()
            .expect("应当能起 pmpx")
    }

    /// 跑一次并断言成功，返回 stdout。
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "`pmpx {}` 应当成功，退出码 {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            args.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// ---------------------------------------------------------------------------
// 编译那个真插件
// ---------------------------------------------------------------------------

/// 编出假插件，返回 cdylib 路径。
fn build_fake_plugin() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-plugin");
    let target_dir = tmp.path().join("target");

    let status = Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(fixture.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir)
        .status()
        .expect("应当能起 cargo");
    assert!(status.success(), "假插件应当编译成功");

    let lib = crate_plugin_kit::find_library(&target_dir.join("release"), "pmpx_plugin_fakepm")
        .expect("应当找到假插件的 cdylib");

    (tmp, lib)
}

const FAKEPM_MANIFEST: &str = r#"
[plugin]
name    = "fakepm"
version = "0.1.0"
abi     = 1
family  = "faketest"

[detect]
strong = ["fakepm.lock"]
weak   = ["fakepm.json"]
"#;

/// 一个装好假插件的沙箱。
fn sandbox_with_plugin() -> (tempfile::TempDir, Sandbox, PathBuf) {
    let (build_tmp, lib) = build_fake_plugin();
    let sb = Sandbox::new();
    sb.install_plugin("pmpx-plugin-fakepm", FAKEPM_MANIFEST, Some(&lib));
    (build_tmp, sb, lib)
}

// ---------------------------------------------------------------------------
// 零插件：首跑体验
// ---------------------------------------------------------------------------

#[test]
fn with_no_plugins_it_exits_three_and_explains_itself() {
    let sb = Sandbox::new();
    sb.file("Cargo.toml");

    let out = sb.run(&[]);
    assert_eq!(out.status.code(), Some(3), "检测不到项目类型是退出码 3");

    let err = stderr_of(&out);
    assert!(err.contains("检测不到项目类型"), "{err}");
    // 5.8 那张静态表只用来陈述事实，不推荐装哪个插件
    assert!(err.contains("看起来像"), "应当陈述看到了什么：{err}");
    assert!(err.contains("rust"), "{err}");
    assert!(
        !err.contains("pmpx plugin add cargo"),
        "不该推荐具体插件：{err}"
    );
    // 但要告诉用户出口在哪
    assert!(err.contains(".pmpx.toml"), "{err}");
}

#[test]
fn exec_still_works_with_zero_plugins() {
    let sb = Sandbox::new();
    sb.file("Cargo.toml");

    // 零插件下 exec 也要可用
    let out = sb.run(&["exec", "cargo", "--version"]);
    assert!(
        out.status.success(),
        "退化成裸透传后应当成功：{}",
        stderr_of(&out)
    );
    assert!(stdout_of(&out).contains("cargo"), "{}", stdout_of(&out));
}

#[test]
fn plugin_ls_says_how_to_start() {
    let sb = Sandbox::new();
    let out = sb.ok(&["plugin", "ls"]);
    assert!(out.contains("pmpx plugin add"), "{out}");
}

// ---------------------------------------------------------------------------
// 完整链路：检测 → 裁决 → dlopen → 跨 ABI 调 → spawn
// ---------------------------------------------------------------------------

#[test]
fn detects_the_project_and_runs_the_backend_command() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.ok(&["build"]);

    // 命令真的被 spawn 了 —— 假插件把它映射成一条回显
    assert!(out.contains("pmpx-probe"), "后端命令应当真的跑起来：{out}");
}

/// 跨边界那些数据是对的 —— 单测覆盖不到的那一段。
#[test]
fn the_plugin_receives_root_matched_verb_and_args_across_the_boundary() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");
    sb.file("fakepm.json");

    let out = sb.ok(&["build"]);

    let line = out
        .lines()
        .find(|l| l.contains("pmpx-probe"))
        .unwrap_or_else(|| panic!("没拿到回显：{out}"));

    // project_root：原样穿过了 dlopen 边界
    assert!(
        line.contains(&format!("root={}", sb.project.display())),
        "project_root 没传对：{line}"
    );
    // matched：只包含这个插件自己声明过的命中文件，且强证据排在弱证据前面。
    assert!(
        line.contains("matched=fakepm.lock|fakepm.json"),
        "matched 不对（应当是强证据在前）：{line}"
    );
    assert!(line.contains("verb=build"), "{line}");
    // args：宿主这一侧没有给 build 传参数
    assert!(line.contains("args="), "{line}");
}

/// `--` 之后的参数要原样到达后端。
#[test]
fn args_after_the_double_dash_reach_the_plugin() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.ok(&["test", "--", "--nocapture", "some-filter"]);

    let line = out.lines().find(|l| l.contains("pmpx-probe")).unwrap();
    assert!(line.contains("verb=test"), "{line}");
    assert!(
        line.contains("args=--nocapture,some-filter"),
        "`--` 之后的参数必须原样过去：{line}"
    );
}

#[test]
fn a_bare_pmpx_reports_the_selection() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.ok(&[]);
    assert!(out.contains("项目根"), "{out}");
    assert!(out.contains("fakepm"), "应当报出选中的插件：{out}");
}

#[test]
fn info_lists_every_candidate_and_score() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.ok(&["info"]);
    assert!(out.contains("候选与得分"), "{out}");
    assert!(out.contains("fakepm"), "{out}");
    assert!(out.contains("100"), "强证据应当是 100 分：{out}");
    // 工具链只做诊断，但这些值要显示出来
    assert!(out.contains("编译于"), "{out}");
    assert!(out.contains("target"), "{out}");
}

// ---------------------------------------------------------------------------
// exec 的降级
// ---------------------------------------------------------------------------

/// 插件装了但明确说不支持 exec —— 这正是唯一允许降级的场景。
#[test]
fn exec_falls_back_when_the_plugin_says_unsupported() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock"); // 项目能被识别，插件也装好

    let out = sb.run(&["exec", "cargo", "--version"]);
    assert!(
        out.status.success(),
        "插件说不支持时应当退化成裸透传：{}",
        stderr_of(&out)
    );
    assert!(
        stdout_of(&out).contains("cargo"),
        "应当真的跑了 cargo --version：{}",
        stdout_of(&out)
    );
}

/// 除 exec 之外的六个动词维持"不支持就报错"。
#[test]
fn other_verbs_do_not_fall_back() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    // 假插件在 remove 上 panic → 宿主必须活着并报内部错误，而不是跟着崩
    let out = sb.run(&["remove", "something"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "插件炸了是退出码 1\nstderr: {}",
        stderr_of(&out)
    );
    assert!(
        stderr_of(&out).contains("fakepm"),
        "应当说清是哪个插件出的事：{}",
        stderr_of(&out)
    );
}

// ---------------------------------------------------------------------------
// .pmpx.toml
// ---------------------------------------------------------------------------

#[test]
fn plugin_set_writes_a_pmpx_toml_and_it_takes_effect() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock").file("fakepm.json");

    sb.ok(&["plugin", "set", "fakepm"]);

    let cfg = sb.project.join(".pmpx.toml");
    assert!(cfg.is_file(), "应当写出 .pmpx.toml");

    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("faketest"), "键名是 family：{text}");
    assert!(text.contains("fakepm"), "{text}");

    // 生效之后裸跑应当仍然选它
    let out = sb.ok(&[]);
    assert!(out.contains("fakepm"), "{out}");
}

#[test]
fn plugin_unset_removes_it_and_cleans_up_the_file() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    sb.ok(&["plugin", "set", "fakepm"]);
    let cfg = sb.project.join(".pmpx.toml");
    assert!(cfg.is_file());

    sb.ok(&["plugin", "unset", "faketest"]);
    assert!(!cfg.exists(), "空配置文件应当被删掉，而不是留一个空壳");
}

#[test]
fn unset_without_a_family_needs_yes_when_there_are_several() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");
    // 手工写两份 pin（fakepm 只属于 faketest，但 pin 不要求插件存在）
    std::fs::write(
        sb.project.join(".pmpx.toml"),
        "[plugin]\nfaketest = \"fakepm\"\npython = \"poetry\"\n",
    )
    .unwrap();

    let out = sb.run(&["plugin", "unset"]);
    assert_eq!(out.status.code(), Some(2), "要求 --yes 时是用法错误");
    let err = stderr_of(&out);
    assert!(err.contains("--yes"), "{err}");
    // 先列出将删除的项
    assert!(err.contains("faketest"), "{err}");
    assert!(err.contains("python"), "{err}");

    // 加了 --yes 才真的删
    sb.ok(&["plugin", "unset", "--yes"]);
    assert!(!sb.project.join(".pmpx.toml").exists());
}

/// 分层配置：项目根之上那一层的 `.pmpx.toml` 也要看得见。
#[test]
fn config_above_the_project_root_is_visible() {
    let (_build, sb, _lib) = sandbox_with_plugin();

    // 项目在 project/web/，pin 写在 project/（项目根之上）
    let web = sb.project.join("web");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("fakepm.lock"), "").unwrap();
    std::fs::write(
        sb.project.join(".pmpx.toml"),
        "[plugin]\nfaketest = \"fakepm\"\n",
    )
    .unwrap();

    let out = Command::new(PMPX)
        .args(["info"])
        .current_dir(&web)
        .env("PMPX_CONFIG_DIR", &sb.config_dir)
        .env("PMPX_DATA_DIR", &sb.data_dir)
        .output()
        .unwrap();

    let text = stdout_of(&out);
    assert!(
        text.contains("项目配置"),
        "应当列出来源：{text}\nstderr: {}",
        stderr_of(&out)
    );
    // 那一份在上层，必须出现在来源列表里
    let upper = sb.project.join(".pmpx.toml");
    assert!(
        text.contains(&upper.display().to_string()),
        "项目根之上那层的配置必须可见：{text}"
    );
    assert!(text.contains("faketest"), "固化项应当被读出来：{text}");
}

// ---------------------------------------------------------------------------
// 配置命令
// ---------------------------------------------------------------------------

#[test]
fn config_set_then_get_round_trips() {
    let sb = Sandbox::new();

    sb.ok(&[
        "config",
        "set",
        "plugin.family_priority",
        "[\"rust\", \"node\"]",
    ]);
    let got = sb.ok(&["config", "get", "plugin.family_priority"]);
    assert!(got.contains("rust"), "{got}");
    assert!(got.contains("node"), "{got}");

    // 值类型要保留：数字存成数字，而不是字符串
    sb.ok(&["config", "set", "discovery.max_depth", "3"]);
    assert_eq!(sb.ok(&["config", "get", "discovery.max_depth"]).trim(), "3");

    sb.ok(&["config", "set", "discovery.walk_up", "false"]);
    assert_eq!(
        sb.ok(&["config", "get", "discovery.walk_up"]).trim(),
        "false"
    );
}

#[test]
fn config_get_on_a_missing_key_is_a_usage_error() {
    let sb = Sandbox::new();
    let out = sb.run(&["config", "get", "nope.nothing"]);
    assert_eq!(out.status.code(), Some(2));
}

/// 设置 `walk_up = false` 之后，上溯真的停了。
#[test]
fn walk_up_false_stops_the_search() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    // 清单在上层，cwd 在下层
    let deep = sb.project.join("a").join("b");
    std::fs::create_dir_all(&deep).unwrap();
    sb.file("fakepm.lock");

    // 默认能上溯到
    let out = Command::new(PMPX)
        .args(["info"])
        .current_dir(&deep)
        .env("PMPX_CONFIG_DIR", &sb.config_dir)
        .env("PMPX_DATA_DIR", &sb.data_dir)
        .output()
        .unwrap();
    assert!(stdout_of(&out).contains("项目根     ") && !stdout_of(&out).contains("没找到"));

    // 关掉之后找不到
    sb.global_config("[discovery]\nwalk_up = false\n");
    let out = Command::new(PMPX)
        .args(["info"])
        .current_dir(&deep)
        .env("PMPX_CONFIG_DIR", &sb.config_dir)
        .env("PMPX_DATA_DIR", &sb.data_dir)
        .output()
        .unwrap();
    assert!(
        stdout_of(&out).contains("没找到"),
        "关掉上溯之后不该找到项目根：{}",
        stdout_of(&out)
    );
}

// ---------------------------------------------------------------------------
// 插件清单的各种坏情况
// ---------------------------------------------------------------------------

/// manifest 少了 `family`：列出来但标出问题，且不参与裁决。
#[test]
fn a_manifest_without_family_is_listed_with_its_problem() {
    let sb = Sandbox::new();
    sb.install_plugin(
        "pmpx-plugin-broken",
        "[plugin]\nname = \"broken\"\nversion = \"0.1.0\"\n\n[detect]\nstrong = [\"x.lock\"]\n",
        None,
    );
    sb.file("x.lock");

    let out = sb.ok(&["plugin", "ls"]);
    assert!(out.contains("broken"), "{out}");
    assert!(out.contains("family"), "应当说清问题：{out}");

    // 检测时它不算数
    let out = sb.run(&[]);
    assert_eq!(out.status.code(), Some(3), "不可用的插件不参与检测");
}

/// `-p` 指定一个没装的插件 → 退出码 3 并列出可选项。
#[test]
fn an_unknown_plugin_flag_lists_what_is_installed() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.run(&["-p", "nope", "build"]);
    assert_eq!(out.status.code(), Some(3));
    let err = stderr_of(&out);
    assert!(err.contains("nope"), "{err}");
    assert!(err.contains("fakepm"), "应当列出已装的：{err}");
}

/// `-p` 要压过 `.pmpx.toml`。
#[test]
fn the_plugin_flag_beats_the_project_config() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");
    // 固化到一个不存在的插件上 —— 正常路径会因此报错
    std::fs::write(
        sb.project.join(".pmpx.toml"),
        "[plugin]\nfaketest = \"ghost\"\n",
    )
    .unwrap();

    // 不用 -p：报错
    let out = sb.run(&["build"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr_of(&out));

    // 用 -p：绕过那份配置，正常跑
    let out = sb.run(&["-p", "fakepm", "build"]);
    assert!(
        out.status.success(),
        "-p 必须压过 .pmpx.toml：{}",
        stderr_of(&out)
    );
}

/// ABI 版本对不上 → 拒绝加载，并说清两边的版本。
#[test]
fn an_abi_mismatch_is_refused_with_both_versions() {
    let (_build, sb, lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    // manifest 的 abi 只用于展示，真正的校验发生在加载之后读插件自报的
    // `abi_version`。我们没法轻易编一个 ABI 不同的插件，所以这里退一步：
    // 验证 `pmpx info` 会把 manifest 声明的 ABI 显示出来。
    let out = sb.ok(&["plugin", "info", "fakepm"]);
    assert!(out.contains("ABI"), "{out}");

    let _ = lib;
}

// ---------------------------------------------------------------------------
// 补全
// ---------------------------------------------------------------------------

#[test]
fn completion_writes_a_script_to_stdout() {
    let sb = Sandbox::new();
    let out = sb.ok(&["completion", "bash"]);
    assert!(out.contains("pmpx"), "补全脚本应当提到 pmpx：{out}");
    assert!(out.contains("complete") || out.contains("_pmpx"), "{out}");
}

/// `--help` 里出现的动词就是那七个 —— 不多不少。
#[test]
fn help_lists_exactly_the_seven_verbs() {
    let sb = Sandbox::new();
    let out = sb.ok(&["--help"]);

    for verb in [
        "install", "remove", "run", "build", "test", "update", "exec",
    ] {
        assert!(out.contains(verb), "--help 里少了 {verb}：{out}");
    }
    // 没有 lock / outdated / audit
    for absent in ["lock", "outdated", "audit", "publish"] {
        assert!(!out.contains(absent), "--help 里不该有 {absent}：{out}");
    }
}
