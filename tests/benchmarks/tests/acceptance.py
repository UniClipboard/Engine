#!/usr/bin/env python3
"""Process-level acceptance; written before implementation. No mock Engine success."""
import argparse
import json
import os
import signal
import time
import pathlib
import subprocess

p = argparse.ArgumentParser()
p.add_argument('--binary', required=True)
p.add_argument('--host', required=True)
p.add_argument('--output', required=True)
a = p.parse_args()
root = pathlib.Path(a.output)
root.mkdir(parents=True, exist_ok=False)
results = []
for case in ['smoke', 'timeout', 'host-crash', 'collector-crash', 'controller-interrupt', 'startup', 'disconnected', 'connected', 'full-text']:
    out = root / case
    run = subprocess.run([a.binary, 'self-test', '--host', a.host, '--output', str(out), '--case', case], timeout=180)
    result = json.loads((out / 'acceptance.json').read_text())
    assert run.returncode == 0 and result['passed'], case
    run_result = json.loads((out / 'result.json').read_text())
    if case in ['smoke','full-text']:
        assert run_result['work']['verified_entries'] == (2 if case == 'smoke' else 200)
        assert run_result['work']['restart_verified']
        streams=[]
        for endpoint in ['a','b']:
            samples=[json.loads(line) for line in (out/f'{endpoint}.jsonl').read_text().splitlines()]
            assert len(samples)>=2 and all(row['resident_bytes']>0 for row in samples)
            assert all(y['cpu_time_ns']>=x['cpu_time_ns'] for x,y in zip(samples,samples[1:]))
            streams.append(samples)
        assert streams[0][0]['process_start_abstime'] != streams[1][0]['process_start_abstime']
        subprocess.run([a.binary,'verify','--output',str(out)],check=True)
    if case == 'host-crash': assert run_result['work']['failure'] == 'injected host crash'
    if case == 'collector-crash': assert run_result['work']['failure'] == 'collector failed'
    if case in ['timeout','controller-interrupt']: assert run_result['supervisor_outcome'] == case
    results.append(result)
# 整个监督进程被强杀后，控制器通过管道 EOF 结束自身进程组；下次运行回收私有租约。
killed=root/'supervisor-killed'
process=subprocess.Popen([a.binary,'self-test','--host',a.host,'--output',str(killed),'--case','full-text'])
try:
    deadline=time.monotonic()+30
    while not (killed/'b.jsonl').exists():
        assert process.poll() is None and time.monotonic()<deadline
        time.sleep(.05)
    process.kill()
    process.wait(timeout=10)
finally:
    if process.poll() is None:
        process.kill()
        process.wait(timeout=10)
time.sleep(1)
recovery=root/'stale-recovery'
subprocess.run([a.binary,'self-test','--host',a.host,'--output',str(recovery),'--case','smoke'],check=True,timeout=180)
assert json.loads((recovery/'result.json').read_text())['recovered_stale_lease']
results.append({'case':'supervisor-kill-and-stale-recovery','passed':True})
(root / 'acceptance.json').write_text(json.dumps({'schema_version': 1, 'cases': results, 'passed': True}, indent=2))
