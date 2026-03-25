# Zellij MCP

Rust MCP daemon for safe agent interaction with Zellij panes through `zjctl`.

Current release line: `0.1.0`

## What it provides

The daemon exposes these MCP tools:

- `zellij_spawn`
- `zellij_attach`
- `zellij_discover`
- `zellij_send`
- `zellij_wait`
- `zellij_capture`
- `zellij_close`
- `zellij_list`

It keeps agent-facing handles stable, persists lightweight state, and adds capture semantics (`full`, `delta`, `current`) on top of raw pane access.

Recent spawn hardening:

- `zellij_spawn(wait_ready=true)` may return `status="busy"` when the pane is real but rendered-screen idle detection does not settle within the bounded wait window
- the daemon now persists spawned handles before post-launch probing so a real launch is not lost just because follow-up readiness or capture work degrades
- `spawn_target="new_tab"` now uses a direct `zellij run` + post-list resolution path because the older fresh-tab RPC handoff could stall after the pane was already created

## Requirements

- Rust toolchain
- local `zjctl`
- Zellij session with the required plugin approved for `zjctl` RPC use

## Build

```bash
cargo build --release
```

Binary path:

```text
target/release/zellij_mcp
```

## Run manually

The daemon serves MCP over stdio.

```bash
ZJCTL_BIN=/home/yang/Documents/git/zjctl/target/release/zjctl \
ZELLIJ_MCP_STATE_DIR=/home/yang/.local/state/zellij-mcp-opencode \
./target/release/zellij_mcp
```

## Manual smoke testing

`mcp2cli` is optional. It is useful for manual local verification, but OpenCode does not need it for normal usage.

Example:

```bash
mcp2cli --mcp-stdio "./target/release/zellij_mcp" --list
```

## OpenCode setup on this machine

OpenCode is configured from:

```text
/home/yang/dotfiles/opencode/opencode.json
```

This machine now uses a local MCP entry named `zellij` that launches the daemon on demand.

Important consequence:

- you do not need to pre-spawn the daemon before agent usage in OpenCode
- OpenCode starts it when the MCP tools are needed
- `mcp2cli` is only for manual testing, not required for OpenCode agent use

## Remote over SSH

The current model keeps a single local MCP daemon and lets selection tools opt into a remote target.

- omit `target` to use the local backend
- set `target` on `zellij_spawn`, `zellij_attach`, `zellij_discover`, or `zellij_list` to select an SSH target alias such as `aws`
- the target value can be a bare alias like `aws`, and the daemon resolves it canonically to `ssh:aws`
- follow-up tools such as `zellij_send`, `zellij_wait`, `zellij_capture`, and `zellij_close` do not need `target`; the daemon routes them by the persisted handle binding
- remote `zjctl` and `zellij` commands are executed over SSH by the local daemon

Runtime configuration is daemon-side through `ZELLIJ_MCP_TARGETS`.

The current shape is layered, with shared `defaults` and optional per-alias partial `overrides`:

Example:

```bash
ZELLIJ_MCP_TARGETS='{"defaults":{"remote_zjctl_bin":"zjctl","remote_zellij_bin":"zellij","remote_env":{"ZELLIJ_SESSION_NAME":"remote"},"ssh_options":["-o","BatchMode=yes"]},"overrides":{"aws":{"host":"aws","remote_env":{"ZELLIJ_SESSION_NAME":"aws"}},"a100":{"host":"a100","remote_env":{"ZELLIJ_SESSION_NAME":"a100"}}}}' \
./target/release/zellij_mcp
```

In that layered form, each alias starts from `defaults`, then only the fields present under its override replace or extend them. The daemon also keeps accepting the older legacy map shape for backward compatibility, but the layered `defaults` plus `overrides` form is the intended alias-only setup.

Readiness and remediation:

- the daemon checks remote SSH targets before it tries to use them, then classifies the target as `Ready`, `AutoFixable`, or `ManualActionRequired`
- `AutoFixable` means the daemon can safely apply bounded remediation, such as starting a detached helper client, running `zjctl install` when `zjctl` already resolves, and retrying readiness exactly once
- `AutoFixable` is used specifically for missing binaries, helper-client absence, and RPC-not-ready drift that can still be recovered through deterministic user-space setup
- when the remote command path does not already include it, the daemon prepends `$HOME/.local/bin` for non-interactive SSH probing and execution so ordinary hosts do not need per-host binary paths
- `ManualActionRequired` covers the remaining interactive cases, especially unmanaged plugin approval prompts that still need a human to confirm them in the remote Zellij session
- readiness does not claim zero-touch success for every host, it only fixes the safe, bounded cases automatically

