# Progress

## 2026-08-28
- Confirmed a clean worktree at `a5c8c28`.
- Defined the long invitation as canonical and the short code as its discovery alias.
- Split the work into four independently verifiable phases.
- Traced issuance from Infra through Application, Engine, and bindings.
- Confirmed cloud and LAN can carry the same full invitation as opaque payload without changing the rendezvous HTTP schema.

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

## Verification

| Check | Result |
|---|---|
| Pairing session tests | 18 passed |
| Application invitation tests | 20 passed |
| Core/Application/Infra all-target check | passed |
| Architecture preflight | passed |
| Core invitation tests | 11 passed |
| Application full lib tests | 687 passed |
| Infra all-target tests | 792 passed, 4 ignored |
| Metadata | passed |
| Final scoped all-target check | passed |
| Final architecture preflight | passed |
| Diff check | passed |
| Workspace check | existing Engine assembly failures remain (27 lib, 29 test) |
| Workspace format check | existing differences remain in two unrelated files |
| mDNS full-invitation byte equivalence | passed, 1 test |

## Errors

| Error | Resolution |
|---|---|
| Engine public-contract test cannot reach its target because the existing Engine assembly has 27 unrelated compile errors | Keep the contract test, verify lower layers directly, and record the Engine baseline separately |
| `InvitationId` does not implement `Hash` | Used its existing ordering capability with a `BTreeMap` instead of broadening the domain type |
| Architecture guard initially required `full_invitation.rs` under Core persistence | Moved the requirement to the Infra admission ownership check; preflight passed |
| Strict review found direct long invitations could connect without an id-based consume path | Added atomic consume-by-id and a test proving the short-code slot disappears too |
| Strict review found UniFFI setup invitation Debug would expose the new long invitation | Replaced derived Debug with explicit redaction and added an architecture guard |
