## Why

The daemon already persists pane bindings and observations, but phase 1 still leaves cleanup as an implicit side effect of close/failure paths. Phase 2 should add an explicit cleanup/retention helper so stale and closed pane state can be pruned intentionally instead of accumulating forever.

## What Changes

- Add a cleanup helper that removes matching stale/closed pane state for a target.
- Support age-based retention and dry-run reporting.
- Keep cleanup scoped to persisted daemon state rather than background scheduling.

## Impact

- Affected code: request/response shapes, `TerminalService`, router forwarding, MCP tools, and persistence updates.
- Affected docs: `docs/mcp-contract.md`, `docs/phase-2-backlog.md`.
