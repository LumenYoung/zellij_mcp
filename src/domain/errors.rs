use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidArgument,
    HandleNotFound,
    AliasNotFound,
    SelectorNotUnique,
    TargetNotFound,
    TargetStale,
    SpawnFailed,
    AttachFailed,
    SendFailed,
    WaitTimeout,
    WaitFailed,
    CaptureFailed,
    CloseFailed,
    ZjctlUnavailable,
    PluginNotReady,
    PersistenceError,
}

#[derive(Debug, Error, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[error("{code:?}: {message}")]
pub struct DomainError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl DomainError {
    pub fn new(code: ErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}
