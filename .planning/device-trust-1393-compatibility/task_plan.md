# Device trust 1393 compatibility repair

## Goal

Preserve an existing pre-device-trust workspace while making all three required reads succeed after upgrade: space state, device list, and device trust. The repair must remain valid after restart and must not require deleting or recreating user data.

## Completion criteria

- [x] A deterministic old-write/new-read test reproduces the device-trust failure.
- [x] The precise failing record or lookup and underlying error are identified.
- [x] A compatibility read or migration preserves the existing encrypted workspace data.
- [x] Old workspace, new workspace, and restart cases pass automated tests.
- [x] Public failure mapping retains an actionable distinction where needed.
- [x] Architecture documentation maintenance record is updated.
- [x] Focused tests and required repository checks are complete; the architecture script's three unchanged baseline failures are documented.
- [x] The physical-device validation recipe is ready; the fixed commit identifier will be taken from the cohesive commit.

## Phases

1. **Complete**: Build and run the smallest red-capable compatibility loop.
2. **Complete**: Rank and test root-cause hypotheses against exact errors and records.
3. **Complete**: Add the failing regression test, then implement the minimal durable repair.
4. **Complete**: Verify old/new/restart behavior and public error behavior.
5. **Complete**: Complete fresh-space device-trust behavior, update architecture documentation, and run repository-wide delivery checks.
6. **Complete**: Create a cohesive commit and prepare the mobile validation handoff.

## Constraints

- Do not clear, replace, or reinterpret user workspace data in mobile.
- `WorkspaceConvergence` remains the complete workflow owner.
- Persisted sensitive state remains encrypted.
- Do not add a parallel legacy implementation or a client-side fallback.

## Errors encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| Exact test filter ran zero tests | 1 | Limited to `--lib`, removed `--exact`, and confirmed one test executed and failed. |
| Shared storage-test filter ran zero tests | 1 | Re-ran with `state_is_` and confirmed both target tests plus one unrelated matching test executed. |
| Architecture repository check reports three current-peer-scope errors | 1 | Reproduced the identical three errors from an unmodified `git archive HEAD`; recorded as a main-branch baseline blocker. |
| UniFFI public-contract test expected active local membership but received unavailable | 1 | Reproduced from unmodified `HEAD`, then fixed because fresh-space coverage is required by this task: device trust now reuses the existing legacy member-scope result and stays unavailable without proof. |
| Full UniFFI public-contract file had worker-shutdown and follow-on startup failures | 2 | Parallel run passed 19/22; serialized run passed 20/22 but the same unrelated worker-drop timeout still polluted the following startup. Target device-trust, space-management, and restart tests pass independently; rerun the one follow-on failure independently. |
