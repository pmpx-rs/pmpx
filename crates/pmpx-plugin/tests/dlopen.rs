//! `export!`, through a real `dlopen`.
//!
//! The in-process tests drive the generated shell directly; this one proves the same shell really
//! becomes a loadable library with the symbol the ABI names -- and that the host's own loader can pick
//! it up. The fixture (`tests/fixtures/toy-plugin`) is a plugin like any other, built by cargo.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use pmpx_loader::{CallError, ContextSource, Files, Plugin};
use pmpx_plugin::Verb;

fn workspace_target_dir() -> PathBuf {
    // `<target>/<profile>/deps/<test binary>` -> three levels up.
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .and_then(|deps| deps.parent().map(Path::to_path_buf))
        .and_then(|profile| profile.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

fn find_cdylib(dir: &Path, stem: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    let candidate = dir.join(format!("{stem}.dll"));
    #[cfg(target_os = "macos")]
    let candidate = dir.join(format!("lib{stem}.dylib"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let candidate = dir.join(format!("lib{stem}.so"));

    assert!(
        candidate.is_file(),
        "the fixture's cdylib should be at {}",
        candidate.display()
    );
    candidate
}

/// Build the fixture once per test binary and load it.
///
/// The path is cached, the handle is not: a `Plugin` owns a `Library`, which belongs to the thread
/// that opened it, so every test opens its own.
fn plugin() -> Plugin {
    // SAFETY: the fixture is built from this repository's own sources.
    unsafe { Plugin::open(library()) }.expect("the exported plugin should load")
}

/// Build the fixture once per test binary.
fn library() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();

    BUILT
        .get_or_init(|| {
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("toy-plugin")
                .join("Cargo.toml");
            let target_dir = workspace_target_dir().join("fixture-toy-plugin");

            let status = Command::new("cargo")
                .args(["build", "--release", "--manifest-path"])
                .arg(&manifest)
                .arg("--target-dir")
                .arg(&target_dir)
                .status()
                .expect("should be able to start cargo");
            assert!(status.success(), "the fixture should compile");

            find_cdylib(&target_dir.join("release"), "pmpx_plugin_toy")
        })
        .as_path()
}

/// A provider that answers one file, so `[context] files` can be exercised.
struct One(&'static str, &'static [u8]);

impl Files for One {
    fn contents(&self, name: &str) -> Option<Vec<u8>> {
        (name == self.0).then(|| self.1.to_vec())
    }
}

fn call(verb: Verb, _matched: &[&str], _args: &[&str], files: &dyn Files) -> Result<(), CallError> {
    let root = PathBuf::from("/work/project");
    let matched: Vec<String> = _matched.iter().map(|s| s.to_string()).collect();
    let args: Vec<OsString> = _args.iter().map(OsString::from).collect();
    let pins = BTreeMap::new();

    let source = ContextSource {
        root: &root,
        start_dir: &root,
        matched: &matched,
        config_files: &[],
        pins: &pins,
        args: &args,
        verb: verb.to_abi(),
        reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
        score: 100,
        files,
    };

    plugin().tables().call(&source).map(|_| ())
}

/// The library loads through the entry symbol, and the identity the host reads is the plugin's own.
#[test]
fn a_plugin_built_with_export_loads() {
    assert_eq!(plugin().name(), "toy");
    assert_eq!(plugin().family(), "node");

    let (rustc, target) = plugin().tables().build_info();
    assert!(!rustc.is_empty(), "the shell fills in the rustc version");
    assert!(!target.is_empty(), "and the target");
}

/// The context crosses a real library boundary: the matched file is what `has_matched` sees inside the
/// plugin, and it shows up in the command it answers with.
#[test]
fn the_context_crosses_a_real_dlopen() {
    let root = PathBuf::from("/work/project");
    let matched = vec![".yarnrc.yml".to_string()];
    let args = vec![OsString::from("left-pad")];
    let pins = BTreeMap::new();

    let source = ContextSource {
        root: &root,
        start_dir: &root,
        matched: &matched,
        config_files: &[],
        pins: &pins,
        args: &args,
        verb: Verb::Install.to_abi(),
        reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
        score: 100,
        files: &pmpx_loader::NoFiles,
    };

    let command = plugin()
        .tables()
        .call(&source)
        .expect("install maps across the boundary");

    assert_eq!(command.program, OsString::from("toy-bin"));
    let args: Vec<String> = command
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "add");
    assert_eq!(args[1], "left-pad", "the user's arguments arrive");
    assert!(
        args.contains(&"--berry".to_string()),
        "`has_matched` has to see the matched file: {args:?}"
    );
}

/// A path the plugin puts in its answer crosses back out, unchanged.
#[test]
fn the_project_root_crosses_back_as_a_path() {
    let root = PathBuf::from("/work/project");
    let matched = Vec::new();
    let args = Vec::new();
    let pins = BTreeMap::new();

    let source = ContextSource {
        root: &root,
        start_dir: &root,
        matched: &matched,
        config_files: &[],
        pins: &pins,
        args: &args,
        verb: Verb::Run.to_abi(),
        reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
        score: 0,
        files: &One("package.json", b"{}"),
    };

    let command = plugin().tables().call(&source).expect("run maps");

    assert_eq!(command.args, vec![OsString::from("/work/project")]);
}

/// `unsupported` and a panic both stay inside the plugin and arrive as codes.
#[test]
fn an_unsupported_verb_and_a_panic_both_arrive_as_codes() {
    assert_eq!(
        call(Verb::Exec, &[], &[], &pmpx_loader::NoFiles),
        Err(CallError::UnsupportedVerb)
    );
    assert_eq!(
        call(Verb::Test, &[], &[], &pmpx_loader::NoFiles),
        Err(CallError::Internal),
        "a panic inside the plugin must not abort the host"
    );
}
