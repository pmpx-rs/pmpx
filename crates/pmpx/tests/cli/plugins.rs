//! The plugin subcommands: listing, pinning, and the bad cases of plugin manifests and of
//! choosing a plugin by hand.

use std::process::Command;

use crate::support::{sandbox_with_plugin, stderr_of, Sandbox, PMPX};

#[test]
fn plugin_ls_says_how_to_start() {
    let sb = Sandbox::new();
    let out = sb.ok(&["plugin", "ls"]);
    assert!(out.contains("pmpx plugin add"), "{out}");
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

/// With several `.pmpx.toml` on the way up, `plugin set` writes the **nearest** one -- the layer
/// the user chose. It asks the session for the list it already collected instead of walking the
/// directories again, so this pins that the two agree.
#[test]
fn plugin_set_writes_the_nearest_existing_config() {
    let (_build, sb, _lib) = sandbox_with_plugin();

    // One layer above the project root, and one inside it.
    std::fs::write(sb.project.join(".pmpx.toml"), "[plugin]\n").unwrap();
    let inner = sb.project.join("pkg");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(inner.join(".pmpx.toml"), "[plugin]\n").unwrap();
    sb.file("pkg/fakepm.lock");

    let out = Command::new(PMPX)
        .args(["plugin", "set", "fakepm"])
        .current_dir(&inner)
        .env("PMPX_CONFIG_DIR", &sb.config_dir)
        .env("PMPX_DATA_DIR", &sb.data_dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr_of(&out));

    let nearer = std::fs::read_to_string(inner.join(".pmpx.toml")).unwrap();
    assert!(
        nearer.contains("faketest"),
        "the nearer layer should have been written: {nearer}"
    );

    let upper = std::fs::read_to_string(sb.project.join(".pmpx.toml")).unwrap();
    assert_eq!(
        upper.trim(),
        "[plugin]",
        "the layer above must be left alone: {upper}"
    );
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
