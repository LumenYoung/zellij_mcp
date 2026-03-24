## Scope

This slice improves capture output shaping, not backend scrollback internals.

- line-based chunking after semantic capture selection
- resumable line cursors for the same semantic output model
- optional ANSI escape stripping

True scrollback-aware delta and deeper TUI diffing remain out of scope.
