# Progress

## 2026-09-25
- Inventory: Application legacy already removed in S3-S5; Core leftovers identified.
- P2/P3 done: deleted gossip.rs, workspace_convergence.rs (AdmissionChangeFacts -> admission_change_facts.rs),
  PendingRemovalFacts, ports (attestation/gossip/security-update/workspace peer scope/admission gate,
  wait_for_announcement_change), matching errors; Infra attestation adapter -> membership_identity_adapter.rs
  (identity + announcement), node build/install fns + ALPN route, MembershipSecurityUpdatePort impl and unused
  fields; observability ConnectionPurpose/InboundPeerProtocol variants; Engine test-only summary mapping.
- workspace check, lan-compat, dev-tools check, fmt, style, engine repository, diff check: pass.
- A3/A4 were not fully closed (rules sat in Application); moved to Core `MembershipLedger::read_model` /
  `admits_inbound_peer` with 2 new Core tests; Application delegates. A7 only partial (branch recovery states remain).
- Docs updated: space-application, pairing-lifecycle, membership-history-ownership, core-boundary plan,
  spec 022 + product spec 021 supersession notes; observability inventory regenerated.
- Acceptance: nextest 2993/2993, inbound 1/1 5/5, membership-e2e 51/51, all delivery checks pass. Nothing committed.
