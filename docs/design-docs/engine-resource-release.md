# Engine resource-release contract

## Complete outcomes

Rust exposes `ResourceReleaseOutcome` through `Engine::suspend_with_outcome`, `shutdown_with_outcome` and `shutdown_until_released`.
UniFFI exposes `BindingResourceRelease` through `MobileEngine::suspend_with_outcome(deadline_ms)` and `shutdown_with_outcome(deadline_ms)`.

| Outcome | Host action |
| --- | --- |
| Pending | Retain the instance and ownership. Wait again with backoff. The accepted cleanup continues. |
| Released { error: None } | Engine release in the stated scope is confirmed. Finish the independent host-file release handshake. |
| Released { error: Some(error) } | Engine release is confirmed. Preserve/report the operational error. Do not retry release solely to erase the error. |
| Failed { error } | Release is unconfirmed. Retain ownership and surface failure. Do not turn the error or its retryable flag into release evidence. |

Failed is a completed failed attempt or a final failed worker, not a pending wait. A live owner keeps its failed required closer for explicit retry. A worker already terminated cannot restart through another wait. No eventual-success or platform-grant-deadline guarantee exists.

The error on Released preserves the first failed shutdown attempt after a subsequent successful full release. It can also describe an event-forwarder failure after successful core release and final worker join. An unknown persistence/store error remains Failed until a positive full release result exists. Error categories alone never certify release or identify a held lock.

## Resource scope

Engine release covers closed operation admission, drained accepted operations, successful runtime shutdown, destruction of the retained production runtime, required host change-stream closure, and release of this Engine's network-log registration. Final UniFFI shutdown also joins the worker, including its Tokio runtime destruction. Recoverable suspension closes its pools/security through the existing lifecycle participants and remains resumable.

The final-stop participant still allows profile-reset work to use stopped databases. Final public Engine shutdown separately removes its retained production runtime before certifying release. This preserves reset semantics while removing the former Stopped-before-pool-destruction gap.

A required closer failure retains its original handle. Host stream shutdown restores the failed stream to its owner. Final closure acquires an untaken optional stream; a still-borrowed stream returns an unconfirmed failure. Network-log parking retains the registration token and repeats the real queue acknowledgement. A failed blocking join does not become success.

The receipt excludes process observability, detached observability flushes, another active Engine's shared network writer, and Mobile-owned SQLite/history/diagnostic/Share/runtime-lock resources. The last active Engine waits for network-log parking. Another active Engine can legitimately keep that process writer alive. A per-Engine receipt is not a process-wide all-descriptor certificate.

## Repeated calls

Repeated outcome shutdown waits on the existing reply after a caller timeout. It does not close the lifecycle channel or abandon its cleanup. Concurrent waiters have independent deadlines. After confirmation, repeated shutdown returns the cached final receipt. The new outcome suspend method waits for the same shutdown once shutdown has taken over the instance.

Existing suspend methods retain their state-error behavior after shutdown. Existing Result-returning APIs remain available. The Rust legacy shutdown returns success after a successful later teardown; the outcome API additionally retains an earlier error. The UniFFI Result API preserves errors carried by its final receipt. Hosts requiring the release/error distinction must consume the outcome API.

Stopped remains a lifecycle state. Hosts must consume the result of their complete action rather than infer release from Stopped, a closed event stream, InvalidState, AlreadyStopped, RuntimeUnavailable or DeadlineExceeded.

## Validation boundary

The real temporary-host E2E runs on macOS/Linux. It owns a real flock in a controlled required closer and probes it from a separate Python process using `fcntl`. Other targets skip this probe; a skip is not a Windows runtime pass. The real binding E2E samples selected SQLite/redb/lease/network-log descriptors and performs independent lock checks and cross-process data access. The controlled binding-owner scenarios do not enumerate real blob resources.

No held actual blob import at the binding boundary, iOS background transition or physical-device crash sequence is certified by these desktop scenarios. The original iOS crash root cause remains unproved.

Run `bash scripts/testing/run-resource-release.sh` from the repository root to preserve repeatable non-graphical scenario logs, source/executable hashes and original command statuses. Set `UC_RELEASE_EVIDENCE` to a fresh private artifact directory. The restricted persistence reproduction is not part of this runner.
