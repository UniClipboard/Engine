# Task plan: pairing latency (t-0147)

Goal: shorten two-device pairing (invitation reaches joiner Engine -> joiner commits Settled) from the field-measured 28.7 s toward ~6.5 s without changing the admission wire protocol. Source of truth for design and evidence: `docs/exec-plans/active/038-pairing-local-latency-budget.md`.

Owner / caller / result / retry:
- Owner: Application `SpaceAdmissionProtocol` (pairing) and the single space work owner; Engine `SessionSupervisor` for session handover ordering.
- Caller action: unchanged (`join_space`, then query membership).
- Success: same final member state; failure results unchanged; no new persisted format.
- Retry/restart: unchanged; cancelled maintenance rounds re-run because work is persisted.

## Slices (order fixed)
1. [done] Step 1: pairing request preempts a running ordinary maintenance round (revocable work permit).
2. [done] Step 2: Space background recovery starts only after the new session's handlers are published.
3. [ ] Step 4: joiner persistence (snapshot reuse, fewer commits); check what 038 Slice 1 already did.
4. [ ] Step 5: reuse one admission connection for rounds 1-3 with fallback to reconnect.
5. [ ] Step 6: invitation resolver HTTP client reuse inside Infra (no new Engine API).
6. [ ] Step 7: joiner prepare steps (only if output stays byte-identical).
7. [ ] Step 3: handover gaps. Do NOT shorten the quiesce drain (spec 043 / 038 record the failed attempts); only remove avoidable DB opens.
8. [ ] Plan A: after Settled rebuild session. Needs user re-confirmation (changes crash-recovery semantics); do last.

Constraints: Cargo serial in repo root; no CARGO_TARGET_DIR override; no real-device or user-space actions; synthetic profiles only.
