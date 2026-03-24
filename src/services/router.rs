use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::domain::errors::{DomainError, ErrorCode};
use crate::domain::requests::{
    AttachRequest, CaptureRequest, CleanupRequest, CloseRequest, DiscoverRequest, LayoutRequest,
    ListRequest, ReplaceRequest, SendRequest, SpawnRequest, TakeoverRequest, WaitRequest,
};
use crate::domain::responses::{
    AttachResponse, CaptureResponse, CleanupResponse, CloseResponse, DiscoverResponse,
    LayoutResponse, ListResponse, ReplaceResponse, SendResponse, SpawnResponse, TakeoverResponse,
    WaitResponse,
};
use crate::persistence::RegistryStore;

use super::TerminalManager;

type BackendFactory =
    dyn Fn(&str) -> Result<Option<Arc<dyn TerminalManager>>, DomainError> + Send + Sync;

pub struct TargetRouter {
    registry_store: RegistryStore,
    backends: RwLock<HashMap<String, Arc<dyn TerminalManager>>>,
    remote_backend_factory: Option<Box<BackendFactory>>,
}

impl TargetRouter {
    pub fn new(
        registry_store: RegistryStore,
        backends: HashMap<String, Arc<dyn TerminalManager>>,
        remote_backend_factory: Option<Box<BackendFactory>>,
    ) -> Self {
        Self {
            registry_store,
            backends: RwLock::new(backends),
            remote_backend_factory,
        }
    }

    fn resolve_target_id(target: Option<&str>) -> String {
        match target {
            Some(target) if !target.trim().is_empty() => {
                let target = target.trim();
                if target == "local" || target.starts_with("ssh:") {
                    target.to_string()
                } else {
                    format!("ssh:{target}")
                }
            }
            _ => "local".to_string(),
        }
    }

    fn backend_for_target(
        &self,
        target: Option<&str>,
    ) -> Result<Arc<dyn TerminalManager>, DomainError> {
        let target_id = Self::resolve_target_id(target);
        self.backend_for_target_id(&target_id)
    }

    fn backend_for_target_id(
        &self,
        target_id: &str,
    ) -> Result<Arc<dyn TerminalManager>, DomainError> {
        if let Some(backend) = self
            .backends
            .read()
            .expect("router backend read lock should succeed")
            .get(target_id)
            .cloned()
        {
            return Ok(backend);
        }

        if let Some(factory) = &self.remote_backend_factory
            && let Some(backend) = factory(target_id)?
        {
            let mut backends = self
                .backends
                .write()
                .expect("router backend write lock should succeed");
            return Ok(backends
                .entry(target_id.to_string())
                .or_insert_with(|| backend.clone())
                .clone());
        }

        Err(DomainError::new(
            ErrorCode::TargetNotFound,
            format!("target `{target_id}` is not configured"),
            false,
        ))
    }

    fn backend_for_handle(&self, handle: &str) -> Result<Arc<dyn TerminalManager>, DomainError> {
        let bindings = self.registry_store.load()?;
        let binding = bindings
            .iter()
            .find(|binding| binding.handle == handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("handle `{handle}` is not registered"),
                    false,
                )
            })?;
        self.backend_for_target_id(&binding.target_id)
            .map_err(|error| {
                if error.code == ErrorCode::TargetNotFound {
                    DomainError::new(
                        ErrorCode::TargetNotFound,
                        format!("binding target `{}` is not configured", binding.target_id),
                        false,
                    )
                } else {
                    error
                }
            })
    }
}

impl TerminalManager for TargetRouter {
    fn spawn(&self, request: SpawnRequest) -> Result<SpawnResponse, DomainError> {
        self.backend_for_target(request.target.as_deref())?
            .spawn(request)
    }

    fn attach(&self, request: AttachRequest) -> Result<AttachResponse, DomainError> {
        self.backend_for_target(request.target.as_deref())?
            .attach(request)
    }

