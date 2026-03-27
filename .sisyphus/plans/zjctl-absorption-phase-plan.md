# Zjctl Absorption Phase Plan

## TL;DR
> **Summary**: Absorb `zjctl` into this repo in controlled phases so the daemon owns protocol, selector, transport, and plugin behavior directly, while preserving the current MCP contract and explicitly redesigning spawn as a detached launch-and-reconcile workflow that fixes the current remote spawn hang.
> **Deliverables**:
> - in-repo ownership of `zjctl-proto` types and selector semantics
> - in-repo ownership of steady-state pane operations (`discover`, `attach`, `send`, `wait`, `capture`, `close`)
> - spawn redesigned as bounded detached launch + pane reconciliation instead of synchronous blocking launch
> - in-repo ownership of plugin build/version/readiness lifecycle
> - preserved MCP request/response contract throughout the migration
> - explicit phase checkpoints and acceptance criteria for each migration boundary
> **Effort**: High
> **Parallel**: NO
> **Critical Path**: 1 -> 2 -> 3 -> 4 -> 5 -> Final Verification Wave

## Context
### Original Request
Convert the proposed `zjctl` absorption strategy into a concrete implementation plan file with checkpoints and acceptance terms, and include how the new architecture should solve the current remote spawn failures.

### Interview Summary
- Current repo already isolates terminal control behind `src/adapters/zjctl/*` and `trait ZjctlAdapter`.
- The real control plane currently lives outside the repo in `~/Documents/git/zjctl`, split into `crates/zjctl`, `crates/zjctl-proto`, and `crates/zrpc`.
- Current remote discover/layout/attach flows can succeed after readiness fixes, but remote spawn still fails or hangs.
- Live and fresh-daemon repro both showed that the current spawn failure is not an MCP restart problem; the deeper launch path (`zjctl pane launch` / remote `zellij run`) can block.
- The desired long-term direction is to absorb `zjctl` into this repo for tighter control, fewer runtime dependencies, and direct ability to add agent-oriented features.

### Key Planning Decisions
- Preserve the current MCP tool names and request/response shapes during the migration.
- Treat `spawn` as a separate subsystem from steady-state pane operations.
- First remove the external `zjctl` binary dependency, then internalize steady-state operations, then redesign spawn.
- Keep `zellij` itself external; own protocol/plugin/runtime coordination inside this repo.
- Record every intentionally deferred capability in docs so later phases stay explicit.

## Work Objectives
### Core Objective
Turn the current MCP daemon from a thin wrapper over an external `zjctl` binary into an in-repo owned Zellij control plane, while preserving the public MCP contract and solving the current remote spawn hang via a detached launch-and-reconcile architecture.

### Deliverables
- internal workspace ownership of `zjctl-proto` request/response/selector types
- internal backend ownership of list/resolve/send/wait/capture/close behavior
- structured handle/binding/observation model that survives daemon restarts
- detached spawn subsystem that returns `ready` or recoverable `busy` instead of hanging indefinitely
- internal ownership of plugin build/install/readiness/version handshake behavior
- docs capturing implemented behavior and the still-deferred backlog

### Definition of Done (verifiable conditions with commands)
- `cargo test` exits `0`
- `cargo build --release` exits `0`
- a fresh daemon can complete `discover/layout/attach/send/wait/capture/close` on local + remote targets without an external `zjctl` binary on PATH
- the reproduced remote spawn scenario no longer hangs the caller indefinitely and returns either `ready` or recoverable `busy`
- plugin/runtime readiness checks are emitted by this repo’s owned logic instead of relying on external `zjctl doctor` semantics

### Must Have
- current MCP request/response schema remains stable throughout the migration
- `target`, `handle`, selector semantics, and binding persistence behavior remain stable for agents
- each phase has a checkpoint proving parity before the next phase begins
- spawn is redesigned as a bounded detached workflow, not patched with unbounded retries
- protocol + plugin version compatibility becomes repo-owned before final cleanup
- deferred work is documented explicitly for later phases

