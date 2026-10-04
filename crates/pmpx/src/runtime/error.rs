//! A failure the plugin reported explicitly, as opposed to one of pmpx's own.

/// A failure the plugin reported explicitly.
///
/// **Kept separate from [`PmpxError`](crate::error::PmpxError)** because the meaning differs:
/// `PmpxError` is "something went wrong on pmpx's side", this is "the plugin answered normally
/// that it cannot do this".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// The plugin does not support this verb.
    ///
    /// `pmpx exec` degrades to passing through verbatim when it gets this; the other six
    /// verbs keep "unsupported is an error".
    UnsupportedVerb,

    /// The plugin says the arguments are wrong.
    InvalidArgs(String),

    /// The plugin failed internally, or it panicked.
    Internal(String),
}

impl BackendError {
    /// The matching process exit code.
    pub fn exit_code(&self) -> u8 {
        match self {
            // "this backend cannot do what you asked" -- same code as a usage error
            BackendError::UnsupportedVerb | BackendError::InvalidArgs(_) => {
                crate::error::EXIT_USAGE
            }
            // the plugin blew up -- counted as a pmpx error of its own
            BackendError::Internal(_) => crate::error::EXIT_INTERNAL,
        }
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::UnsupportedVerb => f.write_str("this backend does not support that verb"),
            BackendError::InvalidArgs(m) => write!(f, "the backend rejected the arguments: {m}"),
            BackendError::Internal(m) => write!(f, "the backend failed internally: {m}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_error_exit_codes_match_the_table() {
        assert_eq!(
            BackendError::UnsupportedVerb.exit_code(),
            crate::error::EXIT_USAGE
        );
        assert_eq!(
            BackendError::InvalidArgs("x".into()).exit_code(),
            crate::error::EXIT_USAGE
        );
        assert_eq!(
            BackendError::Internal("x".into()).exit_code(),
            crate::error::EXIT_INTERNAL
        );
    }

    #[test]
    fn backend_error_messages_name_the_backend_situation() {
        assert!(BackendError::UnsupportedVerb
            .to_string()
            .contains("does not support"));
        assert!(BackendError::InvalidArgs("a".into())
            .to_string()
            .contains('a'));
        assert!(BackendError::Internal("b".into()).to_string().contains('b'));
    }
}
