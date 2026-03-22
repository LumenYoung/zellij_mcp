# MCP Contract

## Principles

- expose stable daemon-native tools rather than raw Zellij actions
- identify terminals by daemon handle, not by raw pane id
- keep phase 1 tool semantics small and explicit
- document degraded behavior where the backend cannot provide exact semantics

## Transport

- the daemon is exposed as a real MCP stdio server through `rmcp`
- `mcp2cli --mcp-stdio "cargo run --quiet --manifest-path /path/to/Cargo.toml" --list` is the intended local integration path
- each MCP tool returns its structured result serialized as JSON text content, which keeps the transport layer thin while remaining readable to `mcp2cli`
- MCP error responses preserve daemon error details in their `data` payload with stable fields: `code`, `message`, and `retryable`

## Tools

### `zellij_spawn`

Create a managed execution target.

Input:

```json
{
  "session_name": "gpu",
  "target": "new_tab",
  "tab_name": "editor",
  "cwd": "/home/yang/Documents/git/project",
  "command": "nvim",
  "title": "main-editor",
  "wait_ready": true
}
```

Notes:

- `target` supports `new_tab` and `existing_tab`
- `existing_tab` means spawn a new dedicated pane inside the tab
- phase 1 does not replace existing processes in an existing pane
- `command` is parsed with shell-style quoting, so inputs like `bash -lc 'echo hello world'` preserve the intended argv shape
- malformed shell quoting in `command` fails early as an argument parse error instead of spawning a mangled command
- `wait_ready=true` currently runs the same rendered-screen idle check as `zellij_wait`; it works for shell-like startup and was live-tested with `lazygit`, but redraw-heavy TUIs may still make it a noisy readiness proxy

Response:

```json
{
  "handle": "zh_...",
  "session_name": "gpu",
  "tab_name": "editor",
  "selector": "id:terminal:7",
  "status": "ready"
}
```

### `zellij_attach`

Attach a live pane to daemon management.

Input:

```json
{
  "session_name": "gpu",
  "tab_name": "editor",
  "selector": "title:main-editor",
  "alias": "main-editor"
}
```

Notes:

- the selector must resolve to exactly one target
- attach establishes a baseline observation immediately
- attached panes are considered interactive and may already contain user state

Response:

```json
{
  "handle": "zh_...",
  "attached": true,
  "baseline_established": true
}
```

### `zellij_send`

Send input to a managed pane.

Input:

```json
{
  "handle": "zh_...",
  "text": ":w",
  "keys": ["escape", "up"],
  "submit": true
}
```

Notes:

- `submit=true` indicates the daemon should treat this as the start of a new interaction boundary for `current` capture mode
- `keys` is optional and supports named special inputs: `enter`, `tab`, `escape`/`esc`, `up`, `down`, `left`, `right`, `backspace`, and `ctrl_c`
- `text` and `keys` can be combined in one request; special keys are translated to terminal byte sequences before dispatch
- `submit=false` is suitable for raw printable input into an interactive TUI, for example sending `q` to quit `lazygit`
- printable input is verified in `lazygit`, and named `up`/`escape` key sequences have been verified live through a raw terminal pane
- function keys and richer modified key combinations are still out of scope for phase 1

### `zellij_wait`

Wait for idle or timeout.

Input:

```json
{
  "handle": "zh_...",
  "idle_ms": 1200,
  "timeout_ms": 30000
}
```

Response:

```json
{
  "handle": "zh_...",
  "status": "idle",
  "observed_at": "2026-03-22T00:00:00Z"
}
```

Notes:

- `zellij_wait` uses `zjctl pane wait-idle`, so it observes rendered-screen stability rather than process completion
- this has been verified live for a spawned `lazygit` pane, but should still be read as an idle heuristic rather than an exact app-ready guarantee

### `zellij_capture`

Capture output from a managed pane.

Input:

```json
{
  "handle": "zh_...",
  "mode": "full"
}
```

Supported modes:

- `full`: latest complete capture available from the backend
- `delta`: textual difference against the previous successful capture for the same handle
- `current`: best-effort textual difference against the command boundary established by spawn, attach, or `send(submit=true)`

Response:

```json
{
  "handle": "zh_...",
  "mode": "delta",
  "content": "new lines...",
  "truncated": false,
  "captured_at": "2026-03-22T00:00:00Z",
  "baseline": "last_capture"
}
```

Semantics:

- first `delta` call may return the full captured content and initialize the baseline
- `current` is best-effort and may over-include prior output if the pane was already active before attach

### `zellij_close`

Close a managed pane.

Input:

```json
{
  "handle": "zh_...",
  "force": false
}
```

Notes:

- `force=true` is required when the target pane is focused and the backend would otherwise refuse to close it
- close keeps the binding in the registry with `status="closed"` and removes the active observation snapshot

### `zellij_list`

List known bindings.

Input:

```json
{
  "session_name": "gpu"
}
```

## Stable Error Codes

- `INVALID_ARGUMENT`
- `HANDLE_NOT_FOUND`
- `ALIAS_NOT_FOUND`
- `SELECTOR_NOT_UNIQUE`
- `TARGET_NOT_FOUND`
- `TARGET_STALE`
- `SPAWN_FAILED`
- `ATTACH_FAILED`
- `SEND_FAILED`
- `WAIT_TIMEOUT`
- `WAIT_FAILED`
- `CAPTURE_FAILED`
- `CLOSE_FAILED`
- `ZJCTL_UNAVAILABLE`
- `PLUGIN_NOT_READY`
- `PERSISTENCE_ERROR`

## Phase 1 Guarantees

- all tools accept and return daemon-native data structures
- all managed terminals can be listed and revalidated after restart
- full capture is the primary source of truth
- delta and current modes are derived from daemon snapshots
- `send` can deliver printable text input to an attached interactive pane
- `send` supports a basic named-key layer for common control sequences in addition to printable text
- `spawn`, `wait`, and `close` are available through the daemon and have been verified against a real Zellij session
- `spawn` preserves quoted command arguments using shell-aware parsing

## Phase 1 Limitations

- capture does not promise true shell process boundaries
- delta mode is snapshot-based, not scrollback-cursor-based
- attach to an existing pane may include pre-existing output in the first current capture
- no phase 1 tool manages layout or pane scheduling
- the backend plugin must be loaded in the target session and its permission prompt approved before RPC-backed operations can succeed
- full-screen TUIs may redraw large portions of the screen, so `delta` and `current` may over-report changes compared with shell-style output
