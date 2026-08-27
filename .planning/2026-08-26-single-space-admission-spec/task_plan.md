# Single Space Admission Protocol Spec

## Goal

Research and write an implementation-ready specification for one new Space admission protocol with no legacy protocol, compatibility adapter, fallback, or dual implementation.

## Completion Criteria

- [x] Current broken data flow and all partial/dead contracts are inventoried.
- [x] Product, security, persistence, retry, crash-recovery, and lifecycle invariants are reconciled from current authoritative docs.
- [x] Mature protocol/library patterns are researched from primary sources.
- [x] Core, Application, Infra, Engine, binding, persistence, and test ownership are explicit.
- [x] The new typed message protocol and state machine are complete and deterministic.
- [x] No compatibility path, old ALPN, old protocol object, fallback, or double implementation remains.
- [x] Implementation order, deletion list, edge cases, and executable acceptance matrix are specified.
- [x] `docs/specs/028-single-space-admission-protocol.md`, docs index, and architecture maintenance record pass documentation checks.

## Phases

### Phase 1: Repository and requirements research

- Trace current Core/Application/Infra/Engine admission flow, missing callers, partial ports, and compile failures.
- Read current authoritative ADR/spec/security documents and identify superseded material.
- **Status:** complete

### Phase 2: External pattern research and design decisions

- Verify mature cryptographic, MLS, transport, idempotency, and retry patterns using primary sources.
- Select one protocol, state machine, ownership model, persistence model, and assembly order.
- **Status:** complete

### Phase 3: Write implementation-ready spec

- Write the required 11-section spec with exact paths, interfaces, workflows, errors, deletion list, and test matrix.
- Update docs index and architecture maintenance record.
- **Status:** complete

### Phase 4: Verify the documentation artifact

- Check links, relative paths, required sections, forbidden compatibility language, architecture checks, formatting, and diff whitespace.
- **Status:** complete

## Decisions

- Historical code is evidence for product invariants only; no historical runtime structure or compatibility path will be restored.
- The spec covers the complete vertical slice across Core, Application, Infra, Engine, bindings/hosts where contracts change, and real two-device verification.
- Existing unrelated worktree changes must remain intact.

## Errors Encountered

| Error | Attempt | Resolution |
| --- | ---: | --- |
| Current Engine check reaches Infra and fails with 17 unfinished migration errors | 1 | Record as current architecture evidence; do not treat the partial branch as a valid baseline or restore removed ports. |
| Untracked-spec whitespace check could not resolve bare `mktemp` in the current shell | 1 | Re-run with `/usr/bin/mktemp`; no spec content change required. |
| zsh link-check loop used reserved special variable `path` and erased command lookup PATH | 1 | Rename loop variable to `doc_ref` and use absolute cleanup command; no spec content change required. |