    fn takeover(&self, request: TakeoverRequest) -> Result<TakeoverResponse, DomainError> {
        self.backend_for_target(request.target.as_deref())?
            .takeover(request)
    }

    fn discover(&self, request: DiscoverRequest) -> Result<DiscoverResponse, DomainError> {
        self.backend_for_target(request.target.as_deref())?
            .discover(request)
    }

    fn list(&self, request: ListRequest) -> Result<ListResponse, DomainError> {
        self.backend_for_target(request.target.as_deref())?
            .list(request)
    }

    fn capture(&self, request: CaptureRequest) -> Result<CaptureResponse, DomainError> {
        self.backend_for_handle(&request.handle)?.capture(request)
    }

    fn send(&self, request: SendRequest) -> Result<SendResponse, DomainError> {
        self.backend_for_handle(&request.handle)?.send(request)
    }

    fn replace(&self, request: ReplaceRequest) -> Result<ReplaceResponse, DomainError> {
        self.backend_for_handle(&request.handle)?.replace(request)
    }

    fn cleanup(&self, request: CleanupRequest) -> Result<CleanupResponse, DomainError> {
        self.backend_for_target(request.target.as_deref())?
            .cleanup(request)
    }

    fn layout(&self, request: LayoutRequest) -> Result<LayoutResponse, DomainError> {
        self.backend_for_target(request.target.as_deref())?
            .layout(request)
    }

    fn wait(&self, request: WaitRequest) -> Result<WaitResponse, DomainError> {
        self.backend_for_handle(&request.handle)?.wait(request)
    }

    fn close(&self, request: CloseRequest) -> Result<CloseResponse, DomainError> {
        self.backend_for_handle(&request.handle)?.close(request)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use chrono::Utc;

    use super::TargetRouter;
    use crate::domain::binding::TerminalBinding;
    use crate::domain::errors::{DomainError, ErrorCode};
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
    use crate::domain::status::{BindingSource, CaptureMode, SpawnTarget, TerminalStatus};
    use crate::persistence::RegistryStore;
    use crate::services::TerminalManager;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Spawn(Option<String>),
        Attach(Option<String>),
        Discover(Option<String>),
        List(Option<String>),
        Capture(String),
        Send(String),
        Wait(String),
        Close(String),
    }

    #[derive(Clone)]
    struct RecordingTerminalManager {
        target_id: String,
        calls: Arc<Mutex<Vec<Call>>>,
        bindings: Vec<TerminalBinding>,
    }

    impl RecordingTerminalManager {
        fn new(target_id: impl Into<String>) -> Self {
            Self::with_bindings(target_id, Vec::new())
        }

        fn with_bindings(target_id: impl Into<String>, bindings: Vec<TerminalBinding>) -> Self {
            Self {
                target_id: target_id.into(),
                calls: Arc::new(Mutex::new(Vec::new())),
                bindings,
            }
        }

        fn record(&self, call: Call) {
            self.calls
                .lock()
                .expect("calls lock should succeed")
                .push(call);
        }
    }

    impl TerminalManager for RecordingTerminalManager {
        fn spawn(&self, request: SpawnRequest) -> Result<SpawnResponse, DomainError> {
            self.record(Call::Spawn(request.target.clone()));
            Ok(SpawnResponse {
                handle: "zh_spawn".to_string(),
                target_id: self.target_id.clone(),
                session_name: request.session_name,
                tab_name: request.tab_name,
                selector: "id:terminal:1".to_string(),
                status: "ready".to_string(),
            })
        }

        fn attach(&self, request: AttachRequest) -> Result<AttachResponse, DomainError> {
            self.record(Call::Attach(request.target.clone()));
            Ok(AttachResponse {
                handle: "zh_attach".to_string(),
                target_id: self.target_id.clone(),
                attached: true,
                baseline_established: true,
            })
        }

        fn takeover(&self, request: TakeoverRequest) -> Result<TakeoverResponse, DomainError> {
            self.record(Call::Attach(request.target.clone()));
            Ok(TakeoverResponse {
                handle: "zh_takeover".to_string(),
                target_id: self.target_id.clone(),
                attached: true,
                baseline_established: true,
                matched_selector: "id:terminal:1".to_string(),
            })
        }

