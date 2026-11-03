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
    command.write_text('#!/usr/bin/env python3\nprint(\'{"devices":{"test":[{"state":"Booted"}]}}\')\n')
    command.chmod(0o700)
    environment=dict(os.environ,PATH=directory+os.pathsep+os.environ['PATH'])
    result=subprocess.run([args.binary,'run','--host',args.host,'--scenario','startup','--output',str(args.output/'rejected-run')],env=environment,capture_output=True,timeout=30)
    assert result.returncode!=0 and not (args.output/'rejected-run').exists()
(args.output/'acceptance.json').write_text(json.dumps({'schema_version':1,'case':'booted-simulator-rejected-before-host-start','passed':True},indent=2))
