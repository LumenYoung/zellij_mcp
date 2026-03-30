## 1. Canonical wrapper embedding

- [x] 1.1 Define the canonical fish wrapper metadata contract, including a stable hash or version mode that the daemon can query.
- [x] 1.2 Embed the canonical wrapper source and expected canonical hash into the binary from a single repo-owned source file.
- [x] 1.3 Add tests that fail if the embedded wrapper identity drifts from the canonical source artifact.

## 2. Lazy bootstrap execution path

- [x] 2.1 Replace the current fish wrapped-submit path with a validate-or-define bootstrap flow that reuses an already-valid wrapper.
- [x] 2.2 Add stale-wrapper detection so a mismatched function is replaced before wrapped command execution.
- [x] 2.3 Ensure repeated wrapped commands in the same shell do not resend the wrapper definition once the canonical wrapper is already loaded.
- [x] 2.4 Keep the legacy inline wrapper as the terminal fallback when bootstrap, validation, or clean invocation fails.

## 3. Verification and documentation

- [x] 3.1 Add targeted tests for missing-wrapper bootstrap, stale-wrapper replacement, canonical-wrapper reuse, and fallback behavior.
- [x] 3.2 Run live local QA showing that a fish pane with no preinstalled wrapper still takes the clean wrapper path after lazy bootstrap.
- [x] 3.3 Update docs to describe binary-owned wrapper distribution, canonical hash validation, and the revised runtime contract.
