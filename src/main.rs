use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Deserialize;
use zellij_mcp::adapters::zjctl::{
    SshBackendReadiness, SshReadinessFailure, SshTargetConfig, SshZjctlClient, ZjctlClient,
    attempt_safe_ssh_readiness_remediation, classify_ssh_backend_readiness,
};
use zellij_mcp::domain::errors::{DomainError, ErrorCode};
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

fn map_remote_readiness_failure(target_id: &str, failure: SshReadinessFailure) -> DomainError {
    match failure {
        SshReadinessFailure::MissingBinary { host, binary } => DomainError::new(
            ErrorCode::ZjctlUnavailable,
            format!(
                "remote target `{target_id}` on host `{host}` is missing required binary `{binary}`; install it on the remote PATH (for example ~/.local/bin) before retrying"
            ),
            true,
        ),
        SshReadinessFailure::SshUnreachable { host, .. } => DomainError::new(
            ErrorCode::ZjctlUnavailable,
            format!(
                "remote target `{target_id}` on host `{host}` is not reachable over SSH; verify the SSH alias, connectivity, and command availability before retrying"
            ),
            true,
        ),
        SshReadinessFailure::PluginPermissionPrompt { host, .. } => DomainError::new(
            ErrorCode::PluginNotReady,
            format!(
                "remote target `{target_id}` on host `{host}` is waiting on a Zellij plugin permission prompt; approve the plugin permissions before retrying"
            ),
            false,
        ),
        SshReadinessFailure::HelperClientMissing { host, .. } => DomainError::new(
            ErrorCode::PluginNotReady,
            format!(
                "remote target `{target_id}` on host `{host}` does not have an attached Zellij helper/client yet; start or attach a helper client before retrying"
            ),
            true,
        ),
        SshReadinessFailure::RpcNotReady { host, .. } => DomainError::new(
            ErrorCode::PluginNotReady,
            format!(
                "remote target `{target_id}` on host `{host}` is not ready for zjctl RPC yet; ensure the remote Zellij helper/client is attached and RPC is ready before retrying"
            ),
            true,
        ),
    }
}

fn build_remote_backend_with_classifier<F>(
    target_id: String,
    config: &SshTargetConfig,
    registry_store: &RegistryStore,
    observation_store: &ObservationStore,
    classify: F,
) -> Result<Arc<dyn TerminalManager>, DomainError>
where
    F: Fn(&SshTargetConfig) -> SshBackendReadiness,
{
    build_remote_backend_with_classifier_and_remediation(
        target_id,
        config,
        registry_store,
        observation_store,
        classify,
        attempt_safe_ssh_readiness_remediation,
    )
}

fn build_remote_backend_with_classifier_and_remediation<F, R>(
    target_id: String,
    config: &SshTargetConfig,
    registry_store: &RegistryStore,
    observation_store: &ObservationStore,
    classify: F,
    remediate: R,
) -> Result<Arc<dyn TerminalManager>, DomainError>
where
    F: Fn(&SshTargetConfig) -> SshBackendReadiness,
    R: Fn(&SshTargetConfig, &SshReadinessFailure) -> bool,
{
    let resolved_config = match classify(config) {
        SshBackendReadiness::Ready(resolved) => resolved,
        SshBackendReadiness::AutoFixable(failure) => {
            if remediate(config, &failure) {
                match classify(config) {
                    SshBackendReadiness::Ready(resolved) => resolved,
                    SshBackendReadiness::AutoFixable(retry_failure)
                    | SshBackendReadiness::ManualActionRequired(retry_failure) => {
                        return Err(map_remote_readiness_failure(&target_id, retry_failure));
                    }
                }
            } else {
                return Err(map_remote_readiness_failure(&target_id, failure));
            }
        }
        SshBackendReadiness::ManualActionRequired(failure) => {
            return Err(map_remote_readiness_failure(&target_id, failure));
        }
    };

    let service = TerminalService::new(
        target_id,
        SshZjctlClient::new(resolved_config),
        registry_store.clone(),
        observation_store.clone(),
    );
    let _ = service.revalidate_all();

    Ok(Arc::new(service) as Arc<dyn TerminalManager>)
}

fn persisted_remote_target_ids(registry_store: &RegistryStore) -> Result<Vec<String>, DomainError> {
    let mut targets: Vec<String> = registry_store
        .load()?
        .into_iter()
        .map(|binding| binding.target_id)
        .filter(|target_id| target_id != "local")
        .collect();
    targets.sort();
    targets.dedup();
    Ok(targets)
}

fn build_remote_backend(
    target_id: String,
    config: &SshTargetConfig,
    registry_store: &RegistryStore,
    observation_store: &ObservationStore,
) -> Result<Arc<dyn TerminalManager>, DomainError> {
    build_remote_backend_with_classifier(
        target_id,
        config,
        registry_store,
        observation_store,
        classify_ssh_backend_readiness,
    )
}

