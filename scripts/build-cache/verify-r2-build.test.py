#!/usr/bin/env python3
"""固定 MBX 1.18.0 的预取漏计回归，并验证工件不保留原始错误正文。"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("verify-r2-build.py")


class R2VerificationTests(unittest.TestCase):
    def run_build(self, warning="", code=0, consumer=False, **changes):
        report = dict(remote_failures=0, background_upload_failures=0,
                      uploaded_bytes=0, downloaded_bytes=123, hits=1)
        report.update(changes)
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "producer.json"
            # 预置一份通过的报告，证明入口不会复用上一轮统计。
            path.write_text(json.dumps(report))
            command = [sys.executable, str(SCRIPT)]
            if consumer:
                command.append("--consumer")
            command.extend(["--", sys.executable, "-c",
                            "import json, os, sys; from pathlib import Path; "
                            f"print({warning!r}); "
                            + (f"Path(os.environ['MBX_STATS_REPORT']).write_text({json.dumps(report)!r}); "
                               if code == 0 else "")
                            + f"sys.exit({code})"])
            result = subprocess.run(command, env={**os.environ, "MBX_STATS_REPORT": str(path),
                                                  "MBX_REMOTE_MODE": "read-only"},
                                    capture_output=True, text=True)
            evidence = path.with_suffix(".diagnostics.json").read_text()
            return result.returncode, json.loads(evidence), evidence

    def test_prefetch_error_with_zero_native_failures_is_rejected(self):
        code, evidence, raw = self.run_build(
            "[WARN ] remote cache blob prefetch failed for digest: error sending request for url (https://private.invalid/token)")
        self.assertEqual(code, 1)
        self.assertEqual(evidence["failures"]["prefetch"], 1)
        self.assertNotIn("private.invalid", raw)
        self.assertNotIn("token", raw)

    def test_incomplete_prefetch_and_enobufs_are_rejected(self):
        for warning in ("remote action prefetch was incomplete", "No buffer space available (os error 55)"):
            with self.subTest(warning=warning):
                self.assertEqual(self.run_build(warning)[0], 1)

    def test_existing_native_failure_checks_remain(self):
        for key in ("remote_failures", "background_upload_failures", "uploaded_bytes"):
            with self.subTest(key=key):
                self.assertEqual(self.run_build(**{key: 1})[0], 1)

    def test_consumer_requires_downloads_and_hits(self):
        self.assertEqual(self.run_build(consumer=True)[0], 0)
        for key in ("downloaded_bytes", "hits"):
            with self.subTest(key=key):
                self.assertEqual(self.run_build(consumer=True, **{key: 0})[0], 1)

    def test_build_failure_preserves_exit_code_despite_stale_report(self):
        code, evidence, _ = self.run_build(code=7)
        self.assertEqual(code, 7)
        self.assertEqual(evidence["exit_code"], 7)

    def test_successful_build(self):
        self.assertEqual(self.run_build()[0], 0)


if __name__ == "__main__":
    unittest.main()
