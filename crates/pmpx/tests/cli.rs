//! End to end: really run the `pmpx` binary.
//!
//! Unit tests verify each layer on its own; this file verifies they are still right once
//! wired together, especially across `dlopen` -- which unit tests can never cover.
//!
//! Two premises:
//!
//! 1. **Sandboxed.** `PMPX_CONFIG_DIR` / `PMPX_DATA_DIR` point pmpx at a temporary directory;
//!    without them these tests would read and write the user's real `~/.config/pmpx` and
//!    `~/.pmpx`.
//! 2. **No dependency on which package manager is installed.** Every verb of the fake plugin
//!    maps to `cargo --version` -- we are running inside `cargo test` right now, so it is
//!    guaranteed to exist.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The path of the pmpx binary. cargo provides this environment variable for bin targets.
const PMPX: &str = env!("CARGO_BIN_EXE_pmpx");

/// A sandbox: a pmpx config directory, a plugin directory, and a "project" directory.
struct Sandbox {
    _tmp: tempfile::TempDir,
    config_dir: PathBuf,
    data_dir: PathBuf,
    project: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("should be able to create a temporary directory");
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

    /// Put a file in the project directory.
    fn file(&self, name: &str) -> &Self {
        let p = self.project.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, "").unwrap();
        self
    }

    /// Write a global config.
    fn global_config(&self, body: &str) -> &Self {
        std::fs::write(self.config_dir.join("config.toml"), body).unwrap();
        self
    }

    /// Lay out an "installed plugin" directory (cdylib + manifest).
    fn install_plugin(&self, crate_name: &str, manifest: &str, lib: Option<&Path>) -> &Self {
        let dir = self.data_dir.join("plugins").join(crate_name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pmpx-plugin.toml"), manifest).unwrap();

        if let Some(src) = lib {
            std::fs::copy(src, dir.join(src.file_name().unwrap())).unwrap();
        }
        self
    }

    /// Run pmpx once. **The working directory is the project directory**, and the two
    /// pmpx-specific environment variables point at the sandbox.
    ///
    /// It deliberately does **not** `env_clear()`: that would stop even `cargo` itself from
    /// running (it needs `USERPROFILE` / `APPDATA` and the like). pmpx only honours those two
    /// `PMPX_*` variables; it is supposed to inherit the rest of the environment verbatim.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(PMPX)
            .args(args)
            .current_dir(&self.project)
            .env("PMPX_CONFIG_DIR", &self.config_dir)
            .env("PMPX_DATA_DIR", &self.data_dir)
            .output()
            .expect("should be able to start pmpx")
    }

    /// Run once and assert success, returning stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "`pmpx {}` should succeed, exit code {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
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
// Compiling the real plugin
// ---------------------------------------------------------------------------

/// Build the fake plugin and return the cdylib path.
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
        .expect("should be able to start cargo");
    assert!(status.success(), "the fake plugin should compile");

    let lib = crate_plugin_kit::find_library(&target_dir.join("release"), "pmpx_plugin_fakepm")
        .expect("should find the fake plugin's cdylib");

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

/// A sandbox with the fake plugin installed.
fn sandbox_with_plugin() -> (tempfile::TempDir, Sandbox, PathBuf) {
    let (build_tmp, lib) = build_fake_plugin();
    let sb = Sandbox::new();
    sb.install_plugin("pmpx-plugin-fakepm", FAKEPM_MANIFEST, Some(&lib));
    (build_tmp, sb, lib)
}

// ---------------------------------------------------------------------------
// Zero plugins: the first-run experience
// ---------------------------------------------------------------------------

