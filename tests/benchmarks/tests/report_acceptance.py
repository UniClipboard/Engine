#!/usr/bin/env python3
"""Exercise artifact rejection and control failure through the public executable."""
import argparse
import json
import pathlib
import shutil
import subprocess
import tempfile

p=argparse.ArgumentParser()
p.add_argument('--binary',required=True)
p.add_argument('--acceptance',required=True,type=pathlib.Path)
p.add_argument('--output',required=True,type=pathlib.Path)
a=p.parse_args()
a.output.mkdir(parents=True,exist_ok=False)
results=[]
valid=a.acceptance/'smoke'
subprocess.run([a.binary,'compare','--baseline',str(valid),'--candidate',str(valid),'--output',str(a.output/'aa')],check=True)
comparison=json.loads((a.output/'aa/comparison.json').read_text())
assert comparison['baseline']==comparison['candidate'] and not comparison['measurement_eligible']
results.append({'case':'aa-acceptance-comparison','passed':True})
for case in ['tampered-sample','missing-sample','missing-cleanup-receipt']:
    target=a.output/case
    shutil.copytree(valid,target)
    if case=='tampered-sample':
        with (target/'a.jsonl').open('a') as file: file.write('{}\n')
    elif case=='missing-sample': (target/'a.jsonl').unlink()
    else:
        receipt=next(target.glob('engine-benchmark-*/result.json'))
        receipt.unlink()
    checked=subprocess.run([a.binary,'verify','--output',str(target)],capture_output=True)
    assert checked.returncode!=0,case
    results.append({'case':case,'passed':True})
for case,payload in [('malformed-control','not-json'),('oversized-control','x'*65537)]:
    with tempfile.TemporaryDirectory(prefix='uc-benchmark-control-') as tmp:
        script=pathlib.Path(tmp)/'host'
        script.write_text('#!/usr/bin/env python3\nimport sys\nsys.stdin.readline()\nprint('+repr(payload)+',flush=True)\nsys.stdin.read()\n')
        script.chmod(0o700)
        output=a.output/case
        checked=subprocess.run([a.binary,'self-test','--host',str(script),'--output',str(output),'--case','smoke'],capture_output=True,timeout=60)
        assert checked.returncode!=0
        result=json.loads((output/'result.json').read_text())
        assert not result['passed'] and result['cleanup']=='completed'
        results.append({'case':case,'passed':True})
(a.output/'acceptance.json').write_text(json.dumps({'schema_version':1,'passed':True,'cases':results},indent=2))
