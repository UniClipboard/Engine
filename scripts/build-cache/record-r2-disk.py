#!/usr/bin/env python3
"""记录 R2 验收的磁盘余量及保留输出大小，只输出固定标签和计数。"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess


def directory_kib(path):
    if not path.exists():
        return 0
    # GNU/BSD du 都支持 -sk；不保留文件路径或可能含路径的错误正文。
    try:
        result = subprocess.run(["du", "-sk", str(path)], capture_output=True,
                                text=True, timeout=30, check=True)
        return int(result.stdout.split()[0])
    except (OSError, subprocess.SubprocessError, ValueError, IndexError):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("before", "after"))
    args = parser.parse_args()
    temporary = Path(os.environ["RUNNER_TEMP"])
    workspace = Path(os.environ["GITHUB_WORKSPACE"])
    report = Path(os.environ["MBX_STATS_REPORT"])
    evidence = {"phase": args.phase, "filesystems": {}, "directory_kib": {}}
    # 两个位置可能共享同一文件系统，不能把容量相加。
    for label, path in (("temporary", temporary), ("workspace", workspace)):
        usage = shutil.disk_usage(path)
        evidence["filesystems"][label] = dict(zip(("total", "used", "free"), usage))
    for role in ("producer", "consumer"):
        evidence["directory_kib"][f"{role}_cache"] = directory_kib(
            temporary / f"mbx-r2-{role}-cache")
        evidence["directory_kib"][f"{role}_target"] = directory_kib(
            workspace / "target" / f"mbx-r2-{role}")
    # 缺失目录为 0；测量失败为 null，不能把未知大小当作空目录。
    payload = json.dumps(evidence, indent=2) + "\n"
    print(payload, end="")
    report.with_suffix(f".disk-{args.phase}.json").write_text(payload)


if __name__ == "__main__":
    main()
