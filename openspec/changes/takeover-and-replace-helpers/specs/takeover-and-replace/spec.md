## ADDED Requirements

### Requirement: The daemon supports one-step takeover of a uniquely matching existing pane
The daemon SHALL allow callers to search for an existing pane and attach it in one step when the match is unique.

#### Scenario: Unique takeover search attaches the matching pane
- **WHEN** a takeover request identifies exactly one pane in the target session
- **THEN** the daemon attaches that pane, creates a managed handle, and establishes a baseline

#### Scenario: Ambiguous takeover search fails explicitly
- **WHEN** a takeover request matches multiple panes
- **THEN** the daemon returns an explicit selector ambiguity error instead of choosing arbitrarily

### Requirement: The daemon supports cooperative replace for supported shell-like panes
The daemon SHALL let callers cooperatively reuse a supported shell-like managed pane for a new shell command.

#### Scenario: Replace reuses the same handle on a supported shell pane
- **WHEN** a replace request targets a supported shell-like managed pane
- **THEN** the daemon interrupts the current interaction, submits the replacement shell command, and keeps using the same handle

#### Scenario: Replace rejects unsupported panes
- **WHEN** a replace request targets a pane whose command model is not a supported shell-like environment
- **THEN** the daemon returns an argument error instead of pretending it can universally replace arbitrary pane processes
