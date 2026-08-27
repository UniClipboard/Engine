# Findings

- The current partial `InboundAdmissionStatePort` is the correct ownership direction, but its `accept` method still exposes `InboundAdmissionExpectation` and `PreparedInboundAdmission` from membership.
- `HandleSpaceAdmissionMessageUseCase` still stores a concrete `MembershipLedger` and retains the old ledger error mapping.
- `LoadedMemberAdmissionActivation` currently derives `Clone` and `Debug` despite containing signed history and the intended one-shot commit token.
- The case needs current record, signed history, required invitation generation, and a commit token from one verified snapshot.
- The existing stale-expectation ledger test already proves the required no-write conflict behavior and should be migrated to the new token interface.
- The existing integration test proves commit-before-reply, invitation consumption, and maintenance wake-up ordering.
- The final interface can reuse the existing integration repository as both production-style reader and committer; no new mock layer is required.
- `SpaceAdmissionPreparationContext.revision` is redundant after invitation validation. The context should carry only the validated optional invitation generation, signed history, and current record.
- `LoadedMemberAdmissionActivation` should own an opaque `MemberAdmissionCommitToken` and expose behavior methods rather than public fields.
- The current application assembly already has one `Arc<MembershipLedger>` suitable for coercion to the unified state interface; the separate concrete ledger constructor argument can be removed.
- The admission state implementation can derive its token directly from `load_verified()` and must reject a missing signed history before returning a context.
- The reader's load error does not need `StateChanged`; an unexpected lower-level conflict is treated as temporarily unavailable, while stale-token conflict remains an accept-time `StateChanged`.
- The loaded admission state does not need `Debug`; removing it is stronger than maintaining a redacted formatter and prevents accidental sensitive-state logging.
