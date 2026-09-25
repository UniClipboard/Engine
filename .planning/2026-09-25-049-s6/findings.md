# Findings

## Gossip/attestation protocol (2026-09-25)
- ALPN `uniclipboard/membership-gossip/1` (`MEMBERSHIP_ATTESTATION_ALPN`), adapter
  `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs` (2478 lines, attestation + gossip transport).
- `IrohNode::install_membership_handler` / `install_membership_attestation_handler` are never called outside
  node.rs tests: Engine never registers the ALPN, so the protocol is unreachable in production today. Deleting it
  does not change device-to-device behaviour (old peers dialing it already fail with ALPN not supported).
- Outbound `attest_candidate` / `MembershipGossipTransportPort::exchange` have no production caller.
- `MembershipSecurityUpdatePort` (relayed group epoch updates) only serves this protocol; the same Infra adapter
  also implements live `ApplyMembershipSecurityPort` (keep).
- `CurrentMembershipAnnouncementPort` is used by `InitializeSpaceMembershipUseCase` (keep).
- Public `WorkspaceConvergenceSummary` is never produced in production; keep contract (non-goal).
- settlement_window is used by space_admission/state.rs (kept; earlier grep excluded membership/).
