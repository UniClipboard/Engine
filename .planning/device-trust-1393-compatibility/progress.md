# Progress

## 2026-08-14

- Read the investigation handoff and confirmed a clean `main` worktree at `94c5964`.
- Read repository, application-facade, core, and infrastructure instructions.
- Selected diagnosis-first and test-first workflows.
- Started phase 1: build a deterministic old-write/new-read compatibility loop.
- Compared the pre-feature release with the device-trust commit; no direct serialized-state field addition was found.
- Narrowed the first probes to convergence-state loading versus member-profile listing.
- Traced mobile's immediately preceding Engine pin to `6a7b644`.
- Added and ran a real encrypted old-layout/new-reader test. It failed deterministically at row integrity validation, confirming the persistence migration gap.
- Rejected the first over-filtered command because it ran zero tests; reran the exact library test and observed one failing test.
- Implemented versioned encrypted writes and one-time reads for used unversioned layouts.
- Verified five storage tests and 48 convergence tests, including restart, migrated query, and two-device legacy upgrade.
- Verified distinct corrupt-state mapping at the application and Engine boundaries.
- Updated the architecture bible body and maintenance record.
- Required checks passed: locked metadata, workspace all-target compilation, rustfmt, and diff check.
- Architecture script remains blocked by three identical main-branch baseline failures reproduced from an unmodified archive.
- Ran the mobile UniFFI public-contract test and confirmed one test executed. It failed because the returned sync relationship was `unavailable` rather than the fixture's expected `active`; investigation is in progress and this is not counted as passing evidence.
- Reproduced the identical UniFFI assertion failure from an unmodified `HEAD` archive, establishing it as a main-branch baseline failure rather than a regression from this repair.
- Replaced one newly added Chinese code comment with English to comply with repository rules; behavior is unchanged.
- Added a storage-level check that a correctly encrypted but unsupported payload is classified as corrupt, covering the source of the new public 1394 distinction.
- The acceptance-required fresh-space binding flow exposed an existing query gap: the device-trust read ignored the Engine-owned legacy member-scope result when signed history did not yet exist. A first attempt to initialize signed history during space creation failed because fresh legacy-protection spaces do not yet have current signing material; that attempt was removed.
- Reused the existing current-member-scope result in device trust only when no local member instance is available. Added positive legacy-mode and negative ready-without-history tests; 13 focused device-trust tests passed.
- The UniFFI device-trust test, full space-management test, and process-restart identity test each executed one test and passed after the query correction.
- Full UniFFI public-contract run executed 22 tests: 19 passed; one worker-shutdown timeout and two resulting process-wide network-node startup conflicts failed. This run is not passing evidence; a serialized rerun is required.
- Serialized full UniFFI run executed 22 tests: 20 passed; the same unrelated worker-drop timeout failed and left the process-wide node occupied for the immediately following export test. The full file remains non-green; target acceptance tests pass independently.
- Final review tightened the corrupt-state boundary: a versioned payload whose embedded space or timestamp disagrees with its database row now maps to the same distinct corrupt result. Added a direct storage test.
- Final verification passed: locked metadata, workspace all-target compilation, formatting, diff check, 7 storage tests, 50 convergence tests, 4 core state tests, 1 Engine error-contract test, and the three independently run binding acceptance flows.
- The architecture check still reports only the same three current-peer-scope baseline errors previously reproduced from an unmodified `HEAD` archive. The full binding file still contains the unrelated worker-drop timeout; target tests and its follow-on victim pass independently.
- Created the cohesive local commit `fix(membership): migrate legacy device trust state`; mobile can pin the final amended commit identifier from the handoff.
