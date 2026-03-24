## 1. Explicit Input Modes

- [x] 1.1 Add `input_mode` to `SendRequest` with backward-compatible default behavior.
- [x] 1.2 Make `zellij_send` reject invalid explicit mode combinations instead of silently guessing.

## 2. Expanded Key Support

- [x] 2.1 Extend named-key support for navigation and function keys.
- [x] 2.2 Extend named-key support for generic `ctrl_<letter>` chords.

## 3. Verification And Docs

- [x] 3.1 Add targeted tests for explicit raw/submit-line handling and extended key mapping.
- [x] 3.2 Update `docs/mcp-contract.md` and `docs/phase-2-backlog.md` to reflect the landed input-mode/key-sequence behavior.
