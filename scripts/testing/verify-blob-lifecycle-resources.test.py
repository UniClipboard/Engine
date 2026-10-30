"""报告覆盖必须与场景及次数匹配，重复项不能抵消缺失项。"""
import copy
import importlib.util
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "lifecycle_resources", Path(__file__).with_name("verify-blob-lifecycle-resources.py")
)
validator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(validator)


def reports():
    return [
        dict(case=case, resource_settlement_verified=True,
             fd_return_to_initial_verified=True, thread_return_to_initial_verified=True,
             runtime_tasks_return_to_initial_verified=True,
             actual_native_blob_handles_observed=True,
             native_blob_handles_returned_to_initial=True)
        for case, count in validator.EXPECTED_CASE_COUNTS.items()
        for _ in range(count)
    ]


class LifecycleCoverage(unittest.TestCase):
    def test_all_scenarios_and_both_deletion_runs_pass(self):
        validator.verify_lifecycle_reports(reports())

    def test_missing_second_deletion_run_is_rejected(self):
        data = reports()
        deletion = validator.PREFIX + "real_engine_abandoned_deletion_stops_retry_and_removes_all_receive_state"
        index = next(index for index, report in enumerate(data) if report["case"] == deletion)
        del data[index]
        with self.assertRaises(AssertionError):
            validator.verify_lifecycle_reports(data)

    def test_duplicate_cannot_mask_missing_scenario_with_expected_report_count(self):
        data = reports()
        data[0] = copy.deepcopy(data[1])
        with self.assertRaises(AssertionError):
            validator.verify_lifecycle_reports(data)

    def test_unknown_scenario_cannot_replace_expected_scenario(self):
        data = reports()
        data[0]["case"] = "unexpected"
        with self.assertRaises(AssertionError):
            validator.verify_lifecycle_reports(data)

    def test_extra_duplicate_is_rejected(self):
        data = reports()
        data.append(copy.deepcopy(data[0]))
        with self.assertRaises(AssertionError):
            validator.verify_lifecycle_reports(data)

    def test_missing_capacity_scenario_is_rejected(self):
        capacity = validator.PREFIX + "backpressure::real_engine_terminal_capacity_overflow_does_not_block_cold_downloads_cancel_or_cleanup"
        data = [report for report in reports() if report["case"] != capacity]
        with self.assertRaises(AssertionError):
            validator.verify_lifecycle_reports(data)

    def test_every_report_must_preserve_resource_checks(self):
        for index, report in enumerate(reports()):
            for field in report.keys() - {"case"}:
                with self.subTest(case=report["case"], index=index, field=field):
                    data = reports()
                    data[index][field] = False
                    with self.assertRaises(AssertionError):
                        validator.verify_lifecycle_reports(data)


def large_report():
    sample=dict(runtime_tasks=40,fds=20,threads=12,rss_kib=1000)
    progress=[]
    for index in range(2):
        progress.append(dict(queries=12,reconciliations=14,reconnects=2,updates=(index+1)*10,
            exact_ack=True,samples=[dict(sample) for _ in range(3)],peaks=[dict(sample) for _ in range(3)],
            settlement_ms=[50,50,50],native_handles=[[index*6+4+burst*2,4] for burst in range(3)]))
    return dict(bytes_per_file=1024**3,completed_rounds=2,exact_bytes=True,completed_exchange_rounds=2,
        native_blob_connections_verified=True,thread_return_to_initial_verified=True,
        tasks_without_sampler_return_to_initial_verified=True,closed_after_idle=dict(fds=10),initial=dict(fds=10),
        source_max_chunk_bytes=128*1024,peaks=dict(peak_rss_kib=2000),prepared=dict(peak_rss_kib=1000),
        progress_rounds=progress,progress_handles_initial=[0,0],progress_handles_after_idle=[14,0])