### Must NOT Have (guardrails, scope boundaries)
- Must NOT rewrite everything in one step without parity checkpoints
- Must NOT change MCP request fields or force client-side workflow changes during absorption
- Must NOT depend on an external `zjctl` binary after Phase 1 checkpoint passes
- Must NOT treat spawn as “just another pane RPC” once Phase 3 begins
- Must NOT hide plugin/version drift behind vague errors
- Must NOT claim that `zellij` itself or SSH transport is absorbed into this repo

## Verification Strategy
> ZERO HUMAN INTERVENTION — all checkpoints should be provable by agent-run commands, live MCP calls, or black-box shell scenarios.
- Test decision: parity-first migration with checkpointed regression coverage
- QA policy: each phase must pass its checkpoint before the next phase starts
- Evidence: `.sisyphus/evidence/phase-{N}-{slug}.{ext}`

## Execution Strategy
### Architecture Direction
- Keep `TerminalService`, `TargetRouter`, persistence, and MCP transport as the stable outer shell.
- Replace the current adapter implementation incrementally behind the existing backend seam.
- Split the migration into two backend categories:
  - **steady-state ops**: discover/attach/send/wait/capture/close/list
  - **spawn ops**: launch + reconcile + busy/ready lifecycle
- Vendor the protocol and plugin sources into this repo so versioning is owned locally.

### Parallel Execution Waves
Wave 1: Phase 1
Wave 2: Phase 2
Wave 3: Phase 3
Wave 4: Phase 4
Wave 5: Phase 5

### Dependency Matrix (phase level)
- 1 blocks 2, 3, 4, 5
- 2 blocks 3, 4, 5
- 3 blocks 4, 5
- 4 blocks 5
- 5 blocks Final Verification Wave

## TODOs
> Implementation + Test = ONE phase task. Never separate.
> EVERY phase MUST end with a checkpoint and explicit acceptance criteria.

- [x] 1. Absorb protocol and transport internals while preserving the current backend seam

  **What to do**: Bring `zjctl-proto` and the reusable `zjctl` transport/client logic into this repo’s workspace. Keep the current outer service/MCP architecture intact, and replace external `zjctl` binary invocation with in-repo Rust logic behind the existing adapter seam.
  **Must NOT do**: Do not redesign spawn yet. Do not change MCP schemas. Do not rename public abstractions yet.

  **Checkpoint**:
  - The daemon no longer requires an external `zjctl` binary at runtime for local or remote steady-state operations.
  - Existing MCP tools continue to function with unchanged request/response shapes.

  **Acceptance Criteria**:
  - [x] `cargo test` exits `0`
  - [x] `cargo build --release` exits `0`
  - [x] Local `discover/layout/attach/send/wait/capture/close` still work after removing external `zjctl` runtime dependency
  - [x] Remote `discover/layout` still work on both `aws` and `a100`
  - [x] Runtime no longer depends on `ZJCTL_BIN` for normal operation

  **Phase 1 status (2026-03-26)**:
  - Completed the adapter-side absorption by vendoring the minimum useful `zjctl-proto` RPC contract into `src/adapters/zjctl/protocol.rs` and replacing local/remote steady-state adapter calls with in-repo `zellij pipe` + `zellij action` logic in `src/adapters/zjctl/client.rs`.
  - Preserved `TerminalService`, `TargetRouter`, persistence, MCP tool names, and request/response shapes. The migration stayed behind the existing `ZjctlAdapter` seam.
  - Local live verification succeeded with `ZJCTL_BIN=/does/not/exist`: `discover`, `layout`, `spawn`, `attach`, `send`, `wait`, `capture`, and `close` all completed against a temporary `phase1-smoke` Zellij session. Evidence is recorded in `.sisyphus/evidence/phase-1-no-external-binary.txt`.
  - Contract parity verification is recorded in `.sisyphus/evidence/phase-1-contract-parity.txt`.
  - Fresh-daemon remote verification also succeeded on both `aws` and `a100`: `zellij-discover` and `zellij-layout` returned successful results without relying on an external local `zjctl` binary.

  **Findings**:
  - The useful Phase 1 absorption boundary was smaller than the full `zjctl` workspace: the repo only needed the JSON-RPC request/response contract, method names, and transport behavior that talks to the existing `zrpc` plugin over `zellij pipe`.
  - `capture`, `wait`, and `close` are now driven by direct `zellij action dump-screen` / `close-pane` calls using explicit pane ids instead of the old `zjctl` CLI text contract.
  - Remote backend construction no longer hard-fails just because `remote_zjctl_bin` cannot be resolved. Remote `zellij` remains the required binary for Phase 1 runtime.

  **Deferred / blockers for later phases**:
  - Full remote steady-state pane-loop QA (`attach/send/wait/capture`) is still Phase 2 work; Phase 1 only closed the remote `discover/layout` parity checkpoint.
  - The `zrpc` plugin artifact and readiness lifecycle are still external to this repo in Phase 1; only the client-side protocol and transport ownership were absorbed here.
  - Spawn was intentionally not redesigned as detached launch + reconciliation yet. The adapter still preserves the existing bounded behavior, and full spawn semantics remain Phase 3 work.

  **QA Scenarios**:
  ```text
  Scenario: No external zjctl runtime dependency for steady-state ops
    Tool: Bash + live MCP
    Steps: Build the repo, run the daemon-backed flow, and verify steady-state ops still succeed without relying on PATH-provided zjctl.
    Expected: Local and remote discovery/layout/attach-style operations still succeed.
    Evidence: .sisyphus/evidence/phase-1-no-external-binary.txt

  Scenario: Current MCP contract stays stable
    Tool: Bash
    Steps: Compare tool names/request fields before and after the change and run the full test suite.
    Expected: No schema drift; `cargo test` exits 0.
    Evidence: .sisyphus/evidence/phase-1-contract-parity.txt
  ```

