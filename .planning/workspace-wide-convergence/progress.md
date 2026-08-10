# Progress Log

## Session: 2026-08-10

### Reopened after independent specification review

- **Status:** in progress
- An independent review found six material gaps in the first documentation draft: the offline-member scenario used the wrong roles; recovery could disclose current data to a removed instance; recovery payload encryption was underspecified; the unified snapshot omitted the removal-intent count; `JoinSpace` completion was overstated; and ADR responsibility statements still conflicted.
- Next: inspect the current security-session and membership-proof implementation, correct the ADR/specification/architecture documents, then repeat validation.
- Read the ADR-016 and specification-016 draft in full. Confirmed exact contradictions in the baseline offline scenario and `JoinSpace` result; recorded line-level evidence in `findings.md`.
- Read ADR-015 and the related specification. Confirmed that ADR-015 still assigns complete ownership to a separate module, while its specification correctly anticipates the unified owner and requires a removal-intent count in the final snapshot.
- Read the relevant architecture overview. It requires application-layer encryption for non-file business messages and still contains the outdated standalone member-removal ownership statement. A broad source search was truncated, so the next research pass will inspect the known recovery, session, and attestation files directly.
- Searched the in-progress recovery port and current security/attestation references. The recovery port is a limited push-only update path, while attestation has a Space- and peer-bound transcript. Both broad command outputs were truncated, so exact source slices are required before selecting the specification's payload-protection rule.
- Read the focused recovery loop and session key catalog. A lagging member can lack the current content key, and the current recovery loop has no modeled authorization or acknowledgement. The documentation will require a new authenticated, per-handoff encryption context rather than relying on either existing mechanism by itself.
- Completed a focused security investigation with source evidence. The existing attestation proves peer identity and binds a fresh transcript, but its frames have no application-layer AEAD. Existing session material has historical purpose-separated keys and an AEAD framing precedent. The corrected specification will require a one-use handoff key derived from verified historical material plus fresh endpoint contributions, with current-member authorization before every offer.
- A follow-up independent review found one more material gap: the 64-change handoff bound lacked continuation and final-ack semantics. Added it to the correction checklist; the implementation specification will require range-aware batches, immediate next-batch scheduling, and only final-target acknowledgement to clear a whole pending handoff.
- Verified that the workspace already has a maintained AEAD dependency and that the existing OpenMLS validation contains the correct three-device relay topology. The documentation will use them as implementation constraints without claiming the full workflow is already implemented.
- Corrected ADR-016's offline topology and acceptance criteria, and explicitly made it revise ADR-015's ownership and public-state boundary. Updated ADR-015 so `WorkspaceConvergence` owns the complete workflow while the ADR retains its intent legality and merge rules.
- Corrected specification 016: `JoinSpace` now means local readiness only; the snapshot includes `removal_intent_count`; recovery requires a current-effective-member check before any offer; encrypted request/offer/ack binding and replay rejection are specified; multi-batch continuation is explicit; and the acceptance matrix now covers sponsor crash, removed-instance requests, message tampering/replay, and over-limit transfer recovery.
- Searched the remaining member-removal specification and architecture overview. Specification 015 needs only the literal snapshot field name. The architecture body still contains old owner and event names; historical maintenance records will be preserved, while the current body will get an explicitly pending ADR-016 target boundary.
- Updated specification 015 and the architecture overview. The overview now labels the ADR-016 boundary as pending rather than pretending a new recovery channel or unified interface already exists, while its current ownership guidance points to the future single owner. Historical maintenance records were left intact.

### Phases 1-5: Discovery, design, documentation, verification, and delivery

- **Status:** complete
- Actions taken:
  - Read the ADR-016 decision and its acceptance paths.
  - Read the ADR-015 and ADR-012 implementation specifications.
  - Checked the working tree and identified concurrent source changes that are outside this task.
  - Located prior project research on membership convergence for terminology and validation boundaries.
  - Mapped current separate convergence, refresh, removal, and event contracts to the ADR-016 replacement requirement.
  - Confirmed that in-progress code contains only a narrow recovery-transfer mechanism and new acceptance tests; no implementation result is claimed by this documentation work.
  - Drafted the ADR-016 specification, indexed it, and added the required architecture maintenance entry.
- Errors encountered:
  - A documentation search used unescaped backticks and the shell attempted command substitution. The command did not modify files; later searches use literal quoted patterns.
  - A broad ADR-015 consistency patch failed its context check and made no changes. The update will be split after inspecting the exact paragraphs.
  - Reconciled ADR-015's final public state and entry references with ADR-016, without changing its removal-intent rules or acceptance coverage.
  - Replaced an invented recovery-channel identifier with an implementation-stage naming requirement and added a bounded, history-proven request for missing continuous changes.
  - Verified documentation links, required contracts, obsolete interface references, whitespace, and diff whitespace.
  - Ran the required repository checks and reviewed the final documentation diff.
