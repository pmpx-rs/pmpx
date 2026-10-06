//! The sandbox every end-to-end area builds on: a temporary pmpx config directory, a plugin
//! directory, a project directory, and the fake plugin compiled on demand.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

/// The path of the pmpx binary. cargo provides this environment variable for bin targets.
pub(crate) const PMPX: &str = env!("CARGO_BIN_EXE_pmpx");

/// The workspace `target` directory this test binary was built into.
///
/// The layout is `<target>/<profile>/deps/<test binary>`, so three levels up from the running
/// executable. Everything a test compiles for itself goes under here rather than into a temporary
/// directory of its own: it is the same fixture crate every time, and rebuilding it once per test
/// was most of the suite's runtime.
fn workspace_target_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .and_then(|deps| deps.parent().map(Path::to_path_buf))
        .and_then(|profile| profile.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

/// A sandbox: a pmpx config directory, a plugin directory, and a "project" directory.
pub(crate) struct Sandbox {
    _tmp: tempfile::TempDir,
    pub(crate) config_dir: PathBuf,
    pub(crate) data_dir: PathBuf,
    pub(crate) project: PathBuf,
}

impl Sandbox {
    pub(crate) fn new() -> Self {
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
    pub(crate) fn file(&self, name: &str) -> &Self {
        let p = self.project.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, "").unwrap();
        self
    }

    /// Write a global config.
    pub(crate) fn global_config(&self, body: &str) -> &Self {
        std::fs::write(self.config_dir.join("config.toml"), body).unwrap();
        self
    }

    /// Lay out an "installed plugin" directory (cdylib + manifest).
    pub(crate) fn install_plugin(
        &self,
        crate_name: &str,
        manifest: &str,
        lib: Option<&Path>,
    ) -> &Self {
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
    pub(crate) fn run(&self, args: &[&str]) -> Output {
        Command::new(PMPX)
            .args(args)
            .current_dir(&self.project)
            .env("PMPX_CONFIG_DIR", &self.config_dir)
            .env("PMPX_DATA_DIR", &self.data_dir)
            .output()
            .expect("should be able to start pmpx")
    }

    /// Run once and assert success, returning stdout.
    pub(crate) fn ok(&self, args: &[&str]) -> String {
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

pub(crate) fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(crate) fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// ---------------------------------------------------------------------------
// Compiling the real plugin
// ---------------------------------------------------------------------------

/// Build the fake plugin once per test binary and return the cdylib path.
///
/// The build output lives under the workspace's own `target`, so a second run of the suite only
/// has to relink what changed instead of compiling the fixture from scratch for every test.
pub(crate) fn build_fake_plugin() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();

    BUILT
        .get_or_init(|| {
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("fake-plugin");
            let target_dir = workspace_target_dir().join("fixture-fake-plugin");

            let status = Command::new("cargo")
                .args(["build", "--release", "--manifest-path"])
                .arg(fixture.join("Cargo.toml"))
                .arg("--target-dir")
                .arg(&target_dir)
                .status()
                .expect("should be able to start cargo");
            assert!(status.success(), "the fake plugin should compile");

            crate_plugin_kit::find_library(&target_dir.join("release"), "pmpx_plugin_fakepm")
                .expect("should find the fake plugin's cdylib")
        })
        .clone()
}

pub(crate) const FAKEPM_MANIFEST: &str = r#"
[plugin]
name    = "fakepm"
version = "0.1.0"
abi     = 3
family  = "faketest"

[detect]
strong = ["fakepm.lock"]
weak   = ["fakepm.json"]

[context]
files = ["fakepm.json"]
"#;

/// A sandbox with the fake plugin installed.
pub(crate) fn sandbox_with_plugin() -> (Sandbox, PathBuf) {
    let lib = build_fake_plugin();
    let sb = Sandbox::new();
    sb.install_plugin("pmpx-plugin-fakepm", FAKEPM_MANIFEST, Some(&lib));
    (sb, lib)
}
