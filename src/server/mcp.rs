use serde_json::Value;

use crate::domain::errors::{DomainError, ErrorCode};
use crate::domain::requests::{
    AttachRequest, CaptureRequest, CleanupRequest, CloseRequest, DiscoverRequest, LayoutRequest,
    ListRequest, ReplaceRequest, SendRequest, SpawnRequest, TakeoverRequest, WaitRequest,
};
use crate::domain::status::CaptureMode;
use crate::server::daemon_identity_json;
use crate::services::TerminalManager;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
}

pub const TOOL_DEFINITIONS: [ToolDefinition; 12] = [
    ToolDefinition {
        name: "zellij_spawn",
        description: "Create a managed Zellij execution target.",
    },
    ToolDefinition {
        name: "zellij_attach",
        description: "Attach an existing Zellij pane to daemon management.",
    },
    ToolDefinition {
        name: "zellij_takeover",
        description: "Search and attach an existing Zellij pane in one step.",
    },
    ToolDefinition {
        name: "zellij_discover",
        description: "Discover live Zellij panes before attaching.",
    },
    ToolDefinition {
        name: "zellij_send",
        description: "Send input to a managed pane.",
    },
    ToolDefinition {
        name: "zellij_replace",
        description: "Cooperatively reuse a managed shell-like pane for a new command.",
    },
    ToolDefinition {
        name: "zellij_wait",
        description: "Wait for a managed pane to become idle.",
    },
    ToolDefinition {
        name: "zellij_capture",
        description: "Capture output from a managed pane.",
    },
    ToolDefinition {
        name: "zellij_close",
        description: "Close a managed pane.",
    },
    ToolDefinition {
        name: "zellij_list",
        description: "List known managed Zellij handles.",
    },
    ToolDefinition {
        name: "zellij_layout",
        description: "Inspect tabs and panes grouped by layout.",
    },
    ToolDefinition {
        name: "zellij_cleanup",
        description: "Clean up persisted stale or closed pane state.",
    },
];

pub struct McpServer {
    tools: Vec<ToolDefinition>,
    terminal_manager: Box<dyn TerminalManager>,
}

impl McpServer {
    pub fn new(terminal_manager: Box<dyn TerminalManager>) -> Self {
        Self {
            tools: TOOL_DEFINITIONS.to_vec(),
            terminal_manager,
        }
    }

