## ADDED Requirements

### Requirement: Remote spawn outcomes remain recoverable when post-launch verification is noisy
The daemon SHALL preserve a usable remote handle and canonical `target_id` for SSH-backed spawns that successfully create a pane but do not fully settle during immediate post-launch verification.

#### Scenario: Remote spawn succeeds but launch verification does not settle
- **WHEN** an SSH-backed `zellij_spawn` creates a real remote pane but idle detection, baseline capture, or immediate target resolution remains transiently inconclusive
- **THEN** the daemon returns a handle bound to the canonical `ssh:<alias>` target instead of discarding the launch

#### Scenario: Busy remote spawn remains recoverable through follow-up operations
- **WHEN** the daemon returns a remote spawned handle in a provisional or busy state
- **THEN** later `zellij_wait`, `zellij_capture`, or `zellij_list` calls can revalidate that same persisted binding without requiring the caller to resend `target`

### Requirement: Remote follow-up routing remains binding-driven after transient uncertainty
The daemon SHALL continue to route SSH-backed follow-up operations by persisted binding ownership even after transient spawn or capture uncertainty.

#### Scenario: Follow-up operation resolves through persisted remote binding
- **WHEN** a remote handle was created earlier for target `ssh:<alias>`
- **THEN** `zellij_send`, `zellij_wait`, `zellij_capture`, and `zellij_close` route through that persisted binding target without adding new request-time target inputs

#### Scenario: Revalidation does not silently remap remote ownership
- **WHEN** the daemon revalidates a remote binding after a transient target lookup or capture failure
- **THEN** it either preserves the existing remote ownership or surfaces a deterministic stale/missing failure instead of silently remapping the handle to a different backend

### Requirement: Remote discover degrades safely when previews are unstable
The daemon SHALL keep SSH-backed discover usable even when preview capture on some panes is unavailable or too unstable.

#### Scenario: Discover returns metadata-only candidates on preview failure
- **WHEN** an SSH-backed `zellij_discover` request can list remote panes but preview capture fails for one or more candidates
- **THEN** the daemon returns those candidates with metadata intact and marks preview output as unavailable instead of failing the entire discover operation

#### Scenario: Discover preserves canonical target identity during degraded preview
- **WHEN** preview capture is degraded during SSH-backed discover
- **THEN** each returned candidate still exposes the canonical `ssh:<alias>` target identity needed for follow-up attach and debugging
