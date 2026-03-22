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
  "argv": null,
  "title": "main-editor",
  "wait_ready": true
}
```

Explicit argv form:

```json
{
  "session_name": "gpu",
  "target": "existing_tab",
  "tab_name": "editor",
  "cwd": "/home/yang/Documents/git/project",
  "argv": ["bash", "-lc", "printf 'hello from argv\\n'"],
  "title": "argv-demo",
  "wait_ready": true
}
```

Notes:

- `target` supports `new_tab` and `existing_tab`
- `existing_tab` means spawn a new dedicated pane inside the tab
- phase 1 does not replace existing processes in an existing pane
- use either `command` or `argv`, not both
- `command` is parsed with shell-style quoting, so inputs like `bash -lc 'echo hello world'` preserve the intended argv shape
- `argv` bypasses shell parsing and is passed to `zjctl` as-is
- malformed shell quoting in `command`, blank `command`, empty `argv`, blank `argv[0]`, or mixed `command` + `argv` input fails early as an argument parse error instead of spawning a mangled command
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

This is the current takeover path when a pane already exists and the agent should start managing it after the fact.

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
- after attach returns, the pane behaves like any other managed handle: the agent can `send`, `wait`, `capture`, `close`, and `list` against it
- this is the intended flow when a human or another tool already spawned the job and the agent should continue from the current pane state rather than spawn a fresh one
- recommended flow: call `zellij_discover` first, inspect the returned metadata and preview, then attach with the exact returned `selector`

### `zellij_discover`

Discover live panes before attaching.

Input:

```json
{
  "session_name": "zellij-lazygit-demo",
  "tab_name": "Tab 4",
  "selector": "id:terminal:7",
  "include_preview": true,
  "preview_lines": 8
}
```

Notes:

- `zellij_discover` is read-only and does not create handles or persistence state
- `tab_name` and `selector` are optional narrowing filters
- `include_preview=false` returns metadata only
- `preview_lines` must be greater than zero when provided
- shell-like panes return `preview_basis="recent_lines"`
- repaint-heavy panes return `preview_basis="visible_frame"`
- the returned `selector` is attach-ready and should be reused directly for `zellij_attach`

Response:

```json
{
  "candidates": [
    {
      "selector": "id:terminal:7",
      "pane_id": "terminal:7",
      "session_name": "zellij-lazygit-demo",
      "tab_name": "Tab 4",
      "title": "step5-argv",
      "command": "sh -c printf argv-form-demo\\n; exec cat",
      "focused": true,
      "preview": "...",
      "preview_basis": "recent_lines",
      "captured_at": "2026-03-22T00:00:00Z"
    }
  ]
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
  "mode": "full",
  "tail_lines": 20
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
  "tail_lines": 20,
  "line_window_applied": true,
  "truncated": false,
  "captured_at": "2026-03-22T00:00:00Z",
  "baseline": "last_capture"
}
```

Semantics:

- first `delta` call may return the full captured content and initialize the baseline
- `current` is best-effort and may over-include prior output if the pane was already active before attach
- for redraw-heavy TUIs, `current` now prefers a normalized latest visible frame over raw prefix subtraction when the capture contains clear-screen, home-cursor, or carriage-return style repaint sequences
- `delta` and `full` intentionally keep their previous snapshot semantics so shell-style append flows do not regress
- `tail_lines` is optional output shaping applied after semantic capture computation; it does not change baseline tracking
- `tail_lines` must be greater than zero when provided

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
- `spawn` also supports explicit `argv` input without shell parsing
- `attach` can convert an existing live pane into a managed daemon handle for takeover-style agent workflows
- `discover` can inspect unmanaged panes before attach and returns attach-ready selectors with bounded preview text
- `capture` can clip returned output with `tail_lines` while keeping `full` / `delta` / `current` semantics stable

## Phase 1 Limitations

- capture does not promise true shell process boundaries
- delta mode is snapshot-based, not scrollback-cursor-based
- attach to an existing pane may include pre-existing output in the first current capture
- no phase 1 tool manages layout or pane scheduling
- the backend plugin must be loaded in the target session and its permission prompt approved before RPC-backed operations can succeed
- full-screen TUIs may redraw large portions of the screen, so `delta` and `current` may over-report changes compared with shell-style output
