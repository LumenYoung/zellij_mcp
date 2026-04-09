use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;
use zjctl_proto::{PROTOCOL_VERSION, RpcRequest, RpcResponse, methods};

use crate::adapters::zjctl::AdapterError;
use crate::domain::requests::{AttachRequest, SpawnRequest};
use crate::domain::status::SpawnTarget;

use super::commands::ZjctlCommand;
use super::parser::{parse_capture_output, parse_list_output, parse_spawn_output};

fn quote_posix_sh(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

const RPC_PIPE_NAME: &str = "zjctl-rpc";
const REMOTE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const REMOTE_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const HELPER_CLIENT_COLS: usize = 160;
const HELPER_CLIENT_ROWS: usize = 48;

fn parse_helper_geometry_value(value: Option<String>) -> Option<usize> {
    value
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn helper_client_geometry_from_sources(
    explicit_cols: Option<usize>,
    explicit_rows: Option<usize>,
    terminal_cols: Option<usize>,
    terminal_rows: Option<usize>,
) -> (usize, usize) {
    let cols = explicit_cols
        .or_else(|| terminal_cols.map(|value| value.max(HELPER_CLIENT_COLS)))
        .unwrap_or(HELPER_CLIENT_COLS);
    let rows = explicit_rows
        .or_else(|| terminal_rows.map(|value| value.max(HELPER_CLIENT_ROWS)))
        .unwrap_or(HELPER_CLIENT_ROWS);
    (cols, rows)
}

fn helper_client_geometry() -> (usize, usize) {
    helper_client_geometry_from_sources(
        parse_helper_geometry_value(std::env::var("ZELLIJ_MCP_HELPER_COLS").ok()),
        parse_helper_geometry_value(std::env::var("ZELLIJ_MCP_HELPER_ROWS").ok()),
        parse_helper_geometry_value(std::env::var("COLUMNS").ok()),
        parse_helper_geometry_value(std::env::var("LINES").ok()),
    )
}

fn wait_for_output_with_timeout(
    mut child: Child,
    timeout: Duration,
) -> Result<Output, AdapterError> {
    let start = std::time::Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|_| AdapterError::ZjctlUnavailable)?
            .is_some()
        {
            return child
                .wait_with_output()
                .map_err(|_| AdapterError::ZjctlUnavailable);
        }

        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AdapterError::Timeout);
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct PaneInfo {
    id: String,
    pane_type: String,
    title: String,
    command: Option<String>,
    tab_index: usize,
    tab_name: String,
    focused: bool,
    floating: bool,
    suppressed: bool,
    #[serde(default)]
    rows: usize,
    #[serde(default)]
    cols: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct TabInfo {
    position: usize,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub selector: String,
    pub pane_id: Option<String>,
    pub session_name: String,
    pub tab_name: Option<String>,
    pub title: Option<String>,
    pub command: Option<String>,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSnapshot {
    pub content: String,
    pub captured_at: DateTime<Utc>,
    pub truncated: bool,
}

pub trait BackendAdapter {
    fn is_available(&self) -> bool;
    fn ensure_session_ready(&self, session_name: &str) -> Result<(), AdapterError>;
    fn spawn(&self, request: &SpawnRequest) -> Result<ResolvedTarget, AdapterError>;
    fn launch_spawn(&self, request: &SpawnRequest) -> Result<Option<ResolvedTarget>, AdapterError> {
        self.spawn(request).map(Some)
    }
    fn resolve_selector(&self, request: &AttachRequest) -> Result<ResolvedTarget, AdapterError>;
    fn list_targets_in_session(
        &self,
        session_name: &str,
    ) -> Result<Vec<ResolvedTarget>, AdapterError>;
    fn send_input(
        &self,
        session_name: &str,
        handle: &str,
        text: &str,
        submit: bool,
    ) -> Result<(), AdapterError>;
    fn wait_idle(
        &self,
        session_name: &str,
        handle: &str,
        idle_ms: u64,
        timeout_ms: u64,
    ) -> Result<(), AdapterError>;
    fn capture_full(
        &self,
        session_name: &str,
        handle: &str,
    ) -> Result<CaptureSnapshot, AdapterError>;
    fn close(&self, session_name: &str, handle: &str, force: bool) -> Result<(), AdapterError>;
    fn list_targets(&self) -> Result<Vec<ResolvedTarget>, AdapterError>;
}

#[derive(Debug, Clone)]
pub struct LocalBackend;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SshTargetConfig {
    pub host: String,
    pub remote_zjctl_bin: String,
    pub remote_zellij_bin: String,
    #[serde(default)]
    pub remote_env: BTreeMap<String, String>,
    #[serde(default)]
    pub ssh_options: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SshBackend {
    config: SshTargetConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshBackendReadiness {
    Ready(SshTargetConfig),
    AutoFixable(SshReadinessFailure),
    ManualActionRequired(SshReadinessFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshReadinessFailure {
    MissingBinary { host: String, binary: String },
    SshUnreachable { host: String, detail: String },
    MissingPlugin { host: String, detail: String },
    PluginPermissionPrompt { host: String, detail: String },
    HelperClientMissing { host: String, detail: String },
    RpcNotReady { host: String, detail: String },
    ProtocolVersionMismatch { host: String, detail: String },
}

trait RemoteRemediationRunner {
    fn run(&self, config: &SshTargetConfig, remote_command: &str) -> Result<(), AdapterError>;
}

struct SshRemoteRemediationRunner;

impl RemoteRemediationRunner for SshRemoteRemediationRunner {
    fn run(&self, config: &SshTargetConfig, remote_command: &str) -> Result<(), AdapterError> {
        let output = run_remote_command_output(config, remote_command)?;
        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("command exited with status {}", output.status)
        } else {
            stderr
        };
        Err(AdapterError::CommandFailed(message))
    }
}

trait RemoteProbe {
    fn discover_home(&self) -> Result<String, AdapterError>;
    fn discover_path(&self) -> Result<String, AdapterError>;
    fn command_v(
        &self,
        normalized_path: &str,
        binary: &str,
    ) -> Result<Option<String>, AdapterError>;
    fn is_executable(&self, absolute_path: &str) -> Result<bool, AdapterError>;
    fn check_rpc_readiness(
        &self,
        config: &SshTargetConfig,
        session_name: &str,
    ) -> Result<(), AdapterError>;
}

struct SshRemoteProbe<'a> {
    config: &'a SshTargetConfig,
}

impl<'a> SshRemoteProbe<'a> {
    fn new(config: &'a SshTargetConfig) -> Self {
        Self { config }
    }
}

impl RemoteProbe for SshRemoteProbe<'_> {
    fn discover_home(&self) -> Result<String, AdapterError> {
        let stdout = run_remote_command_string(self.config, r#"printf %s "$HOME""#)?;
        let home = String::from_utf8_lossy(&stdout).trim().to_string();
        if home.is_empty() {
            return Err(AdapterError::CommandFailed(
                "failed to determine remote home directory".to_string(),
            ));
        }

        Ok(home)
    }

    fn discover_path(&self) -> Result<String, AdapterError> {
        let stdout = run_remote_command_string(self.config, r#"printf %s "$PATH""#)?;
        Ok(String::from_utf8_lossy(&stdout).trim().to_string())
    }

    fn command_v(
        &self,
        normalized_path: &str,
        binary: &str,
    ) -> Result<Option<String>, AdapterError> {
        let command = format!(
            "PATH={}; command -v {}",
            quote_posix_sh(normalized_path),
            quote_posix_sh(binary)
        );
        let output = run_remote_command_output(self.config, &command)?;
        if !output.status.success() {
            return Ok(None);
        }

        let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if resolved.is_empty() {
            Ok(None)
        } else {
            Ok(Some(resolved))
        }
    }

    fn is_executable(&self, absolute_path: &str) -> Result<bool, AdapterError> {
        let command = format!("[ -x {} ]", quote_posix_sh(absolute_path));
        let output = run_remote_command_output(self.config, &command)?;
        Ok(output.status.success())
    }

    fn check_rpc_readiness(
        &self,
        config: &SshTargetConfig,
        session_name: &str,
    ) -> Result<(), AdapterError> {
        let client = SshBackend::new(config.clone());
        client.list_targets_for_session(session_name).map(|_| ())
    }
}

pub fn classify_ssh_backend_readiness(
    config: &SshTargetConfig,
    session_name: &str,
) -> SshBackendReadiness {
    classify_ssh_backend_readiness_with_probe(config, session_name, &SshRemoteProbe::new(config))
}

pub fn attempt_safe_ssh_readiness_remediation(
    config: &SshTargetConfig,
    session_name: &str,
    failure: &SshReadinessFailure,
) -> bool {
    attempt_safe_ssh_readiness_remediation_with_probe_and_runner(
        config,
        session_name,
        failure,
        &SshRemoteProbe::new(config),
        &SshRemoteRemediationRunner,
    )
}

fn classify_ssh_backend_readiness_with_probe(
    config: &SshTargetConfig,
    session_name: &str,
    probe: &dyn RemoteProbe,
) -> SshBackendReadiness {
    match resolve_ssh_runtime_config_with_probe(config, probe) {
        Ok(resolved) => match probe.check_rpc_readiness(&resolved, session_name) {
            Ok(()) => SshBackendReadiness::Ready(resolved),
            Err(error) => classify_ssh_readiness_failure(&resolved.host, error),
        },
        Err(error) => classify_ssh_readiness_failure(&config.host, error),
    }
}

fn attempt_safe_ssh_readiness_remediation_with_probe_and_runner(
    config: &SshTargetConfig,
    session_name: &str,
    failure: &SshReadinessFailure,
    probe: &dyn RemoteProbe,
    runner: &dyn RemoteRemediationRunner,
) -> bool {
    match failure {
        SshReadinessFailure::MissingBinary { .. }
        | SshReadinessFailure::HelperClientMissing { .. }
        | SshReadinessFailure::RpcNotReady { .. } => {}
        SshReadinessFailure::SshUnreachable { .. }
        | SshReadinessFailure::MissingPlugin { .. }
        | SshReadinessFailure::PluginPermissionPrompt { .. }
        | SshReadinessFailure::ProtocolVersionMismatch { .. } => return false,
    }

    let context = SshReadinessRemediationContext::discover(config, session_name, probe);
    let mut remediated = false;

    if let (Some(tmux_bin), Some(zellij_bin)) =
        (context.tmux_bin.as_deref(), context.zellij_bin.as_deref())
    {
        remediated |= start_helper_client(config, &context, tmux_bin, zellij_bin, runner);
        remediated |= launch_zrpc_plugin(config, &context, zellij_bin, runner);
    } else if let Some(zellij_bin) = context.zellij_bin.as_deref() {
        remediated |= launch_zrpc_plugin(config, &context, zellij_bin, runner);
    }

    remediated
}

#[derive(Debug, Clone, Default)]
struct SshReadinessRemediationContext {
    home: Option<String>,
    normalized_path: Option<String>,
    zellij_bin: Option<String>,
    tmux_bin: Option<String>,
    session_name: String,
    helper_session_name: String,
}

impl SshReadinessRemediationContext {
    fn discover(config: &SshTargetConfig, session_name: &str, probe: &dyn RemoteProbe) -> Self {
        let home = probe.discover_home().ok();
        let configured_path = config.remote_env.get("PATH").cloned();
        let normalized_path = match (configured_path, home.as_deref()) {
            (Some(path), Some(home)) => {
                Some(prepend_path_entry(&path, &format!("{home}/.local/bin")))
            }
            (Some(path), None) => Some(path),
            (None, Some(home)) => probe
                .discover_path()
                .ok()
                .map(|path| prepend_path_entry(&path, &format!("{home}/.local/bin"))),
            (None, None) => probe.discover_path().ok(),
        };

        let zellij_bin = resolve_existing_remote_binary(
            &config.remote_zellij_bin,
            normalized_path.as_deref(),
            home.as_deref(),
            probe,
        );
        let tmux_bin = resolve_existing_remote_binary(
            "tmux",
            normalized_path.as_deref(),
            home.as_deref(),
            probe,
        );
        let session_name = session_name.trim().to_string();
        let helper_session_name = format!("zellij-mcp-client-{session_name}");

        Self {
            home,
            normalized_path,
            zellij_bin,
            tmux_bin,
            session_name,
            helper_session_name,
        }
    }

    fn env_assignments(
        &self,
        config: &SshTargetConfig,
        include_session: bool,
    ) -> Vec<(String, String)> {
        let mut assignments: Vec<(String, String)> = config
            .remote_env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        if let Some(path) = self.normalized_path.as_ref() {
            assignments.retain(|(key, _)| key != "PATH");
            assignments.push(("PATH".to_string(), path.clone()));
        }

        assignments.retain(|(key, _)| key != "ZELLIJ_SESSION_NAME");

        if include_session {
            assignments.push(("ZELLIJ_SESSION_NAME".to_string(), self.session_name.clone()));
        }

        assignments
    }

    fn plugin_path(&self, config: &SshTargetConfig) -> String {
        if let Some(xdg_config_home) = config.remote_env.get("XDG_CONFIG_HOME") {
            return format!("{xdg_config_home}/zellij/plugins/zrpc.wasm");
        }

        self.home
            .as_ref()
            .map(|home| format!("{home}/.config/zellij/plugins/zrpc.wasm"))
            .unwrap_or_else(|| "$HOME/.config/zellij/plugins/zrpc.wasm".to_string())
    }
}

fn resolve_existing_remote_binary(
    configured: &str,
    normalized_path: Option<&str>,
    home: Option<&str>,
    probe: &dyn RemoteProbe,
) -> Option<String> {
    if configured.trim().is_empty() {
        return None;
    }

    if !binary_needs_probe(configured) {
        return probe
            .is_executable(configured)
            .ok()
            .and_then(|is_executable| is_executable.then(|| configured.to_string()));
    }

    if let Some(normalized_path) = normalized_path
        && let Ok(Some(resolved)) = probe.command_v(normalized_path, configured)
    {
        return Some(resolved);
    }

    home.and_then(|home| {
        common_binary_fallbacks(home, configured)
            .into_iter()
            .find(|candidate| probe.is_executable(candidate).unwrap_or(false))
    })
}

fn build_remote_exec_env_command(
    binary: &str,
    env_assignments: &[(String, String)],
    args: &[String],
) -> String {
    let mut command = String::from("exec env");
    for (key, value) in env_assignments {
        command.push(' ');
        command.push_str(&quote_posix_sh(&format!("{key}={value}")));
    }
    command.push(' ');
    command.push_str(&quote_posix_sh(binary));
    for arg in args {
        command.push(' ');
        command.push_str(&quote_posix_sh(arg));
    }
    command
}

fn launch_zrpc_plugin(
    config: &SshTargetConfig,
    context: &SshReadinessRemediationContext,
    zellij_bin: &str,
    runner: &dyn RemoteRemediationRunner,
) -> bool {
    let command = build_remote_exec_env_command(
        zellij_bin,
        &context.env_assignments(config, true),
        &[
            "action".to_string(),
            "launch-plugin".to_string(),
            format!("file:{}", context.plugin_path(config)),
        ],
    );
    runner.run(config, &command).is_ok()
}

fn start_helper_client(
    config: &SshTargetConfig,
    context: &SshReadinessRemediationContext,
    tmux_bin: &str,
    zellij_bin: &str,
    runner: &dyn RemoteRemediationRunner,
) -> bool {
    let (helper_cols, helper_rows) = helper_client_geometry();
    let attach_command = build_remote_exec_env_command(
        zellij_bin,
        &context.env_assignments(config, false),
        &["attach".to_string(), context.session_name.clone()],
    );
    let command = format!(
        "if {tmux} has-session -t {helper} >/dev/null 2>&1; then exit 0; fi; {tmux} new-session -d -x {cols} -y {rows} -s {helper} {attach}",
        cols = helper_cols,
        rows = helper_rows,
        tmux = quote_posix_sh(tmux_bin),
        helper = quote_posix_sh(&context.helper_session_name),
        attach = quote_posix_sh(&attach_command),
    );
    runner.run(config, &command).is_ok()
}

fn classify_ssh_readiness_failure(host: &str, error: AdapterError) -> SshBackendReadiness {
    match error {
        AdapterError::ParseError(message) => {
            SshBackendReadiness::ManualActionRequired(SshReadinessFailure::RpcNotReady {
                host: host.to_string(),
                detail: message,
            })
        }
        AdapterError::AmbiguousTarget(message) => {
            SshBackendReadiness::ManualActionRequired(SshReadinessFailure::RpcNotReady {
                host: host.to_string(),
                detail: message,
            })
        }
        AdapterError::ZjctlUnavailable => {
            SshBackendReadiness::ManualActionRequired(SshReadinessFailure::SshUnreachable {
                host: host.to_string(),
                detail: "failed to launch SSH transport".to_string(),
            })
        }
        AdapterError::Timeout => {
            SshBackendReadiness::AutoFixable(SshReadinessFailure::RpcNotReady {
                host: host.to_string(),
                detail: "zjctl RPC readiness check timed out".to_string(),
            })
        }
        AdapterError::CommandFailed(message) => {
            if let Some(binary) = missing_binary_name(&message) {
                return SshBackendReadiness::AutoFixable(SshReadinessFailure::MissingBinary {
                    host: host.to_string(),
                    binary,
                });
            }

            if is_missing_plugin_message(&message) {
                return SshBackendReadiness::ManualActionRequired(
                    SshReadinessFailure::MissingPlugin {
                        host: host.to_string(),
                        detail: message,
                    },
                );
            }

            if is_plugin_permission_prompt(&message) {
                return SshBackendReadiness::ManualActionRequired(
                    SshReadinessFailure::PluginPermissionPrompt {
                        host: host.to_string(),
                        detail: message,
                    },
                );
            }

            if is_helper_client_missing_message(&message) {
                return SshBackendReadiness::AutoFixable(
                    SshReadinessFailure::HelperClientMissing {
                        host: host.to_string(),
                        detail: message,
                    },
                );
            }

            if is_protocol_version_mismatch_message(&message) {
                return SshBackendReadiness::ManualActionRequired(
                    SshReadinessFailure::ProtocolVersionMismatch {
                        host: host.to_string(),
                        detail: message,
                    },
                );
            }

            if is_rpc_not_ready_message(&message) {
                return SshBackendReadiness::AutoFixable(SshReadinessFailure::RpcNotReady {
                    host: host.to_string(),
                    detail: message,
                });
            }

            SshBackendReadiness::ManualActionRequired(SshReadinessFailure::SshUnreachable {
                host: host.to_string(),
                detail: message,
            })
        }
        AdapterError::Unimplemented => {
            SshBackendReadiness::ManualActionRequired(SshReadinessFailure::RpcNotReady {
                host: host.to_string(),
                detail: "zjctl readiness probe is not implemented".to_string(),
            })
        }
    }
}

fn readiness_failure_to_adapter_error(failure: SshReadinessFailure) -> AdapterError {
    match failure {
        SshReadinessFailure::MissingBinary { binary, .. } => AdapterError::CommandFailed(format!(
            "remote binary `{binary}` was not found after probing PATH and common locations"
        )),
        SshReadinessFailure::SshUnreachable { detail, .. }
        | SshReadinessFailure::MissingPlugin { detail, .. }
        | SshReadinessFailure::PluginPermissionPrompt { detail, .. }
        | SshReadinessFailure::HelperClientMissing { detail, .. }
        | SshReadinessFailure::RpcNotReady { detail, .. }
        | SshReadinessFailure::ProtocolVersionMismatch { detail, .. } => {
            AdapterError::CommandFailed(detail)
        }
    }
}

pub fn missing_binary_name(message: &str) -> Option<String> {
    let prefix = "remote binary `";
    let suffix = "` was not found after probing PATH and common locations";
    message
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .map(ToString::to_string)
}

pub fn is_plugin_permission_prompt(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    (lower.contains("plugin") || lower.contains("zjctl"))
        && (lower.contains("permission") || lower.contains("approve") || lower.contains("allow"))
}

pub fn is_missing_plugin_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("zrpc plugin not found at")
        || (lower.contains("plugin not found at") && lower.contains("zrpc"))
}

pub fn is_rpc_not_ready_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("rpc")
        || lower.contains("zrpc")
        || lower.contains("no response from plugin")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("not ready")
        || lower.contains("no active session")
        || lower.contains("helper")
        || lower.contains("client")
}

pub fn is_protocol_version_mismatch_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("zrpc protocol version mismatch")
        || (lower.contains("protocol version mismatch")
            && lower.contains("expected")
            && lower.contains("got"))
}

pub fn is_helper_client_missing_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    (lower.contains("helper") && lower.contains("client"))
        || lower.contains("not attached yet")
        || lower.contains("no active clients")
        || lower.contains("no active session")
}

pub fn resolve_ssh_runtime_config(
    config: &SshTargetConfig,
) -> Result<SshTargetConfig, AdapterError> {
    resolve_ssh_runtime_config_with_probe(config, &SshRemoteProbe::new(config))
}

fn resolve_ssh_runtime_config_with_probe(
    config: &SshTargetConfig,
    probe: &dyn RemoteProbe,
) -> Result<SshTargetConfig, AdapterError> {
    let needs_zellij_probe = binary_needs_probe(&config.remote_zellij_bin);

    if !needs_zellij_probe {
        return Ok(config.clone());
    }

    let home = probe.discover_home()?;
    let normalized_path = normalize_remote_path(
        config
            .remote_env
            .get("PATH")
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| probe.discover_path()),
        &home,
    )?;

    let mut resolved = config.clone();
    resolved
        .remote_env
        .insert("PATH".to_string(), normalized_path.clone());

    resolved.remote_zellij_bin =
        resolve_remote_binary(&config.remote_zellij_bin, &home, &normalized_path, probe)?;
    if let Some(remote_zjctl_bin) = resolve_existing_remote_binary(
        &config.remote_zjctl_bin,
        Some(&normalized_path),
        Some(&home),
        probe,
    ) {
        resolved.remote_zjctl_bin = remote_zjctl_bin;
    }

    Ok(resolved)
}

fn binary_needs_probe(binary: &str) -> bool {
    !binary.trim().is_empty() && !binary.contains('/')
}

fn normalize_remote_path(
    base_path: Result<String, AdapterError>,
    home: &str,
) -> Result<String, AdapterError> {
    let local_bin = format!("{home}/.local/bin");
    Ok(prepend_path_entry(&base_path?, &local_bin))
}

fn prepend_path_entry(path: &str, entry: &str) -> String {
    let mut segments: Vec<&str> = path
        .split(':')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.first().copied() == Some(entry) || segments.contains(&entry) {
        return segments.join(":");
    }

    segments.insert(0, entry);
    segments.join(":")
}

fn augment_path_with_binary_dirs(path: Option<String>, binaries: &[&str]) -> Option<String> {
    let mut combined = path.unwrap_or_default();
    let mut added_any = false;

    for binary in binaries {
        let binary = binary.trim();
        if binary.is_empty() || !binary.contains('/') {
            continue;
        }

        let Some(parent) = Path::new(binary).parent() else {
            continue;
        };
        let parent = parent.to_string_lossy();
        if parent.is_empty() {
            continue;
        }

        combined = prepend_path_entry(&combined, &parent);
        added_any = true;
    }

    if combined.is_empty() && !added_any {
        None
    } else {
        Some(combined)
    }
}

fn resolve_remote_binary(
    configured: &str,
    home: &str,
    normalized_path: &str,
    probe: &dyn RemoteProbe,
) -> Result<String, AdapterError> {
    if !binary_needs_probe(configured) {
        return Ok(configured.to_string());
    }

    if let Some(resolved) = probe.command_v(normalized_path, configured)? {
        return Ok(resolved);
    }

    for candidate in common_binary_fallbacks(home, configured) {
        if probe.is_executable(&candidate)? {
            return Ok(candidate);
        }
    }

    Err(AdapterError::CommandFailed(format!(
        "remote binary `{configured}` was not found after probing PATH and common locations"
    )))
}

fn common_binary_fallbacks(home: &str, binary: &str) -> Vec<String> {
    [
        format!("{home}/.local/bin/{binary}"),
        format!("/usr/local/bin/{binary}"),
        format!("/usr/bin/{binary}"),
        format!("/bin/{binary}"),
    ]
    .into_iter()
    .collect()
}

fn run_remote_command_output(
    config: &SshTargetConfig,
    remote_command: &str,
) -> Result<std::process::Output, AdapterError> {
    run_remote_command_output_with_timeout(config, remote_command, REMOTE_PROBE_TIMEOUT)
}

fn run_remote_command_output_with_timeout(
    config: &SshTargetConfig,
    remote_command: &str,
    timeout: Duration,
) -> Result<std::process::Output, AdapterError> {
    let child = Command::new("ssh")
        .arg("-T")
        .arg("-oBatchMode=yes")
        .args(&config.ssh_options)
        .arg(&config.host)
        .arg(remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| AdapterError::ZjctlUnavailable)?;
    wait_for_output_with_timeout(child, timeout)
}

fn run_remote_command_string(
    config: &SshTargetConfig,
    remote_command: &str,
) -> Result<Vec<u8>, AdapterError> {
    let output = run_remote_command_output(config, remote_command)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("command exited with status {}", output.status)
        } else {
            stderr
        };
        return Err(AdapterError::CommandFailed(message));
    }

    Ok(output.stdout)
}

