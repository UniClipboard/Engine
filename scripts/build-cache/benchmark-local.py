#!/usr/bin/env python3
"""从仓库根目录对比 MBX 编译并行度；只创建专用输出，保存完整日志与 Cargo timings。"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import shutil
import signal
import statistics
import subprocess
import tempfile
import time
from threading import Event


def capture(command):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
        return {"command": command, "exit": result.returncode,
                "stdout": result.stdout, "stderr": result.stderr, "timed_out": False}
    except subprocess.TimeoutExpired:
        return {"command": command, "exit": 124, "stdout": "",
                "stderr": "工具信息采集超过 30 秒", "timed_out": True}


def execute_build(command, env, log, timeout, cancelled=None):
    # 单次构建拥有独立进程组；仅杀掉 MBX 父进程会留下仍在读取源码的 Cargo/rustc。
    process = subprocess.Popen(command, env=env, stdout=log, stderr=log,
                               start_new_session=True)

    def group_active():
        listing = subprocess.run(["ps", "-axo", "pgid=,stat="], capture_output=True,
                                 text=True, check=True, timeout=30)
        return any(parts[0] == str(process.pid) and not parts[1].startswith("Z")
                   for line in listing.stdout.splitlines() if len(parts := line.split()) == 2)

    def stop():
        try:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                pass
            # macOS 对仅剩僵尸进程的组发信号可能返回 EPERM；不把它当作活跃子进程。
            if group_active():
                os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except PermissionError:
            if group_active():
                raise
        process.wait()

    deadline = time.monotonic() + timeout
    try:
        while True:
            remaining = deadline - time.monotonic()
            timed_out = remaining <= 0
            if timed_out or (cancelled is not None and cancelled.is_set()):
                stop()
                log.write("构建超时，已停止本次进程组\n" if timed_out else "构建取消，已停止本次进程组\n")
                return {"exit": 124 if timed_out else 130, "timed_out": timed_out}
            try:
                return {"exit": process.wait(timeout=min(remaining, 0.2)), "timed_out": False}
            except subprocess.TimeoutExpired:
                continue
    except BaseException:
        stop()
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scratch", type=Path, required=True,
                        help="已存在的外置可再生目录；每次在其中创建独立子目录")
    parser.add_argument("--output", type=Path, required=True, help="新建且不存在的日志目录")
    parser.add_argument("--jobs", type=int, nargs="+", default=[2, 4])
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--timeout", type=int, default=3600, help="单次构建超时秒数")
    args = parser.parse_args()
    root = Path.cwd()
    if not (root / "crates/uc-engine/Cargo.toml").is_file():
        parser.error("请从 Engine 仓库根目录运行")
    if args.timeout < 1 or args.repeats < 1 or any(j < 1 for j in args.jobs) or len(set(args.jobs)) != len(args.jobs):
        parser.error("jobs 必须为不重复的正整数，repeats 与 timeout 必须大于零")
    scratch = args.scratch.resolve(strict=True)
    if not scratch.is_dir() or scratch == root or root in scratch.parents:
        parser.error("scratch 必须为仓库外的可再生目录")
    if any(scratch == p or p in scratch.parents for p in
           (Path('/tmp'), Path('/private/tmp'), Path(tempfile.gettempdir()).resolve())):
        parser.error("scratch 不得放在系统临时目录")
    # 不覆盖旧证据，也不删除调用方提供的目录。
    args.output.mkdir(parents=True, exist_ok=False)
    output = args.output.resolve()
    run_root = Path(tempfile.mkdtemp(prefix="engine-build-", dir=scratch))
    env = dict(os.environ)
    env.pop("CARGO_BUILD_BUILD_DIR", None)
    commands = [["git", "rev-parse", "HEAD"], ["git", "status", "--porcelain"],
                ["rustc", "-Vv"], ["cargo", "-V"], ["uname", "-a"]]
    if os.uname().sysname == "Darwin":
        commands += [["sysctl", "-n", "machdep.cpu.brand_string", "hw.memsize", "hw.ncpu"],
                     ["xcrun", "ld", "-v"]]
    metadata = {"tools": [capture(c) for c in commands], "scratch": str(run_root),
                "environment": {k: v for k, v in env.items()
                                if k in ("CARGO_INCREMENTAL", "CARGO_BUILD_JOBS", "CARGO_TARGET_DIR",
                                         "CARGO_BUILD_BUILD_DIR", "RUSTC_WRAPPER", "RUSTFLAGS",
                                         "MBX_CACHE_DIR", "MBX_TARGET_VIEWS", "MBX_TARGET_SEED",
                                         "MBX_LEARNED_INCREMENTAL", "UC_ENGINE_SOURCE_COMMIT",
                                         "UC_ENGINE_SOURCE_STATE")},
                "cache_state": "空 target/MBX 动作缓存；下载内容与 OS 页面缓存未清空",
                "scope": "workspace all-targets、默认 features 统一、dev check、本机 target"}
    (output / "environment.json").write_text(json.dumps(metadata, indent=2, ensure_ascii=False))
    command = ["bash", "scripts/build-cache/mbx.sh", "check", "--workspace",
               "--all-targets", "--locked", "--timings"]
    source = root / "crates/uc-core/src/lib.rs"
    original = source.read_bytes()
    results = []

    def run(label, jobs, target, cache):
        build_env = dict(env, CARGO_TARGET_DIR=str(target), MBX_CACHE_DIR=str(cache),
                         CARGO_BUILD_JOBS=str(jobs))
        start = time.monotonic()
        measured = command
        if os.uname().sysname == "Darwin":
            measured = ["/usr/bin/time", "-l", *command]
        with (output / (label + ".log")).open("w") as log:
            result = execute_build(measured, build_env, log, args.timeout)
        results.append({"label": label, "jobs": jobs, "seconds": time.monotonic() - start,
                        **result, "command": command,
                        "target": str(target), "cache": str(cache)})
        (output / "results.json").write_text(json.dumps(results, indent=2))
        timings = target / "cargo-timings"
        if timings.exists():
            shutil.copytree(timings, output / (label + "-timings"))
        print(json.dumps(results[-1]), flush=True)
        if result["exit"]:
            raise RuntimeError(f"构建失败，完整日志：{output / (label + '.log')}")

    def lock_pair(jobs, target, cache):
        build_env = dict(env, CARGO_TARGET_DIR=str(target), MBX_CACHE_DIR=str(cache),
                         CARGO_BUILD_JOBS=str(jobs))

        cancelled = Event()

        def worker(index):
            label = f"j{jobs}-lock-{index}"
            start = time.monotonic()
            with (output / (label + ".log")).open("w") as log:
                result = execute_build(command, build_env, log, args.timeout, cancelled)
            waits = [line for line in (output / (label + ".log")).read_text().splitlines()
                     if "Blocking waiting for file lock" in line]
            return {"label": label, "jobs": jobs, "seconds": time.monotonic() - start,
                    **result, "lock_wait_observed": bool(waits), "lock_wait_messages": waits,
                    "command": command,
                    "target": str(target), "cache": str(cache)}

        with ThreadPoolExecutor(max_workers=2) as pool:
            futures = [pool.submit(worker, index) for index in range(2)]
            try:
                pair = [future.result() for future in futures]
            except BaseException:
                cancelled.set()
                raise
        results.extend(pair)
        (output / "results.json").write_text(json.dumps(results, indent=2))
        if any(row["exit"] for row in pair):
            raise RuntimeError("并行锁竞争构建失败")

    try:
        for jobs in args.jobs:
            target, cache = run_root / f"jobs-{jobs}-target", run_root / f"jobs-{jobs}-mbx"
            target.mkdir()
            cache.mkdir()
            run(f"j{jobs}-cold", jobs, target, cache)
            for index in range(args.repeats):
                run(f"j{jobs}-hot-{index}", jobs, target, cache)
            for index in range(args.repeats):
                source.write_bytes(original + f"\n// 构建基准注释 {jobs}/{index}\n".encode())
                run(f"j{jobs}-edit-{index}", jobs, target, cache)
            source.write_bytes(original)
            run(f"j{jobs}-restore", jobs, target, cache)
            source.write_bytes(original + f"\n// 构建基准锁竞争 {jobs}\n".encode())
            lock_pair(jobs, target, cache)
            source.write_bytes(original)
    finally:
        source.write_bytes(original)
        rows = ["# 本地构建基准", "", "| 并行度 | 冷构建秒 | 热构建中位数秒 | 注释微改中位数秒 |",
                "| --- | --- | --- | --- |"]
        for count in args.jobs:
            cold = [r["seconds"] for r in results if r["label"] == f"j{count}-cold" and not r["exit"]]
            hot = [r["seconds"] for r in results if r["label"].startswith(f"j{count}-hot-") and not r["exit"]]
            edit = [r["seconds"] for r in results if r["label"].startswith(f"j{count}-edit-") and not r["exit"]]
            values = [f"{statistics.median(v):.3f}" if v else "待测" for v in (cold, hot, edit)]
            rows.append(f"| {count} | " + " | ".join(values) + " |")
        rows += ["", "| 锁竞争样本 | 退出码 | 超时 | 实测锁等待 |", "| --- | --- | --- | --- |"]
        for result in results:
            if "lock_wait_observed" in result:
                rows.append(f"| {result['label']} | {result['exit']} | {result['timed_out']} | {result['lock_wait_observed']} |")
        rows += ["", "锁等待匹配原文见 results.json 与 *-lock-*.log；未观测到等待时不能据标签宣称竞争。",
                 "失败/超时结果不计入耗时中位数；不能当作独立 worker 吞吐。"]
        (output / "summary.md").write_text("\n".join(rows) + "\n")
        print(f"源码已恢复；专用构建目录：{run_root}。确认无活动构建进程后回收。", flush=True)


if __name__ == "__main__":
    main()
