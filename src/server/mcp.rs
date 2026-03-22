use crate::domain::status::CaptureMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Debug)]
pub struct McpServer {
    tools: Vec<ToolDefinition>,
}

impl McpServer {
    pub fn new() -> Self {
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
        }
    }

    pub fn tool_definitions(&self) -> &[ToolDefinition] {
        &self.tools
    }

    pub fn supported_capture_modes(&self) -> [CaptureMode; 3] {
        [CaptureMode::Full, CaptureMode::Delta, CaptureMode::Current]
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
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::McpServer;

    #[test]
    fn registers_phase_one_tools() {
        let server = McpServer::new();
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
}
