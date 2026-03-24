## Context

The current repo already has the building blocks for takeover and reuse:

- `discover` can inspect panes before attach
- `attach` can turn a selected pane into a managed handle
- `send` can drive an existing managed pane

What is missing is a more direct helper for common agent flows.

## Goals / Non-Goals

**Goals:**
- Let callers search and attach in one step when they already know the matching criteria.
- Let callers cooperatively reuse a supported shell-like pane for a new command without creating a new handle.

**Non-Goals:**
- No claim of universal OS-level process replacement.
- No layout mutation in this change.
- No broader scheduler/orchestration behavior.

## Decisions

### 1. Takeover remains attach-based under the hood
The helper should still attach an existing pane and establish a baseline. It just performs the candidate search first and fails if the result is zero or ambiguous.

### 2. Replace is cooperative and shell-scoped
Replacement should only work for supported shell-like panes. The daemon will interrupt the current pane input with `Ctrl-C`, then submit a new shell command on the same handle.

That is honest about what the daemon can guarantee without kernel-level process control.

## Migration Plan

1. Add request/response types and MCP tools.
2. Add takeover search/attach and cooperative replace logic in `TerminalService`.
3. Add tests and update docs/backlog.
