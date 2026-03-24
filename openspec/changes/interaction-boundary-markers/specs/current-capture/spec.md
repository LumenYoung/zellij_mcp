## ADDED Requirements

### Requirement: Current capture prefers explicit interaction boundaries when available
The daemon SHALL prefer explicit daemon-owned interaction boundaries over heuristic boundary subtraction when a shell-submitted interaction was explicitly marked.

#### Scenario: Current capture returns marked interaction output
- **WHEN** a daemon-submitted shell interaction has explicit start/end markers in the captured output
- **THEN** `capture(current)` returns the output for that marked interaction instead of the broader suffix-after-boundary snapshot

#### Scenario: Current capture preserves legacy fallback when explicit markers are unavailable
- **WHEN** the pane is not eligible for explicit shell wrapping or no explicit markers are present
- **THEN** `capture(current)` keeps the existing best-effort command-boundary and repaint-aware behavior