    pub fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.tools
    }

    pub fn supported_capture_modes(&self) -> [CaptureMode; 3] {
        [CaptureMode::Full, CaptureMode::Delta, CaptureMode::Current]
    }

    pub fn execute_tool(&self, name: &str, arguments: Value) -> Result<Value, DomainError> {
        let response = match name {
            "zellij_spawn" => {
                let request: SpawnRequest = serde_json::from_value(arguments).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })?;
                let response = self.terminal_manager.spawn(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_attach" => {
                let request: AttachRequest =
                    serde_json::from_value(arguments).map_err(|error| {
                        DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                    })?;
                let response = self.terminal_manager.attach(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_takeover" => {
                let request: TakeoverRequest =
                    serde_json::from_value(arguments).map_err(|error| {
                        DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                    })?;
                let response = self.terminal_manager.takeover(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_discover" => {
                let request: DiscoverRequest =
                    serde_json::from_value(arguments).map_err(|error| {
                        DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                    })?;
                let response = self.terminal_manager.discover(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_capture" => {
                let request: CaptureRequest =
                    serde_json::from_value(arguments).map_err(|error| {
                        DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                    })?;
                let response = self.terminal_manager.capture(request)?;
                serde_json::to_value(response.capture).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_list" => {
                let request: ListRequest = serde_json::from_value(arguments).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })?;
                let response = self.terminal_manager.list(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_layout" => {
                let request: LayoutRequest =
                    serde_json::from_value(arguments).map_err(|error| {
                        DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                    })?;
                let response = self.terminal_manager.layout(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_cleanup" => {
                let request: CleanupRequest =
                    serde_json::from_value(arguments).map_err(|error| {
                        DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                    })?;
                let response = self.terminal_manager.cleanup(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_send" => {
                let request: SendRequest = serde_json::from_value(arguments).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })?;
                let response = self.terminal_manager.send(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_replace" => {
                let request: ReplaceRequest =
                    serde_json::from_value(arguments).map_err(|error| {
                        DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                    })?;
                let response = self.terminal_manager.replace(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_wait" => {
                let request: WaitRequest = serde_json::from_value(arguments).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })?;
                let response = self.terminal_manager.wait(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            "zellij_close" => {
                let request: CloseRequest = serde_json::from_value(arguments).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })?;
                let response = self.terminal_manager.close(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            _ => Err(DomainError::new(
                ErrorCode::InvalidArgument,
                format!("unsupported tool `{name}`"),
                false,
            )),
        }?;

        Ok(attach_daemon_identity(response))
    }
}

fn attach_daemon_identity(mut response: Value) -> Value {
    if let Some(object) = response.as_object_mut() {
        object.insert("_daemon".to_string(), daemon_identity_json());
    }
    response
}

impl Default for ToolDefinition {
    fn default() -> Self {
        Self {
            name: "zellij_list",
            description: "List known managed Zellij handles.",
        }
    }
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new(Box::new(NoopTerminalManager))
    }
}

#[derive(Debug)]
struct NoopTerminalManager;

impl TerminalManager for NoopTerminalManager {
    fn spawn(
        &self,
        _request: SpawnRequest,
    ) -> Result<crate::domain::responses::SpawnResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn attach(
        &self,
        _request: AttachRequest,
    ) -> Result<crate::domain::responses::AttachResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn takeover(
        &self,
        _request: TakeoverRequest,
    ) -> Result<crate::domain::responses::TakeoverResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn cleanup(
        &self,
        _request: CleanupRequest,
    ) -> Result<crate::domain::responses::CleanupResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn layout(
        &self,
        _request: LayoutRequest,
    ) -> Result<crate::domain::responses::LayoutResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn discover(
        &self,
        _request: DiscoverRequest,
    ) -> Result<crate::domain::responses::DiscoverResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn list(
        &self,
        _request: ListRequest,
    ) -> Result<crate::domain::responses::ListResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn capture(
        &self,
        _request: CaptureRequest,
    ) -> Result<crate::domain::responses::CaptureResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn send(
        &self,
        _request: SendRequest,
    ) -> Result<crate::domain::responses::SendResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn replace(
        &self,
        _request: ReplaceRequest,
    ) -> Result<crate::domain::responses::ReplaceResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn wait(
        &self,
        _request: WaitRequest,
    ) -> Result<crate::domain::responses::WaitResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }

    fn close(
        &self,
        _request: CloseRequest,
    ) -> Result<crate::domain::responses::CloseResponse, DomainError> {
        Err(DomainError::new(
            ErrorCode::InvalidArgument,
            "terminal manager is not configured",
            false,
        ))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domain::binding::TerminalBinding;
    use crate::domain::errors::DomainError;
    use crate::domain::observation::CaptureResult;
    use crate::domain::requests::{
        AttachRequest, CaptureRequest, CleanupRequest, CloseRequest, DiscoverRequest,
        LayoutRequest, ListRequest, ReplaceRequest, SendRequest, SpawnRequest, TakeoverRequest,
        WaitRequest,
    };
    use crate::domain::responses::{
        AttachResponse, CaptureResponse, CleanupResponse, CloseResponse, DiscoverCandidate,
        DiscoverResponse, LayoutResponse, LayoutTab, ListResponse, ReplaceResponse, SendResponse,
        SpawnResponse, TakeoverResponse, WaitResponse,
    };
    use crate::domain::status::{BindingSource, SpawnTarget, TerminalStatus};
    use crate::services::TerminalManager;

    use super::McpServer;

    #[derive(Debug)]
    struct MockTerminalManager;

    impl TerminalManager for MockTerminalManager {
        fn spawn(&self, _request: SpawnRequest) -> Result<SpawnResponse, DomainError> {
            Ok(SpawnResponse {
                handle: "zh_test".to_string(),
                target_id: "local".to_string(),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                status: "ready".to_string(),
            })
        }

        fn attach(&self, _request: AttachRequest) -> Result<AttachResponse, DomainError> {
            Ok(AttachResponse {
                handle: "zh_test".to_string(),
                target_id: "local".to_string(),
                attached: true,
                baseline_established: true,
            })
        }

        fn takeover(&self, _request: TakeoverRequest) -> Result<TakeoverResponse, DomainError> {
            Ok(TakeoverResponse {
                handle: "zh_test".to_string(),
                target_id: "local".to_string(),
                attached: true,
                baseline_established: true,
                matched_selector: "id:terminal:7".to_string(),
            })
        }

        fn cleanup(&self, _request: CleanupRequest) -> Result<CleanupResponse, DomainError> {
            Ok(CleanupResponse {
                removed_handles: vec!["zh_cleanup".to_string()],
                removed_count: 1,
                dry_run: false,
            })
        }

        fn layout(&self, _request: LayoutRequest) -> Result<LayoutResponse, DomainError> {
            Ok(LayoutResponse {
                target_id: "local".to_string(),
                session_name: "gpu".to_string(),
                tabs: vec![LayoutTab {
                    tab_name: "editor".to_string(),
                    panes: vec![DiscoverCandidate {
                        target_id: "local".to_string(),
                        selector: "id:terminal:7".to_string(),
                        pane_id: Some("terminal:7".to_string()),
                        session_name: "gpu".to_string(),
                        tab_name: Some("editor".to_string()),
                        title: Some("editor".to_string()),
                        command: Some("fish".to_string()),
                        focused: false,
                        preview: None,
                        preview_basis: None,
                        captured_at: None,
                    }],
                }],
            })
        }

        fn discover(&self, _request: DiscoverRequest) -> Result<DiscoverResponse, DomainError> {
            Ok(DiscoverResponse {
                candidates: vec![DiscoverCandidate {
                    target_id: "local".to_string(),
                    selector: "id:terminal:7".to_string(),
                    pane_id: Some("terminal:7".to_string()),
                    session_name: "gpu".to_string(),
                    tab_name: Some("editor".to_string()),
                    title: Some("editor".to_string()),
                    command: Some("fish".to_string()),
                    focused: false,
                    preview: Some("hello".to_string()),
                    preview_basis: Some("recent_lines".to_string()),
                    captured_at: Some(chrono::Utc::now()),
                }],
            })
        }

        fn list(&self, _request: ListRequest) -> Result<ListResponse, DomainError> {
            Ok(ListResponse {
                bindings: vec![TerminalBinding {
                    handle: "zh_test".to_string(),
                    target_id: "local".to_string(),
                    alias: Some("editor".to_string()),
                    session_name: "gpu".to_string(),
                    tab_name: Some("editor".to_string()),
                    selector: "id:terminal:7".to_string(),
                    pane_id: Some("terminal:7".to_string()),
                    cwd: None,
                    launch_command: None,
                    source: BindingSource::Attached,
                    status: TerminalStatus::Ready,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                }],
            })
        }

        fn capture(&self, _request: CaptureRequest) -> Result<CaptureResponse, DomainError> {
            Ok(CaptureResponse {
                capture: CaptureResult {
                    handle: "zh_test".to_string(),
                    mode: "full".to_string(),
                    content: "hello".to_string(),
                    tail_lines: None,
                    line_offset: None,
                    line_limit: None,
                    line_window_applied: false,
                    next_cursor: None,
                    ansi_normalized: false,
                    truncated: false,
                    captured_at: chrono::Utc::now(),
                    baseline: None,
                    interaction_id: None,
                    interaction_completed: None,
                    interaction_exit_code: None,
                },
            })
        }

        fn send(&self, _request: SendRequest) -> Result<SendResponse, DomainError> {
            Ok(SendResponse {
                handle: "zh_test".to_string(),
                accepted: true,
            })
        }

        fn replace(&self, _request: ReplaceRequest) -> Result<ReplaceResponse, DomainError> {
            Ok(ReplaceResponse {
                handle: "zh_test".to_string(),
                replaced: true,
                interaction_id: Some("zi_test".to_string()),
            })
        }

        fn wait(&self, _request: WaitRequest) -> Result<WaitResponse, DomainError> {
            Ok(WaitResponse {
                handle: "zh_test".to_string(),
                status: "idle".to_string(),
                observed_at: chrono::Utc::now(),
                completion_basis: None,
                interaction_id: None,
                interaction_completed: None,
                interaction_exit_code: None,
            })
        }

        fn close(&self, _request: CloseRequest) -> Result<CloseResponse, DomainError> {
            Ok(CloseResponse {
                handle: "zh_test".to_string(),
                closed: true,
            })
        }
    }

    #[test]
    fn registers_phase_one_tools() {
        let server = McpServer::new(Box::new(MockTerminalManager));
        let names: Vec<_> = server
            .tool_definitions()
            .iter()
            .map(|tool| tool.name)
            .collect();

        assert_eq!(
            names,
            vec![
                "zellij_spawn",
                "zellij_attach",
                "zellij_takeover",
                "zellij_discover",
                "zellij_send",
                "zellij_replace",
                "zellij_wait",
                "zellij_capture",
                "zellij_close",
                "zellij_list",
                "zellij_layout",
                "zellij_cleanup",
            ]
        );
    }

    #[test]
    fn executes_attach_tool() {
        let server = McpServer::new(Box::new(MockTerminalManager));

        let response = server
            .execute_tool(
                "zellij_attach",
                json!({
                    "session_name": "gpu",
                    "selector": "id:terminal:7",
                    "tab_name": "editor",
                    "alias": "main-editor"
                }),
            )
            .expect("attach tool should succeed");

        assert_eq!(response["handle"], "zh_test");
        assert_eq!(response["attached"], true);
        assert_eq!(response["_daemon"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn executes_discover_tool() {
        let server = McpServer::new(Box::new(MockTerminalManager));

        let response = server
            .execute_tool(
                "zellij_discover",
                json!({
                    "session_name": "gpu",
                    "tab_name": "editor",
                    "include_preview": true,
                    "preview_lines": 8
                }),
            )
            .expect("discover tool should succeed");

        assert_eq!(response["candidates"][0]["selector"], "id:terminal:7");
        assert_eq!(response["candidates"][0]["command"], "fish");
        assert_eq!(response["_daemon"]["package"], env!("CARGO_PKG_NAME"));
    }

    #[test]
    fn executes_send_tool() {
        let server = McpServer::new(Box::new(MockTerminalManager));

        let response = server
            .execute_tool(
                "zellij_send",
                json!({
                    "handle": "zh_test",
                    "text": "printf 'ok'",
                    "submit": true
                }),
            )
            .expect("send tool should succeed");

        assert_eq!(response["accepted"], true);
    }

    #[test]
    fn executes_spawn_tool() {
        let server = McpServer::new(Box::new(MockTerminalManager));

        let response = server
            .execute_tool(
                "zellij_spawn",
                json!({
                    "session_name": "gpu",
                    "spawn_target": SpawnTarget::ExistingTab,
                    "tab_name": "editor",
                    "cwd": "/tmp",
                    "command": "lazygit",
                    "argv": null,
                    "title": "lg",
                    "wait_ready": false
                }),
            )
            .expect("spawn tool should succeed");

        assert_eq!(response["status"], "ready");
    }

    #[test]
    fn executes_spawn_tool_with_argv_form() {
        let server = McpServer::new(Box::new(MockTerminalManager));

        let response = server
            .execute_tool(
                "zellij_spawn",
                json!({
                    "session_name": "gpu",
                    "spawn_target": SpawnTarget::ExistingTab,
                    "tab_name": "editor",
                    "cwd": "/tmp",
                    "argv": ["git", "status"],
                    "title": "git-status",
                    "wait_ready": false
                }),
            )
            .expect("spawn argv tool should succeed");

        assert_eq!(response["status"], "ready");
    }

    #[test]
    fn executes_wait_tool() {
        let server = McpServer::new(Box::new(MockTerminalManager));

        let response = server
            .execute_tool(
                "zellij_wait",
                json!({
                    "handle": "zh_test",
                    "idle_ms": 1200,
                    "timeout_ms": 30000
                }),
            )
            .expect("wait tool should succeed");

        assert_eq!(response["status"], "idle");
    }

    #[test]
    fn executes_close_tool() {
        let server = McpServer::new(Box::new(MockTerminalManager));

        let response = server
            .execute_tool(
                "zellij_close",
                json!({
                    "handle": "zh_test",
                    "force": true
                }),
            )
            .expect("close tool should succeed");

        assert_eq!(response["closed"], true);
    }
}
