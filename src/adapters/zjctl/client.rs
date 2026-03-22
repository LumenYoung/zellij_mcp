use chrono::{DateTime, Utc};
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
        let output = Command::new("zellij")
            .arg("--session")
            .arg(session_name)
            .arg("action")
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

        Ok(())
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

impl ZjctlAdapter for ZjctlClient {
    fn is_available(&self) -> bool {
        self.run_command(None, ZjctlCommand::Availability).is_ok()
    }

    fn spawn(&self, _request: &SpawnRequest) -> Result<ResolvedTarget, AdapterError> {
        match _request.target {
            SpawnTarget::NewTab => {
                let mut args = vec!["new-tab".to_string()];
                if let Some(tab_name) = &_request.tab_name {
                    args.push("--name".to_string());
                    args.push(tab_name.clone());
                }
                if let Some(cwd) = &_request.cwd {
                    args.push("--cwd".to_string());
                    args.push(cwd.clone());
                }
                self.run_zellij_action(&_request.session_name, &args)?;
            }
            SpawnTarget::ExistingTab => {
                if let Some(tab_name) = &_request.tab_name {
                    self.run_zellij_action(
                        &_request.session_name,
                        &["go-to-tab-name".to_string(), tab_name.clone()],
                    )?;
                }
            }
        }

        let stdout = self.run_command(
            Some(&_request.session_name),
            ZjctlCommand::Spawn {
                cwd: _request.cwd.clone(),
                title: _request.title.clone(),
                command: split_command(&_request.command),
            },
        )?;
        let text = String::from_utf8_lossy(&stdout);
        let spawned = parse_spawn_output(
            &text,
            &_request.session_name,
            _request.tab_name.as_deref(),
            _request.title.as_deref(),
        )?;

        if let Some(title) = &_request.title {
            let attach_request = AttachRequest {
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

fn split_command(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn format_seconds(milliseconds: u64) -> String {
    let duration = Duration::from_millis(milliseconds);
    format!("{:.1}", duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::{ResolvedTarget, format_seconds, matches_selector, split_command};

    #[test]
    fn matches_exact_id_selector() {
        let target = ResolvedTarget {
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("editor".to_string()),
            title: Some("editor".to_string()),
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
        };

        assert!(matches_selector("title:editor", &target));
    }

    #[test]
    fn splits_command_on_whitespace() {
        assert_eq!(split_command("lazygit --debug"), vec!["lazygit", "--debug"]);
    }

    #[test]
    fn formats_wait_durations_as_seconds() {
        assert_eq!(format_seconds(1200), "1.2");
    }
}
