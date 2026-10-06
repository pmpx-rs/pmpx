//! Putting the new binary in place of the running one.
//!
//! The staged copy lives in the same directory as the binary it replaces, because the swap
//! is a `rename` and that is only atomic within one filesystem. On Windows a running
//! executable cannot be deleted or overwritten, but it can be renamed -- so the old one is
//! moved aside first, and whatever is left of it is cleaned up on the next start.

use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;

/// How long a lock file may sit there before it counts as a leftover from a killed run.
///
/// Downloading an archive and swapping it in takes seconds; a file this old means nobody is
/// coming back for it, and refusing forever would leave the user with no way to update.
const STALE_LOCK: Duration = Duration::from_secs(10 * 60);

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

/// Serializes `self update` runs.
///
/// Everything the swap touches beside the binary has a fixed name -- `pmpx.exe.old`,
/// `pmpx.exe.new` -- so two runs at once would install each other's archive or delete each other's
/// backup. The lock file is created with `create_new`, which is the whole protocol: exactly one
/// process can create it.
#[derive(Debug)]
pub(super) struct UpdateLock {
    path: PathBuf,
}

impl UpdateLock {
    /// Take the lock beside `exe`, or explain who is holding it.
    pub(super) fn acquire(exe: &Path) -> anyhow::Result<Self> {
        let path = append_to_name(exe, ".update-lock");

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = write!(file, "{}", std::process::id());
                Ok(Self { path })
            }

            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                if is_stale_lock(&path) {
                    let _ = fs::remove_file(&path);
                    return Self::acquire(exe);
                }

                Err(anyhow::anyhow!(
                    "another `pmpx self update` looks like it is running ({}).\n\
                     If it is not, delete that file and try again.",
                    path.display()
                ))
            }

            Err(e) => Err(e).with_context(|| format!("cannot create {}", path.display())),
        }
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Whether the lock file at `path` is old enough to be a leftover.
fn is_stale_lock(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(lock_is_stale)
}

/// The age rule on its own, so it can be tested without faking file times.
fn lock_is_stale(age: Duration) -> bool {
    age > STALE_LOCK
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

    // A leftover that cannot be removed is why the rename below can fail, so it is recorded here
    // rather than blamed on the directory. A leftover that is merely absent is not a problem.
    let removed_old = match fs::remove_file(&old) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };

    fs::rename(exe, &old).with_context(|| match &removed_old {
        Ok(()) => format!(
            "cannot move the running binary aside to {} -- is that directory writable by you?",
            old.display()
        ),
        Err(e) => format!(
            "cannot replace the leftover {} ({e}) -- close whatever is still running it and try \
             again",
            old.display()
        ),
    })?;

    if let Err(error) = fs::rename(staged, exe) {
        // Put the old binary back rather than leaving the user with no pmpx at all.
        let error = anyhow::Error::new(error).context(format!(
            "cannot install the new binary at {}",
            exe.display()
        ));

        return Err(match fs::rename(&old, exe) {
            Ok(()) => {
                let _ = fs::remove_file(staged);
                error
            }
            // Nothing else will tell the user where the only usable copy went, so say it here --
            // and keep both files, because a retry can still use them.
            Err(rollback) => error.context(format!(
                "putting the old binary back failed too ({rollback}); it is still at {}",
                old.display()
            )),
        });
    }

    let _ = fs::remove_file(&old);
    Ok(())
}

/// Deletes what a previous update had to leave behind.
///
/// On Windows that is the `.old` binary -- it cannot be deleted at update time, because that
/// process is still running it -- and on every platform a `.new` that never made it into place.
/// Both are fixed names, so they are removed by name, and a failure only means "next time": a
/// concurrent update holds the lock and fails cleanly rather than corrupting anything.
pub fn cleanup_stale_old() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };

    let _ = fs::remove_file(append_to_name(&exe, ".new"));

    #[cfg(windows)]
    let _ = fs::remove_file(append_to_name(&exe, ".old"));
}

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
    // Duration is already in scope through the parent module.

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

    /// The lock is `create_new`, so the second run has to be refused rather than starting a
    /// download it would then fight the first one over.
    #[test]
    fn a_second_update_is_refused_while_the_lock_is_held() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("pmpx");

        let held = UpdateLock::acquire(&exe).expect("the first lock should be free");
        let second = UpdateLock::acquire(&exe);
        assert!(second.is_err(), "a second update must not start");
        assert!(
            second.unwrap_err().to_string().contains(".update-lock"),
            "the message should name the file to remove"
        );

        drop(held);
        UpdateLock::acquire(&exe).expect("after the lock is released it is free again");
    }

    /// A lock file left by a killed run is taken over, or the user could never update again.
    #[test]
    fn an_old_lock_counts_as_a_leftover() {
        assert!(!lock_is_stale(Duration::from_secs(1)));
        assert!(!lock_is_stale(STALE_LOCK));
        assert!(lock_is_stale(STALE_LOCK + Duration::from_secs(1)));
    }
}