Freshness and diagnosability:

- startup now logs daemon instance id, package version, build stamp, pid, and started-at timestamp to stderr
- every successful MCP tool response now includes `_daemon` metadata with the same identity fields so stale local processes are visible per request
- MCP error data now includes daemon identity as well, which makes mixed local/remote failures easier to attribute to the right running binary

Practical setup note:

- if a locally copied Linux binary fails on the remote host with a glibc version error, build it natively on the remote host in user space and install it into the remote `~/.local/bin`
- the daemon normalizes the remote HOME and PATH for non-interactive SSH probing and execution, so remote tools installed in `~/.local/bin` are found without per-host path overrides
- `zjctl` RPC still needs an attached Zellij client, so a headless remote host may still need a user-space helper such as a detached `tmux` session running `zellij attach <session>`

Bootstrap helper:

```bash
./scripts/zellij-mcp-bootstrap-ssh a100 --session a100
```

The bootstrap helper stays entirely in user space. It installs Rust if needed, syncs this repo, clones or updates `zjctl`, builds the required binaries natively on the remote host, installs the plugin, starts a detached helper client, and finishes by running `zjctl doctor`.

Operational helper note:

- `scripts/zellij-mcp-ssh` remains useful for smoke testing or fallback operations
- it is no longer the primary remote architecture or the recommended OpenCode MCP shape

Path note:

- prefer absolute remote paths or plain executable names that already resolve on the remote non-interactive `PATH`
- do not rely on `~` expansion in helper-script arguments; the wrapper intentionally quotes remote tokens before handing them to SSH

Important constraint:

- this does not create a detached network daemon on the remote host; the local daemon still initiates remote operations over SSH on demand
- if you later want the remote daemon to stay reachable without any SSH transport, that becomes a separate transport feature
- this also does not remove Zellij's own requirement for a connected client when the remote plugin RPC path is in use
- `zellij_discover` now degrades preview failures to metadata-only candidates instead of failing the whole call, but metadata-only discovery can still be the cleaner choice on very busy live panes

Target and handle semantics:

- the canonical target id stored in responses and bindings is `ssh:<alias>`
- selection tools accept the alias form from the user, while follow-up calls keep using the persisted handle binding
- that separation keeps the single local MCP architecture intact and avoids nested remote daemons

Configured runtime values:

- `ZJCTL_BIN=/home/yang/Documents/git/zjctl/target/release/zjctl`
- `ZELLIJ_MCP_STATE_DIR=/home/yang/.local/state/zellij-mcp-opencode`

## Recommended agent flow

For existing user panes:

1. `zellij_discover`
2. `zellij_attach`
3. `zellij_wait`
4. `zellij_capture`
5. `zellij_send`

For new agent-owned panes:

1. `zellij_spawn`
2. if spawn returns `status="busy"`, keep the returned handle and follow with `zellij_wait` or `zellij_capture`
3. `zellij_capture`
4. `zellij_send`
5. `zellij_close`

Notes:

- `wait_ready=true` is a convenience idle probe, not a process-start or app-ready guarantee
- a later `zellij_list` revalidation can upgrade a spawned handle from `busy` to `ready` once the pane selector is reachable again; use `zellij_capture` when you need a fresh output baseline

## Remote troubleshooting quick map

- `TARGET_NOT_FOUND`: the selected alias is unknown or the remote pane selector no longer resolves; verify `ZELLIJ_MCP_TARGETS`, the session name, and whether the pane still exists
- `CAPTURE_FAILED`: the handle exists but the capture path degraded; retry with the same handle, then use `zellij_list` to revalidate ownership before assuming the pane is gone
- `PLUGIN_NOT_READY`: the host is reachable but RPC preconditions are missing; distinguish helper-client absence, RPC-not-ready drift, and manual plugin approval before retrying
- `ZJCTL_UNAVAILABLE`: the transport or remote binary path is the problem; verify SSH reachability, native remote install, and that `zjctl` / `zellij` resolve in the non-interactive remote PATH

## More detail

- `docs/architecture.md`
- `docs/mcp-contract.md`
- `docs/ssh-remote-design.md`
`zellij_discover` returns live pane metadata plus attach-ready `selector` values. `zellij_attach` then requires one exact selector so the daemon can bind one specific pane and return a stable `handle` for later `send`, `wait`, `capture`, and `close` calls.
