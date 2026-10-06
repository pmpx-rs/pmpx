//! Reading the files a plugin asked to see.
//!
//! A plugin declares them in its manifest's `[context] files`, and the loader asks this provider for
//! one by name when the plugin asks for it. That keeps two things true at once: what a plugin can see is
//! written down in one auditable place, and pmpx still knows nothing about what any of these files
//! mean.
//!
//! The declaration is the **allowlist**: a name that is not in it is never read, whether or not the
//! plugin asks, and a name that would leave the project is refused whatever it says.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use pmpx_loader::Files;

/// The most one file may contribute. A lockfile or a manifest is kilobytes; anything at this size is a
/// mistake, and the plugin is handed the beginning rather than the lot.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The most files one plugin may declare. Past this the declarations are ignored, with a note -- a
/// plugin that declares hundreds of files is not doing something this channel is for.
pub const MAX_FILES: usize = 16;

/// What a plugin declared, and the root to read it from.
pub struct Declared {
    root: PathBuf,
    names: Vec<String>,
    /// Answers already given: a file is read once per run, however often it is asked for.
    cache: RefCell<BTreeMap<String, Option<Vec<u8>>>>,
    /// What happened while reading, for the caller to turn into events.
    notes: RefCell<Vec<String>>,
}

impl Declared {
    /// Take the declarations from one plugin's manifest.
    pub fn new(root: &Path, wanted: &[String]) -> Self {
        let mut notes = Vec::new();
        if wanted.len() > MAX_FILES {
            notes.push(format!(
                "the manifest declares {} context files; only the first {MAX_FILES} are read",
                wanted.len()
            ));
        }

        let names = wanted.iter().take(MAX_FILES).cloned().collect();
        Self {
            root: root.to_path_buf(),
            names,
            cache: RefCell::new(BTreeMap::new()),
            notes: RefCell::new(notes),
        }
    }

    /// Everything worth saying about the reads so far, oldest first.
    ///
    /// Collected rather than reported because the loader calls this from inside a plugin's call, where
    /// there is no event sink to hand: the engine drains this once the call returns.
    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut self.notes.borrow_mut())
    }

    /// Read one file, or explain why it cannot be handed over.
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        let note = |message: String| self.notes.borrow_mut().push(message);

        if !self.names.iter().any(|declared| declared == name) {
            // Not the plugin's to read: the manifest is the allowlist, so this is refused without
            // touching the filesystem at all.
            note(format!(
                "ignoring the context file {name:?}: the manifest does not declare it"
            ));
            return None;
        }

        let Some(relative) = inside_project(name) else {
            note(format!(
                "ignoring the declared context file {name:?}: only plain relative paths inside the \
                 project can be read"
            ));
            return None;
        };

        match fs::read(self.root.join(&relative)) {
            Ok(mut bytes) => {
                if bytes.len() as u64 > MAX_FILE_BYTES {
                    bytes.truncate(MAX_FILE_BYTES as usize);
                    note(format!(
                        "the declared context file {name:?} is larger than {MAX_FILE_BYTES} bytes; the \
                         plugin gets only its beginning"
                    ));
                }
                Some(bytes)
            }
            // The plugin asked for something that is not there; that is an answer too, and the plugin
            // can tell the difference between "empty" and "not given".
            Err(e) => {
                note(format!(
                    "the declared context file {name:?} was not read: {e}"
                ));
                None
            }
        }
    }
}

impl Files for Declared {
    fn contents(&self, name: &str) -> Option<Vec<u8>> {
        let mut cache = self.cache.borrow_mut();

        if let Some(known) = cache.get(name) {
            return known.clone();
        }

        let answer = self.read(name);
        cache.insert(name.to_string(), answer.clone());
        answer
    }
}

