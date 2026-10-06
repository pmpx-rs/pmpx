//! The loaded plugin: one `dlopen` handle, one validated vtable, and every call that goes through
//! it.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate_plugin_kit::{CratePluginKit, LoadedPlugin};
use pmpx_plugin::abi::{
    self, PmpxCommand, PmpxContextV1, PmpxFile, PmpxKeyValue, PmpxPin, PmpxPluginV1, PmpxStr,
    ABI_VERSION, PMPX_ERR_INTERNAL, PMPX_ERR_INVALID_ARGS, PMPX_ERR_UNSUPPORTED_VERB, PMPX_OK,
};
use pmpx_plugin::{CommandSpec, Verb};

use super::error::BackendError;
use super::files::ContextFile;
use super::strings::{read_bytes, read_plugin_str};
use super::BackendDiagnostics;
use crate::error::{PmpxError, Result};
use crate::plugins::InstalledPlugin;

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
    /// The files it asked to see, from its manifest's `[context] files`.
    wanted: Vec<String>,
}

/// Everything one call needs that the plugin cannot work out for itself.
///
/// A borrowed view of the run's own state, assembled at the call site: the plugin reads it through
/// the ABI, and nothing here is copied for the host's own benefit.
pub struct Invocation<'a> {
    /// Project root: where the command runs unless the answer names a `cwd` of its own.
    pub root: &'a Path,
    /// The directory the person ran pmpx from -- **not** where the command will run.
    pub start_dir: &'a Path,
    /// The files the winning plugin's detection matched.
    pub matched: &'a [String],
    /// Why this plugin was selected.
    pub reason: pmpx_plugin::SelectionReason,
    /// The score it won with.
    pub score: u32,
    /// `[plugin]` pins from the project config.
    pub pins: &'a BTreeMap<String, String>,
    /// `[scripts]` from the project config.
    pub scripts: &'a BTreeMap<String, String>,
    /// The `.pmpx.toml` files that were read, nearest first.
    pub config_files: &'a [PathBuf],
    /// What this plugin asked to see the contents of, already read.
    pub files: &'a [ContextFile],
}

impl Backend {
    /// Load one plugin and validate it.
    ///
    /// `host` is the hooks the plugin may log through; the level inside them is what decides how
    /// much of a plugin's output is ever formatted, and installing them is also what tells the
    /// plugin it is running under a host at all.
    pub fn load(
        kit: &CratePluginKit<PmpxPluginV1>,
        plugin: &InstalledPlugin,
        host: &'static abi::PmpxHostV1,
    ) -> Result<Self> {
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

        // The manifest's own `abi` claim is checked against the library it installed. It is *not* a
        // hard check: the field is written by whoever published the plugin, and a stale declaration
        // must not disable a plugin that works. But the two disagreeing means one of them is wrong,
        // and the manifest is what `plugin list` shows and what a user reads before deciding whether
        // an update is needed.
        if let Some(declared) = plugin.abi {
            if declared != entry.abi_version {
                crate::error::note_line(format!(
                    "plugin {} declares ABI {declared} in its manifest, but the library reports \
                     {}. One of the two is stale; `pmpx plugin update {}` should settle it.",
                    plugin.crate_name, entry.abi_version, plugin.name
                ));
            }
        }

        // The self-reported name must match what the manifest declares
        let self_reported = unsafe { read_plugin_str(entry.name, entry.free_str) };

        // A plugin that panicked inside `name()` (or its factory) answers with the contract's
        // marker instead of a name; the mismatch below would report that as an odd-looking pair of
        // names, so say what actually happened.
        if self_reported == pmpx_plugin::abi::PANIC_MARKER {
            return Err(PmpxError::not_found(format!(
                "plugin {} panicked while reporting its name -- its own panic message is on \
                 stderr above.\n\
                 Delete {} and install it again.",
                plugin.name,
                plugin.dir.display()
            )));
        }

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

        // Everything is validated, so the plugin may now be told about its host. This is also what
        // makes `pmpx_plugin::debug!` reach pmpx instead of falling back to the plugin's own
        // stderr, and `host.max_level` is what keeps a plugin's notes from being formatted at all
        // when nobody asked for them.
        crate::runtime::set_current_plugin(&plugin.name);
        unsafe { (entry.set_host)(host as *const abi::PmpxHostV1) };

        Ok(Self {
            loaded,
            name: plugin.name.clone(),
            wanted: plugin.wanted.clone(),
        })
    }

