# Space Rebuild Refactor Findings

## Existing Workflow

The current `DeviceManagementResetUseCase` is defined in `crates/uc-application/src/facade/space_setup/facade.rs`.

It currently combines two distinct actions:

1. User-requested reset through `SpaceFacade::reset()`.
2. Upgrade-triggered legacy profile isolation after successful session resume or unlock.

The user reset entry point cancels pending invitations, then invokes `execute_user_requested()`.

## Shared Rebuild Sequence

The reusable rebuild sequence is currently implemented by `execute_reset(pending_isolation_target)`:

1. Resolve a non-empty local device name.
2. Ensure the local identity fingerprint.
3. Reuse the persisted target or mint and persist a new target `SpaceId`.
4. Prepare and stage reset data.
5. Adopt the isolated space.
6. Reset admission, relationships, and remote members.
7. Persist the local member and initialize single-device membership.
8. Promote transition data.
9. Clear old security state and finalize transition data.
10. Persist completed setup state and clear the pending target.

## Rebuild Target Checkpoint

`set_device_management_reset_target` is a durable checkpoint, not the reset operation itself. It records the selected target `SpaceId` before irreversible work. A retry must reuse the target after interruption.

The current core port places this progress state on `SetupStatusPort` and uses historical device-management terminology. The planned replacement is `SpaceRebuildCheckpointPort`, with semantic operations to load, save, and clear the pending rebuild target. The user created its core definition in `crates/uc-core/src/ports/space/rebuild.rs`.

The current infra implementation persists the target in a separate file. The repository security rule requires new persisted business payloads to be encrypted by default. The new port implementation must not introduce plaintext persistence for the target.

## Rebuild Method Boundaries

The `RebuildSpaceError` categories define five workflow phases: preparation, staging, rebuild, commit, and finalization. `RebuildSpaceUseCase` should preserve them as separate methods: `prepare` resolves inputs and establishes the target, `stage` only stages target data, `rebuild` rebinds the session and reconstructs local membership, `commit` promotes target data, and `finalize` clears obsolete security/progress state and writes final setup state. The top-level `execute` must use `?` for methods that already return `RebuildSpaceError`; wrapping them again loses their phase-specific error category.

## Member State During Rebuild

Reuse `MemberRepositoryPort` directly. `RebuildSpaceUseCase` owns the application rule to retain the current device and remove every other `SpaceMember`; it should express the loop in a private `remove_remote_members` helper. Do not add a one-off bulk-clear port, because the repository already supplies the required list, remove, and upsert capabilities.

## Relationship State During Rebuild

Reuse the existing `RelationshipStateResetPort::clear_all_relationships` for space rebuild. Its `EncryptedRelationshipStore` adapter deletes the complete encrypted relationship state, and factory reset also uses the same capability. Do not add a rebuild-specific relationship port or fold it into `SpaceRebuildDataTransitionPort`; the relationship store has its own state owner and the cleanup operation has independent reuse.

## Workspace Convergence During Rebuild

`WorkspaceConvergence::reset_admission_for_device_management()` resets durable admission-attempt state through its owned `AdmissionAttemptRepositoryPort`. It is required because a freshly rebuilt single-device space cannot retain an incomplete or completed admission attempt from the previous space. This is distinct from relationship cleanup and member-table rebuilding. `RebuildSpaceUseCase` should call the workspace owner instead of accessing the admission repository directly; the later `initialize_new_space_membership()` call establishes the new single-device baseline.

## Current Files

- `crates/uc-application/src/space/lifecycle/rebuild_space.rs`: user-created draft for the shared rebuild workflow.
- `crates/uc-application/src/space/lifecycle/errors.rs`: user-created `RebuildSpaceError` draft.
- `crates/uc-application/src/space/lifecycle/reset_space.rs`: user-created reset use-case draft.

## Interface Decision

`RebuildSpaceUseCase` owns the checkpoint dependency. Its final `execute` method should load the pending target from `SpaceRebuildCheckpointPort` itself rather than accepting `Option<SpaceId>` from its caller. This prevents callers from knowing checkpoint persistence and gives the rebuild workflow a single recovery-state source.

## Known Intermediate Errors

- `reset_space.rs` currently places an `async fn` inside a struct definition. Rust methods must live in an `impl ResetSpaceUseCase` block.
- `rebuild_space.rs` currently stops after minting a `SpaceId`; it has no progress port yet and has an unfinished match expression and `todo!()`.
- Compilation and tests have not been run because these draft modules are incomplete.
