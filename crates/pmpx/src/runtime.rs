//! Load the selected plugin and call it across the ABI.
//!
//! Validation happens before the call, not at load time: `crate-plugin-kit`'s `load()` only
//! guarantees "it can be `dlopen`ed and the entry symbol can be fetched"; whether this
//! plugin can talk to this host is decided by pmpx -- right after getting the vtable it
//! checks `abi_version == ABI_VERSION` (equality is enough, not full crate version equality)
//! and that `name()` matches what the manifest declared.
//!
//! Every call into the plugin is wrapped in `catch_unwind`, but that is only a fallback --
//! the main defence is the `extern "C"` shell that the plugin-side `export!` generates:
//! since Rust 1.81, letting a panic cross `extern "C"` aborts outright and the host cannot
//! rescue it.

use std::ffi::OsString;
use std::path::Path;

use crate_plugin_kit::{CratePluginKit, LoadedPlugin};
use pmpx_plugin::abi::{
    self, PmpxCommand, PmpxPluginV1, PmpxStr, ABI_VERSION, PMPX_ERR_INTERNAL,
    PMPX_ERR_INVALID_ARGS, PMPX_ERR_UNSUPPORTED_VERB, PMPX_OK,
};
use pmpx_plugin::{CommandSpec, Verb};

use crate::error::{PmpxError, Result};
use crate::plugins::InstalledPlugin;

/// A failure the plugin reported explicitly.
///
/// **Kept separate from [`PmpxError`]** because the meaning differs: `PmpxError` is "something
/// went wrong on pmpx's side", this is "the plugin answered normally that it cannot do this".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// The plugin does not support this verb.
    ///
    /// `pmpx exec` degrades to passing through verbatim when it gets this; the other six
    /// verbs keep "unsupported is an error".
    UnsupportedVerb,

    /// The plugin says the arguments are wrong.
    InvalidArgs(String),

    /// The plugin failed internally, or it panicked.
    Internal(String),
}

impl BackendError {
    /// The matching process exit code.
    pub fn exit_code(&self) -> u8 {
        match self {
            // "this backend cannot do what you asked" -- same code as a usage error
            BackendError::UnsupportedVerb | BackendError::InvalidArgs(_) => {
                crate::error::EXIT_USAGE
            }
            // the plugin blew up -- counted as a pmpx error of its own
            BackendError::Internal(_) => crate::error::EXIT_INTERNAL,
        }
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::UnsupportedVerb => f.write_str("this backend does not support that verb"),
            BackendError::InvalidArgs(m) => write!(f, "the backend rejected the arguments: {m}"),
            BackendError::Internal(m) => write!(f, "the backend failed internally: {m}"),
        }
    }
}

/// A plugin that is loaded and has passed the ABI and name checks.
///
/// It holds the `dlopen` handle, so the dynamic library is not unloaded during its lifetime.
/// It must never be `Send`/`Sync`: the function pointers in the vtable belong to that dynamic
/// library, and calling them from another thread would remove the guarantee that the library
/// is still there.
pub struct Backend {
    loaded: LoadedPlugin<PmpxPluginV1>,
    /// The plugin name declared in the manifest.
    pub name: String,
}

impl Backend {
    /// Load one plugin and validate it.
    pub fn load(kit: &CratePluginKit<PmpxPluginV1>, plugin: &InstalledPlugin) -> Result<Self> {
        let loaded = kit.load(&plugin.crate_name).map_err(|e| {
            PmpxError::not_found(format!(
                "failed to load plugin {}: {e}\n\
                 It is installed at {} -- try `pmpx plugin rm {}` and install it again.",
                plugin.crate_name,
                plugin.dir.display(),
                plugin.name
            ))
        })?;

        let entry = unsafe { &*loaded.entry() };

        // The ABI version must be equal
        if entry.abi_version != ABI_VERSION {
            return Err(PmpxError::not_found(format!(
                "plugin {} has ABI version {}, pmpx needs {}.\n\
                 They are two separately compiled worlds; when the layouts do not line up it \
                 cannot be loaded -- that is better than crashing.\n\
                 Use `pmpx plugin update {}` to upgrade the plugin, or upgrade pmpx to a \
                 matching version.",
                plugin.crate_name, entry.abi_version, ABI_VERSION, plugin.name
            )));
        }

        // The self-reported name must match what the manifest declares
        let self_reported = unsafe { read_plugin_str(entry.name, entry.free_str) };
        if self_reported != plugin.name {
            // Both names have to be printed -- just saying "they differ" leaves the user
            // unable to tell which one to trust
            return Err(PmpxError::not_found(format!(
                "the plugin calls itself \"{self_reported}\", but the manifest declares \
                 \"{}\" -- refusing to load.\n\
                 Delete {} and install it again.",
                plugin.name,
                plugin.dir.display()
            )));
        }

        Ok(Self {
            loaded,
            name: plugin.name.clone(),
        })
    }