- [x] 2. Internalize steady-state pane operations and observation semantics

  **What to do**: Move selector resolution, pane listing, `attach`, `send`, `wait-idle`, `capture(full|delta|current)`, and `close` fully into repo-owned backend logic. Reduce dependence on stderr substring classification and make observation semantics explicit and structured.
  **Must NOT do**: Do not redesign spawn here. Do not weaken current handle semantics. Do not silently change `delta`/`current` behavior without documenting it.

  **Checkpoint**:
  - The daemon fully owns steady-state pane semantics behind the repo adapter/service seam, with a repo-owned remote compatibility fallback for older Zellij action surfaces that cannot target pane ids.
  - `full`, `delta`, and `current` capture semantics are documented and tested.

  **Acceptance Criteria**:
  - [x] `cargo test` exits `0`
  - [x] `cargo build --release` exits `0`
  - [x] `discover -> attach -> send -> wait -> capture -> close` succeeds locally
  - [x] `discover -> attach -> send -> wait -> capture` succeeds on `aws`
  - [x] `discover -> attach -> send -> wait -> capture` succeeds on `a100`
  - [x] first `delta` call establishes baseline and later `delta` returns only newer output
  - [x] `current` capture is implemented as documented best-effort command-boundary output

  **QA Scenarios**:
  ```text
  Scenario: Steady-state remote pane loop works end to end
    Tool: live MCP
    Steps: Run discover/attach/send/wait/capture on aws and a100.
    Expected: Both hosts complete the loop without relying on old binary wrapper behavior.
    Evidence: .sisyphus/evidence/phase-2-remote-pane-loop.txt

  Scenario: Capture semantics are stable
    Tool: Bash + targeted tests
    Steps: Run targeted capture tests for full/delta/current and then full cargo test.
    Expected: Baseline, delta, and current semantics match docs.
    Evidence: .sisyphus/evidence/phase-2-capture-semantics.txt
  ```

  **Phase 2 status (2026-03-26)**:
  - Hardened `TerminalService` so `capture`, `wait`, and `close` no longer overwrite refreshed selector/pane-id state after revalidation, and follow-up operations recreate missing `TerminalObservation` state instead of failing stale-but-valid handles.
  - Added real regressions for refreshed-selector persistence and observation self-healing, then reran the full Rust suite successfully (`134` tests) and a release build successfully.
  - Proved the local steady-state loop on a temporary `phase2-smoke` session with `ZJCTL_BIN=/does/not/exist`: `discover -> attach -> send -> wait -> capture -> close` completed and captured `phase2-local-ok`.
  - Proved the remote steady-state loop on `aws`: fresh-daemon `attach -> send -> wait -> capture` succeeded and captured `phase2-aws-ok`.
  - Proved the remote steady-state loop on `a100`: after restoring the detached helper-client path and confirming `zjctl doctor` was healthy, fresh-daemon `attach -> send -> wait -> capture` succeeded and captured `phase2-a100-ok`.

  **Findings**:
  - The biggest Phase 2 correctness bug was state, not transport: revalidation wrote fresh selector metadata to disk, but later steady-state operations could silently write an older in-memory binding back over it.
  - Older remote Zellij builds on `aws` and `a100` expose `dump-screen` and `close-pane` only for the focused pane, so pane-targeted steady-state operations need a compatibility fallback to remote `zjctl pane capture|wait-idle|close`.
  - `a100` needed the same detached helper-client remediation path that earlier restored RPC health before rendered-screen capture became usable again.

  **Deferred / blockers for later phases**:
  - The remote compatibility fallback still shells out through remote `zjctl pane *` commands on hosts with older Zellij action surfaces; removing that compatibility layer cleanly is now tied to later ownership of plugin/runtime/version behavior.
  - Spawn redesign remains intentionally deferred to Phase 3.

