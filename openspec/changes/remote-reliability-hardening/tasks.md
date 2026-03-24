## 1. Baseline Remote Reliability Failures

- [x] 1.1 Capture current `a100` remote spawn flakiness as a reproducible daemon-backed regression target.
- [x] 1.2 Capture current blocked-host readiness outcomes, including plugin approval and helper-client preconditions, as explicit acceptance evidence.

## 2. Harden Remote Spawn And Revalidation

- [x] 2.1 Strengthen SSH-backed spawn so successful-but-unsettled launches preserve handles, canonical `ssh:<alias>` ownership, and recoverability through follow-up calls.
- [x] 2.2 Tighten remote revalidation paths in `wait`, `capture`, and `list` so transient post-launch uncertainty does not silently lose or remap remote bindings.
- [x] 2.3 Make degraded discover preview behavior explicit and stable for SSH-backed targets without failing the entire discover operation.

## 3. Strengthen Readiness Diagnostics And Recovery Boundaries

- [x] 3.1 Make remote readiness/manual-action outcomes consistently distinguish missing binaries, helper-client absence, RPC-not-ready states, and human approval blockers.
- [x] 3.2 Keep safe remediation bounded to deterministic user-space actions and verify the daemon retries readiness exactly once where allowed.

## 4. Expand Verification And Operator Guidance

- [x] 4.1 Extend `tests/zellij-mcp-ssh.sh` and targeted Rust tests to cover the new reliability guarantees for spawn, revalidation, and degraded remote flows.
- [x] 4.2 Record real-host daemon-backed QA for one known-good host and one known-blocked host using the updated reliability contract.
- [x] 4.3 Update `README.md`, `docs/ssh-remote-design.md`, and any backlog notes so the documented phase-2 remote reliability contract matches the implemented behavior.

## 5. Stability Hardening Checklist (from VJEPA2 failure investigation)

### P0 (highest priority)

- [x] 5.1 Add explicit daemon/binary freshness evidence at startup and per request (build stamp, version, and process identity) so "old MCP instance still running" is diagnosable immediately.
- [x] 5.2 Strengthen remote backend lifecycle after restart: ensure remote bindings can be revalidated deterministically (not only local startup revalidation) and stale-remote behavior is explicit.

### P1

- [x] 5.3 Harden `spawn(wait_ready=true)` semantics for redraw-heavy/slow remote panes so successful launches degrade predictably (`busy` + recoverable handle) instead of surfacing ambiguous timeouts.
- [x] 5.4 Distinguish remote readiness drift after backend creation (helper-client/RPC/plugin state changes) from transport reachability and missing-binary failures.

### P2

- [x] 5.5 Improve error diagnosability in mixed local/remote flows by reducing ambiguity between selector loss, target configuration errors, and capture-path failures.
- [x] 5.6 Add an operator-facing troubleshooting matrix that maps `TARGET_NOT_FOUND`, `CAPTURE_FAILED`, `PLUGIN_NOT_READY`, and `ZJCTL_UNAVAILABLE` to concrete stage-specific remediation steps.
