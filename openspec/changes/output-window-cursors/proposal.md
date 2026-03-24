## Why

Current capture semantics already support `tail_lines`, but they still return one whole semantic result at a time. Phase 2 should make large captures easier to consume incrementally and optionally strip ANSI control sequences for clients that want normalized text.

## What Changes

- Add line-window chunking to `zellij_capture`.
- Add resumable line cursors.
- Add optional ANSI normalization on capture output.

## Impact

- Affected code: capture request/response shapes, terminal capture logic, docs/backlog.
