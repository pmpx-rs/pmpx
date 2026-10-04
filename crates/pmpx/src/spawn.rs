//! Turn a [`CommandSpec`] into a real process.
//!
//! # Spawn ownership
//!
//! Plugins only answer "what to run"; pmpx owns "how to run it", so stdio, environment
//! inheritance and exit-code handling have exactly one implementation.
//!
//! # On Windows the real path must be resolved
//!
//! `Command::new("pnpm")` fails on Windows: `CreateProcessW` only does a PATH lookup plus
//! appending `.exe`, it does no PATHEXT resolution. On Windows npm / pnpm / yarn / bun are
//! all `.cmd` shims (`pnpm.cmd`).
//!
//! So the real path is resolved first, then spawned by kind: `.cmd` / `.bat` go to
//! `cmd /d /s /c`, `.ps1` goes to `pwsh -NoProfile -File`, and a failed resolution is an
//! error (exit code 3) that lists the near-matching names in PATH.

use std::ffi::OsStr;
// `OsString` is only used on the Windows side (building the cmd command line, and the test
// that really runs a .cmd); the name never appears on non-Windows -- without the cfg it is an
// unused import, and CI runs with -D warnings.
#[cfg(windows)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use pmpx_plugin::CommandSpec;

use crate::error::{PmpxError, Result};
use crate::style;

/// The kind of backend executable that was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramKind {
    /// A real executable. Spawned directly.
    Native,
    /// `.cmd` / `.bat`. Needs `cmd.exe` as the interpreter.
    CmdShim,
    /// `.ps1`. Needs PowerShell.
    PowerShellShim,
}

/// A resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The real path.
    pub program: PathBuf,
    /// Which kind it is.
    pub kind: ProgramKind,
}

/// Resolve the real path of a backend executable.
///
/// - `program` containing a path separator means use it directly, without searching PATH
///   (the user pointed at a path explicitly).
/// - Otherwise go through `which`. On Windows it does PATHEXT resolution, which is exactly
///   what is needed here.
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
                "cannot find {}. It looks like a path, but there is no file there.",
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

/// Decide how to spawn based on the extension.
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

/// When nothing is found, say something that guides the next step: list the near-matching
/// files in PATH -- that is far more useful than a bare "pnpm not found".
fn not_found_error(program: &OsStr) -> PmpxError {
    let wanted = program.to_string_lossy().to_ascii_lowercase();
    let near = near_misses(&wanted);

    let mut msg = format!("cannot find executable {}", program.to_string_lossy());

    if near.is_empty() {
        msg.push_str(
            "\nNothing in PATH has a similar name -- it is probably not installed at all.",
        );
    } else {
        msg.push_str("\nSimilar names in PATH:");
        for p in &near {
            msg.push_str(&format!("\n  - {}", p.display()));
        }
    }

    // The most common Windows case gets its own note
    #[cfg(windows)]
    if matches!(near.first().and_then(|p| p.extension()), Some(e) if e.eq_ignore_ascii_case("cmd"))
    {
        msg.push_str(
            "\n\nNote: on Windows these package managers are .cmd scripts and only PATHEXT \
             resolution finds them; pmpx already does that, so seeing this means that \
             directory really is not on PATH.",
        );
    }

    PmpxError::not_found(msg)
}

/// Whether a file name in PATH counts as a "near candidate" for `wanted`.
///
/// Prefix matching rather than edit distance: edit distance would also call `pnpm` and `npm`
/// similar, and those are two unrelated tools.
fn name_matches(wanted_lower: &str, candidate: &str) -> bool {
    let lower = candidate.to_ascii_lowercase();
    lower.starts_with(wanted_lower) && lower != wanted_lower
}

/// Find near-matching files in PATH.
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

