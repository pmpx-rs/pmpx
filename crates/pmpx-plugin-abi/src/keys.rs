//! The keys a host may answer, and the meaning of each.
//!
//! Keys are strings, not struct fields, and that is the point: a host that learns a new one does not
//! change any layout, so no plugin has to be rebuilt (see `docs/refactor.md` §3.2-3.3).
//!
//! Rules both sides rely on:
//!
//! - **An unknown key is not an error.** It answers `0` / absent, which means "this host does not
//!   know that". A plugin must cope: an older host simply knows fewer keys.
//! - **`file.` is gated by the plugin's own manifest.** A host answers `file.<name>` only for names
//!   the plugin declared in its `[context] files`, so the declaration stays the allowlist. A name
//!   that was not declared answers absent, exactly like a file that is not there.
//! - **Only a plugin writes these strings.** They are constants here so a typo is a compile error on
//!   the Rust side; a hand-written plugin that mistypes one fails closed, which is the safe
//!   direction.

/// `0`/`1`: the project root, as an absolute path.
pub const PMPX_KEY_PROJECT_ROOT: &str = "project.root";

/// `0`/`1`: the directory the person ran pmpx from, as an absolute path.
///
/// It differs from [`PMPX_KEY_PROJECT_ROOT`] whenever the root was found by walking up, which makes
/// it the only way to tell which package of a monorepo this is. **Not** where the command will run:
/// that is the answer's `cwd`.
pub const PMPX_KEY_PROJECT_START_DIR: &str = "project.start_dir";

/// `n`: the files this plugin's detection matched, relative to the root.
pub const PMPX_KEY_PROJECT_MATCHED: &str = "project.matched";

/// `n`: the project config files that were read, nearest first.
pub const PMPX_KEY_PROJECT_CONFIG_FILES: &str = "project.config_files";

/// `n`: the arguments the user typed, verbatim, in order.
pub const PMPX_KEY_ARGS: &str = "args";

/// `n`: the project config's plugin pins. `get(i)` is the pinned plugin, `name(i)` the family.
pub const PMPX_KEY_CONFIG_PIN: &str = "config.pin";

/// Prefix of the per-file keys: `file.<name>` answers the contents the plugin declared.
pub const PMPX_KEY_FILE_PREFIX: &str = "file.";

/// Every key of this major version, as `(key, macro name in the C header)`.
///
/// The pairs are explicit rather than derived, so a key like [`PMPX_KEY_FILE_PREFIX`] gets the
/// readable `PMPX_KEY_FILE_PREFIX` instead of a mechanically produced `PMPX_KEY_FILE_`.
pub const PMPX_KEYS: &[(&str, &str)] = &[
    (PMPX_KEY_ARGS, "PMPX_KEY_ARGS"),
    (PMPX_KEY_CONFIG_PIN, "PMPX_KEY_CONFIG_PIN"),
    (PMPX_KEY_FILE_PREFIX, "PMPX_KEY_FILE_PREFIX"),
    (
        PMPX_KEY_PROJECT_CONFIG_FILES,
        "PMPX_KEY_PROJECT_CONFIG_FILES",
    ),
    (PMPX_KEY_PROJECT_MATCHED, "PMPX_KEY_PROJECT_MATCHED"),
    (PMPX_KEY_PROJECT_ROOT, "PMPX_KEY_PROJECT_ROOT"),
    (PMPX_KEY_PROJECT_START_DIR, "PMPX_KEY_PROJECT_START_DIR"),
];
