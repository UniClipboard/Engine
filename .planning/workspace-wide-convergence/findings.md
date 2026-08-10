# Findings and Decisions

## Requirements

- Read `docs/adr/016-workspace-wide-convergence.md`, research its implications, and write the resulting implementation specification in `docs/specs`.
- Follow the repository rule that any documentation change checks and updates `docs/architecture/architecture-bible.md`.
- Preserve the user's concurrent working-tree changes; this task changes documentation only.

## Research Findings

- ADR-016 requires one application-level owner for joining, removal, rejoining, transfer, confirmation, restart recovery, and published state.
- A valid recovery handoff must advance the receiver continuously from its own saved state; transport source and arrival order cannot decide correctness.
- ADR-012 already defines `MembershipConvergence` as the owner of device discovery and verified-peer promotion. ADR-015 defines durable member-removal intent, recovery, and receiver-confirmed completion.
- ADR-016 must replace their fragmented post-pairing/security-update paths with a single workspace-wide convergence workflow, rather than adding a parallel queue or product-side orchestration.
- The active working tree contains concurrent implementation changes in convergence, pairing, networking, bindings, and documentation. Do not modify those source files for this documentation task.
- The current stable surface exposes separate `QueryMembershipConvergence`, `RefreshSharedDevices`, `QuerySharedDeviceRefresh`, `RemoveMember`, and `QueryMemberRemoval` operations, plus separate events. Their state models do not provide the ADR-016 states `LocallyApplied` and `WaitingForOfflineMember` or one workspace-completion proof.
- Current in-progress code has a limited `WorkspaceRecoveryTransportPort` for contiguous relayed security updates and a persistent applied-security-update source. The specification must generalize this into a complete, encrypted, receiver-verified workspace-change chain, not retain it as an independent side route.
- Active tests already add three-, four-, and five-device scenarios. They are useful implementation targets, but documentation must not claim them passed merely because the tests exist in an uncommitted worktree.
- The repository does not yet establish a stable literal protocol name for the recovery channel. The specification must require a separate versioned restricted channel without documenting an invented production identifier.

## Technical Decisions

| Decision | Rationale |
| --- | --- |
| Specify one `WorkspaceConvergence` owner conceptually at the application layer | It creates a single external seam and keeps transfer, persistence, retry, and completion knowledge local. |
| Define stable input/result/state contracts rather than exposing transfer steps | Callers need to submit a membership operation, query a snapshot, and observe changes; they must not sequence verification or networking steps. |
| Treat completion as receiver-confirmed shared state | Sender writes, elapsed time, and a local state change do not prove workspace convergence. |
| Publish one full workspace snapshot and one change event | A lagging consumer can re-query facts without combining device-discovery, removal, and recovery events locally. |
| State explicitly that old public models are removed in the implementation phase | The repository disallows compatibility layers and ADR-016 rejects parallel paths. |

## Independent Review Findings To Resolve

- The core offline-member scenario must be: A and B are paired; A is offline; B admits C; B is offline; C meets the returning A. A verifies B's continuous change chain before creating the A/C relationship and beginning bidirectional sync.
- A recovery reply must authorize the requester as a currently active member instance, not merely accept a historical proof. A removed instance may submit its removal notice or historic intent but must receive no current identities, addresses, or security changes.
- The recovery payload needs an explicit application-layer encryption rule tied to a context both endpoints can establish. It cannot assume they share the current workspace key across security generations. The specification must require confidentiality, integrity, peer binding, workspace binding, freshness, and replay rejection.
- The unified workspace snapshot needs a separate `removal_intent_count` field; it is not interchangeable with the total workspace-change count.
- `JoinSpace` success means the joiner is locally prepared to receive changes. Global convergence begins only after the sponsor durably records the joining change after receiving readiness confirmation. Before that confirmation, the joiner cannot participate in normal content exchange. The acceptance matrix needs the sponsor-crashes-after-readiness case.
- ADR-016 must explicitly revise the responsibility and public-state portions of ADR-015. ADR-015 and the architecture overview must consistently say that `WorkspaceConvergence` owns the complete workflow, with member removal as one internal operation.
- A single bounded recovery offer cannot be mistaken for completion: each offer must identify its continuous range and whether more remains; only an acknowledgement of the current target digest clears the full pending handoff. The acceptance matrix must cover more than one batch, restart, and a changed handoff device.
- The workspace already depends on `chacha20poly1305`; no separate cryptographic package is necessary to specify the recovery envelope. The specification will require a purpose-separated historical transport key and fresh handoff binding to derive the existing AEAD key, with no current-key fallback.
- The OpenMLS validation test `new_member_can_relay_the_sponsors_commit_to_an_offline_existing_member` is the exact cryptographic topology needed for W01. It proves the secure-state prerequisite, not the application-level persistence, authorization, or content-delivery result.
- Specification 015 already uses only the unified public entrypoints, but its snapshot wording needs the literal `removal_intent_count` name to match specification 016.
- The architecture overview still calls a standalone removal coordinator the owner at its current member-security and removal sections, and still names its old query/event in the current notification description. Historical maintenance entries are records of earlier work and should remain untouched. The body needs a clearly marked pending ADR-016 target boundary, then the present-tense ownership/entry assertions must defer to it without claiming the future channel or interface already exists.

