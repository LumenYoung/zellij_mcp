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
        let mut bindings = self.read_bindings()?;
        if let Some(session_name) = request.session_name {
            bindings.retain(|binding| binding.session_name == session_name);
        }

        Ok(ListResponse { bindings })
    }

    fn capture(&self, request: CaptureRequest) -> Result<CaptureResponse, DomainError> {
        self.ensure_available()?;

        let mut bindings = self.read_bindings()?;
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

        if binding.status == TerminalStatus::Stale || binding.status == TerminalStatus::Closed {
            return Err(DomainError::new(
                ErrorCode::TargetStale,
                format!("handle `{}` is not active", request.handle),
                false,
            ));
        }

        let snapshot = self
            .adapter
            .capture_full(&binding.session_name, &binding.selector)
            .map_err(|error| self.map_adapter_error(error, ErrorCode::CaptureFailed))?;

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

        let bindings = self.read_bindings()?;
        let binding = bindings
            .iter()
            .find(|binding| binding.handle == request.handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("handle `{}` is not registered", request.handle),
                    false,
                )
            })?;

        self.adapter
            .send_input(
                &binding.session_name,
                &binding.selector,
                &request.text,
                request.submit,
            )
            .map_err(|error| self.map_adapter_error(error, ErrorCode::SendFailed))?;

        Ok(SendResponse {
            handle: request.handle,
            accepted: true,
        })
    }

    fn wait(&self, request: WaitRequest) -> Result<WaitResponse, DomainError> {
        self.ensure_available()?;

        let mut bindings = self.read_bindings()?;
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

        let session_name = binding.session_name.clone();
        let selector = binding.selector.clone();
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
            .map_err(|error| self.map_adapter_error(error, ErrorCode::WaitFailed));

        let observed_at = Utc::now();
        let mut bindings = self.read_bindings()?;
        if let Some(binding) = bindings
            .iter_mut()
            .find(|binding| binding.handle == request.handle)
        {
            binding.status = TerminalStatus::Ready;
            binding.updated_at = observed_at;
        }
        self.write_bindings(&bindings)?;

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

        let session_name = bindings[index].session_name.clone();
        let selector = bindings[index].selector.clone();

        self.adapter
            .close(&session_name, &selector, request.force)
            .map_err(|error| self.map_adapter_error(error, ErrorCode::CloseFailed))?;

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

#[cfg(test)]
mod tests {
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
            Ok(self.target.clone())
        }

        fn send_input(
            &self,
            _session_name: &str,
            _handle: &str,
            _text: &str,
            _submit: bool,
        ) -> Result<(), AdapterError> {
            Ok(())
        }

        fn wait_idle(
            &self,
            _session_name: &str,
            _handle: &str,
            _idle_ms: u64,
            _timeout_ms: u64,
        ) -> Result<(), AdapterError> {
            Ok(())
        }

        fn capture_full(
            &self,
            _session_name: &str,
            _handle: &str,
        ) -> Result<CaptureSnapshot, AdapterError> {
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
                submit: true,
            })
            .expect("send should succeed");

        assert!(response.accepted);
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
}
