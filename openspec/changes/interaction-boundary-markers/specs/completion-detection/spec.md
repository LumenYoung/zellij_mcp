## ADDED Requirements

### Requirement: Wait distinguishes explicit interaction completion from pane idleness when possible
The daemon SHALL surface explicit interaction completion separately from generic pane idleness when a daemon-submitted shell interaction was explicitly marked.

#### Scenario: Wait reports marked interaction completion
- **WHEN** a daemon-submitted shell interaction reaches its explicit completion marker
- **THEN** `wait` reports that the interaction completed instead of only reporting rendered-idle state

#### Scenario: Wait preserves idle-based fallback for unmarked panes
- **WHEN** no explicit interaction marker is available for the current pane
- **THEN** `wait` keeps the existing idle-based behavior and does not claim stronger completion semantics than it can prove
