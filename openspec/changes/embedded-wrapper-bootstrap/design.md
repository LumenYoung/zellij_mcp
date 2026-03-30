## Context

The daemon now has a cleaner fish wrapper entrypoint, but the current contract still assumes external shell state for the preferred path. In practice, the clean path only works when the target shell already defines `__zellij_mcp_run_b64`; otherwise the daemon falls back to the legacy inline wrapper after detecting an execution failure from pane capture.

That is no longer the desired operational model. The new requirement is that the repo may be absent at runtime and only the built binary may be available. This means the binary itself must own the canonical wrapper definition and must be able to bootstrap that definition into a target fish shell without depending on dotfiles or repo-local scripts. At the same time, the daemon must not redefine the function on every wrapped command, and it must handle drift when a shell already has a different function body under the same function name.

Constraints:
- runtime must work when only the binary is present and the repo tree is unavailable
- fish remains the default interactive shell for wrapper bootstrapping
- wrapped command execution still needs daemon-owned interaction markers and complex shell-text preservation
- bootstrap must be lazy and low-noise rather than redefining the function on every send
- a stale or user-modified wrapper definition must be detectable before reuse

## Goals / Non-Goals

**Goals:**
- Make the binary the runtime source of truth for the canonical fish wrapper implementation.
- Bootstrap the wrapper lazily only when it is missing or does not match the binary-owned canonical definition.
- Detect wrapper drift using a stable binary-owned hash/version contract before invoking the clean wrapper entrypoint.
- Preserve the clean visible command path when bootstrap succeeds.
- Preserve the existing inline wrapper path as a final correctness fallback if bootstrap or validation fails.

**Non-Goals:**
- This change does not eliminate the repo-shipped wrapper script as a development artifact or reference copy.
- This change does not redesign wrapped execution for non-fish shells.
- This change does not require persistent shell installation across sessions; bootstrap may remain session-local.
- This change does not attempt to merge arbitrary user customizations into the canonical wrapper body.

## Decisions

### 1. The binary owns the canonical runtime wrapper body
The canonical fish wrapper source should be embedded into the binary at build time from a repo-owned source file.

The repo copy remains the authoring surface, but runtime correctness must not depend on reading that file from disk. Embedding removes the assumption that the repo checkout exists on the target machine.

**Why this over reading the script from disk at runtime?** The user explicitly wants runtime behavior to work when only the binary is present.

### 2. Wrapper reuse is guarded by a canonical hash contract
The wrapper contract should expose a machine-checkable canonical identity, such as a hash derived from the embedded canonical wrapper body. The daemon should verify that the already-defined shell function reports the same hash before treating it as safe to reuse.

This can be implemented either by embedding a helper subcommand inside the wrapper that prints its canonical hash, or by requiring the wrapper to expose a metadata mode that returns the canonical hash/version string.

**Why this over trusting any existing function with the right name?** Function-name reuse alone is unsafe because a stale or locally modified definition could silently drift from the daemon's expected behavior.

### 3. Bootstrap is lazy: define only when missing or stale
For fish wrapped submit flows, the daemon should first check whether `__zellij_mcp_run_b64` exists and whether its reported hash matches the binary-owned canonical hash. Only if that check fails should the daemon send the canonical wrapper definition into the pane and then invoke it.

This preserves the low-noise, low-overhead path once the wrapper is already valid in the target shell while still self-healing when the shell is missing the function or contains a stale definition.

**Why this over redefining every time?** Constant redefinition adds unnecessary payload, pane noise, and shell churn even in the healthy case.

### 4. Bootstrap and invocation should be a single fish-side transaction
The daemon should send a compact fish bootstrap-and-run payload that performs these steps in order:
1. check whether the wrapper function exists;
2. if it exists, ask it for its canonical hash/version;
3. if missing or mismatched, define the canonical function body from the binary-owned source;
4. invoke the wrapper entrypoint with the encoded command payload.

This should happen within one submitted shell payload so the daemon does not depend on race-prone multi-send coordination.

**Why this over separate "install" and "run" sends?** A single transactional payload is less error-prone and avoids intermediate shell state assumptions between sends.

### 5. The existing inline wrapper remains the terminal fallback
If wrapper bootstrap, validation, or invocation still fails at runtime, the daemon should retain the current inline wrapper as the final fallback path so commands continue to execute correctly.

**Why this over failing hard after bootstrap errors?** The clean wrapper path is a UX improvement, but command execution reliability remains the higher-order requirement.

## Risks / Trade-offs

- **Embedded wrapper and repo source drift apart during development** → Generate the embedded payload from one canonical repo-owned file and test that the binary hash matches the source-derived hash.
- **Hash-check protocol adds wrapper complexity** → Keep the metadata interface tiny, e.g. a `--hash`/`--version` mode with one stable output string.
- **Bootstrap payload becomes harder to escape safely** → Treat the embedded wrapper body as data and use a robust transport/quoting strategy rather than hand-built ad hoc escaping.
- **Session-local bootstrap means fresh panes still pay first-use cost** → Accept one-time per-shell bootstrap overhead in exchange for no persistent install requirement.
- **User-customized wrapper bodies get overwritten when stale** → Make the canonical wrapper contract intentionally narrow and documented; user customization outside that contract is out of scope.

## Migration Plan

1. Define a canonical wrapper metadata contract that can report the wrapper hash/version from inside fish.
2. Embed the canonical wrapper source and expected hash into the binary at build time.
3. Replace the current fish wrapper invocation path with lazy validate-or-define bootstrap logic.
4. Keep the inline wrapper fallback behind the bootstrap path until validation shows the bootstrap contract is reliable.
5. Update docs to state that repo/dotfiles presence is optional for runtime correctness because the binary can self-bootstrap the wrapper.

## Open Questions

- Should the wrapper expose `-H/--hash`, `--version`, or another metadata mode for daemon validation?
- Should bootstrap replace a stale wrapper unconditionally, or should it support a stricter operator-visible warning path before replacement?
- Should the repo-shipped script be generated from the same embedded template source, or remain the canonical authored file that the build embeds directly?
