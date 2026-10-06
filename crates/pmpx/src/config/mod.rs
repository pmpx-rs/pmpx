//! pmpx's own configuration, as this host uses it.
//!
//! The files, the merge rule and the writing live in `pmpx-project`; this module is the name the rest
//! of the CLI reaches them by, so that "where does a pin come from" has exactly one answer. Only what
//! the host actually names is re-exported -- the crate has the rest.

pub use pmpx_project::{
    atomic_write, global_config_path, DiscoveryConfig, GlobalConfig, MergedProjectConfig,
    ProjectConfig,
};
