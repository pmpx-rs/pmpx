//! A plugin that is only ever installed from its own directory, by the end-to-end test.
//!
//! It exists to prove the development loop: a checkout with no published version anywhere is built,
//! installed, selected by real detection, and asked for a command.

use std::ffi::OsString;

use pmpx_plugin::{CommandSpec, Context, Family, PackageManager, PluginError, Verb};

struct Local;

impl PackageManager for Local {
    fn name(&self) -> &str {
        "local"
    }

    fn family(&self) -> Family {
        Family::NODE
    }

    fn command(
        &self,
        ctx: &Context,
        verb: Verb,
        args: &[OsString],
    ) -> Result<CommandSpec, PluginError> {
        match verb {
            Verb::Install | Verb::Build | Verb::Test | Verb::Run | Verb::Update => {
                let probe = format!(
                    "pmpx-local-probe root={} matched={} verb={} args={} declared={}",
                    ctx.project_root.display(),
                    ctx.matched.join("|"),
                    verb,
                    args.iter()
                        .map(|a| a.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join(","),
                    ctx.file_str("local.json").unwrap_or_else(|| "none".to_string()),
                );

                #[cfg(windows)]
                let spec = CommandSpec::new("cmd").arg("/c").arg("echo").arg(probe);
                #[cfg(not(windows))]
                let spec = CommandSpec::new("echo").arg(probe);

                Ok(spec)
            }
            other => Err(PluginError::other(format!("not implemented: {other}"))),
        }
    }
}

/// Factory function. The wrapper that an install generates calls `export!` on it — a plugin crate
/// itself is a plain rlib with no exported symbol of its own, which is what lets `cargo test` use it
/// directly.
pub fn create() -> Box<dyn PackageManager> {
    Box::new(Local)
}