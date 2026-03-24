## Context

Today the daemon tracks `last_full_*` snapshots and a `command_boundary_*` snapshot in `TerminalObservation`. Submitted shell commands reset the boundary, and later `capture(current)` returns the suffix after that boundary unless the content is repaint-heavy.

That gives useful best-effort behavior, but it has two known limits:

- the boundary is inferred from a pre-send snapshot rather than an explicit runtime marker
- `wait` can tell when the pane is visually idle, but not when a daemon-submitted interaction has explicitly completed

## Goals / Non-Goals

**Goals:**
- Add explicit interaction markers for shell-like panes when the daemon itself submits the command.
- Improve `current` capture and `wait` reporting when those explicit markers exist.
- Keep legacy best-effort behavior for attached panes or shells the daemon cannot safely wrap.

**Non-Goals:**
- No claim of universal process completion for every pane type.
- No change to raw/TUI input semantics from the interactive-input slice.
- No new queueing/orchestration behavior in this change.

## Decisions

### 1. Explicit boundaries are opt-in by daemon capability, not assumed universally
Only shell-submit flows on panes whose shell is known and wrappable should use explicit markers.

If a pane cannot be wrapped safely, the daemon should keep the existing boundary-snapshot and idle-based behavior rather than pretending explicit completion exists.

### 2. Explicit interaction state lives in observations
Extend `TerminalObservation` with explicit interaction metadata rather than inventing a separate store.

That keeps all capture/current/wait reasoning in the same persisted handle-local state that already owns full snapshots and command-boundary snapshots.

### 3. Current capture prefers explicit interaction output when present
When a capture contains an explicit marker-delimited interaction for the current handle, `capture(current)` should return the interaction output instead of suffix-after-boundary heuristics.

When the interaction is not explicitly marked, the daemon should keep the current fallback behavior.

### 4. Wait reports explicit completion when available
`wait` should continue to use idle probing as the transport-level liveness check, but when an explicit interaction marker is present it should report whether that interaction actually completed.

This keeps “pane looks idle” separate from “daemon-submitted interaction completed,” which is the safer boundary.

## Risks / Trade-offs

- Shell wrapper syntax varies. This change should support only clearly recognized shell families and preserve legacy behavior elsewhere.
- Explicit completion markers improve submitted-command flows, but they do not automatically solve third-party output or human-typed commands that bypass the daemon.

## Migration Plan

1. Add explicit interaction state and response fields.
2. Add tests for marked versus unmarked current-capture and wait behavior.
3. Add wrapper construction for supported shell-submit flows.
4. Update docs and backlog once the marked interaction path is verified.
