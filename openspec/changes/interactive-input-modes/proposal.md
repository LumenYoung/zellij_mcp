## Why

The current `zellij_send` contract mixes two different intents behind one `submit` boolean: shell-style line submission and raw TUI-style input. It also already has a lightweight key-sequence mechanism through `keys`, but the supported named keys are too narrow for many interactive applications.

Phase 2 should make that input contract more explicit without breaking the existing MCP surface: callers should be able to say when they want raw terminal input versus shell-style line submission, and the daemon should support a broader named-key vocabulary for common navigation, function keys, and control chords.

## What Changes

- Add an explicit `input_mode` option for `zellij_send` so shell-submit and raw/TUI input are no longer inferred only from `submit`.
- Expand named-key support beyond the current basic set to include extended navigation keys, function keys, and generic `ctrl_<letter>` chords.
- Preserve backward compatibility for existing callers that only use `text`, `keys`, and `submit`.
- Update tests and docs so the richer interactive input contract is specified and verified.

## Capabilities

### New Capabilities
- `interactive-input-modes`: explicit input-mode semantics and broader named-key support for `zellij_send`.

### Modified Capabilities
- None.

## Impact

- Affected code: `src/domain/requests.rs`, `src/services/terminal.rs`, and any MCP serialization/docs surfaces that describe send semantics.
- Affected verification: targeted terminal-service tests plus full `cargo test`.
- Affected docs: `docs/mcp-contract.md` and `docs/phase-2-backlog.md`.
