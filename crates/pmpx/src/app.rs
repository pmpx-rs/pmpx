//! Wires the layers together: the context of one run, and the whole flow from argv to
//! "run one command".

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use crate_plugin_kit::{CratePluginKit, KitConfig};
use pmpx_plugin::abi::PmpxPluginV1;
use pmpx_plugin::Verb;

use crate::cli::Cli;
use crate::config::{GlobalConfig, MergedProjectConfig};
use crate::detect::{self, DetectFailure, ScoredPlugin, Selection};
use crate::discovery::{self, StopReason, Walk};
use crate::error::PmpxError;
use crate::plugins::PluginSet;
use crate::runtime::{Backend, BackendError};
use crate::spawn;
use crate::style;

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
    pub kit: CratePluginKit<PmpxPluginV1>,
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
    pub fn open(args: &Cli) -> Result<Self> {
        let start_dir = resolve_start_dir(args.dir.as_deref())?;
        let global = GlobalConfig::load()?;

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
        let kit = CratePluginKit::<PmpxPluginV1>::new(kit_cfg).with_context(|| {
            format!(
                "failed to initialise the plugin store: {}",
                data_dir.display()
            )
        })?;

        let plugins = PluginSet::load(&kit)?;

        let project_root =
            discovery::find_project_root(&start_dir, &discovery_cfg, |d| plugins.marks_root(d));
        let config_paths = discovery::collect_config_paths(&start_dir, &discovery_cfg);
        let project = MergedProjectConfig::from_paths_near_to_far(&config_paths)?;

        // Walk-up path and stop reason -- used by `info` and by "why was nothing found"
        let walk = discovery::walk(&start_dir, &discovery_cfg);

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

        let hints = crate::hints::probe(&self.start_dir);
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

    /// The files the selected plugin declares, matched in the project root -- exactly what
    /// is passed as `Context::matched`.
    ///
    /// Relative to `project_root`, sorted and deduplicated.
    pub fn matched_for(&self, root: &Path, selection: &Selection) -> Vec<String> {
        let Some(plugin) = self.plugins.by_crate_name(&selection.crate_name) else {
            return Vec::new();
        };
        ScoredPlugin::score(plugin, root)
            .all_hits()
            .map(str::to_string)
            .collect()
    }

    /// Load the selected plugin.
    pub fn load_backend(&self, selection: &Selection) -> crate::error::Result<Backend> {
        let Some(plugin) = self.plugins.by_crate_name(&selection.crate_name) else {
            return Err(PmpxError::not_found(format!(
                "plugin {} is not in the list (deleted between the two reads?)",
                selection.crate_name
            )));
        };
        Backend::load(&self.kit, plugin)
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

/// Run one verb all the way: resolve -> load -> ask the plugin -> spawn -> pass the exit
/// code through.
///
/// `allow_exec_fallback` is the single exception: it is true only for `exec`, where an
/// unsupported verb degrades into pmpx passing the command through verbatim itself; the
/// other six verbs keep "unsupported is an error".
pub fn run_verb(
    session: &Session,
    verb: Verb,
    args: &[OsString],
    allow_exec_fallback: bool,
) -> crate::error::Result<u8> {
    let root = match session.project_root() {
        Some(r) => r.to_path_buf(),
        None => {
            // `exec` is the escape hatch: it has to work with zero plugins too. With no
            // project root it falls back to the start directory.
            if allow_exec_fallback {
                let cwd = session.start_dir.clone();
                return passthrough(&cwd, args, session.quiet);
            }
            return Err(session.no_project_error());
        }
    };

    let selection = match session.select(&root) {
        Ok(s) => s,
        Err(failure) => {
            if allow_exec_fallback {
                return passthrough(&root, args, session.quiet);
            }
            // Every `DetectFailure` is exit code 3
            return Err(failure.into());
        }
    };

    session.emit_notes(&selection);

    let backend = session.load_backend(&selection)?;
    let matched = session.matched_for(&root, &selection);

    match backend.command(&root, &matched, verb, args) {
        Ok(Ok(spec)) => {
            let cwd = spec.cwd.clone().unwrap_or_else(|| root.clone());
            announce(session.quiet, &spec);
            spawn::run(&spec, &cwd)
        }

        Ok(Err(BackendError::UnsupportedVerb)) if allow_exec_fallback => {
            passthrough(&root, args, session.quiet)
        }

        Ok(Err(e)) => Err(PmpxError::Backend(
            format!("{} cannot do `{verb}`: {e}", selection.name),
            e.exit_code(),
        )),

        Err(e) => Err(e),
    }
}

/// Pass through verbatim: run the command the user gave, cwd = project root.
///
/// This is the one exception to "unsupported is an error" and happens only for `exec`.
fn passthrough(cwd: &Path, args: &[OsString], quiet: bool) -> crate::error::Result<u8> {
    let Some((program, rest)) = args.split_first() else {
        return Err(PmpxError::Usage(
            "`pmpx exec` needs a command, for example `pmpx exec ls`".to_string(),
        ));
    };

    let spec = pmpx_plugin::CommandSpec {
        program: program.clone(),
        args: rest.to_vec(),
        cwd: Some(cwd.to_path_buf()),
    };
    announce(quiet, &spec);
    spawn::run(&spec, cwd)
}

/// Show the command that is about to run, unless `--quiet` is set.
fn announce(quiet: bool, spec: &pmpx_plugin::CommandSpec) {
    if quiet {
        return;
    }
    spawn::announce(spec);
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
