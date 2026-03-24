use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TerminalObservation {
    pub handle: String,
    pub last_full_content: Option<String>,
    pub last_full_hash: Option<String>,
    pub last_capture_at: Option<DateTime<Utc>>,
    pub command_boundary_content: Option<String>,
    pub command_boundary_hash: Option<String>,
    pub command_boundary_at: Option<DateTime<Utc>>,
    pub interaction_id: Option<String>,
    pub interaction_started_at: Option<DateTime<Utc>>,
    pub interaction_completed_at: Option<DateTime<Utc>>,
    pub interaction_exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureResult {
    pub handle: String,
    pub mode: String,
    pub content: String,
    pub tail_lines: Option<usize>,
    pub line_offset: Option<usize>,
    pub line_limit: Option<usize>,
    pub line_window_applied: bool,
    pub next_cursor: Option<String>,
    pub ansi_normalized: bool,
    pub truncated: bool,
    pub captured_at: DateTime<Utc>,
    pub baseline: Option<String>,
    pub interaction_id: Option<String>,
    pub interaction_completed: Option<bool>,
    pub interaction_exit_code: Option<i32>,
}

impl TerminalObservation {
    pub fn update_full_snapshot(&mut self, content: String, hash: String, now: DateTime<Utc>) {
        self.last_full_content = Some(content);
        self.last_full_hash = Some(hash);
        self.last_capture_at = Some(now);
    }

    pub fn reset_command_boundary(&mut self) {
        self.command_boundary_content = self.last_full_content.clone();
        self.command_boundary_hash = self.last_full_hash.clone();
        self.command_boundary_at = self.last_capture_at;
    }

    pub fn start_interaction(&mut self, interaction_id: String, now: DateTime<Utc>) {
        self.interaction_id = Some(interaction_id);
        self.interaction_started_at = Some(now);
        self.interaction_completed_at = None;
        self.interaction_exit_code = None;
    }

    pub fn complete_interaction(&mut self, exit_code: Option<i32>, now: DateTime<Utc>) {
        self.interaction_completed_at = Some(now);
        self.interaction_exit_code = exit_code;
    }

    pub fn clear_interaction(&mut self) {
        self.interaction_id = None;
        self.interaction_started_at = None;
        self.interaction_completed_at = None;
        self.interaction_exit_code = None;
    }
}
