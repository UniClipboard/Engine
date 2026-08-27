# Findings and Decisions

## Requirements

- Make the purpose of the membership ledger admission file understandable without tracing the entire repository.
- Reduce the knowledge required by admission callers.
- Preserve behavior and all stable external and persisted contracts.
- Verify the result before delivery.

## Research Findings

- The file has eight `MembershipLedger` methods but no module-level responsibility statement.
- `commit_inbound_admission` takes eight arguments and suppresses the argument-count lint.
- Production callers live in six admission workflows: join, cancel, inbound message handling, recovery, transition query, and transition completion.
- Callers manually advance `record_version`; cancel and transition completion also assemble terminal state directly.
- Existing tests cover successful atomic commits, message settlement, join-before-recovery, cancellation, and completed Space switching.
- Focused stale-state and complete transition use-case coverage needs to be added before refactoring.
- The worktree already contains user changes that renamed join and cancel files; all work must target the current paths.
- The current cancel use case directly supersedes messages, creates the cancel outbox entry, sets the terminal result and rejection reason, changes the joiner stage, and increments the record version.
- The current completion use case directly replaces transition bytes, terminal result, role stage, and record version before calling two generic ledger methods.
- The inbound test repository can simulate a stale commit by changing its loaded revision immediately before `compare_and_commit`; this gives a real all-or-nothing regression without adding a mock framework.
- Existing focused tests use in-memory implementations of the real ledger interfaces, so new regressions can exercise actual ledger behavior.
- `SpaceJoinRecord` already owns the closely related “superseded by a new join” transition, including outbox validation and terminal-state assembly. User cancellation should follow the same ownership pattern.
- The transition types already know whether one phase can advance to another and whether a result matches cleanup; `SpaceJoinRecord` can combine those rules with its own completion and terminal-state requirements.
- After migration, admission use cases contain no direct writes to `record_version`, terminal result, role state, or Space transition fields.
- All eight old ledger method names have been removed from application sources; the remaining methods now use Space join-record language and outcome-oriented names.
- The old ledger `admission.rs` path has been removed and replaced by `membership/ledger/join_record.rs` with a responsibility header.

## Decisions

| Decision | Rationale |
| --- | --- |
| Keep one `MembershipLedger` | It already centralizes the all-or-nothing membership write and earns its role. |
| Do not add an `AdmissionLedger` | A second ledger would split ownership and increase navigation cost. |
| Use a ledger-owned expectation value | Callers should not manually coordinate revision, history, and record version. |
| Keep protocol preparation outside the ledger | The ledger saves a prepared result; it does not decide network workflow. |
| Move version advancement into the ledger | Record version is a persistence concern and should not be repeated in callers. |
| Add Core transition methods | Legal join-state changes belong with `SpaceJoinRecord`, not field assignments in use cases. |
| Keep record-version stamping in the application ledger | The version coordinates persistence; Core should validate state transitions but should not own storage concurrency. |
| Add outcome methods to the existing `SpaceJoinRecord` | This keeps legal state changes beside the record without creating another coordinator or ledger. |

## Resources

- `crates/uc-application/src/space/membership/ledger/admission.rs`
- `crates/uc-application/src/space/admission/handle_space_admission_message/`
- `crates/uc-application/src/space/admission/join_space/`
- `crates/uc-application/src/space/admission/cancel_space_join/`
- `crates/uc-application/src/space/admission/complete_pending_space_transition/`
- `crates/uc-core/src/membership/space_join_record.rs`
- `crates/uc-application/src/space/AGENTS.md`
- `docs/architecture/architecture-bible.md`

## Issues Encountered

| Issue | Resolution |
| --- | --- |
| Existing active planning files belong to another completed task | Created a separate scoped plan without changing `.planning/.active_plan`. |
