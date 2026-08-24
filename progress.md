# Progress Log

## 2026-08-21

- Resumed the existing dirty worktree without reverting user changes.
- Replaced the stale rebuild-only plan with the approved eight-phase
  `WorkspaceConvergence` responsibility refactor.
- Confirmed the target owners: `SpaceAdmission`, `WorkspaceMembership`,
  `SpaceConnectivity`, and `SpaceLifecycle`, composed by `SpaceFacade`.
- Began Phase 2 inventory of callers, interfaces, state, recovery, endpoints,
  runtimes, and tests.
- Confirmed the first implementation slice: clear prior-Space admission state
  through a narrow internal adapter, not through `WorkspaceConvergence` and not
  by injecting the low-level admission repository into Space rebuild.
- Completed the Phase 2 inventory of responsibilities, state, locks,
  persistence, runtime recovery, endpoints, callers, and test locations.
- Classified lifecycle, admission, membership, and connectivity ownership and
  recorded the migration invariants in `findings.md`.

## Verification

- No new implementation verification yet. A current baseline check is next.
- A baseline `uc-application` check reported 45 errors from the existing
  half-completed lifecycle move. Per user direction, these are not being fixed
  as a separate preliminary phase; they will disappear as the old path is
  replaced and final wiring is completed.

## Errors

- The prior root planning files were stale relative to the approved broader
  refactor; they were replaced while retaining their relevant lifecycle facts
  in `findings.md`.
- The first combined replacement patch was rejected; replacements were split
  into valid patch operations.

## 2026-08-22

- Began paired implementation of the approved six-slice removal of the mixed
  setup-status snapshot. Slice 1 first completes independent Space rebuild
  progress ownership before any re-pairing, active-Space, or readiness split.
- Switched manifest-based setup projection to the independent rebuild-progress
  source and restored focused projection tests using the real file adapter.
  The tests are written and formatted but remain unexecuted because the current
  application ownership migration still fails compilation with 67 errors.
- Switched the final production reset-commit query in `SpaceFacade` to the
  independent rebuild-progress source. No production application caller reads
  rebuild targets through `SetupStatusPort`; the application compile baseline
  remains the same 67 unrelated ownership-migration errors.
- Deleted the three rebuild-target methods from `SetupStatusPort` and both
  setup-status implementations. Facade tests now use an independent shared
  progress fake. Core checks pass and all-target application checking reports
  no rebuild-progress errors, but still stops on the same 67 wider migration
  errors before focused runtime tests can execute.
- Started slice 2 with a standard `space/re_pairing/` application module. Its
  store port only reads and saves state; `RePairingState` exposes the explicit
  relationship-reset and successful-pairing transitions, covered by focused
  in-memory tests. Production storage and caller wiring are still pending.
- Completed slice 2 without legacy migration: a missing new record means
  re-pairing is not required. The V1 record is profile-encrypted, both boolean
  values are persisted, rebuild requires it, successful active joining clears
  it, upgrade checks it, and setup-state queries expose it. The old
  `SetupStatus.re_pairing_required` field and internal reads/writes are gone.
  Core checks pass; application and Engine remain at the same 67 unrelated
  ownership-migration errors, which prevent the new focused tests from running.
- Began slice 3 with an application-owned read-only active-Space interface and
  a manifest-backed infrastructure adapter. Missing, promoted, and corrupt
  manifest tests are written and formatted but cannot execute until the same
  67 application migration errors are resolved. No production reader has been
  switched because first-time creation does not yet guarantee a manifest.
- Corrected the slice 3 model after tracing startup and transition behavior:
  renamed the V2 type/store/modules to `ActiveSpaceGenerationManifest`, kept
  all persisted strings and bytes unchanged, and deleted the incorrect generic
  active-Space port/adapter before production wiring. The focused core manifest
  test ran once and passed; the wider application baseline remains 67 errors.
- Completed the replacement current-Space model: a profile-encrypted Legacy ID
  is activated only by successful first-time creation, while a valid generation
  manifest takes precedence and a corrupt one fails closed. Main production
  readers now use `CurrentSpaceIdentityPort`, and `SetupStatus.space_id` is
  deleted. Core tests pass; resolver tests are written but remain blocked before
  execution by the same 67 application migration errors.
- Completed current-Space factory-reset safety: Legacy identity is deleted
  before the generation manifest, and the facade removes current identity only
  after key material, peer state, rebuild progress, re-pairing state, and
  invitations are cleared. Core checks pass; four-mode resolver tests are
  written but still blocked by the same 67 application errors.
- Removed the remaining setup-status runtime and type surface. Config bundles
  materialize and carry the encrypted current-Space identity; generation
  manifests never travel with source-only generation paths. `SetupStatusPort`,
  `SetupStatus`, the setup facade, file repositories, projection wrapper,
  Engine wiring, and obsolete fakes are gone. Only the immutable legacy rebuild
  progress filename still contains `.setup_status`. Core all-target checks pass;
  workspace checks remain blocked by the same 67 admission/membership errors.
- Standardized first-time Space initialization under `space/initialize_space/`:
  request/result/error/port/use-case ownership is local, errors preserve sources
  through the shared anyhow constructor macro, Facade only converts public
  strings, and infra implements the application-owned initialization port.
  Core checks pass and application/Engine remain at the same 67 unrelated
  migration errors.

- Began the Phase 3 unlock slice without adding tests, per explicit user
  direction; existing verification remains required.
- Moved `UnlockSpacePort` from `uc-core` into the application-owned
  `space/unlock_space` module and removed the old core and infrastructure
  narrow-port implementations.
- Added the Engine composition-root adapter from the aggregate space-access
  store to the application unlock seam.
- Moved the post-unlock upgrade, backfill, membership-readiness check, and
  best-effort presence prime into `UnlockSpaceUseCase`; deleted the old
  lifecycle unlock implementation and reduced the facade unlock method to
  input/result conversion.
- `git diff --check` passed for the touched scope. The first application check
  still reported the pre-existing incomplete admission/membership move plus
  an unlock module visibility error; the visibility error has been corrected.
- Final reference scan confirms `UnlockSpacePort` is defined only under the
  application unlock module; core and infrastructure no longer define or
  export the narrow port, and the old lifecycle path and internal unlock
  command have no remaining references.
- `cargo check -p uc-core -p uc-infra --all-targets --locked`, Cargo metadata,
  and `git diff --check` passed.
- `cargo check -p uc-application --lib --locked` reaches the unfinished
  admission/membership migration and reports 66 existing errors; none refer to
  `unlock_space` or post-unlock readiness.
- The architecture check is blocked because its current-peer-scope rule still
  reads the deleted `space/convergence/connectivity/reachability.rs` path.
- The workspace format check remains blocked by formatting differences across
  the broader in-progress refactor. New unlock module files were formatted
  directly without reformatting unrelated user changes.
- No tests were added or run for this slice, per explicit user direction; the
  application test target cannot compile until the broader migration errors
  are resolved.