- [x] 3. Redesign spawn as detached launch plus reconciliation

  **What to do**: Replace the current synchronous launch path with a detached spawn workflow that (a) submits launch, (b) creates a provisional handle, (c) reconciles the new pane from before/after state and markers, and (d) returns `ready` or recoverable `busy` instead of hanging the caller. Apply this to both `new_tab` and `existing_tab` spawn flows.
  **Must NOT do**: Do not keep unbounded blocking launch semantics. Do not collapse all degraded states into generic `SPAWN_FAILED`.

  **Checkpoint**:
  - The reproduced remote spawn hang no longer blocks the caller indefinitely.
  - Spawn returns a recoverable state (`ready` or `busy`) even when launch stabilization is delayed.

  **Acceptance Criteria**:
  - [x] `cargo test` exits `0`
  - [x] `cargo build --release` exits `0`
  - [x] `zellij_spawn(new_tab)` on `aws` returns within bounded time
  - [x] `zellij_spawn(existing_tab)` on `aws` returns within bounded time
  - [x] `zellij_spawn(new_tab)` on `a100` returns within bounded time
  - [x] `zellij_spawn(existing_tab)` on `a100` returns within bounded time
  - [x] when immediate readiness is unavailable, spawn returns a recoverable `busy` handle instead of only hard failure
  - [x] follow-up `wait/capture/list` can recover the launched pane after a `busy` spawn result

  **QA Scenarios**:
  ```text
  Scenario: Reproduced remote spawn no longer hangs
    Tool: live MCP + fresh daemon via mcp2cli
    Steps: Re-run the aws/a100 spawn repro cases that previously timed out or returned SPAWN_FAILED.
    Expected: Calls return in bounded time and produce either `ready` or recoverable `busy`.
    Evidence: .sisyphus/evidence/phase-3-remote-spawn.txt

  Scenario: Busy spawn is recoverable
    Tool: live MCP
    Steps: Force a delayed-start case, then follow with wait/capture/list.
    Expected: The provisional handle is usable and can be reconciled to a real pane.
    Evidence: .sisyphus/evidence/phase-3-busy-recovery.txt
  ```

  **Phase 3 status (2026-03-26)**:
  - Reshaped remote spawn into a detached launch path that persists a provisional spawned binding/observation before any post-launch probing, and returns the stored handle immediately for detached SSH launch paths.
  - Added spawn reconciliation hints to `TerminalObservation` and used them to keep spawned handles recoverable without changing MCP request/response shapes.
  - Removed two pre-launch fresh-daemon blockers from the SSH path: startup no longer preloads persisted remote backends, and detached remote spawn no longer pays the redundant `is_available()` probe before returning a provisional handle.
  - Added service-side guards so remote selector-only spawned handles no longer force eager availability/revalidation in `list`, and added adapter-side selector-direct follow-up paths so remote `wait` / `capture` / `close` can operate on the stored selector instead of first requiring RPC selector resolution.
  - Added adapter-level SSH subprocess timeouts so remote `zjctl` probe/capture/wait paths fail in bounded time instead of wedging the whole MCP request.
  - Fresh-daemon live verification now proves bounded remote spawn on both hosts: `aws` and `a100` `new_tab` / `existing_tab` return `status="busy"` in bounded time instead of hanging.
  - Fresh-daemon live verification on `a100` initially only proved bounded degraded follow-up behavior (`list` returned quickly and `wait` / `capture` failed in bounded time instead of hanging), which isolated the remaining blocker to host-side plugin/RPC readiness rather than spawn design.
  - Broader host recovery on `a100` showed the live Zellij session itself was still healthy while RPC was not: `zellij list-sessions` succeeded, helper tmux sessions were present, but `zjctl doctor` and `zjctl panes ls --json` still timed out until an explicit bounded `zellij action launch-plugin "file:$HOME/.config/zellij/plugins/zrpc.wasm"` repaired the plugin state.
  - After that broader recovery, raw readiness (`zjctl doctor`, `zjctl panes ls --json`), fresh-daemon `discover` / `layout`, and the full `busy -> ready` follow-up proof (`list`, `wait`, `capture`) all succeeded again on `a100` for both `new_tab` and `existing_tab` spawn modes.

  **Findings**:
  - The original Phase 3 blocker was no longer the detached launch itself; it was synchronous remote prep before the provisional handle could be returned.
  - After removing those pre-launch blockers, the next bottleneck was follow-up recovery: remote busy-handle `wait` / `capture` still forced readiness/revalidation work before reaching the stored selector path.
  - The a100 regression was narrower than a dead session: the host could still report healthy `zellij list-sessions` output while plugin/RPC readiness remained broken.
  - The decisive broader recovery was an explicit bounded plugin launch into the live session. Helper presence alone was not sufficient once the remote plugin state drifted.
  - Once plugin/RPC readiness was restored, the detached launch + reconciliation design proved recoverable busy semantics end-to-end on a fresh daemon.

  **Deferred / blockers for later phases**:
  - Phase 3 acceptance is now complete, but the broader recovery finding matters for Phase 4: repo-owned readiness/version logic should distinguish live-session health from plugin/RPC drift and surface explicit remediation when an additional plugin launch is needed.
  - The remote compatibility fallback still depends on `zjctl`/plugin runtime health on older Zellij hosts, so long-term elimination of this operator-sensitive layer remains coupled to Phase 4 ownership of plugin lifecycle and readiness semantics.

