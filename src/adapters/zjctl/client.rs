use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::process::Command;
use std::time::Duration;

use crate::adapters::zjctl::{AdapterError, ZjctlCommand};
use crate::domain::requests::{AttachRequest, SpawnRequest};
use crate::domain::status::SpawnTarget;

use super::parser::{parse_capture_output, parse_list_output, parse_spawn_output};

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

    fn quote_posix_sh(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
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
            command.push_str(&Self::quote_posix_sh(&format!("{key}={value}")));
        }
        command.push(' ');
        command.push_str(&Self::quote_posix_sh(binary));
        for arg in args {
            command.push(' ');
            command.push_str(&Self::quote_posix_sh(arg));
        }
        command
    }

    fn run_remote_command_string(&self, remote_command: &str) -> Result<Vec<u8>, AdapterError> {
        let output = Command::new("ssh")
            .arg("-T")
            .arg("-oBatchMode=yes")
            .args(&self.config.ssh_options)
            .arg(&self.config.host)
            .arg(remote_command)
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
    if selector == "focused" {
        return false;
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
    use crate::adapters::zjctl::AdapterError;
    use crate::adapters::zjctl::ZjctlCommand;

    use crate::domain::requests::SpawnRequest;
    use crate::domain::status::SpawnTarget;

    use super::{
        format_seconds, matches_selector, parse_command, prepare_spawn, resolve_new_tab_target,
        resolve_spawn_command, PreparedSpawn, ResolvedTarget,
    };

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
}
