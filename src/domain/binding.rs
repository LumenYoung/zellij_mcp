use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::status::{BindingSource, TerminalStatus};

fn default_target_id() -> String {
    "local".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalBinding {
    pub handle: String,
    #[serde(default = "default_target_id")]
    pub target_id: String,
    pub alias: Option<String>,
    pub session_name: String,
    pub tab_name: Option<String>,
    pub selector: String,
    pub pane_id: Option<String>,
    pub cwd: Option<String>,
    pub launch_command: Option<String>,
    pub source: BindingSource,
    pub status: TerminalStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