    /// A reference to the vtable.
    fn entry(&self) -> &PmpxPluginV1 {
        // SAFETY: `load()` checked that it is not null, and `self.loaded` holds that dynamic
        // library.
        unsafe { &*self.loaded.entry() }
    }

    /// The family the plugin reports for itself.
    ///
    /// Before loading, the value came from the manifest (detection can only read that); after
    /// loading this is what the plugin itself says. The two disagreeing is not fatal (so it
    /// does not refuse to load), but it is worth showing.
    pub fn family(&self) -> String {
        let e = self.entry();
        unsafe { read_plugin_str(e.family, e.free_str) }
    }

    /// The rustc version the plugin was compiled with. Diagnostics only.
    pub fn rustc_version(&self) -> String {
        unsafe { read_bytes(self.entry().rustc_version) }
    }

    /// The target triple the plugin was compiled for. Diagnostics only.
    pub fn target(&self) -> String {
        unsafe { read_bytes(self.entry().target) }
    }

    /// Hand "verb + arguments" to the plugin and let it translate them into one command.
    ///
    /// The return value is `Result<Result<...>>`: the outer [`PmpxError`] means crossing the
    /// boundary itself failed (which should not happen); the inner [`BackendError`] means the
    /// plugin answered normally that it cannot do this -- that is a business result, not a
    /// fault.
    pub fn command(
        &self,
        project_root: &Path,
        matched: &[String],
        verb: Verb,
        args: &[OsString],
    ) -> Result<std::result::Result<CommandSpec, BackendError>> {
        let entry = self.entry();

        // ---------------------------------------------------------------
        // Input direction: allocated by the host, freed by the host, read-only for the
        // plugin -- these Vecs live until the end of this function, and the plugin is
        // required to finish reading before that.
        // ---------------------------------------------------------------
        let root_bytes = abi::os_to_bytes(project_root.as_os_str());
        let root = PmpxStr {
            ptr: root_bytes.as_ptr(),
            len: root_bytes.len(),
        };

        let matched_bytes: Vec<Vec<u8>> = matched.iter().map(|m| m.as_bytes().to_vec()).collect();
        let matched_raw: Vec<PmpxStr> = matched_bytes
            .iter()
            .map(|b| PmpxStr {
                ptr: b.as_ptr(),
                len: b.len(),
            })
            .collect();

        let arg_bytes: Vec<Vec<u8>> = args
            .iter()
            .map(|a| abi::os_to_bytes(a.as_os_str()))
            .collect();
        let args_raw: Vec<PmpxStr> = arg_bytes
            .iter()
            .map(|b| PmpxStr {
                ptr: b.as_ptr(),
                len: b.len(),
            })
            .collect();

        // ---------------------------------------------------------------
        // Output direction: allocated by the plugin, freed by the plugin, read-only for the
        // host
        // ---------------------------------------------------------------
        let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();

        let code = {
            let out_ptr = out.as_mut_ptr();
            // The fallback catch_unwind layer. The main defence is in the plugin-side
            // export! shell.
            abi::guard(move || {
                // SAFETY: the inputs are allocated by this function and stay alive for the
                // call; out points to local writable memory. The remaining contract is
                // stated in the Safety section of `PmpxPluginV1::command`.
                unsafe {
                    (entry.command)(
                        root,
                        matched_raw.as_ptr(),
                        matched_raw.len(),
                        verb.to_abi(),
                        args_raw.as_ptr(),
                        args_raw.len(),
                        out_ptr,
                    )
                }
            })
        };

        match code {
            PMPX_OK => {}
            PMPX_ERR_UNSUPPORTED_VERB => return Ok(Err(BackendError::UnsupportedVerb)),
            PMPX_ERR_INVALID_ARGS => {
                return Ok(Err(BackendError::InvalidArgs(format!(
                    "plugin {} thinks the arguments are invalid",
                    self.name
                ))))
            }
            PMPX_ERR_INTERNAL => {
                return Ok(Err(BackendError::Internal(format!(
                    "plugin {} failed internally or panicked (details on its own stderr)",
                    self.name
                ))))
            }
            other => {
                return Ok(Err(BackendError::Internal(format!(
                    "plugin {} returned unknown error code {other}",
                    self.name
                ))))
            }
        }

        // SAFETY: on `PMPX_OK` the plugin guarantees it filled out.
        let mut cmd = unsafe { out.assume_init() };

        // Copy this memory out, then give it back to the plugin as agreed -- the host stays
        // read-only throughout.
        let spec = unsafe {
            let program = abi::read_os(cmd.program);
            let cwd = if cmd.cwd.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(abi::read_os(cmd.cwd)))
            };
            let mut argv = Vec::with_capacity(cmd.args_len);
            for i in 0..cmd.args_len {
                argv.push(abi::read_os(*cmd.args.add(i)));
            }
            CommandSpec {
                program,
                args: argv,
                cwd,
            }
        };

        // SAFETY: it comes from the successful call above, and is freed exactly once.
        unsafe { (entry.free_command)(&mut cmd as *mut _) };

        Ok(Ok(spec))
    }

    /// The diagnostics `pmpx info` shows.
    pub fn diagnostics(&self) -> BackendDiagnostics {
        BackendDiagnostics {
            name: self.name.clone(),
            family: self.family(),
            rustc_version: self.rustc_version(),
            target: self.target(),
        }
    }
}

