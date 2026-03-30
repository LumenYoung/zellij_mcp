## ADDED Requirements

### Requirement: Binary embeds the canonical fish wrapper implementation
The system SHALL carry the canonical fish wrapper implementation in the built binary so wrapped fish command execution does not depend on the repo being present at runtime.

#### Scenario: Runtime works without repo checkout
- **WHEN** the daemon runs on a machine where the repo checkout and wrapper script are not present on disk
- **THEN** the daemon still has access to the canonical fish wrapper implementation needed for wrapped fish command execution

### Requirement: Existing wrapper definition is validated before reuse
The system SHALL validate that an already-defined `__zellij_mcp_run_b64` function matches the binary-owned canonical wrapper identity before reusing it for wrapped command execution.

#### Scenario: Existing wrapper matches canonical identity
- **WHEN** the target fish shell already defines `__zellij_mcp_run_b64` and that definition reports the same canonical hash or version as the binary-owned wrapper
- **THEN** the daemon reuses the existing function without redefining it

#### Scenario: Existing wrapper does not match canonical identity
- **WHEN** the target fish shell already defines `__zellij_mcp_run_b64` but that definition reports a different canonical hash or version than the binary-owned wrapper
- **THEN** the daemon treats the existing function as stale and does not trust it for direct reuse

### Requirement: Wrapper is lazily bootstrapped when missing or stale
The system SHALL define the canonical fish wrapper in the target shell only when the wrapper is missing or fails canonical identity validation.

#### Scenario: Wrapper missing from target shell
- **WHEN** the target fish shell does not define `__zellij_mcp_run_b64`
- **THEN** the daemon defines the canonical wrapper before invoking the wrapped command

#### Scenario: Wrapper stale in target shell
- **WHEN** the target fish shell defines `__zellij_mcp_run_b64` with a non-canonical hash or version
- **THEN** the daemon replaces that definition with the canonical wrapper before invoking the wrapped command

### Requirement: Bootstrap avoids unconditional per-command redefinition
The system SHALL NOT resend the canonical wrapper definition on every wrapped fish command once the target shell already has the canonical wrapper definition.

#### Scenario: Consecutive wrapped commands in same shell
- **WHEN** a wrapped fish command has already bootstrapped the canonical wrapper into a target shell and a later wrapped fish command is sent to the same shell
- **THEN** the later command reuses the existing validated wrapper instead of redefining it again

### Requirement: Bootstrap failure preserves command execution fallback
The system SHALL retain a correctness fallback when wrapper validation or bootstrap fails so wrapped commands still execute.

#### Scenario: Bootstrap path fails
- **WHEN** wrapper validation, definition, or invocation through the clean bootstrap path fails at runtime
- **THEN** the daemon retries using the legacy inline wrapper path instead of failing the wrapped command outright
