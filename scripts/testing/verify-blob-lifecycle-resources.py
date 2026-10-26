#!/usr/bin/env python3
"""校验原生生命周期原始报告，独立重放同一 CI 验收入口。"""
from collections import Counter
from pathlib import Path
import json
import sys

PREFIX = "testing::host_adapter_contract::receive_retry::"
EXPECTED_CASE_COUNTS = {
    PREFIX + "lifecycle::real_engine_lock_waits_for_active_retry_export_and_preserves_saved_files": 1,
    PREFIX + "lifecycle::real_engine_revocation_during_retry_prevents_old_output_from_being_committed": 1,
    PREFIX + "lifecycle::space::real_engine_space_reset_seals_active_retry_before_switching_identity": 1,
    PREFIX + "lifecycle::space::real_engine_confirmed_local_removal_prevents_active_retry_from_publishing": 1,
    PREFIX + "real_engine_abandoned_deletion_stops_retry_and_removes_all_receive_state": 2,
    PREFIX + "save_failure::real_engine_shutdown_waits_for_failed_file_settlement_repair": 1,
}


def verify_lifecycle_reports(reports):
    # 删除验收在即时调度和延迟调度下各运行一次；必须逐场景核对，不能只数文件。
    actual = Counter(report["case"] for report in reports)
    assert actual == Counter(EXPECTED_CASE_COUNTS), (
        "lifecycle scenario coverage mismatch: "
        f"missing={dict(Counter(EXPECTED_CASE_COUNTS) - actual)}, "
        f"unexpected={dict(actual - Counter(EXPECTED_CASE_COUNTS))}"
    )
    assert all(report['resource_settlement_verified'] is True for report in reports)
    assert all(report['fd_return_to_initial_verified'] and report['thread_return_to_initial_verified']
               and report['runtime_tasks_return_to_initial_verified'] for report in reports)
    assert all(report['actual_native_blob_handles_observed'] and report['native_blob_handles_returned_to_initial'] for report in reports)

def main():
    root=Path(sys.argv[1])
    reports=[json.loads(path.read_text()) for path in root.glob('receive-lifecycle-*.json')]
    verify_lifecycle_reports(reports)
    tags=[json.loads(path.read_text()) for path in root.glob('sdk-tags-*.json')]
    assert len(tags)>=12, 'SDK store shutdown observations missing'
    assert all(report['store_open_during_count'] and report['sdk_temp_tags']==0 for report in tags)
    large=json.loads((root/'managed-engine-large.json').read_text())
    assert large['bytes_per_file']>=1024**3 and large['completed_rounds']==2 and large['exact_bytes']
    assert large['completed_exchange_rounds']==2, 'both rounds must finish the real inbound result and sender acknowledgement before sampling'
    assert large['native_blob_connections_verified'] and large['thread_return_to_initial_verified']
    assert large['tasks_without_sampler_return_to_initial_verified']
    assert large['closed_after_idle']['fds']<=large['initial']['fds']
    assert large['source_max_chunk_bytes']<=128*1024
    assert large['peaks']['peak_rss_kib']-large['prepared']['peak_rss_kib']<512*1024
    schedulers=[]
    for path in root.glob('download-scheduler-*.jsonl'):
     records=[json.loads(line) for line in path.read_text().splitlines()]
     assert records and records[0]['event']=='created' and records[0]['slots']==0
     assert all(r['active']<=4 and r['peer_active_max']<=2 and r['queued']<=64 and r['peer_queued_max']<=16 for r in records)
     if any(r['slots'] for r in records):
      final=records[-1]
      assert final['event']=='settled' and final['closed'] and final['slots']==final['active']==final['queued']==0
      schedulers.append({'evidence':path.name,'peak_active':max(r['active'] for r in records),
       'peak_peer_active':max(r['peer_active_max'] for r in records),'final':final})
    assert any(r['peak_active']==4 and r['peak_peer_active']==2 for r in schedulers), 'actual provider C quota occupancy missing'
    (root/'summary.json').write_text(json.dumps({'completed_lifecycle_cases':len(EXPECTED_CASE_COUNTS),
     'lifecycle_report_count':len(reports),
     'fd_threads_and_runtime_tasks_verified':True,'download_quota_counters_verified':True,
     'sdk_tags_and_native_connection_counts_directly_observed':True,
     'sdk_store_tag_observations':tags,'large_file_engine':large,
     'download_schedulers':schedulers,'cases':reports},indent=2)+'\n')
    print('six native lifecycles and two 1GiB files: kernel resources, SDK tags and native handles, and download quotas passed')


if __name__ == "__main__":
    main()
