# Findings: rc.3 Workspace Recovery Repair

## Requirements
- Make the recent workspace recovery actually repair the affected persisted profile.
- Preserve encrypted persistence and fail-closed behavior.
- Verify the work before reporting.
- Update `docs/architecture/architecture-bible.md` for every repository change.

## Research Findings
- The desktop worktree pins Engine `f287d32715fbdac7655db5f8642e3301182ced68`.
- The latest launch still reports current peer scope unavailable and cascades into history delivery failures.
- Diagnostic commit `4e04a0d` observed 103 instances with Ready protection, three member rows, no current history, and `migrated_from_pre_adr_020=false`.
- `d3f7aef` converted rc.3 V2 state to V3 while preserving the false provenance bit.
- `8ce0c84` repairs direct V2 reads only, so it cannot repair a state already converted by `d3f7aef`.
- `f287d32` repairs V3 only when phase is `Complete`, own identity exists, peer relationships are nonempty, and current history is absent.
- Production state transitions initialize to `LocallyApplied`; the pre-ADR-020 converter uses `Converging`. Phase is therefore not durable migration provenance.
- `recover_legacy_migration_marker` only clears a true marker after current history exists. It does not reconstruct a missing marker.
- The repository test seam can write the exact rc.4 V3 layout directly into the active encrypted slot, then reopen it through current production loading.

## Technical Decisions
| Decision | Rationale |
|----------|-----------|
| Reproduce the two-step migration in the repository test | It exercises the same persistence boundary that failed in production. |
| Remove only the invalid phase requirement if stable identity and peer evidence remain | This is the smallest change supported by the observed profile and preserves fail-closed checks. |
| Add explicit negative cases for missing identity, missing peer evidence, removed state, and recovery failure | These prevent broad inference from absence of current history alone. |

## Issues Encountered
| Issue | Resolution |
|-------|------------|
| Sanitized diagnostics do not expose the persisted phase or relationship map | Use release source semantics and replay the exact upgrade sequence at the repository seam. |

## Resources
- `crates/uc-infra/src/db/repositories/workspace_convergence_store.rs`
- `crates/uc-application/src/space/convergence/membership/bootstrap.rs`
- `docs/architecture/architecture-bible.md`
- `docs/adr/021-workspace-convergence-internal-boundaries.md`
- `docs/adr/023-legacy-profile-isolation-and-re-pairing.md`
