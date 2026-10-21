# Suspend releases Engine-owned files

Thread: uni-t-0352. Upstream diagnosis: uni-t-0351 (Mobile, iOS `0xdead10cc`).
Base: `a461b894` (origin/main).

## Goal

After a successful Engine suspend, the process holds no Engine-owned file lock and no
open SQLite connection, and resume reopens them. Scope is Engine only.

## Owners and contract

- Session-level resources (profile/control SQLite pools): Application `RuntimeLifecycle`
  gets a fourth participant `local_storage`. Suspend: after `local_resources`.
  Resume: first. Infra-storage implements the close/reopen.
- Process-level network log: `uc-observability-runtime` owns park/unpark, keyed by a
  process count of active Engines. The only trigger is the Engine lifecycle transition,
  before `Suspended` is published. Hosts never call park.
- Suspend success = no Engine-owned lock or SQLite connection remains. Excluded and owned by
  Mobile: `p2p/.runtime.lock`, JS `uniclipboard.db`.

## Phases

1. [x] E2E written first and run red on base.
2. [x] E2: DbPool suspend/resume + lifecycle participant.
3. [x] E1: network-log park/unpark + Engine activity count.
4. [x] E4: open lock-capable file census in the suspend record.
5. [~] E3: identified (iroh QAD probe drain); bounding waits for a user decision.
6. [x] Docs, checks, local commit, report.

## Failure modes (isolated DbPool test, written before code)

1. A connection checked out during suspend is closed under its user.
2. Suspend reports success while a checked-out connection still holds the files.
3. The deadline ends the first call; the continuation call does not wait on the same
   detached pool and reports success early.
4. A `:memory:` (ephemeral) pool is dropped on suspend and loses its data.
5. Resume reopens the old URL after `replace_database` changed it.
6. Suspend/resume bumps `generation`, so readers think the database was replaced.
7. `get()` during suspension silently opens a new connection.
8. Resume reruns migrations or triggers.
9. Profile and control share one instance; the second suspend or resume fails.
10. r2d2 worker threads reopen a connection after suspend.
