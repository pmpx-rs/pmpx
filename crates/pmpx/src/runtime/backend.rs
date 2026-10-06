//! Loading one plugin and asking it for a command.
//!
//! Everything below this line is the ABI's problem, and `pmpx-loader` is where that lives: this
//! module is the *host's* half of the conversation -- which library to open, whether the plugin is
//! the one the manifest promised, and when to hand over the logging hooks.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use pmpx_loader::{ContextSource, Plugin};
use pmpx_plugin::abi::PmpxHost;
use pmpx_plugin::{CommandSpec, SelectionReason, Verb};

use crate::error::{PmpxError, Result};
use crate::plugins::InstalledPlugin;

/// A plugin that is loaded and has passed the identity check.
///
/// It holds the library open, so every function pointer in its tables stays valid for as long as this
/// value lives.
pub struct Backend {
    plugin: Plugin,
    /// The plugin name declared in the manifest.
    pub name: String,

    /// The files the manifest declared, which the plugin may ask to see. Not read yet: the
    /// declaration is the allowlist, and the reading happens on demand.
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
    pub reason: SelectionReason,
    /// The score it won with.
    pub score: u32,
    /// `[plugin]` pins from the project config.
    pub pins: &'a BTreeMap<String, String>,
    /// The `.pmpx.toml` files that were read, nearest first.
    pub config_files: &'a [PathBuf],
    /// The arguments the user typed.
    pub args: &'a [OsString],
    /// Reads the files the plugin declared in its manifest, on demand.
    pub files: &'a dyn pmpx_loader::Files,
}

impl Backend {
    /// Open the plugin's library, check what it says about itself, and hand it the host's hooks.
    ///
    /// `host` is the hooks the plugin may log through; the level inside them is what decides how much
    /// of a plugin's output is ever formatted, and installing them is also what tells the plugin it
    /// is running under a host at all.
    pub fn load(plugin: &InstalledPlugin, host: &'static PmpxHost) -> Result<Self> {
        // The library file lives in the plugin's own directory, named after its crate with the
        // platform's conventions -- `find_library` knows the last part of that.
        let stem = plugin.crate_name.replace('-', "_");
        let library = crate_plugin_kit::find_library(&plugin.dir, &stem).map_err(|e| {
            PmpxError::not_found(format!(
                "cannot find the plugin library for {} in {}: {e}\n\
                 Try `pmpx plugin rm {}` and install it again.",
                plugin.crate_name,
                plugin.dir.display(),
                plugin.name
            ))
        })?;

        // SAFETY: the file comes from the plugin store, and loading a dynamic library runs whatever
        // code is inside it -- that is the point of a plugin.
        let loaded = unsafe { Plugin::open(&library) }.map_err(|e| {
            PmpxError::not_found(format!(
                "cannot load {}\n{e}\n\
                 The file is {} -- `pmpx plugin rm {}` and installing it again usually settles it.",
                plugin.crate_name,
                library.display(),
                plugin.name
            ))
        })?;

        // The self-reported name must match what the manifest declares. A plugin that panicked inside
        // `name()` answers with the contract's marker instead, which would otherwise look like an odd
        // pair of names -- so say what actually happened.
        let self_reported = loaded.name();
        if self_reported == pmpx_plugin::shell::PANIC_MARKER {
            return Err(PmpxError::not_found(format!(
                "plugin {} panicked while reporting its name -- its own panic message is on \
                 stderr above.\n\
                 Delete {} and install it again.",
                plugin.name,
                plugin.dir.display()
            )));
        }

        if self_reported != plugin.name {
            return Err(PmpxError::not_found(format!(
                "the plugin calls itself \"{self_reported}\", but the manifest declares \
                 \"{}\" -- refusing to load.\n\
                 Delete {} and install it again.",
                plugin.name,
                plugin.dir.display()
            )));
        }

        // The manifest is metadata: what the plugin *reports* is the authority, and the loader has
        // already refused a plugin built against another major. A manifest that disagrees with the
        // library beside it means the wrapper and the plugin came from different builds, which is
        // worth saying out loud -- it is exactly what a stale install looks like.
        if let Some(declared) = plugin.abi {
            if declared != pmpx_plugin::abi::PMPX_ABI_MAJOR {
                crate::error::note_line(format!(
                    "plugin {} declares ABI {declared} in its manifest but its library reports {}",
                    plugin.name,
                    pmpx_plugin::abi::PMPX_ABI_MAJOR
                ));
            }
        }

        // Everything is validated, so the plugin may now be told about its host. This is also what
        // makes `pmpx_plugin::debug!` reach pmpx instead of falling back to the plugin's own stderr,
        // and what keeps a plugin's notes from being formatted at all when nobody asked for them.
        crate::runtime::set_current_plugin(&plugin.name);
        loaded.tables().attach(host);

        Ok(Self {
            plugin: loaded,
            name: plugin.name.clone(),
            wanted: plugin.wanted.clone(),
        })
    }

    /// The files the manifest declared this plugin may be handed.
    pub fn wanted_files(&self) -> &[String] {
        &self.wanted
    }

    /// The rustc version and target the plugin was built with. Diagnostics only.
    pub fn build_info(&self) -> (String, String) {
        self.plugin.tables().build_info()
    }

    /// The family the plugin reports for itself.
    ///
    /// Before loading, the value came from the manifest (detection can only read that); after loading
    /// this is what the plugin itself says. The two disagreeing is not fatal, but it is worth showing.
    pub fn family(&self) -> String {
        self.plugin.family().to_string()
    }

    /// Ask the plugin to map one call to one command.
    ///
    /// A [`CallError`](pmpx_loader::CallError) is a *business* answer -- "I cannot do that" -- not a
    /// failure of the crossing itself, which is why it is returned rather than raised here.
    pub fn command(
        &self,
        invocation: &Invocation<'_>,
        verb: Verb,
    ) -> std::result::Result<CommandSpec, pmpx_loader::CallError> {
        let source = ContextSource {
            root: invocation.root,
            start_dir: invocation.start_dir,
            matched: invocation.matched,
            config_files: invocation.config_files,
            pins: invocation.pins,
            args: invocation.args,
            verb: verb.to_abi(),
            reason: invocation.reason.to_abi(),
            score: invocation.score,
            files: invocation.files,
        };

        let command = self.plugin.tables().call(&source)?;

        Ok(CommandSpec {
            program: command.program,
            args: command.args,
            cwd: command.cwd,
        })
    }
}
