use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::adapters::zjctl::{
    AdapterError, ZjctlAdapter, is_plugin_permission_prompt, is_rpc_not_ready_message,
    missing_binary_name,
};
use crate::domain::binding::TerminalBinding;
use crate::domain::errors::{DomainError, ErrorCode};
use crate::domain::observation::{CaptureResult, TerminalObservation};
use chrono::Utc;

use crate::domain::requests::{
    AttachRequest, CaptureRequest, CleanupRequest, CloseRequest, DiscoverRequest, InputMode,
    LayoutRequest, ListRequest, ReplaceRequest, SendRequest, SpawnRequest, TakeoverRequest,
    WaitRequest,
};
use crate::domain::responses::{
    AttachResponse, CaptureResponse, CleanupResponse, CloseResponse, DiscoverCandidate,
    DiscoverResponse, LayoutResponse, LayoutTab, ListResponse, ReplaceResponse, SendResponse,
    SpawnResponse, TakeoverResponse, WaitResponse,
};
use crate::domain::status::{BindingSource, CaptureMode, TerminalStatus};
use crate::persistence::{ObservationStore, RegistryStore};

pub trait TerminalManager: Send + Sync {
    fn spawn(&self, request: SpawnRequest) -> Result<SpawnResponse, DomainError>;
    fn attach(&self, request: AttachRequest) -> Result<AttachResponse, DomainError>;
    fn takeover(&self, request: TakeoverRequest) -> Result<TakeoverResponse, DomainError>;
    fn discover(&self, request: DiscoverRequest) -> Result<DiscoverResponse, DomainError>;
    fn list(&self, request: ListRequest) -> Result<ListResponse, DomainError>;
    fn capture(&self, request: CaptureRequest) -> Result<CaptureResponse, DomainError>;
    fn send(&self, request: SendRequest) -> Result<SendResponse, DomainError>;
    fn replace(&self, request: ReplaceRequest) -> Result<ReplaceResponse, DomainError>;
    fn cleanup(&self, request: CleanupRequest) -> Result<CleanupResponse, DomainError>;
    fn layout(&self, request: LayoutRequest) -> Result<LayoutResponse, DomainError>;
    fn wait(&self, request: WaitRequest) -> Result<WaitResponse, DomainError>;
    fn close(&self, request: CloseRequest) -> Result<CloseResponse, DomainError>;
}

#[derive(Debug, Clone)]
pub struct TerminalService<A> {
    target_id: String,
    adapter: A,
    registry_store: RegistryStore,
    observation_store: ObservationStore,
}

