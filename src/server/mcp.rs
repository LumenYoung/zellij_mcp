use serde_json::Value;

use crate::domain::errors::{DomainError, ErrorCode};
use crate::domain::requests::{AttachRequest, CaptureRequest, ListRequest, SendRequest};
use crate::domain::status::CaptureMode;
use crate::services::TerminalManager;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
}

pub struct McpServer {
    tools: Vec<ToolDefinition>,
    terminal_manager: Box<dyn TerminalManager>,
}

impl McpServer {
    pub fn new(terminal_manager: Box<dyn TerminalManager>) -> Self {
        Self {
            tools: vec![
                ToolDefinition {
                    name: "zellij_spawn",
                    description: "Create a managed Zellij execution target.",
                },
                ToolDefinition {
                    name: "zellij_attach",
                    description: "Attach an existing Zellij pane to daemon management.",
                },
                ToolDefinition {
                    name: "zellij_send",
                    description: "Send input to a managed pane.",
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
            ],
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
        match name {
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
            "zellij_send" => {
                let request: SendRequest = serde_json::from_value(arguments).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })?;
                let response = self.terminal_manager.send(request)?;
                serde_json::to_value(response).map_err(|error| {
                    DomainError::new(ErrorCode::InvalidArgument, error.to_string(), false)
                })
            }
            _ => Err(DomainError::new(
                ErrorCode::InvalidArgument,
                format!("unsupported tool `{name}`"),
                false,
            )),
        }
    }
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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domain::binding::TerminalBinding;
    use crate::domain::errors::DomainError;
    use crate::domain::observation::CaptureResult;
    use crate::domain::requests::{AttachRequest, CaptureRequest, ListRequest, SendRequest};
    use crate::domain::responses::{AttachResponse, CaptureResponse, ListResponse, SendResponse};
    use crate::domain::status::{BindingSource, TerminalStatus};
    use crate::services::TerminalManager;

    use super::McpServer;

    #[derive(Debug)]
    struct MockTerminalManager;

    impl TerminalManager for MockTerminalManager {
        fn attach(&self, _request: AttachRequest) -> Result<AttachResponse, DomainError> {
            Ok(AttachResponse {
                handle: "zh_test".to_string(),
                attached: true,
                baseline_established: true,
            })
        }

        fn list(&self, _request: ListRequest) -> Result<ListResponse, DomainError> {
            Ok(ListResponse {
                bindings: vec![TerminalBinding {
                    handle: "zh_test".to_string(),
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
                    truncated: false,
                    captured_at: chrono::Utc::now(),
                    baseline: None,
                },
            })
        }

        fn send(&self, _request: SendRequest) -> Result<SendResponse, DomainError> {
            Ok(SendResponse {
                handle: "zh_test".to_string(),
                accepted: true,
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
                "zellij_send",
                "zellij_wait",
                "zellij_capture",
                "zellij_close",
                "zellij_list",
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
}