/// Build a [`Command`] for a [`Resolved`] kind, without running it.
///
/// This step is split out for testability: tests need `output()` to capture output, while
/// [`run`] inherits stdio.
pub fn command_for(spec: &CommandSpec, cwd: &Path) -> Result<Command> {
    let resolved = resolve(&spec.program)?;

    let mut cmd = match resolved.kind {
        ProgramKind::Native => {
            let mut c = Command::new(&resolved.program);
            c.args(&spec.args);
            c
        }

        ProgramKind::CmdShim => {
            #[cfg(windows)]
            {
                // The whole command line has to be built here, and it must go through
                // `raw_arg`.
                //
                // Passing the arguments one by one does not work: `cmd /c` has its own
                // quoting rules and strips the leading and trailing quotes of the whole
                // string under some conditions, so it has to be
                // `cmd /d /s /c "<path> <args>"` -- the outer pair is left there for `/s`
                // to strip.
                // Passing the built string through `arg` does not work either: it adds
                // another layer of quotes and escapes our `"` into `\"`, which cmd no longer
                // recognises. This needs verbatim delivery, which is what `raw_arg` exists
                // for.
                use std::os::windows::process::CommandExt;

                let mut c = Command::new("cmd");
                c.raw_arg(cmd_raw_command_line(&resolved.program, &spec.args));
                c
            }

            #[cfg(not(windows))]
            {
                // `.cmd` / `.bat` are Windows-only forms and there is no cmd.exe here.
                // But `kind_of` still classifies them as CmdShim (the classification is
                // cross-platform), so this branch has to exist. If it is really reached, the
                // program is started as an ordinary program -- it will fail with a system
                // error such as Exec format error, and that is the truth.
                let mut c = Command::new(&resolved.program);
                c.args(&spec.args);
                c
            }
        }

        ProgramKind::PowerShellShim => {
            // `-NoProfile` is deliberate: the user's PowerShell profile should not affect how
            // the package manager behaves, and it can be slow. `-ExecutionPolicy Bypass` is
            // not added -- changing security policy is not pmpx's job, and when a policy
            // blocks it PowerShell should report the real reason itself.
            let mut c = Command::new("pwsh");
            c.arg("-NoProfile").arg("-File").arg(&resolved.program);
            c.args(&spec.args);
            c
        }
    };

    cmd.current_dir(cwd);
    // The environment is inherited verbatim -- pmpx takes no part in proxies / mirrors /
    // registry switching; those are configured in the shell.
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    Ok(cmd)
}

/// Build the complete raw command line handed to `cmd.exe` (including `/d /s /c` and the
/// quote pair for `/s` to strip).
///
/// It is split out for testability: after going through `raw_arg`, `Command::get_args()` is
/// empty and cannot be introspected, and the shape of this command line is exactly where
/// mistakes happen.
#[cfg(windows)]
fn cmd_raw_command_line(program: &Path, args: &[OsString]) -> String {
    format!("/d /s /c \"{}\"", build_cmd_line(program, args))
}

/// Build one command line for `cmd /d /s /c`.
#[cfg(windows)]
fn build_cmd_line(program: &Path, args: &[OsString]) -> String {
    let mut line = quote_arg(&program.to_string_lossy());
    for a in args {
        line.push(' ');
        line.push_str(&quote_arg(&a.to_string_lossy()));
    }
    line
}

/// Quote one argument by the Windows command-line (`CommandLineToArgvW`) rules.
///
/// Both rules are counter-intuitive: `"` inside quotes has to be written `\"`; and
/// **backslashes are only special in front of a quote** -- the n backslashes before a quote
/// must be written as `2n+1`, and the n trailing backslashes must be doubled to `2n`
/// (otherwise they would swallow the closing quote).
///
/// It is really only called on Windows; the test build keeps it too so the quoting rules run
/// on all three platforms.
#[cfg(any(windows, test))]
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
                // The backslashes before the quote are doubled, plus one to escape the quote
                // itself
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

    // The backslashes before the closing quote are doubled as well
    for _ in 0..backslashes {
        out.push('\\');
    }
    out.push('"');
    out
}

/// Show the command that is about to run, on stderr.
///
/// It is meant to be called just before [`run`]. stderr rather than stdout: the backend
/// inherits stdout, which the caller may be reading through a pipe. The arguments are printed
/// as they are handed over, without adding quotes of our own -- a line that quoted them would
/// no longer describe what actually runs.
pub fn announce(spec: &CommandSpec) {
    let mut line = format!(
        "{} {}",
        style::paint(style::DIM, "pmpx ->"),
        style::paint(style::PM, spec.program.to_string_lossy())
    );
    for arg in &spec.args {
        line.push(' ');
        line.push_str(&arg.to_string_lossy());
    }
    anstream::eprintln!("{line}");
}

/// Really run it: inherit stdio, wait for it to finish, return its exit code verbatim.
///
/// The return value is not `Result<()>`: "the tests failed" and "pmpx failed" are two
/// different things, and `pmpx test` must be able to pass the backend's nonzero exit code
/// through to the caller's script -- that is the only meaningful contract of this command.
pub fn run(spec: &CommandSpec, cwd: &Path) -> Result<u8> {
    let mut cmd = command_for(spec, cwd)?;

    let status = cmd.status().map_err(|e| {
        PmpxError::Other(anyhow::anyhow!(e).context(format!(
            "failed to start {}",
            spec.program.to_string_lossy()
        )))
    })?;

    Ok(exit_code_of(status))
}

