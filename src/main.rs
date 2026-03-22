use zellij_mcp::adapters::zjctl::ZjctlClient;
use zellij_mcp::persistence::{ObservationStore, RegistryStore};
use zellij_mcp::server::McpServer;
use zellij_mcp::services::TerminalService;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let zjctl_binary = std::env::var("ZJCTL_BIN").unwrap_or_else(|_| "zjctl".to_string());
    let state_dir = std::env::var("ZELLIJ_MCP_STATE_DIR").unwrap_or_else(|_| "state".to_string());
    let service = TerminalService::new(
        ZjctlClient::new(zjctl_binary),
        RegistryStore::new(format!("{state_dir}/registry.json")),
        ObservationStore::new(format!("{state_dir}/observations.json")),
    );
    let server = McpServer::new(Box::new(service));

    if args.len() == 3 {
        match serde_json::from_str(&args[2])
            .map_err(|error| error.to_string())
            .and_then(|value| {
                server
                    .execute_tool(&args[1], value)
                    .map_err(|error| error.message)
            }) {
            Ok(value) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value)
                        .expect("json serialization should succeed")
                );
                return;
            }
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(1);
            }
        }
    }

    println!(
        "zellij_mcp skeleton loaded with {} tools",
        server.tool_definitions().len()
    );
}
