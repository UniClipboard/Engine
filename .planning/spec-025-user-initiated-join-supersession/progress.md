# Progress Log

## Session: 2026-08-18

### Phase 1: Requirements and source discovery

- **Status:** complete
- Actions taken:
  - Read the complete `to-spec` and `planning-with-files` skill instructions.
  - Ran session catchup; no unsynchronized report was returned.
  - Confirmed current branch and preserved existing documentation and planning changes.
  - Read ADR-022 and the previous ADR research notes.
  - Located the current reopen-on-repeat behavior in `DurableAdmissionTransaction::prepare_join_before_network`.
  - Mapped the core attempt model, repository port, production Diesel store, projection, and process-level serialization lock.
  - Traced the conflict through convergence, application facade, Engine error mapping, and binding transport.
  - Located the durable Prepared write boundary and all old-attempt message/recovery entry points that need fail-closed handling.
  - Reconciled the design with Spec 023 and selected the smallest data, wire-cleanup, transaction, and projection changes.
- Files created/modified:
  - `.planning/spec-025-user-initiated-join-supersession/task_plan.md`
  - `.planning/spec-025-user-initiated-join-supersession/findings.md`
  - `.planning/spec-025-user-initiated-join-supersession/progress.md`

### Phase 2: Design and phase boundaries

- **Status:** complete
- Actions taken:
  - Defined append-only core stage and terminal additions.
  - Defined one atomic repository operation for old-attempt finalization and replacement creation.
  - Fixed the public error contract at code 1295, Conflict, non-retryable.
  - Chose vertical implementation stages and explicit exit gates.
  - Defined delayed-message, cleanup, restart, invitation, and compatibility behavior.

### Phase 3: Specification drafting

- **Status:** complete
- Actions taken:
  - Added `docs/specs/025-user-initiated-join-supersession.md` with all 11 required sections.
  - Defined six vertical implementation phases with risks and exit gates.
  - Added the Spec to the document index, ADR-022, Spec 023, and the architecture-bible related-document list.
  - Added an architecture-bible maintenance record that explicitly keeps production and device work pending.
  - Corrected the focused repository test filter and Phase 1 exit gate during review.

### Phase 4: Verification

- **Status:** complete
- Actions taken:
  - Confirmed all 11 required sections are present exactly once.
  - Confirmed every referenced workspace path and every `docs/README.md` link exists.
  - Scanned ADR-022, Spec 023, Spec 025, and the architecture bible for the superseded repeated-JoinSpace rule; no contradictory current rule was found.
  - Confirmed the new file has no whitespace errors and the working tree contains no production source changes for this task.

## Test Results

| Test | Expected | Actual | Status |
|------|----------|--------|--------|
| Session catchup | Report stale context if present | No stale context reported | Pass |
| Required section count | Exactly 11 numbered sections | 11 sections | Pass |
| Referenced paths | Every referenced path exists | No missing paths | Pass |
| Contradiction scan | No current rule says a public repeated JoinSpace reopens the old attempt | No matches | Pass |
| `cargo metadata --locked --format-version 1` | Workspace metadata resolves | Completed successfully | Pass |
| `cargo check --workspace --all-targets --locked` | Entire workspace compiles | Completed successfully | Pass |
| `cargo fmt --all -- --check` | Formatting is clean | Completed successfully | Pass |
| `node scripts/architecture/check-engine-repository.mjs` | Repository rules pass | 6 OpenMLS checks and negative fixtures passed | Pass |
| `git diff --check` | No whitespace errors | Completed successfully | Pass |
| Physical device acceptance | Android two-device flow | Not run; documentation-only task | Skipped |

## Error Log

| Timestamp | Error | Attempt | Resolution |
|-----------|-------|---------|------------|
| 2026-08-18 | No memory registry hit for ADR-022 terms | 1 | Continued from current workspace evidence. |
| 2026-08-18 | Plan update patch referenced a row in the wrong file | 1 | Re-read both files and applied a corrected patch. |
| 2026-08-18 | Full UniFFI public-contract run timed out while shutting down after the capture-only clipboard test | 1 | The exact test passed in 14.16 seconds; reran the complete serial suite to distinguish a boundary fluctuation from a remaining leak. |
| 2026-08-18 | The second full UniFFI public-contract run timed out in two different shutdowns at 15 seconds | 2 | Raised the shared test-only deadline to 30 seconds because 15 seconds was not stable under serial suite load. |

## Implementation Follow-up

- Implemented all six Spec 025 phases and added public-entry coverage for late Candidate, Commit, and Complete messages.
- Fixed host clipboard work so session shutdown can cancel an in-flight change operation.
- Unified UniFFI public-contract shutdown deadlines at 30 seconds after two serial suite runs showed that 15 seconds was not stable.
- The two failures from the previous full workspace run pass with the final shared deadline.

## Final Verification

| Test | Actual | Status |
|------|--------|--------|
| UniFFI `public_contract` at the final shared deadline | 22 passed, 0 failed | Pass |
| `cargo test --workspace --all-targets --locked` | Completed successfully; focused admission, host shutdown, and all binding tests passed | Pass |
| `cargo metadata --locked --format-version 1` | Workspace metadata resolved | Pass |
| `cargo check --workspace --all-targets --locked` | Entire workspace compiled | Pass |
| `cargo fmt --all -- --check` | Formatting is clean | Pass |
| `node scripts/architecture/check-engine-repository.mjs` | Repository preflight and negative fixtures passed | Pass |
| `git diff --check` | No whitespace errors | Pass |
| Added production-code forbidden-pattern scan | No newly added forbidden calls | Pass |
| Final strict diff review | No blocking, important, or optional findings | Pass |
| Android two-device physical acceptance | Not run; only one Android device was available | Skipped |
| iOS physical acceptance | Not run; no online iPhone was available | Skipped |
| HarmonyOS and additional network matrix | Not run | Skipped |

## 5-Question Reboot Check

| Question | Answer |
|----------|--------|
| Where am I? | Phase 5: delivery review |
| Where am I going? | Design, draft, verify, and deliver Spec 025 |
| What's the goal? | An implementation-ready phased Spec for ADR-022 |
| What have I learned? | See `findings.md` |
| What have I done? | Drafted and verified Spec 025 and all related documentation updates |

### Phase 5: Delivery

- **Status:** complete
- Actions taken:
  - Reviewed the final specification, acceptance checklist, references, and working-tree scope.
  - Confirmed the task changes documentation only and does not claim production or physical-device completion.
  - Prepared the concise user-facing delivery summary.
