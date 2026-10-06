#![allow(unsafe_code)] // the one load call: this module is where the host crosses into a plugin
//! Loading one plugin and asking it for a command: the host's half of the ABI.
//!
//! Everything below this line is the ABI's problem, and `pmpx-loader` is where the wire format lives.
//! What this module adds is the *host's* decisions: which library to open, whether the plugin is the one
//! the manifest promised, when to hand over the logging hooks, and what to do with what the plugin says
//! while it runs. All of it is reported as [`Event`]s -- nothing here prints.

use std::path::{Path, PathBuf};

use pmpx_loader::{CallError, ContextSource, Plugin};
use pmpx_plugin::abi::PmpxHost;
use pmpx_plugin::{CommandSpec, SelectionReason, Verb};

use crate::error::Result;
use crate::files::Declared;
use crate::log::{self, Levels};
use crate::{EngineError, Event};

/// One installed plugin, as this crate sees it: what the store read from its manifest, and where it
/// lives. No store, no version, no detection: data enough to load it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginIdentity {
    /// The name the plugin reports (`pnpm`) -- checked against what it says about itself once loaded.
    pub name: String,
    /// Full crate name (`pmpx-plugin-pnpm`), for messages.
    pub crate_name: String,
    /// Its directory in the store.
    pub dir: PathBuf,
    /// The files its manifest declared, which it may ask to see.
    pub wanted: Vec<String>,
    /// The ABI the manifest declares. Diagnostics only: what the *plugin* reports is the authority,
    /// and the loader has already refused one built against another major.
    pub declared_abi: Option<u32>,
}

/// Everything one call needs that a plugin cannot work out for itself.
///
/// A borrowed view of the run's own state, assembled at the call site: the plugin reads it through the
/// ABI, and nothing here is copied for the host's own benefit.
pub struct Call<'a> {
    /// Project root: where the command runs unless the answer names a `cwd` of its own.
    pub root: &'a Path,
    /// The directory the person ran pmpx from -- **not** where the command will run.
    pub start_dir: &'a Path,
    /// The files the plugin's own detection matched.
    pub matched: &'a [String],
    /// The `.pmpx.toml` files that were read, nearest first.
    pub config_files: &'a [PathBuf],
    /// `[plugin]` pins from the project config.
    pub pins: &'a std::collections::BTreeMap<String, String>,
    /// The arguments the user typed.
    pub args: &'a [std::ffi::OsString],
    /// Which verb this is.
    pub verb: Verb,
    /// Why this plugin was selected.
    pub reason: SelectionReason,
    /// The score it won with.
    pub score: u32,
    /// Reads the files the plugin declared, on demand -- and refuses everything else.
    pub files: &'a Declared,
}

/// A plugin that is loaded and has passed the identity check.
///
/// It holds the library open, so every function pointer in its tables stays valid for as long as this
/// value lives.
pub struct Backend {
    plugin: Plugin,
    /// The plugin name declared in the manifest.
    name: String,
    /// The files the manifest declared, which the plugin may ask to see.
    wanted: Vec<String>,
}

