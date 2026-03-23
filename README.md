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
