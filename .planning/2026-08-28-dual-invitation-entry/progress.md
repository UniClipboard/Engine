# Progress

## 2026-08-28
- Confirmed a clean worktree at `a5c8c28`.
- Defined the long invitation as canonical and the short code as its discovery alias.
- Split the work into four independently verifiable phases.
- Traced issuance from Infra through Application, Engine, and bindings.
- Confirmed cloud and LAN can carry the same full invitation as opaque payload without changing the rendezvous HTTP schema.

## 2026-08-29
- Recorded the confirmed at-most-once cloud short-code rule.
- Revised Phase 3 so an in-flight or ambiguous short-code lookup never retries after restart.
- Added the Core invitation-resolution lifecycle and encrypted persistence.
- Added Application start/recovery ownership and production Infra preparation/resolution adapters.
- Began paired Phase 4. The developer delegated the OPAQUE tests to Codex; the agreed first seam is an Infra public capability covering registration and a complete three-message authentication exchange.
- Added the first Phase 4 tracer-bullet test for matching-passphrase registration and KE1/KE2/KE3. It deliberately targets the not-yet-existing Infra capability and must be recorded as RED before production implementation.
- Added the developer-started `SpaceAdmissionAuthContext` module skeleton, completed its constructor and public capability export, and kept all `opaque-ke` types below Infra. The RED test now reaches the four missing behavior methods.
- Fixed the OPAQUE ciphersuite to Ristretto255, TripleDH/SHA-512, and Argon2; wrapped the third-party server setup in a non-Debug Infra type and generated it with the crate-compatible OS RNG. The RED test now advances past setup generation to registration.
- Completed the first Phase 4 OPAQUE tracer bullet: registration and KE1/KE2/KE3 now derive the same context-bound, zeroizing continuation credential for the matching passphrase.
- Added minimal repository-rule coverage showing an authentication failure retains its stable classification and a non-empty source chain.
- Corrected Spec 028 to RFC 9807 KSF salt semantics: the OPRF key is the secret salt and no extra Argon2 salt is persisted.
- Validated the next OPAQUE slice at the established Infra seam: changing the admission id, invitation id, Joiner peer id, or Sponsor peer id independently causes authentication failure with its source chain preserved. No production adjustment was needed because the prior transcript binding was already complete.
- Added a versioned OPAQUE registration encoding explicitly named for the encryption/decryption boundary. Its temporary bytes zeroize on drop, restored records authenticate normally, and truncated encodings fail with stable Registration classification and source.
- Added same-length tamper evidence: a structurally decodable registration with a modified OPAQUE envelope cannot authenticate and retains the Authentication source chain.
- Covered the registration envelope guards for a wrong marker, unsupported version, and trailing bytes; each fails with Registration classification and a non-empty source.
- Added versioned, zeroizing `ServerSetup` encryption-bound encoding. Restoring the original setup authenticates an existing registration; a replacement setup or truncated encoding fails closed.

## TDD Evidence