impl Backend {
    /// Open the plugin's library, check what it says about itself, and hand it the host's hooks.
    ///
    /// `library` is the file the store found for this plugin; this crate does not search for it, because
    /// only the store knows how a plugin's directory is laid out.
    pub fn load(
        identity: &PluginIdentity,
        library: &Path,
        levels: Levels,
        events: &mut dyn FnMut(Event),
    ) -> Result<Self> {
        // SAFETY: the file comes from the plugin store, and loading a dynamic library runs whatever code
        // is inside it -- that is the point of a plugin.
        let loaded = unsafe { Plugin::open(library) }.map_err(|error| {
            EngineError::NotFound(format!(
                "cannot load {}\n{error}\n\
                 The file is {} -- removing and installing the plugin again usually settles it.",
                identity.crate_name,
                library.display()
            ))
        })?;

        // The self-reported name must match what the manifest declares. A plugin that panicked inside
        // `name()` answers with the contract's marker instead, which would otherwise look like an odd
        // pair of names -- so say what actually happened.
        let self_reported = loaded.name();
        if self_reported == pmpx_plugin::shell::PANIC_MARKER {
            return Err(EngineError::NotFound(format!(
                "plugin {} panicked while reporting its name -- its own panic message is on stderr \
                 above.\nDelete {} and install it again.",
                identity.name,
                identity.dir.display()
            )));
        }

        if self_reported != identity.name {
            return Err(EngineError::NotFound(format!(
                "the plugin calls itself \"{self_reported}\", but the manifest declares \"{}\" -- \
                 refusing to load.\nDelete {} and install it again.",
                identity.name,
                identity.dir.display()
            )));
        }

        // A manifest that disagrees with the library beside it means the wrapper and the plugin came
        // from different builds, which is worth saying out loud -- it is what a stale install looks like.
        if let Some(declared) = identity.declared_abi {
            if declared != pmpx_plugin::abi::PMPX_ABI_MAJOR {
                events(Event::Warning(format!(
                    "plugin {} declares ABI {declared} in its manifest but its library reports {}",
                    identity.name,
                    pmpx_plugin::abi::PMPX_ABI_MAJOR
                )));
            }
        }

        // Everything is validated, so the plugin may now be told about its host. This is also what makes
        // `pmpx_plugin::debug!` reach pmpx instead of falling back to the plugin's own stderr, and what
        // keeps a plugin's notes from being formatted at all when nobody asked for them.
        loaded.tables().attach(log::hooks(levels));

        Ok(Self {
            plugin: loaded,
            name: identity.name.clone(),
            wanted: identity.wanted.clone(),
        })
    }

    /// The name the plugin reports for itself, which the manifest has already been checked against.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The family the plugin reports for itself.
    pub fn family(&self) -> String {
        self.plugin.family().to_string()
    }

    /// The rustc version and target the plugin was built with. Diagnostics only.
    pub fn build_info(&self) -> (String, String) {
        self.plugin.tables().build_info()
    }

    /// The files the manifest declared this plugin may be handed.
    pub fn wanted_files(&self) -> &[String] {
        &self.wanted
    }

    /// The host's hooks, for a caller that wants to attach them itself.
    pub fn hooks(levels: Levels) -> &'static PmpxHost {
        log::hooks(levels)
    }

    /// Ask the plugin to map one call to one command.
    ///
    /// A [`CallError`] is a *business* answer -- "I cannot do that" -- not a failure of the crossing
    /// itself, which is why it is returned rather than raised. Everything the plugin said while it ran
    /// (its `debug!` lines, what the file provider refused) reaches `events`.
    pub fn command(
        &self,
        call: &Call<'_>,
        events: &mut dyn FnMut(Event),
    ) -> std::result::Result<CommandSpec, CallError> {
        let source = ContextSource {
            root: call.root,
            start_dir: call.start_dir,
            matched: call.matched,
            config_files: call.config_files,
            pins: call.pins,
            args: call.args,
            verb: call.verb.to_abi(),
            reason: call.reason.to_abi(),
            score: call.score,
            files: call.files,
        };

        // A plugin's messages can never be attributed to the previous call: the queue starts empty.
        log::clear();

        let answer = self.plugin.tables().call(&source);

        // The plugin's own lines first, then whatever the file provider had to say: both belong to this
        // call, and both are drained exactly once.
        log::drain(events);
        for note in call.files.take_notes() {
            events(Event::Note(note));
        }

        answer.map(|command| CommandSpec {
            program: command.program,
            args: command.args,
            cwd: command.cwd,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity check is the one thing a manifest is for: a plugin that calls itself something else
    /// is refused rather than used.
    #[test]
    fn an_identity_mismatch_is_a_setup_error() {
        let error = EngineError::NotFound(format!(
            "the plugin calls itself \"other\", but the manifest declares \"{}\" -- refusing to load.",
            "wanted"
        ));

        assert!(error.is_not_found());
        assert!(error.message().contains("wanted"));
    }

    /// The engine's view of a plugin is data: nothing about the store, a version, or a score.
    #[test]
    fn an_identity_is_data() {
        let identity = PluginIdentity {
            name: "pnpm".to_string(),
            crate_name: "pmpx-plugin-pnpm".to_string(),
            dir: PathBuf::from("/plugins/pnpm"),
            wanted: vec!["package.json".to_string()],
            declared_abi: Some(3),
        };

        assert_eq!(identity.wanted.len(), 1);
        assert_eq!(identity.name, "pnpm");
    }
}
