mod client;
mod commands;
mod errors;
mod parser;

pub use client::{
    CaptureSnapshot, ResolvedTarget, SshBackendReadiness, SshReadinessFailure, SshTargetConfig,
    SshZjctlClient, ZjctlAdapter, ZjctlClient, attempt_safe_ssh_readiness_remediation,
    classify_ssh_backend_readiness, is_helper_client_missing_message,
    is_plugin_permission_prompt, is_protocol_version_mismatch_message, is_rpc_not_ready_message,
    missing_binary_name, resolve_ssh_runtime_config,
};
pub use errors::AdapterError;
