use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdapterError {
    #[error("backend command execution is not implemented yet")]
    Unimplemented,
    #[error("required backend binaries are not available on PATH")]
    ZjctlUnavailable,
    #[error("backend command failed: {0}")]
    CommandFailed(String),
    #[error("backend output could not be parsed: {0}")]
    ParseError(String),
    #[error("backend target selection is ambiguous: {0}")]
    AmbiguousTarget(String),
    #[error("backend command timed out")]
    Timeout,
}
