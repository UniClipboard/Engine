# Progress Log: rc.3 Workspace Recovery Repair

## Session: 2026-08-21

### Phase 1: Diagnose the production failure
- **Status:** complete
- Confirmed all four Engine commits and the matching desktop pin.
- Replayed the final launch from `uniclipboard-diagnostics-20260820-161225.zip`.
- Verified the latest launch still reports the exact peer-scope failure.
- Ran both existing V3 recovery tests with one test executed in each command; both passed, proving they cover only the narrow modeled states.

### Phase 2: Write the regression first
- **Status:** complete
- Added repository-level replay tests for rc.4 V3 states originating from rc.3 with `LocallyApplied` and `Converging` phases.
- Reworked the existing negative test to preserve fail-closed cases without treating a mutable phase as missing evidence.
- Listed both tests before running them, then executed both with a nonzero count.
- Both failed at the intended assertion because `migrated_from_pre_adr_020` remained false.

### Phase 3: Apply the smallest durable fix
- **Status:** complete
- Replaced the `Complete` requirement with explicit not-removed, non-recovery, and no-failure guards.
- Positive replay tests now pass for both `LocallyApplied` and `Converging` states.
- Added an explicit stored-failure negative case, removed its production guard, and watched the negative test fail at the intended assertion before restoring the guard.

### Phase 4: Verify behavior and repository health
- **Status:** complete
- Ran all 19 workspace convergence store tests; all passed with a nonzero count.
- Passed workspace all-target checks, locked metadata, formatting, architecture validation, and diff validation.
- Updated the architecture bible behavior description and maintenance record.

## Test Results
| Test | Expected | Actual | Status |
|------|----------|--------|--------|
| Latest diagnostic launch replay | Failure detected | 1 peer-scope failure after final launch | red |
| Existing positive V3 recovery test | 1 test passes | 1 passed | pass |
| Existing negative V3 recovery test | 1 test passes | 1 passed | pass |
| Real rc.4 V3 replay for nonterminal rc.3 states | 2 tests fail before implementation | 0 passed, 2 failed at provenance assertion | red |
| Real rc.4 V3 replay after implementation | 2 tests pass | 2 passed | pass |
| Stored-failure fail-closed guard without implementation | 1 test fails | Failed at legacy-provenance assertion | red |
| Complete workspace convergence store suite | 19 tests pass | 19 passed | pass |
| Repository delivery checks | All required checks pass | All passed | pass |

## Error Log
| Timestamp | Error | Attempt | Resolution |
|-----------|-------|---------|------------|
| 2026-08-21 | Streaming log assertion incorrectly passed | 1 | Counted matching events after the final launch before deciding pass/fail. |

## 5-Question Reboot Check
| Question | Answer |
|----------|--------|
| Where am I? | Complete and ready to commit |
| Where am I going? | Local verified commit |
| What's the goal? | Recover already-converted rc.3 workspaces safely |
| What have I learned? | The phase requirement is not stable migration evidence |
| What have I done? | Reproduced the failure, repaired it, and passed focused plus repository checks |
