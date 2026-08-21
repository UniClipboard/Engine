# Progress Log

## 2026-08-21

- Located the existing reset implementation in `facade/space_setup/facade.rs`.
- Distinguished user-requested reset from upgrade-triggered legacy profile isolation recovery.
- Agreed to extract a shared private rebuild workflow, a user reset use case, and a separate legacy recovery use case.
- Defined the rebuild method shape as `execute(pending_target: Option<SpaceId>) -> Result<SpaceId, RebuildSpaceError>` for the current design discussion; final shape may change if the shared execution gate requires the rebuild workflow to load progress itself.
- Defined phase-specific `RebuildSpaceError` categories in the user-created lifecycle errors module.
- Identified the persisted pending target as a crash-recovery checkpoint.
- User created `SpaceRebuildCheckpointPort` in `uc-core` with `load`, `save`, and `clear` operations. The core interface still needs English doc comments per repository rules, and its infra adapter, dependency wiring, encrypted persistence, and tests are pending.
- Revised the rebuild interface decision: `RebuildSpaceUseCase` must load its own pending target through the checkpoint port rather than receive `Option<SpaceId>` from callers.
- Updated `crates/uc-core/AGENTS.md`: Rust code comments and doc comments in `uc-core` now use Chinese, while identifiers and commit messages remain English. The architecture maintenance record was updated; no runtime behavior changed.
- Added Chinese domain-contract docstrings for `SpaceRebuildDataTransitionPort::{prepare, stage, promote, finalize}`. The automated diagnostic cascade reported only a stale-snapshot advisory; no fresh compile or test has run.
- `RebuildSpaceUseCase` now owns the checkpoint and new data-transition ports. The draft flow resolves local prerequisites, loads or saves the pending target, and invokes `prepare`; `stage` and all subsequent rebuild phases are pending.
- Replaced the former `AdoptIsolatedSpacePort` trait with `RebindSpaceSessionPort` in the core space-access contract, the infra adapter, and the application dependency bundle. The bundle field is still named `adopt_isolated_space`, and the method still returns `ActiveSpace`; those follow-up consistency changes are pending.
- Added `SpaceRebuildAdmissionStatePort` and wired its call into the rebuild draft after session rebinding. No infra adapter implementation or dependency wiring exists yet. The draft has known compile issues: `pub(crate) impl` is invalid, and `to_stirng()` is misspelled.

## Verification

- Not run. The refactor is in an intentionally incomplete intermediate state.

## Errors

- Attempted to read a project-local `planning-with-files` skill path; it did not exist. The configured global skill path was then loaded successfully.
