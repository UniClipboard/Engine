#!/usr/bin/env python3
"""Replay or run an Engine idle E2E and flag large within-window memory changes."""
import argparse
import json
import pathlib
import statistics
import subprocess

parser=argparse.ArgumentParser()
parser.add_argument('--binary',required=True)
parser.add_argument('--host',required=True)
parser.add_argument('--output',required=True,type=pathlib.Path)
parser.add_argument('--existing',type=pathlib.Path)
args=parser.parse_args()
args.output.mkdir(parents=True,exist_ok=False)
run=args.existing
if run is None:
    run=args.output/'run'
    subprocess.run([args.binary,'self-test','--host',args.host,'--output',str(run),'--case','disconnected'],check=True,timeout=240)
subprocess.run([args.binary,'verify','--output',str(run)],check=True)
work=json.loads((run/'work.json').read_text())
metrics=json.loads((run/'metrics.json').read_text())
windows={item['phase']:item['monotonic_ns'] for item in work['windows']}
endpoints={}
for endpoint in ['a','b']:
    samples=[json.loads(line) for line in (run/f'{endpoint}.jsonl').read_text().splitlines()]
    idle=[sample for sample in samples if windows['work_start']<=sample['monotonic_ns']<=windows['work_end']]
    assert len(idle)>=2
    resident=[sample['resident_bytes'] for sample in idle]
    assert metrics[endpoint]['phases']['work_start']['median_rss']==statistics.median(resident), 'reported median differs from independent calculation'
    endpoints[endpoint]={'samples':len(idle),'idle_median_bytes':statistics.median(resident),'idle_min_bytes':min(resident),'idle_max_bytes':max(resident),'idle_span_bytes':max(resident)-min(resident),'first_bytes':resident[0],'last_bytes':resident[-1]}
# 仅用于复现已观察到的大幅波动，不是资源预算或 CI 性能门禁。
stable=all(value['idle_span_bytes']<64*1024**2 for value in endpoints.values())
report={'schema_version':1,'stable_window':stable,'diagnostic_only':True,'endpoints':endpoints}
(args.output/'diagnostic.json').write_text(json.dumps(report,indent=2))
print(json.dumps(report))
raise SystemExit(0 if stable else 1)
