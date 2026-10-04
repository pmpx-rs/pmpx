//! 把 [`CommandSpec`] 变成真正的进程。
//!
//! # 为什么 spawn 由 pmpx 做，而不是插件
//!
//! 插件只回答"跑什么"，pmpx 负责"怎么跑"，于是 stdio、环境继承、退出码处理
//! 只有一处实现。
//!
//! # Windows 上必须解析真实路径
//!
//! `Command::new("pnpm")` 在 Windows 上会失败：`CreateProcessW` 只做 PATH 查找加补
//! `.exe`，不做 PATHEXT 解析。而 npm / pnpm / yarn / bun 在 Windows 上全是 `.cmd`
//! shim（`pnpm.cmd`）。
//!
//! 所以先解析出真实路径，再按类型 spawn：`.cmd` / `.bat` 交给 `cmd /d /s /c`，
//! `.ps1` 交给 `pwsh -NoProfile -File`，解析不到就报错（退出码 3）并列出 PATH 里
//! 名字相近的候选。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use pmpx_plugin::CommandSpec;

use crate::error::{PmpxError, Result};

/// 解析到的后端可执行文件的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramKind {
    /// 真正的可执行文件。直接 spawn。
    Native,
    /// `.cmd` / `.bat`。需要 `cmd.exe` 当解释器。
    CmdShim,
    /// `.ps1`。需要 PowerShell。
    PowerShellShim,
}

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// 真实路径。
    pub program: PathBuf,
    /// 它是哪一类。
    pub kind: ProgramKind,
}

/// 解析后端可执行文件的真实路径。
///
/// - `program` 里带路径分隔符 → 直接用，不去 PATH 里搜（用户明确指了路径）。
/// - 否则走 `which`。它在 Windows 上会做 PATHEXT 解析，这正是我们要的。
pub fn resolve(program: &OsStr) -> Result<Resolved> {
    let as_path = Path::new(program);

    let has_separator = as_path.components().any(|c| {
        matches!(
            c,
            std::path::Component::RootDir | std::path::Component::ParentDir
        )
    }) || program.to_string_lossy().contains(['/', '\\']);

    let path = if has_separator {
        if as_path.is_file() {
            as_path.to_path_buf()
        } else {
            return Err(PmpxError::not_found(format!(
                "找不到 {}。它看起来是个路径，但那里没有文件。",
                as_path.display()
            )));
        }
    } else {
        which::which(program).map_err(|_| not_found_error(program))?
    };

    Ok(Resolved {
        kind: kind_of(&path),
        program: path,
    })
}

/// 按扩展名判断该怎么 spawn。
fn kind_of(path: &Path) -> ProgramKind {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "cmd" | "bat" => ProgramKind::CmdShim,
        "ps1" => ProgramKind::PowerShellShim,
        _ => ProgramKind::Native,
    }
}

/// 找不到时给一句能指导下一步的话：列出 PATH 里名字相近的文件 ——
/// 那比干说一句"找不到 pnpm"有用得多。
fn not_found_error(program: &OsStr) -> PmpxError {
    let wanted = program.to_string_lossy().to_ascii_lowercase();
    let near = near_misses(&wanted);

    let mut msg = format!("找不到可执行文件 {}", program.to_string_lossy());

    if near.is_empty() {
        msg.push_str("\nPATH 里没有名字相近的东西 —— 它可能根本没装。");
    } else {
        msg.push_str("\nPATH 里名字相近的有：");
        for p in &near {
            msg.push_str(&format!("\n  · {}", p.display()));
        }
    }

    // Windows 上最常见的那一种，单独点出来
    #[cfg(windows)]
    if matches!(near.first().and_then(|p| p.extension()), Some(e) if e.eq_ignore_ascii_case("cmd"))
    {
        msg.push_str(
            "\n\n提示：Windows 上这些包管理器是 .cmd 脚本，需要靠 PATHEXT 才能解析到；\
             pmpx 已经做了这件事，所以看到这条说明那个目录确实不在 PATH 里。",
        );
    }

    PmpxError::not_found(msg)
}

