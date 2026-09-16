# Findings: Admission Expiry S2-S8

## Baseline
- S0 commit: `44701858`.
- S1 commit: `91287ca1`.
- S1 only ends Joiner attempts before Prepared; S2 must not weaken that boundary.
- Existing public result types already contain pairing confirmation values from S0, but S2 must connect them to real Sponsor state and queries.
- Confirmation state is descriptive only and must not grant membership or synchronization access.
- The existing encrypted admission repository remains the only persistence source; no confirmation fields belong in the membership ledger.

## S2 invariants
- Sponsor inherits the Joiner attempt's original deadline; authentication or message receipt cannot start a new five-minute budget.
- Formal local application immediately becomes awaiting peer confirmation.
- At the original deadline it becomes unconfirmed without removing the member.
- A valid late confirmation can move unconfirmed to confirmed; removal or termination fences remain final.
- Wrong peer, predecessor, attempt, member binding, or replay must not change confirmation state.
- Confirmation-only changes notify the host without pretending that membership history changed.

## S2 current wiring
- The five-minute timeline exists only in the Joiner V2 disk record. Initial authentication, JoinRequest, and Sponsor records are still fixed to V1, so the Sponsor cannot derive or verify the same deadline.
- Sponsor Applied already saves the activation receipt, exact Complete reply, peer binding, and committed history in one transition. It is the narrow place to persist the confirmation state.
- Existing CompleteAck settlement already checks the authenticated peer, exact predecessor, and replay state. Late confirmation must reuse that path rather than add a looser acknowledgement path.
- The recovery summary is only a pending boolean and always restores JoinerAdmission. S2 needs Sponsor expiry to run through the sole Recovery owner; S7 can later finish bounded batches and full dual-role scheduling.
- Device-trust queries currently always return `pairing_confirmation=None`. The lookup must bind the Sponsor admission to the exact active member instance, not infer it from device_id alone.

## S4 invariants
- Prepared means the Sponsor may have committed even when the Joiner did not receive Commit; local termination therefore saves an unknown-commit cleanup obligation rather than claiming no member exists.
- Once Commit is saved, cleanup binds the authenticated attempt digest to the exact Space, member instance and Add event; later cleanup cannot select a newer instance by device id.
- Local cancellation does not wait for a CancelRequested exchange after the attempt is bounded by the new five-minute contract.
- The terminal record keeps the authenticated peer binding and continuation credential required by S5; the current Join pointer is released independently.
- Old records without the new deadline keep their previous recovery behavior and do not gain an invented cleanup contract.
- S2–S7 曾按切片顺序增加过渡磁盘格式；功能未发布，因此这些布局后来统一收敛到最终 V2，不形成长期兼容合同。

## S5 invariants
- A bounded terminal Joiner fixes one V2 Abandonment request, route and continuation credential; retries cannot generate a different request.
- Abandoned acknowledges durable receipt of the abandonment fact. It clears delivery work but never removes the local terminal fence.
- Sponsor validates the exact attempt digest, authenticated peer and current-stage predecessor before saving its exact reply.
- Sponsor Committed preserves the full original member binding. Sponsor Applied can preserve member/Add for lookup, but lookup must verify the current history and must never select by device id.
- Sponsor persists cleanup before replying. Restart scans that responsibility and transfers exact removal to the existing S3 owner.
- Missing original membership or loss of local membership cannot keep the user-visible pairing flow blocked forever; corrupt self-targets and invalid bindings still fail closed.
- Record V1-V3 layouts stay unchanged. V4 is the first format that persists the terminal outbox or Sponsor abandonment cleanup.
