use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

static DAEMON_IDENTITY: OnceLock<DaemonIdentity> = OnceLock::new();

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DaemonIdentity {
    pub package: String,
    pub version: String,
    pub build_stamp: String,
    pub instance_id: String,
    pub process_id: u32,
    pub started_at: DateTime<Utc>,
}

pub fn daemon_identity() -> &'static DaemonIdentity {
    DAEMON_IDENTITY.get_or_init(|| DaemonIdentity {
        package: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        build_stamp: option_env!("ZELLIJ_MCP_BUILD_STAMP")
            .or(option_env!("VERGEN_BUILD_TIMESTAMP"))
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .to_string(),
        instance_id: format!("zmd_{}", uuid::Uuid::new_v4().simple()),
        process_id: std::process::id(),
        started_at: Utc::now(),
    })
}

pub fn daemon_identity_json() -> Value {
    let identity = daemon_identity();
    json!({
        "package": identity.package,
        "version": identity.version,
        "build_stamp": identity.build_stamp,
        "instance_id": identity.instance_id,
        "process_id": identity.process_id,
        "started_at": identity.started_at,
    })
}