    /// The files this plugin's manifest asked to see.
    ///
    /// Read by the caller rather than here: the library is loaded and validated at this point, but
    /// nothing on the filesystem should be touched until a call is actually about to happen.
    pub fn wanted_files(&self) -> &[String] {
        &self.wanted
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
        invocation: &Invocation<'_>,
        verb: Verb,
        args: &[OsString],
    ) -> Result<std::result::Result<CommandSpec, BackendError>> {
        let entry = self.entry();
        let project_root = invocation.root;
        let matched = invocation.matched;

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
        // Start from a value the plugin has to overwrite. A plugin that answers `PMPX_OK` without
        // filling `out` in then looks like "a command with no program", which this side can
        // report -- instead of reading uninitialised memory.
        let mut cmd = PmpxCommand {
            program: PmpxStr::EMPTY,
            args: std::ptr::null(),
            args_len: 0,
            cwd: PmpxStr::EMPTY,
        };

        // Everything the host knows about this call, in one struct the plugin reads. What the host
        // has nothing for stays empty, which reads as "the host does not know this" rather than as
        // an error.
        //
        // Every view here points either at those `Vec`s (which live until the end of this function)
        // or straight into the session's own config maps, which outlive the call.
        let start_bytes = abi::os_to_bytes(invocation.start_dir.as_os_str());
        let config_bytes: Vec<Vec<u8>> = invocation
            .config_files
            .iter()
            .map(|path| abi::os_to_bytes(path.as_os_str()))
            .collect();

        let text = |s: &str| PmpxStr {
            ptr: s.as_ptr(),
            len: s.len(),
        };

        let pins_raw: Vec<PmpxPin> = invocation
            .pins
            .iter()
            .map(|(family, plugin)| PmpxPin {
                family: text(family),
                plugin: text(plugin),
            })
            .collect();

        let scripts_raw: Vec<PmpxKeyValue> = invocation
            .scripts
            .iter()
            .map(|(key, value)| PmpxKeyValue {
                key: text(key),
                value: text(value),
            })
            .collect();

        let config_raw: Vec<PmpxStr> = config_bytes
            .iter()
            .map(|b| PmpxStr {
                ptr: b.as_ptr(),
                len: b.len(),
            })
            .collect();

        // What the plugin asked to see: the file contents were read by the caller, and the name
        // strings and bytes live there for the length of this call.
        let files_raw: Vec<PmpxFile> = invocation
            .files
            .iter()
            .map(|file| PmpxFile {
                name: text(&file.name),
                contents: PmpxStr {
                    ptr: file.bytes.as_ptr(),
                    len: file.bytes.len(),
                },
                truncated: u32::from(file.truncated),
            })
            .collect();

        let mut context = PmpxContextV1::empty();
        context.root = root;
        context.start_dir = PmpxStr {
            ptr: start_bytes.as_ptr(),
            len: start_bytes.len(),
        };
        context.matched = matched_raw.as_ptr();
        context.matched_len = matched_raw.len();
        context.verb = verb.to_abi();
        context.reason = invocation.reason.to_abi();
        context.score = invocation.score;
        context.args = args_raw.as_ptr();
        context.args_len = args_raw.len();
        context.pins = pins_raw.as_ptr();
        context.pins_len = pins_raw.len();
        context.scripts = scripts_raw.as_ptr();
        context.scripts_len = scripts_raw.len();
        context.config_paths = config_raw.as_ptr();
        context.config_paths_len = config_raw.len();
        context.files = files_raw.as_ptr();
        context.files_len = files_raw.len();

        let code = {
            let out_ptr = &mut cmd as *mut PmpxCommand;
            let context_ptr = &context as *const PmpxContextV1;
            // The fallback catch_unwind layer. The main defence is in the plugin-side
            // export! shell.
            abi::guard(move || {
                // SAFETY: the context and the arrays it points at are allocated by this function
                // and stay alive for the call; out points to local writable memory. The remaining
                // contract is stated in the Safety section of `PmpxPluginV1::command`.
                unsafe { (entry.command)(context_ptr, out_ptr) }
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

        // `out` was filled in, but it is still data from the other side of a boundary this side
        // does not control: `read_os` on a pointer-less length is undefined behaviour, and a
        // garbage `args_len` is an out-of-bounds walk that `free_command` would repeat.
        if let Some(why) = implausible(&cmd) {
            // The argument array itself cannot be trusted, so it must not be handed back either:
            // leaking it is the smaller mistake. Every other rejection is freed as agreed.
            if why != Implausible::ArgsLen {
                unsafe { (entry.free_command)(&mut cmd as *mut _) };
            }
            return Ok(Err(BackendError::Internal(format!(
                "the command plugin {} produced cannot be used: {why}",
                self.name
            ))));
        }

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

/// The largest argument list a plugin may report.
///
/// No command line can be longer than the OS allows, so a bigger number is a plugin bug -- and
/// `args_len` is also what `free_command` walks, so it has to be checked before anything else
/// touches the array.
const MAX_COMMAND_ARGS: usize = 4096;

/// Why a [`PmpxCommand`] a plugin just filled in cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Implausible {
    /// More arguments than any command line could hold.
    ArgsLen,
    /// A length without a pointer, or nothing to run at all.
    Shape(&'static str),
}

impl std::fmt::Display for Implausible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Implausible::ArgsLen => {
                write!(f, "it reported more than {MAX_COMMAND_ARGS} arguments")
            }
            Implausible::Shape(what) => f.write_str(what),
        }
    }
}

/// Whether a string the plugin returned can be read at all.
///
/// `len == 0` is the contract's "no string" and may carry a null pointer; a length without a
/// pointer is not an empty string, it is a struct this side must not dereference.
fn readable(s: PmpxStr) -> bool {
    s.len == 0 || !s.ptr.is_null()
}

/// Check a command description before anything reads it.
///
/// The order matters: the argument count first, because it is both the one value that can make a
/// later walk run off the end and the one that `free_command` would use.
fn implausible(cmd: &PmpxCommand) -> Option<Implausible> {
    if cmd.args_len > MAX_COMMAND_ARGS {
        return Some(Implausible::ArgsLen);
    }
    if cmd.args_len > 0 && cmd.args.is_null() {
        return Some(Implausible::Shape("it reported arguments with no array"));
    }
    if cmd.program.is_empty() {
        return Some(Implausible::Shape("it reported a command with no program"));
    }
    if !readable(cmd.program) || !readable(cmd.cwd) {
        return Some(Implausible::Shape(
            "it reported a string with a length but no pointer",
        ));
    }

    // SAFETY: `args` is non-null here and `args_len` is bounded, so the array the contract says
    // the plugin allocated can be walked.
    let bad_element = unsafe { (0..cmd.args_len).any(|i| !readable(*cmd.args.add(i))) };
    if bad_element {
        return Some(Implausible::Shape(
            "it reported an argument with a length but no pointer",
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `PmpxStr` over a literal that outlives the test's structs.
    fn s(text: &'static str) -> PmpxStr {
        PmpxStr {
            ptr: text.as_ptr(),
            len: text.len(),
        }
    }

    fn command(program: PmpxStr, args: &[PmpxStr]) -> PmpxCommand {
        PmpxCommand {
            program,
            args: args.as_ptr(),
            args_len: args.len(),
            cwd: PmpxStr::EMPTY,
        }
    }

    #[test]
    fn a_well_formed_command_passes() {
        let args = [s("add"), s("serde")];
        assert_eq!(implausible(&command(s("cargo"), &args)), None);
    }

    /// The "returned `PMPX_OK` without filling `out`" case: the host pre-fills the struct, so it
    /// arrives here as an empty program rather than as uninitialised memory.
    #[test]
    fn a_command_with_no_program_is_refused() {
        assert_eq!(
            implausible(&command(PmpxStr::EMPTY, &[])),
            Some(Implausible::Shape("it reported a command with no program"))
        );
    }

    #[test]
    fn an_absurd_argument_count_is_refused_before_the_array_is_touched() {
        let cmd = PmpxCommand {
            program: s("cargo"),
            args: std::ptr::null(),
            args_len: MAX_COMMAND_ARGS + 1,
            cwd: PmpxStr::EMPTY,
        };

        // `ArgsLen` specifically: it is the one rejection that must not hand the array back.
        assert_eq!(implausible(&cmd), Some(Implausible::ArgsLen));
    }

    #[test]
    fn arguments_without_an_array_are_refused() {
        let cmd = PmpxCommand {
            program: s("cargo"),
            args: std::ptr::null(),
            args_len: 2,
            cwd: PmpxStr::EMPTY,
        };

        assert_eq!(
            implausible(&cmd),
            Some(Implausible::Shape("it reported arguments with no array"))
        );
    }

    #[test]
    fn a_string_with_a_length_but_no_pointer_is_refused() {
        let program = PmpxStr {
            ptr: std::ptr::null(),
            len: 5,
        };

        assert_eq!(
            implausible(&command(program, &[])),
            Some(Implausible::Shape(
                "it reported a string with a length but no pointer"
            ))
        );
    }

    #[test]
    fn an_argument_with_a_length_but_no_pointer_is_refused() {
        let args = [
            s("add"),
            PmpxStr {
                ptr: std::ptr::null(),
                len: 3,
            },
        ];

        assert_eq!(
            implausible(&command(s("cargo"), &args)),
            Some(Implausible::Shape(
                "it reported an argument with a length but no pointer"
            ))
        );
    }
}
