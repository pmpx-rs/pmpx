//! Test helpers for plugin authors.
//!
//! A plugin's [`command`](pmpx_plugin::PackageManager::command) is a pure function: it is handed a
//! [`Context`](pmpx_plugin::Context) and answers with a command. That makes it testable without a
//! project, a host, or a process -- and this crate is the two ways of doing it:
//!
//! - **By hand, fast.** [`context`] is the builder, re-exported so a test needs one import. Build the
//!   exact context a case is about and assert the answer.
//! - **For real.** [`Fixture`] writes a directory of files, runs the **real** detection over them (the
//!   same [`pmpx-detect`](pmpx_detect) the host uses, with the markers your manifest declares), and hands
//!   you the context your plugin would have been called with. This is what catches "my plugin stopped
//!   matching this project".
//!
//! And one thing a plugin cannot do alone: [`capture`] installs a fake host, so a test can assert the
//! lines the plugin wrote with [`debug!`](macro@pmpx_plugin::debug) -- which is the only channel its text has.
//!
//! ```no_run
//! use pmpx_testkit::{capture, context, Fixture};
//! # use pmpx_plugin::{CommandSpec, Context, Family, PackageManager, PluginError, Verb};
//! # struct Mine;
//! # impl PackageManager for Mine {
//! #     fn name(&self) -> &str { "mine" }
//! #     fn family(&self) -> Family { Family::NODE }
//! #     fn command(&self, ctx: &Context, verb: Verb, args: &[std::ffi::OsString]) -> Result<CommandSpec, PluginError> {
//! #         Ok(CommandSpec::new("tool").args(args.iter()))
//! #     }
//! # }
//! // The case itself: two matched files, one of them read.
//! let ctx = context()
//!     .project_root("/work")
//!     .matched(["pnpm-lock.yaml"])
//!     .file("package.json", "{\"name\":\"x\"}")
//!     .build();
//! let plan = Mine.command(&ctx, Verb::Install, &[]).unwrap();
//! assert_eq!(plan.program, "tool");
//!
//! // The same case, from a directory and the real detection.
//! let fixture = Fixture::new()
//!     .file("pnpm-lock.yaml", "")
//!     .plugin("mine", "node", &["pnpm-lock.yaml"], &["package.json"])
//!     .declares(["package.json"])
//!     .file("package.json", "{\"name\":\"x\"}");
//! let ctx = fixture.context();
//! assert!(ctx.has_matched("pnpm-lock.yaml"));
//!
//! // And what the plugin says while it runs.
//! let host = capture();
//! let out = Mine.command(&ctx, Verb::Install, &[]);
//! assert!(host.take().is_empty(), "this plugin is quiet");
//! ```

mod fixture;
mod host;

pub use fixture::Fixture;
pub use host::{capture, Captured};

/// The context builder, for a test that only needs a [`Context`](pmpx_plugin::Context) by hand.
///
/// The same builder the host's own tests use: there is exactly one way to build a context, and a plugin
/// author should not have to learn a second one.
pub fn context() -> pmpx_plugin::ContextBuilder {
    pmpx_plugin::Context::builder()
}