- [x] 4. Own plugin lifecycle, readiness, and version handshake

  **What to do**: Vendor the `zrpc` plugin source into this repo, build the plugin artifact from this workspace, implement daemon/plugin compatibility checks, and move install/doctor/readiness semantics under this repo’s control.
  **Must NOT do**: Do not leave protocol drift unmanaged. Do not treat plugin/version mismatches as generic runtime failures.

  **Checkpoint**:
  - This repo builds both daemon and plugin artifacts.
  - Readiness/version diagnostics are emitted by repo-owned logic.

  **Acceptance Criteria**:
  - [x] `cargo test` exits `0`
  - [x] `cargo build --release` exits `0`
  - [x] the workspace produces the daemon binary and plugin artifact together
  - [x] readiness diagnostics distinguish missing plugin, permission prompt, helper client missing, rpc not ready, and version mismatch
  - [x] local, `aws`, and `a100` bootstrap/readiness paths can be explained entirely from this repo’s docs and outputs

  **QA Scenarios**:
  ```text
  Scenario: Daemon and plugin are version-compatible
    Tool: Bash + targeted tests
    Steps: Build both artifacts and run compatibility/readiness tests.
    Expected: Compatible builds pass; mismatches fail with explicit diagnostics.
    Evidence: .sisyphus/evidence/phase-4-version-handshake.txt

  Scenario: Repo-owned readiness matrix is actionable
    Tool: live MCP + shell verification
    Steps: Trigger representative readiness states and inspect structured outputs.
    Expected: Errors are classified precisely, without relying on external zjctl semantics.
    Evidence: .sisyphus/evidence/phase-4-readiness-matrix.txt
  ```

  **Phase 4 status (2026-03-27, completed)**:
  - The adapter now uses workspace-owned `zjctl-proto` as the single RPC wire-format source and validates `response.v` on both local and SSH RPC paths.
  - SSH readiness classification now distinguishes protocol/version mismatch from generic `RpcNotReady` drift, so daemon/plugin incompatibility is surfaced as a separate manual-action state instead of being retried as if a helper or plugin relaunch would fix it.
  - The MCP-facing service/domain boundary now preserves that distinction too: terminal error mapping surfaces protocol skew as `PROTOCOL_VERSION_MISMATCH` instead of flattening it into the broader `PLUGIN_NOT_READY` bucket.
  - Safe SSH remediation remains bounded and deterministic: helper startup and explicit plugin launch are still attempted for missing-binary/helper/RPC drift cases, but not for protocol mismatch.
  - Added focused regressions for protocol mismatch classification, remediation skipping, and service-layer domain error mapping, then reran the full Rust suite successfully (`143` tests) and a release build successfully.
  - Operator-facing docs now describe `PROTOCOL_VERSION_MISMATCH` separately from `PLUGIN_NOT_READY`, so the published troubleshooting surface matches the current MCP error model.
  - Live regression follow-up on the latest release uncovered two runtime seams that the earlier Phase 3/4 slices had not closed: local spawn was still resolving the wrong pane when `zellij run` created multiple terminals in a new tab, and older remote Zellij builds still rejected pane-targeted `dump-screen` / `close-pane` actions even after the repo stopped depending on textual `zjctl doctor` semantics.
  - The local spawn path now trusts the pane id returned by `zellij run` (including `terminal_<id>` output) before falling back to before/after diff heuristics, which prevents real spawned panes from collapsing into `SPAWN_FAILED` just because runtime tab names differ from requested names.
  - The SSH compatibility path now exports explicit remote binary parent directories into `PATH`, and for older remote Zellij action surfaces it uses repo-owned `pane.focus` RPC plus focused-pane `dump-screen` / `close-pane` actions before falling back to remote `zjctl` commands.
  - Fresh-daemon live proof on 2026-03-26 succeeded for the latest rebuilt release on both backends: local `spawn -> wait -> send -> capture -> close` on `sisyphus-proof-local` captured `proof-local-hello`, and SSH `spawn -> wait -> send -> capture -> close` on `a100` `sisyphus-proof-a100d` captured `proof-a100d-hello`.
  - Full validation after these runtime fixes succeeded with `cargo test` (`147` tests) and `cargo build --release`.

  **Findings**:
  - The previous Phase 4 slice detected protocol drift at parse time but still collapsed it into the broader RPC-not-ready bucket during SSH readiness classification.
  - Version mismatch needs different operator guidance than transient plugin drift because relaunching the same plugin cannot resolve an incompatible daemon/plugin pair.
  - Zellij's own `run` output is a more trustworthy spawn identity source than post-hoc pane-diff heuristics; on live sessions it returns the created pane id even when tab names and focused-pane layouts make after-the-fact matching ambiguous.
  - Older remote Zellij action surfaces are compatible with focused-pane operations even when they do not support `--pane-id`, so the repo-owned RPC focus path is a better compatibility bridge than relying only on remote `zjctl pane *` commands.

  **Deferred / blockers for later phases**:
  - The readiness matrix still needs live/manual proof for local, `aws`, and `a100` states with evidence recorded under the planned Phase 4 evidence files.
  - Shell/bootstrap verification still needs a repo-owned readiness-matrix proof for protocol/version mismatch; the existing SSH shell harness mostly covers older wrapper-era readiness cases and needs a deliberate follow-up instead of a superficial assertion.
  - Docs and bootstrap guidance still describe parts of readiness in terms of external `zjctl`/`doctor` semantics and need further rewriting around repo-owned outputs before the Phase 4 checkpoint can close.