/// 一个 PATH 里的文件名算不算 `wanted` 的"相近候选"。
///
/// 用前缀匹配而不是编辑距离：编辑距离会把 `pnpm` 与 `npm` 也算成相近，而那是
/// 两个毫无关系的工具。
fn name_matches(wanted_lower: &str, candidate: &str) -> bool {
    let lower = candidate.to_ascii_lowercase();
    lower.starts_with(wanted_lower) && lower != wanted_lower
}

/// 在 PATH 里找名字相近的文件。
fn near_misses(wanted_lower: &str) -> Vec<PathBuf> {
    let Ok(path_var) = std::env::var("PATH") else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for dir in std::env::split_paths(&path_var) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            if name_matches(wanted_lower, &entry.file_name().to_string_lossy()) {
                out.push(entry.path());
            }
        }
        if out.len() >= 5 {
            break;
        }
    }

    out.sort();
    out.truncate(5);
    out
}

/// 按 [`Resolved`] 的类型构造一个 [`Command`]，但不跑它。
///
/// 拆出这一步是为了可测：测试要用 `output()` 抓输出，而 [`run`] 是继承 stdio 的。
pub fn command_for(spec: &CommandSpec, cwd: &Path) -> Result<Command> {
    let resolved = resolve(&spec.program)?;

    let mut cmd = match resolved.kind {
        ProgramKind::Native => {
            let mut c = Command::new(&resolved.program);
            c.args(&spec.args);
            c
        }

        ProgramKind::CmdShim => {
            // ⚠️ 必须自己拼整条命令行，而且必须用 `raw_arg`。
            //
            // 不能逐个 arg 传：`cmd /c` 有自己的引号规则，会按条件剥掉整串的首尾引号，
            // 所以要 `cmd /d /s /c "<path> <args>"` —— 外面那层留给 `/s` 剥。
            // 也不能用 `arg` 传拼好的串：它会再套一层引号、把我们写好的 `"` 转义成
            // `\"`，cmd 就认不出来了。这一段要的是逐字送达，`raw_arg` 就是为这个存在的。
            use std::os::windows::process::CommandExt;

            let mut c = Command::new("cmd");
            c.raw_arg(cmd_raw_command_line(&resolved.program, &spec.args));
            c
        }

        ProgramKind::PowerShellShim => {
            // `-NoProfile` 是刻意的：用户的 PowerShell profile 不该影响包管理器的行为，
            // 而且它可能很慢。`-ExecutionPolicy Bypass` 不加 —— 改安全策略不是 pmpx
            // 该做的事，被策略挡住时让 PowerShell 自己报出真正的原因。
            let mut c = Command::new("pwsh");
            c.arg("-NoProfile").arg("-File").arg(&resolved.program);
            c.args(&spec.args);
            c
        }
    };

    cmd.current_dir(cwd);
    // 原样继承环境 —— pmpx 不参与代理 / 镜像 / 换源的任何一环，那些在 shell 里配。
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    Ok(cmd)
}

/// 拼出交给 `cmd.exe` 的完整原始命令行（含 `/d /s /c` 与给 `/s` 剥的那层引号）。
///
/// 单独抽出来是为了可测：走 `raw_arg` 之后 `Command::get_args()` 是空的，没法内省，
/// 而这条命令行的形状正是最容易写错的地方。
#[cfg(windows)]
fn cmd_raw_command_line(program: &Path, args: &[OsString]) -> String {
    format!("/d /s /c \"{}\"", build_cmd_line(program, args))
}

/// 拼一条给 `cmd /d /s /c` 用的命令行。
#[cfg(windows)]
fn build_cmd_line(program: &Path, args: &[OsString]) -> String {
    let mut line = quote_arg(&program.to_string_lossy());
    for a in args {
        line.push(' ');
        line.push_str(&quote_arg(&a.to_string_lossy()));
    }
    line
}

