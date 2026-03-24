## ADDED Requirements

### Requirement: Remote readiness failures remain actionable and bounded
The daemon SHALL classify SSH-backed readiness outcomes into actionable states without broadening automatic remediation beyond safe user-space actions.

#### Scenario: Manual approval remains an explicit operator step
- **WHEN** a remote host requires unmanaged plugin approval or another human-only action before RPC can proceed
- **THEN** the daemon returns a stable manual-action-required outcome with guidance instead of trying to auto-approve the prompt

#### Scenario: Safe remediation retries readiness once
- **WHEN** the daemon encounters an auto-fixable remote readiness failure such as missing helper-client state or a safe user-space plugin installation precondition
- **THEN** it applies only the bounded remediation path and retries readiness exactly once before surfacing the final result

### Requirement: Remote readiness messages identify the blocking condition
The daemon SHALL surface stable, operator-usable remote readiness outcomes that distinguish missing binaries, helper-client absence, RPC-not-ready states, and manual approval blocks.

#### Scenario: Missing binary guidance stays explicit
- **WHEN** non-interactive SSH probing still cannot resolve `zjctl` or `zellij` after PATH normalization and documented fallback discovery
- **THEN** the daemon reports which prerequisite is missing and what class of remediation is expected instead of collapsing the result into a generic remote failure

#### Scenario: Manual-action-required outcome stays distinct from transport failure
- **WHEN** SSH connectivity succeeds but remote plugin approval or RPC readiness still blocks use
- **THEN** the daemon reports that condition separately from SSH-unreachable or target-not-found failures

### Requirement: Remote reliability validation includes daemon-backed and real-host evidence
The reliability hardening work SHALL be accepted only with daemon-backed regression coverage and real-host validation for the known remote failure modes.

#### Scenario: Daemon-backed shell regression covers alias-only remote flow
- **WHEN** the remote reliability change is verified in automation
- **THEN** the shell harness exercises the real stdio daemon path for SSH-backed discover, attach, and follow-up capture rather than only helper or wrapper scripts

#### Scenario: Real-host validation records both success and bounded block cases
- **WHEN** the change is validated on representative remote targets
- **THEN** the evidence includes at least one successful daemon-backed remote operation on a known-good host and one bounded manual-action-required outcome on a blocked host
