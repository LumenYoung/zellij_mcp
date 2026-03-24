## Context

The repository has already chosen the remote architecture that phase 2 should preserve: one local MCP daemon, target-aware routing inside that daemon, SSH-backed execution for remote `zjctl` / `zellij` work, and handle-routed follow-up operations keyed by persisted binding ownership. Phase 1 and the alias-only readiness work made remote control viable, but the current docs and real-host evidence still show a reliability gap between “can work” and “works predictably.”

The most important concrete pain points are already known:
- SSH-backed readiness is intentionally bounded and may still stop at `ManualActionRequired` for plugin approval or helper-client dependence.
- Remote spawn can create a real pane while immediate post-launch verification remains noisy, especially around redraw-heavy or timing-sensitive workflows.
- `a100`-class hosts demonstrated that attach/capture can work while spawn remains flaky, which means phase 2 must focus on lifecycle hardening rather than new transport surface area.
- The phase-2 backlog explicitly says this stage should strengthen guarantees and ergonomics instead of expanding the daemon into a broader scheduler or alternate transport system.

## Goals / Non-Goals

**Goals:**
- Strengthen the reliability contract for SSH-backed `spawn`, `attach`, `discover`, `wait`, `capture`, and follow-up routing without changing the external MCP shape.
- Make remote readiness and blocked/manual-action outcomes more diagnosable and operator-usable.
- Expand regression coverage around the exact remote failure modes the repo has already observed: spawn flakiness, helper-client readiness, plugin approval blocking, preview degradation, and alias-only daemon-backed flows.
- Preserve the current single local-daemon ownership model for handles, bindings, observations, and lifecycle state.

**Non-Goals:**
- No nested remote-daemon architecture or second exposed MCP server.
- No new MCP request fields for follow-up tools.
- No attempt to fully automate human-gated remote approval prompts.
- No broader phase-2 feature sweep into richer orchestration, layout mutation, or transport redesign.

## Decisions

### 1. Keep phase 2 inside the existing single-daemon SSH architecture
The current docs already defer persistent remote-daemon transport to a later decision point triggered by measured SSH-overhead pain. The more immediate issues are correctness, recoverability, and observability on the path that already exists.

**Alternatives considered:**
- Introduce a remote daemon now: rejected because it adds state-ownership and restart complexity before current SSH-backed behavior is dependable enough to justify the jump.
- Only improve docs and UX: rejected because the highest-value gap is still operational reliability, especially around spawn and revalidation.

### 2. Treat remote spawn reliability as a lifecycle problem, not just a transport problem
The next plan should harden how the daemon handles successful-but-not-yet-settled remote spawns: preserve handles, preserve canonical target ownership, and make later follow-up calls capable of finishing revalidation instead of losing the launch.

**Alternatives considered:**
- Tighten only SSH transport reuse or connection setup: useful later, but insufficient if a real pane can still be lost to post-launch uncertainty.
- Expand `wait_ready` semantics into a stronger promise immediately: rejected because the docs already avoid promising true application readiness, and redraw-heavy TUIs make that promise unsafe without more validation.

### 3. Keep readiness remediation bounded and human-visible
The current `Ready` / `AutoFixable` / `ManualActionRequired` model is the right boundary. Phase 2 should make those states more actionable and stable, but not broaden automatic remediation beyond safe user-space actions.

**Alternatives considered:**
- Auto-approve plugin prompts or drive arbitrary interactive recovery: rejected as unsafe and inconsistent with the current architecture.
- Collapse helper-client, RPC-not-ready, and plugin approval into one generic remote error: rejected because that hides the exact operator action needed.

### 4. Use verification as a first-class deliverable
For this phase, tests are part of the feature. The change should not be considered complete unless daemon-backed shell coverage and real-host QA both reflect the strengthened reliability contract.

**Alternatives considered:**
- Treat shell/real-host validation as optional evidence outside the spec: rejected because the gap we are closing is confidence on real remote hosts, not only internal code structure.

## Risks / Trade-offs

- **[Spawn flake is partly upstream in Zellij / zjctl]** → Mitigation: encode the daemon-side contract around recoverability and evidence even if the root cause remains timing-sensitive upstream.
- **[Richer diagnostics can drift into unstable message prose]** → Mitigation: anchor improvements on stable error classes and action categories rather than verbose one-off text.
- **[Phase 2 scope can sprawl into transport redesign or broader orchestration]** → Mitigation: keep tasks tied to remote lifecycle hardening, bounded readiness, and verification evidence only.
- **[Real-host validation can be hard to reproduce across environments]** → Mitigation: require one known-good host path and one known-blocked host path, and keep daemon-backed shell harnesses as the repeatable baseline.

## Migration Plan

1. Land additional Rust regression tests and daemon-backed shell coverage before changing behavior that is hard to observe.
2. Harden remote spawn / revalidation / degraded preview behavior while preserving the existing MCP interface and persisted binding semantics.
3. Tighten readiness/manual-action diagnostics and update docs to match the new reliability guarantees.
4. Re-run full local verification plus explicit real-host manual QA on the representative remote targets.

Rollback remains straightforward because this phase does not intentionally change the external MCP schema or introduce a new transport layer.

## Open Questions

- Should phase 2 include the separate SSH connection reuse workstream, or should that remain an independent plan that can land before or after lifecycle hardening?
- What exact evidence format should be required for real-host QA so future reviews can cite it directly without relying on conversation transcripts?
- Do we want to formalize a stable machine-readable subcategory for manual-action-required outcomes, or is the current error-code plus guidance model sufficient?
