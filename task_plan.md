# Space Rebuild Refactor Plan

## Goal

Move the reset-space rebuild workflow out of `facade/space_setup/facade.rs` into lifecycle modules while preserving reset recovery, phase-specific errors, and the stable facade contract.

## Constraints

- `SpaceFacade` remains the only external application entry point.
- `RebuildSpaceUseCase` is crate-private and owns only the shared single-device-space rebuild sequence.
- User-requested reset and legacy-profile isolation recovery remain separate application actions.
- The persisted rebuild target must survive interruption and reuse the same `SpaceId` on retry.
- Any new persisted business payload, including a rebuild target, must follow the repository encrypted-persistence rule.
- Do not retain `DeviceManagementResetUseCase` as a second implementation after migration.
- Keep `factory_reset` out of this refactor.

## Phases

- [x] Phase 1: Identify the existing reset workflow, public contract, and recovery paths.
- [x] Phase 2: Define the shared rebuild responsibility, input, output, ordered phases, and error categories.
- [ ] Phase 3: Define and implement `SpaceRebuildCheckpointPort` for durable pending-target progress, including encrypted infra persistence and adapter wiring.
- [ ] Phase 4: Implement `RebuildSpaceUseCase` in `space/lifecycle/rebuild_space.rs` using the checkpoint and data-transition ports.
- [ ] Phase 5: Implement `ResetSpaceUseCase` for the user-requested action.
- [ ] Phase 6: Implement `RecoverLegacyProfileIsolationUseCase` for the upgrade migration action.
- [ ] Phase 7: Wire the lifecycle use cases into `SpaceFacade`, migrate focused tests, and remove `DeviceManagementResetUseCase`.
- [ ] Phase 8: Update architecture documentation, format, compile, and run focused regression tests.

## Open Decisions

- Define one shared execution gate for user reset and legacy recovery so both flows cannot rebuild concurrently.
- Decide whether invitation cancellation is owned by `ResetSpaceUseCase` through an internal application dependency or stays as a facade pre-step. The final design must retain one owner for the full user action.
- Choose the exact `SpaceRebuildProgressPort` error type and encrypted infra representation.

## Current Status

Phase 3 is active. No behavior migration is complete yet.
