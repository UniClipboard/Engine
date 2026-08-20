# Findings: Missing Workspace State Recovery

## Production Evidence
- The final `17:03:36Z` launch resumed the key session, then reported three peer-scope failures and forty delivery-view failures.
- The Windows dev database was created on 2026-07-18 and still contains three member, three trusted-peer, and three peer-address relationships.
- `workspace_convergence_state`, `workspace_convergence_v3_active`, `workspace_convergence_v3_slots`, and `workspace_convergence_v3_migrations` all contain zero rows.
- The database therefore matches an initialized legacy profile with no convergence state, not a malformed V2 or V3 convergence row.

## Root Cause
- `build_sync_engine_assembly` always supplies `WorkspaceConvergenceStateOrigin::CurrentInstallation`.
- When the repository returns no state, `WorkspaceConvergence::load_state` creates a fresh state and only sets legacy provenance for `UpgradeWithoutConvergenceState`.
- All previous repairs operate on existing V2 or V3 payloads and cannot affect this profile.

## Constraints
- The decision belongs to `WorkspaceConvergence`; desktop callers must not orchestrate it.
- Missing state alone is insufficient because a fresh install is also empty.
- Recovery must use durable initialized-profile evidence, run after the encrypted session is ready, and fail closed on unreadable evidence.
- No user content, device names, filenames, or paths may be logged.
