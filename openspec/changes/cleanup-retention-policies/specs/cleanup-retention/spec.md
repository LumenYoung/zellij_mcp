## ADDED Requirements

### Requirement: The daemon supports explicit cleanup of persisted stale or closed pane state
The daemon SHALL allow callers to explicitly prune persisted pane state for stale or closed handles.

#### Scenario: Cleanup removes matching persisted state
- **WHEN** a cleanup request matches stale or closed handles for a target
- **THEN** the daemon removes those bindings and any associated observations

#### Scenario: Dry-run cleanup reports what would be removed
- **WHEN** a cleanup request is marked as dry-run
- **THEN** the daemon reports the matching handles without deleting them
