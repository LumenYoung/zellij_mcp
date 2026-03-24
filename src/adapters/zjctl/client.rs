use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::process::Command;
use std::time::Duration;

use crate::adapters::zjctl::{AdapterError, ZjctlCommand};
use crate::domain::requests::{AttachRequest, SpawnRequest};
use crate::domain::status::SpawnTarget;

use super::parser::{parse_capture_output, parse_list_output, parse_spawn_output};

fn quote_posix_sh(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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

pub trait ZjctlAdapter {
    fn is_available(&self) -> bool;
    fn spawn(&self, request: &SpawnRequest) -> Result<ResolvedTarget, AdapterError>;
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
pub struct ZjctlClient {
    binary: String,
}

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
pub struct SshZjctlClient {
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
    PluginPermissionPrompt { host: String, detail: String },
    HelperClientMissing { host: String, detail: String },
    RpcNotReady { host: String, detail: String },
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
    fn check_rpc_readiness(&self, config: &SshTargetConfig) -> Result<(), AdapterError>;
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

    fn check_rpc_readiness(&self, config: &SshTargetConfig) -> Result<(), AdapterError> {
        let client = SshZjctlClient::new(config.clone());
        client.run_command(None, ZjctlCommand::List).map(|_| ())
    }
}

pub fn classify_ssh_backend_readiness(config: &SshTargetConfig) -> SshBackendReadiness {
    classify_ssh_backend_readiness_with_probe(config, &SshRemoteProbe::new(config))
}

pub fn attempt_safe_ssh_readiness_remediation(
    config: &SshTargetConfig,
    failure: &SshReadinessFailure,
) -> bool {
    attempt_safe_ssh_readiness_remediation_with_probe_and_runner(
        config,
        failure,
        &SshRemoteProbe::new(config),
        &SshRemoteRemediationRunner,
    )
}

fn classify_ssh_backend_readiness_with_probe(
    config: &SshTargetConfig,
    probe: &dyn RemoteProbe,
) -> SshBackendReadiness {
    match resolve_ssh_runtime_config_with_probe(config, probe) {
        Ok(resolved) => match probe.check_rpc_readiness(&resolved) {
            Ok(()) => SshBackendReadiness::Ready(resolved),
            Err(error) => classify_ssh_readiness_failure(&resolved.host, error),
        },
        Err(error) => classify_ssh_readiness_failure(&config.host, error),
    }
}

fn attempt_safe_ssh_readiness_remediation_with_probe_and_runner(
    config: &SshTargetConfig,
    failure: &SshReadinessFailure,
    probe: &dyn RemoteProbe,
    runner: &dyn RemoteRemediationRunner,
) -> bool {
    match failure {
        SshReadinessFailure::MissingBinary { .. }
        | SshReadinessFailure::HelperClientMissing { .. }
        | SshReadinessFailure::RpcNotReady { .. } => {}
        SshReadinessFailure::SshUnreachable { .. }
        | SshReadinessFailure::PluginPermissionPrompt { .. } => return false,
    }

    let context = SshReadinessRemediationContext::discover(config, probe);
    let mut remediated = false;

    if let Some(zjctl_bin) = context.zjctl_bin.as_deref() {
        remediated |= install_zjctl_plugin(config, &context, zjctl_bin, runner);
    }

    if let (Some(tmux_bin), Some(zellij_bin)) =
        (context.tmux_bin.as_deref(), context.zellij_bin.as_deref())
    {
        remediated |= start_helper_client(config, &context, tmux_bin, zellij_bin, runner);
    }

    remediated
}

#[derive(Debug, Clone, Default)]
struct SshReadinessRemediationContext {
    normalized_path: Option<String>,
    zjctl_bin: Option<String>,
    zellij_bin: Option<String>,
    tmux_bin: Option<String>,
    session_name: String,
    helper_session_name: String,
}

impl SshReadinessRemediationContext {
    fn discover(config: &SshTargetConfig, probe: &dyn RemoteProbe) -> Self {
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

        let zjctl_bin = resolve_existing_remote_binary(
            &config.remote_zjctl_bin,
            normalized_path.as_deref(),
            home.as_deref(),
            probe,
        );
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
        let session_name = remote_session_name(config);
        let helper_session_name = format!("zellij-mcp-client-{session_name}");

        Self {
            normalized_path,
            zjctl_bin,
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

        if include_session {
            assignments.retain(|(key, _)| key != "ZELLIJ_SESSION_NAME");
            assignments.push(("ZELLIJ_SESSION_NAME".to_string(), self.session_name.clone()));
        }

        assignments
    }
}

fn remote_session_name(config: &SshTargetConfig) -> String {
    config
        .remote_env
        .get("ZELLIJ_SESSION_NAME")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(config.host.as_str())
        .to_string()
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

    if let Some(normalized_path) = normalized_path {
        if let Ok(Some(resolved)) = probe.command_v(normalized_path, configured) {
            return Some(resolved);
        }
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

fn install_zjctl_plugin(
    config: &SshTargetConfig,
    context: &SshReadinessRemediationContext,
    zjctl_bin: &str,
    runner: &dyn RemoteRemediationRunner,
) -> bool {
    let command = build_remote_exec_env_command(
        zjctl_bin,
        &context.env_assignments(config, false),
        &["install".to_string()],
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
    let attach_command = build_remote_exec_env_command(
        zellij_bin,
        &context.env_assignments(config, true),
        &["attach".to_string(), context.session_name.clone()],
    );
    let command = format!(
        "if {tmux} has-session -t {helper} >/dev/null 2>&1; then exit 0; fi; {tmux} new-session -d -s {helper} {attach}",
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

pub fn is_rpc_not_ready_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("rpc")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("not ready")
        || lower.contains("helper")
        || lower.contains("client")
}

pub fn is_helper_client_missing_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    (lower.contains("helper") && lower.contains("client"))
        || lower.contains("not attached yet")
        || lower.contains("no active clients")
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
    let needs_zjctl_probe = binary_needs_probe(&config.remote_zjctl_bin);
    let needs_zellij_probe = binary_needs_probe(&config.remote_zellij_bin);

    if !needs_zjctl_probe && !needs_zellij_probe {
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

    resolved.remote_zjctl_bin =
        resolve_remote_binary(&config.remote_zjctl_bin, &home, &normalized_path, probe)?;
    resolved.remote_zellij_bin =
        resolve_remote_binary(&config.remote_zellij_bin, &home, &normalized_path, probe)?;

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
    Command::new("ssh")
        .arg("-T")
        .arg("-oBatchMode=yes")
        .args(&config.ssh_options)
        .arg(&config.host)
        .arg(remote_command)
        .output()
        .map_err(|_| AdapterError::ZjctlUnavailable)
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

impl ZjctlClient {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    fn run_command(
        &self,
        session_name: Option<&str>,
        command: ZjctlCommand,
    ) -> Result<Vec<u8>, AdapterError> {
        let mut process = Command::new(&self.binary);
        process.args(command.args());

        if let Some(session_name) = session_name {
            process.env("ZELLIJ_SESSION_NAME", session_name);
        }

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

        Ok(output.stdout)
    }

    fn run_zellij_action(&self, session_name: &str, args: &[String]) -> Result<(), AdapterError> {
        self.run_zellij_command(session_name, "action", args)
            .map(|_| ())
    }

    fn run_zellij_command(
        &self,
        session_name: &str,
        subcommand: &str,
        args: &[String],
    ) -> Result<Vec<u8>, AdapterError> {
        let output = Command::new("zellij")
            .arg("--session")
            .arg(session_name)
            .arg(subcommand)
            .args(args)
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

        Ok(output.stdout)
    }

    fn list_targets_for_session(
        &self,
        session_name: Option<&str>,
    ) -> Result<Vec<ResolvedTarget>, AdapterError> {
        let stdout = self.run_command(session_name, ZjctlCommand::List)?;
        let text = String::from_utf8_lossy(&stdout);
        parse_list_output(&text, session_name)
    }

    fn resolve_from_candidates(
        &self,
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
}

impl Default for ZjctlClient {
    fn default() -> Self {
        Self::new("zjctl")
    }
}

impl SshZjctlClient {
    pub fn new(config: SshTargetConfig) -> Self {
        Self { config }
    }

    fn remote_env_assignments(&self, session_name: Option<&str>) -> Vec<(String, String)> {
        let mut assignments: Vec<(String, String)> = self
            .config
            .remote_env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
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

    fn run_remote_command_string(&self, remote_command: &str) -> Result<Vec<u8>, AdapterError> {
        run_remote_command_string(&self.config, remote_command)
    }

    fn run_command(
        &self,
        session_name: Option<&str>,
        command: ZjctlCommand,
    ) -> Result<Vec<u8>, AdapterError> {
        let env_assignments = self.remote_env_assignments(session_name);
        let args = command.args();
        let remote_command =
            self.build_remote_exec_command(&self.config.remote_zjctl_bin, &env_assignments, &args);
        self.run_remote_command_string(&remote_command)
    }

    fn run_zellij_command(
        &self,
        session_name: &str,
        subcommand: &str,
        args: &[String],
    ) -> Result<Vec<u8>, AdapterError> {
        let mut command_args = vec![
            "--session".to_string(),
            session_name.to_string(),
            subcommand.to_string(),
        ];
        command_args.extend(args.iter().cloned());
        let remote_command = self.build_remote_exec_command(
            &self.config.remote_zellij_bin,
            &self.remote_env_assignments(None),
            &command_args,
        );
        self.run_remote_command_string(&remote_command)
    }

    fn run_zellij_action(&self, session_name: &str, args: &[String]) -> Result<(), AdapterError> {
        self.run_zellij_command(session_name, "action", args)
            .map(|_| ())
    }

    fn list_targets_for_session(
        &self,
        session_name: Option<&str>,
    ) -> Result<Vec<ResolvedTarget>, AdapterError> {
        let stdout = self.run_command(session_name, ZjctlCommand::List)?;
        let text = String::from_utf8_lossy(&stdout);
        parse_list_output(&text, session_name)
    }

    fn resolve_from_candidates(
        &self,
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
}

impl ZjctlAdapter for SshZjctlClient {
    fn is_available(&self) -> bool {
        self.run_command(None, ZjctlCommand::Availability).is_ok()
    }

    fn spawn(&self, request: &SpawnRequest) -> Result<ResolvedTarget, AdapterError> {
        let prepared = prepare_spawn(request)?;
        let spawn_command = match &prepared.command {
            ZjctlCommand::Spawn { command, .. } => command.clone(),
            _ => unreachable!("prepared spawn must produce a spawn command"),
        };
        let command_summary = spawn_command.join(" ");

        if let Some(action_args) = prepared.action_args.as_ref() {
            self.run_zellij_action(&request.session_name, action_args)?;
        }

        if matches!(request.spawn_target, SpawnTarget::NewTab) {
            let before = self.list_targets_for_session(Some(&request.session_name))?;
            let mut run_args = Vec::new();
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
            self.run_zellij_command(&request.session_name, "run", &run_args)?;

            let after = self.list_targets_for_session(Some(&request.session_name))?;
            return resolve_new_tab_target(request, before, after, &command_summary);
        }

        let stdout = self.run_command(Some(&request.session_name), prepared.command)?;
        let text = String::from_utf8_lossy(&stdout);
        let spawned = parse_spawn_output(
            &text,
            &request.session_name,
            request.tab_name.as_deref(),
            request.title.as_deref(),
        )?;

        if let Some(title) = &request.title {
            let attach_request = AttachRequest {
                target: None,
                session_name: request.session_name.clone(),
                tab_name: request.tab_name.clone(),
                selector: format!("title:{title}"),
                alias: None,
            };

            return self.resolve_selector(&attach_request).or(Ok(spawned));
        }

        Ok(spawned)
    }

    fn resolve_selector(&self, request: &AttachRequest) -> Result<ResolvedTarget, AdapterError> {
        let candidates = self.list_targets_for_session(Some(&request.session_name))?;
        self.resolve_from_candidates(request, candidates)
    }

    fn list_targets_in_session(
        &self,
        session_name: &str,
    ) -> Result<Vec<ResolvedTarget>, AdapterError> {
        self.list_targets_for_session(Some(session_name))
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
        self.run_command(
            Some(session_name),
            ZjctlCommand::Send {
                selector: handle.to_string(),
                text: payload,
            },
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
        let result = self.run_command(
            Some(session_name),
            ZjctlCommand::WaitIdle {
                selector: handle.to_string(),
                idle_seconds: format_seconds(idle_ms),
                timeout_seconds: format_seconds(timeout_ms),
            },
        );

        match result {
            Ok(_) => Ok(()),
            Err(AdapterError::CommandFailed(message)) if message.contains("timed out after") => {
                Err(AdapterError::Timeout)
            }
            Err(error) => Err(error),
        }
    }

    fn capture_full(
        &self,
        session_name: &str,
        handle: &str,
    ) -> Result<CaptureSnapshot, AdapterError> {
        let stdout = self.run_command(
            Some(session_name),
            ZjctlCommand::Capture {
                selector: handle.to_string(),
                full: true,
            },
        )?;

        Ok(CaptureSnapshot {
            content: parse_capture_output(&stdout),
            captured_at: Utc::now(),
            truncated: false,
        })
    }

    fn close(&self, session_name: &str, handle: &str, force: bool) -> Result<(), AdapterError> {
        self.run_command(
            Some(session_name),
            ZjctlCommand::Close {
                selector: handle.to_string(),
                force,
            },
        )
        .map(|_| ())
    }

    fn list_targets(&self) -> Result<Vec<ResolvedTarget>, AdapterError> {
        self.list_targets_for_session(None)
    }
}

impl ZjctlAdapter for ZjctlClient {
    fn is_available(&self) -> bool {
        self.run_command(None, ZjctlCommand::Availability).is_ok()
    }

    fn spawn(&self, _request: &SpawnRequest) -> Result<ResolvedTarget, AdapterError> {
        let prepared = prepare_spawn(_request)?;
        let spawn_command = match &prepared.command {
            ZjctlCommand::Spawn { command, .. } => command.clone(),
            _ => unreachable!("prepared spawn must produce a spawn command"),
        };
        let command_summary = spawn_command.join(" ");

        if let Some(action_args) = prepared.action_args.as_ref() {
            self.run_zellij_action(&_request.session_name, action_args)?;
        }

        if matches!(_request.spawn_target, SpawnTarget::NewTab) {
            let before = self.list_targets_for_session(Some(&_request.session_name))?;
            let mut run_args = Vec::new();
            if let Some(cwd) = &_request.cwd {
                run_args.push("--cwd".to_string());
                run_args.push(cwd.clone());
            }
            if let Some(title) = &_request.title {
                run_args.push("--name".to_string());
                run_args.push(title.clone());
            }
            run_args.push("--".to_string());
            run_args.extend(spawn_command);
            self.run_zellij_command(&_request.session_name, "run", &run_args)?;

            let after = self.list_targets_for_session(Some(&_request.session_name))?;
            return resolve_new_tab_target(_request, before, after, &command_summary);
        }

        let stdout = self.run_command(Some(&_request.session_name), prepared.command)?;
        let text = String::from_utf8_lossy(&stdout);
        let spawned = parse_spawn_output(
            &text,
            &_request.session_name,
            _request.tab_name.as_deref(),
            _request.title.as_deref(),
        )?;

        if let Some(title) = &_request.title {
            let attach_request = AttachRequest {
                target: None,
                session_name: _request.session_name.clone(),
                tab_name: _request.tab_name.clone(),
                selector: format!("title:{title}"),
                alias: None,
            };

            return self.resolve_selector(&attach_request).or(Ok(spawned));
        }

        Ok(spawned)
    }

    fn resolve_selector(&self, _request: &AttachRequest) -> Result<ResolvedTarget, AdapterError> {
        let candidates = self.list_targets_for_session(Some(&_request.session_name))?;
        self.resolve_from_candidates(_request, candidates)
    }

    fn list_targets_in_session(
        &self,
        session_name: &str,
    ) -> Result<Vec<ResolvedTarget>, AdapterError> {
        self.list_targets_for_session(Some(session_name))
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

        self.run_command(
            Some(session_name),
            ZjctlCommand::Send {
                selector: handle.to_string(),
                text: payload,
            },
        )?;

        Ok(())
    }

    fn wait_idle(
        &self,
        session_name: &str,
        handle: &str,
        idle_ms: u64,
        timeout_ms: u64,
    ) -> Result<(), AdapterError> {
        let result = self.run_command(
            Some(session_name),
            ZjctlCommand::WaitIdle {
                selector: handle.to_string(),
                idle_seconds: format_seconds(idle_ms),
                timeout_seconds: format_seconds(timeout_ms),
            },
        );

        match result {
            Ok(_) => Ok(()),
            Err(AdapterError::CommandFailed(message)) if message.contains("timed out after") => {
                Err(AdapterError::Timeout)
            }
            Err(error) => Err(error),
        }
    }

    fn capture_full(
        &self,
        session_name: &str,
        handle: &str,
    ) -> Result<CaptureSnapshot, AdapterError> {
        let stdout = self.run_command(
            Some(session_name),
            ZjctlCommand::Capture {
                selector: handle.to_string(),
                full: true,
            },
        )?;

        Ok(CaptureSnapshot {
            content: parse_capture_output(&stdout),
            captured_at: Utc::now(),
            truncated: false,
        })
    }

    fn close(&self, session_name: &str, handle: &str, force: bool) -> Result<(), AdapterError> {
        self.run_command(
            Some(session_name),
            ZjctlCommand::Close {
                selector: handle.to_string(),
                force,
            },
        )?;

        Ok(())
    }

    fn list_targets(&self) -> Result<Vec<ResolvedTarget>, AdapterError> {
        self.list_targets_for_session(std::env::var("ZELLIJ_SESSION_NAME").ok().as_deref())
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

fn resolve_new_tab_target(
    request: &SpawnRequest,
    before: Vec<ResolvedTarget>,
    after: Vec<ResolvedTarget>,
    command_summary: &str,
) -> Result<ResolvedTarget, AdapterError> {
    let before_ids: HashSet<&str> = before
        .iter()
        .filter_map(|target| target.pane_id.as_deref())
        .collect();
    let mut candidates: Vec<ResolvedTarget> = after
        .into_iter()
        .filter(is_terminal_target)
        .filter(|target| {
            target
                .pane_id
                .as_deref()
                .is_some_and(|pane_id| !before_ids.contains(pane_id))
        })
        .collect();

    if let Some(tab_name) = request.tab_name.as_ref() {
        candidates.retain(|target| target.tab_name.as_deref() == Some(tab_name.as_str()));
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
                "new tab spawn matched multiple candidate panes".to_string(),
            )),
        },
        _ => Err(AdapterError::CommandFailed(
            "spawn command matched multiple candidate panes".to_string(),
        )),
    }
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
        (None, None) => Err(AdapterError::ParseError(
            "spawn requires either `command` or `argv`".to_string(),
        )),
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedSpawn {
    action_args: Option<Vec<String>>,
    command: ZjctlCommand,
}

fn prepare_spawn(request: &SpawnRequest) -> Result<PreparedSpawn, AdapterError> {
    let command = resolve_spawn_command(request)?;
    let action_args = match request.spawn_target {
        SpawnTarget::NewTab => {
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
        SpawnTarget::ExistingTab => request
            .tab_name
            .as_ref()
            .map(|tab_name| vec!["go-to-tab-name".to_string(), tab_name.clone()]),
    };

    Ok(PreparedSpawn {
        action_args,
        command: ZjctlCommand::Spawn {
            cwd: request.cwd.clone(),
            title: request.title.clone(),
            command,
        },
    })
}

fn format_seconds(milliseconds: u64) -> String {
    let duration = Duration::from_millis(milliseconds);
    format!("{:.1}", duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashMap};

    use crate::adapters::zjctl::AdapterError;
    use crate::adapters::zjctl::ZjctlCommand;

    use crate::domain::requests::SpawnRequest;
    use crate::domain::status::SpawnTarget;

    use super::{
        attempt_safe_ssh_readiness_remediation_with_probe_and_runner,
        classify_ssh_backend_readiness_with_probe, format_seconds, matches_selector, parse_command,
        prepare_spawn, resolve_new_tab_target, resolve_spawn_command,
        resolve_ssh_runtime_config_with_probe, PreparedSpawn, RemoteProbe, RemoteRemediationRunner,
        ResolvedTarget, SshBackendReadiness, SshReadinessFailure, SshTargetConfig,
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

        fn check_rpc_readiness(&self, _config: &SshTargetConfig) -> Result<(), AdapterError> {
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
        assert!(error
            .to_string()
            .contains("either `command` or `argv`, not both"));
    }

    #[test]
    fn spawn_rejects_missing_command_and_argv() {
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

        let error = resolve_spawn_command(&request).expect_err("missing command forms should fail");
        assert!(matches!(error, AdapterError::ParseError(_)));
        assert!(error.to_string().contains("either `command` or `argv`"));
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
        assert!(error
            .to_string()
            .contains("must contain at least one element"));
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

        let error = prepare_spawn(&request).expect_err("invalid spawn should fail before planning");
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
            prepare_spawn(&request).expect("spawn should prepare"),
            PreparedSpawn {
                action_args: Some(vec![
                    "new-tab".to_string(),
                    "--name".to_string(),
                    "editor".to_string(),
                    "--cwd".to_string(),
                    "/tmp".to_string(),
                ]),
                command: ZjctlCommand::Spawn {
                    cwd: Some("/tmp".to_string()),
                    title: Some("git-status".to_string()),
                    command: vec!["git".to_string(), "status".to_string()],
                },
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

        let resolved = resolve_new_tab_target(
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

        let resolved = resolve_new_tab_target(&request, before, after, "lazygit")
            .expect("new tab target should resolve from command");

        assert_eq!(resolved.pane_id.as_deref(), Some("terminal:13"));
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
        assert!(probe
            .calls
            .borrow()
            .iter()
            .any(|call| call == &format!("command_v:zjctl:{normalized_path}")));
        assert!(probe
            .calls
            .borrow()
            .iter()
            .any(|call| call == &format!("command_v:zellij:{normalized_path}")));
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

        let readiness = classify_ssh_backend_readiness_with_probe(&config, &probe);

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

        let readiness = classify_ssh_backend_readiness_with_probe(&config, &probe);

        assert_eq!(
            readiness,
            SshBackendReadiness::AutoFixable(SshReadinessFailure::MissingBinary {
                host: "aws".to_string(),
                binary: "zjctl".to_string(),
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

        let readiness = classify_ssh_backend_readiness_with_probe(&config, &probe);

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

        let readiness = classify_ssh_backend_readiness_with_probe(&config, &probe);

        assert_eq!(
            readiness,
            SshBackendReadiness::AutoFixable(SshReadinessFailure::HelperClientMissing {
                host: "aws".to_string(),
                detail: "helper client is not attached yet".to_string(),
            })
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
            &SshReadinessFailure::MissingBinary {
                host: "aws".to_string(),
                binary: "zjctl".to_string(),
            },
            &probe,
            &runner,
        );

        assert!(remediated);
        assert!(runner
            .commands
            .borrow()
            .iter()
            .any(|command| command.contains("zellij-mcp-client-aws")));
        assert!(runner
            .commands
            .borrow()
            .iter()
            .any(|command| command.contains("new-session -d -s")));
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
            &SshReadinessFailure::RpcNotReady {
                host: "aws".to_string(),
                detail: "helper client is not attached yet".to_string(),
            },
            &probe,
            &runner,
        );

        assert!(remediated);
        assert!(runner
            .commands
            .borrow()
            .iter()
            .all(|command| !command.contains("send-keys")));
        assert!(runner
            .commands
            .borrow()
            .iter()
            .all(|command| !command.contains("Allow? (y/n)")));
    }
}
