use std::{error::Error, sync::Arc};

use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use serde::Serialize;
use serde_json::json;

use crate::domain::errors::{DomainError, ErrorCode};
use crate::domain::requests::{
    AttachRequest, CaptureRequest, CloseRequest, ListRequest, SendRequest, SpawnRequest,
    WaitRequest,
};
use crate::server::McpServer;

#[derive(Clone)]
pub struct RmcpServer {
    inner: Arc<McpServer>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl RmcpServer {
    pub fn new(inner: McpServer) -> Self {
        Self {
            inner: Arc::new(inner),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Create a managed Zellij execution target.")]
    fn zellij_spawn(
        &self,
        Parameters(request): Parameters<SpawnRequest>,
    ) -> Result<String, ErrorData> {
        self.execute("zellij_spawn", request)
    }

    #[tool(description = "Attach an existing Zellij pane to daemon management.")]
    fn zellij_attach(
        &self,
        Parameters(request): Parameters<AttachRequest>,
    ) -> Result<String, ErrorData> {
        self.execute("zellij_attach", request)
    }

    #[tool(description = "Send input to a managed pane.")]
    fn zellij_send(
        &self,
        Parameters(request): Parameters<SendRequest>,
    ) -> Result<String, ErrorData> {
        self.execute("zellij_send", request)
    }

    #[tool(description = "Wait for a managed pane to become idle.")]
    fn zellij_wait(
        &self,
        Parameters(request): Parameters<WaitRequest>,
    ) -> Result<String, ErrorData> {
        self.execute("zellij_wait", request)
    }

    #[tool(description = "Capture output from a managed pane.")]
    fn zellij_capture(
        &self,
        Parameters(request): Parameters<CaptureRequest>,
    ) -> Result<String, ErrorData> {
        self.execute("zellij_capture", request)
    }

    #[tool(description = "Close a managed pane.")]
    fn zellij_close(
        &self,
        Parameters(request): Parameters<CloseRequest>,
    ) -> Result<String, ErrorData> {
        self.execute("zellij_close", request)
    }

    #[tool(description = "List known managed Zellij handles.")]
    fn zellij_list(
        &self,
        Parameters(request): Parameters<ListRequest>,
    ) -> Result<String, ErrorData> {
        self.execute("zellij_list", request)
    }

    fn execute<T>(&self, tool_name: &str, request: T) -> Result<String, ErrorData>
    where
        T: Serialize,
    {
        let arguments = serde_json::to_value(request)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        let response = self
            .inner
            .execute_tool(tool_name, arguments)
            .map_err(mcp_error_from_domain)?;

        serde_json::to_string_pretty(&response)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))
    }

    pub async fn serve_stdio(self) -> Result<(), Box<dyn Error>> {
        let service = self.serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    }
}

fn mcp_error_from_domain(error: DomainError) -> ErrorData {
    let code = serialized_domain_code(&error.code);
    let data = json!({
        "code": code,
        "message": error.message,
        "retryable": error.retryable,
    });

    match error.code {
        ErrorCode::InvalidArgument
        | ErrorCode::HandleNotFound
        | ErrorCode::AliasNotFound
        | ErrorCode::SelectorNotUnique
        | ErrorCode::TargetNotFound
        | ErrorCode::TargetStale
        | ErrorCode::WaitTimeout => ErrorData::invalid_params(code, Some(data)),
        _ => ErrorData::internal_error(code, Some(data)),
    }
}

fn serialized_domain_code(code: &ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{code:?}"))
}

#[cfg(test)]
mod tests {
    use super::mcp_error_from_domain;
    use crate::domain::errors::{DomainError, ErrorCode};

    #[test]
    fn preserves_stable_domain_code_in_mcp_error_data() {
        let error = mcp_error_from_domain(DomainError::new(
            ErrorCode::HandleNotFound,
            "missing handle",
            false,
        ));

        assert_eq!(error.message, "HANDLE_NOT_FOUND");
        assert_eq!(error.data.expect("error data")["code"], "HANDLE_NOT_FOUND");
    }
}

#[tool_handler]
impl ServerHandler for RmcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Managed Zellij daemon tools exposed over MCP stdio.".to_string())
    }
}