- [x] 5. Clean up abstraction names, compatibility layers, and docs after parity is stable

  **What to do**: Rename history-leaking abstractions like `ZjctlAdapter` to neutral names, remove leftover wrapper-only code paths, unify backend terminology, and align docs with the final owned architecture.
  **Must NOT do**: Do not do this cleanup before earlier phase checkpoints pass. Do not remove compatibility shims that are still needed for regression safety.

  **Checkpoint**:
  - The codebase no longer conceptually depends on external `zjctl`.
  - Public docs describe the final owned backend model clearly.

  **Acceptance Criteria**:
  - [x] `cargo test` exits `0`
  - [x] `cargo build --release` exits `0`
  - [x] core runtime paths no longer require `ZJCTL_BIN`
  - [x] backend abstraction names are neutral and reflect owned implementation
  - [x] docs no longer describe `zjctl` as an external prerequisite for normal operation

  **QA Scenarios**:
  ```text
  Scenario: Final architecture no longer leaks old dependency model
    Tool: Bash + grep
    Steps: Search the repo for obsolete external-zjctl dependency guidance and run the full test/build suite.
    Expected: Old dependency language is removed where no longer true; full verification stays green.
    Evidence: .sisyphus/evidence/phase-5-final-cleanup.txt
  ```

## Phase Checkpoint Summary
- **Checkpoint A (after Phase 1)**: external `zjctl` binary removed from normal runtime path; contract unchanged
- **Checkpoint B (after Phase 2)**: steady-state pane operations fully owned in-repo and stable on local + remote
- **Checkpoint C (after Phase 3)**: remote spawn no longer blocks indefinitely; recoverable `busy` semantics exist
- **Checkpoint D (after Phase 4)**: plugin/version/readiness lifecycle owned in-repo
- **Checkpoint E (after Phase 5)**: final architecture and docs reflect full ownership cleanly

