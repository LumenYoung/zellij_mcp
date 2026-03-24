## Real-host daemon-backed QA

This change records the phase-2 host evidence that motivated and validated the hardening work.

### Known-good host: `a100`

- SSH batch connectivity succeeded through the daemon-backed alias flow.
- Remote user-space binaries in `~/.local/bin` were sufficient once the host had a native build compatible with the remote glibc.
- A detached helper client restored `zjctl` RPC health on the host.
- After daemon restart, `discover` and `attach` recovered again through the canonical `ssh:a100` path.
- `spawn(wait_ready=true)` could still return a bounded busy/timeout outcome even when the pane existed, which is why the phase-2 contract now treats that result as recoverable instead of fatal.

### Known-blocked host: `aws`

- SSH transport itself was reachable, so the failure mode was not a generic transport outage.
- The daemon observed bounded readiness blockers including plugin approval and helper-client absence.
- Those outcomes now map to explicit `PLUGIN_NOT_READY` guidance instead of collapsing into generic selector or capture failures.
- The blocked path is intentionally preserved as a manual-action-required acceptance case rather than something the daemon tries to auto-approve blindly.

### Operator conclusions

- Old daemon instances and stale remote backend state were a real source of confusion, so startup logs, successful responses, and error payloads now surface daemon identity and build freshness.
- Slow or redraw-heavy remote panes can be real and still miss the bounded `wait_ready` window, so the contract now guarantees a recoverable handle with canonical remote ownership.
- Discover preview capture is best-effort for remote panes; metadata survives even when preview capture does not.
