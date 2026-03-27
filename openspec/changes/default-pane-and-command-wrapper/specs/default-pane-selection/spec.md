## ADDED Requirements

### Requirement: Planner reuses the obvious default pane for tab intent
When a request targets a specific session and tab without naming a pane, the system SHALL reuse the tab's single reusable shell-like terminal pane when exactly one such pane exists.

#### Scenario: Single reusable terminal in existing tab
- **WHEN** a request targets session `gpu` and tab `test`, and `test` contains exactly one reusable shell-like terminal pane
- **THEN** the planner selects that pane directly without requiring the caller to specify a pane selector

### Requirement: Planner surfaces ambiguity instead of silently choosing among multiple panes
When a request targets a specific session and tab without naming a pane, and that tab contains multiple plausible reusable terminal panes, the system SHALL not silently choose one of them.

#### Scenario: Multiple plausible panes in target tab
- **WHEN** a request targets a tab that contains more than one reusable shell-like terminal pane
- **THEN** the system returns an ambiguity result that allows the caller to choose an existing pane, create a new pane in the tab, or choose another tab

### Requirement: New-tab spawn binds the tab's default terminal pane
When a request creates a new tab for an interactive shell flow, the system SHALL bind the default terminal pane already present in that new tab before considering creation of any additional terminal pane.

#### Scenario: New tab creates one default terminal
- **WHEN** a request creates a new tab and the resulting tab contains one reusable shell-like terminal pane
- **THEN** the spawn result binds that pane and does not create an additional terminal pane

### Requirement: Additional pane creation requires explicit need
The system SHALL create a second terminal pane in a target tab only when the caller explicitly requests another pane or when the target tab does not contain any reusable shell-like terminal pane.

#### Scenario: New tab already has reusable terminal
- **WHEN** a request creates a new tab whose initial layout already contains a reusable shell-like terminal pane
- **THEN** the system does not create a second terminal pane implicitly
