"""报告覆盖必须与场景及次数匹配，重复项不能抵消缺失项。"""
import copy
import importlib.util
from pathlib import Path
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


if __name__ == "__main__":
    unittest.main()
