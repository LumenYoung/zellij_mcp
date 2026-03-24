## ADDED Requirements

### Requirement: Selector matching supports richer pane metadata filters
The daemon SHALL support richer selector forms for matching existing panes.

#### Scenario: Command selector matches pane command text
- **WHEN** a selector uses `command:<substring>`
- **THEN** panes whose command contains that substring are eligible matches

#### Scenario: Focus selectors match focused state
- **WHEN** a selector uses `focused`, `focused:true`, or `focused:false`
- **THEN** the daemon matches panes by focused state instead of treating that token as unsupported

### Requirement: The daemon supports grouped layout inspection for a session
The daemon SHALL expose a grouped inspection view of tabs and panes for a session.

#### Scenario: Layout inspection groups panes by tab
- **WHEN** layout inspection runs for a session
- **THEN** the response groups panes under their tab names instead of returning only a flat list
