use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::binding::TerminalBinding;
use crate::domain::observation::CaptureResult;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpawnResponse {
    pub handle: String,
    pub target_id: String,
    pub session_name: String,
    pub tab_name: Option<String>,
    pub selector: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachResponse {
    pub handle: String,
    pub target_id: String,
    pub attached: bool,
    pub baseline_established: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoverResponse {
    pub candidates: Vec<DiscoverCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoverCandidate {
    pub target_id: String,
    pub selector: String,
    pub pane_id: Option<String>,
    pub session_name: String,
    pub tab_name: Option<String>,
    pub title: Option<String>,
    pub command: Option<String>,
    pub focused: bool,
    pub preview: Option<String>,
    pub preview_basis: Option<String>,
    pub captured_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WaitResponse {
    pub handle: String,
    pub status: String,
    pub observed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_basis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_completed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListResponse {
    pub bindings: Vec<TerminalBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureResponse {
    #[serde(flatten)]
    pub capture: CaptureResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendResponse {
    pub handle: String,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TakeoverResponse {
    pub handle: String,
    pub target_id: String,
    pub attached: bool,
    pub baseline_established: bool,
    pub matched_selector: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplaceResponse {
    pub handle: String,
    pub replaced: bool,
    pub interaction_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CleanupResponse {
    pub removed_handles: Vec<String>,
    pub removed_count: usize,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayoutResponse {
    pub target_id: String,
    pub session_name: String,
    pub tabs: Vec<LayoutTab>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayoutTab {
    pub tab_name: String,
    pub panes: Vec<DiscoverCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloseResponse {
    pub handle: String,
    pub closed: bool,
}
