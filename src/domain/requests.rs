use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::domain::status::{CaptureMode, SpawnTarget};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    Raw,
    SubmitLine,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct SpawnRequest {
    #[serde(default)]
    pub target: Option<String>,
    pub session_name: String,
    pub spawn_target: SpawnTarget,
    pub tab_name: Option<String>,
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub argv: Option<Vec<String>>,
    pub title: Option<String>,
    pub wait_ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct AttachRequest {
    #[serde(default)]
    pub target: Option<String>,
    pub session_name: String,
    pub tab_name: Option<String>,
    pub selector: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct DiscoverRequest {
    #[serde(default)]
    pub target: Option<String>,
    pub session_name: String,
    pub tab_name: Option<String>,
    pub selector: Option<String>,
    #[serde(default = "default_true")]
    pub include_preview: bool,
    pub preview_lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct SendRequest {
    pub handle: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub input_mode: Option<InputMode>,
    pub submit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct TakeoverRequest {
    #[serde(default)]
    pub target: Option<String>,
    pub session_name: String,
    pub tab_name: Option<String>,
    pub selector: Option<String>,
    pub command_contains: Option<String>,
    #[serde(default)]
    pub focused: Option<bool>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct ReplaceRequest {
    pub handle: String,
    pub command: String,
    #[serde(default = "default_true")]
    pub interrupt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct CleanupRequest {
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub statuses: Vec<crate::domain::status::TerminalStatus>,
    pub max_age_ms: Option<u64>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct LayoutRequest {
    #[serde(default)]
    pub target: Option<String>,
    pub session_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct WaitRequest {
    pub handle: String,
    pub idle_ms: u64,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct CaptureRequest {
    pub handle: String,
    pub mode: CaptureMode,
    pub tail_lines: Option<usize>,
    pub line_offset: Option<usize>,
    pub line_limit: Option<usize>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub normalize_ansi: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct CloseRequest {
    pub handle: String,
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, schemars::JsonSchema)]
pub struct ListRequest {
    #[serde(default)]
    pub target: Option<String>,
    pub session_name: Option<String>,
}

const fn default_true() -> bool {
    true
}
