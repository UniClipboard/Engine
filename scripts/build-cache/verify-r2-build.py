#!/usr/bin/env python3
"""运行一次真实 R2 构建，保留安全诊断并拒绝未计入 MBX 统计的远端失败。"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


FAILURES = {
    "prefetch": "remote cache blob prefetch failed",
    "incomplete_prefetch": "remote action prefetch was incomplete",
    "lookup": "remote cache action lookup failed",
    "upload": "remote cache blob upload failed",
    "buffer_exhaustion": "No buffer space available",
}


def verify(report, failures, mode, consumer):
    problems = []
    for key in ("remote_failures", "background_upload_failures"):
        if report[key] != 0:
            problems.append(key)
    problems.extend(key for key, count in failures.items() if count)
    if mode == "read-only" and report["uploaded_bytes"] != 0:
        problems.append("read_only_uploaded")
    if consumer and not (report["downloaded_bytes"] > 0 and report["hits"] > 0):
        problems.append("consumer_did_not_restore")
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--consumer", action="store_true")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("missing build command")
    report_path = Path(os.environ["MBX_STATS_REPORT"])
    mode = os.environ["MBX_REMOTE_MODE"]
    # 不允许旧统计把本次未完成的构建误判为成功。
    report_path.unlink(missing_ok=True)
    failures = dict.fromkeys(FAILURES, 0)
    # 日志只流经 Actions 的 secret masking；工件只保存固定分类和计数，绝不保存原始端点或凭据。
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                          text=True, errors="replace", bufsize=1) as process:
        for line in process.stdout:
            sys.stdout.write(line)
            sys.stdout.flush()
            for key, marker in FAILURES.items():
                if marker in line:
                    failures[key] += 1
        returncode = process.wait()
    evidence = {"exit_code": returncode, "failures": failures}
    report_path.with_suffix(".diagnostics.json").write_text(json.dumps(evidence, indent=2) + "\n")
    if returncode:
        return returncode if returncode > 0 else 128 - returncode
    problems = verify(json.loads(report_path.read_text()), failures, mode, args.consumer)
    if problems:
        print("R2 verification failed: " + ", ".join(problems), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
