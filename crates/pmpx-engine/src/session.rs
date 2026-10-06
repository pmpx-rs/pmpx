//! One run's state: the configuration, the project, and the plugins that are installed.
//!
//! Everything here is read **once**, at the start: `[discovery]` decides how the project is found, which
//! decides which `.pmpx.toml` layers apply, which decides which plugins can answer. Reading any of it on
//! demand would make the order of reads observable in the result.
//!
//! Nothing here prints and nothing here exits: the phases of the setup are reported as [`Event`]s, and
//! every failure is a value. The two argv facts it needs -- the start directory and the contract version
//! to ask the store for -- arrive as [`Options`], because a library that parsed arguments would drag
//! `clap` into every embedder.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate_plugin_kit::{CratePluginKit, KitConfig};
use pmpx_detect::{DetectFailure, FamilyScore, Selection};
use pmpx_plugin::abi::PmpxPlugin;
use pmpx_project::{GlobalConfig, MergedProjectConfig};

use crate::discovery::{self, StopReason, Walk};
use crate::error::Result;
use crate::log::Levels;
use crate::store::{InstalledPlugin, PluginSet};
use crate::{EngineError, Event};

/// What the command line said, as this crate needs it.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The directory `-C` named, if any. `None` means the process's own directory.
    pub start_dir: Option<PathBuf>,
    /// `--no-walk-up`: only the start directory is considered a project root.
    pub no_walk_up: bool,
    /// `-p/--plugin`: name a plugin outright, skipping detection.
    pub wanted_plugin: Option<String>,
    /// `--quiet`: no notes on stderr.
    pub quiet: bool,
    /// `--debug`: whether the trace is on, which also decides how loud a plugin may be.
    pub trace: bool,
    /// The contract version to ask the store for, which is the host's own version.
    pub contract_version: String,
}

/// The whole context of one run.
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
    /// The project root. Computed in `open` -- it is the single answer to "where do we run", not
    /// recomputed every time.
    pub project_root: Option<PathBuf>,
    /// The value of `-p/--plugin`.
    pub wanted_plugin: Option<String>,
    /// `--quiet`: turns off the notes on stderr.
    pub quiet: bool,
    /// `--debug`: whether the trace is on.
    trace: bool,
}

impl Session {
    /// Read config, scan plugins, walk up. Selects no plugin and loads no code.
    ///
    /// Every step here is a phase of the trace: this is where a run spends the time that is not the
    /// backend's, so it is the part that has to be attributable.
    pub fn open(options: Options, events: &mut dyn FnMut(Event)) -> Result<Self> {
        let start_dir = phase(
            events,
            "session.start-dir",
            || resolve_start_dir(options.start_dir.as_deref()),
            |dir| dir.display().to_string(),
        )?;

        let global = phase(
            events,
            "session.config",
            || {
                GlobalConfig::load().map_err(|error| {
                    EngineError::Setup(format!("failed to read the config: {error}"))
                })
            },
            |_| "global config.toml".to_string(),
        )?;

        // `--no-walk-up` can only tighten `[discovery] walk_up`, never loosen it.
        let mut discovery_cfg = global.discovery.clone();
        if options.no_walk_up {
            discovery_cfg.walk_up = false;
        }

        let data_dir = global.plugin_store.effective_data_dir().map_err(|error| {
            EngineError::Setup(format!("cannot decide the plugin directory: {error}"))
        })?;

        let mut kit_cfg = KitConfig::new("pmpx").with_data_dir(&data_dir);

        // Only a fallback: the wrapper crate-plugin-kit generates repeats whatever the plugin crate
        // itself declares for the contract crate. `KitConfig`'s default here is "0.1", which resolves to
        // nothing while this workspace releases at 0.0.x.
        //
        // The host's own version is the right fallback because the workspace carries a single version, so
        // the CLI and the contract are always published as the same number.
        kit_cfg.contract_version = options.contract_version;

        // The kit's defaults are derived from the id and say `_v1`; this host speaks v3, and the symbol
        // is what a plugin exports. The wrapper body needs no change: the derived default already asks
        // the contract crate for `export!`.
        kit_cfg.entry_symbol = pmpx_plugin::abi::PMPX_ENTRY_SYMBOL.as_bytes().to_vec();

        kit_cfg.prefer_prebuilt = global.plugin_store.effective_prefer_prebuilt();

        let kit = phase(
            events,
            "session.store",
            || {
                CratePluginKit::<PmpxPlugin>::new(kit_cfg).map_err(|error| {
                    EngineError::Setup(format!(
                        "failed to initialise the plugin store: {}\n{error}",
                        data_dir.display()
                    ))
                })
            },
            |_| data_dir.display().to_string(),
        )?;

        let plugins = phase(
            events,
            "session.plugins",
            || {
                PluginSet::load(&kit).map_err(|error| {
                    EngineError::Setup(format!("failed to read the installed plugins: {error}"))
                })
            },
            |set| {
                format!(
                    "{} installed, {} usable",
                    set.plugins.len(),
                    set.usable().count()
                )
            },
        )?;

        // Walk up **once**. The path and the stop reason, the project root, and the config layers are
        // three views of the same traversal, so they are derived here instead of walking again -- that
        // also makes it impossible for them to disagree about where the walk stopped.
        let walk = phase(
            events,
            "session.walk",
            || Ok(discovery::walk(&start_dir, &discovery_cfg)),
            |walk| {
                format!(
                    "{}, stopped because: {}",
                    discovery::dirs(walk.dirs.len()),
                    walk.stopped.describe(discovery_cfg.max_depth)
                )
            },
        )?;

        // The candidates are stat'ed one by one and the search stops at the first hit, so the count is
        // the real cost of this phase -- reporting the walked directories instead would overstate it in a
        // deep tree.
        let checked = Cell::new(0usize);
        let project_root = phase(
            events,
            "session.root",
            || {
                Ok(walk.project_root(|dir| {
                    checked.set(checked.get() + 1);
                    plugins.marks_root(dir)
                }))
            },
            |root| match root {
                Some(root) => format!("{} checked -> {}", checked.get(), root.display()),
                None => format!("{} checked -> none", checked.get()),
            },
        )?;

        let project = phase(
            events,
            "session.project-conf",
            || {
                MergedProjectConfig::from_paths_near_to_far(&walk.config_paths()).map_err(|error| {
                    EngineError::Setup(format!("failed to read the project config: {error}"))
                })
            },
            |project| format!("{} .pmpx.toml", project.sources.len()),
        )?;

        Ok(Self {
            start_dir,
            global,
            project,
            plugins,
            kit,
            walk,
            project_root,
            wanted_plugin: options.wanted_plugin,
            quiet: options.quiet,
            trace: options.trace,
        })
    }

