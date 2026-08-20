# Task Plan: Missing Workspace State Recovery

## Goal
Recover an upgraded initialized profile that retains legacy member relationships but has no persisted workspace convergence state.

## Current Phase
Phase 4: Verify and document

## Phases

### Phase 1: Confirm the production data shape
- [x] Replay the final launch failure after session resume.
- [x] Inspect the Windows dev database read-only.
- [x] Confirm legacy relationships exist while all convergence state tables are empty.
- **Status:** complete

### Phase 2: Write the regression first
- [x] Reproduce missing state plus initialized legacy relationships at the application owner seam.
- [x] Prove current code classifies it as a current installation and does not save recovery state.
- [x] Preserve a fresh-install negative case.
- **Status:** complete

### Phase 3: Implement the minimal owner-side repair
- [x] Derive missing-state origin from durable initialized-profile evidence.
- [x] Persist the recovered encrypted state after session readiness.
- [x] Keep callers and public interfaces unchanged.
- **Status:** complete

### Phase 4: Verify and document
- [ ] Pass the focused regression and convergence suites with nonzero counts.
- [ ] Run all required repository checks.
- [x] Update the architecture bible behavior and maintenance record.
- [ ] Recheck the original diagnostic failure boundary.
- **Status:** in_progress

## Completion Criteria
- The production-shaped missing-state regression fails before and passes after the fix.
- Fresh installations remain unclassified as legacy upgrades.
- The recovered state is encrypted and survives reopen.
- All mandatory repository checks pass.

## Errors Encountered
| Error | Resolution |
|---|---|
| Earlier fixes assumed a convergence row existed | Use the live database row counts as the regression fixture shape. |
| First exact Cargo filter executed zero tests | List full test names, then rerun each exact fully-qualified name and verify nonzero counts. |