| Stage | Test | Result |
|---|---|---|
| RED | `invitation_entries_are_bounded_and_redacted` | failed because the full invitation type and redacted short-code Debug did not exist |
| GREEN | `invitation_entries_are_bounded_and_redacted` | passed, 1 test |
| RED | `full_invitation_round_trips_identity_route_and_expiry` | failed because the full invitation codec did not exist |
| GREEN | `full_invitation_round_trips_identity_route_and_expiry` | passed, 1 test |
| RED | full invitation rejection tests | failed because route, expiry, and version rejection did not exist |
| GREEN | full invitation rejection tests | passed, 2 tests |
| RED | `issue_invitation_happy_path` | failed because the production issuer did not return invitation id or full invitation |
| GREEN | `issue_invitation_happy_path` | passed after real Sponsor issuance generated both fields |
| RED | Application invitation issue happy path | failed because Application fixtures and aggregate lacked the new identity and full invitation |
| GREEN | Application invitation issue happy path | passed and preserved both fields in the parked invitation |
| RED (masked) | Engine public invitation contract | the new assertion was added, but the known Engine assembly baseline fails before integration-test compilation |
| RED | Sponsor holder invitation-id lookup | failed because the holder had only a short-code lookup |
| GREEN | Sponsor holder invitation-id lookup | passed with one invitation stored behind code and id indexes |
| RED | short/full invitation dial equivalence | failed because dial results lacked invitation identity and no direct source existed |
| GREEN | short/full invitation dial equivalence | passed; both forms reached the same Sponsor and invitation id |
| RED | cloud publish payload | failed because rendezvous still received the raw endpoint ticket |
| GREEN | cloud publish payload | passed after cloud and mDNS publish the same full invitation |
| RED | setup-state invitation query | failed because the query returned only the short code |
| GREEN | setup-state invitation query | passed and returns the same full invitation from the Sponsor holder |
| RED | directory expiry consistency | failed because a short-code alias could expire before its full invitation |
| GREEN | directory expiry consistency | passed; issuance rejects an earlier directory expiry |
| RED | consume by invitation id | failed because only short-code consumption existed |
| GREEN | consume by invitation id | passed and removes the short-code slot under the same lock |
| RED | Core at-most-once invitation resolution persistence | failed because Ready, Started, Resolved, and opaque start context did not exist |
| GREEN | Core at-most-once invitation resolution persistence | passed; Started persistence contains no short code and ambiguous Started can only reject |
| RED | Application short-code start | failed because start always required complete J0 material |
| GREEN | Application short-code start | passed; Ready is committed and Pending returned before material generation or network |
| RED | Application one-shot resolution recovery | failed because no resolver ownership or resolution states existed in recovery |
| GREEN | Application one-shot resolution recovery | passed; Started commits before resolve and Resolved commits after the response |
| RED | Restart from Started | failed because recovery had no fail-closed path for a possibly consumed code |
| GREEN | Restart from Started | passed; resolver is not called and the attempt becomes a stable rejection |
| RED | Production invitation preparation | failed because Infra could not classify full and short invitation entries |
| GREEN | Production invitation preparation | passed; full stays local and short produces opaque zeroizing context |
| RED | Pre-connection cancellation | failed with `UnsafeCancellation` from invitation-resolution states |
| GREEN | Pre-connection cancellation | passed; Ready, Started, and Resolved remain locally cancellable |
| RED | Ambiguous resolver failure | failed because the test path still used complete start material |
| GREEN | Ambiguous resolver failure | passed; one resolver error commits stable rejection and never retries the code |

| RED | `matching_passphrase_establishes_the_same_bound_continuation_credential` | compile failed only because `SpaceAdmissionAuth` and `SpaceAdmissionAuthContext` do not yet exist |
| GREEN | `matching_passphrase_establishes_the_same_bound_continuation_credential` | passed; both peers derived the same context-bound continuation credential |
| GREEN | `authentication_failure_preserves_classification_and_source` | passed; authentication failure retained stable classification and a non-empty source chain |
| GREEN (existing behavior) | `mismatched_admission_identity_context_cannot_authenticate_the_exchange` | passed for independent admission, invitation, Joiner peer, and Sponsor peer mismatches; the prior context binding already enforced the requirement |
| RED | `restored_registration_authenticates_and_truncated_encoding_is_rejected` | compile failed because registration had no encryption-bound encoding or restoration capability |
| GREEN | `restored_registration_authenticates_and_truncated_encoding_is_rejected` | passed; restored registration authenticated and truncated bytes were rejected with a source chain |
| GREEN | `tampered_registration_record_cannot_authenticate` | passed; a same-length modified OPAQUE record reached protocol verification but could not authenticate |
| GREEN | `registration_encoding_rejects_wrong_marker_version_and_length` | passed for marker, version, and trailing-byte corruption |
| RED | `restored_server_setup_authenticates_existing_registration` | compile failed because ServerSetup had no encryption-bound encoding or restoration capability |
| GREEN | `restored_server_setup_authenticates_existing_registration` | passed; the restored setup authenticated the existing registration |
| GREEN | `changed_or_invalid_server_setup_cannot_resume_registration` | passed; replacement setup authentication and truncated setup restoration were rejected with sources |

## Verification

