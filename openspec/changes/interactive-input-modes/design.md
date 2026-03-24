## Context

`zellij_send` currently supports `text`, `keys`, and `submit`. The implementation in `src/services/terminal.rs` already treats `submit=true` as a signal to reset the current-capture boundary, and for attached shell-like panes it also clears pending line input before dispatch. That behavior is useful, but the current API leaves two ambiguities:

- raw TUI input versus shell-style line submission are not clearly separated
- named-key support is too narrow for many interactive tools

## Goals / Non-Goals

**Goals:**
- Make the send contract explicit enough that TUI flows can choose raw input without piggybacking on shell-submit semantics.
- Preserve the existing `submit` behavior for backward compatibility.
- Add broader named-key support without introducing a second send tool.

**Non-Goals:**
- No completion-marker or command-wrapper protocol in this change.
- No pane replacement, queueing, or orchestration work.
- No transport or remote-routing changes.

## Decisions

### 1. Add explicit input modes without removing `submit`
Introduce `input_mode` as an optional field on `SendRequest`.

- `raw`: treat the request as direct terminal input and never use shell-submit semantics.
- `submit_line`: treat the request as a shell-style line submission.
- omitted: preserve the legacy `submit`-driven behavior.

This keeps existing callers working while giving new callers a clearer contract.

### 2. Keep legacy behavior only for legacy requests
When `input_mode` is omitted, `submit` keeps its current meaning.

When `input_mode` is present, invalid combinations should fail fast instead of being silently reinterpreted. In particular:

- `input_mode=raw` with `submit=true` is invalid
- `input_mode=submit_line` with non-empty `keys` is invalid
- `input_mode=submit_line` requires non-empty `text`

### 3. Expand named-key support centrally
Extend the existing `map_key_sequence(...)` helper rather than introducing a second key parser.

This change should add:

- navigation keys: `home`, `end`, `insert`, `delete`, `page_up`, `page_down`, `shift_tab`
- function keys: `f1` through `f12`
- generic control chords: `ctrl_a` through `ctrl_z`

## Risks / Trade-offs

- Some terminals vary on function-key escape sequences. This change should use the common ANSI sequences and document them as best-effort terminal input, not a cross-terminal guarantee.
- Keeping both `submit` and `input_mode` means there is a temporary dual contract, but that is safer than breaking existing callers.

## Migration Plan

1. Add request-shape support for `input_mode`.
2. Add failing tests for explicit raw/submit-line semantics and extended key support.
3. Update `send(...)` and key mapping while preserving legacy behavior.
4. Update the MCP contract docs and phase-2 backlog.
