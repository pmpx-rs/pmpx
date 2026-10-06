//! Writing a config file without ever leaving a half-written one behind.
//!
//! Both files pmpx writes are the user's: `<config-dir>/pmpx/config.toml` is personal, and a
//! project's `.pmpx.toml` is meant to be committed. `fs::write` truncates first, so an interrupted
//! write -- or a full disk -- leaves a truncated file, and for the project one that is the version
//! that would be committed.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// Write `text` to `path`, replacing the file in one step.
///
/// The new content goes into a temporary file in the *same directory* (a rename is only atomic
/// within one filesystem) and is then renamed over the target, so a reader sees either the old
/// file or the new one, never a partial one.
///
/// A symlink is followed: the target file is what gets replaced, which is what a plain write would
/// have done, and the link keeps pointing at it.
pub fn atomic_write(path: &Path, text: &str) -> Result<()> {
    // Follow a symlink so that replacing the file does not replace the link itself.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_string());

    // The pid keeps two concurrent runs from writing into the same temporary file.
    let tmp = dir.join(format!(".{name}.{}.pmpx-tmp", std::process::id()));

    write_and_rename(&tmp, &target, text).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// The body of [`atomic_write`], split out so the cleanup above has one exit path.
fn write_and_rename(tmp: &Path, target: &Path, text: &str) -> Result<()> {
    {
        let mut file =
            fs::File::create(tmp).with_context(|| format!("failed to create {}", tmp.display()))?;

        std::io::Write::write_all(&mut file, text.as_bytes())
            .with_context(|| format!("failed to write {}", tmp.display()))?;

        // Make sure the content is on disk before the rename can make it visible: without this a
        // crash can leave an empty file under the new name.
        file.sync_all()
            .with_context(|| format!("failed to flush {}", tmp.display()))?;

        // A rewrite must not widen what the file already allowed.
        if let Ok(meta) = fs::metadata(target) {
            let _ = fs::set_permissions(tmp, meta.permissions());
        }
    }

    fs::rename(tmp, target).with_context(|| format!("failed to replace {}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");

        atomic_write(&path, "a = 1\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "a = 1\n");
    }

    #[test]
    fn replaces_the_whole_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "old = 1\nwith more text\n").unwrap();

        atomic_write(&path, "new = 2\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new = 2\n");
    }

    #[test]
    fn leaves_no_temporary_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");

        atomic_write(&path, "a = 1\n").unwrap();

        let names: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["config.toml".to_string()], "{names:?}");
    }

    /// A failed write must leave the original file alone -- that is the whole point.
    #[test]
    fn a_failed_write_keeps_the_original() {
        let tmp = tempfile::tempdir().unwrap();
        // A directory in the target's place: creating the temporary file beside it works, but the
        // rename over a directory cannot.
        let path = tmp.path().join("config.toml");
        fs::create_dir(&path).unwrap();

        assert!(atomic_write(&path, "a = 1\n").is_err());
        assert!(path.is_dir(), "the target must be untouched");

        let leftovers: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["config.toml".to_string()], "{leftovers:?}");
    }
}
