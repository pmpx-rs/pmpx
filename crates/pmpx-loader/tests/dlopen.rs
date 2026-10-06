//! The loader against a real cdylib: `dlopen`, capability lookup, a call, and the ownership rules.
//!
//! The fixture (`tests/fixtures/raw-plugin`) is written straight against the raw ABI -- no
//! `pmpx-plugin`, no `export!` -- because that is what a plugin in C would look like, and the loader
//! is supposed to serve it too.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use pmpx_loader::{ContextSource, NoFiles, Plugin};

/// The ABI's entry symbol, spelled out here on purpose: the loader reads `PMPX_ENTRY_SYMBOL`, and a
/// *test* that used the same constant could not catch that constant being wrong.
const ENTRY_SYMBOL: &[u8] = b"pmpx_plugin_entry_v3\0";

/// The fixture's own extra symbol, which is not part of the ABI.
const ATTACHED_SYMBOL: &[u8] = b"pmpx_raw_plugin_attached\0";

fn workspace_target_dir() -> PathBuf {
    // `<target>/<profile>/deps/<test binary>` -> three levels up.
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .and_then(|deps| deps.parent().map(Path::to_path_buf))
        .and_then(|profile| profile.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

/// The cdylib one cargo build produced, named by the platform's rules.
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

/// Build the fixture once per test binary and return the library path.
fn fixture() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();

    BUILT
        .get_or_init(|| {
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("raw-plugin")
                .join("Cargo.toml");
            let target_dir = workspace_target_dir().join("fixture-raw-plugin");

            // Called `cargo` directly: this test is running inside cargo, so it is on PATH.
            let status = Command::new("cargo")
                .args(["build", "--release", "--manifest-path"])
                .arg(&manifest)
                .arg("--target-dir")
                .arg(&target_dir)
                .status()
                .expect("should be able to start cargo");
            assert!(status.success(), "the fixture should compile");

            find_cdylib(&target_dir.join("release"), "pmpx_plugin_raw")
        })
        .as_path()
}

/// What the fixture needs to be told about a call.
struct Fixture {
    root: PathBuf,
    matched: Vec<String>,
    config_files: Vec<PathBuf>,
    pins: BTreeMap<String, String>,
    args: Vec<OsString>,
}

impl Fixture {
    fn new(args: &[&str]) -> Self {
        Self {
            root: PathBuf::from("/work/project"),
            matched: Vec::new(),
            config_files: Vec::new(),
            pins: BTreeMap::new(),
            args: args.iter().map(OsString::from).collect(),
        }
    }

    fn source<'a>(&'a self, files: &'a dyn pmpx_loader::Files) -> ContextSource<'a> {
        ContextSource {
            root: &self.root,
            start_dir: &self.root,
            matched: &self.matched,
            config_files: &self.config_files,
            pins: &self.pins,
            args: &self.args,
            verb: pmpx_plugin_abi::PMPX_VERB_RUN,
            reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
            score: 42,
            files,
        }
    }
}

/// A plugin really loaded from a file, with the identity it reports.
#[test]
fn a_real_library_loads_and_reports_its_identity() {
    // SAFETY: the fixture is built from this repository's own sources.
    let plugin = unsafe { Plugin::open(fixture()) }.expect("the fixture should load");

    assert_eq!(plugin.name(), "raw");
    assert_eq!(plugin.family(), "rawfam");

    let (rustc, target) = plugin.tables().build_info();
    assert_eq!(rustc, "the fixture's rustc");
    assert_eq!(target, "the fixture's target");
}

/// A call crosses the boundary in both directions: the verb and the arguments reach the plugin, and
/// the command it answers with comes back as owned Rust values.
#[test]
fn a_call_crosses_in_both_directions() {
    // SAFETY: as above.
    let plugin = unsafe { Plugin::open(fixture()) }.expect("the fixture should load");
    let call = Fixture::new(&["left-pad", "--save-dev"]);

    let command = plugin
        .tables()
        .call(&call.source(&NoFiles))
        .expect("the fixture always answers ok");

    assert_eq!(
        command.program,
        OsString::from(format!("raw-{}", pmpx_plugin_abi::PMPX_VERB_RUN)),
        "the verb has to reach the plugin"
    );
    assert_eq!(
        command.args,
        vec![OsString::from("left-pad")],
        "the arguments have to reach the plugin, and its answer has to come back"
    );
    assert_eq!(command.cwd, None, "no cwd means the project root");
}

/// The plugin is handed the host's hooks only because it offers `attach`.
#[test]
fn the_host_hooks_are_installed_when_the_plugin_offers_them() {
    // SAFETY: loading the fixture, and reading a symbol from the same library that is still open.
    let plugin = unsafe { Plugin::open(fixture()) }.expect("the fixture should load");

    static HOOKS: pmpx_plugin_abi::PmpxHost = pmpx_plugin_abi::PmpxHost {
        abi_major: pmpx_plugin_abi::PMPX_ABI_MAJOR,
        size: std::mem::size_of::<pmpx_plugin_abi::PmpxHost>(),
        capability: no_capabilities,
    };

    unsafe extern "C" fn no_capabilities(
        _name: pmpx_plugin_abi::PmpxStr,
    ) -> *const std::ffi::c_void {
        std::ptr::null()
    }

    let before = attached_count();
    plugin.tables().attach(&HOOKS);
    assert_eq!(
        attached_count(),
        before + 1,
        "the plugin's `attach` has to have been called"
    );
}

/// Read the fixture's counter through its own exported symbol.
///
/// The loader deliberately does not expose arbitrary symbol lookup, so the test does it itself: that
/// is a test's job, not an ABI's.
fn attached_count() -> u32 {
    // SAFETY: the library is the fixture, built from this repository, and the symbol is the one it
    // exports for exactly this purpose.
    unsafe {
        let library = libloading::Library::new(fixture()).expect("the fixture should load");
        let symbol: libloading::Symbol<unsafe extern "C" fn() -> u32> = library
            .get(ATTACHED_SYMBOL)
            .expect("the fixture exports its own counter");
        symbol()
    }
}

/// A library that is not a plugin at all is refused by name, not by crashing.
#[test]
fn a_library_without_the_entry_symbol_is_refused() {
    // The fixture library *is* a plugin, so this uses a file that cannot be a plugin: the loader has
    // to open it (or fail to) rather than assume.
    let not_a_library = std::env::current_exe().expect("the test binary's path");
    let error = match unsafe { Plugin::open(&not_a_library) } {
        Ok(_) => panic!("a library with no entry symbol must be refused"),
        Err(error) => error,
    };

    let text = error.to_string();
    assert!(
        text.contains("cannot load") || text.contains("exports no"),
        "the refusal should say what happened: {text}"
    );
}

/// The entry symbol the loader looks for is the one the ABI declares.
///
/// Spelled out as a byte string above, so this catches the constant and the fixture drifting apart
/// from each other.
#[test]
fn the_entry_symbol_is_the_one_the_abi_declares() {
    let expected = pmpx_plugin_abi::PMPX_ENTRY_SYMBOL.as_bytes();
    assert_eq!(
        &ENTRY_SYMBOL[..ENTRY_SYMBOL.len() - 1],
        expected,
        "the ABI's entry symbol and the one this test looks for have to be the same"
    );
    assert!(
        OsStr::new(pmpx_plugin_abi::PMPX_ENTRY_SYMBOL)
            .to_string_lossy()
            .ends_with("_v3"),
        "the root structure's layout is what the symbol names"
    );
}
