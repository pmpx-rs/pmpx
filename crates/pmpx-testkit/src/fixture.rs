//! A directory of files, run through the real detection.

use std::collections::BTreeMap;
use std::path::Path;

use pmpx_detect::{select, Candidate, Preferences, Presence};
use pmpx_plugin::{Context, SelectionReason};

/// One project, on disk, detected the way the host detects it.
///
/// The files are written into a temporary directory that lives as long as the fixture. The detection is
/// the real one: the same scoring, the same family and plugin choice, on the markers a manifest
/// declares -- so a fixture test fails when the plugin's own markers stop matching, which is exactly the
/// kind of breakage a hand-built context cannot catch.
pub struct Fixture {
    dir: tempfile::TempDir,
    candidates: Vec<Candidate>,
    pins: BTreeMap<String, String>,
    declares: Vec<String>,
    family_priority: Vec<String>,
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixture {
    /// An empty project.
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("should be able to create a temporary directory"),
            candidates: Vec::new(),
            pins: BTreeMap::new(),
            declares: Vec::new(),
            family_priority: Vec::new(),
        }
    }

    /// Write one file, creating its directory if it has one.
    pub fn file(self, name: &str, contents: &str) -> Self {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("should be able to create the file's directory");
        }
        std::fs::write(path, contents).expect("should be able to write the file");
        self
    }

    /// Declare a plugin, with the markers its manifest would carry.
    pub fn plugin(self, name: &str, family: &str, strong: &[&str], weak: &[&str]) -> Self {
        let mut fixture = self;
        fixture.candidates.push(Candidate {
            crate_name: format!("pmpx-plugin-{name}"),
            name: name.to_string(),
            family: family.to_string(),
            strong: strong.iter().map(|s| s.to_string()).collect(),
            weak: weak.iter().map(|s| s.to_string()).collect(),
            problem: None,
        });
        fixture
    }

    /// Pin a family to a plugin, as a `.pmpx.toml` would.
    pub fn pin(self, family: &str, plugin: &str) -> Self {
        let mut fixture = self;
        fixture.pins.insert(family.to_string(), plugin.to_string());
        fixture
    }

    /// Order families, for the cases where two of them tie.
    pub fn prefer(self, families: &[&str]) -> Self {
        let mut fixture = self;
        fixture.family_priority = families.iter().map(|s| s.to_string()).collect();
        fixture
    }

    /// The files the plugin's manifest says it wants to see.
    ///
    /// Only these are read into the context -- the declaration is the allowlist, so a test that asks for
    /// something undeclared gets a context without it, exactly as the host would.
    pub fn declares<I, S>(self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut fixture = self;
        fixture.declares = names.into_iter().map(Into::into).collect();
        fixture
    }

    /// The project directory.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Run the detection and build the context the host would hand over.
    ///
    /// The verb and the arguments are not part of a [`Context`]: they are what the plugin is *called*
    /// with, so a test passes them to
    /// [`command`](pmpx_plugin::PackageManager::command) itself.
    ///
    /// # Panics
    /// When the detection picks nothing -- which in a fixture test almost always means the markers a
    /// plugin declares do not match the files the test wrote, so the panic says which files are there.
    pub fn context(&self) -> Context<'static> {
        let selection = select(
            &self.candidates,
            &self.presence(),
            &self.pins,
            &Preferences {
                family_priority: self.family_priority.clone(),
                priority: Vec::new(),
            },
            None,
        )
        .unwrap_or_else(|failure| {
            panic!(
                "the fixture detected nothing: {}\nfiles in {}:\n{}",
                failure.message(),
                self.dir.path().display(),
                self.listing()
            )
        });

        let mut builder = Context::builder()
            .project_root(self.dir.path())
            .start_dir(self.dir.path())
            .matched(selection.matched.iter().cloned())
            .reason(match selection.reason {
                pmpx_detect::Reason::Scored => SelectionReason::Scored,
                pmpx_detect::Reason::Pinned => SelectionReason::Pinned,
                pmpx_detect::Reason::Explicit => SelectionReason::Explicit,
            })
            .score(selection.score);

        for (family, plugin) in &self.pins {
            builder = builder.pin(family, plugin);
        }

        for name in &self.declares {
            if let Ok(contents) = std::fs::read(self.dir.path().join(name)) {
                builder = builder.file(name, contents);
            }
        }

        builder.build()
    }

    /// The files the fixture contains, for a failure message.
    fn listing(&self) -> String {
        let mut names = Vec::new();
        collect(self.dir.path(), self.dir.path(), &mut names);
        names.sort();
        names.join("\n")
    }

    /// The directory, as the decision asks about it.
    fn presence(&self) -> TestDir<'_> {
        TestDir {
            root: self.dir.path(),
        }
    }
}

/// A directory, answering the decision's one question.
struct TestDir<'a> {
    root: &'a Path,
}

impl Presence for TestDir<'_> {
    fn has(&self, relative: &str) -> bool {
        self.root.join(relative).exists()
    }
}

/// Every file under `root`, relative to it, for a failure message.
fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.display().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_fixture() -> Fixture {
        Fixture::new()
            .plugin("pnpm", "node", &["pnpm-lock.yaml"], &["package.json"])
            .plugin("cargo", "rust", &["Cargo.lock"], &["Cargo.toml"])
            .declares(["package.json"])
    }

    /// The fixture's context is the one the real detection produces: the matched file, the score, and the
    /// declared file's contents.
    #[test]
    fn the_fixture_runs_the_real_detection() {
        let fixture = plugin_fixture()
            .file("pnpm-lock.yaml", "")
            .file("package.json", "{\"name\":\"x\"}");

        let context = fixture.context();

        assert!(context.has_matched("pnpm-lock.yaml"));
        assert_eq!(context.reason, SelectionReason::Scored);
        assert_eq!(context.score, 110, "a lockfile plus a manifest");
        assert_eq!(
            context.file_str("package.json").as_deref(),
            Some("{\"name\":\"x\"}")
        );
    }

    /// A file the manifest did not declare never reaches the context, however it is written in the
    /// fixture: the declaration is the allowlist.
    #[test]
    fn an_undeclared_file_is_not_handed_over() {
        let fixture = plugin_fixture()
            .file("pnpm-lock.yaml", "")
            .file("extra.toml", "x = 1");

        let context = fixture.context();

        assert!(context.file("extra.toml").is_none());
    }

    /// A pin decides, and the context says so.
    #[test]
    fn a_pin_is_reported_as_the_reason() {
        let fixture = Fixture::new()
            .plugin("pnpm", "node", &["pnpm-lock.yaml"], &["package.json"])
            .plugin("cargo", "rust", &[], &["Cargo.toml"])
            .pin("rust", "cargo")
            .file("package.json", "{}")
            .file("Cargo.toml", "");

        let context = fixture.context();

        assert_eq!(context.reason, SelectionReason::Pinned);
        assert_eq!(context.pinned_for("rust"), Some("cargo"));
    }

    /// Nothing matching is a panic that says what was there, because that is the failure mode a fixture
    /// test is written to catch.
    #[test]
    #[should_panic(expected = "the fixture detected nothing")]
    fn nothing_matching_panics_with_the_listing() {
        plugin_fixture().file("unrelated.txt", "").context();
    }
}
