## Why

Phase 1 established the correct remote architecture: one local MCP daemon, SSH-backed remote execution, alias-only target selection, bounded readiness remediation, and stable handle-based follow-up routing. What remains is not a control-plane gap, but a confidence gap: real-host operation is still less predictable than local operation, with known pain around remote spawn flakiness, readiness/manual-action recovery, and operator visibility into degraded-but-recoverable states.

Phase 2 should therefore focus on stronger guarantees and better ergonomics for the existing SSH-backed path rather than introducing a new remote-daemon transport model.

## What Changes

- Define reliability requirements for SSH-backed `spawn`, `attach`, `discover`, and handle-routed follow-up operations so successful remote work is not lost when post-launch verification is noisy or delayed.
- Define stronger remote readiness and recovery requirements so blocked states remain actionable, bounded, and observable instead of collapsing into ambiguous failures.
- Require daemon-backed regression coverage and real-host validation for the known remote failure modes that motivated this phase, especially spawn flakiness, helper-client dependence, plugin approval blocking, and degraded preview/capture paths.
- Preserve the existing single local-daemon contract, MCP request shapes, and canonical `ssh:<alias>` target model.

## Capabilities

### New Capabilities
- `remote-operation-reliability`: stronger lifecycle guarantees for SSH-backed spawn, revalidation, and follow-up handle operations.
- `remote-readiness-observability`: actionable, stable remote readiness and manual-recovery behavior for SSH-backed targets.

### Modified Capabilities
- None.

## Impact

- Affected code: `src/services/terminal.rs`, `src/adapters/zjctl/client.rs`, `src/services/router.rs`, and supporting request/response or persistence seams only if required to preserve existing contracts.
- Affected verification: `tests/zellij-mcp-ssh.sh`, targeted Rust unit/integration tests, and real-host manual QA evidence for `a100` and `aws`-class targets.
- Affected docs: `README.md`, `docs/ssh-remote-design.md`, and any phase/backlog notes that describe the remote reliability contract.
- Explicitly not in scope: introducing a second exposed MCP server, nested remote-daemon routing, or new MCP request fields.
