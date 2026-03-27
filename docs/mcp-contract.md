# MCP Contract

## Principles

- expose stable daemon-native tools rather than raw Zellij actions
- prefer daemon handles for managed follow-up, while still allowing exact location intent where the tool explicitly supports it
- keep phase 1 tool semantics small and explicit
- document degraded behavior where the backend cannot provide exact semantics

## Transport

- the daemon is exposed as a real MCP stdio server through `rmcp`
- `mcp2cli --mcp-stdio "cargo run --quiet --manifest-path /path/to/Cargo.toml" --list` is a convenient local smoke/debug path; OpenCode on this machine uses the daemon directly as a local MCP server
- each MCP tool returns its structured result serialized as JSON text content, which keeps the transport layer thin while remaining readable to `mcp2cli`
- MCP error responses preserve daemon error details in their `data` payload with stable fields: `code`, `message`, and `retryable`
- remote-over-SSH keeps the exact same MCP contract through one local daemon: selection tools may include an optional `target` alias, and the daemon executes the matching backend over SSH
- this phase-1 remote model does not require `mcp2cli` in the runtime path; only backend selection changes for the relevant tools

## Tools

### `zellij_spawn`

Create a managed execution target.

Input:

```json
{
  "target": "a100",
  "session_name": "gpu",
  "spawn_target": "new_tab",
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
  "target": "a100",
  "session_name": "gpu",
  "spawn_target": "existing_tab",
  "tab_name": "editor",
  "cwd": "/home/yang/Documents/git/project",
  "argv": ["bash", "-lc", "printf 'hello from argv\\n'"],
  "title": "argv-demo",
  "wait_ready": true
}
```

Notes:

- `target` is optional backend selection; omit it for local, set it to a configured SSH alias such as `a100`, or use the canonical form `ssh:a100`
- `spawn_target` supports `new_tab` and `existing_tab`
- `existing_tab` means spawn a new dedicated pane inside the tab
- phase 1 does not replace existing processes in an existing pane
- use either `command` or `argv`, not both
- `command` is parsed with shell-style quoting, so inputs like `bash -lc 'echo hello world'` preserve the intended argv shape
- `argv` bypasses shell parsing and is passed to the backend as-is
- malformed shell quoting in `command`, blank `command`, empty `argv`, blank `argv[0]`, or mixed `command` + `argv` input fails early as an argument parse error instead of spawning a mangled command
- `wait_ready=true` runs the same rendered-screen idle check as `zellij_wait`; it works for shell-like startup and was live-tested with `lazygit`, but redraw-heavy TUIs may still make it a noisy readiness proxy
- when that bounded idle check times out after the pane is already real, `zellij_spawn` returns the new handle with `status="busy"` instead of failing the whole launch
- `spawn_target="new_tab"` now creates the tab, launches the command with `zellij run`, then resolves the spawned pane from post-launch session state; this avoids the earlier fresh-tab RPC handoff stall where the pane could exist before the request returned
- fatal post-launch errors that happen after early persistence now clean up the provisional binding instead of leaving an orphaned busy handle behind

Remote readiness and bootstrap note:

- repo-owned shell/bootstrap outputs use the same readiness vocabulary as MCP: `readiness_state`, `readiness_reason`, and `mcp_error_code`
- `PLUGIN_NOT_READY` remains the umbrella MCP class for missing plugin artifacts, plugin approval, helper-client absence, and RPC-not-ready drift
- `PROTOCOL_VERSION_MISMATCH` stays separate because repeating the same bounded remediation cannot fix daemon/plugin skew

Response:

```json
{
  "handle": "zh_...",
  "target_id": "ssh:a100",
  "session_name": "gpu",
  "tab_name": "editor",
  "selector": "id:terminal:7",
  "status": "ready",
  "_daemon": {
    "package": "zellij_mcp",
    "version": "0.1.0",
    "build_stamp": "0.1.0",
    "instance_id": "zmd_...",
    "process_id": 12345,
    "started_at": "2026-03-22T00:00:00Z"
  }
}
```

