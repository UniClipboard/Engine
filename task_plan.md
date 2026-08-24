# Workspace Convergence Responsibility Refactor

## Goal

Replace `WorkspaceConvergence` with four business-owned modules while preserving
the stable Engine contract, persisted state, restart recovery, and network
protocol behavior.

## Completion Criteria

- `SpaceFacade` composes `SpaceAdmission`, `WorkspaceMembership`,
  `SpaceConnectivity`, and `SpaceLifecycle`.
- Rebuild, reset, and Engine-version upgrade do not depend on
  `WorkspaceConvergence`.
- Admission has one complete owner from invitation through durable completion
  and recovery.
- Membership has one complete owner for history, verified admission, removal,
  decisions, reconciliation, and current-member scope.
- Connectivity consumes the effective member scope and owns connection and
  network recovery behavior.
- `WorkspaceConvergence` and permanent forwarding adapters are deleted.
- Existing public Engine behavior and persisted formats remain unchanged.
- ADR-021, Spec-024, and the architecture bible describe the final ownership.
- Focused tests, workspace checks, formatting, architecture checks, and diff
  checks pass, or pre-existing blockers are documented with evidence.

## Constraints

- Preserve all existing user changes in the dirty worktree.
- Move one observable behavior slice at a time and remove its old path in the
  same slice.
- Tests exercise the new business interface, not internal step ordering.
- Do not add new persistence formats, public protocol steps, dependencies, or
  compatibility layers.
- Keep sensitive persisted payloads encrypted and logs free of sensitive data.

## Phases

- [x] Phase 1: Freeze new responsibilities on `WorkspaceConvergence` and agree
  the target ownership model.
- [x] Phase 2: Inventory callers, public and restricted methods, state owners,
  locks, persistence, restart paths, endpoints, runtimes, and tests.
- [ ] Phase 3: Complete migration of Space rebuild, reset, and Engine upgrade
  into `SpaceLifecycle`, including old-space admission-state cleanup.
- [ ] Phase 4: Merge invitation and durable admission behavior into one
  `SpaceAdmission` owner.
- [ ] Phase 5: Move remaining membership history, admission effects, removal,
  decisions, reconciliation, and current scope into `WorkspaceMembership`.
- [ ] Phase 6: Move effective-scope connectivity and network recovery into
  `SpaceConnectivity`.
- [ ] Phase 7: Rewire assembly and facade, then delete `WorkspaceConvergence`
  and obsolete forwarding interfaces.
- [ ] Phase 8: Update ADR-021, Spec-024, architecture bible, run final review,
  verification, and commit the completed refactor.

## Current Slice

Phase 3 is active. Remove the mixed `SetupStatusPort` snapshot in six verified
slices:

1. [x] Complete independent Space rebuild progress ownership and delete its
   setup status methods. Implementation and source-level verification are
   complete; focused runtime tests remain blocked by the wider application
   compilation failures.
2. [x] Extract durable re-pairing requirement ownership. The application owner,
   encrypted V1 store, rebuild/upgrade/query/pairing wiring, and old field
   deletion are complete; focused runtime tests remain blocked by the wider
   application compilation failures.
3. [x] Establish a canonical current-Space identity owner. The renamed
   `ActiveSpaceGenerationManifest` remains only the atomic pointer to a fully
   generated key/database/security set; the resolver uses it first and falls
   back only when absent to a profile-encrypted Legacy ID. Production readers
   and initial activation are wired; focused tests remain blocked by wider
   application compilation failures.
4. [x] Make current-Space identity the profile-readiness result and add an
   idempotent factory-reset action that clears both generation and Legacy
   identity records. Production reset wiring is complete; focused tests remain
   blocked by the wider application compilation failures.
5. [x] Remove the legacy setup-status file from runtime recovery,
   configuration migration, and Engine wiring. Portable exports now carry the
   encrypted current-Space identity instead of a setup marker.
6. [x] Delete `SetupStatusPort`, `SetupStatus`, `SetupStatusFacade`, the
   manifest projection wrapper, and all obsolete tests/fakes. The only retained
   `.setup_status` text is the immutable interrupted-rebuild progress filename.

Each slice preserves interrupted rebuild recovery and existing file paths,
move all readers before deleting the old writer, and leave no parallel runtime
source of truth.

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| Existing root plan described only the earlier rebuild extraction | 1 | Replaced it with the approved end-to-end responsibility plan. |
| Worktree contains extensive incomplete user edits | 1 | Treat current files as authoritative and avoid reverting unrelated changes. |
| One patch tried to replace a file with delete and add operations together | 1 | Split each replacement into separate patch operations. |
