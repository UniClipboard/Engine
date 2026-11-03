#!/usr/bin/env python3
"""Exercise queued admission and real host failure with the real benchmark executable."""
import argparse
import json
import pathlib
import subprocess
import sys

p=argparse.ArgumentParser()
p.add_argument('--binary',required=True,type=pathlib.Path)
p.add_argument('--host',required=True,type=pathlib.Path)
p.add_argument('--output',required=True,type=pathlib.Path)
a=p.parse_args()
a.output.mkdir(parents=True,exist_ok=False)
wrapper=a.output/'harness'
wrapper.write_text('''#!/usr/bin/env python3
import json,os,pathlib,subprocess,sys
root=pathlib.Path(__file__).resolve().parent
config=json.loads((root/'fixture.json').read_text())
arguments=sys.argv[1:]
if arguments[0]=='environment':
    count_file=root/'probes.json'
    count=json.loads(count_file.read_text()) if count_file.exists() else 0
    count_file.write_text(json.dumps(count+1))
    if count<2:
        print(json.dumps({'blocking_workloads':1}))
        raise SystemExit(0)
if arguments[0]=='run' and config['crash']:
    host=arguments[arguments.index('--host')+1]
    output=arguments[arguments.index('--output')+1]
    subprocess.run([config['binary'],'self-test','--host',host,'--output',output,'--case','host-crash'],check=True)
    raise SystemExit(7)
os.execv(config['binary'],[config['binary'],*arguments])
''')
wrapper.chmod(0o700)
fixture=a.output/'fixture.json'
fixture.write_text(json.dumps({'binary':str(a.binary.resolve()),'crash':False}))
runner=pathlib.Path('scripts/testing/run-engine-benchmark.py').resolve()
def command(output):
    return [sys.executable,str(runner),'--harness',str(wrapper.resolve()),'--baseline-host',str(a.host.resolve()),'--reference-only','--scenarios','startup','--environment-wait-seconds','120','--output',str(output)]
subprocess.run(command(a.output/'queued'),check=True,timeout=240)
queued=json.loads((a.output/'queued/suite.json').read_text())
assert queued['passed'] and queued['complete'] and queued['reference_only']
assert len(queued['runs'])==7 and sum(not row['warmup'] for row in queued['runs'])==6
assert queued['environment_events'] and not queued['failures']
assert list(queued['summary']['startup'])==['a']
fixture.write_text(json.dumps({'binary':str(a.binary.resolve()),'crash':True}))
failed=subprocess.run(command(a.output/'host-failed'),timeout=240)
assert failed.returncode!=0
result=json.loads((a.output/'host-failed/suite.json').read_text())
assert not result['passed'] and not result['complete'] and len(result['runs'])==1
assert result['runs'][0]['exit_code']==7
actual=json.loads((a.output/'host-failed/startup-00-a/result.json').read_text())
assert not actual['passed'] and actual['cleanup']=='completed'
(a.output/'acceptance.json').write_text(json.dumps({'schema_version':1,'passed':True,'cases':['busy-admission-waits-without-dropping-runs','reference-only-six-real-startups','real-host-crash-stops-without-retry']},indent=2))
