# Task Plan: rc.3 Workspace Recovery Repair

## Goal
Recover affected rc.3 workspaces that were already converted by the first rc.4 repair, without weakening fail-closed handling for unrelated or incomplete state.

## Next Step
Commit the verified recovery repair.

## Current Phase
Complete

## Phases

### Phase 1: Diagnose the production failure
- [x] Confirm the desktop build pins the latest Engine repair.
- [x] Replay the latest diagnostic log and reproduce the exact failure.
- [x] Identify the gap between the migration tests and the real upgrade sequence.
- **Status:** complete

### Phase 2: Write the regression first
- [x] Model an rc.3 legacy-origin state with normal nonterminal phases.
- [x] Write it using the early rc.4 V3 layout with lost provenance.
- [x] Prove the current code fails to restore provenance after reopen.
- **Status:** complete

### Phase 3: Apply the smallest durable fix
- [x] Restore provenance from stable legacy evidence without requiring `Complete`.
- [x] Preserve rejection for new, removed, failed, or evidence-free states.
- [x] Update the architecture bible and maintenance record.
- **Status:** complete

### Phase 4: Verify behavior and repository health
- [x] Pass focused repository tests with a nonzero count.
- [x] Pass affected crate and workspace checks.
- [x] Run required architecture, formatting, metadata, and diff checks.
- [x] Confirm no debug instrumentation or unrelated changes remain.
- **Status:** complete

## Key Questions
1. Which persisted facts survive both rc.3 and the early rc.4 conversion and uniquely identify this legacy-origin state?
2. Can recovery avoid relying on the mutable workspace phase?
3. Which negative cases prevent a fresh or unrelated state from being promoted?

## Decisions Made
| Decision | Rationale |
|----------|-----------|
| Test the full persisted upgrade sequence | Direct conversion tests missed the already-migrated user state. |
| Treat workspace phase as mutable, not migration provenance | Production transitions normally use nonterminal phases. |

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
| Initial streaming log assertion returned a false pass | 1 | Replaced it with a deterministic count over the final launch. |

## Notes
- The original diagnostic replay remains the user-symptom boundary, but a repository sequence test is the agent-runnable fix loop.
- Do not alter public interfaces or persistence encryption.
