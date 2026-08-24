# Workspace Convergence Refactor Findings

## Approved Ownership

```text
SpaceFacade
|- SpaceAdmission
|- WorkspaceMembership
|- SpaceConnectivity
`- SpaceLifecycle
```

- `SpaceAdmission` owns invitation, handshake, join attempts, durable recovery,
  and the final admission result.
- `WorkspaceMembership` owns membership history, admission of verified members,
  removal, user decisions, reconciliation, and current-member scope.
- `SpaceConnectivity` owns connections and network recovery for the current
  effective member scope.
- `SpaceLifecycle` owns create, unlock, rebuild, reset, and Engine-version
  migrations.
- `SpaceFacade` only composes and forwards complete actions.

## Current Architecture Problem

ADR-021 successfully grouped implementation files, but retained one broad
`WorkspaceConvergence` type as the owner of admission, membership, projections,
connectivity, recovery, and lifecycle hooks. The interface and dependency set
remain close to the full implementation complexity, so callers and tests still
need knowledge from several business areas.

## Existing State and Assembly

- `WorkspaceConvergenceDeps` currently combines convergence state storage,
  profile-level admission attempts, admission security transitions, membership
  history exchange, member persistence, trusted peers, peer addresses,
  presence, security updates, group bootstrap, and legacy migration recovery.
- `SpaceConvergenceAssembly` constructs the broad owner and casts it to history,
  recovery, current-scope, content-gate, and lifecycle recovery interfaces.
- `space/admission/adapter.rs` exposes many protocol-step methods by forwarding
  them to `WorkspaceConvergence`; this is a shallow interface that leaks the
  admission state machine.

## Lifecycle Migration State

- Draft modules exist at `space/rebuild_space`, `space/reset_space`, and
  `space/upgrade_space` and contain user-written changes.
- Rebuild currently depends on `SpaceRebuildAdmissionStatePort` to clear state
  inherited from the previous Space.
- `AdmissionAttemptRepositoryPort::reset_for_device_management` is misleading:
  the store includes join attempts plus membership history, consumed
  invitations, recovery challenges, and trust revision. The operation clears
  prior-Space admission and membership-history state.
- The rebuild owner should request the result "clear prior-Space admission
  state". A private adapter should translate that to the existing storage.
  `WorkspaceConvergence` must not implement this lifecycle-facing interface.

## Verification Baseline

- The worktree is dirty and includes incomplete moves outside the new slice.
- Previous checks found unrelated formatting and trailing-whitespace blockers;
  all checks must be rerun against the current files before relying on them.

## Inventory To Complete

- Public and crate-restricted methods and their callers.
- Shared state fields, locks, repositories, and event publishers.
- Network endpoint and runtime entry points.
- Restart and retry entry points for admission, membership effects, and network
  recovery.
- Focused tests that prove each moved behavior before deleting its old path.

## Completed Responsibility Inventory

### SpaceAdmission

- Owns `DurableAdmissionTransaction` and all invitation-to-terminal admission
  stages currently in `convergence/admission/{transaction,flow,completion_recovery}`.
- Owns profile-level admission attempts, consumed invitations, completion
  recovery challenges, admission outbox delivery, and admission transition
  recovery.
- The current `WorkspaceAdmissionOwnerPort` exposes individual protocol stages;
  it must become an internal implementation detail of the single admission
  action, not a facade-facing orchestration surface.

### WorkspaceMembership

- Owns `WorkspaceConvergenceState`, its encrypted repository, state lock,
  decision lock, per-peer reconciliation locks, wake notification, and snapshot
  events.
- Owns member history exchange, verified membership effects, removal, device
  trust decisions, bootstrap, legacy membership repair, current peer scope, and
  content-exchange gating.
- Restart work includes pending membership effects, pending decisions, legacy
  marker repair, and membership history synchronization.

### SpaceConnectivity

- Owns reachability refresh, current-member connection maintenance,
  authenticated discovery runtime, and `NetworkRecoveryFacade`.
- It consumes `CurrentWorkspacePeerScopePort`; it must not infer membership
  from addresses, presence, or transport success.
- Runtime pause, resume, periodic retry, and shutdown belong here rather than
  on a membership state owner.

### SpaceLifecycle

- Owns initialize, unlock, rebuild, user reset, factory reset, and Engine
  version transitions.
- The rebuild transaction owns its persisted target and ordered prepare,
  stage, rebuild, promote, security cleanup, setup-status update, and finalize
  behavior.
- Prior-Space admission cleanup is a lifecycle result requested through a
  narrow internal interface; its adapter owns the low-level admission store.

### Current Callers and Wiring

- `SpaceConvergenceAssembly` currently constructs one broad owner and casts it
  to membership history, completion recovery, content gate, peer scope, and
  lifecycle recovery interfaces.
- `SpaceFacade` directly obtains the broad owner for initialization, invitation
  gating, inbound and outbound admission orchestration, legacy reset recovery,
  and peer scope.
- `space/runtime.rs` and `convergence/runtime.rs` jointly start recovery,
  membership synchronization, and connectivity work, so their ownership must
  be split when the new owners are wired.

## Migration Invariants

- Keep encrypted repository formats and network messages unchanged.
- Preserve lock scope: storage decisions happen under the existing state lock;
  bounded network calls remain outside it; re-entry validates persisted state.
- Preserve retry identity: admission attempt IDs, rebuild target Space IDs, and
  saved membership event IDs are reused after interruption.
- Move endpoint implementations with their business owner and remove the old
  implementation in the same slice.

## Unlock Ownership

- `UnlockSpacePort` is consumed only by `UnlockSpaceUseCase`, so its seam now
  belongs to `space/unlock_space` rather than `uc-core`.
- `uc-core` retains the aggregate space-access store used by infrastructure;
  the Engine composition root adapts that store to the application-owned
  unlock capability without introducing an infrastructure-to-application
  dependency cycle.
- The unlock use case now owns interactive unlock, Engine-version transition,
  mobile-consumable backfill, membership-storage validation, and best-effort
  presence priming. `SpaceFacade` converts the public input and result only.