/// What the plugin reports about itself, as shown by `pmpx info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendDiagnostics {
    /// The plugin's self-reported name.
    pub name: String,
    /// The family the plugin reports for itself.
    pub family: String,
    /// The rustc that compiled it.
    pub rustc_version: String,
    /// The target triple it was compiled for.
    pub target: String,
}

/// Call one vtable function that returns a [`PmpxStr`], read it as a Rust string, and
/// **release it as agreed**.
///
/// # Safety
///
/// `f` and `free` must come from the same still-valid vtable.
unsafe fn read_plugin_str(
    f: unsafe extern "C" fn() -> PmpxStr,
    free: unsafe extern "C" fn(PmpxStr),
) -> String {
    let s = unsafe { f() };
    let out = unsafe { read_bytes(s) };
    // The memory belongs to the plugin and must be given back after reading -- the host
    // never frees what it did not allocate.
    unsafe { free(s) };
    out
}

/// Read a [`PmpxStr`] without releasing it.
///
/// # Safety
///
/// `s` must describe read-only memory that is valid for the duration of the call.
unsafe fn read_bytes(s: PmpxStr) -> String {
    if s.ptr.is_null() || s.len == 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_error_exit_codes_match_the_table() {
        assert_eq!(
            BackendError::UnsupportedVerb.exit_code(),
            crate::error::EXIT_USAGE
        );
        assert_eq!(
            BackendError::InvalidArgs("x".into()).exit_code(),
            crate::error::EXIT_USAGE
        );
        assert_eq!(
            BackendError::Internal("x".into()).exit_code(),
            crate::error::EXIT_INTERNAL
        );
    }

    #[test]
    fn backend_error_messages_name_the_backend_situation() {
        assert!(BackendError::UnsupportedVerb
            .to_string()
            .contains("does not support"));
        assert!(BackendError::InvalidArgs("a".into())
            .to_string()
            .contains('a'));
        assert!(BackendError::Internal("b".into()).to_string().contains('b'));
    }

    #[test]
    fn reading_an_empty_string_is_safe() {
        assert_eq!(unsafe { read_bytes(PmpxStr::EMPTY) }, "");
    }

    #[test]
    fn reading_a_null_pointer_is_safe() {
        let s = PmpxStr {
            ptr: std::ptr::null(),
            len: 99,
        };
        assert_eq!(
            unsafe { read_bytes(s) },
            "",
            "a null pointer must not be dereferenced"
        );
    }

    #[test]
    fn reading_non_utf8_bytes_does_not_panic() {
        let bytes = [0xff, 0xfe, 0xfd];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        // Diagnostics only, so a lossy conversion is enough -- but it must not panic
        assert!(!unsafe { read_bytes(s) }.is_empty());
    }
}
