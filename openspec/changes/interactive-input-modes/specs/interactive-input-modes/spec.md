## ADDED Requirements

### Requirement: Send input modes distinguish raw terminal input from shell-style submission
The daemon SHALL let callers express raw/TUI input separately from shell-style line submission.

#### Scenario: Explicit raw mode avoids shell-submit semantics
- **WHEN** `zellij_send` is called with `input_mode="raw"`
- **THEN** the daemon sends the request as direct terminal input without submit-line boundary behavior

#### Scenario: Explicit submit-line mode behaves like shell submission
- **WHEN** `zellij_send` is called with `input_mode="submit_line"` and non-empty `text`
- **THEN** the daemon treats the request as a shell-style line submission and preserves the current-boundary behavior used for submitted commands

### Requirement: Explicit input-mode combinations fail fast when contradictory
The daemon SHALL reject explicit send requests that mix incompatible fields.

#### Scenario: Raw mode rejects submit flag
- **WHEN** `zellij_send` is called with `input_mode="raw"` and `submit=true`
- **THEN** the daemon returns an argument error instead of silently converting the request

#### Scenario: Submit-line mode rejects key-only sequences
- **WHEN** `zellij_send` is called with `input_mode="submit_line"` and non-empty `keys`
- **THEN** the daemon returns an argument error instead of guessing how shell submission should combine with named keys

### Requirement: Named-key input supports a broader interactive vocabulary
The daemon SHALL support a broader named-key set for interactive terminal control.

#### Scenario: Extended navigation keys are supported
- **WHEN** `zellij_send` is called with named keys such as `home`, `end`, `delete`, `page_up`, `page_down`, `insert`, or `shift_tab`
- **THEN** the daemon translates them to terminal byte sequences and dispatches them like the existing basic keys

#### Scenario: Generic control chords are supported
- **WHEN** `zellij_send` is called with `ctrl_<letter>` for letters `a` through `z`
- **THEN** the daemon translates that chord to the corresponding control byte sequence
