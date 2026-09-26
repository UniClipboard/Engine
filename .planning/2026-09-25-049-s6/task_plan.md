# Task Plan: Plan 049 S6 deletion and docs

Planning files live in `.planning/2026-09-25-049-s6/`.

## Goal

Finish plan 049 slice S6: delete the remaining legacy membership code listed in ADR-027 "删除", update
design docs, mark Core boundary items A1/A3/A4/A7 closed, run full acceptance.

## Decisions (user, 2026-09-25)

- Delete: Core `workspace_convergence` state machine (keep `AdmissionChangeFacts`), Engine test-only
  `workspace_convergence_summary`; `settlement_window`; ports `CurrentWorkspacePeerScope*`,
  `MembershipAdmissionGatePort`; `PendingRemovalFacts`.
- Delete the whole gossip/attestation protocol (outbound, candidate state machine, inbound endpoints, ALPN).
  Public contract `WorkspaceConvergenceSummary` unchanged.

## Phases

- [x] P1 Inventory gossip/attestation protocol (inbound owners, ALPN, old-peer dependency)
- [x] P2 Delete Core legacy items (1)-(4)
- [x] P3 Delete gossip/attestation protocol
- [x] P4 Docs: space-application, pairing-lifecycle, membership-history-ownership, Core boundary plan, 049 record
- [x] P5 Acceptance checks (plan 049 "验收")

## Errors
