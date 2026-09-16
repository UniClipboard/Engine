# Progress

## 2026-08-26

- Pair-programmed the first two `SpaceAdmissionId` red-green cycles: redacted Debug and read-only byte access.
- Verified both tests pass without the prior unread-field warning.
- Added and observed the expected failing zero-id test.
- Completed the zero-id green change: constructor is fallible, valid fixtures unwrap explicitly, and all three focused tests pass without warnings.
- Added a red value-key test, observed the missing `Ord` and moved-value failures in sequence, then added only the required ordering/copy traits.
- Added red tests for the remaining four identity types and implemented all five through one private newtype macro.
- Added the single supported protocol version, all three protocol roles, redacted durable-message evidence, and their validation tests.
- Fixed the sender rules for all ten business messages, including the completion helper's deliberately narrow permissions.
- Added exact-replay, conflicting-message-id, and distinct-message evidence classification with focused tests.
- Ran the first whole-workspace compile inventory. Core and Application compile; Infra is blocked by 17 production errors and 31 test-target errors from deleted legacy ports still referenced by unfinished store/network adapters.
- Added validated/redacted message headers, typed envelopes, a complete typed JoinRequest, bounded protocol artifacts, and centralized new/replay/conflict/out-of-order classification.
- Added all ten typed business bodies, durable pending exchange/exact reply facts, and the first closed Joiner aggregate state.
- Added the initial authentication transition: it atomically replaces encrypted password material with a bound continuation credential and increments the record version.
- Added Sponsor Accepted -> Candidate with exact saved reply, and authenticated Joiner Initiated -> Candidate with strict predecessor/evidence checks.
- Moved the growing Core protocol into its dedicated module directory and updated the architecture maintenance record.
- Re-ran the focused Core suite, all-target Core compile, architecture repository check, and whitespace check after the module move.
- Replaced the 2,830-line `space_admission/mod.rs` with a 37-line module entry and private responsibility files for identity, artifacts, messages, exchanges, state, transitions, and tests.
- Preserved every existing public re-export and verified all 38 focused protocol tests after the split.
- Split the 1,195-line test file into identity, message, exchange, and state suites; the same 38 tests pass under the new layout.
- Re-ran the whole-workspace check: it still reaches the same 17 Infra production errors and 31 Infra test-target errors recorded before this refactor, with no new Core or Application failure.
- Added the verified staged-target type and completed Joiner Candidate -> Prepared with an exact durable Prepared request expecting Commit.
- Completed the full Core J0-J3 and settlement path for Joiner and Sponsor, including exact replies and checked record versions.
- Added pre-Commit cancellation, TooLateCommitted, safe supersession, stable rejection, replay/conflict/out-of-order decisions, RecoveryRequired, and compact terminal states.
- Added CompletionHelper challenge, checked counter advance, verified Complete, and helper settlement without membership-commit authority.
- Replaced direct aggregate returns with `AdmissionTransition { replacement, exact_reply, effects }` and removed role/stage types from the membership root surface.
- Split Core state, transitions, and tests by role/responsibility; production builds and tests complete without warnings.
- Added the Application Fresh Joiner -> Candidate -> Prepared target test with explicit save-before-delivery ordering and exact Candidate replay assertions; next verification must confirm the expected red failure is only the missing new protocol surface.
- Split the broad tracer target into the first executable red slice: a fresh join must be saved before Pending is returned. Sponsor Candidate, Joiner Prepared, and exact replay remain the next test-first slices rather than simultaneous compile failures.
- Added Chinese API documentation to every Core admission artifact, then narrowed it to Core-owned business meaning, legal stage, and lifecycle only; removed Infra, network, storage, library, and assembly knowledge from Core comments.
- Added Chinese API documentation to Core admission exchange facts, covering inbound expectations, message evidence, replay/conflict decisions, exact saved replies, pending exchanges, and monotonic retry state without outer-layer knowledge.
- Added the next Application red test for an opaque admission commit token: zero is invalid and Debug must redact its value, preventing protocol code from learning ledger concurrency fields.
- Verified the opaque commit-token test green, then added the next red test for a complete start-state view carrying ordinal, source snapshot, current join, session-transition requirement, and the opaque token as one value.
- Added Chinese code documentation for `LoadedJoinerStartState`, explaining that it is one consumed business view rather than a second state machine and documenting each fact without ledger internals.
- Verified the complete start-state view test green, then wired the Fresh Joiner acceptance test to an expected recording state port that loads the whole view and records only a complete Core transition commit.
- Began the Fresh Joiner implementation by loading and consuming the complete `LoadedJoinerStartState`; intentionally left error mapping and later steps for the next focused red-green cycle.
- Added a red error-mapping test requiring each `JoinerStartStateError` category to remain distinct in `JoinSpaceError` instead of becoming an opaque saved-state string.
- Implemented one-to-one Application mapping for Locked, StateChanged, RecoveryRequired, and Unavailable start-state failures, allowing `start_join` to use the case-owned error contract directly.
- Added a red mapping test for invalid-invitation versus unavailable joiner-start material failures before completing the Fresh Join happy path.
- Completed the Fresh Join local path: load one start view, create typed material, build the pending exchange, call Core `start_join`, commit one transition, and return Pending only after commit.
- Extended the Fresh Join red test to preserve existing device-name validation/persistence before the admission commit, keeping the ordering inside `SpaceAdmissionProtocol`.
- Moved device-name validation and persistence into `SpaceAdmissionProtocol::start_join` before state loading and admission commit, preserving the old user-visible behavior inside the new owner.
- Rewired `SpaceApplication` and `SpaceFacade::join_space` to the new `SpaceAdmissionProtocol`, replacing the legacy JoinSpace production field and dependency with explicit material and state capabilities.
- Deleted the legacy Application JoinSpace preparation port, prepared record DTO, use-case implementation, and old-record tests after removing their production wiring; stable input/result/error types remain.
- Extended the Application composition test to execute the newly wired `SpaceAdmissionProtocol` JoinSpace path and assert one commit before Pending.
- Extended the crate-external public-surface test to keep stable `AppFacade::join_space` reachable while exposing only JoinerStart capability ports through deps.
- Removed the legacy synchronous handshake error wrapper from JoinSpace and mapped the new local start failures to existing stable Engine codes/categories.
- Added a red two-join test requiring an Initiated current admission to be superseded atomically with creation of its replacement.
- Added `JoinerStartMutation` and updated the protocol/state port so replaceable-current supersession and replacement creation are committed together.
- Completed the Application Phase 4 tracer implementation: recovery/authentication, Sponsor Accepted/Candidate, exact Candidate replay, and Joiner Candidate/Prepared now share one `SpaceAdmissionProtocol`.
- Replaced the production maintenance admission step and inbound endpoint with the new protocol interfaces; concrete Infra adapters remain intentionally unimplemented.
- Confirmed Application library and test targets compile. Test execution was skipped at the user's request because the current environment could not run tests reliably.

