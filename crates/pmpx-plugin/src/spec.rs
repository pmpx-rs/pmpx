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
    /// Executable. It may be a bare name (resolved on `PATH`), an absolute path, or a path
    /// relative to this spec's [`cwd`](CommandSpec::cwd) -- which is the project root when the
    /// spec does not name one. On Windows a bare name resolves through `PATHEXT`, so `pnpm` finds
    /// `pnpm.cmd`, and the host starts a batch file the way the platform requires.
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
