# SSH Remote Design

## Goal

Make the Zellij MCP interface available to agents when the target Zellij session lives on a remote machine that is already reachable through an SSH alias.

This design note separates the immediate usability problem from the longer-term transport question.

- immediate problem: an agent running locally cannot access remote Zellij sessions because the MCP server is only configured locally
- phase-1 goal: let the local MCP client start the same daemon on the remote host on demand
- later question: whether the remote daemon should eventually stay reachable without any SSH transport remaining open

## Tested Findings

Real host used for smoke: `a100`

Observed facts:

- `ssh a100` works in non-interactive batch mode
- the wrapper can successfully execute a harmless remote binary through SSH and preserve env injection
- remote `zellij` exists at `/home/jiaye.yang/.local/bin/zellij`
- `zellij_mcp` and `zjctl` are not currently available on the remote non-interactive `PATH`
- because of that, a full end-to-end MCP smoke against `a100` is blocked until the daemon binary and `zjctl` are installed there or their absolute paths are known

Practical conclusion:

- the SSH launcher design itself works against the real host
- the remaining blocker on `a100` is remote binary placement, not the launcher model

## Requirements

### Must have

- use existing SSH aliases and credentials
- keep the same MCP tool contract for `zellij_spawn`, `zellij_attach`, `zellij_discover`, and the rest
- allow OpenCode to pick up the remote interface automatically through MCP config
- avoid requiring a manually attached interactive SSH shell

### Nice to have

- support host-specific remote binary paths and state dirs
- fail fast if the remote host cannot start the daemon cleanly
- leave room for future detached remote operation

## Design Options

### Option A — On-demand remote stdio daemon over SSH

Flow:

```text
OpenCode -> local wrapper -> ssh <alias> -> remote zellij_mcp -> remote zjctl -> remote Zellij
```

How it works:

- OpenCode launches a local wrapper such as `scripts/zellij-mcp-ssh`
- the wrapper execs `ssh` and starts the existing `zellij_mcp` binary on the remote host
- SSH carries stdin/stdout between the local MCP client and the remote daemon
- the daemon runs on the same machine as Zellij, so the adapter and tool semantics stay unchanged

Why it helps:

- the remote interface becomes visible to the agent through normal MCP config
- no manual SSH shell has to remain open before the agent starts
- no Rust transport or tool schema changes are required

Trade-offs:

- the SSH connection remains open for the lifetime of the MCP server process
- if the network drops, the MCP session drops too
- remote shell noise on stdout can corrupt the MCP stream
- the remote host still needs `zellij_mcp`, `zjctl`, and plugin approval in place

Recommendation:

- this is the best first step and is the path implemented in phase 1

### Option B — Detached remote daemon with a network transport

Flow:

```text
OpenCode -> remote MCP endpoint -> remote zellij_mcp -> remote zjctl -> remote Zellij
```

How it works:

- the daemon runs continuously on the remote host under a supervisor
- the local MCP client connects through a network transport rather than stdio-over-SSH
- the remote process remains reachable without keeping an SSH connection open

Why it helps:

- no SSH session needs to stay alive after startup
- reconnect and multi-client behavior can become cleaner
- remote process management can be separated from local client lifecycle

Trade-offs:

- requires a new transport implementation in the daemon
- requires authentication, endpoint exposure, and supervisor decisions
- increases operational complexity far beyond a wrapper-only step
- changes deployment and security posture

Recommendation:

- defer this until the wrapper path proves insufficient in practice

### Option C — Local daemon with an SSH-aware adapter

Flow:

```text
OpenCode -> local zellij_mcp -> ssh-backed adapter -> remote zjctl/zellij -> remote Zellij
```

How it works:

- the MCP daemon stays local
- instead of spawning local commands, the adapter runs `ssh` for each backend operation
- tool semantics remain similar, but every adapter call becomes a remote command hop

Why it helps:

- the local MCP server stays local and could theoretically target multiple hosts
- persistence remains on the local machine if desired

Trade-offs:

- far more invasive Rust changes than the wrapper approach
- command execution, quoting, retries, timeouts, and state assumptions become more complex
- every backend operation depends on SSH instead of only MCP startup depending on SSH
- adapter behavior becomes harder to reason about and test

Recommendation:

- do not choose this for the first remote step

### Option D — Start a remote daemon through SSH and reconnect later through another local proxy

This is the in-between idea where SSH starts something detached remotely, but the local client still expects stdio.

Why it is awkward:

- stdio MCP does not naturally reconnect to a detached process
- once you want later reconnection, you effectively need a new transport anyway
- this adds complexity without solving the core transport mismatch cleanly

Recommendation:

- skip this design and choose either Option A or Option B explicitly

## Trade-off Summary

| Option | User friction | Code change | Infra change | Survives SSH drop | Recommended now |
|---|---|---:|---:|---:|---:|
| A. SSH wrapper + remote stdio | low | low | low | no | yes |
| B. Detached remote network daemon | medium | high | high | yes | later |
| C. Local daemon + SSH adapter | medium | high | low | maybe | no |
| D. Detached via SSH + later proxy | high | high | medium | maybe | no |

## Decision

Recommended path:

1. adopt Option A first
2. add bootstrap helpers separately for remote binary sync and health checks
3. only consider Option B if long-lived remote availability or reconnect behavior becomes a repeated operational problem

## Phase-1 Implementation Scope

Included now:

- local wrapper `scripts/zellij-mcp-ssh`
- docs describing remote-over-SSH launch
- tests for wrapper quoting, env passing, and quiet stdio behavior

Explicitly not included now:

- remote binary installation
- remote health-check automation
- detached remote daemon lifecycle management
- new MCP transport implementations

## Future Work

### Phase 2 — Remote bootstrap helpers

- copy or sync `zellij_mcp` and `zjctl` to the remote host
- verify remote binary paths
- verify plugin readiness and quiet non-interactive shell startup
- generate ready-to-paste OpenCode MCP config snippets per host alias

### Phase 3 — Detached remote daemon decision

Only worth doing if one or more of these become painful in real usage:

- SSH drops frequently enough to disrupt agent workflows
- startup latency from opening SSH is too high
- multiple local clients need to share the same remote MCP server
- remote availability needs to outlive any single local MCP process
