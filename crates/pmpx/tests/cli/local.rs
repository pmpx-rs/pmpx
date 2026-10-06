//! Installing a plugin from its own directory: the development loop.
//!
//! `pmpx plugin add .` — or any path — builds and installs a checkout that was never published, and
//! what lands is indistinguishable from a published install: the same two files, the same loading,
//! the same detection. These tests really compile the fixture and really run the binary.

use crate::support::{stderr_of, stdout_of, Sandbox};

/// The fixture checkout, copied into the sandbox.
///
/// Copied rather than used in place: a test must not be able to write build output anywhere near the
/// repository.
fn checkout(sb: &Sandbox) -> std::path::PathBuf {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("local-plugin");
    let dst = sb.project.join("local-plugin");
    copy_tree(&src, &dst);

    // The fixture reaches the contract by the repo-relative path that works where it sits in the
    // repository; once copied, that path points somewhere else, so it is rewritten to the checkout
    // this test is running from.
    let contract = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pmpx-plugin");
    // `absolute`, not `canonicalize`: the latter returns a Windows verbatim path (`\\?\…`), which
    // cargo will not accept in a manifest. And forward slashes, because the fixture writes the
    // dependency in a TOML *basic* string, where a backslash is an escape.
    let contract = std::path::absolute(&contract).unwrap();
    let contract = contract.to_str().unwrap().replace('\\', "/");
    let manifest = std::fs::read_to_string(dst.join("Cargo.toml")).unwrap();
    std::fs::write(
        dst.join("Cargo.toml"),
        manifest.replace("../../../../pmpx-plugin", &contract),
    )
    .unwrap();

    dst
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// The whole loop, once: install from a directory, see it listed, and have it answer a real verb in a
/// real project — with detection picking it because of the markers in its own manifest.
#[test]
fn a_checkout_installs_and_then_answers() {
    let sb = Sandbox::new();
    let dir = checkout(&sb);

    let out = sb.run(&["plugin", "add", dir.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("pmpx-plugin-local") && stdout.contains("built from"),
        "the install should say where it came from: {stdout}"
    );

    // Listed like any other plugin.
    let listed = sb.ok(&["plugin", "ls"]);
    assert!(listed.contains("local"), "{listed}");

    // And it answers: the fixture project has the marker the checkout declares, so real detection
    // selects it, and its own answer comes back through the boundary.
    sb.file("local.lock");
    sb.file("local.json");
    std::fs::write(sb.project.join("local.json"), "{\"declared\":true}").unwrap();
    let out = sb.run(&["build"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("pmpx-local-probe") && stdout.contains("verb=build"),
        "{stdout}"
    );
    // Not the exact JSON: the probe travels through `cmd /c echo` on Windows, which escapes the
    // quotes on the way out. What matters is that the declared file was handed over at all.
    assert!(
        stdout.contains("declared={") && !stdout.contains("declared=none"),
        "a file the manifest declared reaches the plugin: {stdout}"
    );
}

/// A path that is not a checkout says so, rather than asking crates.io for a crate called `./x`.
#[test]
fn a_path_that_is_not_a_checkout_is_refused_with_a_reason() {
    let sb = Sandbox::new();
    std::fs::create_dir_all(sb.project.join("not-a-plugin")).unwrap();

    let out = sb.run(&["plugin", "add", "./not-a-plugin"]);

    assert_eq!(out.status.code(), Some(2), "{}", stderr_of(&out));
    let err = stderr_of(&out);
    assert!(err.contains("not a plugin checkout"), "{err}");
    assert!(
        !err.contains("registry"),
        "it must not have asked the registry: {err}"
    );
}

/// A checkout built against another contract version is refused before anything is compiled: the host
/// would refuse to load it anyway, and saying so here is cheaper.
#[test]
fn a_checkout_for_another_abi_is_refused() {
    let sb = Sandbox::new();
    let dir = sb.project.join("old-plugin");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("pmpx-plugin.toml"),
        "[plugin]\nname = \"old\"\nversion = \"0.1.0\"\nabi = 2\nfamily = \"node\"\n\n[detect]\nstrong = [\"old.lock\"]\n",
    )
    .unwrap();

    let out = sb.run(&["plugin", "add", "./old-plugin"]);

    assert_eq!(out.status.code(), Some(2), "{}", stderr_of(&out));
    let err = stderr_of(&out);
    assert!(err.contains("declares ABI 2"), "{err}");
    assert!(err.contains("ABI 3"), "{err}");
}

/// A checkout that would silently never run is installed, but the reason is said out loud.
#[test]
fn a_checkout_with_nothing_to_detect_is_installed_with_a_warning() {
    let sb = Sandbox::new();
    let dir = sb.project.join("quiet-plugin");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"pmpx-plugin-quiet\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("pmpx-plugin.toml"),
        "[plugin]\nname = \"quiet\"\nversion = \"0.1.0\"\nabi = 3\nfamily = \"node\"\n",
    )
    .unwrap();

    let out = sb.run(&["plugin", "add", "./quiet-plugin"]);

    // Refused only if the build fails; here the point is the warning, which comes before the build.
    let err = stderr_of(&out);
    assert!(
        err.contains("no [detect] markers"),
        "the silent failure has to be said out loud: {err}"
    );
}
