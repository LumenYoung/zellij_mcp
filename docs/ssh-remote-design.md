# SSH Remote Design

## Goal

Support local and SSH-backed Zellij targets through a single MCP server.

The local daemon should remain the only MCP server exposed to the agent. Local Zellij is the default target. Remote Zellij is selected explicitly when needed.

## Why The Earlier Direction Is Obsolete

The earlier wrapper-based design solved the first remote smoke quickly, but it assumes separate MCP entries such as `zellij-local` and `zellij-a100`.

That is no longer the desired architecture because:

- multiple MCP entries increase MCP/tool-context footprint
- the agent has to choose between servers instead of staying within one interface
- follow-up ergonomics are worse than a single handle space owned by one daemon

The wrapper and bootstrap helper are still useful as operational building blocks, but they are no longer the primary interface design.

## Current Decision

Chosen direction:

- one local MCP daemon
- one target-aware router inside that daemon
- local backend by default
- SSH-backed backend when `target` is specified

The daemon remains the sole owner of:

- handle generation
- binding persistence
- observation persistence
- revalidation and lifecycle state

## Interface

### Request Shape

Implemented shape:

Add optional `target` only to the tools that select a live environment:

- `zellij_spawn`
- `zellij_attach`
- `zellij_discover`
- `zellij_list`

Do not add `target` to:

- `zellij_send`
- `zellij_wait`
- `zellij_capture`
- `zellij_close`

Reason:

- once a handle exists, the daemon should already know which backend owns it
- follow-up calls should stay compact and avoid repeated tokens

Example local request:

```json
{
  "session_name": "gpu",
  "selector": "id:terminal:18"
}
```

Example remote request:

```json
{
  "target": "a100",
  "session_name": "a100",
  "selector": "id:terminal:0"
}
```

### Response Shape

The daemon should echo the resolved target in user-visible places that help inspection and debugging:

- handle-creating responses
- `zellij_discover` candidates
- `zellij_list` bindings

This can be a short stable value such as:

- `local`
- `ssh:a100`

## Persistence

Bindings must become target-aware.

Implemented change:

- add stable `target_id` to `TerminalBinding`

Examples:

- `local`
- `ssh:a100`

Why this is required:

- `send`, `wait`, `capture`, and `close` currently route only by handle
- without persisted target identity, the daemon cannot know which backend owns a handle
- local and remote sessions may reuse the same `session_name`, `tab_name`, or selector space

Observation records can remain keyed by local daemon handle. The local daemon stays the single owner of lifecycle state.

## Backend

### Chosen Backend Model

Use one local daemon with per-target backends.

- local backend: direct local `zjctl` / `zellij`
- remote backend: SSH-backed command execution for `zjctl` / `zellij`

This means remote operations are executed over SSH directly by the local daemon.

### Not Chosen

Do not use nested remote MCP daemons as the main routing model.

Why not:

- local handle to remote handle mapping becomes more complex
- local and remote daemon state ownership becomes split
- restart and revalidation behavior become harder to reason about
- you keep a single external MCP but hide a second stateful daemon layer inside it

The current wrapper and bootstrap helper remain useful for smoke, bootstrap, or fallback operations, but not as the primary long-term request path.

## Routing

### High-Level Structure

Keep `src/server/mcp.rs` conceptually the same: one `TerminalManager` behind one MCP server.

Replace the single-backend startup in `src/main.rs` with a target-aware router manager.

That router should:

- resolve `request.target` for `spawn`, `attach`, `discover`, and `list`
- resolve persisted `binding.target_id` for `send`, `wait`, `capture`, and `close`
- route the operation to the correct backend implementation

### Target Resolution Rules

- missing `target` means local
- explicit `target` selects an SSH target alias and is canonicalized to `ssh:<alias>`
- alias-only defaults make ordinary aliases work without predeclaring per-host binary paths
- transport or readiness failures are reported during remote preflight if the alias cannot actually be reached or prepared

## Target Configuration