#[cfg(not(windows))]
fn build_cmd_line(program: &Path, args: &[OsString]) -> String {
    // 非 Windows 上走不到这里（不会有 .cmd），但函数得存在。
    let mut line = program.to_string_lossy().into_owned();
    for a in args {
        line.push(' ');
        line.push_str(&a.to_string_lossy());
    }
    line
}

/// 按 Windows 命令行（`CommandLineToArgvW`）的规则给一个参数加引号。
///
/// 两条规则都很反直觉：`"` 在引号里要写成 `\"`；而**反斜杠只有在引号前面才有特殊
/// 含义** —— 引号前 n 个反斜杠要写 `2n+1` 个，结尾的 n 个反斜杠要翻倍成 `2n`
/// （否则会把收尾引号吃掉）。
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return arg.to_string();
    }

    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');

    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => {
                backslashes += 1;
                out.push('\\');
            }
            '"' => {
                // 引号前的反斜杠要翻倍，再加一个用来转义引号本身
                for _ in 0..=backslashes {
                    out.push('\\');
                }
                out.push('"');
                backslashes = 0;
            }
            _ => {
                backslashes = 0;
                out.push(c);
            }
        }
    }

    // 收尾引号前的反斜杠同样要翻倍
    for _ in 0..backslashes {
        out.push('\\');
    }
    out.push('"');
    out
}

/// 真的跑起来：继承 stdio、等它结束、把它的退出码原样返回。
///
/// 返回值不是 `Result<()>`：因为"测试失败"和"pmpx 出错"是两回事，`pmpx test` 必须
/// 能把后端的非零退出码透传给调用方的脚本 —— 那是这条命令唯一有意义的契约。
pub fn run(spec: &CommandSpec, cwd: &Path) -> Result<u8> {
    let mut cmd = command_for(spec, cwd)?;

    let status = cmd.status().map_err(|e| {
        PmpxError::Other(
            anyhow::anyhow!(e).context(format!("启动 {} 失败", spec.program.to_string_lossy())),
        )
    })?;

    Ok(exit_code_of(status))
}

