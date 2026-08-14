# Findings

## Initial evidence

- Physical iOS logs show Engine startup, space-state read, and two-device listing succeed.
- Only `query_device_trust` fails with public code 1393.
- The public mapping currently collapses multiple convergence failures into one result.
- Mobile uses Engine revision `6ec8c35c21220c4b41b8be31f235c111b2226e73`; current repository HEAD is `94c5964`.
- Existing data must be preserved; clearing or recreating the workspace is outside acceptance.

## Architecture ownership

- `WorkspaceConvergence` owns the complete query and decision workflow.
- Infrastructure owns encrypted persistence format compatibility.
- The facade may map stable errors but must not recreate business rules.

## Initial code comparison

- The device-trust commit did not add a serialized field to `WorkspaceConvergenceState`.
- The new query loads convergence state and then additionally lists member profiles to build display names.
- Ordinary space-state and device-list reads use separate paths, so their success does not prove convergence-state readability.
- The current public conversion hides whether state loading, member listing, or another query step failed.

## Confirmed root cause

- Mobile used Engine `6a7b644` immediately before pinning `6ec8c35`.
- ADR-020 replaced the persisted convergence model wholesale: the old ordered workspace-change state became signed membership-history state.
- Both formats reused the same encrypted payload AAD/version marker and database row without a migration.
- The current decoder can misinterpret old positional postcard bytes as the new struct, then fails the row integrity check. The focused test reproduces this as `workspace convergence state row integrity mismatch`.
- This explains why the workspace and roster remain readable while the newly required device-trust query fails.

## Implemented compatibility boundary

- New writes include an encrypted payload format marker.
- The reader supports the two unversioned signed-history layouts that were actually used and the final pre-ADR-020 layout from `6a7b644`.
- A successful legacy read is immediately rewritten in the current encrypted format and remains readable after restart.
- Pre-ADR-020 state preserves verifiable space, local identity, effective-device, pending-admission, removal, revision, and timestamp facts. It does not fabricate signed history.
- Migrated peers are reported as upgrade-required and continue through the existing shared legacy-upgrade flow; the migration marker clears after current signed history exists.
- Unrecognized encrypted state maps to a distinct stable corrupt-state error instead of generic 1393.

## Binding verification boundary

- The focused UniFFI device-trust public-contract test executes one test but expects a newly created space to report active local membership and currently receives the binding's non-fatal unavailable snapshot.
- The same assertion failure reproduces from an unmodified archive of `HEAD`, so it is not introduced by the compatibility repair and is not counted as passing evidence.
- The binding failure exposed a fresh-space gap relevant to acceptance: device trust ignored the existing Engine-owned legacy member-scope result before signed history exists. The query now reuses that scope result for local membership and remains unavailable when neither legacy protection nor current history proves membership.