## Exact Draft Evidence

- ADR-016 lines 14-16 and acceptance item 101 currently describe A admitting C, then A returning. That does not prove that a lagging device learns a later admission through an unrelated current member; both must use B admitting C while A is offline.
- Specification 016 lines 82 and 196-198 currently disagree: the public `JoinSpace` result says the workspace change is saved, while the runtime sequence delays that save until the sponsor receives the joiner's readiness confirmation. The public result must describe only local readiness, and the scenario matrix must cover a sponsor crash after readiness.
- Specification 016 lines 94-106 omit `removal_intent_count`, although specification 015 promises that fact in its public summary. The unified snapshot must carry it explicitly.
- Specification 016 lines 159-186 specifies a restricted channel and message bounds but does not yet specify application-layer encryption or current-member authorization before releasing an offer.
- ADR-015 lines 43-50 still delegates the whole workflow and public state to a separate distributed member-removal module. Its background and consequences also say that joining and recovery retain their own paths. ADR-016 must explicitly supersede those responsibility and public-state sections, and ADR-015 must point to the unified owner without changing its intent-merging rules.
- Specification 015 already identifies `WorkspaceConvergence` as the complete owner, but its conflict-precedence sentence is insufficient while the adopted ADR text remains contradictory. It also requires the unified snapshot to expose the number of removal intents, so specification 016 needs the literal `removal_intent_count` field.
- The architecture overview requires every non-file business message to use application-layer AEAD in addition to Iroh's authenticated encrypted transport, and its current network table distinguishes membership, group-update, removal-exchange, removal-late, and removal-notice channels. The recovery specification must meet the same security bar without claiming that a device on an older security generation can derive the current workspace key.
- Architecture overview lines 181-186 still names a standalone member-removal coordinator as the complete owner. This must be revised to identify `WorkspaceConvergence` as the complete owner and member removal as its internal operation, while retaining the special late-intent and removed-member boundaries.
- Current in-progress code has only a narrow `WorkspaceRecoveryTransportPort`: it pushes relayed security updates to member device identifiers and does not yet model a request proof, active-membership authorization, payload encryption context, or acknowledgements. The finished specification must describe the complete target rather than treating that port as sufficient.
- The membership-attestation implementation builds a domain-separated transcript bound to the Space, group epoch, both device identities, nonces, transport keys, and addresses, then verifies a member-held secret's proof. This is evidence for a peer-bound handshake context, but its current source search was too broad to establish that it already provides an AEAD payload key for an unknown recovering peer. The specification must require an explicit derived, authenticated handoff key and implementation-stage proof that it is available before normal mutual trust.
- The current session stores a catalog of Space content keys by epoch and can merge continuous material history, but an incoming device can legitimately lack the donor's current key. This confirms the recovery contract must not encrypt its first offer with the current content key alone.
- The current recovery delivery loop selects all listed members and pushes updates without an explicit current-member-instance check, challenge, encryption context, or acknowledgement. It is useful pre-existing evidence but must be replaced rather than documented as the finished authority.
- The current membership-attestation exchange is useful only as an identity and continuity prerequisite: it binds the Space, group epoch, both device identities and transport keys, fresh nonces, identity/address digests, and the relayed-update digest into mutual current-member signatures. It applies the relayed updates before accepting the peer. Its wire frames are serialized directly into the Iroh stream, so they do not satisfy the required application-layer AEAD for a recovery offer.
- Existing session material can resolve a purpose-separated transport key by historical content-key identifier. Existing clipboard transfer demonstrates an AEAD framing pattern with a key identifier, epoch, and bound additional data, but it is content-transfer-specific and must not become the recovery protocol by copy/paste.
- Recovery must use a separately specified application-layer sealed envelope after an authenticated historical-membership handshake. Its authenticated context must bind the protocol version, Space lineage, both member instances and transport public keys, start/end epochs, chain digest, fresh request/session identifier, and monotonic response number. The implementation must derive the one-use encryption key from a mutually verifiable historic security-generation key plus both fresh endpoint contributions, so it neither assumes the current key nor lets an old removed member decrypt a captured offer. Replayed identifiers must be durably rejected.
- A historical proof establishes only eligibility to request a bounded handshake. Before sending any current offer, the responder must compute whether the requested member instance is currently effective. A removed instance receives no offer and may use only the removal-notice and late-historical-intent paths.

## Issues Encountered

| Issue | Resolution |
| --- | --- |
| Existing worktree is dirty with concurrent source changes | Limit edits to new/related documentation and inspect diffs before patching shared docs. |
| One shell search interpreted Markdown backticks | Use literal quoted search patterns; no repository file was changed by the failed search. |
| One broad documentation patch missed an exact paragraph match | Split the consistency update into focused, context-verified patches; the failed patch made no changes. |

## Resources

- `docs/adr/016-workspace-wide-convergence.md`
- `docs/specs/012-automatic-shared-device-refresh.md`
- `docs/specs/015-offline-first-member-removal.md`
- `docs/specs/uc-engine-interface.md`
- `docs/architecture/architecture-bible.md`