#[tokio::main]
async fn main() {
    let zjctl_binary = std::env::var("ZJCTL_BIN").unwrap_or_else(|_| "zjctl".to_string());
    let state_dir = std::env::var("ZELLIJ_MCP_STATE_DIR").unwrap_or_else(|_| "state".to_string());
    let registry_store = RegistryStore::new(format!("{state_dir}/registry.json"));
    let observation_store = ObservationStore::new(format!("{state_dir}/observations.json"));

    let local_service = Arc::new(TerminalService::new(
        "local",
        ZjctlClient::new(zjctl_binary),
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

    for target_id in persisted_remote_target_ids(&registry_store).unwrap_or_default() {
        match remote_backend_factory(&target_id) {
            Ok(Some(backend)) => {
                backends.insert(target_id, backend);
            }
            Ok(None) => {
                eprintln!(
                    "zellij_mcp startup skipped persisted target that is no longer configured"
                );
            }
            Err(error) => {
                eprintln!(
                    "zellij_mcp startup failed to preload target `{}`: {} ({:?})",
                    target_id, error.message, error.code
                );
            }
        }
    }

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
        SshTargetOverride, build_remote_backend_with_classifier,
        build_remote_backend_with_classifier_and_remediation, default_remote_zellij_bin,
        default_remote_zjctl_bin, load_target_configs, map_remote_readiness_failure,
        parse_target_configs, persisted_remote_target_ids,
    };
    use std::cell::{Cell, RefCell};
    use zellij_mcp::adapters::zjctl::SshTargetConfig;
    use zellij_mcp::adapters::zjctl::{SshBackendReadiness, SshReadinessFailure};
    use zellij_mcp::domain::binding::TerminalBinding;
    use zellij_mcp::domain::errors::ErrorCode;
    use zellij_mcp::domain::status::{BindingSource, TerminalStatus};
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
    fn persisted_remote_target_ids_collects_unique_non_local_targets() {
        let path = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        let registry = RegistryStore::new(path.join("registry.json"));
        let now = chrono::Utc::now();

        registry
            .save(&[
                TerminalBinding {
                    handle: "zh_local".to_string(),
                    target_id: "local".to_string(),
                    alias: None,
                    session_name: "gpu".to_string(),
                    tab_name: None,
                    selector: "id:terminal:1".to_string(),
                    pane_id: Some("terminal:1".to_string()),
                    cwd: None,
                    launch_command: None,
                    source: BindingSource::Attached,
                    status: TerminalStatus::Ready,
                    created_at: now,
                    updated_at: now,
                },
                TerminalBinding {
                    handle: "zh_a".to_string(),
                    target_id: "ssh:a100".to_string(),
                    alias: None,
                    session_name: "gpu".to_string(),
                    tab_name: None,
                    selector: "id:terminal:2".to_string(),
                    pane_id: Some("terminal:2".to_string()),
                    cwd: None,
                    launch_command: None,
                    source: BindingSource::Attached,
                    status: TerminalStatus::Ready,
                    created_at: now,
                    updated_at: now,
                },
                TerminalBinding {
                    handle: "zh_b".to_string(),
                    target_id: "ssh:a100".to_string(),
                    alias: None,
                    session_name: "gpu".to_string(),
                    tab_name: None,
                    selector: "id:terminal:3".to_string(),
                    pane_id: Some("terminal:3".to_string()),
                    cwd: None,
                    launch_command: None,
                    source: BindingSource::Attached,
                    status: TerminalStatus::Busy,
                    created_at: now,
                    updated_at: now,
                },
                TerminalBinding {
                    handle: "zh_c".to_string(),
                    target_id: "ssh:aws".to_string(),
                    alias: None,
                    session_name: "gpu".to_string(),
                    tab_name: None,
                    selector: "id:terminal:4".to_string(),
                    pane_id: Some("terminal:4".to_string()),
                    cwd: None,
                    launch_command: None,
                    source: BindingSource::Attached,
                    status: TerminalStatus::Ready,
                    created_at: now,
                    updated_at: now,
                },
            ])
            .expect("registry should save");

        assert_eq!(
            persisted_remote_target_ids(&registry).expect("target ids should load"),
            vec!["ssh:a100".to_string(), "ssh:aws".to_string()]
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
    fn readiness_reports_missing_binaries_with_actionable_message() {
        let error = map_remote_readiness_failure(
            "ssh:aws",
            SshReadinessFailure::MissingBinary {
                host: "aws".to_string(),
                binary: "zjctl".to_string(),
            },
        );

        assert_eq!(error.code, ErrorCode::ZjctlUnavailable);
        assert!(error.retryable);
        assert!(error.message.contains("missing required binary `zjctl`"));
        assert!(error.message.contains("~/.local/bin"));
    }

    #[test]
    fn readiness_reports_plugin_permission_prompt_with_actionable_message() {
        let error = map_remote_readiness_failure(
            "ssh:aws",
            SshReadinessFailure::PluginPermissionPrompt {
                host: "aws".to_string(),
                detail: "permission prompt".to_string(),
            },
        );

        assert_eq!(error.code, ErrorCode::PluginNotReady);
        assert!(!error.retryable);
        assert!(error.message.contains("plugin permission prompt"));
        assert!(error.message.contains("approve"));
    }

    #[test]
    fn readiness_reports_helper_client_absence_with_actionable_message() {
        let error = map_remote_readiness_failure(
            "ssh:aws",
            SshReadinessFailure::HelperClientMissing {
                host: "aws".to_string(),
                detail: "helper client is not attached yet".to_string(),
            },
        );

        assert_eq!(error.code, ErrorCode::PluginNotReady);
        assert!(error.retryable);
        assert!(error.message.contains("helper/client"));
        assert!(error.message.contains("before retrying"));
    }

    #[test]
    fn readiness_reports_rpc_timeout_without_fresh_ssh_fallback() {
        let (registry_store, observation_store) = remote_backend_stores();
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };

        let result = build_remote_backend_with_classifier(
            "ssh:aws".to_string(),
            &config,
            &registry_store,
            &observation_store,
            |_| {
                SshBackendReadiness::AutoFixable(SshReadinessFailure::RpcNotReady {
                    host: "aws".to_string(),
                    detail: "timed out".to_string(),
                })
            },
        );

        let error = match result {
            Ok(_) => panic!("rpc timeout readiness should fail before backend construction"),
            Err(error) => error,
        };

        assert_eq!(error.code, ErrorCode::PluginNotReady);
        assert!(error.retryable);
        assert!(error.message.contains("not ready for zjctl RPC yet"));
    }

    #[test]
    fn readiness_auto_fix_retries_doctor_once_after_safe_remediation() {
        let (registry_store, observation_store) = remote_backend_stores();
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let classify_calls = Cell::new(0usize);
        let remediation_calls = Cell::new(0usize);

        let result = build_remote_backend_with_classifier_and_remediation(
            "ssh:aws".to_string(),
            &config,
            &registry_store,
            &observation_store,
            |_| {
                let call = classify_calls.get();
                classify_calls.set(call + 1);
                if call == 0 {
                    SshBackendReadiness::AutoFixable(SshReadinessFailure::HelperClientMissing {
                        host: "aws".to_string(),
                        detail: "helper client is not attached yet".to_string(),
                    })
                } else {
                    SshBackendReadiness::Ready(config.clone())
                }
            },
            |_, failure| {
                remediation_calls.set(remediation_calls.get() + 1);
                assert!(matches!(
                    failure,
                    SshReadinessFailure::HelperClientMissing { .. }
                ));
                true
            },
        );

        assert!(
            result.is_ok(),
            "safe remediation should allow one readiness retry"
        );
        assert_eq!(remediation_calls.get(), 1);
        assert_eq!(classify_calls.get(), 2);
    }

    #[test]
    fn readiness_does_not_auto_approve_unmanaged_prompt() {
        let (registry_store, observation_store) = remote_backend_stores();
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let remediation_calls = Cell::new(0usize);

        let result = build_remote_backend_with_classifier_and_remediation(
            "ssh:aws".to_string(),
            &config,
            &registry_store,
            &observation_store,
            |_| {
                SshBackendReadiness::ManualActionRequired(
                    SshReadinessFailure::PluginPermissionPrompt {
                        host: "aws".to_string(),
                        detail: "Allow? (y/n)".to_string(),
                    },
                )
            },
            |_, _| {
                remediation_calls.set(remediation_calls.get() + 1);
                true
            },
        );

        let error = match result {
            Ok(_) => panic!("permission prompt should stay manual"),
            Err(error) => error,
        };
        assert_eq!(remediation_calls.get(), 0);
        assert_eq!(error.code, ErrorCode::PluginNotReady);
        assert!(!error.retryable);
        assert!(error.message.contains("approve the plugin permissions"));
    }

    #[test]
    fn readiness_auto_fix_reports_missing_binary_when_remediation_does_not_help() {
        let (registry_store, observation_store) = remote_backend_stores();
        let config = SshTargetConfig {
            host: "aws".to_string(),
            remote_zjctl_bin: "zjctl".to_string(),
            remote_zellij_bin: "zellij".to_string(),
            remote_env: BTreeMap::new(),
            ssh_options: Vec::new(),
        };
        let attempted_failures = RefCell::new(Vec::new());

        let result = build_remote_backend_with_classifier_and_remediation(
            "ssh:aws".to_string(),
            &config,
            &registry_store,
            &observation_store,
            |_| {
                SshBackendReadiness::AutoFixable(SshReadinessFailure::MissingBinary {
                    host: "aws".to_string(),
                    binary: "zjctl".to_string(),
                })
            },
            |_, failure| {
                attempted_failures.borrow_mut().push(failure.clone());
                false
            },
        );

        let error = match result {
            Ok(_) => panic!("missing binary should fail if remediation does not succeed"),
            Err(error) => error,
        };
        assert_eq!(
            attempted_failures.borrow().as_slice(),
            &[SshReadinessFailure::MissingBinary {
                host: "aws".to_string(),
                binary: "zjctl".to_string(),
            }]
        );
        assert_eq!(error.code, ErrorCode::ZjctlUnavailable);
    }
}
