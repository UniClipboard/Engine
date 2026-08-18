# Findings and Decisions

## Requirements

- Produce a phased, implementation-ready Spec for the adopted ADR-022.
- Follow the 11-section `to-spec` structure.
- Name concrete modules, paths, data structures, interfaces, failure handling, and tests.
- Keep `WorkspaceConvergence` as the only complete workflow owner.
- Update the architecture bible maintenance record for this documentation change.
- Do not write production code.

## Research Findings

- ADR-022 distinguishes explicit user submissions from internal recovery.
- A previous local join is supersedable only before durable `Prepared`.
- Superseding the old attempt and creating the new attempt must be atomic.
- `SupersededByNewJoin` is internal history, not a sponsor rejection or a new public status.
- A new public conflict distinct from generic error 1238 is required after `Prepared`.
- Same-invitation resubmission creates a new local attempt but does not relax sponsor-side one-time consumption.
- `DurableAdmissionFlow::prepare_local_join` holds the existing profile `state_lock` and delegates the whole operation to `DurableAdmissionTransaction::prepare_join_before_network`.
- `prepare_join_before_network` currently loads `current_pending_join` and calls `reopen_join_start`; this is the exact old behavior to replace for explicit user submissions.
- `AdmissionAttemptV1` stores the joiner stage, write-ahead recovery, Space transition, encrypted outboxes, terminal result, and cleanup flag. `AdmissionTerminalResultV1` currently has only `Active`, `Completed`, and `Rejected`.
- `AdmissionAttemptRepositoryPort` owns create, compare-and-advance, recovery scan, terminal compaction, current projection, metadata, and projection-floor changes, but has no atomic supersede-and-create operation.
- `DieselAdmissionAttemptStore` is the production implementation. The new atomic operation belongs there so old finalization, new ordinal allocation, new attempt encryption, metadata revision, and projection change share one SQLite transaction.
- The in-process `state_lock` serializes one owner instance but does not replace repository compare-and-swap and transaction guarantees needed for restart or competing writers.
- `DieselAdmissionAttemptStore::create` rejects any persisted attempt that is nonterminal, has write-ahead recovery, or has cleanup pending. It allocates the local ordinal and increments `device_trust_revision` inside an immediate SQLite transaction.
- `WorkspaceConvergenceError` currently exposes `AdmissionInProgress` but no dedicated previous-join-not-supersedable variant.
- `execute_join_space` maps unrecognized application failures to `JOIN_SPACE_FAILED_CODE = 1238`; the new conflict must travel through the application error chain and map to a new stable Conflict/non-retryable Engine code.
- UniFFI converts `EngineError` into a structured code/category/retryable record. HarmonyOS includes those fields in events and serializes them in direct N-API errors. Binding work is contract verification rather than platform-specific policy.
- `joiner_verify_and_prepare`, `joiner_record_rejected`, `joiner_apply`, delivery acknowledgment, recovery scan, and terminal compaction all reload attempts by `attempt_id`; each relevant entry must explicitly handle a superseded terminal so delayed traffic cannot advance it.
- Durable `Prepared` is written in the same compare-and-advance transaction that stores the proof, target transition, verified history, and Prepared outbox. The supersession cutoff can therefore be tested from persisted state, not inferred from network delivery.
- Existing terminal compaction refuses active outboxes, write-ahead recovery, incomplete Space transition, or cleanup. Superseded records can use the same settle-then-compact lifecycle after retaining the minimum replay facts.
- The joiner handshake currently converts every failure from `prepare_local_join_before_network` into application `Internal`; the dedicated conflict must be mapped before that catch-all and then mapped to a new Engine Conflict code.
- Existing Spec 023 already names `SupersededByNewJoin`, `PreviousJoinCannotBeSuperseded`, the Prepared cutoff, higher ordinal projection, compaction, and acceptance cases. Spec 025 should be the implementation plan and must not redefine those decisions.
- `validate_attempt` currently permits terminal results only for Rejected or completed role stages. Supporting supersession requires a joiner-only `Superseded` stage, an appended `SupersededByNewJoin` terminal result, no rejection reason, and explicit validation.
- The existing `CancelRequested` wire purpose can be reused as an isolated best-effort notice to release sponsor-side pre-commit state. This does not make public JoinSpace supersession equivalent to CancelJoinSpace because the local old attempt becomes terminal immediately and the new attempt does not wait for sponsor judgment.
- The replacement transaction should increment `next_local_join_ordinal` and `device_trust_revision` once, preserve old metadata and invitation claims, reuse the old wrapped key only for the old record, and create a fresh wrapped key for the replacement.
- Earlier superseded attempts may still have isolated cleanup outboxes. A later explicit join must be allowed to supersede the one current nonterminal local join without making settled-or-cleaning superseded records current again.

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| Stage the implementation from characterization tests through device acceptance | Each stage has a narrow proof boundary and avoids simultaneous cross-layer changes without tests. |
| Specify exact current paths after source inspection | The Spec must be executable against the checked-out code, not only the ADR model. |
| Extend `AdmissionAttemptRepositoryPort` with one semantic atomic operation | A pair of ordinary repository calls would expose the exact partial state ADR-022 forbids. |
| Add one application/convergence error and one Engine code | Callers need to distinguish the irreversible Prepared boundary from generic failure without learning internal stages. |
| Keep `SupersededByNewJoin` internal and omit it from `CurrentJoinStatus` | The atomic replacement gives the higher ordinal to the new join, so public projection only needs to expose the new attempt. |
| Reuse `CancelRequested` only as an internal cleanup notice | It avoids a new wire protocol while preserving the different local operation semantics required by ADR-022. |
| Advance the public revision once per successful replacement | The old terminal and new current join are one logical atomic state change. |

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| Memory registry contains no direct ADR-022 entry | Current workspace documents and source remain authoritative. |
| Full workspace tests exposed UniFFI shutdown timeouts at five seconds, and two full serial reruns still fluctuated at 15 seconds | Use a shared 30-second deadline for all public-contract tests; production shutdown behavior remains unchanged. |
| Full workspace tests exposed a host clipboard operation that could outlive session shutdown | Make the operation observe the same session cancellation signal as other Engine operations. |
| The final workspace run exposed a race in a negative Iroh pull test: the peer could be rejected while opening the stream, but the helper only accepted rejection while reading the response | Let the test helper report transport failure at any request stage; positive cases still require a decoded response. |
| Strict review found that an Initiated join became incorrectly non-supersedable after its JoinRequest delivery acknowledgment marked the outbox settled | Treat a valid saved initial request as the replay fact even after delivery settles, and use its message id as the cleanup predecessor when no active outbox remains. |
| Second strict review found Candidate completeness omitted the persisted identity binding, and the negative pull helper would accept a failure to establish the connection | Require the candidate identity binding; keep the negative test's successful dial precondition and only accept rejection after connection establishment. |

## Phase 9 Audit Findings

- The specification's Fresh/Same-Space/Cross-Space regression claim is not directly established by the newly named generic supersession tests.
- `automatic_recovery_keeps_the_same_join_identity` currently proves recovery-material identity after reopening only at the initial stage; it does not directly exercise every protocol stage or lost acknowledgements as the test table says.
- The same-invitation integration test proves a new local attempt and join, but does not directly prove that sponsor invitation consumption stays bound to the original attempt.
- Candidate, Commit, Complete, rejection cleanup, and multiple-cleanup recovery have focused tests; Rejected, Applied, delivery acknowledgement, compaction, and plaintext-at-rest coverage still require an exact evidence audit.
- The Android two-device acceptance item remains skipped and cannot be converted into automated proof.

## Resources

- `docs/adr/022-user-initiated-join-supersession.md`
- `docs/specs/023-durable-membership-proof-and-admission-activation.md`
- `docs/adr/021-workspace-convergence-internal-boundaries.md`
- `docs/architecture/architecture-bible.md`
