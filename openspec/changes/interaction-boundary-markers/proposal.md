## Why

The current capture model is still fundamentally heuristic for submitted shell commands. `current` uses the last command boundary snapshot and repaint normalization, while `wait` still depends on rendered-idle behavior plus capture polling. That works for many flows, but it does not give the daemon an explicit notion of “this submitted interaction started here” or “this submitted interaction completed and all output has drained.”

Phase 2 should tighten that gap for shell-like panes by adding explicit daemon-owned interaction boundaries where the daemon can do so safely, while preserving the current best-effort behavior for panes that are not eligible for shell wrapping.

## What Changes

- Add explicit interaction boundary state to observations so the daemon can distinguish legacy command-boundary snapshots from explicit shell interaction markers.
- Teach `zellij_send` shell-submit flows on supported shell-like panes to wrap the submitted command with daemon-owned boundary markers.
- Let `capture(current)` prefer explicit interaction boundaries when present.
- Let `wait` surface explicit completion state when a marked interaction has completed, while preserving idle-based fallback behavior for unmarked panes.

## Capabilities

### New Capabilities
- `interaction-boundary-markers`: explicit shell interaction markers for stronger current-capture and completion reporting.

### Modified Capabilities
- None.

## Impact

- Affected code: `src/domain/observation.rs`, `src/domain/responses.rs`, `src/services/terminal.rs`, and likely small request/adapter seams required to preserve shell-submit behavior.
- Affected verification: targeted terminal-service tests, any adapter tests needed for shell-wrapper construction, and full `cargo test`.
- Affected docs: `docs/mcp-contract.md`, `docs/architecture.md`, and `docs/phase-2-backlog.md`.
