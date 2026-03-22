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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureResult {
    pub handle: String,
    pub mode: String,
    pub content: String,
    pub truncated: bool,
    pub captured_at: DateTime<Utc>,
    pub baseline: Option<String>,
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
}