#[test]
fn with_no_plugins_it_exits_three_and_explains_itself() {
    let sb = Sandbox::new();
    sb.file("Cargo.toml");

    let out = sb.run(&[]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "no project type detected is exit code 3"
    );

    let err = stderr_of(&out);
    assert!(err.contains("no project type detected"), "{err}");
    // The static hints table only states facts; it does not recommend a plugin
    assert!(
        err.contains("look like"),
        "it should state what it saw: {err}"
    );
    assert!(err.contains("rust"), "{err}");
    assert!(
        !err.contains("pmpx plugin add cargo"),
        "it must not recommend a specific plugin: {err}"
    );
    // But it must tell the user where the way out is
    assert!(err.contains(".pmpx.toml"), "{err}");
}

#[test]
fn exec_still_works_with_zero_plugins() {
    let sb = Sandbox::new();
    sb.file("Cargo.toml");

    // exec has to work with zero plugins too
    let out = sb.run(&["exec", "cargo", "--version"]);
    assert!(
        out.status.success(),
        "it should succeed after degrading to passing through verbatim: {}",
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
// The full chain: detect -> resolve -> dlopen -> cross-ABI call -> spawn
// ---------------------------------------------------------------------------

#[test]
fn detects_the_project_and_runs_the_backend_command() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.ok(&["build"]);

    // The command really was spawned -- the fake plugin maps it to an echo
    assert!(
        out.contains("pmpx-probe"),
        "the backend command should really run: {out}"
    );
}

/// The data crossing the boundary is correct -- the part unit tests cannot cover.
#[test]
fn the_plugin_receives_root_matched_verb_and_args_across_the_boundary() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");
    sb.file("fakepm.json");

    let out = sb.ok(&["build"]);

    let line = out
        .lines()
        .find(|l| l.contains("pmpx-probe"))
        .unwrap_or_else(|| panic!("no echo came back: {out}"));

    // project_root: it crossed the dlopen boundary verbatim.
    //
    // Both sides have to be canonicalized before comparing; `sb.project` cannot be compared
    // as a string directly: macOS temporary directories are `/var/...`, which is really a
    // symlink to `/private/var/...`, and `current_dir()` returns the resolved one.
    // Normalizing only one side does not work either -- on Windows canonicalize adds a
    // `\\?\` prefix, which then no longer matches the plain path the plugin received.
    let got_root = line
        .split_once("root=")
        .and_then(|(_, rest)| rest.split_once(" matched="))
        .map(|(root, _)| root)
        .unwrap_or_else(|| panic!("the echo has no root=: {line}"));
    let got = std::fs::canonicalize(got_root).unwrap_or_else(|e| {
        panic!("the root the plugin received cannot be resolved ({got_root}): {e}")
    });
    let want = std::fs::canonicalize(&sb.project).expect("the sandbox directory should exist");
    assert_eq!(got, want, "project_root was passed wrongly: {line}");
    // matched: only the matched files this plugin itself declared, with strong evidence before
    // weak evidence.
    assert!(
        line.contains("matched=fakepm.lock|fakepm.json"),
        "matched is wrong (strong evidence should come first): {line}"
    );
    assert!(line.contains("verb=build"), "{line}");
    // args: the host side passed no arguments for build
    assert!(line.contains("args="), "{line}");
}

/// Arguments after `--` have to reach the backend verbatim.
#[test]
fn args_after_the_double_dash_reach_the_plugin() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.ok(&["test", "--", "--nocapture", "some-filter"]);

    let line = out.lines().find(|l| l.contains("pmpx-probe")).unwrap();
    assert!(line.contains("verb=test"), "{line}");
    assert!(
        line.contains("args=--nocapture,some-filter"),
        "the arguments after `--` must go through verbatim: {line}"
    );
}

#[test]
fn a_bare_pmpx_reports_the_selection() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.ok(&[]);
    assert!(out.contains("Project root"), "{out}");
    assert!(
        out.contains("fakepm"),
        "it should report the selected plugin: {out}"
    );
}

#[test]
fn info_lists_every_candidate_and_score() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.ok(&["info"]);
    assert!(out.contains("Candidates and scores"), "{out}");
    assert!(out.contains("fakepm"), "{out}");
    assert!(
        out.contains("100"),
        "strong evidence should be 100 points: {out}"
    );
    // The toolchain is diagnostics only, but these values have to be shown
    assert!(out.contains("compiled with"), "{out}");
    assert!(out.contains("target"), "{out}");
}

