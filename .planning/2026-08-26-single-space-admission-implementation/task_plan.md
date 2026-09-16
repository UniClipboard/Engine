# Single Space Admission Protocol Implementation

## Goal

Implement Spec 028 as one clean protocol cutover with no old ALPN, compatibility path, fallback, dual write, or parallel implementation.

## Completion Criteria

- [ ] Core uses one typed admission protocol and stage-carrying aggregate.
- [ ] Application owns complete start/cancel/handle/recover/complete behavior and has no Iroh knowledge.
- [ ] Infra owns OPAQUE, OpenMLS capability adapters, Iroh connection/auth/wire, and encrypted persistence.
- [ ] Engine binds the typed endpoint before Router start and starts runtime afterward.
- [ ] Old protocol/session/event/outbox/store/preparation symbols and tables are deleted.
- [ ] Stable product operations/results remain and all automated/device acceptance gates are recorded accurately.

## Phases

### Phase 1: Core typed protocol and aggregate

- Complete identity value objects through focused red-green tests.
- Add typed envelope, body, evidence, role/stage aggregate, and legal transitions.
- Keep the public module entry small; split internal identity, artifacts, messages, exchanges, state, transitions, and tests by responsibility.
- Complete Candidate -> Prepared, Sponsor commit, Joiner apply, completion/settlement, cancellation/supersession, helper recovery, and terminal compaction.
- **Status:** completed

### Phase 2: OPAQUE and OpenMLS capability validation

- Pin and validate the selected dependency and RFC vectors.
- Establish exact staged security outputs and recovery fixtures.
- **Status:** pending

### Phase 3: Encrypted membership ledger store

- Implement the single application snapshot/conditional commit contract and real encrypted SQLite adapter.
- **Status:** pending

### Phase 4: Application protocol tracer bullet

- Implement J0 -> Candidate -> Prepared with typed authenticated messages and transport-agnostic ports.
- **Status:** implemented (static compilation passed; test execution skipped by user; production Infra adapters pending)

### Phase 5: Complete business protocol

- Implement Commit, Applied, Complete, settlement, cancellation, supersession, transition, and helper recovery.
- **Status:** pending

### Phase 6: New Iroh handler and transport

- Implement direct Infra handler/connector/auth/wire with the new ALPN.
- **Status:** pending

### Phase 7: Engine and binding cutover

- Reorder assembly, preserve stable product contracts, and remove unreachable synchronous paths.
- **Status:** pending

### Phase 8: Mandatory deletion and migration

- Delete all old symbols/tables and enforce the forbidden-symbol checks.
- **Status:** pending

### Phase 9: Full verification and review

- Run focused/full suites, real SQLite/Iroh/Engine E2E, repository checks, review, and physical-device matrix.
- **Status:** pending

## Decisions

- The user delegates technical implementation and verification to the agent; ask only for business behavior or irreversible product choices.
- Follow Spec 028 and TDD at pre-agreed seams.
- Application must not depend on Iroh/ALPN/connection/stream/frame types.
- Preserve unrelated dirty-worktree changes.

## Errors Encountered

| Error | Attempt | Resolution |
| --- | ---: | --- |
| Current zero-id test fails because `from_bytes` is infallible | 1 | Expected red state; make the constructor fallible and update valid fixtures. |
| Test module has an unused `postcard::from_bytes` import | 1 | Remove the accidental import in the same focused green change. |
| Rust 1.95 rejects array equality inside `const fn` because const `PartialEq` is unstable | 1 | Make the fallible constructor a normal `fn`; `as_bytes` remains const. |
| `cargo test` rejected two positional test filters | 1 | Use one module-level `space_admission::tests` filter. |
| Focused format check reported layout-only drift after adding sender rules | 1 | Applied rustfmt to the single touched Core file. |
| Whole-workspace compile reaches Infra references to deleted application ports | 1 | Treat as cutover inventory; replace through the new protocol phases instead of restoring removed compatibility interfaces. |
| `space_admission/mod.rs` remained a 2,830-line monolith after the directory move | 1 | Split the implementation by responsibility while preserving the existing public exports and behavior. |
| Whole-workspace format check reports unrelated existing drift | 1 | Verified every touched Core file independently; existing drift remains in Engine assembly and Iroh clipboard receiver files. |
| Strict whole-crate Clippy reports pre-existing warnings outside Space admission | 1 | Kept the scope unchanged; focused compile/tests and production-source scans cover the refactor. |
| Application tracer test cannot resolve `ProtocolEvent` and `SpaceAdmissionProtocolTestPair` | 1 | Expected red state: implement the real two-node protocol fixture from the new coordinator and in-memory ports; do not satisfy it through the legacy cases. |
| First full Application run missed one expected captured log under parallel execution | 1 | The unrelated log-capture test passed alone; the second full 667-test run also passed. |