        fn discover(&self, request: DiscoverRequest) -> Result<DiscoverResponse, DomainError> {
            self.record(Call::Discover(request.target.clone()));
            Ok(DiscoverResponse {
                candidates: vec![DiscoverCandidate {
                    target_id: self.target_id.clone(),
                    selector: "id:terminal:1".to_string(),
                    pane_id: Some("terminal:1".to_string()),
                    session_name: request.session_name,
                    tab_name: request.tab_name,
                    title: Some("shell".to_string()),
                    command: Some("fish".to_string()),
                    focused: false,
                    preview: None,
                    preview_basis: None,
                    captured_at: None,
                }],
            })
        }

        fn list(&self, request: ListRequest) -> Result<ListResponse, DomainError> {
            self.record(Call::List(request.target.clone()));
            Ok(ListResponse {
                bindings: self.bindings.clone(),
            })
        }

        fn capture(&self, request: CaptureRequest) -> Result<CaptureResponse, DomainError> {
            self.record(Call::Capture(request.handle.clone()));
            Ok(CaptureResponse {
                capture: CaptureResult {
                    handle: request.handle,
                    mode: "full".to_string(),
                    content: "ok".to_string(),
                    tail_lines: None,
                    line_offset: None,
                    line_limit: None,
                    line_window_applied: false,
                    next_cursor: None,
                    ansi_normalized: false,
                    truncated: false,
                    captured_at: Utc::now(),
                    baseline: None,
                    interaction_id: None,
                    interaction_completed: None,
                    interaction_exit_code: None,
                },
            })
        }

        fn send(&self, request: SendRequest) -> Result<SendResponse, DomainError> {
            self.record(Call::Send(request.handle.clone()));
            Ok(SendResponse {
                handle: request.handle,
                accepted: true,
            })
        }

        fn replace(&self, request: ReplaceRequest) -> Result<ReplaceResponse, DomainError> {
            self.record(Call::Send(request.handle.clone()));
            Ok(ReplaceResponse {
                handle: request.handle,
                replaced: true,
                interaction_id: Some("zi_replace".to_string()),
            })
        }

        fn cleanup(&self, request: CleanupRequest) -> Result<CleanupResponse, DomainError> {
            self.record(Call::List(request.target.clone()));
            Ok(CleanupResponse {
                removed_handles: vec!["zh_cleanup".to_string()],
                removed_count: 1,
                dry_run: request.dry_run,
            })
        }

        fn layout(&self, request: LayoutRequest) -> Result<LayoutResponse, DomainError> {
            self.record(Call::List(request.target.clone()));
            Ok(LayoutResponse {
                target_id: self.target_id.clone(),
                session_name: request.session_name,
                tabs: vec![LayoutTab {
                    tab_name: "editor".to_string(),
                    panes: vec![DiscoverCandidate {
                        target_id: self.target_id.clone(),
                        selector: "id:terminal:1".to_string(),
                        pane_id: Some("terminal:1".to_string()),
                        session_name: "gpu".to_string(),
                        tab_name: Some("editor".to_string()),
                        title: Some("shell".to_string()),
                        command: Some("fish".to_string()),
                        focused: false,
                        preview: None,
                        preview_basis: None,
                        captured_at: None,
                    }],
                }],
            })
        }

        fn wait(&self, request: WaitRequest) -> Result<WaitResponse, DomainError> {
            self.record(Call::Wait(request.handle.clone()));
            Ok(WaitResponse {
                handle: request.handle,
                status: "idle".to_string(),
                observed_at: Utc::now(),
                completion_basis: None,
                interaction_id: None,
                interaction_completed: None,
                interaction_exit_code: None,
            })
        }

        fn close(&self, request: CloseRequest) -> Result<CloseResponse, DomainError> {
            self.record(Call::Close(request.handle.clone()));
            Ok(CloseResponse {
                handle: request.handle,
                closed: true,
            })
        }
    }

