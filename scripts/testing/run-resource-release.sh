#!/usr/bin/env bash
# Run non-graphical release scenarios and preserve each command's original status.
set -euo pipefail
cd "$(dirname "$0")/../.."
export UC_RELEASE_EVIDENCE="${UC_RELEASE_EVIDENCE:-$PWD/target/test-artifacts/resource-release/$(date -u +%Y%m%dT%H%M%SZ)-$$}"
python3 - <<'PY'
import datetime,hashlib,json,os,re,subprocess
from pathlib import Path
root=Path(os.environ['UC_RELEASE_EVIDENCE']).resolve()
root.mkdir(parents=True,exist_ok=True)
print('EVIDENCE',root,flush=True)
def git(*args): return subprocess.check_output(['git',*args],text=True).strip()
files=set(git('ls-files','crates/uc-engine','bindings/uc-engine-uniffi','crates/uc-observability-runtime','Cargo.lock').splitlines()+git('ls-files','--others','--exclude-standard').splitlines())
files.update(['third_party/iroh/Cargo.lock','scripts/testing/run-resource-release.sh','scripts/testing/resource-release-runner.test.py'])
provenance={'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'engine':git('rev-parse','HEAD'),'network':git('-C','third_party/iroh','rev-parse','HEAD'),'status':git('status','--porcelain'),'rustc':subprocess.check_output(['rustc','-Vv'],text=True),'sha256':{p:hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in sorted(files) if Path(p).is_file()}}
(root/'provenance.json').write_text(json.dumps(provenance,indent=2)+'\n')
commands=[('required-closer',['just','cargo','test','--locked','-p','uc-engine','--test','host_contract','required_closer_failure_retains_real_lock_for_retry','--','--nocapture']),('binding-resources',['just','cargo','test','--locked','-p','uc-engine-uniffi','--test','suspension_releases_files','--','--nocapture','--test-threads=1']),('controlled-owner',['just','cargo','test','--locked','-p','uc-engine-uniffi','--lib','runtime::release_tests','--','--nocapture','--test-threads=1'])]
results=[]
for name,cmd in commands:
    env=os.environ.copy(); env['UC_TEST_ARTIFACTS_DIR']=str(root/name)
    print('START',name,flush=True)
    with (root/(name+'.log')).open('w') as log:
        result=subprocess.run(cmd,env=env,stdout=log,stderr=subprocess.STDOUT)
    binaries={}
    evidence_errors=[]
    for executable in re.findall(r'Running[^\n]*\(([^)]+)\)',(root/(name+'.log')).read_text()):
        binary=Path(executable).resolve()
        if not binary.is_file():
            evidence_errors.append('Executable is unavailable: '+str(binary))
            continue
        digest=hashlib.sha256()
        with binary.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024*1024), b''):
                digest.update(chunk)
        binaries[str(binary)]=digest.hexdigest()
    if result.returncode == 0 and not binaries:
        evidence_errors.append('Successful scenario has no executable hashes')
    results.append({'name':name,'command':cmd,'exit_code':result.returncode,'executable_sha256':binaries,'evidence_errors':evidence_errors})
    (root/'result.json').write_text(json.dumps(results,indent=2)+'\n')
    print('END',name,result.returncode,flush=True)
    if evidence_errors:
        print('\n'.join(evidence_errors),flush=True)
    if result.returncode or evidence_errors:
        print((root/(name+'.log')).read_text()[-6000:],flush=True)
        raise SystemExit(result.returncode or 1)
PY
