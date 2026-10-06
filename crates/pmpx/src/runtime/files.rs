//! Reading the files a plugin asked to see.
//!
//! A plugin declares them in its manifest's `[context] files`, and the host reads exactly those
//! from the project root: no globbing, no walking, no interpretation. That keeps two things true at
//! once -- what a plugin can see is written down in one auditable place, and pmpx still knows
//! nothing about what any of these files mean.

use std::fs;
use std::path::{Component, Path, PathBuf};

/// The most one file may contribute. A lockfile or a manifest is kilobytes; anything at this size is
/// a mistake, and the plugin is told it got only the beginning rather than being handed the lot.
pub(crate) const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The most files one plugin may ask for. Past this the declarations are ignored, with a note under
/// `--debug` -- a plugin that declares hundreds of files is not doing something this channel is for.
pub(crate) const MAX_FILES: usize = 16;

/// One file, read for a plugin.
pub struct ContextFile {
    /// The path as the manifest declared it.
    pub name: String,
    /// The bytes, which need not be UTF-8: reading them is the plugin's business.
    pub bytes: Vec<u8>,
    /// Whether the file was larger than [`MAX_FILE_BYTES`] and `bytes` is only its beginning.
    pub truncated: bool,
}

/// Read what `wanted` asks for, relative to `root`.
///
/// Nothing here can fail the run: a declared file that is missing, unreadable, or not a plain
/// relative path inside the project is simply not in the answer, which the plugin can tell because
/// it declared it in the first place.
pub(crate) fn read(wanted: &[String], root: &Path) -> Vec<ContextFile> {
    let mut out = Vec::new();

    for declaration in wanted.iter().take(MAX_FILES) {
        let Some(relative) = inside_project(declaration) else {
            crate::debug::note(format!(
                "ignoring the declared context file {declaration:?}: only plain relative paths \
                 inside the project can be read"
            ));
            continue;
        };

        match fs::read(root.join(&relative)) {
            Ok(bytes) => {
                let truncated = bytes.len() as u64 > MAX_FILE_BYTES;
                let mut bytes = bytes;
                if truncated {
                    bytes.truncate(MAX_FILE_BYTES as usize);
                }

                out.push(ContextFile {
                    name: declaration.clone(),
                    bytes,
                    truncated,
                });
            }
            // The plugin asked for something that is not there; that is an answer too, and the
            // plugin can tell the difference between "empty" and "not given".
            Err(e) => crate::debug::note(format!(
                "the declared context file {declaration:?} was not read: {e}"
            )),
        }
    }

    if wanted.len() > MAX_FILES {
        crate::debug::note(format!(
            "the manifest declares {} context files; only the first {MAX_FILES} are read",
            wanted.len()
        ));
    }

    out
}

/// The path to read for one declaration, or `None` when it is not something this channel may read.
///
/// Rejecting is the point: a manifest is written by whoever published the plugin, so `..` or an
/// absolute path there would be a way to ask for files outside the project -- a plugin's business is
/// the project, not the machine it happens to sit on.
fn inside_project(declaration: &str) -> Option<PathBuf> {
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

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temp dir should be creatable")
    }

    fn declared(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_declared_file_is_read() {
        let dir = tmp();
        fs::write(dir.path().join("package.json"), "{\"name\":\"x\"}").unwrap();

        let files = read(&declared(&["package.json"]), dir.path());

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "package.json");
        assert_eq!(files[0].bytes, b"{\"name\":\"x\"}");
        assert!(!files[0].truncated);
    }

    /// A file that is not there is left out rather than reported: the plugin declared it, so it
    /// knows what "not in the list" means, and nothing here should be able to fail a run.
    #[test]
    fn a_missing_file_is_simply_absent() {
        let dir = tmp();

        let files = read(&declared(&["nope.toml"]), dir.path());

        assert!(files.is_empty());
    }

    /// The whole point of the declaration: it is the plugin's, and the plugin is not the machine's
    /// owner. Anything that could leave the project is refused.
    #[test]
    fn a_path_that_leaves_the_project_is_refused() {
        let dir = tmp();
        fs::write(dir.path().join("secret.txt"), "not yours").unwrap();

        for declaration in [
            "../secret.txt",
            "/etc/passwd",
            "a/../../secret.txt",
            "",
            ".",
            "..",
        ] {
            assert!(
                inside_project(declaration).is_none(),
                "{declaration:?} should not be readable"
            );
        }

        // And the normalising that is allowed does not let one through: `./a` stays inside.
        assert_eq!(
            inside_project("./package.json"),
            Some(PathBuf::from("package.json"))
        );
    }

    #[test]
    fn a_file_larger_than_the_limit_comes_back_truncated() {
        let dir = tmp();
        let big = vec![b'x'; (MAX_FILE_BYTES + 10) as usize];
        fs::write(dir.path().join("big.lock"), &big).unwrap();

        let files = read(&declared(&["big.lock"]), dir.path());

        assert_eq!(files.len(), 1);
        assert!(
            files[0].truncated,
            "the plugin must be told it is incomplete"
        );
        assert_eq!(files[0].bytes.len() as u64, MAX_FILE_BYTES);
    }

    #[test]
    fn only_the_first_files_are_read() {
        let dir = tmp();
        let names: Vec<String> = (0..MAX_FILES + 3).map(|i| format!("f{i}.txt")).collect();
        for name in &names {
            fs::write(dir.path().join(name), "x").unwrap();
        }

        let files = read(&names, dir.path());

        assert_eq!(files.len(), MAX_FILES);
    }

    /// A directory is not a file: reading one fails on every platform, and it must not turn into an
    /// empty entry the plugin would take for an empty file.
    #[test]
    fn a_directory_is_not_a_file() {
        let dir = tmp();
        fs::create_dir(dir.path().join("sub")).unwrap();

        let files = read(&declared(&["sub"]), dir.path());

        assert!(files.is_empty());
    }
}
