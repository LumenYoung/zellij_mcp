## ADDED Requirements

### Requirement: Capture supports chunked line windows with resumable cursors
The daemon SHALL allow callers to read semantic capture output in line-based windows.

The forward line-window contract includes explicit `line_offset` starts, `line_limit` bounds, and resumable `cursor` values for supported semantic capture modes.

#### Scenario: Capture returns a chunk and a next cursor
- **WHEN** a capture request sets a line limit smaller than the available semantic output
- **THEN** the daemon returns the selected line window plus a `next_cursor` that can resume after that window

#### Scenario: Capture can resume from a cursor
- **WHEN** a capture request provides a valid cursor from a previous response
- **THEN** the daemon resumes the line window from that cursor instead of starting at the beginning

#### Scenario: Capture can start from an explicit line offset
- **WHEN** a capture request provides `line_offset` and `line_limit`
- **THEN** the daemon returns the requested forward semantic line window starting at that offset

#### Scenario: Delta mode rejects forward line-window cursors
- **WHEN** a capture request uses `mode="delta"` together with `line_offset`, `line_limit`, or `cursor`
- **THEN** the daemon rejects the request instead of claiming resumable paging semantics it does not persist across delta captures

### Requirement: Capture optionally strips ANSI control sequences
The daemon SHALL optionally normalize ANSI escape sequences out of semantic capture output.

#### Scenario: ANSI normalization strips escape codes from output
- **WHEN** capture is requested with ANSI normalization enabled
- **THEN** the daemon returns text with ANSI escape sequences removed while preserving semantic line chunking behavior