fn run_remote_command_with_stdin_timeout(
    config: &SshTargetConfig,
    remote_command: &str,
    stdin_payload: &[u8],
    timeout: Duration,
) -> Result<Output, AdapterError> {
    let mut child = Command::new("ssh")
        .arg("-T")
        .arg("-oBatchMode=yes")
        .args(&config.ssh_options)
        .arg(&config.host)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| AdapterError::ZjctlUnavailable)?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_payload)
            .map_err(|_| AdapterError::ZjctlUnavailable)?;
    }

    wait_for_output_with_timeout(child, timeout)
}

fn default_plugin_path() -> PathBuf {
    let rel = Path::new("zellij").join("plugins").join("zrpc.wasm");

    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join(rel);
    }

    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join(rel);
    }

    PathBuf::from("zrpc.wasm")
}

fn default_plugin_url() -> String {
    format!("file:{}", default_plugin_path().display())
}

fn plugin_launch_command(plugin_url: &str) -> String {
    format!("zellij action launch-plugin \"{plugin_url}\"")
}

fn pipe_plugin_configuration_for(session_name: &str) -> String {
    let sanitized = session_name
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '_',
        })
        .collect::<String>();
    format!("zjctl_session={sanitized}")
}

fn pane_ids(panes: &[PaneInfo]) -> Vec<String> {
    let mut ids: Vec<String> = panes.iter().map(|pane| pane.id.clone()).collect();
    ids.sort();
    ids
}

fn parse_rpc_output(
    stdout: &str,
    request_id: uuid::Uuid,
    plugin_url: &str,
) -> Result<serde_json::Value, AdapterError> {
    for response in serde_json::Deserializer::from_str(stdout)
        .into_iter::<RpcResponse>()
        .flatten()
    {
        if response.id != request_id {
            continue;
        }

        if response.v != PROTOCOL_VERSION {
            return Err(AdapterError::CommandFailed(format!(
                "zrpc protocol version mismatch: expected {}, got {}",
                PROTOCOL_VERSION, response.v
            )));
        }

        if response.ok {
            return Ok(response.result.unwrap_or(serde_json::Value::Null));
        }

        let message = response
            .error
            .map(|error| error.message)
            .unwrap_or_else(|| "unknown error".to_string());
        return Err(AdapterError::CommandFailed(message));
    }

    Err(AdapterError::CommandFailed(format!(
        "no response from zrpc plugin\n\nMake sure it is loaded in your Zellij session:\n  {}",
        plugin_launch_command(plugin_url)
    )))
}

fn pane_info_to_target(pane: PaneInfo, session_name: Option<&str>) -> ResolvedTarget {
    ResolvedTarget {
        selector: format!("id:{}", pane.id),
        pane_id: Some(pane.id),
        session_name: session_name.unwrap_or_default().to_string(),
        tab_name: Some(pane.tab_name),
        title: Some(pane.title),
        command: pane.command,
        focused: pane.focused,
    }
}

fn pane_info_to_target_with_tab_names(
    mut pane: PaneInfo,
    session_name: Option<&str>,
    tab_names: &BTreeMap<usize, String>,
) -> ResolvedTarget {
    if let Some(tab_name) = tab_names.get(&pane.tab_index) {
        pane.tab_name = tab_name.clone();
    }
    pane_info_to_target(pane, session_name)
}

fn resolve_from_candidates(
    request: &AttachRequest,
    candidates: Vec<ResolvedTarget>,
) -> Result<ResolvedTarget, AdapterError> {
    let selector = request.selector.trim();
    let filtered: Vec<_> = candidates
        .into_iter()
        .filter(|target| {
            request
                .tab_name
                .as_ref()
                .is_none_or(|tab_name| target.tab_name.as_deref() == Some(tab_name.as_str()))
        })
        .filter(|target| matches_selector(selector, target))
        .collect();

    match filtered.as_slice() {
        [] => Err(AdapterError::CommandFailed(format!(
            "no pane matched selector `{selector}`"
        ))),
        [target] => Ok(target.clone()),
        _ => Err(AdapterError::CommandFailed(format!(
            "selector `{selector}` matched multiple panes"
        ))),
    }
}

fn selector_to_action_pane_id(selector: &str) -> Option<String> {
    let pane_id = selector
        .strip_prefix("id:")
        .or_else(|| selector.strip_prefix("terminal:"))
        .or_else(|| selector.strip_prefix("plugin:"));

    if let Some(pane_id) = selector.strip_prefix("id:terminal:") {
        return Some(format!("terminal_{pane_id}"));
    }
    if let Some(pane_id) = selector.strip_prefix("id:plugin:") {
        return Some(format!("plugin_{pane_id}"));
    }
    if let Some(pane_id) = selector.strip_prefix("terminal:") {
        return Some(format!("terminal_{pane_id}"));
    }
    if let Some(pane_id) = selector.strip_prefix("plugin:") {
        return Some(format!("plugin_{pane_id}"));
    }

    match pane_id {
        Some(raw) if raw.chars().all(|ch| ch.is_ascii_digit()) => Some(raw.to_string()),
        _ => None,
    }
}

fn is_unsupported_action_pane_target(error: &AdapterError) -> bool {
    matches!(error, AdapterError::CommandFailed(message) if message.contains("Found argument '--pane-id'"))
}

impl LocalBackend {
    pub fn new() -> Self {
        Self
    }

    fn run_zellij_command(
        &self,
        session_name: Option<&str>,
        args: &[String],
    ) -> Result<Output, AdapterError> {
        let mut process = Command::new("zellij");
        if let Some(session_name) = session_name {
            process.arg("--session").arg(session_name);
        }
        process.args(args);

        let output = process
            .output()
            .map_err(|_| AdapterError::ZjctlUnavailable)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                format!("command exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(AdapterError::CommandFailed(message));
        }

        Ok(output)
    }

    fn run_zellij_action(
        &self,
        session_name: &str,
        args: &[String],
    ) -> Result<Vec<u8>, AdapterError> {
        let mut command_args = vec!["action".to_string()];
        command_args.extend(args.iter().cloned());
        Ok(self
            .run_zellij_command(Some(session_name), &command_args)?
            .stdout)
    }

