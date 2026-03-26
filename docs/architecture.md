# Architecture

## Goal

Build a Rust MCP daemon that exposes a small, stable Zellij control surface to agents over MCP stdio.

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

Current implementation status:

- live `spawn`, `attach`, `send`, `wait`, `capture`, `close`, and managed `list` are implemented and verified against a real Zellij session
- live read-only pane discovery is implemented through `zellij_discover`
- lifecycle state is persisted across `spawn`, `wait`, and `close`, including closed-handle registry updates
- startup and per-operation stale revalidation are implemented for persisted handles
- MCP stdio transport is implemented through `rmcp` and verified through local `mcp2cli`
- named special-key input is implemented for common controls such as arrows, escape, tab, enter, backspace, and ctrl-c
- `current` capture is repaint-aware for redraw-heavy TUIs, while `full` and `delta` keep snapshot semantics
- spawn now supports either shell-style `command` or explicit `argv`
- spawn now persists a provisional handle before post-launch readiness/capture work so a real pane is not lost when follow-up probing degrades

Phase 1 does not implement automatic pane scheduling, pane replacement, layout management, or external message bridges.

## System Layers

```text
agent
  -> local launcher or client
    -> zellij MCP daemon
      -> tool handlers
        -> domain services
          -> zjctl adapter
            -> zjctl + Zellij plugin
```

### MCP transport

The transport layer registers tools and translates daemon-native request and response types to an MCP-compatible shape.

The current implementation now serves a real MCP stdio endpoint through `rmcp`, which is the path expected by local `mcp2cli`. The transport remains thin: each exposed MCP tool forwards into the existing synchronous `McpServer::execute_tool(...)` seam, and tool results are returned as JSON text payloads so `mcp2cli` can print them directly.

That same stdio transport now fronts both local and SSH-backed execution through one local daemon. Selection tools may name a remote target alias, and the daemon routes those requests to an SSH-backed adapter while keeping the MCP transport, handle ownership, and persistence local.

### Tool handlers

Handlers validate input, resolve a handle or selector, call the appropriate domain service, and return structured output with stable error codes.

This layer also supports the takeover flow for an already-running pane: `attach` resolves an existing pane selector, creates a fresh daemon handle for it, captures a baseline immediately, and then lets the agent use normal `send`, `wait`, `capture`, `close`, and `list` operations against that handle.

`discover` now sits before that step as a read-only inspection path. It returns live pane metadata plus a bounded preview so the agent can confirm the right target before creating a durable handle.

### Domain services

Services own business semantics:

- spawn vs attach behavior
- discover vs attach behavior
- command boundary resets
- explicit interaction markers for daemon-submitted shell commands on supported shell-like panes
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

The spawn path now accepts either a shell-style `command` string or an explicit `argv` vector before building `zjctl` argv. String commands still use shell-aware quoting so quoted arguments survive intact, while explicit `argv` bypasses shell parsing completely. Mixed `command` + `argv`, missing both, blank `command`, empty `argv`, and blank `argv[0]` all fail before any pane is launched.

For `target="existing_tab"`, the adapter still uses the normal `zjctl` spawn path. For `target="new_tab"`, the adapter now creates the tab, launches the command via direct `zellij run`, then resolves the spawned pane from the before/after session listing. This avoids the earlier case where a fresh tab could contain the real pane while the older RPC-backed selector handoff was still stalled.

Live verification showed one important runtime constraint: the `zrpc.wasm` plugin must be loaded in the target session and its first-run permission prompt must be approved before `zjctl` RPC calls will succeed.

For SSH-backed deployment, the important boundary is still the same: `zjctl` and `zellij` must execute on the same host as the target Zellij session. The current implementation satisfies that with an SSH-aware adapter inside the local daemon rather than by launching a separate remote MCP daemon.

### Persistence

Persistence stores lightweight daemon state in JSON files under a local state directory. The daemon persists bindings and capture metadata, then revalidates them against live Zellij state on startup.

For supported shell-like panes, observations can now also persist an explicit interaction id plus completion metadata for the most recent daemon-submitted shell interaction. That lets `capture(current)` prefer explicit interaction output and lets `wait` report stronger completion evidence than idle-only polling when the marker is present.

Spawn lifecycle detail:

1. create the pane through the adapter
2. persist a provisional spawned binding and empty observation as `busy`
3. optionally run bounded idle detection when `wait_ready=true`
4. establish a baseline capture when possible
5. upgrade the binding to `ready` once revalidation and capture succeed

If the pane is real but bounded idle detection or baseline capture does not settle cleanly, the daemon returns the handle as `busy` so later `wait`, `capture`, `list`, or `close` can continue from that state. Fatal post-launch errors clean up the provisional state instead of leaving a lost handle behind.

Remote routing note:

- phase 1 remote support keeps daemon persistence semantics local; bindings persist the selected `target_id` so follow-up handle operations route correctly
- the local daemon can construct SSH-backed backends from `ZELLIJ_MCP_TARGETS`
- a truly detached remote daemon reachable without SSH would require a new transport and supervisor story, which is intentionally out of scope

## Primary Objects

### Terminal handle

The daemon returns a stable handle to the agent. The handle is the only durable identifier exposed through MCP.

### Terminal binding

