use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::binding::TerminalBinding;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpawnResponse {
    pub handle: String,
    pub session_name: String,
    pub tab_name: Option<String>,
    pub selector: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachResponse {
    pub handle: String,
    pub attached: bool,
    pub baseline_established: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WaitResponse {
    pub handle: String,
    pub status: String,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListResponse {
    pub bindings: Vec<TerminalBinding>,
}
