use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::adapters::zjctl::{AdapterError, ZjctlAdapter};
use crate::domain::binding::TerminalBinding;
use crate::domain::errors::{DomainError, ErrorCode};
use crate::domain::observation::{CaptureResult, TerminalObservation};
use chrono::Utc;

use crate::domain::requests::{
    AttachRequest, CaptureRequest, CloseRequest, DiscoverRequest, ListRequest, SendRequest,
    SpawnRequest, WaitRequest,
};
use crate::domain::responses::{
    AttachResponse, CaptureResponse, CloseResponse, DiscoverCandidate, DiscoverResponse,
    ListResponse, SendResponse, SpawnResponse, WaitResponse,
};
use crate::domain::status::{BindingSource, CaptureMode, TerminalStatus};
use crate::persistence::{ObservationStore, RegistryStore};

pub trait TerminalManager: Send + Sync {
    fn spawn(&self, request: SpawnRequest) -> Result<SpawnResponse, DomainError>;
    fn attach(&self, request: AttachRequest) -> Result<AttachResponse, DomainError>;
    fn discover(&self, request: DiscoverRequest) -> Result<DiscoverResponse, DomainError>;
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

    fn is_repaint_heavy(content: &str) -> bool {
        let mut chars = content.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\r' {
                return true;
            }

            if ch != '\u{1b}' || chars.peek() != Some(&'[') {
                continue;
            }

            chars.next();
            let mut sequence = String::new();
            while let Some(&next) = chars.peek() {
                sequence.push(next);
                chars.next();
                if ('@'..='~').contains(&next) {
                    break;
                }
            }

