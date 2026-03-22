use zellij_mcp::adapters::zjctl::ZjctlClient;
use zellij_mcp::persistence::{ObservationStore, RegistryStore};
use zellij_mcp::server::{McpServer, RmcpServer};
use zellij_mcp::services::TerminalService;

#[tokio::main]
async fn main() {
    let zjctl_binary = std::env::var("ZJCTL_BIN").unwrap_or_else(|_| "zjctl".to_string());
    let state_dir = std::env::var("ZELLIJ_MCP_STATE_DIR").unwrap_or_else(|_| "state".to_string());
    let service = TerminalService::new(
        ZjctlClient::new(zjctl_binary),
        RegistryStore::new(format!("{state_dir}/registry.json")),
        ObservationStore::new(format!("{state_dir}/observations.json")),
    );
    let _ = service.revalidate_all();
    let server = McpServer::new(Box::new(service));

    if let Err(error) = RmcpServer::new(server).serve_stdio().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
