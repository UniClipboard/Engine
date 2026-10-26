# Engine resource-release outcomes

Status: implemented. Focused scenarios, legacy/concurrent binding checks and repository gates passed. Implementation commit: `193487ac3b0409bb523fe5cb923222f4b4d55f2a`. Network commit: `413531dd7503c87ce89f21b69196f8c26eed909d`. Use the verification commands below to generate a fresh source-pinned replay. Generated declarations must match the native library used by the host. Native/device acceptance remains open.

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

The pre-implementation failure inventory included caller timeout, terminal worker failure, early Stopped, failed closer handle loss, log park failure, erased earlier error, duplicate/concurrent shutdown, state rejection after shutdown, multiple Engines, process observability, controlled evidence overstatement and unknown blob release. The inventory preceded product changes.

## Verification

The pre-fix real Engine host scenario returned success after a required stream closer failed and discarded its real flock handle. The scenario exited 101. Its failure record contained `shutdown_error=false`, `lock_retained_after_failure=false` and `calls=1`. This was false success after closer failure; it does not prove an arbitrary failed closer releases resources.
The real host scenario must preserve that handle after failure and confirm independent-process lock availability only after retry succeeds. It must preserve the earlier error and repeat the final receipt without re-closing.
The existing real binding E2E must verify its selected SQLite/redb/lease/log descriptors and locks after the new final receipt, and repeat shutdown/suspend outcomes. Controlled binding-owner scenarios cover cached terminal failure and caller timeout followed by late completion. Controlled scenarios do not certify real blob descriptors.

Run `bash scripts/testing/run-resource-release.sh` from the repository root. The runner writes `provenance.json`, per-scenario logs, JSON/TSV scenario evidence and `result.json` under the printed scenario directory. Set `UC_RELEASE_EVIDENCE` to select a fresh artifact directory. A successful command without executable hashes fails evidence validation. Use `python3 scripts/testing/resource-release-runner.test.py` to check path and Python compatibility with controlled command output; that check does not run Engine resource cleanup.

Run `just cargo test --locked -p uc-engine --test host_contract network_log_park_failure_emits_lifecycle_failure -- --nocapture` for the isolated-process failure-event scenario. It closes process observability before suspend to force a real network-log park failure. It verifies the returned error, Quiesced state and matching LifecycleFailed event. The injection does not represent ordinary process-owner ordering and does not certify device release. Resume registration failure remains source-reviewed; existing runtime-failure tests verify resume event reporting without injecting a log-registration join failure.

Run the focused scenarios, then metadata, workspace/all-target checks, formatting, Rust/repository architecture and diff checks. Generate Swift/Kotlin declarations from the exact host library. Device/framework acceptance and held actual blob-import descriptor acceptance remain separately listed gaps.

## Recovery

A failed live closer remains with its owner. Explicit retry can complete it. A pending wait never closes its lifecycle channel. A final worker failure is reported as Failed, not Pending and not an instruction to retry forever. Keep host ownership while release is unconfirmed.