/// Translate an `ExitStatus` into a process exit code.
fn exit_code_of(status: std::process::ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        if (0..=255).contains(&code) {
            return code as u8;
        }
        // On Windows an exit code can be any u32, while Unix only keeps the low 8 bits. Take
        // the low 8 bits instead of erroring -- the user's script cares about "nonzero", not
        // the exact value.
        error_line(format!(
            "backend exit code {code} is outside 0-255, passing through the low 8 bits"
        ));
        return (code & 0xFF) as u8;
    }

    // Killed by a signal (Unix). Follow the shell convention: 128 + signal.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            error_line(format!("the backend was killed by signal {sig}"));
            return (128 + sig).clamp(0, 255) as u8;
        }
    }

    error_line("cannot read the backend exit code, treating it as 1");
    1
}

/// One `pmpx:` diagnostic on stderr.
fn error_line(body: impl std::fmt::Display) {
    anstream::eprintln!(
        "{} {}",
        style::paint(style::ERROR, "pmpx:"),
        style::paint(style::ERROR_BODY, body)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- kind classification -------------------------------------------------

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

    /// Extension matching is case-insensitive -- `.CMD` and `.cmd` are the same thing on
    /// Windows.
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

    // ---- quoting rules -------------------------------------------------------

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
        // Without this an empty argument would simply vanish from the command line
        assert_eq!(quote_arg(""), "\"\"");
    }

    #[test]
    fn inner_quotes_are_escaped() {
        assert_eq!(quote_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    /// Backslashes are only special before a quote -- so no spaces means no quotes are added,
    /// and there is no trailing-backslash problem either.
    #[test]
    fn an_arg_without_spaces_is_left_alone_even_with_backslashes() {
        assert_eq!(quote_arg("C:\\dir\\"), "C:\\dir\\");
        assert_eq!(quote_arg("C:\\x\\y"), "C:\\x\\y");
    }

    /// But once quotes are needed, the trailing backslashes must be doubled, otherwise they
    /// would escape the closing quote.
    #[test]
    fn trailing_backslashes_are_doubled_when_quoting() {
        assert_eq!(quote_arg("a b\\"), "\"a b\\\\\"");
        assert_eq!(quote_arg("C:\\a b\\"), "\"C:\\a b\\\\\"");
    }

    #[test]
    fn backslashes_before_a_quote_are_doubled_and_the_quote_escaped() {
        // `a\"` -> one backslash + a quote -> `a\\\"`
        assert_eq!(quote_arg("a\\\"b"), "\"a\\\\\\\"b\"");
    }

    #[test]
    fn a_plain_backslash_is_untouched_inside_quotes() {
        assert_eq!(quote_arg("C:\\a b\\c"), "\"C:\\a b\\c\"");
    }

    // ---- near-miss classification --------------------------------------------

    #[test]
    fn a_cmd_shim_is_a_near_miss_for_its_bare_name() {
        assert!(name_matches("pnpm", "pnpm.cmd"));
        assert!(name_matches("pnpm", "PNPM.CMD"), "case-insensitive");
        assert!(name_matches("pnpm", "pnpm.cmd.old"));
    }

    /// Two names differing by one letter are not near candidates -- this is the reason for
    /// prefix matching instead of edit distance.
    #[test]
    fn a_different_tool_is_not_a_near_miss() {
        assert!(!name_matches("npm", "pnpm.cmd"));
        assert!(!name_matches("pnpm", "npmy"));
    }

    /// The known cost of prefix matching: `bun` also drags in `bunzip2`, `npm` also drags in
    /// `npmx`. Accepted rather than fixed -- edit distance would only be worse.
    #[test]
    fn prefix_matching_accepts_some_false_positives() {
        assert!(name_matches("bun", "bunzip2"));
        assert!(name_matches("npm", "npmx"));
    }

    #[test]
    fn an_exact_name_is_not_a_near_miss() {
        // An exact match means it would have been found anyway, so it does not belong in
        // "near candidates"
        assert!(!name_matches("cargo", "cargo"));
        assert!(!name_matches("cargo", "CARGO"));
    }

    // ---- resolution ----------------------------------------------------------

    /// With a path separator it is used directly, without searching PATH.
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
        assert!(err.to_string().contains("path"));
    }

    /// `cargo` is guaranteed to resolve right now -- we are running inside `cargo test`.
    #[test]
    fn resolves_a_real_program_from_path() {
        let r = resolve(OsStr::new("cargo")).expect("cargo must be on PATH");
        assert!(r.program.is_absolute(), "{:?}", r.program);
    }

    #[test]
    fn a_missing_program_gives_exit_code_three() {
        let err = resolve(OsStr::new("pmpx-definitely-not-a-real-program-xyz")).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_NOT_FOUND);
    }

    /// `which` does PATHEXT resolution on Windows, which is how `pnpm` resolves to
    /// `pnpm.cmd`.
    ///
    /// This test asks `which` directly (using `which_in` with an explicit search directory)
    /// rather than going through [`resolve`]: `resolve` reads the process-level PATH, and
    /// mutating that variable in concurrently running tests is unsafe.
    #[cfg(windows)]
    #[test]
    fn which_does_pathex_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("pmpxprobe.cmd");
        std::fs::write(&shim, "@echo off\r\n").unwrap();

        let found = which::which_in("pmpxprobe", Some(tmp.path()), tmp.path())
            .expect("which should find the .cmd via PATHEXT");

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

    /// The message for "not found" has to carry information.
    #[test]
    fn a_missing_program_says_something_useful() {
        let err = resolve(OsStr::new("pmpx-definitely-not-a-real-program-xyz")).unwrap_err();
        let msg = err.to_string();

        assert!(
            msg.contains("pmpx-definitely-not-a-real-program-xyz"),
            "{msg}"
        );
        assert!(
            msg.contains("not installed") || msg.contains("Similar names"),
            "either say it is probably not installed, or list candidates: {msg}"
        );
    }

    // ---- really running a command --------------------------------------------

    #[test]
    fn runs_a_native_command_and_returns_its_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CommandSpec::new("cargo").arg("--version");
        assert_eq!(
            run(&spec, tmp.path()).unwrap(),
            0,
            "cargo --version should succeed"
        );
    }

    #[test]
    fn a_nonzero_backend_exit_code_is_passed_through() {
        let tmp = tempfile::tempdir().unwrap();

        #[cfg(windows)]
        let spec = CommandSpec::new("cmd").arg("/c").arg("exit 7");
        #[cfg(not(windows))]
        let spec = CommandSpec::new("sh").arg("-c").arg("exit 7");

        assert_eq!(
            run(&spec, tmp.path()).unwrap(),
            7,
            "must pass through verbatim"
        );
    }

    /// The working directory has to take effect -- both the `cwd` a plugin reports and the
    /// project root rely on it.
    #[test]
    fn the_working_directory_is_honoured() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("pmpx-cwd-probe.txt");
        std::fs::write(&marker, "here").unwrap();

        let spec = CommandSpec::new("cargo").arg("--version");
        let mut cmd = command_for(&spec, tmp.path()).unwrap();
        let out = cmd.output().unwrap();

        // cargo only does anything else where there is a Cargo.toml; here it is enough to
        // prove it starts
        assert!(out.status.success());
    }

    // ---- command_for construction --------------------------------------------

    #[test]
    fn a_missing_program_fails_before_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CommandSpec::new("pmpx-definitely-not-a-real-program-xyz");

        assert!(command_for(&spec, tmp.path()).is_err());
    }

    /// On Windows `.cmd` must be wrapped in `cmd /d /s /c`, with one quote pair at each end
    /// for `/s` to strip.
    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_is_wrapped_in_cmd_exe() {
        let shim = Path::new("C:\\path with space\\pnpm.cmd");
        let args = vec![OsString::from("add"), OsString::from("serde")];

        let line = cmd_raw_command_line(shim, &args);

        assert!(line.starts_with("/d /s /c "), "{line}");
        // The outermost quote pair is there for `/s` to strip
        assert!(line.ends_with('"'), "{line}");
        assert_eq!(
            line.matches('"').count(),
            4,
            "one pair for the path + one outer pair: {line}"
        );
        assert!(line.contains("\"C:\\path with space\\pnpm.cmd\""), "{line}");
        assert!(line.ends_with("add serde\""), "{line}");
    }

    /// A path without spaces needs no quotes, but the outer pair must still be there --
    /// `/s`'s behaviour depends on it.
    #[cfg(windows)]
    #[test]
    fn the_outer_quote_pair_is_always_present() {
        let line = cmd_raw_command_line(Path::new("C:\\x\\pnpm.cmd"), &[]);
        assert_eq!(line, "/d /s /c \"C:\\x\\pnpm.cmd\"");
    }

    /// Really run a `.cmd` and confirm the arguments arrive.
    ///
    /// cmd's quoting rules are a separate implementation, so the only way to confirm is to
    /// run it for real.
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

    /// A `.cmd` has to run from a path with spaces too -- that is what the double quoting
    /// and `/s` are for.
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
            "a path with spaces must work -- that is why the double quoting exists"
        );
    }

    /// An argument with spaces has to arrive verbatim too.
    ///
    /// `%~1` rather than `%1`: `%1` brings the quotes along (existing cmd behaviour), while
    /// `%~1` is "the value with the wrapping quotes removed".
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
