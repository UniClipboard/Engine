# Task Plan: Spec 025 User-Initiated Join Supersession

## Goal

Implement and verify Spec 025 end to end without caller-side orchestration.

## Next Step

Finish repository-wide verification and create the scoped local commit.

## Current Phase

Phase 8

## Phases

### Phase 1: Requirements and source discovery

- [x] Confirm the adopted ADR behavior and repository constraints
- [x] Map current production paths and tests
- [x] Record implementation gaps and invariants
- **Status:** complete

### Phase 2: Design and phase boundaries

- [x] Define the owner, data changes, atomic operation, and public result
- [x] Split implementation into independently verifiable stages
- [x] Define compatibility, recovery, and late-message handling
- **Status:** complete

### Phase 3: Specification drafting

- [x] Write Spec 025 using the required 11-section structure
- [x] Update related document indexes and architecture maintenance record
- [x] Reconcile references with Spec 023 and ADR-022
- **Status:** complete

### Phase 4: Verification

- [x] Scan for contradictions and broken references
- [x] Run repository-required checks
- [x] Record exact results and skipped device validation
- **Status:** complete

### Phase 5: Delivery

- [x] Review the full diff and completion checklist
- [x] Deliver a concise user-facing summary
- **Status:** complete

### Phase 6: Production implementation

- [x] Implement explicit user-initiated join replacement and stable conflict result
- [x] Keep automatic recovery on the original join
- [x] Preserve one current join under concurrent requests and restart
- [x] Keep all mobile bindings aligned
- **Status:** complete

### Phase 7: Strict review and follow-up fixes

- [x] Cover late Candidate, Commit, and Complete messages through the public protocol entry
- [x] Prevent superseded joins from changing the current join or workspace state
- [x] Make host clipboard work observe session shutdown
- [x] Use the repository-standard shutdown deadline in UniFFI contract tests
- **Status:** complete

### Phase 8: Final verification and commit


- [x] Run focused regressions and the full workspace test suite
- [x] Run every repository delivery check
- [x] Review the complete diff and forbidden production patterns
- [x] Create and inspect the scoped local commit
- **Status:** complete

## Key Questions

1. Which existing type owns the durable attempt stage and terminal outcome?
2. Where can old-attempt finalization and new-attempt creation be made atomic?
3. How are Engine errors mapped through iOS, Android, and HarmonyOS bindings?
4. Which tests prove nonzero coverage for phase transitions, crash recovery, and late messages?

## Decisions Made

| Decision | Rationale |
|----------|-----------|
| Use Spec number 025 | Existing documentation already uses Spec 024; ADR-022 needs a separate implementation specification. |
| Keep `WorkspaceConvergence` as the sole workflow owner | Required by ADR-017, ADR-021, ADR-022, and repository architecture rules. |
| Treat pre-Prepared supersession as one durable operation | Prevents a missing current join or two current joins after failure. |
| Reuse the existing `CancelRequested` wire purpose for isolated old-attempt cleanup | Avoids a new protocol message while keeping local supersession distinct from public cancellation. |
| Reserve JoinSpace error code 1295 | It is the next unused JoinSpace code and distinguishes the irreversible prior-join conflict from generic 1238. |

## Errors Encountered

| Error | Attempt | Resolution |
|-------|---------|------------|
| No ADR-022 memory registry hit | 1 | Use the current ADR and workspace source as authoritative evidence. |
| Plan status patch referenced a decision row in the wrong file | 1 | Re-read the current planning files and applied a scoped correction. |

## Notes

- The original planning pass was documentation-only; the current task now implements and delivers the specification.
- Preserve unrelated working-tree changes and user-owned planning directories.
