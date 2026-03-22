use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("zjctl command execution is not implemented yet")]
    Unimplemented,
    #[error("zjctl is not available on PATH")]
    ZjctlUnavailable,
    #[error("zjctl command failed: {0}")]
    CommandFailed(String),
    #[error("zjctl output could not be parsed: {0}")]
    ParseError(String),
    #[error("zjctl command timed out")]
    Timeout,
}
