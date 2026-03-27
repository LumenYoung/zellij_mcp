## Why

The current interactive shell experience still leaks daemon implementation details into human-visible panes and can choose a less direct pane strategy than users expect. We need a cleaner default contract so agents express only session/tab intent, humans can understand what will run, and the daemon still preserves reliable interaction tracking.

## What Changes

- Introduce a default-pane planning contract that lets agents specify `session` and `tab` intent by default, reuses the obvious terminal pane when the tab is unambiguous, and only asks for confirmation when a target tab has multiple plausible panes.
- Refine `new_tab` behavior so creating a new tab binds to that tab's default terminal pane instead of implicitly creating an additional terminal pane unless the caller explicitly asks for another pane.
- Replace the current human-visible inline interaction wrapper with a cleaner wrapper entrypoint for complex commands, while preserving daemon-owned interaction markers internally.
- Add a wrapper preview mode so users can inspect the decoded command that would run without executing it.

## Capabilities

### New Capabilities
- `default-pane-selection`: planner and spawn semantics for reuse-first default pane selection, ambiguity handling, and new-tab default-pane binding.
- `command-wrapper-presentation`: clean human-visible wrapped-command execution with preview support for complex commands.

### Modified Capabilities
- None.

## Impact

- Affected code: `src/services/terminal.rs`, `src/services/router.rs`, `src/adapters/zjctl/client.rs`, request/response surfaces involved in spawn/send planning, and shell-wrapper generation paths.
- Affected runtime behavior: default tab targeting, new-tab pane reconciliation, human-visible shell command presentation, and command preview flows.
- Affected docs: MCP contract docs, architecture docs, and any operator guidance covering interactive shell semantics.
