#!/usr/bin/env python3
"""Record the current process-release baseline and network lease regression."""

import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
os.chdir(ROOT)
base = Path(os.environ.get("UC_PROCESS_RELEASE_EVIDENCE", ROOT / "target/test-artifacts/process-release-baseline"))
run_name = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S%fZ") + f"-{os.getpid()}"
evidence = base / run_name
evidence.mkdir(parents=True, exist_ok=False)
print(f"evidence: {evidence}", flush=True)


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


sources = [
    "crates/uc-observability-runtime/src/local_file.rs",
    "crates/uc-observability-runtime/src/runtime.rs",
    "crates/uc-observability-runtime/src/process_release.rs",
    "crates/uc-observability-runtime/src/process_census.rs",
    "crates/uc-observability-runtime/src/remote_health.rs",
    "crates/uc-observability-runtime/src/telemetry.rs",
    "crates/uc-observability-runtime/src/subscriber_gate.rs",
    "crates/uc-observability-runtime/Cargo.toml",
    "crates/uc-observability-runtime/src/lib.rs",
    "crates/uc-observability-runtime/src/network_log/store.rs",
    "crates/uc-observability-runtime/src/network_log/runtime.rs",
    "crates/uc-observability-runtime/src/engine_activity.rs",
    "crates/uc-observability-runtime/tests/process_flush_baseline.rs",
    "crates/uc-observability-runtime/tests/network_failed_resume.rs",
    "crates/uc-observability-runtime/tests/network_park_pending.rs",
    "crates/uc-observability-runtime/tests/network_resume_pending.rs",
    "crates/uc-observability-runtime/tests/network_flush_coordination.rs",
    "crates/uc-observability-runtime/examples/file_worker_failure_probe.rs",
    "crates/uc-observability-runtime/examples/process_registration_fence_probe.rs",
    "crates/uc-observability-runtime/examples/process_fence_inflight_probe.rs",
    "crates/uc-observability-runtime/examples/daily_file_park_probe.rs",
    "crates/uc-observability-runtime/examples/process_owner_probe.rs",
    "crates/uc-observability-runtime/examples/process_spawn_failure_probe.rs",
    "crates/uc-engine/examples/binding_delivery_failure_probe.rs",
    "crates/uc-engine/examples/binding_accepted_close_panic_probe.rs",
    "crates/uc-engine/examples/binding_cleanup_task_panic_probe.rs",
    "crates/uc-engine/examples/binding_cleanup_outer_panic_probe.rs",
    "crates/uc-engine/examples/binding_lifecycle_failed_close_probe.rs",
    "crates/uc-engine/examples/binding_lifecycle_deadline_probe.rs",
    "crates/uc-engine/examples/binding_operation_grace_probe.rs",
    "bindings/uc-engine-uniffi/src/runtime/shutdown_wait.rs",
    "bindings/uc-engine-uniffi/src/runtime/worker_lifecycle.rs",
    "bindings/uc-engine-uniffi/src/runtime/startup_delivery.rs",
    "crates/uc-engine/src/engine/log_activity.rs",
    "crates/uc-engine/examples/log_activity_drain_probe.rs",
    "crates/uc-engine/examples/log_registration_queued_probe.rs",
    "crates/uc-engine/examples/host_stream_panic_probe.rs",
    "crates/uc-engine/src/contract/host.rs",
    "crates/uc-engine/tests/host_contract/resource_release.rs",
    "crates/uc-engine/tests/host_contract/startup.rs",
    "crates/uc-engine/tests/host_contract/startup/abandoned_cleanup.rs",
    "crates/uc-engine/tests/host_contract/startup/partial_runtime.rs",
    "crates/uc-engine/tests/host_contract/key_loss.rs",
    "crates/uc-engine/src/engine/startup_owner.rs",
    "crates/uc-engine/src/engine/probe_startup_host.rs",
    "crates/uc-engine/src/engine/startup_outcome.rs",
    "crates/uc-engine/examples/startup_outcome_probe.rs",
    "crates/uc-engine/examples/startup_wait_probe.rs",
    "crates/uc-engine/examples/startup_owner_result_probe.rs",
    "crates/uc-engine/examples/backup_activity_probe.rs",
    "crates/uc-engine/examples/task_registry_release_probe.rs",
    "crates/uc-core/src/task_registry.rs",
    "crates/uc-application/src/application/shutdown.rs",
    "crates/uc-application/src/application/shutdown/owners.rs",
    "crates/uc-application/examples/application_shutdown_probe.rs",
    "crates/uc-infra-profile/src/security/profile_upgrade_backup/activity.rs",
    "crates/uc-infra-profile/src/security/profile_upgrade_backup/store.rs",
    "crates/uc-infra-profile/src/security/profile_upgrade_backup/security_materials.rs",
    "crates/uc-engine/src/assembly/host.rs",
    "crates/uc-engine/src/engine/startup.rs",
    "crates/uc-engine/src/engine/startup_control.rs",
    "crates/uc-engine/src/runtime/reusable_host.rs",
    "crates/uc-engine/src/runtime/profile_recovery.rs",
    "crates/uc-engine/Cargo.toml",
    "crates/uc-engine/src/runtime/mod.rs",
    "crates/uc-engine/src/runtime/shutdown.rs",
    "crates/uc-engine/src/runtime/shutdown/coordination.rs",
    "crates/uc-engine/examples/runtime_shutdown_resource_probe.rs",
    "crates/uc-engine/examples/network_recovery_shutdown_probe.rs",
    "crates/uc-engine/src/engine/mod.rs",
    "crates/uc-engine/src/lib.rs",
    "crates/uc-engine/src/engine/lifecycle/queue.rs",
    "crates/uc-engine/src/engine/shutdown.rs",
    "crates/uc-engine/tests/host_contract/startup/failure.rs",
    "bindings/uc-engine-uniffi/src/runtime.rs",
    "bindings/uc-engine-uniffi/src/lib.rs",
    "bindings/uc-engine-uniffi/src/observability.rs",
    "bindings/uc-engine-uniffi/src/process_release.rs",
    "bindings/uc-engine-uniffi/tests/public_contract.rs",
    "bindings/uc-engine-uniffi/src/runtime/startup_lifecycle.rs",
    "bindings/uc-engine-uniffi/Cargo.toml",
    "bindings/uc-engine-uniffi/tests/public_contract/startup_lifecycle.rs",
    "scripts/testing/run-process-release-baseline.py",
    "Cargo.lock",
]
results = {
    "utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "engine_commit": git("rev-parse", "HEAD"),
    "network_commit": git("-C", "third_party/iroh", "rev-parse", "HEAD"),
    "worktree_status": git("status", "--porcelain"),
    "tracked_diff_sha256": hashlib.sha256(subprocess.check_output(["git", "diff", "--binary", "HEAD"])).hexdigest(),
    "untracked_sha256": {name: digest(ROOT / name) for name in git("ls-files", "--others", "--exclude-standard").splitlines() if (ROOT / name).is_file()},
    "source_sha256": {name: digest(ROOT / name) for name in sources},
    "scenarios": [],
}
for name, package, test, artifact, selector, expected_failure in [
    ("flush", "uc-observability-runtime", "process_flush_baseline", "process-flush-baseline.json", None, False),
    ("failed_resume", "uc-observability-runtime", "network_failed_resume", "network-failed-resume.json", None, False),
    ("park_pending", "uc-observability-runtime", "network_park_pending", "network-park-pending.json", None, False),
    ("resume_pending", "uc-observability-runtime", "network_resume_pending", "network-resume-pending.json", None, False),
    ("flush_coordination", "uc-observability-runtime", "network_flush_coordination", "network-flush-coordination.json", None, False),
    ("worker_failure", "uc-observability-runtime", "file_worker_failure_probe", "file-worker-failure.json", None, False),
    ("registration_fence", "uc-observability-runtime", "process_registration_fence_probe", "process-registration-fence.json", None, False),
    ("fence_inflight", "uc-observability-runtime", "process_fence_inflight_probe", "process-fence-inflight.json", None, False),
    ("daily_file_park", "uc-observability-runtime", "daily_file_park_probe", "daily-file-park.json", None, False),
    ("process_generation_exhaustion", "uc-observability-runtime", "process_owner_probe", "process-generation-exhaustion.json", None, False),
    ("process_owner", "uc-observability-runtime", "process_owner_probe", "process-owner.json", None, False),
    ("process_daily_park_retry", "uc-observability-runtime", "process_owner_probe", "process-daily-park-retry.json", None, False),
    ("terminal_admission_deadline", "uc-observability-runtime", "process_owner_probe", "terminal-admission-deadline.json", None, False),
    ("process_daily_error_queued", "uc-observability-runtime", "process_owner_probe", "process-daily-error-queued.json", None, False),
    ("process_daily_error_queued_spawn_failure", "uc-observability-runtime", "process_owner_probe", "process-daily-error-queued-spawn-failure.json", None, False),
    ("process_release_spawn_error", "uc-observability-runtime", "process_owner_probe", "process-release-spawn-error.json", None, False),
    ("process_queued_release_spawn_error", "uc-observability-runtime", "process_owner_probe", "process-queued-release-spawn-error.json", None, False),
    ("process_daily_park_terminal", "uc-observability-runtime", "process_owner_probe", "process-daily-park-terminal.json", None, False),
    ("process_network_park_retry", "uc-observability-runtime", "process_owner_probe", "process-network-park-retry.json", None, False),
    ("process_both_file_errors", "uc-observability-runtime", "process_owner_probe", "process-both-file-errors.json", None, False),
    ("process_remote_export_error", "uc-observability-runtime", "process_owner_probe", "process-remote-export-error.json", None, False),
    ("process_remote_export_file_error", "uc-observability-runtime", "process_owner_probe", "process-remote-export-file-error.json", None, False),
    ("process_remote_log_export_error", "uc-observability-runtime", "process_owner_probe", "process-remote-log-export-error.json", None, False),
    ("process_network_resume_held", "uc-observability-runtime", "process_owner_probe", "process-network-resume-held.json", None, False),
    ("process_network_retry_with_resume_held", "uc-observability-runtime", "process_owner_probe", "process-network-retry-with-resume-held.json", None, False),
    ("process_network_release_after_resume_held", "uc-observability-runtime", "process_owner_probe", "process-network-release-after-resume-held.json", None, False),
    ("process_census_shutdown_race", "uc-observability-runtime", "process_owner_probe", "process-census-shutdown-race.json", None, False),
    ("process_failed_resume", "uc-observability-runtime", "process_owner_probe", "process-owner-failed-resume.json", None, False),
    ("release_during_resume", "uc-observability-runtime", "process_owner_probe", "process-release-during-resume.json", None, False),
    ("release_after_queued_resume", "uc-observability-runtime", "process_owner_probe", "process-release-after-queued-resume.json", None, False),
    ("network_export_overlap", "uc-observability-runtime", "process_owner_probe", "process-network-export-overlap.json", None, False),
    ("superseded_resume", "uc-observability-runtime", "process_owner_probe", "process-superseded-resume.json", None, False),
    ("spawn_failure", "uc-observability-runtime", "process_spawn_failure_probe", "process-spawn-failure.json", None, False),
    ("binding_delivery_failure", "uc-engine", "binding_delivery_failure_probe", "binding-delivery-failure.json", None, False),
    ("binding_accepted_close_panic", "uc-engine", "binding_accepted_close_panic_probe", "binding-accepted-close-panic.json", None, False),
    ("binding_accepted_close_panic_failed", "uc-engine", "binding_accepted_close_panic_probe", "binding-accepted-close-panic-failed.json", None, False),
    ("binding_cleanup_task_panic", "uc-engine", "binding_cleanup_task_panic_probe", "binding-cleanup-task-panic.json", None, False),
    ("binding_cleanup_outer_panic", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-outer-panic.json", None, False),
    ("binding_cleanup_observer_failure", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-observer-failure.json", None, False),
    ("binding_cleanup_observer_panic", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-observer-panic.json", None, False),
    ("binding_cleanup_observer_run_panic", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-observer-run-panic.json", None, False),
    ("binding_cleanup_observer_polled_panic", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-observer-polled-panic.json", None, False),
    ("binding_cleanup_observer_drop_panic", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-observer-drop-panic.json", None, False),
    ("binding_cleanup_engine_worker", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-outer-engine-worker.json", None, False),
    ("binding_observer_retry", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-observer-retry.json", None, False),
    ("binding_observer_retry_concurrent", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-observer-concurrent.json", None, False),
    ("binding_observer_retry_failure", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-observer-retry-failed.json", None, False),
    ("binding_observer_retry_lost", "uc-engine", "binding_cleanup_outer_panic_probe", "binding-cleanup-observer-retry-lost.json", None, False),
    ("binding_lifecycle_failed_close", "uc-engine", "binding_lifecycle_failed_close_probe", "binding-lifecycle-failed-close.json", None, False),
    ("binding_lifecycle_explicit_retry", "uc-engine", "binding_lifecycle_failed_close_probe", "binding-lifecycle-explicit-retry.json", None, False),
    ("binding_lifecycle_deadline", "uc-engine", "binding_lifecycle_deadline_probe", "binding-lifecycle-deadline.json", None, False),
    ("binding_operation_grace", "uc-engine", "binding_operation_grace_probe", "binding-operation-grace.json", None, False),
    ("binding_operation_grace_expired", "uc-engine", "binding_operation_grace_probe", "binding-operation-grace-expired.json", None, False),
    ("application_shutdown_executor", "uc-application", "application_shutdown_probe", "application-shutdown-executor.json", None, False),
    ("task_registry_release", "uc-engine", "task_registry_release_probe", "task-registry-release.json", None, False),
    ("runtime_shutdown_resource", "uc-engine", "runtime_shutdown_resource_probe", "runtime-shutdown-resource.json", None, False),
    ("network_recovery_shutdown", "uc-engine", "network_recovery_shutdown_probe", "network-recovery-shutdown.json", None, False),
    ("remote_span_overlap", "uc-observability-runtime", "process_owner_probe", "process-remote-span-overlap.json", None, False),
    ("lazy_remote_span", "uc-observability-runtime", "process_owner_probe", "process-remote-span-overlap.json", None, False),
    ("trace_admission_race", "uc-observability-runtime", "process_owner_probe", "trace-admission-race.json", None, False),
    ("trace_after_resume", "uc-observability-runtime", "process_owner_probe", "trace-admission-race.json", None, False),
    ("trace_panic_before", "uc-observability-runtime", "process_owner_probe", "trace-callback-panic.json", None, False),
    ("trace_panic_after", "uc-observability-runtime", "process_owner_probe", "trace-callback-panic.json", None, False),
    ("log_activity_drain", "uc-engine", "log_activity_drain_probe", "log-activity-drain.json", None, False),
    ("log_registration_queued", "uc-engine", "log_registration_queued_probe", "log-registration-queued.json", None, False),
    ("host_stream_panic", "uc-engine", "host_stream_panic_probe", "host-stream-panic.json", None, False),
    ("two_engines", "uc-engine", "host_contract", "shared-network-two-engines.json", "resource_release::shared_network_lease_waits_for_both_engines", False),
    ("orphan_closer_retry", "uc-engine", "host_contract", "abandoned-engine-closer.json", "resource_release::process_retains_and_retries_abandoned_engine_closer", False),
    ("suspended_orphan", "uc-engine", "host_contract", "suspended-engine-orphan.json", "resource_release::suspended_engine_orphan_invalidates_process_receipt", False),
    ("orphan_retry_other_engine", "uc-engine", "host_contract", "orphan-retry-other-engine.json", "resource_release::orphan_retry_waits_for_another_active_engine", False),
    ("new_orphan_during_retry", "uc-engine", "host_contract", "new-orphan-during-retry.json", "resource_release::new_orphan_during_retry_waits_for_running_closer", False),
    ("orphan_retry_fails_again", "uc-engine", "host_contract", "orphan-retry-fails-again.json", "resource_release::orphan_retry_failure_keeps_the_same_closer", False),
    ("orphan_retry_executor_failure", "uc-engine", "host_contract", "orphan-retry-executor-failure.json", "resource_release::orphan_retry_executor_failure_retains_source_and_owner", False),
    ("dropped_engine_inflight_close", "uc-engine", "host_contract", "dropped-engine-inflight-close.json", "resource_release::process_joins_shutdown_after_engine_handle_drops", False),
    ("suspended_inflight_generation", "uc-engine", "host_contract", "suspended-inflight-generation.json", "resource_release::suspended_engine_inflight_close_advances_process_generation", False),
    ("terminated_engine_close", "uc-engine", "host_contract", "terminated-engine-close.json", "resource_release::terminated_engine_close_remains_terminal", False),
    ("pending_orphan_during_retry", "uc-engine", "host_contract", "pending-orphan-during-retry.json", "resource_release::pending_orphan_arriving_during_retry_stays_pending", False),
    ("orphan_span_retry", "uc-engine", "host_contract", "orphan-span-retry.json", "resource_release::process_retry_closes_engine_before_waiting_for_its_span", False),
    ("orphan_during_span", "uc-engine", "host_contract", "orphan-during-span-drain.json", "resource_release::orphan_arriving_during_span_drain_unblocks_retry", False),
    ("pending_orphan_span", "uc-engine", "host_contract", "pending-orphan-span-drain.json", "resource_release::pending_orphan_during_span_drain_waits_for_closer", False),
    ("owned_executor_reservation", "uc-engine", "owned_executor_probe", "owned-executor-reservation.json", None, False),
    ("owned_executor", "uc-engine", "owned_executor_probe", "owned-executor.json", None, False),
    ("owned_executor_panic", "uc-engine", "owned_executor_probe", "owned-executor-panic.json", None, False),
    ("startup_outcome", "uc-engine", "startup_outcome_probe", "startup-outcome.json", None, False),
    ("startup_wait", "uc-engine", "startup_wait_probe", "startup-wait.json", None, False),
    ("startup_owner_result", "uc-engine", "startup_owner_result_probe", "startup-owner-result.json", None, False),
    ("startup_rollback_backup", "uc-engine", "startup_owner_result_probe", "startup-rollback-backup.json", None, False),
    ("startup_rollback_backup_panic", "uc-engine", "startup_owner_result_probe", "startup-rollback-backup-panic.json", None, False),
    ("startup_rollback_backup_settled", "uc-engine", "startup_owner_result_probe", "startup-rollback-backup-settled.json", None, False),
    ("startup_host_failure_backup", "uc-engine", "startup_owner_result_probe", "startup-host-failure-backup.json", None, False),
    ("startup_host_first_backup", "uc-engine", "startup_owner_result_probe", "startup-host-failure-first-backup.json", None, False),
    ("startup_backup_first_host", "uc-engine", "startup_owner_result_probe", "startup-backup-first-host-failure.json", None, False),
    ("startup_host_success_first_backup", "uc-engine", "startup_owner_result_probe", "startup-host-success-first-backup.json", None, False),
    ("startup_backup_first_host_success", "uc-engine", "startup_owner_result_probe", "startup-backup-first-host-success.json", None, False),
    ("startup_worker_thread_backup", "uc-engine", "startup_owner_result_probe", "startup-worker-thread-backup.json", None, False),
    ("startup_worker_executor_backup", "uc-engine", "startup_owner_result_probe", "startup-worker-executor-backup.json", None, False),
    ("startup_worker_panic_backup", "uc-engine", "startup_owner_result_probe", "startup-worker-panic-backup.json", None, False),
    ("startup_worker_backup_settled", "uc-engine", "startup_owner_result_probe", "startup-worker-backup-settled.json", None, False),
    ("startup_retry_worker_success", "uc-engine", "startup_owner_result_probe", "startup-retry-worker-success.json", None, False),
    ("startup_retry_worker_error", "uc-engine", "startup_owner_result_probe", "startup-retry-worker-error.json", None, False),
    ("startup_retry_worker_panic", "uc-engine", "startup_owner_result_probe", "startup-retry-worker-panic.json", None, False),
    ("startup_retry_executor_panic", "uc-engine", "startup_owner_result_probe", "startup-retry-executor-panic.json", None, False),
    ("startup_retry_executor_drop_panic", "uc-engine", "startup_owner_result_probe", "startup-retry-worker-drop-panic.json", None, False),
    ("startup_retry_error_and_drop_panic", "uc-engine", "startup_owner_result_probe", "startup-retry-worker-drop-error.json", None, False),
    ("startup_retry_callback_and_drop_panic", "uc-engine", "startup_owner_result_probe", "startup-retry-worker-drop-callback.json", None, False),
    ("startup_undelivered_worker_thread", "uc-engine", "startup_owner_result_probe", "startup-undelivered-worker-thread.json", None, False),
    ("startup_undelivered_worker_executor", "uc-engine", "startup_owner_result_probe", "startup-undelivered-worker-executor.json", None, False),
    ("startup_undelivered_executor_success", "uc-engine", "startup_owner_result_probe", "startup-undelivered-executor-worker-success.json", None, False),
    ("startup_undelivered_executor_error", "uc-engine", "startup_owner_result_probe", "startup-undelivered-executor-worker-error.json", None, False),
    ("startup_undelivered_executor_panic", "uc-engine", "startup_owner_result_probe", "startup-undelivered-executor-worker-panic.json", None, False),
    ("owned_start_thread", "uc-engine", "startup_owner_result_probe", "owned-start-thread.json", None, False),
    ("owned_start_runtime", "uc-engine", "startup_owner_result_probe", "owned-start-runtime.json", None, False),
    ("owned_start_host_retry", "uc-engine", "startup_owner_result_probe", "owned-start-host-retry.json", None, False),
    ("terminated_owned_engine_retry", "uc-engine", "startup_owner_result_probe", "terminated-owned-engine-retry.json", None, False),
    ("terminated_owned_process_retry", "uc-engine", "startup_owner_result_probe", "terminated-owned-process-retry.json", None, False),
    ("terminated_owned_process_unexpected", "uc-engine", "startup_owner_result_probe", "terminated-owned-process-unexpected.json", None, False),
    ("terminated_owned_process_pending", "uc-engine", "startup_owner_result_probe", "terminated-owned-process-pending.json", None, False),
    ("terminated_owned_waiter", "uc-engine", "startup_owner_result_probe", "terminated-owned-executor-waiter.json", None, False),
    ("terminated_active_lifecycle", "uc-engine", "startup_owner_result_probe", "terminated-active-lifecycle.json", None, False),
    ("delivered_engine_owned_executor", "uc-engine", "startup_owner_result_probe", "delivered-engine-owned-executor.json", None, False),
    ("startup_independent_executor", "uc-engine", "startup_owner_result_probe", "startup-independent-executor.json", None, False),
    ("startup_log_begin_success", "uc-engine", "startup_owner_result_probe", "startup-log-begin-failure-success.json", None, False),
    ("startup_log_begin_error", "uc-engine", "startup_owner_result_probe", "startup-log-begin-failure-error.json", None, False),
    ("startup_log_begin_panic", "uc-engine", "startup_owner_result_probe", "startup-log-begin-failure-panic.json", None, False),
    ("startup_real_rollback_task_panic", "uc-engine", "startup_owner_result_probe", "startup-real-rollback-task-panic.json", None, False),
    ("startup_registered_backup_host_retry", "uc-engine", "startup_owner_result_probe", "startup-registered-backup-host-retry.json", None, False),
    ("startup_detached_host_close", "uc-engine", "startup_owner_result_probe", "startup-detached-host-close.json", None, False),
    ("startup_detached_host_close_error", "uc-engine", "startup_owner_result_probe", "startup-detached-host-close-error.json", None, False),
    ("startup_detached_host_close_panic", "uc-engine", "startup_owner_result_probe", "startup-detached-host-close-panic.json", None, False),
    ("backup_activity", "uc-engine", "backup_activity_probe", "backup-activity.json", None, False),
    ("startup_baseline", "uc-engine", "host_contract", "abandoned-startup-failed-closer.json", "startup::abandoned_cleanup::abandoned_startup_retains_failed_closer", False),
    ("startup_executor", "uc-engine", "host_contract", "abandoned-startup-executor-stopped.json", "startup::abandoned_cleanup::abandoned_startup_keeps_closer_when_executor_stops", False),
    ("partial_runtime", "uc-engine", "host_contract", "partial-runtime-executor-stopped.json", "startup::partial_runtime::executor_loss_during_engine_construction_observes_profile_descriptor", False),
    ("late_registration", "uc-engine", "host_contract", "late-network-registration.json", "startup::partial_runtime::executor_loss_retains_late_network_registration", False),
    ("recovered_unlock_unconfirmed", "uc-engine", "host_contract", "recovered-unlock-unconfirmed.json", "key_loss::recovered_unlock_failure_retains_unconfirmed_runtime", False),
    ("recovered_unlock_released", "uc-engine", "host_contract", "recovered-unlock-released.json", "key_loss::recovered_unlock_failure_releases_after_successful_cleanup", False),
    ("recovery_executor_loss", "uc-engine", "host_contract", "recovery-executor-loss.json", "key_loss::recovery_executor_loss_keeps_accepted_unlock_and_runtime", False),
    ("recovery_construction_executor_loss", "uc-engine", "host_contract", "recovery-construction-executor-loss.json", "key_loss::recovery_executor_loss_during_runtime_construction", False),
    ("recovery_construction_shutdown_race", "uc-engine", "host_contract", "recovery-construction-shutdown-race.json", "key_loss::recovery_construction_completes_during_shutdown", False),
    ("recovery_detached_backup", "uc-engine", "host_contract", "recovery-detached-backup.json", "key_loss::recovery_shutdown_waits_for_detached_backup", False),
    ("recovery_detached_backup_error", "uc-engine", "host_contract", "recovery-detached-backup-error.json", "key_loss::recovery_shutdown_waits_for_failed_detached_backup", False),
    ("recovery_detached_backup_panic", "uc-engine", "host_contract", "recovery-detached-backup-panic.json", "key_loss::recovery_shutdown_waits_for_panicked_detached_backup", False),
    ("recovery_backup_shutdown_executor_loss", "uc-engine", "host_contract", "recovery-backup-shutdown-executor-loss.json", "key_loss::recovery_backup_survives_shutdown_executor_loss", False),
    ("process_recovery_backup", "uc-engine", "host_contract", "process-recovery-backup-shutdown-executor-loss.json", "key_loss::process_waits_for_backup_after_shutdown_executor_loss", False),
    ("factory_reset_backup", "uc-engine", "host_contract", "factory-reset-detached-backup.json", "key_loss::factory_reset_shutdown_waits_for_detached_backup", False),
    ("factory_reset_backup_error", "uc-engine", "host_contract", "factory-reset-detached-backup-error.json", "key_loss::factory_reset_shutdown_waits_for_failed_backup", False),
    ("factory_reset_backup_panic", "uc-engine", "host_contract", "factory-reset-detached-backup-panic.json", "key_loss::factory_reset_shutdown_waits_for_panicked_backup", False),
    ("factory_reset_backup_executor_loss", "uc-engine", "host_contract", "factory-reset-backup-executor-loss.json", "key_loss::factory_reset_backup_survives_shutdown_executor_loss", False),
    ("process_factory_reset_backup", "uc-engine", "host_contract", "process-factory-reset-backup-shutdown-executor-loss.json", "key_loss::process_waits_for_factory_reset_backup_after_shutdown_executor_loss", False),
    ("recovery_handoff_shutdown", "uc-engine", "host_contract", "recovery-handoff-shutdown.json", "key_loss::successful_recovery_handoff_joins_shutdown", False),
    ("ready_reset_reconstruction", "uc-engine", "host_contract", "ready-reset-rebuild-executor-loss.json", "key_loss::ready_reset_reconstruction_survives_executor_loss", False),
    ("process_ready_reset_reconstruction", "uc-engine", "host_contract", "process-ready-reset-rebuild.json", "key_loss::process_waits_for_ready_reset_reconstruction", False),
    ("abandoned_recovery_process", "uc-engine", "host_contract", "abandoned-recovery-process.json", "key_loss::process_release_after_abandoned_recovery", False),
    ("partial_start", "uc-engine", "host_contract", "partial-start-failed-closer.json", "startup::failure::failed_service_start_retains_failed_host_closer", False),
    ("engine_startup_explicit_retry", "uc-engine", "host_contract", "engine-startup-explicit-retry.json", "startup::failure::engine_startup_retry_joins_the_same_host_close", False),
    ("partial_executor", "uc-engine", "host_contract", "partial-start-executor-stopped.json", "startup::failure::failed_start_keeps_primary_error_when_executor_stops_during_host_close", False),
    ("profile_order", "uc-engine", "host_contract", "profile-cleanup-order.json", "startup::failure::profile_preparation_failure_runs_cleanup", False),
    ("unpolled_startup", "uc-engine", "host_contract", "unpolled-startup.json", "startup::failure::unpolled_startup_retains_host_closer", False),
    ("unpolled_cleanup", "uc-engine", "host_contract", "unpolled-startup-cleanup.json", "startup::failure::unpolled_startup_runs_owned_cleanup", False),
    ("startup_no_executor", "uc-engine", "host_contract", "startup-no-executor-cleanup.json", "startup::failure::startup_without_executor_returns_error_and_runs_cleanup", False),
    ("unpolled_startup_task", "uc-engine", "host_contract", "unpolled-startup-task.json", "startup::failure::unpolled_startup_task_runs_owned_cleanup_after_executor_stops", False),
    ("unpolled_cleanup_panic", "uc-engine", "host_contract", "unpolled-cleanup-panic.json", "startup::failure::unpolled_cleanup_panic_retains_host_closer", False),
    ("binding_worker_panic", "uc-engine-uniffi", "public_contract", "binding-worker-panic.json", "startup_lifecycle::binding_worker_panic_before_delivery_cleans_up_real_engine", False),
    ("binding_worker_spawn_failure", "uc-engine-uniffi", "public_contract", "binding-spawn-failure.json", "startup_lifecycle::binding_worker_spawn_failure_retains_primary_before_future_drop", False),
    ("binding_worker_runtime_drain", "uc-engine-uniffi", "public_contract", "binding-worker-runtime-drain.json", "startup_lifecycle::binding_delivery_owner_waits_for_worker_runtime_to_drain", False),
    ("binding_recovery_thread_failure", "uc-engine-uniffi", "public_contract", "binding-recovery-thread-failure.json", "startup_lifecycle::binding_recovery_worker_creation_failure_retains_host", False),
    ("binding_recovery_runtime_failure", "uc-engine-uniffi", "public_contract", "binding-recovery-runtime-failure.json", "startup_lifecycle::binding_recovery_worker_creation_failure_retains_host", False),
    ("binding_recovery_thread_retry", "uc-engine-uniffi", "public_contract", "binding-recovery-thread-retry.json", "startup_lifecycle::binding_recovery_creation_explicit_retry_releases_same_engine", False),
    ("binding_recovery_runtime_retry", "uc-engine-uniffi", "public_contract", "binding-recovery-runtime-retry.json", "startup_lifecycle::binding_recovery_creation_explicit_retry_releases_same_engine", False),
    ("binding_lost_thread_retry", "uc-engine-uniffi", "public_contract", "binding-lost-delivery-thread-retry.json", "startup_lifecycle::binding_recovery_creation_explicit_retry_releases_same_engine", False),
    ("binding_lost_runtime_retry", "uc-engine-uniffi", "public_contract", "binding-lost-delivery-runtime-retry.json", "startup_lifecycle::binding_recovery_creation_explicit_retry_releases_same_engine", False),
    ("binding_recovery_executor_panic", "uc-engine-uniffi", "public_contract", "binding-panic-executor-panic.json", "startup_lifecycle::binding_recovery_creation_explicit_retry_releases_same_engine", False),
    ("binding_lost_executor_panic", "uc-engine-uniffi", "public_contract", "binding-lost-executor-panic.json", "startup_lifecycle::binding_recovery_creation_explicit_retry_releases_same_engine", False),
    ("binding_preparation", "uc-engine-uniffi", "public_contract", "binding-preparation-failure.json", "startup_lifecycle::failed_host_preparation_finishes_the_startup_lifecycle", False),
    ("binding_invalid_engine_startup", "uc-engine-uniffi", "public_contract", "binding-invalid-engine-startup.json", "startup_lifecycle::binding_real_engine_rejected_config_retains_startup_receipt", False),
    ("binding_engine_startup_retry", "uc-engine-uniffi", "public_contract", "binding-engine-startup-retry.json", "startup_lifecycle::binding_real_engine_startup_retry_keeps_host_owner", False),
    ("binding_public_process_production", "uc-engine-uniffi", "public_contract", "binding-public-process-production.json", "process_release_public_contract_without_test_features", False),
    ("binding_process_release", "uc-engine-uniffi", "public_contract", "binding-process-release.json", "binding_process_release_waits_for_two_engines_and_resumes", False),
    ("binding_process_remote_span", "uc-engine-uniffi", "public_contract", "binding-process-remote-span.json", "binding_process_release_waits_for_accepted_remote_span", False),
    ("binding_loopback_disabled", "uc-engine-uniffi", "public_contract", "binding-test-loopback-disabled.json", "binding_test_loopback_keeps_endpoint_policy", False),
    ("binding_nonloopback_denied", "uc-engine-uniffi", "public_contract", "binding-test-nonloopback-denied.json", "binding_test_loopback_keeps_endpoint_policy", False),
    ("binding_executor_failure", "uc-engine-uniffi", "public_contract", "binding-executor-failure.json", "startup_lifecycle::binding_executor_creation_failure_retains_startup_host", False),
]:
    is_example = name in {"network_recovery_shutdown", "runtime_shutdown_resource", "task_registry_release", "application_shutdown_executor", "worker_failure", "registration_fence", "fence_inflight", "daily_file_park", "process_owner", "process_generation_exhaustion", "process_failed_resume", "release_during_resume", "release_after_queued_resume", "network_export_overlap", "superseded_resume", "spawn_failure", "binding_delivery_failure", "binding_accepted_close_panic", "binding_accepted_close_panic_failed", "binding_cleanup_task_panic", "binding_cleanup_outer_panic", "binding_cleanup_observer_failure", "binding_cleanup_engine_worker", "binding_observer_retry", "binding_observer_retry_concurrent", "binding_observer_retry_failure", "binding_observer_retry_lost", "binding_lifecycle_failed_close", "binding_lifecycle_explicit_retry", "binding_lifecycle_deadline", "binding_operation_grace", "binding_operation_grace_expired", "remote_span_overlap", "lazy_remote_span", "trace_admission_race", "trace_after_resume", "trace_panic_before", "trace_panic_after", "owned_executor", "owned_executor_panic", "owned_executor_reservation", "startup_outcome", "startup_wait", "startup_owner_result", "startup_rollback_backup", "startup_rollback_backup_panic", "startup_rollback_backup_settled", "startup_host_failure_backup", "startup_host_first_backup", "startup_backup_first_host", "startup_host_success_first_backup", "startup_backup_first_host_success", "startup_worker_thread_backup", "startup_worker_executor_backup", "startup_worker_panic_backup", "startup_worker_backup_settled", "startup_detached_host_close", "startup_detached_host_close_error", "startup_detached_host_close_panic", "startup_registered_backup_host_retry", "startup_real_rollback_task_panic", "owned_start_thread", "owned_start_runtime", "owned_start_host_retry", "terminated_owned_engine_retry", "terminated_owned_process_retry", "terminated_owned_process_unexpected", "terminated_owned_process_pending", "terminated_owned_waiter", "terminated_active_lifecycle", "delivered_engine_owned_executor", "startup_independent_executor", "startup_log_begin_success", "startup_log_begin_error", "startup_log_begin_panic", "startup_undelivered_worker_thread", "startup_undelivered_worker_executor", "startup_undelivered_executor_success", "startup_undelivered_executor_error", "startup_undelivered_executor_panic", "startup_retry_worker_success", "startup_retry_worker_error", "startup_retry_worker_panic", "backup_activity", "log_activity_drain", "log_registration_queued", "host_stream_panic"}
    is_example = is_example or name in {"binding_cleanup_observer_panic", "binding_cleanup_observer_run_panic", "binding_cleanup_observer_polled_panic", "binding_cleanup_observer_drop_panic", "startup_retry_executor_panic", "startup_retry_executor_drop_panic", "startup_retry_error_and_drop_panic", "startup_retry_callback_and_drop_panic", "process_daily_park_retry", "process_daily_park_terminal"}
    is_example = is_example or name in {"process_network_park_retry", "process_network_resume_held", "process_network_release_after_resume_held", "process_network_retry_with_resume_held"}
    is_example = is_example or name == "process_census_shutdown_race"
    is_example = is_example or name in {"process_daily_error_queued", "process_daily_error_queued_spawn_failure"}
    is_example = is_example or name == "terminal_admission_deadline"
    is_example = is_example or name == "process_both_file_errors"
    is_example = is_example or name in {"process_release_spawn_error", "process_queued_release_spawn_error"}
    is_example = is_example or name in {"process_remote_export_error", "process_remote_export_file_error", "process_remote_log_export_error"}
    command = ["just", "cargo", "run" if is_example else "test", "--locked", "-p", package]
    if name in {"binding_worker_panic", "binding_worker_spawn_failure", "binding_preparation", "binding_executor_failure", "binding_worker_runtime_drain", "binding_recovery_thread_failure", "binding_recovery_runtime_failure", "binding_recovery_thread_retry", "binding_recovery_runtime_retry", "binding_lost_thread_retry", "binding_lost_runtime_retry", "binding_recovery_executor_panic", "binding_lost_executor_panic", "binding_cleanup_outer_panic", "binding_cleanup_observer_failure", "binding_cleanup_engine_worker", "binding_observer_retry", "binding_observer_retry_concurrent", "binding_observer_retry_failure", "binding_observer_retry_lost"}:
        command.extend(["--features", "test-binding-worker-panic"])
    if name in {"binding_cleanup_observer_panic", "binding_cleanup_observer_run_panic", "binding_cleanup_observer_polled_panic", "binding_cleanup_observer_drop_panic"}:
        command.extend(["--features", "test-binding-worker-panic"])
    if name in {"binding_invalid_engine_startup", "binding_engine_startup_retry"}:
        command.extend(["--features", "test-binding-worker-panic"])
    if name in {"startup_retry_executor_panic", "startup_retry_executor_drop_panic", "startup_retry_error_and_drop_panic", "startup_retry_callback_and_drop_panic"}:
        command.extend(["--features", "dev-tools"])
    if name == "engine_startup_explicit_retry":
        command.extend(["--features", "dev-tools,test-binding-worker-panic"])
    if name in {"binding_process_release", "binding_process_remote_span", "binding_loopback_disabled", "binding_nonloopback_denied"}:
        command.extend(["--features", "test-process-release"])
    if name in {"process_daily_park_retry", "process_daily_park_terminal"}:
        command.extend(["--features", "test-process-release"])
    if name in {"process_network_park_retry", "process_network_resume_held", "process_network_release_after_resume_held", "process_network_retry_with_resume_held"}:
        command.extend(["--features", "test-process-release"])
    if name == "process_census_shutdown_race":
        command.extend(["--features", "test-process-release"])
    if name in {"process_daily_error_queued", "process_daily_error_queued_spawn_failure"}:
        command.extend(["--features", "test-process-release"])
    if name == "terminal_admission_deadline":
        command.extend(["--features", "test-process-release"])
    if name == "process_both_file_errors":
        command.extend(["--features", "test-process-release"])
    if name in {"process_release_spawn_error", "process_queued_release_spawn_error"}:
        command.extend(["--features", "test-process-release"])
    if name == "orphan_retry_executor_failure":
        command.extend(["--features", "dev-tools"])
    if name in {"process_remote_export_error", "process_remote_export_file_error", "process_remote_log_export_error"}:
        command.extend(["--features", "test-process-release"])
    if name in {"owned_executor_panic", "partial_start", "partial_executor", "profile_order", "unpolled_startup", "unpolled_cleanup", "startup_no_executor", "unpolled_startup_task", "unpolled_cleanup_panic", "startup_detached_host_close", "startup_detached_host_close_error", "startup_detached_host_close_panic", "startup_registered_backup_host_retry", "startup_real_rollback_task_panic", "owned_start_thread", "owned_start_runtime", "owned_start_host_retry", "terminated_owned_engine_retry", "terminated_owned_process_retry", "terminated_owned_process_unexpected", "terminated_owned_process_pending", "terminated_owned_waiter", "terminated_active_lifecycle", "delivered_engine_owned_executor", "startup_independent_executor", "startup_log_begin_success", "startup_log_begin_error", "startup_log_begin_panic", "startup_undelivered_worker_thread", "startup_undelivered_worker_executor", "startup_undelivered_executor_success", "startup_undelivered_executor_error", "startup_undelivered_executor_panic", "startup_retry_worker_success", "startup_retry_worker_error", "startup_retry_worker_panic"}:
        command.extend(["--features", "dev-tools"])
    command.extend(["--example" if is_example else "--test", test])
    if name in {"process_daily_park_retry", "process_daily_park_terminal"}:
        command.extend(["--", "--daily-park-retry" if name == "process_daily_park_retry" else "--daily-park-terminal"])
    if name == "process_network_park_retry":
        command.extend(["--", "--network-park-retry"])
    if name == "process_both_file_errors":
        command.extend(["--", "--both-file-errors"])
    if name in {"process_release_spawn_error", "process_queued_release_spawn_error"}:
        command.extend(["--", "--release-spawn-error" if name == "process_release_spawn_error" else "--queued-release-spawn-error"])
    if name in {"process_remote_export_error", "process_remote_export_file_error", "process_remote_log_export_error"}:
        arguments = {"process_remote_export_error": "--remote-export-error", "process_remote_export_file_error": "--remote-export-file-error", "process_remote_log_export_error": "--remote-log-export-error"}
        command.extend(["--", arguments[name]])
    if name == "process_network_resume_held":
        command.extend(["--", "--network-resume-held"])
    if name == "process_network_retry_with_resume_held":
        command.extend(["--", "--network-retry-with-resume-held"])
    if name == "process_network_release_after_resume_held":
        command.extend(["--", "--network-release-after-resume-held"])
    if name == "process_census_shutdown_race":
        command.extend(["--", "--census-shutdown-race"])
    if name == "terminal_admission_deadline":
        command.extend(["--", "--terminal-admission-deadline"])
    if name == "process_daily_error_queued":
        command.extend(["--", "--daily-error-queued"])
    if name == "process_daily_error_queued_spawn_failure":
        command.extend(["--", "--daily-error-queued-spawn-failure"])
    if name in {"binding_cleanup_engine_worker", "binding_observer_retry_concurrent"}:
        command.extend(["--features", "dev-tools"])
    if name == "binding_accepted_close_panic_failed":
        command.extend(["--", "--fail"])
    if name == "binding_cleanup_observer_failure":
        command.extend(["--", "--observer-fail"])
    if name == "binding_cleanup_observer_panic":
        command.extend(["--", "--observer-panic"])
    if name == "binding_cleanup_observer_run_panic":
        command.extend(["--", "--observer-run-panic"])
    if name == "binding_cleanup_observer_polled_panic":
        command.extend(["--features", "dev-tools", "--", "--observer-polled-panic"])
    if name == "binding_cleanup_observer_drop_panic":
        command.extend(["--", "--observer-drop-panic"])
    if name == "binding_cleanup_engine_worker":
        command.extend(["--", "--engine-worker"])
    if name == "binding_observer_retry":
        command.extend(["--", "--observer-retry"])
    if name == "binding_observer_retry_concurrent":
        command.extend(["--", "--observer-retry", "--engine-worker"])
    if name == "binding_observer_retry_failure":
        command.extend(["--", "--observer-retry-fail"])
    if name == "binding_observer_retry_lost":
        command.extend(["--", "--observer-retry-lost"])
    if name == "binding_lifecycle_explicit_retry":
        command.extend(["--", "--retry"])
    if name == "binding_operation_grace_expired":
        command.extend(["--", "--exceed"])
    if name in {"startup_rollback_backup", "startup_rollback_backup_panic", "startup_rollback_backup_settled", "startup_host_failure_backup", "startup_host_first_backup", "startup_backup_first_host", "startup_host_success_first_backup", "startup_backup_first_host_success", "startup_worker_thread_backup", "startup_worker_executor_backup", "startup_worker_panic_backup", "startup_worker_backup_settled"}:
        arguments = {"startup_rollback_backup": "--rollback-backup", "startup_rollback_backup_panic": "--rollback-backup-panic", "startup_rollback_backup_settled": "--rollback-backup-settled", "startup_host_failure_backup": "--host-failure-backup", "startup_host_first_backup": "--host-first-backup", "startup_backup_first_host": "--backup-first-host", "startup_host_success_first_backup": "--host-success-first-backup", "startup_backup_first_host_success": "--backup-first-host-success", "startup_worker_thread_backup": "--worker-thread-backup", "startup_worker_executor_backup": "--worker-executor-backup", "startup_worker_panic_backup": "--worker-panic-backup", "startup_worker_backup_settled": "--worker-backup-settled"}
        command.extend(["--", arguments[name]])
    if name in {"startup_detached_host_close", "startup_detached_host_close_error", "startup_detached_host_close_panic"}:
        arguments = {"startup_detached_host_close": "--detached-host-close", "startup_detached_host_close_error": "--detached-host-close-error", "startup_detached_host_close_panic": "--detached-host-close-panic"}
        command.extend(["--", arguments[name]])
    if name in {"startup_retry_worker_success", "startup_retry_worker_error", "startup_retry_worker_panic"}:
        command.extend(["--", "--retry-worker-" + name.rsplit("_", 1)[1]])
    if name == "startup_retry_executor_panic":
        command.extend(["--", "--retry-worker-init-panic"])
    if name == "startup_retry_executor_drop_panic":
        command.extend(["--", "--retry-worker-drop-panic"])
    if name == "startup_retry_error_and_drop_panic":
        command.extend(["--", "--retry-worker-drop-error"])
    if name == "startup_retry_callback_and_drop_panic":
        command.extend(["--", "--retry-worker-drop-callback"])
    if name in {"startup_undelivered_worker_thread", "startup_undelivered_worker_executor"}:
        command.extend(["--", "--undelivered-worker-" + name.rsplit("_", 1)[1]])
    if name in {"startup_undelivered_executor_success", "startup_undelivered_executor_error", "startup_undelivered_executor_panic"}:
        command.extend(["--", "--undelivered-executor-" + name.rsplit("_", 1)[1]])
    if name in {"owned_start_thread", "owned_start_runtime"}:
        command.extend(["--", "--owned-start-" + name.rsplit("_", 1)[1]])
    if name == "owned_start_host_retry":
        command.extend(["--", "--owned-start-host-retry"])
    if name in {"terminated_owned_engine_retry", "terminated_owned_process_retry", "terminated_owned_process_unexpected", "terminated_owned_process_pending"}:
        command.extend(["--", "--" + name.replace("_", "-")])
    if name == "terminated_active_lifecycle":
        command.extend(["--", "--terminated-active-lifecycle"])
    if name == "terminated_owned_waiter":
        command.extend(["--", "--terminated-owned-waiter"])
    if name == "delivered_engine_owned_executor":
        command.extend(["--", "--delivered-owned-executor"])
    if name == "startup_independent_executor":
        command.extend(["--", "--independent-startup-executor"])
    if name in {"startup_log_begin_success", "startup_log_begin_error", "startup_log_begin_panic"}:
        command.extend(["--", "--log-begin-" + name.rsplit("_", 1)[1]])
    if name == "startup_real_rollback_task_panic":
        command.extend(["--", "--real-rollback-task-panic"])
    if name == "startup_registered_backup_host_retry":
        command.extend(["--", "--registered-backup-host-retry"])
    if name == "owned_executor_reservation":
        command.extend(["--", "--reservation"])
    if name == "owned_executor_panic":
        command.extend(["--", "--panic"])
    if name == "process_generation_exhaustion":
        command.extend(["--features", "test-process-release", "--", "--generation-exhaustion"])
    if name == "process_failed_resume":
        command.extend(["--", "--failed-resume"])
    if name == "release_during_resume":
        command.extend(["--", "--release-during-resume"])
    if name == "release_after_queued_resume":
        command.extend(["--", "--release-after-queued-resume"])
    if name == "network_export_overlap":
        command.extend(["--", "--export-overlap"])
    if name == "superseded_resume":
        command.extend(["--", "--superseded-resume"])
    if name == "remote_span_overlap":
        command.extend(["--", "--remote-span-overlap"])
    if name == "lazy_remote_span":
        command.extend(["--", "--lazy-remote-span"])
    if name == "trace_admission_race":
        command.extend(["--", "--trace-admission-race"])
    if name == "trace_after_resume":
        command.extend(["--", "--trace-after-resume"])
    if name == "trace_panic_before":
        command.extend(["--", "--trace-panic-before"])
    if name == "trace_panic_after":
        command.extend(["--", "--trace-panic-after"])
    if selector:
        command.append(selector)
    if not is_example:
        command.extend(["--", "--nocapture"])
    if selector:
        command.append("--exact")
    if expected_failure:
        command.append("--ignored")
    environment = os.environ.copy()
    environment["UC_TEST_ARTIFACTS_DIR"] = str(evidence / name)
    result = subprocess.run(command, env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    (evidence / f"{name}.log").write_text(result.stdout)
    executables = re.findall(r"Running[^\n]*\(([^)]+)\)", result.stdout)
    if is_example:
        invocations = re.findall(r"Running `(?:mbx-launch )?([^`]+)`", result.stdout)
        executables = [shlex.split(invocation)[0] for invocation in invocations if shlex.split(invocation)]
    hashes = {str(Path(item).resolve()): digest(Path(item).resolve()) for item in executables if Path(item).is_file()}
    scenario_file = evidence / name / artifact
    one_test_passed = is_example or bool(re.search(r"test result: ok\. 1 passed; 0 failed", result.stdout))
    expected_failure_observed = False
    if expected_failure and scenario_file.is_file():
        observed = json.loads(scenario_file.read_text())
        expected_failure_observed = (
            result.returncode != 0
            and "failed cleanup must retain the actual closer" in result.stdout
            and observed.get("retry_allowed") is False
            and observed.get("failed_closer_lock_retained") is False
        )
    scenario = {"name": name, "command": command, "exit_code": result.returncode, "executable_sha256": hashes, "artifact_present": scenario_file.is_file(), "one_test_passed": one_test_passed, "expected_failure_observed": expected_failure_observed}
    results["scenarios"].append(scenario)
    (evidence / "result.json").write_text(json.dumps(results, indent=2) + "\n")
    print(f"{name}: exit={result.returncode}, executable={bool(hashes)}, artifact={scenario['artifact_present']}, expected_failure={expected_failure_observed}", flush=True)
    valid = expected_failure_observed if expected_failure else result.returncode == 0 and one_test_passed
    if not valid or not hashes or not scenario_file.is_file():
        print(result.stdout[-5000:])
        sys.exit(result.returncode or 1)
