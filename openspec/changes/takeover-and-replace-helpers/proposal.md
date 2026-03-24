## Why

Phase 1 already supports takeover in two manual steps: discover candidates, then attach by selector. It also lets agents reuse an existing managed handle by sending more input, but it has no explicit helper for “take over the right existing pane now” or “reuse this supported shell pane for a new command now.”

This bucket should land as helper-level ergonomics inside the existing single-daemon model rather than as a universal process-replacement system.

## What Changes

- Add an auto-takeover helper that searches existing panes in a session and attaches the unique match in one step.
- Add a cooperative replace helper for supported shell-like managed panes that interrupts the current interaction and submits a new shell command on the same handle.
- Preserve the existing manual discover/attach flow and plain send path.

## Capabilities

### New Capabilities
- `takeover-and-replace-helpers`: attach/search helper and cooperative replace helper for supported panes.

### Modified Capabilities
- None.

## Impact

- Affected code: request/response shapes, `src/services/terminal.rs`, router forwarding, and MCP tool registration.
- Affected verification: terminal-service and MCP/router tests plus full `cargo test`.
- Affected docs: `docs/mcp-contract.md` and `docs/phase-2-backlog.md`.