    fn run_zellij_command_with_stdin(
        &self,
        session_name: Option<&str>,
        args: &[String],
        stdin_payload: &[u8],
    ) -> Result<Output, AdapterError> {
        let mut process = Command::new("zellij");
        if let Some(session_name) = session_name {
            process.arg("--session").arg(session_name);
        }
        let mut child = process
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| AdapterError::ZjctlUnavailable)?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(stdin_payload)
                .map_err(|_| AdapterError::ZjctlUnavailable)?;
        }

        child
            .wait_with_output()
            .map_err(|_| AdapterError::ZjctlUnavailable)
    }

    fn rpc_call<P: Serialize>(
        &self,
        session_name: &str,
        method: &str,
        params: P,
    ) -> Result<serde_json::Value, AdapterError> {
        let request = RpcRequest::new(method)
            .with_params(params)
            .map_err(|error| AdapterError::ParseError(error.to_string()))?;
        let plugin_url = default_plugin_url();
        let plugin_path = default_plugin_path();
        if !plugin_path.is_file() {
            return Err(AdapterError::CommandFailed(format!(
                "zrpc plugin not found at {}\n\nLoad it in Zellij:\n  {}",
                plugin_path.display(),
                plugin_launch_command(&plugin_url)
            )));
        }

        let request_json = format!(
            "{}\n",
            serde_json::to_string(&request)
                .map_err(|error| AdapterError::ParseError(error.to_string()))?
        );
        let args = vec![
            "pipe".to_string(),
            "--plugin".to_string(),
            plugin_url.clone(),
            "--plugin-configuration".to_string(),
            pipe_plugin_configuration_for(session_name),
            "--name".to_string(),
            RPC_PIPE_NAME.to_string(),
        ];
        let output =
            self.run_zellij_command_with_stdin(Some(session_name), &args, request_json.as_bytes())?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                format!("command exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(AdapterError::CommandFailed(message));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_rpc_output(&stdout, request.id, &plugin_url)
    }

    fn list_panes_once(&self, session_name: &str) -> Result<Vec<PaneInfo>, AdapterError> {
        let result = self.rpc_call(session_name, methods::PANES_LIST, serde_json::json!({}))?;
        serde_json::from_value(result).map_err(|error| AdapterError::ParseError(error.to_string()))
    }

    fn list_tab_names_once(
        &self,
        session_name: &str,
    ) -> Result<BTreeMap<usize, String>, AdapterError> {
        let output = self.run_zellij_action(
            session_name,
            &["list-tabs".to_string(), "--json".to_string()],
        )?;
        let tabs: Vec<TabInfo> = serde_json::from_slice(&output)
            .map_err(|error| AdapterError::ParseError(error.to_string()))?;
        Ok(tabs
            .into_iter()
            .map(|tab| (tab.position, tab.name))
            .collect())
    }

    fn list_targets_for_session(
        &self,
        session_name: &str,
    ) -> Result<Vec<ResolvedTarget>, AdapterError> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(500);
        let interval = Duration::from_millis(50);

        let mut panes = self.list_panes_once(session_name)?;
        let tab_names = self.list_tab_names_once(session_name).unwrap_or_default();
        let mut ids = pane_ids(&panes);

        loop {
            if start.elapsed() >= timeout {
                return Ok(panes
                    .into_iter()
                    .map(|pane| {
                        pane_info_to_target_with_tab_names(pane, Some(session_name), &tab_names)
                    })
                    .collect());
            }

            std::thread::sleep(interval);
            let next = self.list_panes_once(session_name)?;
            let next_ids = pane_ids(&next);
            if next_ids == ids {
                return Ok(next
                    .into_iter()
                    .map(|pane| {
                        pane_info_to_target_with_tab_names(pane, Some(session_name), &tab_names)
                    })
                    .collect());
            }
            panes = next;
            ids = next_ids;
        }
    }

    fn resolved_target(
        &self,
        session_name: &str,
        selector: &str,
    ) -> Result<ResolvedTarget, AdapterError> {
        let request = AttachRequest {
            target: None,
            session_name: session_name.to_string(),
            tab_name: None,
            selector: selector.to_string(),
            alias: None,
        };
        resolve_from_candidates(&request, self.list_targets_for_session(session_name)?)
    }

    fn dump_screen(
        &self,
        session_name: &str,
        selector: &str,
        full: bool,
    ) -> Result<Vec<u8>, AdapterError> {
        let pane_id = selector_to_action_pane_id(selector).ok_or_else(|| {
            AdapterError::CommandFailed(format!(
                "selector `{selector}` could not be converted to a pane id"
            ))
        })?;
        let path =
            std::env::temp_dir().join(format!("zellij-mcp-dump-{}.txt", uuid::Uuid::new_v4()));
        let mut args = vec![
            "dump-screen".to_string(),
            "--pane-id".to_string(),
            pane_id,
            "--path".to_string(),
            path.display().to_string(),
        ];
        if full {
            args.push("--full".to_string());
        }
        self.run_zellij_action(session_name, &args)?;
        let output = std::fs::read(&path).map_err(|error| {
            AdapterError::CommandFailed(format!("failed to read dump-screen output: {error}"))
        });
        let _ = std::fs::remove_file(&path);
        output
    }

    fn wait_for_idle_via_dump(
        &self,
        session_name: &str,
        selector: &str,
        idle_ms: u64,
        timeout_ms: u64,
    ) -> Result<(), AdapterError> {
        let target = self.resolved_target(session_name, selector)?;
        let selector = target.selector;

        (|| {
            let idle_duration = Duration::from_millis(idle_ms);
            let timeout_duration = Duration::from_millis(timeout_ms);
            let poll_interval =
                Duration::from_millis(idle_ms.clamp(100, 1000) / 4).max(Duration::from_millis(100));
            let start = std::time::Instant::now();
            let mut last_change = std::time::Instant::now();
            let mut last_hash = {
                let content = self.dump_screen(session_name, &selector, true)?;
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                use std::hash::{Hash, Hasher};
                content.hash(&mut hasher);
                hasher.finish()
            };

            loop {
                if last_change.elapsed() >= idle_duration {
                    return Ok(());
                }
                if start.elapsed() >= timeout_duration {
                    return Err(AdapterError::Timeout);
                }

                std::thread::sleep(poll_interval);
                let content = self.dump_screen(session_name, &selector, true)?;
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                use std::hash::{Hash, Hasher};
                content.hash(&mut hasher);
                let current_hash = hasher.finish();
                if current_hash != last_hash {
                    last_hash = current_hash;
                    last_change = std::time::Instant::now();
                }
            }
        })()
    }

    fn close_handle(
        &self,
        session_name: &str,
        selector: &str,
        force: bool,
    ) -> Result<(), AdapterError> {
        let target = self.resolved_target(session_name, selector)?;
        if !force && target.focused {
            return Err(AdapterError::CommandFailed(
                "refusing to close focused pane (use --force)".to_string(),
            ));
        }
        let pane_id = selector_to_action_pane_id(target.pane_id.as_deref().unwrap_or(selector))
            .ok_or_else(|| {
                AdapterError::CommandFailed(format!(
                    "selector `{selector}` could not be converted to a pane id"
                ))
            })?;
        self.run_zellij_action(
            session_name,
            &["close-pane".to_string(), "--pane-id".to_string(), pane_id],
        )?;
        Ok(())
    }
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SshBackend {
    pub fn new(config: SshTargetConfig) -> Self {
        let config = resolve_ssh_runtime_config(&config).unwrap_or(config);
        Self { config }
    }

    fn wait_for_target_absent(
        &self,
        session_name: &str,
        selector: &str,
    ) -> Result<bool, AdapterError> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(750);
        let interval = Duration::from_millis(50);

        loop {
            let still_present = self
                .list_targets_for_session(session_name)?
                .into_iter()
                .any(|target| matches_selector(selector, &target));
            if !still_present {
                return Ok(true);
            }

            if start.elapsed() >= timeout {
                return Ok(false);
            }

            std::thread::sleep(interval);
        }
    }

    fn ensure_close_effective(
        &self,
        session_name: &str,
        selector: &str,
        force: bool,
        used_zjctl_close: bool,
    ) -> Result<(), AdapterError> {
        if self.wait_for_target_absent(session_name, selector)? {
            return Ok(());
        }

        if !used_zjctl_close {
            self.close_via_zjctl(session_name, selector, force)?;
            if self.wait_for_target_absent(session_name, selector)? {
                return Ok(());
            }
        }

        Err(AdapterError::CommandFailed(format!(
            "close reported success but selector `{selector}` is still present"
        )))
    }

    fn remote_env_assignments(&self, session_name: Option<&str>) -> Vec<(String, String)> {
        let mut assignments: Vec<(String, String)> = self
            .config
            .remote_env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let path = augment_path_with_binary_dirs(
            assignments
                .iter()
                .find(|(key, _)| key == "PATH")
                .map(|(_, value)| value.clone()),
            &[
                &self.config.remote_zellij_bin,
                &self.config.remote_zjctl_bin,
            ],
        );
        if let Some(path) = path {
            assignments.retain(|(key, _)| key != "PATH");
            assignments.push(("PATH".to_string(), path));
        }
        if let Some(session_name) = session_name {
            assignments.push(("ZELLIJ_SESSION_NAME".to_string(), session_name.to_string()));
        }
        assignments
    }

    fn build_remote_exec_command(
        &self,
        binary: &str,
        env_assignments: &[(String, String)],
        args: &[String],
    ) -> String {
        self.build_remote_command(binary, env_assignments, args, true)
    }

    fn build_remote_shell_command(
        &self,
        binary: &str,
        env_assignments: &[(String, String)],
        args: &[String],
    ) -> String {
        self.build_remote_command(binary, env_assignments, args, false)
    }

    fn build_remote_command(
        &self,
        binary: &str,
        env_assignments: &[(String, String)],
        args: &[String],
        use_exec: bool,
    ) -> String {
        let mut command = if use_exec {
            String::from("exec env")
        } else {
            String::from("env")
        };
        for (key, value) in env_assignments {
            command.push(' ');
            command.push_str(&quote_posix_sh(&format!("{key}={value}")));
        }
        command.push(' ');
        command.push_str(&quote_posix_sh(binary));
        for arg in args {
            command.push(' ');
            command.push_str(&quote_posix_sh(arg));
        }
        command
    }

    fn remote_plugin_path_expr(&self) -> String {
        if let Some(xdg_config_home) = self.config.remote_env.get("XDG_CONFIG_HOME") {
            format!("{xdg_config_home}/zellij/plugins/zrpc.wasm")
        } else {
            "$HOME/.config/zellij/plugins/zrpc.wasm".to_string()
        }
    }

    fn remote_plugin_url_expr(&self) -> String {
        format!("file:{}", self.remote_plugin_path_expr())
    }

    fn run_zellij_command(
        &self,
        session_name: Option<&str>,
        args: &[String],
    ) -> Result<Vec<u8>, AdapterError> {
        let remote_command = self.build_remote_exec_command(
            &self.config.remote_zellij_bin,
            &self.remote_env_assignments(session_name),
            args,
        );
        self.run_remote_command_string(&remote_command)
    }

    fn run_remote_command_string(&self, remote_command: &str) -> Result<Vec<u8>, AdapterError> {
        run_remote_command_string(&self.config, remote_command)
    }

    fn run_remote_command_detached(&self, remote_command: &str) -> Result<(), AdapterError> {
        let detached = format!(
            "nohup sh -lc {} >/dev/null 2>&1 </dev/null &",
            quote_posix_sh(remote_command)
        );
        Command::new("ssh")
            .arg("-T")
            .arg("-oBatchMode=yes")
            .args(&self.config.ssh_options)
            .arg(&self.config.host)
            .arg(detached)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|_| AdapterError::ZjctlUnavailable)
    }

    fn run_remote_command_checked(
        &self,
        remote_command: &str,
        timeout: Duration,
    ) -> Result<(), AdapterError> {
        let output = run_remote_command_output_with_timeout(&self.config, remote_command, timeout)?;
        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("command exited with status {}", output.status)
        } else {
            stderr
        };
        Err(AdapterError::CommandFailed(message))
    }

    fn run_remote_zjctl_command(
        &self,
        session_name: &str,
        command: ZjctlCommand,
    ) -> Result<Vec<u8>, AdapterError> {
        let timeout = match &command {
            ZjctlCommand::WaitIdle {
                timeout_seconds, ..
            } => {
                timeout_seconds
                    .parse::<f64>()
                    .ok()
                    .map(Duration::from_secs_f64)
                    .unwrap_or(REMOTE_COMMAND_TIMEOUT)
                    + Duration::from_secs(2)
            }
            _ => REMOTE_COMMAND_TIMEOUT,
        };
        let remote_command = self.build_remote_exec_command(
            &self.config.remote_zjctl_bin,
            &self.remote_env_assignments(Some(session_name)),
            &command.args(),
        );
        let output =
            run_remote_command_output_with_timeout(&self.config, &remote_command, timeout)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                format!("command exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(AdapterError::CommandFailed(message));
        }

        Ok(output.stdout)
    }

    fn capture_via_zjctl(
        &self,
        session_name: &str,
        selector: &str,
        full: bool,
    ) -> Result<CaptureSnapshot, AdapterError> {
        let content = self.run_remote_zjctl_command(
            session_name,
            ZjctlCommand::Capture {
                selector: selector.to_string(),
                full,
            },
        )?;
        Ok(CaptureSnapshot {
            content: parse_capture_output(&content),
            captured_at: Utc::now(),
            truncated: false,
        })
    }

    fn wait_via_zjctl(
        &self,
        session_name: &str,
        selector: &str,
        idle_ms: u64,
        timeout_ms: u64,
    ) -> Result<(), AdapterError> {
        self.run_remote_zjctl_command(
            session_name,
            ZjctlCommand::WaitIdle {
                selector: selector.to_string(),
                idle_seconds: format_seconds(idle_ms),
                timeout_seconds: format_seconds(timeout_ms),
            },
        )
        .map(|_| ())
    }

    fn close_via_zjctl(
        &self,
        session_name: &str,
        selector: &str,
        force: bool,
    ) -> Result<(), AdapterError> {
        self.run_remote_zjctl_command(
            session_name,
            ZjctlCommand::Close {
                selector: selector.to_string(),
                force,
            },
        )
        .map(|_| ())
    }

    fn rpc_call<P: Serialize>(
        &self,
        session_name: &str,
        method: &str,
        params: P,
    ) -> Result<serde_json::Value, AdapterError> {
        let request = RpcRequest::new(method)
            .with_params(params)
            .map_err(|error| AdapterError::ParseError(error.to_string()))?;
        let request_json = format!(
            "{}\n",
            serde_json::to_string(&request)
                .map_err(|error| AdapterError::ParseError(error.to_string()))?
        );

        let plugin_path = self.remote_plugin_path_expr();
        let plugin_url = self.remote_plugin_url_expr();
        let plugin_check = format!("[ -f \"{plugin_path}\" ]");
        if !run_remote_command_output(&self.config, &plugin_check)
            .map(|output| output.status.success())
            .unwrap_or(false)
        {
            return Err(AdapterError::CommandFailed(format!(
                "zrpc plugin not found at {plugin_path}\n\nLoad it in Zellij:\n  {}",
                plugin_launch_command(&plugin_url)
            )));
        }

        let remote_command = format!(
            "exec env {} {} pipe --plugin \"{}\" --plugin-configuration {} --name {}",
            self.remote_env_assignments(Some(session_name))
                .into_iter()
                .map(|(key, value)| quote_posix_sh(&format!("{key}={value}")))
                .collect::<Vec<_>>()
                .join(" "),
            quote_posix_sh(&self.config.remote_zellij_bin),
            plugin_url,
            quote_posix_sh(&pipe_plugin_configuration_for(session_name)),
            quote_posix_sh(RPC_PIPE_NAME),
        );
        let output = run_remote_command_with_stdin_timeout(
            &self.config,
            &remote_command,
            request_json.as_bytes(),
            REMOTE_PROBE_TIMEOUT,
        )?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                format!("command exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(AdapterError::CommandFailed(message));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_rpc_output(&stdout, request.id, &plugin_url)
    }

    fn list_panes_once(&self, session_name: &str) -> Result<Vec<ResolvedTarget>, AdapterError> {
        let output = self.run_remote_zjctl_command(session_name, ZjctlCommand::List)?;
        parse_list_output(&String::from_utf8_lossy(&output), Some(session_name))
    }

    fn list_targets_for_session(
        &self,
        session_name: &str,
    ) -> Result<Vec<ResolvedTarget>, AdapterError> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(500);
        let interval = Duration::from_millis(50);

        let mut targets = self.list_panes_once(session_name)?;
        let mut ids: Vec<_> = targets
            .iter()
            .map(|target| {
                target
                    .pane_id
                    .as_deref()
                    .unwrap_or(target.selector.as_str())
                    .to_string()
            })
            .collect();

        loop {
            if start.elapsed() >= timeout {
                return Ok(targets);
            }

            std::thread::sleep(interval);
            let next = self.list_panes_once(session_name)?;
            let next_ids: Vec<_> = next
                .iter()
                .map(|target| {
                    target
                        .pane_id
                        .as_deref()
                        .unwrap_or(target.selector.as_str())
                        .to_string()
                })
                .collect();
            if next_ids == ids {
                return Ok(next);
            }
            targets = next;
            ids = next_ids;
        }
    }

    fn resolved_target(
        &self,
        session_name: &str,
        selector: &str,
    ) -> Result<ResolvedTarget, AdapterError> {
        let request = AttachRequest {
            target: None,
            session_name: session_name.to_string(),
            tab_name: None,
            selector: selector.to_string(),
            alias: None,
        };
        resolve_from_candidates(&request, self.list_targets_for_session(session_name)?)
    }

    fn dump_screen(
        &self,
        session_name: &str,
        selector: &str,
        full: bool,
    ) -> Result<Vec<u8>, AdapterError> {
        let pane_id = selector_to_action_pane_id(selector).ok_or_else(|| {
            AdapterError::CommandFailed(format!(
                "selector `{selector}` could not be converted to a pane id"
            ))
        })?;
        let remote_command = format!(
            "tmp=$(mktemp); trap 'rm -f \"$tmp\"' EXIT; {} ; status=$?; cat \"$tmp\"; exit $status",
            self.build_remote_shell_command(
                &self.config.remote_zellij_bin,
                &self.remote_env_assignments(Some(session_name)),
                &{
                    let mut args = vec![
                        "action".to_string(),
                        "dump-screen".to_string(),
                        "--pane-id".to_string(),
                        pane_id,
                        "--path".to_string(),
                        "$tmp".to_string(),
                    ];
                    if full {
                        args.push("--full".to_string());
                    }
                    args
                },
            )
            .replace("'$tmp'", "\"$tmp\"")
        );
        run_remote_command_string(&self.config, &remote_command)
    }

    fn focus_pane(&self, session_name: &str, selector: &str) -> Result<(), AdapterError> {
        self.rpc_call(
            session_name,
            methods::PANE_FOCUS,
            serde_json::json!({ "selector": selector }),
        )
        .map(|_| ())
    }

    fn dump_focused_screen(&self, session_name: &str, full: bool) -> Result<Vec<u8>, AdapterError> {
        let remote_command = format!(
            "tmp=$(mktemp); trap 'rm -f \"$tmp\"' EXIT; {} ; status=$?; cat \"$tmp\"; exit $status",
            self.build_remote_shell_command(
                &self.config.remote_zellij_bin,
                &self.remote_env_assignments(Some(session_name)),
                &{
                    let mut args = vec![
                        "action".to_string(),
                        "dump-screen".to_string(),
                        "$tmp".to_string(),
                    ];
                    if full {
                        args.push("--full".to_string());
                    }
                    args
                },
            )
            .replace("'$tmp'", "\"$tmp\"")
        );
        run_remote_command_string(&self.config, &remote_command)
    }

    fn wait_for_idle_via_focused_dump(
        &self,
        session_name: &str,
        selector: &str,
        idle_ms: u64,
        timeout_ms: u64,
    ) -> Result<(), AdapterError> {
        self.focus_pane(session_name, selector)?;

        let idle_duration = Duration::from_millis(idle_ms);
        let timeout_duration = Duration::from_millis(timeout_ms);
        let poll_interval =
            Duration::from_millis(idle_ms.clamp(100, 1000) / 4).max(Duration::from_millis(100));
        let start = std::time::Instant::now();
        let mut last_change = std::time::Instant::now();
        let mut last_hash = {
            let content = self.dump_focused_screen(session_name, true)?;
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            use std::hash::{Hash, Hasher};
            content.hash(&mut hasher);
            hasher.finish()
        };

        loop {
            if last_change.elapsed() >= idle_duration {
                return Ok(());
            }
            if start.elapsed() >= timeout_duration {
                return Err(AdapterError::Timeout);
            }

            std::thread::sleep(poll_interval);
            let content = self.dump_focused_screen(session_name, true)?;
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            use std::hash::{Hash, Hasher};
            content.hash(&mut hasher);
            let current_hash = hasher.finish();
            if current_hash != last_hash {
                last_hash = current_hash;
                last_change = std::time::Instant::now();
            }
        }
    }

    fn close_focused_pane(&self, session_name: &str, selector: &str) -> Result<(), AdapterError> {
        self.focus_pane(session_name, selector)?;
        self.run_zellij_command(
            Some(session_name),
            &["action".to_string(), "close-pane".to_string()],
        )?;
        Ok(())
    }

    fn wait_for_idle_via_dump(
        &self,
        session_name: &str,
        selector: &str,
        idle_ms: u64,
        timeout_ms: u64,
    ) -> Result<(), AdapterError> {
        let target = self.resolved_target(session_name, selector)?;
        let selector = target.selector;

        (|| {
            let idle_duration = Duration::from_millis(idle_ms);
            let timeout_duration = Duration::from_millis(timeout_ms);
            let poll_interval =
                Duration::from_millis(idle_ms.clamp(100, 1000) / 4).max(Duration::from_millis(100));
            let start = std::time::Instant::now();
            let mut last_change = std::time::Instant::now();
            let mut last_hash = {
                let content = self.dump_screen(session_name, &selector, true)?;
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                use std::hash::{Hash, Hasher};
                content.hash(&mut hasher);
                hasher.finish()
            };

            loop {
                if last_change.elapsed() >= idle_duration {
                    return Ok(());
                }
                if start.elapsed() >= timeout_duration {
                    return Err(AdapterError::Timeout);
                }

                std::thread::sleep(poll_interval);
                let content = self.dump_screen(session_name, &selector, true)?;
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                use std::hash::{Hash, Hasher};
                content.hash(&mut hasher);
                let current_hash = hasher.finish();
                if current_hash != last_hash {
                    last_hash = current_hash;
                    last_change = std::time::Instant::now();
                }
            }
        })()
    }

    fn close_handle(
        &self,
        session_name: &str,
        selector: &str,
        force: bool,
    ) -> Result<(), AdapterError> {
        let target = self.resolved_target(session_name, selector)?;
        if !force && target.focused {
            return Err(AdapterError::CommandFailed(
                "refusing to close focused pane (use --force)".to_string(),
            ));
        }
        let pane_id = selector_to_action_pane_id(target.pane_id.as_deref().unwrap_or(selector))
            .ok_or_else(|| {
                AdapterError::CommandFailed(format!(
                    "selector `{selector}` could not be converted to a pane id"
                ))
            })?;
        self.run_zellij_command(
            Some(session_name),
            &[
                "action".to_string(),
                "close-pane".to_string(),
                "--pane-id".to_string(),
                pane_id,
            ],
        )?;
        Ok(())
    }
}

