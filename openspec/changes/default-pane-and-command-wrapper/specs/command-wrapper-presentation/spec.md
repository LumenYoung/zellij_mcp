## ADDED Requirements

### Requirement: Wrapped execution uses a clean visible entrypoint
For wrapped daemon-submitted shell commands in human-visible fish panes, the system SHALL use a compact wrapper entrypoint instead of printing the full inline interaction script into the pane.

#### Scenario: Human-visible wrapped command submission
- **WHEN** the daemon submits a wrapped shell command to a human-visible fish pane
- **THEN** the pane shows a compact wrapper command rather than the full inline `printf`/`begin` instrumentation script

### Requirement: Wrapper preserves complex command text
The wrapper contract SHALL preserve the full original command text as shell script content rather than reinterpreting it as simple argv tokens.

#### Scenario: Complex command with shell syntax
- **WHEN** the original command contains quoting, pipes, redirects, multiline content, or shell blocks
- **THEN** the wrapper executes the same shell-script semantics as the original command text

### Requirement: Wrapper prints decoded command before execution
The wrapper SHALL print the decoded original command text before executing it so a human can see what the daemon is about to run.

#### Scenario: Executing wrapped command
- **WHEN** the wrapper receives a valid encoded command payload in execution mode
- **THEN** it prints a human-readable command header and the decoded command before running the command

### Requirement: Wrapper supports preview-only mode
The wrapper SHALL support a preview mode that prints the decoded original command without executing it.

#### Scenario: Preview wrapped command
- **WHEN** the wrapper is invoked with `-p`
- **THEN** it prints a preview header and the decoded original command without executing the command payload

### Requirement: Preview mode does not emit execution markers
Preview mode SHALL not emit daemon interaction start/end markers because it does not execute the wrapped command.

#### Scenario: Preview does not create interaction lifecycle
- **WHEN** a user runs the wrapper with `-p`
- **THEN** the wrapper does not produce command-execution markers or runtime side effects