| Check | Result |
|---|---|
| Pairing session tests | 18 passed |
| Application invitation tests | 20 passed |
| Core/Application/Infra all-target check | passed |
| Architecture preflight | passed |
| Core invitation tests | 11 passed |
| Application full lib tests | 687 passed |
| Infra all-target tests | 794 passed, 4 ignored |
| Metadata | passed |
| Final scoped all-target check | passed |
| Final architecture preflight | passed |
| Diff check | passed |
| Workspace check | existing Engine assembly failures remain (27 lib, 29 test) |
| Workspace format check | existing differences remain in two unrelated files |
| mDNS full-invitation byte equivalence | passed, 1 test |
| Focused Phase 3 short-code tests | 4 passed |
| SQLite Started boundary and plaintext probe | passed, 1 test |
| Phase 3 Core state tests | 75 passed |
| Phase 3 Core persistence tests | 11 passed |
| Phase 3 Application full lib tests | 691 passed |
| Phase 3 scoped all-target check | passed |
| Phase 3 architecture preflight | passed |
| Phase 3 workspace check | existing Engine assembly failures remain (27 lib, 29 test) |
| Phase 4 OPAQUE focused tests | 2 passed |
| Phase 4 Infra all-target check | passed |
| Phase 4 metadata | passed |
| Phase 4 architecture preflight | passed |
| Phase 4 diff check | passed |
| Phase 4 workspace check | existing Engine assembly failures remain (27 lib, 29 test) |
| Phase 4 workspace format check | existing differences remain in two unrelated files |
| `opaque-ke` advisory/license review | no package advisory in current RustSec database; crate declares `Apache-2.0 OR MIT`; local `cargo-audit` and `cargo-deny` commands unavailable |
| Phase 4 OPAQUE identity-binding tests | 3 focused tests passed; mismatch test covers 4 identity fields |
| Phase 4 OPAQUE registration encoding tests | 6 focused tests passed |
| Phase 4 OPAQUE restart tests | 8 focused tests passed |
| Phase 4 RFC 9807 vector | passed, 1 test plus compile-time secret constraints |
| Phase 4 OpenMLS admission transition | passed, 1 focused test |
| Phase 4 Joiner start material | passed, 2 Infra tests and Application source-chain test |
| Phase 4 final Application tests | passed, 692 tests |
| Phase 4 final Infra all-target check | passed |
| Phase 4 final metadata / architecture / diff checks | passed |
| Phase 4 final workspace check | existing Engine assembly failures remain (27 lib, 29 test) |
| Phase 4 final workspace format check | existing differences remain in two unrelated files |
| Phase 5 JoinRequest identity-facts red/green | Core mismatch test failed before implementation, then passed with persistence and scoped crate checks |
| Phase 5 Candidate event two-stage binding | passed, focused Core test |
| Phase 5 production Sponsor Candidate adapter | passed, focused end-to-end adapter test and Infra all-target check |
| Phase 5 production Joiner Candidate adapter | passed, real OpenMLS Candidate verification and Prepared-reply test; staged MLS and recovery artifacts zeroize on drop |
| Phase 5 bounded Iroh admission transport | passed, canonical envelope round-trip, pre-allocation frame rejection matrix, continuation MAC binding test, Infra all-target check |
| Phase 5 real Iroh admission loopback | passed, two relay-disabled endpoints completed OPAQUE JoinRequest/Candidate then a fresh continuation-authenticated Prepared/Commit connection; endpoint call count was exactly two |
| Phase 5 encrypted membership ledger | passed, real SQLite stores only profile-AEAD ciphertext, reloads the exact snapshot after reopen, and rejects a stale revision/history CAS; committed as `1e89f8d` |
| Phase 5 OPAQUE credential lifecycle | passed, Initialize/Unlock ensure one encrypted registration, reopen completes a real KE1/KE2/KE3 exchange, plaintext passphrase is absent, and dependency failures retain their source chain |
| Phase 5 OPAQUE Space-generation binding | passed, the full active manifest binds the encrypted registration; a generation change rejects the old record until lifecycle replacement, then a second real OPAQUE exchange succeeds |

## Errors

| Error | Resolution |
|---|---|
| Engine public-contract test cannot reach its target because the existing Engine assembly has 27 unrelated compile errors | Keep the contract test, verify lower layers directly, and record the Engine baseline separately |
| `InvitationId` does not implement `Hash` | Used its existing ordering capability with a `BTreeMap` instead of broadening the domain type |
| Architecture guard initially required `full_invitation.rs` under Core persistence | Moved the requirement to the Infra admission ownership check; preflight passed |
| Strict review found direct long invitations could connect without an id-based consume path | Added atomic consume-by-id and a test proving the short-code slot disappears too |
| Strict review found UniFFI setup invitation Debug would expose the new long invitation | Replaced derived Debug with explicit redaction and added an architecture guard |
| Cargo accepts only one positional test filter | Re-ran the two recovery cases with the shared `short_code` filter; 4 matching tests passed |
| Architecture maintenance-record patch anchor did not match | Retried against the stable related-documents heading; the failed patch made no partial changes |
| Workspace formatting changed two unrelated files while formatting the new module | Reverted only those formatter changes; the existing workspace format baseline remains separately recorded |
| Workspace check remains blocked by existing Engine assembly removals | Verified this slice with the focused tests and `uc-infra --all-targets`; retained the 27 lib / 29 test baseline as failed |
| Engine wiring audit exposed missing production ports beyond Candidate | The new `SpaceApplicationDeps` also requires Commit, Complete, settlement, Joiner activation, membership-effect, observation, cleanup, and activity capabilities. Do not conceal these with restored aliases or no-op adapters; complete the production vertical flow before final assembly. |
