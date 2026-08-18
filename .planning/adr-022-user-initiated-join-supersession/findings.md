# Findings

## Adopted Product Decision

- Every explicit user submission starts a fresh local join, even when the invitation input is unchanged.
- Restart, reconnect, message replay, and background retry continue the same join.
- An old join may be superseded only while the joiner has not persisted/sent Prepared; after that point the sponsor may already have committed, so recovery must remain forward-only.
- The host still invokes one JoinSpace operation and never composes cancel/reset/join steps.

## Known Current Conflict

- Spec 023 currently says repeated JoinSpace while Pending resumes the same attempt and a different invitation conflicts. The adopted decision replaces this behavior.

## Repository Context

- Current HEAD is `b0b707ce908712dff70864b7064ea6194e5e9e06`; ADR numbers currently end at 021, so the new decision will be ADR 022.
- JoinSpace already exposes durable Pending, Active, and Rejected status through one Engine-owned workflow; callers can query/subscribe instead of replaying the public operation for recovery.
- The existing `.planning/adr-023-implementation/` directory remains user-owned and untouched.
- ADR 017 and ADR 021 require `WorkspaceConvergence` to remain the sole owner from one JoinSpace action through durable result, retry, restart recovery, and notification. The new rule must stay inside that owner and add no caller-side cancel/reset sequence.
- The architecture bible already documents one durable local admission slot, terminal history, replay protection, and a current-join projection. ADR 022 should reuse those mechanisms rather than introduce a second workflow.
- Existing ADRs use: status/date/revision metadata, context, decision, non-goals, considered alternatives, consequences, and acceptance criteria. ADR 022 will follow that structure.
- `docs/README.md` is also missing existing Spec 024 and ADR 021 entries; update the index while adding ADR 022 so the decision remains discoverable.
- The architecture bible's admission section is the correct place to state the explicit-user-action versus automatic-recovery split and the pre-Prepared cutoff; its maintenance record must mark this as an adopted architecture change, not implementation completion.

## Current Behavior and Required Changes

- `prepare_join_before_network` currently loads any nonterminal local join and calls `reopen_join_start`; changed input then fails because the old attempt identity is required to match every field.
- Current Spec 023 has repeated/conflicting rules in the slot, public contract, error table, scenarios, tests, and acceptance checklist. All must be reconciled, not patched in only one paragraph.
- The joiner stages are Initiated, Candidate, Prepared, Committed, Applied, Completed/Rejected. Safe local supersession ends before Prepared is durably recorded: before that, the sponsor cannot formally commit; after that, the Prepared message may already have reached the sponsor even if the joiner has not observed Commit.
- Existing encrypted attempt storage already has monotonic local ordinals, terminal facts, replay indexes, outbox supersession, and projection selection. The decision can be expressed by adding a durable superseded terminal reason and atomically replacing the local projection, not by invoking ResetSpace or FactoryResetSpace.
- No current public error uniquely means "the prior join can no longer be superseded". ADR 022 must require a stable conflict outcome rather than mapping this condition to generic code 1238 or pretending the new join started.
- Same invitation input starts a new local attempt, but sponsor-side one-time invitation claims remain immutable. If the old attempt already consumed the invitation, the new attempt is correctly rejected as used; the Engine must never rebind the old claim to the new attempt.

## External Research

- Stripe idempotency documentation says retries of one operation reuse one unique key and return the saved first result; reusing the key with changed parameters is rejected. Applicable principle: an attempt identity belongs to one logical action, while a fresh user action receives a fresh identity.
- AWS EC2 idempotency documentation says asynchronous work may have completed even when the caller cannot determine the result; same client token plus same parameters retries without repeating the action, while changed parameters fail with `IdempotentParameterMismatch`. Applicable principle: uncertainty after a remote commit-capable message requires recovery of the same operation, not replacement.
- Sources: https://docs.stripe.com/api/idempotent_requests and https://docs.aws.amazon.com/ec2/latest/devguide/ec2-api-idempotency.html (read 2026-08-18).