class ProgressResources(unittest.TestCase):
    def test_complete_original_load_and_real_progress_resources_pass(self):
        validator.verify_large_report(large_report())

    def test_missing_progress_evidence_is_rejected(self):
        data=large_report(); del data['progress_rounds']
        with self.assertRaises(KeyError): validator.verify_large_report(data)

    def test_missing_query_reconnect_ack_or_live_update_is_rejected(self):
        for field,value in [('queries',0),('reconciliations',0),('reconnects',0),('exact_ack',False),('updates',0)]:
            with self.subTest(field=field):
                data=large_report();data['progress_rounds'][0][field]=value
                with self.assertRaises(AssertionError):validator.verify_large_report(data)

    def test_second_file_without_new_live_updates_is_rejected(self):
        data=large_report();data['progress_rounds'][1]['updates']=data['progress_rounds'][0]['updates']
        with self.assertRaises(AssertionError):validator.verify_large_report(data)

    def test_fake_reconnect_without_new_native_endpoints_is_rejected(self):
        data=large_report();data['progress_rounds'][0]['native_handles'][1][0]=4
        with self.assertRaises(AssertionError):validator.verify_large_report(data)

    def test_live_native_handle_growth_and_shutdown_leak_are_rejected(self):
        for shutdown in (False,True):
            data=large_report()
            if shutdown:data['progress_handles_after_idle'][1]=1
            else:data['progress_rounds'][0]['native_handles'][1][1]=5
            with self.assertRaises(AssertionError):validator.verify_large_report(data)

    def test_original_resource_thresholds_and_peak_observation_are_preserved(self):
        for field,excess in [('runtime_tasks',5),('fds',5),('threads',3),('rss_kib',64*1024+1)]:
            with self.subTest(field=field):
                data=large_report();data['progress_rounds'][0]['samples'][1][field]+=excess
                data['progress_rounds'][0]['peaks'][1][field]+=excess
                with self.assertRaises(AssertionError):validator.verify_large_report(data)
        data=large_report();data['progress_rounds'][0]['peaks'][0]['runtime_tasks']=0
        with self.assertRaises(AssertionError):validator.verify_large_report(data)

    def test_growth_between_file_rounds_cannot_reset_the_baseline(self):
        data=large_report()
        for sample in data['progress_rounds'][1]['samples']+data['progress_rounds'][1]['peaks']:
            sample['runtime_tasks']+=5
        with self.assertRaises(AssertionError):validator.verify_large_report(data)

    def test_native_counters_cannot_reset_before_shutdown(self):
        data=large_report();data['progress_handles_after_idle'][0]=1
        with self.assertRaises(AssertionError):validator.verify_large_report(data)

    def test_original_file_load_and_sampling_boundary_remain_required(self):
        for field,value in [('bytes_per_file',64*1024),('completed_rounds',1),('completed_exchange_rounds',1),('exact_bytes',False)]:
            with self.subTest(field=field):
                data=large_report();data[field]=value
                with self.assertRaises(AssertionError):validator.verify_large_report(data)


class ExecutionScope(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        for index, report in enumerate(reports()):
            self.write(f'receive-lifecycle-{index}.json', report)
        for index in range(12):
            self.write(f'sdk-tags-{index}.json', dict(store_open_during_count=True, sdk_temp_tags=0))
        events = [
            dict(event='created', slots=0, active=0, peer_active_max=0, queued=0, peer_queued_max=0),
            dict(event='active', slots=4, active=4, peer_active_max=2, queued=0, peer_queued_max=0),
            dict(event='settled', slots=0, active=0, peer_active_max=0, queued=0, peer_queued_max=0, closed=True),
        ]
        (self.root/'download-scheduler-test.jsonl').write_text(''.join(json.dumps(row)+'\n' for row in events))

    def write(self, name, value):
        (self.root/name).write_text(json.dumps(value))

    def verify(self, *args):
        with contextlib.redirect_stdout(io.StringIO()):
            validator.main([str(self.root), *args])
        return json.loads((self.root/'summary.json').read_text())

    def test_default_scope_records_large_case_as_skipped(self):
        result = self.verify()
        self.assertEqual(result['large_file_engine_status'], 'skipped')
        self.assertEqual(result['large_file_engine_skip_reason'], 'developer-local-only')
        self.assertIsNone(result['large_file_engine'])

    def test_default_scope_does_not_use_a_stale_large_report(self):
        (self.root/'managed-engine-large.json').write_text('stale invalid report')
        self.assertEqual(self.verify()['large_file_engine_status'], 'skipped')

    def test_explicit_local_scope_requires_a_real_report(self):
        with self.assertRaises(FileNotFoundError):
            self.verify('--include-local-large')

    def test_explicit_local_scope_preserves_large_resource_validation(self):
        self.write('managed-engine-large.json', large_report())
        self.assertEqual(self.verify('--include-local-large')['large_file_engine_status'], 'passed')
        data = large_report()
        data['completed_rounds'] = 1
        self.write('managed-engine-large.json', data)
        with self.assertRaises(AssertionError):
            self.verify('--include-local-large')

    def test_ci_cannot_enable_the_local_large_runner(self):
        script = Path(__file__).with_name('run-blob-lifecycle-resources.sh')
        result = subprocess.run(['bash', str(script), '--include-local-large'],
                                env={**os.environ, 'CI': 'true'}, capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('CI不得启用', result.stderr)


if __name__ == "__main__":
    unittest.main()
