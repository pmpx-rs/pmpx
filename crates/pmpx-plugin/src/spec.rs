//! What a plugin answers with: one command, still unexecuted.

use std::ffi::OsString;
use std::path::PathBuf;

/// One command waiting to be executed, pure data -- a plugin only describes "what to run" and the
/// actual spawn is done by the host, so stdio, environment, and exit-code handling have exactly
/// one implementation. Construction is a consuming chain (`self` -> `Self`), so it can produce a
/// value directly:
/// ```ignore
/// let spec = CommandSpec::new("cargo").arg("add").args(args.iter()).cwd("/somewhere");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Executable. The host resolves it to a real path with `which` and decides per platform
    /// whether to wrap it in `cmd /C` (on Windows `pnpm` is really `pnpm.cmd`, and spawning it
    /// directly fails).
    pub program: OsString,

    /// Arguments, in order.
    pub args: Vec<OsString>,

    /// Working-directory override. `None` = use the project root given by the host.
    pub cwd: Option<PathBuf>,
}

impl CommandSpec {
    /// Specify the executable.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
        }
    }

    /// Append one argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append a batch of arguments; `args.iter()` can be passed straight in and stays lossless
    /// (`&OsString: Into<OsString>`), unlike `String`, which would corrupt non-UTF-8 arguments on
    /// Unix.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Override the working directory.
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }
}