impl BackendAdapter for SshBackend {
    fn is_available(&self) -> bool {
        self.run_zellij_command(None, &["--help".to_string()])
            .is_ok()
    }

    fn ensure_session_ready(&self, session_name: &str) -> Result<(), AdapterError> {
        match classify_ssh_backend_readiness(&self.config, session_name) {
            SshBackendReadiness::Ready(resolved) => {
                let mut client = self.clone();
                client.config = resolved;
                client.list_targets_for_session(session_name).map(|_| ())
            }
            SshBackendReadiness::AutoFixable(failure) => {
                if attempt_safe_ssh_readiness_remediation(&self.config, session_name, &failure) {
                    match classify_ssh_backend_readiness(&self.config, session_name) {
                        SshBackendReadiness::Ready(resolved) => {
                            let mut client = self.clone();
                            client.config = resolved;
                            client.list_targets_for_session(session_name).map(|_| ())
                        }
                        SshBackendReadiness::AutoFixable(retry_failure)
                        | SshBackendReadiness::ManualActionRequired(retry_failure) => {
                            Err(readiness_failure_to_adapter_error(retry_failure))
                        }
                    }
                } else {
                    Err(readiness_failure_to_adapter_error(failure))
                }
            }
            SshBackendReadiness::ManualActionRequired(failure) => {
                Err(readiness_failure_to_adapter_error(failure))
            }
        }
    }

