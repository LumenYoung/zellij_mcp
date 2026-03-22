# Architecture

## Goal

Build a Rust MCP daemon that exposes a small, stable Zellij control surface to agents through `mcp2cli`.

The daemon owns agent-facing handles, persistence, and capture semantics. It does not expose raw `zellij action` primitives. The daemon delegates terminal targeting and low-level interaction to `zjctl`.

## Phase 1 Scope

Phase 1 implements:

- `spawn` into a new tab or an existing tab
- `attach` to an existing pane
- `send` follow-up input to a managed pane
- `wait` using `zjctl wait-idle`
- `capture` in `full`, `delta`, and best-effort `current` modes
- `close` a managed pane
- `list` managed handles
- lightweight persistence for bindings and observation metadata

Phase 1 does not implement automatic pane scheduling, pane replacement, layout management, or external message bridges.

## System Layers

```text
agent
  -> mcp2cli
    -> zellij MCP daemon
      -> tool handlers
        -> domain services
          -> zjctl adapter
            -> zjctl + Zellij plugin
```

### MCP transport

The transport layer registers tools and translates daemon-native request and response types to an MCP-compatible shape.

### Tool handlers

Handlers validate input, resolve a handle or selector, call the appropriate domain service, and return structured output with stable error codes.

### Domain services

Services own business semantics:

- spawn vs attach behavior
- command boundary resets
- delta and current capture behavior
- stale target revalidation
- per-handle serialization

### Zjctl adapter

The adapter is the only layer allowed to spawn `zjctl` or parse its output. It provides a typed interface for:

- spawning targets
- resolving selectors
- sending input
- waiting for idle
- capturing pane content
- closing targets
- listing visible targets

### Persistence

Persistence stores lightweight daemon state in JSON files under a local state directory. The daemon persists bindings and capture metadata, then revalidates them against live Zellij state on startup.

## Primary Objects

### Terminal handle

The daemon returns a stable handle to the agent. The handle is the only durable identifier exposed through MCP.

### Terminal binding

A binding maps a daemon handle to a live Zellij target.

Fields include:

- handle
- alias
- session name
- tab name
- selector
- pane id if known
- cwd if known
- launch command if known
- source (`spawned` or `attached`)
- status (`ready`, `busy`, `stale`, `closed`)

### Terminal observation

An observation stores capture state for a handle.

Fields include:

- last full content and hash
- last capture timestamp
- command boundary content and hash
- command boundary timestamp

## Capture Modes

### full

Returns the current content that can be captured from the pane. This is the most stable mode and the basis for the other capture modes.

### delta

Returns the textual difference between the latest full capture and the last successful capture for the same handle. This is snapshot-diff semantics rather than true scrollback cursor semantics.

### current

Returns the best-effort textual difference between the latest full capture and the current command boundary. The daemon resets the command boundary when:

- a handle is spawned
- a handle is attached
- `send` is called with `submit=true`

This is a best-effort interaction boundary, not a true process stdout boundary.

## Persistence and Recovery

Suggested storage files:

- `registry.json`
- `observations.json`

On startup the daemon:

1. loads persisted bindings and observations
2. validates each binding against live Zellij state via the adapter
3. restores valid bindings
4. marks missing targets as stale

## Concurrency Model

Operations are serialized per handle. Different handles may execute in parallel.

This avoids races such as:

- sending input while a capture is in progress
- closing a pane while `wait` is running
- mixing two `send` requests into a single interactive pane

## Error Model

The daemon returns stable domain errors rather than backend-specific command output. Core error codes include:

- `HANDLE_NOT_FOUND`
- `SELECTOR_NOT_UNIQUE`
- `TARGET_NOT_FOUND`
- `TARGET_STALE`
- `SPAWN_FAILED`
- `ATTACH_FAILED`
- `SEND_FAILED`
- `WAIT_TIMEOUT`
- `CAPTURE_FAILED`
- `CLOSE_FAILED`
- `ZJCTL_UNAVAILABLE`
- `PLUGIN_NOT_READY`
- `PERSISTENCE_ERROR`

## Validation Plan

Phase 1 should validate these scenarios:

1. spawn a new tab, wait, and capture full output
2. attach to an existing pane, send follow-up input, wait, and capture delta
3. attach to an existing pane and use `current` across multiple submit cycles
4. restart the daemon and verify that handles are either restored or marked stale
5. run two handles in parallel without cross-handle interference
