//! Detection: the first-run experience with no plugins, and the full chain once one is
//! installed -- detect -> resolve -> dlopen -> cross-ABI call -> spawn.

use crate::support::{sandbox_with_plugin, stderr_of, stdout_of, Sandbox};

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
// `--debug`
// ---------------------------------------------------------------------------

/// The trace has to cover a whole run, phase by phase, and never touch stdout: pmpx's answer
/// to "it feels slow" is only useful if it says which side the time went to.
#[test]
fn debug_traces_every_phase_on_stderr() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.run(&["--debug", "build"]);
    assert!(out.status.success(), "{}", stderr_of(&out));

    let err = stderr_of(&out);
    for phase in [
        "pmpx debug:",
        "cli.parse",
        "session.root",
        "detect.score",
        "detect.decide",
        "backend.load",
        "spawn.resolve",
        "backend.run",
        "total",
    ] {
        assert!(err.contains(phase), "the trace is missing {phase}:\n{err}");
    }

    // The backend inherits stdout, so the trace must not be there.
    let stdout = stdout_of(&out);
    assert!(!stdout.contains("pmpx debug:"), "{stdout}");
}

/// `--quiet` turns off the hints, not the trace: asking for a trace is the more explicit
/// request of the two.
#[test]
fn quiet_does_not_turn_the_trace_off() {
    let (_build, sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.run(&["--quiet", "--debug", "build"]);
    assert!(out.status.success(), "{}", stderr_of(&out));

    let err = stderr_of(&out);
    assert!(err.contains("pmpx debug:"), "{err}");
    assert!(
        !err.contains("pmpx ->"),
        "`--quiet` should still have suppressed the resolved command: {err}"
    );
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
