#!/usr/bin/env python3
"""Reject a booted simulator through the actual benchmark command before spawning hosts."""
import argparse
import json
import os
import pathlib
import subprocess
import tempfile

parser=argparse.ArgumentParser()
parser.add_argument('--binary',required=True)
parser.add_argument('--host',required=True)
parser.add_argument('--output',required=True,type=pathlib.Path)
args=parser.parse_args()
args.output.mkdir(parents=True,exist_ok=False)
with tempfile.TemporaryDirectory(prefix='uc-benchmark-preflight-') as directory:
    command=pathlib.Path(directory)/'xcrun'
    marker=pathlib.Path(directory)/'xcrun-invocation.json'
    command.write_text(
        '#!/usr/bin/env python3\n'
        'import json,os,pathlib,sys\n'
        'output={"devices":{"test":[{"state":"Booted"}]}}\n'
        'pathlib.Path(os.environ["UC_XCRUN_MARKER"]).write_text(json.dumps({"arguments":sys.argv[1:],"output":output}))\n'
        'print(json.dumps(output))\n')
    command.chmod(0o700)
    environment=dict(os.environ,PATH=directory+os.pathsep+os.environ['PATH'],UC_XCRUN_MARKER=str(marker))
    result=subprocess.run([args.binary,'run','--host',args.host,'--scenario','startup','--output',str(args.output/'rejected-run')],env=environment,capture_output=True,text=True,timeout=30)
    assert result.returncode!=0 and not (args.output/'rejected-run').exists()
    assert marker.exists(),'simulator environment check was not invoked'
    invocation=json.loads(marker.read_text())
    assert 'simctl' in invocation['arguments'] and 'booted' in invocation['arguments'],invocation['arguments']
    devices=invocation['output']['devices']
    assert any(device['state']=='Booted' for runtime in devices.values() for device in runtime)
(args.output/'acceptance.json').write_text(json.dumps({'schema_version':1,'case':'booted-simulator-rejected-before-host-start','passed':True},indent=2))
