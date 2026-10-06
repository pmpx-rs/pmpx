//! The project config: layering above the project root, and the `config get / set` commands.

use std::process::Command;

use crate::support::{sandbox_with_plugin, stderr_of, stdout_of, Sandbox, PMPX};

/// Layered config: a `.pmpx.toml` above the project root has to be visible too.
#[test]
fn config_above_the_project_root_is_visible() {
    let (sb, _lib) = sandbox_with_plugin();

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
    let (sb, _lib) = sandbox_with_plugin();
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
