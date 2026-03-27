## Context

The daemon currently mixes two concerns in ways that are visible to users:

1. pane planning still exposes implementation bias, especially on `new_tab`, where the system can effectively create a tab and then create an additional terminal instead of directly binding the tab's default terminal pane;
2. wrapped shell submission preserves strong daemon observability, but the human-visible command line becomes an ugly inline script that leaks internal interaction-marker protocol details.

The user wants the default planner contract to become more direct: agents should usually provide `session` and `tab` intent only, the daemon should pick the obvious pane when there is one, and ambiguity should be surfaced explicitly instead of silently taking a surprising low-level action. The user also accepts keeping wrapped execution, but wants the visible shell entrypoint to be cleaner and auditable, including a preview mode that prints the decoded command without executing it.

Constraints:
- default interactive shell remains `fish`
- complex commands are common, so wrapper design cannot assume a simple argv-style command model
- daemon-managed interaction markers still matter for `wait`, `current`, `delta`, and exit-status tracking
- human-visible panes should not show the full inline instrumentation script by default

## Goals / Non-Goals

**Goals:**
- Make planner defaults session/tab-intent first, with reuse of the obvious terminal pane when unambiguous.
- Make `new_tab` bind the new tab's default terminal pane instead of implicitly creating an extra terminal pane.
- Keep wrapped execution for daemon observability while replacing the current visible inline script with a clean wrapper entrypoint.
- Add a preview flag so a user can inspect the decoded wrapped command without running it.
- Preserve support for complex command text, including pipes, quoting, multiline content, and shell blocks.

**Non-Goals:**
- This change does not redesign `wait`/`capture` semantics away from interaction markers.
- This change does not make all multi-pane targeting automatic; genuine ambiguity still requires confirmation or an explicit low-level choice.
- This change does not require every machine to rely exclusively on dotfiles-managed shell state with no fallback.

## Decisions

### 1. Planner defaults become intent-first and ambiguity-aware
Default planning should treat `session + tab` as sufficient user intent when a tab has exactly one reusable shell-like terminal pane.

If the target tab has multiple plausible terminal panes, the daemon should not silently choose one. Instead it should surface an ambiguity result that gives the caller a choice: reuse a specific existing pane, create a new pane in that tab, or create a different tab.

**Why this over always asking?** It keeps the common case direct while avoiding surprising low-level behavior in genuinely ambiguous layouts.

### 2. `new_tab` binds the tab's default terminal pane
When the caller asks for a new tab, the daemon should create the tab, inspect the resulting tab layout, and bind the default terminal pane that already exists in that tab.

The daemon must not implicitly create a second terminal pane in the new tab unless the caller explicitly requested an additional pane or the new tab does not contain a reusable terminal.

**Why this over keeping launch-first reconciliation?** The current behavior can produce a surprising two-terminal tab, which violates the intended human mental model.

### 3. Wrapped execution uses a stable fish wrapper entrypoint with opaque payload
The daemon should stop printing the full inline `printf ... begin ... end ...` wrapper into human-visible panes. Instead it should submit a compact fish wrapper command such as `__zellij_mcp_run_b64 <payload>`.

The payload should encode the full original command text rather than splitting it into argv. This preserves semantics for complex commands, including quoting, pipes, redirection, multiline script blocks, and shell syntax.

**Why this over argv-style wrapper calls?** Complex commands are a default case here, and argv-style forwarding would break shell semantics.

### 4. The wrapper prints the decoded command before execution
The wrapper should print a short, human-readable header followed by the decoded original command text before it executes the command.

Suggested output form:

```text
# zellij-mcp command:
<decoded original command>
```

This keeps the pane auditable without exposing the full internal instrumentation script.

**Why this over hiding the original command entirely?** Users should still be able to see what the daemon is about to run.

### 5. The wrapper supports `-p` preview mode
The wrapper should support `-p` to print the decoded original command without executing it.

Suggested output form:

```text
# zellij-mcp preview:
<decoded original command>
```

In preview mode, the wrapper should not emit interaction markers and should not execute the decoded command.

**Why this over only documenting the payload format?** Preview mode lets a user inspect or copy the wrapped command by reusing shell history, which matches the user's intended workflow.

### 6. Fish wrapper presence may be assumed for the primary path, but runtime fallback remains required
The primary path may assume a fish function or fish-owned wrapper contract exists across the user's machines via dotfiles. However, the daemon still needs a fallback when that wrapper is unavailable, so the system degrades to the current inline wrapping path rather than failing to submit the command.

**Why this over hard requiring the function?** It preserves robustness while still enabling the cleaner UX on configured machines.

## Risks / Trade-offs

- **Wrapper drift between daemon and fish function** → Keep the wrapper entrypoint contract small and versioned, and retain an inline fallback path when the wrapper is unavailable.
- **Preview and execution paths diverge** → Ensure both modes decode the same payload format and share the same command-rendering logic.
- **Ambiguity prompts introduce new planner branches** → Limit prompting to the genuinely ambiguous multi-pane case and keep the single-pane path automatic.
- **New-tab reconciliation still races on some Zellij versions** → Use before/after layout comparison plus tab-local pane filtering before deciding whether an additional pane is needed.

## Migration Plan

1. Define planner rules for single-pane reuse, ambiguous-tab confirmation, and `new_tab` default-pane binding.
2. Implement tab-local reconciliation for `new_tab` so the first terminal is bound instead of launching an extra one.
3. Introduce the fish wrapper entrypoint and payload format, keeping the existing inline wrapper as fallback.
4. Add preview-mode support and document how users can use shell history plus `-p` to inspect commands.
5. Update manual QA coverage for visible command presentation and pane-count semantics.

## Open Questions

- Should ambiguity surface as a new structured planner error/result, or should it reuse an existing target-selection error shape with richer metadata?
- Should the clean wrapper entrypoint live purely in dotfiles-managed fish functions, or should the repo also ship a canonical wrapper implementation that dotfiles call into?