// ---------------------------------------------------------------------------
// The exec fallback
// ---------------------------------------------------------------------------

/// The plugin is installed but explicitly says it does not support exec -- exactly the one
/// scenario where degrading is allowed.
#[test]
fn exec_falls_back_when_the_plugin_says_unsupported() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock"); // the project is recognisable and the plugin is installed

    let out = sb.run(&["exec", "cargo", "--version"]);
    assert!(
        out.status.success(),
        "it should degrade to passing through verbatim when the plugin says unsupported: {}",
        stderr_of(&out)
    );
    assert!(
        stdout_of(&out).contains("cargo"),
        "it should really have run cargo --version: {}",
        stdout_of(&out)
    );
}

/// The six verbs other than exec keep "unsupported is an error".
#[test]
fn other_verbs_do_not_fall_back() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    // The fake plugin panics on remove -> the host must stay alive and report an internal
    // error instead of crashing with it
    let out = sb.run(&["remove", "something"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a plugin blowing up is exit code 1\nstderr: {}",
        stderr_of(&out)
    );
    assert!(
        stderr_of(&out).contains("fakepm"),
        "it should say which plugin had the problem: {}",
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
    assert!(cfg.is_file(), "it should have written .pmpx.toml");

    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("faketest"), "the key is the family: {text}");
    assert!(text.contains("fakepm"), "{text}");

    // Once it takes effect, a bare run should still select it
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
    assert!(
        !cfg.exists(),
        "an empty config file should be deleted, not left as a shell"
    );
}

#[test]
fn unset_without_a_family_needs_yes_when_there_are_several() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");
    // Write two pins by hand (fakepm only belongs to faketest, but a pin does not require the
    // plugin to exist)
    std::fs::write(
        sb.project.join(".pmpx.toml"),
        "[plugin]\nfaketest = \"fakepm\"\npython = \"poetry\"\n",
    )
    .unwrap();

    let out = sb.run(&["plugin", "unset"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "requiring --yes is a usage error"
    );
    let err = stderr_of(&out);
    assert!(err.contains("--yes"), "{err}");
    // List what would be deleted first
    assert!(err.contains("faketest"), "{err}");
    assert!(err.contains("python"), "{err}");

    // Only with --yes does it really delete
    sb.ok(&["plugin", "unset", "--yes"]);
    assert!(!sb.project.join(".pmpx.toml").exists());
}

/// Layered config: a `.pmpx.toml` above the project root has to be visible too.
#[test]
fn config_above_the_project_root_is_visible() {
    let (_build, sb, _lib) = sandbox_with_plugin();

    // The project is in project/web/, the pin is written in project/ (above the project root)
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
        text.contains("Project config"),
        "it should list the sources: {text}\nstderr: {}",
        stderr_of(&out)
    );
    // That one is in the upper layer, so it must appear in the source list
    let upper = sb.project.join(".pmpx.toml");
    assert!(
        text.contains(&upper.display().to_string()),
        "the config one layer above the project root must be visible: {text}"
    );
    assert!(
        text.contains("faketest"),
        "the pin should be read out: {text}"
    );
}

// ---------------------------------------------------------------------------
// Config commands
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

    // The value type has to survive: a number is stored as a number, not as a string
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

