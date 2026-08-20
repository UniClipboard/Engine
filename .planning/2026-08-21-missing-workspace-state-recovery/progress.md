# Progress: Missing Workspace State Recovery

## 2026-08-21
- Confirmed the original failure with a deterministic log replay: one session resume, three peer-scope failures, and forty delivery-view failures.
- Inspected the Windows dev database read-only and found legacy relationship rows with zero convergence-state rows.
- Located the incorrect current-installation classification in Engine assembly and the missing-state fallback in `WorkspaceConvergence::load_state`.
- Added production-shaped tests for existing-install origin, durable missing-state recovery, and the fresh-install negative case.
- The first short exact Cargo filter ran zero tests; it is not counted as evidence.
- Listed the full names and ran nonzero tests: the existing-install classification failed with `CurrentInstallation`, durable recovery failed because no state was saved, and the fresh-install negative case passed.
- Applied the minimal production change: derive origin from whether startup detected a prior installation and persist a newly inferred legacy state during recovery.
- Reran the focused missing-state tests with nonzero counts: both durable recovery cases passed, and the existing-install classification passed.
- Updated the architecture behavior and maintenance record for initialized profiles with no convergence row.
- Next: run the broader convergence suite and all mandatory repository checks.

## Test Results
| Check | Result |
|---|---|
| Final launch log replay | RED: bug reproduced after session resume |
| Live database state shape | Confirmed: legacy relationships present, convergence rows absent |
| Missing-state recovery tests | GREEN: 2 passed |
| Existing-install origin test | GREEN: 1 passed |
