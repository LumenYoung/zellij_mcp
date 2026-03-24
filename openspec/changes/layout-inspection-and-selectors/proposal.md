## Why

Phase 1 already returns useful pane metadata, but selector support is still narrow and layout information is only exposed as flat candidate lists. Phase 2 should make targeting richer and add a grouped inspection view without introducing destructive layout mutation.

## What Changes

- Extend selector matching with command-, tab-, and focus-oriented filters.
- Add a grouped layout-inspection helper that reports tabs and panes for a session.
- Keep layout mutation and focus-changing actions out of scope.
