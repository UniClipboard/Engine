# Space Join Record Readability Plan

## Goal

Make the membership ledger join-record area explain its responsibility through a narrow interface while preserving public behavior, persisted formats, network payloads, and recovery semantics.

## Completion Criteria

- [x] Existing join, cancel, inbound admission, recovery, and Space transition behavior is locked by focused tests.
- [x] The join-record ledger file explains its responsibility and exclusions at the top.
- [x] The inbound commit no longer exposes a long list of unrelated arguments.
- [x] Admission use cases do not manually increment join-record versions.
- [x] Admission use cases do not directly assemble terminal join states.
- [x] No second ledger, compatibility alias, persisted-format change, network-format change, or public Engine contract change is introduced.
- [x] Space maintenance documentation and the architecture bible are updated.
- [x] Focused tests and all required repository checks pass, with unrelated pre-existing failures reported separately.

## Next Step

Complete. Preserve unrelated worktree changes and report the verified boundary.

## Current Phase

Phase 5

## Phases

### Phase 1: Baseline and regression tests

- Inventory the current renamed callers and existing test coverage.
- Run focused tests with a nonzero test-count check.
- Add failing tests for stale inbound state and complete Space transition behavior.
- **Status:** complete

### Phase 2: Deepen inbound admission commit

- Replace the long argument list with a ledger-owned expectation and one prepared change.
- Keep invitation validation, reply handling, and maintenance wakeup in the admission use case.
- Keep all-or-nothing membership persistence in `MembershipLedger`.
- **Status:** complete

### Phase 3: Hide join-record persistence mechanics

- Move record-version advancement into the ledger.
- Add Core operations for cancellation and completed local activation instead of direct field assembly in use cases.
- Migrate join, cancel, recovery, and transition callers one complete flow at a time.
- **Status:** complete

### Phase 4: Finalize names and local readability

- Rename the ledger implementation file to the approved Space join-record term.
- Add the responsibility header and order methods by workflow.
- Remove old names and compatibility paths.
- Update `space/AGENTS.md` and the architecture bible.
- **Status:** complete

### Phase 5: Full verification

- Run focused Core and application tests.
- Run the full application test suite and required repository checks.
- Search for old names, manual version advancement, and direct terminal-state assembly.
- **Status:** complete

## Decisions

- `MembershipLedger` remains the sole owner of atomic application membership persistence.
- Admission use cases remain the owners of user and network workflow order.
- Core owns legal `SpaceJoinRecord` state changes; the ledger owns persistence revision advancement.
- Existing user changes in the dirty worktree must be preserved.

## Errors Encountered

| Error | Attempt | Resolution |
| --- | ---: | --- |
| Plan status tool returned no structured plan after accepting the update | 1 | Continue with the on-disk scoped plan as the source of truth. |
| Focused Cargo test waited on the shared artifact lock while several existing builds were stalled in `sccache` | 1 | Cancelled only this task's waiter; use an isolated target directory with `RUSTC_WRAPPER` disabled. |
| Focused Rust formatting check found local ordering and line-wrap differences | 1 | Apply formatting only to touched files; do not format unrelated worktree changes. |
| Planning progress patch used an outdated Next Step line | 1 | Re-read the scoped plan and applied the status update against current contents. |
| Repository-wide format check found existing differences in two unrelated Engine/Infra files | 1 | Left unrelated user changes untouched; focused formatting for this task is checked separately. |
| Workspace all-target check reached Infra but found removed application contracts still referenced by existing Infra migration work | 1 | Do not restore deleted contracts or edit unrelated migration files; retain application full-suite and focused Core proof. |
