# Progress: Admission Expiry S2-S8

## 2026-09-14
- Confirmed the S1 worktree is clean and commit `91287ca1` exists.
- Read the implementation, Rust, storage, error, security, observability and admission ownership rules.
- Re-read ADR-026 and the S2-S8 implementation plan.
- Created the persistent plan for sequential, one-commit-per-slice delivery.
- Current work: investigate S2 code paths and define the first failing end-to-end test.
- Added the V2 authenticated attempt contract to the initial network handshake and bound it into the existing password-authentication transcript.
- Preserved V2 across every production reply, persisted envelope round trip and restart; mixed V2 authentication/V1 messages now fail as upgrade-required.
- Added Sponsor AwaitingPeerConfirmation, Unconfirmed and Confirmed persistence, exact original-deadline recovery and late CompleteAck settlement.
- Added exact current-member/Add matching to device trust queries without changing membership revision or sync state.
- Added Core, Application and real network test coverage. Rust formatting, diff check, Rust style and observability privacy checks pass.
- Permission was restored; Core, Application, Infra storage, real loopback network and full workspace checks pass.
- Fixed the device-query test fixture so confirmation is verified against a real activated Add record rather than a baseline-only member.
- Strict review moved the combined admission display query out of Joiner cancellation into its own focused module; no blocking structural findings remain.
- Current work: finish final post-review gates and create the S2 commit.