            if Self::is_full_frame_reset_sequence(&sequence) {
                return true;
            }
        }

        false
    }

    fn normalize_current_frame(content: &str) -> String {
        let mut output = String::new();
        let mut chars = content.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    let mut sequence = String::new();
                    while let Some(&next) = chars.peek() {
                        sequence.push(next);
                        chars.next();
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }

                    if Self::is_full_frame_reset_sequence(&sequence) {
                        output.clear();
                    }
                }
                continue;
            }

            if ch == '\r' {
                if let Some(line_start) = output.rfind('\n') {
                    output.truncate(line_start + 1);
                } else {
                    output.clear();
                }
                continue;
            }

            output.push(ch);
        }

        output
    }

    fn is_full_frame_reset_sequence(sequence: &str) -> bool {
        matches!(sequence, "H" | "2J" | "H2J" | "2JH")
    }

    fn map_adapter_error(&self, error: AdapterError, code: ErrorCode) -> DomainError {
        match error {
            AdapterError::ZjctlUnavailable => {
                DomainError::new(ErrorCode::ZjctlUnavailable, error.to_string(), true)
            }
            AdapterError::ParseError(message) => {
                DomainError::new(ErrorCode::InvalidArgument, message, false)
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

    fn build_revalidation_requests(binding: &TerminalBinding) -> Vec<AttachRequest> {
        let mut requests = Vec::new();

        if let Some(pane_id) = binding.pane_id.as_deref() {
            requests.push(AttachRequest {
                session_name: binding.session_name.clone(),
                tab_name: None,
                selector: format!("id:{pane_id}"),
                alias: None,
            });
        }

        if requests
            .iter()
            .all(|request| request.selector != binding.selector)
        {
            requests.push(AttachRequest {
                session_name: binding.session_name.clone(),
                tab_name: binding.tab_name.clone(),
                selector: binding.selector.clone(),
                alias: None,
            });
        }

        requests
    }

    fn should_clear_attached_input(binding: &TerminalBinding, request: &SendRequest) -> bool {
        binding.source == BindingSource::Attached
            && request.submit
            && request.keys.is_empty()
            && !request.text.is_empty()
    }

    fn prepare_attached_send_boundary(
        &self,
        binding: &TerminalBinding,
        handle: &str,
    ) -> Result<(), DomainError> {
        let snapshot = self
            .adapter
            .capture_full(&binding.session_name, &binding.selector)
            .map_err(|error| {
                if Self::is_missing_target_error(&error) {
                    let _ = self.mark_binding_stale(handle);
                    Self::inactive_binding_error(handle)
                } else {
                    self.map_adapter_error(error, ErrorCode::CaptureFailed)
                }
            })?;

        let mut observations = self.read_observations()?;
        let observation = observations
            .iter_mut()
            .find(|item| item.handle == handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("observation for `{handle}` is missing"),
                    false,
                )
            })?;

        let hash = Self::hash_content(&snapshot.content);
        observation.update_full_snapshot(snapshot.content, hash, snapshot.captured_at);
        observation.reset_command_boundary();
        self.write_observations(&observations)?;

        Ok(())
    }

    fn refresh_binding_target(&self, handle: &str) -> Result<TerminalBinding, DomainError> {
        let mut bindings = self.read_bindings()?;
        let index = bindings
            .iter()
            .position(|binding| binding.handle == handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("handle `{handle}` is not registered"),
                    false,
                )
            })?;

        if matches!(bindings[index].status, TerminalStatus::Closed) {
            return Ok(bindings[index].clone());
        }

        for attempt in 0..3 {
            for request in Self::build_revalidation_requests(&bindings[index]) {
                match self.adapter.resolve_selector(&request) {
                    Ok(resolved) => {
                        bindings[index].selector = resolved.selector;
                        bindings[index].pane_id = resolved.pane_id;
                        bindings[index].tab_name = resolved.tab_name;
                        bindings[index].status = TerminalStatus::Ready;
                        bindings[index].updated_at = Utc::now();
                        self.write_bindings(&bindings)?;
                        return Ok(bindings[index].clone());
                    }
                    Err(error) if Self::is_missing_target_error(&error) => continue,
                    Err(error) => {
                        return Err(self.map_adapter_error(error, ErrorCode::TargetNotFound));
                    }
                }
            }

            if attempt < 2 {
                std::thread::sleep(Duration::from_millis(150));
            }
        }

        bindings[index].status = TerminalStatus::Stale;
        bindings[index].updated_at = Utc::now();
        self.write_bindings(&bindings)?;
        Ok(bindings[index].clone())
    }

    fn run_binding_operation_with_retry<T, F>(
        &self,
        handle: &str,
        binding: &TerminalBinding,
        error_code: ErrorCode,
        operation: F,
    ) -> Result<T, DomainError>
    where
        F: Fn(&TerminalBinding) -> Result<T, AdapterError>,
    {
        let mut current = binding.clone();
        for attempt in 0..3 {
            match operation(&current) {
                Ok(result) => return Ok(result),
                Err(error) if Self::is_missing_target_error(&error) => {
                    current = self.refresh_binding_target(handle)?;
                    Self::ensure_binding_active(&current)?;
                    if attempt < 2 {
                        std::thread::sleep(Duration::from_millis(150));
                        continue;
                    }
                    let _ = self.mark_binding_stale(handle);
                    return Err(Self::inactive_binding_error(handle));
                }
                Err(error) => return Err(self.map_adapter_error(error, error_code)),
            }
        }

        unreachable!("retry loop should always return")
    }

    fn wait_via_capture_polling(
        &self,
        handle: &str,
        binding: &TerminalBinding,
        idle_ms: u64,
        timeout_ms: u64,
    ) -> Result<(), DomainError> {
        let start = Instant::now();
        let mut last_hash: Option<String> = None;
        let mut last_change = Instant::now();
        let poll_interval = Duration::from_millis(idle_ms.clamp(50, 250));

        while start.elapsed() < Duration::from_millis(timeout_ms) {
            let snapshot = self.run_binding_operation_with_retry(
                handle,
                binding,
                ErrorCode::CaptureFailed,
                |binding| {
                    self.adapter
                        .capture_full(&binding.session_name, &binding.selector)
                },
            )?;
            let hash = Self::hash_content(&snapshot.content);

            if last_hash.as_deref() == Some(hash.as_str()) {
                if last_change.elapsed() >= Duration::from_millis(idle_ms) {
                    return Ok(());
                }
            } else {
                last_hash = Some(hash);
                last_change = Instant::now();
            }

            std::thread::sleep(poll_interval);
        }

        Err(DomainError::new(
            ErrorCode::WaitTimeout,
            format!("pane `{handle}` did not become idle before timeout"),
            true,
        ))
    }

    fn launch_command_summary(request: &SpawnRequest) -> Option<String> {
        request
            .command
            .clone()
            .or_else(|| request.argv.as_ref().map(|argv| argv.join(" ")))
    }

    fn validate_positive_lines(
        value: Option<usize>,
        field_name: &str,
    ) -> Result<Option<usize>, DomainError> {
        match value {
            Some(0) => Err(DomainError::new(
                ErrorCode::InvalidArgument,
                format!("`{field_name}` must be greater than zero"),
                false,
            )),
            other => Ok(other),
        }
    }

    fn tail_lines(content: &str, count: usize) -> (String, bool) {
        let trailing_newline = content.ends_with('\n');
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() <= count {
            return (content.to_string(), false);
        }

        let mut clipped = lines[lines.len() - count..].join("\n");
        if trailing_newline {
            clipped.push('\n');
        }

        (clipped, true)
    }

    fn preview_for_snapshot(content: &str, preview_lines: usize) -> (String, String) {
        if Self::is_repaint_heavy(content) {
            let normalized = Self::normalize_current_frame(content);
            let (preview, _) = Self::tail_lines(&normalized, preview_lines);
            (preview, "visible_frame".to_string())
        } else {
            let (preview, _) = Self::tail_lines(content, preview_lines);
            (preview, "recent_lines".to_string())
        }
    }

    fn discover_matches_selector(
        selector: &str,
        target: &crate::adapters::zjctl::ResolvedTarget,
    ) -> bool {
        if selector == target.selector {
            return true;
        }

        if let Some(stripped) = selector.strip_prefix("id:") {
            return target.pane_id.as_deref() == Some(stripped);
        }

        if selector.starts_with("terminal:") || selector.starts_with("plugin:") {
            return target.pane_id.as_deref() == Some(selector);
        }

        if let Some(stripped) = selector.strip_prefix("title:") {
            return target
                .title
                .as_deref()
                .is_some_and(|title| title.contains(stripped));
        }

        false
    }

    fn ensure_handle_revalidated(&self, handle: &str) -> Result<TerminalBinding, DomainError> {
        self.refresh_binding_target(handle)
    }

    fn persist_spawn_state(
        &self,
        binding: TerminalBinding,
        observation: TerminalObservation,
    ) -> Result<(), DomainError> {
        let mut bindings = self.read_bindings()?;
        bindings.retain(|item| item.handle != binding.handle);
        bindings.push(binding);
        self.write_bindings(&bindings)?;

        let mut observations = self.read_observations()?;
        observations.retain(|item| item.handle != observation.handle);
        observations.push(observation);
        self.write_observations(&observations)
    }

    fn remove_persisted_handle(&self, handle: &str) -> Result<(), DomainError> {
        let mut bindings = self.read_bindings()?;
        bindings.retain(|item| item.handle != handle);
        self.write_bindings(&bindings)?;

        let mut observations = self.read_observations()?;
        observations.retain(|item| item.handle != handle);
        self.write_observations(&observations)
    }

    fn update_spawn_status(
        &self,
        handle: &str,
        status: TerminalStatus,
        updated_at: chrono::DateTime<Utc>,
    ) -> Result<TerminalBinding, DomainError> {
        let mut bindings = self.read_bindings()?;
        let binding = bindings
            .iter_mut()
            .find(|binding| binding.handle == handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("handle `{handle}` is not registered"),
                    false,
                )
            })?;
        binding.status = status;
        binding.updated_at = updated_at;
        let binding = binding.clone();
        self.write_bindings(&bindings)?;
        Ok(binding)
    }

    fn update_spawn_observation(
        &self,
        handle: &str,
        snapshot: crate::adapters::zjctl::CaptureSnapshot,
    ) -> Result<(), DomainError> {
        let hash = Self::hash_content(&snapshot.content);
        let mut observations = self.read_observations()?;
        let observation = observations
            .iter_mut()
            .find(|item| item.handle == handle)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::HandleNotFound,
                    format!("observation for `{handle}` is missing"),
                    false,
                )
            })?;
        observation.update_full_snapshot(snapshot.content, hash, snapshot.captured_at);
        observation.reset_command_boundary();
        self.write_observations(&observations)
    }

    pub fn revalidate_all(&self) -> Result<(), DomainError> {
        if !self.adapter.is_available() {
            return Ok(());
        }

        let bindings = self.read_bindings()?;
        for binding in bindings {
            let _ = self.refresh_binding_target(&binding.handle)?;
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

        let handle = Self::next_handle();
        let now = Utc::now();
        let binding = TerminalBinding {
            handle: handle.clone(),
            alias: request.title.clone(),
            session_name: resolved.session_name.clone(),
            tab_name: resolved.tab_name.clone(),
            selector: resolved.selector.clone(),
            pane_id: resolved.pane_id.clone(),
            cwd: request.cwd.clone(),
            launch_command: Self::launch_command_summary(&request),
            source: BindingSource::Spawned,
            status: TerminalStatus::Busy,
            created_at: now,
            updated_at: now,
        };
        let observation = TerminalObservation {
            handle: handle.clone(),
            ..TerminalObservation::default()
        };
        self.persist_spawn_state(binding.clone(), observation)?;

        if request.wait_ready {
            let wait_result = self.run_binding_operation_with_retry(
                &handle,
                &binding,
                ErrorCode::WaitFailed,
                |binding| {
                    self.adapter
                        .wait_idle(&binding.session_name, &binding.selector, 1200, 30_000)
                },
            );
            let wait_result = match wait_result {
                Err(DomainError {
                    code: ErrorCode::TargetStale,
                    ..
                }) => self.wait_via_capture_polling(&handle, &binding, 1200, 30_000),
                other => other,
            };

            if let Err(error) = wait_result {
                if error.code == ErrorCode::WaitTimeout {
                    return Ok(SpawnResponse {
                        handle,
                        session_name: resolved.session_name,
                        tab_name: resolved.tab_name,
                        selector: resolved.selector,
                        status: "busy".to_string(),
                    });
                }
                self.remove_persisted_handle(&handle)?;
                return Err(error);
            }
        }

        let capture_binding = match self.ensure_handle_revalidated(&handle) {
            Ok(binding) => binding,
            Err(error) => {
                self.remove_persisted_handle(&handle)?;
                return Err(error);
            }
        };
        let snapshot = match self.run_binding_operation_with_retry(
            &handle,
            &capture_binding,
            ErrorCode::CaptureFailed,
            |binding| {
                self.adapter
                    .capture_full(&binding.session_name, &binding.selector)
            },
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if error.code == ErrorCode::CaptureFailed {
                    self.update_spawn_status(&handle, TerminalStatus::Busy, Utc::now())?;
                    return Ok(SpawnResponse {
                        handle,
                        session_name: resolved.session_name,
                        tab_name: resolved.tab_name,
                        selector: resolved.selector,
                        status: "busy".to_string(),
                    });
                }
                self.remove_persisted_handle(&handle)?;
                return Err(error);
            }
        };
        self.update_spawn_observation(&handle, snapshot.clone())?;
        self.update_spawn_status(&handle, TerminalStatus::Ready, snapshot.captured_at)?;

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

    fn discover(&self, request: DiscoverRequest) -> Result<DiscoverResponse, DomainError> {
        self.ensure_available()?;
        let preview_lines =
            Self::validate_positive_lines(request.preview_lines, "preview_lines")?.unwrap_or(8);

        let mut targets = self
            .adapter
            .list_targets_in_session(&request.session_name)
            .map_err(|error| self.map_adapter_error(error, ErrorCode::AttachFailed))?;

        if let Some(tab_name) = request.tab_name.as_ref() {
            targets.retain(|target| target.tab_name.as_deref() == Some(tab_name.as_str()));
        }

        if let Some(selector) = request.selector.as_deref() {
            let selector = selector.trim();
            targets.retain(|target| Self::discover_matches_selector(selector, target));
        }

        let mut candidates = Vec::with_capacity(targets.len());
        for target in targets {
            let (preview, preview_basis, captured_at) = if request.include_preview {
                let snapshot = self
                    .adapter
                    .capture_full(&target.session_name, &target.selector)
                    .map_err(|error| self.map_adapter_error(error, ErrorCode::CaptureFailed))?;
                let (preview, basis) = Self::preview_for_snapshot(&snapshot.content, preview_lines);
                (Some(preview), Some(basis), Some(snapshot.captured_at))
            } else {
                (None, None, None)
            };

            candidates.push(DiscoverCandidate {
                selector: target.selector,
                pane_id: target.pane_id,
                session_name: target.session_name,
                tab_name: target.tab_name,
                title: target.title,
                command: target.command,
                focused: target.focused,
                preview,
                preview_basis,
                captured_at,
            });
        }

        Ok(DiscoverResponse { candidates })
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
        let tail_lines = Self::validate_positive_lines(request.tail_lines, "tail_lines")?;

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

        let snapshot = self.run_binding_operation_with_retry(
            &request.handle,
            binding,
            ErrorCode::CaptureFailed,
            |binding| {
                self.adapter
                    .capture_full(&binding.session_name, &binding.selector)
            },
        )?;

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
                if Self::is_repaint_heavy(&snapshot.content) {
                    Self::normalize_current_frame(&snapshot.content)
                } else {
                    Self::suffix_after_baseline(&snapshot.content, boundary_full.as_deref())
                }
            }
        };
        let baseline = match request.mode {
            CaptureMode::Full => None,
            CaptureMode::Delta => Some("last_capture".to_string()),
            CaptureMode::Current => Some("command_boundary".to_string()),
        };
        let (content, line_window_applied) = match tail_lines {
            Some(count) => Self::tail_lines(&content, count),
            None => (content, false),
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
                tail_lines,
                line_window_applied,
                truncated: snapshot.truncated,
                captured_at: snapshot.captured_at,
                baseline,
            },
        })
    }

    fn send(&self, request: SendRequest) -> Result<SendResponse, DomainError> {
        self.ensure_available()?;

        let binding = self.ensure_handle_revalidated(&request.handle)?;
        let clear_attached_input = Self::should_clear_attached_input(&binding, &request);
        if clear_attached_input {
            self.prepare_attached_send_boundary(&binding, &request.handle)?;
        }

        let (mut payload, submit) = Self::build_send_payload(&request)?;
        if clear_attached_input {
            payload.insert(0, '\u{15}');
        }

        Self::ensure_binding_active(&binding)?;

        self.run_binding_operation_with_retry(
            &request.handle,
            &binding,
            ErrorCode::SendFailed,
            |binding| {
                self.adapter
                    .send_input(&binding.session_name, &binding.selector, &payload, submit)
            },
        )?;

        if request.submit && !clear_attached_input {
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
        Self::ensure_binding_active(binding)?;
        binding.status = TerminalStatus::Busy;
        let busy_binding = binding.clone();
        self.write_bindings(&bindings)?;

        let result = self.run_binding_operation_with_retry(
            &request.handle,
            &busy_binding,
            ErrorCode::WaitFailed,
            |binding| {
                self.adapter.wait_idle(
                    &binding.session_name,
                    &binding.selector,
                    request.idle_ms,
                    request.timeout_ms,
                )
            },
        );
        let result = match result {
            Err(DomainError {
                code: ErrorCode::TargetStale,
                ..
            }) => self.wait_via_capture_polling(
                &request.handle,
                &busy_binding,
                request.idle_ms,
                request.timeout_ms,
            ),
            other => other,
        };

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
        if bindings[index].status == TerminalStatus::Closed {
            return Ok(CloseResponse {
                handle: request.handle,
                closed: true,
            });
        }
        Self::ensure_binding_active(&bindings[index])?;

        self.run_binding_operation_with_retry(
            &request.handle,
            &bindings[index],
            ErrorCode::CloseFailed,
            |binding| {
                self.adapter
                    .close(&binding.session_name, &binding.selector, request.force)
            },
        )?;

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
        list_targets: Vec<ResolvedTarget>,
        captures: Vec<String>,
        capture_index: Arc<Mutex<usize>>,
        sent_inputs: Arc<Mutex<Vec<(String, bool)>>>,
        resolve_failures_remaining: Arc<Mutex<usize>>,
        resolve_fails: bool,
        send_failures_remaining: Arc<Mutex<usize>>,
        wait_failures_remaining: Arc<Mutex<usize>>,
        capture_failures_remaining: Arc<Mutex<usize>>,
        resolve_missing_target: bool,
        send_missing_target: bool,
        wait_missing_target: bool,
        wait_times_out: bool,
        wait_fails: bool,
        capture_missing_target: bool,
        capture_fails: bool,
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
                    command: Some("fish".to_string()),
                    focused: false,
                },
                list_targets: vec![ResolvedTarget {
                    selector: "id:terminal:7".to_string(),
                    pane_id: Some("terminal:7".to_string()),
                    session_name: "gpu".to_string(),
                    tab_name: Some("editor".to_string()),
                    title: Some("editor".to_string()),
                    command: Some("fish".to_string()),
                    focused: false,
                }],
                captures: vec![content.to_string()],
                capture_index: Arc::new(Mutex::new(0)),
                sent_inputs: Arc::new(Mutex::new(Vec::new())),
                resolve_failures_remaining: Arc::new(Mutex::new(0)),
                resolve_fails: false,
                send_failures_remaining: Arc::new(Mutex::new(0)),
                wait_failures_remaining: Arc::new(Mutex::new(0)),
                capture_failures_remaining: Arc::new(Mutex::new(0)),
                resolve_missing_target: false,
                send_missing_target: false,
                wait_missing_target: false,
                wait_times_out: false,
                wait_fails: false,
                capture_missing_target: false,
                capture_fails: false,
            }
        }

        fn capture_sequence(contents: &[&str]) -> Self {
            let mut adapter = Self::single_capture(
                contents
                    .first()
                    .copied()
                    .expect("mock capture sequence should not be empty"),
            );
            adapter.captures = contents.iter().map(|item| (*item).to_string()).collect();
            adapter
        }

        fn with_targets_and_captures(targets: Vec<ResolvedTarget>, captures: Vec<&str>) -> Self {
            let mut adapter = Self::single_capture(
                captures
                    .first()
                    .copied()
                    .expect("mock captures should not be empty"),
            );
            adapter.list_targets = targets;
            adapter.captures = captures.into_iter().map(ToString::to_string).collect();
            adapter
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
            request: &AttachRequest,
        ) -> Result<ResolvedTarget, AdapterError> {
            if self.resolve_missing_target {
                return Err(AdapterError::CommandFailed(
                    "RPC error: no panes match selector".to_string(),
                ));
            }

            if self.resolve_fails {
                return Err(AdapterError::CommandFailed(
                    "resolve backend failed".to_string(),
                ));
            }

            let mut failures_remaining = self
                .resolve_failures_remaining
                .lock()
                .expect("resolve failures lock should succeed");
            if *failures_remaining > 0 {
                *failures_remaining -= 1;
                return Err(AdapterError::CommandFailed(
                    "RPC error: no panes match selector".to_string(),
                ));
            }

            self.list_targets
                .iter()
                .find(|target| {
                    target.session_name == request.session_name
                        && request.tab_name.as_ref().is_none_or(|tab_name| {
                            target.tab_name.as_deref() == Some(tab_name.as_str())
                        })
                        && mock_matches_selector(&request.selector, target)
                })
                .cloned()
                .ok_or_else(|| {
                    AdapterError::CommandFailed("RPC error: no panes match selector".to_string())
                })
        }

        fn list_targets_in_session(
            &self,
            session_name: &str,
        ) -> Result<Vec<ResolvedTarget>, AdapterError> {
            Ok(self
                .list_targets
                .iter()
                .filter(|target| target.session_name == session_name)
                .cloned()
                .collect())
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

            let mut failures_remaining = self
                .send_failures_remaining
                .lock()
                .expect("send failures lock should succeed");
            if *failures_remaining > 0 {
                *failures_remaining -= 1;
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

            if self.wait_times_out {
                return Err(AdapterError::Timeout);
            }

            if self.wait_fails {
                return Err(AdapterError::CommandFailed(
                    "wait backend failed".to_string(),
                ));
            }

            let mut failures_remaining = self
                .wait_failures_remaining
                .lock()
                .expect("wait failures lock should succeed");
            if *failures_remaining > 0 {
                *failures_remaining -= 1;
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

            if self.capture_fails {
                return Err(AdapterError::CommandFailed(
                    "capture backend failed".to_string(),
                ));
            }

            let mut failures_remaining = self
                .capture_failures_remaining
                .lock()
                .expect("capture failures lock should succeed");
            if *failures_remaining > 0 {
                *failures_remaining -= 1;
                return Err(AdapterError::CommandFailed(
                    "RPC error: no panes match selector".to_string(),
                ));
            }
            let content = self
                .captures
                .get({
                    let mut index = self
                        .capture_index
                        .lock()
                        .expect("capture index lock should succeed");
                    let current = (*index).min(self.captures.len().saturating_sub(1));
                    *index = index.saturating_add(1);
                    current
                })
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

    fn mock_matches_selector(selector: &str, target: &ResolvedTarget) -> bool {
        if selector == target.selector {
            return true;
        }

        if let Some(stripped) = selector.strip_prefix("id:") {
            return target.pane_id.as_deref() == Some(stripped);
        }

        if selector.starts_with("terminal:") || selector.starts_with("plugin:") {
            return target.pane_id.as_deref() == Some(selector);
        }

        if let Some(stripped) = selector.strip_prefix("title:") {
            return target
                .title
                .as_deref()
                .is_some_and(|title| title.contains(stripped));
        }

        false
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
    fn discover_returns_recent_lines_preview_for_shell_like_pane() {
        let targets = vec![ResolvedTarget {
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("editor".to_string()),
            title: Some("shell-job".to_string()),
            command: Some("cargo test".to_string()),
            focused: false,
        }];
        let service = make_service(MockAdapter::with_targets_and_captures(
            targets,
            vec!["l1\nl2\nl3\nl4\n"],
        ));

        let response = service
            .discover(DiscoverRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: None,
                include_preview: true,
                preview_lines: Some(2),
            })
            .expect("discover should succeed");

        assert_eq!(response.candidates.len(), 1);
        assert_eq!(
            response.candidates[0].command.as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            response.candidates[0].preview_basis.as_deref(),
            Some("recent_lines")
        );
        assert_eq!(response.candidates[0].preview.as_deref(), Some("l3\nl4\n"));
    }

    #[test]
    fn discover_returns_visible_frame_preview_for_repaint_heavy_pane() {
        let targets = vec![ResolvedTarget {
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("btop".to_string()),
            title: Some("btop".to_string()),
            command: Some("btop".to_string()),
            focused: true,
        }];
        let service = make_service(MockAdapter::with_targets_and_captures(
            targets,
            vec!["\u{1b}[2J\u{1b}[Htop\ncpu 11%\nmem 30%\n"],
        ));

        let response = service
            .discover(DiscoverRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("btop".to_string()),
                selector: None,
                include_preview: true,
                preview_lines: Some(2),
            })
            .expect("discover should succeed");

        assert_eq!(
            response.candidates[0].preview_basis.as_deref(),
            Some("visible_frame")
        );
        assert!(response.candidates[0].focused);
        assert_eq!(
            response.candidates[0].preview.as_deref(),
            Some("cpu 11%\nmem 30%\n")
        );
    }

    #[test]
    fn discover_without_preview_avoids_capture_payload() {
        let targets = vec![ResolvedTarget {
            selector: "id:terminal:7".to_string(),
            pane_id: Some("terminal:7".to_string()),
            session_name: "gpu".to_string(),
            tab_name: Some("editor".to_string()),
            title: Some("shell-job".to_string()),
            command: Some("cargo test".to_string()),
            focused: false,
        }];
        let service = make_service(MockAdapter::with_targets_and_captures(
            targets,
            vec!["l1\nl2\nl3\n"],
        ));

        let response = service
            .discover(DiscoverRequest {
                session_name: "gpu".to_string(),
                tab_name: None,
                selector: None,
                include_preview: false,
                preview_lines: None,
            })
            .expect("discover should succeed");

        assert_eq!(response.candidates[0].preview, None);
        assert_eq!(response.candidates[0].preview_basis, None);
        assert_eq!(response.candidates[0].captured_at, None);
    }

    #[test]
    fn delta_mode_shell_append_only_still_returns_increment() {
        let service = make_service(MockAdapter::capture_sequence(&[
            "hello\n",
            "hello\nworld\n",
        ]));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let response = service
            .capture(CaptureRequest {
                handle: attach.handle,
                mode: CaptureMode::Delta,
                tail_lines: None,
            })
            .expect("delta capture should succeed");

        assert_eq!(response.capture.content, "world\n");
    }

    #[test]
    fn full_mode_returns_entire_capture_unchanged() {
        let service = make_service(MockAdapter::capture_sequence(&[
            "hello\n",
            "hello\nworld\n",
        ]));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let response = service
            .capture(CaptureRequest {
                handle: attach.handle,
                mode: CaptureMode::Full,
                tail_lines: None,
            })
            .expect("full capture should succeed");

        assert_eq!(response.capture.content, "hello\nworld\n");
    }

    #[test]
    fn full_mode_tail_lines_clips_after_semantic_capture() {
        let service = make_service(MockAdapter::capture_sequence(&[
            "hello\n",
            "hello\nworld\nagain\n",
        ]));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let response = service
            .capture(CaptureRequest {
                handle: attach.handle,
                mode: CaptureMode::Full,
                tail_lines: Some(2),
            })
            .expect("full capture should succeed");

        assert_eq!(response.capture.content, "world\nagain\n");
        assert_eq!(response.capture.tail_lines, Some(2));
        assert!(response.capture.line_window_applied);
    }

    #[test]
    fn delta_mode_tail_lines_clips_after_delta_computation() {
        let service = make_service(MockAdapter::capture_sequence(&[
            "base\n",
            "base\na\nb\nc\n",
        ]));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let response = service
            .capture(CaptureRequest {
                handle: attach.handle,
                mode: CaptureMode::Delta,
                tail_lines: Some(2),
            })
            .expect("delta capture should succeed");

        assert_eq!(response.capture.content, "b\nc\n");
        assert!(response.capture.line_window_applied);
    }

    #[test]
    fn capture_rejects_zero_tail_lines() {
        let service = make_service(MockAdapter::single_capture("ready\n"));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let error = service
            .capture(CaptureRequest {
                handle: attach.handle,
                mode: CaptureMode::Full,
                tail_lines: Some(0),
            })
            .expect_err("tail_lines=0 should fail");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn detect_repaint_from_clear_or_home_sequences() {
        assert!(TerminalService::<MockAdapter>::is_repaint_heavy(
            "\u{1b}[2J\u{1b}[Htop\ncpu 12%\n"
        ));
        assert!(TerminalService::<MockAdapter>::is_repaint_heavy(
            "\u{1b}[Htop\ncpu 12%\n"
        ));
        assert!(!TerminalService::<MockAdapter>::is_repaint_heavy(
            "hello\nworld\n"
        ));
    }

    #[test]
    fn normalize_current_frame_applies_clear_home_cr() {
        let content = "\u{1b}[2J\u{1b}[Htop\ncpu 10%\n\r\u{1b}[Htop\ncpu 12%\nmem 40%\n";

        assert_eq!(
            TerminalService::<MockAdapter>::normalize_current_frame(content),
            "top\ncpu 12%\nmem 40%\n"
        );
    }

    #[test]
    fn current_mode_redraw_returns_latest_stable_screen() {
        let service = make_service(MockAdapter::capture_sequence(&[
            "\u{1b}[2J\u{1b}[Htop\ncpu 10%\nmem 40%\n",
            "\u{1b}[Htop\ncpu 12%\nmem 40%\n",
        ]));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let response = service
            .capture(CaptureRequest {
                handle: attach.handle,
                mode: CaptureMode::Current,
                tail_lines: None,
            })
            .expect("current capture should succeed");

        assert_eq!(response.capture.content, "top\ncpu 12%\nmem 40%\n");
    }

    #[test]
    fn current_mode_without_repaint_keeps_command_boundary_suffix_behavior() {
        let service = make_service(MockAdapter::capture_sequence(&[
            "prompt> ",
            "prompt> result\n",
        ]));
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let response = service
            .capture(CaptureRequest {
                handle: attach.handle,
                mode: CaptureMode::Current,
                tail_lines: None,
            })
            .expect("current capture should succeed");

        assert_eq!(response.capture.content, "result\n");
    }

    #[test]
    fn detect_repaint_accepts_supported_clear_sequence_order() {
        assert!(TerminalService::<MockAdapter>::is_repaint_heavy(
            "\u{1b}[H\u{1b}[2Jtop\n"
        ));
    }

    #[test]
    fn normalize_current_frame_does_not_clear_on_cursor_positioning() {
        let content = "prefix\n\u{1b}[10;20Hsuffix\n";

        assert_eq!(
            TerminalService::<MockAdapter>::normalize_current_frame(content),
            "prefix\nsuffix\n"
        );
    }

    #[test]
    fn normalize_current_frame_does_not_clear_on_partial_erase() {
        let content = "prefix\n\u{1b}[0Jsuffix\n";

        assert_eq!(
            TerminalService::<MockAdapter>::normalize_current_frame(content),
            "prefix\nsuffix\n"
        );
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
        let spawn = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("spawn should succeed");

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
                handle: spawn.handle.clone(),
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
    fn attached_submit_text_clears_pending_input_before_send() {
        let adapter = MockAdapter::capture_sequence(&["baseline", "prompt> npx"]);
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
                handle: attach.handle.clone(),
                text: "echo hello".to_string(),
                keys: vec![],
                submit: true,
            })
            .expect("send should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "\u{15}echo hello");
        assert!(sent[0].1);

        let observations = service
            .observation_store
            .load()
            .expect("observations should load");
        assert_eq!(
            observations[0].command_boundary_content.as_deref(),
            Some("prompt> npx")
        );
    }

    #[test]
    fn spawned_submit_text_does_not_clear_pending_input_before_send() {
        let adapter = MockAdapter::capture_sequence(&["ready"]);
        let sent_inputs = adapter.sent_inputs.clone();
        let service = make_service(adapter);
        let spawn = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("spawn should succeed");

        service
            .send(SendRequest {
                handle: spawn.handle,
                text: "echo hello".to_string(),
                keys: vec![],
                submit: true,
            })
            .expect("send should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "echo hello");
        assert!(sent[0].1);
    }

    #[test]
    fn attached_keys_only_send_does_not_clear_pending_input_before_send() {
        let adapter = MockAdapter::capture_sequence(&["baseline"]);
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
                keys: vec!["up".to_string()],
                submit: false,
            })
            .expect("send should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "\u{1b}[A");
        assert!(!sent[0].1);
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
                command: Some("lazygit".to_string()),
                argv: None,
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
    fn spawn_wait_ready_timeout_returns_busy_handle_and_persists_state() {
        let mut adapter = MockAdapter::single_capture("ready");
        adapter.wait_times_out = true;
        let service = make_service(adapter);

        let response = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::NewTab,
                tab_name: Some("editor".to_string()),
                cwd: Some("/tmp".to_string()),
                command: Some("lazygit".to_string()),
                argv: None,
                title: Some("lg".to_string()),
                wait_ready: true,
            })
            .expect("spawn should return a busy handle after wait timeout");

        let bindings = service.registry_store.load().expect("bindings should load");
        let observations = service
            .observation_store
            .load()
            .expect("observations should load");

        assert_eq!(response.status, "busy");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].handle, response.handle);
        assert_eq!(bindings[0].status, TerminalStatus::Busy);
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].handle, response.handle);
        assert_eq!(observations[0].last_full_content, None);
    }

    #[test]
    fn spawn_capture_failure_returns_busy_handle_and_persists_state() {
        let mut adapter = MockAdapter::single_capture("ready");
        adapter.capture_fails = true;
        let service = make_service(adapter);

        let response = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("spawn should keep a recoverable busy handle after capture failure");

        let bindings = service.registry_store.load().expect("bindings should load");
        let observations = service
            .observation_store
            .load()
            .expect("observations should load");

        assert_eq!(response.status, "busy");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].handle, response.handle);
        assert_eq!(bindings[0].status, TerminalStatus::Busy);
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].handle, response.handle);
        assert_eq!(observations[0].last_full_content, None);
    }

    #[test]
    fn spawn_fatal_wait_error_cleans_up_persisted_state() {
        let mut adapter = MockAdapter::single_capture("ready");
        adapter.wait_fails = true;
        let service = make_service(adapter);

        let error = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::NewTab,
                tab_name: Some("editor".to_string()),
                cwd: Some("/tmp".to_string()),
                command: Some("lazygit".to_string()),
                argv: None,
                title: Some("lg".to_string()),
                wait_ready: true,
            })
            .expect_err("spawn should fail on fatal wait error");

        assert_eq!(error.code, ErrorCode::WaitFailed);
        assert!(service
            .registry_store
            .load()
            .expect("bindings should load")
            .is_empty());
        assert!(service
            .observation_store
            .load()
            .expect("observations should load")
            .is_empty());
    }

    #[test]
    fn spawn_fatal_post_launch_target_loss_cleans_up_persisted_state() {
        let mut adapter = MockAdapter::single_capture("ready");
        adapter.resolve_missing_target = true;
        adapter.capture_missing_target = true;
        let service = make_service(adapter);

        let error = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect_err("spawn should fail on fatal post-launch target loss");

        assert_eq!(error.code, ErrorCode::TargetStale);
        assert!(service
            .registry_store
            .load()
            .expect("bindings should load")
            .is_empty());
        assert!(service
            .observation_store
            .load()
            .expect("observations should load")
            .is_empty());
    }

    #[test]
    fn spawn_fatal_revalidation_error_cleans_up_persisted_state() {
        let mut adapter = MockAdapter::single_capture("ready");
        adapter.resolve_fails = true;
        let service = make_service(adapter);

        let error = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect_err("spawn should fail on fatal revalidation error");

        assert_eq!(error.code, ErrorCode::TargetNotFound);
        assert!(service
            .registry_store
            .load()
            .expect("bindings should load")
            .is_empty());
        assert!(service
            .observation_store
            .load()
            .expect("observations should load")
            .is_empty());
    }

    #[test]
    fn spawn_revalidation_prefers_stored_pane_id_when_selector_is_legacy_raw_form() {
        let root = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        let service = make_service_with_root(MockAdapter::single_capture("ready"), root.clone());

        let spawned = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("spawn should succeed");

        let mut bindings = service.registry_store.load().expect("bindings should load");
        bindings[0].selector = "terminal:7".to_string();
        service
            .registry_store
            .save(&bindings)
            .expect("bindings should save");

        let follow_up = make_service_with_root(MockAdapter::single_capture("ready"), root);
        let response = follow_up
            .wait(WaitRequest {
                handle: spawned.handle,
                idle_ms: 1200,
                timeout_ms: 30_000,
            })
            .expect("wait should revalidate through pane id");

        assert_eq!(response.status, "idle");
    }

    #[test]
    fn spawn_revalidation_ignores_stale_tab_name_when_pane_id_exists() {
        let root = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        let service = make_service_with_root(MockAdapter::single_capture("ready"), root.clone());

        let spawned = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("spawn should succeed");

        let mut bindings = service.registry_store.load().expect("bindings should load");
        bindings[0].tab_name = Some("renamed-tab".to_string());
        service
            .registry_store
            .save(&bindings)
            .expect("bindings should save");

        let follow_up = make_service_with_root(MockAdapter::single_capture("ready"), root);
        let response = follow_up
            .wait(WaitRequest {
                handle: spawned.handle,
                idle_ms: 1200,
                timeout_ms: 30_000,
            })
            .expect("wait should revalidate through pane id without tab name");

        assert_eq!(response.status, "idle");
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
    fn send_retries_once_after_refresh_when_target_lookup_is_transient() {
        let adapter = MockAdapter::single_capture("baseline");
        let sent_inputs = adapter.sent_inputs.clone();
        *adapter
            .send_failures_remaining
            .lock()
            .expect("send failures lock should succeed") = 1;
        let service = make_service(adapter);
        let spawn = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("spawn should succeed");

        let response = service
            .send(SendRequest {
                handle: spawn.handle.clone(),
                text: "echo retry".to_string(),
                keys: vec![],
                submit: true,
            })
            .expect("send should retry and succeed");

        assert!(response.accepted);
        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "echo retry");
    }

    #[test]
    fn wait_marks_binding_stale_when_target_disappears() {
        let mut adapter = MockAdapter::single_capture("baseline");
        adapter.wait_missing_target = true;
        let capture_failures_remaining = adapter.capture_failures_remaining.clone();
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        *capture_failures_remaining
            .lock()
            .expect("capture failures lock should succeed") = 3;

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
    fn wait_falls_back_to_capture_polling_when_backend_wait_is_transiently_missing() {
        let mut adapter = MockAdapter::capture_sequence(&["steady", "steady", "steady"]);
        adapter.wait_missing_target = true;
        let service = make_service(adapter);
        let spawn = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("spawn should succeed");

        let response = service
            .wait(WaitRequest {
                handle: spawn.handle,
                idle_ms: 50,
                timeout_ms: 500,
            })
            .expect("wait should fall back to capture polling");

        assert_eq!(response.status, "idle");
    }

    #[test]
    fn wait_capture_fallback_uses_wait_timeout_code_on_timeout() {
        let mut adapter = MockAdapter::capture_sequence(&["a", "b", "c", "d"]);
        adapter.wait_missing_target = true;
        let service = make_service(adapter);
        let spawn = service
            .spawn(SpawnRequest {
                session_name: "gpu".to_string(),
                target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("spawn should succeed");

        let error = service
            .wait(WaitRequest {
                handle: spawn.handle,
                idle_ms: 200,
                timeout_ms: 150,
            })
            .expect_err("wait should time out via capture fallback");

        assert_eq!(error.code, ErrorCode::WaitTimeout);
    }

    #[test]
    fn revalidate_all_retries_transient_selector_miss() {
        let adapter = MockAdapter::single_capture("baseline");
        let resolve_failures_remaining = adapter.resolve_failures_remaining.clone();
        let root = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        let service = make_service_with_root(adapter, root.clone());
        let attach = service
            .attach(AttachRequest {
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        *resolve_failures_remaining
            .lock()
            .expect("resolve failures lock should succeed") = 1;

        let service = make_service_with_root(MockAdapter::single_capture("baseline"), root);
        service
            .revalidate_all()
            .expect("revalidate should recover after transient miss");

        let bindings = service.registry_store.load().expect("bindings should load");
        assert_eq!(bindings[0].handle, attach.handle);
        assert_eq!(bindings[0].status, TerminalStatus::Ready);
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
