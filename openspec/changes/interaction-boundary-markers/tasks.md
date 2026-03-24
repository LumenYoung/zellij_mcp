## 1. Explicit Interaction State

- [ ] 1.1 Extend persisted observation state with explicit interaction-boundary metadata.
- [ ] 1.2 Add response metadata for explicit interaction completion where needed.

## 2. Shell-Submit Marker Injection

- [ ] 2.1 Detect supported shell-like panes for daemon-submitted shell commands.
- [ ] 2.2 Wrap eligible shell-submit commands with daemon-owned start/end markers while preserving legacy fallback behavior elsewhere.

## 3. Capture And Wait Semantics

- [ ] 3.1 Make `capture(current)` prefer explicit interaction output when markers are present.
- [ ] 3.2 Make `wait` report explicit interaction completion when markers are present, while preserving idle-based fallback for legacy panes.

## 4. Verification And Docs

- [ ] 4.1 Add targeted tests for marked shell-submit flows, unmarked fallback flows, and repaint-heavy fallback behavior.
- [ ] 4.2 Update docs and backlog notes to describe the landed explicit interaction-boundary behavior honestly.