- Files created/modified:
  - `.planning/workspace-wide-convergence/task_plan.md`
  - `.planning/workspace-wide-convergence/findings.md`
  - `.planning/workspace-wide-convergence/progress.md`

## Test Results

| Test | Input | Expected | Actual | Status |
| --- | --- | --- | --- | --- |
| Planning-session catchup | Current worktree | No unfinished prior plan context | No catchup output | Passed |
| Documentation links and contract assertions | Edited documentation | Valid links and required ADR-016 contracts | Passed | Passed |
| Whitespace and diff checks | Edited documentation | No whitespace errors | Passed | Passed |
| `cargo metadata --locked --format-version 1` | Workspace | Metadata resolves | Passed | Passed |
| `cargo check --workspace --all-targets --locked` | Workspace | Workspace compiles | Passed | Passed |
| `node scripts/architecture/check-engine-repository.mjs` | Workspace | Repository structure is valid | Passed | Passed |
| `cargo fmt --all -- --check` | Workspace | All Rust is formatted | Failed on two pre-existing parallel source edits | Blocked outside documentation scope |

## Error Log

| Timestamp | Error | Attempt | Resolution |
| --- | --- | --- | --- |
| 2026-08-10 | `cargo fmt --all -- --check` | 1 | Reported two formatting changes in concurrent source files; documentation files were not implicated or modified. |
| 2026-08-10 | Broad security-source search output was truncated | 1 | Switched to focused source slices for the recovery transport, attestation transcript, and session encryption evidence. |
| 2026-08-10 | Planning patch mixed contexts | 1 | No files changed; split the patch and inspected the current progress context before retrying. |
| 2026-08-10 | Unrelated input prompt triggered during review coordination | 1 | Ignored the response; it did not affect the documentation task. |

## 5-Question Reboot Check

| Question | Answer |
| --- | --- |
| Where am I? | Phase 1: mapping requirements to current contracts. |
| Where am I going? | Design, documentation changes, verification, delivery. |
| What's the goal? | An implementation-ready ADR-016 specification with consistent project documentation. |
| What have I learned? | See `findings.md`. |
| What have I done? | See Phase 1 above. |

## Session: 2026-08-10 (implementation)

### ADR-016 implementation, stages 1-5

- **Status:** stages 1-5 complete; stage 6 (multi-device end-to-end acceptance + old-flow removal) pending.
- Stage 1 (prerequisite): added `a_gapped_commit_cannot_be_applied_directly` to the OpenMLS validation; the three-device relay test, fork rejection and gapped-commit rejection all pass.
- Stage 2 (core model + encrypted persistence): added `uc-core/src/membership/workspace_convergence.rs` (WorkspaceChange, digest, confirmation, phase machine with `apply(event)` single entry, pending handoff records, removal-intent records) and `uc-core/src/membership/recovery_exchange.rs` (RecoveryRequest/Offer/Ack/Reject with transfer bounds). Added `WorkspaceConvergenceRepositoryPort` and the encrypted `DieselWorkspaceConvergenceStore` with migration `2026-08-10-000003_create_workspace_convergence`; plaintext-leak and stale-space tests pass.
- Stage 3 (unified owner): added `uc-application/src/workspace_convergence/` — `WorkspaceConvergence` now owns removal submission (intent -> continuous removal change in one commit), admission-change recording, handoff progress, confirmations, readiness records, query/snapshot and the restricted-recovery endpoint with current-effective-member authorization. The roster facade and app facade route `RemoveMember`/`QueryWorkspaceConvergence` through it; it also implements RemovalTargetGatePort and RemovalAdmissionGatePort.
- Stage 4 (restricted handoff runtime): added `uc-infra/src/security/recovery_seal.rs` (one-use HKDF-derived key from a historical transport key + fresh endpoint contributions, XChaCha20-Poly1305, strong binding AAD) and the `workspace-recovery/1` iroh adapter with size/concurrency bounds; installed in the sync engine assembly.
- Stage 5 (stable interface + bindings): deleted `RefreshSharedDevices`, `QuerySharedDeviceRefresh`, `QueryMembershipConvergence`, `QueryMemberRemoval`, `MemberRemovalChanged`, `SharedDeviceRefreshChanged` and their summary types from uc-engine and the uniffi/ohos bindings; added `QueryWorkspaceConvergence`, `WorkspaceConvergenceChanged` and the unified `WorkspaceConvergenceSummary`. The engine host-adapter contract and all binding tests pass.
- Verification: `cargo check --workspace --all-targets --locked`, `cargo test --workspace`, `cargo fmt --check`, engine-repository preflight, and `git diff --check` all pass. Five pre-existing pairing orchestrator tests fail only because of concurrent uncommitted pairing WIP in the working tree (out of ADR-016 scope).
