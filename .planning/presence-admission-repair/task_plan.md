# Presence admission repair

## Goal

Fix pairing completion and presence semantics so that a newly paired sponsor and joiner have durably applied the same admission state before success, and only remain online after authenticated admission succeeds.

## Completion criteria

- [x] A deterministic regression test reproduces pairing success followed by asymmetric admission.
- [x] A focused presence regression test reproduces optimistic Online followed by `peer_not_admitted` closure.
- [x] Both tests are observed failing for the intended reasons before production changes.
- [x] Pairing success implies both peers have durable, mutually admissible membership and protection state.
- [x] A rejected connection cannot leave either peer Online.
- [x] Unknown, removed, divergent, and unverifiable peers remain rejected.
- [x] Fresh bidirectional clipboard content is persisted/applied by each receiver before and after both engines restart.
- [x] Architecture bible records the behavioral/ownership change.
- [x] Focused tests and required repository checks pass; unexecuted device checks are reported as skipped.

## Phases

1. **Reproduce and minimize** - complete
2. **Rank and test hypotheses** - complete
3. **Add red regression tests** - complete
4. **Implement minimal repair** - complete
5. **Run focused and full verification** - complete
6. **Review diff and report** - complete

## Errors encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| Initial Cargo commands waited on a shared artifact lock and their final output was not retained | 1 | Waited for the owning compile to finish, then reran the exact scenario and captured its verdict |
| Short exact Cargo filter selected 0 presence tests | 1 | Listed tests, used the full module-qualified name, and confirmed 1 intended failure |
| Presence regression initially rejected the correct Offline event as any event | 1 | Tightened the assertion to reject only transient Online while allowing the final Offline notification |
| First explicit admission handshake omitted a request byte, so both sides waited | 1 | Added a one-byte request before the accept/reject response and covered accepted, rejected, unknown, repeated, and shutdown paths |
| Rejection acknowledgement raced with immediate connection close | 1 | Receiver waits after sending rejection; dialer closes after reading it, preserving reliable verdict and prompt cleanup |