/// 把 `ExitStatus` 翻译成进程退出码。
fn exit_code_of(status: std::process::ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        if (0..=255).contains(&code) {
            return code as u8;
        }
        // Windows 上退出码可以是任意 u32，而 Unix 只认低 8 位。取低 8 位而不是报错 ——
        // 用户的脚本关心的是"非零"，不是精确值。
        eprintln!("pmpx: 后端退出码 {code} 超出 0-255，按低 8 位透传");
        return (code & 0xFF) as u8;
    }

    // 被信号杀掉（Unix）。沿用 shell 的惯例：128 + signal。
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            eprintln!("pmpx: 后端被信号 {sig} 终止");
            return (128 + sig).clamp(0, 255) as u8;
        }
    }

    eprintln!("pmpx: 拿不到后端的退出码，按 1 处理");
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 类型判定 ----------------------------------------------------------

    #[test]
    fn exe_and_extensionless_are_native() {
        assert_eq!(kind_of(Path::new("C:/x/cargo.exe")), ProgramKind::Native);
        assert_eq!(kind_of(Path::new("/usr/bin/cargo")), ProgramKind::Native);
        assert_eq!(kind_of(Path::new("C:/x/tool.bin")), ProgramKind::Native);
    }

    #[test]
    fn cmd_and_bat_are_shims() {
        assert_eq!(kind_of(Path::new("C:/x/pnpm.cmd")), ProgramKind::CmdShim);
        assert_eq!(kind_of(Path::new("C:/x/old.bat")), ProgramKind::CmdShim);
    }

    /// 扩展名大小写不敏感 —— Windows 上 `.CMD` 和 `.cmd` 是同一个东西。
    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(kind_of(Path::new("C:/x/PNPM.CMD")), ProgramKind::CmdShim);
        assert_eq!(
            kind_of(Path::new("C:/x/Run.Ps1")),
            ProgramKind::PowerShellShim
        );
    }

    #[test]
    fn ps1_needs_powershell() {
        assert_eq!(
            kind_of(Path::new("C:/x/x.ps1")),
            ProgramKind::PowerShellShim
        );
    }

    // ---- 引号规则 ----------------------------------------------------------

    #[test]
    fn simple_args_are_not_quoted() {
        assert_eq!(quote_arg("add"), "add");
        assert_eq!(quote_arg("--noEmit"), "--noEmit");
        assert_eq!(quote_arg("C:/x/y"), "C:/x/y");
    }

    #[test]
    fn args_with_spaces_are_quoted() {
        assert_eq!(quote_arg("C:/Program Files/x"), "\"C:/Program Files/x\"");
    }

    #[test]
    fn empty_arg_becomes_empty_quotes() {
        // 不这样写的话空参数会在命令行上直接消失
        assert_eq!(quote_arg(""), "\"\"");
    }

    #[test]
    fn inner_quotes_are_escaped() {
        assert_eq!(quote_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    /// 反斜杠只有在引号前才特殊 —— 所以没有空格就不用加引号，
    /// 也就不存在结尾反斜杠的问题。
    #[test]
    fn an_arg_without_spaces_is_left_alone_even_with_backslashes() {
        assert_eq!(quote_arg("C:\\dir\\"), "C:\\dir\\");
        assert_eq!(quote_arg("C:\\x\\y"), "C:\\x\\y");
    }

    /// 但一旦要加引号，结尾的反斜杠必须翻倍，否则会把收尾引号转义掉。
    #[test]
    fn trailing_backslashes_are_doubled_when_quoting() {
        assert_eq!(quote_arg("a b\\"), "\"a b\\\\\"");
        assert_eq!(quote_arg("C:\\a b\\"), "\"C:\\a b\\\\\"");
    }

    #[test]
    fn backslashes_before_a_quote_are_doubled_and_the_quote_escaped() {
        // `a\"` → 一个反斜杠 + 引号 → `a\\\"`
        assert_eq!(quote_arg("a\\\"b"), "\"a\\\\\\\"b\"");
    }

    #[test]
    fn a_plain_backslash_is_untouched_inside_quotes() {
        assert_eq!(quote_arg("C:\\a b\\c"), "\"C:\\a b\\c\"");
    }

    // ---- 相近候选的判定 ----------------------------------------------------

    #[test]
    fn a_cmd_shim_is_a_near_miss_for_its_bare_name() {
        assert!(name_matches("pnpm", "pnpm.cmd"));
        assert!(name_matches("pnpm", "PNPM.CMD"), "大小写不敏感");
        assert!(name_matches("pnpm", "pnpm.cmd.old"));
    }

    /// 只差一个字母的两个名字不是相近候选 —— 这正是用前缀匹配而不是编辑距离的理由。
    #[test]
    fn a_different_tool_is_not_a_near_miss() {
        assert!(!name_matches("npm", "pnpm.cmd"));
        assert!(!name_matches("pnpm", "npmy"));
    }

    /// 前缀匹配的已知代价：`bun` 会把 `bunzip2`、`npm` 会把 `npmx` 也捞进来。
    /// 认账但不修 —— 换成编辑距离只会更糟。
    #[test]
    fn prefix_matching_accepts_some_false_positives() {
        assert!(name_matches("bun", "bunzip2"));
        assert!(name_matches("npm", "npmx"));
    }

    #[test]
    fn an_exact_name_is_not_a_near_miss() {
        // 精确命中说明它本来就会被找到，不该出现在"相近候选"里
        assert!(!name_matches("cargo", "cargo"));
        assert!(!name_matches("cargo", "CARGO"));
    }

    // ---- 解析 --------------------------------------------------------------

    /// 带路径分隔符时直接用，不去 PATH 里搜。
    #[test]
    fn an_explicit_path_is_used_as_is() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("thing");
        std::fs::write(&p, "").unwrap();

        let r = resolve(p.as_os_str()).unwrap();
        assert_eq!(r.program, p);
    }

    #[test]
    fn an_explicit_path_that_does_not_exist_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("nope").join("thing");

        let err = resolve(p.as_os_str()).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_NOT_FOUND);
        assert!(err.to_string().contains("路径"));
    }

    /// 当前一定能解析到 `cargo` —— 我们正跑在 `cargo test` 里。
    #[test]
    fn resolves_a_real_program_from_path() {
        let r = resolve(OsStr::new("cargo")).expect("cargo 必须在 PATH 上");
        assert!(r.program.is_absolute(), "{:?}", r.program);
    }

    #[test]
    fn a_missing_program_gives_exit_code_three() {
        let err = resolve(OsStr::new("pmpx-definitely-not-a-real-program-xyz")).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_NOT_FOUND);
    }

    /// `which` 在 Windows 上做 PATHEXT 解析，`pnpm` 才能解析到 `pnpm.cmd`。
    ///
    /// 这个测试直接问 `which`（用 `which_in` 指定搜索目录），而不是走 [`resolve`]：
    /// `resolve` 读的是进程级 PATH，而并发跑的测试里改那个变量是不安全的。
    #[cfg(windows)]
    #[test]
    fn which_does_pathex_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("pmpxprobe.cmd");
        std::fs::write(&shim, "@echo off\r\n").unwrap();

        let found = which::which_in("pmpxprobe", Some(tmp.path()), tmp.path())
            .expect("which 应当能靠 PATHEXT 找到 .cmd");

        assert_eq!(
            found
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase(),
            "pmpxprobe.cmd"
        );
        assert_eq!(kind_of(&found), ProgramKind::CmdShim);
    }

    /// 找不到时那条消息必须有信息量。
    #[test]
    fn a_missing_program_says_something_useful() {
        let err = resolve(OsStr::new("pmpx-definitely-not-a-real-program-xyz")).unwrap_err();
        let msg = err.to_string();

        assert!(
            msg.contains("pmpx-definitely-not-a-real-program-xyz"),
            "{msg}"
        );
        assert!(
            msg.contains("根本没装") || msg.contains("名字相近"),
            "要么说清可能没装，要么列出候选：{msg}"
        );
    }

    // ---- 真的跑一条命令 ----------------------------------------------------

    #[test]
    fn runs_a_native_command_and_returns_its_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CommandSpec::new("cargo").arg("--version");
        assert_eq!(
            run(&spec, tmp.path()).unwrap(),
            0,
            "cargo --version 应当成功"
        );
    }

    #[test]
    fn a_nonzero_backend_exit_code_is_passed_through() {
        let tmp = tempfile::tempdir().unwrap();

        #[cfg(windows)]
        let spec = CommandSpec::new("cmd").arg("/c").arg("exit 7");
        #[cfg(not(windows))]
        let spec = CommandSpec::new("sh").arg("-c").arg("exit 7");

        assert_eq!(run(&spec, tmp.path()).unwrap(), 7, "必须原样透传");
    }

    /// 工作目录要真的生效 —— 插件报的 `cwd` 与项目根都靠它。
    #[test]
    fn the_working_directory_is_honoured() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("pmpx-cwd-probe.txt");
        std::fs::write(&marker, "here").unwrap();

        let spec = CommandSpec::new("cargo").arg("--version");
        let mut cmd = command_for(&spec, tmp.path()).unwrap();
        let out = cmd.output().unwrap();

        // cargo 只会在有 Cargo.toml 的地方做别的事，这里只要证明"能跑起来"就够了
        assert!(out.status.success());
    }

    // ---- command_for 的构造 ------------------------------------------------

    #[test]
    fn a_missing_program_fails_before_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CommandSpec::new("pmpx-definitely-not-a-real-program-xyz");

        assert!(command_for(&spec, tmp.path()).is_err());
    }

    /// Windows 上 `.cmd` 必须被 `cmd /d /s /c` 包起来，且首尾各有一层给 `/s` 剥的引号。
    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_is_wrapped_in_cmd_exe() {
        let shim = Path::new("C:\\path with space\\pnpm.cmd");
        let args = vec![OsString::from("add"), OsString::from("serde")];

        let line = cmd_raw_command_line(shim, &args);

        assert!(line.starts_with("/d /s /c "), "{line}");
        // 最外层那对引号是给 `/s` 剥的
        assert!(line.ends_with('"'), "{line}");
        assert_eq!(line.matches('"').count(), 4, "路径一对 + 外层一对：{line}");
        assert!(line.contains("\"C:\\path with space\\pnpm.cmd\""), "{line}");
        assert!(line.ends_with("add serde\""), "{line}");
    }

    /// 路径没有空格时不必加引号，但外层那对必须在 —— `/s` 的行为依赖于它。
    #[cfg(windows)]
    #[test]
    fn the_outer_quote_pair_is_always_present() {
        let line = cmd_raw_command_line(Path::new("C:\\x\\pnpm.cmd"), &[]);
        assert_eq!(line, "/d /s /c \"C:\\x\\pnpm.cmd\"");
    }

    /// 真的跑一个 `.cmd`，并确认参数传进去了。
    ///
    /// `cmd` 的引号规则是一套独立实现，只能真跑一遍来确认。
    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_really_runs_and_receives_its_args() {
        let tmp = tempfile::tempdir().unwrap();
        let out_file = tmp.path().join("got.txt");
        let shim = tmp.path().join("probe.cmd");
        std::fs::write(
            &shim,
            format!("@echo off\r\necho %1 %2 > \"{}\"\r\n", out_file.display()),
        )
        .unwrap();

        let spec = CommandSpec::new(shim.as_os_str()).arg("add").arg("serde");
        assert_eq!(run(&spec, tmp.path()).unwrap(), 0);

        let got = std::fs::read_to_string(&out_file).unwrap();
        assert_eq!(got.trim(), "add serde");
    }

    /// 路径里带空格时 `.cmd` 也要能跑 —— `/s` 加双层引号就是为这个。
    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_in_a_path_with_spaces_still_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a dir with spaces");
        std::fs::create_dir_all(&dir).unwrap();

        let shim = dir.join("probe.cmd");
        std::fs::write(&shim, "@echo off\r\nexit 0\r\n").unwrap();

        let spec = CommandSpec::new(shim.as_os_str());
        assert_eq!(
            run(&spec, tmp.path()).unwrap(),
            0,
            "带空格的路径必须能跑通 —— 这是双层引号存在的理由"
        );
    }

    /// 带空格的参数也要能原样到达。
    ///
    /// 用 `%~1` 而不是 `%1`：`%1` 会把引号一起带进来（cmd 的既有行为），
    /// `%~1` 才是"去掉包裹引号的值"。
    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_receives_an_argument_with_spaces() {
        let tmp = tempfile::tempdir().unwrap();
        let out_file = tmp.path().join("got.txt");
        let shim = tmp.path().join("probe.cmd");
        std::fs::write(
            &shim,
            format!("@echo off\r\necho %~1 > \"{}\"\r\n", out_file.display()),
        )
        .unwrap();

        let spec = CommandSpec::new(shim.as_os_str()).arg("hello world");
        assert_eq!(run(&spec, tmp.path()).unwrap(), 0);

        let got = std::fs::read_to_string(&out_file).unwrap();
        assert_eq!(got.trim(), "hello world");
    }

    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_passes_through_a_nonzero_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("probe.cmd");
        std::fs::write(&shim, "@echo off\r\nexit /b 42\r\n").unwrap();

        let spec = CommandSpec::new(shim.as_os_str());
        assert_eq!(run(&spec, tmp.path()).unwrap(), 42);
    }
}
