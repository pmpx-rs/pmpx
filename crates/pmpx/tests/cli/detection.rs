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
    let (sb, _lib) = sandbox_with_plugin();
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
    let (sb, _lib) = sandbox_with_plugin();
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

    // The invocation directory: the plugin is run from inside the project here, so it is the project
    // directory itself -- while the field exists precisely for the case where it is not.
    let got_start = line
        .split_once("start=")
        .and_then(|(_, rest)| rest.split_once(" reason="))
        .map(|(start, _)| start)
        .unwrap_or_else(|| panic!("the echo has no start=: {line}"));
    let got = std::fs::canonicalize(got_start)
        .unwrap_or_else(|e| panic!("start_dir cannot be resolved ({got_start}): {e}"));
    assert_eq!(got, want, "start_dir was passed wrongly: {line}");

    // Why this plugin is the one being asked, and with what evidence: nobody pinned it and `-p`
    // did not name it, so it won on its score -- 100 for the lockfile plus 10 for the weak
    // `fakepm.json` this test also created.
    assert!(line.contains("reason=scored"), "{line}");
    assert!(line.contains("score=110"), "{line}");
}

/// A `.pmpx.toml` pin is reported to the plugin as the reason it was selected -- the plugin cannot
/// work that out for itself, and it is also how someone holds a plugin back on purpose.
#[test]
fn a_pinned_plugin_is_told_it_was_pinned() {
    let (sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");
    sb.file(".pmpx.toml");
    std::fs::write(
        sb.project.join(".pmpx.toml"),
        "[plugin]\nfaketest = \"fakepm\"\n\n[scripts]\nbuild = \"fakepm run build\"\n",
    )
    .unwrap();

    let out = sb.run(&["--debug", "build"]);
    assert!(out.status.success(), "{}", stderr_of(&out));

    let stdout = stdout_of(&out);
    let line = stdout.lines().find(|l| l.contains("pmpx-probe")).unwrap();
    assert!(line.contains("reason=pinned"), "{line}");
    // A pin still carries the plugin's own evidence score: it is the pin that decided, not the
    // score, but the plugin was never asked to pretend otherwise.
    assert!(line.contains("score=100"), "{line}");

    // And the whole context the fixture plugin logged: the pin, the script and the config file that
    // was read all crossed the boundary, not just the reason.
    let err = stderr_of(&out);
    let context_line = err
        .lines()
        .find(|l| l.contains("[fakepm] context:"))
        .unwrap_or_else(|| panic!("the plugin logged no context: {err}"));
    assert!(context_line.contains("reason=pinned"), "{context_line}");
    assert!(context_line.contains("faketest=fakepm"), "{context_line}");
    assert!(context_line.contains("scripts=[build]"), "{context_line}");
    assert!(context_line.contains(".pmpx.toml"), "{context_line}");
}

/// `-p` overrides everything, including a pin, and the plugin is told that too.
#[test]
fn a_plugin_named_with_dash_p_is_told_it_was_explicit() {
    let (sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.ok(&["-p", "fakepm", "build"]);
    let line = out.lines().find(|l| l.contains("pmpx-probe")).unwrap();

    assert!(line.contains("reason=explicit"), "{line}");
    // Nothing had to be weighed, so there is no score to report.
    assert!(line.contains("score=0"), "{line}");
}

/// Arguments after `--` have to reach the backend verbatim.
#[test]
fn args_after_the_double_dash_reach_the_plugin() {
    let (sb, _lib) = sandbox_with_plugin();
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
    let (sb, _lib) = sandbox_with_plugin();
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
    let (sb, _lib) = sandbox_with_plugin();
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
    let (sb, _lib) = sandbox_with_plugin();
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
    let (sb, _lib) = sandbox_with_plugin();
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

/// A plugin's own `debug!` arrives through the host: the host adds the plugin's id, and the line
/// only appears when a trace was asked for -- which is what keeps `--debug` from being an input
/// the plugin could branch on, and keeps a plugin's notes out of a normal run entirely.
#[test]
fn a_plugins_own_log_line_carries_its_id() {
    let (sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let traced = sb.run(&["--debug", "build"]);
    assert!(traced.status.success(), "{}", stderr_of(&traced));
    let err = stderr_of(&traced);
    assert!(
        err.contains("[fakepm] mapping build"),
        "the plugin's line should carry its id: {err}"
    );

    // The same run without the trace: the plugin called `debug!` just the same, and nothing was
    // printed -- not even a bare message without its id.
    let plain = sb.run(&["build"]);
    assert!(plain.status.success(), "{}", stderr_of(&plain));
    let err = stderr_of(&plain);
    assert!(
        !err.contains("mapping build"),
        "a plugin's notes need --debug: {err}"
    );
}

// ---------------------------------------------------------------------------
// The exec fallback
// ---------------------------------------------------------------------------

/// The plugin is installed but explicitly says it does not support exec -- exactly the one
/// scenario where degrading is allowed.
#[test]
fn exec_falls_back_when_the_plugin_says_unsupported() {
    let (sb, _lib) = sandbox_with_plugin();
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
    let (sb, _lib) = sandbox_with_plugin();
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
