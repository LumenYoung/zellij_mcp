use zellij_mcp::server::McpServer;

fn main() {
    let server = McpServer::default();
    println!(
        "zellij_mcp skeleton loaded with {} tools",
        server.tool_definitions().len()
    );
}