    fn temp_registry_path() -> PathBuf {
        std::env::temp_dir().join(format!("zellij-router-test-{}.json", uuid::Uuid::new_v4()))
    }

    fn build_router(
        registry_store: RegistryStore,
        local: RecordingTerminalManager,
        remote: Option<RecordingTerminalManager>,
    ) -> TargetRouter {
        let mut backends: HashMap<String, Arc<dyn TerminalManager>> = HashMap::new();
        backends.insert("local".to_string(), Arc::new(local));
        if let Some(remote) = remote {
            backends.insert(remote.target_id.clone(), Arc::new(remote));
        }
        TargetRouter::new(registry_store, backends, None)
    }

    fn build_lazy_router(
        registry_store: RegistryStore,
        local: RecordingTerminalManager,
        creation_count: Arc<AtomicUsize>,
        created_targets: Arc<Mutex<Vec<String>>>,
        remote_calls: Arc<Mutex<Vec<Call>>>,
    ) -> TargetRouter {
        let mut backends: HashMap<String, Arc<dyn TerminalManager>> = HashMap::new();
        backends.insert("local".to_string(), Arc::new(local));

        TargetRouter::new(
            registry_store,
            backends,
            Some(Box::new(move |target_id| {
                if target_id == "ssh:a100" {
                    creation_count.fetch_add(1, Ordering::SeqCst);
                    created_targets
                        .lock()
                        .expect("created_targets lock should succeed")
                        .push(target_id.to_string());
                    let mut manager = RecordingTerminalManager::with_bindings(
                        target_id,
                        vec![sample_binding("zh_remote_list", target_id)],
                    );
                    manager.calls = remote_calls.clone();
                    Ok(Some(Arc::new(manager)))
                } else {
                    Ok(None)
                }
            })),
        )
    }

