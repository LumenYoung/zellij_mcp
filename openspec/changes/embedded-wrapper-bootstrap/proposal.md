## Why

The current wrapper contract still depends on external fish state for the clean path: if the target shell does not already define `__zellij_mcp_run_b64`, the daemon has to fall back to the legacy inline wrapper. We now want the binary itself to carry the canonical wrapper implementation so the clean path works even when the repo is absent and dotfiles have not been synced.

## What Changes

- Embed the canonical fish wrapper implementation into the binary so runtime wrapper bootstrap does not depend on the repo being present on disk.
- Change fish submit behavior from "assume wrapper exists, then fallback" to a lazy bootstrap flow that checks whether the wrapper is present and whether it matches the binary's canonical implementation before use.
- Add a stable wrapper-version or wrapper-hash contract so the daemon can detect drift between an already-defined fish function and the binary-owned canonical wrapper.
- Redefine the wrapper only when it is missing or stale, rather than re-sending the function body on every wrapped command.
- Preserve the existing inline wrapper as the final fallback path if bootstrap or wrapper validation fails at runtime.

## Capabilities

### New Capabilities
- `command-wrapper-bootstrap`: binary-embedded wrapper distribution, runtime wrapper validation, and lazy wrapper bootstrap for fish-backed wrapped submit flows.

### Modified Capabilities
- `command-wrapper-presentation`: the clean visible wrapper entrypoint now depends on a binary-owned bootstrap contract instead of an external dotfiles/repo presence assumption.

## Impact

- Affected code: `src/services/terminal.rs`, build-time asset embedding for the fish wrapper, any wrapper metadata/hash plumbing, and the canonical wrapper source currently represented by `scripts/__zellij_mcp_run_b64.fish`.
- Affected runtime behavior: fish wrapped submit flows, wrapper discovery/validation, wrapper drift handling, and fallback behavior when bootstrap fails.
- Affected docs: wrapper operator guidance, fish integration guidance, and command-wrapper behavior docs/specs.