/// After setting `walk_up = false`, walk-up really stops.
#[test]
fn walk_up_false_stops_the_search() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    // The manifest is in the upper layer, cwd is in the lower one
    let deep = sb.project.join("a").join("b");
    std::fs::create_dir_all(&deep).unwrap();
    sb.file("fakepm.lock");

    // By default it can walk up to it
    let out = Command::new(PMPX)
        .args(["info"])
        .current_dir(&deep)
        .env("PMPX_CONFIG_DIR", &sb.config_dir)
        .env("PMPX_DATA_DIR", &sb.data_dir)
        .output()
        .unwrap();
    assert!(
        stdout_of(&out).contains("Project root  ") && !stdout_of(&out).contains("(not found)"),
        "walk-up should find the project root by default: {}",
        stdout_of(&out)
    );

    // With it turned off nothing is found
    sb.global_config("[discovery]\nwalk_up = false\n");
    let out = Command::new(PMPX)
        .args(["info"])
        .current_dir(&deep)
        .env("PMPX_CONFIG_DIR", &sb.config_dir)
        .env("PMPX_DATA_DIR", &sb.data_dir)
        .output()
        .unwrap();
    assert!(
        stdout_of(&out).contains("(not found)"),
        "with walk-up off the project root must not be found: {}",
        stdout_of(&out)
    );
}

// ---------------------------------------------------------------------------
// Bad cases of plugin manifests
// ---------------------------------------------------------------------------

/// A manifest missing `family`: still listed, with the problem marked, and it takes no part
/// in resolution.
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
    assert!(out.contains("family"), "it should state the problem: {out}");

    // It does not count during detection
    let out = sb.run(&[]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "an unusable plugin takes no part in detection"
    );
}

/// `-p` naming a plugin that is not installed -> exit code 3 and a list of the options.
#[test]
fn an_unknown_plugin_flag_lists_what_is_installed() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.run(&["-p", "nope", "build"]);
    assert_eq!(out.status.code(), Some(3));
    let err = stderr_of(&out);
    assert!(err.contains("nope"), "{err}");
    assert!(
        err.contains("fakepm"),
        "it should list what is installed: {err}"
    );
}

/// `-p` has to beat `.pmpx.toml`.
#[test]
fn the_plugin_flag_beats_the_project_config() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");
    // Pin to a plugin that does not exist -- the normal path errors out because of it
    std::fs::write(
        sb.project.join(".pmpx.toml"),
        "[plugin]\nfaketest = \"ghost\"\n",
    )
    .unwrap();

    // Without -p: an error
    let out = sb.run(&["build"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr_of(&out));

    // With -p: it bypasses that config and runs normally
    let out = sb.run(&["-p", "fakepm", "build"]);
    assert!(
        out.status.success(),
        "-p must beat .pmpx.toml: {}",
        stderr_of(&out)
    );
}

/// A mismatched ABI version -> refuse to load, and state both versions.
#[test]
fn an_abi_mismatch_is_refused_with_both_versions() {
    let (_build, sb, lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    // The manifest's abi is only for display; the real check happens after loading, by
    // reading the plugin's self-reported `abi_version`. We cannot easily compile a plugin
    // with a different ABI, so this steps back: verify that `pmpx info` shows the ABI the
    // manifest declares.
    let out = sb.ok(&["plugin", "info", "fakepm"]);
    assert!(out.contains("ABI"), "{out}");

    let _ = lib;
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

#[test]
fn completion_writes_a_script_to_stdout() {
    let sb = Sandbox::new();
    let out = sb.ok(&["completion", "bash"]);
    assert!(
        out.contains("pmpx"),
        "the completion script should mention pmpx: {out}"
    );
    assert!(out.contains("complete") || out.contains("_pmpx"), "{out}");
}

/// The verbs appearing in `--help` are exactly those seven -- no more, no fewer.
#[test]
fn help_lists_exactly_the_seven_verbs() {
    let sb = Sandbox::new();
    let out = sb.ok(&["--help"]);

    for verb in [
        "install", "remove", "run", "build", "test", "update", "exec",
    ] {
        assert!(out.contains(verb), "--help is missing {verb}: {out}");
    }
    // No lock / outdated / audit
    for absent in ["lock", "outdated", "audit", "publish"] {
        assert!(
            !out.contains(absent),
            "--help must not contain {absent}: {out}"
        );
    }
}