    fn spawn(&self, request: &SpawnRequest) -> Result<ResolvedTarget, AdapterError> {
        let before = self.list_targets_for_session(&request.session_name)?;
        let prepared = prepare_spawn(request, &before)?;

        let (action_args, spawn_command, command_via_action) = match prepared {
            PreparedSpawn::Reuse(target) => return Ok(target),
            PreparedSpawn::Launch {
                action_args,
                command,
                command_via_action,
            } => (action_args, command, command_via_action),
            PreparedSpawn::Ambiguous {
                tab_name,
                reusable_targets,
            } => {
                let choices = reusable_targets
                    .iter()
                    .map(|target| {
                        let selector = target.selector.as_str();
                        let title = target.title.as_deref().unwrap_or("<untitled>");
                        format!("{selector} ({title})")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(AdapterError::AmbiguousTarget(format!(
                    "tab `{tab_name}` has multiple reusable panes; choose an existing pane selector or create a new pane explicitly: {choices}"
                )));
            }
        };
        let command_summary = spawn_command.join(" ");

        if let Some(action_args) = action_args {
            self.run_zellij_command(
                Some(&request.session_name),
                &std::iter::once("action".to_string())
                    .chain(action_args.into_iter())
                    .collect::<Vec<_>>(),
            )?;
        }

        let output = if command_via_action {
            Vec::new()
        } else {
            let mut run_args = vec!["run".to_string()];
            if let Some(cwd) = &request.cwd {
                run_args.push("--cwd".to_string());
                run_args.push(cwd.clone());
            }
            if let Some(title) = &request.title {
                run_args.push("--name".to_string());
                run_args.push(title.clone());
            }
            run_args.push("--".to_string());
            run_args.extend(spawn_command);
            self.run_zellij_command(Some(&request.session_name), &run_args)?
        };

        let after = self.list_targets_for_session(&request.session_name)?;
        resolve_spawned_target_from_run_output(request, before, after, &output, &command_summary)
    }

    fn launch_spawn(&self, request: &SpawnRequest) -> Result<Option<ResolvedTarget>, AdapterError> {
        let before = self.list_targets_for_session(&request.session_name)?;
        let prepared = prepare_spawn(request, &before)?;

        let (action_args, spawn_command, command_via_action) = match prepared {
            PreparedSpawn::Reuse(target) => return Ok(Some(target)),
            PreparedSpawn::Launch {
                action_args,
                command,
                command_via_action,
            } => (action_args, command, command_via_action),
            PreparedSpawn::Ambiguous {
                tab_name,
                reusable_targets,
            } => {
                let choices = reusable_targets
                    .iter()
                    .map(|target| {
                        let selector = target.selector.as_str();
                        let title = target.title.as_deref().unwrap_or("<untitled>");
                        format!("{selector} ({title})")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(AdapterError::AmbiguousTarget(format!(
                    "tab `{tab_name}` has multiple reusable panes; choose an existing pane selector or create a new pane explicitly: {choices}"
                )));
            }
        };

        if let Some(action_args) = action_args {
            let action_command = self.build_remote_exec_command(
                &self.config.remote_zellij_bin,
                &self.remote_env_assignments(Some(&request.session_name)),
                &std::iter::once("action".to_string())
                    .chain(action_args)
                    .collect::<Vec<_>>(),
            );
            if command_via_action {
                match self.run_remote_command_checked(&action_command, REMOTE_COMMAND_TIMEOUT) {
                    Ok(()) => {
                        let after = self.list_targets_for_session(&request.session_name)?;
                        let resolved = resolve_spawned_target(
                            request,
                            before,
                            after,
                            &spawn_command.join(" "),
                        )?;
                        return Ok(Some(resolved));
                    }
                    Err(AdapterError::CommandFailed(message))
                        if is_unsupported_new_tab_initial_command(&message)
                            && is_default_fish_spawn_request(request, &spawn_command) =>
                    {
                        let compat_action_args = prepare_new_tab_action_args(request)
                            .expect("new-tab action args should exist for missing-tab compatibility fallback");
                        let compat_action_command = self.build_remote_exec_command(
                            &self.config.remote_zellij_bin,
                            &self.remote_env_assignments(Some(&request.session_name)),
                            &std::iter::once("action".to_string())
                                .chain(compat_action_args)
                                .collect::<Vec<_>>(),
                        );
                        self.run_remote_command_checked(
                            &compat_action_command,
                            REMOTE_COMMAND_TIMEOUT,
                        )?;

                        let after = self.list_targets_for_session(&request.session_name)?;
                        let resolved = resolve_spawned_target(
                            request,
                            before,
                            after,
                            &spawn_command.join(" "),
                        )?;
                        if !matches!(
                            shell_name(resolved.command.as_deref()).as_deref(),
                            Some("fish")
                        ) {
                            self.send_input(
                                &request.session_name,
                                &resolved.selector,
                                "exec fish",
                                true,
                            )?;
                        }
                        return Ok(Some(resolved));
                    }
                    Err(error) => return Err(error),
                }
            } else {
                self.run_remote_command_detached(&action_command)?;
                std::thread::sleep(Duration::from_millis(200));
            }
        }

        if !command_via_action {
            let mut run_args = vec!["run".to_string()];
            if let Some(cwd) = &request.cwd {
                run_args.push("--cwd".to_string());
                run_args.push(cwd.clone());
            }
            if let Some(title) = &request.title {
                run_args.push("--name".to_string());
                run_args.push(title.clone());
            }
            run_args.push("--".to_string());
            run_args.extend(spawn_command);

            let remote_command = self.build_remote_exec_command(
                &self.config.remote_zellij_bin,
                &self.remote_env_assignments(Some(&request.session_name)),
                &run_args,
            );
            self.run_remote_command_detached(&remote_command)?;
        }
        Ok(None)
    }

    fn resolve_selector(&self, request: &AttachRequest) -> Result<ResolvedTarget, AdapterError> {
        resolve_from_candidates(
            request,
            self.list_targets_for_session(&request.session_name)?,
        )
    }

    fn list_targets_in_session(
        &self,
        session_name: &str,
    ) -> Result<Vec<ResolvedTarget>, AdapterError> {
        self.list_targets_for_session(session_name)
    }

    fn send_input(
        &self,
        session_name: &str,
        handle: &str,
        text: &str,
        submit: bool,
    ) -> Result<(), AdapterError> {
        let payload = if submit {
            format!("{text}\n")
        } else {
            text.to_string()
        };
        self.rpc_call(
            session_name,
            methods::PANE_SEND,
            serde_json::json!({"selector": handle, "all": false, "text": payload}),
        )
        .map(|_| ())
    }

    fn wait_idle(
        &self,
        session_name: &str,
        handle: &str,
        idle_ms: u64,
        timeout_ms: u64,
    ) -> Result<(), AdapterError> {
        if selector_to_action_pane_id(handle).is_none() {
            return self.wait_via_zjctl(session_name, handle, idle_ms, timeout_ms);
        }
        match self.wait_for_idle_via_dump(session_name, handle, idle_ms, timeout_ms) {
            Err(error) if is_unsupported_action_pane_target(&error) => {
                let target = self.resolved_target(session_name, handle)?;
                self.wait_for_idle_via_focused_dump(
                    session_name,
                    &target.selector,
                    idle_ms,
                    timeout_ms,
                )
                .or_else(|_| {
                    self.wait_via_zjctl(session_name, &target.selector, idle_ms, timeout_ms)
                })
            }
            other => other,
        }
    }

    fn capture_full(
        &self,
        session_name: &str,
        handle: &str,
    ) -> Result<CaptureSnapshot, AdapterError> {
        if selector_to_action_pane_id(handle).is_none() {
            return self.capture_via_zjctl(session_name, handle, true);
        }
        let target = self.resolved_target(session_name, handle)?;
        match self.dump_screen(session_name, &target.selector, true) {
            Ok(content) => {
                let parsed = parse_capture_output(&content);
                if parsed.is_empty() {
                    self.capture_via_zjctl(session_name, &target.selector, true)
                } else {
                    Ok(CaptureSnapshot {
                        content: parsed,
                        captured_at: Utc::now(),
                        truncated: false,
                    })
                }
            }
            Err(error) if is_unsupported_action_pane_target(&error) => self
                .focus_pane(session_name, &target.selector)
                .and_then(|_| self.dump_focused_screen(session_name, true))
                .map(|content| CaptureSnapshot {
                    content: parse_capture_output(&content),
                    captured_at: Utc::now(),
                    truncated: false,
                })
                .or_else(|_| self.capture_via_zjctl(session_name, &target.selector, true)),
            Err(error) => Err(error),
        }
    }

    fn close(&self, session_name: &str, handle: &str, force: bool) -> Result<(), AdapterError> {
        if selector_to_action_pane_id(handle).is_none() {
            self.close_via_zjctl(session_name, handle, force)?;
            return self.ensure_close_effective(session_name, handle, force, true);
        }
        match self.close_handle(session_name, handle, force) {
            Err(error) if is_unsupported_action_pane_target(&error) => {
                let target = self.resolved_target(session_name, handle)?;
                self.close_focused_pane(session_name, &target.selector)
                    .or_else(|_| self.close_via_zjctl(session_name, &target.selector, force))
                    .and_then(|_| {
                        self.ensure_close_effective(session_name, &target.selector, force, false)
                    })
            }
            Ok(()) => self.ensure_close_effective(session_name, handle, force, false),
            Err(error) => Err(error),
        }
    }

    fn list_targets(&self) -> Result<Vec<ResolvedTarget>, AdapterError> {
        let session_name = std::env::var("ZELLIJ_SESSION_NAME")
            .map_err(|_| AdapterError::ParseError("ZELLIJ_SESSION_NAME is not set".to_string()))?;
        self.list_targets_for_session(&session_name)
    }
}

impl BackendAdapter for LocalBackend {
    fn is_available(&self) -> bool {
        self.run_zellij_command(None, &["--help".to_string()])
            .is_ok()
    }

    fn ensure_session_ready(&self, session_name: &str) -> Result<(), AdapterError> {
        self.list_targets_in_session(session_name).map(|_| ())
    }

    fn spawn(&self, request: &SpawnRequest) -> Result<ResolvedTarget, AdapterError> {
        let before = self.list_targets_for_session(&request.session_name)?;
        let prepared = prepare_spawn(request, &before)?;

        let (action_args, spawn_command, command_via_action) = match prepared {
            PreparedSpawn::Reuse(target) => return Ok(target),
            PreparedSpawn::Launch {
                action_args,
                command,
                command_via_action,
            } => (action_args, command, command_via_action),
            PreparedSpawn::Ambiguous {
                tab_name,
                reusable_targets,
            } => {
                let choices = reusable_targets
                    .iter()
                    .map(|target| {
                        let selector = target.selector.as_str();
                        let title = target.title.as_deref().unwrap_or("<untitled>");
                        format!("{selector} ({title})")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(AdapterError::AmbiguousTarget(format!(
                    "tab `{tab_name}` has multiple reusable panes; choose an existing pane selector or create a new pane explicitly: {choices}"
                )));
            }
        };
        let command_summary = spawn_command.join(" ");

        let action_output = if let Some(action_args) = action_args {
            self.run_zellij_action(&request.session_name, &action_args)?
        } else {
            Vec::new()
        };

        if command_via_action && is_default_fish_spawn_request(request, &spawn_command) {
            if let (Some(tab_name), Some(tab_id)) = (
                request.tab_name.as_deref(),
                parse_action_created_tab_id(&action_output),
            ) {
                self.run_zellij_action(
                    &request.session_name,
                    &["rename-tab-by-id".to_string(), tab_id, tab_name.to_string()],
                )?;
            }
            let after = self.list_targets_for_session(&request.session_name)?;
            let resolved = resolve_spawned_target(request, before, after, &command_summary)?;
            if !matches!(
                shell_name(resolved.command.as_deref()).as_deref(),
                Some("fish")
            ) {
                self.send_input(&request.session_name, &resolved.selector, "exec fish", true)?;
            }
            return Ok(resolved);
        }

        let output = if command_via_action {
            Vec::new()
        } else {
            let mut run_args = vec!["run".to_string()];
            if let Some(cwd) = &request.cwd {
                run_args.push("--cwd".to_string());
                run_args.push(cwd.clone());
            }
            if let Some(title) = &request.title {
                run_args.push("--name".to_string());
                run_args.push(title.clone());
            }
            run_args.push("--".to_string());
            run_args.extend(spawn_command);
            self.run_zellij_command(Some(&request.session_name), &run_args)?
                .stdout
        };

        let after = self.list_targets_for_session(&request.session_name)?;
        resolve_spawned_target_from_run_output(request, before, after, &output, &command_summary)
    }

    fn resolve_selector(&self, request: &AttachRequest) -> Result<ResolvedTarget, AdapterError> {
        resolve_from_candidates(
            request,
            self.list_targets_for_session(&request.session_name)?,
        )
    }

    fn list_targets_in_session(
        &self,
        session_name: &str,
    ) -> Result<Vec<ResolvedTarget>, AdapterError> {
        self.list_targets_for_session(session_name)
    }

    fn send_input(
        &self,
        session_name: &str,
        handle: &str,
        text: &str,
        submit: bool,
    ) -> Result<(), AdapterError> {
        let payload = if submit {
            format!("{text}\n")
        } else {
            text.to_string()
        };
        self.rpc_call(
            session_name,
            methods::PANE_SEND,
            serde_json::json!({"selector": handle, "all": false, "text": payload}),
        )
        .map(|_| ())
    }

    fn wait_idle(
        &self,
        session_name: &str,
        handle: &str,
        idle_ms: u64,
        timeout_ms: u64,
    ) -> Result<(), AdapterError> {
        self.wait_for_idle_via_dump(session_name, handle, idle_ms, timeout_ms)
    }

    fn capture_full(
        &self,
        session_name: &str,
        handle: &str,
    ) -> Result<CaptureSnapshot, AdapterError> {
        let target = self.resolved_target(session_name, handle)?;
        let content = self.dump_screen(session_name, &target.selector, true)?;
        Ok(CaptureSnapshot {
            content: parse_capture_output(&content),
            captured_at: Utc::now(),
            truncated: false,
        })
    }

    fn close(&self, session_name: &str, handle: &str, force: bool) -> Result<(), AdapterError> {
        self.close_handle(session_name, handle, force)
    }

    fn list_targets(&self) -> Result<Vec<ResolvedTarget>, AdapterError> {
        let session_name = std::env::var("ZELLIJ_SESSION_NAME")
            .map_err(|_| AdapterError::ParseError("ZELLIJ_SESSION_NAME is not set".to_string()))?;
        self.list_targets_for_session(&session_name)
    }
}

fn matches_selector(selector: &str, target: &ResolvedTarget) -> bool {
    if selector == "focused" || selector == "focused:true" {
        return target.focused;
    }

    if selector == "unfocused" || selector == "focused:false" {
        return !target.focused;
    }

    if selector == target.selector {
        return true;
    }

    if let Some(stripped) = selector.strip_prefix("id:") {
        return target.pane_id.as_deref() == Some(stripped);
    }

    if selector.starts_with("terminal:") || selector.starts_with("plugin:") {
        return target.pane_id.as_deref() == Some(selector);
    }

    if let Some(stripped) = selector.strip_prefix("title:") {
        return target
            .title
            .as_deref()
            .is_some_and(|title| title.contains(stripped));
    }

    if let Some(stripped) = selector.strip_prefix("command:") {
        return target
            .command
            .as_deref()
            .is_some_and(|command| command.contains(stripped));
    }

    if let Some(stripped) = selector.strip_prefix("tab:") {
        return target
            .tab_name
            .as_deref()
            .is_some_and(|tab| tab.contains(stripped));
    }

    false
}

fn is_terminal_target(target: &ResolvedTarget) -> bool {
    target
        .pane_id
        .as_deref()
        .is_some_and(|pane_id| pane_id.starts_with("terminal:"))
}

fn resolve_spawned_target(
    request: &SpawnRequest,
    before: Vec<ResolvedTarget>,
    after: Vec<ResolvedTarget>,
    command_summary: &str,
) -> Result<ResolvedTarget, AdapterError> {
    let before_ids: HashSet<&str> = before
        .iter()
        .filter_map(|target| target.pane_id.as_deref())
        .collect();
    let candidates: Vec<ResolvedTarget> = after
        .into_iter()
        .filter(is_terminal_target)
        .filter(|target| {
            target
                .pane_id
                .as_deref()
                .is_some_and(|pane_id| !before_ids.contains(pane_id))
        })
        .collect();

    let mut candidates = candidates.clone();

    if let Some(tab_name) = request.tab_name.as_ref() {
        let tab_matches: Vec<_> = candidates
            .iter()
            .filter(|target| target.tab_name.as_deref() == Some(tab_name.as_str()))
            .cloned()
            .collect();
        if !tab_matches.is_empty() {
            candidates = tab_matches;
        }
    }

    if let Some(title) = request.title.as_ref() {
        let title_matches: Vec<_> = candidates
            .iter()
            .filter(|target| target.title.as_deref() == Some(title.as_str()))
            .cloned()
            .collect();
        match title_matches.as_slice() {
            [target] => return Ok(target.clone()),
            [] => {}
            _ => {
                return Err(AdapterError::CommandFailed(format!(
                    "spawned pane title `{title}` matched multiple panes"
                )));
            }
        }
    }

    let command_matches: Vec<_> = candidates
        .iter()
        .filter(|target| target.command.as_deref() == Some(command_summary))
        .cloned()
        .collect();
    match command_matches.as_slice() {
        [target] => Ok(target.clone()),
        [] => match candidates.as_slice() {
            [target] => Ok(target.clone()),
            [] => Err(AdapterError::CommandFailed(
                "no spawned pane could be resolved after creating a new tab".to_string(),
            )),
            _ => Err(AdapterError::CommandFailed(
                "spawn matched multiple candidate panes".to_string(),
            )),
        },
        _ => Err(AdapterError::CommandFailed(
            "spawn command matched multiple candidate panes".to_string(),
        )),
    }
}

fn resolve_spawned_target_from_run_output(
    request: &SpawnRequest,
    before: Vec<ResolvedTarget>,
    after: Vec<ResolvedTarget>,
    run_stdout: &[u8],
    command_summary: &str,
) -> Result<ResolvedTarget, AdapterError> {
    let stdout = String::from_utf8_lossy(run_stdout);
    if let Ok(parsed) = parse_spawn_output(
        &stdout,
        &request.session_name,
        request.tab_name.as_deref(),
        request.title.as_deref(),
    ) {
        if let Some(pane_id) = parsed.pane_id.as_deref()
            && let Some(resolved) = after
                .iter()
                .find(|target| target.pane_id.as_deref() == Some(pane_id))
        {
            return Ok(resolved.clone());
        }

        return Ok(parsed);
    }

    resolve_spawned_target(request, before, after, command_summary)
}

fn parse_action_created_tab_id(stdout: &[u8]) -> Option<String> {
    let trimmed = String::from_utf8_lossy(stdout).trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .chars()
        .all(|ch| ch.is_ascii_digit())
        .then_some(trimmed)
}

fn parse_command(command: &str) -> Result<Vec<String>, AdapterError> {
    shell_words::split(command)
        .map_err(|error| AdapterError::ParseError(format!("invalid spawn command: {error}")))
}

fn resolve_spawn_command(request: &SpawnRequest) -> Result<Vec<String>, AdapterError> {
    match (request.command.as_deref(), request.argv.as_ref()) {
        (Some(_), Some(_)) => Err(AdapterError::ParseError(
            "spawn requires either `command` or `argv`, not both".to_string(),
        )),
        (None, None) => Ok(vec!["fish".to_string()]),
        (Some(command), None) => {
            if command.trim().is_empty() {
                return Err(AdapterError::ParseError(
                    "spawn `command` must not be blank".to_string(),
                ));
            }

            let parsed = parse_command(command)?;
            if parsed.is_empty() {
                return Err(AdapterError::ParseError(
                    "spawn `command` must produce at least one argv element".to_string(),
                ));
            }

            Ok(parsed)
        }
        (None, Some(argv)) if argv.is_empty() => Err(AdapterError::ParseError(
            "spawn `argv` must contain at least one element".to_string(),
        )),
        (None, Some(argv)) if argv[0].trim().is_empty() => Err(AdapterError::ParseError(
            "spawn `argv[0]` must not be blank".to_string(),
        )),
        (None, Some(argv)) => Ok(argv.clone()),
    }
}

fn prepare_new_tab_action_args(request: &SpawnRequest) -> Option<Vec<String>> {
    let mut args = vec!["new-tab".to_string()];
    if let Some(tab_name) = &request.tab_name {
        args.push("--name".to_string());
        args.push(tab_name.clone());
    }
    if let Some(cwd) = &request.cwd {
        args.push("--cwd".to_string());
        args.push(cwd.clone());
    }
    Some(args)
}

fn prepare_spawn_action_args(request: &SpawnRequest) -> Option<Vec<String>> {
    match request.spawn_target {
        SpawnTarget::NewTab => prepare_new_tab_action_args(request),
        SpawnTarget::ExistingTab => request
            .tab_name
            .as_ref()
            .map(|tab_name| vec!["go-to-tab-name".to_string(), tab_name.clone()]),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreparedSpawn {
    Reuse(ResolvedTarget),
    Launch {
        action_args: Option<Vec<String>>,
        command: Vec<String>,
        command_via_action: bool,
    },
    Ambiguous {
        tab_name: String,
        reusable_targets: Vec<ResolvedTarget>,
    },
}

fn shell_name(command: Option<&str>) -> Option<String> {
    let command = command?.trim();
    if command.is_empty() {
        return None;
    }

    let first = command.split_whitespace().next()?.rsplit('/').next()?;
    Some(first.to_ascii_lowercase())
}

fn is_reusable_terminal_target(target: &ResolvedTarget) -> bool {
    is_terminal_target(target)
        && matches!(
            shell_name(target.command.as_deref()).as_deref(),
            Some("sh" | "bash" | "zsh" | "fish")
        )
}

fn is_unsupported_new_tab_initial_command(message: &str) -> bool {
    message.contains("Found argument") && message.contains("wasn't expected")
}

fn is_default_fish_spawn_request(request: &SpawnRequest, command: &[String]) -> bool {
    request.command.is_none()
        && request.argv.is_none()
        && matches!(command, [single] if single == "fish")
}

fn terminal_targets_in_tab(tab_targets: &[ResolvedTarget]) -> Vec<&ResolvedTarget> {
    tab_targets
        .iter()
        .filter(|target| is_terminal_target(target))
        .collect()
}

fn is_reusable_terminal_target_for_request(
    request: &SpawnRequest,
    command: &[String],
    tab_targets: &[ResolvedTarget],
    target: &ResolvedTarget,
) -> bool {
    if is_reusable_terminal_target(target) {
        return true;
    }

    let terminal_targets = terminal_targets_in_tab(tab_targets);

    is_default_fish_spawn_request(request, command)
        && terminal_targets.len() == 1
        && is_terminal_target(target)
        && target.command.is_none()
}

#[cfg_attr(not(test), allow(dead_code))]
fn prepare_spawn(
    request: &SpawnRequest,
    existing_targets: &[ResolvedTarget],
) -> Result<PreparedSpawn, AdapterError> {
    let command = resolve_spawn_command(request)?;

    if matches!(request.spawn_target, SpawnTarget::NewTab) {
        let command_via_action = is_default_fish_spawn_request(request, &command);
        return Ok(PreparedSpawn::Launch {
            action_args: prepare_spawn_action_args(request),
            command,
            command_via_action,
        });
    }

    let Some(tab_name) = request.tab_name.as_deref() else {
        return Ok(PreparedSpawn::Launch {
            action_args: None,
            command,
            command_via_action: false,
        });
    };

    let tab_targets: Vec<ResolvedTarget> = existing_targets
        .iter()
        .filter(|target| target.tab_name.as_deref() == Some(tab_name))
        .cloned()
        .collect();

    if tab_targets.is_empty() {
        let mut action_args = prepare_new_tab_action_args(request)
            .expect("new-tab action args should exist for explicit tab routing");
        let command_via_action = !is_default_fish_spawn_request(request, &command);
        if command_via_action {
            action_args.push("--".to_string());
            action_args.extend(command.clone());
        }
        return Ok(PreparedSpawn::Launch {
            action_args: Some(action_args),
            command,
            command_via_action,
        });
    }

    let reusable_targets: Vec<ResolvedTarget> = tab_targets
        .iter()
        .filter(|target| {
            is_reusable_terminal_target_for_request(request, &command, &tab_targets, target)
        })
        .cloned()
        .collect();
    if let [target] = reusable_targets.as_slice() {
        return Ok(PreparedSpawn::Reuse(target.clone()));
    }

    if reusable_targets.len() > 1 {
        return Ok(PreparedSpawn::Ambiguous {
            tab_name: tab_name.to_string(),
            reusable_targets,
        });
    }

    Ok(PreparedSpawn::Launch {
        action_args: Some(vec!["go-to-tab-name".to_string(), tab_name.to_string()]),
        command,
        command_via_action: false,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn format_seconds(milliseconds: u64) -> String {
    let duration = Duration::from_millis(milliseconds);
    format!("{:.1}", duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashMap};
    use uuid::Uuid;
    use zjctl_proto::{PROTOCOL_VERSION, RpcError, RpcErrorCode, RpcResponse};

    use crate::adapters::zjctl::AdapterError;

    use crate::domain::requests::SpawnRequest;
    use crate::domain::status::SpawnTarget;

    use super::{
        PreparedSpawn, RemoteProbe, RemoteRemediationRunner, ResolvedTarget, SshBackendReadiness,
        SshReadinessFailure, SshTargetConfig,
        attempt_safe_ssh_readiness_remediation_with_probe_and_runner,
        augment_path_with_binary_dirs, classify_ssh_backend_readiness_with_probe, format_seconds,
        helper_client_geometry_from_sources, is_missing_plugin_message,
        is_protocol_version_mismatch_message, is_reusable_terminal_target_for_request,
        is_unsupported_new_tab_initial_command, matches_selector, parse_action_created_tab_id,
        parse_command, parse_rpc_output, prepare_spawn, resolve_spawn_command,
        resolve_spawned_target, resolve_spawned_target_from_run_output,
        resolve_ssh_runtime_config_with_probe, terminal_targets_in_tab,
    };

    #[derive(Default)]
    struct FakeProbe {
        home: Option<String>,
        path: Option<String>,
        command_v: HashMap<String, Option<String>>,
        executables: HashMap<String, bool>,
        rpc_readiness: Option<Result<(), AdapterError>>,
        calls: RefCell<Vec<String>>,
    }

    impl FakeProbe {
        fn record(&self, call: impl Into<String>) {
            self.calls.borrow_mut().push(call.into());
        }
    }

    impl RemoteProbe for FakeProbe {
        fn discover_home(&self) -> Result<String, AdapterError> {
            self.record("discover_home");
            self.home
                .clone()
                .ok_or_else(|| AdapterError::CommandFailed("missing fake home".to_string()))
        }

        fn discover_path(&self) -> Result<String, AdapterError> {
            self.record("discover_path");
            self.path
                .clone()
                .ok_or_else(|| AdapterError::CommandFailed("missing fake path".to_string()))
        }

        fn command_v(
            &self,
            normalized_path: &str,
            binary: &str,
        ) -> Result<Option<String>, AdapterError> {
            self.record(format!("command_v:{binary}:{normalized_path}"));
            Ok(self.command_v.get(binary).cloned().unwrap_or(None))
        }

        fn is_executable(&self, absolute_path: &str) -> Result<bool, AdapterError> {
            self.record(format!("is_executable:{absolute_path}"));
            Ok(*self.executables.get(absolute_path).unwrap_or(&false))
        }

        fn check_rpc_readiness(
            &self,
            _config: &SshTargetConfig,
            _session_name: &str,
        ) -> Result<(), AdapterError> {
            self.record("check_rpc_readiness");
            self.rpc_readiness.clone().unwrap_or(Ok(()))
        }
    }

    #[derive(Default)]
    struct RecordingRemediationRunner {
        commands: RefCell<Vec<String>>,
        failures_remaining: Cell<usize>,
    }

    impl RemoteRemediationRunner for RecordingRemediationRunner {
        fn run(&self, _config: &SshTargetConfig, remote_command: &str) -> Result<(), AdapterError> {
            self.commands.borrow_mut().push(remote_command.to_string());
            let failures_remaining = self.failures_remaining.get();
            if failures_remaining > 0 {
                self.failures_remaining.set(failures_remaining - 1);
                return Err(AdapterError::CommandFailed(
                    "mock remediation failure".to_string(),
                ));
            }
            Ok(())
        }
    }

    #[test]
    fn matches_exact_id_selector() {
        let target = ResolvedTarget {
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("editor".to_string()),
            title: Some("editor".to_string()),
            command: None,
            focused: false,
        };

        assert!(matches_selector("id:terminal:7", &target));
        assert!(matches_selector("terminal:7", &target));
        assert!(!matches_selector("id:terminal:8", &target));
    }

    #[test]
    fn matches_title_selector_against_tab_name() {
        let target = ResolvedTarget {
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("editor-main".to_string()),
            title: Some("editor-main".to_string()),
            command: None,
            focused: false,
        };

        assert!(matches_selector("title:editor", &target));
    }

    #[test]
    fn matches_command_and_focus_selectors() {
        let target = ResolvedTarget {
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("editor-main".to_string()),
            title: Some("editor-main".to_string()),
            command: Some("bash -lc cargo test".to_string()),
            focused: true,
        };

        assert!(matches_selector("command:cargo test", &target));
        assert!(matches_selector("tab:editor", &target));
        assert!(matches_selector("focused", &target));
        assert!(matches_selector("focused:true", &target));
        assert!(!matches_selector("focused:false", &target));
    }

    #[test]
    fn parses_quoted_spawn_command() {
        assert_eq!(
            parse_command("bash -lc 'echo hello world'").expect("command should parse"),
            vec!["bash", "-lc", "echo hello world"]
        );
    }

    #[test]
    fn rejects_invalid_quoted_spawn_command() {
        let error = parse_command("bash -lc 'echo hello").expect_err("command should fail");
        assert!(matches!(error, AdapterError::ParseError(_)));
    }

    #[test]
    fn spawn_string_form_preserves_shell_parsing() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("editor".to_string()),
            cwd: None,
            command: Some("bash -lc 'echo hello world'".to_string()),
            argv: None,
            title: None,
            wait_ready: false,
        };

        assert_eq!(
            resolve_spawn_command(&request).expect("command should resolve"),
            vec!["bash", "-lc", "echo hello world"]
        );
    }

    #[test]
    fn spawn_argv_form_bypasses_shell_parsing() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("editor".to_string()),
            cwd: None,
            command: None,
            argv: Some(vec![
                "bash".to_string(),
                "-lc".to_string(),
                "echo $HOME".to_string(),
            ]),
            title: None,
            wait_ready: false,
        };

        assert_eq!(
            resolve_spawn_command(&request).expect("argv should resolve"),
            vec!["bash", "-lc", "echo $HOME"]
        );
    }

    #[test]
    fn spawn_defaults_to_interactive_fish_when_command_is_omitted() {
        let request: SpawnRequest = serde_json::from_value(serde_json::json!({
            "session_name": "gpu",
            "tab_name": "editor",
            "wait_ready": false
        }))
        .expect("spawn request should deserialize with defaults");

        assert_eq!(request.spawn_target, SpawnTarget::ExistingTab);
        assert_eq!(
            resolve_spawn_command(&request).expect("default command should resolve"),
            vec!["fish".to_string()]
        );
    }

    #[test]
    fn spawn_rejects_command_and_argv_together() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("editor".to_string()),
            cwd: None,
            command: Some("git status".to_string()),
            argv: Some(vec!["git".to_string(), "status".to_string()]),
            title: None,
            wait_ready: false,
        };

        let error = resolve_spawn_command(&request).expect_err("mixed command forms should fail");
        assert!(matches!(error, AdapterError::ParseError(_)));
        assert!(
            error
                .to_string()
                .contains("either `command` or `argv`, not both")
        );
    }

    #[test]
    fn prepare_spawn_reuses_single_shell_like_terminal_in_requested_tab() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("editor".to_string()),
            cwd: None,
            command: None,
            argv: None,
            title: None,
            wait_ready: false,
        };
        let existing_targets = vec![ResolvedTarget {
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("editor".to_string()),
            title: Some("shell".to_string()),
            command: Some("fish".to_string()),
            focused: true,
        }];

        assert_eq!(
            prepare_spawn(&request, &existing_targets).expect("spawn should plan a reuse"),
            PreparedSpawn::Reuse(existing_targets[0].clone())
        );
    }

    #[test]
    fn spawn_rejects_empty_argv() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("editor".to_string()),
            cwd: None,
            command: None,
            argv: Some(vec![]),
            title: None,
            wait_ready: false,
        };

        let error = resolve_spawn_command(&request).expect_err("empty argv should fail");
        assert!(matches!(error, AdapterError::ParseError(_)));
        assert!(
            error
                .to_string()
                .contains("must contain at least one element")
        );
    }

    #[test]
    fn spawn_rejects_blank_argv_zero() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("editor".to_string()),
            cwd: None,
            command: None,
            argv: Some(vec!["   ".to_string(), "status".to_string()]),
            title: None,
            wait_ready: false,
        };