impl<A> TerminalService<A> {
    pub fn new(
        target_id: impl Into<String>,
        adapter: A,
        registry_store: RegistryStore,
        observation_store: ObservationStore,
    ) -> Self {
        Self {
            target_id: target_id.into(),
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

    fn binding_belongs_to_target(&self, binding: &TerminalBinding) -> bool {
        binding.target_id == self.target_id
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
            AdapterError::CommandFailed(message) if missing_binary_name(&message).is_some() => {
                DomainError::new(ErrorCode::ZjctlUnavailable, message, true)
            }
            AdapterError::CommandFailed(message) if is_plugin_permission_prompt(&message) => {
                DomainError::new(ErrorCode::PluginNotReady, message, false)
            }
            AdapterError::CommandFailed(message) if is_rpc_not_ready_message(&message) => {
                DomainError::new(ErrorCode::PluginNotReady, message, true)
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

    fn resolve_send_mode(request: &SendRequest) -> Result<ResolvedSendMode, DomainError> {
        match request.input_mode {
            Some(InputMode::Raw) => {
                if request.submit {
                    return Err(DomainError::new(
                        ErrorCode::InvalidArgument,
                        "`input_mode=raw` cannot be combined with `submit=true`".to_string(),
                        false,
                    ));
                }
                Ok(ResolvedSendMode::Raw)
            }
            Some(InputMode::SubmitLine) => {
                if !request.keys.is_empty() {
                    return Err(DomainError::new(
                        ErrorCode::InvalidArgument,
                        "`input_mode=submit_line` does not accept named `keys`; send shell text only"
                            .to_string(),
                        false,
                    ));
                }
                if request.text.is_empty() {
                    return Err(DomainError::new(
                        ErrorCode::InvalidArgument,
                        "`input_mode=submit_line` requires non-empty `text`".to_string(),
                        false,
                    ));
                }
                Ok(ResolvedSendMode::SubmitLine)
            }
            None => Ok(ResolvedSendMode::Legacy),
        }
    }

    fn build_send_payload(
        request: &SendRequest,
        mode: ResolvedSendMode,
    ) -> Result<(String, bool), DomainError> {
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

        let submit = match mode {
            ResolvedSendMode::Raw => false,
            ResolvedSendMode::SubmitLine => true,
            ResolvedSendMode::Legacy => {
                let submit = request.submit && request.keys.is_empty();
                if request.submit && !request.keys.is_empty() {
                    payload.push('\n');
                }
                submit
            }
        };

        Ok((payload, submit))
    }

    fn build_revalidation_requests(binding: &TerminalBinding) -> Vec<AttachRequest> {
        let mut requests = Vec::new();

        if let Some(pane_id) = binding.pane_id.as_deref() {
            requests.push(AttachRequest {
                target: None,
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
                target: None,
                session_name: binding.session_name.clone(),
                tab_name: binding.tab_name.clone(),
                selector: binding.selector.clone(),
                alias: None,
            });
        }

        requests
    }

    fn should_clear_attached_input(
        binding: &TerminalBinding,
        request: &SendRequest,
        mode: ResolvedSendMode,
    ) -> bool {
        binding.source == BindingSource::Attached
            && matches!(
                mode,
                ResolvedSendMode::Legacy | ResolvedSendMode::SubmitLine
            )
            && (request.submit || matches!(mode, ResolvedSendMode::SubmitLine))
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

    fn attach_resolved_target(
        &self,
        resolved: crate::adapters::zjctl::ResolvedTarget,
        alias: Option<String>,
    ) -> Result<AttachResponse, DomainError> {
        let snapshot = self
            .adapter
            .capture_full(&resolved.session_name, &resolved.selector)
            .map_err(|error| self.map_adapter_error(error, ErrorCode::CaptureFailed))?;

        let handle = Self::next_handle();
        let now = snapshot.captured_at;
        let mut bindings = self.read_bindings()?;
        bindings.push(TerminalBinding {
            handle: handle.clone(),
            target_id: self.target_id.clone(),
            alias,
            session_name: resolved.session_name,
            tab_name: resolved.tab_name,
            selector: resolved.selector,
            pane_id: resolved.pane_id,
            cwd: None,
            launch_command: resolved.command,
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
            target_id: self.target_id.clone(),
            attached: true,
            baseline_established: true,
        })
    }

    fn takeover_target_matches(
        request: &TakeoverRequest,
        target: &crate::adapters::zjctl::ResolvedTarget,
    ) -> bool {
        if request
            .tab_name
            .as_ref()
            .is_some_and(|tab| target.tab_name.as_deref() != Some(tab.as_str()))
        {
            return false;
        }

        if request
            .focused
            .is_some_and(|focused| target.focused != focused)
        {
            return false;
        }

        if request
            .selector
            .as_deref()
            .is_some_and(|selector| !Self::discover_matches_selector(selector, target))
        {
            return false;
        }

        if request.command_contains.as_deref().is_some_and(|needle| {
            !target
                .command
                .as_deref()
                .is_some_and(|command| command.contains(needle))
        }) {
            return false;
        }

        true
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
                        return Err(self.map_adapter_error(error, ErrorCode::AttachFailed));
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

    fn explicit_interaction_marker_prefix() -> &'static str {
        "__ZELLIJ_MCP_INTERACTION__"
    }

    fn next_interaction_id() -> String {
        format!("zi_{}", uuid::Uuid::new_v4().simple())
    }

    fn shell_name(command: Option<&str>) -> Option<String> {
        let command = command?.trim();
        if command.is_empty() {
            return None;
        }

        let first = command.split_whitespace().next()?.rsplit('/').next()?;
        Some(first.to_ascii_lowercase())
    }

    fn supports_explicit_interaction_markers(binding: &TerminalBinding) -> bool {
        matches!(
            Self::shell_name(binding.launch_command.as_deref()).as_deref(),
            Some("sh" | "bash" | "zsh" | "fish")
        )
    }

    fn build_wrapped_submit_payload(
        binding: &TerminalBinding,
        text: &str,
        interaction_id: &str,
    ) -> Option<String> {
        let shell = Self::shell_name(binding.launch_command.as_deref())?;
        let marker = Self::explicit_interaction_marker_prefix();

        match shell.as_str() {
            "sh" | "bash" | "zsh" => Some(format!(
                "printf '{marker}:start:{id}\\n'; {text}; __zellij_mcp_status=$?; printf '\\n{marker}:end:{id}:%s\\n' \"$__zellij_mcp_status\"",
                marker = marker,
                id = interaction_id,
                text = text,
            )),
            "fish" => Some(format!(
                "printf '{marker}:start:{id}\\n'; begin; {text}; end; set __zellij_mcp_status $status; printf '\\n{marker}:end:{id}:%s\\n' $__zellij_mcp_status",
                marker = marker,
                id = interaction_id,
                text = text,
            )),
            _ => None,
        }
    }

    fn parse_interaction_capture(
        content: &str,
        interaction_id: &str,
    ) -> Option<(String, bool, Option<i32>)> {
        let marker = Self::explicit_interaction_marker_prefix();
        let start_marker = format!("{marker}:start:{interaction_id}");
        let end_marker_prefix = format!("{marker}:end:{interaction_id}:");
        let start_index = content.find(&start_marker)?;
        let after_start = &content[start_index + start_marker.len()..];
        let after_start = after_start.strip_prefix('\n').unwrap_or(after_start);

        if let Some(end_relative) = after_start.find(&end_marker_prefix) {
            let body = &after_start[..end_relative];
            let mut exit_code = None;
            let after_end = &after_start[end_relative + end_marker_prefix.len()..];
            let status_text = after_end.lines().next().unwrap_or("").trim();
            if !status_text.is_empty() {
                exit_code = status_text.parse::<i32>().ok();
            }
            return Some((body.trim_start_matches('\n').to_string(), true, exit_code));
        }

        Some((after_start.to_string(), false, None))
    }

    fn interaction_capture_from_observation(
        content: &str,
        observation: &TerminalObservation,
    ) -> Option<(String, bool, Option<i32>)> {
        let interaction_id = observation.interaction_id.as_deref()?;
        Self::parse_interaction_capture(content, interaction_id)
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

    fn resolve_capture_window(
        request: &CaptureRequest,
    ) -> Result<(Option<usize>, Option<usize>), DomainError> {
        let line_limit = Self::validate_positive_lines(request.line_limit, "line_limit")?;
        let has_forward_windowing =
            request.line_offset.is_some() || line_limit.is_some() || request.cursor.is_some();

        if request.tail_lines.is_some() && has_forward_windowing {
            return Err(DomainError::new(
                ErrorCode::InvalidArgument,
                "`tail_lines` cannot be combined with `line_offset`, `line_limit`, or `cursor`"
                    .to_string(),
                false,
            ));
        }

        if matches!(request.mode, CaptureMode::Delta) && has_forward_windowing {
            return Err(DomainError::new(
                ErrorCode::InvalidArgument,
                "`line_offset`, `line_limit`, and `cursor` are not supported for `mode=delta`"
                    .to_string(),
                false,
            ));
        }

        if request.cursor.is_some() && request.line_offset.is_some() {
            return Err(DomainError::new(
                ErrorCode::InvalidArgument,
                "`cursor` cannot be combined with `line_offset`".to_string(),
                false,
            ));
        }

        let line_offset = match request.cursor.as_deref() {
            Some(cursor) => Some(Self::parse_line_cursor(cursor)?),
            None => request.line_offset,
        };

        Ok((line_offset, line_limit))
    }

    fn parse_line_cursor(cursor: &str) -> Result<usize, DomainError> {
        let offset = cursor
            .strip_prefix("lines:")
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::InvalidArgument,
                    "`cursor` must use the `lines:<offset>` format".to_string(),
                    false,
                )
            })?
            .parse::<usize>()
            .map_err(|_| {
                DomainError::new(
                    ErrorCode::InvalidArgument,
                    "`cursor` must use the `lines:<offset>` format".to_string(),
                    false,
                )
            })?;

        Ok(offset)
    }

    fn format_line_cursor(offset: usize) -> String {
        format!("lines:{offset}")
    }

    fn strip_ansi_sequences(content: &str) -> String {
        let mut output = String::new();
        let chars: Vec<char> = content.chars().collect();
        let mut index = 0;

        while index < chars.len() {
            let ch = chars[index];
            if ch != '\u{1b}' {
                output.push(ch);
                index += 1;
                continue;
            }

            index += 1;
            if index >= chars.len() {
                break;
            }

            match chars[index] {
                '[' => {
                    index += 1;
                    while index < chars.len() {
                        let next = chars[index];
                        index += 1;
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                ']' => {
                    index += 1;
                    while index < chars.len() {
                        match chars[index] {
                            '\u{7}' => {
                                index += 1;
                                break;
                            }
                            '\u{1b}' if chars.get(index + 1).copied() == Some('\\') => {
                                index += 2;
                                break;
                            }
                            _ => index += 1,
                        }
                    }
                }
                _ => {
                    index += 1;
                }
            }
        }

        output
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

    fn line_window(
        content: &str,
        line_offset: Option<usize>,
        line_limit: Option<usize>,
    ) -> (String, bool, Option<String>) {
        let start = line_offset.unwrap_or(0);

        if start == 0 && line_limit.is_none() {
            return (content.to_string(), false, None);
        }

        let lines: Vec<&str> = if content.is_empty() {
            Vec::new()
        } else {
            content.split_inclusive('\n').collect()
        };
        let total_lines = lines.len();
        let end = line_limit
            .map(|limit| start.saturating_add(limit).min(total_lines))
            .unwrap_or(total_lines);

        let window = if start >= total_lines {
            String::new()
        } else {
            lines[start..end].concat()
        };
        let applied = start > 0 || end < total_lines;
        let next_cursor = if end < total_lines {
            Some(Self::format_line_cursor(end))
        } else {
            None
        };

        (window, applied, next_cursor)
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

        if selector == "focused" || selector == "focused:true" {
            return target.focused;
        }

        if selector == "unfocused" || selector == "focused:false" {
            return !target.focused;
        }

        if let Some(stripped) = selector.strip_prefix("title:") {
            return target
                .title
                .as_deref()
                .is_some_and(|title| title.contains(stripped));
        }

        if let Some(stripped) = selector.strip_prefix("command:") {
            return target
                .command
                .as_deref()
                .is_some_and(|command| command.contains(stripped));
        }

        if let Some(stripped) = selector.strip_prefix("tab:") {
            return target
                .tab_name
                .as_deref()
                .is_some_and(|tab| tab.contains(stripped));
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
        if let Some((_, completed, exit_code)) = Self::interaction_capture_from_observation(
            observation.last_full_content.as_deref().unwrap_or(""),
            observation,
        ) && completed
        {
            observation.complete_interaction(exit_code, snapshot.captured_at);
        }
        self.write_observations(&observations)
    }

    pub fn revalidate_all(&self) -> Result<(), DomainError> {
        if !self.adapter.is_available() {
            return Ok(());
        }

        let bindings = self.read_bindings()?;
        for binding in bindings {
            if !self.binding_belongs_to_target(&binding) {
                continue;
            }
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
            target_id: self.target_id.clone(),
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
                        target_id: self.target_id.clone(),
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
                        target_id: self.target_id.clone(),
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
            target_id: self.target_id.clone(),
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
        self.attach_resolved_target(resolved, request.alias)
    }

    fn takeover(&self, request: TakeoverRequest) -> Result<TakeoverResponse, DomainError> {
        self.ensure_available()?;

        let candidates = self
            .adapter
            .list_targets_in_session(&request.session_name)
            .map_err(|error| self.map_adapter_error(error, ErrorCode::AttachFailed))?;
        let matches: Vec<_> = candidates
            .into_iter()
            .filter(|target| Self::takeover_target_matches(&request, target))
            .collect();

        let matched = match matches.as_slice() {
            [] => {
                return Err(DomainError::new(
                    ErrorCode::TargetNotFound,
                    "takeover request did not match any pane".to_string(),
                    false,
                ));
            }
            [target] => target.clone(),
            _ => {
                return Err(DomainError::new(
                    ErrorCode::SelectorNotUnique,
                    "takeover request matched multiple panes".to_string(),
                    false,
                ));
            }
        };

        let matched_selector = matched.selector.clone();
        let response = self.attach_resolved_target(matched, request.alias)?;

        Ok(TakeoverResponse {
            handle: response.handle,
            target_id: response.target_id,
            attached: response.attached,
            baseline_established: response.baseline_established,
            matched_selector,
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
                match self
                    .adapter
                    .capture_full(&target.session_name, &target.selector)
                {
                    Ok(snapshot) => {
                        let (preview, basis) =
                            Self::preview_for_snapshot(&snapshot.content, preview_lines);
                        (Some(preview), Some(basis), Some(snapshot.captured_at))
                    }
                    Err(_) => (None, None, None),
                }
            } else {
                (None, None, None)
            };

            candidates.push(DiscoverCandidate {
                target_id: self.target_id.clone(),
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
        bindings.retain(|binding| self.binding_belongs_to_target(binding));
        if let Some(session_name) = request.session_name {
            bindings.retain(|binding| binding.session_name == session_name);
        }

        Ok(ListResponse { bindings })
    }

    fn capture(&self, request: CaptureRequest) -> Result<CaptureResponse, DomainError> {
        self.ensure_available()?;
        let tail_lines = Self::validate_positive_lines(request.tail_lines, "tail_lines")?;
        let (line_offset, line_limit) = Self::resolve_capture_window(&request)?;

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
        let explicit_interaction =
            Self::interaction_capture_from_observation(&snapshot.content, observation);
        let content = match request.mode {
            CaptureMode::Full => snapshot.content.clone(),
            CaptureMode::Delta => {
                Self::suffix_after_baseline(&snapshot.content, previous_full.as_deref())
            }
            CaptureMode::Current => {
                if let Some((content, _, _)) = explicit_interaction.as_ref() {
                    content.clone()
                } else if Self::is_repaint_heavy(&snapshot.content) {
                    Self::normalize_current_frame(&snapshot.content)
                } else {
                    Self::suffix_after_baseline(&snapshot.content, boundary_full.as_deref())
                }
            }
        };
        let baseline = match request.mode {
            CaptureMode::Full => None,
            CaptureMode::Delta => Some("last_capture".to_string()),
            CaptureMode::Current => Some(if explicit_interaction.is_some() {
                "interaction_marker".to_string()
            } else {
                "command_boundary".to_string()
            }),
        };
        let content = if request.normalize_ansi {
            Self::strip_ansi_sequences(&content)
        } else {
            content
        };
        let (content, line_window_applied, next_cursor) = match tail_lines {
            Some(count) => {
                let (content, applied) = Self::tail_lines(&content, count);
                (content, applied, None)
            }
            None => Self::line_window(&content, line_offset, line_limit),
        };

        let hash = Self::hash_content(&snapshot.content);
        observation.update_full_snapshot(snapshot.content, hash, snapshot.captured_at);
        if let Some((_, completed, exit_code)) = explicit_interaction
            && completed
        {
            observation.complete_interaction(exit_code, snapshot.captured_at);
        }
        let interaction_id = observation.interaction_id.clone();
        let interaction_completed = observation
            .interaction_id
            .as_ref()
            .map(|_| observation.interaction_completed_at.is_some());
        let interaction_exit_code = observation.interaction_exit_code;
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
                line_offset,
                line_limit,
                line_window_applied,
                next_cursor,
                ansi_normalized: request.normalize_ansi,
                truncated: snapshot.truncated,
                captured_at: snapshot.captured_at,
                baseline,
                interaction_id,
                interaction_completed,
                interaction_exit_code,
            },
        })
    }

    fn send(&self, request: SendRequest) -> Result<SendResponse, DomainError> {
        self.ensure_available()?;

        let binding = self.ensure_handle_revalidated(&request.handle)?;
        let send_mode = Self::resolve_send_mode(&request)?;
        let clear_attached_input = Self::should_clear_attached_input(&binding, &request, send_mode);
        if clear_attached_input {
            self.prepare_attached_send_boundary(&binding, &request.handle)?;
        }

        let (mut payload, submit) = Self::build_send_payload(&request, send_mode)?;
        let interaction_id = if matches!(
            send_mode,
            ResolvedSendMode::Legacy | ResolvedSendMode::SubmitLine
        ) && request.keys.is_empty()
            && !request.text.is_empty()
            && Self::supports_explicit_interaction_markers(&binding)
        {
            Some(Self::next_interaction_id())
        } else {
            None
        };

        if let Some(interaction_id) = interaction_id.as_deref()
            && let Some(wrapped) =
                Self::build_wrapped_submit_payload(&binding, &request.text, interaction_id)
        {
            payload = wrapped;
        }
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

        if let Some(interaction_id) = interaction_id {
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
            observation.start_interaction(interaction_id, Utc::now());
            self.write_observations(&observations)?;
        }

        if matches!(send_mode, ResolvedSendMode::SubmitLine)
            || (matches!(send_mode, ResolvedSendMode::Legacy)
                && request.submit
                && !clear_attached_input)
        {
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

    fn replace(&self, request: ReplaceRequest) -> Result<ReplaceResponse, DomainError> {
        self.ensure_available()?;

        let binding = self.ensure_handle_revalidated(&request.handle)?;
        Self::ensure_binding_active(&binding)?;

        if !Self::supports_explicit_interaction_markers(&binding) {
            return Err(DomainError::new(
                ErrorCode::InvalidArgument,
                "replace is only supported for managed shell-like panes".to_string(),
                false,
            ));
        }

        if request.interrupt {
            self.run_binding_operation_with_retry(
                &request.handle,
                &binding,
                ErrorCode::SendFailed,
                |binding| {
                    self.adapter.send_input(
                        &binding.session_name,
                        &binding.selector,
                        "\u{3}",
                        false,
                    )
                },
            )?;
        }

        self.send(SendRequest {
            handle: request.handle.clone(),
            text: request.command,
            keys: Vec::new(),
            input_mode: Some(InputMode::SubmitLine),
            submit: false,
        })?;

        let observations = self.read_observations()?;
        let interaction_id = observations
            .iter()
            .find(|item| item.handle == request.handle)
            .and_then(|item| item.interaction_id.clone());

        Ok(ReplaceResponse {
            handle: request.handle,
            replaced: true,
            interaction_id,
        })
    }

    fn cleanup(&self, request: CleanupRequest) -> Result<CleanupResponse, DomainError> {
        let statuses = if request.statuses.is_empty() {
            vec![TerminalStatus::Stale, TerminalStatus::Closed]
        } else {
            request.statuses
        };
        let cutoff = request
            .max_age_ms
            .map(|age| Utc::now() - chrono::Duration::milliseconds(age as i64));

        let bindings = self.read_bindings()?;
        let removed_handles: Vec<String> = bindings
            .iter()
            .filter(|binding| self.binding_belongs_to_target(binding))
            .filter(|binding| statuses.contains(&binding.status))
            .filter(|binding| cutoff.is_none_or(|cutoff| binding.updated_at <= cutoff))
            .map(|binding| binding.handle.clone())
            .collect();

        if !request.dry_run && !removed_handles.is_empty() {
            let remaining_bindings: Vec<_> = bindings
                .into_iter()
                .filter(|binding| !removed_handles.contains(&binding.handle))
                .collect();
            self.write_bindings(&remaining_bindings)?;

            let remaining_observations: Vec<_> = self
                .read_observations()?
                .into_iter()
                .filter(|observation| !removed_handles.contains(&observation.handle))
                .collect();
            self.write_observations(&remaining_observations)?;
        }

        Ok(CleanupResponse {
            removed_count: removed_handles.len(),
            removed_handles,
            dry_run: request.dry_run,
        })
    }

    fn layout(&self, request: LayoutRequest) -> Result<LayoutResponse, DomainError> {
        self.ensure_available()?;

        let mut grouped: std::collections::BTreeMap<String, Vec<DiscoverCandidate>> =
            std::collections::BTreeMap::new();
        let targets = self
            .adapter
            .list_targets_in_session(&request.session_name)
            .map_err(|error| self.map_adapter_error(error, ErrorCode::AttachFailed))?;

        for target in targets {
            let tab_name = target
                .tab_name
                .clone()
                .unwrap_or_else(|| "<unknown>".to_string());
            grouped
                .entry(tab_name.clone())
                .or_default()
                .push(DiscoverCandidate {
                    target_id: self.target_id.clone(),
                    selector: target.selector,
                    pane_id: target.pane_id,
                    session_name: target.session_name,
                    tab_name: Some(tab_name),
                    title: target.title,
                    command: target.command,
                    focused: target.focused,
                    preview: None,
                    preview_basis: None,
                    captured_at: None,
                });
        }

        let tabs = grouped
            .into_iter()
            .map(|(tab_name, panes)| LayoutTab { tab_name, panes })
            .collect();

        Ok(LayoutResponse {
            target_id: self.target_id.clone(),
            session_name: request.session_name,
            tabs,
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
        if result.is_ok() {
            let mut bindings = self.read_bindings()?;
            if let Some(binding) = bindings
                .iter_mut()
                .find(|binding| binding.handle == request.handle)
            {
                binding.status = TerminalStatus::Ready;
                binding.updated_at = observed_at;
            }
            self.write_bindings(&bindings)?;
        } else if matches!(
            result,
            Err(DomainError {
                code: ErrorCode::WaitTimeout,
                ..
            })
        ) {
            let mut bindings = self.read_bindings()?;
            if let Some(binding) = bindings
                .iter_mut()
                .find(|binding| binding.handle == request.handle)
            {
                binding.status = TerminalStatus::Busy;
                binding.updated_at = observed_at;
            }
            self.write_bindings(&bindings)?;
        }

        result?;

        let mut completion_basis = None;
        let mut interaction_id = None;
        let mut interaction_completed = None;
        let mut interaction_exit_code = None;

        let snapshot = self.run_binding_operation_with_retry(
            &request.handle,
            &busy_binding,
            ErrorCode::CaptureFailed,
            |binding| {
                self.adapter
                    .capture_full(&binding.session_name, &binding.selector)
            },
        )?;
        let mut observations = self.read_observations()?;
        if let Some(observation) = observations
            .iter_mut()
            .find(|item| item.handle == request.handle)
            && let Some((_, completed, exit_code)) =
                Self::interaction_capture_from_observation(&snapshot.content, observation)
        {
            interaction_id = observation.interaction_id.clone();
            interaction_completed = Some(completed);
            interaction_exit_code = exit_code;
            if completed {
                observation.complete_interaction(exit_code, snapshot.captured_at);
                completion_basis = Some("interaction_marker".to_string());
            }
            let hash = Self::hash_content(&snapshot.content);
            observation.update_full_snapshot(snapshot.content, hash, snapshot.captured_at);
            self.write_observations(&observations)?;
        }

        Ok(WaitResponse {
            handle: request.handle,
            status: "idle".to_string(),
            observed_at,
            completion_basis,
            interaction_id,
            interaction_completed,
            interaction_exit_code,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedSendMode {
    Raw,
    SubmitLine,
    Legacy,
}

fn map_key_sequence(key: &str) -> Result<String, DomainError> {
    let normalized = key.trim().to_ascii_lowercase().replace('-', "_");

    let value = match normalized.as_str() {
        "enter" => "\n".to_string(),
        "tab" => "\t".to_string(),
        "shift_tab" => "\u{1b}[Z".to_string(),
        "escape" | "esc" => "\u{1b}".to_string(),
        "up" => "\u{1b}[A".to_string(),
        "down" => "\u{1b}[B".to_string(),
        "right" => "\u{1b}[C".to_string(),
        "left" => "\u{1b}[D".to_string(),
        "home" => "\u{1b}[H".to_string(),
        "end" => "\u{1b}[F".to_string(),
        "insert" => "\u{1b}[2~".to_string(),
        "delete" => "\u{1b}[3~".to_string(),
        "page_up" => "\u{1b}[5~".to_string(),
        "page_down" => "\u{1b}[6~".to_string(),
        "backspace" => "\u{7f}".to_string(),
        "f1" => "\u{1b}OP".to_string(),
        "f2" => "\u{1b}OQ".to_string(),
        "f3" => "\u{1b}OR".to_string(),
        "f4" => "\u{1b}OS".to_string(),
        "f5" => "\u{1b}[15~".to_string(),
        "f6" => "\u{1b}[17~".to_string(),
        "f7" => "\u{1b}[18~".to_string(),
        "f8" => "\u{1b}[19~".to_string(),
        "f9" => "\u{1b}[20~".to_string(),
        "f10" => "\u{1b}[21~".to_string(),
        "f11" => "\u{1b}[23~".to_string(),
        "f12" => "\u{1b}[24~".to_string(),
        other => {
            if let Some(chord) = other.strip_prefix("ctrl_")
                && chord.len() == 1
            {
                let byte = chord.as_bytes()[0];
                if byte.is_ascii_lowercase() {
                    return Ok(((byte - b'a' + 1) as char).to_string());
                }
            }

            return Err(DomainError::new(
                ErrorCode::InvalidArgument,
                format!("unsupported special key `{key}`"),
                false,
            ));
        }
    };

    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::adapters::zjctl::{CaptureSnapshot, ResolvedTarget, ZjctlAdapter};
    use crate::domain::requests::{
        AttachRequest, CleanupRequest, CloseRequest, InputMode, LayoutRequest, ListRequest,
        ReplaceRequest, SendRequest, SpawnRequest, TakeoverRequest, WaitRequest,
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

    fn make_remote_service(target_id: &str, adapter: MockAdapter) -> TerminalService<MockAdapter> {
        let root = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        TerminalService::new(
            target_id,
            adapter,
            RegistryStore::new(root.join("registry.json")),
            ObservationStore::new(root.join("observations.json")),
        )
    }

    fn make_service_with_root(
        adapter: MockAdapter,
        root: std::path::PathBuf,
    ) -> TerminalService<MockAdapter> {
        TerminalService::new(
            "local",
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
                target: None,
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
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let listed = service
            .list(ListRequest {
                target: None,
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
                target: None,
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
                target: None,
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
                target: None,
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
    fn discover_preserves_metadata_when_one_preview_capture_fails() {
        let targets = vec![
            ResolvedTarget {
                selector: "id:terminal:7".to_string(),
                pane_id: Some("terminal:7".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                title: Some("shell-job".to_string()),
                command: Some("cargo test".to_string()),
                focused: false,
            },
            ResolvedTarget {
                selector: "id:terminal:8".to_string(),
                pane_id: Some("terminal:8".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                title: Some("second-job".to_string()),
                command: Some("htop".to_string()),
                focused: true,
            },
        ];
        let adapter =
            MockAdapter::with_targets_and_captures(targets, vec!["line1\nline2\nline3\n"]);
        *adapter
            .capture_failures_remaining
            .lock()
            .expect("capture failures lock should succeed") = 1;
        let service = make_service(adapter);

        let response = service
            .discover(DiscoverRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: None,
                include_preview: true,
                preview_lines: Some(2),
            })
            .expect("discover should degrade failed previews to metadata-only candidates");

        assert_eq!(response.candidates.len(), 2);
        assert_eq!(response.candidates[0].selector, "id:terminal:7");
        assert_eq!(response.candidates[0].title.as_deref(), Some("shell-job"));
        assert_eq!(response.candidates[0].preview, None);
        assert_eq!(response.candidates[0].preview_basis, None);
        assert_eq!(response.candidates[0].captured_at, None);

        assert_eq!(response.candidates[1].selector, "id:terminal:8");
        assert_eq!(response.candidates[1].title.as_deref(), Some("second-job"));
        assert_eq!(
            response.candidates[1].preview.as_deref(),
            Some("line2\nline3\n")
        );
        assert_eq!(
            response.candidates[1].preview_basis.as_deref(),
            Some("recent_lines")
        );
        assert!(response.candidates[1].captured_at.is_some());
    }

    #[test]
    fn discover_succeeds_when_all_preview_captures_fail() {
        let targets = vec![
            ResolvedTarget {
                selector: "id:terminal:7".to_string(),
                pane_id: Some("terminal:7".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                title: Some("shell-job".to_string()),
                command: Some("cargo test".to_string()),
                focused: false,
            },
            ResolvedTarget {
                selector: "id:terminal:8".to_string(),
                pane_id: Some("terminal:8".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                title: Some("second-job".to_string()),
                command: Some("htop".to_string()),
                focused: true,
            },
        ];
        let adapter = MockAdapter::with_targets_and_captures(targets, vec!["unused\n"]);
        *adapter
            .capture_failures_remaining
            .lock()
            .expect("capture failures lock should succeed") = 10;
        let service = make_service(adapter);

        let response = service
            .discover(DiscoverRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: None,
                include_preview: true,
                preview_lines: Some(2),
            })
            .expect("discover should still succeed when all previews fail");

        assert_eq!(response.candidates.len(), 2);
        for candidate in response.candidates {
            assert!(candidate.preview.is_none());
            assert!(candidate.preview_basis.is_none());
            assert!(candidate.captured_at.is_none());
            assert!(candidate.title.is_some());
        }
    }

    #[test]
    fn takeover_attaches_unique_match_in_one_step() {
        let service = make_service(MockAdapter::single_capture("baseline"));

        let response = service
            .takeover(TakeoverRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: Some("title:editor".to_string()),
                command_contains: Some("fish".to_string()),
                focused: Some(false),
                alias: Some("taken".to_string()),
            })
            .expect("takeover should succeed");

        let bindings = service.registry_store.load().expect("bindings should load");
        assert_eq!(response.target_id, "local");
        assert_eq!(response.matched_selector, "id:terminal:7");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].alias.as_deref(), Some("taken"));
    }

    #[test]
    fn takeover_rejects_ambiguous_matches() {
        let mut adapter = MockAdapter::single_capture("baseline");
        adapter.list_targets = vec![
            adapter.target.clone(),
            ResolvedTarget {
                selector: "id:terminal:8".to_string(),
                pane_id: Some("terminal:8".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                title: Some("editor-two".to_string()),
                command: Some("fish".to_string()),
                focused: false,
            },
        ];
        let service = make_service(adapter);

        let error = service
            .takeover(TakeoverRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: None,
                command_contains: Some("fish".to_string()),
                focused: Some(false),
                alias: None,
            })
            .expect_err("ambiguous takeover should fail");

        assert_eq!(error.code, ErrorCode::SelectorNotUnique);
    }

    #[test]
    fn discover_supports_command_tab_and_focus_selectors() {
        let targets = vec![
            ResolvedTarget {
                selector: "id:terminal:7".to_string(),
                pane_id: Some("terminal:7".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor-main".to_string()),
                title: Some("editor-main".to_string()),
                command: Some("cargo test".to_string()),
                focused: true,
            },
            ResolvedTarget {
                selector: "id:terminal:8".to_string(),
                pane_id: Some("terminal:8".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("ops".to_string()),
                title: Some("ops".to_string()),
                command: Some("htop".to_string()),
                focused: false,
            },
        ];
        let service = make_service(MockAdapter::with_targets_and_captures(
            targets,
            vec!["ok\n"],
        ));

        let response = service
            .discover(DiscoverRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: None,
                selector: Some("command:cargo test".to_string()),
                include_preview: false,
                preview_lines: None,
            })
            .expect("discover should succeed");
        assert_eq!(response.candidates.len(), 1);
        assert_eq!(response.candidates[0].selector, "id:terminal:7");

        let response = service
            .discover(DiscoverRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: None,
                selector: Some("focused".to_string()),
                include_preview: false,
                preview_lines: None,
            })
            .expect("discover should succeed");
        assert_eq!(response.candidates.len(), 1);
        assert!(response.candidates[0].focused);
    }

    #[test]
    fn layout_groups_panes_by_tab() {
        let targets = vec![
            ResolvedTarget {
                selector: "id:terminal:7".to_string(),
                pane_id: Some("terminal:7".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                title: Some("editor".to_string()),
                command: Some("fish".to_string()),
                focused: false,
            },
            ResolvedTarget {
                selector: "id:terminal:8".to_string(),
                pane_id: Some("terminal:8".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                title: Some("tests".to_string()),
                command: Some("cargo test".to_string()),
                focused: true,
            },
            ResolvedTarget {
                selector: "id:terminal:9".to_string(),
                pane_id: Some("terminal:9".to_string()),
                session_name: "gpu".to_string(),
                tab_name: Some("ops".to_string()),
                title: Some("htop".to_string()),
                command: Some("htop".to_string()),
                focused: false,
            },
        ];
        let service = make_service(MockAdapter::with_targets_and_captures(
            targets,
            vec!["ok\n"],
        ));

        let response = service
            .layout(LayoutRequest {
                target: None,
                session_name: "gpu".to_string(),
            })
            .expect("layout should succeed");

        assert_eq!(response.tabs.len(), 2);
        assert_eq!(response.tabs[0].tab_name, "editor");
        assert_eq!(response.tabs[0].panes.len(), 2);
        assert_eq!(response.tabs[1].tab_name, "ops");
        assert_eq!(response.tabs[1].panes.len(), 1);
    }

    #[test]
    fn delta_mode_shell_append_only_still_returns_increment() {
        let service = make_service(MockAdapter::capture_sequence(&[
            "hello\n",
            "hello\nworld\n",
        ]));
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
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
                target: None,
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
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
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
                target: None,
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
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
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
                target: None,
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
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
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
                target: None,
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
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
            })
            .expect_err("tail_lines=0 should fail");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn full_mode_line_limit_returns_next_cursor() {
        let service = make_service(MockAdapter::capture_sequence(&["hello\n", "a\nb\nc\nd\n"]));
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                line_offset: None,
                line_limit: Some(2),
                cursor: None,
                normalize_ansi: false,
            })
            .expect("windowed full capture should succeed");

        assert_eq!(response.capture.content, "a\nb\n");
        assert_eq!(response.capture.line_limit, Some(2));
        assert_eq!(response.capture.next_cursor.as_deref(), Some("lines:2"));
        assert!(response.capture.line_window_applied);
    }

    #[test]
    fn full_mode_cursor_resumes_after_previous_window() {
        let service = make_service(MockAdapter::capture_sequence(&["hello\n", "a\nb\nc\nd\n"]));
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                line_offset: None,
                line_limit: Some(2),
                cursor: Some("lines:2".to_string()),
                normalize_ansi: false,
            })
            .expect("cursor-resumed capture should succeed");

        assert_eq!(response.capture.content, "c\nd\n");
        assert_eq!(response.capture.line_offset, Some(2));
        assert_eq!(response.capture.next_cursor, None);
        assert!(response.capture.line_window_applied);
    }

    #[test]
    fn capture_rejects_combining_tail_lines_with_cursor_windows() {
        let service = make_service(MockAdapter::single_capture("ready\n"));
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                tail_lines: Some(1),
                line_offset: Some(0),
                line_limit: Some(1),
                cursor: None,
                normalize_ansi: false,
            })
            .expect_err("mixed windowing modes should fail");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn capture_rejects_invalid_cursor_format() {
        let service = make_service(MockAdapter::single_capture("ready\n"));
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                tail_lines: None,
                line_offset: None,
                line_limit: Some(1),
                cursor: Some("bad-cursor".to_string()),
                normalize_ansi: false,
            })
            .expect_err("invalid cursor should fail");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn capture_rejects_delta_mode_cursor_windowing() {
        let service = make_service(MockAdapter::capture_sequence(&[
            "base\n",
            "base\na\nb\nc\n",
        ]));
        let attach = service
            .attach(AttachRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let error = service
            .capture(CaptureRequest {
                handle: attach.handle,
                mode: CaptureMode::Delta,
                tail_lines: None,
                line_offset: None,
                line_limit: Some(2),
                cursor: Some("lines:0".to_string()),
                normalize_ansi: false,
            })
            .expect_err("delta forward windowing should fail");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("mode=delta"));
    }

    #[test]
    fn capture_can_strip_ansi_sequences_before_windowing() {
        let service = make_service(MockAdapter::capture_sequence(&[
            "ready\n",
            "\u{1b}[31mred\u{1b}[0m\nplain\n",
        ]));
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                line_offset: None,
                line_limit: Some(1),
                cursor: None,
                normalize_ansi: true,
            })
            .expect("normalized capture should succeed");

        assert_eq!(response.capture.content, "red\n");
        assert!(response.capture.ansi_normalized);
        assert_eq!(response.capture.next_cursor.as_deref(), Some("lines:1"));
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
                target: None,
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
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
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
                target: None,
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
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
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
                target: None,
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
                input_mode: None,
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
                target: None,
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
                input_mode: None,
                submit: false,
            })
            .expect("send should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent[0].0, "\u{1b}[A\u{1b}\t");
        assert!(!sent[0].1);
    }

    #[test]
    fn send_supports_extended_navigation_and_function_keys() {
        let adapter = MockAdapter::single_capture("baseline");
        let sent_inputs = adapter.sent_inputs.clone();
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                keys: vec![
                    "home".to_string(),
                    "end".to_string(),
                    "page_up".to_string(),
                    "f5".to_string(),
                    "shift_tab".to_string(),
                ],
                input_mode: Some(InputMode::Raw),
                submit: false,
            })
            .expect("send should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent[0].0, "\u{1b}[H\u{1b}[F\u{1b}[5~\u{1b}[15~\u{1b}[Z");
        assert!(!sent[0].1);
    }

    #[test]
    fn send_supports_generic_ctrl_chords() {
        let adapter = MockAdapter::single_capture("baseline");
        let sent_inputs = adapter.sent_inputs.clone();
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                keys: vec!["ctrl_a".to_string(), "ctrl_z".to_string()],
                input_mode: Some(InputMode::Raw),
                submit: false,
            })
            .expect("send should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent[0].0, "\u{1}\u{1a}");
        assert!(!sent[0].1);
    }

    #[test]
    fn send_rejects_unknown_special_key() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                input_mode: None,
                submit: false,
            })
            .expect_err("unknown key should fail");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn explicit_raw_input_mode_rejects_submit_flag() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let error = service
            .send(SendRequest {
                handle: attach.handle,
                text: "q".to_string(),
                keys: Vec::new(),
                input_mode: Some(InputMode::Raw),
                submit: true,
            })
            .expect_err("raw mode should reject submit");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn explicit_submit_line_mode_rejects_named_keys() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let error = service
            .send(SendRequest {
                handle: attach.handle,
                text: "ls".to_string(),
                keys: vec!["enter".to_string()],
                input_mode: Some(InputMode::SubmitLine),
                submit: false,
            })
            .expect_err("submit_line mode should reject named keys");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn explicit_submit_line_mode_submits_without_legacy_flag() {
        let adapter = MockAdapter::capture_sequence(&["baseline", "prompt> npx"]);
        let sent_inputs = adapter.sent_inputs.clone();
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        service
            .send(SendRequest {
                handle: attach.handle,
                text: "echo ok".to_string(),
                keys: Vec::new(),
                input_mode: Some(InputMode::SubmitLine),
                submit: false,
            })
            .expect("submit_line mode should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert!(
            sent[0]
                .0
                .starts_with("\u{15}printf '__ZELLIJ_MCP_INTERACTION__:start:")
        );
        assert!(sent[0].0.contains("echo ok"));
        assert!(sent[0].0.contains("__ZELLIJ_MCP_INTERACTION__:end:"));
        assert!(sent[0].1);
    }

    #[test]
    fn send_submit_resets_command_boundary() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let spawn = service
            .spawn(SpawnRequest {
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
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
                input_mode: None,
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
                target: None,
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
                input_mode: None,
                submit: true,
            })
            .expect("send should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0]
                .0
                .starts_with("\u{15}printf '__ZELLIJ_MCP_INTERACTION__:start:")
        );
        assert!(sent[0].0.contains("echo hello"));
        assert!(sent[0].0.contains("__ZELLIJ_MCP_INTERACTION__:end:"));
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
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
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
                input_mode: None,
                submit: true,
            })
            .expect("send should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0]
                .0
                .starts_with("printf '__ZELLIJ_MCP_INTERACTION__:start:")
        );
        assert!(sent[0].0.contains("echo hello"));
        assert!(sent[0].0.contains("__ZELLIJ_MCP_INTERACTION__:end:"));
        assert!(sent[0].1);
    }

    #[test]
    fn attached_keys_only_send_does_not_clear_pending_input_before_send() {
        let adapter = MockAdapter::capture_sequence(&["baseline"]);
        let sent_inputs = adapter.sent_inputs.clone();
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                input_mode: None,
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
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
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
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::NewTab,
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
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
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
    fn remote_spawn_wait_ready_timeout_returns_busy_handle_with_remote_target_id() {
        let mut adapter = MockAdapter::single_capture("ready");
        adapter.wait_times_out = true;
        let service = make_remote_service("ssh:a100", adapter);

        let response = service
            .spawn(SpawnRequest {
                target: Some("a100".to_string()),
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::NewTab,
                tab_name: Some("editor".to_string()),
                cwd: Some("/tmp".to_string()),
                command: Some("lazygit".to_string()),
                argv: None,
                title: Some("lg".to_string()),
                wait_ready: true,
            })
            .expect("remote spawn should return a busy handle after wait timeout");

        let bindings = service.registry_store.load().expect("bindings should load");

        assert_eq!(response.status, "busy");
        assert_eq!(response.target_id, "ssh:a100");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].handle, response.handle);
        assert_eq!(bindings[0].target_id, "ssh:a100");
        assert_eq!(bindings[0].status, TerminalStatus::Busy);
    }

    #[test]
    fn remote_spawn_capture_failure_returns_busy_handle_with_remote_target_id() {
        let mut adapter = MockAdapter::single_capture("ready");
        adapter.capture_fails = true;
        let service = make_remote_service("ssh:a100", adapter);

        let response = service
            .spawn(SpawnRequest {
                target: Some("a100".to_string()),
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("remote spawn should keep a recoverable busy handle after capture failure");

        let bindings = service.registry_store.load().expect("bindings should load");
        let observations = service
            .observation_store
            .load()
            .expect("observations should load");

        assert_eq!(response.status, "busy");
        assert_eq!(response.target_id, "ssh:a100");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].handle, response.handle);
        assert_eq!(bindings[0].target_id, "ssh:a100");
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
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::NewTab,
                tab_name: Some("editor".to_string()),
                cwd: Some("/tmp".to_string()),
                command: Some("lazygit".to_string()),
                argv: None,
                title: Some("lg".to_string()),
                wait_ready: true,
            })
            .expect_err("spawn should fail on fatal wait error");

        assert_eq!(error.code, ErrorCode::WaitFailed);
        assert!(
            service
                .registry_store
                .load()
                .expect("bindings should load")
                .is_empty()
        );
        assert!(
            service
                .observation_store
                .load()
                .expect("observations should load")
                .is_empty()
        );
    }

    #[test]
    fn spawn_fatal_post_launch_target_loss_cleans_up_persisted_state() {
        let mut adapter = MockAdapter::single_capture("ready");
        adapter.resolve_missing_target = true;
        adapter.capture_missing_target = true;
        let service = make_service(adapter);

        let error = service
            .spawn(SpawnRequest {
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect_err("spawn should fail on fatal post-launch target loss");

        assert_eq!(error.code, ErrorCode::TargetStale);
        assert!(
            service
                .registry_store
                .load()
                .expect("bindings should load")
                .is_empty()
        );
        assert!(
            service
                .observation_store
                .load()
                .expect("observations should load")
                .is_empty()
        );
    }

    #[test]
    fn spawn_fatal_revalidation_error_cleans_up_persisted_state() {
        let mut adapter = MockAdapter::single_capture("ready");
        adapter.resolve_fails = true;
        let service = make_service(adapter);

        let error = service
            .spawn(SpawnRequest {
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect_err("spawn should fail on fatal revalidation error");

        assert_eq!(error.code, ErrorCode::AttachFailed);
        assert!(
            service
                .registry_store
                .load()
                .expect("bindings should load")
                .is_empty()
        );
        assert!(
            service
                .observation_store
                .load()
                .expect("observations should load")
                .is_empty()
        );
    }

    #[test]
    fn spawn_revalidation_prefers_stored_pane_id_when_selector_is_legacy_raw_form() {
        let root = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        let service = make_service_with_root(MockAdapter::single_capture("ready"), root.clone());

        let spawned = service
            .spawn(SpawnRequest {
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
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
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
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
                target: None,
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
                target: None,
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
                target: None,
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
                input_mode: None,
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
                target: None,
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
                input_mode: None,
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
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
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
                input_mode: None,
                submit: true,
            })
            .expect("send should retry and succeed");

        assert!(response.accepted);
        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0]
                .0
                .starts_with("printf '__ZELLIJ_MCP_INTERACTION__:start:")
        );
        assert!(sent[0].0.contains("echo retry"));
        assert!(sent[0].0.contains("__ZELLIJ_MCP_INTERACTION__:end:"));
    }

    #[test]
    fn replace_reuses_shell_like_handle_and_starts_new_interaction() {
        let adapter = MockAdapter::capture_sequence(&["baseline", "prompt> ready"]);
        let sent_inputs = adapter.sent_inputs.clone();
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let response = service
            .replace(ReplaceRequest {
                handle: attach.handle.clone(),
                command: "echo swapped".to_string(),
                interrupt: true,
            })
            .expect("replace should succeed");

        let sent = sent_inputs.lock().expect("sent inputs lock should succeed");
        assert_eq!(response.handle, attach.handle);
        assert!(response.replaced);
        assert!(response.interaction_id.is_some());
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, "\u{3}");
        assert!(
            sent[1]
                .0
                .starts_with("\u{15}printf '__ZELLIJ_MCP_INTERACTION__:start:")
        );
        assert!(sent[1].0.contains("echo swapped"));
    }

    #[test]
    fn replace_rejects_non_shell_like_panes() {
        let mut adapter = MockAdapter::single_capture("baseline");
        adapter.target.command = Some("python".to_string());
        adapter.list_targets = vec![adapter.target.clone()];
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                target: None,
                session_name: "gpu".to_string(),
                tab_name: Some("editor".to_string()),
                selector: "id:terminal:7".to_string(),
                alias: None,
            })
            .expect("attach should succeed");

        let error = service
            .replace(ReplaceRequest {
                handle: attach.handle,
                command: "echo no".to_string(),
                interrupt: true,
            })
            .expect_err("replace should reject non-shell panes");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn cleanup_dry_run_reports_matching_handles_without_deleting() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                target: None,
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

        let response = service
            .cleanup(CleanupRequest {
                target: None,
                statuses: vec![TerminalStatus::Closed],
                max_age_ms: None,
                dry_run: true,
            })
            .expect("cleanup dry run should succeed");

        let bindings = service.registry_store.load().expect("bindings should load");
        assert_eq!(response.removed_count, 1);
        assert_eq!(response.removed_handles, vec![attach.handle]);
        assert_eq!(bindings.len(), 1);
    }

    #[test]
    fn cleanup_removes_closed_bindings_and_observations() {
        let service = make_service(MockAdapter::single_capture("baseline"));
        let attach = service
            .attach(AttachRequest {
                target: None,
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

        let response = service
            .cleanup(CleanupRequest {
                target: None,
                statuses: vec![TerminalStatus::Closed],
                max_age_ms: None,
                dry_run: false,
            })
            .expect("cleanup should succeed");

        let bindings = service.registry_store.load().expect("bindings should load");
        let observations = service
            .observation_store
            .load()
            .expect("observations should load");
        assert_eq!(response.removed_count, 1);
        assert_eq!(bindings.len(), 0);
        assert_eq!(observations.len(), 0);
    }

    #[test]
    fn current_capture_prefers_explicit_interaction_output_when_present() {
        let root = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        let setup = make_service_with_root(MockAdapter::single_capture("ready"), root.clone());
        let spawned = setup
            .spawn(SpawnRequest {
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("spawn should succeed");

        let mut observations = setup
            .observation_store
            .load()
            .expect("observations should load");
        observations[0].start_interaction("zi_test".to_string(), Utc::now());
        setup
            .observation_store
            .save(&observations)
            .expect("observations should save");

        let follow_up = make_service_with_root(
            MockAdapter::capture_sequence(&[
                "prompt> old\n__ZELLIJ_MCP_INTERACTION__:start:zi_test\nhello\nworld\n__ZELLIJ_MCP_INTERACTION__:end:zi_test:0\nprompt> done\n",
            ]),
            root,
        );

        let response = follow_up
            .capture(CaptureRequest {
                handle: spawned.handle,
                mode: CaptureMode::Current,
                tail_lines: None,
                line_offset: None,
                line_limit: None,
                cursor: None,
                normalize_ansi: false,
            })
            .expect("current capture should succeed");

        assert_eq!(response.capture.content, "hello\nworld\n");
        assert_eq!(
            response.capture.baseline.as_deref(),
            Some("interaction_marker")
        );
        assert_eq!(response.capture.interaction_id.as_deref(), Some("zi_test"));
        assert_eq!(response.capture.interaction_completed, Some(true));
        assert_eq!(response.capture.interaction_exit_code, Some(0));
    }

    #[test]
    fn wait_reports_explicit_interaction_completion_when_present() {
        let root = std::env::temp_dir().join(format!("zellij-mcp-test-{}", uuid::Uuid::new_v4()));
        let setup = make_service_with_root(MockAdapter::single_capture("ready"), root.clone());
        let spawned = setup
            .spawn(SpawnRequest {
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
                tab_name: Some("editor".to_string()),
                cwd: None,
                command: Some("fish".to_string()),
                argv: None,
                title: None,
                wait_ready: false,
            })
            .expect("spawn should succeed");

        let mut observations = setup
            .observation_store
            .load()
            .expect("observations should load");
        observations[0].start_interaction("zi_wait".to_string(), Utc::now());
        setup
            .observation_store
            .save(&observations)
            .expect("observations should save");

        let follow_up = make_service_with_root(
            MockAdapter::capture_sequence(&[
                "__ZELLIJ_MCP_INTERACTION__:start:zi_wait\nfinished\n__ZELLIJ_MCP_INTERACTION__:end:zi_wait:17\n",
            ]),
            root,
        );

        let response = follow_up
            .wait(WaitRequest {
                handle: spawned.handle,
                idle_ms: 100,
                timeout_ms: 1_000,
            })
            .expect("wait should succeed");

        assert_eq!(response.status, "idle");
        assert_eq!(
            response.completion_basis.as_deref(),
            Some("interaction_marker")
        );
        assert_eq!(response.interaction_id.as_deref(), Some("zi_wait"));
        assert_eq!(response.interaction_completed, Some(true));
        assert_eq!(response.interaction_exit_code, Some(17));
    }

    #[test]
    fn wait_marks_binding_stale_when_target_disappears() {
        let mut adapter = MockAdapter::single_capture("baseline");
        adapter.wait_missing_target = true;
        let capture_failures_remaining = adapter.capture_failures_remaining.clone();
        let service = make_service(adapter);
        let attach = service
            .attach(AttachRequest {
                target: None,
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
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
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
                target: None,
                session_name: "gpu".to_string(),
                spawn_target: SpawnTarget::ExistingTab,
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
                target: None,
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
                target: None,
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
                target: None,
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
                target: None,
                session_name: Some("gpu".to_string()),
            })
            .expect("list should succeed");

        assert_eq!(listed.bindings[0].status, TerminalStatus::Stale);
    }
}
