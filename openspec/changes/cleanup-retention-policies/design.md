## Context

The repo already has close/stale state transitions and persistent registry/observation stores. What it lacks is an explicit lifecycle-management tool for pruning old terminal state.

## Decision

Add a direct cleanup helper instead of trying to smuggle cleanup through unrelated commands. Cleanup will be explicit, target-scoped, and limited to persisted daemon state.

## Scope

- remove stale/closed state by status and optional age threshold
- support dry-run previews
- do not add background workers or delayed spawn queues in this change