        let error = resolve_spawn_command(&request).expect_err("blank argv[0] should fail");
        assert!(matches!(error, AdapterError::ParseError(_)));
        assert!(error.to_string().contains("`argv[0]` must not be blank"));
    }

    #[test]
    fn spawn_rejects_blank_command() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::NewTab,
            tab_name: Some("editor".to_string()),
            cwd: None,
            command: Some("   ".to_string()),
            argv: None,
            title: None,
            wait_ready: false,
        };

        let error = resolve_spawn_command(&request).expect_err("blank command should fail");
        assert!(matches!(error, AdapterError::ParseError(_)));
        assert!(error.to_string().contains("must not be blank"));
    }

    #[test]
    fn invalid_spawn_input_fails_before_any_tab_action_plan() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::NewTab,
            tab_name: Some("editor".to_string()),
            cwd: Some("/tmp".to_string()),
            command: Some("git status".to_string()),
            argv: Some(vec!["git".to_string(), "status".to_string()]),
            title: None,
            wait_ready: false,
        };

        let error =
            prepare_spawn(&request, &[]).expect_err("invalid spawn should fail before planning");
        assert!(matches!(error, AdapterError::ParseError(_)));
    }

    #[test]
    fn prepare_spawn_builds_tab_action_after_validation() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::NewTab,
            tab_name: Some("editor".to_string()),
            cwd: Some("/tmp".to_string()),
            command: Some("git status".to_string()),
            argv: None,
            title: Some("git-status".to_string()),
            wait_ready: false,
        };

        assert_eq!(
            prepare_spawn(&request, &[]).expect("spawn should prepare"),
            PreparedSpawn::Launch {
                action_args: Some(vec![
                    "new-tab".to_string(),
                    "--name".to_string(),
                    "editor".to_string(),
                    "--cwd".to_string(),
                    "/tmp".to_string(),
                ]),
                command: vec!["git".to_string(), "status".to_string()],
                command_via_action: false,
            }
        );
    }

    #[test]
    fn prepare_spawn_creates_new_tab_when_requested_tab_is_missing() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("editor".to_string()),
            cwd: Some("/tmp".to_string()),
            command: None,
            argv: None,
            title: None,
            wait_ready: false,
        };

        assert_eq!(
            prepare_spawn(&request, &[]).expect("missing requested tab should create one"),
            PreparedSpawn::Launch {
                action_args: Some(vec![
                    "new-tab".to_string(),
                    "--name".to_string(),
                    "editor".to_string(),
                    "--cwd".to_string(),
                    "/tmp".to_string(),
                ]),
                command: vec!["fish".to_string()],
                command_via_action: false,
            }
        );
    }

    #[test]
    fn prepare_spawn_new_tab_default_fish_binds_default_pane_without_extra_launch() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::NewTab,
            tab_name: Some("docker".to_string()),
            cwd: None,
            command: None,
            argv: None,
            title: None,
            wait_ready: false,
        };

        assert_eq!(
            prepare_spawn(&request, &[]).expect("new-tab default fish should prepare"),
            PreparedSpawn::Launch {
                action_args: Some(vec![
                    "new-tab".to_string(),
                    "--name".to_string(),
                    "docker".to_string(),
                ]),
                command: vec!["fish".to_string()],
                command_via_action: true,
            }
        );
    }

    #[test]
    fn prepare_spawn_returns_ambiguity_when_tab_has_multiple_reusable_terminals() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("docker".to_string()),
            cwd: None,
            command: None,
            argv: None,
            title: None,
            wait_ready: false,
        };
        let targets = vec![
            ResolvedTarget {
                selector: "id:terminal:2".to_string(),
                pane_id: Some("terminal:2".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("docker".to_string()),
                title: Some("left".to_string()),
                command: Some("fish".to_string()),
                focused: false,
            },
            ResolvedTarget {
                selector: "id:terminal:3".to_string(),
                pane_id: Some("terminal:3".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("docker".to_string()),
                title: Some("right".to_string()),
                command: Some("fish".to_string()),
                focused: true,
            },
        ];

        assert!(matches!(
            prepare_spawn(&request, &targets).expect("planning should succeed"),
            PreparedSpawn::Ambiguous { .. }
        ));
    }

    #[test]
    fn detects_old_remote_new_tab_rejecting_trailing_command() {
        assert!(is_unsupported_new_tab_initial_command(
            "error: Found argument 'fish' which wasn't expected"
        ));
        assert!(!is_unsupported_new_tab_initial_command(
            "error: failed to contact session"
        ));
    }

    #[test]
    fn prepare_spawn_reuses_requested_existing_tab_when_exactly_one_shell_like_terminal_exists() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("editor".to_string()),
            cwd: None,
            command: None,
            argv: None,
            title: None,
            wait_ready: false,
        };
        let existing_targets = vec![
            ResolvedTarget {
                selector: "id:terminal:7".to_string(),
                pane_id: Some("terminal:7".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                title: Some("shell".to_string()),
                command: Some("fish".to_string()),
                focused: true,
            },
            ResolvedTarget {
                selector: "id:terminal:8".to_string(),
                pane_id: Some("terminal:8".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                title: Some("job".to_string()),
                command: Some("cargo test".to_string()),
                focused: false,
            },
        ];

        assert_eq!(
            prepare_spawn(&request, &existing_targets)
                .expect("existing tab with one reusable shell should be reused"),
            PreparedSpawn::Reuse(existing_targets[0].clone())
        );
    }

    #[test]
    fn resolve_spawned_target_accepts_single_new_terminal_in_requested_tab_without_command_match() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("reuse-remote-fix2".to_string()),
            cwd: None,
            command: None,
            argv: None,
            title: None,
            wait_ready: false,
        };
        let before = vec![];
        let after = vec![ResolvedTarget {
            selector: "id:terminal:1".to_string(),
            pane_id: Some("terminal:1".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("reuse-remote-fix2".to_string()),
            title: Some("shell".to_string()),
            command: Some("bash".to_string()),
            focused: true,
        }];

        let resolved = resolve_spawned_target(&request, before, after, "fish")
            .expect("single new terminal in requested tab should still reconcile");

        assert_eq!(resolved.pane_id.as_deref(), Some("terminal:1"));
        assert_eq!(resolved.tab_name.as_deref(), Some("reuse-remote-fix2"));
    }

    #[test]
    fn prepare_spawn_reuses_second_identical_default_spawn_after_missing_tab_creation() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("reuse-local".to_string()),
            cwd: None,
            command: None,
            argv: None,
            title: None,
            wait_ready: false,
        };
        let existing_targets = vec![ResolvedTarget {
            selector: "id:terminal:2".to_string(),
            pane_id: Some("terminal:2".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("reuse-local".to_string()),
            title: Some("shell".to_string()),
            command: Some("fish".to_string()),
            focused: true,
        }];

        assert_eq!(
            prepare_spawn(&request, &existing_targets)
                .expect("second default spawn should reuse the single fish terminal"),
            PreparedSpawn::Reuse(existing_targets[0].clone())
        );
    }

    #[test]
    fn parse_action_created_tab_id_accepts_integer_stdout() {
        assert_eq!(parse_action_created_tab_id(b"17\n"), Some("17".to_string()));
        assert_eq!(parse_action_created_tab_id(b"\n"), None);
        assert_eq!(parse_action_created_tab_id(b"terminal_7\n"), None);
    }

    #[test]
    fn old_remote_single_terminal_with_missing_command_is_reusable_for_default_fish_spawn() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("reuse-remote-fix3".to_string()),
            cwd: None,
            command: None,
            argv: None,
            title: None,
            wait_ready: false,
        };
        let tab_targets = vec![ResolvedTarget {
            selector: "id:terminal:1".to_string(),
            pane_id: Some("terminal:1".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("reuse-remote-fix3".to_string()),
            title: Some("~".to_string()),
            command: None,
            focused: true,
        }];

        assert!(is_reusable_terminal_target_for_request(
            &request,
            &["fish".to_string()],
            &tab_targets,
            &tab_targets[0],
        ));
        assert_eq!(
            prepare_spawn(&request, &tab_targets)
                .expect("second default spawn should reuse compat-created single pane"),
            PreparedSpawn::Reuse(tab_targets[0].clone())
        );
    }

    #[test]
    fn old_remote_single_terminal_with_plugin_pane_is_still_reusable_for_default_fish_spawn() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("reuse-remote-verify5".to_string()),
            cwd: None,
            command: None,
            argv: None,
            title: None,
            wait_ready: false,
        };
        let tab_targets = vec![
            ResolvedTarget {
                selector: "id:plugin:1".to_string(),
                pane_id: Some("plugin:1".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("reuse-remote-verify5".to_string()),
                title: Some("zrpc".to_string()),
                command: None,
                focused: false,
            },
            ResolvedTarget {
                selector: "id:terminal:1".to_string(),
                pane_id: Some("terminal:1".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("reuse-remote-verify5".to_string()),
                title: Some("~".to_string()),
                command: None,
                focused: true,
            },
        ];

        let terminal_targets = terminal_targets_in_tab(&tab_targets);
        assert_eq!(terminal_targets.len(), 1);
        assert_eq!(terminal_targets[0].pane_id.as_deref(), Some("terminal:1"));
        assert!(is_reusable_terminal_target_for_request(
            &request,
            &["fish".to_string()],
            &tab_targets,
            &tab_targets[1],
        ));
        assert_eq!(
            prepare_spawn(&request, &tab_targets)
                .expect("second default spawn should reuse compat-created terminal even with plugin pane present"),
            PreparedSpawn::Reuse(tab_targets[1].clone())
        );
    }

    #[test]
    fn missing_command_terminal_is_not_reused_for_non_default_spawn_requests() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("reuse-remote-fix3".to_string()),
            cwd: None,
            command: Some("lazygit".to_string()),
            argv: None,
            title: None,
            wait_ready: false,
        };
        let tab_targets = vec![ResolvedTarget {
            selector: "id:terminal:1".to_string(),
            pane_id: Some("terminal:1".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("reuse-remote-fix3".to_string()),
            title: Some("~".to_string()),
            command: None,
            focused: true,
        }];

        assert!(!is_reusable_terminal_target_for_request(
            &request,
            &["lazygit".to_string()],
            &tab_targets,
            &tab_targets[0],
        ));
        assert!(matches!(
            prepare_spawn(&request, &tab_targets)
                .expect("non-default spawn should still launch a new pane in existing tab"),
            PreparedSpawn::Launch { .. }
        ));
    }

    #[test]
    fn null_command_terminal_with_plugin_pane_is_not_reused_for_non_default_spawn_requests() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::ExistingTab,
            tab_name: Some("reuse-remote-verify5".to_string()),
            cwd: None,
            command: Some("lazygit".to_string()),
            argv: None,
            title: None,
            wait_ready: false,
        };
        let tab_targets = vec![
            ResolvedTarget {
                selector: "id:plugin:1".to_string(),
                pane_id: Some("plugin:1".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("reuse-remote-verify5".to_string()),
                title: Some("zrpc".to_string()),
                command: None,
                focused: false,
            },
            ResolvedTarget {
                selector: "id:terminal:1".to_string(),
                pane_id: Some("terminal:1".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("reuse-remote-verify5".to_string()),
                title: Some("~".to_string()),
                command: None,
                focused: true,
            },
        ];

        assert!(!is_reusable_terminal_target_for_request(
            &request,
            &["lazygit".to_string()],
            &tab_targets,
            &tab_targets[1],
        ));
        assert!(matches!(
            prepare_spawn(&request, &tab_targets)
                .expect("non-default spawn should still create a new pane when only null-command terminal is present"),
            PreparedSpawn::Launch { .. }
        ));
    }

    #[test]
    fn prepare_spawn_respects_explicit_new_tab_even_when_reusable_terminal_exists() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::NewTab,
            tab_name: Some("editor".to_string()),
            cwd: None,
            command: None,
            argv: None,
            title: None,
            wait_ready: false,
        };
        let existing_targets = vec![ResolvedTarget {
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("editor".to_string()),
            title: Some("shell".to_string()),
            command: Some("fish".to_string()),
            focused: true,
        }];

        assert_eq!(
            prepare_spawn(&request, &existing_targets)
                .expect("explicit new-tab should bypass reuse"),
            PreparedSpawn::Launch {
                action_args: Some(vec![
                    "new-tab".to_string(),
                    "--name".to_string(),
                    "editor".to_string(),
                ]),
                command: vec!["fish".to_string()],
                command_via_action: true,
            }
        );
    }

    #[test]
    fn formats_wait_durations_as_seconds() {
        assert_eq!(format_seconds(1200), "1.2");
    }

    #[test]
    fn resolve_new_tab_target_prefers_exact_title_match() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::NewTab,
            tab_name: Some("logs".to_string()),
            cwd: None,
            command: Some("bash -lc 'while true; do echo tick; sleep 0.2; done'".to_string()),
            argv: None,
            title: Some("repro-busy".to_string()),
            wait_ready: true,
        };
        let before = vec![ResolvedTarget {
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("editor".to_string()),
            title: Some("shell".to_string()),
            command: Some("fish".to_string()),
            focused: false,
        }];
        let after = vec![
            before[0].clone(),
            ResolvedTarget {
                selector: "id:terminal:10".to_string(),
                pane_id: Some("terminal:10".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("logs".to_string()),
                title: Some("shell".to_string()),
                command: None,
                focused: false,
            },
            ResolvedTarget {
                selector: "id:terminal:11".to_string(),
                pane_id: Some("terminal:11".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("logs".to_string()),
                title: Some("repro-busy".to_string()),
                command: Some("bash -lc while true; do echo tick; sleep 0.2; done".to_string()),
                focused: true,
            },
        ];

        let resolved = resolve_spawned_target(
            &request,
            before,
            after,
            "bash -lc while true; do echo tick; sleep 0.2; done",
        )
        .expect("new tab target should resolve");

        assert_eq!(resolved.pane_id.as_deref(), Some("terminal:11"));
    }

    #[test]
    fn resolve_new_tab_target_falls_back_to_command_match_without_title() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::NewTab,
            tab_name: Some("logs".to_string()),
            cwd: None,
            command: Some("lazygit".to_string()),
            argv: None,
            title: None,
            wait_ready: false,
        };
        let before = vec![];
        let after = vec![
            ResolvedTarget {
                selector: "id:terminal:12".to_string(),
                pane_id: Some("terminal:12".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("logs".to_string()),
                title: Some("Pane #1".to_string()),
                command: None,
                focused: false,
            },
            ResolvedTarget {
                selector: "id:terminal:13".to_string(),
                pane_id: Some("terminal:13".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("logs".to_string()),
                title: Some("Pane #2".to_string()),
                command: Some("lazygit".to_string()),
                focused: true,
            },
        ];

        let resolved = resolve_spawned_target(&request, before, after, "lazygit")
            .expect("new tab target should resolve from command");

        assert_eq!(resolved.pane_id.as_deref(), Some("terminal:13"));
    }

    #[test]
    fn resolve_new_tab_target_ignores_non_matching_requested_tab_name_when_command_is_unique() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::NewTab,
            tab_name: Some("verify-live".to_string()),
            cwd: None,
            command: Some("bash -lc \"printf hello\\n; exec bash\"".to_string()),
            argv: None,
            title: Some("hello-live".to_string()),
            wait_ready: false,
        };
        let before = vec![ResolvedTarget {
            selector: "id:terminal:1".to_string(),
            pane_id: Some("terminal:1".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("Tab 1".to_string()),
            title: Some("shell".to_string()),
            command: Some("fish".to_string()),
            focused: false,
        }];
        let after = vec![
            before[0].clone(),
            ResolvedTarget {
                selector: "id:terminal:8".to_string(),
                pane_id: Some("terminal:8".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("Tab 3".to_string()),
                title: Some("Pane #1".to_string()),
                command: None,
                focused: false,
            },
            ResolvedTarget {
                selector: "id:terminal:9".to_string(),
                pane_id: Some("terminal:9".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("Tab 3".to_string()),
                title: Some("shell".to_string()),
                command: Some("bash -lc \"printf hello\\n; exec bash\"".to_string()),
                focused: true,
            },
        ];

        let resolved = resolve_spawned_target(
            &request,
            before,
            after,
            "bash -lc \"printf hello\\n; exec bash\"",
        )
        .expect("spawn should resolve even if the runtime tab name differs");

        assert_eq!(resolved.pane_id.as_deref(), Some("terminal:9"));
        assert_eq!(resolved.tab_name.as_deref(), Some("Tab 3"));
    }

    #[test]
    fn resolve_spawn_target_prefers_exact_run_output_pane_id() {
        let request = SpawnRequest {
            target: None,
            session_name: "gpu".to_string(),
            spawn_target: SpawnTarget::NewTab,
            tab_name: Some("verify-live".to_string()),
            cwd: None,
            command: Some("bash".to_string()),
            argv: None,
            title: Some("bash-live".to_string()),
            wait_ready: false,
        };
        let before = vec![];
        let after = vec![
            ResolvedTarget {
                selector: "id:terminal:14".to_string(),
                pane_id: Some("terminal:14".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("Tab 6".to_string()),
                title: Some("~/repo - fish".to_string()),
                command: None,
                focused: false,
            },
            ResolvedTarget {
                selector: "id:terminal:15".to_string(),
                pane_id: Some("terminal:15".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("Tab 6".to_string()),
                title: Some("~/repo - fish".to_string()),
                command: Some("bash".to_string()),
                focused: true,
            },
        ];

        let resolved = resolve_spawned_target_from_run_output(
            &request,
            before,
            after,
            b"terminal_15\n",
            "bash",
        )
        .expect("spawn should resolve from zellij run output");

        assert_eq!(resolved.pane_id.as_deref(), Some("terminal:15"));
    }

    #[test]
    fn augment_path_with_binary_dirs_adds_explicit_binary_parents() {
        let path = augment_path_with_binary_dirs(
            Some("/usr/bin:/bin".to_string()),
            &[
                "/home/remote/.local/bin/zellij",
                "/home/remote/.local/bin/zjctl",
            ],
        )
        .expect("path should be produced");

        assert_eq!(path, "/home/remote/.local/bin:/usr/bin:/bin");
    }

    #[test]
    fn remote_probe_discovers_default_home_local_bin_paths() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let home = "/home/remote";
        let path = "/usr/local/bin:/usr/bin:/bin";
        let probe = FakeProbe {
            home: Some(home.to_string()),
            path: Some(path.to_string()),
            command_v: HashMap::from([("zjctl".to_string(), None), ("zellij".to_string(), None)]),
            executables: HashMap::from([
                (format!("{home}/.local/bin/zjctl"), true),
                (format!("{home}/.local/bin/zellij"), true),
            ]),
            rpc_readiness: None,
            calls: RefCell::new(Vec::new()),
        };

        let resolved = resolve_ssh_runtime_config_with_probe(&config, &probe)
            .expect("default binaries should resolve from remote home local bin");

        assert_eq!(
            resolved.remote_zjctl_bin,
            format!("{home}/.local/bin/zjctl")
        );
        assert_eq!(
            resolved.remote_zellij_bin,
            format!("{home}/.local/bin/zellij")
        );
        assert_eq!(
            resolved.remote_env.get("PATH"),
            Some(&format!("{home}/.local/bin:{path}"))
        );
        assert_eq!(probe.calls.borrow()[0], "discover_home");
    }

    #[test]
    fn remote_probe_prepends_home_local_bin_for_noninteractive_ssh() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let home = "/home/remote";
        let path = "/usr/local/bin:/usr/bin:/bin";
        let normalized_path = format!("{home}/.local/bin:{path}");
        let probe = FakeProbe {
            home: Some(home.to_string()),
            path: Some(path.to_string()),
            command_v: HashMap::from([
                (
                    "zjctl".to_string(),
                    Some(format!("{home}/.local/bin/zjctl")),
                ),
                (
                    "zellij".to_string(),
                    Some(format!("{home}/.local/bin/zellij")),
                ),
            ]),
            executables: HashMap::new(),
            rpc_readiness: None,
            calls: RefCell::new(Vec::new()),
        };

        let resolved = resolve_ssh_runtime_config_with_probe(&config, &probe)
            .expect("normalized PATH should allow command -v to resolve binaries");

        assert_eq!(
            resolved.remote_zjctl_bin,
            format!("{home}/.local/bin/zjctl")
        );
        assert_eq!(
            resolved.remote_zellij_bin,
            format!("{home}/.local/bin/zellij")
        );
        assert!(
            probe
                .calls
                .borrow()
                .iter()
                .any(|call| call == &format!("command_v:zjctl:{normalized_path}"))
        );
        assert!(
            probe
                .calls
                .borrow()
                .iter()
                .any(|call| call == &format!("command_v:zellij:{normalized_path}"))
        );
    }

    #[test]
    fn dump_screen_wrappers_use_shell_command_without_exec() {
        let backend = super::SshBackend {
            config: SshTargetConfig {
                host: "a100".to_string(),
                remote_zjctl_bin: "/home/test/.local/bin/zjctl".to_string(),
                remote_zellij_bin: "/home/test/.local/bin/zellij".to_string(),
                remote_env: BTreeMap::new(),
                ssh_options: Vec::new(),
            },
        };

        let command = backend.build_remote_shell_command(
            &backend.config.remote_zellij_bin,
            &backend.remote_env_assignments(Some("a100")),
            &[
                "action".to_string(),
                "dump-screen".to_string(),
                "--pane-id".to_string(),
                "terminal_4".to_string(),
                "--path".to_string(),
                "$tmp".to_string(),
                "--full".to_string(),
            ],
        );

        assert!(command.starts_with("env "));
        assert!(!command.starts_with("exec env "));
    }

    #[test]
    fn remote_probe_prefers_explicit_override_before_conventions() {
        let mut remote_env = BTreeMap::new();
        remote_env.insert("ZELLIJ_SESSION_NAME".to_string(), "a100".to_string());
        let config = SshTargetConfig {
            host: "a100".to_string(),
            remote_zjctl_bin: "/opt/zjctl/bin/zjctl".to_string(),
            remote_zellij_bin: "/opt/zellij/bin/zellij".to_string(),
            remote_env: remote_env.clone(),
            ssh_options: vec!["-p".to_string(), "2222".to_string()],
        };
        let probe = FakeProbe::default();

        let resolved = resolve_ssh_runtime_config_with_probe(&config, &probe)
            .expect("explicit overrides should bypass conventional probing");

        assert_eq!(resolved, config);
        assert!(probe.calls.borrow().is_empty());
    }

    #[test]
    fn readiness_classifies_plugin_permission_prompt_as_manual_action() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let home = "/home/remote";
        let probe = FakeProbe {
            home: Some(home.to_string()),
            path: Some("/usr/local/bin:/usr/bin:/bin".to_string()),
            command_v: HashMap::from([
                (
                    "zjctl".to_string(),
                    Some(format!("{home}/.local/bin/zjctl")),
                ),
                (
                    "zellij".to_string(),
                    Some(format!("{home}/.local/bin/zellij")),
                ),
            ]),
            executables: HashMap::new(),
            rpc_readiness: Some(Err(AdapterError::CommandFailed(
                "Plugin permission prompt requires approval before zjctl RPC can continue"
                    .to_string(),
            ))),
            calls: RefCell::new(Vec::new()),
        };

        let readiness = classify_ssh_backend_readiness_with_probe(&config, "aws", &probe);

        assert_eq!(
            readiness,
            SshBackendReadiness::ManualActionRequired(
                SshReadinessFailure::PluginPermissionPrompt {
                    host: "aws".to_string(),
                    detail:
                        "Plugin permission prompt requires approval before zjctl RPC can continue"
                            .to_string(),
                }
            )
        );
    }

    #[test]
    fn readiness_classifies_missing_binaries_as_auto_fixable() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let probe = FakeProbe {
            home: Some("/home/remote".to_string()),
            path: Some("/usr/local/bin:/usr/bin:/bin".to_string()),
            command_v: HashMap::from([("zjctl".to_string(), None), ("zellij".to_string(), None)]),
            executables: HashMap::new(),
            rpc_readiness: None,
            calls: RefCell::new(Vec::new()),
        };

        let readiness = classify_ssh_backend_readiness_with_probe(&config, "aws", &probe);

        assert_eq!(
            readiness,
            SshBackendReadiness::AutoFixable(SshReadinessFailure::MissingBinary {
                host: "aws".to_string(),
                binary: "zellij".to_string(),
            })
        );
    }

    #[test]
    fn readiness_classifies_rpc_timeout_as_auto_fixable() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "/home/remote/.local/bin/zjctl".to_string(),
            remote_zellij_bin: "/home/remote/.local/bin/zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let probe = FakeProbe {
            home: None,
            path: None,
            command_v: HashMap::new(),
            executables: HashMap::new(),
            rpc_readiness: Some(Err(AdapterError::Timeout)),
            calls: RefCell::new(Vec::new()),
        };

        let readiness = classify_ssh_backend_readiness_with_probe(&config, "aws", &probe);

        assert_eq!(
            readiness,
            SshBackendReadiness::AutoFixable(SshReadinessFailure::RpcNotReady {
                host: "aws".to_string(),
                detail: "zjctl RPC readiness check timed out".to_string(),
            })
        );
    }

    #[test]
    fn readiness_classifies_helper_client_absence_as_auto_fixable() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "/home/remote/.local/bin/zjctl".to_string(),
            remote_zellij_bin: "/home/remote/.local/bin/zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let probe = FakeProbe {
            home: None,
            path: None,
            command_v: HashMap::new(),
            executables: HashMap::new(),
            rpc_readiness: Some(Err(AdapterError::CommandFailed(
                "helper client is not attached yet".to_string(),
            ))),
            calls: RefCell::new(Vec::new()),
        };

        let readiness = classify_ssh_backend_readiness_with_probe(&config, "aws", &probe);

        assert_eq!(
            readiness,
            SshBackendReadiness::AutoFixable(SshReadinessFailure::HelperClientMissing {
                host: "aws".to_string(),
                detail: "helper client is not attached yet".to_string(),
            })
        );
    }

    #[test]
    fn readiness_classifies_missing_plugin_distinctly() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "/home/remote/.local/bin/zjctl".to_string(),
            remote_zellij_bin: "/home/remote/.local/bin/zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let message =
            "zrpc plugin not found at /home/remote/.config/zellij/plugins/zrpc.wasm".to_string();
        let probe = FakeProbe {
            home: None,
            path: None,
            command_v: HashMap::new(),
            executables: HashMap::new(),
            rpc_readiness: Some(Err(AdapterError::CommandFailed(message.clone()))),
            calls: RefCell::new(Vec::new()),
        };

        let readiness = classify_ssh_backend_readiness_with_probe(&config, "aws", &probe);

        assert_eq!(
            readiness,
            SshBackendReadiness::ManualActionRequired(SshReadinessFailure::MissingPlugin {
                host: "aws".to_string(),
                detail: message,
            })
        );
    }

    #[test]
    fn readiness_classifies_no_active_session_as_helper_client_missing() {
        let config = SshTargetConfig {
            host: "a100".to_string(),
            remote_zjctl_bin: "/home/remote/.local/bin/zjctl".to_string(),
            remote_zellij_bin: "/home/remote/.local/bin/zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let probe = FakeProbe {
            home: None,
            path: None,
            command_v: HashMap::new(),
            executables: HashMap::new(),
            rpc_readiness: Some(Err(AdapterError::CommandFailed(
                "There is no active session!".to_string(),
            ))),
            calls: RefCell::new(Vec::new()),
        };

        let readiness = classify_ssh_backend_readiness_with_probe(&config, "a100", &probe);

        assert_eq!(
            readiness,
            SshBackendReadiness::AutoFixable(SshReadinessFailure::HelperClientMissing {
                host: "a100".to_string(),
                detail: "There is no active session!".to_string(),
            })
        );
    }

    #[test]
    fn readiness_classifies_zrpc_no_response_as_rpc_not_ready() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "/home/remote/.local/bin/zjctl".to_string(),
            remote_zellij_bin: "/home/remote/.local/bin/zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let probe = FakeProbe {
            home: None,
            path: None,
            command_v: HashMap::new(),
            executables: HashMap::new(),
            rpc_readiness: Some(Err(AdapterError::CommandFailed(
                "Error: no response from zrpc plugin".to_string(),
            ))),
            calls: RefCell::new(Vec::new()),
        };

        let readiness = classify_ssh_backend_readiness_with_probe(&config, "aws", &probe);

        assert_eq!(
            readiness,
            SshBackendReadiness::AutoFixable(SshReadinessFailure::RpcNotReady {
                host: "aws".to_string(),
                detail: "Error: no response from zrpc plugin".to_string(),
            })
        );
    }

    #[test]
    fn readiness_classifies_protocol_version_mismatch_distinctly() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "/home/remote/.local/bin/zjctl".to_string(),
            remote_zellij_bin: "/home/remote/.local/bin/zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let message = format!(
            "zrpc protocol version mismatch: expected {}, got {}",
            PROTOCOL_VERSION,
            PROTOCOL_VERSION + 1
        );
        let probe = FakeProbe {
            home: None,
            path: None,
            command_v: HashMap::new(),
            executables: HashMap::new(),
            rpc_readiness: Some(Err(AdapterError::CommandFailed(message.clone()))),
            calls: RefCell::new(Vec::new()),
        };

        let readiness = classify_ssh_backend_readiness_with_probe(&config, "aws", &probe);

        assert_eq!(
            readiness,
            SshBackendReadiness::ManualActionRequired(
                SshReadinessFailure::ProtocolVersionMismatch {
                    host: "aws".to_string(),
                    detail: message,
                }
            )
        );
    }

    #[test]
    fn readiness_auto_fix_starts_helper_client_when_missing() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let home = "/home/remote";
        let probe = FakeProbe {
            home: Some(home.to_string()),
            path: Some("/usr/local/bin:/usr/bin:/bin".to_string()),
            command_v: HashMap::from([
                (
                    "zjctl".to_string(),
                    Some(format!("{home}/.local/bin/zjctl")),
                ),
                (
                    "zellij".to_string(),
                    Some(format!("{home}/.local/bin/zellij")),
                ),
                ("tmux".to_string(), Some("/usr/bin/tmux".to_string())),
            ]),
            executables: HashMap::new(),
            rpc_readiness: None,
            calls: RefCell::new(Vec::new()),
        };
        let runner = RecordingRemediationRunner::default();

        let remediated = attempt_safe_ssh_readiness_remediation_with_probe_and_runner(
            &config,
            "aws",
            &SshReadinessFailure::MissingBinary {
                host: "aws".to_string(),
                binary: "zjctl".to_string(),
            },
            &probe,
            &runner,
        );

        assert!(remediated);
        assert!(
            runner
                .commands
                .borrow()
                .iter()
                .any(|command| command.contains("zellij-mcp-client-aws"))
        );
        assert!(
            runner
                .commands
                .borrow()
                .iter()
                .any(|command| command.contains("new-session -d -x 160 -y 48 -s"))
        );
        assert!(
            runner
                .commands
                .borrow()
                .iter()
                .any(|command| command.contains("action") && command.contains("launch-plugin"))
        );
    }

    #[test]
    fn readiness_auto_fix_helper_attach_does_not_export_target_session_env() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::from([(
                "ZELLIJ_SESSION_NAME".to_string(),
                "i1-proof-helper-20260327".to_string(),
            )]),
            ssh_options: Vec::new(),
        };
        let home = "/home/remote";
        let probe = FakeProbe {
            home: Some(home.to_string()),
            path: Some("/usr/local/bin:/usr/bin:/bin".to_string()),
            command_v: HashMap::from([
                (
                    "zjctl".to_string(),
                    Some(format!("{home}/.local/bin/zjctl")),
                ),
                (
                    "zellij".to_string(),
                    Some(format!("{home}/.local/bin/zellij")),
                ),
                ("tmux".to_string(), Some("/usr/bin/tmux".to_string())),
            ]),
            executables: HashMap::new(),
            rpc_readiness: None,
            calls: RefCell::new(Vec::new()),
        };
        let runner = RecordingRemediationRunner::default();

        let remediated = attempt_safe_ssh_readiness_remediation_with_probe_and_runner(
            &config,
            "i1-proof-helper-20260327",
            &SshReadinessFailure::HelperClientMissing {
                host: "aws".to_string(),
                detail: "Plugins must have a client id, none was provided and none are connected"
                    .to_string(),
            },
            &probe,
            &runner,
        );

        assert!(remediated);
        let helper_command = runner
            .commands
            .borrow()
            .iter()
            .find(|command| command.contains("new-session -d -x 160 -y 48 -s"))
            .cloned()
            .expect("helper start command should be recorded");
        assert!(helper_command.contains("attach"));
        assert!(helper_command.contains("i1-proof-helper-20260327"));
        assert!(!helper_command.contains("ZELLIJ_SESSION_NAME=i1-proof-helper-20260327"));
    }

    #[test]
    fn readiness_auto_fix_launches_repo_owned_plugin_before_retry() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let home = "/home/remote";
        let probe = FakeProbe {
            home: Some(home.to_string()),
            path: Some("/usr/local/bin:/usr/bin:/bin".to_string()),
            command_v: HashMap::from([
                (
                    "zellij".to_string(),
                    Some(format!("{home}/.local/bin/zellij")),
                ),
                ("tmux".to_string(), Some("/usr/bin/tmux".to_string())),
            ]),
            executables: HashMap::new(),
            rpc_readiness: None,
            calls: RefCell::new(Vec::new()),
        };
        let runner = RecordingRemediationRunner::default();

        let remediated = attempt_safe_ssh_readiness_remediation_with_probe_and_runner(
            &config,
            "aws",
            &SshReadinessFailure::RpcNotReady {
                host: "aws".to_string(),
                detail: "zjctl RPC readiness check timed out".to_string(),
            },
            &probe,
            &runner,
        );

        assert!(remediated);
        assert!(runner.commands.borrow().iter().any(|command| {
            command.contains("action")
                && command.contains("launch-plugin")
                && command.contains("/home/remote/.config/zellij/plugins/zrpc.wasm")
        }));
    }

    #[test]
    fn remediation_never_sends_blind_prompt_approval() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::from([(
                "ZELLIJ_SESSION_NAME".to_string(),
                "prod-shell".to_string(),
            )]),
            ssh_options: Vec::new(),
        };
        let home = "/home/remote";
        let probe = FakeProbe {
            home: Some(home.to_string()),
            path: Some("/usr/local/bin:/usr/bin:/bin".to_string()),
            command_v: HashMap::from([
                (
                    "zjctl".to_string(),
                    Some(format!("{home}/.local/bin/zjctl")),
                ),
                (
                    "zellij".to_string(),
                    Some(format!("{home}/.local/bin/zellij")),
                ),
                ("tmux".to_string(), Some("/usr/bin/tmux".to_string())),
            ]),
            executables: HashMap::new(),
            rpc_readiness: None,
            calls: RefCell::new(Vec::new()),
        };
        let runner = RecordingRemediationRunner::default();

        let remediated = attempt_safe_ssh_readiness_remediation_with_probe_and_runner(
            &config,
            "prod-shell",
            &SshReadinessFailure::RpcNotReady {
                host: "aws".to_string(),
                detail: "helper client is not attached yet".to_string(),
            },
            &probe,
            &runner,
        );

        assert!(remediated);
        assert!(
            runner
                .commands
                .borrow()
                .iter()
                .all(|command| !command.contains("send-keys"))
        );
        assert!(
            runner
                .commands
                .borrow()
                .iter()
                .all(|command| !command.contains("Allow? (y/n)"))
        );
    }

    #[test]
    fn readiness_auto_fix_skips_protocol_version_mismatch() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let probe = FakeProbe::default();
        let runner = RecordingRemediationRunner::default();

        let remediated = attempt_safe_ssh_readiness_remediation_with_probe_and_runner(
            &config,
            "aws",
            &SshReadinessFailure::ProtocolVersionMismatch {
                host: "aws".to_string(),
                detail: format!(
                    "zrpc protocol version mismatch: expected {}, got {}",
                    PROTOCOL_VERSION,
                    PROTOCOL_VERSION + 1
                ),
            },
            &probe,
            &runner,
        );

        assert!(!remediated);
        assert!(runner.commands.borrow().is_empty());
    }

    #[test]
    fn readiness_auto_fix_skips_missing_plugin() {
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let probe = FakeProbe::default();
        let runner = RecordingRemediationRunner::default();

        let remediated = attempt_safe_ssh_readiness_remediation_with_probe_and_runner(
            &config,
            "aws",
            &SshReadinessFailure::MissingPlugin {
                host: "aws".to_string(),
                detail: "zrpc plugin not found at /home/remote/.config/zellij/plugins/zrpc.wasm"
                    .to_string(),
            },
            &probe,
            &runner,
        );

        assert!(!remediated);
        assert!(runner.commands.borrow().is_empty());
    }

    #[test]
    fn detects_protocol_version_mismatch_messages() {
        assert!(is_protocol_version_mismatch_message(&format!(
            "zrpc protocol version mismatch: expected {}, got {}",
            PROTOCOL_VERSION,
            PROTOCOL_VERSION + 1
        )));
        assert!(!is_protocol_version_mismatch_message(
            "Error: no response from zrpc plugin"
        ));
    }

    #[test]
    fn detects_missing_plugin_messages() {
        assert!(is_missing_plugin_message(
            "zrpc plugin not found at /tmp/zrpc.wasm"
        ));
        assert!(!is_missing_plugin_message(
            "Error: no response from zrpc plugin"
        ));
    }

    #[test]
    fn helper_client_geometry_prefers_explicit_over_terminal_size() {
        assert_eq!(
            helper_client_geometry_from_sources(Some(220), Some(70), Some(180), Some(55)),
            (220, 70)
        );
    }

    #[test]
    fn helper_client_geometry_uses_terminal_size_when_explicit_missing() {
        assert_eq!(
            helper_client_geometry_from_sources(None, None, Some(180), Some(55)),
            (180, 55)
        );
    }

    #[test]
    fn helper_client_geometry_clamps_small_terminal_size_to_safe_defaults() {
        assert_eq!(
            helper_client_geometry_from_sources(None, None, Some(80), Some(24)),
            (160, 48)
        );
    }

    #[test]
    fn helper_client_geometry_falls_back_to_defaults() {
        assert_eq!(
            helper_client_geometry_from_sources(None, None, None, None),
            (160, 48)
        );
    }

    #[test]
    fn parse_rpc_output_rejects_version_mismatch() {
        let request_id = Uuid::new_v4();
        let response = RpcResponse {
            v: PROTOCOL_VERSION + 1,
            id: request_id,
            ok: true,
            result: Some(serde_json::json!({"count": 1})),
            error: None,
        };
        let stdout = serde_json::to_string(&response).expect("response should serialize");

        let error = parse_rpc_output(&stdout, request_id, "file:/tmp/zrpc.wasm")
            .expect_err("version mismatch should be rejected");

        assert_eq!(
            error,
            AdapterError::CommandFailed(format!(
                "zrpc protocol version mismatch: expected {}, got {}",
                PROTOCOL_VERSION,
                PROTOCOL_VERSION + 1
            ))
        );
    }

    #[test]
    fn parse_rpc_output_preserves_plugin_error_message() {
        let request_id = Uuid::new_v4();
        let response = RpcResponse {
            v: PROTOCOL_VERSION,
            id: request_id,
            ok: false,
            result: None,
            error: Some(RpcError::new(RpcErrorCode::NoMatch, "no panes found")),
        };
        let stdout = serde_json::to_string(&response).expect("response should serialize");

        let error = parse_rpc_output(&stdout, request_id, "file:/tmp/zrpc.wasm")
            .expect_err("rpc errors should surface");

        assert_eq!(
            error,
            AdapterError::CommandFailed("no panes found".to_string())
        );
    }
}
