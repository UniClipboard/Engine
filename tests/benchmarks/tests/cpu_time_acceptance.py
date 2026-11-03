#!/usr/bin/env python3
"""Compare the real OS collector with an independent child CPU clock."""
import argparse
import json
import pathlib
import resource
import subprocess
import sys
import time

if sys.argv[1:] == ['--cpu-child']:
    print('ready', flush=True)
    sys.stdin.readline()
    before = resource.getrusage(resource.RUSAGE_SELF)
    deadline = time.process_time() + 1
    while time.process_time() < deadline:
        pass
    after = resource.getrusage(resource.RUSAGE_SELF)
    print(json.dumps({'cpu_ns': int(((after.ru_utime + after.ru_stime) - (before.ru_utime + before.ru_stime)) * 1e9)}), flush=True)
    sys.stdin.readline()
    sys.exit(0)

parser = argparse.ArgumentParser()
parser.add_argument('--binary', required=True, type=pathlib.Path)
parser.add_argument('--output', required=True, type=pathlib.Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
child = subprocess.Popen([sys.executable, __file__, '--cpu-child'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
collector = None
try:
    assert child.stdout.readline().strip() == 'ready'
    samples = args.output / 'cpu.jsonl'
    collector = subprocess.Popen([str(args.binary.resolve()), 'collect', '--pid', str(child.pid), '--interval-ms', '50', '--output', str(samples.resolve())], stdin=subprocess.PIPE)
    time.sleep(0.3)
    child.stdin.write('run\n')
    child.stdin.flush()
    independent = json.loads(child.stdout.readline())['cpu_ns']
    time.sleep(0.3)
    collector.stdin.close()
    assert collector.wait(timeout=10) == 0
    rows = [json.loads(line) for line in samples.read_text().splitlines()]
    observed = rows[-1]['cpu_time_ns'] - rows[0]['cpu_time_ns']
    error = abs(observed - independent) / independent
    passed = len(rows) >= 10 and independent >= 900_000_000 and error < 0.15
    (args.output / 'acceptance.json').write_text(json.dumps({'passed': passed, 'independent_cpu_ns': independent, 'collector_cpu_ns': observed, 'relative_error': error, 'sample_count': len(rows)}, indent=2))
    print(json.dumps({'passed': passed, 'relative_error': error}))
finally:
    child.stdin.close()
    child.wait(timeout=10)
    if collector is not None and collector.poll() is None:
        collector.kill()
        collector.wait()
sys.exit(0 if passed else 1)
