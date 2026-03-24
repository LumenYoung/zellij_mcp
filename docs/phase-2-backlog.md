# Phase 2 Backlog

## Deferred Features

The following features are intentionally deferred to phase 2 or later.

### Stronger current-capture semantics

Landed in `openspec/changes/interaction-boundary-markers` with scoped behavior:

- explicit daemon-owned interaction markers for daemon-submitted shell commands on supported shell-like panes
- `capture(current)` now prefers explicit interaction output when those markers are present
- unmarked panes and repaint-heavy TUIs still keep the legacy best-effort boundary and visible-frame fallbacks

### Better completion detection

Landed in `openspec/changes/interaction-boundary-markers` with scoped behavior:

- `wait` can now report `completion_basis="interaction_marker"` plus interaction completion metadata for daemon-submitted shell commands on supported shell-like panes
- panes without explicit markers still use the legacy idle-based wait semantics
- redraw-heavy TUI panes still remain heuristic/idle-based rather than claiming exact process completion

### Richer interactive input

Landed in `openspec/changes/interactive-input-modes`:

- expanded named-key support beyond the original basic set, including function keys, navigation keys, and generic `ctrl_<letter>` chords
- an explicit `input_mode` contract for raw terminal input versus shell-style line submission
- continued backward-compatible support for the legacy `submit` behavior when `input_mode` is omitted

### Pane replacement and takeover

Landed in `openspec/changes/takeover-and-replace-helpers` with scoped behavior:

- a one-step `takeover` helper that searches and attaches the unique matching pane instead of requiring manual discover-plus-attach
- a cooperative `replace` helper for supported shell-like managed panes that interrupts the current interaction and submits a new shell command on the same handle
- unsupported panes still reject `replace` rather than pretending the daemon can universally replace arbitrary processes

### Richer orchestration

Landed in `openspec/changes/cleanup-retention-policies` with scoped behavior:

- an explicit cleanup helper for persisted stale/closed pane state
- optional age-based retention filtering and dry-run previews
- broader deferred spawn queueing and background coordination are still intentionally deferred beyond this scoped lifecycle-management slice

### Layout and targeting enhancements

Landed in `openspec/changes/layout-inspection-and-selectors` with scoped behavior:

- richer selector support including `command:`, `tab:`, and focused/unfocused filters
- a grouped `layout` inspection helper that reports panes by tab for a session
- layout mutation and focus-changing side effects remain intentionally out of scope

### Improved output semantics

Landed in `openspec/changes/output-window-cursors` with scoped behavior:

- `capture` now supports forward line windows with `line_offset`, `line_limit`, and resumable `cursor` values in the `lines:<offset>` format for stable semantic modes
- capture responses now return `next_cursor` when additional semantic output remains
- `normalize_ansi=true` strips ANSI control sequences from the already-selected semantic capture output before windowing
- the existing repaint-aware `current` normalization remains the TUI-focused behavior; true scrollback-aware delta extraction and deeper redraw diffing are still intentionally deferred

## Why These Are Deferred

Phase 1 focuses on stable terminal control for a single managed pane at a time. The omitted features are valuable, but they either depend on backend capabilities that require more validation or they expand the daemon from a control plane into a full scheduler.

## Re-entry Conditions

Phase 2 work should begin once phase 1 proves these behaviors in practice:

- attach and reuse of an existing pane works reliably
- full and delta capture are useful in real agent loops
- persistence plus stale revalidation survive daemon restarts
- backend `zjctl` behavior is stable enough to support stronger guarantees

## Current Position

Phase 1 now covers the core control loop:

- spawn a managed pane or attach to an existing pane to create a handle
- discover unmanaged panes before attach when the agent needs to identify an existing job first
- send text or a basic named-key set to that handle
- wait for idle, capture in `full` / `delta` / repaint-aware `current`, and close when appropriate
- restart the daemon and rely on persisted bindings plus stale revalidation

Phase 2 should therefore focus on stronger guarantees and better ergonomics, not on basic daemon control-plane coverage.

One concrete phase-2 slice has now landed: `openspec/changes/remote-reliability-hardening`.

That completed change delivered these SSH-backed reliability and observability improvements:

- daemon freshness metadata at startup, in successful responses, and in MCP error payloads
- deterministic remote backend preload and revalidation after daemon restart
- recoverable `busy` remote spawn semantics for successful-but-unsettled launches
- metadata-preserving discover degradation when remote preview capture fails
- explicit readiness distinctions for missing binaries, helper-client absence, RPC-not-ready drift, and manual plugin approval blockers
- daemon-backed shell harness coverage plus recorded real-host evidence for one known-good and one known-blocked host path
- richer interactive input modes and expanded named-key support for `zellij_send`
- explicit interaction markers for stronger shell-oriented current capture and wait completion reporting
- scoped takeover and cooperative replace helpers for existing managed/supported panes
- scoped cleanup and retention helpers for stale/closed managed pane state
- richer selector support and grouped layout inspection for existing sessions
- scoped output window chunking, resumable line cursors, and optional ANSI normalization for capture output

## What Is Still Left

The completed remote-reliability-hardening change does **not** mean all phase-2-or-later work is done, but the concrete backlog buckets listed above have now all landed in scoped form.

The remaining follow-on work is beyond the currently landed backlog slices:

- deeper output semantics beyond the landed scoped slice, specifically true scrollback-aware delta extraction and richer TUI diffing

There are also follow-on planning questions that remain open after the reliability hardening slice:

- whether SSH connection reuse should become its own separate workstream
- what durable evidence format future real-host QA should use
- whether manual-action-required outcomes need a more formal machine-readable subcategory

So the current state is:

- the SSH-backed remote reliability/diagnostics hardening slice is complete
- the concrete phase-2 backlog buckets in this document are complete in scoped form
- transport redesign, nested remote daemons, and broader orchestration are still intentionally out of scope for the landed change
