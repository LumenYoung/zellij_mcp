use std::collections::HashMap;

use zellij_mcp::adapters::zjctl::{SshTargetConfig, SshZjctlClient, ZjctlClient};
use zellij_mcp::persistence::{ObservationStore, RegistryStore};
use zellij_mcp::server::{McpServer, RmcpServer};
use zellij_mcp::services::{TargetRouter, TerminalManager, TerminalService};

fn parse_target_configs(
    raw_targets: &str,
) -> Result<HashMap<String, SshTargetConfig>, serde_json::Error> {
    serde_json::from_str(raw_targets)
}

#[tokio::main]
async fn main() {
    let zjctl_binary = std::env::var("ZJCTL_BIN").unwrap_or_else(|_| "zjctl".to_string());
    let state_dir = std::env::var("ZELLIJ_MCP_STATE_DIR").unwrap_or_else(|_| "state".to_string());
    let registry_store = RegistryStore::new(format!("{state_dir}/registry.json"));
    let observation_store = ObservationStore::new(format!("{state_dir}/observations.json"));

    let local_service = TerminalService::new(
        "local",
        ZjctlClient::new(zjctl_binary),
        registry_store.clone(),
        observation_store.clone(),
    );
    let _ = local_service.revalidate_all();

    let mut backends: HashMap<String, Box<dyn TerminalManager>> = HashMap::new();
    backends.insert("local".to_string(), Box::new(local_service));

    if let Ok(raw_targets) = std::env::var("ZELLIJ_MCP_TARGETS") {
        let targets = parse_target_configs(&raw_targets).unwrap_or_else(|error| {
            panic!("failed to parse ZELLIJ_MCP_TARGETS: {error}");
        });
        for (alias, config) in targets {
            let target_id = format!("ssh:{alias}");
            let service = TerminalService::new(
                target_id.clone(),
                SshZjctlClient::new(config),
                registry_store.clone(),
                observation_store.clone(),
            );
            let _ = service.revalidate_all();
            backends.insert(target_id, Box::new(service));
        }
    }

    let server = McpServer::new(Box::new(TargetRouter::new(registry_store, backends)));

    if let Err(error) = RmcpServer::new(server).serve_stdio().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::parse_target_configs;
    use zellij_mcp::adapters::zjctl::SshTargetConfig;

    #[test]
    fn parses_zellij_mcp_targets_alias_map_into_ssh_target_configs() {
        let parsed = parse_target_configs(
            r#"{
                "a100": {
                    "host": "a100",
                    "remote_zjctl_bin": "/home/yang/bin/zjctl",
                    "remote_zellij_bin": "zellij",
                    "remote_env": {"ZELLIJ_SESSION_NAME": "a100"},
                    "ssh_options": ["-o", "BatchMode=yes"]
                }
            }"#,
        )
        .expect("target configs should parse");

        let mut remote_env = BTreeMap::new();
        remote_env.insert("ZELLIJ_SESSION_NAME".to_string(), "a100".to_string());

        assert_eq!(
            parsed.get("a100"),
            Some(&SshTargetConfig {
                host: "a100".to_string(),
                remote_zjctl_bin: "/home/yang/bin/zjctl".to_string(),
                remote_zellij_bin: "zellij".to_string(),
                remote_env,
                ssh_options: vec!["-o".to_string(), "BatchMode=yes".to_string()],
            })
        );
    }

    #[test]
    fn invalid_zellij_mcp_targets_json_returns_parse_error() {
        let error = parse_target_configs("not-json").expect_err("invalid JSON should fail");
        assert!(error.to_string().contains("expected ident") || error.to_string().contains("expected value"));
    }
}
