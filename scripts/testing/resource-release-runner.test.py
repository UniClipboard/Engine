#!/usr/bin/env python3
"""Exercise the complete evidence runner with controlled Cargo output."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

repo = Path(__file__).resolve().parents[2]
base = repo / 'target/test-artifacts'
base.mkdir(parents=True, exist_ok=True)
results = []
interpreters = [sys.executable]
older = Path('/usr/bin/python3')
if older.is_file() and older.resolve() != Path(sys.executable).resolve():
    interpreters.append(str(older))
with tempfile.TemporaryDirectory(prefix='release-runner-', dir=base) as work:
    work = Path(work)
    binary = work / 'scenario-binary'
    binary.write_bytes(b'controlled executable provenance fixture\n')
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    for index, interpreter in enumerate(interpreters):
        for mode in ['relative', 'absolute', 'missing']:
            case = work / ('case-%s-%s' % (index, mode))
            case.mkdir()
            (case / 'python3').symlink_to(interpreter)
            executable = str(binary.relative_to(repo)) if mode == 'relative' else str(binary)
            if mode == 'missing':
                executable += '-missing'
            (case / 'just').write_text('#!' + sys.executable + '\nprint(' + repr('Running tests (' + executable + ')') + ')\n')
            (case / 'just').chmod(0o755)
            env = dict(os.environ, PATH=str(case) + os.pathsep + os.environ['PATH'],
                       UC_RELEASE_EVIDENCE=str(case / 'evidence'))
            run = subprocess.run(['bash', 'scripts/testing/run-resource-release.sh'],
                                 cwd=repo, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            result_file = case / 'evidence/result.json'
            records = json.loads(result_file.read_text()) if result_file.exists() else []
            if mode == 'missing':
                passed = run.returncode != 0
            else:
                passed = run.returncode == 0 and len(records) == 3 and all(
                    r['exit_code'] == 0 and r['executable_sha256'] == {str(binary.resolve()): digest}
                    for r in records)
            results.append({'interpreter': interpreter, 'mode': mode, 'passed': passed,
                            'exit_code': run.returncode, 'records': records, 'output': run.stdout})
output = Path(os.environ.get('UC_RUNNER_TEST_RESULT', str(base / 'resource-release-runner-test.json')))
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text(json.dumps(results, indent=2) + '\n')
print(str(output))
for result in results:
    print(result['interpreter'], result['mode'], 'PASS' if result['passed'] else 'FAIL')
sys.exit(0 if all(r['passed'] for r in results) else 1)