## Final Verification Wave (MANDATORY — after ALL implementation phases)
> 4 review agents run in PARALLEL. ALL must APPROVE.

- [x] F1. Phase Compliance Audit — oracle
- [x] F2. Code Quality Review — unspecified-high
- [x] F3. Real Manual QA — unspecified-high
- [x] F4. Scope Fidelity Check — deep

## Success Criteria
- The repo owns protocol, selector, transport, plugin, and readiness behavior directly
- Current MCP tool contract remains stable throughout the migration
- Remote steady-state operations stay reliable on `aws` and `a100`
- The reproduced remote spawn hang is eliminated through detached launch + reconciliation
- Plugin/version drift is detected and managed by this repo
- The final codebase no longer relies on an external `zjctl` binary or its textual CLI contract for normal operation

## Interactive-First Terminal UX Follow-up TODO

> User requirement: default `spawn` must produce a terminal that is both **human-interactive** and **daemon-manageable**. Agents should express *which session/tab/pane they want*, not manually orchestrate `go-to-tab`, focus, helper-client choreography, or pane creation strategy.

- [x] I1. Fix helper-client geometry so attached/manageable terminals do not render with a tiny or distorted viewport

  **What to do**: Make helper/pseudo-TTY clients start with an explicit, human-usable terminal size instead of inheriting tiny default geometry. Prefer real human client geometry when available.
  **Must NOT do**: Do not let keepalive/helper clients silently define the long-term viewport seen by humans when a real client is already attached.

  **Acceptance Criteria**:
  - [x] local and SSH helper-driven sessions no longer render obviously shrunken panes by default
  - [x] attach remains daemon-manageable after geometry normalization
  - [x] human inspection on `a100` confirms normal interactive pane sizing

