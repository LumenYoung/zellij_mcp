# zellij-skill

Rust MCP daemon for safe agent interaction with Zellij panes through `zjctl`.

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
- `target="new_tab"` now uses a direct `zellij run` + post-list resolution path because the older fresh-tab RPC handoff could stall after the pane was already created

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

For a remote Zellij host, OpenCode can still use the same MCP daemon without a manually attached SSH shell.

- use `scripts/zellij-mcp-ssh` as the local MCP launcher
- the wrapper starts `zellij_mcp` remotely over `ssh <alias> ...` and preserves stdio end-to-end
- OpenCode sees a normal stdio MCP server; the SSH connection only lives for the lifetime of that MCP process
- `mcp2cli` is not required in the runtime path

Phase-1 assumptions:

- the remote host already has `zellij_mcp` available
- the remote host already has `zjctl` available
- SSH credentials and the alias are already configured
- the remote Zellij session/plugin approval is already in place

Example wrapper usage:

```bash
./scripts/zellij-mcp-ssh gpu \
  --remote-bin /home/yang/bin/zellij_mcp \
  --remote-zjctl-bin /home/yang/bin/zjctl \
  --remote-state-dir /home/yang/.local/state/zellij-mcp-gpu
```

Representative OpenCode MCP shape:

```json
{
  "mcp": {
    "zellij-gpu": {
      "type": "local",
      "command": ["/home/yang/Documents/git/zellij-skill/scripts/zellij-mcp-ssh"],
      "args": [
        "gpu",
        "--remote-bin",
        "/home/yang/bin/zellij_mcp",
        "--remote-zjctl-bin",
        "/home/yang/bin/zjctl",
        "--remote-state-dir",
        "/home/yang/.local/state/zellij-mcp-gpu"
      ]
    }
  }
}
```

Path note:

- prefer absolute remote paths or plain executable names that already resolve on the remote non-interactive `PATH`
- do not rely on `~` expansion in wrapper arguments; the wrapper intentionally quotes remote tokens before handing them to SSH

Important constraint:

- this does not create a detached network daemon on the remote host; it launches the daemon on demand over SSH for the duration of the MCP session
- if you later want the remote daemon to stay reachable without any SSH transport, that becomes a separate transport feature, not a wrapper-only change

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
- a later `zellij_list` revalidation can upgrade a spawned handle from `busy` to `ready` once the pane is reachable and a baseline capture succeeds

## More detail

- `docs/architecture.md`
- `docs/mcp-contract.md`
- `docs/ssh-remote-design.md`
