# Phase 2 Backlog

## Deferred Features

The following features are intentionally deferred to phase 2 or later.

### Stronger current-capture semantics

- explicit command markers
- shell integration for command boundary detection
- wrapper-based completion boundaries

### Better completion detection

- explicit completion marker files
- wrapper scripts for long-running agents
- process-aware completion hints beyond idle polling
- stronger readiness semantics for redraw-heavy TUIs where `wait-idle` is useful but not a perfect startup signal

### Richer interactive input

- expanded support for special keys beyond the current basic set, especially function keys and modified chords
- a clearer tool contract for TUI-oriented input versus shell command submission
- optional key-sequence encoding instead of plain text only

### Pane replacement and takeover

- replace a running pane process with a new command
- controlled reuse of an existing pane without manual attach semantics
- higher-level takeover helpers that search candidate panes and attach automatically; manual attach-by-selector already exists in phase 1

### Richer orchestration

- queueing and deferred spawn
- cleanup policies for stale or idle panes
- OMO-style manager behaviors such as eviction and background coordination

### Layout and targeting enhancements

- layout inspection and mutation
- richer selector support surfaced directly to tool inputs beyond the current discover-plus-attach flow
- safer multi-client focus handling

### Improved output semantics

- scrollback-aware delta extraction
- larger output windows with chunking and resume cursors
- ANSI-aware normalization controls
- TUI-aware diffing that handles redraw-heavy screens more gracefully than simple prefix subtraction
- richer preview and clipping controls beyond the current `preview_lines` and `tail_lines` line windows

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
