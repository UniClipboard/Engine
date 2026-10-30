#!/usr/bin/env python3
"""校验原生生命周期原始报告，独立重放同一 CI 验收入口。"""
from collections import Counter
from pathlib import Path
import argparse
import json

PREFIX = "testing::host_adapter_contract::receive_retry::"
EXPECTED_CASE_COUNTS = {
    PREFIX + "lifecycle::real_engine_lock_waits_for_active_retry_export_and_preserves_saved_files": 1,
    PREFIX + "lifecycle::real_engine_revocation_during_retry_prevents_old_output_from_being_committed": 1,
    PREFIX + "lifecycle::space::real_engine_space_reset_seals_active_retry_before_switching_identity": 1,
    PREFIX + "lifecycle::space::real_engine_confirmed_local_removal_prevents_active_retry_from_publishing": 1,
    PREFIX + "real_engine_abandoned_deletion_stops_retry_and_removes_all_receive_state": 2,
    PREFIX + "save_failure::real_engine_shutdown_waits_for_failed_file_settlement_repair": 1,
    PREFIX + "backpressure::real_engine_terminal_capacity_overflow_does_not_block_cold_downloads_cancel_or_cleanup": 1,
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

def verify_large_report(large):
    assert large['bytes_per_file']>=1024**3 and large['completed_rounds']==2 and large['exact_bytes']
    assert large['completed_exchange_rounds']==2, 'both rounds must finish the real inbound result and sender acknowledgement before sampling'
    assert large['native_blob_connections_verified'] and large['thread_return_to_initial_verified']
    assert large['tasks_without_sampler_return_to_initial_verified']
    assert large['closed_after_idle']['fds']<=large['initial']['fds']
    assert large['source_max_chunk_bytes']<=128*1024
    assert large['peaks']['peak_rss_kib']-large['prepared']['peak_rss_kib']<512*1024
    progress=large['progress_rounds']
    assert len(progress)==2, 'both original file rounds require progress evidence'
    previous_updates=0
    for report in progress:
        assert report['queries']==12 and report['reconciliations']==14 and report['reconnects']==2
        assert report['exact_ack'] is True, 'actual exact terminal ACK missing'
        assert report['updates']>previous_updates, 'each large file must emit actual live updates'
        previous_updates=report['updates']
        samples=report['samples']; handles=report['native_handles']
        assert len(samples)==len(handles)==len(report['peaks'])==len(report['settlement_ms'])==3
        assert all(0<=elapsed<=20000 for elapsed in report['settlement_ms'])
        for index,(sample,peak) in enumerate(zip(samples,report['peaks'])):
            assert sample['runtime_tasks']<=samples[0]['runtime_tasks']+4
            assert sample['fds']<=samples[0]['fds']+4
            assert sample['threads']<=samples[0]['threads']+2
            assert sample['rss_kib']<=samples[0]['rss_kib']+64*1024
            assert all(peak[key]>=sample[key] for key in ('runtime_tasks','fds','threads','rss_kib'))
            assert handles[index][1]<=handles[0][1], 'closed progress handles kept growing'
            if index: assert handles[index][0]>=handles[index-1][0]+2, 'new native endpoints after close missing'
    first=progress[0]['samples'][0]
    for report in progress:
        for sample in report['samples']:
            assert sample['runtime_tasks']<=first['runtime_tasks']+4
            assert sample['fds']<=first['fds']+4
            assert sample['threads']<=first['threads']+2
            assert sample['rss_kib']<=first['rss_kib']+64*1024
    assert progress[1]['native_handles'][0][0]>=progress[0]['native_handles'][-1][0], 'native observations cannot reset between files'
    initial=large['progress_handles_initial']; closed=large['progress_handles_after_idle']
    assert closed[0]>=progress[-1]['native_handles'][-1][0], 'native observations cannot reset before shutdown'
    assert closed[0]>initial[0] and closed[1]==initial[1], 'actual progress native handles must return to baseline'


def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root',type=Path)
    parser.add_argument('--include-local-large',action='store_true',help='显式校验开发者本机双1GiB报告；缺失或不合格时失败')
    args=parser.parse_args(argv)
    root=args.root
    reports=[json.loads(path.read_text()) for path in root.glob('receive-lifecycle-*.json')]
    verify_lifecycle_reports(reports)
    tags=[json.loads(path.read_text()) for path in root.glob('sdk-tags-*.json')]
    assert len(tags)>=12, 'SDK store shutdown observations missing'
    assert all(report['store_open_during_count'] and report['sdk_temp_tags']==0 for report in tags)
    # CI默认不读取大文件报告；旧文件也不能被冒认为本轮运行成功。
    large=None
    if args.include_local_large:
        large=json.loads((root/'managed-engine-large.json').read_text())
        verify_large_report(large)
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
     'large_file_engine_status':'passed' if args.include_local_large else 'skipped',
     'large_file_engine_skip_reason':None if args.include_local_large else 'developer-local-only',
     'download_schedulers':schedulers,'cases':reports},indent=2)+'\n')
    print(f'{len(EXPECTED_CASE_COUNTS)} native lifecycle scenarios: kernel resources, SDK tags and native handles, and download quotas passed')
    print('developer-only two 1GiB files: '+('passed' if args.include_local_large else 'skipped'))


if __name__ == "__main__":
    main()
