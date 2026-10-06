//! Wires the layers together: the context of one run, and the whole flow from argv to
//! "run one command".
//!
//! This file is the context itself -- everything one run reads once; [`flow`] is what a verb then
//! does with it.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use crate_plugin_kit::{CratePluginKit, KitConfig};
use pmpx_plugin::abi::PmpxPlugin;
use pmpx_plugin::Family;

use crate::cli::Cli;
use crate::config::{GlobalConfig, MergedProjectConfig};
use crate::debug;
use crate::detect::{self, DetectFailure, FamilyScore, Selection};
use crate::discovery::{self, StopReason, Walk};
use crate::error::PmpxError;
use crate::plugins::PluginSet;
use crate::runtime::Backend;
use crate::style;

mod flow;

pub use flow::run_verb;

/// The whole context of one run. One run reads the config once.
///
/// There is no room for "read on demand": `[discovery]` affects every command, including
/// the ones that look unrelated to plugins.
pub struct Session {
    /// Start directory (what `-C` gave, or cwd).
    pub start_dir: PathBuf,
    /// Global config.
    pub global: GlobalConfig,
    /// Merged project config (several layers of `.pmpx.toml`).
    pub project: MergedProjectConfig,
    /// Manifest list of installed plugins (manifest reads only, no dlopen).
    pub plugins: PluginSet,
    /// Plugin library handle.
    pub kit: CratePluginKit<PmpxPlugin>,
    /// The directories walk-up visited and why it stopped.
    pub walk: Walk,
    /// The project root. Computed in `open` -- it is the single answer to "where do we
    /// run", not recomputed every time.
    pub project_root: Option<PathBuf>,
    /// The value of `-p/--plugin`.
    pub wanted_plugin: Option<String>,
    /// `--quiet`: turns off the notes on stderr.
    pub quiet: bool,
}

impl Session {
    /// Read config, scan plugins, walk up. Selects no plugin and loads no code.
    ///
    /// Every step here is a phase in the `--debug` trace: this is where a run spends the time
    /// that is not the backend's, so it is the part that has to be attributable.
    pub fn open(args: &Cli) -> Result<Self> {
        let t = debug::now();
        let start_dir = resolve_start_dir(args.dir.as_deref())?;
        debug::done("session.start-dir", t, || start_dir.display().to_string());

        let t = debug::now();
        let global = GlobalConfig::load()?;
        debug::done("session.config", t, || "global config.toml");

        // `--no-walk-up` can only tighten `[discovery] walk_up`, never loosen it.
        let mut discovery_cfg = global.discovery.clone();
        if args.no_walk_up {
            discovery_cfg.walk_up = false;
        }

        let data_dir = global.plugin_store.effective_data_dir()?;
        let mut kit_cfg = KitConfig::new("pmpx").with_data_dir(&data_dir);

        // Only a fallback: the wrapper crate-plugin-kit generates repeats whatever the plugin
        // crate itself declares for the contract crate. `KitConfig`'s default here is "0.1",
        // which resolves to nothing while this workspace releases at 0.0.x.
        //
        // Our own version is the right fallback because the workspace carries a single version,
        // so `pmpx` and `pmpx-plugin` are always published as the same number.
        kit_cfg.contract_version = env!("CARGO_PKG_VERSION").to_string();

        kit_cfg.prefer_prebuilt = global.plugin_store.effective_prefer_prebuilt();

        let t = debug::now();
        let kit = CratePluginKit::<PmpxPlugin>::new(kit_cfg).with_context(|| {
            format!(
                "failed to initialise the plugin store: {}",
                data_dir.display()
            )
        })?;
        debug::done("session.store", t, || data_dir.display().to_string());

        let t = debug::now();
        let plugins = PluginSet::load(&kit)?;
        debug::done("session.plugins", t, || {
            format!(
                "{} installed, {} usable",
                plugins.plugins.len(),
                plugins.usable().count()
            )
        });

        // Walk up **once**. The path and the stop reason, the project root, and the config layers
        // are three views of the same traversal, so they are derived here instead of walking
        // again -- that also makes it impossible for them to disagree about where the walk stopped.
        let t = debug::now();
        let walk = discovery::walk(&start_dir, &discovery_cfg);
        debug::done("session.walk", t, || {
            format!(
                "{}, stopped because: {}",
                crate::discovery::dirs(walk.dirs.len()),
                walk.stopped.describe(discovery_cfg.max_depth)
            )
        });

        // The candidates are stat'ed one by one and the search stops at the first hit, so the
        // count is the real cost of this phase -- reporting the walked directories instead
        // would overstate it in a deep tree.
        let t = debug::now();
        let checked = Cell::new(0usize);
        let project_root = walk.project_root(|d| {
            checked.set(checked.get() + 1);
            plugins.marks_root(d)
        });
        debug::done("session.root", t, || match &project_root {
            Some(root) => format!("{} checked -> {}", checked.get(), root.display()),
            None => format!("{} checked -> none", checked.get()),
        });

        let t = debug::now();
        let config_paths = walk.config_paths();
        let project = MergedProjectConfig::from_paths_near_to_far(&config_paths)?;
        debug::done("session.project-conf", t, || {
            format!("{} .pmpx.toml", project.sources.len())
        });

        Ok(Self {
            start_dir,
            global,
            project,
            plugins,
            kit,
            walk,
            project_root,
            wanted_plugin: args.plugin.clone(),
            quiet: args.quiet,
        })
    }

