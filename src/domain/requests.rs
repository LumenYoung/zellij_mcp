use serde::{Deserialize, Serialize};

use crate::domain::status::{CaptureMode, SpawnTarget};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpawnRequest {
    pub session_name: String,
    pub target: SpawnTarget,
    pub tab_name: Option<String>,
    pub cwd: Option<String>,
    pub command: String,
    pub title: Option<String>,
    pub wait_ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachRequest {
    pub session_name: String,
    pub tab_name: Option<String>,
    pub selector: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendRequest {
    pub handle: String,
    pub text: String,
    pub submit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WaitRequest {
    pub handle: String,
    pub idle_ms: u64,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureRequest {
    pub handle: String,
    pub mode: CaptureMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloseRequest {
    pub handle: String,
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ListRequest {
    pub session_name: Option<String>,
}
