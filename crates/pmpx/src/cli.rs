//! The command-line surface.
//!
//! This file only parses: it turns `argv` into a struct. All semantics (detection,
//! resolution, spawn) live elsewhere.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Global arguments. They apply to every subcommand, hence `global`.
#[derive(Debug, Parser)]
#[command(
    name = "pmpx",
    version,
    about = "One command surface: detect the project type and forward to the real tool",
    long_about = None,
    arg_required_else_help = false,
)]
pub struct Cli {
    /// Use this plugin for now, overriding `.pmpx.toml`
    ///
    /// A one-off override that is not written to disk. To pin it, use `pmpx plugin set`.
    #[arg(short = 'p', long = "plugin", global = true, value_name = "NAME")]
    pub plugin: Option<String>,

    /// Operate in this directory (same as cd'ing there first)
    #[arg(short = 'C', long = "dir", global = true, value_name = "PATH")]
    pub dir: Option<PathBuf>,

    /// Only check the current directory, do not walk up to the project root
    #[arg(long = "no-walk-up", global = true)]
    pub no_walk_up: bool,

    /// Suppress hints and the resolved command on stderr (ambiguous detection, uninstalled
    /// candidates, ...)
    #[arg(short = 'q', long = "quiet", global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// The subcommand table.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install dependencies. No arguments = install the whole tree; with arguments = add
    /// them
    #[command(visible_aliases = ["i", "add"])]
    Install {
        /// Dependencies to add; empty means install the whole tree
        #[arg(value_name = "PKG")]
        packages: Vec<OsString>,
    },

    /// Remove dependencies
    #[command(visible_aliases = ["rm", "uninstall"])]
    Remove {
        /// Dependencies to remove
        #[arg(value_name = "PKG", required = true)]
        packages: Vec<OsString>,
    },

    /// Run a script / target
    #[command(visible_alias = "r")]
    Run {
        /// Script name / target name. Usually empty under cargo
        #[arg(value_name = "TARGET")]
        target: Option<OsString>,

        /// What follows `--` is passed to the backend verbatim, with no interpretation
        #[arg(last = true, value_name = "ARG")]
        args: Vec<OsString>,
    },

    /// Build
    #[command(visible_alias = "b")]
    Build {
        /// What follows `--` is passed to the backend verbatim
        #[arg(last = true, value_name = "ARG")]
        args: Vec<OsString>,
    },

    /// Test
    #[command(visible_alias = "t")]
    Test {
        /// What follows `--` is passed to the backend verbatim
        #[arg(last = true, value_name = "ARG")]
        args: Vec<OsString>,
    },

    /// Update dependencies
    #[command(visible_alias = "up")]
    Update {
        /// Only update these dependencies; empty means all of them
        #[arg(value_name = "PKG")]
        packages: Vec<OsString>,
    },

    /// Escape hatch: run any command. When the plugin does not support it, pmpx passes it through verbatim itself
    #[command(visible_alias = "x")]
    Exec {
        /// The command and its arguments
        #[arg(
            value_name = "CMD",
            required = true,
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        command: Vec<OsString>,
    },

    /// Verbose: project root, candidates and scores, plugin versions, ABI diagnostics
    Info,

    /// Manage plugins
    #[command(subcommand)]
    Plugin(PluginCommand),

    /// Read and write the global config
    #[command(subcommand)]
    Config(ConfigCommand),

    /// Print a shell completion script to stdout; redirect it yourself
    Completion {
        /// Target shell
        #[arg(value_name = "SHELL")]
        shell: clap_complete::Shell,
    },

    /// Manage pmpx itself
    #[command(name = "self")]
    SelfTool {
        #[command(subcommand)]
        command: SelfCommand,
    },
}

/// `pmpx self <...>`
#[derive(Debug, Subcommand)]
pub enum SelfCommand {
    /// Replace this binary with one of the project's releases
    ///
    /// Only for installations that are not managed by cargo; a `cargo install` is told to
    /// run `cargo install pmpx --force` instead.
    Update {
        /// Report what is available and change nothing
        #[arg(long)]
        check: bool,

        /// Install this version instead of the newest one; also how a rollback is done
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,

        /// Reinstall even when this version is already the one running
        #[arg(long)]
        force: bool,
    },
}

/// `pmpx plugin <...>`
#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    /// List installed plugins grouped by family (reads manifests only, no dlopen)
    #[command(visible_alias = "list")]
    Ls {
        /// Flat output, no grouping
        #[arg(long)]
        flat: bool,
    },

    /// The current plugin per family, plus candidates and scores
    Current,

    /// Pin a plugin in `.pmpx.toml` (only affects the family it belongs to)
    Set {
        /// Plugin name, for example `pnpm`
        #[arg(value_name = "NAME")]
        name: String,
    },

    /// Delete a pin from `.pmpx.toml`
    Unset {
        /// Only delete this family; empty means all of them (which then requires `--yes`)
        #[arg(value_name = "FAMILY")]
        family: Option<String>,

        /// Confirm deleting all of them
        #[arg(long)]
        yes: bool,
    },

    /// Install plugins
    Add {
        /// Plugin name, for example `pnpm` (expands to `pmpx-plugin-pnpm`)
        #[arg(value_name = "NAME", required = true)]
        names: Vec<String>,

        /// A specific version; empty means latest
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
    },

    /// Remove plugins
    Rm {
        /// Plugin name
        #[arg(value_name = "NAME", required = true)]
        names: Vec<String>,
    },

    /// Update plugins
    Update {
        /// Only update this plugin; empty means all of them
        #[arg(value_name = "NAME")]
        names: Vec<String>,

        /// A specific version; only valid together with a single plugin name
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
    },

    /// Search crates.io for plugins
    Search {
        /// Keyword
        #[arg(value_name = "KEYWORD", required = true)]
        keyword: String,

        /// Show at most this many
        #[arg(long, default_value_t = 20, value_name = "N")]
        limit: usize,
    },

    /// Show a plugin's crates.io information
    Info {
        /// Plugin name
        #[arg(value_name = "NAME")]
        name: String,
    },
}

/// `pmpx config <...>`
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Read one key. Dotted paths work, for example `plugin.family_priority`
    Get {
        /// Key name
        #[arg(value_name = "KEY")]
        key: String,
    },

    /// Write one key. Only writes the global config, never a project `.pmpx.toml`
    Set {
        /// Key name
        #[arg(value_name = "KEY")]
        key: String,

        /// Value. Parsed according to its existing type (array / boolean / number / string)
        #[arg(value_name = "VALUE")]
        value: String,
    },
}