    /// The project root. Not found is not found -- it never falls back to cwd.
    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// The error for "no project detected" (exit code 3).
    ///
    /// The advice names the **installed** plugins and the markers they declared. There is deliberately no
    /// built-in table of "these files look like Rust/Node/…": that would be ecosystem knowledge the plugin
    /// model exists to avoid, and it would duplicate what the plugins already declare.
    pub fn no_project_error(&self) -> EngineError {
        let mut msg = format!("no project type detected in {}.", self.start_dir.display());

        // Where walk-up stopped -- this decides between "searched and found nothing" and "barely searched
        // at all".
        msg.push_str(&format!(
            "\nWalked up {}, stopped because: {}.",
            discovery::dirs(self.walk.dirs.len()),
            self.walk.stopped.describe(self.global.discovery.max_depth)
        ));

        if self.walk.stopped == StopReason::WalkUpDisabled {
            msg.push_str(
                "\n(`--no-walk-up` or the global `[discovery] walk_up = false` turned walk-up off)",
            );
        }

        let installed: Vec<String> = self
            .plugins
            .usable()
            .map(|p| p.name().to_string())
            .collect();
        if installed.is_empty() {
            msg.push_str(
                "\nNo plugins are installed. pmpx decides the project type from the marker files \
                 declared by installed plugins, so nothing can be detected right now.",
            );
        } else {
            msg.push_str(&format!("\nInstalled plugins: {}", installed.join(", ")));

            let mut markers: Vec<&str> = self
                .plugins
                .detect_names()
                .iter()
                .map(String::as_str)
                .collect();
            markers.sort_unstable();
            markers.dedup();
            msg.push_str(&format!(
                "\nThey look for these files: {}",
                markers.join(", ")
            ));
        }

        msg.push_str(
            "\n\nYou can declare it explicitly with a .pmpx.toml in the current directory, for \
             example:\n  [plugin]\n  rust = \"cargo\"",
        );

        EngineError::NoProject(msg)
    }

    /// How much of a plugin's own output this run wants.
    pub fn levels(&self) -> Levels {
        Levels {
            quiet: self.quiet,
            trace: self.trace,
        }
    }

    /// Resolve which plugin to use, scoring the project.
    pub fn select(
        &self,
        root: &Path,
        events: &mut dyn FnMut(Event),
    ) -> std::result::Result<Selection, DetectFailure> {
        crate::detect::select(
            &self.plugins,
            root,
            &self.project,
            &self.global,
            self.wanted_plugin.as_deref(),
            events,
        )
    }

    /// Resolve from scores the caller already computed.
    pub fn select_from(
        &self,
        root: &Path,
        families: &std::collections::BTreeMap<String, FamilyScore>,
        events: &mut dyn FnMut(Event),
    ) -> std::result::Result<Selection, DetectFailure> {
        crate::detect::select_from_scores(
            &self.plugins,
            root,
            families,
            &self.project,
            &self.global,
            self.wanted_plugin.as_deref(),
            events,
        )
    }