## Verification

| Check | Result |
| --- | --- |
| ID debug/access tests | 2 passed |
| Zero-id test | Expected red: infallible constructor has no `is_none` |
| ID debug/access/zero tests | 3 passed |
| All admission identity tests | 6 passed |
| Core admission protocol foundation | 12 passed |
| Core admission evidence classification | 15 passed |
| Core typed header, JoinRequest, envelope, and inbound classification | 28 passed |
| Core complete message set, durable exchange, and initial aggregate | 36 passed |
| Core Sponsor/Joiner Candidate transitions | 38 passed |
| Core all-target compile | Passed |
| Architecture repository check | Passed |
| Whitespace check | Passed |
| Core module split focused tests | 38 passed |
| Core module split all-target compile | Passed |
| Touched Core files format check | Passed |
| Full uc-core library tests after split | 237 passed |
| Whole-workspace compile after split | Same existing Infra blockers; no new Core/Application failure |
| Joiner Candidate -> Prepared focused test | 1 passed |
| Complete Space admission Core focused suite | 67 passed |
| uc-core library suite | 266 passed |
| uc-core key epoch integration suite | 17 passed |
| uc-core membership history integration suite | 30 passed |
| uc-core all-target compile | Passed without warnings |
| Workspace all-target compile after Core completion | Same existing Infra blockers: 17 lib / 31 lib-test errors |
| Workspace all-target compile inventory | Blocked in Infra: 17 lib / 31 lib-test errors |
| Application protocol tracer target | Expected red: only `ProtocolEvent` and `SpaceAdmissionProtocolTestPair` are unresolved |
| Architecture repository check after tracer test | Passed |
| Tracer test files format and whitespace | Passed |
| JoinSpace protocol tests | 6 passed |
| Application library tests after JoinSpace cutover | 667 passed |
| Application integration/public-surface tests | 8 + 2 passed |
| Legacy Application JoinSpace symbol search | Zero Rust matches |
| Full workspace after JoinSpace cutover | Same existing Infra blockers: 17 lib / 31 lib-test errors |