    fn sample_binding(handle: &str, target_id: &str) -> TerminalBinding {
        let now = Utc::now();
        TerminalBinding {
            handle: handle.to_string(),
            target_id: target_id.to_string(),
            alias: None,
            session_name: "gpu".to_string(),
            tab_name: Some("editor".to_string()),
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            cwd: None,
            launch_command: Some("fish".to_string()),
            source: BindingSource::Attached,
            status: TerminalStatus::Ready,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn spawn_without_target_routes_to_local_backend() {
        let registry_store = RegistryStore::new(temp_registry_path());
        let local = RecordingTerminalManager::new("local");
        let remote = RecordingTerminalManager::new("ssh:a100");
        let local_calls = local.calls.clone();
        let remote_calls = remote.calls.clone();
        let router = build_router(registry_store, local, Some(remote));

        let response = router
            .spawn(SpawnRequest {
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("lazygit".to_string()),
                argv: None,
                title: Some("lg".to_string()),
                wait_ready: false,
            })
            .expect("spawn should route to local backend");

        assert_eq!(response.target_id, "local");
        assert_eq!(
            *local_calls.lock().expect("calls lock should succeed"),
            vec![Call::Spawn(None)]
        );
        assert!(
            remote_calls
                .lock()
                .expect("calls lock should succeed")
                .is_empty()
        );
    }

    #[test]
    fn attach_with_trimmed_target_routes_to_remote_backend() {
        let registry_store = RegistryStore::new(temp_registry_path());
        let local = RecordingTerminalManager::new("local");
        let remote = RecordingTerminalManager::new("ssh:a100");
        let local_calls = local.calls.clone();
        let remote_calls = remote.calls.clone();
        let router = build_router(registry_store, local, Some(remote));

        let response = router
            .attach(AttachRequest {
                target: Some("  a100  ".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should route to remote backend");

        assert_eq!(response.target_id, "ssh:a100");
        assert!(
            local_calls
                .lock()
                .expect("calls lock should succeed")
                .is_empty()
        );
        assert_eq!(
            *remote_calls.lock().expect("calls lock should succeed"),
            vec![Call::Attach(Some("  a100  ".to_string()))]
        );
    }

    #[test]
    fn discover_with_canonical_target_id_routes_without_double_prefixing() {
        let registry_store = RegistryStore::new(temp_registry_path());
        let local = RecordingTerminalManager::new("local");
        let remote = RecordingTerminalManager::new("ssh:a100");
        let local_calls = local.calls.clone();
        let remote_calls = remote.calls.clone();
        let router = build_router(registry_store, local, Some(remote));

        let response = router
            .discover(DiscoverRequest {
                target: Some("ssh:a100".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: None,
                include_preview: false,
                preview_lines: None,
            })
            .expect("canonical target_id should route to remote backend");

        assert_eq!(response.candidates[0].target_id, "ssh:a100");
        assert!(
            local_calls
                .lock()
                .expect("calls lock should succeed")
                .is_empty()
        );
        assert_eq!(
            *remote_calls.lock().expect("calls lock should succeed"),
            vec![Call::Discover(Some("ssh:a100".to_string()))]
        );
    }

    #[test]
    fn list_with_blank_target_routes_to_local_backend() {
        let registry_store = RegistryStore::new(temp_registry_path());
        let local = RecordingTerminalManager::new("local");
        let remote = RecordingTerminalManager::new("ssh:a100");
        let local_calls = local.calls.clone();
        let remote_calls = remote.calls.clone();
        let router = build_router(registry_store, local, Some(remote));

        router
            .list(ListRequest {
                target: Some("   ".to_string()),
                session_name: Some("gpu".to_string()),
            })
            .expect("blank target should route to local backend");

        assert_eq!(
            *local_calls.lock().expect("calls lock should succeed"),
            vec![Call::List(Some("   ".to_string()))]
        );
        assert!(
            remote_calls
                .lock()
                .expect("calls lock should succeed")
                .is_empty()
        );
    }

    #[test]
    fn follow_up_calls_route_by_persisted_binding_target() {
        let registry_store = RegistryStore::new(temp_registry_path());
        registry_store
            .save(&[
                sample_binding("zh_capture", "ssh:a100"),
                sample_binding("zh_send", "ssh:a100"),
                sample_binding("zh_wait", "ssh:a100"),
                sample_binding("zh_close", "ssh:a100"),
            ])
            .expect("registry save should succeed");

        let local = RecordingTerminalManager::new("local");
        let remote = RecordingTerminalManager::new("ssh:a100");
        let local_calls = local.calls.clone();
        let remote_calls = remote.calls.clone();
        let router = build_router(registry_store, local, Some(remote));

        router
            .capture(CaptureRequest {
                handle: "zh_capture".to_string(),
                mode: CaptureMode::Full,
                tail_lines: None,
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
            })
            .expect("capture should route by binding target");
        router
            .send(SendRequest {
                handle: "zh_send".to_string(),
                text: "printf 'ok'".to_string(),
                keys: Vec::new(),
                input_mode: None,
                submit: true,
            })
            .expect("send should route by binding target");
        router
            .wait(WaitRequest {
                handle: "zh_wait".to_string(),
                idle_ms: 100,
                timeout_ms: 1000,
            })
            .expect("wait should route by binding target");
        router
            .close(CloseRequest {
                handle: "zh_close".to_string(),
                force: true,
            })
            .expect("close should route by binding target");

        assert!(
            local_calls
                .lock()
                .expect("calls lock should succeed")
                .is_empty()
        );
        assert_eq!(
            *remote_calls.lock().expect("calls lock should succeed"),
            vec![
                Call::Capture("zh_capture".to_string()),
                Call::Send("zh_send".to_string()),
                Call::Wait("zh_wait".to_string()),
                Call::Close("zh_close".to_string()),
            ]
        );
    }

    #[test]
    fn discover_with_alias_only_target_creates_ready_remote_backend() {
        let registry_store = RegistryStore::new(temp_registry_path());
        let local = RecordingTerminalManager::new("local");
        let local_calls = local.calls.clone();
        let creation_count = Arc::new(AtomicUsize::new(0));
        let created_targets = Arc::new(Mutex::new(Vec::new()));
        let remote_calls = Arc::new(Mutex::new(Vec::new()));
        let router = build_lazy_router(
            registry_store,
            local,
            creation_count.clone(),
            created_targets.clone(),
            remote_calls.clone(),
        );

        let discover_response = router
            .discover(DiscoverRequest {
                target: Some("a100".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: None,
                include_preview: false,
                preview_lines: None,
            })
            .expect("alias-only target should lazily create remote backend");

        assert_eq!(discover_response.candidates[0].target_id, "ssh:a100");
        assert!(
            local_calls
                .lock()
                .expect("calls lock should succeed")
                .is_empty()
        );
        assert_eq!(creation_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            *remote_calls.lock().expect("calls lock should succeed"),
            vec![Call::Discover(Some("a100".to_string()))]
        );
        assert_eq!(
            *created_targets
                .lock()
                .expect("created_targets lock should succeed"),
            vec!["ssh:a100".to_string()]
        );
    }

    #[test]
    fn attach_with_alias_only_target_preserves_target_id_shape() {
        let registry_store = RegistryStore::new(temp_registry_path());
        let local = RecordingTerminalManager::new("local");
        let local_calls = local.calls.clone();
        let creation_count = Arc::new(AtomicUsize::new(0));
        let created_targets = Arc::new(Mutex::new(Vec::new()));
        let remote_calls = Arc::new(Mutex::new(Vec::new()));
        let router = build_lazy_router(
            registry_store,
            local,
            creation_count.clone(),
            created_targets.clone(),
            remote_calls.clone(),
        );

        let response = router
            .attach(AttachRequest {
                target: Some("a100".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("alias-only attach should lazily create remote backend");

        assert_eq!(response.target_id, "ssh:a100");
        assert!(
            local_calls
                .lock()
                .expect("calls lock should succeed")
                .is_empty()
        );
        assert_eq!(creation_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            *remote_calls.lock().expect("calls lock should succeed"),
            vec![Call::Attach(Some("a100".to_string()))]
        );
        assert_eq!(
            *created_targets
                .lock()
                .expect("created_targets lock should succeed"),
            vec!["ssh:a100".to_string()]
        );
    }

    #[test]
    fn alias_only_and_canonical_selection_reuse_cached_remote_backend_across_selection_calls() {
        let registry_store = RegistryStore::new(temp_registry_path());
        let local = RecordingTerminalManager::new("local");
        let local_calls = local.calls.clone();
        let creation_count = Arc::new(AtomicUsize::new(0));
        let created_targets = Arc::new(Mutex::new(Vec::new()));
        let remote_calls = Arc::new(Mutex::new(Vec::new()));
        let router = build_lazy_router(
            registry_store,
            local,
            creation_count.clone(),
            created_targets.clone(),
            remote_calls.clone(),
        );

        let spawn_response = router
            .spawn(SpawnRequest {
                target: Some("a100".to_string()),
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("lazygit".to_string()),
                argv: None,
                title: Some("lg".to_string()),
                wait_ready: false,
            })
            .expect("bare alias spawn should use lazy remote backend");
        let discover_response = router
            .discover(DiscoverRequest {
                target: Some("ssh:a100".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: None,
                include_preview: false,
                preview_lines: None,
            })
            .expect("canonical discover should reuse cached backend");
        let list_response = router
            .list(ListRequest {
                target: Some("a100".to_string()),
                session_name: Some("gpu".to_string()),
            })
            .expect("alias-only list should reuse cached backend");

        assert_eq!(spawn_response.target_id, "ssh:a100");
        assert_eq!(discover_response.candidates[0].target_id, "ssh:a100");
        assert_eq!(list_response.bindings.len(), 1);
        assert_eq!(list_response.bindings[0].target_id, "ssh:a100");
        assert!(
            local_calls
                .lock()
                .expect("calls lock should succeed")
                .is_empty()
        );
        assert_eq!(creation_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            *remote_calls.lock().expect("calls lock should succeed"),
            vec![
                Call::Spawn(Some("a100".to_string())),
                Call::Discover(Some("ssh:a100".to_string())),
                Call::List(Some("a100".to_string())),
            ]
        );
        assert_eq!(
            *created_targets
                .lock()
                .expect("created_targets lock should succeed"),
            vec!["ssh:a100".to_string()]
        );
    }

    #[test]
    fn follow_up_routes_legacy_binding_without_target_id_to_local_backend() {
        let registry_path = temp_registry_path();
        fs::write(
            &registry_path,
            r#"[
  {
    "handle": "zh_legacy",
    "alias": null,
    "session_name": "gpu",
    "tab_name": "editor",
    "selector": "id:terminal:7",
    "pane_id": "terminal:7",
    "cwd": null,
    "launch_command": "fish",
    "source": "attached",
    "status": "ready",
    "created_at": "2026-03-23T10:00:00Z",
    "updated_at": "2026-03-23T10:00:00Z"
  }
]"#,
        )
        .expect("legacy registry write should succeed");

        let registry_store = RegistryStore::new(registry_path);
        let bindings = registry_store.load().expect("legacy registry should load");
        assert_eq!(bindings[0].target_id, "local");

        let local = RecordingTerminalManager::new("local");
        let remote = RecordingTerminalManager::new("ssh:a100");
        let local_calls = local.calls.clone();
        let remote_calls = remote.calls.clone();
        let router = build_router(registry_store, local, Some(remote));

        router
            .capture(CaptureRequest {
                handle: "zh_legacy".to_string(),
                mode: CaptureMode::Full,
                tail_lines: None,
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
            })
            .expect("legacy binding should route to local backend");

        assert_eq!(
            *local_calls.lock().expect("calls lock should succeed"),
            vec![Call::Capture("zh_legacy".to_string())]
        );
        assert!(
            remote_calls
                .lock()
                .expect("calls lock should succeed")
                .is_empty()
        );
    }

    #[test]
    fn selection_call_returns_target_not_found_for_unknown_target() {
        let registry_store = RegistryStore::new(temp_registry_path());
        let local = RecordingTerminalManager::new("local");
        let router = build_router(registry_store, local, None);

        let error = router
            .discover(DiscoverRequest {
                target: Some("missing".to_string()),
                session_name: "gpu".to_string(),
                tab_name: None,
                selector: None,
                include_preview: false,
                preview_lines: None,
            })
            .expect_err("unknown target should fail");

        assert_eq!(error.code, ErrorCode::TargetNotFound);
        assert!(error.message.contains("ssh:missing"));
    }

    #[test]
    fn follow_up_returns_handle_not_found_when_binding_is_missing() {
        let registry_store = RegistryStore::new(temp_registry_path());
        let local = RecordingTerminalManager::new("local");
        let router = build_router(registry_store, local, None);

        let error = router
            .capture(CaptureRequest {
                handle: "zh_missing".to_string(),
                mode: CaptureMode::Full,
                tail_lines: None,
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
            })
            .expect_err("missing handle should fail");

        assert_eq!(error.code, ErrorCode::HandleNotFound);
        assert!(error.message.contains("zh_missing"));
    }

    #[test]
    fn follow_up_returns_target_not_found_when_binding_target_is_unconfigured() {
        let registry_store = RegistryStore::new(temp_registry_path());
        registry_store
            .save(&[sample_binding("zh_remote", "ssh:a100")])
            .expect("registry save should succeed");
        let local = RecordingTerminalManager::new("local");
        let router = build_router(registry_store, local, None);

        let error = router
            .wait(WaitRequest {
                handle: "zh_remote".to_string(),
                idle_ms: 100,
                timeout_ms: 1000,
            })
            .expect_err("unconfigured binding target should fail");

        assert_eq!(error.code, ErrorCode::TargetNotFound);
        assert!(error.message.contains("ssh:a100"));
    }
}