- [x] I2. Change `attach` semantics so manageability does not depend on baseline capture succeeding first

  **What to do**: Split "binding/attach success" from "initial baseline capture success" so a healthy visible pane can still be attached and managed even if the first capture attempt needs fallback/retry.
  **Must NOT do**: Do not report attach failure solely because initial capture bootstrapping failed while the target pane itself is alive and selectable.

  **Acceptance Criteria**:
  - [x] attaching to an existing live pane succeeds even when initial capture needs fallback
  - [x] daemon obtains a stable handle without requiring perfect first-capture conditions
  - [x] follow-up capture/send/wait can recover after attach

- [x] I3. Redefine default `spawn` as interactive-first, fish-by-default, and reuse-first

  **What to do**: Make default `spawn` mean "provide a human-interactive shell terminal that the daemon can also manage". Default shell should be `fish`. Prefer reusing an existing single terminal pane in the requested tab before creating new panes/tabs.
  **Must NOT do**: Do not mechanically create a new tab first just because a tab name was provided. Do not require agents to choose low-level placement mechanics when intent is clear.

  **Desired decision order**:
  1. If target tab exists and has one reusable terminal pane → reuse it
  2. Else if target tab exists but needs another pane → create pane inside that tab
  3. Else if target tab does not exist → create tab and terminal there
  4. Default shell for interactive terminals → `fish`

  **Acceptance Criteria**:
  - [x] default spawn uses interactive `fish`
  - [x] specifying a tab with a single terminal pane reuses that pane instead of creating a new tab
  - [x] default spawn does not create extra tabs when a pane in the intended location is sufficient
  - [x] spawned terminal remains human-usable and daemon-manageable

- [x] I4. Introduce location-intent routing so agents specify *where* and *what*, not *how*

  **What to do**: Add an internal planning layer that accepts session/tab/pane intent and chooses whether to focus tabs, focus panes, reuse an existing pane, create a new pane, or attach a helper client.
  **Must NOT do**: Do not force agents to hand-script `go-to-tab`, pane focus, or low-level reuse/new-pane decisions for normal operations.

  **Acceptance Criteria**:
  - [x] agent/tool requests can target session/tab/pane intent directly
  - [x] planner auto-selects `go-to-tab` / focus / reuse / create behavior
  - [x] low-level placement mechanics are hidden behind repo-owned logic

- [x] I5. Extend `send` so it works by location intent, not only by preexisting handle

  **What to do**: Support sending text to a pane selected by session/tab/pane intent, with handle mode remaining supported for existing daemon-managed flows.
  **Must NOT do**: Do not require an agent to attach and persist a handle first when the real task is simply "send this text to that pane".

  **Acceptance Criteria**:
  - [x] `send` can target an existing pane by session/tab/pane intent
  - [x] handle-based follow-up flows still work
  - [x] existing session panes can receive text without spawning new terminals

- [x] I6. Add live QA for human-interactive semantics on both local and `a100`

  **What to do**: Add focused runtime verification for the new interactive-first contract: human can take over the terminal, daemon can still manage it, no redundant tab creation, and default shell is `fish`.
  **Must NOT do**: Do not treat daemon-only manageability as sufficient proof of success.

  **Acceptance Criteria**:
  - [x] local proof: human can directly use the spawned terminal and daemon can still send/capture/close
  - [x] `a100` proof: same dual guarantee holds remotely
  - [x] no extra tab is created when reuse is the intended behavior
  - [x] documented semantics match observed runtime behavior
