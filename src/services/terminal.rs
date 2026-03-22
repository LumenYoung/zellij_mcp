use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::adapters::zjctl::{AdapterError, ZjctlAdapter};
use crate::domain::binding::TerminalBinding;
use crate::domain::errors::{DomainError, ErrorCode};
use crate::domain::observation::{CaptureResult, TerminalObservation};
use chrono::Utc;

use crate::domain::requests::{
    AttachRequest, CaptureRequest, CloseRequest, ListRequest, SendRequest, SpawnRequest,
    WaitRequest,
};
use crate::domain::responses::{
    AttachResponse, CaptureResponse, CloseResponse, ListResponse, SendResponse, SpawnResponse,
    WaitResponse,
};
use crate::domain::status::{BindingSource, CaptureMode, TerminalStatus};
use crate::persistence::{ObservationStore, RegistryStore};

pub trait TerminalManager: Send + Sync {
    fn spawn(&self, request: SpawnRequest) -> Result<SpawnResponse, DomainError>;
    fn attach(&self, request: AttachRequest) -> Result<AttachResponse, DomainError>;
    fn list(&self, request: ListRequest) -> Result<ListResponse, DomainError>;
    fn capture(&self, request: CaptureRequest) -> Result<CaptureResponse, DomainError>;
    fn send(&self, request: SendRequest) -> Result<SendResponse, DomainError>;
    fn wait(&self, request: WaitRequest) -> Result<WaitResponse, DomainError>;
    fn close(&self, request: CloseRequest) -> Result<CloseResponse, DomainError>;
}

#[derive(Debug, Clone)]
pub struct TerminalService<A> {
    adapter: A,
    registry_store: RegistryStore,
    observation_store: ObservationStore,
}

impl<A> TerminalService<A> {
    pub fn new(
        adapter: A,
        registry_store: RegistryStore,
        observation_store: ObservationStore,
    ) -> Self {
        Self {
            adapter,
            registry_store,
            observation_store,
        }
    }
}