A binding maps a daemon handle to a live Zellij target.

Bindings can come from either a daemon-created pane (`spawned`) or a user-existing pane (`attached`). The attached path is the current answer to agent takeover of an already-running job: find the pane, attach it, get a new handle, then poll and interact through that handle.

Fields include:

- handle
- target id
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

For full-screen TUIs, `full` remains the most conservative mode, while `current` now also has live verification for repaint-heavy screens through its frame-normalization path.

Live lifecycle testing also showed that `wait` works for a spawned `lazygit` pane, but `wait_ready=true` should not be treated as a universal readiness signal for redraw-heavy TUIs. The current spawn contract treats it as a bounded best-effort idle probe, not as the definition of whether the spawn itself succeeded.

### delta

Returns the textual difference between the latest full capture and the last successful capture for the same handle. This is snapshot-diff semantics rather than true scrollback cursor semantics.

### current

Returns the best-effort textual difference between the latest full capture and the current command boundary. The daemon resets the command boundary when:

- a handle is spawned
- a handle is attached
- `send` is called with `submit=true`

This is a best-effort interaction boundary, not a true process stdout boundary.

For attached panes, this means the first handle is created against a pane that may already contain user history. The daemon captures that state as the initial baseline at attach time, so follow-up polling is relative to the moment the agent took over rather than to original process start.

Live testing with `lazygit` showed that printable-key input sent through `zjctl pane send` can manipulate a TUI directly. That makes `send` useful beyond shell commands, but it also makes prefix-based `delta` and `current` extraction less trustworthy for redraw-heavy interfaces.

The current implementation now special-cases repaint-heavy captures for `current`: when the pane content includes clear-screen, home-cursor, or carriage-return style redraws, the daemon normalizes the latest visible frame and returns that snapshot instead of trying raw prefix subtraction. `full` and `delta` remain unchanged so append-only shell flows keep their previous behavior.

The daemon now also supports a small named-key layer for common control sequences such as arrows, escape, tab, enter, backspace, and ctrl-c by translating them to terminal byte sequences before dispatch.

`capture` also supports optional post-selection output shaping. `tail_lines` keeps the existing recent-lines clip, while `line_offset`, `line_limit`, and `cursor` now provide resumable forward line windows over the already-computed `full` or `current` result. `delta` intentionally rejects that forward paging path because the daemon still advances delta baselines after each capture and therefore does not persist a snapshot-stable cursor stream there. Optional ANSI stripping happens on that semantic output just before line windowing, while observation baselines continue to update from the unmodified full snapshot.

## Persistence and Recovery

Suggested storage files:

- `registry.json`
- `observations.json`

On startup the daemon:

1. loads persisted bindings and observations
2. validates each binding against live Zellij state via the adapter
3. restores valid bindings
4. marks missing targets as stale
5. emits daemon freshness metadata to stderr and preloads persisted remote target ids so remote bindings can revalidate deterministically after restart

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
- `PROTOCOL_VERSION_MISMATCH`
- `PERSISTENCE_ERROR`

`PLUGIN_NOT_READY` should cover cases where `zjctl` is installed but the target session still lacks RPC readiness. In practice that includes manual plugin approval, helper-client absence, and post-start RPC drift.

`PROTOCOL_VERSION_MISMATCH` covers the narrower compatibility case where the daemon reaches the loaded `zrpc` plugin but the plugin replies with a different protocol version than the daemon expects. That is a compatibility problem, not transient RPC drift, so the daemon treats it as non-retryable until matching artifacts are loaded.

Successful MCP responses now also carry `_daemon` identity metadata, and MCP error payloads carry the same daemon identity under `data.daemon`, so stale binaries and mixed-instance reports can be diagnosed without guessing.

## Validation Plan

Phase 1 should validate these scenarios:

1. spawn a new tab, wait, and capture full output
2. attach to an existing pane, send follow-up input, wait, and capture delta
3. attach to an existing pane and use `current` across multiple submit cycles
4. restart the daemon and verify that handles are either restored or marked stale
5. run two handles in parallel without cross-handle interference

Verified so far:

1. a fresh session can host the `zrpc` plugin after approval and pass `zjctl doctor`
2. a `lazygit` pane can be attached, listed, captured, and controlled with printable-key input through the daemon
3. a new managed pane can be spawned, waited on, and closed through the daemon, with closed status persisted in local state
4. named `up` and `escape` key input have been verified through a real pane using the daemon's special-key send path
5. `mcp2cli --mcp-stdio` can initialize the daemon, list tools, and call `zellij_list` against the stdio transport
6. `current` capture against a live `btop` pane now returns a normalized readable screen snapshot after redraws instead of ANSI-heavy prefix noise
7. both string-command and explicit-argv spawn forms are validated and covered by tests, with invalid input rejected before any Zellij tab action runs
8. attach already supports taking over an existing pane by selector and converting it into a managed daemon handle
9. `zellij_discover` has been verified live in a non-`gpu` session and returns command, focus state, and bounded preview text for candidate panes
10. timeout-prone `new_tab` spawn now returns a usable busy handle instead of hanging externally, and the resulting handle can be revalidated, captured, and closed in a later step
11. the daemon can route operations to a configured remote host over SSH without changing the MCP tool contract
