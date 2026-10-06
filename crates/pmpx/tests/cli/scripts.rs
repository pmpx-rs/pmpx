//! `[scripts]`: the project's own named commands.
//!
//! `pmpx run <name>` looks the name up in the project's merged `.pmpx.toml` first and runs it directly,
//! so a project can have a `pmpx fmt` without a plugin, and without pmpx knowing what "fmt" means. A
//! name that is not in the table keeps going to the plugin, which is the second half of the promise.

use std::process::Command;

use crate::support::{sandbox_with_plugin, stderr_of, stdout_of, Sandbox};

/// Write a `.pmpx.toml` into the sandbox.
fn write_config(sb: &Sandbox, name: &str, body: &str) {
    std::fs::write(sb.project.join(name), body).unwrap();
}

/// A script that exists on both platforms: `echo` is a cmd builtin on Windows, so the array form names
/// the interpreter, and the string form is only used where a bare `echo` resolves.
#[cfg(windows)]
const SAY_HI: &str = r#"hi = ["cmd", "/c", "echo", "hi-from-script"]"#;
#[cfg(not(windows))]
const SAY_HI: &str = r#"hi = "echo hi-from-script""#;

/// A string entry with quotes, which is what the splitter has to get right.
#[cfg(windows)]
const QUOTED: &str = r#"say = "cmd /c echo \"a b\"""#;
#[cfg(not(windows))]
const QUOTED: &str = r#"say = "echo \"a b\"""#;

/// The whole point: a named command is a convenience for the person, so it needs no plugin at all.
#[test]
fn a_named_command_runs_with_no_plugin_installed() {
    let sb = Sandbox::new();
    sb.file("fakepm.lock");
    write_config(&sb, ".pmpx.toml", &format!("[scripts]\n{SAY_HI}\n"));

    let out = sb.run(&["run", "hi"]);

    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(
        stdout_of(&out).contains("hi-from-script"),
        "the script has to have run: {}",
        stdout_of(&out)
    );
}

/// Anything the user typed after `--` is appended to the named command -- one mechanism, not two.
#[test]
fn the_arguments_after_the_separator_are_appended() {
    let sb = Sandbox::new();
    sb.file("fakepm.lock");
    write_config(&sb, ".pmpx.toml", &format!("[scripts]\n{SAY_HI}\n"));

    let out = sb.run(&["run", "hi", "--", "extra"]);

    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(
        stdout_of(&out).contains("hi-from-script extra"),
        "the extra argument has to reach it: {}",
        stdout_of(&out)
    );
}

/// A string entry is a command line the way a person writes one: quotes group.
#[test]
fn a_string_entry_is_split_with_quotes_respected() {
    let sb = Sandbox::new();
    sb.file("fakepm.lock");
    write_config(&sb, ".pmpx.toml", &format!("[scripts]\n{QUOTED}\n"));

    let out = sb.run(&["run", "say"]);

    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(
        stdout_of(&out).contains("a b"),
        "the quoted argument has to arrive as one: {}",
        stdout_of(&out)
    );
}

/// A name the project does not define keeps going to the plugin: `[scripts]` adds a route, it does not
/// take one away.
#[test]
fn a_name_that_is_not_in_the_table_still_reaches_the_plugin() {
    let (sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");
    write_config(&sb, ".pmpx.toml", "[scripts]\nother = \"echo not-this\"\n");

    let out = sb.run(&["run", "build"]);

    assert!(out.status.success(), "{}", stderr_of(&out));
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("pmpx-probe") && stdout.contains("verb=run"),
        "the plugin is asked, with the name as its argument: {stdout}"
    );
    assert!(
        stdout.contains("args=build"),
        "and the name travels with it: {stdout}"
    );
}

/// The nearest layer wins, exactly as it does for pins: that is the merge rule of the whole file.
#[test]
fn the_nearest_layer_defines_the_command() {
    let sb = Sandbox::new();
    sb.file("fakepm.lock");
    // The sandbox root has no `.pmpx.toml` of its own beyond the one written here, so the nearest
    // layer is this one; a nested directory with its own file must win over it.
    write_config(&sb, ".pmpx.toml", "[scripts]\nhi = \"echo from-far\"\n");
    std::fs::create_dir_all(sb.project.join("inner")).unwrap();
    std::fs::write(
        sb.project.join("inner/.pmpx.toml"),
        "[scripts]\nhi = \"echo from-near\"\n",
    )
    .unwrap();

    let inner = sb.project.join("inner");
    let out = Command::new(env!("CARGO_BIN_EXE_pmpx"))
        .args(["-C"])
        .arg(&inner)
        .args(["run", "hi"])
        .env("PMPX_CONFIG_DIR", &sb.config_dir)
        .env("PMPX_DATA_DIR", &sb.data_dir)
        .output()
        .expect("should be able to start pmpx");

    assert!(out.status.success(), "{}", stderr_of(&out));
    let stdout = stdout_of(&out);
    assert!(stdout.contains("from-near"), "{stdout}");
    assert!(!stdout.contains("from-far"), "{stdout}");
}