impl<A> TerminalService<A>
where
    A: ZjctlAdapter,
{
    fn next_handle() -> String {
        format!("zh_{}", uuid::Uuid::new_v4().simple())
    }

    fn read_bindings(&self) -> Result<Vec<TerminalBinding>, DomainError> {
        self.registry_store.load()
    }

    fn write_bindings(&self, bindings: &[TerminalBinding]) -> Result<(), DomainError> {
        self.registry_store.save(bindings)
    }

    fn read_observations(&self) -> Result<Vec<TerminalObservation>, DomainError> {
        self.observation_store.load()
    }

    fn write_observations(&self, observations: &[TerminalObservation]) -> Result<(), DomainError> {
        self.observation_store.save(observations)
    }

    fn ensure_available(&self) -> Result<(), DomainError> {
        if self.adapter.is_available() {
            Ok(())
        } else {
            Err(DomainError::new(
                ErrorCode::ZjctlUnavailable,
                "zjctl is not available on PATH",
                true,
            ))
        }
    }

    fn hash_content(content: &str) -> String {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    fn suffix_after_baseline(content: &str, baseline: Option<&str>) -> String {
        match baseline {
            Some(baseline) if content.starts_with(baseline) => {
                content[baseline.len()..].to_string()
            }
            _ => content.to_string(),
        }
    }

    fn map_adapter_error(&self, error: AdapterError, code: ErrorCode) -> DomainError {
        match error {
            AdapterError::ZjctlUnavailable => {
                DomainError::new(ErrorCode::ZjctlUnavailable, error.to_string(), true)
            }
            AdapterError::Timeout => {
                DomainError::new(ErrorCode::WaitTimeout, error.to_string(), true)
            }
            AdapterError::CommandFailed(message) if message.contains("multiple panes") => {
                DomainError::new(ErrorCode::SelectorNotUnique, message, false)
            }
            AdapterError::CommandFailed(message) if message.contains("no pane matched") => {
                DomainError::new(ErrorCode::TargetNotFound, message, false)
            }
            other => DomainError::new(code, other.to_string(), false),
        }
    }

    fn inactive_binding_error(handle: &str) -> DomainError {
        DomainError::new(
            ErrorCode::TargetStale,
            format!("handle `{handle}` is not active"),
            false,
        )
    }

    fn ensure_binding_active(binding: &TerminalBinding) -> Result<(), DomainError> {
        match binding.status {
            TerminalStatus::Closed | TerminalStatus::Stale => {
                Err(Self::inactive_binding_error(&binding.handle))
            }
            TerminalStatus::Ready | TerminalStatus::Busy => Ok(()),
        }
    }

    fn is_missing_target_error(error: &AdapterError) -> bool {
        matches!(error, AdapterError::CommandFailed(message) if message.contains("no pane matched") || message.contains("no panes match selector"))
    }

    fn mark_binding_stale(&self, handle: &str) -> Result<(), DomainError> {
        let mut bindings = self.read_bindings()?;
        if let Some(binding) = bindings.iter_mut().find(|binding| binding.handle == handle) {
            binding.status = TerminalStatus::Stale;
            binding.updated_at = Utc::now();
            self.write_bindings(&bindings)?;
        }

        Ok(())
    }

    fn set_binding_status(&self, handle: &str, status: TerminalStatus) -> Result<(), DomainError> {
        let mut bindings = self.read_bindings()?;
        if let Some(binding) = bindings.iter_mut().find(|binding| binding.handle == handle) {
            binding.status = status;
            binding.updated_at = Utc::now();
            self.write_bindings(&bindings)?;
        }

        Ok(())
    }

    fn build_send_payload(request: &SendRequest) -> Result<(String, bool), DomainError> {
        let mut payload = request.text.clone();

        for key in &request.keys {
            payload.push_str(&map_key_sequence(key)?);
        }

        if payload.is_empty() {
            return Err(DomainError::new(
                ErrorCode::InvalidArgument,
                "send requires non-empty text or at least one key".to_string(),
                false,
            ));
        }

        let submit = request.submit && request.keys.is_empty();
        if request.submit && !request.keys.is_empty() {
            payload.push('\n');
        }

        Ok((payload, submit))
    }

    fn revalidate_binding(&self, binding: &TerminalBinding) -> Result<TerminalStatus, DomainError> {
        if matches!(binding.status, TerminalStatus::Closed) {
            return Ok(TerminalStatus::Closed);
        }

        let request = AttachRequest {
            session_name: binding.session_name.clone(),
            tab_name: binding.tab_name.clone(),
            selector: binding.selector.clone(),
            alias: None,
        };

        match self.adapter.resolve_selector(&request) {
            Ok(_) => Ok(TerminalStatus::Ready),
            Err(error) if Self::is_missing_target_error(&error) => Ok(TerminalStatus::Stale),
            Err(error) => Err(self.map_adapter_error(error, ErrorCode::TargetNotFound)),
        }
    }

    fn ensure_handle_revalidated(&self, handle: &str) -> Result<TerminalBinding, DomainError> {
        let bindings = self.read_bindings()?;
        let binding = bindings
            .iter()
            .find(|binding| binding.handle == handle)
            .cloned()
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("handle `{handle}` is not registered"),
                    false,
                )
            })?;

        let status = self.revalidate_binding(&binding)?;
        if status != binding.status {
            self.set_binding_status(handle, status)?;
        }

        let refreshed = self.read_bindings()?;
        refreshed
            .into_iter()
            .find(|binding| binding.handle == handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("handle `{handle}` is not registered"),
                    false,
                )
            })
    }

    pub fn revalidate_all(&self) -> Result<(), DomainError> {
        if !self.adapter.is_available() {
            return Ok(());
        }

        let bindings = self.read_bindings()?;
        for binding in bindings {
            let status = self.revalidate_binding(&binding)?;
            if status != binding.status {
                self.set_binding_status(&binding.handle, status)?;
            }
        }

        Ok(())
    }
}

