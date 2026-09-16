# Inbound Admission State Interface

## Goal

Complete the case-owned inbound admission state interface so one consistent read supplies protocol context and an opaque one-shot commit token while membership persistence remains hidden.

## Completion Criteria

- [ ] The case depends on one inbound admission state interface, not `MembershipLedger`.
- [ ] One read returns the current join record, signed history, required invitation generation, and an opaque commit token.
- [ ] The commit token is not cloneable, printable, serializable, or inspectable by the case.
- [ ] Admission-facing inputs and errors do not expose ledger types.
- [ ] Stale commits write nothing and return `StateChanged`.
- [ ] Missing or invalid membership state returns the admission-facing recovery error.
- [ ] Focused tests, the application suite, formatting, architecture checks, and diff checks are verified.

## Phases

### Phase 1: Test-first interface contract

- Add or adapt focused tests for the case-owned read result, opaque commit flow, and stale-state rejection.
- Observe the expected compile/test failures against the partial implementation.
- **Status:** complete

### Phase 2: Minimal implementation

- Complete the admission-owned types and the single state interface.
- Implement read and accept in `MembershipLedger`.
- Remove concrete ledger knowledge and stale code from the case and assembly.
- Update the architecture maintenance record.
- **Status:** complete

### Phase 3: Verification

- Run focused and full application tests.
- Run required repository checks and record unrelated failures separately.
- **Status:** in_progress

## Decisions

- Keep invitation validation, protocol preparation, reply handling, invitation consumption, and maintenance wake-up in the admission case.
- Keep verified loading, concurrency-token creation, record-version advancement, and atomic persistence in `MembershipLedger`.
- Use one state interface with `load` and `accept`; do not inject the same ledger through multiple interfaces.

## Errors Encountered

| Error | Attempt | Resolution |
| --- | ---: | --- |
| Partial implementation has 12 compile errors | 1 | Treat as the failing baseline; complete the interface test-first rather than patching old variables individually. |
| Final-interface tests fail with missing token, context methods, admission-owned prepared input, reader implementation, and five-argument constructor | 1 | Expected red state confirmed; proceed with the minimal interface implementation. |
