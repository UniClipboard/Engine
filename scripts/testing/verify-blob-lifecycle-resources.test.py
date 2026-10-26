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
    def test_all_six_scenarios_and_both_deletion_runs_pass(self):
        validator.verify_lifecycle_reports(reports())

    def test_missing_second_deletion_run_is_rejected(self):
        data = reports()
        del data[-2]
        with self.assertRaises(AssertionError):
            validator.verify_lifecycle_reports(data)

    def test_duplicate_cannot_mask_missing_scenario_with_six_reports(self):
        data = reports()[1:]
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

    def test_every_deletion_report_must_preserve_resource_checks(self):
        for field in reports()[0].keys() - {"case"}:
            with self.subTest(field=field):
                data = reports()
                data[-2][field] = False
                with self.assertRaises(AssertionError):
                    validator.verify_lifecycle_reports(data)


if __name__ == "__main__":
    unittest.main()