impl<A> TerminalManager for TerminalService<A>
where
    A: ZjctlAdapter + Send + Sync,
{
    fn spawn(&self, request: SpawnRequest) -> Result<SpawnResponse, DomainError> {
        self.ensure_available()?;

        let resolved = self
            .adapter
            .spawn(&request)
            .map_err(|error| self.map_adapter_error(error, ErrorCode::SpawnFailed))?;

        if request.wait_ready {
            self.adapter
                .wait_idle(&resolved.session_name, &resolved.selector, 1200, 30_000)
                .map_err(|error| self.map_adapter_error(error, ErrorCode::WaitFailed))?;
        }

        let snapshot = self
            .adapter
            .capture_full(&resolved.session_name, &resolved.selector)
            .map_err(|error| self.map_adapter_error(error, ErrorCode::CaptureFailed))?;

        let handle = Self::next_handle();
        let now = snapshot.captured_at;
        let mut bindings = self.read_bindings()?;
        bindings.push(TerminalBinding {
            handle: handle.clone(),
            alias: request.title.clone(),
            session_name: resolved.session_name.clone(),
            tab_name: resolved.tab_name.clone(),
            selector: resolved.selector.clone(),
            pane_id: resolved.pane_id.clone(),
            cwd: request.cwd.clone(),
            launch_command: Some(request.command.clone()),
            source: BindingSource::Spawned,
            status: TerminalStatus::Ready,
            created_at: now,
            updated_at: now,
        });
        self.write_bindings(&bindings)?;

        let hash = Self::hash_content(&snapshot.content);
        let mut observations = self.read_observations()?;
        let mut observation = TerminalObservation {
            handle: handle.clone(),
            ..TerminalObservation::default()
        };
        observation.update_full_snapshot(snapshot.content, hash, now);
        observation.reset_command_boundary();
        observations.retain(|item| item.handle != handle);
        observations.push(observation);
        self.write_observations(&observations)?;

        Ok(SpawnResponse {
            handle,
            session_name: resolved.session_name,
            tab_name: resolved.tab_name,
            selector: resolved.selector,
            status: "ready".to_string(),
        })
    }

    fn attach(&self, request: AttachRequest) -> Result<AttachResponse, DomainError> {
        self.ensure_available()?;

        let resolved = self
            .adapter
            .resolve_selector(&request)
            .map_err(|error| self.map_adapter_error(error, ErrorCode::AttachFailed))?;
        let snapshot = self
            .adapter
            .capture_full(&resolved.session_name, &resolved.selector)
            .map_err(|error| self.map_adapter_error(error, ErrorCode::CaptureFailed))?;

        let handle = Self::next_handle();
        let now = snapshot.captured_at;
        let mut bindings = self.read_bindings()?;
        bindings.push(TerminalBinding {
            handle: handle.clone(),
            alias: request.alias,
            session_name: resolved.session_name,
            tab_name: resolved.tab_name,
            selector: resolved.selector,
            pane_id: resolved.pane_id,
            cwd: None,
            launch_command: None,
            source: BindingSource::Attached,
            status: TerminalStatus::Ready,
            created_at: now,
            updated_at: now,
        });
        self.write_bindings(&bindings)?;

        let hash = Self::hash_content(&snapshot.content);
        let mut observations = self.read_observations()?;
        let mut observation = TerminalObservation {
            handle: handle.clone(),
            ..TerminalObservation::default()
        };
        observation.update_full_snapshot(snapshot.content, hash, now);
        observation.reset_command_boundary();
        observations.retain(|item| item.handle != handle);
        observations.push(observation);
        self.write_observations(&observations)?;

        Ok(AttachResponse {
            handle,
            attached: true,
            baseline_established: true,
        })
    }

    fn list(&self, request: ListRequest) -> Result<ListResponse, DomainError> {
        self.revalidate_all()?;
        let mut bindings = self.read_bindings()?;
        if let Some(session_name) = request.session_name {
            bindings.retain(|binding| binding.session_name == session_name);
        }

        Ok(ListResponse { bindings })
    }

    fn capture(&self, request: CaptureRequest) -> Result<CaptureResponse, DomainError> {
        self.ensure_available()?;

        let mut bindings = self.read_bindings()?;
        let active = self.ensure_handle_revalidated(&request.handle)?;
        let binding = bindings
            .iter_mut()
            .find(|binding| binding.handle == request.handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("handle `{}` is not registered", request.handle),
                    false,
                )
            })?;
        binding.status = active.status;
        binding.updated_at = active.updated_at;

        Self::ensure_binding_active(&binding)?;

        let snapshot = self
            .adapter
            .capture_full(&binding.session_name, &binding.selector)
            .map_err(|error| {
                if Self::is_missing_target_error(&error) {
                    let _ = self.mark_binding_stale(&request.handle);
                    Self::inactive_binding_error(&request.handle)
                } else {
                    self.map_adapter_error(error, ErrorCode::CaptureFailed)
                }
            })?;

        let mut observations = self.read_observations()?;
        let index = observations
            .iter()
            .position(|item| item.handle == request.handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("observation for `{}` is missing", request.handle),
                    false,
                )
            })?;

        let observation = &mut observations[index];
        let previous_full = observation.last_full_content.clone();
        let boundary_full = observation.command_boundary_content.clone();
        let content = match request.mode {
            CaptureMode::Full => snapshot.content.clone(),
            CaptureMode::Delta => {
                Self::suffix_after_baseline(&snapshot.content, previous_full.as_deref())
            }
            CaptureMode::Current => {
                Self::suffix_after_baseline(&snapshot.content, boundary_full.as_deref())
            }
        };
        let baseline = match request.mode {
            CaptureMode::Full => None,
            CaptureMode::Delta => Some("last_capture".to_string()),
            CaptureMode::Current => Some("command_boundary".to_string()),
        };

        let hash = Self::hash_content(&snapshot.content);
        observation.update_full_snapshot(snapshot.content, hash, snapshot.captured_at);
        binding.updated_at = snapshot.captured_at;
        self.write_bindings(&bindings)?;
        self.write_observations(&observations)?;

        Ok(CaptureResponse {
            capture: CaptureResult {
                handle: request.handle,
                mode: match request.mode {
                    CaptureMode::Full => "full".to_string(),
                    CaptureMode::Delta => "delta".to_string(),
                    CaptureMode::Current => "current".to_string(),
                },
                content,
                truncated: snapshot.truncated,
                captured_at: snapshot.captured_at,
                baseline,
            },
        })
    }

    fn send(&self, request: SendRequest) -> Result<SendResponse, DomainError> {
        self.ensure_available()?;

        let binding = self.ensure_handle_revalidated(&request.handle)?;
        let (payload, submit) = Self::build_send_payload(&request)?;

        Self::ensure_binding_active(&binding)?;

        self.adapter
            .send_input(&binding.session_name, &binding.selector, &payload, submit)
            .map_err(|error| {
                if Self::is_missing_target_error(&error) {
                    let _ = self.mark_binding_stale(&request.handle);
                    Self::inactive_binding_error(&request.handle)
                } else {
                    self.map_adapter_error(error, ErrorCode::SendFailed)
                }
            })?;

        if request.submit {
            let mut observations = self.read_observations()?;
            let observation = observations
                .iter_mut()
                .find(|item| item.handle == request.handle)
                .ok_or_else(|| {
                    DomainError::new(
                        ErrorCode::HandleNotFound,
                        format!("observation for `{}` is missing", request.handle),
                        false,
                    )
                })?;
            observation.reset_command_boundary();
            self.write_observations(&observations)?;
        }

        Ok(SendResponse {
            handle: request.handle,
            accepted: true,
        })
    }

    fn wait(&self, request: WaitRequest) -> Result<WaitResponse, DomainError> {
        self.ensure_available()?;

        let mut bindings = self.read_bindings()?;
        let active = self.ensure_handle_revalidated(&request.handle)?;
        let binding = bindings
            .iter_mut()
            .find(|binding| binding.handle == request.handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("handle `{}` is not registered", request.handle),
                    false,
                )
            })?;

        binding.status = active.status;
        binding.updated_at = active.updated_at;
        let session_name = binding.session_name.clone();
        let selector = binding.selector.clone();
        Self::ensure_binding_active(binding)?;
        binding.status = TerminalStatus::Busy;
        self.write_bindings(&bindings)?;

        let result = self
            .adapter
            .wait_idle(
                &session_name,
                &selector,
                request.idle_ms,
                request.timeout_ms,
            )
            .map_err(|error| {
                if Self::is_missing_target_error(&error) {
                    let _ = self.mark_binding_stale(&request.handle);
                    Self::inactive_binding_error(&request.handle)
                } else {
                    self.map_adapter_error(error, ErrorCode::WaitFailed)
                }
            });

        let observed_at = Utc::now();
        if !matches!(
            result,
            Err(DomainError {
                code: ErrorCode::TargetStale,
                ..
            })
        ) {
            let mut bindings = self.read_bindings()?;
            if let Some(binding) = bindings
                .iter_mut()
                .find(|binding| binding.handle == request.handle)
            {
                binding.status = TerminalStatus::Ready;
                binding.updated_at = observed_at;
            }
            self.write_bindings(&bindings)?;
        }

        result?;

        Ok(WaitResponse {
            handle: request.handle,
            status: "idle".to_string(),
            observed_at,
        })
    }

    fn close(&self, request: CloseRequest) -> Result<CloseResponse, DomainError> {
        self.ensure_available()?;

        let mut bindings = self.read_bindings()?;
        let active = self.ensure_handle_revalidated(&request.handle)?;
        let index = bindings
            .iter()
            .position(|binding| binding.handle == request.handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("handle `{}` is not registered", request.handle),
                    false,
                )
            })?;

        bindings[index].status = active.status;
        bindings[index].updated_at = active.updated_at;
        let session_name = bindings[index].session_name.clone();
        let selector = bindings[index].selector.clone();
        if bindings[index].status == TerminalStatus::Closed {
            return Ok(CloseResponse {
                handle: request.handle,
                closed: true,
            });
        }
        Self::ensure_binding_active(&bindings[index])?;

        self.adapter
            .close(&session_name, &selector, request.force)
            .map_err(|error| {
                if Self::is_missing_target_error(&error) {
                    let _ = self.mark_binding_stale(&request.handle);
                    Self::inactive_binding_error(&request.handle)
                } else {
                    self.map_adapter_error(error, ErrorCode::CloseFailed)
                }
            })?;

        bindings[index].status = TerminalStatus::Closed;
        bindings[index].updated_at = Utc::now();
        self.write_bindings(&bindings)?;

        let mut observations = self.read_observations()?;
        observations.retain(|item| item.handle != request.handle);
        self.write_observations(&observations)?;

        Ok(CloseResponse {
            handle: request.handle,
            closed: true,
        })
    }
}

