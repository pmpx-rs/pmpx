//! Putting the new binary in place of the running one.
//!
//! The staged copy lives in the same directory as the binary it replaces, because the swap
//! is a `rename` and that is only atomic within one filesystem. On Windows a running
//! executable cannot be deleted or overwritten, but it can be renamed -- so the old one is
//! moved aside first, and whatever is left of it is cleaned up on the next start.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
/// Removes a scratch directory when it goes out of scope.
pub(super) struct Scratch {
    pub(super) path: PathBuf,
}

impl Scratch {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(super) fn remove_dir_if_exists(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Puts `staged` in place of `exe`.
#[cfg(unix)]
pub(super) fn replace(exe: &Path, staged: &Path) -> anyhow::Result<()> {
    // On Unix a running executable can simply be replaced: the old inode stays alive
    // until this process exits.
    fs::rename(staged, exe).with_context(|| format!("cannot replace {}", exe.display()))
}

/// Puts `staged` in place of `exe`.
#[cfg(windows)]
pub(super) fn replace(exe: &Path, staged: &Path) -> anyhow::Result<()> {
    // Windows will not let a running executable be overwritten or deleted, but it will let
    // it be *renamed*. So: move the old one aside, move the new one in, and try to delete
    // the old one -- which usually fails while this process is still running it, hence
    // `cleanup_stale_old` on the next start.
    let old = append_to_name(exe, ".old");
    let _ = fs::remove_file(&old);

    fs::rename(exe, &old).with_context(|| {
        format!(
            "cannot move the running binary aside to {} -- is that directory writable by you?",
            old.display()
        )
    })?;

    if let Err(error) = fs::rename(staged, exe) {
        // Put the old binary back rather than leaving the user with no pmpx at all.
        let _ = fs::rename(&old, exe);
        let _ = fs::remove_file(staged);
        return Err(error)
            .with_context(|| format!("cannot install the new binary at {}", exe.display()));
    }

    let _ = fs::remove_file(&old);
    Ok(())
}

/// Deletes the `.old` binary a previous Windows update had to leave behind.
///
/// It cannot be deleted at update time -- that process is still running it -- so every
/// start tries once and says nothing: a failure only means "next time". On other platforms
/// an update never leaves one, so this costs nothing at all.
#[cfg(windows)]
pub fn cleanup_stale_old() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(append_to_name(&exe, ".old"));
    }
}

/// Deletes the `.old` binary a previous Windows update had to leave behind.
#[cfg(not(windows))]
pub fn cleanup_stale_old() {}

/// `pmpx` -> `pmpx.new` (the extension is appended, not replaced, so `pmpx.exe` becomes
/// `pmpx.exe.new` rather than `pmpx.new`).
pub(super) fn append_to_name(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_puts_the_new_binary_in_place_of_the_old() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("pmpx");
        let staged = append_to_name(&exe, ".new");

        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(&staged, b"new").unwrap();

        replace(&exe, &staged).expect("should replace");

        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert!(!staged.exists(), "the staged file should have been moved");
    }

    /// The name the leftover gets: appended, not substituted, so `pmpx.exe` does not turn
    /// into `pmpx.old`.
    #[test]
    fn the_backup_name_keeps_the_extension() {
        assert_eq!(
            append_to_name(Path::new("/usr/local/bin/pmpx"), ".new"),
            PathBuf::from("/usr/local/bin/pmpx.new")
        );
        assert_eq!(
            append_to_name(Path::new(r"C:\bin\pmpx.exe"), ".old"),
            PathBuf::from(r"C:\bin\pmpx.exe.old")
        );
    }
}