    /// Load the selected plugin.
    ///
    /// `dlopen` and the ABI checks happen here, so this is the phase that turns "which plugin" into "code
    /// in this process".
    pub fn load_backend(
        &self,
        selection: &Selection,
        events: &mut dyn FnMut(Event),
    ) -> Result<crate::Backend> {
        let Some(plugin) = self.plugins.by_crate_name(&selection.crate_name) else {
            return Err(EngineError::Setup(format!(
                "plugin {} is not in the list (deleted between the two reads?)",
                selection.crate_name
            )));
        };

        let started = Instant::now();
        let loaded = self.load_plugin(plugin, events);
        events(Event::Phase {
            name: "backend.load",
            micros: started.elapsed().as_micros(),
            detail: match &loaded {
                Ok(_) => plugin.crate_name().to_string(),
                Err(error) => format!("{} FAILED: {error}", plugin.crate_name()),
            },
        });

        loaded
    }

    /// Load one installed plugin, whatever the reason: a verb, `plugin info`, or a store listing its
    /// diagnostics.
    ///
    /// The store knows how a plugin's directory is laid out, so finding the library is this side's job;
    /// everything past that -- opening it, checking what it says about itself, installing the hooks -- is
    /// [`crate::Backend`]'s.
    pub fn load_plugin(
        &self,
        plugin: &InstalledPlugin,
        events: &mut dyn FnMut(Event),
    ) -> Result<crate::Backend> {
        let identity = crate::PluginIdentity {
            name: plugin.name().to_string(),
            crate_name: plugin.crate_name().to_string(),
            dir: plugin.dir().to_path_buf(),
            wanted: plugin.wanted.clone(),
            declared_abi: plugin.abi(),
        };

        // The library file lives in the plugin's own directory, named after its crate with the platform's
        // conventions -- `find_library` knows the last part of that.
        let stem = plugin.crate_name().replace('-', "_");
        let library = crate_plugin_kit::find_library(plugin.dir(), &stem).map_err(|error| {
            EngineError::Setup(format!(
                "cannot find the plugin library for {} in {}: {error}",
                plugin.crate_name(),
                plugin.dir().display()
            ))
        })?;

        crate::Backend::load(&identity, &library, self.levels(), events)
    }
}

/// Run `f`, reporting it as one named phase with the detail `describe` derives from its result.
///
/// A phase that fails is reported too: "the store took 3ms and then failed" is exactly what someone
/// reading a trace wants to know.
fn phase<T, D>(
    events: &mut dyn FnMut(Event),
    name: &'static str,
    f: impl FnOnce() -> Result<T>,
    describe: D,
) -> Result<T>
where
    D: FnOnce(&T) -> String,
{
    let started = Instant::now();
    let out = f();
    events(Event::Phase {
        name,
        micros: started.elapsed().as_micros(),
        detail: match &out {
            Ok(value) => describe(value),
            Err(error) => error.message(),
        },
    });
    out
}

/// The directory a run starts in.
///
/// `-C` must exist, otherwise it is a clear setup error -- quietly falling back to cwd would make "I ran
/// the command in the wrong directory" hard to notice.
///
/// Normalized by [`discovery::normalize`], **not** `canonicalize`: `std::fs::canonicalize` -- and
/// `std::path::absolute` with it -- returns a Windows verbatim path (`\\?\C:\…`), which then travels into
/// the context a plugin is handed, and tools that take a path as an argument (cargo, `cmd`, the package
/// managers themselves) either refuse that form or read it differently. Resolving symlinks is not this
/// layer's business either: the person asked for the directory they named.
fn resolve_start_dir(dir: Option<&Path>) -> Result<PathBuf> {
    match dir {
        Some(dir) => {
            if !dir.is_dir() {
                return Err(EngineError::Usage(format!(
                    "cannot use {} as the directory: it is not a directory",
                    dir.display()
                )));
            }
            Ok(discovery::normalize(dir))
        }
        None => std::env::current_dir().map_err(|error| {
            EngineError::Setup(format!("cannot read the current directory: {error}"))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_start_dir_rejects_a_missing_directory() {
        let error = resolve_start_dir(Some(Path::new("definitely/not/here"))).unwrap_err();

        assert!(
            error.message().contains("definitely"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn resolve_start_dir_accepts_an_existing_one() {
        // A directory named the way a person names one -- `tempfile` is not used here because its
        // paths already carry the Windows verbatim prefix, which would hide what this asserts.
        let dir = std::env::temp_dir().join(format!("pmpx-start-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let resolved = resolve_start_dir(Some(&dir)).unwrap();

        assert!(resolved.is_absolute());
        assert_eq!(
            resolved.file_name(),
            dir.file_name(),
            "a plain path stays the path it was: {resolved:?}"
        );
        assert!(
            !resolved.to_string_lossy().starts_with(r"\\?\"),
            "pmpx must never introduce a Windows verbatim path: {resolved:?}"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