fn map_key_sequence(key: &str) -> Result<&'static str, DomainError> {
    match key {
        "enter" => Ok("\n"),
        "tab" => Ok("\t"),
        "escape" | "esc" => Ok("\u{1b}"),
        "up" => Ok("\u{1b}[A"),
        "down" => Ok("\u{1b}[B"),
        "right" => Ok("\u{1b}[C"),
        "left" => Ok("\u{1b}[D"),
        "backspace" => Ok("\u{7f}"),
        "ctrl_c" => Ok("\u{3}"),
        other => Err(DomainError::new(
            ErrorCode::InvalidArgument,
            format!("unsupported special key `{other}`"),
            false,
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::adapters::zjctl::{CaptureSnapshot, ResolvedTarget, ZjctlAdapter};
    use crate::domain::requests::{
        AttachRequest, CloseRequest, ListRequest, SendRequest, SpawnRequest, WaitRequest,
    };
    use crate::domain::status::SpawnTarget;

    use super::*;

    #[derive(Debug, Clone)]
    struct MockAdapter {
        target: ResolvedTarget,
        captures: Vec<String>,
        sent_inputs: Arc<Mutex<Vec<(String, bool)>>>,
        resolve_missing_target: bool,
        send_missing_target: bool,
        wait_missing_target: bool,
        capture_missing_target: bool,
    }

    impl MockAdapter {
        fn single_capture(content: &str) -> Self {
            Self {
                target: ResolvedTarget {
                    selector: "id:terminal:7".to_string(),
                    pane_id: Some("terminal:7".to_string()),
                    session_name: "gpu".to_string(),
                    tab_name: Some("editor".to_string()),
                    title: Some("editor".to_string()),
                },
                captures: vec![content.to_string()],
                sent_inputs: Arc::new(Mutex::new(Vec::new())),
                resolve_missing_target: false,
                send_missing_target: false,
                wait_missing_target: false,
                capture_missing_target: false,
            }
        }
    }

    impl ZjctlAdapter for MockAdapter {
        fn is_available(&self) -> bool {
            true
        }

        fn spawn(&self, _request: &SpawnRequest) -> Result<ResolvedTarget, AdapterError> {
            Ok(self.target.clone())
        }

        fn resolve_selector(
            &self,
            _request: &AttachRequest,
        ) -> Result<ResolvedTarget, AdapterError> {
            if self.resolve_missing_target {
                return Err(AdapterError::CommandFailed(
                    "RPC error: no panes match selector".to_string(),
                ));
            }
            Ok(self.target.clone())
        }

        fn send_input(
            &self,
            _session_name: &str,
            _handle: &str,
            text: &str,
            submit: bool,
        ) -> Result<(), AdapterError> {
            if self.send_missing_target {
                return Err(AdapterError::CommandFailed(
                    "RPC error: no panes match selector".to_string(),
                ));
            }
            self.sent_inputs
                .lock()
                .expect("sent inputs lock should succeed")
                .push((text.to_string(), submit));
            Ok(())
        }

        fn wait_idle(
            &self,
            _session_name: &str,
            _handle: &str,
            _idle_ms: u64,
            _timeout_ms: u64,
        ) -> Result<(), AdapterError> {
            if self.wait_missing_target {
                return Err(AdapterError::CommandFailed(
                    "RPC error: no panes match selector".to_string(),
                ));
            }
            Ok(())
        }

        fn capture_full(
            &self,
            _session_name: &str,
            _handle: &str,
        ) -> Result<CaptureSnapshot, AdapterError> {
            if self.capture_missing_target {
                return Err(AdapterError::CommandFailed(
                    "RPC error: no panes match selector".to_string(),
                ));
            }
            let content = self
                .captures
                .last()
                .cloned()
                .expect("mock capture content should exist");
            Ok(CaptureSnapshot {
                content,
                captured_at: Utc::now(),
                truncated: false,
            })
        }

        fn close(
            &self,
            _session_name: &str,
            _handle: &str,
            _force: bool,
        ) -> Result<(), AdapterError> {
            Ok(())
        }

        fn list_targets(&self) -> Result<Vec<ResolvedTarget>, AdapterError> {
            Ok(vec![self.target.clone()])
        }
    }

    fn make_service(adapter: MockAdapter) -> TerminalService<MockAdapter> {
        let root = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        make_service_with_root(adapter, root)
    }

    fn make_service_with_root(
        adapter: MockAdapter,
        root: std::path::PathBuf,
    ) -> TerminalService<MockAdapter> {
        TerminalService::new(
            adapter,
            RegistryStore::new(root.join("registry.json")),
            ObservationStore::new(root.join("observations.json")),
        )
    }

    #[test]
    fn attach_persists_binding_and_observation() {
        let service = make_service(MockAdapter::single_capture("baseline"));

        let response = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: Some("main-editor".to_string()),
            })
            .expect("attach should succeed");

        let bindings = service.registry_store.load().expect("bindings should load");
        let observations = service
            .observation_store
            .load()
            .expect("observations should load");

        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].handle, response.handle);
        assert_eq!(observations.len(), 1);
        assert_eq!(
            observations[0].command_boundary_content.as_deref(),
            Some("baseline")
        );
    }

    #[test]
    fn list_filters_by_session() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let _ = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let listed = service
            .list(ListRequest {
                session_name: Some("gpu".to_string()),
            })
            .expect("list should succeed");

        assert_eq!(listed.bindings.len(), 1);
    }

    #[test]
    fn send_returns_acknowledged_response() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let response = service
            .send(SendRequest {
                handle: attach.handle,
                text: "printf 'ok'".to_string(),
                keys: vec![],
                submit: true,
            })
            .expect("send should succeed");

        assert!(response.accepted);
    }

    #[test]
    fn send_maps_special_keys_to_control_sequences() {
        let adapter = MockAdapter::single_capture("baseline");
        let sent_inputs = adapter.sent_inputs.clone();
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        service
            .send(SendRequest {
                handle: attach.handle,
                text: String::new(),
                keys: vec!["up".to_string(), "escape".to_string(), "tab".to_string()],
                submit: false,
            })
            .expect("send should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent[0].0, "\u{1b}[A\u{1b}\t");
        assert!(!sent[0].1);
    }

    #[test]
    fn send_rejects_unknown_special_key() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let error = service
            .send(SendRequest {
                handle: attach.handle,
                text: String::new(),
                keys: vec!["hyperjump".to_string()],
                submit: false,
            })
            .expect_err("unknown key should fail");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn send_submit_resets_command_boundary() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        {
            let mut observations = service
                .observation_store
                .load()
                .expect("observations should load");
            observations[0].last_full_content = Some("after-send".to_string());
            observations[0].last_full_hash = Some("hash123".to_string());
            service
                .observation_store
                .save(&observations)
                .expect("observations should save");
        }

        service
            .send(SendRequest {
                handle: attach.handle.clone(),
                text: "run".to_string(),
                keys: vec![],
                submit: true,
            })
            .expect("send should succeed");

        let observations = service
            .observation_store
            .load()
            .expect("observations should load");
        assert_eq!(
            observations[0].command_boundary_content.as_deref(),
            Some("after-send")
        );
    }

    #[test]
    fn spawn_persists_spawned_binding() {
        let service = make_service(MockAdapter::single_capture("ready"));

        let response = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: Some("/tmp".to_string()),
                command: "lazygit".to_string(),
                title: Some("lg".to_string()),
                wait_ready: false,
            })
            .expect("spawn should succeed");

        let bindings = service.registry_store.load().expect("bindings should load");

        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].source, BindingSource::Spawned);
        assert_eq!(response.status, "ready");
    }

    #[test]
    fn wait_returns_idle_status() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let response = service
            .wait(WaitRequest {
                handle: attach.handle,
                idle_ms: 1200,
                timeout_ms: 30_000,
            })
            .expect("wait should succeed");

        assert_eq!(response.status, "idle");
    }

    #[test]
    fn close_marks_binding_closed() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let response = service
            .close(CloseRequest {
                handle: attach.handle.clone(),
                force: true,
            })
            .expect("close should succeed");

        let bindings = service.registry_store.load().expect("bindings should load");
        assert_eq!(response.handle, attach.handle);
        assert_eq!(bindings[0].status, TerminalStatus::Closed);
    }

    #[test]
    fn send_rejects_closed_handle() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");
        service
            .close(CloseRequest {
                handle: attach.handle.clone(),
                force: true,
            })
            .expect("close should succeed");

        let error = service
            .send(SendRequest {
                handle: attach.handle,
                text: "run".to_string(),
                keys: vec![],
                submit: false,
            })
            .expect_err("send should reject closed handle");
        assert_eq!(error.code, ErrorCode::TargetStale);
    }

    #[test]
    fn send_marks_binding_stale_when_target_disappears() {
        let mut adapter = MockAdapter::single_capture("baseline");
        adapter.send_missing_target = true;
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let error = service
            .send(SendRequest {
                handle: attach.handle.clone(),
                text: "run".to_string(),
                keys: vec![],
                submit: false,
            })
            .expect_err("send should fail on missing target");
        assert_eq!(error.code, ErrorCode::TargetStale);

        let bindings = service.registry_store.load().expect("bindings should load");
        assert_eq!(bindings[0].status, TerminalStatus::Stale);
    }

    #[test]
    fn wait_marks_binding_stale_when_target_disappears() {
        let mut adapter = MockAdapter::single_capture("baseline");
        adapter.wait_missing_target = true;
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let error = service
            .wait(WaitRequest {
                handle: attach.handle.clone(),
                idle_ms: 1200,
                timeout_ms: 30_000,
            })
            .expect_err("wait should fail on missing target");
        assert_eq!(error.code, ErrorCode::TargetStale);

        let bindings = service.registry_store.load().expect("bindings should load");
        assert_eq!(bindings[0].status, TerminalStatus::Stale);
    }

    #[test]
    fn revalidate_all_marks_missing_binding_stale() {
        let root = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        let service = make_service_with_root(MockAdapter::single_capture("baseline"), root.clone());
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let mut missing_adapter = MockAdapter::single_capture("baseline");
        missing_adapter.resolve_missing_target = true;
        let service = make_service_with_root(missing_adapter, root);
        service.revalidate_all().expect("revalidate should succeed");

        let bindings = service.registry_store.load().expect("bindings should load");
        assert_eq!(bindings[0].handle, attach.handle);
        assert_eq!(bindings[0].status, TerminalStatus::Stale);
    }

    #[test]
    fn list_revalidates_before_returning_bindings() {
        let root = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        let service = make_service_with_root(MockAdapter::single_capture("baseline"), root.clone());
        let _ = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let mut missing_adapter = MockAdapter::single_capture("baseline");
        missing_adapter.resolve_missing_target = true;
        let service = make_service_with_root(missing_adapter, root);
        let listed = service
            .list(ListRequest {
                session_name: Some("gpu".to_string()),
            })
            .expect("list should succeed");

        assert_eq!(listed.bindings[0].status, TerminalStatus::Stale);
    }
}
