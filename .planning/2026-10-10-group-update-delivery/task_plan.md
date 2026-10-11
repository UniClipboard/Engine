# Issue #171 group-update delivery classification

Base: main 193e9d27f0be4157a28d9bc065507e753d8188aa

## Owner / action
- Owner: DeliverPendingGroupUpdatesUseCase (application) + space_security_store delivery (storage) + iroh group_update_adapter (wire).
- Caller action: one maintenance round; unchanged.

## Design
1. Wire: new ALPN group-update/2 (ack 1 accepted, 2 rejected-by-receiver, 3 receiver-unavailable); /1 kept as fallback (old peers). On /1 receiver never answers REJECTED for transient (closes without ack -> old sender defers).
2. Receiver classification (fail-closed allow-list for transient): uc-infra-security next to group_update_failure_detail.
3. Core: GroupUpdateDispatchError::{Rejected(GroupUpdateRejection::{ByReceiver,Unclassified}), ReceiverUnavailable}.
4. Storage DeliveryState: additive serde(default) fields (rejection class, last cause); old rejected=true rows => Unclassified.
5. Recovery: Unclassified rejection re-sent only when peer is in reachable_peers (fresh authenticated history confirmation) AND next_attempt_ms elapsed; success only via ACK_ACCEPTED. ByReceiver never auto resent.
6. Projection: per-device group_update delivery in DeviceTrustDevice + engine summary.
## Phases
- [ ] E2E driver + old-impl failing baseline
- [ ] implement
- [ ] docs
- [ ] checks, commits, report