/// The path to read for one declaration, or `None` when it is not something this channel may read.
///
/// Rejecting is the point: a manifest is written by whoever published the plugin, so `..` or an
/// absolute path there would be a way to ask for files outside the project -- a plugin's business is the
/// project, not the machine it happens to sit on.
pub fn inside_project(declaration: &str) -> Option<PathBuf> {
    let path = Path::new(declaration);

    if path.as_os_str().is_empty() || path.is_absolute() {
        return None;
    }

    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            // `./x` and `x` mean the same thing; normalising keeps the join honest.
            Component::CurDir => {}
            Component::Normal(part) => relative.push(part),
            // `..`, a drive letter, a root, or a UNC prefix: all ways out of the project.
            _ => return None,
        }
    }

    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn provider(root: &Path, names: &[&str]) -> Declared {
        Declared::new(root, &declared(names))
    }

    #[test]
    fn a_declared_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{\"name\":\"x\"}").unwrap();

        let files = provider(dir.path(), &["package.json"]);

        assert_eq!(
            files.contents("package.json"),
            Some(b"{\"name\":\"x\"}".to_vec())
        );
        assert!(files.take_notes().is_empty(), "nothing to report");
    }

    /// A file the manifest did not declare is never read, even if the plugin asks for it by name.
    #[test]
    fn an_undeclared_file_is_refused_and_said_so() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("secret.txt"), "not yours").unwrap();

        let files = provider(dir.path(), &["package.json"]);

        assert_eq!(files.contents("secret.txt"), None);
        let notes = files.take_notes();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("does not declare"), "{notes:?}");
        assert!(files.take_notes().is_empty(), "notes are drained once");
    }

    /// A file that is not there is simply absent: the plugin declared it, so it knows what "not in the
    /// answer" means, and nothing here should be able to fail a run.
    #[test]
    fn a_missing_file_is_simply_absent() {
        let dir = tempfile::tempdir().unwrap();

        let files = provider(dir.path(), &["nope.toml"]);

        assert_eq!(files.contents("nope.toml"), None);
        assert!(
            files.take_notes()[0].contains("was not read"),
            "{:?}",
            files.take_notes()
        );
    }

    /// Anything that could leave the project is refused, declared or not.
    #[test]
    fn a_path_that_leaves_the_project_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("secret.txt"), "not yours").unwrap();

        let files = provider(
            dir.path(),
            &["../secret.txt", "/etc/passwd", "a/../../secret.txt"],
        );

        for name in ["../secret.txt", "/etc/passwd", "a/../../secret.txt"] {
            assert_eq!(files.contents(name), None, "{name} must not be readable");
        }
        assert!(inside_project("../secret.txt").is_none());
        assert!(inside_project("/etc/passwd").is_none());
        assert!(inside_project("a/../../secret.txt").is_none());
        assert_eq!(
            inside_project("./package.json"),
            Some(PathBuf::from("package.json"))
        );
    }

    #[test]
    fn a_file_larger_than_the_limit_comes_back_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![b'x'; (MAX_FILE_BYTES + 10) as usize];
        fs::write(dir.path().join("big.lock"), &big).unwrap();

        let files = provider(dir.path(), &["big.lock"]);

        assert_eq!(
            files.contents("big.lock").map(|bytes| bytes.len() as u64),
            Some(MAX_FILE_BYTES)
        );
        assert!(
            files.take_notes()[0].contains("larger than"),
            "truncation is reported"
        );
    }

    #[test]
    fn only_the_first_files_are_readable() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (0..MAX_FILES + 3).map(|i| format!("f{i}.txt")).collect();
        for name in &names {
            fs::write(dir.path().join(name), "x").unwrap();
        }

        let declared: Vec<&str> = names.iter().map(String::as_str).collect();
        let files = provider(dir.path(), &declared);

        assert!(files.contents(&names[MAX_FILES - 1]).is_some());
        assert_eq!(
            files.contents(&names[MAX_FILES]),
            None,
            "past the declared cap"
        );
        assert!(
            files.take_notes()[0].contains("only the first"),
            "the cap is reported"
        );
    }

    /// A directory is not a file: reading one fails on every platform, and it must not turn into an
    /// empty entry the plugin would take for an empty file.
    #[test]
    fn a_directory_is_not_a_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();

        let files = provider(dir.path(), &["sub"]);

        assert_eq!(files.contents("sub"), None);
    }
}
