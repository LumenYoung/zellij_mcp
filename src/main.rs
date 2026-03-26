use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Deserialize;
use zellij_mcp::adapters::zjctl::{
    SshTargetConfig, SshZjctlClient, ZjctlClient, missing_binary_name, resolve_ssh_runtime_config,
};
use zellij_mcp::domain::errors::DomainError;
use zellij_mcp::persistence::{ObservationStore, RegistryStore};
use zellij_mcp::server::{McpServer, RmcpServer, daemon_identity};
use zellij_mcp::services::{TargetRouter, TerminalManager, TerminalService};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TargetConfigs {
    alias_defaults: AliasOnlyTargetDefaults,
    explicit_overrides: HashMap<String, SshTargetOverride>,
}

impl TargetConfigs {
    fn resolve_ssh_alias_target(&self, target: &str) -> Option<(String, SshTargetConfig)> {
        let target = target.trim();
        if target.is_empty() || target == "local" {
            return None;
        }

        let alias = target.strip_prefix("ssh:").unwrap_or(target);
        let target_id = format!("ssh:{alias}");
        let config = self
            .explicit_overrides
            .get(alias)
            .map(|override_config| override_config.merge_with_defaults(alias, &self.alias_defaults))
            .unwrap_or_else(|| self.alias_defaults.for_alias(alias));

        Some((target_id, config))
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
struct SshTargetOverride {
    host: Option<String>,
    remote_zjctl_bin: Option<String>,
    remote_zellij_bin: Option<String>,
    #[serde(default)]
    remote_env: BTreeMap<String, String>,
    ssh_options: Option<Vec<String>>,
}

impl SshTargetOverride {
    fn merge_with_defaults(
        &self,
        alias: &str,
        defaults: &AliasOnlyTargetDefaults,
    ) -> SshTargetConfig {
        let mut resolved = defaults.for_alias(alias);

        if let Some(host) = self.host.as_ref().filter(|host| !host.trim().is_empty()) {
            resolved.host = host.clone();
        }
        if let Some(remote_zjctl_bin) = self
            .remote_zjctl_bin
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            resolved.remote_zjctl_bin = remote_zjctl_bin.clone();
        }
        if let Some(remote_zellij_bin) = self
            .remote_zellij_bin
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            resolved.remote_zellij_bin = remote_zellij_bin.clone();
        }
        resolved.remote_env.extend(self.remote_env.clone());
        if let Some(ssh_options) = self.ssh_options.as_ref() {
            resolved.ssh_options = ssh_options.clone();
        }

        resolved
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct AliasOnlyTargetDefaults {
    #[serde(default = "default_remote_zjctl_bin")]
    remote_zjctl_bin: String,
    #[serde(default = "default_remote_zellij_bin")]
    remote_zellij_bin: String,
    #[serde(default)]
    remote_env: BTreeMap<String, String>,
    #[serde(default)]
    ssh_options: Vec<String>,
}

impl AliasOnlyTargetDefaults {
    fn for_alias(&self, alias: &str) -> SshTargetConfig {
        SshTargetConfig {
            host: alias.to_string(),
            remote_zjctl_bin: self.remote_zjctl_bin.clone(),
            remote_zellij_bin: self.remote_zellij_bin.clone(),
            remote_env: self.remote_env.clone(),
            ssh_options: self.ssh_options.clone(),
        }
    }
}

impl Default for AliasOnlyTargetDefaults {
    fn default() -> Self {
        Self {
            remote_zjctl_bin: default_remote_zjctl_bin(),
            remote_zellij_bin: default_remote_zellij_bin(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct LayeredTargetConfigs {
    #[serde(default, rename = "defaults", alias = "default")]
    alias_defaults: AliasOnlyTargetDefaults,
    #[serde(default, rename = "overrides")]
    explicit_overrides: HashMap<String, SshTargetOverride>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
enum RawTargetConfigs {
    Legacy(HashMap<String, SshTargetConfig>),
    Layered(LayeredTargetConfigs),
}

impl From<RawTargetConfigs> for TargetConfigs {
    fn from(value: RawTargetConfigs) -> Self {
        match value {
            RawTargetConfigs::Legacy(explicit_overrides) => Self {
                alias_defaults: AliasOnlyTargetDefaults::default(),
                explicit_overrides: explicit_overrides
                    .into_iter()
                    .map(|(alias, config)| {
                        (
                            alias,
                            SshTargetOverride {
                                host: Some(config.host),
                                remote_zjctl_bin: Some(config.remote_zjctl_bin),
                                remote_zellij_bin: Some(config.remote_zellij_bin),
                                remote_env: config.remote_env,
                                ssh_options: Some(config.ssh_options),
                            },
                        )
                    })
                    .collect(),
            },
            RawTargetConfigs::Layered(layered) => Self {
                alias_defaults: layered.alias_defaults,
                explicit_overrides: layered.explicit_overrides,
            },
        }
    }
}

fn default_remote_zjctl_bin() -> String {
    "zjctl".to_string()
}

fn default_remote_zellij_bin() -> String {
    "zellij".to_string()
}

fn parse_target_configs(raw_targets: &str) -> Result<TargetConfigs, serde_json::Error> {
    serde_json::from_str::<RawTargetConfigs>(raw_targets).map(Into::into)
}

fn load_target_configs(raw_targets: Option<&str>) -> Result<TargetConfigs, serde_json::Error> {
    raw_targets
        .map(parse_target_configs)
        .transpose()
        .map(|configs| configs.unwrap_or_else(TargetConfigs::default))
}

fn build_remote_backend(
    target_id: String,
    config: &SshTargetConfig,
    registry_store: &RegistryStore,
    observation_store: &ObservationStore,
) -> Result<Arc<dyn TerminalManager>, DomainError> {
    let resolved_config = resolve_ssh_runtime_config(config).map_err(|error| match error {
        zellij_mcp::adapters::zjctl::AdapterError::ZjctlUnavailable => DomainError::new(
            zellij_mcp::domain::errors::ErrorCode::ZjctlUnavailable,
            format!(
                "remote target `{target_id}` on host `{}` is not reachable over SSH; verify the SSH alias, connectivity, and command availability before retrying",
                config.host
            ),
            true,
        ),
        zellij_mcp::adapters::zjctl::AdapterError::CommandFailed(message)
            if missing_binary_name(&message).is_some() =>
        {
            DomainError::new(
                zellij_mcp::domain::errors::ErrorCode::ZjctlUnavailable,
                format!(
                    "remote target `{target_id}` on host `{}` could not resolve required binaries before session selection: {message}",
                    config.host
                ),
                true,
            )
        }
        zellij_mcp::adapters::zjctl::AdapterError::CommandFailed(message) => DomainError::new(
            zellij_mcp::domain::errors::ErrorCode::ZjctlUnavailable,
            format!(
                "remote target `{target_id}` on host `{}` failed during runtime preparation before session selection: {message}",
                config.host
            ),
            true,
        ),
        other => DomainError::new(
            zellij_mcp::domain::errors::ErrorCode::ZjctlUnavailable,
            format!(
                "remote target `{target_id}` on host `{}` failed during runtime preparation before session selection: {}",
                config.host, other
            ),
            true,
        ),
    })?;

    Ok(Arc::new(TerminalService::new(
        target_id,
        SshZjctlClient::new(resolved_config),
        registry_store.clone(),
        observation_store.clone(),
    )) as Arc<dyn TerminalManager>)
}

#[tokio::main]
async fn main() {
    let state_dir = std::env::var("ZELLIJ_MCP_STATE_DIR").unwrap_or_else(|_| "state".to_string());
    let registry_store = RegistryStore::new(format!("{state_dir}/registry.json"));
    let observation_store = ObservationStore::new(format!("{state_dir}/observations.json"));

    let local_service = Arc::new(TerminalService::new(
        "local",
        ZjctlClient::new(),
        registry_store.clone(),
        observation_store.clone(),
    ));
    let _ = local_service.revalidate_all();
    eprintln!(
        "zellij_mcp starting: daemon={} version={} build_stamp={} pid={} started_at={}",
        daemon_identity().instance_id,
        daemon_identity().version,
        daemon_identity().build_stamp,
        daemon_identity().process_id,
        daemon_identity().started_at.to_rfc3339(),
    );

    let mut backends: HashMap<String, Arc<dyn TerminalManager>> = HashMap::new();
    backends.insert("local".to_string(), local_service);

    let target_configs = load_target_configs(std::env::var("ZELLIJ_MCP_TARGETS").ok().as_deref())
        .unwrap_or_else(|error| {
            panic!("failed to parse ZELLIJ_MCP_TARGETS: {error}");
        });

    let remote_backend_factory = {
        let target_configs = target_configs.clone();
        let registry_store = registry_store.clone();
        let observation_store = observation_store.clone();

        Box::new(
            move |target_id: &str| -> Result<Option<Arc<dyn TerminalManager>>, DomainError> {
                let Some((target_id, config)) = target_configs.resolve_ssh_alias_target(target_id)
                else {
                    return Ok(None);
                };

                Ok(Some(build_remote_backend(
                    target_id,
                    &config,
                    &registry_store,
                    &observation_store,
                )?))
            },
        )
    };

    let server = McpServer::new(Box::new(TargetRouter::new(
        registry_store,
        backends,
        Some(remote_backend_factory),
    )));

    if let Err(error) = RmcpServer::new(server).serve_stdio().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        SshTargetOverride, build_remote_backend, default_remote_zellij_bin,
        default_remote_zjctl_bin, load_target_configs, parse_target_configs,
    };
    use zellij_mcp::adapters::zjctl::SshTargetConfig;
    use zellij_mcp::persistence::{ObservationStore, RegistryStore};

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
            parsed.explicit_overrides.get("a100"),
            Some(&SshTargetOverride {
                host: Some("a100".to_string()),
                remote_zjctl_bin: Some("/home/yang/bin/zjctl".to_string()),
                remote_zellij_bin: Some("zellij".to_string()),
                remote_env,
                ssh_options: Some(vec!["-o".to_string(), "BatchMode=yes".to_string()]),
            })
        );
    }

    #[test]
    fn resolves_ssh_alias_target_without_explicit_config() {
        let parsed = load_target_configs(None).expect("missing env should use alias defaults");

        assert!(parsed.explicit_overrides.is_empty());
        assert_eq!(
            parsed.resolve_ssh_alias_target("aws"),
            Some((
                "ssh:aws".to_string(),
                SshTargetConfig {
                    host: "aws".to_string(),
                    remote_zjctl_bin: default_remote_zjctl_bin(),
                    remote_zellij_bin: default_remote_zellij_bin(),
                    remote_env: BTreeMap::new(),
                    ssh_options: Vec::new(),
                }
            ))
        );
        assert_eq!(
            parsed.resolve_ssh_alias_target("ssh:aws"),
            parsed.resolve_ssh_alias_target("aws")
        );
        assert_eq!(parsed.resolve_ssh_alias_target("local"), None);
    }

    #[test]
    fn remote_target_override_still_wins_over_alias_defaults() {
        let parsed = parse_target_configs(
            r#"{
                "defaults": {
                    "remote_zjctl_bin": "zjctl",
                    "remote_zellij_bin": "zellij",
                    "remote_env": {"ZELLIJ_SESSION_NAME": "default"},
                    "ssh_options": ["-o", "BatchMode=yes"]
                },
                "overrides": {
                    "a100": {
                        "host": "gpu-a100.internal",
                        "remote_env": {"ZELLIJ_SESSION_NAME": "a100", "GPU_CLASS": "a100"},
                        "ssh_options": ["-p", "2222"]
                    }
                }
            }"#,
        )
        .expect("layered config should parse");

        let mut override_env = BTreeMap::new();
        override_env.insert("GPU_CLASS".to_string(), "a100".to_string());
        override_env.insert("ZELLIJ_SESSION_NAME".to_string(), "a100".to_string());

        assert_eq!(
            parsed.resolve_ssh_alias_target("a100"),
            Some((
                "ssh:a100".to_string(),
                SshTargetConfig {
                    host: "gpu-a100.internal".to_string(),
                    remote_zjctl_bin: "zjctl".to_string(),
                    remote_zellij_bin: "zellij".to_string(),
                    remote_env: override_env,
                    ssh_options: vec!["-p".to_string(), "2222".to_string()],
                }
            ))
        );
    }

    #[test]
    fn parses_defaults_field_name_for_layered_target_configs() {
        let parsed = parse_target_configs(
            r#"{
                "defaults": {
                    "remote_zjctl_bin": "zjctl",
                    "remote_zellij_bin": "zellij",
                    "remote_env": {"GLOBAL": "1"},
                    "ssh_options": ["-o", "BatchMode=yes"]
                }
            }"#,
        )
        .expect("defaults field should parse");

        assert_eq!(
            parsed.resolve_ssh_alias_target("aws"),
            Some((
                "ssh:aws".to_string(),
                SshTargetConfig {
                    host: "aws".to_string(),
                    remote_zjctl_bin: "zjctl".to_string(),
                    remote_zellij_bin: "zellij".to_string(),
                    remote_env: BTreeMap::from([("GLOBAL".to_string(), "1".to_string())]),
                    ssh_options: vec!["-o".to_string(), "BatchMode=yes".to_string()],
                }
            ))
        );
    }

    #[test]
    fn partial_override_can_change_remote_env_without_repeating_binary_paths() {
        let parsed = parse_target_configs(
            r#"{
                "defaults": {
                    "remote_zjctl_bin": "zjctl",
                    "remote_zellij_bin": "zellij",
                    "remote_env": {"GLOBAL": "1", "ZELLIJ_SESSION_NAME": "default"}
                },
                "overrides": {
                    "aws": {
                        "remote_env": {"ZELLIJ_SESSION_NAME": "aws"}
                    }
                }
            }"#,
        )
        .expect("partial remote_env override should parse");

        assert_eq!(
            parsed.resolve_ssh_alias_target("aws"),
            Some((
                "ssh:aws".to_string(),
                SshTargetConfig {
                    host: "aws".to_string(),
                    remote_zjctl_bin: "zjctl".to_string(),
                    remote_zellij_bin: "zellij".to_string(),
                    remote_env: BTreeMap::from([
                        ("GLOBAL".to_string(), "1".to_string()),
                        ("ZELLIJ_SESSION_NAME".to_string(), "aws".to_string()),
                    ]),
                    ssh_options: Vec::new(),
                }
            ))
        );
    }

    #[test]
    fn invalid_zellij_mcp_targets_json_returns_parse_error() {
        let error = parse_target_configs("not-json").expect_err("invalid JSON should fail");
        assert!(
            error.to_string().contains("expected ident")
                || error.to_string().contains("expected value")
        );
    }

    fn remote_backend_stores() -> (RegistryStore, ObservationStore) {
        let path = std::env::temp_dir().join(format!("zellij-main-test-{}", uuid::Uuid::new_v4()));
        (
            RegistryStore::new(path.join("registry.json")),
            ObservationStore::new(path.join("observations.json")),
        )
    }

    #[test]
    fn remote_backend_construction_is_session_agnostic() {
        let (registry_store, observation_store) = remote_backend_stores();
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let result = build_remote_backend(
            "ssh:aws".to_string(),
            &config,
            &registry_store,
            &observation_store,
        );

        assert!(
            result.is_ok(),
            "backend construction should not require session readiness"
        );
    }
}
