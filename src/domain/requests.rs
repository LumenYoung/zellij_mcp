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
    /// SSH target; omit for local backend.
    #[serde(default)]
    pub target: Option<String>,
    /// Zellij session name on the selected backend.
    pub session_name: String,
    /// Where to create the new pane: a new tab or an existing tab.
    #[serde(default = "default_spawn_target")]
    pub spawn_target: SpawnTarget,
    pub tab_name: Option<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub argv: Option<Vec<String>>,
    pub title: Option<String>,
    pub wait_ready: bool,
}

impl SpawnRequest {
    pub fn launch_command_summary(&self) -> Option<String> {
        self.command
            .clone()
            .or_else(|| self.argv.as_ref().map(|argv| argv.join(" ")))
            .or_else(|| Some("fish".to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct AttachRequest {
    /// SSH target; omit for local backend.
    #[serde(default)]
    pub target: Option<String>,
    /// Zellij session name on the selected backend.
    pub session_name: String,
    pub tab_name: Option<String>,
    #[serde(default)]
    pub selector: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct DiscoverRequest {
    /// SSH target; omit for local backend.
    #[serde(default)]
    pub target: Option<String>,
    /// Zellij session name on the selected backend.
    pub session_name: String,
    pub tab_name: Option<String>,
    /// Optional pane selector filter used before attach.
    pub selector: Option<String>,
    #[serde(default = "default_true")]
    pub include_preview: bool,
    pub preview_lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct SendRequest {
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub handle: String,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default)]
    pub tab_name: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
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
    /// SSH target; omit for local backend.
    #[serde(default)]
    pub target: Option<String>,
    /// Zellij session name on the selected backend.
    pub session_name: String,
    pub tab_name: Option<String>,
    /// Optional exact pane selector to narrow takeover to one pane.
    pub selector: Option<String>,
    pub command_contains: Option<String>,
    #[serde(default)]
    pub focused: Option<bool>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct ReplaceRequest {
    /// Daemon-managed shell-like pane handle to reuse.
    pub handle: String,
    pub command: String,
    #[serde(default = "default_true")]
    pub interrupt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct CleanupRequest {
    /// SSH target; omit for local backend.
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
    /// SSH target; omit for local backend.
    #[serde(default)]
    pub target: Option<String>,
    /// Zellij session name on the selected backend.
    pub session_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct WaitRequest {
    /// Daemon-managed pane handle from spawn, attach, or takeover.
    pub handle: String,
    pub idle_ms: u64,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct CaptureRequest {
    /// Daemon-managed pane handle from spawn, attach, or takeover.
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
    /// Daemon-managed pane handle from spawn, attach, or takeover.
    pub handle: String,
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, schemars::JsonSchema)]
pub struct ListRequest {
    /// SSH target; omit for local backend.
    #[serde(default)]
    pub target: Option<String>,
    /// Optional Zellij session filter on the selected backend.
    pub session_name: Option<String>,
}

const fn default_true() -> bool {
    true
}

const fn default_spawn_target() -> SpawnTarget {
    SpawnTarget::ExistingTab
}
