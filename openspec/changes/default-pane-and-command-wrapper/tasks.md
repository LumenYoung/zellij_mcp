## 1. Planner Semantics

- [x] 1.1 Update session/tab intent planning so a tab with exactly one reusable shell-like terminal pane is selected automatically.
- [x] 1.2 Add an ambiguity path for tabs with multiple plausible terminal panes, including a caller-visible choice between reusing an existing pane, creating a new pane, or choosing another tab.
- [x] 1.3 Change `new_tab` reconciliation so the new tab's default terminal pane is bound before any additional terminal creation is considered.

## 2. Wrapped Command Presentation

- [x] 2.1 Introduce a clean fish wrapper entrypoint for daemon-submitted complex command text using an opaque payload format.
- [x] 2.2 Make wrapped execution print the decoded original command before execution while preserving the daemon's interaction-marker lifecycle.
- [x] 2.3 Add `-p` preview mode that prints the decoded command without executing it or emitting interaction markers.
- [x] 2.4 Keep a runtime fallback to the current inline wrapper path when the clean fish wrapper is unavailable.

## 3. Verification And Docs

- [x] 3.1 Add targeted tests for single-pane reuse, ambiguous tab targeting, and `new_tab` default-pane binding without implicit extra terminals.
- [x] 3.2 Add targeted tests for complex wrapped command preservation, visible command printing, and `-p` preview semantics.
- [x] 3.3 Run live local QA covering human-visible wrapped execution and history-based `-p` preview inspection.
- [x] 3.4 Update docs to describe the default pane contract, ambiguity behavior, clean wrapper entrypoint, and preview workflow.
