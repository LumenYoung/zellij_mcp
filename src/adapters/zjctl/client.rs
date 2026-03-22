use chrono::{DateTime, Utc};

use crate::adapters::zjctl::AdapterError;
use crate::domain::requests::{AttachRequest, SpawnRequest, WaitRequest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub selector: String,
    pub pane_id: Option<String>,
    pub session_name: String,
    pub tab_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSnapshot {
    pub content: String,
    pub captured_at: DateTime<Utc>,
    pub truncated: bool,
}

pub trait ZjctlAdapter {
    fn is_available(&self) -> bool;
    fn spawn(&self, request: &SpawnRequest) -> Result<ResolvedTarget, AdapterError>;
    fn resolve_selector(&self, request: &AttachRequest) -> Result<ResolvedTarget, AdapterError>;
    fn send_input(&self, handle: &str, text: &str, submit: bool) -> Result<(), AdapterError>;
    fn wait_idle(&self, request: &WaitRequest) -> Result<(), AdapterError>;
    fn capture_full(&self, handle: &str) -> Result<CaptureSnapshot, AdapterError>;
    fn close(&self, handle: &str, force: bool) -> Result<(), AdapterError>;
    fn list_targets(&self) -> Result<Vec<ResolvedTarget>, AdapterError>;
}

#[derive(Debug, Default, Clone)]
pub struct ZjctlClient;

impl ZjctlAdapter for ZjctlClient {
    fn is_available(&self) -> bool {
        false
    }

    fn spawn(&self, _request: &SpawnRequest) -> Result<ResolvedTarget, AdapterError> {
        Err(AdapterError::Unimplemented)
    }

    fn resolve_selector(&self, _request: &AttachRequest) -> Result<ResolvedTarget, AdapterError> {
        Err(AdapterError::Unimplemented)
    }

    fn send_input(&self, _handle: &str, _text: &str, _submit: bool) -> Result<(), AdapterError> {
        Err(AdapterError::Unimplemented)
    }

    fn wait_idle(&self, _request: &WaitRequest) -> Result<(), AdapterError> {
        Err(AdapterError::Unimplemented)
    }

    fn capture_full(&self, _handle: &str) -> Result<CaptureSnapshot, AdapterError> {
        Err(AdapterError::Unimplemented)
    }

    fn close(&self, _handle: &str, _force: bool) -> Result<(), AdapterError> {
        Err(AdapterError::Unimplemented)
    }

    fn list_targets(&self) -> Result<Vec<ResolvedTarget>, AdapterError> {
        Err(AdapterError::Unimplemented)
    }
}
