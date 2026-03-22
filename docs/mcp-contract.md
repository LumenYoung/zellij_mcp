# MCP Contract

## Principles

- expose stable daemon-native tools rather than raw Zellij actions
- identify terminals by daemon handle, not by raw pane id
- keep phase 1 tool semantics small and explicit
- document degraded behavior where the backend cannot provide exact semantics

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
  "submit": true
}
```

Notes:

- `submit=true` indicates the daemon should treat this as the start of a new interaction boundary for `current` capture mode

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

## Phase 1 Limitations

- capture does not promise true shell process boundaries
- delta mode is snapshot-based, not scrollback-cursor-based
- attach to an existing pane may include pre-existing output in the first current capture
- no phase 1 tool manages layout or pane scheduling
