# Progress

## 2026-08-13

- Read the handoff, repository instructions, CONTEXT glossary, ADR-017, ADR-020, product contract 021, and architecture bible.
- Confirmed repository and Desktop pin both use Engine `6ec8c35` and the worktree started clean.
- Selected diagnosing-bugs, test-driven-development, and planning-with-files workflows.
- Defined completion criteria and began the reproduce/minimize phase.
- Queried both profile logs through the Desktop log tool. Confirmed repeated admission rejection and discovered that membership-history hello signatures fail despite matching device, member instance, and lineage identifiers.
- Identified existing real-engine E2E seams for pairing/restart/content and focused presence behavior.
- Ran `members_converge_when_sponsor_stays_offline_after_joining_c`: 1 test passed in 59.53s, including receiver-side bidirectional content before and after restart.
- Confirmed pairing completion ignores failed pending protection-state delivery and presence marks Online before remote admission is known.
- Added the focused outbound-admission presence regression. The first command selected 0 tests and was discarded; the full exact name then ran 1 test and failed as intended (`Online` returned where `Offline` is required).
- Added and observed a second red regression: stale persisted local member instance was allowed to begin a new admission.
- Implemented local-identity consistency validation before admission and remote rejection confirmation before publishing Online.
- Updated the architecture bible behavior and maintenance record.
- Verification passed: 38 convergence tests, 16 presence tests, and the real three-device offline-member pairing scenario with bidirectional receiver persistence before/after restart.
- Replaced the provisional timing window with an explicit current-space accept/reject handshake and advanced the private presence protocol version.
- Re-ran all 16 presence tests and the real three-device scenario after the protocol change; both passed.
- Required checks passed: Cargo metadata, full workspace/all-target check, formatting, architecture preflight, and diff whitespace validation.
- Original Desktop profiles A/C were not rerun with this worktree: their daemons were stale/stopped and the Desktop checkout remains pinned to the pre-fix Engine revision. This is skipped, not passed.
