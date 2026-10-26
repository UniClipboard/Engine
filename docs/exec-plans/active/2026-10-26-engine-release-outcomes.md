# Engine resource-release outcomes

Status: implemented. Focused scenarios, legacy/concurrent binding checks and repository gates passed. Final source-pinned replay and generated declarations are recorded in the task handoff. Native/device acceptance remains open.

## Owner and scope

The existing Engine shutdown owner owns admission closure, accepted-work drain, runtime teardown, required host-stream closure and this Engine's network-log registration. The binding worker owns its final join. The caller requests one complete suspend or shutdown action with a wait budget.

Process observability, other active Engines and Mobile-owned files remain outside this receipt. No internal phase query is added. No dependency or persistence format is added. The retained network stack and archive refs remain unchanged.

## Outcomes

- Pending: the caller's wait expired. The original cleanup/reply remains owned.
- Released with no error: all required closers in scope succeeded.
- Released with an error: a later successful full teardown confirmed release; the first error from an earlier shutdown attempt remains available. A post-release event-forwarder failure also preserves a released result at the binding boundary.
- Failed with an error: the attempt or worker ended without release confirmation. The host stops automatic pending retries and retains ownership. A live owner retains failed closer handles for explicit retry. A terminated worker cannot be restarted by waiting on it.

No store error is itself a release certificate. A persistence error without a positive final release result remains unconfirmed. This change does not extend the restricted persistence reproduction.

## Failure inventory before implementation

The task failure inventory includes caller timeout, terminal worker failure, early Stopped, failed closer handle loss, log park failure, erased earlier error, duplicate/concurrent shutdown, state rejection after shutdown, multiple Engines, process observability, controlled evidence overstatement and unknown blob release. It was written before product changes.

## Verification

The pre-fix real Engine host scenario returned success after a required stream closer failed and discarded its real flock handle. Exit 101 and the JSON failure record are preserved in the task library.
The real host scenario must preserve that handle after failure and confirm independent-process lock availability only after retry succeeds. It must preserve the earlier error and repeat the final receipt without re-closing.
The existing real binding E2E must verify its selected SQLite/redb/lease/log descriptors and locks after the new final receipt, and repeat shutdown/suspend outcomes. Controlled binding-owner scenarios cover cached terminal failure and caller timeout followed by late completion. Controlled scenarios do not certify real blob descriptors.

Run the focused scenarios, then metadata, workspace/all-target checks, formatting, Rust/repository architecture and diff checks. Generate Swift/Kotlin declarations from the exact host library. Device/framework acceptance and held actual blob-import descriptor acceptance remain separately listed gaps.

## Recovery

A failed live closer remains with its owner. Explicit retry can complete it. A pending wait never closes its lifecycle channel. A final worker failure is reported as Failed, not Pending and not an instruction to retry forever. Keep host ownership while release is unconfirmed.
