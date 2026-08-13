# Findings

## Confirmed input evidence

- Repository HEAD is `6ec8c35c21220c4b41b8be31f235c111b2226e73`, matching the Engine revision used by the reproducing Desktop build.
- Worktree was clean at investigation start.
- The handoff correlates both profiles at 2026-08-13 13:40:43 UTC: transport connects, both briefly report Online, sponsor rejects joiner admission, closes with `peer_not_admitted`, and joiner becomes Offline.
- Membership attestation also fails in both directions (`gossip_source_rejected` and `gossip_application_rejected`), so this is durable membership/admission inconsistency rather than a UI refresh defect.
- ADR-017/020 require pairing success to mean sponsor and joiner have saved and enabled the same signed membership branch; transport connection alone is not success or presence.
- Architecture bible says reachability, membership, and trust are separate facts and `WorkspaceConvergence` owns durable admission/completion.

## Investigation targets

- Pairing completion events and the sponsor/joiner activation acknowledgements.
- Durable membership history (`known_head`, `applied_head`) and space protection/admission state installed on each side.
- Presence adapter transition ordering and connection close handling.
- Existing real-engine pairing and clipboard E2E fixtures suitable for a deterministic regression.

## Live log refinement

- Both profiles repeatedly log membership-history hello validation with `device_matches=true`, `instance_matches=true`, `lineage_matches=true`, but `signature_valid=false`.
- Sponsor repeatedly rejects the joiner's current-space admission while legacy upgrade reports that both peers already share a protection group.
- Sponsor's presence dial logs `dial succeeded, peer marked Online` before the remote admission decision is known; the later close is therefore a separate optimistic-presence defect.
- Existing `space_membership_auto_pairing_e2e` already contains a two-engine restart plus bidirectional receiver-persistence scenario and is the preferred full-loop seam.
- Existing `slice2_phase1_presence_e2e` exercises real Iroh presence and is the preferred focused presence seam.

## Root-cause direction

- A normal three-device relay/restart/bidirectional-content baseline passes, so the defect needs the failed-delivery timing rather than membership topology alone.
- Sponsor group admission persists pending updates for existing members, then `complete()` calls `deliver_pending()` once after sending `AdmissionSaved` to the joiner.
- Any delivery error is only logged; pairing closes and the joiner can return `SpaceJoined` regardless. This permits externally visible success while an old member still has the pre-admission protection state.
- Presence outbound currently writes and broadcasts Online immediately after transport connect. The receiver's admission rejection arrives only through subsequent connection closure, so the caller can observe a false Online interval.
- Fresh A/B/C topology with B offline while A admits C passes before the repair, including restart and bidirectional receiver persistence. The production incident therefore depends on stale persisted local-member identity, not the topology alone.
- Membership hello is freshly signed each time, but it previously combined the persisted `own_instance` with the current security identity's signing key. If those drift, the receiver sees matching device/instance/lineage fields yet signature verification fails exactly as in the live logs.
