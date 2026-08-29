# Dual Invitation Entry

## Goal
Support both a self-contained long invitation and a human-entered short code, with both resolving to one Sponsor-issued invitation identity and one route before the new admission flow consumes them.

## Completion Criteria
- Sponsor creates a random 256-bit `InvitationId` for every invitation.
- Sponsor produces one versioned full invitation containing the id, opaque Sponsor route, and expiry.
- The existing short code is only an alias for that same full invitation through cloud or LAN discovery.
- Full invitations decode locally; short codes resolve through existing discovery channels.
- Both paths return the same typed resolved invitation and reject malformed, expired, or mismatched data.
- Invitation text and routes remain redacted from Debug and logs.
- Existing public short-code behavior continues while a long-invitation result becomes available to QR/link callers.
- Each implementation phase is test-first, documented, checked, reviewed, and committed independently.

## Phases

### Phase 1: Canonical invitation and Sponsor issuance
- [x] Trace the current invitation result through Core, Application, Engine, and bindings
- [x] Add failing tests for one identity shared by short and full invitation forms
- [x] Implement versioned full invitation encoding and Sponsor generation
- [x] Return both forms from the existing invitation result
- [x] Keep the existing short-code dial path behavior unchanged in this phase
- [x] Update docs, verify, and review
- [x] Commit the phase
- **Status:** complete

### Phase 2: Unified resolution
- [x] Add one resolved invitation result used by both entry forms
- [x] Full invitation resolves locally without network I/O
- [x] Short code cloud/LAN records return the exact same full invitation
- [x] Reject expiry, corruption, identity mismatch, and route mismatch
- **Status:** complete

### Phase 3: Durable unresolved short-code state
- [x] Add pre-network Joiner states for a saved short code and a saved full invitation
- [x] Persist the short code before discovery, then atomically mark its single resolution attempt started
- [x] Never retry a short code after the resolution request may have consumed it
- [x] Save the returned full invitation before any Sponsor connection
- [x] Treat timeout, lost response, save failure, or restart from in-flight resolution as requiring a new invitation
- [x] Full invitations bypass discovery and continue through the locally validated start path
- **Status:** complete

### Phase 4: Standard authentication and Joiner start material
- [ ] Integrate fixed-version OPAQUE per Spec 028 and validate it
  - [x] Correct/wrong passphrase, identity binding, registration restoration, and corrupt-record rejection
  - [x] ServerSetup restoration and mismatched-setup rejection
  - [x] RFC 9807 vector evidence and secret/debug lifecycle checks
- [ ] Validate existing OpenMLS Add/Commit/Welcome, staged restore, and public commitment at the admission seam
- [ ] Generate complete JoinRequest identity, OpenMLS, recovery, and password material
- [ ] Implement production `JoinerStartMaterialPort`
- [ ] Keep Candidate, transport, and Engine final wiring outside this phase
- **Status:** in_progress

**Next Step:** Validate the existing OpenMLS admission transition seam and its restart-safe staged state.

## Decisions
- A short code and a full invitation are two entry forms for one invitation, not two protocols.
- The full invitation is the canonical fact; discovery channels store and return it as opaque data.
- The full invitation contains no passphrase or private key material.
- Existing rendezvous and mDNS transports remain indexes; they do not become admission state owners.
- No id is derived from the 40-bit short code.
- Cloud short-code lookup is at-most-once: a successful lookup consumes the alias even when pairing never completes.
- The only durable and retryable result of short-code lookup is the full invitation saved before dialing.
- An ambiguous lookup outcome fails closed and asks for a newly issued invitation; it never reuses the short code.
- The first Phase 4 TDD seam is the public Infra `space_admission_auth` capability; tests cover registration plus a complete KE1/KE2/KE3 exchange without exposing `opaque-ke` types above Infra.
- The developer delegated the current Phase 4 OPAQUE slices to Codex; each remains independently test-first, reviewed, documented, and committed.

## Errors Encountered

| Error | Resolution |
|---|---|
| Architecture maintenance-record patch anchor did not match | Retried against the stable `## 相关文档` heading; no partial file changes occurred. |
| Initial OPAQUE implementation patch no longer matched the paired skeleton | Re-read the complete module and applied the implementation against its current structure. |
| Argon2 and HKDF dependency error types lacked `std::error::Error` support | Enabled Argon2 `std`; wrapped HKDF's preserved fixed-length error value in a concrete internal error before adding safe context. |
| Initial RFC vector RNG repeated bytes into the whole destination and produced the wrong registration request | Matched the pinned library's official `CycleRng`: copy at most the source length, leave the remaining initialized bytes unchanged, then rotate the source. |
| Downstream RFC KE1 generation differed even with the official RNG bytes | Confirmed the pinned crate gates direct deterministic blind injection behind its own `cfg(test)`; downstream validation now computes the official registration request and strictly round-trips the official registration upload and KE1/KE2/KE3 frames, while production Argon2 tests retain end-to-end coverage. |