Degraded but successful response:

```json
{
  "handle": "zh_...",
  "target_id": "ssh:a100",
  "session_name": "gpu",
  "tab_name": "editor",
  "selector": "id:terminal:7",
  "status": "busy"
}
```

### `zellij_attach`

Attach a live pane to daemon management.

This is the current takeover path when a pane already exists and the agent should start managing it after the fact.

Input:

```json
{
  "target": "a100",
  "session_name": "gpu",
  "tab_name": "editor",
  "selector": "title:main-editor",
  "alias": "main-editor"
}
```

Notes:

- the selector must resolve to exactly one target
- `target` is optional backend selection; omit it for local, or set it to a configured SSH alias
- responses return canonical `target_id` values such as `local` or `ssh:a100`; callers may keep using alias form on later selection requests, and the router also accepts canonical `ssh:<alias>` input
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
  "target": "a100",
  "session_name": "zellij-lazygit-demo",
  "tab_name": "Tab 4",
  "selector": "id:terminal:7",
  "include_preview": true,
  "preview_lines": 8
}
```

Notes:

- `zellij_discover` is read-only and does not create handles or persistence state
- `target` is optional backend selection; omit it for local, set it to a configured SSH alias, or use the canonical form `ssh:a100`
- `tab_name` and `selector` are optional narrowing filters
- `include_preview=false` returns metadata only
- `preview_lines` must be greater than zero when provided
- shell-like panes return `preview_basis="recent_lines"`
- repaint-heavy panes return `preview_basis="visible_frame"`
- if preview capture fails for a specific pane, `zellij_discover` still returns that candidate with `preview=null`, `preview_basis=null`, and `captured_at=null` instead of failing the whole tool
- the returned `selector` is attach-ready and should be reused directly for `zellij_attach`

Response:

```json
{
  "candidates": [
    {
      "target_id": "ssh:a100",
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

Send input to a managed pane handle or directly to one exact existing pane by location intent.

Handle-based input:

```json
{
  "handle": "zh_...",
  "text": ":w",
  "keys": ["escape", "up"],
  "input_mode": "raw",
  "submit": false
}
```

Location-intent input:

```json
{
  "target": "a100",
  "session_name": "gpu",
  "tab_name": "editor",
  "selector": "id:terminal:7",
  "text": "printf 'ok'",
  "submit": true
}
```

Notes:

- `zellij_send` accepts exactly one targeting mode: either `handle`, or location intent via `session_name` + `selector`
- when `handle` is omitted, `session_name` and `selector` are required, `tab_name` is optional narrowing context, and `target` may be used to select a local or SSH backend directly
- location-intent sends do not create or persist a managed handle; they resolve the target pane for that one request and return `accepted=true`
- `submit` is still a required field in both handle and location-intent forms; omitting it is currently an argument-parse error rather than an implicit default
- `input_mode` is optional: `raw` means direct terminal input, `submit_line` means explicit shell-style line submission, and omitting it preserves the legacy `submit` behavior
- `submit=true` remains the backward-compatible way to request shell-style line submission and current-boundary reset when `input_mode` is omitted
- `keys` is optional and supports named special inputs including `enter`, `tab`, `shift_tab`, `escape`/`esc`, arrows, `home`, `end`, `insert`, `delete`, `page_up`, `page_down`, `f1` through `f12`, and generic `ctrl_<letter>` chords such as `ctrl_c` or `ctrl_l`
- `text` and `keys` can be combined in one request; special keys are translated to terminal byte sequences before dispatch
- for attached shell-like takeover flows, a submit-text send now refreshes the current screen boundary and clears any pending unsubmitted line input before dispatch so the new command does not get appended onto partially typed shell text
- `input_mode=raw` or legacy `submit=false` is suitable for raw printable input into an interactive TUI, for example sending `q` to quit `lazygit`
- printable input is verified in `lazygit`, and named `up`/`escape` key sequences have been verified live through a raw terminal pane
- explicit `submit_line` mode only accepts shell text, not named `keys`, so callers do not accidentally mix TUI key sequences into shell-submit semantics

### `zellij_takeover`

Search and attach an existing pane in one step.

Input:

```json
{
  "session_name": "gpu",
  "tab_name": "editor",
  "selector": "title:editor",
  "command_contains": "fish",
  "focused": false,
  "alias": "taken"
}
```

Notes:

- takeover is attach-based under the hood: it searches the session, requires exactly one match, then establishes a normal managed handle with a fresh baseline
- `selector`, `command_contains`, and `focused` are optional filters that can be combined; zero matches return `TARGET_NOT_FOUND`, multiple matches return `SELECTOR_NOT_UNIQUE`

### `zellij_replace`

Cooperatively reuse a managed shell-like pane for a new command.

Input:

```json
{
  "handle": "zh_...",
  "command": "echo swapped",
  "interrupt": true
}
```

Notes:

- replace is intentionally scoped to supported shell-like panes; it is not a universal OS-level process replacement primitive
- when `interrupt=true`, the daemon first sends `Ctrl-C`, then submits the new shell command on the same handle
- replace reuses the same handle and starts a fresh explicit interaction when marker support is available

### `zellij_cleanup`

Clean up persisted stale or closed pane state.

Input:

```json
{
  "statuses": ["closed", "stale"],
  "max_age_ms": 60000,
  "dry_run": true
}
```

Notes:

- cleanup is target-scoped and only removes persisted daemon state for the selected target
- when `statuses` is omitted, cleanup defaults to stale and closed handles only
- `dry_run=true` reports which handles would be removed without deleting them

### `zellij_layout`

Inspect tabs and panes grouped by tab for a session.

Input:

```json
{
  "session_name": "gpu"
}
```

Notes:

- this is a read-only grouped inspection view over the same live pane metadata used by discover/attach/takeover
- selector support now also includes `command:<substring>`, `tab:<substring>`, and focus filters such as `focused`, `focused:true`, `focused:false`, and `unfocused`
- layout mutation and focus-changing actions are still intentionally out of scope

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
  "observed_at": "2026-03-22T00:00:00Z",
  "completion_basis": "interaction_marker",
  "interaction_id": "zi_...",
  "interaction_completed": true,
  "interaction_exit_code": 0
}
```

Notes:

- `zellij_wait` uses the backend idle-wait path, so it observes rendered-screen stability rather than process completion
- for daemon-submitted shell interactions on supported shell-like panes, `zellij_wait` can additionally report `completion_basis="interaction_marker"` plus interaction completion metadata when the explicit end marker is visible in capture output
- if the backend temporarily fails to resolve a freshly managed pane, the daemon retries and can fall back to capture-based stability polling before declaring the handle stale
- this has been verified live for a spawned `lazygit` pane, but should still be read as an idle heuristic rather than an exact app-ready guarantee
- a handle becoming `ready` after `zellij_list` or another later operation means the daemon revalidated selector reachability for that pane; use `zellij_capture` when you need a fresh output baseline, and do not treat it as a stronger readiness promise than `zellij_wait`

### `zellij_capture`

Capture output from a managed pane.

Input:

```json
{
  "handle": "zh_...",
  "mode": "full",
  "line_limit": 200,
  "cursor": "lines:400",
  "normalize_ansi": true
}
```

Supported modes:

- `full`: latest complete capture available from the backend
- `delta`: textual difference against the previous successful capture for the same handle
- `current`: best-effort textual difference against the command boundary established by spawn, attach, or `send(submit=true)`; for daemon-submitted shell interactions on supported shell-like panes it prefers explicit interaction-marker output when available

Response:

```json
{
  "handle": "zh_...",
  "mode": "delta",
  "content": "new lines...",
  "tail_lines": null,
  "line_offset": 400,
  "line_limit": 200,
  "line_window_applied": true,
  "next_cursor": "lines:600",
  "ansi_normalized": true,
  "truncated": false,
  "captured_at": "2026-03-22T00:00:00Z",
  "baseline": "interaction_marker",
  "interaction_id": "zi_...",
  "interaction_completed": true,
  "interaction_exit_code": 0
}
```

Semantics:

- first `delta` call may return the full captured content and initialize the baseline
- `current` is best-effort and may over-include prior output if the pane was already active before attach
- for redraw-heavy TUIs, `current` now prefers a normalized latest visible frame over raw prefix subtraction when the capture contains clear-screen, home-cursor, or carriage-return style repaint sequences
- explicit interaction-marker capture is only available when the daemon itself submitted a shell command on a supported shell-like pane; attached or unsupported panes still use the legacy boundary/repaint heuristics
- `delta` and `full` intentionally keep their previous snapshot semantics so shell-style append flows do not regress
- `tail_lines` is optional output shaping applied after semantic capture computation; it does not change baseline tracking
- `tail_lines` must be greater than zero when provided
- `line_offset` starts a forward line window from an explicit semantic line offset; `line_limit` caps the returned line count from that point
- `cursor` currently uses the daemon-owned `lines:<offset>` format and resumes from that semantic line offset
- `line_offset` in the response is the effective starting line after cursor resolution; `next_cursor` is present only when more semantic output remains
- forward line-window paging is currently supported for `full` and `current`; `mode="delta"` rejects `line_offset`, `line_limit`, and `cursor` because delta baselines advance after each capture
- `tail_lines` cannot be combined with `line_offset`, `line_limit`, or `cursor`
- `normalize_ansi=true` strips ANSI escape/control sequences from the already-selected semantic capture output before line-window chunking; baseline tracking still uses the unmodified full snapshot

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
  "target": "a100",
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
- `PROTOCOL_VERSION_MISMATCH`
- `PERSISTENCE_ERROR`

## Response Metadata

- every successful tool response includes `_daemon`
- `_daemon` currently includes `package`, `version`, `build_stamp`, `instance_id`, `process_id`, and `started_at`
- MCP error responses preserve the stable domain error code in `data.code` and now also include daemon identity in `data.daemon`

## Troubleshooting Matrix

- `TARGET_NOT_FOUND`: either the configured target alias is missing or a previously known selector no longer resolves; check target config first, then revalidate session/pane existence
- `CAPTURE_FAILED`: the daemon still has a routed handle, but capture could not complete; retry with the same handle and let `zellij_list` or `zellij_capture` revalidate before discarding the binding
- `PLUGIN_NOT_READY`: SSH transport reached the host, but plugin/runtime preconditions are still missing; distinguish missing plugin artifacts, plugin approval, helper-client presence, and RPC readiness drift before escalating to transport debugging
- `PROTOCOL_VERSION_MISMATCH`: the daemon reached the plugin but `response.v` does not match the daemon's expected protocol version; use matching daemon/plugin artifacts before retrying rather than repeating helper or plugin-launch remediation
- `ZJCTL_UNAVAILABLE`: the daemon could not reach the binary or the SSH-backed execution path; verify SSH reachability, non-interactive PATH resolution, and native binary availability on the remote host

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
- `spawn(wait_ready=true)` can degrade to `status="busy"` while still returning a usable handle when the launch succeeded but idle detection did not settle in time
- `spawn(spawn_target="new_tab")` avoids the earlier fresh-tab RPC stall by resolving the pane after `zellij run` from live session state
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
- spawn persistence still writes registry and observation state in separate files, so a crash or storage failure between those writes can leave partial local state even though normal runtime cleanup now covers the post-launch failure paths