Transport details should stay in daemon-side config, not in tool requests.

The request should only say:

- `target: "a100"`
- the alias can be bare, and the daemon resolves it canonically to `ssh:a100`

The daemon-side target config owns things like:

- SSH alias or host
- remote `zjctl` path
- remote `zellij` path
- readiness/bootstrap policy

Current `ZELLIJ_MCP_TARGETS` shape:

```json
{
  "defaults": {
    "remote_zjctl_bin": "zjctl",
    "remote_zellij_bin": "zellij",
    "remote_env": {
      "ZELLIJ_SESSION_NAME": "remote"
    },
    "ssh_options": ["-o", "BatchMode=yes"]
  },
  "overrides": {
    "aws": {
      "host": "aws",
      "remote_env": {
        "ZELLIJ_SESSION_NAME": "aws"
      }
    },
    "a100": {
      "host": "a100",
      "remote_env": {
        "ZELLIJ_SESSION_NAME": "a100"
      }
    }
  }
}
```

The daemon parses that layered shape into per-target `SshTargetConfig` values, then applies alias-specific partial overrides on top of the shared defaults.

This keeps ordinary hosts from needing per-host binary paths when the tools already live in standard locations or in `~/.local/bin`. Bare aliases therefore do not need to be predeclared unless you want to override the shared defaults for a particular host.

Current code parses that shape into `SshTargetConfig` with these fields:

- `host`
- `remote_zjctl_bin`
- `remote_zellij_bin`
- `remote_env`
- `ssh_options`

The local backend is configured separately through the normal local daemon env such as `ZJCTL_BIN` and `ZELLIJ_MCP_STATE_DIR`.

## Readiness And Bootstrap

The implementation now performs remote readiness checks before it routes SSH-backed work.

The readiness model is intentionally narrow and bounded:

- `Ready`: the target already has what it needs for the current request
- `AutoFixable`: the daemon can safely apply bounded remediation and try again
- `ManualActionRequired`: a human still has to take action before the target can proceed, or the loaded daemon/plugin artifacts must be brought back into version alignment

In operator-facing language, that last state is the manual-action-required path.

Remote probing and execution already normalize the remote HOME and PATH. When non-interactive SSH is missing it, the daemon prepends `$HOME/.local/bin` before concluding that `zellij` or `zjctl` are unavailable.

Auto-fix stays safe and bounded. It can install the repo-built `zrpc.wasm` into the standard plugin directory, start a detached helper client when that is the missing precondition, and retry readiness exactly once.

Protocol/version mismatch is intentionally not part of that auto-fix bucket. Once the daemon can talk to the plugin well enough to learn that their protocol versions differ, retrying the same helper or plugin-launch flow is not enough; the operator needs matching artifacts loaded on the host.

The daemon does not try to force through unmanaged interactive approval prompts. Those stay in `ManualActionRequired` until a person approves them in the remote Zellij session.

Remote `zjctl` RPC still depends on a connected Zellij client. If the host is otherwise healthy but still needs a helper client, the daemon treats that as a bounded auto-fixable condition. If the host needs unmanaged plugin approval, the daemon stops at `ManualActionRequired` instead of guessing.

The practical meaning of the states is:

- `Ready`: proceed with the routed SSH request
- `AutoFixable`: apply safe remediation, then retry the same bounded probe or request
- `ManualActionRequired`: stop and tell the user what still needs human input or compatibility repair

The daemon now also surfaces freshness metadata in two places that matter operationally:

- startup stderr logs include daemon instance id, version, build stamp, pid, and start time
- every successful response and every MCP error payload includes daemon identity so a stale local process is obvious during debugging

## Practical Findings From `a100`

Validated facts from the current real-host experiments:

- `ssh a100` works in non-interactive batch mode
- copied local Linux binaries can fail on remote glibc mismatch
- native user-space rebuild on the remote host solves that compatibility issue
- `~/.local/bin` is a useful remote install location because the daemon now prepends it for non-interactive SSH probing and execution when it is otherwise missing from `PATH`
- `zjctl` plugin RPC can require a connected Zellij client on a headless host
- a detached user-space `tmux` helper client was sufficient to make RPC healthy on `a100`
- once the remote host was ready, alias-only remote MCP operations succeeded
- preview-enabled discover now degrades cleanly instead of failing the whole tool on pane capture issues

These findings affect backend readiness and bootstrap design, but they do not change the single-daemon routing decision.

## Trade-Offs

### Why This Plan Is Best Now

- one MCP server keeps token footprint down
- local default keeps existing workflows unchanged
- only four tools gain one optional field
- handle-based follow-up flow stays compact
- the daemon remains the single owner of state and lifecycle

### Known Costs

- remote operations pay SSH overhead per operation in step 1
- alias stability is assumed; if that changes later, we may want a target fingerprint safeguard
- remote readiness and any remaining manual-action-required steps remain operationally non-trivial on some hosts

### When We Would Revisit Backend Strategy

Consider a more persistent remote transport only if one or more of these become painful in practice:

- SSH startup latency dominates normal workflows
- remote `wait` / `capture` loops are too slow over repeated SSH startup
- one remote target must outlive multiple local daemon lifetimes
- multiple local clients must share one remote backend statefully

## Current Implementation Status

Implemented:

- add `target: Option<String>` to `SpawnRequest`
- add `target: Option<String>` to `AttachRequest`
- add `target: Option<String>` to `DiscoverRequest`
- add `target: Option<String>` to `ListRequest`
- add persisted `target_id` to `TerminalBinding`
- expose target identity in list/discover/handle-creating responses where useful
- route selection tools by request `target`
- route follow-up handle tools by persisted `binding.target_id`
- accept both alias input such as `a100` and canonical input such as `ssh:a100`
- return `TARGET_NOT_FOUND` for unknown configured targets
- support backward-compatible registry loads where older bindings omitted `target_id`
- parse `ZELLIJ_MCP_TARGETS` into per-target SSH backend configs at startup
- classify readiness as `Ready`, `AutoFixable`, or `ManualActionRequired`
- perform bounded safe remediation before retrying a remote probe or request
- preserve handle semantics so follow-up operations stay routed by the stored binding, not by request-time `target`
- preload persisted remote target ids on startup and revalidate remote bindings through the backend factory after restart
- return daemon freshness metadata on every successful MCP response and inside MCP error payloads
- degrade remote spawn and discover follow-up uncertainty to recoverable `busy` / metadata-only outcomes instead of losing ownership or failing the whole discover call

Still manual when the host needs it:

- unmanaged interactive plugin approval prompts
- any other remote action that cannot be resolved safely without a human session on the target host

## Practical Smoke Workflow

1. Start the local daemon with `ZELLIJ_MCP_TARGETS` configured using `defaults` and any alias overrides you need.
2. Call one of the selection tools with `target: "<alias>"`, for example `zellij_discover` or `zellij_attach`.
3. Confirm the response carries `target_id: "ssh:<alias>"`.
4. Follow with `zellij_capture`, `zellij_wait`, `zellij_send`, or `zellij_close` using only the returned handle.

Example remote discover request:

```json
{
  "target": "a100",
  "session_name": "a100",
  "include_preview": false
}
```

Expected behavior:

- the request is routed by the local daemon to the SSH-backed backend
- returned candidates or handles use canonical `target_id` values such as `ssh:a100`
- follow-up handle operations no longer need `target`
- remote HOME and PATH normalization happens before the non-interactive probe or execution path if the environment needs it
- `~/.local/bin` is included automatically when the daemon has to make the remote environment usable

## Operational Helpers

- `scripts/zellij-mcp-bootstrap-ssh` remains useful as a user-space helper for remote setup and recovery
- `scripts/zellij-mcp-ssh` remains useful for fallback or smoke comparisons against the older wrapper-based flow
- neither helper is the primary interface contract for normal single-MCP usage