    /// The project root. Not found is not found -- it never falls back to cwd.
    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// The error for "no project detected" (exit code 3).
    ///
    /// The static `hints` table only states facts; it never recommends a plugin to install.
    pub fn no_project_error(&self) -> PmpxError {
        let mut msg = format!("no project type detected in {}.", self.start_dir.display());

        // Where walk-up stopped -- this decides between "searched and found nothing" and
        // "barely searched at all"
        msg.push_str(&format!(
            "\nWalked up {}, stopped because: {}.",
            crate::discovery::dirs(self.walk.dirs.len()),
            self.walk.stopped.describe(self.global.discovery.max_depth)
        ));

        if self.walk.stopped == StopReason::WalkUpDisabled {
            msg.push_str(
                "\n(`--no-walk-up` or the global `[discovery] walk_up = false` turned walk-up off)",
            );
        }

        let installed: Vec<String> = self.plugins.usable().map(|p| p.name.clone()).collect();
        if installed.is_empty() {
            msg.push_str(
                "\nNo plugins are installed. pmpx decides the project type from the marker \
                 files declared by installed plugins, so nothing can be detected right now.",
            );
        } else {
            msg.push_str(&format!("\nInstalled plugins: {}", installed.join(", ")));
        }

        // Only reached when nothing was detected, which is exactly when "how long did the
        // search take" is worth having in the trace.
        let t = debug::now();
        let hints = crate::hints::probe(&self.start_dir);
        debug::done("hints.probe", t, || {
            format!("{} families matched", hints.len())
        });

        if !hints.is_empty() {
            msg.push_str("\n\nThese files look like:");
            for h in &hints {
                msg.push_str(&format!("\n  - {} ({})", h.family, h.matched.join(", ")));
            }
        }

        msg.push_str(
            "\n\nYou can declare it explicitly with a .pmpx.toml in the current directory, \
             for example:\n  [plugin]\n  rust = \"cargo\"",
        );

        PmpxError::not_found(msg)
    }

    /// What a plugin's logging reaches for this run.
    ///
    /// The level is decided once, here: `--quiet` asks for the least, `--debug` for the most, and a
    /// plain run sits between them so that a plugin's warnings still get through while its notes
    /// do not.
    pub fn host_hooks(&self) -> &'static pmpx_plugin::abi::PmpxHost {
        crate::runtime::hooks(self.quiet, debug::enabled())
    }

    /// Resolve which plugin to use.
    pub fn select(&self, root: &Path) -> std::result::Result<Selection, DetectFailure> {
        detect::select(
            &self.plugins,
            root,
            &self.project,
            &self.global,
            self.wanted_plugin.as_deref(),
        )
    }

    /// Resolve from scores the caller already computed ([`detect::score_all`]), instead of scoring
    /// the project a second time in the same run.
    pub fn select_from(
        &self,
        root: &Path,
        families: &BTreeMap<Family, FamilyScore>,
    ) -> std::result::Result<Selection, DetectFailure> {
        detect::select_from_scores(
            &self.plugins,
            root,
            families,
            &self.project,
            &self.global,
            self.wanted_plugin.as_deref(),
        )
    }

    /// Print the notes from the resolution. `--quiet` turns them off.
    pub fn emit_notes(&self, selection: &Selection) {
        if self.quiet {
            return;
        }
        for note in &selection.notes {
            anstream::eprintln!("{}", style::paint(style::DIM, format!("pmpx: {note}")));
        }
        if !selection.notes.is_empty() {
            anstream::eprintln!(
                "{}",
                style::paint(
                    style::DIM,
                    format!(
                        "pmpx: override it for one run with `pmpx -p <name>`, or pin it in \
                         .pmpx.toml with `pmpx plugin set {}`",
                        selection.name
                    )
                )
            );
        }
    }

    /// Load the selected plugin.
    ///
    /// `dlopen` and the ABI checks happen here, so this is the phase that turns "which plugin"
    /// into "code in this process" -- worth telling apart from detection in the trace.
    pub fn load_backend(&self, selection: &Selection) -> crate::error::Result<Backend> {
        let Some(plugin) = self.plugins.by_crate_name(&selection.crate_name) else {
            return Err(PmpxError::not_found(format!(
                "plugin {} is not in the list (deleted between the two reads?)",
                selection.crate_name
            )));
        };

        let t = debug::now();
        let backend = Backend::load(plugin, self.host_hooks());
        debug::done("backend.load", t, || match &backend {
            Ok(_) => plugin.crate_name.clone(),
            Err(e) => format!("{} FAILED: {e}", plugin.crate_name),
        });

        backend
    }
}

/// Compute the start directory.
///
/// The directory given by `-C` must exist, otherwise it is a clear usage error -- quietly
/// falling back to cwd would make "I ran the command in the wrong directory" hard to notice.
fn resolve_start_dir(dir: Option<&Path>) -> Result<PathBuf> {
    match dir {
        Some(d) => {
            let abs = if d.is_absolute() {
                d.to_path_buf()
            } else {
                std::env::current_dir()?.join(d)
            };
            if !abs.is_dir() {
                anyhow::bail!(
                    "the directory given by -C does not exist: {}",
                    abs.display()
                );
            }
            Ok(abs)
        }
        None => Ok(std::env::current_dir()?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_start_dir_rejects_a_missing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");

        let err = resolve_start_dir(Some(&missing)).unwrap_err();
        assert!(err.to_string().contains("-C"), "{err}");
    }

    #[test]
    fn resolve_start_dir_accepts_an_existing_one() {
        let tmp = tempfile::tempdir().unwrap();
        let got = resolve_start_dir(Some(tmp.path())).unwrap();
        assert_eq!(got, tmp.path());
    }

    #[test]
    fn resolve_start_dir_makes_relative_paths_absolute() {
        let got = resolve_start_dir(Some(Path::new("."))).unwrap();
        assert!(got.is_absolute());
    }
}
